### Task 4: 排空状态机 `drain.rs`（纯逻辑）

**Files:**
- Create: `src/drain.rs`（`src/lib.rs` 加 `pub mod drain;`）

**Interfaces:**
- Consumes: `crate::session::SessionState`。
- Produces:

```rust
pub struct DrainConfig { pub wait: Duration, pub nudge_wait: Duration }
impl Default for DrainConfig { /* 15 分钟、3 分钟 */ }

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
    /// 排空结束，可以落盘重启了。
    Finish { interrupted: HashSet<u32>, handoffs: HashMap<u32, PathBuf> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// 还在等的会话 id。
    pub waiting: Vec<u32>,
    /// 距离下一个时限还剩多少秒。
    pub remaining_s: u64,
    /// 已经进入兜底阶段。
    pub nudged: bool,
}

pub struct Drain { /* 私有 */ }
impl Drain {
    pub fn new(members: Vec<Member>, initiator: Option<u32>, cfg: DrainConfig,
               handoff_dir: PathBuf, now: Instant) -> Drain;
    /// 每拍调一次。`states` 是此刻各会话状态（不在表里 = 会话已经没了，算停下）。
    pub fn step(&mut self, now: Instant, states: &HashMap<u32, SessionState>) -> Vec<Action>;
    pub fn progress(&self, now: Instant) -> Progress;
    pub fn finished(&self) -> bool;
}

/// 交接文件名：`<handoff_dir>/<id>-<YYYYmmdd-HHMMSS>.md`（时间由调用方给，便于测试）。
pub fn handoff_path(dir: &Path, id: u32, stamp: &str) -> PathBuf;
pub const NUDGE_TEXT: &str = "dct 马上要重启。请在安全的地方停下手上的事，然后用一句话写下你做到哪了、下一步是什么。";
pub fn handoff_text(path: &Path) -> String; // spec 第 5 节那句，带路径
```

规则（逐条对应 spec 第 3 节）：
- 「停下了」= 状态是 `Idle`、`Stopped`、`Failed`，或者不在 `states` 里。
- 非 agent 成员一开始就算停下，永不发话。
- 发起者（`initiator`）一开始就算停下，**直接进 `interrupted`**，不发任何话。
- 接不回（`!resumable`）的 agent：第一次被看到「停下了」时发一次 `AskHandoff`，然后**重新等它再停下一次**（它在写交接）；它的路径进 `handoffs`。
- 能接回的 agent：停下了就好，不发话。
- 到 `wait` 还没停下的：每个发一次 `Nudge`，进入兜底阶段，这些会话都进 `interrupted`（不管之后有没有停）。
- 兜底阶段再过 `nudge_wait` 还没停的：不再等，直接 `Finish`。
- 所有成员都停下 → `Finish`（只发一次，之后 `step` 返回空）。

