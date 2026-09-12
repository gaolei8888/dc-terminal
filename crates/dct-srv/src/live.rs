//! 直播：中转记住每一路的**最新一帧**，学生只读。
//!
//! # 这是对 spec 决定一的第一处偏离，写在这里免得将来有人读不出边界
//!
//! 原话是「中转只搬信封，不看里面」。这里多出来的是一个**角色**（记住最新
//! 一帧），不是一份**理解**：帧对中转始终是不透明字节，它不解析、不认识里面
//! 是 span 还是别的。有一条守卫钉着这一点（见 `lib.rs` 的
//! `the_relay_never_looks_inside_a_frame_either`）。
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

#[derive(Default)]
pub struct Live {
    rooms: Mutex<HashMap<String, Session>>,
}

impl Live {
    pub fn new() -> Self {
        Live::default()
    }

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
        let lanes = lanes
            .into_iter()
            .map(|name| Lane {
                name,
                frame: Vec::new(),
                etag: 0,
                tx: watch::channel(0).0,
            })
            .collect();
        let mut rooms = self.rooms.lock().expect("live 锁");
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

    /// 在看的人数 = 挂着的订阅数。这是老师那行常驻提示里的「7 人在看」，
    /// 也是他唯一会一直看着的那个数。
    pub fn viewers(&self, id: &str) -> u32 {
        let rooms = self.rooms.lock().expect("live 锁");
        rooms
            .get(id)
            .map(|r| r.lanes.iter().map(|l| l.tx.receiver_count() as u32).sum())
            .unwrap_or(0)
    }

    pub fn stop(&self, id: &str) {
        self.rooms.lock().expect("live 锁").remove(id);
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
        live.stop("abc");
        assert_eq!(
            live.lanes("abc", &"t".repeat(64)).unwrap_err(),
            LinkError::Unauthorized
        );
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
