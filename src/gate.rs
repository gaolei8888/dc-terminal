//! 远程版那道门：一个只做鉴权的反向代理。
//!
//! 它坐在 ttyd（或者第二期任何一个 HTTP/WebSocket 上游）前面。没带对钥匙的
//! 连接一律 401，带对了就退化成一根字节管道，`101 Switching Protocols` 之后
//! 的 WebSocket 帧原样对穿。
//!
//! 计划：`docs/superpowers/plans/2026-09-06-dc-workspace-phase1.md` 任务 3。
//!
//! ## 为什么不是 `ttyd --credential`
//!
//! 那是 HTTP Basic：浏览器弹一个原生的用户名/密码框。给「压根没有合适环境」
//! 的人发一条链接，让他先理解「用户名密码」这四个字，已经把他挡在外面了。
//! 而且第二期（本地 TUI 直连远程守护进程）用不上 HTTP Basic 的任何一行——
//! 那时候要的还是「一个钥匙、常数时间比对」，等于从零再建一遍。
//!
//! ## 钥匙这一半是**复用**的，不是新写的
//!
//! 生成、落盘、常数时间比对、fragment 换 cookie，这四样 `web/mod.rs` 里三周前
//! 就有了（手机端那条路踩出来的）。这个模块只写了它没有的那一半：**上游**。
//! 具体复用的东西各自标了出处，别在这里再写一份。
//!
//! 钥匙自己存一个键（`secrets::GATE_TOKEN_KEY`），**不跟手机端共用**：
//! 手机端那个钥匙给的是只读画面加一行输入，这个给的是一整台机器上的终端。
//! 泄漏后果不一样，撤销时机也不一样，共用一个键会让「换掉远程的钥匙」
//! 顺手把手机踢下线。
//!
//! ## 三条从 `web/mod.rs` 原样搬过来的规矩
//!
//! - **认证在路由之前。** 没带对钥匙的请求一律 401，连「这个路径存不存在」
//!   都不告诉他。反过来写的话，401 和 404 的差别就是一张免费的接口地图。
//! - **401 的 body 恒为空。** 要说给人听的那句话在 `/` 那一页上（见下），
//!   不在每一个子资源的错误响应里。
//! - **比对用常数时间循环**（`web::same_secret`，就是那一份）。
//!
//! ## 一条自己锁死自己的循环，以及它的口子
//!
//! 钥匙在 fragment 里（`#t=…`），而 **fragment 根本不发给服务器**。所以进门
//! 的第一个请求身上不可能带着钥匙——把 `/` 也锁上的话，页面永远加载不出来，
//! 也就永远执行不到「把 fragment 换成 cookie」那一步。
//!
//! 所以 `GET /` 在**没带钥匙时**由这道门自己答一页静态 HTML（`gate.html`）：
//! 它只做两件事——有钥匙就换成 cookie 再重载，没钥匙就说一句人话。里面
//! **没有任何用户数据**：没有会话、没有项目名、没有路径。带对钥匙之后
//! `/` 就跟别的路径一样转给上游，学生看到的是 ttyd 发的那一页。
//!
//! 这个洞和它的理由跟 `web::is_public` 上那段是同一条，那边是把页面外壳放行，
//! 这边是自己发一页外壳——差别只在于门后面那一页不是我们画的。
//!
//! ## 明说的两条边界
//!
//! **一、鉴权是一条 TCP 连接查一次，在它的第一个请求上。** 之后这条连接就是
//! 一根字节管道，我们不再解析里面的东西。这是有意的：要每个请求都查，就得
//! 完整地解析 HTTP 的消息边界（`Content-Length`、chunked、pipelining），
//! 而那正是 `web/mod.rs` 顶上「不引框架、把支持的子集写清楚」拒绝去做的事。
//! 一根不复用、不解析、不回收的管道没有请求走私那一类问题——走私要的是
//! 前后端对消息边界看法不一致**并且连接被复用**，这两条我们都不占。
//! 代价诚实地说：拿到过钥匙的人可以在同一条连接上继续发请求。而他本来
//! 就有钥匙。
//!
//! **二、这道门不做加密。** 明文 HTTP 上，cookie 里那把钥匙每个请求都在线上
//! 裸奔。所以它前面必须有 TLS 才能给真实用户用——那是部署方的事（反代 +
//! 证书），也是 `container/README.md` 里写死的一句。这跟 `dct-srv` 的
//! `must_be_loopback` 是同一条线：没有加密之前，不许当成对外可用。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::i18n::{text, Key, Lang};

