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

---

# Fix round 1

**Status:** complete. Commits on `feat/phone-channel`:
- `e6d378d` — `fix: pairing survives a restart instead of reopening backward in time` (both Criticals, all three Importants, the Minors)
- `2734b9b` — `test: prove poll_forever actually calls discard_backlog, not just that discard_backlog works` (a regression test the mutation sweep below showed was missing)

Test summary: `cargo test --lib -- --test-threads=1` → 838 passed, 0 failed, 29.92s. `cargo fmt -- --check`, `cargo clippy --all-targets`, `git diff --check` all clean.

## Critical 1 — pairing must survive a restart, not reopen a window backward in time

Two complementary fixes, as required:

1. **Persist + restore the owner.** `secrets::PHONE_OWNER_KEY` is a new key alongside the token and bot name. `Bridge` gained an `on_owner_changed: Box<dyn Fn(Option<i64>) + Send + Sync>` field, called from `accept()`'s pairing branch (`Some(new_owner)`) and from `unpair()` (`None`) — the same discipline as `set_destination`: one place decides, everything else is told. `daemon.rs::persist_owner_hook(secrets)` builds the production closure; `run_with_manager` reads `PHONE_OWNER_KEY` at startup and passes it to the new `Bridge::new_with_owner(ch, owner, on_owner_changed)`, which calls `set_destination` immediately if `owner` is `Some` — a restart resumes the same pairing, it doesn't reopen one. `initial_phone_status` and `apply_phone_set_token` both got matching updates: a saved owner now means the startup status is `Paired` (not `WaitingForPairing`), and a successful new-token verification clears any stale owner left over from a previous token (three-key gated write, same "two `lock()` calls must be separate statements" discipline as the existing token/bot-name write).

2. **Discard the backlog.** `channel::telegram::Telegram::with_transport` starts `offset` at 0, and Telegram's `getUpdates` with `offset=0` returns the whole unconfirmed backlog — up to ~24h, oldest first. A restart with a restored owner is safe from *pairing* being stolen (strangers are still rejected), but a never-paired restart (owner still `None`) is not: the oldest unclaimed backlog message would win pairing, and it doesn't have to be from someone currently online. `bridge.rs::discard_backlog()` runs before the first real long poll: it calls `ch.poll(Duration::ZERO)` repeatedly — Telegram returns immediately if there's anything queued, regardless of the requested timeout — discarding every batch until an empty one signals "caught up," never calling `accept()` on any of it. Reuses `terminal_reason`/`backoff_for` for its own error handling, so there's one error-mapping table, not two.

I verified reason 2 is load-bearing with its own end-to-end test (`poll_forever_discards_the_backlog_before_the_real_owner_can_pair`) rather than trusting the `discard_backlog`-level unit tests, which call the function directly and can't see whether `poll_forever` still calls it — see the mutation table.

**On the framing point requested:** my Task 5 report characterized "no startup re-verification needed" (carried-forward item 4) as pure gain — the first real `poll()` after a restart surfaces a revoked token within one cycle instead of leaving the page lying. That conclusion still holds, but it's the same `poll()` that Critical 1 is about: an unconditional first poll is exactly the mechanism that would have handed pairing to a stranger from the backlog if nothing else changed. Item 4's benefit and Critical 1's danger come from the identical code path — discovering a bad token fast and discovering a stranger's stale message fast are two faces of "the first poll after restart is unconditionally trusted." `discard_backlog` is what makes trusting it safe.

## Critical 2 — every write to `phone` from the background thread rechecks `is_retired()`, in the same critical section as the write

The bug: `poll_forever`'s `Paired` write and `send_pairing_confirmation`'s `Broken{BotBlocked}` write both acquired the `phone` lock and wrote unconditionally, with no recheck of retirement after the lock was acquired. A user pressing `x` (`PhoneDisable`) mid-race — either while a poll thread is blocked waiting for the `phone` lock `PhoneDisable` currently holds, or during `send_pairing_confirmation`'s up-to-10-second network call — could see their `Off` overwritten right back to `Paired` or `Broken{BotBlocked}`.

