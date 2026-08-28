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
use std::sync::Arc;
use std::time::Duration;

use dct_link::{
    AuthFrame, EndpointId, EndpointKind, Envelope, PollResponse, SendRequest, LINK_VERSION,
    PATH_POLL, PATH_SEND, POLL_TIMEOUT,
};

use crate::proto::{ErrorCode, Request, Response};

/// 连中转要知道的一切。
#[derive(Clone, Debug)]
pub struct LinkConfig {
    /// 中转的地址，比如 `http://127.0.0.1:8787`。结尾有没有斜杠都行。
    pub base: String,
    /// 我这台电脑在中转上叫什么。
    pub endpoint: EndpointId,
    /// 配对时拿到的令牌（任务 6 才有真的；任务 5 之前中转不验）。
    pub token: String,
    /// 连不上之后第一次重试等多久。
    pub backoff_start: Duration,
    /// 重试间隔的上限。
    pub backoff_max: Duration,
}

impl LinkConfig {
    pub fn new(base: impl Into<String>, endpoint: EndpointId, token: impl Into<String>) -> Self {
        LinkConfig {
            base: base.into(),
            endpoint,
            token: token.into(),
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
            token: self.token.clone(),
        }
    }
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

pub struct Link {
    cfg: LinkConfig,
    agent: ureq::Agent,
    dispatch: Dispatch,
    stop: Arc<AtomicBool>,
}

impl Link {
    pub fn new(cfg: LinkConfig, dispatch: Dispatch) -> Self {
        let agent = crate::sys::tls::agent_builder()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(cfg.read_timeout())
            .timeout_write(Duration::from_secs(30))
            .build();
        Link {
            cfg,
            agent,
            dispatch,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 一直跑，直到有人叫停。
    pub fn run(&self) {
        let mut backoff = Backoff::new(self.cfg.backoff_start, self.cfg.backoff_max);
        while !self.stop.load(Ordering::Relaxed) {
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

    /// 处理一个信封，把答复发回去。
    fn answer(&self, env: &Envelope) {
        let reply = self.reply_to(env);
        // 发不回去就算了：手机那边的请求会超时，它自己会再问一次。为一条答复
        // 反复重试，只会让后面积着的请求排更久。
        let _ = self.send(&reply);
    }

    /// 一个信封进来，该回什么信封出去。**不碰网络**，所以可以直接测。
    fn reply_to(&self, env: &Envelope) -> Envelope {
        let resp = match serde_json::from_slice::<Request>(&env.payload) {
            Ok(req) => (self.dispatch)(req),
            // 跟 socket 那条路一模一样的处理（见 `daemon.rs` 的读循环）：
            // 解不出来的请求回一句 `BadRequest`，不是断线。
            Err(e) => Response::Error(ErrorCode::BadRequest(e.to_string())),
        };
        let payload = serde_json::to_vec(&resp).unwrap_or_else(|e| {
            serde_json::to_vec(&Response::Error(ErrorCode::Internal(format!(
                "答复序列化失败：{e}"
            ))))
            .unwrap_or_default()
        });
        Envelope {
            from: self.cfg.endpoint.clone(),
            to: env.from.clone(),
            // **原样带回**：这是手机把答复和请求配起来的唯一依据。
            seq: env.seq,
            payload,
            recipients: vec![],
        }
    }

    fn send(&self, env: &Envelope) -> Result<(), ()> {
        self.agent
            .post(&self.cfg.url(PATH_SEND))
            .send_json(SendRequest {
                auth: self.cfg.auth(),
                envelope: env.clone(),
            })
            .map(|_| ())
            .map_err(|_| ())
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

    /// 地址结尾多一个斜杠不该变成 `//link/poll`。
    #[test]
    fn a_trailing_slash_in_the_address_does_not_double_up() {
        let cfg = LinkConfig::new("http://x:1/", id("laptop"), "t");
        assert_eq!(cfg.url(PATH_POLL), "http://x:1/link/poll");
    }
}
