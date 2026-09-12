//! 守护进程这一侧的直播状态槽。
//!
//! **同一时刻最多一场直播**——学生看的是「老师现在在播的这一场」，不是
//! 「老师今天开过的第几场」，多场并存没有对应的界面概念，先不做。
//!
//! 这里管「有没有在播、钥匙是什么、上架了哪几路」这几件事本身；本文件
//! 下半部分是把终端画面真正推出去的那条线程。
//!
//! # 推帧线程绝不能碰 200ms 那个 tick
//!
//! 跟 `link.rs` 模块头是同一条规矩：网络 IO 一旦混进 `daemon.rs` 那个给
//! 全部会话续命的 200ms tick，一次推帧卡住就是所有会话的画面一起卡住。
//! 所以这条线程自己起（`spawn_pusher`），线程体包 `catch_unwind`——直播
//! 死掉是遗憾，会话死掉是灾难，两件事不能连在一起。
//!
//! # 中转地址从哪来（临时办法）
//!
//! 眼下 dct 还没有配对信息可用，`relay_base()` 只能从环境变量
//! `DCT_RELAY` 或者一个内置默认值里猜。**这是临时办法**：将来 dct 接上
//! dc_classroom 登录之后，中转地址该从配对结果里来，而不是从进程环境变量
//! 猜——到那一天之前，`resolve_relay_base` 是唯一定义这件事的地方。

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dct_link::live::{KEEPALIVE, MAX_FRAME_BYTES, PATH_FRAME, PUSH_INTERVAL};

use crate::proto::{LiveInfo, LiveReadiness};
use crate::pty::ScreenSpan;
use crate::session::SessionManager;

/// 一场直播的内部记录。跟 [`LiveInfo`] 分开，是因为 [`LiveInfo`] 要经手线
/// 上协议——`push_secret` 压根不许出现在那个类型里（见它上面的文档注释），
/// 而这里是 daemon 自己进程内存里的那一份原文，两者不能是同一个类型。
struct Room {
    id: String,
    token: String,
    // 眼下唯一的读者是下面的 `push_secret()`，而它本身要等推帧线程
    // （下一个任务）落地才有真正的调用点——先把字段和取用口都建好，
    // 免得那个任务一上来就要碰这个类型的私有字段。
    #[allow(dead_code)]
    push_secret: String,
    staged: Vec<(u32, String)>,
    /// 上架名单改过几次。**推帧线程靠它认出「同一场直播，但 lanes 换了」**
    /// ——`restage()` 有意保持 `id` 不变（链接不作废），可中转那边存的还是
    /// 旧的一份 lanes，非得再调一次 `POST /live/start` 才会换过来。光看
    /// `id` 的话推帧线程会以为「这一场早注册过了」，于是学生看到的路名
    /// 永远停在老师第一次上架的那一份。
    staged_rev: u64,
    /// 此刻有几个人挂在中转的长轮询上——由推帧线程搭着保活那一拍去
    /// `GET /live/{id}/lanes` 读回来。见 `LiveInfo::viewers` 的文档注释：
    /// 它是个**偏低**的近似值，不是名册。
    viewers: u32,
    /// 中转认没认得这场直播——见 `LiveInfo::readiness` 的文档注释。
    /// `start()` 时总是 `Pending`：这一刻中转还完全没听说过这个 id，
    /// 推帧线程调用 `POST /live/start` 成功之后才会翻成 `Ready`。
    readiness: LiveReadiness,
}

/// 守护进程里的直播状态槽。
pub struct LiveState {
    /// 学生链接的 origin，比如 `https://example.tzspace.cn`——`start()` 拼
    /// 链接时要用，跟中转那一侧约定好的地址由调用方传进来，这里不猜。
    ///
    /// **这个任务给的是空串。** 真正的中转地址是下一个任务（推帧线程）要
    /// 接的线——它本来就得知道中转在哪儿才能把帧 POST 过去。空串期间
    /// `start()`/`info()` 拼出来的 `url` 的 host 部分是空的，**不能直接给
    /// 学生用**；谁把这里换成真实地址，谁就接手了这条职责。
    base: String,
    room: Mutex<Option<Room>>,
}

impl LiveState {
    pub fn new(base: String) -> Self {
        Self {
            base,
            room: Mutex::new(None),
        }
    }

    /// 开一场：生成 live-id、两把钥匙、拼出学生链接。
    ///
    /// 后开的这一场**直接顶掉**前一场（如果有）——`stop()` 之外还有一条能
    /// 让上一场结束的路，避免忘了手动停播还得再点一次。
    pub fn start(&self, staged: Vec<(u32, String)>) -> LiveInfo {
        let id = new_hex(dct_link::live::LIVE_ID_LEN);
        let token = new_hex(dct_link::live::LIVE_TOKEN_LEN);
        let push_secret = new_hex(dct_link::live::LIVE_TOKEN_LEN);
        // token 放在 fragment 里：浏览器不会把 `#` 后面的东西发给服务器，
        // 它不进任何一层访问日志（中转的、反代的都不进）。
        let url = format!("{}/live/{id}#t={token}", self.base);

        let info = LiveInfo {
            id: id.clone(),
            token: token.clone(),
            url,
            staged: staged.clone(),
            viewers: 0,
            // 这一刻中转还完全不知道这场直播存在——`readiness` 如实说
            // 「还没就绪」，不能骗调用方（最终是老师那块屏幕）说链接已经
            // 能用了。见 `LiveInfo::readiness` 的文档注释。
            readiness: LiveReadiness::Pending,
        };

        *recover(self.room.lock()) = Some(Room {
            id,
            token,
            push_secret,
            staged,
            staged_rev: 0,
            viewers: 0,
            readiness: LiveReadiness::Pending,
        });

        info
    }

    /// 改上架名单，**链接一个字都不变**：同一个 id、同一把学生 token、
    /// 同一把 push_secret，只换 lanes。
    ///
    /// # 为什么这条路必须跟 `start()` 分开
    ///
    /// 老师上架「后端」、把链接发给全班之后想再加一路「前端」，按的是同一个
    /// 空格键。走 `start()` 的话每次都生成新 id、新 token——200 个学生手里
    /// 那条链接当场作废、同时掉线，而老师屏幕上什么提示都没有。改上架和
    /// 开新场是两件事，钥匙换不换是它们唯一的区别，所以它们必须是两条
    /// 不同的请求。
    ///
    /// 中转那一侧本来就支持这件事：`Live::start` 带同一把 push_secret 重开
    /// 会替换 lanes 而不是拒绝（`starting_again_with_the_same_secret_replaces_the_lanes`
    /// 钉着）。这里把 `staged_rev` 加一，推帧线程看到它就会重新注册一次。
    ///
    /// `readiness` 退回 `Pending`：新的 lanes 这一刻中转还不知道，说
    /// `Ready` 就是骗老师。没在播时回 `None`——没有房间可改，调用方
    /// （`daemon.rs::live_restage`）要把它变成一句说得清的错误。
    pub fn restage(&self, staged: Vec<(u32, String)>) -> Option<LiveInfo> {
        let mut guard = recover(self.room.lock());
        let room = guard.as_mut()?;
        room.staged = staged;
        room.staged_rev += 1;
        room.readiness = LiveReadiness::Pending;
        Some(LiveInfo {
            id: room.id.clone(),
            token: room.token.clone(),
            url: format!("{}/live/{}#t={}", self.base, room.id, room.token),
            staged: room.staged.clone(),
            viewers: room.viewers,
            readiness: room.readiness.clone(),
        })
    }

    pub fn stop(&self) {
        *recover(self.room.lock()) = None;
    }

