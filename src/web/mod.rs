//! 手机端第 0 期：局域网上的一个极小 HTTP 服务。
//!
//! **它只做三件事**：监听、认 token、把请求交给一个 `Handler`。真正的路由
//! （会话列表、画面）在 `routes.rs`，网页在 `page.html`——这一层对它们一无所知，
//! 这样才测得动：测试塞一个假 `Handler` 进来，不用起守护进程。
//!
//! ## 为什么自己写 HTTP，不引框架
//!
//! 需要的是五个路由加一个静态页面。axum/hyper 会把 tokio 整个拖进 `dct` 的
//! 依赖树，而守护进程和 TUI 明确不要 async 运行时（见
//! `docs/superpowers/plans/2026-08-10-dct-phone-channel.md` 的约束）。这跟仓库
//! 自己写 `sys::ipc`、自己写线协议是同一个取舍。
//!
//! **代价是必须把支持的子集写清楚，并且拒绝子集之外的东西，而不是猜**：
//!
//! - 只认 HTTP/1.1 的请求行 + 头，头总长不超过 [`MAX_HEADER_BYTES`]
//! - 请求体只认 `Content-Length`，**不支持 chunked**——收到 `Transfer-Encoding`
//!   直接 400，绝不当成"没有 body"往下走（那会把半个请求当成完整请求处理）
//! - 响应恒为 `Connection: close`，一个连接一个请求，不做 keep-alive
//!
//! 浏览器全都接受这个子集。fetch 发的就是 `Content-Length` 的请求。
//!
//! ## 这一层的安全边界
//!
//! 局域网模式下数据不出你的网络，所以没有加密（见 spec 决定二：加密是走公网
//! 那一期的事）。但暴露面不是零——**同一个 WiFi 上的任何人只要拿到 token 就能
//! 往你的终端里敲字**。所以：
//!
//! - 默认不监听，只有用户在设置页里打开才起（`daemon.rs`）
//! - token 是 32 字节的系统随机数，存在密钥仓里
//! - **认证在路由之前**：没带对 token 的请求一律 401，连"这个路径存不存在"
//!   都不告诉他。否则局域网里的人可以拿 404/401 的差别把接口摸个遍。
//! - 比对用常数时间循环，不用 `==`

pub mod keys;
pub mod routes;
pub mod strings;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 请求行 + 全部头的上限。超了直接拒——不是为了省内存，是因为一个不设限的
/// 读循环等于让局域网里的任何人用一条永不结束的头把内存吃光。
const MAX_HEADER_BYTES: usize = 8 * 1024;

/// 请求体上限。手机端最大的一次请求是用户打的一段字，离这个数远得很。
const MAX_BODY_BYTES: usize = 64 * 1024;

/// 单条连接的读写超时。没有它，一个连上来就不说话的客户端能永久占住一个线程。
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// 同时在处理的连接数上限。超了立刻回 503 并关掉——线程一个连接一个，
/// 不设上限的话，局域网里一个循环连接的脚本就能把线程开爆。
const MAX_INFLIGHT: usize = 32;

/// token 的字节数。32 字节 = 256 位，够到"猜不出来"这件事不用再想。
const TOKEN_BYTES: usize = 32;

/// 一个进来的请求。**借用的，不拷贝**——`path`/`query` 指向同一段缓冲。
pub struct Req<'a> {
    pub method: &'a str,
    /// 不含查询串，已经去掉前后空白。**没有做百分号解码**：这一期的路径全是
    /// ASCII 字面量，解码要等真有需要解码的路径时再加，现在加等于加一处
    /// 没人验证过的解析。
    pub path: &'a str,
    /// `?` 后面那一段，没有就是空串。
    pub query: &'a str,
    pub body: &'a [u8],
}

/// 一个要发回去的响应。
pub struct Resp {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(body: impl Into<Vec<u8>>) -> Resp {
        Resp {
            status: 200,
            content_type: "application/json; charset=utf-8",
            body: body.into(),
        }
    }

    pub fn html(body: impl Into<Vec<u8>>) -> Resp {
        Resp {
            status: 200,
            content_type: "text/html; charset=utf-8",
            body: body.into(),
        }
    }

    /// 只有状态码的响应。**body 恒为空**：错误的细节一个字都不往局域网上写，
    /// 那些话属于用户面前那块屏幕（网页自己知道 401 该说什么），不属于
    /// 任何一个能连到这个端口的人。
    pub fn status(status: u16) -> Resp {
        Resp {
            status,
            content_type: "text/plain; charset=utf-8",
            body: Vec::new(),
        }
    }
}