Fix, applied uniformly: every write to `phone` from the background thread now goes through one of four small functions — `record_pairing`, `record_bot_blocked`, `record_terminal_error` (also used by `discard_backlog`'s error path), `record_bridge_panicked` (new, see Important 3) — each of which acquires the `phone` lock, rechecks `bridge.is_retired()`, and only writes if still false. This is a general fix, not two point patches: "every write to `phone`" is enforced by giving every write site the same shape, not by asking a mutator to remember to add a check.

On the `PhoneDisable` side: `retire()` is now called **before** `PhoneDisable` ever touches the `phone` lock (previously: lock `phone`, write `Off`, then lock `bridge_slot` and retire, all while still holding the `phone` guard). I considered the reviewer's literal wording — "drop the guard before retiring" — against a race analysis: dropping the guard *before* calling `retire()` (rather than never holding it during `retire()` at all) still leaves a real window between the unlock and the `retire()` call, where a poll thread could acquire the lock, recheck `is_retired()`, see `false` (retire hasn't run yet), and write anyway. Calling `retire()` strictly before `PhoneDisable` acquires the `phone` lock at all closes that window: any writer that later acquires the lock is guaranteed `retire()` already ran, because `PhoneDisable`'s own `phone`-write happens-after its `retire()` call on the same thread, and any contending writer either got the lock before `PhoneDisable` (in which case its write is legitimately superseded by `Off`) or after (in which case `retire()` already happened). This also incidentally satisfies the Minor about `bridge_slot`/`phone` never being held nested — the two locks are now acquired fully sequentially in `PhoneDisable`, same as `PhoneUnpair` already did.

**Known gap, stated plainly:** I did not write an automated test that catches a regression of this *ordering* (retire-before-phone vs. the original phone-then-retire). The existing `phone_disable_retires_a_present_bridge_and_clears_the_slot` test runs single-threaded and confirms retirement happens, but not *when* relative to the `phone` write — and I judged that forcing a real concurrent race into a test would trade a clear bug for a flaky one (this codebase's own house style, e.g. `session.rs`'s comment on `recv_timeout`, explicitly rejects tests that pass by chance under load). The ordering is enforced by code structure and documented at the call site; a reviewer reading `daemon.rs::Request::PhoneDisable` can verify it, but a mutation that reintroduces the old order would not be caught automatically. I mutated the four `record_*` recheck sites (see table) — those *are* caught, deterministically, because "retire before calling" is a clean substitute for the race with no timing dependency.

## Important 1 — `start_phone_bridge` retires whatever was in the slot before installing a new one

Unless it's the *same* `Bridge` (`Arc::ptr_eq`) being reinstalled — the `PhoneUnpair`-from-`BotBlocked` restart path passes the same `Arc<Bridge>` back in, and an unconditional retire would have the new thread see itself as already retired and exit immediately, undoing its own restart. This is now a single fix at the one place a `Bridge` gets installed into the slot, so it covers all three call sites (startup, `PhoneSetToken`, `PhoneUnpair`'s restart) without needing three separate patches.

## Important 2 — `PhoneStatus.owner` gets the chat id (minimum truthful surface)

`record_pairing` now sets `ph.owner = Some(id.to_string())`. `ui/phone.rs::status_line`/`msg::phone_paired` already existed and were already tested for both `Some`/`None` owner — this was a dead producer waiting for a caller, not new UI work. I did not implement a real display name (`parse_updates` reading `from.first_name`/`username`, `Incoming` growing a field) — review explicitly offered the chat-id-only version as acceptable ("at minimum"), and the fuller version is a larger, separable change touching the `Channel` trait's data shape across all four implementors.

## Important 3 — a panicking poll thread now leaves an honest `Broken`, not a silently dead listener

