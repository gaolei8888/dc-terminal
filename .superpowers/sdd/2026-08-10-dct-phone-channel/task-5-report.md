# Task 5 report: bridge skeleton — long poll, pairing, one chat id only

**Status:** complete. Commits on `feat/phone-channel` (base `864b9c8`):
- `0149c20` — `feat: pair with exactly one person and ignore everyone else` (Bridge itself: `accept`/`unpair`/`retire`, `poll_forever`, `run`, the three rejection tests from the brief, plus `set_destination` wiring and `BotBlocked` production)
- `cb43267` — `test: strengthen backoff test to catch a missing attempt reset`
- `864b9c8` — `test: make the fake channel's exhausted script fail closed`
- `488622f` — `feat: wire the phone bridge into daemon.rs, fix a self-deadlock in PhoneUnpair`

The first three commits (bridge.rs itself, fully tested and mutation-hardened) were already on the branch, committed, when I picked this up — `git log` showed them with today's timestamps and the exact commit message the brief specifies for Step 6. `daemon.rs`'s wiring (starting/stopping the Bridge from the three points that own that decision) was present but **uncommitted**, and no report existed yet. I finished the wiring, fixed a real bug in it, added test coverage for the parts that had none, ran the full mutation sweep, and wrote this report.

## Inaccuracy found in the brief, and what I did

The brief's Step 3 code sample for `poll_forever`/`run` is a *sketch*, not literal code — it says "再写轮询线程：`loop { ch.poll(25s) }`" without specifying the retired-flag check, the pairing-confirmation send, or the `BotBlocked` production path. That's not wrong, just underspecified; the actual implementation (already on the branch when I arrived) correctly filled in all three, each with its own justification in the module doc comment. I verified this by reading `src/bridge.rs` end to end rather than assuming the brief's snippet was complete — nothing to fix here, just confirming the gap was already closed correctly.

The concrete errors I found and fixed were in the **daemon.rs wiring**, which the brief doesn't script at all (see "Also: this task unblocks the merge" in my instructions) — see the deadlock section below.

## How `set_destination` is wired, and why nothing else can decide the destination

`Channel::set_destination` has exactly two call sites, both inside `bridge.rs`, both documented in the module's `## set_destination 只从这里调用` section:

- `Bridge::accept()`, in the `None => { ... }` arm, the instant a chat becomes owner: `self.ch.set_destination(Some(msg.chat_id))`.
- `Bridge::unpair()`: `self.ch.set_destination(None)`.

Both drop the `owner` mutex guard *before* touching `ch` — `accept()`'s comment calls out that there's no reason to hold the lock across a call to `set_destination`, mirroring the "two `lock()` calls must be two separate statements" discipline already established in `apply_phone_set_token`. `channel/telegram.rs`'s own `destination` field doc comment tells the same story from the other side: it names the *removed* design (the channel learning its own destination from raw polls) as the actual security hole this task closes, and states `bridge.rs` is now the only legal caller.

Nothing else can decide the destination because nothing else touches `Bridge::owner` or calls `set_destination` — `daemon.rs` never reaches into `Bridge`'s internals; it only calls the two public methods (`accept` indirectly via `poll_forever`, `unpair` directly) and the two lifecycle methods (`retire`, and `Bridge::new`/`Arc::new(Telegram::new(...))` to construct one). I re-verified this is still true after my own changes: `grep -n "set_destination" src/*.rs src/**/*.rs` finds the trait definition, the two call sites in `bridge.rs`, and test-only calls inside `#[cfg(test)]` fakes — nothing in `daemon.rs`.

## Carried-forward items 3, 4, 5

