# Task 6 report: outbound — tick pushes events, three gates, debounce

**Status:** complete. Two commits on `feat/phone-channel`:
- `1d8c38d` — `feat: tick() pushes an event when an agent stops, fails or dies` (`src/session.rs`: `should_notify`, `SessionManager::set_event_sink`/`clock_origin`, `Session::last_notified`, the three `tick()` call sites, all tests)
- `feaabe5` — `feat: consume the outbound event queue and forward it to the channel` (`src/bridge.rs`: `describe_event`, `consume_events`, `run_events`, tests)

## Two things the brief could not have known, both confirmed and handled

**1. The naming gate.** Re-read the current `tick()` before touching it, as instructed — `Session::name_attempted` (not `name_slot.is_none()`) is indeed the "only ask once" gate today. `should_notify`'s three conditions don't touch it at all: naming and notifying are two independent consumers of the same `was == Working && next ∈ {Idle, Asking}` transition, so I split the existing single `if` into `let just_stopped = …` plus two separate `if` blocks — naming keeps its four original conditions (including `!s.name_attempted`, which must never gate notification), notification only needs `just_stopped` plus `should_notify`'s own three gates evaluated per call, per session, every time.

**2. `debounce`'s clock.** `Session` gained `last_notified: Option<Duration>`; `SessionManager` gained `clock_origin: Instant` captured once in `new()`. `tick()` computes `let now = self.clock_origin.elapsed();` once per sweep (not once per session — same "now" for every session in one tick, so debounce measures real gaps between sweeps, not scan-order skew) and passes it down. `SystemTime::now()` never appears in this path.

## An interface gap in the brief, not worked around

The plan's Interfaces line says `Sessions::set_event_sink(mpsc::Sender<Event>)`. I checked before copying: `std::sync::mpsc::Sender` has no `try_send` on this toolchain (rustc 1.97.1) — confirmed by compiling a two-line probe, `error[E0599]: no method named try_send found for struct std::sync::mpsc::Sender<T>`. `Sender` backs an unbounded queue; `send()` only fails when the receiver is gone, never because the queue is full, because it can't be full. That directly contradicts the task's own hard constraint ("投递用 `try_send` 语义，队列满了就丢，绝不阻塞 tick"). `std::sync::mpsc::SyncSender` is the type that actually has bounded capacity and `try_send`, so `set_event_sink` takes `Option<mpsc::SyncSender<Event>>`, documented at the call site with the reasoning above. `Option` (rather than a bare sender) mirrors this same file's existing `set_backend`/`set_llm_problem` convention for optional, swappable dependencies.

## What `tick()` does now, at the three call sites

1. **Vanished** (the reap branch, `!s.pty.is_alive()`): `maybe_notify(&mut s, EventKind::Vanished, now)` right next to the existing `journal.died(..., Vanished, ...)` call.
2. **Failed** (`next == Failed && was != Failed`): `maybe_notify(&mut s, EventKind::Failed, now)` alongside the existing `request_explanation` call — same one-shot-per-entry condition, no new logic.
3. **Stopped** (`just_stopped = was == Working && matches!(next, Idle | Asking)`): `maybe_notify(&mut s, EventKind::Stopped, now)`, split out from the naming `if` as described above.

`maybe_notify` (new private method on `SessionManager`): clones the `Option<SyncSender<Event>>` out from behind one lock (so `should_notify`'s third gate — `has_channel` — is a plain `bool`, not a held lock), checks the three gates, checks `debounce`, records `last_notified = Some(now)` *before* attempting the send (debounce tracks "was this session judged notify-worthy recently", not "did the send actually land" — a momentarily full queue is already a degraded mode and shouldn't be compounded by immediate retries, which is exactly what debounce exists to prevent), then `try_send`s an `Event` built from already-user-facing fields: `name` falls back to `s.profile.name` when `name_slot` is still empty (the exact same convention `SessionInfo.tag` already uses for the UI — this matters because the very first `Stopped` event fires in the same tick that kicks off auto-naming, so `name_slot` is very often still empty at that instant), `project` is the session directory's file name (there's no separate "project" concept anywhere in this codebase — `projects.rs::Store` only tracks paths, not display names — so the directory basename is the closest already-available approximation; flagging this as a judgment call, not a spec-mandated value).

