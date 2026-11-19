# Task 2 report: `KeepWatch` state machine

Status: DONE. Commit `d2c8c96 feat(keep): watcher that reopens kept sessions with backoff` on feat/kept-sessions (worktree .worktrees/keep). The only file changed is `src/keep.rs`.

## What changed

I added the following to `src/keep.rs`:
- the `Roster` trait
- the constants
- `backoff()`
- a private `Watched` struct that holds each keep's memory
- `KeepWatch::{new, step}` (the per-keep logic is in a private `step_one`)
- a new test module, `watch_tests`, with a fake roster and a manual clock

## Final signatures (for Task 3)

```rust
pub trait Roster {
    fn paused(&self) -> bool;                       // is_restoring() || is_draining()
    fn find(&self, k: &Keep) -> Option<u32>;        // live, tag==name, profile==profile, canonical dir==dir
    fn name_taken_elsewhere(&self, k: &Keep) -> bool;
    fn conversation_of(&self, id: u32) -> Option<String>;
    fn open(&self, k: &Keep, conversation: Option<&str>, want_id: Option<u32>) -> Result<u32, String>;
    fn warn(&self, w: WarningCode);
}
pub const RESTART_BASE: Duration = 5s;  RESTART_CAP = 300s;  HEALTHY_AFTER = 60s;
pub const RESUME_FAIL_WINDOW: Duration = 20s;  CRASH_WARN_AFTER: u32 = 3;
#[derive(Default)] pub struct KeepWatch { .. }
impl KeepWatch {
    pub fn new() -> Self;
    pub fn step(&mut self, now: Instant, keeps: &[Keep], profiles: &[Profile], r: &dyn Roster);
}
```

There is one change from the brief. `open` returns `Result<u32, String>` instead of `Result<u32, ()>`: the error text is for Task 3's log, and the watcher ignores it. `Result<_, ()>` would also hit clippy `result_unit_err` on a pub trait.

## How a round works

1. If `paused()` is true, return at once. No state is touched and no names are forgotten.
2. Names that are no longer in `keeps` are forgotten. A keep that is re-added later counts as new: no old number and no old conversation.
3. For each keep, check `name_taken_elsewhere` every round, whether the session is found or not. A clash warns `NameClash` once. When the clash goes away the flag clears, so a later clash warns again. Opening is never blocked by a clash.
4. **Found** (`find` returns an id):
   - Remember the id, set `seen` and clear `next_try`.
   - Store the result of `conversation_of`, but only when it is Some. When it is None, keep the remembered conversation.
   - Clear the `DirGone` and `ProfileGone` flags.
   - If `opened_at` is None, or was at least `HEALTHY_AFTER` ago, the session counts as healthy: set failures to 0 and clear every warned flag except `NameClash`.
5. **Not found, seen before, no retry scheduled yet.** This is the round where we notice the death.
   - `lived = now - opened_at`.
   - If the previous open resumed by conversation and `lived < RESUME_FAIL_WINDOW`, drop the remembered conversation. "Resumed" means a conversation was passed and `Profile::resumes_by_id()` is true; I checked that in src/profile.rs: both `session_id_args` and `resume_id_args` must be non-empty.
   - If `lived < HEALTHY_AFTER`, add a failure (at `CRASH_WARN_AFTER` this warns `CrashLooping` once). Otherwise the failure count stays the same.
   - Set `next_try = now + min(5s·2^failures, 300s)`, then return for this keep.
6. **Not found and `now < next_try`:** wait.
7. **Due, or never seen:**
   - If the directory is missing, warn `DirGone` once and return. If the profile is not in `profiles`, warn `ProfileGone` once and return.
   - Neither of these adds a failure, and `next_try` is left alone, so the open happens in the first round after the directory or profile comes back.
   - Otherwise call `open(remembered conversation, remembered id)`.
   - If the open succeeds, set `opened_at = now` and work out `resumed`.
   - If it fails, warn `StartFailed` once, add a failure and schedule the next try.

"Seen" means it was found alive at least once or we opened it. If we opened it and it died before the next round, that still counts as a quick death.

The first death of a session we did not open ourselves (for example one restored after a restart) waits 5 s. That matches the spec: the first reopen waits 5 s.

## Tests (src/keep.rs `watch_tests`, 17 tests, all deterministic, no sleeps or threads)

- a_fresh_keep_is_opened_once_with_nothing_remembered
- a_session_that_stops_comes_back_after_5s_with_its_number_and_conversation
- quick_deaths_back_off_doubling_to_the_cap_and_reset_after_a_healthy_minute (waits 10,20,40,80,160,300,300,300, then 5 after 60 s alive)
- being_seen_alive_briefly_does_not_reset_the_backoff
- a_session_found_alive_and_then_lost_is_not_counted_as_a_quick_death
- a_resumed_conversation_that_dies_within_20s_is_dropped_next_time
- a_resumed_conversation_that_lives_past_20s_is_kept
- a_quick_death_without_resume_by_id_keeps_the_conversation (shell profile)
- a_later_missing_conversation_id_keeps_the_remembered_one
- nothing_is_opened_while_paused
- a_due_reopen_waits_out_a_pause
- a_removed_keep_is_forgotten_and_never_reopened (re-adding it opens fresh with None/None)
- a_missing_dir_warns_once_and_again_only_after_it_came_back
- a_missing_profile_warns_once_and_again_only_after_it_came_back
- a_failed_open_warns_once_and_backs_off (attempts at 0, 10, 30, 70)
- repeated_quick_deaths_warn_crash_looping_once_until_healthy_again
- a_name_clash_warns_once_and_still_opens (warns again after the clash cleared and came back)

