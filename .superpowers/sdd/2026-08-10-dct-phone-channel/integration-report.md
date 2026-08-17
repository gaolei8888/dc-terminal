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

## Round 2: adversarial review fixes (commit after 35e5efc)

Review of `35e5efc` found the feature was structurally sound (`route_and_
deliver` reachability, lock ordering, map bounding, `merge` staying
model-free, post-restart `Gone` semantics, and the strengthened no-orphan
test were all confirmed correct) but not functional end to end: **C1
(Critical) — a phone reply was typed into the input buffer but never
submitted.**

### C1 — `type_into` must press Enter, not just write

`session.rs::send_input` splits "write characters" and "press Enter" into
two separate calls — writing the body never advances the agent; only a
follow-up call with an **empty string** does (`\r`, checkpoint, `Working`
transition — see `session.rs` around line 785). `ui/grid.rs::send_reply`
already follows this convention explicitly and its doc comment says the
two steps must never be merged or reordered. `SessionWriter::type_into`
(the implementation for `SessionManager`, `bridge.rs`) only did the first
half — a phone reply would sit in the input buffer forever while the
receipt claimed "已经敲进「name」" and the journal recorded `Typed(id)`,
both lying to the one user who cannot look at the terminal to notice.

Fixed by making `impl SessionWriter for SessionManager::type_into` call
`send_input(id, text)` (skipped if `text` is empty, matching `send_reply`'s
own branch) and then unconditionally `send_input(id, "")`, propagating
`Err` from *either* step. A failure on the second step (Enter) after the
first step (body) succeeded still returns `Err` — the PTY may already have
the half-typed body sitting in it, but the receipt/journal will honestly
report `Failed`, never `Typed`, for that case. Trait doc comment
(`SessionWriter::type_into`) rewritten to state this contract explicitly so
a future alternate implementation doesn't regress it silently.

**Test:** `bridge::tests::session_manager_type_into_submits_not_just_types`
— spins up a real `SessionManager` session (`cat`, non-agent so no git
repo needed), calls `type_into` directly, and waits for `SessionState` to
reach `Working` (only the empty-string/Enter branch of `send_input` flips
that state for a non-agent session) rather than merely checking screen
text.

**Strengthened the existing daemon.rs e2e test**
(`start_phone_bridge_wires_the_real_session_manager_and_journal`), which
previously only asserted the reply text appeared on screen — exactly the
blind spot the reviewer identified, since a typed-but-unsubmitted buffer
also shows up on screen. It now additionally waits for the real session to
reach `SessionState::Working`.

**Mutation performed and confirmed to fail both tests:** removed the
"press Enter" step from `type_into`, leaving only the body write.
- `session_manager_type_into_submits_not_just_types` failed: state stayed
  `Unknown`, deadline hit.
- `start_phone_bridge_wires_the_real_session_manager_and_journal` failed:
  `会话此刻的状态是 Some(Unknown)，一直没有推进到 Working`.

Reverted; both pass again.

### I1 (Important) — a merged push's reply must not claim `Gone`

Only single-event batches were ever recorded in `outbound_map`
(`RouteInput::map` is typed `HashMap<MsgId, u32>` — one session per id), so
long-press-replying to a push that merged several stopped agents fell
through `route()`'s rule 1 to `Route::Gone` ("这条消息对应的会话已经不在
了") — a lie, since both sessions were alive and idle. Per the reviewer's
suggested fix, added a second table, `Bridge::ambiguous_pushes:
Mutex<VecDeque<(MsgId, Vec<u32>)>>` (same `MSG_MAP_CAP`/drop-oldest
policy as `outbound_map`, and disjoint from it by construction — an id
goes into exactly one of the two, decided in `run_sender` by
`events.as_slice()` matching `[only]` vs. everything else).