## `bridge.rs`'s consumer — deliberately minimal, and why

The brief's file list only says "Modify: `src/bridge.rs`（消费队列）" with no test list and no message-format spec. The design spec explicitly assigns message-content tiering and merging to Task 9 (`merge(&[Event], Lang) -> String`, not yet written) and Task 11 (`Bridge::enqueue`/`QUEUE_CAP`, a *different* bounded-queue design living inside `Bridge` itself with drop-oldest semantics, contradicting nothing here since it supersedes this task's `mpsc`-based queue later). So I did not invent a smart consumer: `describe_event` renders one `Event` to one line using only its own already-user-facing fields (no screen, no model call — satisfies the "没配 `[llm]`" privacy tier by construction, not by restraint), `consume_events` blocks on `rx.recv()` and forwards each line via `bridge.ch.send`, and `run_events` wraps it in `catch_unwind` exactly like the existing `run()` does for the polling thread, with the same "a dead phone channel is a disappointment, a dead session is a disaster" justification from the module header. A single send failure is not retried and does not touch the `phone` status slot — that's `poll_forever`'s job today (backoff, `Broken` transitions), and giving the outbound path a second, independent copy of that decision would just create two disagreeing definitions of "when to back off." Meaningful outbound backoff/queueing is the later task's job per the plan.

## Known gap, stated plainly, not fixed

**`daemon.rs` is not wired.** `SessionManager::set_event_sink` and `bridge::run_events` both exist and are tested in isolation, but nothing in `daemon.rs` calls `set_event_sink` or spawns a thread running `run_events` yet — the brief's file list names only `src/session.rs` and `src/bridge.rs`, and no task in the plan (I checked Tasks 7–11) assigns this wiring to anyone. Concretely, this means the outbound half does not fire in the running daemon today, even though every unit underneath it is real and tested. This mirrors Task 5's own report noting `Bridge`'s inbound forwarding was "explicitly Task 7's job." I'm flagging it rather than guessing at the wiring myself: `Bridge` is constructed at three separate call sites in `daemon.rs` with `bridge_slot`/`PhoneDisable`/`PhoneUnpair` lifecycle rules that Task 5's fix rounds spent considerable effort getting race-free (Critical 2's "every write to `phone` rechecks `is_retired()` in the same critical section as the write"), and wiring a second thread's lifecycle into that without a scripted brief risks reintroducing exactly that class of bug.

**A panicking `run_events` thread has no visible `PhoneState`.** Documented in the function's own doc comment: there's no `PhoneState` variant for "pairing and inbound listening are both fine, only outbound notification died." Inventing one wasn't asked for and touches `proto.rs`/`ui/phone.rs` beyond this task's file list — noting it for whoever wires the thread up.

## Mutation table

All mutations applied by hand with `Edit`, confirmed RED, then reverted with `Edit` (never `git checkout --`) before the next mutation.

