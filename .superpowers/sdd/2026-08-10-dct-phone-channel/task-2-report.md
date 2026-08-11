# Task 2 report: Telegram adapter

BASE = 1ff7575 (Task 1, `src/channel/mod.rs`)
Commits:
- bfccd1e — test: add failing tests for the telegram channel adapter (declares `pub mod telegram;`, adds Step 1 tests)
- 6f27293 — feat: parse telegram updates, send results and getMe replies (Step 3/4: parsing + minimal `Telegram` scaffold)
- 80f496c — feat: implement Channel for Telegram and its real ureq transport (Step 5: `Channel` impl, cursor, real transport)

## Inaccuracies found in the brief, and what I did

### 1. Reference code names the transport type alias `Send` — does not compile

Step 3's reference code declares:

```rust
pub type Send = dyn Fn(&str, &str) -> Result<String, String> + Send + Sync;
```

`Send` collides with `std::marker::Send`. Inside the same bound list, `+ Send + Sync`
resolves `Send` to the type alias being defined (types and traits share one namespace),
not the marker trait — a type alias can't be used as a trait bound. Confirmed with a
throwaway `rustc` compile before writing anything into the repo:

```
error[E0404]: expected trait, found type alias `Send`
```

Fixed by naming it `Sender` instead, matching the existing sibling idiom
(`llm/http.rs::Sender`). Documented the reasoning inline as a doc comment on the type
so a future reader doesn't reintroduce it.

### 2. The interface list never says how `send()` learns its destination chat_id