- [ ] **Step 1: 写失败的测试**（`src/drain.rs` 底部）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use SessionState::*;

    fn cfg() -> DrainConfig { DrainConfig { wait: Duration::from_secs(900), nudge_wait: Duration::from_secs(180) } }
    fn m(id: u32, resumable: bool) -> Member { Member { id, resumable, is_agent: true } }
    fn st(v: &[(u32, SessionState)]) -> HashMap<u32, SessionState> { v.iter().cloned().collect() }
    fn finish(a: &[Action]) -> Option<(&HashSet<u32>, &HashMap<u32, PathBuf>)> {
        a.iter().find_map(|x| match x { Action::Finish { interrupted, handoffs } => Some((interrupted, handoffs)), _ => None })
    }

    #[test]
    fn all_idle_finishes_immediately_with_no_interruptions() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true), m(2, true)], None, cfg(), "/h".into(), t0);
        let a = d.step(t0, &st(&[(1, Idle), (2, Idle)]));
        let (i, h) = finish(&a).unwrap();
        assert!(i.is_empty() && h.is_empty());
        assert!(d.finished());
        assert!(d.step(t0, &st(&[])).is_empty());
    }

    #[test]
    fn waits_for_working_then_finishes_when_it_stops() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true)], None, cfg(), "/h".into(), t0);
        assert!(d.step(t0, &st(&[(1, Working)])).is_empty());
        assert_eq!(d.progress(t0).waiting, vec![1]);
        let a = d.step(t0 + Duration::from_secs(60), &st(&[(1, Idle)]));
        assert!(finish(&a).unwrap().0.is_empty());
    }

    #[test]
    fn nudges_at_deadline_and_marks_interrupted_even_if_it_then_stops() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true), m(2, true)], None, cfg(), "/h".into(), t0);
        d.step(t0, &st(&[(1, Working), (2, Idle)]));
        let a = d.step(t0 + Duration::from_secs(900), &st(&[(1, Working), (2, Idle)]));
        assert_eq!(a, vec![Action::Nudge { id: 1 }]);
        assert!(d.progress(t0 + Duration::from_secs(900)).nudged);
        // 兜底话发出去后它先变 Working 再变 Idle（Review Focus 5）
        d.step(t0 + Duration::from_secs(910), &st(&[(1, Working), (2, Idle)]));
        let a = d.step(t0 + Duration::from_secs(950), &st(&[(1, Idle), (2, Idle)]));
        let (i, _) = finish(&a).unwrap();
        assert_eq!(i, &HashSet::from([1]));
    }

    #[test]
    fn hard_deadline_after_nudge_finishes_anyway() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true)], None, cfg(), "/h".into(), t0);
        d.step(t0 + Duration::from_secs(900), &st(&[(1, Working)]));
        assert!(d.step(t0 + Duration::from_secs(1079), &st(&[(1, Working)])).is_empty());
        let a = d.step(t0 + Duration::from_secs(1080), &st(&[(1, Working)]));
        assert_eq!(finish(&a).unwrap().0, &HashSet::from([1]));
    }

    #[test]
    fn initiator_is_not_waited_for_and_counts_as_interrupted() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(7, true)], Some(7), cfg(), "/h".into(), t0);
        let a = d.step(t0, &st(&[(7, Working)]));
        assert_eq!(finish(&a).unwrap().0, &HashSet::from([7]));
        assert!(!a.iter().any(|x| matches!(x, Action::Nudge { .. })));
    }

    #[test]
    fn unresumable_idle_session_is_asked_for_handoff_then_waited_again() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(3, false)], None, cfg(), "/h".into(), t0);
        let a = d.step(t0, &st(&[(3, Idle)]));
        assert!(matches!(&a[..], [Action::AskHandoff { id: 3, .. }]));
        // 它开始写交接
        assert!(d.step(t0 + Duration::from_secs(1), &st(&[(3, Working)])).is_empty());
        let a = d.step(t0 + Duration::from_secs(30), &st(&[(3, Idle)]));
        let (i, h) = finish(&a).unwrap();
        assert!(i.is_empty());
        assert!(h.contains_key(&3));
    }

    #[test]
    fn unresumable_busy_session_is_asked_only_after_it_stops() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(3, false)], None, cfg(), "/h".into(), t0);
        assert!(d.step(t0, &st(&[(3, Working)])).is_empty());
        let a = d.step(t0 + Duration::from_secs(5), &st(&[(3, Idle)]));
        assert!(matches!(&a[..], [Action::AskHandoff { id: 3, .. }]));
    }

    #[test]
    fn vanished_or_failed_sessions_count_as_stopped_and_non_agents_are_ignored() {
        let t0 = Instant::now();
        let shell = Member { id: 9, resumable: false, is_agent: false };
        let mut d = Drain::new(vec![m(1, true), m(2, true), shell], None, cfg(), "/h".into(), t0);
        let a = d.step(t0, &st(&[(2, Failed), (9, Working)]));
        assert!(finish(&a).is_some());
        assert!(!a.iter().any(|x| matches!(x, Action::AskHandoff { id: 9, .. })));
    }

    #[test]
    fn handoff_path_is_under_dir() {
        assert_eq!(handoff_path(Path::new("/h"), 3, "20261014-120000"), PathBuf::from("/h/3-20261014-120000.md"));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib drain::`
Expected: 编译失败。

- [ ] **Step 3: 实现** `Drain`（每个成员记一个小状态：`Waiting` / `HandoffAsked{seen_busy: bool}` / `Done`；全局记 `deadline = t0 + wait`、`nudged_at: Option<Instant>`、`interrupted`、`handoffs`、`finished`）。`AskHandoff` 的时间戳用 `std::time::SystemTime` 格式化成 `YYYYmmdd-HHMMSS`（不引新依赖：从 UNIX 秒手算 UTC 日期，或沿用仓库里已有的时间格式化函数——先 grep `fn .*stamp\|format_time`）。`HandoffAsked` 的会话要等「先看到过一次忙、或者已经过了 10 秒」之后再看到停下才算完——防止同一拍里刚发完话、屏幕还没变就被判成写完了。为这一条补测试：

```rust
    #[test]
    fn handoff_not_considered_written_in_the_same_breath() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(3, false)], None, cfg(), "/h".into(), t0);
        d.step(t0, &st(&[(3, Idle)]));
        // 屏幕还没来得及变忙
        assert!(d.step(t0 + Duration::from_millis(500), &st(&[(3, Idle)])).is_empty());
        // 10 秒还没变忙，当它不打算写了，算完
        assert!(finish(&d.step(t0 + Duration::from_secs(10), &st(&[(3, Idle)]))).is_some());
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib drain::`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/drain.rs src/lib.rs
git commit -m "feat(drain): pure state machine that waits for sessions to stop, nudges late ones and asks unresumable ones for a handoff"
```

---

