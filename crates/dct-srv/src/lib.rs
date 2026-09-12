//! 中转：把信封从一个端点搬到另一个端点，**不看里面**。
//!
//! 整个服务只有两个动作。笔记本上的守护进程挂一个长轮询问「有我的东西吗」，
//! 手机把信封 POST 过来，中转按 `to` 找到那个挂着的轮询、把信封递过去。
//! `payload` 对它自始至终是一段不透明的字节——第一期是明文 JSON，第二期是
//! 密文，而中转两期的代码完全一样。**任何一天这里出现
//! `from_slice` 把 payload 认成一个 `Request`，spec 决定一就已经破了。**
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

mod live;
pub use live::Live;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, FromRef, Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dct_link::{
    AuthFrame, EndpointId, Envelope, ErrorBody, LinkError, PollResponse, SendRequest, LINK_VERSION,
    MAX_PAYLOAD, PATH_ASK, PATH_POLL, PATH_SEND,
};
use tokio::sync::mpsc;

/// 一次长轮询最多挂多久。**这个数字两边共用**，理由见 `dct_link::POLL_TIMEOUT`
/// ——守护进程要拿它算自己的读超时，算错一边整条链路就永远连不上。
pub const DEFAULT_POLL_TIMEOUT: Duration = dct_link::POLL_TIMEOUT;

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

/// 正挂在 `/link/ask` 上等答复的人。键是「等的是谁的、哪一个 `seq`」，
/// 也就是答复信封上的 `to` 和 `seq`。
type Waiting = Mutex<HashMap<(EndpointId, u64), tokio::sync::oneshot::Sender<Envelope>>>;

pub struct Relay {
    devices: Mutex<HashMap<EndpointId, Device>>,
    waiting: Waiting,
    cfg: Config,
}