| # | Mutation | File | Test(s) that should catch it | Result |
|---|---|---|---|---|
| 1 | `should_notify`: drop `!first_input_empty` | `session.rs` | `a_brand_new_session_does_not_page_you`, `a_newly_created_session_pushes_no_event_on_its_first_tick` | **RED**, both |
| 2 | `should_notify`: `&&` → `||` (all three) | `session.rs` | `a_brand_new_session_does_not_page_you`, `no_channel_means_no_page`, `a_plain_shell_never_pages_you`, `a_newly_created_session_pushes_no_event_on_its_first_tick` | **RED**, 4 tests (brief asked for "at least two") |
| 3 | `should_notify`: drop `is_agent` | `session.rs` | `a_plain_shell_never_pages_you` | **RED** |
| 4 | `maybe_notify`: bypass the `debounce(...)` check | `session.rs` | *(new)* `a_second_stop_within_the_debounce_window_is_suppressed` | **RED** — no existing test caught this; added per the standing instruction ("if a named mutation does not turn a test red, add the test") |
| 5 | `tick()`'s Stopped call site: `EventKind::Stopped` → `EventKind::Failed` | `session.rs` | *(new)* `a_real_round_of_work_pushes_a_stopped_event` | **RED** — also added because no prior test asserted a real event's `kind` |
| 6 | `describe_event`: collapse `Failed`'s text to match `Stopped`'s | `bridge.rs` | `describe_event_tells_the_three_kinds_apart` | **RED** |
| 7 | `consume_events`: drain the receiver without calling `ch.send` | `bridge.rs` | `consume_events_sends_every_queued_event_in_order` | **RED** |
| 8 | `run_events`: drop the `catch_unwind` wrapper | `bridge.rs` | `a_panic_inside_the_consumer_never_escapes_run_events` | **RED** (the panic itself fails the test, same mechanism as the existing `run()` test) |

Mutations 4 and 5 are the two genuine gaps this task's own mutation sweep found — the brief's Step 5 only scripted mutations on `should_notify`'s pure logic (1 and 2 above), which don't exercise `tick()`'s actual wiring or the debounce integration. Both gaps are now closed with real tests rather than left as findings.

## Exact test commands and output tails

```
$ cargo test --lib session:: -- --test-threads=1
test result: ok. 80 passed; 0 failed; 0 ignored; 0 measured; 773 filtered out; finished in 21.0s

$ cargo test --lib bridge:: -- --test-threads=1
test result: ok. 44 passed; 0 failed; 0 ignored; 0 measured; 813 filtered out; finished in 0.01s

$ cargo test --lib -- --test-threads=1
test result: ok. 857 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in ~31s
```

(Baseline before this task, per Task 5's final report: 846 passed. This task added 11 tests net: 7 in `session.rs`, 4 in `bridge.rs` — matches 857.)

`cargo fmt -- --check`: clean. `cargo clippy --all-targets`: clean, no warnings. `git diff --check`: clean.

## New tests added, beyond the brief's four `should_notify` cases and the one tick-integration case

- `session.rs::a_real_round_of_work_pushes_a_stopped_event` — positive case: a real `Working → Idle` transition with real `first_input` actually produces a queued `Stopped` event with the right `session`/`kind`. The brief's own tests only prove "should not fire"; without a positive case, all three gates or the wiring itself could point the wrong way and every scripted test would still be green.
- `session.rs::a_second_stop_within_the_debounce_window_is_suppressed` — forces two real `Working → Idle` transitions well inside the 30s `DEBOUNCE_WINDOW` (not a timed/narrow-window test — it only needs the whole test to finish faster than 30 real seconds, which is not the flaky "land inside 0.2s" pattern the standing instructions warned against) and asserts the second is suppressed.
- `bridge.rs::describe_event_tells_the_three_kinds_apart`, `consume_events_sends_every_queued_event_in_order`, `consume_events_returns_quietly_when_the_sender_is_dropped_with_nothing_sent`, `a_panic_inside_the_consumer_never_escapes_run_events` — cover the new `bridge.rs` surface, which the brief's Files list touches but doesn't script tests for.

## Concerns for whoever picks up the next task

- `daemon.rs` wiring is the real blocker for this feature actually doing anything at runtime — see "Known gap" above.
- `describe_event`'s wording is a placeholder Task 9 is expected to replace wholesale (`merge`); I did not try to make it pretty or match the spec's illustrative example verbatim, since the spec's example implies an "agent name" field `Event` doesn't carry.
- `project` being the directory basename is a judgment call, not a spec value — worth a second look if Task 9's message copy ever surfaces it directly to the user.
