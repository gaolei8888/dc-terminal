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

use dct_link::live::{KEEPALIVE, PATH_FRAME, PUSH_INTERVAL};

use crate::proto::LiveInfo;
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
        };

        *recover(self.room.lock()) = Some(Room {
            id,
            token,
            push_secret,
            staged,
        });

        info
    }

    pub fn stop(&self) {
        *recover(self.room.lock()) = None;
    }

    /// 没在播的时候：`id`/`token`/`url` 都是空串，`staged` 是空表，
    /// `viewers` 是 0。**空串而不是 `Option`**——`LiveInfo` 是要经过协议线
    /// 的形状，`Option<LiveInfo>` 才是「有没有在播」该长的样子，但这一层
    /// 就要先定下「没有」具体长什么样，好让界面不用先判断一次「有没有这个
    /// 字段」才能往下渲染；空串本身就是一个不会被误认成真实 id/链接的值。
    /// 观众数眼下没有真实来源（推帧线程是下一个任务的事），在播时也先答
    /// 0，不假装知道。
    pub fn info(&self) -> LiveInfo {
        match &*recover(self.room.lock()) {
            Some(room) => LiveInfo {
                id: room.id.clone(),
                token: room.token.clone(),
                url: format!("{}/live/{}#t={}", self.base, room.id, room.token),
                staged: room.staged.clone(),
                viewers: 0,
            },
            None => LiveInfo {
                id: String::new(),
                token: String::new(),
                url: String::new(),
                staged: Vec::new(),
                viewers: 0,
            },
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
            push_secret: r.push_secret.clone(),
            staged: r.staged.clone(),
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
    push_secret: String,
    staged: Vec<(u32, String)>,
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
/// **哈希的是压缩前的 JSON，不是 `frame_of` 的输出。** gzip 的头里带着
/// 时间戳（`flate2` 默认的 `GzHeader` 不是空的），同一屏内容两次压缩出来
/// 的字节不保证相同——拿压缩后的字节去比较「画面变没变」，会把「没变」
/// 误判成「变了」，`should_push` 因此永远推，白白丢掉这个函数存在的意义。
/// `ScreenSpan`/`ScreenStyle` 没派生 `Hash`（它们是协议类型，不该为了这
/// 一处内部用途多背一个 derive），序列化成 JSON 再哈希就绕开了这件事，
/// 代价是每一路每一轮多算一次 JSON——量级上跟 `frame_of` 自己那次持平，
/// 换一次画面不变时省下的整条网络请求，这笔账划算。
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
pub fn spawn_pusher(live: Arc<LiveState>, mgr: Arc<SessionManager>) -> PusherHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    std::thread::spawn(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| pusher_loop(&live, &mgr, &st)));
    });
    PusherHandle { stop }
}

