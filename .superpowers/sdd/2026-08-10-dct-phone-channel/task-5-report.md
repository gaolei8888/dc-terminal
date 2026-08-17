# Task 5 report: bridge skeleton — long polling, pairing, exactly one chat id

## What was implemented

`src/bridge.rs` (new): `Bridge::new(ch: Arc<dyn Channel>, phone: Arc<Mutex<PhoneStatus>>)`,
`Bridge::accept(&self, msg: &Incoming) -> Accepted` with `Accepted { Paired(i64), FromOwner,
Rejected }`, `Bridge::dispatch` (writes the shared status slot only on the pairing message),
`Bridge::run` (the poll loop) and module-level `spawn()` which owns the `catch_unwind` boundary.

`src/channel/mod.rs`: added `Channel::get_me(&self) -> Result<String, ChannelError>` to the
trait so `bridge.rs` can ask "who is this bot" through `Arc<dyn Channel>` without knowing it's
Telegram.

`src/channel/telegram.rs`: `impl Channel for Telegram` now has a trait-level `get_me` that
delegates to the existing inherent `Telegram::get_me` (used directly by
`Request::PhoneSetToken`, which doesn't go through `dyn Channel`). Confirmed no ownership state
survives here — the adapter has only `token`, `offset`, and the injected `send` closure;
`Channel::send`'s `to` parameter is what decides the destination on every call, proven by
`send_posts_to_the_chat_id_the_caller_passed`.

`src/daemon.rs`: two call sites now call `crate::bridge::spawn(ch, phone.clone())` — one at
startup if a token is already in the secret store (restart case), one inside
`Request::PhoneSetToken` right after the token is verified and persisted (first-run case). Both
pass the *same* `Arc<Mutex<PhoneStatus>>` created once near the top of `run_with_manager`; I
grepped for a second `Arc::new(Mutex::new(PhoneStatus` in the runtime path and found only the
one (the other constructions are in test helpers).

`src/lib.rs`: added `pub mod bridge;`.

## Pairing rule — checked directly, not just via tests

```rust
match *owner {
    None => { *owner = Some(msg.chat_id); Accepted::Paired(msg.chat_id) }
    Some(o) if o == msg.chat_id => Accepted::FromOwner,
    Some(_) => Accepted::Rejected,
}
```

This match is exhaustive over `Option<i64>` and total: `None` only ever happens once (it's
overwritten synchronously under the same `Mutex` guard before the match returns), so a second
caller can never observe `None` again — pairing happens exactly once. Every `Some(o)` branch is
covered (`o == chat_id` or not), so every non-owner message is `Rejected` unconditionally,
forever, regardless of content. No `continue`/early return bypasses the lock.

## Thread isolation

- `Bridge::run` is never called directly from `spawn()`'s public contract; `spawn()` wraps the
  whole body in `catch_unwind(AssertUnwindSafe(|| bridge.run()))` and discards the `Result`, so a
  panic anywhere in `ensure_bot_known`/`run`/`dispatch` only ends the polling thread. Verified
  with `a_panic_inside_run_is_caught_not_propagated`.
- Non-blocking: `spawn()` uses `std::thread::spawn`, called from `Request::PhoneSetToken`'s
  handler and from daemon startup — neither blocks the daemon's request loop or other sessions.
- Backoff: `next_backoff` doubles and clamps at `MAX_BACKOFF = 300s` (5 min), starting from
  `INITIAL_BACKOFF = 1s`. Pure function, tested by `backoff_doubles_and_caps_at_five_minutes`
  without any real sleeping.
- Non-retryable errors (`BadToken`, `Malformed`) call `mark_broken` and `return` — the loop stops
  and `PhoneState::Broken(<prose>)` is written. `Unreachable` always retries and never reaches
  `mark_broken`.

## `bot` population

`ensure_bot_known()` runs first inside `Bridge::run`, before entering the poll loop, calling
`ch.get_me()` with the same backoff as polling. On success it writes
`recover(self.phone.lock()).bot = Some(username)` into the one shared slot. If `get_me` returns a
non-retryable error, `Broken` is written and `run` returns without ever calling `poll` — verified
by `a_bad_token_at_startup_never_reaches_poll`. This closes the Task 4 gap where `bot` stayed
`None` across a restart until the first successful poll.

## Mutation testing (both run myself)

1. `Some(o) if o == msg.chat_id` → `!=`: ran `cargo test --lib bridge:: -- --test-threads=1`.
   Result: 6 of 15 bridge tests failed, including the required
   `a_stranger_is_rejected_even_after_pairing` and `the_first_person_to_message_becomes_the_owner`
   and `pairing_happens_exactly_once`. **Caught.**
2. `None` arm changed to not store the owner (always returns `Paired`): same command. Result: 8
   of 15 bridge tests failed, including the required `pairing_happens_exactly_once` (also broke
   `the_first_person_to_message_becomes_the_owner` and both `dispatch` pairing tests). **Caught.**

File was restored to the pre-mutation version after each run (diffed against a saved copy to
confirm exact restoration).

## Adversarial tests added beyond the brief's three

- `a_crowd_of_strangers_are_all_rejected` — cycles many distinct chat ids (including negative,
  zero, `i64::MAX`/`MIN`) after pairing; a single-stranger test wouldn't catch an off-by-one or a
  small allowlist bug.
- `message_text_never_influences_who_the_owner_is` — message content that *claims* to be the
  owner must not matter; only `chat_id` counts.
- `negative_and_zero_chat_ids_pair_normally` — Telegram group/channel ids are negative; the
  equality check must not special-case sign or zero.
- `dispatch_on_pairing_writes_paired_state_and_owner` / `dispatch_from_owner_does_not_touch_the_slot_again`
  / `dispatch_from_a_stranger_leaves_the_slot_untouched` — the status slot must change on the
  pairing moment only, never again, and never for a rejected sender (no partial writes that could
  leak a stranger's id into `owner`).
- `broken_message_is_prose_not_a_debug_dump` / `broken_message_never_contains_anything_token_shaped`
  — enforces the "never the token, never raw error text" requirement structurally.
- `run_populates_bot_then_pairs_then_stops_on_bad_token`, `a_bad_token_at_startup_never_reaches_poll`,
  `a_panic_inside_run_is_caught_not_propagated` — exercise the full poll loop, the get_me-before-poll
  ordering, and the panic boundary via an injected mock `Channel` (no network).

## Things found and fixed

Nothing needed fixing in this diff. The duplicate "first sender wins" latch that used to live in
`src/channel/telegram.rs` (a `chat_id: Mutex<Option<i64>>` field) is already gone — confirmed by
reading the current `Telegram` struct (only `token`, `offset`, `send`) and by the test comment in
`send_posts_to_the_chat_id_the_caller_passed` explicitly naming that removed field as the bug it
guards against. Ownership now exists in exactly one place: `Bridge::owner`.

## Commands run

```
cargo fmt --check                     # clean
cargo clippy --all-targets            # 0 warnings
cargo test -- --test-threads=1        # 787 passed, 0 failed (matches controller's baseline)
cargo test --lib bridge:: -- --test-threads=1   # 15 passed (pre-mutation)
# mutation 1 (== -> !=): 6 failed, restored
# mutation 2 (None arm drops the write): 8 failed, restored
```

## Concerns

- `dispatch` is only exercised directly by unit tests; it isn't yet wired to type text into a
  session (that's Task 7, per the brief's own comment `// …… 消息映射与当前会话见 Task 7`). This
  task's scope is correctly the gate, not the delivery.
- `owner: String` display in `PhoneStatus` is currently just the numeric chat id (no Telegram
  username available from `Incoming`) — the code says so honestly in a comment; not a defect for
  this task, flagging for whoever writes the phone status UI copy.

---

# Fix report: independent security review (commit 9e551be) — C1/C2/C3 + I1/I2

The independent review confirmed `Bridge::accept` itself was correct (total match, guard held
across the whole decision, `recover()` preserves the owner across a poisoned lock, a rejected
stranger gets total silence). Every finding was at the lifecycle level around `accept()` — how
`owner` is initialized across a restart, and how the polling thread is stopped/replaced. This
section covers each finding, what changed, and the mutation testing that backs it.

## C1 (Critical) — pairing re-opened on every daemon restart

**Root cause confirmed as reported.** `daemon.rs` spawned a fresh `Bridge` at startup whenever a
token existed in the secret store, always with `owner: None` and a `Telegram` starting at offset
0. A stranger who messaged the (public, searchable) bot username while `dct` was down would have
their queued update be the first `Incoming` after a restart, and `accept()` would correctly (per
its own contract) pair them.

**Fix, per Ruling 9:**

1. **Persist the owner.** New `PHONE_OWNER_KEY` constant in `src/secrets.rs`, stored in the same
   `secrets.toml` with the same atomic-write path as the token — no second storage mechanism.
2. **`Bridge::new` now takes an explicit `owner: Option<i64>`.** `daemon.rs`'s
   `startup_bridge_owner(secrets: &SecretStore) -> Option<i64>` reads `PHONE_OWNER_KEY` and parses
   it; a missing or unparsable value degrades to `None` (never panics, never invents an owner).
   When `Bridge::run()` sees a non-`None` owner at construction, it **skips both pairing and
   backlog draining** and goes straight to the normal poll loop (`run_with_a_known_owner_skips_
   backlog_draining_and_polls_directly`, asserted via the timeout passed to `poll()`: `POLL_TIMEOUT`
   for the direct path vs. `Duration::ZERO` for draining).
3. **When pairing is genuinely open (`owner` is `None`), `Bridge::run()` calls the new
   `drain_backlog()` before entering the normal loop.** It calls `ch.poll(Duration::ZERO)`
   repeatedly — Telegram's `getUpdates` returns any currently-buffered batch immediately at a
   0-second timeout rather than waiting for the timeout — discarding every non-empty batch without
   ever calling `dispatch()`/`accept()` on those messages, until a batch comes back empty. Only
   messages that arrive *after* that point can pair. Covered by
   `a_backlog_message_present_before_pairing_opens_is_discarded_not_paired`, which queues an
   attacker's message ahead of the real owner's and asserts the owner ends up being the real
   sender, not the backlog message, and that the backlog sender is still `Rejected` afterward.
4. **Persistence happens at the moment of pairing, not later.** `Bridge::dispatch()` now calls a
   `persist_owner: Box<dyn Fn(i64) + Send + Sync>` closure exactly once, before it writes the
   in-memory `PhoneStatus` slot (disk before cache, so the on-disk truth is never behind what the
   UI claims). Production closure (`daemon.rs::persist_owner_closure`) writes to the secret store;
   test closures are no-ops or record calls. `dispatch_on_pairing_calls_persist_owner_exactly_once`
   pins that `FromOwner`/`Rejected` never re-trigger it.
5. **A fresh token clears the persisted owner.** `Request::PhoneSetToken` now removes
   `PHONE_OWNER_KEY` before starting the new bridge with `owner: None` — a new token means a new
   bot that nobody has paired with yet; without this, `startup_bridge_owner` on the next restart
   would hand the *previous* bot's chat id to the new bot.

**Known limitation, noted rather than silently accepted:** if `persist_owner`'s disk write fails
(e.g. disk full), pairing succeeds for this session but the next restart won't recall the owner
and will briefly re-open pairing (draining the backlog again, same protection as first-time
pairing — not a silent hole, but a repeat of the "first message after restart wins" window). This
mirrors the project's existing "bookkeeping failure shouldn't take down the primary function"
stance (`journal.rs`); the failure is logged to stderr for diagnosis.

## C2 (Critical) — `PhoneUnpair`/`PhoneDisable` didn't reach the thread

**Root cause confirmed.** Neither request touched anything but the `PhoneStatus` cache; the live
`Bridge::owner` (unpair) or the live polling thread itself (disable) were invisible to `daemon.rs`
because `bridge::spawn` returned nothing.

**Fix:** `spawn()` now returns a `BridgeHandle` wrapping `Arc<Bridge>`, with `stop()`, `unpair()`
(clears `Bridge::owner` in place, no thread restart, no backlog re-drain needed — the channel's
offset is already past everything already consumed), and `accept()` (pass-through, used by tests
and by the eventual Task 7 dispatch path). `daemon.rs` now holds one
`Arc<Mutex<Option<BridgeHandle>>>` slot, created once in `run_with_manager` and threaded through
`serve`/`handle` (a new parameter on both, plus a `#[allow(clippy::too_many_arguments)]` on
`handle` — seven parameters now, all necessary state handles, no bundling attempted since existing
sibling parameters aren't bundled either).

- `Request::PhoneUnpair` now: removes `PHONE_OWNER_KEY` from the secret store, calls
  `handle.unpair()` on the live bridge if one exists, then updates the `PhoneStatus` cache as
  before. Regression test `phone_unpair_forgets_the_owner_but_keeps_the_token` was rewritten to
  spin up a real `BridgeHandle` (via `StubChannel`, which panics if its network methods are ever
  called — the handle's background thread panicking on `get_me()` doesn't affect the test's
  synchronous assertions against `Bridge::owner`) with a known owner, unpair through `handle()`,
  and then directly call `bridge_handle.accept()` to prove a new chat id can now pair — this is
  "does unpair reach the bridge," not just "does unpair touch a cache."
- `Request::PhoneDisable` now: removes both `PHONE_TOKEN_KEY` and `PHONE_OWNER_KEY`, calls
  `bridge::stop_current(&bridge)`, then resets the `PhoneStatus` cache. Rewritten
  `phone_disable_deletes_the_token_and_resets_the_slot` asserts the bridge slot is `None`
  afterward, not just that the response says `Off`.

## C3 (Critical) — two live bridges with independent latches

**Root cause confirmed.** `Request::PhoneSetToken` called `bridge::spawn` unconditionally,
regardless of whether a bridge from a previous token (or the startup path) was still running.

**Fix:** New `bridge::replace(slot, ch, phone, owner, persist_owner)` is the *only* way `daemon.rs`
starts a bridge now (startup and `PhoneSetToken` both call it; disable calls the sibling
`stop_current(slot)`). `replace()` takes the slot's mutex, stops whatever `BridgeHandle` is
currently there (if any), and only then installs the new one — so the slot never holds more than
one live handle, and there's a well-defined instant (inside the lock) where the old thread has
been told to stop before the new one starts.

Regression test `replace_stops_the_old_bridge_before_starting_the_new_one`: spins up bridge A with
a `MockChannel`, waits until it has actually polled at least once, calls `replace()` with bridge
B's channel, then asserts A's poll-call counter stops growing (sampled twice with a wait between,
not just "it didn't grow once") while B's counter is greater than zero. `stop_current_stops_the_
bridge_and_leaves_the_slot_empty` covers the disable path the same way, plus asserts the slot is
`None` afterward.

## I1 — untested properties

Added, all in `src/bridge.rs` unless noted:

- `only_one_thread_ever_wins_pairing_when_racing` — 64 threads call `accept()` concurrently on a
  fresh bridge with distinct chat ids; asserts exactly one `Paired` and the other 63 are all
  `Rejected`. (`accept()`'s check-and-set already happens under one lock acquisition, so this is a
  confirmation, not a fix, but it's the concurrency property the review asked to see pinned down.)
- `bridge_handle_unpair_clears_the_owner_and_reopens_pairing` (bridge.rs) and
  `phone_unpair_forgets_the_owner_but_keeps_the_token` (daemon.rs, rewritten as above) — unpair
  actually reaching the bridge, at both the `BridgeHandle` level and the full `Request::PhoneUnpair`
  level.
- `stop_actually_stops_the_polling_thread`, `replace_stops_the_old_bridge_before_starting_the_new_
  one`, `stop_current_stops_the_bridge_and_leaves_the_slot_empty` — a second spawn (or a stop)
  cannot leave an orphaned poller alive. `wait_for_join()` wraps `JoinHandle::join()` with a
  bounded `recv_timeout` on a watcher thread, since the std API has no native join-with-timeout —
  this gives a real "the thread exited" proof instead of a fixed `sleep` guess.
- `owner_survives_a_panic_while_the_lock_was_held` — mirrors
  `session.rs::recovers_from_poisoned_sessions_lock`: panics while holding `Bridge::owner`'s lock,
  catches it, and asserts `accept()` still remembers the owner and still rejects strangers
  afterward (exercises `recover()`'s poison-recovery path specifically for this lock).
- `startup_bridge_owner_reads_the_persisted_value_or_says_so_honestly` and
  `startup_uses_the_persisted_owner_and_never_reopens_pairing` (daemon.rs) — the C1 fix wired all
  the way through the real startup call site, not just at the `Bridge` level. The second test
  required extracting `start_phone_bridge(secrets, phone, bridge, make_channel)` with an injectable
  channel constructor, because the production startup path calls real `Telegram::new(token)`,
  which would otherwise force this test to touch the network. `StubChannel` (daemon.rs test module)
  panics if its network methods are ever invoked; the assertions are synchronous calls to
  `BridgeHandle::accept()` and don't depend on the (possibly-panicking) background thread.
- `stop_interrupts_a_long_backoff_sleep_instead_of_waiting_it_out` — `stop()` must be able to cut a
  backoff sleep short rather than making the caller wait out the full (up to 5-minute) delay.

## I2 — `broken_message_never_contains_anything_token_shaped` couldn't fail

Deleted. It scanned three `const`-derived literals for `':'` and the substring `"token"` — since
those literals are hand-written Chinese prose with no interpolation, no input could ever make the
test fail; it was testing that the author didn't type an unlikely string, not a real guarantee.
The actual guarantee — `ChannelError`'s three variants carry no `String` field, so `broken_message`
structurally cannot forward a token or raw error text — is now stated as a doc comment on
`broken_message` itself instead of as a test that can't fail.

## Minors

- **`bridge.rs`'s `broken_message` and `daemon.rs`'s `phone_set_token_failure_message` prose for
  the same three `ChannelError` variants.** Evaluated merging into one shared table and decided
  against it: the two functions answer different UX moments — `phone_set_token_failure_message`
  fires synchronously while the user is still looking at the token-entry field ("go back to
  BotFather and check it, paste it again"), `broken_message` fires later, after the channel had
  been working and then broke ("go to settings and reconnect"). Collapsing them would flatten that
  distinction for a low (and already test-covered per-function) drift risk. Left as-is, documented
  here per the "fix if cheap, otherwise note and leave" instruction.
- **`dispatch_from_owner_does_not_touch_the_slot_again` only asserted `owner`, not `state`.**
  Fixed — the test now also sets `state` to a sentinel-adjacent value (`WaitingForPairing`, which a
  `Paired` dispatch had just overwritten to `Paired`) before the second `dispatch()` call and
  asserts `state` is unchanged by the `FromOwner` path, not just `owner`.

## Mutation testing on the new lifecycle code

All four run directly, each restored via `diff` confirmation before moving to the next:

1. **`accept()`'s `==` → `!=`** (re-run after the rewrite, to confirm the refactor didn't weaken
   coverage): `cargo test --lib bridge:: -- --test-threads=1` → 11 of 25 bridge tests failed,
   including `a_stranger_is_rejected_even_after_pairing`, `pairing_happens_exactly_once`, and the
   new `a_bridge_restored_with_a_known_owner_never_reopens_pairing`. **Caught.**
2. **`None` arm drops the owner write** (re-run): 12 of 25 failed, including
   `pairing_happens_exactly_once` and `dispatch_on_pairing_calls_persist_owner_exactly_once`.
   **Caught.**
3. **`Bridge::stop()` made a no-op** (the controller's required lifecycle mutation): same command.
   `stop_actually_stops_the_polling_thread`, `replace_stops_the_old_bridge_before_starting_the_new_
   one`, and `stop_current_stops_the_bridge_and_leaves_the_slot_empty` all **FAILED** immediately
   (their `wait_for_join`/poll-count-stops-growing assertions have nothing to observe once `stop()`
   does nothing). A fourth test, `stop_interrupts_a_long_backoff_sleep_instead_of_waiting_it_out`,
   originally asked `sleep_or_stop` to sleep for the real `MAX_BACKOFF` (5 minutes) and only failed
   once that ran out — a correct but slow catch. Fixed during this same pass: the test now asks for
   a 2-second sleep instead (a local `A_LONG_SLEEP` constant, with a comment explaining that the
   test is about "does stop() interrupt a sleep," not about the specific 5-minute cap, which
   `backoff_doubles_and_caps_at_five_minutes` already pins separately) and tightened the passing
   assertion to `< 500ms`. Re-ran the mutation after that change
   (`cargo test --lib bridge::tests::stop -- --test-threads=1`): `stop_actually_stops_the_polling_
   thread`, `stop_current_stops_the_bridge_and_leaves_the_slot_empty`, and
   `stop_interrupts_a_long_backoff_sleep_instead_of_waiting_it_out` all **FAILED**, the whole
   3-test run finishing in under 5 seconds (`replace_stops_the_old_bridge_before_starting_the_new_
   one` wasn't included in this narrower re-run but shares the same "old bridge's poll count keeps
   growing" assertion shape as `stop_current_...`, already confirmed failing on the first pass of
   this mutation). **Caught, quickly, by all affected tests.**
4. **Startup path ignores the persisted owner** (`start_phone_bridge`'s tuple literal changed from
   `(Some(token.to_string()), startup_bridge_owner(&s))` to `(Some(token.to_string()), None)`):
   `cargo test --lib daemon:: -- --test-threads=1` → `startup_uses_the_persisted_owner_and_never_
   reopens_pairing` **FAILED** (`left: Paired(999), right: Rejected` — the stranger's chat id 999
   was accepted as a fresh pairing instead of being rejected against the persisted owner 555).
   **Caught.**

## Commands re-run after the fix

```
cargo fmt --check                     # clean
cargo clippy --all-targets            # 0 warnings
cargo test -- --test-threads=1        # 801 passed, 0 failed (was 787 before this fix; +14 new tests)
```

## Concerns carried forward

- `PhoneSetToken`'s synchronous `Telegram::new(&token).get_me()` validation call (separate from the
  bridge's own `get_me()`) still isn't unit-testable without hitting the network — this is a
  pre-existing limitation of that request handler, not something newly introduced or newly
  regressed by this fix. The bridge-lifecycle behavior it triggers (`replace()`, owner-key
  clearing) is unit-tested at the `bridge.rs` level and via `start_phone_bridge` for the startup
  path; the request's own network validation call is exercised the same way it always was
  (manually / integration-level, not in `cargo test`).
- `stop()` is cooperative, not preemptive: a thread currently blocked inside a real `poll()` call
  (up to `POLL_TIMEOUT` + 5s network slack) won't exit until that call returns. This was true
  before this fix too and is bounded by the existing 25-second constant; not changed here.
- (Resolved during this pass, not carried forward: `stop_interrupts_a_long_backoff_sleep_
  instead_of_waiting_it_out` originally asked for a `MAX_BACKOFF`-length sleep and would have only
  caught a `stop()` regression after 5 minutes; it now asks for 2 seconds and catches the same
  mutation in well under a second, without weakening what it asserts about `stop()` interrupting a
  sleep.)

---

# Fix report: round-2 review — F1 (Critical), F2/F3 (Important)

Round 2 confirmed C2, C3, I1, and I2 fully closed, and C1's core (persistence, startup load,
skip-drain-when-owner-known, mid-drain messages discarded) correct. Three findings remained, all at
points the round-1 fix didn't reach: the drain's termination condition, a stop/dispatch race, and
degrade-on-corruption in the owner reader.

## F1 (Critical) — the backlog drain could be defeated with stickers

**Root cause confirmed.** `drain_backlog` terminated on `Ok(batch) if batch.is_empty()`, where
`batch` was `Channel::poll`'s *parsed* `Vec<Incoming>`. `channel/telegram.rs::parse_updates` (Task
2's correct, still-unchanged rule for `poll()`) silently drops every update without a `text` field
— photos, stickers, group-join notices — while still advancing the offset. A `getUpdates` batch
containing only non-text updates therefore parsed to an empty `Vec`, and `drain_backlog` declared
the backlog clear while raw updates (potentially including an attacker's queued text message
sitting behind 100 stickers) were still being consumed batch by batch. That queued text would then
surface as the very next `poll()` result and pair.

**Fix:** added `Channel::drain(&self, timeout: Duration) -> Result<usize, ChannelError>` to the
trait (`src/channel/mod.rs`) — a method whose contract is explicitly "count every raw update,
`text` or not" (documented on the trait method with the sticker attack spelled out). `Telegram`
implements it (`src/channel/telegram.rs`) via a new `count_raw_updates(body: &str) -> Result<usize,
ChannelError>` function that parses the same JSON `parse_updates` does but returns
`result.as_array().len()` instead of filtering — it does **not** reuse `parse_updates`'s return
value, specifically because that value is the thing F1 exploited. `drain()` advances the offset the
same way `poll()` does. `drain_backlog()` now calls `self.ch.drain(Duration::ZERO)` and terminates
only on `Ok(0)` — there is no parsed message list in this path at all for an attacker's content to
hide behind.

Tests added:
- `channel/telegram.rs::count_raw_updates_counts_updates_with_no_text_too` — the same JSON body
  that makes `parse_updates` return an empty `Vec` must make `count_raw_updates` return `1`.
- `channel/telegram.rs::drain_reports_the_raw_count_of_a_sticker_only_batch_and_still_advances_the_
  offset` — end-to-end on the real `Telegram` struct (injected transport, no network): a
  sticker-only batch reports count `1` via `drain()`, and a follow-up call carries the advanced
  offset forward.
- `bridge.rs::drain_backlog_does_not_stop_on_a_batch_that_is_all_non_text_updates` — `Bridge`-level:
  a `MockChannel` `drain()` queue of `[Ok(100), Ok(0)]` (representing "100 raw updates, all
  stickers" then "now it's empty") must be asked twice before the bridge proceeds to normal
  polling.
- `bridge.rs::a_backlog_message_present_before_pairing_opens_is_discarded_not_paired` — rewritten
  so the backlog phase uses `queue_drain` (counts only) and the real pairing message only ever
  reaches `queue_poll` — the attacker's message is now structurally never parsed during drain, not
  merely "parsed but discarded" as in the round-1 version.

`MockChannel` was restructured with independent `poll_results`/`drain_results` queues and
`poll_calls`/`drain_calls` counters (previously it had one shared `poll_results` queue and
distinguished the drain phase from the normal-poll phase only by comparing the `timeout` argument
each call received — that trick stops working once drain and poll are different trait methods, and
was also incidental, not load-bearing, evidence). All existing `MockChannel`-based tests were
migrated to `queue_poll`/`queue_drain` and to asserting `poll_calls`/`drain_calls` directly instead
of timeout vectors.

## F2 (Important) — a stopped thread could still dispatch one poll's worth of messages

**Root cause confirmed.** `run()`'s `Ok(incoming) =>` arm dispatched immediately without rechecking
`self.stop`. Since `poll()` can block for up to `POLL_TIMEOUT` (25s), `stop()` (from
`PhoneDisable`/`PhoneUnpair`/re-tokening) could land while a poll was in flight; when it returned
with a message, the thread — already told to die — would still write `PhoneStatus{state: Paired,
owner: ...}` into the slot the UI and any *replacement* bridge read, and call `persist_owner`,
writing `PHONE_OWNER_KEY` back to disk after the handler had just deleted or replaced it.

**Fix:** one `if self.stop.load(Ordering::Relaxed) { return; }` added at the top of the `Ok(incoming)
=>` arm in `run()`, before `dispatch()` is called on anything in the batch.

Test: `bridge.rs::a_stop_right_after_poll_returns_prevents_the_message_from_being_dispatched`.
`MockChannel` gained an `on_poll_return: Mutex<Option<Box<dyn Fn() + Send + Sync>>>` hook, invoked
by the mock's `poll()` right before it returns its queued result — the test wires this hook to call
`bridge.stop()`, precisely reproducing "stop() lands in the gap between poll() returning and
dispatch() running" without any real timing/sleep dependency. Asserts the queued stranger message
never becomes the owner.

## F3 (Important) — the persisted owner failed open on an unparseable value

**Root cause confirmed.** `startup_bridge_owner` was `secrets.get(PHONE_OWNER_KEY).and_then(|v|
v.parse().ok())` — `None` (no owner stored, pairing may open) and "stored but corrupt" (pairing
must stay shut) both collapsed to the same `Option::None`, and the caller could not tell them
apart. A valid token plus a corrupted owner field reopened pairing to the first sender, which
Ruling 9 forbids.

**Fix:** `startup_bridge_owner` now returns a three-way `StartupOwner { None, Known(i64), Corrupt }`
(`src/daemon.rs`) instead of `Option<i64>`. `start_phone_bridge` matches on it: `None` proceeds with
`owner: None` (pairing opens, backlog gets drained, as before); `Known(id)` proceeds with
`owner: Some(id)` (pairing stays closed, as before); `Corrupt` **does not start a bridge at all** —
it writes `PhoneState::Broken("手机配对信息读不出来了，去设置页重新粘贴一遍令牌".to_string())` into
the shared status slot and returns. No bridge means no `accept()` call exists for anyone to win —
the strongest available guarantee, not merely "opens with a `None` owner that immediately rejects."
Re-entering a token (`Request::PhoneSetToken`) already clears `PHONE_OWNER_KEY` before starting a
fresh bridge (round-1 fix), so this is also the user's actual recovery path, which the Broken
message points at.

Tests:
- `daemon.rs::startup_bridge_owner_distinguishes_absent_known_and_corrupt` (replaces the round-1
  `startup_bridge_owner_reads_the_persisted_value_or_says_so_honestly`, which asserted the old,
  now-forbidden "corrupt degrades to None" behavior) — asserts all three `StartupOwner` variants.
- `daemon.rs::startup_refuses_to_open_pairing_when_the_persisted_owner_is_corrupt` — full
  `start_phone_bridge` path with a token present and a corrupt `PHONE_OWNER_KEY`: asserts the
  bridge slot stays `None` and the status slot is `Broken` with non-empty Chinese prose.

## Mutation testing (all three, each restored via `diff` before moving to the next)

1. **F1** — `drain_backlog`'s termination condition reverted to `self.ch.poll(Duration::ZERO).
   map(|v| v.len())` checked against `Ok(0)` (i.e., back to the parsed-length check):
   `cargo test --lib bridge:: -- --test-threads=1` → 3 failures:
   `a_backlog_message_present_before_pairing_opens_is_discarded_not_paired`,
   `drain_backlog_does_not_stop_on_a_batch_that_is_all_non_text_updates` (the sticker-batch test —
   `left: 0, right: 2`, meaning it stopped after one drain call instead of two), and
   `run_populates_bot_then_pairs_then_stops_on_bad_token`. **Caught.**
2. **F2** — removed the `if self.stop.load(...) { return; }` check from the `Ok(incoming) =>` arm:
   `a_stop_right_after_poll_returns_prevents_the_message_from_being_dispatched` **FAILED**
   (`left: Some("999"), right: None` — the stranger got paired after stop() had already fired).
   **Caught.**
3. **F3** — `startup_bridge_owner`'s `Err(_) => StartupOwner::Corrupt` reverted to `Err(_) =>
   StartupOwner::None`: `cargo test --lib daemon:: -- --test-threads=1` → both
   `startup_bridge_owner_distinguishes_absent_known_and_corrupt` and
   `startup_refuses_to_open_pairing_when_the_persisted_owner_is_corrupt` **FAILED** (the latter's
   failure message printed literally: `配对信息读不出来时不该起任何 bridge——那等于把"读不出来"
   当成"随便谁来都行"`). **Caught.**

## Commands re-run after this round

```
cargo fmt --check                     # clean
cargo clippy --all-targets            # 0 warnings
cargo test -- --test-threads=1        # 804 passed, 0 failed (was 801 before this round; +3 net new tests)
```

## Things reviewed and confirmed unchanged/still correct

Per the reviewer's list: `nothing joins the poller` (no deadlock — `stop()` is fire-and-forget,
`BridgeHandle` holds no `JoinHandle`), no reply or receipt of any kind is ever sent to a rejected
stranger (`Channel::send` is never called from `dispatch`/`accept`/`drain_backlog`), no token
appears in any status text/log/journal (unchanged — `ChannelError` still carries no strings,
`broken_message` and the new `Corrupt` message are both hand-written prose), no ownership state
exists below `bridge.rs` (unchanged — `Telegram` still holds only `token`/`offset`/`send`; the new
`drain()` method doesn't add any), and `catch_unwind` still wraps the entire `run()` body inside
`spawn()` (unchanged).

## Concerns

- `drain()`'s implementation in `telegram.rs` duplicates `poll()`'s URL-construction and
  offset-advancement lines rather than sharing a private helper. Kept separate deliberately: `poll()`
  and `drain()` now have meaningfully different contracts (parsed messages vs. raw count), and a
  shared "fetch raw JSON" helper would need to move the offset-advancement's "only after a validated
  ok:true response" guard into a third function anyway. Given the size of the duplication (URL
  string plus one `if let Ok(v) = ...` block), factoring it out was judged not to reduce net
  complexity; flagging in case a future third caller changes that calculus.
- The Broken-state message for a corrupt persisted owner ("手机配对信息读不出来了，去设置页重新
  粘贴一遍令牌") is plain Chinese, matching every other `PhoneState::Broken` string in this feature
  — none of them currently go through `i18n.rs`'s bilingual machinery (see `broken_message` and
  `phone_set_token_failure_message`, both pre-existing). Not a new inconsistency introduced by this
  fix, but noting it since `PhoneState::Broken` strings are user-facing.
