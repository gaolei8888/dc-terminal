//! 最近结束的会话：只在内存里记一阵子，不落盘。
//!
//! 会话一停（`stop`/`kill`/agent 自己退出）就从名册上抹掉，不留墓碑。可抹掉
//! 之后还有人会问到它：贴在会话里的界面下一帧的 `Screen`、`dct stop 3` 敲了
//! 两遍、重启后盯着接回会话的看守线程（它要分清「自己退出了」和「被人停掉
//! 了」）。这些问题要的不是一整个会话，只是「它结束了、怎么结束的」——
//! 这里就只记这么多。
//!
//! 只记 10 分钟、最多 200 条：问到它的那些地方都在结束后几秒内，记长了只是
//! 占内存；不落盘是因为守护进程一重启，旧编号对新进程就没有意义了。

use crate::proto::EndedHow;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// 记多久。
pub const KEEP_FOR: Duration = Duration::from_secs(10 * 60);
/// 最多记几条，多了先丢最早的。
pub const MAX_ENTRIES: usize = 200;

/// 一条记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub dir: PathBuf,
    pub profile: String,
    pub how: EndedHow,
    pub ended_at: Instant,
}

/// 见模块注释。时刻由调用方传进来（测试不用真等 10 分钟）。
#[derive(Debug)]
pub struct RecentlyEnded {
    keep_for: Duration,
    cap: usize,
    /// 按结束先后排，最早的在前面。
    entries: VecDeque<(u32, Ended)>,
}

impl Default for RecentlyEnded {
    fn default() -> Self {
        Self::with_limits(KEEP_FOR, MAX_ENTRIES)
    }
}

impl RecentlyEnded {
    pub fn with_limits(keep_for: Duration, cap: usize) -> Self {
        RecentlyEnded {
            keep_for,
            cap,
            entries: VecDeque::new(),
        }
    }

    /// 记下一个刚结束的会话。同一个编号再记一次就换成新的那条
    /// （编号可能被重启恢复时复用，见 `SessionManager::restore`）。
    pub fn record(&mut self, id: u32, e: Ended) {
        self.forget_expired(e.ended_at);
        self.entries.retain(|(i, _)| *i != id);
        self.entries.push_back((id, e));
        while self.entries.len() > self.cap {
            self.entries.pop_front();
        }
    }

    /// 这个编号最近结束过吗？过期的当没有。
    pub fn get(&self, id: u32, now: Instant) -> Option<&Ended> {
        self.entries
            .iter()
            .rev()
            .find(|(i, e)| *i == id && !self.expired(e, now))
            .map(|(_, e)| e)
    }

    fn expired(&self, e: &Ended, now: Instant) -> bool {
        now.saturating_duration_since(e.ended_at) >= self.keep_for
    }

    fn forget_expired(&mut self, now: Instant) {
        while self
            .entries
            .front()
            .is_some_and(|(_, e)| now.saturating_duration_since(e.ended_at) >= self.keep_for)
        {
            self.entries.pop_front();
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ended(how: EndedHow, at: Instant) -> Ended {
        Ended {
            dir: "/p".into(),
            profile: "claude".into(),
            how,
            ended_at: at,
        }
    }

    #[test]
    fn an_entry_is_kept_for_ten_minutes_and_then_forgotten() {
        let t0 = Instant::now();
        let mut r = RecentlyEnded::default();
        r.record(1, ended(EndedHow::StoppedByUser, t0));

        let just_before = t0 + KEEP_FOR - Duration::from_secs(1);
        assert_eq!(r.get(1, just_before).map(|e| e.how), Some(EndedHow::StoppedByUser));
        assert_eq!(r.get(1, t0 + KEEP_FOR), None, "满 10 分钟就不该再记得");
        assert_eq!(KEEP_FOR, Duration::from_secs(600));
    }

    #[test]
    fn expired_entries_are_dropped_when_a_new_one_is_recorded() {
        let t0 = Instant::now();
        let mut r = RecentlyEnded::default();
        r.record(1, ended(EndedHow::Killed, t0));
        r.record(2, ended(EndedHow::Killed, t0 + KEEP_FOR));
        assert_eq!(r.len(), 1, "过期的那条要真的丢掉，不只是查不到");
    }

    #[test]
    fn at_most_two_hundred_are_kept_and_the_oldest_go_first() {
        let t0 = Instant::now();
        let mut r = RecentlyEnded::default();
        for id in 1..=(MAX_ENTRIES as u32 + 1) {
            r.record(id, ended(EndedHow::ExitedOnItsOwn, t0));
        }
        assert_eq!(MAX_ENTRIES, 200);
        assert_eq!(r.len(), MAX_ENTRIES);
        assert_eq!(r.get(1, t0), None, "最早的那条该先丢");
        assert!(r.get(2, t0).is_some());
        assert!(r.get(MAX_ENTRIES as u32 + 1, t0).is_some());
    }

    #[test]
    fn recording_the_same_id_again_keeps_only_the_newest() {
        let t0 = Instant::now();
        let mut r = RecentlyEnded::default();
        r.record(5, ended(EndedHow::ExitedOnItsOwn, t0));
        r.record(5, ended(EndedHow::StoppedByUser, t0));
        assert_eq!(r.len(), 1);
        assert_eq!(r.get(5, t0).map(|e| e.how), Some(EndedHow::StoppedByUser));
    }
}
