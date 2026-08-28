//! 中转：把信封从一个端点搬到另一个端点，**不看里面**。
//!
//! 整个服务只有两个动作。笔记本上的守护进程挂一个长轮询问「有我的东西吗」，
//! 手机把信封 POST 过来，中转按 `to` 找到那个挂着的轮询、把信封递过去。
//! `payload` 对它自始至终是一段不透明的字节——第一期是明文 JSON，第二期是
//! 密文，而中转两期的代码完全一样。**任何一天这里出现
//! `from_slice::<Request>`，spec 决定一就已经破了。**
//!
//! # 在线是什么意思
//!
//! 最直觉的写法是「有一个挂着的轮询才算在线」，但那样会漏：守护进程收到一个
//! 信封、处理完、再发起下一次轮询，中间有一小段谁都不挂着的缝。手机的下一条
//! 请求正好落在那条缝里，就会得到一句「你的电脑离线了」——而那台电脑好好的。
//!
//! 所以在线的定义是**最近轮询过**（`presence_ttl` 之内），每台设备身上挂一个
//! 小信箱（有界 channel）。落在缝里的信封进信箱，下一次轮询立刻取走。
//!
//! 这不违反 spec 的「不排队、不落盘」：那条说的是**不给离线的人存东西**。
//! 信箱只在设备还活着的时候存在，进程一停就什么都没了，有界，满了就明说
//! （`Busy`），不落盘。
//!
//! # 第一期没有鉴权
//!
//! `token` 现在没人验（任务 5 才接 dc_classroom），所以**这个服务在第一期
//! 不能对公网开口**。这不是靠自觉：`main.rs` 直接拒绝绑非环回地址。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use dct_link::{
    AuthFrame, EndpointId, Envelope, ErrorBody, LinkError, PollResponse, SendRequest, LINK_VERSION,
    MAX_PAYLOAD, PATH_POLL, PATH_SEND,
};
use tokio::sync::mpsc;

/// 一次长轮询最多挂多久。
///
/// 30 秒是照着运营商 NAT 的回收时间挑的（spec「断线」那一节）：挂得比它久，
/// 连接会被中间的某个盒子悄悄掐掉，而两端都不知道。
pub const DEFAULT_POLL_TIMEOUT: Duration = Duration::from_secs(30);

/// 每台设备的信箱能存几个信封。
///
/// 这条链路本质是一问一答，正常情况下信箱里最多躺着一个。定成 32 是给突发
/// 留的余量；真的堆到 32 个还没人取，说明对面已经不在干活了，那时候说
/// `Busy` 比继续攒着诚实。
pub const DEFAULT_INBOX: usize = 32;

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub poll_timeout: Duration,
    pub inbox: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            poll_timeout: DEFAULT_POLL_TIMEOUT,
            inbox: DEFAULT_INBOX,
        }
    }
}

impl Config {
    /// 多久没轮询就算离线。
    ///
    /// 必须**大于**一次轮询的时长，否则一个正挂着的轮询会把自己熬成「离线」。
    /// 三倍留出了「超时返回 → 重新发起」这一趟往返，加上一次重试。
    fn presence_ttl(&self) -> Duration {
        self.poll_timeout * 3
    }
}

struct Device {
    tx: mpsc::Sender<Envelope>,
    /// 取件的一端。同一台设备同时只该有一个轮询，多出来的就在这把锁上排队。
    rx: Arc<tokio::sync::Mutex<mpsc::Receiver<Envelope>>>,
    /// 最后一次**发起**轮询的时刻。见 `Config::presence_ttl`。
    last_poll: Instant,
}

pub struct Relay {
    devices: Mutex<HashMap<EndpointId, Device>>,
    cfg: Config,
}

impl Relay {
    pub fn new(cfg: Config) -> Self {
        Relay {
            devices: Mutex::new(HashMap::new()),
            cfg,
        }
    }

    /// 出示凭据的人说得通吗。
    fn check(&self, auth: &AuthFrame) -> Result<(), LinkError> {
        if auth.version != LINK_VERSION {
            return Err(LinkError::VersionMismatch);
        }
        // TODO(任务 5)：`auth.token` 还没人验，`auth.kind` 也还没用上——它存在
        // 就是为了让那时候的中转知道该拿哪个验证器去验。在那之前这个服务只
        // 监听环回地址（见 `main.rs`）。
        Ok(())
    }