    /// 没在播的时候：`id`/`token`/`url` 都是空串，`staged` 是空表，
    /// `viewers` 是 0。**空串而不是 `Option`**——`LiveInfo` 是要经过协议线
    /// 的形状，`Option<LiveInfo>` 才是「有没有在播」该长的样子，但这一层
    /// 就要先定下「没有」具体长什么样，好让界面不用先判断一次「有没有这个
    /// 字段」才能往下渲染；空串本身就是一个不会被误认成真实 id/链接的值。
    /// 没在播时观众数当然是 0；在播时答的是推帧线程最近一次从中转读回来
    /// 的那个数（见 `LiveInfo::viewers`）。
    pub fn info(&self) -> LiveInfo {
        match &*recover(self.room.lock()) {
            Some(room) => LiveInfo {
                id: room.id.clone(),
                token: room.token.clone(),
                url: format!("{}/live/{}#t={}", self.base, room.id, room.token),
                staged: room.staged.clone(),
                viewers: room.viewers,
                readiness: room.readiness.clone(),
            },
            None => LiveInfo {
                id: String::new(),
                token: String::new(),
                url: String::new(),
                staged: Vec::new(),
                viewers: 0,
                readiness: LiveReadiness::Pending,
            },
        }
    }

    /// 推帧线程告诉中转「这场直播存在」成功之后调这个方法，把 `readiness`
    /// 翻成 `Ready`。**只在 `id` 还对得上时才生效**：推帧线程这次
    /// `POST /live/start` 可能是对着一场已经被 `stop()`/被新的一场顶掉的
    /// 直播做的（网络慢、正好撞上老师连点两下），这时候写回来的「已就绪」
    /// 属于一场已经不存在的直播，不能套到当前这一场头上。
    ///
    /// **`rev` 也要对得上**：`restage()` 保持 `id` 不变只换 lanes，光比 id
    /// 的话，一次针对旧 lanes 的 `POST /live/start` 在改上架之后才回来，
    /// 就会把「中转已认得新 lanes」这句假话写上去。
    pub(crate) fn mark_ready(&self, id: &str, rev: u64) {
        if let Some(room) = recover(self.room.lock()).as_mut() {
            if room.id == id && room.staged_rev == rev {
                room.readiness = LiveReadiness::Ready;
            }
        }
    }

    /// 同 [`Self::mark_ready`]，但这次是失败——`reason` 是一句已经本地化
    /// 过的人话，不是原始错误文本。
    pub(crate) fn mark_failed(&self, id: &str, rev: u64, reason: String) {
        if let Some(room) = recover(self.room.lock()).as_mut() {
            if room.id == id && room.staged_rev == rev {
                room.readiness = LiveReadiness::Failed(reason);
            }
        }
    }

    /// 推帧线程从中转读回来的在看人数，写回这一场。**只在 `id` 还对得上
    /// 时才生效**，理由同 [`Self::mark_ready`]：这次读到的数属于哪一场，
    /// 网络回来之后已经不一定还是当前这一场了。
    ///
    /// 不比 `staged_rev`：人数跟上架了哪几路无关，改一次名单不该把刚读到
    /// 的人数扔掉。
    pub(crate) fn set_viewers(&self, id: &str, viewers: u32) {
        if let Some(room) = recover(self.room.lock()).as_mut() {
            if room.id == id {
                room.viewers = viewers;
            }
        }
    }

    /// 老师那把推帧/停播用的钥匙。**`pub(crate)`，不经过协议**——取用者是
    /// 跟 `LiveState` 活在同一个进程里的推帧线程，它没有理由绕道
    /// `Request`/`Response` 去问 daemon 自己已经握在手里的东西；一旦这条
    /// 路走了协议，`push_secret` 就会跟着 `Response::Live` 一起发到手机
    /// 网页上，等于把它交给了每一个能读状态的人（详见 `LiveInfo` 上的
    /// 文档注释）。没在播就是 `None`。
    #[allow(dead_code)]
    pub(crate) fn push_secret(&self) -> Option<String> {
        recover(self.room.lock())
            .as_ref()
            .map(|r| r.push_secret.clone())
    }

    /// 推帧线程一轮要用到的全部东西，一次锁拿全。**不能分两次读**（先问
    /// `id`、再问 `push_secret`）：`stop()` 可能恰好插在两次读之间，读出
    /// 一个已经不存在的房间的 id 配一把新房间的钥匙，推出去的帧会被中转
    /// 用「认不出这把钥匙」拒收，而日志上看起来像是钥匙坏了。
    fn snapshot(&self) -> Option<RoomSnapshot> {
        recover(self.room.lock()).as_ref().map(|r| RoomSnapshot {
            id: r.id.clone(),
            token: r.token.clone(),
            push_secret: r.push_secret.clone(),
            staged: r.staged.clone(),
            staged_rev: r.staged_rev,
        })
    }

    /// 中转的地址，给推帧线程拼 URL 用。
    fn base(&self) -> &str {
        &self.base
    }
}

/// `LiveState::snapshot()` 的返回形状——推帧线程要知道的这一场直播的
/// 全部内容。
struct RoomSnapshot {
    id: String,
    /// 学生那把只读的钥匙——`start_room` 拼 `/live/start` 的请求体要用，
    /// 中转靠它认学生的读请求（`x-live-token`）。
    token: String,
    push_secret: String,
    staged: Vec<(u32, String)>,
    /// 见 `Room::staged_rev`：推帧线程要靠它认出「同一场直播，但 lanes
    /// 换了」，光比 `id` 的话改上架永远传不到中转。
    staged_rev: u64,
}