/// 请求行 + 全部头的上限，同 `web::MAX_HEADER_BYTES`，理由也同：一个不设限的
/// 读循环等于让任何人用一条永不结束的头把内存吃光。
const MAX_HEAD_BYTES: usize = 8 * 1024;

/// **只管到握手为止**的读写超时。连上来不说话的客户端不能永久占住一个线程。
///
/// 握手之后必须摘掉（见 `pump_both`）：WebSocket 本来就是长时间没有字节来往，
/// 超时留着的话，一个安安静静看着屏幕的学生会被这道门自己踢掉。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// 同时在处理的连接数上限。一个浏览器标签大约占两条（页面 + WebSocket），
/// 所以这个数字不是「几个人」，是「几条连接」。超了立刻回 503 并关掉。
const MAX_INFLIGHT: usize = 64;

/// 这道门自己的 cookie 名。
///
/// **必须跟 `web::COOKIE_NAME` 不同名，这不是洁癖。** cookie 不按端口隔离：
/// 同一个主机名下，手机端那个服务和这道门看到的是同一批 cookie。重名的话
/// 后设的那个会把前一个盖掉，表现是「开了远程之后手机端莫名其妙要重新扫码」。
pub const COOKIE_NAME: &str = "dct_gate";

/// fragment 里那个键：`#t=<钥匙>`。跟手机端那条路同一个字母，同一个含义
/// （`crates/dct-page/page.html` 里 `fromHash("t")`）。
const FRAGMENT_KEY: &str = "t";

/// 拿到这台机器的门钥匙：有就用，没有才生成一个并存下来。
///
/// **是 `ensure` 不是 `new`**，理由跟 `web::ensure_token` 上那段一字不差：
/// 每次启动重新生成的话，已经发出去的链接会在下一次重启后集体失效，而
/// 收到链接的人对此毫无感知。换钥匙是一个**人主动做的动作**，不该是重启的
/// 副作用。
///
/// 生成器用的就是 `web::new_token`（32 字节系统随机数、十六进制），
/// 整个仓库里只有那一份。
pub fn ensure_token(store: &mut crate::secrets::SecretStore) -> anyhow::Result<String> {
    if let Some(t) = store.get(crate::secrets::GATE_TOKEN_KEY) {
        return Ok(t.to_string());
    }
    let t = crate::web::new_token()?;
    store.set(crate::secrets::GATE_TOKEN_KEY, &t)?;
    Ok(t)
}

/// 把钥匙拼成一条能直接发出去的链接。
///
/// `base` 是**外面看到的**地址（`http://box.example.com:7681`），不是容器里
/// 那个——容器不知道自己前面有什么反代、被映射到哪个端口，那正是
/// `container/README.md` 里那条契约说的「镜像不知道自己被起了几份」。
pub fn link(base: &str, token: &str) -> String {
    format!("{}/#{FRAGMENT_KEY}={token}", base.trim_end_matches('/'))
}

/// 跑着的门。丢掉它不会停掉服务，要停得显式 `stop()`——同 `web::Server`。
pub struct Gate {
    addr: SocketAddr,
    stopping: Arc<AtomicBool>,
    accept: Option<std::thread::JoinHandle<()>>,
}

impl Gate {
    /// 真实监听地址。测试绑 `:0` 让系统挑端口，所以必须从这里问。
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// 停掉监听。`accept()` 是阻塞的，光置标志叫不醒它，所以置完自己连一下
    /// 自己——同 `web::Server::stop`，那边写清了为什么不用 `shutdown()`。
    ///
    /// 已经接通的管道不强断：它们连着的是学生正看着的画面。
    pub fn stop(mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
    }
}

/// 开始守门。
///
/// `listener` 由调用方绑好——**绑哪个地址是调用方的决定**，同 `web::serve`：
/// 测试绑 `127.0.0.1:0`，容器里绑 `0.0.0.0:<DCW_PORT>`（前面是宿主的端口
/// 映射和反代）。把这个选择留在外面，测试就不会顺手把一个真的对外开放的
/// 端口带起来。
pub fn serve(listener: TcpListener, token: String, upstream: SocketAddr, lang: Lang) -> Gate {
    let addr = listener.local_addr().expect("监听器必须已经绑好");
    let stopping = Arc::new(AtomicBool::new(false));
    let inflight = Arc::new(AtomicUsize::new(0));

    let accept = {
        let stopping = Arc::clone(&stopping);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };

                if inflight.load(Ordering::SeqCst) >= MAX_INFLIGHT {
                    // 直接关掉的话，用户看到的是「转圈然后失败」，
                    // 而不是「现在太忙」。同 `web::serve` 那一条。
                    let mut s = stream;
                    let _ = write_status(&mut s, 503);
                    continue;
                }

                let token = token.clone();
                let inflight = Arc::clone(&inflight);
                inflight.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    // 一条连接上的 panic 只能毁掉这条连接：这个进程后面还
                    // 连着别人的会话。同 `web::serve`。
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handle_conn(stream, &token, upstream, lang);
                    }));
                    inflight.fetch_sub(1, Ordering::SeqCst);
                });
            }
            drop(listener);
        })
    };

    Gate {
        addr,
        stopping,
        accept: Some(accept),
    }
}