`run()` calls `record_bridge_panicked` after `catch_unwind` reports an error — a new i18n string (`msg::phone_bridge_panicked`, tested in both languages) composes into `Broken{Unreachable, ...}`, subject to the same `is_retired()` recheck as every other write. `Unreachable` was chosen because it's the one `PhoneBrokenReason` that doesn't assert something false about the panic's actual cause (a panic isn't a `ChannelError`, so "the token is bad" or "this chat blocked the bot" would both be inventions).

## Minors

- The third `if let Some(bridge) = recover(bridge_slot.lock())...` in `PhoneDisable` is now also part of a strictly-sequential (never nested) lock pattern, addressed as part of the Critical 2 fix above, not as a separate patch.
- `PhoneBridgeSlot`'s doc comment now states the "`phone`/`bridge_slot` never held nested" discipline explicitly.
- `msg::phone_pairing_confirmation` has its own composition test (`phone_pairing_confirmation_composes_in_both_languages`), matching `phone_blocked`'s existing one.
- `BRIDGE_LANG` hardcoded to `Lang::Zh` (an English user gets a Chinese phone message and a Chinese panic-recovery message) is unchanged — it was already documented as a deliberate, known gap in the module's header, and I extended that same paragraph to say so explicitly rather than fix it; changing it means deciding where the background thread learns the user's chosen language from, which is a design question beyond this round.
- `secrets.rs`'s `PHONE_BOT_KEY` doc comment, which argued persisting the bot name means "startup never touches the network," is corrected: startup now always touches the network (the Bridge's poll thread) whenever a token is on disk; what persisting the bot name and owner still avoids is a *specific synchronous* `getMe` call blocking startup.
- The four daemon tests that call `handle()` with a populated `bridge_slot` now go through a new `handle_with_deadline` helper (spawns `handle()` on its own thread, `mpsc::Receiver::recv_timeout(Duration::from_secs(5))`) instead of calling `handle()` directly. I verified this actually converts a hang into a fast failure by deliberately reintroducing the Task 5 deadlock (reverting the `let bridge = recover(bridge_slot.lock()).clone(); if let Some(bridge) = bridge` rebinding back to the `if let Some(bridge) = recover(bridge_slot.lock()).clone()` form) and running just `phone_unpair_from_bot_blocked_restarts_polling_on_the_same_bridge`: it failed in 5.01s with `handle() 没有在 5 秒内返回` instead of hanging.

## An additional finding, out of scope for this round

Two pre-existing UI-layer integration tests — `ui::mod::tests::fetch_phone_status_reaches_the_real_daemon_when_connected` and `ui::settings_view::tests::entering_the_phone_item_reaches_the_real_daemon_not_a_hardcoded_default` — pre-seed `PHONE_TOKEN_KEY` with a fake token and then call `start_daemon_at`, which runs the real `daemon::run` in-process. Since Task 5's original commit, that now means a real `Bridge` starts and polls `https://api.telegram.org` with a garbage token on every `cargo test` run. I confirmed this is live, not theoretical: `curl -s -o /dev/null -w "%{http_code} in %{time_total}s\n" --max-time 5 https://api.telegram.org/` returns `302 in 0.93s` from this environment. The calls fail fast (401, terminal, not retried) so they don't cause flakiness or slow the suite today, but they violate the project's established "no unit test touches real network" discipline (the whole reason `initial_phone_status`/`apply_phone_set_token` take injected closures). My `discard_backlog` change makes this slightly worse — one more real HTTP round trip per affected test run — not better. I did not fix it: the clean fix is a `Channel`-construction seam in `run_with_manager` (mirroring `apply_phone_set_token`'s injected `get_me`), which touches `run_with_manager`'s signature and therefore every one of its call sites (`run()`, and the `tests/*.rs` integration tests) — a larger, separable change from this round's brief.

## Mutation table (fix round 1)

All mutations applied by hand, confirmed RED, then reverted; `cargo build --lib` confirmed clean (no leftover unused-variable warnings) after each revert.

