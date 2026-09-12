//! 直播：中转记住每一路的**最新一帧**，学生只读。
//!
//! # 这是对 spec 决定一的第一处偏离，写在这里免得将来有人读不出边界
//!
//! 原话是「中转只搬信封，不看里面」。这里多出来的是一个**角色**（记住最新
//! 一帧），不是一份**理解**：帧对中转始终是不透明字节，它不解析、不认识里面
//! 是 span 还是别的——这个模块里没有一行反序列化 `frame`。这条性质本任务
//! 没有专门的守卫测试钉住它，留给下一个任务（HTTP 路由）随路由一起落地：
//! 那时请求体真正从网络进来，才有实际的反序列化路径可测。
//!
//! # 为什么不是队列
//!
//! 只留最新一帧，不攒历史。攒历史等于给离线的人排队（spec 明令不做），而且
//! 学生晚进来看到的应该是"现在"，不是十分钟前那一屏。
//!
//! # 两把钥匙，不是一把
//!
//! live-id 本身不保密——它就写在学生打开的链接里，每个学生都知道。如果推帧
//! 只认 id，任何一个知道链接的学生都能往这场直播里推假画面，而这个功能的
//! 全部前提是**学生只读**。所以拆成两把钥匙：`viewer_token` 只能读
//! （`frame`/`lanes`），`push_secret` 只能写（`push`）。两把都只存摘要，
//! 不存原文。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use dct_link::live::{LIVE_TTL, MAX_FRAME_BYTES, MAX_LANES};
use dct_link::LinkError;
use tokio::sync::watch;

struct Lane {
    name: String,
    frame: Vec<u8>,
    etag: u64,
    tx: watch::Sender<u64>,
}

struct Session {
    viewer_hash: [u8; 32],
    push_hash: [u8; 32],
    lanes: Vec<Lane>,
    fed_at: Instant,
    next_etag: u64,
}

/// 一个来源最近这一个窗口里建了几场。见 [`Live::note_start`]。
struct Starts {
    /// 这个窗口是什么时候开始的。到点就整段翻篇，不做滑动窗口——建房是
    /// 稀疏事件，滑动窗口那点精度换不来什么，反倒要给每个来源存一串时间戳。
    since: Instant,
    count: u32,
}

/// 限流表最多记几个来源。
///
/// 表本身也得有上限：`X-Forwarded-For` 是请求方写的，换一个值就是一个新
/// 键，不封顶的话「限流表」自己就成了那个能把内存吃光的东西。满了之后
/// 不认识的来源一律先拒——这一条只在真的被打的时候才会生效。
const MAX_RATE_KEYS: usize = 4096;

#[derive(Default)]
pub struct Live {
    rooms: Mutex<HashMap<String, Session>>,
    /// 建房限流的账本，按来源记。跟 `rooms` 分开一把锁：它只在
    /// `POST /live/start` 那一条路上被碰，而 `rooms` 是每一帧都要碰的。
    starts: Mutex<HashMap<String, Starts>>,
}

impl Live {
    pub fn new() -> Self {
        Live::default()
    }

    /// 开一场直播（或者老师自己重开同一场）。
    ///
    /// **这一期还没有配对身份可验**，谁都能拿一个 id 来调这个方法——所以
    /// `id` 已经在播的时候，必须先证明「你就是当初开这场直播的那个人」才
    /// 允许顶掉它：带的 `push_secret` 要跟房间里存的那把对得上。不然任何
    /// 人拿老师正在播的那个 id 重开一次，就能把老师的画面换成自己的——
    /// 老师那边屏幕一切正常，学生看到的却是别人推的东西。校验放在这里
    /// （而不是路由层），是因为「同一把钥匙才能重开」是这个类型自己的
    /// 不变量，不是某一条路由的临时规矩。
    pub fn start(
        &self,
        id: String,
        viewer_token: String,
        push_secret: String,
        lanes: Vec<String>,
    ) -> Result<(), LinkError> {
        if lanes.is_empty() || lanes.len() > MAX_LANES {
            return Err(LinkError::TooBig);
        }
        let mut rooms = self.rooms.lock().expect("live 锁");
        // 常数时间比较（`same`），不能提前 return——理由跟 `authed` 一样：
        // 早退出的分支耗时不同，就是一个能拿响应时间探测「这把钥匙对不对」
        // 的边信道。
        let reopening = match rooms.get(&id) {
            Some(existing) => {
                if !same(&existing.push_hash, &hash(&push_secret)) {
                    return Err(LinkError::Unauthorized);
                }
                true
            }
            None => false,
        };
        // **房间数有上限。** `POST /live/start` 是 spec 里唯一要对公网开的
        // 路由，而这一期它没有配对身份可验：没有这条上限，一个脚本几秒钟
        // 就能把中转的内存吃光（见 `MAX_ROOMS` 的文档注释）。
        //
        // 只拦新开的那一种：老师带着同一把 push_secret 重开自己已经在播的
        // 那一场（改 lanes、断线重连）不占新名额，满了也不该把正在上课的
        // 人踢出去。
        if !reopening && rooms.len() >= dct_link::live::MAX_ROOMS {
            return Err(LinkError::QuotaExceeded);
        }
        let lanes = lanes
            .into_iter()
            .map(|name| Lane {
                name,
                frame: Vec::new(),
                etag: 0,
                tx: watch::channel(0).0,
            })
            .collect();
        rooms.insert(
            id,
            Session {
                viewer_hash: hash(&viewer_token),
                push_hash: hash(&push_secret),
                lanes,
                fed_at: Instant::now(),
                next_etag: 1,
            },
        );
        Ok(())
    }

