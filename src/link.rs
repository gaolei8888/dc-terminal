//! 守护进程主动出网的那一条线。
//!
//! 家里的笔记本没有公网地址，路由器也不该为它开洞。所以方向是反的：**笔记本
//! 主动连中转**，挂一个长轮询问「有我的东西吗」，中转把手机发来的信封递下来，
//! 处理完再 POST 回去。这样两端都只需要能出网。
//!
//! # 它不是第二套分派
//!
//! 信封里装的就是界面用的那个 `Request`，处理它走的是 `daemon.rs` 里**同一个**
//! `handle`（调用方把那个闭包传进来）。手机上看到的东西必须跟桌面一致，而保证
//! 一致最省力的办法是根本不存在第二份实现——`src/web` 那条 HTTP 路走的也是这
//! 个闭包，同一条道理。
//!
//! # 为什么没有心跳
//!
//! 计划里写着「心跳 45 秒」。这里没有定时器：长轮询挂 30 秒就会返回一次，
//! 守护进程立刻再发一条，于是这条连接上每 30 秒必有一次往返，本来就短于 45 秒。
//! 再加一个心跳定时器，是两套机制在做同一件事，而两套机制会各自超时、各自
//! 重连。见 `dct_link::POLL_TIMEOUT`。
//!
//! # 它跑在自己的线程上
//!
//! 绝不能把网络 IO 放进守护进程那个 200ms 的 tick——那条线程卡一下，所有会话
//! 的画面就一起卡一下。这是 `bridge.rs` 立的规矩，这里照办。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dct_link::{
    AuthFrame, EndpointId, EndpointKind, Envelope, ErrorBody, PeersResponse, PollResponse,
    SendRequest, LINK_VERSION, PATH_ASK, PATH_PEERS, PATH_POLL, PATH_SEND, POLL_TIMEOUT,
};

use crate::proto::{ErrorCode, Request, Response};

/// 连中转要知道的一切。
#[derive(Clone, Debug)]
pub struct LinkConfig {
    /// 中转的地址，比如 `http://127.0.0.1:8787`。结尾有没有斜杠都行。
    pub base: String,
    /// 我这台电脑在中转上叫什么。
    pub endpoint: EndpointId,
    /// 中转令牌。**是一个共享的格子，不是一份拷贝**：令牌会续期，而同一张
    /// 令牌同时被轮询线程（`Link`）和往外发东西的那一侧（`mesh::net::LinkNet`）
    /// 用着。两边各拿一份 `String` 的话，续期只换得了其中一边，另一边拿着过期
    /// 的令牌被中转拒掉，而它自己的日志里只有一句 `Unauthorized`。`clone()`
    /// 一个 `LinkConfig` 得到的是同一个格子。
    pub token: Token,
    /// 连不上之后第一次重试等多久。
    pub backoff_start: Duration,
    /// 重试间隔的上限。
    pub backoff_max: Duration,
}

/// 见 `LinkConfig::token`。`Debug` 不打印内容：`LinkConfig` 会进日志和 panic
/// 信息，令牌不该跟着去。
#[derive(Clone, Default)]
pub struct Token(Arc<Mutex<String>>);

impl Token {
    pub fn new(s: impl Into<String>) -> Token {
        Token(Arc::new(Mutex::new(s.into())))
    }

    pub fn get(&self) -> String {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set(&self, s: impl Into<String>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = s.into();
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(..)")
    }
}

impl LinkConfig {
    pub fn new(base: impl Into<String>, endpoint: EndpointId, token: impl Into<String>) -> Self {
        LinkConfig {
            base: base.into(),
            endpoint,
            token: Token::new(token),
            backoff_start: Duration::from_millis(500),
            backoff_max: Duration::from_secs(30),
        }
    }

    /// 等中转开口最多等多久。
    ///
    /// **必须比中转的长轮询长**，而且这件事不能交给调用方去配：配短了的症状是
    /// 每一次轮询都在中转开口之前被自己掐断，看起来像"网络有问题"，而两边的
    /// 日志都显示自己没做错什么。所以这里是算出来的，不是填出来的。
    fn read_timeout(&self) -> Duration {
        POLL_TIMEOUT + Duration::from_secs(10)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base.trim_end_matches('/'), path)
    }