The brief's "Produces" list gives `Telegram::new(token)` / `Telegram::with_transport(token, send)`
— no chat id anywhere — and Task 1's `Channel::send(&self, text: &str)` has no chat_id
parameter either. But Telegram's real `sendMessage` API requires a `chat_id`. I checked
both the plan (`docs/superpowers/plans/2026-08-10-dct-phone-channel.md`) and the design
spec (`docs/superpowers/specs/2026-08-10-dct-phone-channel-design.md`); neither resolves
this — Task 5 (`bridge.rs`, not yet built) tracks its own `owner: Option<i64>` for a
different purpose (rejecting strangers' *incoming* messages), but that's a separate
struct with no wiring back into `Channel::send`.

Resolved it inside `Telegram` itself: it now has a `chat_id: Mutex<Option<i64>>` field
that `poll()` sets to the first incoming message's `chat_id`, first-write-wins, and never
overwrites once set — the same "first sender becomes the owner, permanently" model
`bridge.rs` is planned to use for pairing, just enforced independently at the transport
layer since `Channel::send` gives it no other way to learn a target. Before any chat is
known, `send()` returns `ChannelError::Unreachable` (retryable — once `poll()` learns a
chat, a retry succeeds, which is exactly what `worth_retrying()` is for).

This also forced a design decision the brief didn't specify: the `Sender` transport type
now carries an explicit `Duration` timeout parameter (`Fn(&str, &str, Duration) -> ...`),
computed by the caller — `poll()` passes `long_poll_secs + 5s`, `send()` passes a fixed
10s constant. The brief's Step 5 prose says "超时取长轮询秒数 + 5 秒余量" but the
reference `Sender` shape from Step 3 has no way to carry a per-call timeout into a fixed
transport closure; threading it through the type is the only way to satisfy that
requirement for both a long-poll call and a quick `sendMessage` call sharing one
`Box<Sender>`.

Both of these are documented in code with WHY comments at the `chat_id` field and the
`Sender` type alias.

## Mutation table (Step 6)

| Mutation | Expected | Result |
|---|---|---|
| `error_from`: `Some(401) \| Some(403)` → `Some(401)` only | brief predicts `get_me_with_a_bad_token_says_bad_token` still passes (it uses 401) — proving 403 uncovered | Confirmed: only `a_403_is_also_bad_token_not_unreachable` failed (added this test in Step 1, before Step 6 — brief's failure mode never actually manifested because 403 was covered from the start) |
| `poll()` offset update: `max_id + 1` → `max_id` | `the_second_poll_carries_the_offset_past_the_last_update_id` must fail | Confirmed: failed with `第二次轮询该带上 7 + 1 = 8，实际 url: ...offset=7...` |
| (extra, not requested by brief) `chat_id` first-write-wins guard removed (always overwrite) | `the_learned_chat_does_not_get_overwritten_by_a_later_sender` must fail | Confirmed: failed, `left: Number(222), right: 111` |

All three mutations were applied one at a time to the working tree, verified to fail the
expected test (and only that test), then reverted before the Step 5 commit — the
committed code is the unmutated version confirmed clean by `cargo test --lib channel::telegram`
afterward.

## Cursor test

`the_second_poll_carries_the_offset_past_the_last_update_id` (`src/channel/telegram.rs`):
builds a `FakeTransport` that records every `(url, body)` it's called with and replays two
canned responses. First response has updates with `update_id` 5 and 7. Asserts:
- first `poll()` call's URL contains `offset=0` (nothing seen yet)
- second `poll()` call's URL contains `offset=8` — i.e. `max(5, 7) + 1`

This is the test the brief calls out as non-optional: without the cursor, Telegram
re-sends the same update on every long-poll, which in this feature means the same
sentence gets typed into an agent every 25 seconds forever.

## Test commands and output tails

```
$ cargo test --lib channel::telegram -- --test-threads=1
running 16 tests
test channel::telegram::tests::a_403_is_also_bad_token_not_unreachable ... ok
test channel::telegram::tests::a_revoked_token_is_bad_token_not_unreachable ... ok
test channel::telegram::tests::a_transport_level_failure_is_unreachable ... ok
test channel::telegram::tests::garbage_is_malformed ... ok
test channel::telegram::tests::get_me_returns_the_bot_username ... ok
test channel::telegram::tests::get_me_with_a_bad_token_says_bad_token ... ok
test channel::telegram::tests::parses_a_normal_update ... ok
test channel::telegram::tests::picks_up_the_message_being_replied_to ... ok
test channel::telegram::tests::poll_puts_the_requested_seconds_in_the_url ... ok
test channel::telegram::tests::reads_the_new_message_id_back ... ok
test channel::telegram::tests::send_surfaces_bad_token ... ok
test channel::telegram::tests::send_targets_the_chat_learned_from_poll ... ok
test channel::telegram::tests::sending_before_any_chat_is_known_is_unreachable ... ok
test channel::telegram::tests::the_learned_chat_does_not_get_overwritten_by_a_later_sender ... ok
test channel::telegram::tests::the_second_poll_carries_the_offset_past_the_last_update_id ... ok
test channel::telegram::tests::updates_without_text_are_skipped_not_errors ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 702 filtered out; finished in 0.00s
```

```
$ cargo test --lib
...
test result: ok. 718 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.51s
```

```
$ cargo test   # full suite incl. integration tests
... (all integration test binaries) ...
test result: ok. (every binary: 0 failed)
```

```
$ cargo fmt -- --check
(no output — clean)

$ cargo clippy --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.20s
(no warnings)

$ git diff --check
(no output — clean)
```

### Red-step evidence (Steps 1–2)

Before `pub mod telegram;` was declared in `src/channel/mod.rs`:

```
$ cargo test --lib channel::telegram -- --test-threads=1
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 702 filtered out; finished in 0.00s
```

This is the coordinator's flagged "reads like green, is actually red" signal — treated
as such rather than as a pass.

After declaring the module (per the carry-forward instruction: Task 1 deliberately
omitted it since the file didn't exist yet):

```
$ cargo test --lib channel::telegram -- --test-threads=1
error[E0425]: cannot find function `parse_updates` in this scope
...
error: could not compile `dct` (lib test) due to 11 previous errors
```

This matches the brief's predicted Step 2 outcome exactly.

## Concerns / notes for later tasks

- **Task 5 (`bridge.rs`) needs to know about `Telegram`'s own chat-id learning.** Bridge's
  planned `owner: Mutex<Option<i64>>` and `Telegram`'s `chat_id: Mutex<Option<i64>>` are
  two independent "first message wins" trackers fed by the same `poll()` stream. As long
  as both consume updates in the same order (they will, since both derive from the same
  `Vec<Incoming>` per poll batch) they stay in sync, but this duplication is worth a
  second look when Task 5 is implemented — ideally Bridge becomes the single source of
  truth and either Telegram's own tracking is proven redundant-but-harmless, or the two
  are unified.
- Telegram's real transport (`send_real`) has no unit test coverage, by design — same
  boundary as `llm/http.rs::send_real` and `verify.rs::send_probe`. It needs a real bot
  token run before shipping, consistent with the existing "四个 vendor base_url 从没拿真
  key 跑过" risk already on record for this codebase.
- `getUpdates` is called via `agent.get(url).call()`; `sendMessage` via `agent.post(url).send_string(body)`
  keyed off whether `body` is empty. This wasn't specified anywhere and is my own choice
  — reasonable (Telegram's Bot API accepts both GET-with-query-params and POST-with-JSON
  for `getUpdates`), but worth a real-network smoke test before shipping, same risk noted
  above.

---

## Fix round 1 (Critical + 4 Important + Minor)

Commit: `22fbe60` — fix: stop learning the outbound telegram destination from raw polls

### Critical — destination tracker duplicated state Bridge owns, and could disagree

The coordinator identified that `Telegram.chat_id`, learned first-write-wins inside
`poll()`, ran *before* Bridge's ownership check — a stranger who messaged the bot first
could permanently capture the outbound destination, and neither re-pairing nor a new
token had any reset path (the field was private, no setter existed, and the only writer
refused to overwrite). Confirmed this analysis is correct: Bridge's planned `accept()`
only filters what gets *typed into a session*; it was never going to be consulted before
`Telegram` decided who to *send to*.

Fix, exactly as directed:
- Deleted the learning code that used to sit at the end of `poll()` (previously around
  `telegram.rs:190-196`) and the `the_learned_chat_does_not_get_overwritten_by_a_later_sender`
  test.
- Added `fn set_destination(&self, chat: Option<i64>);` to the `Channel` trait in
  `src/channel/mod.rs`. `Telegram` implements it as `*recover(self.destination.lock()) = chat;`
  — the field was renamed from `chat_id` to `destination` to match.
- Kept `send()` returning `Unreachable` before any destination is set, and kept
  `sending_before_any_destination_is_set_is_unreachable` (renamed from
  `sending_before_any_chat_is_known_is_unreachable` to match the new vocabulary).
- Added the inverse test, `polling_a_strangers_message_never_sets_a_destination`: polls a
  message from an unrecognized chat, asserts the `Incoming` is still returned (rejecting it
  is Bridge's job, not the transport's), then asserts `send()` is still `Unreachable` and
  no network call happened for it.
- Added `send_targets_the_configured_destination` (destination set via `set_destination`,
  not learned) and `set_destination_can_be_cleared` (covers the re-pair-to-a-different-account
  path: `Some` → `None` → `send()` goes back to `Unreachable`).

Task 5 (`bridge.rs`) will call `set_destination` wherever it decides ownership — not built
here, per the coordinator's note that it's carrying that forward in the ledger.

### Important 1 — the transport `Duration` was never asserted

`FakeTransport::sender` previously took `_timeout: Duration` and dropped it. Changed
`FakeTransport.calls` to `Vec<(String, String, Duration)>`, recording the third argument.
Added `poll_and_send_each_carry_their_own_client_timeout`, asserting `30s` for `poll(25s)`
(`POLL_TIMEOUT_MARGIN` = 5s) and `10s` for `send` (`SEND_TIMEOUT`).

### Important 2 — nothing pinned "the cursor advances past a text-less update"

Added `the_cursor_advances_past_a_text_less_update_too`: polls a sticker-only batch
(`update_id: 9`, no `text`), asserts the returned `Vec<Incoming>` is empty, then polls
again and asserts the URL carries `offset=10&` — proving the cursor moved past an update
that never became an `Incoming`. Verified by mutation (see table below) that gating the
cursor advance on `!incoming.is_empty()` instead of `max_update_id(&body).is_some()` now
fails this test.

### Important 3 — two tests that didn't discriminate what they claimed

- `continue`-vs-`break` in the text-skip branch: added
  `a_text_less_update_does_not_stop_the_rest_of_the_batch_from_being_read`, a two-element
  batch (text-less update, then a real text message). Mutating the `continue` to `break`
  now fails this test (previously all fixtures were single-element, so `break` and
  `continue` were indistinguishable).
- `send_real` dropping the HTTP status: changed `Sender` to
  `Fn(&str, &str, Duration) -> Result<(u16, String), String>`, matching `llm/http.rs`'s
  shape. Added `reinterpret(err, status)`, called at all three call sites (`send`, `poll`,
  `get_me`), mapping `Malformed` + a 5xx status to `Unreachable`. Added
  `a_5xx_with_an_unparseable_body_is_unreachable_not_malformed` (502 + HTML body →
  `Unreachable`) and `a_4xx_with_an_unparseable_body_is_still_malformed` (400 + garbage
  body → stays `Malformed`, proving the boundary is 5xx-only, not "any non-2xx").

### Important 4 — `parse_get_me` had no caller and no seam

Added `Telegram::get_me(&self) -> Result<String, ChannelError>`, an inherent method (not
on the `Channel` trait — Task 4 doesn't need it there) going through `self.send` with
`SEND_TIMEOUT`, hitting `getMe`. Added `get_me_goes_through_the_injected_transport`
(asserts the URL ends `/getMe` and the timeout is `SEND_TIMEOUT`, not the long-poll one)
and `get_me_surfaces_bad_token_through_the_channel`.

### Minor

- Replaced all production-code `.lock().unwrap()` in `telegram.rs` with `crate::session::recover`
  (offset lock in `poll()`, destination lock in `send()`/`set_destination()`) — `Telegram`
  is shared between the inbound and outbound threads and the daemon has no supervisor to
  restart a poisoned mutex.
- Fixed `calls[1].0.contains("offset=8")` → `contains("offset=8&")` (the old assertion
  would also have passed against `offset=80`).

### Not fixed here (recorded against Task 4, not this task's error)

403 → `BadToken` is wrong for the most common real-world 403
(`Forbidden: bot was blocked by the user` — literally the test fixture's own description).
The brief mandated this mapping, so it isn't this task's bug to fix; Task 4 needs a
distinct user-facing string for "the bot was blocked," not "re-enter your token."

### Mutation table, fix round 1

All mutations applied to the working tree one at a time, confirmed to fail only the
expected test(s), then reverted before the fix-round commit.

| Mutation | Expected | Result |
|---|---|---|
| `error_from`: `Some(401) \| Some(403)` → `Some(401)` only (re-verified after refactor) | `a_403_is_also_bad_token_not_unreachable` fails | Confirmed: FAILED (1) |
| `poll()` offset update: `max_id + 1` → `max_id` (re-verified after refactor) | `the_second_poll_carries_the_offset_past_the_last_update_id` fails | Confirmed: FAILED (2 — this one and `the_cursor_advances_past_a_text_less_update_too` both fail) |
| Cursor gate: `if let Some(max_id) = max_update_id(&body)` → `if !incoming.is_empty() { if let Some(max_id) = ... }` | `the_cursor_advances_past_a_text_less_update_too` fails | Confirmed: FAILED, `offset=0` instead of `offset=10&` |
| Text-skip: `continue` → `break` in `parse_updates` | `a_text_less_update_does_not_stop_the_rest_of_the_batch_from_being_read` fails | Confirmed: FAILED, `left: 0, right: 1` |
| `reinterpret`: `(500..600)` → `(400..600)` (over-broad) | `a_4xx_with_an_unparseable_body_is_still_malformed` fails | Confirmed: FAILED, `left: Unreachable, right: Malformed` |

### Test commands and output tails, fix round 1

```
$ cargo test --lib channel::telegram -- --test-threads=1
running 24 tests
test channel::telegram::tests::a_403_is_also_bad_token_not_unreachable ... ok
test channel::telegram::tests::a_4xx_with_an_unparseable_body_is_still_malformed ... ok
test channel::telegram::tests::a_5xx_with_an_unparseable_body_is_unreachable_not_malformed ... ok
test channel::telegram::tests::a_revoked_token_is_bad_token_not_unreachable ... ok
test channel::telegram::tests::a_text_less_update_does_not_stop_the_rest_of_the_batch_from_being_read ... ok
test channel::telegram::tests::a_transport_level_failure_is_unreachable ... ok
test channel::telegram::tests::garbage_is_malformed ... ok
test channel::telegram::tests::get_me_goes_through_the_injected_transport ... ok
test channel::telegram::tests::get_me_returns_the_bot_username ... ok
test channel::telegram::tests::get_me_surfaces_bad_token_through_the_channel ... ok
test channel::telegram::tests::get_me_with_a_bad_token_says_bad_token ... ok
test channel::telegram::tests::parses_a_normal_update ... ok
test channel::telegram::tests::picks_up_the_message_being_replied_to ... ok
test channel::telegram::tests::poll_and_send_each_carry_their_own_client_timeout ... ok
test channel::telegram::tests::poll_puts_the_requested_seconds_in_the_url ... ok
test channel::telegram::tests::polling_a_strangers_message_never_sets_a_destination ... ok
test channel::telegram::tests::reads_the_new_message_id_back ... ok
test channel::telegram::tests::send_surfaces_bad_token ... ok
test channel::telegram::tests::send_targets_the_configured_destination ... ok
test channel::telegram::tests::sending_before_any_destination_is_set_is_unreachable ... ok
test channel::telegram::tests::set_destination_can_be_cleared ... ok
test channel::telegram::tests::the_cursor_advances_past_a_text_less_update_too ... ok
test channel::telegram::tests::the_second_poll_carries_the_offset_past_the_last_update_id ... ok
test channel::telegram::tests::updates_without_text_are_skipped_not_errors ... ok

test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 702 filtered out; finished in 0.00s
```

```
$ cargo test --lib channel::
running 26 tests (24 telegram + 2 channel::tests from mod.rs)
test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 700 filtered out; finished in 0.01s
```

```
$ cargo test --lib
test result: ok. 726 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.42s
```

```
$ cargo test   # full workspace incl. integration test binaries
... (every binary) ...
test result: ok. (every binary: 0 failed)
```

```
$ cargo fmt -- --check
(no output — clean)

$ cargo clippy --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.38s
(no warnings)

$ git diff --check
(no output — clean)
```

---

## Fix round 2 (1 Important, 2 Minor)

Commit: `1a5344c` — fix: strengthen the destination-clear test and document send's staleness window

The scoped re-review verdicted the Critical and all four Important findings from round 1
ADDRESSED (verified by reading, not trusting: destination is only ever written by the
constructor and `set_destination`; `send` re-reads the lock every call so a clear is
visible immediately; `reinterpret` can't rewrite `BadToken` since that value only comes
from a successfully-parsed `ok:false` body, never `Malformed`; `&self` confirmed as the
only workable signature given `Arc<dyn Channel>`). Three small items came back.

### Important — `set_destination_can_be_cleared` did not discriminate

The test only asserted `send("hi")` returns `Unreachable` after `Some(777)` then `None`.
The reviewer found that mutating `set_destination` to first-write-wins
(`if d.is_none() { *d = chat; }`) leaves the destination at `Some(777)`: `send()` still
builds the request and calls the (out-of-replies) fake transport, which returns `Err`,
which `send()` maps to the same `Unreachable` the assertion expects — so the reset half
of the round-1 Critical fix had zero real coverage. Added:

```rust
assert!(fake.calls.lock().unwrap().is_empty(), "不该打网络");
```

Applied the exact mutation described (`if d.is_none() { *d = chat; }` in `set_destination`)
to the working tree, confirmed `set_destination_can_be_cleared` now fails with `不该打网络`
(the network *was* called), then reverted.

### Minor — `offset=0` vs. its sibling's `offset=8&`

`the_second_poll_carries_the_offset_past_the_last_update_id`'s first assertion used
`contains("offset=0")` while its second used the already-tightened `contains("offset=8&")`.
Changed the first to `contains("offset=0&")` for consistency (no known false-positive today,
just the two lines disagreeing).

### Minor — documented `send()`'s actual staleness guarantee

Added a comment at the destination snapshot in `send()` (reads the lock once, releases it
before the up-to-10s network call — holding it across the request would block
`set_destination` for that whole window, a worse trade). The comment states plainly what
this buys: **no new `send()` reaches the old chat after a reset — not that an in-flight
send started just before the reset gets recalled.** Written so Task 5's unpair path
doesn't assume a stronger guarantee than what's actually implemented.

### Not fixed here (recorded for later, not this task's error)

- `get_me` is inherent, not on the `Channel` trait, exactly as the brief prescribed. If a
  future task holds `Arc<dyn Channel>` instead of a concrete `Telegram`, it won't reach
  `get_me` and the trait will need the method then.
- `Option<i64>` in `set_destination`/`Incoming.chat_id` hard-codes a numeric chat id — a
  future adapter with string handles couldn't represent its destination through this
  trait. Pre-existing constraint from Task 1's `Incoming.chat_id: i64`; `set_destination`
  is deliberately symmetric with it, so nothing new was introduced.

### Test commands and output tails, fix round 2

```
$ cargo test --lib channel::telegram -- --test-threads=1
running 24 tests
...
test channel::telegram::tests::set_destination_can_be_cleared ... ok
...
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 702 filtered out; finished in 0.00s
```

```
$ cargo test --lib
test result: ok. 726 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.44s
```

```
$ cargo test   # full workspace incl. integration test binaries
... (every binary) ...
test result: ok. (every binary: 0 failed)
```

```
$ cargo fmt -- --check
(no output — clean)

$ cargo clippy --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.66s
(no warnings)

$ git diff --check
(no output — clean)
```