/// 路由。`web` 这一层只认这个 trait——生产环境是 `routes.rs` 里那个真的会去问
/// 守护进程的实现，测试里是一个记下"被调用过什么"的假实现。
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, req: &Req) -> Resp;
}

impl<F> Handler for F
where
    F: Fn(&Req) -> Resp + Send + Sync + 'static,
{
    fn handle(&self, req: &Req) -> Resp {
        self(req)
    }
}

/// 起一个新 token。**只在用户第一次打开手机端时调一次**，之后存在密钥仓里。
///
/// 换 token = 已经配过对的手机全部失效，所以不要在启动时无条件重新生成——
/// 那会让"扫过一次码"这件事每次重启都作废。
pub fn new_token() -> anyhow::Result<String> {
    let mut raw = [0u8; TOKEN_BYTES];
    getrandom::getrandom(&mut raw)
        .map_err(|e| anyhow::anyhow!("系统随机数不可用，没法生成手机端令牌：{e}"))?;
    // 十六进制而不是 base64：token 要塞进二维码，也可能被人肉眼核对，
    // 而 base64 里的 `+/=` 在 URL 和二维码字符集里都要额外操心。
    Ok(raw.iter().map(|b| format!("{b:02x}")).collect())
}

/// 拿到这台机器的手机端令牌：有就用，没有才生成一个并存下来。
///
/// **是 `ensure` 不是 `new`**，这一点是这个函数存在的全部理由。每次启动重新
/// 生成的话，凡是扫过码的手机在你下一次开 dct 之后全部失效——而用户对此毫无
/// 感知，他只会看到手机上突然要重新扫码，且不知道为什么。令牌换代是一个**用户
/// 主动做的动作**（撤销设备），不该是重启的副作用。
///
/// 密钥仓读不了的时候（文件损坏、权限不对）`set` 会拒绝写入，这里如实往上报——
/// 不静默退回一个只活在内存里的令牌：那样手机能连上，但重启之后又连不上了，
/// 而中间没有任何一句话解释发生了什么。
pub fn ensure_token(store: &mut crate::secrets::SecretStore) -> anyhow::Result<String> {
    if let Some(t) = store.get(crate::secrets::WEB_TOKEN_KEY) {
        return Ok(t.to_string());
    }
    let t = new_token()?;
    store.set(crate::secrets::WEB_TOKEN_KEY, &t)?;
    Ok(t)
}

/// 这台机器在局域网上的地址。
///
/// 做法是**问路由表**：开一个 UDP socket「连」到一个外网地址上，然后读它的
/// 本地地址。UDP 的 `connect` 不发任何包（连对面在不在都不知道），它只是让
/// 内核挑一条路由，而挑出来那条路的源地址正是「这台机器在局域网上叫什么」。
///
/// 为什么不枚举网卡：标准库没有这个能力，要么加一个 crate，要么两套平台
/// 代码。而枚举出来还要挑——一台机器上常年挂着 WSL、Docker、VPN 的虚拟网卡，
/// 挑错一个，手机上就是连不上，且用户完全不知道为什么。路由表挑的那一个，
/// 正是这台机器"往外走"用的那一个，也就是手机最可能够得着的那一个。
///
/// **一个包都不会发出去**，所以断网时它也能立刻返回（拿到的多半是 `None`，
/// 那时候 `WebInfo::address_unknown` 为真，界面负责说人话）。
pub fn lan_ip() -> Option<std::net::IpAddr> {
    // 8.8.8.8 只是一个"肯定不在本机"的地址，不会被访问到。
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect("8.8.8.8:80").ok()?;
    let addr = probe.local_addr().ok()?.ip();
    // 回环地址对手机没用——那是"只有这台机器自己能连"的意思。
    if addr.is_loopback() || addr.is_unspecified() {
        return None;
    }
    Some(addr)
}