## Mutation checks (each mutant applied, tests run, file restored and diffed)

| Mutant | Result |
|---|---|
| Remove the `paused()` early return (required) | Killed by `nothing_is_opened_while_paused` and `a_due_reopen_waits_out_a_pause` |
| Drop `w.resumed &&` from the resume-fail check | Killed |
| Overwrite the conversation with None when `conversation_of` returns None | Killed |
| Remove the `failures = 0` healthy reset | Killed |
| Never clear `NameClash` | Killed |
| Healthy-reset condition replaced with `true` | Survived at first. I added `being_seen_alive_briefly_does_not_reset_the_backoff`, which now kills it |

## Verification

- `cargo test --lib keep::` passes: 29 tests (12 parse + 17 watch).
- `cargo +1.99.0 clippy --all-targets -- -D warnings` is clean.
- Full `cargo test`: 2115 passed, 4 failed. All the failures are in `session::tests`.
  - The set of failing tests changed between runs (4 to 6 different ones each time, load average about 5). The failures are spawned real processes (the first one I looked at panicked on a `send_input` unwrap).
  - This commit only touches `src/keep.rs`.
  - So these are the pre-existing flaky ones, not caused by this task. That is more than the ~3 the brief mentioned; I did not rerun them on the base commit to compare.

## Concerns / notes for Task 3

1. **`CrashLooping` doc comment.** In `proto.rs`, the doc for `CrashLooping` says "反复崩溃，不再自动重启" (keeps crashing, no more automatic restarts). Following the spec, the watcher **keeps** reopening with backoff (capped at 5 min) and only warns. The proto doc comment should probably be corrected to say "反复崩溃，放慢重启" (keeps crashing, restarts slowed down). I did not touch proto.rs because it is outside this task.
2. **`find` must match the canonical dir.** `step` checks `k.dir.is_dir()` on the `~`-expanded path, but it does not canonicalize it. The `Roster::find` implementation has to compare canonical paths.
3. **`open` must discard the leftover slot.** When `want_id` is Some and a Stopped slot with that number is still on the roster, Task 3's `open` must `discard` it first. The watcher only passes the number in.
4. **Dir/profile problems do not slow down retries.** They are checked every round once the retry is due, so the reopen happens as soon as the directory or profile is back. This follows the spec's "下一轮再看" (look again next round). I read the spec's "同样按上面的等待时间" (use the same wait times as above) as applying to the backoff that was already scheduled when the death was noticed, not as an extra failure.

## Fix round 1

Two commits on top of d2c8c96:
- `fix(keep): reset backoff on a late-noticed healthy death; crash-loop warning only for real stops` (src/keep.rs)
- `fix(keep): say a crash-looping kept session is still being restarted, with growing waits` (src/proto.rs, src/i18n.rs)

The review asked for five fixes:

1. **A death noticed after a healthy minute now counts as healthy.** A session can be last seen alive under 60 s after opening and only noticed gone at 60 s or later. That death now resets the failure count and clears every warned flag except `NameClash`, through a new `Watched::healthy()`. The same function is used by the found-alive reset. New test: `a_death_first_noticed_after_a_healthy_minute_resets_the_backoff` (seen alive up to t+55, missing at t+60, next wait 5 s; `CrashLooping` can then be raised again).
2. **Comment on measurement.** A comment at the death-detection branch says that time alive is measured to the round where the watcher notices, so it is up to one round late. In practice the 20 s window is 15–20 s and the 60 s window is 55–60 s.
3. **Name-clash test extended.** `a_name_clash_warns_once_and_still_opens` now runs to 120 s, past the healthy minute, and still expects exactly one `NameClash`.
4. **`CrashLooping` only on the "session died" path.**
   - `Watched::fail(now)` no longer warns. It only adds a failure and schedules the next try.
   - The `CrashLooping` warning is raised only in death detection.
   - A failed open raises `StartFailed` only.
   - New test: `failed_opens_alone_do_not_say_crash_looping`.
5. **Wording changed.**
   - proto.rs: the `KeptSessionReason` doc now says "为什么一直在停" (why it keeps stopping). The `CrashLooping` doc says the watcher keeps reopening with growing waits, up to 5 minutes, and does not give up.
   - i18n.rs: the user text is now "它一直在停；dct 会继续重开，每次多等一会儿（最多等 5 分钟）" (it keeps stopping; dct will keep reopening it, waiting a little longer each time, up to 5 minutes), and in English "it keeps stopping; dct keeps restarting it, waiting longer each time (up to 5 minutes)".

**Mutation checks.** Each mutant was applied, tested and then restored:
- `healthy()` using `clear()` instead of `retain`: killed by the name-clash test.
- Removing the reset on a late-noticed death: killed by the new test.
- Raising `CrashLooping` again on failed opens: killed by `failed_opens_alone_do_not_say_crash_looping`.

**Verification.**
- `keep::` has 31 tests and all pass (19 of them are watcher tests).
- clippy +1.99.0 is clean.
- Full `cargo test`: 2116 passed, 5 failed.
  - Four of the failures are in `session::tests`, which were already failing on and off before this change.
  - The fifth was `daemon::drain_tests::an_id_less_session_with_an_id_sibling_is_asked_for_a_handoff`. It passed 3 out of 3 times when I ran it alone, so it is a load-dependent flake. That module does not use `keep`.
