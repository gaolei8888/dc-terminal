# Task 2 report: Telegram adapter

Commit: `6e01db5` on branch `worktree-phone-channel` ("feat: talk to telegram without touching the network in tests")

## What was implemented

`src/channel/telegram.rs` (new file, 334 lines with tests):

- `error_from(&Value) -> ChannelError` — maps `error_code` 401/403 to `BadToken`, everything else to `Unreachable`.
- `parse_updates(&str) -> Result<Vec<Incoming>, ChannelError>` — parses `getUpdates` bodies; updates without a `text` field (photo/sticker/join event) are silently skipped, not errors; malformed JSON or missing `result` array is `Malformed`; `ok:false` maps through `error_from`.
- `max_update_id(&Value) -> Option<i64>` — reads the raw JSON a second time (after `parse_updates` already validated it via `?`) to find the highest `update_id` in the batch, since `Incoming` itself doesn't carry that field.
- `parse_send_result(&str) -> Result<MsgId, ChannelError>` — reads `result.message_id` from a `sendMessage` reply.
- `parse_get_me(&str) -> Result<String, ChannelError>` — reads `result.username` from a `getMe` reply.
- `timeout_from_url(&str) -> Duration` — pulls the `timeout=` query param back out of a URL and adds a 5s margin; used to size the real ureq agent's timeout without changing the `Send` closure's signature.
- `send_real(url, body) -> Result<String, String>` — the real transport, POSTs (GET-equivalent via empty body + `.call()`) through a per-call `ureq::AgentBuilder` sized by `timeout_from_url`. Not unit tested (would touch the network); only reachable through `Telegram::new`.
- `Telegram` struct: `token`, `offset: Mutex<i64>` (poll cursor), `chat_id: Mutex<Option<i64>>`, `send: Box<Send>`. `Telegram::new(token)` wires up `send_real`; `Telegram::with_transport(token, send)` is the test seam.
- `impl Channel for Telegram`: `send()` requires a paired `chat_id` (set the first time `poll()` sees an incoming message — "first sender to the bot is the master" per the design doc) and posts to `sendMessage`; returns `Unreachable` if nothing has paired yet. `poll()` hits `getUpdates?offset={o}&timeout={s}`, parses updates, and — using the raw JSON, not the `Incoming` list — advances `offset` to `max(update_id) + 1`.

`src/channel/mod.rs`: added `pub mod telegram;` (Task 1 deliberately omitted this per Ruling 1 in progress.md since the file didn't exist yet).

### Design decision beyond the brief's literal text

The brief specifies `Channel::send(&self, text: &str)` with no destination parameter, so `Telegram` must know its own target chat id. The brief doesn't say how. I implemented "pair on first incoming message" inside `Telegram::poll()` (matching the design spec's own description: "配对是一次性的...第一个给 bot 发消息的人被记为主人"), stored in `chat_id: Mutex<Option<i64>>`. This is NOT security-filtering ("只接受配对过的那一个 chat id，其余一律丢弃") — `poll()` still returns every incoming message from every chat id; only the send-target pairing happens here. The security filter (reject messages from any chat id other than the paired one) is not implemented in this file and isn't tested by Task 2's brief; per the design doc and the plan's own interface table it looks like Task 5 (`bridge.rs`) is the natural owner of that filter. Flagging this explicitly for review since it's a judgment call the brief didn't spell out.

## Defect found in the brief's reference code

The reference code in Step 3 of the brief defines:
```rust
pub type Send = dyn Fn(&str, &str) -> Result<String, String> + Send + Sync;
```
This does not compile: the type alias itself is named `Send`, so `+ Send` inside its own definition resolves to the type alias, not `std::marker::Send` (E0404: "expected trait, found type alias"). Fixed by fully qualifying: `+ std::marker::Send + Sync`. Caught immediately at Step 3/4 compilation, not by mutation testing — worth noting since the global constraints warned the reference code has had real defects in prior rounds.

## Test commands and results

1. `cargo test --lib channel::telegram -- --test-threads=1` (Step 2, before implementation) — failed to compile: `cannot find function parse_updates`, etc. (11 errors), as expected.
2. Same command after Step 3 minimal implementation — all 10 tests pass (8 from the brief + `the_second_poll_carries_the_offset_forward` from Step 5 + `a_forbidden_token_is_also_bad_token_not_unreachable` added during mutation testing).
3. `cargo test --lib -- --test-threads=1` (full lib suite): `707 passed; 0 failed` (697 pre-existing + 10 new).
4. `cargo test -- --test-threads=1` (full suite, all integration test binaries + doctests): every binary reported `0 failed`; no regressions.
5. `cargo fmt --check`: initially flagged 1 diff (line-wrapping in `send()`), fixed with `cargo fmt`; re-ran clean.
6. `cargo clippy --all-targets`: clean, no warnings.

## Mutation testing (Step 6 + extra)

All mutations applied to a working copy, tested, then reverted (`diff` confirmed byte-identical to the pre-mutation backup after each revert).

1. **Prescribed: narrow `Some(401) | Some(403)` to `Some(401)` only.**
   Result: `get_me_with_a_bad_token_says_bad_token` (uses 401) still passed, confirming the brief's claim that 403 was untested by the original 8 tests. **Added `a_forbidden_token_is_also_bad_token_not_unreachable`** (uses `parse_updates` with `error_code: 403`), which failed under this mutation (`left: Err(Unreachable), right: Err(BadToken)`) and passes on the real code. This new test is now part of the committed suite.

