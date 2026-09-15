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
pub use live::{Control, Live, PublicEntry};
pub mod keys;

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
    /// 当前生效的发布密钥。`None` = 没开公开功能（没带 `--publish-keys`）。
    /// 重载任务（见 `serve`）写，公开路由（`live_publish_route`）读。
    pub keys: Arc<std::sync::RwLock<Option<crate::keys::PublishKeys>>>,
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

/// 令牌头：没带、或者带了个空串，都当成没带。
///
/// 观看页在没有令牌时也会发起请求（读公开房间），一个空字符串的
/// `x-live-token` 不该因为"带了这个头"就被当成"带了令牌"去跟正确的哈希比。
fn token_header(headers: &HeaderMap) -> Option<&str> {
    header(headers, "x-live-token").filter(|t| !t.is_empty())
}

/// 证明控制房间的那个头：推帧钥匙优先，其次凭证。
///
/// 两者都没带，交给调用方按各自路由的规矩回 401——公开/取消公开这两条路
/// 跟推帧、停播一样，都要求证明控制房间。
fn control_header(headers: &HeaderMap) -> Option<crate::live::Control<'_>> {
    header(headers, "x-live-push")
        .map(crate::live::Control::Push)
        .or_else(|| header(headers, "x-live-grant").map(crate::live::Control::Grant))
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
/// **这条路由是 spec 里唯一要对公网开的东西，而这一期它没有配对身份可验。**
/// 所以它自己得带两道闸：房间总数有上限（`Live::start` 里判，回
/// `QuotaExceeded` → 429），以及按来源的建房节流（`Live::note_start`，回
/// `Busy` → 429）。两道都在，缺一道就是另一半攻击面敞着——只有总数上限，
/// 一个脚本把上限占满就能让真正的老师开不了播；只有节流，换一堆来源照样
/// 能把内存撑爆。
///
/// 节流在建房**之前**：一次会被拒的建房不该先把房间表动一遍。
async fn live_start_route(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Json(req): Json<LiveStartRequest>,
) -> Result<StatusCode, Rejected> {
    live.note_start(&client_key(&headers), Instant::now())?;
    live.start(req.id, req.viewer_token, req.push_secret, req.lanes)?;
    Ok(StatusCode::NO_CONTENT)
}