`route_and_deliver` now checks `ambiguous_reply_sessions(reply_id)`
*before* constructing `RouteInput` and calling `route()` — `route()`
itself stays a pure five-rule function untouched, unaware this second
table exists; the ambiguity check is resolved one layer up, in the glue
code whose whole job is assembling `route()`'s inputs from live state. A
hit answers `Route::Ask(sessions)` (the same "several candidates, don't
guess" path already used for "several sessions waiting") and flips
`replied_since_use`, exactly like a normal routed reply would.

**Test:** `bridge::tests::replying_to_a_merged_push_asks_which_one_
instead_of_lying_gone` — enqueues two named events, lets the real
`run_sender()` merge and send them, then replies to the returned `MsgId`
and asserts the reply neither writes to any session nor contains "已经不
在了", and names both candidates.

**Mutation performed and confirmed to fail this test:** removed the
`ambiguous_reply_sessions` check from `route_and_deliver`. Result: the
reply fell through to `Route::Gone` and the test failed on the "不该说成
Gone" assertion with the exact `Gone` message text. Reverted; passes.

### Three small-but-not-cosmetic fixes

- **`/use`/`/ls` command matching swallowed unrelated commands and missed
  `@botname`.** `text.strip_prefix("/use")` matched `/user`, `/useless`,
  etc. as `/use` with garbled arguments (and, on parse failure, silently
  dropped the original message instead of typing it), and neither `/use`
  nor the exact-match `/ls` recognized Telegram's `/cmd@botname` form
  (added automatically when a bot is @-mentioned in a group). Added a
  `strip_command(text, cmd)` helper: a match requires the command to be
  followed by end-of-string, whitespace, or an `@botname` suffix — anything
  else means "this is a different command," returned as `None`. Both
  `/use` and `/ls` now go through it.

  **Tests:** `use_prefix_does_not_swallow_unrelated_commands` (`/user`,
  `/useless` sent to a session with one waiting candidate must type
  through verbatim, not get eaten as malformed `/use`) and
  `use_and_ls_recognize_the_at_botname_suffix` (`/use@my_dct_bot 3` then
  `/ls@my_dct_bot`, against **two** waiting candidates specifically so a
  false pass via the "only one waiting" rule can't mask a broken `@botname`
  parse). **Mutation performed:** reverted `strip_command(text, "/use")`
  back to `text.strip_prefix("/use")`. Both tests failed (the first because
  `/user`/`/useless` got swallowed and typed nothing; the second because
  `/use@my_dct_bot 3` failed to parse and the follow-up message fell
  through to `Route::Ask` instead of `Route::To(3)`). Reverted; both pass.

- **`reply()` held the `owner` mutex across `Channel::send()`.** `if let
  Some(to) = *recover(self.owner.lock()) { self.ch.send(to, text); }` —
  Rust's `if let` temporary-lifetime extension keeps the `MutexGuard` alive
  for the whole arm body, so the lock was held for the full duration of the
  network call (up to several seconds), blocking `PhoneUnpair`'s
  `clear_owner()` and the send loop's owner read for no reason. Fixed by
  copying the `Option<i64>` out of the guard on its own line before the
  `if let`, so the guard drops immediately.

- **No `stop` check between `drain_outbound()` and `ch.send()` in
  `run_sender`.** Mirrors the same class of bug `run()`'s F2 fix already
  addressed on the inbound side (`stop()` can land in the narrow window
  between a blocking call returning and the next side effect). Added a
  `self.stop.load()` check right after `drain_outbound()` and before the
  network send; on a hit the already-drained batch is dropped (not
  requeued), consistent with the existing "no accumulation across a
  disabled channel" policy used everywhere else in this file.

- **`session.rs::maybe_notify`'s project-name fallback leaked a full path.**
  `dir.file_name()` returns `None` for root (`/`) or paths ending in `..`;
  the old fallback was `s.dir.display().to_string()` — a full local
  filesystem path sent to the phone, crossing the no-paths privacy
  boundary from CLAUDE.md. Changed the fallback to a fixed placeholder
  string ("未命名项目") — honest, not a fabricated name, and never a path.
  No dedicated test added (this is a genuinely degenerate path shape,
  `SessionState`/`Event` plumbing here is otherwise already covered); flag
  for the final review if a path-shaped fixture is wanted.

### Commands run (round 2)

```
cargo fmt --check                    -> clean
cargo clippy --all-targets           -> clean, zero warnings
cargo test --lib -- bridge:: daemon:: session:: journal:: channel:: --test-threads=2
                                      -> 192 passed, 0 failed
cargo test -- --test-threads=1 (background, full suite) -> see below
```

Round 2 adds 5 new tests (4 in `bridge.rs`, 0 new in `daemon.rs` — the
existing e2e test was strengthened in place rather than duplicated).
Round-1 baseline to hold was 852 passing; round 2 should land at 857
(pending the background run).

### Concerns (round 2)

- The project-name-fallback fix (`session.rs`) has no dedicated regression
  test — it's a one-line change covering a genuinely degenerate path shape
  (root directory or a dir ending in `..`), and constructing a `Session`
  with such a `dir` in a unit test seemed like more machinery than the fix
  warranted, but flagging in case the final review disagrees.
- `ambiguous_pushes` and `outbound_map` are two separate bounded structures
  now instead of one; they're kept disjoint by construction (a `MsgId` is
  written to exactly one, decided once in `run_sender`), but a future
  change to either has to remember to preserve that invariant — there's no
  type-level enforcement of "never in both."