/// 生成 `hex_len` 个十六进制字符的随机串，取自系统 CSPRNG。**不是**时间戳、
/// **不是**引入 `rand`——两把直播钥匙和 live-id 都是能让人推帧/停播/看到别
/// 人直播的凭据，可预测的来源在这里是安全缺陷，不是实现细节。做法照抄
/// `web::mod::new_token`。
fn new_hex(hex_len: usize) -> String {
    let mut raw = vec![0u8; hex_len / 2];
    getrandom::getrandom(&mut raw).expect("系统随机数不可用，没法生成直播钥匙");
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

/// 拿到一把互斥锁；中毒（另一个持锁线程 panic 了）也要能继续用——直播状态
/// 槽跟手机通知那份共享状态是同一个道理，守护进程活得久，不能因为一次
/// panic 就让这个槽永远锁死。
fn recover<T>(r: std::sync::LockResult<T>) -> T {
    r.unwrap_or_else(|e| e.into_inner())
}

/// 环境变量没设的时候，中转用这个地址。**改这个值就是改中转地址唯一
/// 该改的地方**——见模块头「中转地址从哪来」那一段。
const DEFAULT_RELAY_BASE: &str = "https://link.tzspace.cn";

/// 覆盖中转地址用的环境变量名。
const RELAY_ENV: &str = "DCT_RELAY";

/// 中转地址该是什么。**纯函数，好测**：真正去读环境变量的是
/// [`relay_base`]，这里只管「给了什么输入、该给什么输出」。
///
/// 尾部斜杠有没有都要能用，做法照抄 `link.rs::LinkConfig::url()`：在这里
/// 一次性去掉，下游拼 `{base}{path}` 就不用各自再处理一遍——两处各自
/// trim 是「一边改了另一边没改」的那种 bug。空字符串当成没设（`DCT_RELAY=`
/// 这种半吊子配置不该悄悄指向一个空 host）。
fn resolve_relay_base(env: Option<&str>) -> String {
    let raw = env.filter(|s| !s.is_empty()).unwrap_or(DEFAULT_RELAY_BASE);
    raw.trim_end_matches('/').to_string()
}

/// 中转地址：`DCT_RELAY` 环境变量优先，没设就用内置默认值。
///
/// **临时办法**——见模块头「中转地址从哪来」那一段：将来 dct 接上
/// dc_classroom 登录之后，这个值该从配对结果里来，而不是从进程环境变量猜。
pub fn relay_base() -> String {
    resolve_relay_base(std::env::var(RELAY_ENV).ok().as_deref())
}

/// 该不该推这一帧。**纯函数，好测**——推帧线程剩下的部分全是 IO。
fn should_push(last: Option<u64>, now: u64, since: Duration) -> bool {
    match last {
        None => true,
        Some(h) if h != now => true,
        // 画面没变，但太久没说话了：中转按 `LIVE_TTL` 回收，保活是这条
        // 链路上唯一阻止「老师想事情想久了直播自己断掉」的东西。
        _ => since >= KEEPALIVE,
    }
}

/// 序列化 + gzip 压过。**纯函数**：不摸网络，一屏终端能压成多小、压没压
/// 起作用，直接拿这个函数测就行。
fn frame_of(lines: &[Vec<ScreenSpan>]) -> Vec<u8> {
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;
    let json = serde_json::to_vec(lines).unwrap_or_default();
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    let _ = gz.write_all(&json);
    gz.finish().unwrap_or_default()
}

/// 这一屏现在长什么样，压成一个能比较的数。
///
/// **哈希的是压缩前的 JSON，不是 `frame_of` 的输出**——不是因为 gzip 头
/// 的时间戳（本仓库锁定的 `flate2` + `rust_backend` 组合下 mtime 恒为 0，
/// 同一份输入两次压缩字节完全相同，压缩后的字节其实也能拿来比较），而是
/// 不想让「画面变没变」这个判断依赖某个压缩库版本/后端的实现细节——
/// 哪天 `flate2` 的默认行为变了（比如换后端、换了 mtime 策略），这里不该
/// 跟着遭殃。`ScreenSpan`/`ScreenStyle` 没派生 `Hash`（它们是协议类型，
/// 不该为了这一处内部用途多背一个 derive），序列化成 JSON 再哈希顺带绕开
/// 了这件事，代价是每一路每一轮多算一次 JSON——量级上跟 `frame_of` 自己
/// 那次持平，换一次画面不变时省下的整条网络请求，这笔账划算。
fn hash_of(lines: &[Vec<ScreenSpan>]) -> u64 {
    let json = serde_json::to_vec(lines).unwrap_or_default();
    let mut h = DefaultHasher::new();
    json.hash(&mut h);
    h.finish()
}

/// 停这条推帧线程的把手。
pub struct PusherHandle {
    stop: Arc<AtomicBool>,
}

impl PusherHandle {
    /// 叫停。**不等它真的停下来**——理由同 `link.rs::LinkHandle::stop`：
    /// 它可能正卡在一次 HTTP 请求的超时里，最长要等到那个超时才会退出，
    /// 守护进程退出不该被这一下拖住。
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// 起推帧线程，自己的线程——绝不能进 `daemon.rs` 那个 200ms 的 tick，
/// 理由见模块头。线程体包在 `catch_unwind` 里：直播死掉是遗憾，会话死掉
/// 是灾难，两件事不能连在一起（同 `link.rs::spawn`）。
///
/// 只起一条：同一时刻最多一场直播（见 `LiveState` 上的文档注释），这条
/// 线程整个守护进程生命周期里只需要有一个，它自己每一轮去问「现在在播
/// 哪一场」，没有播就什么也不做。
///
/// 返回的 `PusherHandle` 在生产唯一的调用点（`daemon.rs::run_with_manager`）
/// **有意具名丢弃**：这条线程本就该活到进程退出，没有谁会在运行中把它
/// 叫停——`stop()` 存在只是为了让测试能在一次 `cargo test` 里干净地结束
/// 这条线程，不是给生产代码用的开关。
pub fn spawn_pusher(live: Arc<LiveState>, mgr: Arc<SessionManager>) -> PusherHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    std::thread::spawn(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| pusher_loop(&live, &mgr, &st)));
    });
    PusherHandle { stop }
}

/// 推帧线程的主循环。**IO 薄薄一层**：该不该推（`should_push`）、帧长什么
/// 样（`frame_of`/`hash_of`）、一路该怎么处理一次尝试（`attempt_lane`）都
/// 是能脱离整条线程单独测的函数，这里只负责醒过来、读一次会话屏幕、把
/// 结果记回 `lanes`。
fn pusher_loop(live: &LiveState, mgr: &SessionManager, stop: &AtomicBool) {
    let agent = crate::sys::tls::agent_builder()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .build();
    let base = live.base().to_string();

    // 每一路各自的「上次**成功推送**的哈希、时间」，按会话 id 记账（不是
    // 下标）：一场直播里 `staged` 的顺序定了就不会再变，但用 id 对齐比
    // 信任下标稳——万一将来上架顺序会变，这里不用跟着改。
    let mut lanes: HashMap<u32, (u64, Instant)> = HashMap::new();
    // 上一轮看到的房间，停播时要靠它才知道该对哪个 id、拿哪把钥匙发
    // `DELETE`——那一轮 `snapshot()` 已经答不出来了。
    let mut last_room: Option<(String, String)> = None;
    // **成功** `start_room` 过的那个 id。跟 `last_room` 分开是因为它们回答
    // 不同的问题：`last_room` 是「上一轮看到的是哪一场」，这个是「中转
    // 那边是不是真的已经认得这一场」——一次 `/live/start` 失败之后
    // `last_room` 照样会更新（下一轮还得知道找谁 DELETE），但绝不能把
    // 这个也标记成功，否则就是本节点自己骗自己「已经开播了」。
    //
    // **记的是 `(id, staged_rev)` 而不是光一个 id**：`restage()` 改上架时
    // 有意保持 id 不变（链接不作废），可中转那边存的还是旧的一份 lanes，
    // 非得再调一次 `POST /live/start` 才换得过来。只比 id 的话，学生看到
    // 的路名会永远停在老师第一次上架的那一份。
    let mut started: Option<(String, u64)> = None;
    // 上次去中转读「几个人在看」是什么时候。**搭在保活那一拍上，不另开
    // 一条轮询**：这个数不需要比保活更快——老师要的是「有没有人、大概
    // 多少人」，而多一条按 `PUSH_INTERVAL` 跑的请求，200 人的课上就是
    // 中转每 500ms 多挨一次问。`None` = 还没读过，下一轮立刻读一次。
    let mut viewers_at: Option<Instant> = None;

    while !stop.load(Ordering::Relaxed) {
        match live.snapshot() {
            Some(room) => {
                // 新的一场（或者中转还没答应过这一场）：先注册，成功之前
                // 绝不推帧——推了也是白推，中转会把它当成「这场直播不
                // 存在」拒收，而更要紧的是：**不能把「本地生成了 id、
                // 拼好了链接」悄悄当成「学生的链接已经能打开」**。失败就
                // 原地留着，下一轮 `PUSH_INTERVAL` 自动重试；`LiveInfo`
                // 那侧的 `readiness` 字段能让老师那块屏幕如实反映这件事，
                // 而不是无条件显示"已经能用了"。
                if started.as_ref() != Some(&(room.id.clone(), room.staged_rev)) {
                    match start_room(&agent, &base, &room) {
                        Ok(()) => {
                            live.mark_ready(&room.id, room.staged_rev);
                            started = Some((room.id.clone(), room.staged_rev));
                            // 新的一场（或者换了上架名单），旧的哈希对不上号，
                            // 从头判断该不该推。
                            lanes.clear();
                        }
                        Err(reason) => {
                            live.mark_failed(&room.id, room.staged_rev, reason);
                            nap(stop, PUSH_INTERVAL);
                            continue;
                        }
                    }
                }
                last_room = Some((room.id.clone(), room.push_secret.clone()));
                // 「N 人在看」是 spec 四道防线里的第二道——它必须是活的：
                // 一个恒为 0 的数字比没有这个数字更糟，老师会据此以为没人
                // 在看。读不到就留着上一次的值，不归零。
                if viewers_at.is_none_or(|t| t.elapsed() >= KEEPALIVE) {
                    viewers_at = Some(Instant::now());
                    if let Some(n) = fetch_viewers(&agent, &base, &room.id, &room.token) {
                        live.set_viewers(&room.id, n);
                    }
                }
                let ids: Vec<u32> = room.staged.iter().map(|(id, _)| *id).collect();
                let screens = mgr.screens(&ids);
                let now = Instant::now();
                let client = RelayClient {
                    agent: &agent,
                    base: &base,
                    id: &room.id,
                    secret: &room.push_secret,
                };
                for (lane, (sid, _name)) in room.staged.iter().enumerate() {
                    // 会话可能在上架之后、这一轮之前就没了（被停掉、被
                    // 强杀）：跳过，不是错误——下一次上架会给出一份新的
                    // `staged`，这里不用替它擦屁股。
                    let Some(entry) = screens.iter().find(|e| e.id == *sid) else {
                        continue;
                    };
                    match attempt_lane(&client, lane, &entry.lines, lanes.get(sid).copied(), now) {
                        Some(recorded) => {
                            lanes.insert(*sid, recorded);
                        }
                        None => {
                            lanes.remove(sid);
                        }
                    }
                }
            }
            None => {
                // 上一轮还在播、这一轮没了：直播是被 `stop()` 收掉的（或者
                // 被新的一场顶掉了），中转那边这场直播还挂着，主动告诉它
                // 收摊——不然要等到 `LIVE_TTL` 自己过期，这段时间里学生页
                // 显示的还是最后一帧，而不是「直播结束了」。
                if let Some((id, secret)) = last_room.take() {
                    stop_room(&agent, &base, &id, &secret);
                    lanes.clear();
                }
                started = None;
                viewers_at = None;
            }
        }
        nap(stop, PUSH_INTERVAL);
    }
}