**Item 3 — `PhoneBrokenReason::BotBlocked`'s producer.** `bridge.rs::send_pairing_confirmation` is it: right after `accept()` claims ownership, it sends a pairing-confirmation message to the newly-owned chat; a 403 there is a genuine "this chat blocked the bot" fact, unlike `getMe`/`getUpdates` which never had a chat to be blocked by. I updated `proto.rs::PhoneState::has_confirmed_token`'s doc comment, which previously said (accurately, at the time it was written) that `BotBlocked` was "a promise, not a fact today, and any future producer must first guarantee a token really is on disk." That promise is now discharged **by construction**, not by convention: reaching `send_pairing_confirmation` requires a prior successful `apply_phone_set_token` (token + bot name on disk) and a prior successful `Bridge::accept` (pairing already happened) — both preconditions are structural, not maintained by a comment someone could forget to update. I rewrote the doc comment to say this plainly instead of describing a future that had already arrived. Same fix applied to the near-duplicate stale comments in `daemon.rs` (`phone_unpair_on_a_blocked_bot_repairs_and_keeps_the_bot_name`'s doc, which said "this state has no producer today") and `i18n.rs`/`proto.rs::Paired`'s doc comment, all of which described Task 5 as future.

**Item 4 — nothing re-verifies a saved token after a daemon restart.** My conclusion: **this is now resolved as a side effect of Bridge's design, and no separate re-verification code is needed.** `run_with_manager` now starts a real `Bridge` poll thread at startup whenever a token is on disk (new code, guarded by `if let Some(token) = ...`). That thread's very first `poll()` call is a real `getUpdates` against Telegram. If the token was revoked, that call comes back `401 → ChannelError::BadToken`, which is not `worth_retrying()`, so `poll_forever` writes `PhoneState::Broken{BadToken}` and exits — within one poll cycle (bounded by `POLL_TIMEOUT` = 25s), not "forever." Once in `Broken{BadToken}`, `has_confirmed_token()` is `false`, `Enter` becomes available again, and the "only way out is `x` then `Enter`" trap Task 4 flagged no longer applies. The long-poll loop *is* the re-verification — continuous, not a one-shot startup probe — so reinstating something like the removed `spawn_phone_startup_refresh` would be redundant, not an improvement.

**Item 5 — `PhoneUnpair` folding `Broken{BotBlocked}` into `WaitingForPairing` unconditionally.** My judgment: **the fold itself is still correct; what was missing was making it true.** All three states covered by `had_confirmed_token` (`WaitingForPairing`, `Paired`, `Broken{BotBlocked}`) genuinely mean "there's a live, previously-validated token — re-pairing makes sense," so collapsing all three to the same next state (`WaitingForPairing`, owner cleared) is the right user-facing behavior; the reader shouldn't see three different `r` outcomes for what's the same intent. What was actually broken is that `Broken{BotBlocked}`'s poll thread had already exited (it stops right after producing that state — see `send_pairing_confirmation`'s doc comment on why continuing to poll is pointless once blocked), so folding the *status* back to `WaitingForPairing` without restarting a listener would have been a lie again, just a differently-shaped one than Critical 2. I kept the unconditional fold and added `restart_needed`, computed once before the fold overwrites `ph.state`, that calls `start_phone_bridge` on the same `Bridge` (same token, no need to rebuild the `Telegram` client) specifically for that one case. This is tested end to end by `phone_unpair_from_bot_blocked_restarts_polling_on_the_same_bridge`.

## The deadlock I found and fixed (not in the brief, found while adding coverage)

The uncommitted `daemon.rs` wiring I inherited had this shape in `PhoneUnpair`:

```rust
if let Some(bridge) = recover(bridge_slot.lock()).clone() {
    bridge.unpair();
    if restart_needed {
        start_phone_bridge(bridge, phone.clone(), bridge_slot);
    }
}
```

This self-deadlocks. Rust extends the temporary `MutexGuard` produced by `bridge_slot.lock()` (inside `recover(...)`) to live for the entire `if let` body, not just the pattern match — the same rule that governs `match` scrutinees. `start_phone_bridge` immediately tries `recover(slot.lock())` on the *same* `bridge_slot`, on the *same* thread that's still holding the outer guard. `std::sync::Mutex` isn't reentrant, so that's a hang, not a panic.

Every existing `PhoneUnpair` test used an empty `bridge_slot` (`test_bridge_slot()` → `None`), so this branch had never been exercised end to end — the `if let Some(bridge) = ...` never matched, so the deadlocked path was never taken. I only found it because `phone_unpair_from_bot_blocked_restarts_polling_on_the_same_bridge` is the first test to populate `bridge_slot` *and* drive the `restart_needed` branch. `cargo test --lib daemon::` hung for 7+ minutes; `ps` showed the actual linked test binary running (not `rustc` compiling), which is what pointed me at a real hang rather than a slow build.

Fix: bind the lock result to a named variable first, so the guard drops at the end of that statement, before the `if let` body runs:

```rust
let bridge = recover(bridge_slot.lock()).clone();
if let Some(bridge) = bridge {
    bridge.unpair();
    if restart_needed {
        start_phone_bridge(bridge, phone.clone(), bridge_slot);
    }
}
```

I applied the same defensive rebinding to the startup call site in `run_with_manager` (`if let Some(token) = recover(secrets.lock())...`), which holds a *different* mutex (`secrets`) across the same shape of `if let` and was **not** actually buggy today — nothing inside that body re-locks `secrets` — but it's the identical fragile pattern that just caused a real hang once, so I closed it defensively rather than leave a second copy of the same footgun.

I did **not** re-run this as a scripted mutation in the sweep below (reverting the fix and re-running the test would just re-trigger the multi-minute hang); the live discovery-and-fix cycle described above is the verification.

## Mutation table

`bridge.rs`, brief-named (Step 5) plus the rest of the module's logic:

