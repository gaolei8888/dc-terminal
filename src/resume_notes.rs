//! 平稳重启之后补投递的话：被打断且按 id 接回的会话收到「继续」，
//! 接不回但留了交接文件的会话收到「先读交接」。
//! 逻辑是纯的（`note_for` / `step`），线程和敲键由 `daemon.rs` 接上。
//! 设计见 `docs/superpowers/specs/2026-10-14-graceful-restart-design.md` 第 5、6 节。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::i18n::Lang;
use crate::last_sessions::RecordedSession;
use crate::session::SessionState;

pub const CONTINUE_TEXT: &str =
    "dct 重启完了。请先检查一下你做到哪了（看看文件和改动的状态），再接着做重启前没做完的事。";
const CONTINUE_TEXT_EN: &str = "dct has restarted. First check how far you got (look at the files and the state of your changes), \
     then carry on with what you had not finished before the restart.";

/// 敲 `dct restart` 的那个会话重启后收到的话。它没做完的事正是那条命令，
/// 叫它「继续」会让它再重启一次。
pub const INITIATOR_TEXT: &str =
    "你要的 dct 重启已经完成了，不用再重启。请接着做重启之前手上的事。";
const INITIATOR_TEXT_EN: &str = "The dct restart you asked for is done, so there is no need to restart again. \
     Please carry on with what you were doing before the restart.";

/// 等会话第一次空闲，最多等这么久；过了就不打了，改成提示用户。
pub const NOTE_WAIT: Duration = Duration::from_secs(120);

pub fn read_handoff_text(path: &Path) -> String {
    read_handoff_text_in(path, Lang::Zh)
}

pub fn read_handoff_text_in(path: &Path, lang: Lang) -> String {
    match lang {
        Lang::Zh => format!(
            "dct 刚重启。你上次的交接在 {}，先读它，再接着做。",
            path.display()
        ),
        Lang::En => format!(
            "dct has just restarted. Your handoff from last time is at {}. Read it first, then carry on.",
            path.display()
        ),
    }
}

/// 一条等着投递的话。
#[derive(Debug, Clone)]
pub struct Note {
    pub id: u32,
    pub text: String,
    pub deadline: Instant,
    pub dir: String,
    pub kind: NoteKind,
    /// 要等这个文件出现了才打（「先读交接」）。agent 可能在旧后台换掉之后
    /// 才写完——codex 的 TUI 被关掉，它背后的 `codex app-server` 还会把那一轮
    /// 做完（2026-10-15 实测）。到期限还没出现就作罢，提示用户。
    pub needs_file: Option<PathBuf>,
}

/// 这句话是哪一类——送不到时给用户的提示不一样。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    /// 重启后 dct 要说的（「继续」「先读交接」「重启完了」）。
    Note,
    /// 取消重启后的「dct 不重启了」。
    Cancel,
    /// 替别的电脑存着的留言。
    Message,
}

/// 一条重启后该说的话，和它要不要等交接文件出现。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub text: String,
    pub needs_file: Option<PathBuf>,
}

/// 替别的电脑存着的留言，重启后最多等这么久：会话可能先要处理「继续」，
/// 再干上一阵活才空下来。
pub const MESSAGE_WAIT: Duration = Duration::from_secs(30 * 60);

/// 一条恢复出来的会话该收到什么话：
/// - 敲 `dct restart` 的那个、按 id 接回了 → `INITIATOR_TEXT`（不是「继续」）
/// - interrupted 且按 id 接回了 → `CONTINUE_TEXT`
/// - 有 handoff → `read_handoff_text`，等那个文件出现了再打（`needs_file`）
/// - 其余 → `None`
///
/// 被打断但没接回原对话的，叫它「继续」它也不知道继续什么，所以不打。
pub fn note_for(record: &RecordedSession, resumed_by_id: bool, lang: Lang) -> Option<Planned> {
    let now = |text: &str| {
        Some(Planned {
            text: text.to_string(),
            needs_file: None,
        })
    };
    if record.restart_initiator && resumed_by_id {
        return now(match lang {
            Lang::Zh => INITIATOR_TEXT,
            Lang::En => INITIATOR_TEXT_EN,
        });
    }
    if record.interrupted && resumed_by_id {
        return now(match lang {
            Lang::Zh => CONTINUE_TEXT,
            Lang::En => CONTINUE_TEXT_EN,
        });
    }
    let h = record.handoff.as_ref()?;
    Some(Planned {
        text: read_handoff_text_in(h, lang),
        needs_file: Some(h.clone()),
    })
}