fn handle_conn(mut down: TcpStream, token: &str, upstream: SocketAddr, lang: Lang) {
    let _ = down.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = down.set_write_timeout(Some(HANDSHAKE_TIMEOUT));

    let head = match read_head(&mut down) {
        Ok(h) => h,
        Err(status) => {
            let _ = write_status(&mut down, status);
            return;
        }
    };

    // **认证在路由之前。** 下面这个 if 里没有任何一处看 `path` 去决定
    // 401 还是 404——这道门永远不回 404，路径存不存在是上游的事，而
    // 上游只有带对钥匙的人见得到。
    if !authorized(head.cookie.as_deref(), token) {
        if head.method == "GET" && head.path == "/" {
            // 唯一的口子，见模块头「一条自己锁死自己的循环」。
            let _ = write_page(&mut down, lang);
        } else {
            let _ = write_status(&mut down, 401);
        }
        return;
    }

    let Ok(mut up) = TcpStream::connect(upstream) else {
        // 502 而不是 500：错在上游没起来，不在这条请求。body 照样是空的——
        // 上游的地址和状态一个字都不往外写。
        let _ = write_status(&mut down, 502);
        return;
    };
    // 已经读进来的那一段头要原样吐给上游，一个字节不改：这道门不重写 HTTP，
    // 它只是决定放不放行。改了的话，`Upgrade`/`Sec-WebSocket-Key` 这些握手
    // 用的头就得由我们负责正确重建，而那是另一个能出错的地方。
    if up.write_all(&head.raw).is_err() {
        return;
    }
    pump_both(down, up);
}

/// 两个方向各一条泵，谁先读到 EOF 就把对面的写端半关，让另一条也收工。
///
/// **进来第一件事是摘超时。** 握手阶段那 10 秒是防「连上来不说话」的，
/// 到了这里它变成「安静地看着屏幕十秒就被踢」。半关而不是直接 `shutdown`
/// 两端：另一个方向可能还有没送完的字节（比如 agent 最后那半屏输出）。
fn pump_both(down: TcpStream, up: TcpStream) {
    for s in [&down, &up] {
        let _ = s.set_read_timeout(None);
        let _ = s.set_write_timeout(None);
    }
    let (Ok(mut up_r), Ok(mut down_w)) = (up.try_clone(), down.try_clone()) else {
        return;
    };
    let (mut down_r, mut up_w) = (down, up);

    let back = std::thread::spawn(move || {
        let _ = std::io::copy(&mut up_r, &mut down_w);
        let _ = down_w.shutdown(Shutdown::Write);
    });
    let _ = std::io::copy(&mut down_r, &mut up_w);
    let _ = up_w.shutdown(Shutdown::Write);
    let _ = back.join();
}

/// 读下来的请求头：**原始字节留着**（要原样转给上游），另外只解出三样东西。
///
/// 只解三样是有意的：这道门要判断的只有「带钥匙了吗」和「是不是那个口子」。
/// 多解一个字段就多一处要跟上游保持一致的解析——而上游是 ttyd，不是我们。
struct Head {
    raw: Vec<u8>,
    method: String,
    path: String,
    cookie: Option<String>,
}