/// 建房限流按什么分桶。
///
/// **中转永远只听本机**（`must_be_loopback`），所以公网上的请求必然经过
/// 反向代理，真正的来源只在 `X-Forwarded-For` 里——取头一跳。部署文档
/// （`docs/deploy-live-relay.md`）因此把「反代必须设这个头」写成了要求
/// 而不是建议。
///
/// 两个头都没有（本机直连、单元测试）就归到同一个桶：那时候这条限流退化
/// 成「整体每分钟 N 次」，仍然是一道闸，只是不再分得清是谁。
///
/// **取最右边那一跳，不是最左边。** `X-Forwarded-For` 最左边是请求方自己
/// 写的：Caddy 按部署文档那样 `header_up X-Forwarded-For {remote_host}` 会整个
/// 覆盖掉，两种取法没区别；可 nginx 默认的 `$proxy_add_x_forwarded_for` 只是
/// 把真实来源**追加**在后面。取最左边的话，一个脚本每次换一个假前缀就是一个
/// 新桶，这道闸形同虚设。最右边那一跳是离中转最近的那个反代写的，请求方够不着。
///
/// 前面还有一层 CDN 的时候，最右边会是 CDN 的地址，所有人落进同一个桶——
/// 那是偏严，不是被绕过；那种部署该让反代把真实来源写成单值。
///
/// **这仍然不是身份**，只是一个够用的分桶依据；桶被换着花样打满时，
/// `MAX_RATE_KEYS` 那条上限接着拦。
fn client_key(headers: &HeaderMap) -> String {
    for name in ["x-forwarded-for", "x-real-ip"] {
        if let Some(raw) = header(headers, name) {
            let last = raw.rsplit(',').next().unwrap_or("").trim();
            if !last.is_empty() {
                return last.to_string();
            }
        }
    }
    "direct".to_string()
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
    // 没带令牌不再当场拒绝：公开的房间允许无令牌读帧（`Live::frame`/`authed`
    // 自己判断这场是不是公开的），私密房间照旧回 401。
    let token = token_header(&headers);
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

#[derive(serde::Serialize, serde::Deserialize)]
struct PublicTitle {
    title: String,
}

/// `GET /live/{id}/lanes` 的答复：老师起的名字，不是会话标题或项目路径——
/// 那两样一个字都不许上公网（整份 spec 的前提之一）。`viewers` 搭这班车
/// 一起回，是因为学生页开场只该拉一次这条路径，之后人数跟着帧的节奏走，
/// 不该为了一个数字单独起一条轮询。
/// 这场直播公开着的话，公开标题另起一段——`title` 只在这里出现，观众页
/// 用它跟自己已经知道的路名（`lanes`）分开显示。
#[derive(serde::Serialize, serde::Deserialize)]
struct LiveLanesResponse {
    lanes: Vec<String>,
    viewers: u32,
    /// 公开就是 `Some`，私密是 `None`。观众页拿它判断"这场直播现在是不是
    /// 公开的"——包括管理台代为公开的情况。
    public: Option<PublicTitle>,
}

/// 学生页开场问一次「这场直播上架了哪几路，叫什么名字」。
///
/// 鉴权跟取帧同一条路（`x-live-token`），带着令牌走的时候也跟取帧一样
/// **认不出来和这场直播根本不存在回同一个 401**——`Live::lanes` 内部走的
/// 是跟 `Live::frame` 同一个 `authed()`，理由写在 `live.rs` 那段注释里：
/// 分开回的话，拿一把猜的/过期的令牌把一串 live-id 挨个问一遍，就能靠
/// 错误码反推出哪个 id 现在正播着，把这条路径当探测器用。**没带令牌**
/// （或者带了个空串，见 `token_header`）只放行公开的房间。
async fn live_lanes_route(
    State(live): State<Arc<Live>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<LiveLanesResponse>, Rejected> {
    let (lanes, public) = live.lanes(&id, token_header(&headers))?;
    let viewers = live.viewers(&id);
    Ok(Json(LiveLanesResponse {
        lanes,
        viewers,
        public: public.map(|title| PublicTitle { title }),
    }))
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

/// 手机网页本体。挂在 `/phone`——`/` 现在是公开列表页（`public_page_route`）。
///
/// **跟守护进程在局域网上发的是同一份字节**（`dct_page::page()`），不是抄过来
/// 的一份。两份各自演化的网页，最贵的地方在于其中一份的 bug 只在另一种模式
/// 下才复现，而那时候没人会想到去对比两个文件。
///
/// 眼下这一页在中转上还打不开：它取数走的是守护进程那套 `/api/*`，换成信封
/// 是下一步的事。现在就把路由接上，是因为"两边发同一份"这条性质要从它有
/// 第二个服务端的第一天起就成立——补挂上去的那天，多半已经有人拷过一份了。
async fn page_route() -> axum::response::Html<&'static str> {
    axum::response::Html(dct_page::page())
}

/// 请求体：`{"title": "...", "key": "..."}`。
#[derive(serde::Deserialize)]
struct PublishRequest {
    title: String,
    key: String,
}

/// 老师（或者代他操作的管理台）公开这场直播。跟建房共用按来源的限流
/// （`Live::note_start`，同一本账）——这条路一样对公网开着、一样没有配对
/// 身份可验，攻击面跟 `POST /live/start` 一样大。
///
/// 判断顺序交给 `Live::publish`（先证明控制房间再看总开关再验发布密钥），
/// 这里只是把 HTTP 那几个头翻译成它要的参数。
async fn live_publish_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<PublishRequest>,
) -> Result<StatusCode, Rejected> {
    state.live.note_start(&client_key(&headers), Instant::now())?;
    let control = control_header(&headers).ok_or(LinkError::Unauthorized)?;
    let keys = state.keys.read().expect("keys 锁");
    state
        .live
        .publish(&id, control, keys.as_ref(), &req.key, &req.title)?;
    Ok(StatusCode::NO_CONTENT)
}

/// 取消公开。跟公开同一把控制凭据，`Live::unpublish` 本身是幂等的。
async fn live_unpublish_route(
    State(live): State<Arc<Live>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, Rejected> {
    let control = control_header(&headers).ok_or(LinkError::Unauthorized)?;
    live.unpublish(&id, control)?;
    Ok(StatusCode::NO_CONTENT)
}

/// 公开列表。谁都能读，不需要任何令牌——这就是"公开"的意思。
async fn live_public_list_route(
    State(live): State<Arc<Live>>,
) -> Json<Vec<crate::live::PublicEntry>> {
    Json(live.public_list())
}

/// 公开列表页。跟 `page_route`/`live_page_route` 同一个理由挂在这儿：
/// `dct-page` 是两边唯一的真相来源，这里只是把已经打包好的字节交给 axum。
async fn public_page_route() -> axum::response::Html<&'static str> {
    axum::response::Html(dct_page::public_page())
}

/// 直播观众页。**只读，只有中转发**——它没有局域网那一档（学生从来不在
/// 老师家的局域网里）。跟 `page_route` 同一个理由挂在这儿：`dct-page` 是
/// 两边唯一的真相来源，这里只是把已经打包好的字节交给 axum。
async fn live_page_route() -> axum::response::Html<&'static str> {
    axum::response::Html(dct_page::live_page())
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

/// 中转挂哪些路由。
///
/// **默认只有直播那一组，外加公开列表。** 配对信封那三条（`/link/*`）和
/// 手机网页（`/phone`）没有鉴权（见 [`must_be_loopback`]），以前全靠部署方
/// 在反代上写一条「只放行 `/live/*`」的白名单挡着——那是一条配置里的约定，
/// 换一份「全部转发」的反代配置它就没了，公网上的任何人就能冒充任何一台
/// 设备收发信封。现在不打开就根本不存在，安全不再取决于反代写没写对。
/// `/` 和 `GET /live/public` 是例外：公开列表本来就该谁都能读，两档都挂。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Routes {
    /// 只有 `/live/*`（含公开列表 `/`、`/live/public`）。生产上跑的就是
    /// 这一档。
    LiveOnly,
    /// 再加上 `/link/*` 和 `/phone`（手机网页）。只给本机开发、以及鉴权
    /// 落地之后用（`dct-srv --with-link`）。
    WithLink,
}