    fn auth(&self) -> AuthFrame {
        AuthFrame {
            version: LINK_VERSION,
            kind: EndpointKind::Computer,
            endpoint: self.endpoint.clone(),
            token: self.token.get(),
        }
    }
}

/// 这条线收发要用的 HTTP 客户端。读超时按 `LinkConfig::read_timeout` 算，
/// 见那边的注释。`Link` 自己用它，`peers`/`send`/`ask` 的调用方也该用它。
pub fn agent(cfg: &LinkConfig) -> ureq::Agent {
    agent_with(cfg.read_timeout())
}

/// ureq 2 把连接放回连接池时会清掉读超时，再拿出来用时不重设：只有
/// `timeout_read` 的话，一条睡眠或断网后半死的旧连接能让轮询永远卡住。
/// 整个请求的期限（`timeout`）每次都会重新设到连接上，所以两个都要。
fn agent_with(read: Duration) -> ureq::Agent {
    crate::sys::tls::agent_builder()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(read)
        .timeout_write(Duration::from_secs(30))
        .timeout(read + Duration::from_secs(10))
        .build()
}

/// 往中转发东西时能出的错。
///
/// 中转自己说的「不」（`dct_link::LinkError`，是码）跟「根本没说上话」分开：
/// 前者是对方的判断，该按码翻译给用户；后者是网络，该让连接线程退避重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    /// 中转答了，答的是这个码。
    Relay(dct_link::LinkError),
    /// 连不上、超时、或者答回来的东西看不懂。
    Unreachable,
    /// 收件地址不是合法的端点 id，这一条根本没发出去。
    BadEndpoint,
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Relay(e) => write!(f, "relay said: {e}"),
            LinkError::Unreachable => f.write_str("relay unreachable"),
            LinkError::BadEndpoint => f.write_str("bad endpoint id"),
        }
    }
}

impl std::error::Error for LinkError {}

/// 中转答非 2xx 时，body 里的码才是真话（见 `dct_link::ErrorBody`）；答不出
/// 码的一律当成没连上。
fn classify(e: ureq::Error) -> LinkError {
    match e {
        ureq::Error::Status(_, resp) => resp
            .into_json::<ErrorBody>()
            .map(|b| LinkError::Relay(b.error))
            .unwrap_or(LinkError::Unreachable),
        ureq::Error::Transport(_) => LinkError::Unreachable,
    }
}

/// 跟我同账号、此刻在线的其它端点。
pub fn peers(cfg: &LinkConfig, agent: &ureq::Agent) -> Result<Vec<EndpointId>, LinkError> {
    let resp = agent
        .post(&cfg.url(PATH_PEERS))
        .send_json(cfg.auth())
        .map_err(classify)?;
    let body: PeersResponse = resp.into_json().map_err(|_| LinkError::Unreachable)?;
    Ok(body.online)
}

/// 投一个信封，不等答复。
pub fn send(cfg: &LinkConfig, agent: &ureq::Agent, env: Envelope) -> Result<(), LinkError> {
    agent
        .post(&cfg.url(PATH_SEND))
        .send_json(SendRequest {
            auth: cfg.auth(),
            envelope: env,
        })
        .map(|_| ())
        .map_err(classify)
}

/// 投一个信封，挂着等对面配对的答复。
pub fn ask(cfg: &LinkConfig, agent: &ureq::Agent, env: Envelope) -> Result<Envelope, LinkError> {
    ask_within(cfg, agent, env, None)
}