/// 逐行读到空行为止，同时盯着总长度。**只读头，不读 body**：body 属于
/// 上游那条管道，我们读进来反而要负责再吐出去。
fn read_head(stream: &mut TcpStream) -> Result<Head, u16> {
    let mut reader = BufReader::new(stream);
    let mut raw = Vec::new();
    let mut line = Vec::new();

    loop {
        line.clear();
        let room = MAX_HEAD_BYTES.saturating_sub(raw.len()) as u64 + 1;
        let n = reader
            .by_ref()
            .take(room)
            .read_until(b'\n', &mut line)
            .map_err(|_| 400u16)?;
        if n == 0 {
            return Err(400); // 连请求行都没读完就断了
        }
        raw.extend_from_slice(&line);
        if raw.len() > MAX_HEAD_BYTES {
            return Err(431);
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }

    // `BufReader` 可能已经预读了 body 的头几个字节。**必须把它们捞回来**，
    // 否则那几个字节既没进 `raw`、也不在 socket 里，转给上游的请求就少了一截
    // ——症状是「大部分请求正常，偶尔有一个卡住」，最难查的那一种。
    let buffered = reader.buffer().to_vec();
    raw.extend_from_slice(&buffered);

    // 头必须是 UTF-8（实际上是 ASCII）。不是的话别猜，直接 400。
    let text = std::str::from_utf8(&raw[..raw.len() - buffered.len()]).map_err(|_| 400u16)?;
    let mut lines = text.lines();
    let request_line = lines.next().ok_or(400u16)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or(400u16)?.to_string();
    let target = parts.next().ok_or(400u16)?;
    // 请求行必须是三段。两段的是 HTTP/0.9，那是另一种协议，别猜。
    parts.next().ok_or(400u16)?;
    let path = match target.split_once('?') {
        Some((p, _)) => p.to_string(),
        None => target.to_string(),
    };

    let mut cookie = None;
    for l in lines {
        let Some((name, value)) = l.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("cookie") {
            // 多条 Cookie 头是合法的，全都要看：只认第一条的话，浏览器把
            // 顺序换一下这道门就打不开了。
            let v = value.trim().to_string();
            cookie = Some(match cookie {
                Some(prev) => format!("{prev}; {v}"),
                None => v,
            });
        }
    }

    Ok(Head {
        raw,
        method,
        path,
        cookie,
    })
}

/// cookie 里那把钥匙对不对。**只认 cookie，不认查询串**——查询串会进浏览器
/// 历史和任何中间日志，而这一整条路的设计就是不让钥匙进那些地方。
fn authorized(cookie: Option<&str>, token: &str) -> bool {
    let Some(cookie) = cookie else { return false };
    for pair in cookie.split(';') {
        if let Some((k, v)) = pair.split_once('=') {
            if k.trim() == COOKIE_NAME && crate::web::same_secret(v.trim(), token) {
                return true;
            }
        }
    }
    false
}

/// 门口那一页。文案从 `i18n` 取，**不写死在 HTML 里**——`web/strings.rs` 顶上
/// 那条规矩（决定用户看到什么字的地方只有一个）在这里同样成立。
fn page(lang: Lang) -> String {
    include_str!("gate.html")
        .replace("__LANG__", lang.code())
        .replace("__TITLE__", &html_escape(text(Key::BoardTitle, lang)))
        .replace("__NEEDS_LINK__", &html_escape(text(Key::GateNeedsLink, lang)))
        .replace("__COOKIE__", COOKIE_NAME)
}

/// 文案是 `i18n` 里的常量，今天里面没有 `<`，但**转义不是给今天写的**：
/// 有一天有人在词条里写一个 `&`，页面就该照常显示它，而不是坏掉。
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn write_page(stream: &mut TcpStream, lang: Lang) -> std::io::Result<()> {
    let body = page(lang);
    let head = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// 只有状态码的响应，**body 恒为空**——同 `web::Resp::status` 那一条：
/// 错误的细节属于用户面前那块屏幕，不属于任何一个能连到这个端口的人。
fn write_status(stream: &mut TcpStream, status: u16) -> std::io::Result<()> {
    let reason = match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        431 => "Request Header Fields Too Large",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    };
    stream.write_all(
        format!(
            "HTTP/1.1 {status} {reason}\r\n\
             Content-Length: 0\r\n\
             Connection: close\r\n\r\n"
        )
        .as_bytes(),
    )?;
    stream.flush()
}

// ---------------------------------------------------------------------------
// `dct gate` —— 命令行这一层
// ---------------------------------------------------------------------------
//
// **这几句提示不走 `i18n`，是有意的**，跟页面上那句话不一样：页面的读者是学生
// （产品表面，`web/strings.rs` 那条「一个字都不许写死」管着它），这里的读者是
// 起容器的那个人，看到它的地方是 `docker compose logs`。main.rs 里 `dct install`
// 的用法提示也是这么写的，跟着那条线走。

/// `dct gate` 的参数。**抽成纯函数是为了能测**——「默认绑哪儿」这件事一旦
/// 写错就是把一个没有加密的终端挂到局域网上，那必须有测试钉着。
#[derive(Debug, PartialEq, Eq)]
pub struct GateArgs {
    /// 绑哪个地址。**默认只绑环回。**
    ///
    /// 想让别人连得上必须显式写 `--bind 0.0.0.0`。这条门后面是一整台机器上的
    /// 终端，而这一层还没有加密（模块头「明说的两条边界」第二条）——默认值
    /// 必须是那个错了也不出事的。同 `dct-srv` 的 `must_be_loopback`。
    pub bind: String,
    pub port: u16,
    /// 上游。默认是容器里 ttyd 监听的那个环回地址。
    pub upstream: String,
    /// 外面看到的地址前缀，用来拼那条能发出去的链接。
    /// 不给就只能按 `bind` 猜一个，并且明说是猜的。
    pub public_url: Option<String>,
    /// 只印链接，不起服务。
    pub link_only: bool,
}

/// 参数不对时返回一句说清楚的话，不返回 `Err`——用法错误不是异常。
pub fn parse_gate_args(args: &[String]) -> Result<GateArgs, String> {
    let mut out = GateArgs {
        bind: "127.0.0.1".to_string(),
        port: 7681,
        upstream: "127.0.0.1:7682".to_string(),
        public_url: None,
        link_only: false,
    };
    let mut i = 0;
    while i < args.len() {
        let need = |i: usize| -> Result<&String, String> {
            args.get(i + 1)
                .ok_or_else(|| format!("{} 后面要跟一个值", args[i]))
        };
        match args[i].as_str() {
            "--bind" => {
                out.bind = need(i)?.clone();
                i += 2;
            }
            "--port" => {
                out.port = need(i)?
                    .parse()
                    .map_err(|_| format!("--port 要一个 1..65535 的数，收到的是 {}", args[i + 1]))?;
                i += 2;
            }
            "--upstream" => {
                out.upstream = need(i)?.clone();
                i += 2;
            }
            "--url" => {
                out.public_url = Some(need(i)?.clone());
                i += 2;
            }
            "--link" => {
                out.link_only = true;
                i += 1;
            }
            other => return Err(format!("不认识的参数：{other}")),
        }
    }
    if out.port == 0 {
        return Err("--port 不能是 0：这道门的地址要写进发出去的链接里".into());
    }
    Ok(out)
}

/// `dct gate` 的入口。返回进程退出码。
pub fn run_cli(args: &[String], lang: Lang) -> i32 {
    let args = match parse_gate_args(args) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}

用法：dct gate [--bind 地址] [--port 端口] [--upstream 主机:端口] [--url 外面看到的地址] [--link]");
            return 2;
        }
    };

    // 钥匙跟着 socket 走（`secrets_path_for_socket`），所以它落在 `~/.dct` 上——
    // 容器里那正是命名卷，重建容器不会让已经发出去的链接失效。
    let sock = crate::proto::socket_path();
    let path = crate::secrets::secrets_path_for_socket(&sock);
    let mut store = crate::secrets::SecretStore::load(&path);
    let token = match ensure_token(&mut store) {
        Ok(t) => t,
        Err(e) => {
            // 不退回一个只活在内存里的钥匙：那样这次能连上，重启之后所有
            // 发出去的链接一起失效，而中间没有任何一句话解释发生了什么。
            // 同 `web::ensure_token` 上那段。
            eprintln!("拿不到门钥匙（{}）：{e}", path.display());
            return 1;
        }
    };

    let base = args.public_url.clone().unwrap_or_else(|| {
        format!("http://{}:{}", guessed_host(&args.bind), args.port)
    });
    println!("{}", link(&base, &token));
    if args.public_url.is_none() {
        // **明说是猜的。** 容器里 `lan_ip()` 拿到的是网桥上那个 172.x，
        // 外面谁也连不上——不说清楚的话，发链接的人会把一个必然打不开的
        // 地址发给学生，而学生只会说「打不开」。
        eprintln!("（上面这条链接的地址是猜的。外面看到的不是这个的话，用 --url 告诉它。）");
    }
    eprintln!("这条链接本身就是钥匙，别发到公开的地方。换钥匙：删掉 {} 里的 __gate__ 这一项。", path.display());
    if args.link_only {
        return 0;
    }

    let addr = format!("{}:{}", args.bind, args.port);
    let upstream: SocketAddr = match resolve_one(&args.upstream) {
        Some(a) => a,
        None => {
            eprintln!("--upstream 解析不了：{}", args.upstream);
            return 1;
        }
    };
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("绑不上 {addr}：{e}");
            return 1;
        }
    };
    if args.bind == "127.0.0.1" || args.bind == "::1" || args.bind == "localhost" {
        eprintln!("只绑了 {addr}（默认）。要让别的机器连得上，加 --bind 0.0.0.0，并且**在前面架一层 TLS**——这道门自己不加密。");
    }
    let mut gate = serve(listener, token, upstream, lang);
    eprintln!("门开在 {}，上游 {upstream}。", gate.addr());
    // accept 线程就是这个进程的全部工作，join 到它自己结束为止。
    //
    // **不走 `Gate::stop()`**：那是给测试和「起在别的东西里面」用的（置标志、
    // 自己连自己叫醒 accept）。这里没有谁会来叫停——容器里停这个进程的是
    // `docker stop` 发下来的 SIGTERM，默认动作直接把它打死，跟守护进程一样。
    if let Some(h) = gate.accept.take() {
        let _ = h.join();
    }
    0
}

