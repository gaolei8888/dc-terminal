//! 平稳重启之后补投递的话：被打断且按 id 接回的会话收到「继续」，
//! 接不回但留了交接文件的会话收到「先读交接」。
//! 逻辑是纯的（`note_for` / `step`），线程和敲键由 `daemon.rs` 接上。
//! 设计见 `docs/superpowers/specs/2026-10-14-graceful-restart-design.md` 第 5、6 节。

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::last_sessions::RecordedSession;
use crate::session::SessionState;

pub const CONTINUE_TEXT: &str =
    "dct 重启完了。请先检查一下你做到哪了（看看文件和改动的状态），再接着做重启前没做完的事。";

/// 等会话第一次空闲，最多等这么久；过了就不打了，改成提示用户。
pub const NOTE_WAIT: Duration = Duration::from_secs(120);

pub fn read_handoff_text(path: &Path) -> String {
    format!(
        "dct 刚重启。你上次的交接在 {}，先读它，再接着做。",
        path.display()
    )
}

/// 一条等着投递的话。
#[derive(Debug, Clone)]
pub struct Note {
    pub id: u32,
    pub text: String,
    pub deadline: Instant,
    pub dir: String,
}

/// 一条恢复出来的会话该收到什么话：
/// - interrupted 且按 id 接回了 → `CONTINUE_TEXT`
/// - 有 handoff 且文件存在 → `read_handoff_text`
/// - 其余 → `None`
///
/// 被打断但没接回原对话的，叫它「继续」它也不知道继续什么，所以不打。
pub fn note_for(record: &RecordedSession, resumed_by_id: bool) -> Option<String> {
    if record.interrupted && resumed_by_id {
        return Some(CONTINUE_TEXT.to_string());
    }
    let h = record.handoff.as_ref()?;
    h.is_file().then(|| read_handoff_text(h))
}

/// 每拍：空闲的投递并移除；过了期限的移除并返回它们（给警告用）。
/// 会话已经不在名册里、或已经停了的，静默丢掉——比如按 id 接回失败后被
/// 换成了新开的会话（新 id），这句「继续」不能落到那个全新的对话上。
/// `send` 返回 false 表示没打进去，留着下拍再试。
pub fn step(
    notes: &mut Vec<Note>,
    now: Instant,
    states: &HashMap<u32, SessionState>,
    send: &mut dyn FnMut(u32, &str) -> bool,
) -> Vec<Note> {
    let mut dropped = Vec::new();
    let mut keep = Vec::new();
    for n in notes.drain(..) {
        match states.get(&n.id) {
            None | Some(SessionState::Stopped) => continue,
            Some(SessionState::Idle) if send(n.id, &n.text) => continue,
            _ => {}
        }
        if now >= n.deadline {
            dropped.push(n);
        } else {
            keep.push(n);
        }
    }
    *notes = keep;
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> RecordedSession {
        RecordedSession {
            dir: "/x".into(),
            profile: "p".into(),
            tag: String::new(),
            last_active: 1,
            conversation_id: Some("a".into()),
            interrupted: false,
            handoff: None,
        }
    }

    #[test]
    fn note_for_rules() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path().join("h.md");
        let mut r = record();
        assert_eq!(note_for(&r, true), None);
        r.interrupted = true;
        assert_eq!(note_for(&r, true).as_deref(), Some(CONTINUE_TEXT));
        assert_eq!(note_for(&r, false), None);
        r.interrupted = false;
        r.handoff = Some(h.clone());
        assert_eq!(note_for(&r, false), None, "文件不存在");
        std::fs::write(&h, "x").unwrap();
        assert_eq!(note_for(&r, false), Some(read_handoff_text(&h)));
    }

    fn note(id: u32, t0: Instant) -> Note {
        Note {
            id,
            text: format!("t{id}"),
            deadline: t0 + Duration::from_secs(120),
            dir: format!("/d{id}"),
        }
    }

    #[test]
    fn step_waits_for_idle_and_drops_after_deadline() {
        let t0 = Instant::now();
        let mut notes = vec![note(1, t0), note(2, t0)];
        let mut sent = vec![];
        let mut send = |id: u32, t: &str| {
            sent.push((id, t.to_string()));
            true
        };
        let states = HashMap::from([(1, SessionState::Working), (2, SessionState::Idle)]);
        assert!(step(&mut notes, t0, &states, &mut send).is_empty());
        let dropped = step(
            &mut notes,
            t0 + Duration::from_secs(120),
            &states,
            &mut send,
        );
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].id, 1);
        assert!(notes.is_empty());
        assert_eq!(sent, vec![(2, "t2".to_string())]);
    }

    #[test]
    fn failed_send_keeps_note_for_next_tick() {
        let t0 = Instant::now();
        let mut notes = vec![note(1, t0)];
        let states = HashMap::from([(1, SessionState::Idle)]);
        let mut send = |_: u32, _: &str| false;
        assert!(step(&mut notes, t0, &states, &mut send).is_empty());
        assert_eq!(notes.len(), 1);
        let mut send2 = |_: u32, _: &str| true;
        assert!(step(&mut notes, t0, &states, &mut send2).is_empty());
        assert!(notes.is_empty());
    }

    #[test]
    fn vanished_or_stopped_sessions_are_dropped_silently() {
        let t0 = Instant::now();
        let mut notes = vec![note(1, t0), note(2, t0)];
        let states = HashMap::from([(2, SessionState::Stopped)]);
        let mut send = |_: u32, _: &str| panic!("不该打");
        assert!(step(
            &mut notes,
            t0 + Duration::from_secs(500),
            &states,
            &mut send
        )
        .is_empty());
        assert!(notes.is_empty());
    }
}
