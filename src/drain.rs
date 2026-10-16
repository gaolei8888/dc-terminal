//! 排空状态机：优雅重启时决定「继续等 / 催一句 / 请它写交接 / 结束」。
//! 纯逻辑，不碰 IO；守护线程每拍调 `step`，把返回的 `Action` 去执行。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::i18n::Lang;
use crate::journal::civil_from_days;
use crate::session::SessionState;

/// 请它写交接后，至少过这么久还没见它忙起来，就当它不打算写了。
const HANDOFF_GRACE: Duration = Duration::from_secs(10);

pub struct DrainConfig {
    pub wait: Duration,
    pub nudge_wait: Duration,
    /// `dct restart --now`：什么都不等、不请谁写交接、不催谁，直接记状态
    /// 重启。还在忙的记成被打断。
    pub now: bool,
}

impl Default for DrainConfig {
    fn default() -> Self {
        DrainConfig {
            wait: Duration::from_secs(15 * 60),
            nudge_wait: Duration::from_secs(3 * 60),
            now: false,
        }
    }
}

/// 排空开始那一刻对每个会话的认识。
pub struct Member {
    pub id: u32,
    /// 能按 id 接回对话（`Profile::resumes_by_id`）。
    pub resumable: bool,
    /// 是 agent。非 agent（shell）不等、不发话。
    pub is_agent: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// 接不回的会话：请它写交接到这个文件。
    AskHandoff { id: u32, path: PathBuf },
    /// 到点还在忙：请它在安全处停下。
    Nudge { id: u32 },
    /// 排空结束，可以落盘重启了。`initiator` 是敲 `dct restart` 的那个会话，
    /// 重启后它收到的是「重启完了，不用再重启」，不是「继续」。
    Finish {
        interrupted: HashSet<u32>,
        handoffs: HashMap<u32, PathBuf>,
        initiator: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// 还在等的会话 id（从小到大）。
    pub waiting: Vec<u32>,
    /// 距离下一个时限还剩多少秒。
    pub remaining_s: u64,
    /// 已经进入兜底阶段。
    pub nudged: bool,
}

#[derive(Clone, Copy)]
enum Slot {
    Waiting,
    /// 已请它写交接；见过它忙起来（或过了宽限）再停下才算写完。
    HandoffAsked {
        asked_at: Instant,
        seen_busy: bool,
    },
    Done,
}

pub struct Drain {
    /// 按 id 从小到大排好，保证输出顺序确定。
    slots: Vec<(Member, Slot)>,
    cfg: DrainConfig,
    handoff_dir: PathBuf,
    deadline: Instant,
    nudged_at: Option<Instant>,
    /// 到点该催、还没催出去的（还没准备好接按键）。
    nudge_due: Vec<u32>,
    interrupted: HashSet<u32>,
    handoffs: HashMap<u32, PathBuf>,
    initiator: Option<u32>,
    finished: bool,
}

fn stopped(states: &HashMap<u32, SessionState>, id: u32) -> bool {
    match states.get(&id) {
        None => true,
        Some(s) => matches!(
            s,
            SessionState::Idle | SessionState::Stopped | SessionState::Failed
        ),
    }
}

impl Drain {
    pub fn new(
        mut members: Vec<Member>,
        initiator: Option<u32>,
        cfg: DrainConfig,
        handoff_dir: PathBuf,
        now: Instant,
    ) -> Drain {
        members.sort_by_key(|m| m.id);
        // 发起者在等命令跑完，永远不会空闲：不等、不发话，无条件记为打断，重启后叫它继续
        let interrupted: HashSet<u32> = initiator.into_iter().collect();
        let slots = members
            .into_iter()
            .map(|m| {
                let slot = if Some(m.id) == initiator || !m.is_agent {
                    Slot::Done
                } else {
                    Slot::Waiting
                };
                (m, slot)
            })
            .collect();
        Drain {
            slots,
            deadline: now + cfg.wait,
            cfg,
            handoff_dir,
            nudged_at: None,
            nudge_due: Vec::new(),
            interrupted,
            handoffs: HashMap::new(),
            initiator,
            finished: false,
        }
    }

