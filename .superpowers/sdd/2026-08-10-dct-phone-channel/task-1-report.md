# Task 1 Report: channel trait 与出站事件类型

## What was implemented

Created `src/channel/mod.rs` with:
- `pub type MsgId = i64;`
- `Incoming { text, reply_to: Option<MsgId>, chat_id: i64 }`
- `ChannelError { Unreachable, BadToken, Malformed }` with `worth_retrying(self) -> bool` (true only for `Unreachable`)
- `trait Channel: Send + Sync { fn send(&self, text: &str) -> Result<MsgId, ChannelError>; fn poll(&self, timeout: Duration) -> Result<Vec<Incoming>, ChannelError>; }`
- `EventKind { Stopped, Failed, Vanished }`
- `Event { session: u32, kind: EventKind, name: String, project: String }`
- `pub fn debounce(last: Option<Duration>, now: Duration, window: Duration) -> bool` — `None` last always sends; otherwise sends only when `now.saturating_sub(last) > window` (boundary counts as inside the window, suppressed)
- `pub const DEBOUNCE_WINDOW: Duration = Duration::from_secs(30);`

All doc comments taken verbatim from the brief (Chinese, matching repo convention).

Modified `src/lib.rs` to add `pub mod channel;`.

**Deviation from the brief per controller ruling:** omitted `pub mod telegram;` from the top of `src/channel/mod.rs`. That module is created in Task 2; including the line now would break compilation since `src/channel/telegram.rs` does not exist yet. Everything else matches the brief exactly.

## Mutation testing

1. **`>` → `>=`** in `debounce`'s `now.saturating_sub(last) > window`.
   - Caught by `debounce_suppresses_only_inside_the_window`, specifically the boundary assertion: `assert!(!debounce(Some(10s), 40s, 30s window))` — with `>=` this becomes `10s delta >= 30s` → false → `debounce` returns true (send), but the test expects `false` (suppress). Test failed as required.
   - Reverted cleanly (verified via `diff` against a pre-mutation backup — identical).

2. **`matches!` → `!matches!`** in `ChannelError::worth_retrying`.
   - Caught by `bad_token_is_not_retryable_but_unreachable_is`, specifically `assert!(ChannelError::Unreachable.worth_retrying())` — with the negation, `Unreachable.worth_retrying()` returns `false`, failing the assertion. Test failed as required.
   - Reverted cleanly (verified via `diff` — identical to backup).

No additional tests were needed; both prescribed mutations were caught by the existing two tests.

## Exact commands run and results

1. `cargo test --lib channel:: -- --test-threads=1` (before implementation) — compile error: `cannot find function debounce`, `cannot find type ChannelError` (as expected, confirms failing test step).
2. After writing implementation: `cargo test --lib channel:: -- --test-threads=1` → `test result: ok. 2 passed; 0 failed`.
3. `cargo test -- --test-threads=1` (full suite) → all test binaries reported `ok`, 697 passed in the lib target (was 695 before this task's 2 new tests; consistent with prior baseline of 738 total across the whole suite — no regressions, no failures anywhere).
4. `cargo fmt --check` → clean (after running `cargo fmt` once to apply rustfmt's multi-line wrapping of the three `debounce(...)` calls in the test body — content unchanged, only line-wrapping).
5. `cargo clippy --all-targets` → clean, no warnings.
6. Mutation 1 (`>` → `>=`): `cargo test --lib channel:: -- --test-threads=1` → 1 failed (as required), then reverted and diff-verified identical to pre-mutation state.
7. Mutation 2 (`matches!` → `!matches!`): same procedure → 1 failed (as required), then reverted and diff-verified identical.
8. Final re-run after revert: `cargo test --lib channel:: -- --test-threads=1` → 2 passed; `cargo fmt --check` → clean; `cargo clippy --all-targets` → clean; `cargo test -- --test-threads=1` (full suite) → all `ok`, no failures.

## Concerns

None. No network access, no filesystem access to `~/.dct` — this task is pure data types and a trait definition with no I/O. `pub mod telegram;` intentionally omitted per controller instruction; Task 2 must add it back when it creates `src/channel/telegram.rs`.

## Commit

`6a00dda` — "feat: a channel is something you can send to and poll from" (worktree-phone-channel branch). 2 files changed: `src/channel/mod.rs` (new, 125 lines incl. tests/doc comments), `src/lib.rs` (+1 line).