/// 一路一轮该做什么、做完之后这一路的记账该更新成什么样。从 `pusher_loop`
/// 里抽出来，好让「失败不改记账，下一轮带着真内容重试」这条规则脱离整条
/// 线程、只用一个假中转就能测（见 `attempt_lane` 相关测试）。
///
/// `last` 是这一路上次**成功**推送的 `(哈希, 时间)`；返回值是这一路推完
/// 这一次尝试之后该记的新值——`None` 表示「什么都还没成功过，保持
/// `lanes` 里没有这一路」。
///
/// **只有真的推成功才更新记账。** 早先的版本无条件 `lanes.insert`，导致
/// 任何一次真实失败（网络抖动、中转不可达、`413`）都会被本地记成
/// "已送达"：此后只要画面不再变化，`should_push` 走的是保活分支、发的是
/// 空 body——那一帧真正的内容再也没有机会重发，除非画面又变成一个新
/// hash。现在失败就原样返回 `last`，下一轮 `should_push` 看到的还是那个
/// 没被更新的旧记账，会认为内容还没送到，带着真内容重试。
///
/// 这一轮里推帧要用到的四样跟"这场直播、这个中转"有关、每一路都一样的
/// 东西——拆出来是为了让 `attempt_lane` 的参数少一点（clippy 的
/// `too_many_arguments`），不是有什么别的意义。
struct RelayClient<'a> {
    agent: &'a ureq::Agent,
    base: &'a str,
    id: &'a str,
    secret: &'a str,
}

fn attempt_lane(
    client: &RelayClient,
    lane: usize,
    lines: &[Vec<ScreenSpan>],
    last: Option<(u64, Instant)>,
    now: Instant,
) -> Option<(u64, Instant)> {
    let hash = hash_of(lines);
    let (last_hash, since) = match last {
        Some((h, t)) => (Some(h), now.duration_since(t)),
        None => (None, Duration::ZERO),
    };
    if !should_push(last_hash, hash, since) {
        return last;
    }
    // 画面没变、纯粹是保活：空 body，中转只续 TTL，不换 etag、不叫醒任何
    // 学生（见 `dct-srv` 那侧 `Live::push` 的注释）。
    let keepalive = last_hash == Some(hash);
    let body = if keepalive { Vec::new() } else { frame_of(lines) };
    if !keepalive && body.len() > MAX_FRAME_BYTES {
        // 帧本身压完还是超过中转能收的上限：这不是网络抖动，重试没有
        // 意义——同样的内容再压一次还是这么大，一直重试就是用一个必然
        // 失败的请求换不会成功的结果，是个死循环。这里按「已经处理过」
        // 记账（等价于推成功），好让 `should_push` 不再对着**同一屏内容**
        // 反复触发；等画面变了（哈希变了）才会再试一次——学生就是会停在
        // 上一帧能用的画面上，这比无限重试或者假装发出去了都诚实。
        return Some((hash, now));
    }
    if push_frame(client.agent, client.base, client.id, client.secret, lane, &body) {
        Some((hash, now))
    } else {
        last
    }
}

/// `POST {base}/live/start` 的请求体——跟 `dct-srv::LiveStartRequest` 字段
/// 一一对应，两边各写各的、靠字段名对齐（两个 crate 谁也不依赖谁，见
/// 模块头）。
#[derive(serde::Serialize)]
struct StartBody<'a> {
    id: &'a str,
    viewer_token: &'a str,
    push_secret: &'a str,
    lanes: Vec<&'a str>,
}

/// 开播：告诉中转这场直播的两把钥匙、上架了哪几路。**必须在第一次推帧
/// 之前成功过一次**——不然中转认不出这个 id，第一次推帧会被当成「这场
/// 直播不存在」拒收。
///
/// 回 `Result<(), String>`：失败的原因是一句已经本地化过的人话（连不上
/// 中转 / 中转拒绝了），要经 `LiveState::mark_failed` 走到 `LiveInfo`
/// 上给界面看——**绝不能是原始错误文本**，那可能带着 URL 之外的实现
/// 细节；也**绝不能是 `push_secret`**，这条路径就是为了不让它上协议线。
/// `pusher_loop` 靠这个返回值决定要不要往下推帧：失败就原地留着，下一轮
/// `PUSH_INTERVAL` 再试一次，绝不假装成功。
fn start_room(agent: &ureq::Agent, base: &str, room: &RoomSnapshot) -> Result<(), String> {
    let url = format!("{base}{}", dct_link::live::PATH_START);
    let lanes = room.staged.iter().map(|(_, name)| name.as_str()).collect();
    let body = StartBody {
        id: &room.id,
        viewer_token: &room.token,
        push_secret: &room.push_secret,
        lanes,
    };
    match agent.post(&url).send_json(body) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, _)) => Err(format!("中转拒绝了这场直播（状态码 {code}）")),
        Err(ureq::Error::Transport(_)) => Err("连不上中转，稍后会自动重试".to_string()),
    }
}

/// 推一帧。**回成没成功**——调用点靠这个决定记不记账，见 `attempt_lane`
/// 上面那段「只有真的推成功才更新记账」的注释；**不能把 push_secret 或者
/// 失败原因写进日志**，前者是凭据，后者多半就是网络错误本身，没有值得
/// 诊断的信息。
fn push_frame(agent: &ureq::Agent, base: &str, id: &str, secret: &str, lane: usize, body: &[u8]) -> bool {
    let url = format!("{base}{PATH_FRAME}");
    agent
        .post(&url)
        .set("x-live-id", id)
        .set("x-live-lane", &lane.to_string())
        .set("x-live-push", secret)
        .send_bytes(body)
        .is_ok()
}

