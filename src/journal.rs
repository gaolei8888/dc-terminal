//! 会话生死簿：谁在什么时候起来的、又是**怎么**没的。
//!
//! 存在的理由是一个查不下去的 bug —— 用户报「换项目就把会话杀掉」，而
//! `dct ps` 里一个 `stopped` 说明不了任何事：会话变成 `Stopped` 有两条完全
//! 不同的路，
//!
//! - `requested` —— 有人显式发了 `Request::Stop`（界面按 `s`，或者 `dct stop`）
//! - `vanished` —— `tick()` 发现进程已经自己没了，只是过来收尸
//!
//! 两条路留下的痕迹一模一样，而该查哪一边完全取决于是哪一条。
//!
//! 它也不只是给这一个 bug 用的。守护进程活得比界面久，正是这个产品存在的
//! 理由 —— 于是「我不在的时候那个 agent 是怎么没的」本来就没有任何地方
//! 说得清。现在有了。
//!
//! **绝不 panic，绝不阻断调用方。** 记不下来是记账的事，不该连累会话。
//! 所有 IO 错误一律吞掉：一个写不进去的日志文件不值得让 `stop()` 失败。

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// 超过这个大小就从头写。守护进程一活好几天，日志不能无限长；
/// 而这是排查用的近况，旧的没有保留价值。
const MAX_BYTES: u64 = 256 * 1024;

/// 会话为什么变成 `Stopped`。**这个区分就是整个模块存在的理由**，
/// 别把两者合并成一句「stopped」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Death {
    /// 收到了 `Request::Stop`。要查就去查是谁发的。
    Requested,
    /// `tick()` 过来时进程已经没了。要查就去查它自己为什么退。
    Vanished,
}

impl fmt::Display for Death {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Death::Requested => "requested",
            Death::Vanished => "vanished",
        })
    }
}

/// 一本日志。`None` 路径 = 不记账，`SessionManager::new()` 的默认值——
/// 单元测试因此不会去写用户真实的 `~/.dct/sessions.log`。
#[derive(Debug, Default)]
pub struct Journal {
    path: Mutex<Option<PathBuf>>,
}

impl Journal {
    pub fn new() -> Journal {
        Journal::default()
    }

    pub fn set_path(&self, p: PathBuf) {
        *self.path.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
    }

    pub fn born(&self, id: u32, profile: &str, dir: &Path, pid: Option<u32>) {
        self.write(&format!(
            "session {id} born  profile={profile} pid={} dir={}",
            pid_word(pid),
            dir.display()
        ));
    }

    pub fn died(&self, id: u32, why: Death, pid: Option<u32>) {
        self.write(&format!(
            "session {id} stopped  why={why} pid={}",
            pid_word(pid)
        ));
    }

    fn write(&self, line: &str) {
        let guard = self.path.lock().unwrap_or_else(|e| e.into_inner());
        let Some(path) = guard.as_ref() else {
            return;
        };
        // 超长就整份换掉，不做滚动归档——排查看的是近况，
        // 而多留一份 `.log.1` 只是多一个没人读的文件。
        let truncate = std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES);
        let opened = std::fs::OpenOptions::new()
            .create(true)
            .append(!truncate)
            .write(true)
            .truncate(truncate)
            .open(path);
        if let Ok(mut f) = opened {
            let _ = writeln!(f, "{}  {line}", stamp());
        }
    }
}

fn pid_word(pid: Option<u32>) -> String {
    match pid {
        Some(p) => p.to_string(),
        // 拿不到 pid 本身就是线索：说明子进程已经被回收过一次了
        None => "gone".into(),
    }
}

/// `YYYY-MM-DD HH:MM:SSZ`（UTC）。自己算是因为不想为一行时间戳
/// 引入 chrono —— 这份日志是给人扫一眼对时间的，不做时区展示。
fn stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{y:04}-{mo:02}-{d:02} {:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant 的 `civil_from_days`：天数（1970-01-01 为 0）→ 年月日。
/// 直译，不是自创算法——闰年和世纪规则都在里面，别顺手"简化"。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 没设过路径就什么都不写。单元测试里的 `SessionManager` 走的是这条路，
    /// 它绝不能去碰用户真实的 `~/.dct/`。
    #[test]
    fn a_journal_without_a_path_writes_nothing() {
        let j = Journal::new();
        j.died(1, Death::Requested, Some(42));
        // 没有路径可查，能跑完不 panic 就是全部要求
    }

    /// **这个区分是整个模块的理由。** 两条路必须在日志里长得不一样，
    /// 否则「换项目杀会话」这种报告永远查不下去。
    #[test]
    fn the_journal_tells_a_kill_apart_from_a_death() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.log");
        let j = Journal::new();
        j.set_path(path.clone());

        j.died(1, Death::Requested, Some(11));
        j.died(2, Death::Vanished, None);

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "两条记录各占一行：\n{text}");
        assert!(lines[0].contains("session 1"), "{}", lines[0]);
        assert!(lines[0].contains("why=requested"), "{}", lines[0]);
        assert!(lines[1].contains("why=vanished"), "{}", lines[1]);
        assert!(
            lines[1].contains("pid=gone"),
            "拿不到 pid 本身就是线索，得写出来：{}",
            lines[1]
        );
    }

    #[test]
    fn births_and_deaths_land_in_the_same_file_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.log");
        let j = Journal::new();
        j.set_path(path.clone());

        j.born(7, "claude", Path::new("/w/a"), Some(99));
        j.died(7, Death::Vanished, Some(99));

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("born"), "{}", lines[0]);
        assert!(lines[0].contains("profile=claude"), "{}", lines[0]);
        assert!(lines[0].contains("dir=/w/a"), "{}", lines[0]);
        assert!(lines[1].contains("stopped"), "{}", lines[1]);
    }

    /// 时间戳错了的话，整份日志就没法跟「我刚才按了什么」对上，
    /// 而对时间正是它唯一的用法。
    #[test]
    fn the_stamp_matches_a_known_instant() {
        // 2026-08-05 00:00:00Z
        assert_eq!(civil_from_days(20_670), (2026, 8, 5));
        // 闰日和世纪规则：2000 是闰年，1900 不是
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn the_stamp_looks_like_a_timestamp() {
        let s = stamp();
        assert_eq!(s.len(), 20, "YYYY-MM-DD HH:MM:SSZ：{s}");
        assert!(s.ends_with('Z'), "{s}");
    }
}
