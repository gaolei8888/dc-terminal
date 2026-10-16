# Task 4 report: src/drain.rs

Status: DONE.
Commit: see `git log -1` on feat/graceful-restart ("feat(drain): pure state machine ...").

## TDD
- RED: tests-only drain.rs + `pub mod drain;` -> `cargo test --lib drain::` failed to compile (23 errors, types missing).
- GREEN: 11 drain tests pass (9 from brief, the "same breath" test, plus my `file_stamp_has_the_documented_shape`).
- Full `cargo test`: all pass (1838 lib). `cargo +1.99.0 clippy --all-targets -- -D warnings`: clean. rustfmt run on drain.rs only.

## Files
- src/drain.rs (new), src/lib.rs (`pub mod drain;`).

## Decisions / deviations
- Timestamp: reused `journal::civil_from_days` (already pub(crate)); `file_stamp()` is a private helper in drain.rs (journal's `stamp()` has a different format).
- Members stored sorted by id, so `Progress.waiting` and Nudge order are ascending.
- Handoff grace is 10s (const HANDOFF_GRACE); `seen_busy` set whenever the asked session is seen not-stopped.
- Nudge goes only to members still in Waiting. Members in HandoffAsked (writing a handoff) are not nudged but keep the drain in the nudge phase, are marked interrupted, and are bounded by the hard deadline.
- At hard-deadline Finish, any member not Done is added to `interrupted` (covers handoff writers who never finished).
- Initiator goes into `interrupted` only if it is among the members.
- A nudged unresumable session that stops during the nudge phase still gets AskHandoff (same rule as before).
- Handoff path is recorded in `handoffs` when AskHandoff is emitted (not when the write completes).
- `remaining_s` truncates (as_secs) and is 0 once past the limit.

## Concerns
- The spec does not say whether the handoff-writing sessions should be nudged at the wait deadline; my choice above is flagged for review.
- File existence of the handoff is not checked here (caller's job after restart, per spec section 5).

## Fix round 1
1. Initiator is inserted into `interrupted` unconditionally in `Drain::new` (test: initiator not a member).
2. Unresumable agent that is not `Idle` (Stopped/Failed/missing) is simply Done: no AskHandoff, no grace, no path recorded (test).
3. Pinned by test: handoff writer is not nudged at the wait deadline; finishes normally before the hard deadline; interrupted only if unfinished at the hard deadline.
4. Fixed: at nudge time only `Waiting` members (the ones actually nudged) are marked interrupted, so an idle session asked for handoff in the deadline step is not (test). Hard-deadline Finish still marks any unfinished member.
5. "(Review Focus 5)" comment replaced with plain explanation.
Result: 15 drain tests pass; clippy +1.99.0 clean (also fixed two lints from my edits: needless mut, identical if-blocks merged).