/// `GET /live/{id}/lanes` 的答复里推帧线程要的那一半。中转那侧的
/// `LiveLanesResponse` 还带着 lanes 名字——那是给学生页渲染按钮用的，
/// 这里用不上，`serde` 会自动忽略多出来的字段。
#[derive(serde::Deserialize)]
struct LanesBody {
    viewers: u32,
}

/// 去中转问一次「现在几个人在看」。
///
/// **用的是学生那把只读 token**（`x-live-token`）：这是一条读路由，
/// `push_secret` 在这里不管用也不该用——推帧线程手上本来就有 viewer
/// token（`RoomSnapshot::token`），没必要为一次读把写的钥匙拿出来。
///
/// 读不到就答 `None`，调用方原样留着上一次的数——网络抖一下不该让老师
/// 屏幕上的人数瞬间归零，那比慢几秒更容易被误读成「学生都走了」。
fn fetch_viewers(agent: &ureq::Agent, base: &str, id: &str, token: &str) -> Option<u32> {
    let url = format!("{base}{}/{id}/lanes", dct_link::live::LIVE_PREFIX);
    let body: LanesBody = agent
        .get(&url)
        .set("x-live-token", token)
        .call()
        .ok()?
        .into_json()
        .ok()?;
    Some(body.viewers)
}

/// 停播：告诉中转把这场直播连同两把钥匙一起收掉。同样发不出去就算了——
/// 收不到这条 `DELETE`，中转会在 `LIVE_TTL` 之后自己回收，不是永久卡住。
fn stop_room(agent: &ureq::Agent, base: &str, id: &str, secret: &str) {
    let url = format!("{base}/live/{id}");
    let _ = agent.delete(&url).set("x-live-push", secret).call();
}