/// 推帧线程的主循环。**IO 薄薄一层**：该不该推（`should_push`）、帧长什么
/// 样（`frame_of`/`hash_of`）都是上面那几个纯函数，这里只负责醒过来、
/// 读一次会话屏幕、决定发不发、真的发。
fn pusher_loop(live: &LiveState, mgr: &SessionManager, stop: &AtomicBool) {
    let agent = crate::sys::tls::agent_builder()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .build();
    let base = live.base().to_string();

    // 每一路各自的「上次哈希、上次推的时间」，按会话 id 记账（不是下标）：
    // 一场直播里 `staged` 的顺序定了就不会再变，但用 id 对齐比信任下标
    // 稳——万一将来上架顺序会变，这里不用跟着改。
    let mut lanes: HashMap<u32, (u64, Instant)> = HashMap::new();
    // 上一轮看到的房间，停播时要靠它才知道该对哪个 id、拿哪把钥匙发
    // `DELETE`——那一轮 `snapshot()` 已经答不出来了。
    let mut last_room: Option<(String, String)> = None;

    while !stop.load(Ordering::Relaxed) {
        match live.snapshot() {
            Some(room) => {
                last_room = Some((room.id.clone(), room.push_secret.clone()));
                let ids: Vec<u32> = room.staged.iter().map(|(id, _)| *id).collect();
                let screens = mgr.screens(&ids);
                let now = Instant::now();
                for (lane, (sid, _name)) in room.staged.iter().enumerate() {
                    // 会话可能在上架之后、这一轮之前就没了（被停掉、被
                    // 强杀）：跳过，不是错误——下一次上架会给出一份新的
                    // `staged`，这里不用替它擦屁股。
                    let Some(entry) = screens.iter().find(|e| e.id == *sid) else {
                        continue;
                    };
                    let hash = hash_of(&entry.lines);
                    let (last_hash, since) = match lanes.get(sid) {
                        Some((h, t)) => (Some(*h), now.duration_since(*t)),
                        None => (None, Duration::ZERO),
                    };
                    if !should_push(last_hash, hash, since) {
                        continue;
                    }
                    // 画面没变、纯粹是保活：空 body，中转只续 TTL，不换
                    // etag、不叫醒任何学生（见 `dct-srv` 那侧 `Live::push`
                    // 的注释）。
                    let keepalive = last_hash == Some(hash);
                    let body = if keepalive {
                        Vec::new()
                    } else {
                        frame_of(&entry.lines)
                    };
                    push_frame(&agent, &base, &room.id, &room.push_secret, lane, body);
                    lanes.insert(*sid, (hash, now));
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
            }
        }
        nap(stop, PUSH_INTERVAL);
    }
}

/// 推一帧。发不出去就算了——理由同 `link.rs::Link::send`：直播是允许丢的
/// 那条单向线，下一轮 `PUSH_INTERVAL` 会再试一次，反复重试只会让下一帧
/// 排更久；**不能把 push_secret 或者失败原因写进日志**，前者是凭据，后者
/// 多半就是网络错误本身，没有值得诊断的信息。
fn push_frame(agent: &ureq::Agent, base: &str, id: &str, secret: &str, lane: usize, body: Vec<u8>) {
    let url = format!("{base}{PATH_FRAME}");
    let _ = agent
        .post(&url)
        .set("x-live-id", id)
        .set("x-live-lane", &lane.to_string())
        .set("x-live-push", secret)
        .send_bytes(&body);
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

    /// 画面完全一样的两屏，哈希也该一样——`should_push` 全靠这件事才分得清
    /// 「没变」和「变了」。
    #[test]
    fn identical_screens_hash_the_same() {
        let a = vec![vec![span("hi".into())]];
        let b = vec![vec![span("hi".into())]];
        assert_eq!(hash_of(&a), hash_of(&b));
    }

    /// 哪怕只多一个字符，哈希也要变——不然一屏内容悄悄改了却被当成「没变」，
    /// 学生就永远看着一帧旧画面。
    #[test]
    fn a_changed_screen_hashes_differently() {
        let a = vec![vec![span("hi".into())]];
        let b = vec![vec![span("hi!".into())]];
        assert_ne!(hash_of(&a), hash_of(&b));
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

    /// 假中转：只认推帧和停播这两条路径，记下收到的方法/路径/头/body，
    /// 好让测试断言真正发出去的请求长什么样。照抄 `link.rs::FakeSrv` 的
    /// 做法——不拉真的 `dct-srv` 进来，这里只关心 `push_frame`/`stop_room`
    /// 自己拼的请求对不对，不重新验一遍中转怎么处理它。
    struct FakeSrv {
        addr: std::net::SocketAddr,
        got: Arc<Mutex<Vec<Seen>>>,
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
            let got = Arc::new(Mutex::new(Vec::new()));
            let g = got.clone();
            std::thread::spawn(move || {
                for conn in listener.incoming() {
                    let Ok(conn) = conn else { break };
                    let g = g.clone();
                    std::thread::spawn(move || serve_one(conn, g));
                }
            });
            FakeSrv { addr, got }
        }

        fn base(&self) -> String {
            format!("http://{}", self.addr)
        }
    }

    fn serve_one(mut conn: std::net::TcpStream, got: Arc<Mutex<Vec<Seen>>>) {
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

        recover(got.lock()).push(Seen {
            method,
            path,
            headers,
            body,
        });

        let out = "HTTP/1.1 204 X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = conn.write_all(out.as_bytes());
    }

    /// `push_frame` 带着约定好的三个头（id/lane/push secret）POST 到
    /// `{base}{PATH_FRAME}`，body 就是传进去的那份字节——学生那把 token
    /// 绝不出现在这条请求里。
    #[test]
    fn push_frame_sends_the_agreed_headers_to_the_agreed_path() {
        let srv = FakeSrv::start();
        let agent = crate::sys::tls::agent_builder().build();
        push_frame(&agent, &srv.base(), "abc123", "s3cr3t", 2, vec![1, 2, 3]);

        let seen = recover(srv.got.lock());
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
        push_frame(&agent, &srv.base(), "abc123", "s3cr3t", 0, Vec::new());

        let seen = recover(srv.got.lock());
        assert!(seen[0].body.is_empty());
    }

    /// 停播发的是 `DELETE {base}/live/{id}`，带同一把 `x-live-push`。
    #[test]
    fn stop_room_sends_a_delete_with_the_push_secret() {
        let srv = FakeSrv::start();
        let agent = crate::sys::tls::agent_builder().build();
        stop_room(&agent, &srv.base(), "abc123", "s3cr3t");

        let seen = recover(srv.got.lock());
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "DELETE");
        assert_eq!(seen[0].path, "/live/abc123");
        assert_eq!(seen[0].headers.get("x-live-push").unwrap(), "s3cr3t");
    }
}