| # | Mutation | Where | Test(s) that should catch it | Result |
|---|---|---|---|---|
| 1 | Remove `poll_forever`'s `if !discard_backlog(...) { return; }` call site entirely | `bridge.rs::poll_forever` | `poll_forever_discards_the_backlog_before_the_real_owner_can_pair` (the `discard_backlog_*` unit tests, which call the function directly, stayed green — this is *why* the test above exists) | **RED** |
| 2 | `record_pairing`: drop the `is_retired()` guard | `bridge.rs` | `record_pairing_does_nothing_once_retired` | **RED** |
| 3 | `record_bot_blocked`/`record_terminal_error`/`record_bridge_panicked`: replace `if !bridge.is_retired() {` with `if true {` (all three at once) | `bridge.rs` | `record_bot_blocked_does_nothing_once_retired`, `record_terminal_error_does_nothing_once_retired`, `record_bridge_panicked_does_nothing_once_retired` | **RED** (all three) |
| 4 | `start_phone_bridge`: drop the `Arc::ptr_eq` guard, always retire whatever was in the slot | `daemon.rs` | `start_phone_bridge_does_not_retire_the_same_bridge_being_reinstalled` | **RED** |
| 5 | `start_phone_bridge`: drop the retire-the-old-bridge block entirely | `daemon.rs` | `start_phone_bridge_retires_a_different_previous_bridge` | **RED** |
| 6 | `persist_owner_hook`: swap the `Some`/`None` arms (set on unpair, no-op on pair) | `daemon.rs` | `persist_owner_hook_sets_and_removes_the_owner_key` | **RED** |
| 7 | `initial_phone_status`: drop the `owner.is_some()` branch, always `WaitingForPairing` when there's a token | `daemon.rs` | `initial_phone_status_is_paired_when_an_owner_is_saved` | **RED** |
| 8 | `apply_phone_set_token`: drop the `.and_then(|_| ...remove(PHONE_OWNER_KEY))` step | `daemon.rs` | `apply_phone_set_token_clears_a_stale_owner_from_the_previous_token` | **RED** |
| 9 | Deadlock regression: revert the `bridge_slot` rebinding fix from Task 5 back to `if let Some(bridge) = recover(bridge_slot.lock()).clone() { ... start_phone_bridge(...) }` | `daemon.rs::Request::PhoneUnpair` | `phone_unpair_from_bot_blocked_restarts_polling_on_the_same_bridge` (via the new `handle_with_deadline` wrapper) | **RED in 5.01s** (previously: hang) |

Not mutated (see "Known gap" under Critical 2 above): the ordering of `PhoneDisable`'s `retire()` call relative to its `phone`-lock acquisition. No automated test distinguishes the fixed order from the original buggy one without introducing real thread timing.

## Exact test commands and output tails

```
$ cargo test --lib bridge:: -- --test-threads=1
test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 804 filtered out; finished in 0.00s

$ cargo test --lib daemon:: -- --test-threads=1
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 802 filtered out; finished in 5.6[4-7]s

$ cargo test --lib -- --test-threads=1
test result: ok. 838 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 29.92s

$ cargo fmt -- --check
(no output, exit 0)

$ cargo clippy --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.5xs
(no warnings)

$ git diff --check
(no output, exit 0)

# Deadlock-regression proof (mutation #9 above, reverted after):
$ cargo test --lib phone_unpair_from_bot_blocked_restarts_polling_on_the_same_bridge -- --test-threads=1
thread '...' panicked at src/daemon.rs:842:23:
handle() 没有在 5 秒内返回——大概率是 bridge_slot 相关的一个死锁回归了，见 Request::PhoneUnpair 那条锁死注释
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 836 filtered out; finished in 5.01s
```

---

# Fix round 2 (final four items)

**Status:** complete. Four commits on `feat/phone-channel`, one per item:
- `ec1210c` — `test: cover PhoneDisable's owner-key removal from secrets.toml`
- `87a86d1` — `fix: parse the saved owner id once, shared by status and Bridge`
- `3691d42` — `test: pin PhoneDisable's retire-before-phone-lock ordering`
- `b1f4ea5` — `fix: rebind the third bridge_slot if-let in PhoneDisable`