impl Relay {
    pub fn new(cfg: Config) -> Self {
        Relay {
            devices: Mutex::new(HashMap::new()),
            waiting: Mutex::new(HashMap::new()),
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

        // **先看有没有人正挂着等这一封。** 走 `/link/ask` 的手机根本不轮询，
        // 它不在设备表里；这一步要是排在在线检查后面，笔记本发回去的答复
        // 会被判成"收件人不在线"直接丢掉，而提问的人还在那头挂着。
        if let Some(tx) = self
            .waiting
            .lock()
            .expect("waiting table poisoned")
            .remove(&(req.envelope.to.clone(), req.envelope.seq))
        {
            // 送不进去只有一种可能：提问的人已经不等了（超时或者断开）。
            // 那封答复就没有意义了，丢掉是对的。
            let _ = tx.send(req.envelope.clone());
            return Ok(());
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

impl Relay {
    /// 投一个信封，挂着等配对的答复。
    ///
    /// 手机用这条：一次 fetch 一个答案，页面里因此不需要任何「哪个 seq 对应
    /// 哪个还没兑现的 Promise」的簿记——那份簿记要是放在页面里，就落在这个
    /// 仓库里唯一跑不了测试的地方。
    pub async fn ask(&self, req: &SendRequest) -> Result<Envelope, LinkError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let key = (req.envelope.from.clone(), req.envelope.seq);

        // **先挂号再投递。** 反过来的话，笔记本答得够快就会在挂号之前把答复
        // 送到，那一封找不到人等它，于是走进设备信箱再也没人取——而提问的人
        // 在这头一直挂到超时。这种 bug 只在快的机器上出现。
        self.waiting
            .lock()
            .expect("waiting table poisoned")
            .insert(key.clone(), tx);

        if let Err(e) = self.send(req) {
            self.forget(&key);
            return Err(e);
        }

        match tokio::time::timeout(self.cfg.poll_timeout, rx).await {
            Ok(Ok(env)) => Ok(env),
            // 发端没了：只可能是别处把这个挂号顶掉了（同一个 seq 被用了两次）。
            Ok(Err(_)) => {
                self.forget(&key);
                Err(LinkError::NoAnswer)
            }
            Err(_) => {
                self.forget(&key);
                Err(LinkError::NoAnswer)
            }
        }
    }

    /// 不等了。**必须清掉**，否则一个超时的提问会在表里留下一个永远没人
    /// 取走的挂号，而那正是这类表长成内存泄漏的方式。
    fn forget(&self, key: &(EndpointId, u64)) {
        self.waiting
            .lock()
            .expect("waiting table poisoned")
            .remove(key);
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
            // 504：信封投到了，是对面没在时限内回话。跟 409 分开，网页才
            // 能说两句不同的话。
            LinkError::NoAnswer => StatusCode::GATEWAY_TIMEOUT,
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

/// 中转的全部状态：配对信封那半（`relay`）和直播那半（`live`）。两者互不
/// 知道对方存在——直播的路由只碰 `live`，配对的路由只碰 `relay`；合流只是
/// 因为 axum 的 `Router` 一次只挂一份 state，`FromRef` 让各自的 handler
/// 照旧各拿各的那一半，不用互相知道。
#[derive(Clone)]
pub struct AppState {
    pub relay: Arc<Relay>,
    pub live: Arc<Live>,
}

impl FromRef<AppState> for Arc<Relay> {
    fn from_ref(state: &AppState) -> Self {
        state.relay.clone()
    }
}

impl FromRef<AppState> for Arc<Live> {
    fn from_ref(state: &AppState) -> Self {
        state.live.clone()
    }
}

/// 从请求头里取一个字符串值。取不到、或者不是合法 UTF-8，一律当作没带。
fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

/// 手写的查询串解析。不用 axum 的 `Query` 提取器——那需要额外打开一个
/// cargo feature（`query`，牵出 `serde_urlencoded`/`serde_path_to_error`），
/// 而这里只需要两个数字参数，用不上一整套 serde 反序列化。
fn parse_query(uri: &Uri) -> HashMap<String, String> {
    uri.query()
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| {
            let mut it = kv.splitn(2, '=');
            let k = it.next()?;
            if k.is_empty() {
                return None;
            }
            Some((k.to_string(), it.next().unwrap_or("").to_string()))
        })
        .collect()
}

/// `POST /live/start` 的请求体：老师开播（或者自己重开同一场）时带来的
/// 一切——两把钥匙和这场直播上架了哪几路。**这一期没有配对身份可验**，
/// 认不认这次 `start` 全靠 `Live::start` 自己那条「已存在的 id 只认原来
/// 那把 push_secret」的规矩，路由这一层不做额外校验。
#[derive(serde::Deserialize)]
struct LiveStartRequest {
    id: String,
    viewer_token: String,
    push_secret: String,
    lanes: Vec<String>,
}

/// 老师开播：把 `Live::start` 接到网上。守护进程的推帧线程在第一次推帧
/// 之前调它，好让中转认得 `viewer_token`/`push_secret` 和这场直播上架了
/// 哪几路——不然中转会把第一次推帧当成「这场直播不存在」拒收。
async fn live_start_route(
    State(live): State<Arc<Live>>,
    Json(req): Json<LiveStartRequest>,
) -> Result<StatusCode, Rejected> {
    live.start(req.id, req.viewer_token, req.push_secret, req.lanes)?;
    Ok(StatusCode::NO_CONTENT)
}

/// 老师推一帧。带的是 push secret（`x-live-push`），不是学生那把
/// viewer token——推帧是写，看帧是读，两件事不共用凭据（见
/// `live.rs` 顶上「两把钥匙，不是一把」那段）。live-id 和 lane 也在头里，
/// 不在 URL 上：一条 `PATH_FRAME` 服务所有直播，省得 URL 上再带一遍 id、
/// 多一处会对不上的地方。
async fn live_push_route(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<StatusCode, Rejected> {
    let id = header(&headers, "x-live-id").ok_or(LinkError::Unauthorized)?;
    let lane: usize = header(&headers, "x-live-lane")
        .and_then(|v| v.parse().ok())
        .ok_or(LinkError::Unauthorized)?;
    let secret = header(&headers, "x-live-push").ok_or(LinkError::Unauthorized)?;
    // 空 body（保活）也是合法的推帧，`Live::push` 自己认得出来，这里不用
    // 再另外判断一次。
    live.push(id, secret, lane, body.to_vec())?;
    Ok(StatusCode::NO_CONTENT)
}

/// 学生拉一帧。`If-None-Match` 命中回一个空的 304；`?wait=1` 时挂到换帧或
/// 超时——超时也回 304，不是错误，学生页会立刻再挂一次。
async fn live_frame_route(
    State(live): State<Arc<Live>>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, Rejected> {
    let token = header(&headers, "x-live-token").ok_or(LinkError::Unauthorized)?;
    let query = parse_query(&uri);
    let lane: usize = query.get("lane").and_then(|v| v.parse().ok()).unwrap_or(0);
    let wait = query.contains_key("wait");
    let seen: Option<u64> = header(&headers, "if-none-match")
        .and_then(|v| v.trim_matches('"').parse().ok());

    let (mut body, mut etag) = live.frame(&id, token, lane)?;
    if wait && seen == Some(etag) {
        if let Some(mut rx) = live.subscribe(&id, lane) {
            let _ = tokio::time::timeout(dct_link::live::WAIT_TIMEOUT, rx.changed()).await;
        }
        (body, etag) = live.frame(&id, token, lane)?;
    }
    if seen == Some(etag) {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    Ok((
        [
            (axum::http::header::ETAG, format!("\"{etag}\"")),
            (axum::http::header::CONTENT_ENCODING, "gzip".to_string()),
            (axum::http::header::CACHE_CONTROL, "no-store".to_string()),
        ],
        body,
    )
        .into_response())
}

/// 老师停播：整场直播连同两把钥匙一起立刻蒸发。跟推帧同一把 `x-live-push`
/// 校验——live-id 对每个学生都是已知的，停播要是只认 id，随便一个学生打开
/// devtools 发一个 `DELETE` 就能掐断全班的课，比伪造画面还省事。
async fn live_stop_route(
    State(live): State<Arc<Live>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, Rejected> {
    let secret = header(&headers, "x-live-push").ok_or(LinkError::Unauthorized)?;
    live.stop(&id, secret)?;
    Ok(StatusCode::NO_CONTENT)
}

/// 手机网页本体。
///
/// **跟守护进程在局域网上发的是同一份字节**（`dct_page::PAGE`），不是抄过来
/// 的一份。两份各自演化的网页，最贵的地方在于其中一份的 bug 只在另一种模式
/// 下才复现，而那时候没人会想到去对比两个文件。
///
/// 眼下这一页在中转上还打不开：它取数走的是守护进程那套 `/api/*`，换成信封
/// 是下一步的事。现在就把路由接上，是因为"两边发同一份"这条性质要从它有
/// 第二个服务端的第一天起就成立——补挂上去的那天，多半已经有人拷过一份了。
async fn page_route() -> axum::response::Html<&'static str> {
    axum::response::Html(dct_page::PAGE)
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

async fn ask_route(
    State(relay): State<Arc<Relay>>,
    Json(req): Json<SendRequest>,
) -> Result<Json<Envelope>, Rejected> {
    Ok(Json(relay.ask(&req).await?))
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(page_route))
        .route(PATH_POLL, post(poll_route))
        .route(PATH_SEND, post(send_route))
        .route(PATH_ASK, post(ask_route))
        .route(dct_link::live::PATH_START, post(live_start_route))
        .route(dct_link::live::PATH_FRAME, post(live_push_route))
        .route("/live/{id}/frame", get(live_frame_route))
        .route("/live/{id}", axum::routing::delete(live_stop_route))
        // base64 放大 1.33 倍，再给信封的其余字段留点空。比这还大的东西在
        // 读进内存之前就该被挡掉——`send` 里那条 `TooBig` 管的是这条线以下、
        // `MAX_PAYLOAD` 以上的部分，那部分才值得回一个说得清的错误码。
        // `MAX_FRAME_BYTES`（256KB）比这个上限小得多，直播那三条路由借用
        // 同一层就够。
        .layer(DefaultBodyLimit::max(MAX_PAYLOAD * 2 + 4096))
        .with_state(state)
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    relay: Arc<Relay>,
    live: Arc<Live>,
) -> Result<(), std::io::Error> {
    // TTL 清扫：老师断线（拔网线、合上笔记本）之后，直播连同两把钥匙要在
    // 一分钟内自己收掉，不然就是永远播着。见 `live.rs` 里 `sweep` 的注释。
    let sweeping = live.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        loop {
            tick.tick().await;
            sweeping.sweep(Instant::now());
        }
    });
    axum::serve(listener, router(AppState { relay, live })).await
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

    /// 只测配对信封那半路由时，直播那半随便配一个就够——这些既有测试不碰
    /// `Live`，用不着关心它。
    fn app(relay: Arc<Relay>) -> Router {
        router(AppState {
            relay,
            live: Arc::new(Live::new()),
        })
    }

    /// 直播路由测试的底子：一份挂着直播路由的 `Router`，和它背后那个还没
    /// 开播的 `Live`——每条测试自己 `start`，各测各的 viewer/push token。
    fn app_with_live() -> (Router, Arc<Live>) {
        let live = Arc::new(Live::new());
        let app = router(AppState {
            relay: Arc::new(Relay::new(cfg(200))),
            live: live.clone(),
        });
        (app, live)
    }

    /// 本文件里所有直播测试统一用的 push secret——固定值，因为推帧测试
    /// 只关心「带对了 push secret 能不能推上去」，不关心它具体是什么。
    fn push_secret() -> String {
        "p".repeat(64)
    }

    struct FrameResp {
        status: u16,
        etag: Option<String>,
        body: Vec<u8>,
    }

    async fn get_frame(app: &Router, id: &str, token: &str, if_none_match: Option<&str>) -> FrameResp {
        let mut req = Request::builder()
            .method("GET")
            .uri(format!("/live/{id}/frame"))
            .header("x-live-token", token);
        if let Some(tag) = if_none_match {
            req = req.header("if-none-match", format!("\"{tag}\""));
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status().as_u16();
        let etag = res
            .headers()
            .get(axum::http::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim_matches('"').to_string());
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        FrameResp { status, etag, body }
    }

    async fn push_frame(app: &Router, id: &str, lane: usize, body: Vec<u8>) -> u16 {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(dct_link::live::PATH_FRAME)
                    .header("x-live-id", id)
                    .header("x-live-lane", lane.to_string())
                    .header("x-live-push", push_secret())
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        res.status().as_u16()
    }

    /// 等一个条件成立，**必须带死线**。
    ///
    /// 不带的话，任何一个让条件永远不成立的改动都会让测试**卡死**而不是
    /// 挂掉——而卡死的测试等于没有测试：变异测试跑不完，CI 只会超时，
    /// 没有人能从那个现象看出是哪一行出了问题。这条是自己踩出来的。
    async fn until(what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready() {
            assert!(Instant::now() < deadline, "等不到{what}");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
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
        until("b 挂上轮询", || {
            relay.devices.lock().unwrap().contains_key(&id("b"))
        })
        .await;

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

    // ——— 挂着等答复 ———

    fn question(from: &str, to: &str, seq: u64) -> SendRequest {
        SendRequest {
            auth: auth(from),
            envelope: Envelope {
                from: id(from),
                to: id(to),
                seq,
                payload: b"a question".to_vec(),
                recipients: vec![],
            },
        }
    }

    /// 让一台"笔记本"挂上轮询，收到什么就回一封同 `seq` 的信。
    async fn a_laptop_that_answers(relay: Arc<Relay>, who: &'static str, body: &'static [u8]) {
        let r = relay.clone();
        tokio::spawn(async move {
            if let Ok(Some(got)) = r.poll(&auth(who)).await {
                let _ = r.send(&SendRequest {
                    auth: auth(who),
                    envelope: Envelope {
                        from: id(who),
                        to: got.from.clone(),
                        seq: got.seq,
                        payload: body.to_vec(),
                        recipients: vec![],
                    },
                });
            }
        });
        // 等它真的挂上去，否则下面那一问会撞上"从没来过"。
        until("笔记本挂上轮询", || {
            relay.devices.lock().unwrap().contains_key(&id(who))
        })
        .await;
    }

    #[tokio::test]
    async fn a_question_gets_its_answer_back_in_one_round_trip() {
        let relay = Arc::new(Relay::new(cfg(2000)));
        a_laptop_that_answers(relay.clone(), "laptop", b"the answer").await;

        let answer = relay.ask(&question("phone:1", "laptop", 7)).await.unwrap();
        assert_eq!(answer.from, id("laptop"));
        assert_eq!(answer.to, id("phone:1"));
        assert_eq!(answer.seq, 7, "答复要带着提问那个 seq 回来");
        assert_eq!(answer.payload, b"the answer");
    }

    /// **提问的人从不轮询。** 它不在设备表里，所以答复那一封必须先看挂号表
    /// 再看在线表——顺序反了的话，笔记本的答复会被判成"收件人不在线"丢掉，
    /// 而提问的人在那头一直挂到超时。
    #[tokio::test]
    async fn the_asker_never_has_to_register_as_a_device() {
        let relay = Arc::new(Relay::new(cfg(2000)));
        a_laptop_that_answers(relay.clone(), "laptop", b"ok").await;

        relay.ask(&question("phone:1", "laptop", 1)).await.unwrap();
        assert!(
            relay.devices.lock().unwrap().get(&id("phone:1")).is_none(),
            "提问的人不该因为问了一句就变成一台设备"
        );
    }

    /// 「你的电脑离线了」和「你的电脑没回话」是两句不同的话，指向两个不同的
    /// 地方。合成一句的话，一台睡过去的电脑会让用户去查网络。
    #[tokio::test]
    async fn a_computer_that_is_gone_and_one_that_is_silent_say_different_things() {
        let relay = Arc::new(Relay::new(cfg(200)));

        // 压根没来过：立刻就知道，不用挂满超时。
        let started = std::time::Instant::now();
        assert_eq!(
            relay.ask(&question("phone:1", "nobody", 1)).await,
            Err(LinkError::Offline)
        );
        assert!(
            started.elapsed() < Duration::from_millis(150),
            "对面不在线是当场就知道的事，不该挂满超时"
        );

        // 在线，但不回话。
        assert_eq!(relay.poll(&auth("mute")).await.unwrap(), None);
        assert_eq!(
            relay.ask(&question("phone:1", "mute", 2)).await,
            Err(LinkError::NoAnswer)
        );
    }

    /// 超时的提问必须把自己的挂号清掉。留着的话，这张表就是一条内存泄漏——
    /// 每一个没等到答复的请求都往里加一条，永远没人取走。
    #[tokio::test]
    async fn a_question_that_times_out_leaves_nothing_behind() {
        let relay = Arc::new(Relay::new(cfg(100)));
        assert_eq!(relay.poll(&auth("mute")).await.unwrap(), None);
        assert!(relay.ask(&question("phone:1", "mute", 1)).await.is_err());
        assert!(
            relay.waiting.lock().unwrap().is_empty(),
            "等超时了还留着挂号"
        );

        // 投不出去的那一路也一样：`send` 失败之后不能把挂号丢在表里。
        assert!(relay.ask(&question("phone:1", "nobody", 2)).await.is_err());
        assert!(
            relay.waiting.lock().unwrap().is_empty(),
            "投不出去也留了挂号"
        );
    }

    /// 答复是按 `seq` 配对的，不是按到达顺序。两个问题同时挂着、答复倒着
    /// 回来，各自也得回到各自那一头。
    #[tokio::test]
    async fn answers_find_their_own_question_even_when_they_arrive_backwards() {
        let relay = Arc::new(Relay::new(cfg(2000)));
        assert_eq!(relay.poll(&auth("laptop")).await.unwrap(), None);

        let (a, b) = (relay.clone(), relay.clone());
        let q1 = tokio::spawn(async move { a.ask(&question("phone:1", "laptop", 1)).await });
        let q2 = tokio::spawn(async move { b.ask(&question("phone:1", "laptop", 2)).await });

        // 等两个都挂上号。
        until("两个问题都挂上号", || {
            relay.waiting.lock().unwrap().len() >= 2
        })
        .await;

        // 后问的先答。
        for (seq, body) in [(2u64, &b"second"[..]), (1, &b"first"[..])] {
            relay
                .send(&SendRequest {
                    auth: auth("laptop"),
                    envelope: Envelope {
                        from: id("laptop"),
                        to: id("phone:1"),
                        seq,
                        payload: body.to_vec(),
                        recipients: vec![],
                    },
                })
                .unwrap();
        }

        assert_eq!(q1.await.unwrap().unwrap().payload, b"first");
        assert_eq!(q2.await.unwrap().unwrap().payload, b"second");
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

    /// 网页里那个 `LINK_VERSION` 必须跟这儿的一样。
    ///
    /// 那是整页唯一一个**抄过来的**数字——JS 引用不了 Rust 的常量。抄错或者
    /// 漏改的症状是：中转当场回一句 `VersionMismatch`，而手机上什么都打不开，
    /// 没有任何线索指向"页面里那个数字没跟着加一"。
    ///
    /// 这条测试写在这儿，是因为 `dct-srv` 是唯一同时够得着这一页和那个常量
    /// 的地方（`dct-page` 故意没有任何依赖）。
    #[test]
    fn the_page_speaks_the_same_envelope_version_this_relay_does() {
        let want = format!("var LINK_VERSION = {LINK_VERSION};");
        assert!(
            dct_page::PAGE.contains(&want),
            "网页里找不到 `{want}`——信封版本改了，那一页没跟上"
        );
    }

    /// 中转发的网页必须**逐字节**等于守护进程发的那一份。
    ///
    /// 这条测试是"只有一份网页"那件事唯一的看门人。哪天有人图省事在
    /// `dct-srv` 里放一份自己的 `page.html`，它当场就红。
    #[tokio::test]
    async fn the_relay_serves_the_very_same_page_the_daemon_does() {
        let relay = Arc::new(Relay::new(cfg(50)));
        let res = app(relay)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            body.as_ref(),
            dct_page::PAGE.as_bytes(),
            "中转发的网页跟守护进程发的不是同一份了"
        );
    }

    #[tokio::test]
    async fn the_two_routes_speak_the_shapes_the_other_side_expects() {
        let relay = Arc::new(Relay::new(cfg(50)));

        // 空手而归的轮询：200 + envelope 为 null，不是错误。
        let (status, body) = post(
            app(relay.clone()),
            PATH_POLL,
            &serde_json::to_string(&auth("b")).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, r#"{"envelope":null}"#);

        // 投给刚刚来过的 b：204，没有 body。
        let (status, body) = post(
            app(relay.clone()),
            PATH_SEND,
            &serde_json::to_string(&letter("a", "b", b"hi")).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(body, "");

        // 投给谁都不是的人：坏消息要带着码回来，光有状态码不够。
        let (status, body) = post(
            app(relay),
            PATH_SEND,
            &serde_json::to_string(&letter("a", "nobody", b"hi")).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body, r#"{"error":"Offline"}"#);
    }

    /// 「没回话」要以自己的码穿过 HTTP 那一层回到网页，别混进 409。
    #[tokio::test]
    async fn a_silent_computer_comes_back_as_its_own_code() {
        let relay = Arc::new(Relay::new(cfg(100)));
        assert_eq!(relay.poll(&auth("mute")).await.unwrap(), None);

        let (status, body) = post(
            app(relay),
            PATH_ASK,
            &serde_json::to_string(&question("phone:1", "mute", 1)).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(body, r#"{"error":"NoAnswer"}"#);
    }

    // ——— 直播路由 ———

    /// 学生带着上一帧的 etag 再来，没换帧就该拿到一个空的 304——老师手停
    /// 着不动的那几十秒里，两百个学生一个字节都不该传。
    #[tokio::test]
    async fn an_unchanged_frame_comes_back_as_an_empty_304() {
        let (app, live) = app_with_live();
        live.start(
            "abc".into(),
            "t".repeat(64),
            push_secret(),
            vec!["前端".into()],
        )
        .unwrap();
        live.push("abc", &push_secret(), 0, b"hello".to_vec())
            .unwrap();

        let first = get_frame(&app, "abc", &"t".repeat(64), None).await;
        assert_eq!(first.status, 200);
        let etag = first.etag.clone().expect("第一帧该带 ETag");

        let again = get_frame(&app, "abc", &"t".repeat(64), Some(&etag)).await;
        assert_eq!(again.status, 304);
        assert!(again.body.is_empty(), "304 不该带 body");
    }

    /// 认证在路由之前：错 token 连「这场直播存不存在」都不告诉他。
    #[tokio::test]
    async fn a_wrong_token_learns_nothing_about_the_live() {
        let (app, live) = app_with_live();
        live.start(
            "abc".into(),
            "t".repeat(64),
            push_secret(),
            vec!["前端".into()],
        )
        .unwrap();
        let real = get_frame(&app, "abc", &"w".repeat(64), None).await;
        let fake = get_frame(&app, "zzz", &"w".repeat(64), None).await;
        assert_eq!(real.status, 401);
        assert_eq!(fake.status, 401);
    }

    #[tokio::test]
    async fn a_frame_over_the_cap_is_refused_with_413() {
        let (app, live) = app_with_live();
        live.start(
            "abc".into(),
            "t".repeat(64),
            push_secret(),
            vec!["前端".into()],
        )
        .unwrap();
        let body = vec![b'x'; dct_link::live::MAX_FRAME_BYTES + 1];
        assert_eq!(push_frame(&app, "abc", 0, body).await, 413);
    }

    /// 学生那把钥匙推不动帧：`x-live-push` 缺失或者错都得当场 401，
    /// 不能靠 `x-live-token` 蒙混过去——推帧是写，看帧是读。
    #[tokio::test]
    async fn a_viewer_cannot_push_by_reusing_their_read_token() {
        let (app, live) = app_with_live();
        live.start(
            "abc".into(),
            "t".repeat(64),
            push_secret(),
            vec!["前端".into()],
        )
        .unwrap();
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(dct_link::live::PATH_FRAME)
                    .header("x-live-id", "abc")
                    .header("x-live-lane", "0")
                    .header("x-live-push", "t".repeat(64)) // 学生的 token，不是 push secret
                    .body(Body::from("假画面".as_bytes().to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 学生那把钥匙也停不掉直播：拿 viewer token 当 push secret 去
    /// `DELETE` 必须 401，而且直播还活着——之后还能正常取到帧。live-id
    /// 对每个学生都是已知的，停播若只认 id，随便一个学生打开 devtools
    /// 发一个 `DELETE` 就能掐断全班的课，比伪造画面还省事。
    #[tokio::test]
    async fn a_viewer_token_cannot_stop_the_live_over_http() {
        let (app, live) = app_with_live();
        live.start(
            "abc".into(),
            "t".repeat(64),
            push_secret(),
            vec!["前端".into()],
        )
        .unwrap();
        live.push("abc", &push_secret(), 0, b"hello".to_vec())
            .unwrap();

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/live/abc")
                    .header("x-live-push", "t".repeat(64)) // 学生的 token，不是 push secret
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // 直播还活着：正常取帧不受影响。
        let still_there = get_frame(&app, "abc", &"t".repeat(64), None).await;
        assert_eq!(still_there.status, 200, "假 secret 停播不该成功");
    }

    /// 中转仍然不看帧里面是什么——这是 spec 决定一在直播上的那条线。
    ///
    /// 用 `concat!` 把每个禁词拆成两半再拼，是因为这条测试本身也会被
    /// `include_str!("lib.rs")` 读进来：要是禁词整个原样写在这个文件里，
    /// 它就会命中自己这一行，红得毫无意义。
    #[test]
    fn the_relay_never_looks_inside_a_frame_either() {
        let src = concat!(include_str!("live.rs"), include_str!("lib.rs"));
        let banned = [
            concat!("Screen", "Span"),
            concat!("from_slice::<", "Request>"),
            concat!("dct", "::proto"),
        ];
        for name in banned {
            assert!(
                !src.contains(name),
                "{name} 出现在中转里——它开始认识 dct 的协议了"
            );
        }
    }
}