    /// 记一次建房尝试，顺便判断这个来源是不是敲得太快了。
    ///
    /// **在 `start` 之前调**，路由层负责：房间总数有上限之后，剩下的攻击
    /// 是「把上限占满」——反复建房间让真正的老师开不了播。按来源节流拦的
    /// 是这个。
    ///
    /// `who` 是路由层算出来的来源标识（反代给的 `X-Forwarded-For` 头一跳，
    /// 没有就是对端地址）。它不是身份，只是一个够用的分桶依据：能换的人
    /// 自然换得动，但那时 `MAX_RATE_KEYS` 那条上限接着拦。
    ///
    /// 超了回 `Busy`（→ 429）。**不是 `Unauthorized`**：这不是"你没资格"，
    /// 是"等一下再来"，而这两句话该让调用方做的事完全不同。
    pub fn note_start(&self, who: &str, now: Instant) -> Result<(), LinkError> {
        let mut starts = self.starts.lock().expect("live 限流锁");
        // 过期的窗口先扫掉——不扫的话这张表只增不减，而键是请求方能随手
        // 换的东西。
        starts.retain(|_, s| now.duration_since(s.since) < dct_link::live::START_RATE_WINDOW);
        match starts.get_mut(who) {
            Some(s) => {
                if s.count >= dct_link::live::MAX_STARTS_PER_WINDOW {
                    return Err(LinkError::Busy);
                }
                s.count += 1;
            }
            None => {
                // 扫完还是满的：正在被人拿一堆假来源打。不认识的来源一律
                // 先拒，别让这张表自己成了那个吃内存的东西。
                if starts.len() >= MAX_RATE_KEYS {
                    return Err(LinkError::Busy);
                }
                starts.insert(who.to_string(), Starts { since: now, count: 1 });
            }
        }
        Ok(())
    }

    /// 推一帧。**只认 `push_secret`，`viewer_token` 在这里不管用**——那把
    /// 钥匙每个学生都有，认它就等于谁都能推假画面。
    ///
    /// 认不出来和这场直播根本不存在回同一句话，理由跟 `frame` 里的
    /// `authed` 一样：不能让人拿错误码把中转上正在播的号摸一遍。
    pub fn push(
        &self,
        id: &str,
        push_secret: &str,
        lane: usize,
        frame: Vec<u8>,
    ) -> Result<u64, LinkError> {
        if frame.len() > MAX_FRAME_BYTES {
            return Err(LinkError::TooBig);
        }
        let mut rooms = self.rooms.lock().expect("live 锁");
        let room = rooms.get_mut(id).ok_or(LinkError::Unauthorized)?;
        if !same(&room.push_hash, &hash(push_secret)) {
            return Err(LinkError::Unauthorized);
        }
        room.fed_at = Instant::now();
        let etag = room.next_etag;
        room.next_etag += 1;
        let slot = room.lanes.get_mut(lane).ok_or(LinkError::Offline)?;
        // 空 body = 保活，只续 `fed_at`，不动画面也不叫醒任何人。
        if frame.is_empty() {
            return Ok(slot.etag);
        }
        slot.frame = frame;
        slot.etag = etag;
        let _ = slot.tx.send(etag);
        Ok(etag)
    }

    pub fn frame(&self, id: &str, token: &str, lane: usize) -> Result<(Vec<u8>, u64), LinkError> {
        let rooms = self.rooms.lock().expect("live 锁");
        let room = authed(&rooms, id, token)?;
        let slot = room.lanes.get(lane).ok_or(LinkError::Unauthorized)?;
        Ok((slot.frame.clone(), slot.etag))
    }