Test summary: `cargo test --lib -- --test-threads=1` → 844 passed, 0 failed, 29.69s (up from the base 842: two genuinely new tests, `initial_phone_status_treats_an_unparseable_owner_as_no_owner` and `phone_disable_retires_before_it_ever_touches_the_phone_lock`; `phone_disable_deletes_the_token_and_resets_to_off` and `phone_disable_retires_a_present_bridge_and_clears_the_slot` gained assertions but are the same tests). `cargo fmt -- --check`, `cargo clippy --all-targets`, `git diff --check` all clean after every item.

## Item 1 — `PhoneDisable`'s owner removal now has coverage

Extended `phone_disable_deletes_the_token_and_resets_to_off` (`src/daemon.rs`) to seed `PHONE_OWNER_KEY` on disk and assert it's gone afterward, with a message naming the actual harm ("a privacy leftover after an explicit off"). Confirmed by mutation: removing the `.and_then(|_| recover(secrets.lock()).remove(PHONE_OWNER_KEY))` step made the test fail red; reverted with `Edit` (not `git checkout --`, per the standing instruction — item 1's real change was committed first).

## Item 2 — owner key parsed once, shared by status and `Bridge`

`initial_phone_status` and `run_with_manager` read `PHONE_OWNER_KEY` with two different rules: the former only checked whether the raw string existed, the latter required `.parse::<i64>()` to succeed. Added `parse_saved_owner(sec: &SecretStore) -> Option<i64>` and made both call sites use it — `initial_phone_status` now derives its `Paired`/`WaitingForPairing` decision and its `owner: Option<String>` field from the same parsed value `run_with_manager` feeds `Bridge::new_with_owner`. A garbled value (only reachable via a hand-edited or corrupted `secrets.toml`) now reports `WaitingForPairing` consistently instead of a `Paired` that lies about the real `Bridge`'s open pairing window.

New test `initial_phone_status_treats_an_unparseable_owner_as_no_owner` seeds `PHONE_OWNER_KEY = "not-a-chat-id"` and asserts `WaitingForPairing`/`owner: None`. Confirmed by mutation: reverting `initial_phone_status`'s owner line back to the old raw-string check (`sec.get(PHONE_OWNER_KEY).map(str::to_string)`, `owner.is_some()`) made it fail red (`Paired` vs expected `WaitingForPairing`); reverted with `Edit`.

One implementation snag worth recording: writing `parse_saved_owner(&recover(secrets.lock()))` inline in `run_with_manager` doesn't compile — Rust infers `recover`'s generic `T` from the *expected* argument type of `parse_saved_owner` (`&SecretStore`), so it tries to unify `T = SecretStore` against `secrets.lock() : LockResult<MutexGuard<SecretStore>>` and fails with a confusing "expected `Result<SecretStore, ...>`, found `Result<MutexGuard<...>, ...>`" pointing at `recover`, not at the real mismatch. Fixed by binding the guard to a named `sec` variable first, matching the codebase's existing "one `lock()` per statement" discipline.

## Item 3 — deterministic pin for the retire-before-lock ordering

A previous implementer judged no non-flaky test was possible and left the ordering enforced only by code structure and comment. Added `phone_disable_retires_before_it_ever_touches_the_phone_lock`: the test thread locks `phone` and holds the guard, spawns `handle(Request::PhoneDisable, ...)` on another thread, then bounded-waits (2s) for `bridge.is_retired_for_test()` **while still holding the guard**, then drops it and joins.

