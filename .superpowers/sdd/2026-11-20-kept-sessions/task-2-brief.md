### Task 2: `KeepWatch` state machine

**Files:**
- Modify: `src/keep.rs`
- Test: unit tests in `src/keep.rs` with a fake `Roster`

**Interfaces — Consumes:** `Keep`, `WarningCode::KeptSessionProblem` from Task 1. **Produces:**
```rust
pub trait Roster {
    /// restoring || draining: do nothing this round.
    fn paused(&self) -> bool;
    /// The live (not Stopped) session whose tag == name, profile == profile, canonical dir == dir.
    fn find(&self, k: &Keep) -> Option<u32>;
    /// Another session (live) has tag == name but different profile or dir.
    fn name_taken_elsewhere(&self, k: &Keep) -> bool;
    fn conversation_of(&self, id: u32) -> Option<String>;
    /// Open k. `want_id` = remembered number; impl discards a leftover Stopped slot with that id first.
    fn open(&self, k: &Keep, conversation: Option<&str>, want_id: Option<u32>) -> Result<u32, ()>;
    fn warn(&self, w: crate::proto::WarningCode);
}
pub struct KeepWatch { /* per-name state */ }
impl KeepWatch {
    pub fn new() -> Self;
    /// One round. `profiles` = current profile list (for existence + resumes_by_id).
    pub fn step(&mut self, now: std::time::Instant, keeps: &[Keep], profiles: &[crate::profile::Profile], r: &dyn Roster);
}
pub const RESTART_BASE: Duration = 5 s; pub const RESTART_CAP: Duration = 300 s;
pub const HEALTHY_AFTER: Duration = 60 s; pub const RESUME_FAIL_WINDOW: Duration = 20 s;
pub const CRASH_WARN_AFTER: u32 = 3;
```
Behaviour per keep (state keyed by name; names no longer in `keeps` are forgotten; spec §2–§4 is the authority):
- `paused()` → return without touching state.
- `find` → Some(id): remember `id`; remember `conversation_of(id)` when Some (keep the old one when None); if opened by us ≥ `HEALTHY_AFTER` ago → failures = 0 and clear "already warned" set. Check `name_taken_elsewhere` → warn `NameClash` once.
- `find` → None:
  - dir missing → warn `DirGone` once, skip. Profile not in `profiles` → warn `ProfileGone` once, skip.
  - Never seen and never opened → open now (`conversation None`, `want_id None`).
  - Seen before: if no retry scheduled → if it died < `HEALTHY_AFTER` after we opened it, failures += 1; schedule `next_try = now + min(RESTART_BASE * 2^failures, RESTART_CAP)`; return for this keep. When `now >= next_try` → open with remembered id; conversation = remembered one, **except** None if the previous open resumed by conversation and it died < `RESUME_FAIL_WINDOW` after opening (also drop the remembered conversation then).
  - failures reaching `CRASH_WARN_AFTER` → warn `CrashLooping` once.
  - `open` Err → warn `StartFailed` once, failures += 1, schedule next_try as above.
- "Once" = per (name, reason) until that keep is healthy again (alive ≥ `HEALTHY_AFTER`) or the problem clears (dir/profile back).

- [ ] Step 1: Failing tests with a fake roster (records `open` calls, scripted `find` results, `paused` flag, collected warnings) and a manual clock (`Instant` base + offsets):
  - fresh start → one `open(None, None)`, tag/name passed through.
  - alive then missing → no open at once; open at +5 s with remembered id and conversation.
  - stopped again within 60 s → next wait 10 s, then 20 s; cap at 300 s; after alive ≥ 60 s the next wait is 5 s again.
  - resumed by conversation and died within 20 s → next open has `conversation None`.
  - paused → zero opens, even when missing and due.
  - keep removed from list → no more opens (and the live one is not touched — the trait has no stop).
  - dir missing / profile missing → no open, exactly one warning across many steps; warning again after it recovered and broke again.
  - open Err → `StartFailed` once, backoff applies.
  - 3 quick deaths → exactly one `CrashLooping`.
  - name clash → one `NameClash`, open still happens.
  - conversation_of returns None on a later round → remembered conversation kept.
- [ ] Step 2: Implement until green. Mutation check: delete the `paused()` early return, confirm a test fails, restore.
- [ ] Step 3: clippy clean; commit `feat(keep): watcher that reopens kept sessions with backoff`.