/// 跑着的服务。**丢掉它不会停掉服务**——要停得显式 `stop()`，理由见那个方法。
pub struct Server {
    addr: std::net::SocketAddr,
    stopping: Arc<AtomicBool>,
    accept: Option<std::thread::JoinHandle<()>>,
    /// accept 线程**把监听套接字丢掉之后**才置起来的标志。
    ///
    /// 存在的理由只有一个：让「端口真的关了」这件事在进程内可观测。
    /// 从外面观测是做不稳的——端口是绑 `:0` 让系统挑的，`stop()` 之后再去
    /// 连一次那个地址，连上了也不能说明什么，因为并行跑的别的测试完全
    /// 可能已经绑到这个刚被释放的端口上了（这条测试以前就是那么写的，
    /// 大约每六次全量跑红一次，单跑却永远是绿的）。
    ///
    /// 只有测试读它，所以非测试构建下"没人读"是对的，不是漏了什么。
    #[cfg_attr(not(test), allow(dead_code))]
    closed: Arc<AtomicBool>,
}

impl Server {
    /// 真实监听地址。端口是让系统挑的（绑 `:0`），所以**必须从这里问**，
    /// 不能在别处硬写一个端口号——设置页要把它显示给用户，二维码里也是它。
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    /// 监听套接字有没有真的被丢掉。见 [`Server::closed`] 字段上的注释。
    #[cfg(test)]
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// 停掉监听。
    ///
    /// `accept()` 是阻塞的，光把标志位置起来叫不醒它——所以置完标志**自己连
    /// 一下自己**，让那次 accept 返回，循环再去看标志。这是跨平台都成立的做法；
    /// 依赖 `shutdown()` 或者平台特有的取消机制在 Windows 上并不可靠。
    ///
    /// 已经在处理中的连接会自己跑完（它们有 [`IO_TIMEOUT`]），不强杀。
    pub fn stop(mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
    }
}

/// 开始服务。`listener` 由调用方绑好——**绑哪个地址是调用方的决定**，
/// 测试绑 `127.0.0.1:0`（进不了局域网），生产绑 `0.0.0.0:0`（手机要够得着）。
/// 把这个选择留在外面，测试就不会顺手把一个真的对外开放的端口带起来。
pub fn serve(listener: TcpListener, token: String, handler: Arc<dyn Handler>) -> Server {
    let addr = listener.local_addr().expect("监听器必须已经绑好");
    let stopping = Arc::new(AtomicBool::new(false));
    let inflight = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(AtomicBool::new(false));

    let accept = {
        let stopping = Arc::clone(&stopping);
        let closed = Arc::clone(&closed);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };

                // 上限之外的连接立刻打发走，**并且要真的回一句 503**：
                // 直接关掉的话，用户看到的是"网页转圈然后失败"，而不是
                // "现在太忙"。
                if inflight.load(Ordering::SeqCst) >= MAX_INFLIGHT {
                    let mut s = stream;
                    let _ = write_resp(&mut s, &Resp::status(503));
                    continue;
                }

                let token = token.clone();
                let handler = Arc::clone(&handler);
                let inflight = Arc::clone(&inflight);
                inflight.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    // 一条连接上的 panic 只能毁掉这条连接。守护进程里还跑着
                    // 用户的会话，一个畸形请求不该把它们一起带走——同
                    // `bridge::spawn` 那条规矩。
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handle_conn(stream, &token, handler.as_ref());
                    }));
                    inflight.fetch_sub(1, Ordering::SeqCst);
                });
            }
            // **先丢掉监听套接字，再落标志。** 顺序就是这条标志的全部含义：
            // 标志为真 = 端口已经不在我们手里了。反过来写的话，标志会在
            // 套接字还开着的时候就为真，那它什么都不保证。
            drop(listener);
            closed.store(true, Ordering::SeqCst);
        })
    };

    Server {
        addr,
        stopping,
        accept: Some(accept),
        closed,
    }
}

fn handle_conn(mut stream: TcpStream, token: &str, handler: &dyn Handler) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

    let resp = match read_request(&mut stream) {
        Ok(raw) => {
            // **认证在路由之前。** 顺序反过来的话，401 和 404 的差别就成了
            // 一张接口地图，局域网里的任何人都能免费拿走。
            if is_public(&raw) || authorized(&raw, token) {
                handler.handle(&Req {
                    method: &raw.method,
                    path: &raw.path,
                    query: &raw.query,
                    body: &raw.body,
                })
            } else {
                Resp::status(401)
            }
        }
        Err(status) => Resp::status(status),
    };
    let _ = write_resp(&mut stream, &resp);
}

