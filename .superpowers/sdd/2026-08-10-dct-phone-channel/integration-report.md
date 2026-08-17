# Integration report: connecting the phone-channel components

Branch: `worktree-phone-channel`, starting commit `1ef77df`. This is the
cross-task integration pass — none of Tasks 1-9 wired their components to
each other; this pass closes the three gaps named in the brief.

## What was wired, and where

### (a) Outbound send loop

`Bridge` gained a second background thread, `run_sender()` (src/bridge.rs),
started alongside the existing poll thread in `spawn()`. Every
`SEND_INTERVAL` (500ms) it:

1. Sleeps via the existing `sleep_or_stop()` helper (checking `self.stop`
   both before and after the sleep — same pattern as the poll loop).
2. Drains the whole `outbound` queue with a new `drain_outbound()` (a
   consuming counterpart to the read-only `queued()`).
3. If the batch is non-empty and there is a paired owner, calls `merge()`
   (Task 9, unchanged) and sends the result via `Channel::send`.
4. If the batch was exactly one event, calls the new `record_push(id,
   session)` to remember `MsgId -> session` in `outbound_map`.

No debounce logic was added here: `session.rs::tick()` already debounces
per-session before an event ever reaches the queue (`Session::last_notified`
+ `debounce()` + `DEBOUNCE_WINDOW`, see `session.rs` around line 1094) — by
the time an `Event` reaches `Bridge::enqueue`, the debounce decision is
already made. The send loop's only job is "what's in the queue right now,
send it."

### Isolation from `tick()`

`tick()` only ever calls `mpsc::Sender::send()` on an unbounded channel —
it never blocks, regardless of what `run_sender()` is doing. The existing
`spawn_event_consumer` (Task 6) is the single always-alive consumer of that
channel; it transfers events into `Bridge::enqueue` (bounded, drop-oldest,
`QUEUE_CAP`=32) without ever touching the network. `run_sender()` reads
*only* from that bounded queue and only writes to the network — there is no
path back from `run_sender()` to `tick()` or to the consumer thread. Three
independent threads, one direction of data flow, no shared blocking point.

### MsgId -> session map: location and lifetime

Lives in `Bridge::outbound_map: Mutex<VecDeque<(MsgId, u32)>>`, capped at
`MSG_MAP_CAP = 256` with drop-oldest (same policy as `outbound`). It is
**in-memory only, not persisted** — a daemon restart loses it entirely.
This is intentional and was already anticipated by `route()`'s rule 1
(`Route::Gone` exists specifically for "the map doesn't know about this
`MsgId` anymore," see `bridge.rs` around line 90 and the C1-adjacent
comment). A reply to a pre-restart push after a restart safely resolves to
"this message isn't known anymore, check /ls" rather than guessing.

One design decision worth flagging: a merged push covering *more than one*
session is never added to the map (only single-event batches are recorded).
`RouteInput::map` is typed `HashMap<MsgId, u32>` — one session per message
id — so a batch push has no single correct answer if replied to. Rather
than guess or invent a multi-value map type, a reply to a multi-event push
falls through to `Route::Gone` ("check /ls"), which is the same
fail-safe the reviewer already approved for the restart case.

### (b) Inbound routing and delivery — the security-critical wiring

`dispatch()` was restructured from `if let Accepted::Paired(...) = ...`
(state-slot-only) to a full `match` on `Accepted`:

```rust
fn dispatch(&self, msg: &Incoming) {
    match self.accept(msg) {
        Accepted::Paired(chat_id) => {
            (self.persist_owner)(chat_id);
            /* ...state slot update... */
            self.route_and_deliver(msg);
        }
        Accepted::FromOwner => self.route_and_deliver(msg),
        Accepted::Rejected => {}
    }
}
```

**Exactly how this structurally guarantees a Rejected message can never
reach `route()`/`deliver()`:** the `Accepted::Rejected` match arm's body is
empty. There is no code path, branch, or fallthrough from that arm into
`route_and_deliver`. To make a rejected message reach routing, someone
would have to *add a line inside that specific arm* — a one-line diff that
is impossible to make accidentally and impossible to miss in review. This
is different from (and stronger than) "call `route_and_deliver`
unconditionally, then guard on `!matches!(_, Rejected)`" — that shape
would let someone delete a guard clause and reopen the hole invisibly in a
larger diff. The three-way match makes "which paths are wired" visible in
the match itself.