Verified both directions by hand:
- With the real (fixed) ordering: passes immediately, every run.
- Temporarily swapped `PhoneDisable`'s two steps back to the pre-Critical-2 shape (lock `phone`, write `Off`, *then* lock `bridge_slot` and retire, all under the same `phone` guard): the test failed deterministically in ~2.02s (`retire() 必须在 PhoneDisable 碰 phone 锁之前发生...`), because `handle()`'s thread blocks on `phone.lock()` (held by the test thread) before it ever reaches `retire()`. Not a timing coincidence — the old order makes the wait's success structurally impossible within the budget, and the new order makes it structurally certain. Reverted the mutation with `Edit`, re-ran `phone_disable::` tests green.

Fail-closed as specified: the only failure mode under load is a false red (the 2s budget is generous relative to the pure in-memory work involved), never a false green.

## Item 4 — third `bridge_slot` if-let rebound

`PhoneDisable`'s `if let Some(bridge) = recover(bridge_slot.lock()).take() { bridge.retire(); }` was the one site of three left in the fragile shape (`if let` scrutinee's `MutexGuard` lives across the whole block). Rebound it the same way as the other two (`run_with_manager`'s startup `if let Some(token) = ...`, `PhoneUnpair`'s `if let Some(bridge) = ...`): lock-and-take into a named `bridge` variable first, then `if let Some(bridge) = bridge`. Added a comment explaining this site was safe only because its body happens not to re-lock — reasoned around, not structurally prevented — matching the reasoning already given at the other two sites.

No new test for this item: it's a refactor of a pattern already covered end-to-end by `phone_disable_retires_a_present_bridge_and_clears_the_slot` and the new ordering test in item 3, both of which exercise this exact `if let` with a populated `bridge_slot` and would hang (caught by `handle_with_deadline`'s 5s deadline or item 3's own bounded wait) if the rebind were wrong.

## Concerns

- None new. The four items were independently scoped and don't interact with each other's code paths beyond both touching `Request::PhoneDisable`'s body (items 1, 3, 4 all touch it; verified the full `daemon::` and `bridge::` suites stay green after each commit, not just at the end).

---

# Fix round 3 (two Minors, final for this task)

**Status:** complete. Two commits on `feat/phone-channel`, one per item (plus a stale-comment fix bundled into item 1's commit, same file, adjacent code):
- `635f002` — `fix: stop discard_backlog from busy-spinning when the cursor stalls`
- `b114eb6` — `fix: gate accept()'s set_destination the same as the disk write`

Test summary: `cargo test --lib -- --test-threads=1` → 846 passed, 0 failed, 30.32s (up from 844: two new tests, one per item). `cargo fmt -- --check`, `cargo clippy --all-targets`, `git diff --check` all clean after both commits.

## On the ordering pin (item 3, previous round) — a property I built in but didn't report

The reviewer traced the lock sequence and confirmed something my report didn't call out: when `phone_disable_retires_before_it_ever_touches_the_phone_lock`'s assertion fires (old ordering), `phone_guard` is dropped during unwind and `recover()` tolerates the resulting poisoning — so a failing run releases the spawned `handle()` thread instead of leaving it blocked forever on `phone.lock()`. That's why the failing run in my mutation check completed in ~2.02s rather than hanging the process. I hadn't verified this consciously; it falls out of `recover()`'s existing poison-tolerant design (used everywhere else in this file) combined with Rust's unwind-drops-locals semantics. Worth stating explicitly for the next reader: the test is fail-closed at two levels, not one — a false red from load (documented already) and a panicking assertion that doesn't itself leave the suite stuck.

## Item 1 — `discard_backlog` could busy-spin when the cursor stalls

`src/bridge.rs::discard_backlog` (Critical fix from an earlier round) terminates on `raw_len == 0`, not on `messages.is_empty()`, to defeat the sticker-flood attack. But it never checked that a non-empty batch actually advanced the polling cursor — if a batch comes back with `raw_len > 0` but no `update_id` anywhere in it (a malformed or proxy-mangled `ok:true` body; `src/channel/telegram.rs::max_update_id` returns `None`), `Telegram::poll`'s `offset` stays put. The next `poll(ZERO)` asks the identical offset, likely gets the identical batch back, and `raw_len` never drops to 0 — a tight, un-backed-off `getUpdates?timeout=0` loop against Telegram for as long as the daemon lives, breakable only by `retire()` (the user pressing `x`).

Fix: `channel::Batch` gained a `cursor_advanced: bool` field (`src/channel/mod.rs`), set by `Telegram::poll` to whether `max_update_id` found anything to advance past (`src/channel/telegram.rs`). `discard_backlog` now treats `raw_len > 0 && !cursor_advanced` as a terminal error — reusing `record_terminal_error`/`ChannelError::Malformed` (the same mapping already used for "came back but we can't parse it") rather than inventing a new error path — instead of resetting `attempt` and looping.

This is a real, if small, contract change: `Batch`'s two other construction sites (`FakeChannel`'s `batch`/`batch_with_raw_len` test helpers in `bridge.rs`) needed the new field filled in (`true` for both — real Telegram always carries `update_id` on every item, sticker or not, so any legitimate `raw_len > 0` batch does advance the cursor). Added a third helper, `batch_stuck(raw_len)`, with `cursor_advanced: false`, for the new test.