/// 同 `ask`，但整条请求最多等 `timeout`（`None` = 用 agent 自己的读超时）。
/// 超时算 `Unreachable`：我们这边没等到，说不清是谁的问题。
pub fn ask_within(
    cfg: &LinkConfig,
    agent: &ureq::Agent,
    env: Envelope,
    timeout: Option<Duration>,
) -> Result<Envelope, LinkError> {
    let mut req = agent.post(&cfg.url(PATH_ASK));
    if let Some(t) = timeout {
        req = req.timeout(t);
    }
    let resp = req
        .send_json(SendRequest {
            auth: cfg.auth(),
            envelope: env,
        })
        .map_err(classify)?;
    resp.into_json().map_err(|_| LinkError::Unreachable)
}

/// 连不上就越等越久，连上了就归零。
///
/// 上限存在的理由是：中转可能只是重启一下，二十分钟后才重试等于让用户的手机
/// 白白多断二十分钟。上限之内一直重试的代价只是几条失败的请求。
#[derive(Debug)]
struct Backoff {
    start: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    fn new(start: Duration, max: Duration) -> Self {
        Backoff {
            start,
            max,
            next: start,
        }
    }

    /// 又失败了：这次该等多久。
    fn hit(&mut self) -> Duration {
        let now = self.next;
        self.next = (self.next * 2).min(self.max);
        now
    }

    fn reset(&mut self) {
        self.next = self.start;
    }
}

/// 分派一条请求。就是 `daemon.rs` 里那个 `handle`，包成闭包传进来。
pub type Dispatch = Arc<dyn Fn(Request) -> Response + Send + Sync>;

/// 一个信封进来，回什么 payload 出去；`None` = 什么都不回。
///
/// 比 `Dispatch` 低一层：它看得见整个信封（尤其是 `from`），所以能按来的是
/// 电脑还是手机分流（见 `daemon.rs` 里 mesh 那一段），也能**选择沉默**——
/// 验不过的电脑信封一个字都不回，免得给伪造者当探针。
pub type Handler = Arc<dyn Fn(&Envelope) -> Option<Vec<u8>> + Send + Sync>;

/// 每次轮询之前调一下，在连接线程上。给令牌续期这类「偶尔要打一次网络、
/// 但绝不能进 tick」的活用。
pub type BeforePoll = Arc<dyn Fn() + Send + Sync>;

/// 把一个 proto 分派包成 `Handler`：解码 `Request` → 分派 → 编码 `Response`。
/// 这就是 `Link::new` 原来的全部行为。
pub fn proto_handler(dispatch: Dispatch) -> Handler {
    Arc::new(move |env: &Envelope| {
        let resp = match serde_json::from_slice::<Request>(&env.payload) {
            Ok(req) => dispatch(req),
            // 跟 socket 那条路一模一样的处理（见 `daemon.rs` 的读循环）：
            // 解不出来的请求回一句 `BadRequest`，不是断线。
            Err(e) => Response::Error(ErrorCode::BadRequest(e.to_string())),
        };
        Some(serde_json::to_vec(&resp).unwrap_or_else(|e| {
            serde_json::to_vec(&Response::Error(ErrorCode::Internal(format!(
                "答复序列化失败：{e}"
            ))))
            .unwrap_or_default()
        }))
    })
}

pub struct Link {
    cfg: LinkConfig,
    agent: ureq::Agent,
    handler: Handler,
    before_poll: Option<BeforePoll>,
    stop: Arc<AtomicBool>,
}

impl Link {
    pub fn new(cfg: LinkConfig, dispatch: Dispatch) -> Self {
        Link::with_handler(cfg, proto_handler(dispatch))
    }