/// `--url` 没给的时候，拿什么当地址。只是为了让印出来的那一行像个链接，
/// 真实性由上面那句「是猜的」负责。
fn guessed_host(bind: &str) -> String {
    if bind == "0.0.0.0" || bind == "::" {
        return crate::web::lan_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "127.0.0.1".to_string());
    }
    bind.to_string()
}

/// `主机:端口` 解析成一个地址。多个的话取第一个——上游是我们自己在同一台
/// 机器上起的那一个，不存在挑哪个的问题。
fn resolve_one(s: &str) -> Option<SocketAddr> {
    use std::net::ToSocketAddrs;
    s.to_socket_addrs().ok()?.next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    /// 一个只会把收到的东西原样吐回去的上游，外加把第一段请求头记下来。
    /// 用它就能验「转给上游的字节跟收到的一模一样」，不用起 ttyd。
    struct FakeUpstream {
        addr: SocketAddr,
        seen: Arc<std::sync::Mutex<Vec<u8>>>,
    }

    fn fake_upstream() -> FakeUpstream {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { continue };
                let seen = Arc::clone(&seen2);
                std::thread::spawn(move || {
                    let mut r = BufReader::new(s.try_clone().unwrap());
                    let mut head = Vec::new();
                    let mut line = Vec::new();
                    loop {
                        line.clear();
                        if r.read_until(b'\n', &mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        head.extend_from_slice(&line);
                        if line == b"\r\n" {
                            break;
                        }
                    }
                    seen.lock().unwrap().extend_from_slice(&head);
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");
                    // 之后原样回显，用来验双向对穿。
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = r.read(&mut buf) {
                        if n == 0 || s.write_all(&buf[..n]).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        FakeUpstream { addr, seen }
    }

    fn gate_with(token: &str) -> (Gate, FakeUpstream) {
        let up = fake_upstream();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let g = serve(l, token.to_string(), up.addr, Lang::Zh);
        (g, up)
    }

    /// 发一条请求，返回整个响应（含头）。
    fn request(addr: SocketAddr, raw: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    fn get(addr: SocketAddr, path: &str, cookie: Option<&str>) -> String {
        let c = cookie
            .map(|c| format!("Cookie: {c}\r\n"))
            .unwrap_or_default();
        request(
            addr,
            &format!("GET {path} HTTP/1.1\r\nHost: x\r\n{c}\r\n"),
        )
    }

    #[test]
    fn without_the_key_every_path_answers_the_same_401() {
        let (g, _up) = gate_with("aaaa");
        // 存在的路径（上游真的有 /ws）和不存在的路径必须**长得一模一样**，
        // 否则 401/404 的差别就是一张接口地图。
        let a = get(g.addr(), "/ws", None);
        let b = get(g.addr(), "/definitely-not-a-real-path", None);
        assert!(a.starts_with("HTTP/1.1 401 "), "{a}");
        assert_eq!(a, b, "不同路径的 401 必须一个字节都不差");
        g.stop();
    }

    #[test]
    fn a_wrong_key_is_no_better_than_no_key() {
        let (g, _up) = gate_with("aaaa");
        let r = get(g.addr(), "/ws", Some("dct_gate=bbbb"));
        assert!(r.starts_with("HTTP/1.1 401 "), "{r}");
        // 长度不同的也一样：`same_secret` 先比长度，这里钉住它没被绕过。
        let r2 = get(g.addr(), "/ws", Some("dct_gate=a"));
        assert!(r2.starts_with("HTTP/1.1 401 "), "{r2}");
        g.stop();
    }

    #[test]
    fn the_401_body_is_empty_so_nothing_leaks() {
        let (g, _up) = gate_with("aaaa");
        let r = get(g.addr(), "/ws", None);
        assert!(r.ends_with("\r\n\r\n"), "401 后面不该跟任何 body：{r}");
        g.stop();
    }

    /// 那个口子：没钥匙的 `GET /` 拿到的是门口那一页，不是 401。
    /// 没有它，「fragment 换 cookie」这一步永远执行不到——一个自己锁死
    /// 自己的循环。
    #[test]
    fn the_door_page_is_the_only_thing_open_without_a_key() {
        let (g, up) = gate_with("aaaa");
        let r = get(g.addr(), "/", None);
        assert!(r.starts_with("HTTP/1.1 200 "), "{r}");
        assert!(r.contains("<!doctype html>"), "{r}");
        // **上游一个字节都没收到。** 这一条不能靠「响应里没有上游那句回话」
        // 去反推——门口那一页里正好有个 `history.replaceState`，而它开头
        // 那两个字母就是上游回的 `hi`。问上游自己是唯一问得准的办法。
        assert!(
            up.seen.lock().unwrap().is_empty(),
            "没带钥匙的 / 不许惊动上游"
        );
        g.stop();
    }

    /// 门口那一页里必须有钥匙换 cookie 的那段脚本，且 cookie 名要跟服务端
    /// 认的那个是同一个——两处写岔了的话，页面刷新一次还是那一页，
    /// 而没有任何测试会红。
    #[test]
    fn the_door_page_hands_the_key_to_the_same_cookie_the_gate_checks() {
        let html = page(Lang::Zh);
        assert!(html.contains(COOKIE_NAME), "页面里没有服务端认的那个 cookie 名");
        assert!(html.contains("location.hash"), "页面没去读 fragment");
        // 把链接粘进已经打开着这一页的那个标签，只会改 fragment，脚本不会
        // 再跑一次——没有这个监听器，页面就纹丝不动，而那是最自然的动作。
        assert!(html.contains("hashchange"), "页面没接住 fragment 变化");
        assert!(!html.contains("__COOKIE__"), "占位符没被替换");
        assert!(!html.contains("__NEEDS_LINK__"), "占位符没被替换");
    }

    /// 门口那一页在两种语言下都得说得出话——`i18n` 那两条守卫查的是词条，
    /// 这一条查的是它真的被塞进了页面。
    #[test]
    fn the_door_page_speaks_both_languages() {
        for lang in Lang::all() {
            let html = page(*lang);
            let msg = text(Key::GateNeedsLink, *lang);
            assert!(html.contains(msg), "{lang:?} 下页面里没有那句话");
        }
    }

    #[test]
    fn the_right_key_reaches_upstream() {
        let (g, up) = gate_with("aaaa");
        let r = get(g.addr(), "/anything", Some("dct_gate=aaaa"));
        assert!(r.contains("hi"), "带对钥匙就该看到上游的回答：{r}");
        let seen = String::from_utf8_lossy(&up.seen.lock().unwrap().clone()).into_owned();
        assert!(seen.starts_with("GET /anything HTTP/1.1"), "{seen}");
        // 请求头**原样**转过去，包括 cookie：改写头就等于我们要负责重建
        // WebSocket 握手，那是另一个能出错的地方。
        assert!(seen.contains("Cookie: dct_gate=aaaa"), "{seen}");
        g.stop();
    }

    /// cookie 可以有很多个，钥匙不一定排第一；浏览器也可能分成多条
    /// `Cookie:` 头发过来。两种都得认出来。
    #[test]
    fn the_key_is_found_among_other_cookies() {
        let (g, _up) = gate_with("aaaa");
        let r = get(g.addr(), "/x", Some("theme=dark; dct_gate=aaaa; other=1"));
        assert!(r.contains("hi"), "{r}");
        let r2 = request(
            g.addr(),
            "GET /x HTTP/1.1\r\nHost: x\r\nCookie: theme=dark\r\nCookie: dct_gate=aaaa\r\n\r\n",
        );
        assert!(r2.contains("hi"), "多条 Cookie 头也要认：{r2}");
        g.stop();
    }

    /// 握手之后就是一根字节管道：`101` 之后的 WebSocket 帧不该被这道门
    /// 碰一下。用回显上游验双向对穿。
    #[test]
    fn bytes_flow_both_ways_after_the_handshake() {
        let (g, _up) = gate_with("aaaa");
        let mut s = TcpStream::connect(g.addr()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(b"GET /ws HTTP/1.1\r\nHost: x\r\nCookie: dct_gate=aaaa\r\n\r\n")
            .unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            r.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
        }
        let mut two = [0u8; 2];
        r.read_exact(&mut two).unwrap();
        assert_eq!(&two, b"hi");
        // 握手之后再往上游推一段，回显要原样回来。
        s.write_all(b"\x81\x03abc").unwrap();
        let mut back = [0u8; 5];
        r.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"\x81\x03abc");
        g.stop();
    }

    /// 上游没起来的时候不能装作没事：回 502，而且**一个字都不说是谁没起来**。
    #[test]
    fn a_dead_upstream_is_502_and_says_nothing_about_it() {
        // 绑一个端口再立刻丢掉，拿到一个几乎肯定没人监听的地址。
        let dead = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let g = serve(l, "aaaa".into(), dead, Lang::Zh);
        let r = get(g.addr(), "/x", Some("dct_gate=aaaa"));
        assert!(r.starts_with("HTTP/1.1 502 "), "{r}");
        assert!(r.ends_with("\r\n\r\n"), "502 也不许带 body：{r}");
        g.stop();
    }

    /// 一条永不结束的请求头不能把内存吃光。
    #[test]
    fn an_endless_header_is_refused() {
        let (g, _up) = gate_with("aaaa");
        let junk = "X-Pad: ".to_string() + &"a".repeat(MAX_HEAD_BYTES + 100) + "\r\n";
        let r = request(
            g.addr(),
            &format!("GET / HTTP/1.1\r\nHost: x\r\n{junk}\r\n"),
        );
        assert!(r.starts_with("HTTP/1.1 431 "), "{r}");
        g.stop();
    }

    /// 链接长什么样：钥匙在 fragment 上，不在查询串里。这一条钉的是
    /// 「不进浏览器历史、不进中间日志」那个决定本身。
    #[test]
    fn the_link_carries_the_key_in_the_fragment() {
        let l = link("http://box.example.com:7681", "deadbeef");
        assert_eq!(l, "http://box.example.com:7681/#t=deadbeef");
        assert!(!l.contains('?'), "钥匙不许进查询串：{l}");
        // 末尾多一个斜杠不该拼出两个。
        assert_eq!(link("http://x/", "k"), "http://x/#t=k");
    }

    /// 钥匙跟手机端那把不是同一把，也不共用 cookie 名。
    /// 共用的话，「换掉远程的钥匙」会顺手把手机端踢下线。
    #[test]
    fn the_gate_key_is_not_the_phone_key() {
        assert_ne!(
            crate::secrets::GATE_TOKEN_KEY,
            crate::secrets::WEB_TOKEN_KEY
        );
        assert_ne!(COOKIE_NAME, crate::web::COOKIE_NAME);
    }

    #[test]
    fn the_gate_binds_loopback_unless_you_say_otherwise() {
        // 这一条钉的是一个安全默认值：写错了就是把一个没有加密的终端挂到
        // 局域网上。改它之前先读 `GateArgs::bind` 上那段。
        let a = parse_gate_args(&[]).unwrap();
        assert_eq!(a.bind, "127.0.0.1");
        assert_eq!(a.port, 7681);
        assert_eq!(a.upstream, "127.0.0.1:7682");
        assert!(a.public_url.is_none());
        assert!(!a.link_only);
    }

    #[test]
    fn gate_args_are_parsed() {
        let v: Vec<String> = ["--bind", "0.0.0.0", "--port", "8080", "--upstream", "127.0.0.1:9", "--url", "https://x.example", "--link"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = parse_gate_args(&v).unwrap();
        assert_eq!(a.bind, "0.0.0.0");
        assert_eq!(a.port, 8080);
        assert_eq!(a.upstream, "127.0.0.1:9");
        assert_eq!(a.public_url.as_deref(), Some("https://x.example"));
        assert!(a.link_only);
    }

    #[test]
    fn bad_gate_args_say_what_is_wrong() {
        let one = |s: &str| vec![s.to_string()];
        assert!(parse_gate_args(&one("--nope")).unwrap_err().contains("--nope"));
        assert!(parse_gate_args(&one("--port")).unwrap_err().contains("要跟一个值"));
        let v: Vec<String> = ["--port", "abc"].iter().map(|s| s.to_string()).collect();
        assert!(parse_gate_args(&v).unwrap_err().contains("--port"));
    }
}
