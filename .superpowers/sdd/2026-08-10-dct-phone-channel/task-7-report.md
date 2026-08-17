# Task 7 report — 入站路由五条规则

## What was implemented

In `src/bridge.rs`, added (before `struct Bridge`):

- `RouteInput<'a>` — `{ reply_to: Option<MsgId>, map: &'a HashMap<MsgId, u32>, used: Option<u32>, replied_since_use: bool, waiting: &'a [u32] }`. Pure data, no IO.
- `Route` enum — `To(u32)`, `Ask(Vec<u32>)`, `Gone`, `NeedUse`. `Debug + Clone + PartialEq + Eq` for test assertions.
- `pub fn route(i: &RouteInput) -> Route` — the five-rule decision function. Pure: no locks, no channel calls, no side effects.

`MsgId` and `HashMap` were newly imported (`crate::channel::MsgId`, `std::collections::HashMap`); `Incoming`/`Channel`/`ChannelError`/`Event` imports were already present.

Task 8 will build the `RouteInput` from the bridge's real message map / `/use` state / waiting-session set and act on the `Route` (type into PTY, send receipt, journal). This task only decides.

## Rule order and why each rule sits where it does

The five rules, fixed order, not to be rearranged:

1. **Reply to a specific pushed message → that message's session, never ask.** If `reply_to` is `Some(m)` and `m` is in `map`, return `To(session)` unconditionally — this check runs first and short-circuits everything else, including an active `/use` or multiple waiting sessions. A reply is the most explicit signal the user can give; nothing later should be allowed to override it.
   - If `m` is **not** in `map` (daemon restarted, mapping gone), the answer is `Route::Gone` — **type nothing**. This does not fall through to rule 2 or later; it returns immediately from the `match`. See "How Gone is guaranteed to type nothing" below.
2. **Explicit `/use` outranks "the one session that's waiting."** Checked only after rule 1 has been ruled out (`reply_to` was `None`, or was `Some` but rule 1 already returned). This must come before rule 3 (single-waiting) — otherwise a session that happens to be polling/waiting would steal the message from the session the user deliberately switched to.
3. **`/use` expires once the user has replied to a push.** This isn't a separate rule in the code — it's baked into rule 2's condition: `(i.used, i.replied_since_use)` matches `(Some(u), false)`, i.e. `/use` only fires while `replied_since_use` is `false`. Once the user has replied to any push, `replied_since_use` becomes `true` and rule 2's guard fails, falling through to rule 4/5. This is why the order "2 outranks 3" is really "one combined guard," and why inverting `replied_since_use` or reordering it independently breaks the semantics (see mutation 3).
4. **Exactly one session waiting → give it to that session.** Only reached once neither a reply nor a live `/use` applied.
5. **Several sessions waiting → `Ask`, never guess.** Typing into the wrong agent costs more than one extra question.
6. **Nothing waiting and no live `/use` → `NeedUse`**, telling the user (in Task 8/i18n) to check the session list.

## How `Gone` is guaranteed to type nothing

`Gone` is a `Route` variant with no associated data and is produced only inside the `match i.map.get(&m)` on the `None` arm of rule 1 — there is no code path that falls through from `Gone` to any later rule (it's a `return` inside the `if let Some(m) = i.reply_to` block). Task 8 (not written here) is responsible for the actual "don't type" behavior when it receives `Route::Gone`, but this task guarantees the *decision layer* never launders a stale/unknown message id into a `To(session)` — the only way to get `To` for a reply is if the id is present in the caller-supplied map. Tests `a_reply_to_a_message_we_no_longer_know_types_nothing` and the added `gone_wins_over_use_and_waiting_too` pin this: the latter proves `Gone` isn't just the default when there's nothing else to fall back to — it wins even when `/use` and multiple waiting sessions are simultaneously present, which is exactly the scenario an attacker/bug would need to route a stale reply into an active session.

## Mutations tried (all caught)

Performed by hand-editing `route()` in place, running the targeted test, confirming failure, then restoring from a saved copy (`src/bridge.rs.orig`, deleted afterward) and re-verifying full pass + `cargo fmt --check` + clippy clean.

1. **Swap rules 2 and 3** (moved the single-waiting check before the `/use` check): `an_explicit_use_beats_a_waiting_session` failed — expected `To(3)`, got `To(9)`. Caught as specified.
2. **`Route::Gone` → `Route::To(i.used.unwrap_or(0))`**: both `a_reply_to_a_message_we_no_longer_know_types_nothing` (expected `Gone`, got `To(3)`) and the added `gone_wins_over_use_and_waiting_too` (expected `Gone`, got `To(3)`) failed. Caught as specified, and by an extra test.
3. **Invert `replied_since_use`** (matched `(Some(u), true)` instead of `(Some(u), false)`): both `an_explicit_use_beats_a_waiting_session` (expected `To(3)`, got `To(9)`) and `use_expires_once_you_have_replied_to_a_push` (expected `To(9)`, got `To(3)`) failed. Caught as specified.

## Extra tests added beyond the brief's seven

The brief's seven tests only probe the three named mutations. Considered whether "rule ordering as a whole" is fully pinned — it wasn't quite, so added:

- `a_reply_wins_over_everything_else_even_with_use_and_many_waiting` — reply_to present, `/use` set, several sessions waiting: asserts rule 1 still wins. Guards against a reordering that moves rule 1 down (not just 2/3 swap).
- `gone_wins_over_use_and_waiting_too` — unknown reply_to, `/use` set, several waiting: asserts `Gone` still wins. Same rationale, for the `Gone` branch specifically (this is the case an attacker/race would actually hit: a stale reply arriving while other state looks "routable").
- `replied_since_use_does_not_affect_the_waiting_rules_when_there_is_no_use` — `replied_since_use = true` but `used = None`: confirms the flag only gates `/use`'s validity and has zero effect on the single/multiple-waiting rules when there's no `/use` to expire in the first place. Guards against a mutant that accidentally routes `replied_since_use` into rule 4/5's logic.

All ten `route()` tests plus the three extras (10 total: 7 from brief + 3 added) pass.

## Commands run and results

```
cargo test --lib bridge:: -- --test-threads=1     → 41 passed (bridge module total, incl. routing)
cargo fmt --check                                  → clean (after one `cargo fmt` run to fix a long single-line assert)
cargo clippy --all-targets                         → clean, no warnings
```

Full suite (`cargo test -- --test-threads=1`) was started in the background; per instructions I did not wait on it. Baseline before this task was 814 passing tests; this task adds 10 new tests (7 brief + 3 extra) to the bridge module, so the expectation is 824 passing with 0 failures. Whoever verifies should check the background log.

## Concerns

- None regarding correctness of `route()` itself — all three specified mutations were caught by the brief's own tests, plus the three extra tests I added close the "what if the whole order got shuffled" gap the brief called out as worth considering.
- `route()` is unused outside of tests as of this task (Task 8 wires it up) — clippy did not flag this as dead code since it's `pub`, consistent with the brief's scope ("produce the decision; do not act on it").
- Did not touch `Bridge`'s internal state (message map, `/use` state, waiting-session bookkeeping) — per the brief, those don't exist in `Bridge` yet; Task 8 owns wiring `RouteInput` construction from real state.