    pub fn with_handler(cfg: LinkConfig, handler: Handler) -> Self {
        let agent = agent(&cfg);
        Link {
            cfg,
            agent,
            handler,
            before_poll: None,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 见 `BeforePoll`。
    pub fn before_poll(mut self, f: BeforePoll) -> Self {
        self.before_poll = Some(f);
        self
    }

    /// 一直跑，直到有人叫停。
    pub fn run(&self) {
        let mut backoff = Backoff::new(self.cfg.backoff_start, self.cfg.backoff_max);
        while !self.stop.load(Ordering::Relaxed) {
            if let Some(f) = &self.before_poll {
                // 续期炸了不该把这条线一起带走：旧令牌还能用到它过期为止。
                let _ = catch_unwind(AssertUnwindSafe(|| f()));
            }
            match self.poll_once() {
                Ok(Some(env)) => {
                    backoff.reset();
                    self.answer(&env);
                }
                // 空手而归是这条链路上最正常的一件事，不是错误：立刻再来一次。
                Ok(None) => backoff.reset(),
                Err(_) => {
                    let wait = backoff.hit();
                    self.nap(wait);
                }
            }
        }
    }

    /// 问一次「有我的东西吗」。
    fn poll_once(&self) -> Result<Option<Envelope>, ()> {
        let resp = self
            .agent
            .post(&self.cfg.url(PATH_POLL))
            .send_json(self.cfg.auth())
            .map_err(|_| ())?;
        let body: PollResponse = resp.into_json().map_err(|_| ())?;
        Ok(body.envelope)
    }

    /// 处理一个信封，把答复发回去（如果有的话）。
    fn answer(&self, env: &Envelope) {
        let Some(reply) = self.reply_to(env) else {
            return;
        };
        // 发不回去就算了：对面的请求会超时，它自己会再问一次。为一条答复
        // 反复重试，只会让后面积着的请求排更久。
        let _ = send(&self.cfg, &self.agent, reply);
    }

    /// 一个信封进来，该回什么信封出去。**不碰网络**，所以可以直接测。
    fn reply_to(&self, env: &Envelope) -> Option<Envelope> {
        let payload = (self.handler)(env)?;
        Some(Envelope {
            from: self.cfg.endpoint.clone(),
            to: env.from.clone(),
            // **原样带回**：这是对面把答复和请求配起来的唯一依据。
            seq: env.seq,
            payload,
            recipients: vec![],
        })
    }

    /// 睡一会儿，但叫停了就别接着睡。
    fn nap(&self, total: Duration) {
        let slice = Duration::from_millis(50);
        let mut left = total;
        while left > Duration::ZERO && !self.stop.load(Ordering::Relaxed) {
            let this = slice.min(left);
            std::thread::sleep(this);
            left -= this;
        }
    }
}

/// 停这条线的把手。
pub struct LinkHandle {
    stop: Arc<AtomicBool>,
}

impl LinkHandle {
    /// 叫停。**不等它真的停下来**：轮询可能正挂在中转那边，最长要到读超时
    /// 才回来。守护进程退出不该被这一下拖住。
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// 把这条线起在自己的线程上。
///
/// 线程体包在 `catch_unwind` 里，规矩同 `bridge.rs::spawn`：手机通道死掉是
/// 遗憾，会话死掉是灾难，两件事绝不能连在一起。
pub fn spawn(link: Link) -> LinkHandle {
    let stop = link.stop.clone();
    std::thread::spawn(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| link.run()));
    });
    LinkHandle { stop }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Mutex;

    /// 假中转：只认这条链路要用的两条路径，行为由测试摆布。
    ///
    /// 不用真的 `dct-srv`：那会把 tokio 拖进 `dct` 的测试依赖树里，而且真中转
    /// 做不到"头几次故意失败"这种事——重连恰恰是这里最该测的。
    struct FakeSrv {
        addr: std::net::SocketAddr,
        state: Arc<Mutex<SrvState>>,
    }

    #[derive(Default)]
    struct SrvState {
        /// 下一次轮询要递下去的信封，先进先出。
        outbox: Vec<Envelope>,
        /// 收到的答复。
        got: Vec<Envelope>,
        /// 还要故意失败几次（任何路径）。
        fail: usize,
        polls: usize,
        /// 每次轮询带来的令牌，按到达顺序。
        poll_tokens: Vec<String>,
        /// `/link/peers` 答什么。
        peers: Vec<EndpointId>,
        /// `/link/ask` 答什么：`Ok` = 把这个信封当答复，`Err` = 回这个码。
        ask: Option<Result<Envelope, dct_link::LinkError>>,
        /// `/link/ask` 收到的请求。
        asked: Vec<SendRequest>,
    }