    pub fn lanes(&self, id: &str, token: &str) -> Result<Vec<String>, LinkError> {
        let rooms = self.rooms.lock().expect("live 锁");
        let room = authed(&rooms, id, token)?;
        Ok(room.lanes.iter().map(|l| l.name.clone()).collect())
    }

    pub fn subscribe(&self, id: &str, lane: usize) -> Option<watch::Receiver<u64>> {
        let rooms = self.rooms.lock().expect("live 锁");
        Some(rooms.get(id)?.lanes.get(lane)?.tx.subscribe())
    }

    /// 在看的人数——**近似值**，数的是此刻挂在长轮询上的订阅数，不是精确
    /// 名册。这是老师那行常驻提示里的「7 人在看」，也是他唯一会一直看着的
    /// 那个数。
    ///
    /// 学生页一次只显示一路画面，所以按 lane 累加不会把一个人算成两个——
    /// 一个学生同一时刻只订阅自己正在看的那一路。
    ///
    /// 会偏低的情况：短轮询本身有间隙（拿到一帧、还没发起下一次订阅的那一
    /// 小段时间没人挂着），学生数会在这段间隙里被漏掉。这个近似是可以接受
    /// 的——老师要的是「有没有人、大概多少人」，不是逐个对花名册，为了这几
    /// 个名额去维护一份精确在线表不值得。
    pub fn viewers(&self, id: &str) -> u32 {
        let rooms = self.rooms.lock().expect("live 锁");
        rooms
            .get(id)
            .map(|r| r.lanes.iter().map(|l| l.tx.receiver_count() as u32).sum())
            .unwrap_or(0)
    }

    /// 老师停播。**跟推帧同一把 push secret**——理由对称：live-id 对每个
    /// 学生都是已知的，停播要是只认 id，随便一个学生打开 devtools 发一个
    /// `DELETE` 就能掐断全班的直播，单次请求、确定性拒绝服务，比伪造画面
    /// 帧还省事。认不出来和这场直播根本不存在回同一句话，理由同 `push`。
    pub fn stop(&self, id: &str, push_secret: &str) -> Result<(), LinkError> {
        let mut rooms = self.rooms.lock().expect("live 锁");
        match rooms.get(id) {
            Some(room) if same(&room.push_hash, &hash(push_secret)) => {
                rooms.remove(id);
                Ok(())
            }
            _ => Err(LinkError::Unauthorized),
        }
    }

    pub fn sweep(&self, now: Instant) {
        self.rooms
            .lock()
            .expect("live 锁")
            .retain(|_, r| now.duration_since(r.fed_at) < LIVE_TTL);
    }
}

/// 认证在取数之前，而且**认不出来和不存在回同一句话**。
///
/// 这里只认 `viewer_hash`——`push_secret` 不能拿来读，也没必要：老师端如果
/// 想看自己推的画面，走的是同一个学生页面、同一把 viewer token。
fn authed<'a>(
    rooms: &'a HashMap<String, Session>,
    id: &str,
    token: &str,
) -> Result<&'a Session, LinkError> {
    let room = rooms.get(id).ok_or(LinkError::Unauthorized)?;
    if !same(&room.viewer_hash, &hash(token)) {
        return Err(LinkError::Unauthorized);
    }
    Ok(room)
}