/// 读下来的一条请求。字段是自有的 `String`——请求行和头读完就没了，
/// 借用它们只会把生命周期传染到整条调用链上，换不来任何东西。
struct RawReq {
    method: String,
    path: String,
    query: String,
    body: Vec<u8>,
    cookie: Option<String>,
    authorization: Option<String>,
}

/// 解析失败一律返回一个状态码，**不返回原因字符串**：原因要么对客户端没用，
/// 要么正是不该告诉他的东西。
fn read_request(stream: &mut TcpStream) -> Result<RawReq, u16> {
    let mut reader = BufReader::new(stream);
    let mut head = Vec::new();
    let mut line = Vec::new();

    // 逐行读到空行为止，同时盯着总长度。
    loop {
        line.clear();
        let n = reader
            .by_ref()
            .take((MAX_HEADER_BYTES - head.len().min(MAX_HEADER_BYTES)) as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| 400u16)?;
        if n == 0 {
            return Err(400); // 连请求行都没读完就断了
        }
        head.extend_from_slice(&line);
        if head.len() > MAX_HEADER_BYTES {
            return Err(431);
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }

    let text = String::from_utf8(head).map_err(|_| 400u16)?;
    let mut lines = text.lines();
    let request_line = lines.next().ok_or(400u16)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or(400u16)?.to_string();
    let target = parts.next().ok_or(400u16)?;
    // 版本那一段存在与否不影响我们怎么答，但请求行必须是三段——
    // 两段的是 HTTP/0.9，那是另一种协议，别猜。
    parts.next().ok_or(400u16)?;

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.to_string(), String::new()),
    };

    let mut content_length: Option<usize> = None;
    let mut cookie = None;
    let mut authorization = None;
    for l in lines {
        let Some((name, value)) = l.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            // chunked 不支持。**必须显式拒绝**：忽略这个头会把一个带 body 的
            // 请求当成没有 body 的请求处理，剩下的字节留在连接上——那是
            // 请求走私那一类问题的入口，不是"少支持一个特性"。
            "transfer-encoding" => return Err(400),
            "content-length" => {
                let n: usize = value.parse().map_err(|_| 400u16)?;
                if n > MAX_BODY_BYTES {
                    return Err(413);
                }
                content_length = Some(n);
            }
            "cookie" => cookie = Some(value.to_string()),
            "authorization" => authorization = Some(value.to_string()),
            _ => {}
        }
    }

    let mut body = vec![0u8; content_length.unwrap_or(0)];
    if !body.is_empty() {
        reader.read_exact(&mut body).map_err(|_| 400u16)?;
    }

    Ok(RawReq {
        method,
        path,
        query,
        body,
        cookie,
        authorization,
    })
}

/// 不需要令牌就能拿到的东西。**只有网页外壳这一样。**
///
/// 为什么必须有这个口子：令牌在二维码里放的是 fragment（`#t=…`），而
/// **fragment 根本不会发给服务器**——浏览器只把它留在本地。所以第一次打开
/// 页面的那个请求身上不可能带着令牌。把外壳也锁上的话，页面永远加载不出来，
/// 也就永远执行不到「把 fragment 换成 cookie」那一步：一个自己锁死自己的
/// 循环。**这个洞是把页面真的用浏览器打开一次才发现的**，所有单测都是绿的。
///
/// 放出去的代价：一段静态 HTML，里面**没有任何用户数据**——没有会话、没有
/// 项目名、没有路径。局域网里的人拿到它，只能知道「这台机器上跑着 dct」，
/// 而那件事在他扫到这个开着的端口时就已经知道了。
///
/// 换成 `/open?t=…` 那样的重定向也能解开循环，但那等于把令牌写进查询串——
/// 进浏览器历史、进任何中间日志。fragment 存在的全部意义就是不进那些地方。
fn is_public(req: &RawReq) -> bool {
    req.method == "GET" && req.path == "/"
}

/// 带对 token 了吗。两种带法：
///
/// - `Authorization: Bearer <token>`——二维码刚扫开、还没换成 cookie 的那一下
/// - `Cookie: dct_web=<token>`——之后的每一次请求
///
/// 两种都收是因为它们是同一条路的两段，不是两条路：网页拿到 fragment 里的
/// token 之后立刻用 Bearer 换一个同源 cookie，地址栏里就不留东西了
/// （查询串会进浏览器历史和中间日志，fragment 不上行）。
fn authorized(req: &RawReq, token: &str) -> bool {
    if let Some(auth) = &req.authorization {
        if let Some(rest) = auth.strip_prefix("Bearer ") {
            if same_secret(rest.trim(), token) {
                return true;
            }
        }
    }
    if let Some(cookie) = &req.cookie {
        for pair in cookie.split(';') {
            if let Some((k, v)) = pair.split_once('=') {
                if k.trim() == COOKIE_NAME && same_secret(v.trim(), token) {
                    return true;
                }
            }
        }
    }
    false
}