/// 睡一会儿，但叫停了就别接着睡。做法照抄 `link.rs::Link::nap`。
fn nap(stop: &AtomicBool, total: Duration) {
    let slice = Duration::from_millis(50);
    let mut left = total;
    while left > Duration::ZERO && !stop.load(Ordering::Relaxed) {
        let this = slice.min(left);
        std::thread::sleep(this);
        left -= this;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 没在播的时候：`info()` 回一个空但完整的形状，界面不用先判断「这个
    /// 字段存不存在」才能往下渲染。
    #[test]
    fn info_before_any_start_is_an_empty_but_complete_shape() {
        let live = LiveState::new("https://x".into());
        let info = live.info();
        assert_eq!(info.id, "");
        assert_eq!(info.token, "");
        assert_eq!(info.url, "");
        assert!(info.staged.is_empty());
        assert_eq!(info.viewers, 0);
    }

    /// 链接形状固定：`{base}/live/{id}#t={token}`，token 在 fragment 里。
    #[test]
    fn start_builds_the_student_link_with_the_token_in_the_fragment() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![(1, "前端".into())]);
        assert_eq!(
            info.url,
            format!("https://x/live/{}#t={}", info.id, info.token)
        );
    }

    /// 两把钥匙必须不同——合成一把的话，学生手上那份链接就能拿去推假画面
    /// （见 `LiveInfo` 上的文档注释）。`push_secret` 不在 `LiveInfo` 里，
    /// 只能从 `LiveState::push_secret()` 这条 crate 内部的口子拿。
    #[test]
    fn the_viewer_token_and_the_push_secret_are_different_keys() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![]);
        let secret = live.push_secret().expect("刚开播，该有 push_secret");
        assert_ne!(info.token, secret);
    }

    /// id 和两把钥匙的长度要跟 `dct_link::live` 里定的常量对得上——
    /// 两个 crate 各写一份长度就是「一边改了另一边没改」。
    #[test]
    fn the_generated_lengths_match_the_shared_constants() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![]);
        assert_eq!(info.id.len(), dct_link::live::LIVE_ID_LEN);
        assert_eq!(info.token.len(), dct_link::live::LIVE_TOKEN_LEN);
        assert_eq!(
            live.push_secret().unwrap().len(),
            dct_link::live::LIVE_TOKEN_LEN
        );
    }

    /// 没在播的时候，`push_secret()` 答不出来——不能让推帧线程拿着一把
    /// 上一场留下来的钥匙以为自己还能推。
    #[test]
    fn push_secret_is_none_when_nothing_is_live() {
        let live = LiveState::new("https://x".into());
        assert!(live.push_secret().is_none());
    }

    /// 停播之后 `info()` 要回到「没在播」的那个空形状，不能留着上一场的
    /// 尾巴——尤其是钥匙，停播之后它们不该再答得出来。
    #[test]
    fn stop_clears_the_room() {
        let live = LiveState::new("https://x".into());
        live.start(vec![(1, "前端".into())]);
        live.stop();
        let info = live.info();
        assert_eq!(info.id, "");
        assert!(info.staged.is_empty());
        assert!(live.push_secret().is_none());
    }

    /// 再开一场直接顶掉上一场——不用先手动停播。
    #[test]
    fn starting_again_replaces_the_previous_room() {
        let live = LiveState::new("https://x".into());
        let first = live.start(vec![(1, "前端".into())]);
        let second = live.start(vec![(2, "后端".into())]);
        assert_ne!(first.id, second.id);
        let info = live.info();
        assert_eq!(info.id, second.id);
        assert_eq!(info.staged, vec![(2, "后端".into())]);
    }

    /// **改上架名单不许动链接。** 老师上架「后端」把链接发给全班之后再加
    /// 一路「前端」，走 `start()` 的话 200 个学生当场一起掉线——id、学生
    /// token、push_secret 三样一个都不能变。
    #[test]
    fn restaging_keeps_the_id_and_both_keys_so_the_link_stays_valid() {
        let live = LiveState::new("https://x".into());
        let first = live.start(vec![(1, "后端".into())]);
        let secret_before = live.push_secret().unwrap();

        let after = live
            .restage(vec![(1, "后端".into()), (2, "前端".into())])
            .expect("在播的时候 restage 该成功");

        assert_eq!(after.id, first.id, "改上架换了 id，全班的链接就作废了");
        assert_eq!(after.token, first.token, "改上架换了学生 token，全班当场掉线");
        assert_eq!(after.url, first.url, "链接必须一个字都不变");
        assert_eq!(
            live.push_secret().unwrap(),
            secret_before,
            "push_secret 也得留着——中转靠它认出「还是同一个老师在重开这一场」"
        );
        assert_eq!(after.staged.len(), 2, "新上架的那一路没生效");
    }

    /// 改完上架，中转还不知道新的 lanes——`readiness` 要如实退回 `Pending`，
    /// 不能继续说 `Ready`。
    #[test]
    fn restaging_drops_back_to_pending_until_the_relay_knows_the_new_lanes() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![(1, "后端".into())]);
        live.mark_ready(&info.id, 0);

        live.restage(vec![(2, "前端".into())]).unwrap();

        assert_eq!(live.info().readiness, LiveReadiness::Pending);
    }

    /// 改上架之后，一次针对**旧** lanes 的迟到 `mark_ready` 不该把
    /// 「中转已经认得新 lanes」这句假话写上去——`id` 没变，只有 `rev` 能
    /// 分辨这两次。
    #[test]
    fn a_late_mark_ready_for_the_previous_staging_does_not_claim_the_new_one_is_ready() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![(1, "后端".into())]);
        live.restage(vec![(2, "前端".into())]).unwrap();

        live.mark_ready(&info.id, 0); // 旧那一版的回执，迟到了

        assert_eq!(
            live.info().readiness,
            LiveReadiness::Pending,
            "旧 lanes 的回执被当成了新 lanes 的"
        );
    }

    /// 「N 人在看」必须是活的——spec 四道防线里的第二道。推帧线程读回来
    /// 的数要真的走到 `info()` 上，而不是一个恒为 0 的硬编码。
    #[test]
    fn the_viewer_count_comes_from_the_relay_not_a_hardcoded_zero() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![(1, "前端".into())]);
        assert_eq!(info.viewers, 0, "刚开播还没人，也还没读过");

        live.set_viewers(&info.id, 7);

        assert_eq!(live.info().viewers, 7);
    }

    /// 改上架不该把刚读到的人数扔掉——人数跟上架了哪几路无关。
    #[test]
    fn restaging_keeps_the_viewer_count() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![(1, "前端".into())]);
        live.set_viewers(&info.id, 12);

        let after = live.restage(vec![(2, "后端".into())]).unwrap();

        assert_eq!(after.viewers, 12);
    }

    /// 一次迟到的人数回执不该落到另一场直播头上。
    #[test]
    fn a_late_viewer_count_for_a_previous_live_is_ignored() {
        let live = LiveState::new("https://x".into());
        let stale = live.start(vec![(1, "前端".into())]);
        live.start(vec![(2, "后端".into())]);

        live.set_viewers(&stale.id, 99);

        assert_eq!(live.info().viewers, 0, "上一场的人数被套到这一场头上了");
    }

    /// `fetch_viewers` 走的是学生那条只读路由：`GET /live/{id}/lanes`，
    /// 带 `x-live-token`——**push_secret 绝不出现在这条读请求里**。
    #[test]
    fn fetch_viewers_asks_the_lanes_route_with_the_read_only_token() {
        let srv = FakeSrv::start();
        let agent = crate::sys::tls::agent_builder().build();

        let n = fetch_viewers(&agent, &srv.base(), "abc123", "viewer-t");

        assert_eq!(n, Some(5), "假中转答的是 5 个人在看");
        let seen = srv.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "GET");
        assert_eq!(seen[0].path, "/live/abc123/lanes");
        assert_eq!(seen[0].headers.get("x-live-token").unwrap(), "viewer-t");
        assert!(
            !seen[0].headers.contains_key("x-live-push"),
            "读人数是读，不该把写的钥匙带上"
        );
    }

    /// 读不到就留着上一次的数，不归零——网络抖一下让屏幕上的人数瞬间变
    /// 成 0，比慢几秒更容易被误读成「学生都走了」。
    #[test]
    fn an_unreachable_relay_leaves_the_viewer_count_alone() {
        let agent = crate::sys::tls::agent_builder()
            .timeout_connect(Duration::from_millis(200))
            .build();
        // 127.0.0.1:1 没人监听。
        assert_eq!(fetch_viewers(&agent, "http://127.0.0.1:1", "abc", "t"), None);
    }

    /// 没在播的时候没有名单可改——回 `None`，不能悄悄开一场新的。
    #[test]
    fn restaging_when_nothing_is_live_answers_none() {
        let live = LiveState::new("https://x".into());
        assert!(live.restage(vec![(1, "前端".into())]).is_none());
        assert!(live.info().id.is_empty(), "restage 不该凭空开出一场直播");
    }

    fn span(text: String) -> ScreenSpan {
        ScreenSpan {
            text,
            style: crate::pty::ScreenStyle::default(),
        }
    }

    /// 画面没变就不推——终端大多数时候是静止的，这一条省掉的是绝大部分流量。
    #[test]
    fn an_unchanged_screen_is_not_pushed_again() {
        assert!(!should_push(Some(7), 7, Duration::from_secs(1)));
        assert!(should_push(Some(7), 8, Duration::from_secs(1)));
        assert!(should_push(None, 7, Duration::from_secs(0)));
    }

    /// 但静止太久也要推一次保活，否则中转按 TTL 把整条直播扔掉，
    /// 而老师只是盯着屏幕想了一分钟事情。
    #[test]
    fn a_still_screen_is_still_kept_alive() {
        assert!(should_push(Some(7), 7, KEEPALIVE));
    }

    /// 帧是压过的：一屏终端压完该比原文小得多，不然带宽那三条里最重的一条
    /// 等于没做。
    #[test]
    fn a_frame_is_compressed() {
        let line = vec![span("x".repeat(120))];
        let lines: Vec<_> = (0..40).map(|_| line.clone()).collect();
        let raw = serde_json::to_vec(&lines).unwrap();
        let frame = frame_of(&lines);
        assert!(
            frame.len() * 4 < raw.len(),
            "压完 {} 字节，原文 {} 字节——压缩没起作用",
            frame.len(),
            raw.len()
        );
    }

    /// **两侧对"一帧长什么样"的理解必须是同一件事——这条测试钉的正是这个
    /// 缝，而不是"多一个断言"。**
    ///
    /// 这个 bug 是怎么发生的：`frame_of` 序列化的是 `lines` 本身
    /// （`serde_json::to_vec(lines)`，一个 `Vec<Vec<ScreenSpan>>`），学生页
    /// 第一版却当它是包了一层 `Response::Screen` 的对象（`if (!frame ||
    /// !frame.Screen)`）——那层壳只在守护进程直接答复 `Request::Screen`
    /// 时才存在（手机端 `page.html` 走的是那条路），直播这条独立的推帧
    /// 管道从来没套过它。于是每一帧都在网页那一行判断里被当成"形状不对"
    /// 静默丢弃：`dct` 这边的单元测试测的是"`frame_of` 能不能序列化/
    /// 压缩"，`dct-page` 那边的单元测试测的是"页面有没有读该读的字段"，
    /// 两边各自都是绿的，**但中间"Rust 发的形状"和"JS 读的形状"是不是
    /// 同一个东西，两边都没有测过**——这道缝只有真的拿浏览器打开链接才
    /// 看得见：画面永远是空的，60 秒后 staleness 计时器还会把"页面根本
    /// 没解析成功"误报成「老师暂停了」，很容易被当成"老师没在动"。
    ///
    /// 所以这条测试让 Rust 这边发的真实字节，流过页面读它的那一行代码：
    /// 拿一屏真实的 `lines` 走 `frame_of` 编码、解压回 JSON，断言它的形状
    /// 是"顶层数组、每行是数组、每个 span 有 `text` 字段"；再反过来断言
    /// `live.html` 里没有那种只在"包了一层 Screen"的假设下才成立的写法。
    #[test]
    fn the_wire_shape_frame_of_sends_is_the_shape_the_student_page_reads() {
        use std::io::Read;

        let lines = vec![
            vec![span("hi".into())],
            vec![span("there".into())],
        ];

        let gz = frame_of(&lines);
        let mut plain = Vec::new();
        flate2::read::GzDecoder::new(gz.as_slice())
            .read_to_end(&mut plain)
            .expect("frame_of 编出来的不是合法 gzip——学生页那边解不开");
        let value: serde_json::Value = serde_json::from_slice(&plain)
            .expect("解压出来的不是合法 JSON");

        // 顶层是"行的数组"，不是带 `Screen` 键的对象——这正是当初读错的
        // 那个形状。
        let rows = value
            .as_array()
            .unwrap_or_else(|| panic!("frame_of 发的顶层不是数组了：{value}"));
        assert_eq!(rows.len(), 2, "行数对不上，frame_of 的形状变了？");
        let first_row = rows[0]
            .as_array()
            .unwrap_or_else(|| panic!("行不是数组：{}", rows[0]));
        assert_eq!(
            first_row[0]["text"], "hi",
            "span 里没有 text 字段，或者根本不是同一个形状"
        );

        // 学生页必须读的是这同一个形状：数组本身，不是某个包了一层的对象
        // ——`.Screen`/`frame.lines` 都是"以为帧外面还套了一层"才会写出来
        // 的取法，`frame_of` 从来没套过那一层。
        let page = dct_page::live_page();
        assert!(
            !page.contains(".Screen"),
            "学生页里出现了 .Screen——但 frame_of 发的是行数组本身，没有 \
             Screen 这层包装，这正是让画面一直空白的那个 bug"
        );
        assert!(
            !page.contains("frame.lines"),
            "学生页在读 frame.lines——但 frame_of 发的顶层就是行数组，没有 \
             lines 这层包装"
        );
    }

    /// 链接里那串东西必须够长、而且每次都不一样。
    #[test]
    fn two_lives_never_get_the_same_link() {
        let a = LiveState::new("https://x".into()).start(vec![(1, "一".into())]);
        let b = LiveState::new("https://x".into()).start(vec![(1, "一".into())]);
        assert_ne!(a.token, b.token);
        assert_eq!(a.token.len(), dct_link::live::LIVE_TOKEN_LEN);
        assert!(a.url.starts_with("https://x/live/"), "链接形状不对：{}", a.url);
        assert!(a.url.contains("#t="), "token 必须在 fragment 里：{}", a.url);
    }

    /// 环境变量没设的时候用内置默认值。
    #[test]
    fn no_env_var_falls_back_to_the_default() {
        assert_eq!(resolve_relay_base(None), DEFAULT_RELAY_BASE);
        assert_eq!(resolve_relay_base(Some("")), DEFAULT_RELAY_BASE, "空字符串不算设了");
    }

    /// 环境变量设了就用它。
    #[test]
    fn an_env_var_overrides_the_default() {
        assert_eq!(resolve_relay_base(Some("https://relay.example")), "https://relay.example");
    }

    /// 尾部斜杠有没有都要能用，答案得一样——照 `LinkConfig::url()` 的做法。
    #[test]
    fn a_trailing_slash_does_not_change_the_result() {
        assert_eq!(
            resolve_relay_base(Some("https://relay.example/")),
            resolve_relay_base(Some("https://relay.example"))
        );
        assert_eq!(
            resolve_relay_base(None),
            resolve_relay_base(Some(&format!("{DEFAULT_RELAY_BASE}/")))
        );
    }

    /// 假中转：只认推帧/开播/停播这三条路径，记下收到的方法/路径/头/body，
    /// 好让测试断言真正发出去的请求长什么样；还能按吩咐故意失败几次——
    /// 照抄 `link.rs::FakeSrv` 的做法，不拉真的 `dct-srv` 进来，这里只关心
    /// `push_frame`/`stop_room`/`start_room`/`attempt_lane` 自己拼的请求
    /// 对不对，也测「网络抖了一下」这种不重新验一遍中转怎么处理它。
    struct FakeSrv {
        addr: std::net::SocketAddr,
        state: Arc<Mutex<FakeState>>,
    }

    #[derive(Default)]
    struct FakeState {
        seen: Vec<Seen>,
        /// 接下来还要故意失败几次（连响应都不给，直接摔断连接）。
        fail: usize,
    }

    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    impl FakeSrv {
        fn start() -> FakeSrv {
            use std::net::TcpListener;
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let state = Arc::new(Mutex::new(FakeState::default()));
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

        fn seen(&self) -> Vec<Seen> {
            recover(self.state.lock()).seen.clone()
        }
    }

    fn serve_one(mut conn: std::net::TcpStream, state: Arc<Mutex<FakeState>>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.is_empty() {
            return;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();

        let mut headers = HashMap::new();
        let mut len = 0usize;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                let k = k.trim().to_ascii_lowercase();
                let v = v.trim().to_string();
                if k == "content-length" {
                    len = v.parse().unwrap_or(0);
                }
                headers.insert(k, v);
            }
        }
        let mut body = vec![0u8; len];
        let _ = reader.read_exact(&mut body);

        let mut st = recover(state.lock());
        if st.fail > 0 {
            st.fail -= 1;
            // 连响应都不给，直接把连接摔上——这是网络抖动/中转挂掉时
            // 最像的样子（照抄 `link.rs::FakeSrv` 的做法）。
            return;
        }
        // 读人数那条路要答一份真的 JSON，别的路（开播/推帧/停播）答 204
        // 就够——调用方只看成没成功。
        let lanes_query = path.ends_with("/lanes");
        st.seen.push(Seen {
            method,
            path,
            headers,
            body,
        });
        drop(st);

        let out = if lanes_query {
            let json = r#"{"lanes":["前端"],"viewers":5}"#;
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                json.len()
            )
        } else {
            "HTTP/1.1 204 X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
        };
        let _ = conn.write_all(out.as_bytes());
    }

    /// `push_frame` 带着约定好的三个头（id/lane/push secret）POST 到
    /// `{base}{PATH_FRAME}`，body 就是传进去的那份字节——学生那把 token
    /// 绝不出现在这条请求里，成功回 `true`。
    #[test]
    fn push_frame_sends_the_agreed_headers_to_the_agreed_path() {
        let srv = FakeSrv::start();
        let agent = crate::sys::tls::agent_builder().build();
        assert!(push_frame(&agent, &srv.base(), "abc123", "s3cr3t", 2, &[1, 2, 3]));

        let seen = srv.seen();
        assert_eq!(seen.len(), 1);
        let req = &seen[0];
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, PATH_FRAME);
        assert_eq!(req.headers.get("x-live-id").unwrap(), "abc123");
        assert_eq!(req.headers.get("x-live-lane").unwrap(), "2");
        assert_eq!(req.headers.get("x-live-push").unwrap(), "s3cr3t");
        assert_eq!(req.body, vec![1, 2, 3]);
    }

    /// 保活推的是空 body——中转靠这个分辨「只续 TTL」和「真的换了一帧」。
    #[test]
    fn a_keepalive_push_has_an_empty_body() {
        let srv = FakeSrv::start();
        let agent = crate::sys::tls::agent_builder().build();
        assert!(push_frame(&agent, &srv.base(), "abc123", "s3cr3t", 0, &[]));

        let seen = srv.seen();
        assert!(seen[0].body.is_empty());
    }

    /// 停播发的是 `DELETE {base}/live/{id}`，带同一把 `x-live-push`。
    #[test]
    fn stop_room_sends_a_delete_with_the_push_secret() {
        let srv = FakeSrv::start();
        let agent = crate::sys::tls::agent_builder().build();
        stop_room(&agent, &srv.base(), "abc123", "s3cr3t");

        let seen = srv.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "DELETE");
        assert_eq!(seen[0].path, "/live/abc123");
        assert_eq!(seen[0].headers.get("x-live-push").unwrap(), "s3cr3t");
    }

    /// `start_room` 把两把钥匙和 lane 名字都带上，POST 到 `PATH_START`——
    /// 中转靠这条请求才知道这场直播存在，不然第一次推帧会被当成「这场
    /// 直播不存在」拒收。
    #[test]
    fn start_room_sends_both_keys_and_the_lane_names() {
        let srv = FakeSrv::start();
        let agent = crate::sys::tls::agent_builder().build();
        let room = RoomSnapshot {
            id: "abc123".into(),
            token: "viewer-t".into(),
            push_secret: "push-s".into(),
            staged: vec![(1, "前端".into()), (2, "后端".into())],
            staged_rev: 0,
        };
        assert!(
            start_room(&agent, &srv.base(), &room).is_ok(),
            "假中转总是回 204，不该失败"
        );

        let seen = srv.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].path, dct_link::live::PATH_START);
        let body: serde_json::Value = serde_json::from_slice(&seen[0].body).unwrap();
        assert_eq!(body["id"], "abc123");
        assert_eq!(body["viewer_token"], "viewer-t");
        assert_eq!(body["push_secret"], "push-s");
        assert_eq!(body["lanes"], serde_json::json!(["前端", "后端"]));
    }

    /// 中转连不上（或者拒了）的时候，`start_room` 必须老实回 `Err`——
    /// `pusher_loop` 靠这个决定要不要往下推帧，绝不能假装开播成功；原因
    /// 是一句人话，不能是原始错误文本或者 `push_secret`。
    #[test]
    fn start_room_reports_failure_when_the_relay_is_unreachable() {
        let agent = crate::sys::tls::agent_builder()
            .timeout_connect(Duration::from_millis(200))
            .build();
        let room = RoomSnapshot {
            id: "abc123".into(),
            token: "viewer-t".into(),
            push_secret: "push-s".into(),
            staged: vec![(1, "前端".into())],
            staged_rev: 0,
        };
        // 127.0.0.1:1 没人监听，连接会被立刻拒绝——不需要真的等超时。
        let err = start_room(&agent, "http://127.0.0.1:1", &room).unwrap_err();
        assert!(!err.contains("push-s"), "失败原因里绝不能带着凭据：{err}");
    }

    /// **推送失败之后，下一轮仍然会带着内容重推，而不是发空 body 保活。**
    /// 这是本模块要挡住的那个真实 bug：早先的实现无条件记账，失败之后
    /// `should_push` 会误以为「已经推过了」，此后只发保活的空 body，真内容
    /// 再也没有机会重发。
    #[test]
    fn a_failed_push_is_retried_with_content_not_a_keepalive() {
        let srv = FakeSrv::start();
        recover(srv.state.lock()).fail = 1; // 第一次网络请求故意失败
        let agent = crate::sys::tls::agent_builder().build();
        let base = srv.base();
        let client = RelayClient {
            agent: &agent,
            base: &base,
            id: "abc",
            secret: "s3cr3t",
        };
        let lines = vec![vec![span("hi".into())]];

        let now0 = Instant::now();
        let after_first = attempt_lane(&client, 0, &lines, None, now0);
        assert_eq!(after_first, None, "第一次没发出去，不该记账");

        let now1 = now0 + Duration::from_millis(10);
        let after_second = attempt_lane(&client, 0, &lines, after_first, now1);
        assert!(after_second.is_some(), "第二次该发成功、记上账");

        let seen = srv.seen();
        assert_eq!(seen.len(), 1, "失败的那次假中转直接摔断连接，不该被记成收到");
        assert!(!seen[0].body.is_empty(), "第二次带的必须是真内容，不是保活的空 body");
    }

    /// 帧压完仍然超过中转能收的上限：这不是网络抖动，重试没有意义——一直
    /// 重试同一份必然超限的内容就是死循环。这里要按「已经处理过」记账，
    /// 好让 `should_push` 不再对着同一屏内容反复触发，而且这份超限的帧
    /// 根本不该被发出去。
    #[test]
    fn an_oversized_frame_is_skipped_and_not_retried_forever() {
        let srv = FakeSrv::start();
        let agent = crate::sys::tls::agent_builder().build();
        let base = srv.base();
        let client = RelayClient {
            agent: &agent,
            base: &base,
            id: "abc",
            secret: "s3cr3t",
        };
        let lines = noisy_lines(4000, 160);
        let frame = frame_of(&lines);
        assert!(
            frame.len() > dct_link::live::MAX_FRAME_BYTES,
            "测试前提不成立：这一屏压完只有 {} 字节，没触发该测的分支",
            frame.len()
        );

        let now = Instant::now();
        let after = attempt_lane(&client, 0, &lines, None, now);
        assert!(after.is_some(), "超限也要记账，不然会对着同一屏内容永远重试");
        assert!(srv.seen().is_empty(), "超限的帧根本不该被发出去");
    }

    /// 同一屏画面连着两轮：第一轮推真内容，紧接着第二轮（时间没到保活线）
    /// 什么都不该发，撑到保活线的第三轮发的必须是空 body——不是内容。
    #[test]
    fn the_same_screen_across_rounds_sends_content_once_then_a_keepalive_later() {
        let srv = FakeSrv::start();
        let agent = crate::sys::tls::agent_builder().build();
        let base = srv.base();
        let client = RelayClient {
            agent: &agent,
            base: &base,
            id: "abc",
            secret: "s3cr3t",
        };
        let lines = vec![vec![span("hi".into())]];

        let now0 = Instant::now();
        let after_first = attempt_lane(&client, 0, &lines, None, now0);
        assert!(after_first.is_some());

        let now1 = now0 + Duration::from_millis(10);
        let after_second = attempt_lane(&client, 0, &lines, after_first, now1);
        assert_eq!(after_second, after_first, "画面没变又没到保活线，不该动");

        let now2 = now0 + KEEPALIVE;
        let after_third = attempt_lane(&client, 0, &lines, after_second, now2);
        assert!(after_third.is_some());

        let seen = srv.seen();
        assert_eq!(seen.len(), 2, "只有第一轮和保活那一轮真的发了请求");
        assert!(!seen[0].body.is_empty(), "第一轮该是真内容");
        assert!(seen[1].body.is_empty(), "保活那一轮该是空 body");
    }

    /// 刚 `start()` 完，中转还完全没听说过这场直播——`readiness` 必须如实
    /// 说「还没就绪」，不能让老师那块屏幕以为链接已经能用了。
    #[test]
    fn a_freshly_started_live_is_not_ready_yet() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![(1, "前端".into())]);
        assert_eq!(info.readiness, crate::proto::LiveReadiness::Pending);
    }

    /// 推帧线程告诉中转成功之后，`readiness` 要翻成 `Ready`。
    #[test]
    fn marking_ready_flips_the_readiness() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![(1, "前端".into())]);
        live.mark_ready(&info.id, 0);
        assert_eq!(live.info().readiness, crate::proto::LiveReadiness::Ready);
    }

    /// 注册失败要留下原因，供界面显示。
    #[test]
    fn marking_failed_records_a_reason() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![(1, "前端".into())]);
        live.mark_failed(&info.id, 0, "连不上中转".into());
        assert_eq!(
            live.info().readiness,
            crate::proto::LiveReadiness::Failed("连不上中转".into())
        );
    }

    /// 一场已经不是当前这一场的直播（被 `stop()`/被新的一场顶掉）不该再
    /// 被一次迟到的 `mark_ready`/`mark_failed` 写坏——那次网络请求本来就
    /// 是对着一场已经不存在的直播做的。
    #[test]
    fn marking_a_stale_id_does_not_touch_the_current_room() {
        let live = LiveState::new("https://x".into());
        let stale = live.start(vec![(1, "前端".into())]);
        let current = live.start(vec![(2, "后端".into())]);
        live.mark_ready(&stale.id, 0);
        assert_eq!(
            live.info().readiness,
            crate::proto::LiveReadiness::Pending,
            "迟到的 mark_ready 认错了 id，不该影响当前这一场"
        );
        assert_eq!(live.info().id, current.id);
    }

    /// 造一屏「看起来随机」的内容：足够大、足够没有重复模式，让 gzip 压不
    /// 小——不需要真随机，一个简单的异或移位生成器就够。
    fn noisy_lines(rows: usize, cols: usize) -> Vec<Vec<ScreenSpan>> {
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        (0..rows)
            .map(|_| {
                let text: String = (0..cols).map(|_| format!("{:x}", next() & 0xf)).collect();
                vec![span(text)]
            })
            .collect()
    }
}