| # | Mutation | Test(s) that should catch it | Result |
|---|---|---|---|
| 1 | `accept()`: `Some(o) if o == msg.chat_id` → `!=` | **`a_stranger_is_rejected_even_after_pairing`** (+ `the_first_person_to_message_becomes_the_owner`, `pairing_happens_exactly_once`) | **RED** |
| 2 | `accept()`: `None` arm drops the `*owner = Some(msg.chat_id)` write (accept always reports `Paired`) | **`pairing_happens_exactly_once`** (+ `a_stranger_is_rejected_even_after_pairing`, `a_strangers_message_after_pairing_produces_no_side_effects_in_the_loop`, `accepting_the_first_message_tells_the_channel_where_to_send`, `the_first_person_to_message_becomes_the_owner`) | **RED** |
| 3 | `unpair()`: drop `self.ch.set_destination(None)` | `unpair_clears_the_channel_destination_and_lets_someone_new_pair` | RED |
| 4 | `send_pairing_confirmation`: `Err(Blocked)` arm returns `false` instead of `true` | `being_blocked_right_after_pairing_produces_bot_blocked_and_stops` | RED |
| 5 | `poll_forever`: drop `attempt = 0` on the `Ok` arm | `a_retryable_error_backs_off_and_keeps_polling` | RED |
| 6 | `terminal_reason`: `BadToken`/`Blocked` map to `PhoneBrokenReason::Unreachable` instead of `BadToken` | `bad_token_and_blocked_both_mean_bad_token_here_no_chat_context` (+ `a_pairing_message_flips_phone_status_to_paired_and_sends_a_confirmation`, `a_retryable_error_backs_off_and_keeps_polling`) | RED |
| 7 | `backoff_for`: `attempt.saturating_sub(1)` → `attempt` (off-by-one shift) | `backoff_starts_at_one_second_and_doubles` (+ `a_retryable_error_backs_off_and_keeps_polling`) | RED |
| 8 | `run()`: drop the `catch_unwind` wrapper entirely | `a_panic_inside_the_loop_never_escapes_run` | RED (the test itself panics — exactly the design: no `catch_unwind` around a real panic means the panic escapes the test's own call, and `#[test]`'s default panic-catching turns that into a normal failure, not a crashed binary) |

`daemon.rs`, the bridge-lifecycle wiring added/fixed this task (all new tests, all exercising code paths that had zero prior coverage):

| # | Mutation | Test | Result |
|---|---|---|---|
| 9 | `PhoneUnpair`: drop the `bridge.unpair()` call | `phone_unpair_tells_a_present_bridge_to_forget_its_destination` | RED |
| 10 | `PhoneUnpair`: `let had_confirmed_token = ph.state.has_confirmed_token();` → `let had_confirmed_token = true;` | `phone_unpair_leaves_a_present_bridge_alone_when_there_was_no_confirmed_token` (+ `phone_unpair_on_a_bad_token_is_a_no_op`, `phone_unpair_on_off_stays_off`) | RED |
| 11 | `PhoneUnpair`: `restart_needed = matches!(...)` → `restart_needed = false` | `phone_unpair_from_bot_blocked_restarts_polling_on_the_same_bridge` | RED |
| 12 | `PhoneDisable`: drop the `bridge.retire()` call | `phone_disable_retires_a_present_bridge_and_clears_the_slot` | RED |

All 12 mutations were applied by hand, confirmed RED, then reverted with `git checkout -- <file>` (safe because real work was committed first, per the standing instruction). Mutations #1 and #2 are the ones the brief calls "the one test in this feature that maps directly onto someone else getting to type on your machine" — both fail exactly the stranger-rejection and pairing-once tests they're meant to.

## Test commands and output tails

```
$ cargo test --lib bridge:: -- --test-threads=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 797 filtered out; finished in 0.00s

$ cargo test --lib daemon:: -- --test-threads=1
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 783 filtered out; finished in 5.50s

$ cargo test --lib -- --test-threads=1
test result: ok. 813 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 29.85s

$ cargo fmt -- --check
(no output, exit 0)

$ cargo clippy --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.33s
(no warnings)

$ git diff --check
(no output, exit 0)
```

`daemon::tests::create_does_not_hold_the_secrets_lock_across_the_slow_work` (the 8000-file git-checkpoint test) is a pre-existing, legitimately slow test and passes within the 5.5s `daemon::` total above — it is not related to this task's changes. Its presence is what made naive `cargo test --lib daemon::` runs take longer than a quick glance suggests; the actual deadlock (before the fix) took over 7 minutes and was distinguishable from "just slow" by `ps` showing the linked test binary running rather than `rustc` compiling.

## Concerns / things a future task should know

- **No network-integration test exists for the three real `Bridge::new(Arc::new(Telegram::new(&token)))` construction sites in `daemon.rs`** (startup, `PhoneSetToken` success, and the BotBlocked-restart path's reuse of an existing `Bridge`). This is deliberate, matching the project's existing line for real-transport code (`send_real`, `verify.rs::send_probe`, `phone_verify_token`'s injected `get_me` closure) — but it does mean the *decision logic* around starting/stopping a Bridge is tested via a `RecordingChannel` fake (see below), while the literal "does `Telegram::new` get called with the right token and does the thread actually run against the real Telegram API" is unverified at the unit level, same as every other real-transport call site in this codebase.
- I added a small `RecordingChannel` fake inside `daemon.rs`'s own test module (deliberately not sharing `bridge.rs`'s private `FakeChannel`, to keep the two modules' test concerns separate) plus a `#[cfg(test)] pub(crate) fn is_retired_for_test()` accessor on `Bridge`, mirroring the existing `i18n::has_han` pattern for test-only crate-visible hooks.
- Bridge's forwarding of owner-authored messages into a session (`Accepted::FromOwner` today does nothing but return) is explicitly Task 7's job per the module's own comments — I did not touch it.