    /// 有我的东西吗。没有就挂着，挂到超时为止。
    pub async fn poll(&self, auth: &AuthFrame) -> Result<Option<Envelope>, LinkError> {
        self.check(auth)?;

        let rx = {
            let mut map = self.devices.lock().expect("device table poisoned");
            // 顺手把死掉的清了。设备数量是一个教室的量级，这点开销无所谓，
            // 而单独起一个清扫任务要多一条生命周期去管。
            let ttl = self.cfg.presence_ttl();
            map.retain(|_, d| d.last_poll.elapsed() < ttl);

            let inbox = self.cfg.inbox;
            let d = map.entry(auth.endpoint.clone()).or_insert_with(|| {
                let (tx, rx) = mpsc::channel(inbox);
                Device {
                    tx,
                    rx: Arc::new(tokio::sync::Mutex::new(rx)),
                    last_poll: Instant::now(),
                }
            });
            d.last_poll = Instant::now();
            d.rx.clone()
        };

        let mut rx = rx.lock().await;
        match tokio::time::timeout(self.cfg.poll_timeout, rx.recv()).await {
            Ok(Some(e)) => Ok(Some(e)),
            // 发件的一端没了——只可能是上面那次清扫把这台设备的条目删了。
            // 对调用方来说跟"这一轮没东西"没有区别：再来一次就是了。
            Ok(None) => Ok(None),
            Err(_) => Ok(None),
        }
    }

    /// 投一个信封。**不排队给不在线的人**，投不到就当场说。
    pub fn send(&self, req: &SendRequest) -> Result<(), LinkError> {
        self.check(&req.auth)?;

        // 信封上的寄件人必须就是出示凭据的那个人。不比这一下，任何一个连得上
        // 中转的人都能冒充别人发东西——而收件方唯一能用来判断"这是谁说的"的
        // 依据就是 `from`。
        if req.envelope.from != req.auth.endpoint {
            return Err(LinkError::Unauthorized);
        }

        if req.envelope.payload.len() > MAX_PAYLOAD {
            return Err(LinkError::TooBig);
        }

        let tx = {
            let map = self.devices.lock().expect("device table poisoned");
            match map.get(&req.envelope.to) {
                Some(d) if d.last_poll.elapsed() < self.cfg.presence_ttl() => d.tx.clone(),
                _ => return Err(LinkError::Offline),
            }
        };

        tx.try_send(req.envelope.clone()).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => LinkError::Busy,
            // 取件的一端被清扫掉了：设备已经不在了，跟从没来过一样。
            mpsc::error::TrySendError::Closed(_) => LinkError::Offline,
        })
    }
}

/// `LinkError` 怎么变成一个 HTTP 响应。
///
/// body 恒为 `ErrorBody`，**状态码只是给中间那些盒子看的**：真正的判断依据是
/// body 里那个码。两者对不上时以 body 为准——手机上要显示的话是按码选的，
/// 不是按状态码选的。
struct Rejected(LinkError);

impl IntoResponse for Rejected {
    fn into_response(self) -> Response {
        let status = match self.0 {
            LinkError::Unauthorized => StatusCode::UNAUTHORIZED,
            LinkError::VersionMismatch => StatusCode::BAD_REQUEST,
            // 409 而不是 404：这台设备存在，只是现在不在。404 会让人以为
            // 地址写错了。
            LinkError::Offline => StatusCode::CONFLICT,
            LinkError::Busy => StatusCode::TOO_MANY_REQUESTS,
            LinkError::TooBig => StatusCode::PAYLOAD_TOO_LARGE,
            LinkError::QuotaExceeded => StatusCode::TOO_MANY_REQUESTS,
            LinkError::NotYours => StatusCode::FORBIDDEN,
        };
        (status, Json(ErrorBody { error: self.0 })).into_response()
    }
}

impl From<LinkError> for Rejected {
    fn from(e: LinkError) -> Self {
        Rejected(e)
    }
}

async fn poll_route(
    State(relay): State<Arc<Relay>>,
    Json(auth): Json<AuthFrame>,
) -> Result<Json<PollResponse>, Rejected> {
    Ok(Json(PollResponse {
        envelope: relay.poll(&auth).await?,
    }))
}

