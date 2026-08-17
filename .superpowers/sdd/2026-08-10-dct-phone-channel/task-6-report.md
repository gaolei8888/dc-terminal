# Task 6 report — 出站：tick 投事件、三道门、防抖

## What was implemented

- `should_notify(is_agent, first_input_empty, has_channel) -> bool` — free function
  in `src/session.rs`, right after `classify()`. Pure AND of the three gates.
- `SessionManager::set_event_sink(&self, tx: mpsc::Sender<Event>)` — the interface
  the brief asked for (the type is `SessionManager`, not `Sessions`; that's just
  what this codebase already calls it — same struct, same role).
- `SessionManager::event_tx: Mutex<Option<mpsc::Sender<Event>>>` and
  `SessionManager::started: Instant` (the debounce clock's epoch) — new fields.
- `Session::last_notified: Option<Duration>` — per-session debounce state, next to
  `scroll_mark` in the `Session` struct.
- `SessionManager::maybe_notify(&self, s: &mut Session, kind: EventKind)` — the
  single choke point all three call sites go through: checks `should_notify`,
  checks `debounce`, updates `last_notified`, builds the `Event` and sends it.
- `Bridge::outbound: Mutex<VecDeque<Event>>` plus `Bridge::enqueue`/`Bridge::queued`
  and `pub const QUEUE_CAP: usize = 32` in `src/bridge.rs`.

## Where each of the three events is posted in `tick()`

All three call `self.maybe_notify(&mut s, EventKind::…)`, which is where the actual
three-gate check + debounce + send lives — none of the three call sites duplicate
that logic.

1. **`Vanished`** — right after the existing "reap a dead pty" branch marks
   `s.state = Stopped` and calls `journal.died(..., Vanished, ...)`, before the
   `continue`. (`src/session.rs`, in `tick()`, the `if !s.pty.is_alive()` block.)
2. **`Failed`** — inside the existing `if next == Failed && was != Failed` block,
   right next to the existing `self.request_explanation(&mut s)` call. Same trigger
   condition already computed by the state machine — no new detection logic.
3. **`Stopped`** — inside the existing `if was == Working && matches!(next, Idle |
   Asking)` block. **Important:** I pulled the notify call *out* of the inner
   `if s.is_agent && !s.first_input.is_empty() && !s.name_attempted` guard that
   gates `request_name`. Naming has its own once-only gate (`name_attempted`) that
   must not also gate notifications — an agent finishing a *second* round of work
   is still worth a phone buzz even though it was already named after the first
   round. So the `if was == Working && …` block now does two independent things:
   conditionally call `request_name` (naming's own gate), then unconditionally
   call `maybe_notify` (which has its own three gates + debounce).

## Non-blocking hand-off

`SessionManager::event_tx` is a plain `std::sync::mpsc::Sender<Event>` —
**unbounded**. `Sender::send()` on an unbounded channel never blocks; it either
succeeds (pushes onto an ever-growing internal queue) or fails immediately if the
receiver has been dropped. `maybe_notify` does `let _ = tx.send(...)` — a failed
send (no consumer wired up, e.g. no phone configured) is silently ignored. There is
no `try_send`/timeout/lock-and-wait anywhere on this path; `tick()` cannot stall on
it under any circumstance.

## Where `QUEUE_CAP` lives, and drop-oldest

`pub const QUEUE_CAP: usize = 32` in `src/bridge.rs`, next to `POLL_TIMEOUT` etc.
`Bridge::enqueue(&self, e: Event)`:
```rust
pub fn enqueue(&self, e: Event) {
    let mut q = recover(self.outbound.lock());
    if q.len() >= QUEUE_CAP {
        q.pop_front();
    }
    q.push_back(e);
}
```
`pop_front()` on a `VecDeque` drops the oldest (front) element when the queue is
already at capacity, then the new event is pushed to the back — so the queue never
exceeds `QUEUE_CAP` and always keeps the most recent events. `Bridge::queued()`
returns a `Vec<Event>` snapshot (oldest-first) for tests/future consumers; it's
read-only, it doesn't drain.

**Scope note on wiring:** Task 6's file list is `session.rs` + `bridge.rs` only.
I did **not** touch `daemon.rs`, so there is currently no live thread pulling
events off the `mpsc::Receiver<Event>` (the other end of `set_event_sink`'s
sender) and feeding them into `Bridge::enqueue`. That plumbing — connecting
`SessionManager`'s sender to a `Bridge`'s receiver end and to `daemon.rs`'s setup
code — touches call sites of `Bridge::new`/`spawn`/`replace` across `daemon.rs`
and is naturally a later task's job (the brief's own file list agrees). What Task
6 delivers is: (a) `tick()` can now produce events onto an unbounded channel
without ever blocking, and (b) `Bridge` has a fully-tested bounded, drop-oldest
sink ready to receive them, satisfying Ruling 4's "build it so Task 11's test can
exist."

## Per-session debounce state

`Session::last_notified: Option<Duration>`, relative to `SessionManager::started:
Instant` (set once in `SessionManager::new()`). In `maybe_notify`:
```rust
let now = self.started.elapsed();
if !debounce(s.last_notified, now, DEBOUNCE_WINDOW) { return; }
s.last_notified = Some(now);
```
This reuses `channel::debounce`/`DEBOUNCE_WINDOW` verbatim, as instructed — no new
debounce logic.

## Tests added

In `src/session.rs`:
- `a_brand_new_session_does_not_page_you`, `a_plain_shell_never_pages_you`,
  `no_channel_means_no_page`, `an_agent_you_have_talked_to_pages_you` — the four
  unit tests from the brief, verbatim.
- `a_brand_new_session_does_not_wake_your_phone` — full-tick integration test:
  registers a fake profile with only `busy_pattern` set (the real-profile shape),
  `create()`s a session, ticks it 5 times, asserts the event channel's `rx` never
  yields anything.
- `an_agent_that_finishes_a_real_turn_wakes_your_phone` — the positive
  counterpart: sends real input, waits for a genuine Working→Idle transition, and
  asserts a `Stopped` event for the right session id actually arrives. Exists so
  that "delete the whole gate, always return true" can't sail through by making
  the negative test the only one that matters.

In `src/bridge.rs`:
- `enqueue_keeps_everything_under_the_cap` — sanity check, no drops below cap.
- `enqueue_drops_the_oldest_when_the_queue_is_full` — fills to `QUEUE_CAP`, adds
  one more, asserts the front (oldest) is gone and the newest survived. This is
  the seed for what Task 11 will exercise further.

## Mutation testing (both prescribed, both caught)

1. **Delete `!first_input_empty`** (`should_notify` becomes `is_agent &&
   has_channel`):
   ```
   test session::tests::a_brand_new_session_does_not_page_you ... FAILED
   test session::tests::a_brand_new_session_does_not_wake_your_phone ... FAILED
   ```
   Both required tests failed, as specified.

2. **Change all three `&&` to `||`** (`should_notify` becomes `is_agent ||
   !first_input_empty || has_channel`):
   ```
   test result: FAILED. 758 passed; 16 failed; 0 ignored; 0 measured
   ```
   16 tests failed (well over the required "at least two"), including both
   `should_notify` unit tests above, `a_plain_shell_never_pages_you`,
   `no_channel_means_no_page`, and — because the mutated gate is looser than the
   real one — several *unrelated* naming tests also broke (e.g.
   `a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen`),
   which is expected collateral: those tests share the same `tick()` code path
   and this particular mutation makes `should_notify` degenerate to "almost always
   true," which happens to interact with test setup that also exercises
   `maybe_notify`/backend calls in ways that surface as failures elsewhere too.

Both mutations were reverted afterward; `diff` against the pre-mutation file
confirmed the restored file is byte-identical to the correct version.

## Commands run and results

```
cargo build --lib                         → clean
cargo test --lib session:: -- --test-threads=1     → 79 passed, 0 failed
cargo test --lib bridge -- --test-threads=1        → 30 passed, 0 failed
cargo test -- --test-threads=1                     → exit code 0, all suites ok
cargo test --lib -- --test-threads=1               → 774 passed, 0 failed
cargo fmt --check                                  → no output (clean)
cargo clippy --all-targets                         → clean, no warnings
```

Baseline was 804 tests passing at 60670e7; this task adds 8 new tests (4
`should_notify` unit tests + 2 full-`tick()` integration tests in `session.rs`,
2 queue tests in `bridge.rs`), all passing, no regressions, no reduction in the
baseline count.

## Concerns

- **`QUEUE_CAP = 32`** is a guessed number (documented as such in the code
  comment), same status as `DEBOUNCE_WINDOW` — not validated against real usage
  patterns yet.
- **The mpsc receiver → `Bridge::enqueue` wiring does not exist yet.** Right now
  `tick()` can produce events and `Bridge` can hold them, but nothing in this
  branch connects the two at runtime (no thread reads from the `Receiver<Event>`
  that pairs with the `Sender` handed to `set_event_sink`). This is consistent
  with the brief's file scope (`session.rs` + `bridge.rs` only, no `daemon.rs`),
  but it means Task 6 alone does not yet make phone notifications actually fire —
  it only makes the two halves individually correct and testable. Flagging this
  explicitly so the next task doesn't assume the wiring already exists.
- The `project` field on `Event` is derived as `dir.file_name()` (falling back to
  the full path display if that's `None`, e.g. `dir == "/"`), since no existing
  convention for "project display name" was found elsewhere in the codebase to
  reuse. `name` is `name_slot.clone().unwrap_or_default()` (empty string if the
  session hasn't been auto-named yet) — same pattern `list()`/`SessionInfo.tag`
  already uses. Neither of these choices is tested directly by mutation, since
  the brief's mutation set targets `should_notify` only; formatting/rendering of
  these fields for the actual phone message is presumably a later task's
  responsibility.

## Addendum — Ruling 10: the wiring was closed in this task

The controller flagged, correctly, that no task in the plan ever connects the
`mpsc::Receiver<Event>` half of the channel `set_event_sink` creates to a live
`Bridge`. Task 6's file list, Task 7's, and Task 8's all miss it. Per Ruling 10,
that connection now lands here, in `src/bridge.rs` and `src/daemon.rs`.

### The wiring

`bridge.rs` gained one new function:

```rust
pub fn spawn_event_consumer(
    rx: mpsc::Receiver<Event>,
    slot: Arc<Mutex<Option<BridgeHandle>>>,
) -> std::thread::JoinHandle<()>
```

It loops on `rx.recv()` (blocking — that's fine, it's the *consumer's own*
thread, not `tick()`'s), and for each event received, looks at **whatever is in
the bridge slot right now** and, if there's a live `BridgeHandle`, calls
`handle.bridge.enqueue(event)` (the `enqueue` this task already built and
tested). If the slot is `None`, the event is dropped — nothing is written
anywhere.

`daemon.rs::run_with_manager` wires it up once, right after the `bridge` slot
is created and `start_phone_bridge` has had its first chance to populate it:

```rust
let (event_tx, event_rx) = std::sync::mpsc::channel::<Event>();
mgr.set_event_sink(event_tx);
crate::bridge::spawn_event_consumer(event_rx, bridge.clone());
```

This is the **only** place `spawn_event_consumer` is called, and it runs
exactly once for the lifetime of the daemon process — same lifecycle shape as
the existing `tick()` thread a few lines below it (fire-and-forget,
`std::thread::spawn`, no `JoinHandle` retained, matching the codebase's existing
convention for daemon-lifetime background threads).

### What happens to events when no bridge is live, and why that's deliberate

If `spawn_event_consumer` looks at the slot and finds `None` — no token ever
configured, `PhoneDisable` just ran, or `PhoneSetToken`/`replace()` is mid-swap
— the event is dropped on the spot. Nothing buffers it, nothing retries it,
nothing writes it anywhere else first. This is a deliberate design decision, not
an oversight, for two reasons:

1. **There's nowhere correct to put it.** The only bounded, tested store for
   these events is `Bridge::outbound`, and by construction that only exists
   once a `Bridge` exists. Inventing a second, bridge-independent holding area
   just to redeliver later would mean maintaining two queues with two different
   lifetimes and two different drop policies — more surface area, not less, for
   a case (no phone configured yet) that's the default state for most users.
2. **Stale notifications are actively worse than no notification.** If a user
   disables phone notifications, works for an hour, then re-enables them, a
   backlog of "session 3 stopped 47 minutes ago" messages arriving all at once
   is confusing and not actionable — the user has likely already looked at
   the session directly by then. This mirrors the reasoning already in this
   codebase for `Bridge::drain_backlog` (C1's fix): a bot that's been silent
   accumulates real Telegram messages that must be explicitly discarded rather
   than replayed into pairing. Silently dropping stale *outbound* notifications
   is the same instinct applied to the other direction of the channel.

### Interaction with `stop_current` / `replace` — no orphaned consumer

`spawn_event_consumer` is spawned exactly once and never re-spawned by
`replace()`, `stop_current()`, or anything else — it doesn't belong to any one
`Bridge` instance, it belongs to the daemon process. It reads the *current*
value of the shared `bridge` slot on every event, not a value it captured at
spawn time. Concretely:

- **`PhoneDisable` (`stop_current`)**: sets the slot to `None`. The next event
  the consumer thread receives sees `None` and is dropped. The consumer thread
  itself keeps running (blocked in `rx.recv()`), ready for the next
  `PhoneSetToken`/reconnect — there is no second consumer thread to leak, and
  the disabled `Bridge`'s own poll thread is separately stopped by
  `stop_current` calling `old.stop()`, exactly as before this task.
- **`PhoneSetToken` (`replace`)**: swaps the slot from the old `BridgeHandle` to
  a new one (after stopping the old poller, per the existing C3 fix). The
  consumer thread doesn't know or care that a swap happened — it just reads
  whatever is in the slot at the moment the next event arrives. There is no
  window where two consumer threads exist, because there is only ever one.
- **Never configured**: the slot starts `None` and `spawn_event_consumer` is
  still spawned (unconditionally, in `run_with_manager`) — it just drops
  everything until/unless a token is ever set. No wasted thread churn, no
  conditional spawn/despawn logic to get wrong.

This is the same shape as the C2/C3 fixes already documented at the top of
`bridge.rs`: exactly one thread owns "am I currently consuming," and it answers
that question by reading shared state, not by being told to start/stop in sync
with `Bridge` instances coming and going.

### Why `tick()` still cannot block

Nothing about this wiring touches the producer side. `SessionManager::event_tx`
is still a plain unbounded `std::sync::mpsc::Sender<Event>`; `tick()` still only
ever calls `tx.send(event)` on it inside `maybe_notify`, and that call cannot
block — an unbounded `mpsc::Sender::send()` either succeeds (grows the
in-process channel buffer) or fails immediately if the receiver was dropped
(daemon shutting down), it never waits on a consumer. The new consumer thread
is purely a second, independent thread pulling from the *other* end of that
same channel at its own pace; how fast or slow it drains, or whether it's
draining into a live bridge or a `None` slot, has zero effect on how long
`tick()`'s call to `send()` takes. The bound (`QUEUE_CAP`) lives entirely on
the far side of that boundary, inside `Bridge::enqueue`, which `tick()` never
calls directly.

### Tests added (in `bridge.rs`)

- `an_event_sent_through_the_channel_reaches_the_live_bridge` — builds a
  `BridgeHandle` around a `Bridge::for_test()`, puts it in the slot, calls the
  real `spawn_event_consumer`, sends one event through the `mpsc::Sender`, and
  polls `handle.bridge.queued()` until it shows up. This goes through the real
  wiring end to end — it does not call `enqueue` directly.
- `events_are_dropped_without_blocking_when_no_bridge_is_live` — starts the
  slot at `None`, sends 5 events (asserting each `send()` returns immediately,
  under a 2s deadline), sleeps 200ms to let the consumer thread actually attempt
  and drop them while the slot is still `None` (needed to avoid a genuine race:
  the very first version of this test flipped the slot to `Some` immediately
  after sending, and depending on OS thread scheduling the consumer sometimes
  hadn't gotten to event 1 yet, so it saw a live bridge and enqueued it — a test
  bug, not a production bug, since the guarantee this task provides is "no
  bridge *at the moment the event is drained*", which is the only guarantee
  that's actually implementable given retention here is by construction
  momentary, not a second buffer with its own consistency story), then attaches
  a bridge and sends one more event, asserting only that last one appears —
  proving the earlier 5 were genuinely gone, not stashed somewhere and replayed
  late.

### Mutation testing (Ruling 10's two, both caught)

1. **Break the connection** — replaced the `if let Some(handle) = ... {
   handle.bridge.enqueue(event); }` body with a no-op that just touches
   `event` and does nothing else:
   ```
   test bridge::tests::an_event_sent_through_the_channel_reaches_the_live_bridge ... FAILED
   thread '...' panicked: 事件该经真实的消费者线程落到 bridge 的队列里，实际看到 []
   ```
   Caught, as required.

2. **Make the no-bridge path block instead of drop** — replaced the `if let
   Some(handle) = ... { enqueue } ` / implicit-drop-on-`None` logic with a
   `loop { if Some(handle) { enqueue(event.clone()); break } else { sleep(10ms) } }`
   — i.e. instead of dropping when there's no bridge, spin-wait for one to
   show up and deliver the event late:
   ```
   test bridge::tests::events_are_dropped_without_blocking_when_no_bridge_is_live ... FAILED
   assertion `left == right` failed: 没有 bridge 时发的那 5 条不该被攒起来、事后补发
     left: [1, 2, 3, 4, 5, 99]
    right: [99]
   ```
   Caught — the 5 events that should have been dropped were instead delivered
   late once a bridge appeared, exactly the "stale notifications delivered
   after the fact" failure mode Ruling 10 asked to be pinned against.

Both mutations were reverted afterward; the file was diffed against the
pre-mutation backup and confirmed byte-identical before moving on.

### Commands re-run for this addendum

```
cargo build --lib                                        → clean
cargo test --lib bridge -- --test-threads=1               → 32 passed, 0 failed
cargo test --lib -- --test-threads=1                       → 776 passed, 0 failed
cargo fmt --check (before/after `cargo fmt`)               → clean after formatting bridge.rs
cargo clippy --all-targets                                 → clean, no warnings
```

The controller separately ran the full `cargo test -- --test-threads=1` and
reported **814 passed / 0 failed** against a 812 baseline (Task 6's original 8
new tests + this addendum's 2 new wiring tests = +2 over the 812 checkpoint),
confirming no regression across integration/doc tests as well.

### Concerns (updated)

- The 200ms sleep in `events_are_dropped_without_blocking_when_no_bridge_is_live`
  is a pragmatic wait, not a hard guarantee — on an extremely loaded CI box it's
  conceivable (if very unlikely) the consumer thread hasn't been scheduled yet
  after 200ms, which would make the test's precondition wrong rather than the
  production code. This follows the same convention already used elsewhere in
  `bridge.rs` (e.g. `replace_stops_the_old_bridge_before_starting_the_new_one`
  sleeps 150ms twice for a similar reason) rather than inventing a new pattern.
- `spawn_event_consumer`'s `JoinHandle` is discarded (fire-and-forget), matching
  the existing `tick()` thread's pattern in `daemon.rs` — there is currently no
  graceful daemon shutdown path anywhere in this codebase that joins background
  threads, so this isn't a new gap introduced here.