    /// 每拍调一次。`states` 是此刻各会话状态（不在表里 = 会话已经没了，算停下）。
    /// `ready(id)`：现在能不能往这个会话里打字（`SessionManager::ready_for_keys`）。
    /// 请写交接、催停下这两句都要等它答「能」才发；没准备好就下一拍再看。
    pub fn step(
        &mut self,
        now: Instant,
        states: &HashMap<u32, SessionState>,
        ready: &dyn Fn(u32) -> bool,
    ) -> Vec<Action> {
        if self.finished {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (m, slot) in self.slots.iter_mut() {
            let is_stopped = stopped(states, m.id);
            match *slot {
                Slot::Done => {}
                Slot::Waiting if is_stopped => {
                    // 能接回的不用交接；已经死了/没了/报错的没法再往里打字，也不用等。
                    // `--now` 不请谁写交接。
                    let idle = matches!(states.get(&m.id), Some(SessionState::Idle));
                    if m.resumable || !idle || self.cfg.now {
                        *slot = Slot::Done;
                    } else if !ready(m.id) {
                        // 看着空闲，但还没准备好接按键（刚起来、空闲不够久）：
                        // 这一拍先不打，接着等。
                    } else {
                        let path = handoff_path(&self.handoff_dir, m.id, &file_stamp());
                        self.handoffs.insert(m.id, path.clone());
                        out.push(Action::AskHandoff { id: m.id, path });
                        *slot = Slot::HandoffAsked {
                            asked_at: now,
                            seen_busy: false,
                        };
                    }
                }
                // 分不出忙闲的会话（profile 没给忙/闲的标记，比如 qwen）：dct 看
                // 不出它什么时候停下，等多久都白等，也不该对着可能空闲的它说
                // 「请停下」。不等、不催，记成被打断——重启后叫它检查一下再继续。
                Slot::Waiting if matches!(states.get(&m.id), Some(SessionState::Unknown)) => {
                    *slot = Slot::Done;
                    self.interrupted.insert(m.id);
                }
                Slot::Waiting => {}
                Slot::HandoffAsked {
                    asked_at,
                    seen_busy,
                } => {
                    if !is_stopped {
                        *slot = Slot::HandoffAsked {
                            asked_at,
                            seen_busy: true,
                        };
                    } else if seen_busy || now.saturating_duration_since(asked_at) >= HANDOFF_GRACE
                    {
                        *slot = Slot::Done;
                    }
                }
            }
        }

        // 到点还没停的：催一句，进入兜底阶段
        if self.nudged_at.is_none()
            && now >= self.deadline
            && self.slots.iter().any(|(_, s)| !matches!(s, Slot::Done))
        {
            self.nudged_at = Some(now);
            for (m, slot) in &self.slots {
                // 只有被催的（到点还在干活的）才算打断，不管之后停没停。
                // 正在写交接的不催：它本来是空闲的，没被打断。空闲着、还在等
                // 准备好再请它写交接的也不催。`--now` 谁都不催。
                if matches!(slot, Slot::Waiting) && !stopped(states, m.id) {
                    self.interrupted.insert(m.id);
                    if !self.cfg.now {
                        self.nudge_due.push(m.id);
                    }
                }
            }
        }
        // 催的那句也等它准备好接按键再打；已经自己停下的就不用催了。
        self.nudge_due.retain(|&id| {
            if stopped(states, id) {
                return false;
            }
            if ready(id) {
                out.push(Action::Nudge { id });
                return false;
            }
            true
        });

        let all_done = self.slots.iter().all(|(_, s)| matches!(s, Slot::Done));
        let hard = self
            .nudged_at
            .is_some_and(|t| now >= t + self.cfg.nudge_wait);
        if all_done || hard {
            for (m, slot) in &self.slots {
                if !matches!(slot, Slot::Done) {
                    self.interrupted.insert(m.id);
                }
            }
            self.finished = true;
            out.push(Action::Finish {
                interrupted: self.interrupted.clone(),
                handoffs: self.handoffs.clone(),
                initiator: self.initiator,
            });
        }
        out
    }

    pub fn progress(&self, now: Instant) -> Progress {
        let limit = match self.nudged_at {
            Some(t) => t + self.cfg.nudge_wait,
            None => self.deadline,
        };
        Progress {
            waiting: self
                .slots
                .iter()
                .filter(|(_, s)| !matches!(s, Slot::Done))
                .map(|(m, _)| m.id)
                .collect(),
            remaining_s: limit.saturating_duration_since(now).as_secs(),
            nudged: self.nudged_at.is_some(),
        }
    }

    pub fn finished(&self) -> bool {
        self.finished
    }
}

/// 交接文件名：`<handoff_dir>/<id>-<YYYYmmdd-HHMMSS>.md`（时间由调用方给，便于测试）。
pub fn handoff_path(dir: &Path, id: u32, stamp: &str) -> PathBuf {
    dir.join(format!("{id}-{stamp}.md"))
}

/// 现在的本地时间，`YYYYmmdd-HHMMSS`。文件名是给用户看的：用 UTC 的话，
/// 晚上写的交接文件名会是「明天」。
fn file_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    stamp_of(secs as i64 + utc_offset_secs(secs))
}

/// 一个「本地时间的秒数」→ `YYYYmmdd-HHMMSS`。
fn stamp_of(local_secs: i64) -> String {
    let days = local_secs.div_euclid(86_400);
    let rem = local_secs.rem_euclid(86_400);
    let (y, mo, d) = civil_from_days(days);
    format!(
        "{y:04}{mo:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 本地时区在 `secs` 那一刻比 UTC 快多少秒。
#[cfg(unix)]
fn utc_offset_secs(secs: u64) -> i64 {
    let t = secs as libc::time_t;
    // SAFETY: `tm` 是纯数据结构，全零合法；`localtime_r` 只写它、不留指针。
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i64
}

/// Windows 上没有现成的一句话可问，先用 UTC（文件名照样唯一，只是钟点
/// 不是本地的）。
#[cfg(not(unix))]
fn utc_offset_secs(_secs: u64) -> i64 {
    0
}

pub const NUDGE_TEXT: &str =
    "dct 马上要重启。请在安全的地方停下手上的事，然后用一句话写下你做到哪了、下一步是什么。";
const NUDGE_TEXT_EN: &str = "dct is about to restart. Please stop what you are doing at a safe point, \
     then write one sentence on how far you got and what the next step is.";

/// 到点还在忙的会话收到的那句。语言跟着用户在设置里选的（见 `agent_lang`）。
pub fn nudge_text(lang: Lang) -> &'static str {
    match lang {
        Lang::Zh => NUDGE_TEXT,
        Lang::En => NUDGE_TEXT_EN,
    }
}

pub fn handoff_text(path: &Path) -> String {
    handoff_text_in(path, Lang::Zh)
}

pub fn handoff_text_in(path: &Path, lang: Lang) -> String {
    match lang {
        Lang::Zh => format!(
            "dct 马上要重启，这个会话的对话接不回来。请把要接着做的事写进 `{}`：在做什么、做到哪了、下一步、要注意的坑。写完就停下。",
            path.display()
        ),
        Lang::En => format!(
            "dct is about to restart and this session's conversation cannot be brought back. \
             Please write what is needed to carry on into `{}`: what you are doing, how far you got, \
             the next step, and any pitfalls. Stop once it is written.",
            path.display()
        ),
    }
}

pub const CANCEL_TEXT: &str = "dct 不重启了，请接着做刚才的事。";
const CANCEL_TEXT_EN: &str = "dct is not restarting after all. Please carry on with what you were doing.";

/// 请过写交接、催过停下之后又取消了重启：告诉它们接着做。
pub fn cancel_text(lang: Lang) -> &'static str {
    match lang {
        Lang::Zh => CANCEL_TEXT,
        Lang::En => CANCEL_TEXT_EN,
    }
}

/// dct 自己打给 agent 的那几句话用哪种语言：用户在设置里选过的；没选过
/// 用中文。**不看环境变量**：后台的环境是被谁拉起来就继承谁的，跟用户眼前
/// 那个界面不一定一样。
pub fn agent_lang(socket: &Path) -> Lang {
    crate::settings::load_lang(&crate::settings::settings_path_for_socket(socket)).unwrap_or(Lang::Zh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use SessionState::*;

    fn cfg() -> DrainConfig {
        DrainConfig {
            wait: Duration::from_secs(900),
            nudge_wait: Duration::from_secs(180),
            now: false,
        }
    }
    fn m(id: u32, resumable: bool) -> Member {
        Member {
            id,
            resumable,
            is_agent: true,
        }
    }
    fn st(v: &[(u32, SessionState)]) -> HashMap<u32, SessionState> {
        v.iter().cloned().collect()
    }
    fn finish(a: &[Action]) -> Option<(&HashSet<u32>, &HashMap<u32, PathBuf>)> {
        a.iter().find_map(|x| match x {
            Action::Finish {
                interrupted,
                handoffs,
                ..
            } => Some((interrupted, handoffs)),
            _ => None,
        })
    }

    impl Drain {
        fn step_all_ready(
            &mut self,
            now: Instant,
            states: &HashMap<u32, SessionState>,
        ) -> Vec<Action> {
            self.step(now, states, &|_| true)
        }
    }

    #[test]
    fn all_idle_finishes_immediately_with_no_interruptions() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true), m(2, true)], None, cfg(), "/h".into(), t0);
        let a = d.step_all_ready(t0, &st(&[(1, Idle), (2, Idle)]));
        let (i, h) = finish(&a).unwrap();
        assert!(i.is_empty() && h.is_empty());
        assert!(d.finished());
        assert!(d.step_all_ready(t0, &st(&[])).is_empty());
    }

    #[test]
    fn waits_for_working_then_finishes_when_it_stops() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true)], None, cfg(), "/h".into(), t0);
        assert!(d.step_all_ready(t0, &st(&[(1, Working)])).is_empty());
        assert_eq!(d.progress(t0).waiting, vec![1]);
        let a = d.step_all_ready(t0 + Duration::from_secs(60), &st(&[(1, Idle)]));
        assert!(finish(&a).unwrap().0.is_empty());
    }

    #[test]
    fn nudges_at_deadline_and_marks_interrupted_even_if_it_then_stops() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true), m(2, true)], None, cfg(), "/h".into(), t0);
        d.step_all_ready(t0, &st(&[(1, Working), (2, Idle)]));
        let a = d.step_all_ready(
            t0 + Duration::from_secs(900),
            &st(&[(1, Working), (2, Idle)]),
        );
        assert_eq!(a, vec![Action::Nudge { id: 1 }]);
        assert!(d.progress(t0 + Duration::from_secs(900)).nudged);
        // 兜底话发出去后，会话会先变忙、处理完再变空闲；即使最后停了，也已经被打断过
        d.step_all_ready(
            t0 + Duration::from_secs(910),
            &st(&[(1, Working), (2, Idle)]),
        );
        let a = d.step_all_ready(t0 + Duration::from_secs(950), &st(&[(1, Idle), (2, Idle)]));
        let (i, _) = finish(&a).unwrap();
        assert_eq!(i, &HashSet::from([1]));
    }

    #[test]
    fn hard_deadline_after_nudge_finishes_anyway() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true)], None, cfg(), "/h".into(), t0);
        d.step_all_ready(t0 + Duration::from_secs(900), &st(&[(1, Working)]));
        assert!(d
            .step_all_ready(t0 + Duration::from_secs(1079), &st(&[(1, Working)]))
            .is_empty());
        let a = d.step_all_ready(t0 + Duration::from_secs(1080), &st(&[(1, Working)]));
        assert_eq!(finish(&a).unwrap().0, &HashSet::from([1]));
    }

    #[test]
    fn initiator_is_not_waited_for_and_counts_as_interrupted() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(7, true)], Some(7), cfg(), "/h".into(), t0);
        let a = d.step_all_ready(t0, &st(&[(7, Working)]));
        assert_eq!(finish(&a).unwrap().0, &HashSet::from([7]));
        assert!(!a.iter().any(|x| matches!(x, Action::Nudge { .. })));
    }

    #[test]
    fn unresumable_idle_session_is_asked_for_handoff_then_waited_again() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(3, false)], None, cfg(), "/h".into(), t0);
        let a = d.step_all_ready(t0, &st(&[(3, Idle)]));
        assert!(matches!(&a[..], [Action::AskHandoff { id: 3, .. }]));
        // 它开始写交接
        assert!(d
            .step_all_ready(t0 + Duration::from_secs(1), &st(&[(3, Working)]))
            .is_empty());
        let a = d.step_all_ready(t0 + Duration::from_secs(30), &st(&[(3, Idle)]));
        let (i, h) = finish(&a).unwrap();
        assert!(i.is_empty());
        assert!(h.contains_key(&3));
    }

    #[test]
    fn unresumable_busy_session_is_asked_only_after_it_stops() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(3, false)], None, cfg(), "/h".into(), t0);
        assert!(d.step_all_ready(t0, &st(&[(3, Working)])).is_empty());
        let a = d.step_all_ready(t0 + Duration::from_secs(5), &st(&[(3, Idle)]));
        assert!(matches!(&a[..], [Action::AskHandoff { id: 3, .. }]));
    }

    #[test]
    fn vanished_or_failed_sessions_count_as_stopped_and_non_agents_are_ignored() {
        let t0 = Instant::now();
        let shell = Member {
            id: 9,
            resumable: false,
            is_agent: false,
        };
        let mut d = Drain::new(
            vec![m(1, true), m(2, true), shell],
            None,
            cfg(),
            "/h".into(),
            t0,
        );
        let a = d.step_all_ready(t0, &st(&[(2, Failed), (9, Working)]));
        assert!(finish(&a).is_some());
        assert!(!a
            .iter()
            .any(|x| matches!(x, Action::AskHandoff { id: 9, .. })));
    }

    #[test]
    fn handoff_path_is_under_dir() {
        assert_eq!(
            handoff_path(Path::new("/h"), 3, "20261014-120000"),
            PathBuf::from("/h/3-20261014-120000.md")
        );
    }

    #[test]
    fn handoff_not_considered_written_in_the_same_breath() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(3, false)], None, cfg(), "/h".into(), t0);
        d.step_all_ready(t0, &st(&[(3, Idle)]));
        // 屏幕还没来得及变忙
        assert!(d
            .step_all_ready(t0 + Duration::from_millis(500), &st(&[(3, Idle)]))
            .is_empty());
        // 10 秒还没变忙，当它不打算写了，算完
        assert!(finish(&d.step_all_ready(t0 + Duration::from_secs(10), &st(&[(3, Idle)]))).is_some());
    }

    #[test]
    fn stamp_of_formats_civil_time() {
        assert_eq!(stamp_of(0), "19700101-000000");
        assert_eq!(stamp_of(86_400 + 3_661), "19700102-010101");
    }

    /// 交接文件名用本地时间：跟系统 `date` 说的钟点一致。
    #[test]
    #[cfg(unix)]
    fn file_stamp_is_local_time() {
        for _ in 0..3 {
            let out = std::process::Command::new("date")
                .arg("+%Y%m%d-%H%M")
                .output()
                .unwrap();
            let want = String::from_utf8(out.stdout).unwrap().trim().to_string();
            if file_stamp().starts_with(&want) {
                return;
            }
            // 正好跨了一分钟，再来一次。
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("文件名里的时间跟本地时间对不上：{}", file_stamp());
    }

    #[test]
    fn file_stamp_has_the_documented_shape() {
        let s = file_stamp();
        assert_eq!(s.len(), 15);
        assert_eq!(s.as_bytes()[8], b'-');
        assert!(s
            .chars()
            .enumerate()
            .all(|(i, c)| i == 8 || c.is_ascii_digit()));
    }

    #[test]
    fn initiator_is_interrupted_even_if_not_a_member() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true)], Some(7), cfg(), "/h".into(), t0);
        let a = d.step_all_ready(t0, &st(&[(1, Idle)]));
        assert_eq!(finish(&a).unwrap().0, &HashSet::from([7]));
    }

    #[test]
    fn unresumable_dead_sessions_get_no_handoff_request() {
        let t0 = Instant::now();
        let mut d = Drain::new(
            vec![m(1, false), m(2, false), m(3, false)],
            None,
            cfg(),
            "/h".into(),
            t0,
        );
        let a = d.step_all_ready(t0, &st(&[(1, Stopped), (2, Failed)]));
        assert_eq!(a.len(), 1);
        let (i, h) = finish(&a).unwrap();
        assert!(i.is_empty() && h.is_empty());
    }

    #[test]
    fn handoff_writer_is_not_nudged_and_only_interrupted_at_the_hard_deadline() {
        let t0 = Instant::now();
        let mk = || {
            let mut d = Drain::new(vec![m(3, false)], None, cfg(), "/h".into(), t0);
            d.step_all_ready(t0, &st(&[(3, Idle)]));
            d.step_all_ready(t0 + Duration::from_secs(1), &st(&[(3, Working)]));
            d
        };
        // 到点还在写：不催
        let mut d = mk();
        assert!(d
            .step_all_ready(t0 + Duration::from_secs(900), &st(&[(3, Working)]))
            .is_empty());
        assert!(d.progress(t0 + Duration::from_secs(900)).nudged);
        // 硬时限前写完：正常结束，没被打断
        let a = d.step_all_ready(t0 + Duration::from_secs(1000), &st(&[(3, Idle)]));
        let (i, h) = finish(&a).unwrap();
        assert!(i.is_empty() && h.contains_key(&3));
        // 硬时限到了还没写完：算打断
        let mut d = mk();
        d.step_all_ready(t0 + Duration::from_secs(900), &st(&[(3, Working)]));
        let a = d.step_all_ready(t0 + Duration::from_secs(1080), &st(&[(3, Working)]));
        assert_eq!(finish(&a).unwrap().0, &HashSet::from([3]));
    }

    #[test]
    fn idle_session_asked_for_handoff_at_the_deadline_is_not_interrupted() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(3, false)], None, cfg(), "/h".into(), t0);
        let a = d.step_all_ready(t0 + Duration::from_secs(900), &st(&[(3, Idle)]));
        assert!(matches!(&a[..], [Action::AskHandoff { id: 3, .. }]));
        d.step_all_ready(t0 + Duration::from_secs(901), &st(&[(3, Working)]));
        let a = d.step_all_ready(t0 + Duration::from_secs(950), &st(&[(3, Idle)]));
        assert!(finish(&a).unwrap().0.is_empty());
    }

    /// 分不出忙闲的会话：不等、不催，直接记成被打断。
    #[test]
    fn unknown_state_sessions_are_not_waited_for_and_count_as_interrupted() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true), m(2, true)], None, cfg(), "/h".into(), t0);
        let a = d.step_all_ready(t0, &st(&[(1, Unknown), (2, Idle)]));
        assert!(!a.iter().any(|x| matches!(x, Action::Nudge { .. })));
        assert_eq!(finish(&a).unwrap().0, &HashSet::from([1]));
    }

    /// 看着空闲但还没准备好接按键：先不请它写交接，准备好了再请。
    #[test]
    fn handoff_request_waits_until_the_session_is_ready() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(3, false)], None, cfg(), "/h".into(), t0);
        assert!(d.step(t0, &st(&[(3, Idle)]), &|_| false).is_empty());
        assert_eq!(d.progress(t0).waiting, vec![3]);
        let a = d.step(t0 + Duration::from_secs(4), &st(&[(3, Idle)]), &|_| true);
        assert!(matches!(&a[..], [Action::AskHandoff { id: 3, .. }]));
    }

    /// 催的那句也等准备好；到点还在忙的照样记成被打断。
    #[test]
    fn nudge_waits_until_ready_but_interruption_is_recorded_at_the_deadline() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true)], None, cfg(), "/h".into(), t0);
        let t = t0 + Duration::from_secs(900);
        assert!(d.step(t, &st(&[(1, Working)]), &|_| false).is_empty());
        let a = d.step(t + Duration::from_secs(1), &st(&[(1, Working)]), &|_| true);
        assert_eq!(a, vec![Action::Nudge { id: 1 }]);
        // 只催一次
        assert!(d
            .step(t + Duration::from_secs(2), &st(&[(1, Working)]), &|_| true)
            .is_empty());
        let a = d.step(t + Duration::from_secs(180), &st(&[(1, Working)]), &|_| true);
        assert_eq!(finish(&a).unwrap().0, &HashSet::from([1]));
    }

    /// `--now`：不请写交接、不催，在忙的记成打断，第一拍就结束。
    #[test]
    fn now_skips_handoffs_and_nudges() {
        let t0 = Instant::now();
        let cfg = DrainConfig {
            wait: Duration::ZERO,
            nudge_wait: Duration::ZERO,
            now: true,
        };
        let mut d = Drain::new(vec![m(1, true), m(3, false)], None, cfg, "/h".into(), t0);
        let a = d.step_all_ready(t0, &st(&[(1, Working), (3, Idle)]));
        assert_eq!(a.len(), 1, "{a:?}");
        let (i, h) = finish(&a).unwrap();
        assert_eq!(i, &HashSet::from([1]));
        assert!(h.is_empty());
    }

    /// 结束时带出发起者：重启后它收到的话跟别人不一样。
    #[test]
    fn finish_names_the_initiator() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(7, true)], Some(7), cfg(), "/h".into(), t0);
        let a = d.step_all_ready(t0, &st(&[(7, Working)]));
        assert!(a
            .iter()
            .any(|x| matches!(x, Action::Finish { initiator: Some(7), .. })));
    }
}