New test `discard_backlog_stops_instead_of_spinning_when_the_cursor_cannot_advance` scripts two stuck batches in a row and asserts exactly one `poll()` call happens (not two) and that a `Broken{Unreachable}` state is left behind — not just "it returns", since silently stopping would leave the page waiting on an update that will never come, the same Important 3 concern the panic-recovery path already addresses. Confirmed by mutation: removing the guard let `poll_calls` reach 3 instead of 1 (bounded only by `FakeChannel`'s own fail-closed fallback — the real production loop against real Telegram has no such bound); reverted with `Edit`.

Bundled into the same commit: corrected `FakeChannel`'s stale struct doc comment (`src/bridge.rs`), which still said the exhausted `poll_script` returns `Ok(空批次)` while the impl — and its own inline comment, two lines below — return a terminal `Err`. Pre-existing drift from `864b9c8`, unrelated to this fix but directly adjacent to it in the file.

## Item 2 — `accept()`'s retire gate was asymmetric

An earlier round gated `accept()`'s disk write (`on_owner_changed`) on `is_retired()` but left `self.ch.set_destination(Some(msg.chat_id))` unconditional. In the exact window the gate exists for — `PhoneDisable` retiring a `Bridge` while its poll thread is blocked inside a `poll()` call (up to 25s) — a stranger's message could still reach `accept()`, `set_destination` would still hand the channel their chat id, and `poll_forever`'s subsequent call to `send_pairing_confirmation` (which has no `is_retired()` check of its own — its doc comment's reasoning, "pairing is already a fact, don't undo it over a failed goodbye", doesn't apply here because pairing shouldn't have happened at all) would then actually send the "已配对" confirmation to that stranger. The persisted-owner harm was closed by the earlier fix; this adjacent, narrower harm (an attacker gets positive proof they won a pairing race, even though nothing survives a restart) was not.

Fix: `accept()` now computes `is_retired()` once into `already_retired` and gates both `set_destination` and `on_owner_changed` behind the same check — they can no longer disagree. This also means `send_pairing_confirmation` never gets a destination to send to in the retired case: `Channel::send` (both `Telegram`'s and `FakeChannel`'s) returns `Unreachable` when no destination has ever been set, which `send_pairing_confirmation` already treats as a no-op.

New test `accept_does_not_set_destination_once_retired`, sibling to the existing `accept_does_not_persist_the_owner_once_retired`. Confirmed by mutation: made `set_destination` unconditional again (restoring the exact pre-fix shape) while leaving `on_owner_changed` gated; the test failed red. Reverted with `Edit`.

## Not mine to fix (per the coordinator)

The missing mutation-testing record for the Critical (backlog raw-count) and both Importants (retire gate, test-network) from earlier rounds — the coordinator is addressing that in the ledger, not this report.

## Concerns

- None new. Both items are independently scoped; the full `bridge::`, `channel::`, and full `--lib` suites stayed green after each commit, not just at the end.