pub fn router(state: AppState, routes: Routes) -> Router {
    let app = Router::new();
    let app = match routes {
        Routes::LiveOnly => app,
        // 手机网页挂在 `/phone`，不再是 `/`——那个位置现在是公开列表页，
        // 两档都挂（见下面 `.route("/", ...)`）。
        Routes::WithLink => app
            .route("/phone", get(page_route))
            .route(PATH_POLL, post(poll_route))
            .route(PATH_SEND, post(send_route))
            .route(PATH_ASK, post(ask_route)),
    };
    app
        // 公开列表页和列表接口：谁都能读，两档都挂。
        .route("/", get(public_page_route))
        .route(dct_link::live::PATH_PUBLIC_LIST, get(live_public_list_route))
        .route(
            "/live/{id}/public",
            axum::routing::put(live_publish_route)
                .delete(live_unpublish_route)
                // 跟 `PATH_START` 一样：请求体只有一个标题和一把密钥，
                // 16 KB 绰绰有余，不该跟信封共用两兆。
                .layer(DefaultBodyLimit::max(16 * 1024)),
        )
        // 建房的请求体只有两把钥匙和几个路名，16 KB 绰绰有余。单独给它一个
        // 小上限：这是对公网开着、谁都能调的那一条，不该跟信封共用两兆。
        .route(
            dct_link::live::PATH_START,
            post(live_start_route).layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route(dct_link::live::PATH_FRAME, post(live_push_route))
        .route("/live/{id}/frame", get(live_frame_route))
        .route("/live/{id}/lanes", get(live_lanes_route))
        .route(
            "/live/{id}",
            get(live_page_route).delete(live_stop_route),
        )
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
    routes: Routes,
    keys: Option<crate::keys::KeyFile>,
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

    // 发布密钥的重载任务：没带 `--publish-keys` 就没有这个任务，公开路由
    // 读到的 `keys` 一直是 `None`（没开公开功能）。
    let shared = Arc::new(std::sync::RwLock::new(
        keys.as_ref().map(|k| k.keys().clone()),
    ));
    if let Some(mut file) = keys {
        let shared = shared.clone();
        let live = live.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(dct_link::live::PUBLISH_KEYS_RELOAD);
            loop {
                tick.tick().await;
                match file.reload_if_changed() {
                    Ok(true) => {
                        let fresh = file.keys().clone();
                        // **先换密钥，再 `reconcile`。** `live_publish_route`
                        // 在调 `Live::publish` 期间全程攥着 `keys` 的读锁；
                        // 这里的写锁因此会等到每一个正拿着旧密钥在办公开的
                        // 请求做完才能拿到手。锁一到手，新密钥立刻对之后
                        // 所有请求生效，而紧接着这一次 `reconcile` 用的正是
                        // 这份新密钥，收得掉"刚才那个请求拿着旧密钥、在写锁
                        // 排队的当口侥幸公开成功"的房间。
                        //
                        // 反过来做（先 `reconcile` 后换密钥）会漏：
                        // `reconcile` 只碰 `rooms` 那把锁，跟 `keys` 的锁毫
                        // 无关系，一放手就有窗口——一个正拿着旧读锁执行
                        // `Live::publish` 的请求会在这条窗口里用一把已经
                        // 吊销的密钥把房间发布出去，而 `reconcile` 不会
                        // 再跑第二次（下一次触发要等文件再变一次），那间
                        // 房就一直公开到自然下线为止，破了"10 秒内收回"
                        // 的承诺。
                        *shared.write().expect("keys 锁") = Some(fresh.clone());
                        live.reconcile(Some(&fresh));
                    }
                    Ok(false) => {}
                    // 坏文件：保留上一份，只记日志，见
                    // `KeyFile::reload_if_changed` 那段注释——写坏文件不能
                    // 等于吊销全部，也不能等于全部放行。
                    Err(why) => {
                        eprintln!("dct-srv：发布密钥文件没重新加载，继续用上一份：{why}")
                    }
                }
            }
        });
    }

    axum::serve(
        listener,
        router(
            AppState {
                relay,
                live,
                keys: shared,
            },
            routes,
        ),
    )
    .await
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