    impl FakeSrv {
        fn start() -> FakeSrv {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let state = Arc::new(Mutex::new(SrvState::default()));
            let s = state.clone();
            std::thread::spawn(move || {
                for conn in listener.incoming() {
                    let Ok(conn) = conn else { break };
                    let s = s.clone();
                    std::thread::spawn(move || serve_one(conn, s));
                }
            });
            FakeSrv { addr, state }
        }

        fn base(&self) -> String {
            format!("http://{}", self.addr)
        }
    }

    fn serve_one(mut conn: TcpStream, state: Arc<Mutex<SrvState>>) {
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.is_empty() {
            return;
        }
        let path = line.split_whitespace().nth(1).unwrap_or("").to_string();

        let mut len = 0usize;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                break;
            }
            if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; len];
        let _ = reader.read_exact(&mut body);

        let mut st = state.lock().unwrap();
        if st.fail > 0 {
            st.fail -= 1;
            // 连响应都不给，直接把连接摔上——这是中转挂掉时最像的样子。
            return;
        }

        let (code, payload) = if path == PATH_POLL {
            st.polls += 1;
            let auth: AuthFrame = serde_json::from_slice(&body).unwrap();
            st.poll_tokens.push(auth.token);
            let env = if st.outbox.is_empty() {
                None
            } else {
                Some(st.outbox.remove(0))
            };
            (
                200,
                serde_json::to_string(&PollResponse { envelope: env }).unwrap(),
            )
        } else if path == PATH_SEND {
            let req: SendRequest = serde_json::from_slice(&body).unwrap();
            st.got.push(req.envelope);
            (204, String::new())
        } else if path == PATH_PEERS {
            (
                200,
                serde_json::to_string(&PeersResponse {
                    online: st.peers.clone(),
                })
                .unwrap(),
            )
        } else if path == PATH_ASK {
            st.asked.push(serde_json::from_slice(&body).unwrap());
            match st.ask.clone() {
                Some(Ok(env)) => (200, serde_json::to_string(&env).unwrap()),
                Some(Err(e)) => (409, serde_json::to_string(&ErrorBody { error: e }).unwrap()),
                None => (404, String::new()),
            }
        } else {
            (404, String::new())
        };
        drop(st);

        let out = format!(
            "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{payload}",
            payload.len()
        );
        let _ = conn.write_all(out.as_bytes());
    }

    fn id(s: &str) -> EndpointId {
        EndpointId::new(s).unwrap()
    }

    fn letter(from: &str, to: &str, req: &Request) -> Envelope {
        Envelope {
            from: id(from),
            to: id(to),
            seq: 42,
            payload: serde_json::to_vec(req).unwrap(),
            recipients: vec![],
        }
    }

    /// 起一条线，等到假中转收下第一份答复为止。
    fn run_until_answered(srv: &FakeSrv, dispatch: Dispatch) -> Envelope {
        let cfg = LinkConfig {
            backoff_start: Duration::from_millis(20),
            backoff_max: Duration::from_millis(60),
            ..LinkConfig::new(srv.base(), id("laptop"), "t")
        };
        let handle = spawn(Link::new(cfg, dispatch));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(e) = srv.state.lock().unwrap().got.first().cloned() {
                handle.stop();
                return e;
            }
            assert!(std::time::Instant::now() < deadline, "等不到答复");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// 手机发来的请求走的必须是桌面那同一个 `handle`——这条测试盯的是"没有
    /// 第二套分派"这件事：闭包被调到了，回来的就是它的答复。
    #[test]
    fn a_request_from_the_phone_goes_through_the_dispatch_it_was_given() {
        let srv = FakeSrv::start();
        srv.state
            .lock()
            .unwrap()
            .outbox
            .push(letter("phone:1", "laptop", &Request::List));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let reply = run_until_answered(
            &srv,
            Arc::new(move |req: Request| {
                s.lock().unwrap().push(format!("{req:?}"));
                Response::Sessions(vec![])
            }),
        );

        assert_eq!(seen.lock().unwrap().len(), 1, "闭包该被调用一次");
        assert!(seen.lock().unwrap()[0].contains("List"));
        assert_eq!(reply.to, id("phone:1"), "答复要回给问的人");
        assert_eq!(reply.from, id("laptop"));
        assert_eq!(reply.seq, 42, "seq 要原样带回，手机靠它配对");
        let resp: Response = serde_json::from_slice(&reply.payload).unwrap();
        assert!(matches!(resp, Response::Sessions(_)));
    }

    /// 解不出来的请求跟 socket 那条路一样：回一句 `BadRequest`，不是断线，
    /// 也不是沉默。
    #[test]
    fn a_payload_that_is_not_a_request_comes_back_as_a_bad_request() {
        let srv = FakeSrv::start();
        srv.state.lock().unwrap().outbox.push(Envelope {
            payload: b"{not json".to_vec(),
            ..letter("phone:1", "laptop", &Request::List)
        });

        let reply = run_until_answered(&srv, Arc::new(|_| panic!("请求都解不出来，不该轮到分派")));
        let resp: Response = serde_json::from_slice(&reply.payload).unwrap();
        assert!(
            matches!(resp, Response::Error(ErrorCode::BadRequest(_))),
            "{resp:?}"
        );
    }

    /// 中转不在的时候不能就此放弃——用户的路由器重启一下，手机端就该自己回来。
    #[test]
    fn the_link_keeps_trying_after_the_relay_goes_away() {
        let srv = FakeSrv::start();
        {
            let mut st = srv.state.lock().unwrap();
            st.fail = 5; // 头五条请求连响应都没有
            st.outbox.push(letter("phone:1", "laptop", &Request::List));
        }
        let reply = run_until_answered(&srv, Arc::new(|_| Response::Sessions(vec![])));
        assert_eq!(reply.to, id("phone:1"));
    }

    #[test]
    fn backoff_grows_and_then_stops_growing() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_millis(400));
        assert_eq!(b.hit(), Duration::from_millis(100));
        assert_eq!(b.hit(), Duration::from_millis(200));
        assert_eq!(b.hit(), Duration::from_millis(400));
        assert_eq!(b.hit(), Duration::from_millis(400), "到上限就别再涨了");
        b.reset();
        assert_eq!(b.hit(), Duration::from_millis(100), "连上了要归零");
    }

    /// 第一次请求正常答完、连接留着复用；第二次请求走同一条连接，对面再也
    /// 不出声（睡眠或断网后的半死连接）。第二次必须按时放弃，不能卡死。
    #[test]
    fn a_reused_connection_that_goes_silent_is_given_up_on() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (c, _) = listener.accept().unwrap();
            let mut r = BufReader::new(c.try_clone().unwrap());
            let mut w = c;
            let read_request = |r: &mut BufReader<TcpStream>| {
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; len];
                let _ = r.read_exact(&mut body);
            };
            read_request(&mut r);
            let body = "{}";
            let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            read_request(&mut r);
            std::thread::sleep(Duration::from_secs(60));
        });
        let agent = agent_with(Duration::from_millis(300));
        let url = format!("http://{addr}/x");
        agent.post(&url).send_string("a").unwrap().into_string().unwrap();
        let t = std::time::Instant::now();
        assert!(agent.post(&url).send_string("b").is_err());
        assert!(t.elapsed() < Duration::from_secs(20), "卡了 {:?}", t.elapsed());
    }

    /// 读超时必须**长于**中转的长轮询。反过来的话每一次轮询都会被自己掐断，
    /// 症状是「怎么都连不上」而两边日志都干净。
    #[test]
    fn we_wait_longer_than_the_relay_holds_the_line() {
        let cfg = LinkConfig::new("http://x", id("laptop"), "t");
        assert!(
            cfg.read_timeout() > POLL_TIMEOUT,
            "read_timeout={:?} 不该短于 POLL_TIMEOUT={POLL_TIMEOUT:?}",
            cfg.read_timeout()
        );
    }

    /// **退避到一半也要能停下来。**
    ///
    /// 退避上限在生产里是 30 秒，所以"睡完这一觉再看要不要停"跟"停"之间差着
    /// 半分钟——用户按下关闭、界面卡住不动的那半分钟。所以这条测试特意把退避
    /// 设成远长于它给的等待时间：`run` 循环顶上那次检查在这里救不了场，只有
    /// `nap` 自己盯着叫停标志才行。
    #[test]
    fn stopping_the_link_ends_the_thread_even_mid_backoff() {
        let srv = FakeSrv::start();
        srv.state.lock().unwrap().fail = usize::MAX; // 永远连不上，一直在退避
        let cfg = LinkConfig {
            backoff_start: Duration::from_secs(5),
            backoff_max: Duration::from_secs(5),
            ..LinkConfig::new(srv.base(), id("laptop"), "t")
        };
        let link = Link::new(cfg, Arc::new(|_| Response::Sessions(vec![])));
        let stop = link.stop.clone();
        let t = std::thread::spawn(move || link.run());
        std::thread::sleep(Duration::from_millis(200));
        stop.store(true, Ordering::Relaxed);
        // 给的时间必须**短于**退避间隔，否则睡醒了自然会停，测不出东西。
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !t.is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "叫停之后线程该退出，不该等退避睡完"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// 通用的 `Handler` 那一层：回复的 `to` 是来信的 `from`，`seq` 原样带回——
    /// 电脑之间的 `ask` 靠这两样配对，跟手机那一路是同一条规矩。
    #[test]
    fn with_handler_replies_with_the_same_seq_to_the_sender() {
        let srv = FakeSrv::start();
        srv.state.lock().unwrap().outbox.push(Envelope {
            from: id("c-aaaa"),
            to: id("laptop"),
            seq: 7777,
            payload: b"ping".to_vec(),
            recipients: vec![],
        });
        let cfg = LinkConfig {
            backoff_start: Duration::from_millis(20),
            backoff_max: Duration::from_millis(60),
            ..LinkConfig::new(srv.base(), id("laptop"), "t")
        };
        let seen = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let s = seen.clone();
        let handle = spawn(Link::with_handler(
            cfg,
            Arc::new(move |env: &Envelope| {
                s.lock().unwrap().push(env.payload.clone());
                Some(b"pong".to_vec())
            }),
        ));
        let reply = wait_for(|| srv.state.lock().unwrap().got.first().cloned());
        handle.stop();
        assert_eq!(seen.lock().unwrap().as_slice(), &[b"ping".to_vec()]);
        assert_eq!(reply.to, id("c-aaaa"));
        assert_eq!(reply.from, id("laptop"));
        assert_eq!(reply.seq, 7777);
        assert_eq!(reply.payload, b"pong");
    }

    /// handler 选择沉默时一个字节都不回——验不过的电脑信封就是走这条路。
    #[test]
    fn a_handler_that_returns_none_sends_nothing_back() {
        let srv = FakeSrv::start();
        {
            let mut st = srv.state.lock().unwrap();
            st.outbox.push(letter("c-aaaa", "laptop", &Request::List));
        }
        let cfg = LinkConfig {
            backoff_start: Duration::from_millis(20),
            backoff_max: Duration::from_millis(60),
            ..LinkConfig::new(srv.base(), id("laptop"), "t")
        };
        let handle = spawn(Link::with_handler(cfg, Arc::new(|_: &Envelope| None)));
        // 等到信封被取走、再多轮询几次，确保真的没有答复在路上。
        wait_for(|| (srv.state.lock().unwrap().polls >= 3).then_some(()));
        handle.stop();
        assert!(srv.state.lock().unwrap().outbox.is_empty(), "信封该被取走");
        assert!(srv.state.lock().unwrap().got.is_empty(), "不该回任何东西");
    }

    /// 令牌续期换的是共享的那一格：轮询线程下一次就带上新令牌，不用重起。
    /// `before_poll` 也确实在连接线程上、每次轮询之前被调到。
    #[test]
    fn a_renewed_token_is_used_by_the_next_poll_and_before_poll_runs() {
        let srv = FakeSrv::start();
        let cfg = LinkConfig {
            backoff_start: Duration::from_millis(20),
            backoff_max: Duration::from_millis(60),
            ..LinkConfig::new(srv.base(), id("laptop"), "old")
        };
        let token = cfg.token.clone();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let link = Link::with_handler(cfg, Arc::new(|_: &Envelope| None)).before_poll(Arc::new(
            move || {
                if c.fetch_add(1, Ordering::SeqCst) == 1 {
                    token.set("new");
                }
            },
        ));
        let handle = spawn(link);
        wait_for(|| {
            let st = srv.state.lock().unwrap();
            st.poll_tokens.iter().any(|t| t == "new").then_some(())
        });
        handle.stop();
        let st = srv.state.lock().unwrap();
        assert_eq!(st.poll_tokens[0], "old");
        assert!(calls.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn peers_returns_what_the_relay_lists() {
        let srv = FakeSrv::start();
        srv.state.lock().unwrap().peers = vec![id("c-bbbb"), id("c-cccc")];
        let cfg = LinkConfig::new(srv.base(), id("c-aaaa"), "t");
        let got = peers(&cfg, &agent(&cfg)).unwrap();
        assert_eq!(got, vec![id("c-bbbb"), id("c-cccc")]);
    }

    #[test]
    fn ask_returns_the_reply_envelope_and_carries_auth() {
        let srv = FakeSrv::start();
        let reply = Envelope {
            from: id("c-bbbb"),
            to: id("c-aaaa"),
            seq: 5,
            payload: b"answer".to_vec(),
            recipients: vec![],
        };
        srv.state.lock().unwrap().ask = Some(Ok(reply.clone()));
        let cfg = LinkConfig::new(srv.base(), id("c-aaaa"), "tok");
        let q = Envelope {
            from: id("c-aaaa"),
            to: id("c-bbbb"),
            seq: 5,
            payload: b"question".to_vec(),
            recipients: vec![],
        };
        assert_eq!(ask(&cfg, &agent(&cfg), q.clone()).unwrap(), reply);
        let asked = srv.state.lock().unwrap().asked.clone();
        assert_eq!(asked[0].envelope, q);
        assert_eq!(asked[0].auth.token, "tok");
        assert_eq!(asked[0].auth.endpoint, id("c-aaaa"));
    }

    /// 中转说「不」的时候，调用方拿到的是它说的那个码，不是笼统的「失败」。
    #[test]
    fn a_relay_refusal_comes_back_as_its_code() {
        let srv = FakeSrv::start();
        srv.state.lock().unwrap().ask = Some(Err(dct_link::LinkError::Offline));
        let cfg = LinkConfig::new(srv.base(), id("c-aaaa"), "t");
        let q = letter("c-aaaa", "c-bbbb", &Request::List);
        assert_eq!(
            ask(&cfg, &agent(&cfg), q),
            Err(LinkError::Relay(dct_link::LinkError::Offline))
        );
    }

    #[test]
    fn a_relay_that_is_not_there_is_unreachable() {
        // 绑一个口再放掉：这个地址上保证没人听。
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let cfg = LinkConfig::new(format!("http://{addr}"), id("c-aaaa"), "t");
        let env = letter("c-aaaa", "c-bbbb", &Request::List);
        assert_eq!(send(&cfg, &agent(&cfg), env), Err(LinkError::Unreachable));
    }

    #[test]
    fn the_token_does_not_show_up_in_debug_output() {
        let cfg = LinkConfig::new("http://x", id("laptop"), "sekrit-token");
        assert!(!format!("{cfg:?}").contains("sekrit-token"));
    }

    fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(std::time::Instant::now() < deadline, "等不到");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// 地址结尾多一个斜杠不该变成 `//link/poll`。
    #[test]
    fn a_trailing_slash_in_the_address_does_not_double_up() {
        let cfg = LinkConfig::new("http://x:1/", id("laptop"), "t");
        assert_eq!(cfg.url(PATH_POLL), "http://x:1/link/poll");
    }
}