async fn send_route(
    State(relay): State<Arc<Relay>>,
    Json(req): Json<SendRequest>,
) -> Result<StatusCode, Rejected> {
    relay.send(&req)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router(relay: Arc<Relay>) -> Router {
    Router::new()
        .route(PATH_POLL, post(poll_route))
        .route(PATH_SEND, post(send_route))
        // base64 放大 1.33 倍，再给信封的其余字段留点空。比这还大的东西在
        // 读进内存之前就该被挡掉——`send` 里那条 `TooBig` 管的是这条线以下、
        // `MAX_PAYLOAD` 以上的部分，那部分才值得回一个说得清的错误码。
        .layer(DefaultBodyLimit::max(MAX_PAYLOAD * 2 + 4096))
        .with_state(relay)
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    relay: Arc<Relay>,
) -> Result<(), std::io::Error> {
    axum::serve(listener, router(relay)).await
}

/// 第一期只许在环回地址上跑。
///
/// 计划把「srv 只监听内网地址」列在任务 7，但那是几天之后的事，而**现在**
/// `token` 没人验、内容没加密：对公网开口的那一刻，任何人都能冒充任何一台
/// 设备收发信封。所以这条判断写成代码而不是文档里的一句话，并且写在库里而
/// 不是 `main` 里——`main` 没法测。
///
/// 等任务 5 和第二期落地，这里换成一个要人动手打开的开关。
pub fn must_be_loopback(addr: std::net::SocketAddr) -> Result<(), String> {
    if addr.ip().is_loopback() {
        return Ok(());
    }
    Err(format!(
        "{addr} 不是环回地址。第一期的中转没有鉴权也没有加密，只能在 \
         127.0.0.1 上跑；要对外提供服务，先做完任务 5（接 dc_classroom）\
         和第二期（端到端加密）。"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use dct_link::EndpointKind;
    use tower::ServiceExt;

    fn cfg(ms: u64) -> Config {
        Config {
            poll_timeout: Duration::from_millis(ms),
            inbox: 4,
        }
    }

    fn id(s: &str) -> EndpointId {
        EndpointId::new(s).unwrap()
    }

    fn auth(who: &str) -> AuthFrame {
        AuthFrame {
            version: LINK_VERSION,
            kind: EndpointKind::Computer,
            endpoint: id(who),
            token: "t".into(),
        }
    }

    fn letter(from: &str, to: &str, payload: &[u8]) -> SendRequest {
        SendRequest {
            auth: auth(from),
            envelope: Envelope {
                from: id(from),
                to: id(to),
                seq: 1,
                payload: payload.to_vec(),
                recipients: vec![],
            },
        }
    }

    #[tokio::test]
    async fn an_envelope_reaches_a_peer_that_is_waiting() {
        let relay = Arc::new(Relay::new(cfg(2000)));

        let r = relay.clone();
        let waiting = tokio::spawn(async move { r.poll(&auth("b")).await });
        // 让 b 先真的挂上去，否则下面这一投会撞上"从没来过"。
        while relay.devices.lock().unwrap().get(&id("b")).is_none() {
            tokio::task::yield_now().await;
        }

        relay.send(&letter("a", "b", b"hello")).unwrap();

        let got = waiting.await.unwrap().unwrap().unwrap();
        assert_eq!(got.from, id("a"));
        assert_eq!(got.payload, b"hello");
    }

    /// spec「不做什么」第一条：不给不在线的人排队。**而且要说出来**——
    /// 静默丢弃会让手机一直转圈等一个永远不会来的答复。
    #[tokio::test]
    async fn sending_to_someone_who_never_showed_up_is_an_error() {
        let relay = Relay::new(cfg(50));
        assert_eq!(
            relay.send(&letter("a", "nobody", b"x")),
            Err(LinkError::Offline)
        );
    }

    /// 守护进程收完一个信封、还没发起下一次轮询，中间那条缝。落在缝里的信封
    /// 不能丢，也不能被判成"离线"——那台电脑好好的。
    #[tokio::test]
    async fn an_envelope_waits_in_the_gap_between_two_polls() {
        let relay = Arc::new(Relay::new(cfg(200)));

        // 一次什么都没等到的轮询：它建立了"这台设备在线"，然后结束了。
        assert_eq!(relay.poll(&auth("b")).await.unwrap(), None);

        // 现在没有任何轮询挂着，但 b 刚刚才来过。
        relay.send(&letter("a", "b", b"in the gap")).unwrap();

        let got = relay.poll(&auth("b")).await.unwrap().unwrap();
        assert_eq!(got.payload, b"in the gap");
    }

    /// 缝可以短，但不能无限长。停了太久的设备就是离线。
    #[tokio::test]
    async fn a_peer_that_stopped_polling_goes_offline() {
        let relay = Relay::new(cfg(50)); // presence_ttl = 150ms
        assert_eq!(relay.poll(&auth("b")).await.unwrap(), None);
        assert!(relay.send(&letter("a", "b", b"x")).is_ok());

        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(relay.send(&letter("a", "b", b"x")), Err(LinkError::Offline));
    }

    /// 长轮询挂满了什么都没等到，是这条链路上最正常的一件事。
    #[tokio::test]
    async fn a_poll_that_finds_nothing_is_not_an_error() {
        let relay = Relay::new(cfg(50));
        assert_eq!(relay.poll(&auth("b")).await, Ok(None));
    }

    /// 收件方判断"这是谁说的"只有 `from` 一个依据。
    #[tokio::test]
    async fn you_cannot_put_someone_elses_name_on_the_envelope() {
        let relay = Arc::new(Relay::new(cfg(200)));
        assert_eq!(relay.poll(&auth("b")).await.unwrap(), None);

        let mut forged = letter("a", "b", b"x");
        forged.envelope.from = id("someone-else");
        assert_eq!(relay.send(&forged), Err(LinkError::Unauthorized));
    }

    /// **中转不解析 payload。** 第二期那里是密文，里面什么字节都有。
    #[tokio::test]
    async fn the_relay_never_looks_inside_the_payload() {
        let relay = Arc::new(Relay::new(cfg(200)));
        assert_eq!(relay.poll(&auth("b")).await.unwrap(), None);

        let junk = &[0u8, 0xff, 0xfe, b'{', b'\n'];
        relay.send(&letter("a", "b", junk)).unwrap();
        let got = relay.poll(&auth("b")).await.unwrap().unwrap();
        assert_eq!(got.payload, junk);
    }

    #[tokio::test]
    async fn an_old_client_is_told_the_versions_do_not_match() {
        let relay = Relay::new(cfg(50));
        let mut old = auth("a");
        old.version = LINK_VERSION - 1;
        assert_eq!(relay.poll(&old).await, Err(LinkError::VersionMismatch));

        let mut req = letter("a", "b", b"x");
        req.auth.version = LINK_VERSION + 1;
        assert_eq!(relay.send(&req), Err(LinkError::VersionMismatch));
    }

    /// 信箱满了要说「对面忙不过来」，不能说「对面离线」——那两句话把人指向
    /// 完全不同的地方。
    #[tokio::test]
    async fn an_inbox_that_is_not_being_drained_says_busy() {
        let relay = Arc::new(Relay::new(cfg(500))); // inbox = 4
        assert_eq!(relay.poll(&auth("b")).await, Ok(None));

        for _ in 0..4 {
            relay.send(&letter("a", "b", b"x")).unwrap();
        }
        assert_eq!(relay.send(&letter("a", "b", b"x")), Err(LinkError::Busy));
    }

    // ——— 接口这一层 ———

    async fn post(app: Router, path: &str, body: &str) -> (StatusCode, String) {
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn the_two_routes_speak_the_shapes_the_other_side_expects() {
        let relay = Arc::new(Relay::new(cfg(50)));

        // 空手而归的轮询：200 + envelope 为 null，不是错误。
        let (status, body) = post(
            router(relay.clone()),
            PATH_POLL,
            &serde_json::to_string(&auth("b")).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, r#"{"envelope":null}"#);

        // 投给刚刚来过的 b：204，没有 body。
        let (status, body) = post(
            router(relay.clone()),
            PATH_SEND,
            &serde_json::to_string(&letter("a", "b", b"hi")).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(body, "");

        // 投给谁都不是的人：坏消息要带着码回来，光有状态码不够。
        let (status, body) = post(
            router(relay),
            PATH_SEND,
            &serde_json::to_string(&letter("a", "nobody", b"hi")).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body, r#"{"error":"Offline"}"#);
    }
}