/// 这条会话重启后**什么都收不到、对话也没接回来**：被打断了，接不回原
/// 对话（没按 id 接回、也没带 `--continue`），又没留交接。用户得自己告诉它
/// 之前在做什么，要提示一声。敲 `dct restart` 的那个不算——命令当场已经
/// 提醒过它先自己写好交接。
pub fn lost_without_handoff(
    record: &RecordedSession,
    resumed_by_id: bool,
    continued: bool,
) -> bool {
    record.interrupted
        && !resumed_by_id
        && !continued
        && !record.restart_initiator
        && record.handoff.is_none()
}

/// 打一句话的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    /// 字和回车都进去了。
    Yes,
    /// 一个字都没进去，下拍可以再试。
    No,
    /// 字进去了、回车没成。**不能再打一遍**——再打字会叠在输入框里那一份
    /// 后面。这句话就此作罢，算没送到。
    Partial,
}

/// 每拍：准备好了的投递并移除；过了期限的移除并返回它们（给警告用）。
/// 会话已经不在名册里、或已经停了的，dct 自己的话静默丢掉——比如按 id 接回
/// 失败后被换成了新开的会话（新 id），这句「继续」不能落到那个全新的对话
/// 上。替别的电脑存着的留言不一样：丢了要告诉用户，所以也交出去。
///
/// 要等文件的（`needs_file`），`written` 答「在了」才打；到期限还没有就交出去。
///
/// 「准备好了」= 空闲（或者状态说不清：qwen 这种分不出忙闲的）**并且**
/// `ready(id)`（起来够久、键盘已交给 agent、空闲够久/屏幕够久没动，见
/// `SessionManager::ready_for_keys`）。同一个会话一拍最多打一句，下一句等它
/// 处理完这句、再次停下。
///
/// 注意：按 id 接回的会话，这句话可能在看守窗口（`RESUME_WATCH_WINDOW`）
/// 结束之前就打进去了。无妨：接回失败的会话几秒内就会退出，那时它要么
/// 还没准备好（不打），要么已经换成新 id 的新会话（这条按旧 id 静默丢掉）。
pub fn step(
    notes: &mut Vec<Note>,
    now: Instant,
    states: &HashMap<u32, SessionState>,
    ready: &dyn Fn(u32) -> bool,
    written: &dyn Fn(&Path) -> bool,
    send: &mut dyn FnMut(u32, &str) -> Sent,
) -> Vec<Note> {
    let mut dropped = Vec::new();
    let mut keep = Vec::new();
    let mut typed_this_tick = std::collections::HashSet::new();
    for n in notes.drain(..) {
        let file_ok = n.needs_file.as_deref().is_none_or(written);
        match states.get(&n.id) {
            None | Some(SessionState::Stopped) => {
                if n.kind == NoteKind::Message {
                    dropped.push(n);
                }
                continue;
            }
            Some(SessionState::Idle | SessionState::Unknown)
                if file_ok && !typed_this_tick.contains(&n.id) && ready(n.id) =>
            {
                match send(n.id, &n.text) {
                    Sent::Yes => {
                        typed_this_tick.insert(n.id);
                        continue;
                    }
                    Sent::Partial => {
                        typed_this_tick.insert(n.id);
                        dropped.push(n);
                        continue;
                    }
                    Sent::No => {}
                }
            }
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
            last_active: 1,
            conversation_id: Some("a".into()),
            ..Default::default()
        }
    }

    #[test]
    fn note_for_rules() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path().join("h.md");
        let zh = Lang::Zh;
        let mut r = record();
        let text = |p: Option<Planned>| p.map(|p| p.text);
        assert_eq!(note_for(&r, true, zh), None);
        r.interrupted = true;
        assert_eq!(text(note_for(&r, true, zh)).as_deref(), Some(CONTINUE_TEXT));
        assert_eq!(note_for(&r, true, zh).unwrap().needs_file, None);
        assert_eq!(note_for(&r, false, zh), None);
        r.interrupted = false;
        r.handoff = Some(h.clone());
        // 文件还没有也排上：等它出现再打（见 `step`）。
        assert_eq!(
            note_for(&r, false, zh),
            Some(Planned {
                text: read_handoff_text(&h),
                needs_file: Some(h.clone()),
            })
        );
        assert_eq!(
            text(note_for(&r, false, Lang::En)),
            Some(read_handoff_text_in(&h, Lang::En))
        );
    }

    /// I5：敲 `dct restart` 的那个会话（也被记成打断）重启后收到的是「重启
    /// 已经完成，不用再重启」，不是「继续」——它没做完的事正是那条命令。
    #[test]
    fn the_initiator_is_told_the_restart_is_done_not_to_continue() {
        let mut r = record();
        r.interrupted = true;
        r.restart_initiator = true;
        assert_eq!(
            note_for(&r, true, Lang::Zh).map(|p| p.text).as_deref(),
            Some(INITIATOR_TEXT)
        );
        assert_eq!(
            note_for(&r, true, Lang::En).map(|p| p.text).as_deref(),
            Some(INITIATOR_TEXT_EN)
        );
    }

    fn yes(_: u32) -> bool {
        true
    }

    fn note(id: u32, t0: Instant) -> Note {
        Note {
            id,
            text: format!("t{id}"),
            deadline: t0 + Duration::from_secs(120),
            dir: format!("/d{id}"),
            kind: NoteKind::Note,
            needs_file: None,
        }
    }

    fn there(_: &Path) -> bool {
        true
    }

    #[test]
    fn step_waits_for_idle_and_drops_after_deadline() {
        let t0 = Instant::now();
        let mut notes = vec![note(1, t0), note(2, t0)];
        let mut sent = vec![];
        let mut send = |id: u32, t: &str| {
            sent.push((id, t.to_string()));
            Sent::Yes
        };
        let states = HashMap::from([(1, SessionState::Working), (2, SessionState::Idle)]);
        assert!(step(&mut notes, t0, &states, &yes, &there, &mut send).is_empty());
        let dropped = step(
            &mut notes,
            t0 + Duration::from_secs(120),
            &states,
            &yes,
            &there,
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
        let mut send = |_: u32, _: &str| Sent::No;
        assert!(step(&mut notes, t0, &states, &yes, &there, &mut send).is_empty());
        assert_eq!(notes.len(), 1);
        let mut send2 = |_: u32, _: &str| Sent::Yes;
        assert!(step(&mut notes, t0, &states, &yes, &there, &mut send2).is_empty());
        assert!(notes.is_empty());
    }

    #[test]
    fn vanished_or_stopped_sessions_are_dropped_silently() {
        let t0 = Instant::now();
        let mut notes = vec![note(1, t0), note(2, t0)];
        let states = HashMap::from([(2, SessionState::Stopped)]);
        let mut send = |_: u32, _: &str| -> Sent { panic!("不该打") };
        assert!(step(
            &mut notes,
            t0 + Duration::from_secs(500),
            &states,
            &yes,
            &there,
            &mut send
        )
        .is_empty());
        assert!(notes.is_empty());
    }

    /// 没准备好（刚起来、键盘还没交给 agent、空闲不够久）就不打，等下一拍；
    /// 准备好了才打。
    #[test]
    fn idle_but_not_ready_waits() {
        let t0 = Instant::now();
        let mut notes = vec![note(1, t0)];
        let states = HashMap::from([(1, SessionState::Idle)]);
        let mut sent = 0;
        let mut send = |_: u32, _: &str| {
            sent += 1;
            Sent::Yes
        };
        assert!(step(&mut notes, t0, &states, &|_| false, &there, &mut send).is_empty());
        assert_eq!(notes.len(), 1);
        assert!(step(&mut notes, t0, &states, &yes, &there, &mut send).is_empty());
        assert!(notes.is_empty());
        assert_eq!(sent, 1);
    }

    /// 字进去了、回车没成：不再打第二遍（会叠在输入框里），当没送到交出去。
    #[test]
    fn partial_send_is_never_retried() {
        let t0 = Instant::now();
        let mut notes = vec![note(1, t0)];
        let states = HashMap::from([(1, SessionState::Idle)]);
        let mut calls = 0;
        let mut send = |_: u32, _: &str| {
            calls += 1;
            Sent::Partial
        };
        let dropped = step(&mut notes, t0, &states, &yes, &there, &mut send);
        assert_eq!(dropped.len(), 1);
        assert!(notes.is_empty());
        step(&mut notes, t0, &states, &yes, &there, &mut send);
        assert_eq!(calls, 1);
    }

    /// 分不出忙闲的会话（qwen 这种）：准备好了照样打，不会干等到期限被丢掉。
    #[test]
    fn unknown_state_sessions_get_their_note_once_ready() {
        let t0 = Instant::now();
        let mut notes = vec![note(1, t0)];
        let states = HashMap::from([(1, SessionState::Unknown)]);
        let mut sent = vec![];
        let mut send = |id: u32, _: &str| {
            sent.push(id);
            Sent::Yes
        };
        assert!(step(&mut notes, t0, &states, &yes, &there, &mut send).is_empty());
        assert_eq!(sent, vec![1]);
    }

    /// 同一个会话一拍只打一句：两句一起打，第二句会落进它正在处理第一句的时候。
    #[test]
    fn one_note_per_session_per_tick() {
        let t0 = Instant::now();
        let mut notes = vec![note(1, t0), note(1, t0)];
        let states = HashMap::from([(1, SessionState::Idle)]);
        let mut sent = 0;
        let mut send = |_: u32, _: &str| {
            sent += 1;
            Sent::Yes
        };
        step(&mut notes, t0, &states, &yes, &there, &mut send);
        assert_eq!(sent, 1);
        assert_eq!(notes.len(), 1);
    }

    /// 「先读交接」等交接文件出现了才打：codex 在旧后台换掉之后才写完的，
    /// 重启后接着看，到期限都没出现才作罢。
    #[test]
    fn a_handoff_note_waits_for_its_file_until_the_deadline() {
        let t0 = Instant::now();
        let mut n = note(1, t0);
        n.needs_file = Some("/h/1.md".into());
        let states = HashMap::from([(1, SessionState::Idle)]);
        let sent = std::cell::Cell::new(0);
        let mut send = |_: u32, _: &str| {
            sent.set(sent.get() + 1);
            Sent::Yes
        };
        let absent = |_: &Path| false;
        let mut notes = vec![n.clone()];
        assert!(step(&mut notes, t0, &states, &yes, &absent, &mut send).is_empty());
        assert_eq!(notes.len(), 1, "文件还没有：留着下一拍再看");
        assert!(step(&mut notes, t0, &states, &yes, &there, &mut send).is_empty());
        assert!(notes.is_empty());
        assert_eq!(sent.get(), 1);

        let mut notes = vec![n];
        let dropped = step(
            &mut notes,
            t0 + Duration::from_secs(120),
            &states,
            &yes,
            &absent,
            &mut send,
        );
        assert_eq!(dropped.len(), 1, "到期限还没有：交出去给警告用");
        assert_eq!(sent.get(), 1);
    }

    /// N6：替别的电脑存着的留言，会话没了（接回失败被换掉）也不能静默丢掉。
    #[test]
    fn a_held_message_for_a_vanished_session_is_reported_dropped() {
        let t0 = Instant::now();
        let mut msg = note(1, t0);
        msg.kind = NoteKind::Message;
        let mut notes = vec![msg, note(1, t0)];
        let mut send = |_: u32, _: &str| -> Sent { panic!("不该打") };
        let dropped = step(&mut notes, t0, &HashMap::new(), &yes, &there, &mut send);
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].kind, NoteKind::Message);
        assert!(notes.is_empty());
    }

    #[test]
    fn lost_without_handoff_rules() {
        let mut r = record();
        assert!(!lost_without_handoff(&r, false, false), "没被打断");
        r.interrupted = true;
        assert!(lost_without_handoff(&r, false, false));
        assert!(!lost_without_handoff(&r, true, false), "按 id 接回了");
        assert!(
            !lost_without_handoff(&r, false, true),
            "带 --continue 接回了"
        );
        r.handoff = Some("/h/1.md".into());
        assert!(
            !lost_without_handoff(&r, false, false),
            "有交接（文件没出现另有提示）"
        );
        r.handoff = None;
        r.restart_initiator = true;
        assert!(
            !lost_without_handoff(&r, false, false),
            "发起者已经当场提醒过"
        );
    }
}