2. **Prescribed: remove the `+ 1` from `*self.offset.lock().unwrap() = max_id + 1`.**
   Result: `the_second_poll_carries_the_offset_forward` failed as required (`https://.../getUpdates?offset=7&timeout=10` instead of the expected `offset=8`). Confirms the offset test is load-bearing.

3. **Extra: changed the "no text → skip" branch in `parse_updates` from `continue` to `return Err(ChannelError::Malformed)`.**
   Result: `updates_without_text_are_skipped_not_errors` failed (`unwrap()` on `Err(Malformed)`). Confirms that test actually exercises the skip-not-error branch, not just an incidental pass.

No survivors were left unaddressed; every mutation attempted was caught by an existing or newly-added test.

## Concerns

- The "pair on first incoming message" behavior in `poll()` (see Design decision section above) is my own addition to make the `Channel` impl functional — it is untested by any brief-mandated test and not itself security-filtering. Worth explicit review against whatever Task 5 (`bridge.rs`) ends up doing, to avoid either duplicating or missing the "only one chat id after pairing" security requirement from the design spec (`docs/superpowers/specs/2026-08-10-dct-phone-channel-design.md` line 214).
- `send_real` and the real ureq transport path have zero test coverage by design (per the brief and the global constraint against touching the network in tests) — consistent with `verify.rs::send_probe` and `llm/http.rs::send_real`, but it means the actual HTTP wire format against Telegram's live API has never been exercised, only the fake-transport seam.
- `timeout_from_url` is a bit unusual (parsing a timeout back out of a URL string rather than threading it through the `Send` closure's signature) — chosen to keep the `Send` type exactly as specified in the brief (`Fn(&str, &str) -> Result<String, String>`) without adding a parameter. Flagging in case a cleaner mechanism is preferred later.

## Follow-up: destination made an explicit argument, adapter ownership state removed

This picks up exactly the concern raised in the section above — the "pair on first incoming message" latch in `poll()` was a second, independent "first sender wins" latch alongside the one `bridge.rs` will hold. Two independent latches in different layers can diverge (there is a re-pair key in the UI, and a daemon restart may restore one and not the other), and when they diverge the adapter would send the user's private session notifications to whichever chat it happened to latch — not necessarily the owner. Ruling: the channel layer must be stateless about ownership; the caller passes the destination every time.

### What changed

- `src/channel/mod.rs`: `Channel::send` signature was already changed (by the prior implementer, before the crash) to `fn send(&self, to: i64, text: &str) -> Result<MsgId, ChannelError>` with an updated doc comment stating the recipient is decided by the caller. No further change needed here.
- `src/channel/telegram.rs`:
  - Removed the `chat_id: Mutex<Option<i64>>` field from `Telegram`, its initialization in both `Telegram::new` (via `with_transport`) and `Telegram::with_transport`, and the auto-latch block at the end of `poll()` that used to set it from the first incoming message's `chat_id`. `poll()` now does exactly fetch + parse + advance the offset cursor, nothing else.
  - Changed `impl Channel for Telegram { fn send(...) }` to take `to: i64` and post `{"chat_id": to, "text": text}` to `sendMessage`. The old `Unreachable` early-return for "nothing paired yet" is gone along with the field — that state no longer exists.
  - Added test `send_posts_to_the_chat_id_the_caller_passed`: uses the fake transport to capture `(url, body)` pairs, calls `tg.send(4242, "hello")` then `tg.send(9999, "hello again")`, and asserts the captured request bodies contain `"chat_id":4242` and `"chat_id":9999` respectively (not the same value reused). This is the test that pins the new seam and would have caught a divergent/stale-latch send target.

### Commands run

- `cargo build` — clean build, no warnings.
- `cargo test -- --test-threads=1` — full suite: lib tests `708 passed; 0 failed` (707 baseline + 1 new `send_posts_to_the_chat_id_the_caller_passed`), all integration test binaries green, 0 doc-tests.
- `cargo fmt --check` — failed once (new test block needed rustfmt's multi-line wrap on a `.lock().unwrap().push(...)` chain), ran `cargo fmt`, re-ran `--check` clean.
- `cargo clippy --all-targets` — clean, no warnings.

### Mutation test on the new seam

Mutated `send` to ignore its `to` argument and hardcode a constant chat id:

```rust
fn send(&self, to: i64, text: &str) -> Result<MsgId, ChannelError> {
    let _ = to;
    let body = serde_json::json!({"chat_id": 1234567890i64, "text": text}).to_string();
    ...
```

Ran `cargo test --lib channel::telegram::tests::send_posts_to_the_chat_id_the_caller_passed -- --test-threads=1`: **FAILED** as required — assertion on the first call's body (`"chat_id":4242`) failed because the mutated code always sent `1234567890`. Reverted the mutation; `diff` against a pre-mutation backup copy of the file confirmed it is byte-identical to the fix as committed. Re-ran the full suite afterward (708 passed) to confirm the revert left no residue.

### Concerns carried forward

- The "who is the owner" latch now lives solely in `bridge.rs` (not yet implemented as of this task) — this task only removes the second, competing latch from the channel layer per the ruling. No behavior change was made to how `bridge.rs` should pick or re-pair an owner; that remains out of scope here.
