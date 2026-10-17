//! 对话记忆：只在守护进程内存里，**永不落盘**（设计 §4）。没有摘要、没有向量、
//! 没有跨对话共享。本模块不做 IO，也不打印任何东西；时钟由调用方传进来，测试不用睡觉。

use std::time::{Duration, Instant};

pub const MAX_TURNS: usize = 6;
pub const MAX_CHARS: usize = 3000;
pub const MAX_CONVERSATIONS: usize = 4;
pub const IDLE: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub user: String,
    pub reply: String,
    /// 超时 / 断流时只说出了前几句：入记忆的是已说出的那几句。
    pub truncated: bool,
}

impl Turn {
    fn chars(&self) -> usize {
        self.user.chars().count() + self.reply.chars().count()
    }
}

struct Conv {
    id: String,
    turns: Vec<Turn>,
    /// 最近一次**入记忆**的时刻：失败 / 取消的轮次不续命。
    last: Instant,
}

#[derive(Default)]
pub struct Memory {
    convs: Vec<Conv>,
}

impl Memory {
    fn expire(&mut self, now: Instant) {
        self.convs.retain(|c| now.saturating_duration_since(c.last) <= IDLE);
    }

    /// 这个对话到现在为止的历史（最老的在前）。
    pub fn history(&mut self, id: &str, now: Instant) -> Vec<Turn> {
        self.expire(now);
        self.convs.iter().find(|c| c.id == id).map(|c| c.turns.clone()).unwrap_or_default()
    }

    /// 一轮说完了、没被取消：记下来，再按轮数 / 字符数从最老的丢，最多留 4 个对话（LRU）。
    pub fn commit(&mut self, id: &str, turn: Turn, now: Instant) {
        self.expire(now);
        let i = match self.convs.iter().position(|c| c.id == id) {
            Some(i) => i,
            None => {
                if self.convs.len() >= MAX_CONVERSATIONS {
                    let oldest = (0..self.convs.len()).min_by_key(|&i| self.convs[i].last).unwrap_or(0);
                    self.convs.remove(oldest);
                }
                self.convs.push(Conv { id: id.to_string(), turns: Vec::new(), last: now });
                self.convs.len() - 1
            }
        };
        let c = &mut self.convs[i];
        c.last = now;
        c.turns.push(turn);
        if c.turns.len() > MAX_TURNS {
            c.turns.drain(..c.turns.len() - MAX_TURNS);
        }
        // 总字符数超了就从最老的丢；单轮就超（不会发生，回复有 max_tokens）则整个对话清空。
        while c.turns.iter().map(Turn::chars).sum::<usize>() > MAX_CHARS {
            c.turns.remove(0);
        }
    }

    pub fn forget(&mut self, id: &str) {
        self.convs.retain(|c| c.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(u: &str, r: &str) -> Turn {
        Turn { user: u.into(), reply: r.into(), truncated: false }
    }

    fn users(m: &mut Memory, id: &str, now: Instant) -> Vec<String> {
        m.history(id, now).into_iter().map(|t| t.user).collect()
    }

    #[test]
    fn keeps_the_last_six_turns_oldest_first() {
        let (mut m, t0) = (Memory::default(), Instant::now());
        for i in 0..8 {
            m.commit("c", turn(&format!("q{i}"), "a"), t0);
        }
        assert_eq!(users(&mut m, "c", t0), ["q2", "q3", "q4", "q5", "q6", "q7"]);
    }

    #[test]
    fn drops_the_oldest_turns_when_over_3000_chars() {
        let (mut m, t0) = (Memory::default(), Instant::now());
        // 每轮 1000 字（汉字按字符算，不按字节）：只留得下 3 轮。
        for i in 0..5 {
            m.commit("c", turn(&format!("{i}"), &"字".repeat(999)), t0);
        }
        assert_eq!(users(&mut m, "c", t0), ["2", "3", "4"]);
    }

    #[test]
    fn a_conversation_idle_for_more_than_30_minutes_is_gone() {
        let (mut m, t0) = (Memory::default(), Instant::now());
        m.commit("c", turn("q", "a"), t0);
        assert_eq!(users(&mut m, "c", t0 + IDLE), ["q"]);
        assert!(m.history("c", t0 + IDLE + Duration::from_secs(1)).is_empty());
        // 再来一轮从头开始，不会把过期的接回来。
        m.commit("c", turn("new", "a"), t0 + IDLE + Duration::from_secs(2));
        assert_eq!(users(&mut m, "c", t0 + IDLE + Duration::from_secs(2)), ["new"]);
    }

    #[test]
    fn at_most_four_conversations_and_the_least_recent_goes_first() {
        let (mut m, t0) = (Memory::default(), Instant::now());
        for (i, id) in ["a", "b", "c", "d"].iter().enumerate() {
            m.commit(id, turn("q", "a"), t0 + Duration::from_secs(i as u64));
        }
        // a 又说了一轮，最久没动的变成 b。
        m.commit("a", turn("q2", "a"), t0 + Duration::from_secs(10));
        m.commit("e", turn("q", "a"), t0 + Duration::from_secs(11));
        let now = t0 + Duration::from_secs(12);
        assert!(m.history("b", now).is_empty());
        for id in ["a", "c", "d", "e"] {
            assert!(!m.history(id, now).is_empty(), "{id} 不该被挤掉");
        }
    }

    #[test]
    fn conversations_do_not_share_and_forget_clears_only_one() {
        let (mut m, t0) = (Memory::default(), Instant::now());
        m.commit("a", turn("qa", "x"), t0);
        m.commit("b", turn("qb", "x"), t0);
        m.forget("a");
        assert!(m.history("a", t0).is_empty());
        assert_eq!(users(&mut m, "b", t0), ["qb"]);
        m.forget("nope"); // 不存在也不出错
    }

    #[test]
    fn the_truncated_flag_is_kept() {
        let (mut m, t0) = (Memory::default(), Instant::now());
        m.commit("c", Turn { user: "q".into(), reply: "半".into(), truncated: true }, t0);
        assert!(m.history("c", t0)[0].truncated);
    }
}