/// 网页存 token 用的 cookie 名。
pub const COOKIE_NAME: &str = "dct_web";

/// 常数时间比较。
///
/// `==` 会在第一个不同的字节上短路，于是"猜对了几个字节"变成一个可以从
/// 响应时间上读出来的信号。局域网里的延迟抖动大到多半淹掉它，但这条防线
/// 值一个八行的循环，而"局域网里没人测得准"不是一个能长期成立的假设。
///
/// 长度不同直接返回假：token 长度本来就是公开的（32 字节十六进制），
/// 从长度上读不出任何秘密。
fn same_secret(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn write_resp(stream: &mut TcpStream, resp: &Resp) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\
         \r\n",
        resp.status,
        reason(resp.status),
        resp.content_type,
        resp.body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&resp.body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    /// 起一个只回 200 的服务，返回 (server, 地址, token)。
    fn up() -> (Server, String, String) {
        up_with(|_: &Req| Resp::json("{}"))
    }

    fn up_with<H: Handler>(h: H) -> (Server, String, String) {
        // **绑回环地址**：测试不该把一个真的能从局域网连上的端口带起来。
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let token = new_token().unwrap();
        let s = serve(l, token.clone(), Arc::new(h));
        let addr = s.addr().to_string();
        (s, addr, token)
    }

    /// 手写请求，不走 HTTP 客户端库——这几条测试要发的正是**畸形**请求
    /// （chunked、超长头），而一个正经客户端根本发不出来。
    fn raw(addr: &str, req: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    fn status_of(resp: &str) -> u16 {
        resp.split_whitespace().nth(1).unwrap().parse().unwrap()
    }

    #[test]
    fn without_the_token_nothing_gets_through() {
        let (s, addr, _t) = up();
        let r = raw(&addr, "GET /api/sessions HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(status_of(&r), 401, "没带令牌就该 401：{r}");
        s.stop();
    }

    /// **网页外壳是唯一不要令牌的东西，而且只有 `GET`。** 它必须放出去，
    /// 否则页面加载不出来 → 执行不到「把 fragment 里的令牌换成 cookie」那一步
    /// → 永远也带不上令牌（自己锁死自己）。
    #[test]
    fn only_the_page_shell_is_public_and_only_for_get() {
        let (s, addr, _t) = up_with(|_: &Req| Resp::html("<!doctype html>"));

        let shell = raw(&addr, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(status_of(&shell), 200, "外壳该放出去：{shell}");

        // 但只有 GET：POST / 不在白名单里。
        let post = raw(
            &addr,
            "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
        );
        assert_eq!(status_of(&post), 401, "POST / 不该免票：{post}");

        // 带数据的路径一个都不许免票，长得像 `/` 的也不行。
        for path in [
            "/api/sessions",
            "/api/screen?id=1",
            "/api/strings",
            "/index.html",
            "//",
        ] {
            let r = raw(&addr, &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"));
            assert_eq!(status_of(&r), 401, "{path} 不该免票：{r}");
        }
        s.stop();
    }

    #[test]
    fn the_token_gets_through_as_a_header_or_a_cookie() {
        let (s, addr, t) = up();
        let bearer = raw(
            &addr,
            &format!("GET /api/sessions HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {t}\r\n\r\n"),
        );
        assert_eq!(status_of(&bearer), 200, "Bearer 该放行：{bearer}");

        let cookie = raw(
            &addr,
            &format!("GET /api/sessions HTTP/1.1\r\nHost: x\r\nCookie: {COOKIE_NAME}={t}\r\n\r\n"),
        );
        assert_eq!(status_of(&cookie), 200, "cookie 该放行：{cookie}");
        s.stop();
    }

    /// 差一个字符也不行。这条看着显然，但它钉的是 `same_secret` 那个手写的
    /// 常数时间比较——一个写错的循环（比如忘了 `|=` 写成 `=`）会让**最后一个
    /// 字节相同**的任何令牌都通过。
    #[test]
    fn a_token_that_is_almost_right_is_still_refused() {
        let (s, addr, t) = up();
        let mut wrong: Vec<char> = t.chars().collect();
        wrong[0] = if wrong[0] == 'a' { 'b' } else { 'a' };
        let wrong: String = wrong.into_iter().collect();
        let r = raw(
            &addr,
            &format!(
                "GET /api/sessions HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {wrong}\r\n\r\n"
            ),
        );
        assert_eq!(status_of(&r), 401, "第一个字符不同就得拒：{r}");

        let mut wrong: Vec<char> = t.chars().collect();
        let last = wrong.len() - 1;
        wrong[last] = if wrong[last] == 'a' { 'b' } else { 'a' };
        let wrong: String = wrong.into_iter().collect();
        let r = raw(
            &addr,
            &format!(
                "GET /api/sessions HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {wrong}\r\n\r\n"
            ),
        );
        assert_eq!(status_of(&r), 401, "最后一个字符不同也得拒：{r}");
        s.stop();
    }

    /// **没带令牌时，存在的路径和不存在的路径必须长得一模一样。** 否则
    /// 局域网里的任何人都能拿 401/404 的差别把接口摸一遍。
    #[test]
    fn an_unauthenticated_probe_learns_nothing_about_the_routes() {
        let (s, addr, _t) = up_with(|req: &Req| {
            if req.path == "/api/sessions" {
                Resp::json("{}")
            } else {
                Resp::status(404)
            }
        });
        let real = raw(&addr, "GET /api/sessions HTTP/1.1\r\nHost: x\r\n\r\n");
        let fake = raw(&addr, "GET /nope HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(status_of(&real), 401);
        assert_eq!(status_of(&fake), 401);
        assert_eq!(
            real, fake,
            "两条响应必须逐字节一样，差别本身就是情报：\n{real}\n{fake}"
        );
        s.stop();
    }

    /// 认证之前不许调用路由。上面那条测的是"看起来一样"，这条测的是
    /// "真的没跑"——一个先路由后认证的实现，前一条照样能过。
    #[test]
    fn the_handler_never_runs_for_an_unauthenticated_request() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let s2 = Arc::clone(&seen);
        let (s, addr, _t) = up_with(move |req: &Req| {
            s2.lock().unwrap().push(req.path.to_string());
            Resp::json("{}")
        });
        let _ = raw(&addr, "GET /api/sessions HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            seen.lock().unwrap().is_empty(),
            "没认证就到了路由：{:?}",
            seen.lock().unwrap()
        );
        s.stop();
    }

    /// chunked 必须**显式拒绝**，不能当成"没有 body"往下走——那样剩下的
    /// 字节会留在连接上，是请求走私那一类问题的入口。
    #[test]
    fn a_chunked_body_is_refused_rather_than_misread() {
        let (s, addr, t) = up();
        let r = raw(
            &addr,
            &format!(
                "POST /api/input HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {t}\r\n\
                 Transfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n"
            ),
        );
        assert_eq!(status_of(&r), 400, "chunked 该被拒：{r}");
        s.stop();
    }

    #[test]
    fn an_oversized_body_is_refused_by_the_declared_length_alone() {
        let (s, addr, t) = up();
        // 只声明一个大得离谱的长度，一个字节都不发——真读进来才拒就晚了。
        let r = raw(
            &addr,
            &format!(
                "POST /api/input HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {t}\r\n\
                 Content-Length: {}\r\n\r\n",
                MAX_BODY_BYTES + 1
            ),
        );
        assert_eq!(status_of(&r), 413, "超长 body 该被拒：{r}");
        s.stop();
    }

    #[test]
    fn an_endless_header_is_cut_off() {
        let (s, addr, t) = up();
        let filler = "X-Pad: ".to_string() + &"a".repeat(MAX_HEADER_BYTES) + "\r\n";
        let r = raw(
            &addr,
            &format!(
                "GET /api/sessions HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {t}\r\n{filler}\r\n"
            ),
        );
        assert_eq!(status_of(&r), 431, "超长头该被拒：{r}");
        s.stop();
    }

    /// 请求体要原样交到路由手里——`Content-Length` 读少一个字节，
    /// 用户在手机上打的最后一个字就没了。
    #[test]
    fn the_body_reaches_the_handler_whole() {
        let got = Arc::new(Mutex::new(Vec::<u8>::new()));
        let g2 = Arc::clone(&got);
        let (s, addr, t) = up_with(move |req: &Req| {
            *g2.lock().unwrap() = req.body.to_vec();
            Resp::json("{}")
        });
        let body = "{\"text\":\"你好\"}";
        let r = raw(
            &addr,
            &format!(
                "POST /api/input HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {t}\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert_eq!(status_of(&r), 200);
        assert_eq!(
            String::from_utf8(got.lock().unwrap().clone()).unwrap(),
            body,
            "body 到路由那儿必须一个字节不差"
        );
        s.stop();
    }

    #[test]
    fn the_query_string_is_split_off_the_path() {
        let seen = Arc::new(Mutex::new((String::new(), String::new())));
        let s2 = Arc::clone(&seen);
        let (s, addr, t) = up_with(move |req: &Req| {
            *s2.lock().unwrap() = (req.path.to_string(), req.query.to_string());
            Resp::json("{}")
        });
        let _ = raw(
            &addr,
            &format!(
                "GET /api/screen?id=3 HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {t}\r\n\r\n"
            ),
        );
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen, ("/api/screen".to_string(), "id=3".to_string()));
        s.stop();
    }

    /// 停掉之后端口必须真的不再接受连接。**这条不是洁癖**：设置页里那个开关
    /// 关上之后端口还开着，等于屏幕上写着"已关闭"而实际还在监听——
    /// 这个仓库对"屏幕和事实不符"是零容忍的。
    #[test]
    fn stopping_really_closes_the_port() {
        let (s, addr, _t) = up();
        assert!(TcpStream::connect(&addr).is_ok(), "前提：现在连得上");
        assert!(!s.is_closed(), "前提：还没停，标志不该已经立起来");

        // 拿一个自己的 `Arc` 副本，因为 `stop()` 要吃掉 `s`。
        let closed = Arc::clone(&s.closed);
        s.stop();

        // **`stop()` 返回时监听套接字必须已经被丢掉。**
        //
        // 这一条以前是从外面测的：`stop()` 之后再连一次那个地址，断言连不上。
        // 那么写**大约每六次全量跑红一次**，单独跑却永远是绿的——端口是绑
        // `:0` 让系统挑的，释放之后并行跑的别的测试完全可能立刻绑到同一个
        // 端口上，于是"连得上"根本不代表我们的服务还在。它测的是地址，而
        // 地址会被操作系统重新发出去。
        //
        // **这条断言有多强，说清楚，别高估它。** 试过两个变异：删掉 `stop()`
        // 里的 `join`、把 `drop(listener)` 挪到落标志之后——**两个它都抓不到**，
        // 因为 accept 线程被那次自连唤醒之后微秒级就跑完了，断言执行时标志
        // 早已立起。真正保证这件事的是所有权：`listener` 是 move 进线程的，
        // 线程一结束就 drop，而 `stop()` join 了它。这条断言只是把那个不变量
        // 写成可执行的形式，外加挡住"标志根本没人置"这种整段丢失的改动。
        //
        // 不改回从外面连的写法，是因为那个版本抖，而**一条会随机变红的测试
        // 比一条弱测试更贵**：它训练人忽略红色。要真做强，得让线程退出这件事
        // 可控（比如注入一个能挡住它的闸），那是另一件事，现在没做。
        assert!(
            closed.load(Ordering::SeqCst),
            "stop() 回来了，但监听套接字还没被丢掉：{addr}"
        );
    }

    /// 令牌必须活过重启。生成一次就存下来，下次原样读回来——否则每次开 dct
    /// 都会把所有扫过码的手机踢下线。
    #[test]
    fn the_token_is_generated_once_and_then_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.toml");

        let mut store = crate::secrets::SecretStore::load(&path);
        let first = ensure_token(&mut store).unwrap();
        let again = ensure_token(&mut store).unwrap();
        assert_eq!(first, again, "同一个进程里问两次，不该换令牌");

        // 重开一份（= 重启守护进程），必须还是同一个。
        let mut reloaded = crate::secrets::SecretStore::load(&path);
        assert_eq!(
            ensure_token(&mut reloaded).unwrap(),
            first,
            "重启之后令牌变了，扫过码的手机全掉线"
        );
    }

    #[test]
    fn two_tokens_are_never_the_same_and_are_long_enough() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), TOKEN_BYTES * 2, "32 字节 = 64 个十六进制字符");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