/// 常数时间比对，理由同 `web::mod` 里那一处：`==` 会按第一个不同的字节
/// 提前返回，而那个时间差是可测的。
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hash(token: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn started() -> Live {
        let live = Live::new();
        live.start(
            "abc".into(),
            "t".repeat(64),
            "p".repeat(64),
            vec!["前端".into(), "后端".into()],
        )
        .unwrap();
        live
    }

    #[test]
    fn a_frame_goes_in_and_comes_back_out_with_an_etag() {
        let live = started();
        let etag = live.push("abc", &"p".repeat(64), 0, b"hello".to_vec()).unwrap();
        let (body, tag) = live.frame("abc", &"t".repeat(64), 0).unwrap();
        assert_eq!(body, b"hello");
        assert_eq!(tag, etag);
    }

    /// 每换一帧 etag 必须变，否则学生那边的 304 会把新画面挡在外面。
    #[test]
    fn every_new_frame_gets_a_new_etag() {
        let live = started();
        let a = live.push("abc", &"p".repeat(64), 0, b"one".to_vec()).unwrap();
        let b = live.push("abc", &"p".repeat(64), 0, b"two".to_vec()).unwrap();
        assert_ne!(a, b);
    }

    /// 错的 token 一律 Unauthorized，**不告诉他这场直播存不存在**——
    /// 拿旧链接的人不该能把中转上正在播的号摸一遍。
    #[test]
    fn a_wrong_token_cannot_tell_a_live_apart_from_a_missing_one() {
        let live = started();
        live.push("abc", &"p".repeat(64), 0, b"x".to_vec()).unwrap();
        let wrong = live.frame("abc", &"w".repeat(64), 0).unwrap_err();
        let missing = live.frame("nope", &"t".repeat(64), 0).unwrap_err();
        assert_eq!(wrong, LinkError::Unauthorized);
        assert_eq!(missing, LinkError::Unauthorized);
    }

    /// 学生那把钥匙只能读。拿它去推帧必须失败——每个学生都知道 live-id，
    /// 要是推帧只认 id，随便一个学生就能往这场直播里推假画面，而这个功能的
    /// 全部前提是学生只读。
    #[test]
    fn a_viewer_token_cannot_push_a_frame() {
        let live = started();
        assert_eq!(
            live.push("abc", &"t".repeat(64), 0, "假画面".as_bytes().to_vec())
                .unwrap_err(),
            LinkError::Unauthorized
        );
    }

    #[test]
    fn a_frame_bigger_than_the_cap_is_refused() {
        let live = started();
        let big = vec![0u8; dct_link::live::MAX_FRAME_BYTES + 1];
        assert_eq!(
            live.push("abc", &"p".repeat(64), 0, big).unwrap_err(),
            LinkError::TooBig
        );
    }

    #[test]
    fn more_lanes_than_the_cap_is_refused() {
        let live = Live::new();
        let too_many = (0..dct_link::live::MAX_LANES + 1)
            .map(|i| format!("第 {i} 路"))
            .collect();
        assert_eq!(
            live.start("x".into(), "t".repeat(64), "p".repeat(64), too_many)
                .unwrap_err(),
            LinkError::TooBig
        );
    }

    /// 老师停推之后整条直播连同两把钥匙一起蒸发——只有 TTL 没有主动删，
    /// 拔网线就等于永远播着；只有主动删没有 TTL，合上笔记本就收不回来。
    #[test]
    fn a_live_that_stopped_being_fed_disappears_with_its_token() {
        let live = started();
        live.push("abc", &"p".repeat(64), 0, b"x".to_vec()).unwrap();
        live.sweep(Instant::now() + dct_link::live::LIVE_TTL + Duration::from_secs(1));
        assert_eq!(
            live.frame("abc", &"t".repeat(64), 0).unwrap_err(),
            LinkError::Unauthorized
        );
    }

    #[test]
    fn stopping_takes_it_away_immediately() {
        let live = started();
        live.stop("abc", &"p".repeat(64)).unwrap();
        assert_eq!(
            live.lanes("abc", &"t".repeat(64)).unwrap_err(),
            LinkError::Unauthorized
        );
    }

    /// 学生那把钥匙停不掉直播。live-id 对每个学生都是已知的，停播若只认
    /// id，随便一个学生就能掐断全班的课——单次请求的拒绝服务，比伪造画面
    /// 还省事。
    #[test]
    fn a_viewer_token_cannot_stop_the_live() {
        let live = started();
        assert_eq!(
            live.stop("abc", &"t".repeat(64)).unwrap_err(),
            LinkError::Unauthorized
        );
        // 停不掉：直播还活着。
        assert!(live.lanes("abc", &"t".repeat(64)).is_ok());
    }

    /// 在看的人数是挂着的订阅数：订阅两次数到 2，drop 掉一个之后数回 1。
    #[test]
    fn viewers_counts_currently_held_subscriptions() {
        let live = started();
        let a = live.subscribe("abc", 0).unwrap();
        let _b = live.subscribe("abc", 1).unwrap();
        assert_eq!(live.viewers("abc"), 2, "两个订阅应该数出 2 个人在看");
        drop(a);
        assert_eq!(live.viewers("abc"), 1, "掉了一个订阅之后应该数回 1");
    }

    /// 别人拿老师正在播的那个 id、换一把 push_secret 去 `start`，必须被拒
    /// ——不然任何知道 id 的人都能把老师正在播的那场顶掉，老师那边一切
    /// 正常，学生看到的却是别人推的画面。**而且第一场必须还活着**：被拒
    /// 的这次尝试不能把原来那场顺手抹掉。
    #[test]
    fn starting_with_the_wrong_secret_does_not_steal_an_existing_live() {
        let live = started();
        let err = live
            .start(
                "abc".into(),
                "t".repeat(64),
                "别人的钥匙".repeat(20),
                vec!["抢来的一路".into()],
            )
            .unwrap_err();
        assert_eq!(err, LinkError::Unauthorized);
        // 原来那场没被换掉：老师原来的 push_secret 还能推、原来的
        // viewer_token 还能读。
        assert!(live.push("abc", &"p".repeat(64), 0, b"still mine".to_vec()).is_ok());
        assert_eq!(
            live.frame("abc", &"t".repeat(64), 0).unwrap().0,
            b"still mine"
        );
    }

    /// 老师自己重开同一场（换上架列表、断线重连）：带着同一把 push_secret
    /// 必须放行，而且新的 lanes 要真的生效。
    #[test]
    fn starting_again_with_the_same_secret_replaces_the_lanes() {
        let live = started();
        live.start(
            "abc".into(),
            "t".repeat(64),
            "p".repeat(64),
            vec!["新的一路".into()],
        )
        .unwrap();
        assert_eq!(
            live.lanes("abc", &"t".repeat(64)).unwrap(),
            vec!["新的一路".to_string()]
        );
    }

    /// **房间数有上限。** `POST /live/start` 是 spec 里唯一要对公网开的
    /// 路由，而这一期它没有配对身份可验：没有这条上限，一个脚本几秒钟就能
    /// 把中转的内存吃光。
    #[test]
    fn the_relay_refuses_to_open_more_rooms_than_it_can_hold() {
        let live = Live::new();
        for i in 0..dct_link::live::MAX_ROOMS {
            live.start(
                format!("room{i}"),
                "t".repeat(64),
                "p".repeat(64),
                vec!["一路".into()],
            )
            .expect("上限之内该开得起来");
        }
        assert_eq!(
            live.start(
                "one-too-many".into(),
                "t".repeat(64),
                "p".repeat(64),
                vec!["一路".into()],
            )
            .unwrap_err(),
            LinkError::QuotaExceeded,
            "满了要回一个说得清的错误码"
        );
    }

    /// 满了也不许把正在上课的人踢出去：老师带着同一把 push_secret 重开
    /// 自己那一场（改 lanes、断线重连）不占新名额。
    #[test]
    fn a_full_relay_still_lets_an_existing_teacher_reopen_their_own_room() {
        let live = Live::new();
        for i in 0..dct_link::live::MAX_ROOMS {
            live.start(
                format!("room{i}"),
                "t".repeat(64),
                "p".repeat(64),
                vec!["一路".into()],
            )
            .unwrap();
        }
        assert!(
            live.start(
                "room0".into(),
                "t".repeat(64),
                "p".repeat(64),
                vec!["换了的一路".into()],
            )
            .is_ok(),
            "中转满了就连原来的老师都重开不了自己那一场，等于把正在上的课掐了"
        );
    }

    /// 按来源节流：房间总数有上限之后，剩下的攻击是「把上限占满」，让真正
    /// 的老师开不了播。
    #[test]
    fn one_source_cannot_keep_opening_rooms_forever() {
        let live = Live::new();
        let now = Instant::now();
        for _ in 0..dct_link::live::MAX_STARTS_PER_WINDOW {
            live.note_start("1.2.3.4", now).expect("窗口之内该放行");
        }
        assert_eq!(
            live.note_start("1.2.3.4", now).unwrap_err(),
            LinkError::Busy,
            "同一个来源在一个窗口里建房没有上限"
        );
    }

    /// 节流是按来源分桶的：一个人敲爆了不该连累别人。
    #[test]
    fn throttling_one_source_does_not_block_another() {
        let live = Live::new();
        let now = Instant::now();
        for _ in 0..dct_link::live::MAX_STARTS_PER_WINDOW {
            live.note_start("1.2.3.4", now).unwrap();
        }
        assert!(live.note_start("5.6.7.8", now).is_ok());
    }

    /// 窗口过去就翻篇——限流是"等一下再来"，不是"今天别来了"。
    #[test]
    fn the_throttle_window_rolls_over() {
        let live = Live::new();
        let now = Instant::now();
        for _ in 0..dct_link::live::MAX_STARTS_PER_WINDOW {
            live.note_start("1.2.3.4", now).unwrap();
        }
        let later = now + dct_link::live::START_RATE_WINDOW + Duration::from_secs(1);
        assert!(live.note_start("1.2.3.4", later).is_ok());
    }

    /// 挂着的学生要被新帧叫醒，这是 `?wait=1` 的全部机制。
    #[tokio::test]
    async fn a_waiting_viewer_is_woken_by_a_new_frame() {
        let live = started();
        let mut rx = live.subscribe("abc", 0).unwrap();
        live.push("abc", &"p".repeat(64), 0, b"new".to_vec()).unwrap();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), live.frame("abc", &"t".repeat(64), 0).unwrap().1);
    }
}