/// 中转的五种启动方式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cli {
    Serve {
        addr: String,
        with_link: bool,
        publish_keys: Option<std::path::PathBuf>,
    },
    KeyAdd {
        name: String,
        file: std::path::PathBuf,
    },
    KeyRevoke {
        name: String,
        file: std::path::PathBuf,
    },
    KeyList {
        file: std::path::PathBuf,
    },
    Takedown {
        id: String,
        file: std::path::PathBuf,
    },
}

/// 手写解析：五种形状，不值得为此引一个参数库。
pub fn parse_cli(args: &[String]) -> Result<Cli, String> {
    fn flag_value(args: &[String], flag: &str) -> Result<Option<std::path::PathBuf>, String> {
        match args.iter().position(|a| a == flag) {
            None => Ok(None),
            Some(i) => args
                .get(i + 1)
                .filter(|v| !v.starts_with("--"))
                .map(|v| Some(v.into()))
                .ok_or_else(|| format!("{flag} 后面要跟一个文件路径")),
        }
    }
    let need_file = |args: &[String]| {
        flag_value(args, "--file")?.ok_or_else(|| "管理命令要写明 --file <密钥文件>".to_string())
    };
    match args.first().map(String::as_str) {
        Some("key") => {
            // `args.get(2)` 直接当名字用的话，`key add --file X`（漏了名字，
            // `--file` 紧跟在 `add` 后面）会把 `--file` 当成密钥的名字，
            // 而 `--file` 后面那个真正的文件路径反而没人管——过滤掉长得
            // 像另一个 flag 的值，走到 `ok_or_else` 那句一样的「缺少名字」。
            let name = || {
                args.get(2)
                    .filter(|n| !n.starts_with("--"))
                    .cloned()
                    .ok_or_else(|| "缺少名字".to_string())
            };
            match args.get(1).map(String::as_str) {
                Some("add") => Ok(Cli::KeyAdd {
                    name: name()?,
                    file: need_file(args)?,
                }),
                Some("revoke") => Ok(Cli::KeyRevoke {
                    name: name()?,
                    file: need_file(args)?,
                }),
                Some("list") => Ok(Cli::KeyList {
                    file: need_file(args)?,
                }),
                _ => Err("用法：dct-srv key add|revoke|list ...".into()),
            }
        }
        Some("takedown") => Ok(Cli::Takedown {
            id: args
                .get(1)
                .cloned()
                .ok_or_else(|| "缺少房间号".to_string())?,
            file: need_file(args)?,
        }),
        _ => {
            let publish_keys = flag_value(args, "--publish-keys")?;
            let mut skip_next = false;
            let mut addr = None;
            for a in args {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if a == "--publish-keys" {
                    skip_next = true;
                } else if !a.starts_with("--") && addr.is_none() {
                    addr = Some(a.clone());
                }
            }
            Ok(Cli::Serve {
                addr: addr.unwrap_or_else(|| "127.0.0.1:8787".into()),
                with_link: args.iter().any(|a| a == "--with-link"),
                publish_keys,
            })
        }
    }
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
        router(
            AppState {
                relay,
                live: Arc::new(Live::new()),
                keys: Arc::new(std::sync::RwLock::new(None)),
            },
            Routes::WithLink,
        )
    }

    /// 直播路由测试的底子：一份挂着直播路由的 `Router`，和它背后那个还没
    /// 开播的 `Live`——每条测试自己 `start`，各测各的 viewer/push token。
    fn app_with_live() -> (Router, Arc<Live>) {
        let live = Arc::new(Live::new());
        // 默认那一档：生产上跑的就是它。
        let app = router(
            AppState {
                relay: Arc::new(Relay::new(cfg(200))),
                live: live.clone(),
                keys: Arc::new(std::sync::RwLock::new(None)),
            },
            Routes::LiveOnly,
        );
        (app, live)
    }

    /// 公开路由测试的底子：一场已经开播的直播，加一把能公开它的发布密钥。
    fn app_with_keys() -> (Router, Arc<Live>, String) {
        let live = Arc::new(Live::new());
        let mut k = crate::keys::PublishKeys::default();
        let key = k.add("姜老师", 0).unwrap();
        let app = router(
            AppState {
                relay: Arc::new(Relay::new(cfg(200))),
                live: live.clone(),
                keys: Arc::new(std::sync::RwLock::new(Some(k))),
            },
            Routes::LiveOnly,
        );
        live.start(
            "abc".into(),
            "t".repeat(64),
            push_secret(),
            vec!["前端".into()],
        )
        .unwrap();
        (app, live, key)
    }

    /// 发一条带任意方法/头/body 的请求，把状态码和 body 读回来。
    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String) {
        let mut req = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// **不开口子的中转，配对信封那三条和手机网页根本不存在。** 这几条路由
    /// 没有鉴权（见 `must_be_loopback`），以前全靠反代上那条「只放行
    /// `/live/*`」的白名单挡着——换一份「全部转发」的反代配置，公网上的
    /// 任何人就能冒充任何一台设备收发信封。现在不带 `--with-link` 起的中转
    /// 自己就不答这几条路。**`/` 是例外**：它是公开列表页，本来就该谁都能
    /// 看，两档都挂。
    #[tokio::test]
    async fn the_default_relay_does_not_answer_the_unauthenticated_routes() {
        let (app, _live) = app_with_live();
        for path in [PATH_POLL, PATH_SEND, PATH_ASK, "/phone"] {
            let (status, _) = post(app.clone(), path, "{}").await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path} 不该在默认中转上存在");
        }
        // `/` 是公开列表页，两档都挂。
        let root = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(root.status(), StatusCode::OK, "`/` 该在默认中转上打得开");
        // 直播那一半照常：学生页打得开
        let page = app
            .oneshot(
                Request::builder()
                    .uri("/live/abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
    }

    /// **限流按最右边那一跳分桶，不是最左边。** `X-Forwarded-For` 最左边
    /// 是请求方自己写的；nginx 默认的 `$proxy_add_x_forwarded_for` 只是把
    /// 真实来源**追加**在后面。按最左边分桶，一个脚本每次换一个假前缀就是
    /// 一个新桶，那道闸形同虚设——十几秒就能把 `MAX_ROOMS` 占满，真正的
    /// 老师开不了播。
    #[tokio::test]
    async fn a_forged_leftmost_hop_does_not_escape_the_throttle() {
        let (app, _live) = app_with_live();
        for i in 0..dct_link::live::MAX_STARTS_PER_WINDOW {
            let from = format!("10.0.0.{i}, 1.2.3.4");
            assert_eq!(post_start(&app, &format!("room{i}"), &from).await, 204);
        }
        assert_eq!(
            post_start(&app, "one-too-many", "10.9.9.9, 1.2.3.4").await,
            429,
            "换一个伪造的前缀就绕过了节流"
        );
    }

    /// 走 `POST /live/start` 开一场，带上一个来源标识（反代会设的那个头）。
    /// 回状态码。
    async fn post_start(app: &Router, id: &str, from: &str) -> u16 {
        let body = format!(
            r#"{{"id":"{id}","viewer_token":"{}","push_secret":"{}","lanes":["一路"]}}"#,
            "t".repeat(64),
            push_secret()
        );
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(dct_link::live::PATH_START)
                    .header("content-type", "application/json")
                    .header("x-forwarded-for", from)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    /// **建房这条路对公网开着，所以它自己得带一道节流闸。** 同一个来源
    /// 连着建房，超过窗口上限要回 429——不是 401（那是「你没资格」），
    /// 是「等一下再来」。
    #[tokio::test]
    async fn opening_rooms_too_fast_from_one_source_is_throttled() {
        let (app, _live) = app_with_live();
        for i in 0..dct_link::live::MAX_STARTS_PER_WINDOW {
            assert_eq!(
                post_start(&app, &format!("room{i}"), "1.2.3.4").await,
                204,
                "窗口之内该放行"
            );
        }
        assert_eq!(
            post_start(&app, "one-too-many", "1.2.3.4").await,
            429,
            "同一个来源建房没有节流"
        );
        // 换一个来源不受连累。
        assert_eq!(post_start(&app, "someone-else", "5.6.7.8").await, 204);
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

    async fn get_frame(
        app: &Router,
        id: &str,
        token: &str,
        if_none_match: Option<&str>,
    ) -> FrameResp {
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
            dct_page::page().contains(&want),
            "网页里找不到 `{want}`——信封版本改了，那一页没跟上"
        );
    }

    /// 中转发的网页必须**逐字节**等于守护进程发的那一份。
    ///
    /// 这条测试是"只有一份网页"那件事唯一的看门人。哪天有人图省事在
    /// `dct-srv` 里放一份自己的 `page.html`，它当场就红。
    ///
    /// 手机网页挂在 `/phone`——`/` 现在是公开列表页。
    #[tokio::test]
    async fn the_relay_serves_the_very_same_page_the_daemon_does() {
        let relay = Arc::new(Relay::new(cfg(50)));
        let res = app(relay)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/phone")
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
            dct_page::page().as_bytes(),
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

    /// 学生页拿到的名字必须是老师起的那几个——**不是数字，不是会话标题**。
    /// 这条直接对着 `POST /live/start` 传进去的 `lanes` 核对，钉的是
    /// "路由没有偷偷换一套名字出来"这件事。
    #[tokio::test]
    async fn the_lanes_route_returns_the_names_given_at_start() {
        let (app, live) = app_with_live();
        live.start(
            "abc".into(),
            "t".repeat(64),
            push_secret(),
            vec!["前端调试".into(), "后端接口".into()],
        )
        .unwrap();

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/live/abc/lanes")
                    .header("x-live-token", "t".repeat(64))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: LiveLanesResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.lanes, vec!["前端调试", "后端接口"]);
    }

    /// 跟取帧同一条规矩：令牌不对，连「这场直播存不存在」都不告诉他。
    #[tokio::test]
    async fn a_wrong_token_cannot_list_the_lanes() {
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
                    .method("GET")
                    .uri("/live/abc/lanes")
                    .header("x-live-token", "x".repeat(64))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
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

    /// 公开 → 列表里出现 → 无令牌能读路名（带公开标题）→ 取消公开 → 无令牌被拒。
    #[tokio::test]
    async fn publishing_over_http_end_to_end() {
        let (app, _live, key) = app_with_keys();
        let push = push_secret();
        let body = format!(r#"{{"title":"第3课","key":"{key}"}}"#);
        let ct = ("content-type", "application/json");

        let (s, _) = call(&app, "PUT", "/live/abc/public", &[ct, ("x-live-push", &push)], &body).await;
        assert_eq!(s, 204);
        let (s, list) = call(&app, "GET", "/live/public", &[], "").await;
        assert_eq!(s, 200);
        assert!(list.contains("第3课") && list.contains("前端") && !list.contains("姜老师"), "{list}");
        let (s, lanes) = call(&app, "GET", "/live/abc/lanes", &[], "").await;
        assert_eq!(s, 200, "公开房间无令牌能读路名");
        assert!(lanes.contains(r#""public":{"title":"第3课"}"#), "{lanes}");

        let (s, _) = call(&app, "DELETE", "/live/abc/public", &[("x-live-push", &push)], "").await;
        assert_eq!(s, 204);
        let (s, _) = call(&app, "GET", "/live/abc/lanes", &[], "").await;
        assert_eq!(s, 401);
    }

    #[tokio::test]
    async fn a_grant_header_can_publish() {
        let (app, _live, key) = app_with_keys();
        let grant = dct_link::live::publish_grant(&dct_link::live::push_hash(&push_secret()), "abc");
        let body = format!(r#"{{"title":"课","key":"{key}"}}"#);
        let (s, _) = call(
            &app,
            "PUT",
            "/live/abc/public",
            &[("content-type", "application/json"), ("x-live-grant", &grant)],
            &body,
        )
        .await;
        assert_eq!(s, 204);
    }

    /// 服务器没开公开功能：403；公开页照样打得开（空列表）。
    #[tokio::test]
    async fn without_a_key_file_publishing_is_forbidden_and_the_list_is_empty() {
        let (app, live) = app_with_live();
        live.start(
            "abc".into(),
            "t".repeat(64),
            push_secret(),
            vec!["前端".into()],
        )
        .unwrap();
        let body = format!(r#"{{"title":"课","key":"{}"}}"#, "0".repeat(64));
        let (s, _) = call(
            &app,
            "PUT",
            "/live/abc/public",
            &[
                ("content-type", "application/json"),
                ("x-live-push", &push_secret()),
            ],
            &body,
        )
        .await;
        assert_eq!(s, 403);
        assert_eq!(
            call(&app, "GET", "/live/public", &[], "").await,
            (200, "[]".to_string())
        );
        assert_eq!(call(&app, "GET", "/", &[], "").await.0, 200);
    }

    /// 既没带推帧钥匙也没带凭证：401，跟推帧、停播一样。
    #[tokio::test]
    async fn publishing_without_proof_of_control_is_unauthorized() {
        let (app, _live, key) = app_with_keys();
        let body = format!(r#"{{"title":"课","key":"{key}"}}"#);
        let (s, _) = call(
            &app,
            "PUT",
            "/live/abc/public",
            &[("content-type", "application/json")],
            &body,
        )
        .await;
        assert_eq!(s, 401);
    }

    /// 空的 `x-live-token` 当成没带：观看页没有令牌时不该因为发了个空头就被拒。
    #[tokio::test]
    async fn an_empty_token_header_counts_as_no_token() {
        let (app, _live, key) = app_with_keys();
        let body = format!(r#"{{"title":"课","key":"{key}"}}"#);
        let (s, _) = call(
            &app,
            "PUT",
            "/live/abc/public",
            &[
                ("content-type", "application/json"),
                ("x-live-push", &push_secret()),
            ],
            &body,
        )
        .await;
        assert_eq!(s, 204);
        assert_eq!(
            call(&app, "GET", "/live/abc/lanes", &[("x-live-token", "")], "")
                .await
                .0,
            200
        );
    }

    /// **取帧路由跟取路名同一条规矩，不止 `/lanes`。** 公开的房间没带令牌、
    /// 或者带了个空的 `x-live-token`，都该读得到帧。
    #[tokio::test]
    async fn a_published_rooms_frame_can_be_read_without_a_token_over_http() {
        let (app, live, key) = app_with_keys();
        live.push("abc", &push_secret(), 0, b"hi".to_vec()).unwrap();
        let body = format!(r#"{{"title":"课","key":"{key}"}}"#);
        let (s, _) = call(
            &app,
            "PUT",
            "/live/abc/public",
            &[
                ("content-type", "application/json"),
                ("x-live-push", &push_secret()),
            ],
            &body,
        )
        .await;
        assert_eq!(s, 204);

        assert_eq!(
            call(&app, "GET", "/live/abc/frame?lane=0", &[], "").await.0,
            200,
            "公开房间没带令牌该读得到帧"
        );
        assert_eq!(
            call(
                &app,
                "GET",
                "/live/abc/frame?lane=0",
                &[("x-live-token", "")],
                "",
            )
            .await
            .0,
            200,
            "空令牌当成没带"
        );
    }

    /// 私密房间不因为"没带令牌"就被放行——没带、带空串，取帧都得 401。
    #[tokio::test]
    async fn a_private_rooms_frame_stays_401_without_a_token_over_http() {
        let (app, live) = app_with_live();
        live.start(
            "abc".into(),
            "t".repeat(64),
            push_secret(),
            vec!["前端".into()],
        )
        .unwrap();

        assert_eq!(
            call(&app, "GET", "/live/abc/frame?lane=0", &[], "").await.0,
            401,
            "私密房间没带令牌不该放行"
        );
        assert_eq!(
            call(
                &app,
                "GET",
                "/live/abc/frame?lane=0",
                &[("x-live-token", "")],
                "",
            )
            .await
            .0,
            401,
            "空令牌不该借着私密房间照旧放行"
        );
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

    #[test]
    fn the_command_line_is_parsed_into_one_of_five_shapes() {
        let a = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(
            parse_cli(&a("")).unwrap(),
            Cli::Serve {
                addr: "127.0.0.1:8787".into(),
                with_link: false,
                publish_keys: None
            }
        );
        assert_eq!(
            parse_cli(&a("127.0.0.1:9000 --with-link --publish-keys /k.json")).unwrap(),
            Cli::Serve {
                addr: "127.0.0.1:9000".into(),
                with_link: true,
                publish_keys: Some("/k.json".into())
            }
        );
        assert_eq!(
            parse_cli(&a("key add 姜老师 --file /k.json")).unwrap(),
            Cli::KeyAdd {
                name: "姜老师".into(),
                file: "/k.json".into()
            }
        );
        assert_eq!(
            parse_cli(&a("key revoke a --file /k.json")).unwrap(),
            Cli::KeyRevoke {
                name: "a".into(),
                file: "/k.json".into()
            }
        );
        assert_eq!(
            parse_cli(&a("key list --file /k.json")).unwrap(),
            Cli::KeyList {
                file: "/k.json".into()
            }
        );
        assert_eq!(
            parse_cli(&a("takedown abc --file /k.json")).unwrap(),
            Cli::Takedown {
                id: "abc".into(),
                file: "/k.json".into()
            }
        );
        assert!(
            parse_cli(&a("key add a")).is_err(),
            "管理命令必须写明 --file"
        );
        assert!(parse_cli(&a("--publish-keys")).is_err(), "参数缺值要报错");
    }

    /// T2：漏了名字直接写 `--file`（`key add --file X`）不该把 `--file`
    /// 当成密钥的名字——那样一来真正的文件路径 `X` 就没人认领了，会往
    /// `need_file` 里再吃一次 `--file` 之后的下一个词，拼出一把名叫
    /// `"--file"` 的密钥。得报「缺少名字」，就像压根没写这个参数一样。
    #[test]
    fn key_add_without_a_name_does_not_treat_the_file_flag_as_the_name() {
        let a = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        let err = parse_cli(&a("key add --file /k.json")).unwrap_err();
        assert_eq!(err, "缺少名字", "{err}");
        let err = parse_cli(&a("key revoke --file /k.json")).unwrap_err();
        assert_eq!(err, "缺少名字", "{err}");
    }
}