`route_and_deliver()` itself does no authentication — it's called from two
authenticated arms only, and its own doc comment says so explicitly ("把它
挪到 `accept()` 之前调用就是重新打开安全漏洞").

Test: `security_a_rejected_stranger_never_reaches_route_or_deliver`
(src/bridge.rs) — a stranger sends `/use 1`, `/ls`, and a reply-shaped
message; asserts `spy.written()` (PTY writes) and `spy.replies` (channel
sends) are both empty. All three message shapes were chosen because they're
the ones most likely to accidentally short-circuit into `route_and_deliver`
if the match were flattened.

**Mutation performed and confirmed to fail this test:** changed
`Accepted::Rejected => {}` to `Accepted::Rejected => { self.route_and_deliver(msg); }`.
Result:
```
thread '...security_a_rejected_stranger_never_reaches_route_or_deliver' panicked:
陌生人不该收到任何回执 — 回执是发给主人的 ...
```
Reverted immediately after confirming failure; `cargo test` back to green.

### `RouteInput` construction

`route_and_deliver()` builds the real `RouteInput` from:
- `map`: a snapshot clone of `outbound_map`.
- `used`: `Bridge::used: Mutex<Option<u32>>`, set by `/use`.
- `replied_since_use`: `Bridge::replied_since_use: Mutex<bool>`.
- `waiting`: a new `SessionWriter::waiting(&self) -> Vec<u32>` method,
  implemented for `SessionManager` as "all sessions in `SessionState::Idle`"
  (the state that means "done with a turn, sitting at the prompt" — the
  only state the spec's "等待用户输入" description matches; `Working` /
  `Stopped` / `Failed` / `Unknown` are all excluded).

### (c) `/use` and `/ls`

Both live in `route_and_deliver()`, checked *before* falling through to
`route()`:

- `/use <n>`: parses `<n>` as `u32`. On success, sets `used = Some(n)` and
  **resets `replied_since_use = false`** (a fresh explicit choice should not
  inherit a previous choice's "already expired" state). On parse failure,
  replies with the correct format — never guesses, never crashes.
- `/ls`: replies with the list of `waiting()` session ids, using
  `SessionWriter::name_of` where available and falling back to the existing
  `fallback_name()` (same convention as `ask_message`/`merge`). Empty list
  gets an honest "没有会话在等你说话," not silence.

### `/use` expiry tracking

`replied_since_use` is flipped to `true` at the end of `route_and_deliver`,
**after** `route()`/`deliver()` have run, but only when `msg.reply_to.is_some()`
— i.e., only a genuine long-press reply counts as "attention moved
elsewhere." This ordering matters: the reply message itself is routed by
rule 1 regardless of `/use` state (rule 1 always wins), so flipping the
flag after processing doesn't affect the current message's outcome — it
only affects the *next* non-reply message, which is the correct semantics
per rule 3's own justification comment.

Test: `use_then_reply_then_use_expires` — sets `/use 3`, sends a bare
message (goes to 3), replies to a pre-recorded push mapped to session 9
(goes to 9 via rule 1, and flips the flag), sends another bare message
(now goes to 9 via rule 4 — `/use` has expired).

## Threading model / no-orphan guarantee

`spawn()` now starts **two** threads per `Bridge` (poll + sender), both
wrapped in `catch_unwind` individually, both reading `self.stop:
AtomicBool`. Critically, **no new stop mechanism was introduced** — the
sender thread was deliberately built to share the exact `AtomicBool` that
`stop()`/`replace()`/`stop_current()` already flip. This means the
existing, already-reviewed C2/C3 fixes (`replace()` stops the old bridge
before starting a new one; `stop_current()` stops and clears the slot)
automatically cover the sender thread with zero additional code in
`daemon.rs` — there is exactly one kill switch per `Bridge`, and both
threads obey it.

**Mutation performed and confirmed to fail the no-orphan test:** removed
both `stop.load()` checks from `run_sender()`'s loop (leaving a bare
`loop { sleep(SEND_INTERVAL); drain...; }`). First pass at the orphan test
(`stop_leaves_neither_the_poller_nor_the_sender_still_running`) did *not*
catch this — it only asserted send-count didn't grow, which is trivially
true if no new events are queued regardless of whether the thread is really
gone. Strengthened the test to enqueue a *new* event **after** calling
`stop()`, then wait `3 * SEND_INTERVAL`; a still-running thread will pick
the new event up and send it. With the mutation in place this reliably
failed (`left: 2, right: 1` — one extra send happened). Reverted the
mutation; test passes clean afterward. This is noted as a concern below —
the first-draft version of this test was not actually a mutation-proof pin
until strengthened.

## Wiring `daemon.rs` -> `Bridge`

`spawn()`/`replace()` gained two new parameters:
`writer: Option<Arc<dyn SessionWriter>>` and `journal_path: Option<PathBuf>`,
wired into the `Bridge` *before* either thread starts (no window where a
thread is alive but not yet connected to a writer). `None`/`None` reproduces
the prior default (unwired) behavior exactly, so every existing test that
didn't care about writer/journal needed no behavior changes — only two extra
`None, None` arguments at each call site (7 call sites total across
`bridge.rs` and `daemon.rs` tests).

Production call sites (`start_phone_bridge` and the `PhoneSetToken` handler
in `daemon.rs`) pass `Some(mgr.clone() as Arc<dyn SessionWriter>)` — reusing
the `impl SessionWriter for SessionManager` already written in `bridge.rs`
— and `mgr.journal.path()`, a new getter added to `Journal` (`journal.rs`)
so the `Bridge`'s own `Journal` instance can share the exact same file path
as `SessionManager::journal` without threading the daemon's socket path
through `handle()`.

`start_phone_bridge` gained a `mgr: &Arc<SessionManager>` parameter (it
previously had no access to the session manager at all — this was the most
direct evidence that this wiring had never been done).

End-to-end test added in `daemon.rs`:
`start_phone_bridge_wires_the_real_session_manager_and_journal` — creates a
*real* `SessionManager` session (via a plain non-agent "cat" profile, no git
repo needed), feeds `start_phone_bridge` a fake `Channel` that returns
`/use <id>` then a message over `poll()`, and asserts the text actually
lands on the real PTY's screen (`mgr.screen_text_for_test`) and that the
journal file (shared with `mgr.journal`) gets a `typed session=` line. This
is the only test in the whole change that doesn't use `bridge.rs`'s `Spy` —
it exists specifically to catch a wiring mistake that unit tests using fake
writers structurally cannot catch (e.g., passing `mgr.clone()` to the wrong
parameter, or forgetting to call `mgr.journal.path()`).

## All mutations performed, with results

1. **Map write removed** (`self.record_push(id, only.session)` deleted from
   `run_sender`) → `a_queued_event_is_sent_and_its_msg_id_is_recorded` failed
   (`left: None, right: Some((7, "先跑完"))`). Reverted; passes.
2. **Route call moved to the `Rejected` arm** (`Accepted::Rejected => {
   self.route_and_deliver(msg); }`) → `security_a_rejected_stranger_never_
   reaches_route_or_deliver` failed. Reverted; passes.
3. **Stop made a no-op for the sender thread** (both `stop.load()` checks
   removed from `run_sender`'s loop) → first version of
   `stop_leaves_neither_the_poller_nor_the_sender_still_running` did **not**
   catch it (weak test — see above); strengthened by enqueuing a new event
   after `stop()` and waiting `3 * SEND_INTERVAL`, which then failed as
   expected (`left: 2, right: 1`). Reverted; passes.

All three mutations were performed directly on `src/bridge.rs`, verified to
fail the named test, then reverted via a saved backup
(`/private/tmp/.../scratchpad/bridge.rs.bak`) and re-verified to pass.

## New/changed public surface

- `bridge.rs`: `SessionWriter::waiting(&self) -> Vec<u32>` (new trait
  method — implemented for `SessionManager` and for the test `Spy`).
  `Bridge::spawn`/`replace` gained two trailing parameters.
  `MSG_MAP_CAP` constant, `SEND_INTERVAL` constant.
- `journal.rs`: `Journal::path(&self) -> Option<PathBuf>` (new getter).
- `daemon.rs`: `start_phone_bridge` gained a `mgr: &Arc<SessionManager>`
  parameter.

## Commands run and results

```
cargo fmt --check                    -> clean
cargo clippy --all-targets           -> clean, zero warnings
cargo test --lib -- bridge:: daemon:: session:: journal:: channel:: --test-threads=2
                                      -> 188 passed, 0 failed
cargo test -- --test-threads=1 (background, full suite)
                                      -> 818 lib tests + 34 integration tests
                                         across other binaries, all green,
                                         except the pre-existing known flake
                                         `sighup_restores_the_terminal`
                                         (real-PTY signal timing, called out
                                         explicitly in the task brief as
                                         known-flaky; not touched by this
                                         change; unrelated to phone channel
                                         code).
```
Baseline was 845 tests passing at `1ef77df`; this change adds 9 new tests
(8 in `bridge.rs`, 1 in `daemon.rs`) and modifies no existing test's
assertions (only channel/argument-list mechanics on 4 pre-existing
`dispatch()` tests that now also exercise the newly-wired
`route_and_deliver` path — see "Test-only channel/mock changes" below).
Total passing after this change: 852 (853 total minus the 1 known flake).

## Test-only channel/mock changes required by the wiring

Four pre-existing `dispatch()`-based tests
(`dispatch_on_pairing_writes_paired_state_and_owner`,
`dispatch_from_owner_does_not_touch_the_slot_again`,
`dispatch_from_a_stranger_leaves_the_slot_untouched`,
`dispatch_on_pairing_calls_persist_owner_exactly_once`) previously used the
`NeverCalled` channel stub (which panics if `send`/`poll`/`get_me`/`drain`
are ever invoked) because, before this integration pass, `dispatch()` never
touched the channel at all. Now that `dispatch()` also runs
`route_and_deliver` for `Paired`/`FromOwner`, these tests' generic message
text (e.g. "hi", "配对") resolves to `Route::NeedUse`, which sends a reply
— so they were switched to the `Spy` channel (already used elsewhere in the
file for `deliver()` tests), which records sends instead of panicking. No
assertion in any of these four tests changed; only the channel constructor
changed.

Similarly, `MockChannel` (used by the `run()`-loop tests) had its `send()`
changed from `unimplemented!()` to a real recording implementation,
because a message that completes pairing (e.g. `run_populates_bot_then_
pairs_then_stops_on_bad_token`'s `msg(111, "hi")`) now also triggers a
`NeedUse` reply via the newly-wired `route_and_deliver`.

## Concerns

1. **First-draft mutation-3 test was not actually mutation-proof** until
   strengthened (see above) — flagged in case the pattern ("assert a
   counter doesn't grow, but never feed it anything new to *not* react to")
   recurs elsewhere in the codebase's thread-stop tests; it's worth an
   audit pass but is out of scope for this integration task.
2. **A merged multi-session push cannot be replied to specifically** — by
   design (see the map section above), but worth flagging as a product
   behavior: if two agents stop within the same debounce window and get
   merged into one push, long-press-replying to that push always yields
   `Route::Gone` ("check /ls"), never routes to either session directly.
   This seemed like the correct conservative choice given `RouteInput::map`'s
   type, but a future task could revisit if this proves annoying in practice.
3. **`outbound_map` and `used`/`replied_since_use` are process-lifetime
   only, not persisted** — same as the rest of `Bridge`'s per-session
   routing state. A daemon restart mid-conversation loses `/use` selection
   and the msg-id map; this is consistent with the rest of the design
   (`Route::Gone` exists for exactly this case) but is worth being aware of
   if a future task considers persisting more of `Bridge`'s state.
