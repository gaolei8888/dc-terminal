# SDD ledger — plan: docs/superpowers/plans/2026-08-10-dct-phone-channel.md

Spec: docs/superpowers/specs/2026-08-10-dct-phone-channel-design.md (reachable — rulings are binding, not provisional)
Worktree: .claude/worktrees/phone-channel, branch `worktree-phone-channel`
Baseline: 738 tests passing, 0 failing, at 1a78887

## Ruling 0 (setup)
Worktree was branched from origin/main (d8ec739), one commit behind local main (1a78887,
"drop the global Ctrl+Q escape key"). Reset the branch to local main tip.
Ruling: build on 1a78887 — this plan edits key handling in ui/mod.rs, board.rs,
settings_view.rs, and building on stale key-handling code invites conflicts.
Cost if wrong: the branch carries one unpushed local commit that origin lacks;
rebase away if the user wanted origin/main as the base.

## Pre-flight scan

### Interface pairs (producer → consumer)

| Producer | Consumer | Produces vs consumes | Finding |
|---|---|---|---|
| T1 `Channel`/`Incoming`/`ChannelError`/`MsgId` | T2 | matches | clean |
| T1 `Channel`/`Incoming` | T5 `Bridge::new(Arc<dyn Channel>)` | matches | clean |
| T1 `Event`/`EventKind`/`debounce`/`DEBOUNCE_WINDOW` | T6 | matches | clean |
| T1 `MsgId` | T7 (`map: HashMap<MsgId,u32>`) | matches | clean |
| T1 `Event` | T9 `merge(&[Event], Lang)` | matches | clean |
| T2 `Telegram::new` + `parse_get_me` | T4 (PhoneSetToken → getMe) | matches; T2 Step 5 supplies real transport | clean |
| T3 `SettingsItem::Phone` | T4 `View::Phone` | **T3 Step 3 dispatches Enter→`View::Phone`, which T4 creates** | CONFLICT 2 |
| T4 `PhoneState` | T5 (writes `Broken`), T11 (writes `Broken`) | type matches, but **no task defines the shared status slot the bridge writes and `Request::PhoneStatus` reads** | CONFLICT 3 |
| T5 `Bridge`/`Accepted` | T7, T8, T9, T10, T11 | matches | clean |
| T6 `Sessions::set_event_sink(mpsc::Sender<Event>)` | T5/T11 queue | **T6 says "queue full → drop"; T11 says "drop the oldest"; T11 also "produces" QUEUE_CAP that T6 needs** | CONFLICT 4 |
| T7 `Route`/`route` | T8 `deliver`, T10 `narrow` (Ask only) | matches | clean |
| T8 `Delivered` | T11 | matches | clean |
| T9 `llm::complete_with_timeout`, `llm::Backend` | verified present at src/llm/mod.rs:31,40 | matches | clean |
| T8 `Sessions::send_input` | verified present at src/session.rs:729 | matches | clean |

### Shared-file pairs

| File | Tasks | Finding |
|---|---|---|
| `src/channel/mod.rs` | T1 creates, T2 adds telegram | **T1's code declares `pub mod telegram;` but T2 creates that file → T1 cannot compile** — CONFLICT 1 |
| `src/lib.rs` | T1 (`pub mod channel`), T5 (`pub mod bridge`) | disjoint lines, sequential | clean |
| `src/bridge.rs` | T5 creates; T6,T7,T8,T9,T10,T11 modify | strictly sequential | clean |
| `src/ui/view.rs` | T3 (`View::Settings`), T4 (`View::Phone`) | disjoint variants | clean |
| `src/i18n.rs` | T3, T4 add Keys | additive, sequential | clean |
| `src/daemon.rs` | T4 (Phone requests), T5 (bridge thread) | disjoint | clean |
| `src/session.rs` | T6 only | clean |
| `src/proto.rs`, `src/secrets.rs` | T4 only | clean |
| `src/journal.rs` | T8 only | clean |
| `src/llm/mod.rs` | T9 only | clean |
| `src/ui/phone.rs` | T4 creates, T11 modifies | sequential | clean |

### Per-task self-consistency

| Task | Own text agrees with itself? |
|---|---|
| T1 | tests cover `debounce` + `worth_retrying`; impl supplies both — yes, except CONFLICT 1 |
| T2 | 8 tests vs 3 parse fns + Telegram; Step 5 adds offset test as required — yes |
| T3 | 3 tests vs `SettingsItem::all`/`at`; Step 6 requires an arrow-key test if mutation survives — yes, except CONFLICT 2 |
| T4 | 3 tests vs `status_line`/`next_step`; token-leak test is the load-bearing one — yes, except CONFLICT 3 |
| T5 | 3 tests vs `accept`; poll thread has no test (acknowledged, network-bound) — yes |
| T6 | 4 gate tests + 1 tick integration test vs `should_notify` — yes, except CONFLICT 4 |
| T7 | 7 tests vs 5 rules in fixed order — yes |
| T8 | 3 tests vs `deliver`; test asserts receipt names the session ("修登录白屏") but `Route::To(7)` carries only an id — the writer spy must resolve id→name, so `deliver` needs a name source. Noted for the T8 dispatch. |
| T9 | 4 tests vs `merge`/`parse_options`; `options_prompt` untested (LLM-bound, fallback path is what matters) — yes |
| T10 | 6 tests vs `map_answer`/`narrow`; Step 5 requires an `answering("0")` test if the range mutation survives — yes |
| T11 | 2 tests vs `backoff`/queue cap; Step 6 is manual and needs a real bot token — deferred, see Ruling 5 |

### Global-constraint vs rubric conflicts
None. The plan mandates mutation testing and forbids `continue` in key branches; neither
is treated as a defect by the review rubric.

## Rulings

**Ruling 1 (CONFLICT 1) — `pub mod telegram;` moves from Task 1 to Task 2.**
Task 1 creates `src/channel/mod.rs` WITHOUT that line; Task 2 adds it when it creates
telegram.rs. Reason: Task 1 must compile and pass its own tests at its own commit, which
the plan's Step 4 explicitly requires. Cost if wrong: one line lands in the next commit
instead of this one — no behavioural difference.

**Ruling 2 (CONFLICT 2) — Task 3 does not reference `View::Phone`.**
Task 3 delivers the `SettingsItem` list refactor and the `Language` dispatch only; its
`Phone` arm is inert. Task 4 replaces that arm with the `View::Phone` transition and owns
the test for it. Reason: same as Ruling 1 — Task 3 must compile standalone, and
`View::Phone` does not exist until Task 4. Cost if wrong: between the two commits the
Phone row is selectable but does nothing; no user ever sees that intermediate state.

**Ruling 3 (CONFLICT 3) — Task 4 owns the shared phone-status slot.**
Task 4 adds `Arc<Mutex<PhoneStatus>>` to the daemon's state and serves
`Request::PhoneStatus` from it. Task 5's bridge thread writes into that same slot.
Reason: Task 4 already modifies daemon.rs and must return a real status; the plan simply
never says who creates the slot. Cost if wrong: Task 5 relocates the field — a small,
contained refactor.

**Ruling 4 (CONFLICT 4) — bounded, drop-oldest, and `QUEUE_CAP` is defined in Task 6.**
`Sessions` holds an unbounded `mpsc::Sender<Event>` so `tick()` can never block (T6's
hard invariant). The bridge's consumer thread drains it into a bounded
`Mutex<VecDeque<Event>>` with drop-oldest on overflow; `Bridge::enqueue`/`queued()` act on
that deque, which is what T11's test measures. `QUEUE_CAP` is defined in Task 6 because
Task 6 is the first task that needs a bound. Reason: T6 says "drop", T11 says "drop the
oldest" — T11 is the more specific statement, and for stop/fail notifications the newest
events are the informative ones. Cost if wrong: after a long disconnect the user sees the
newest N events rather than the oldest N; a one-line change to reverse.
Residual risk recorded: if the bridge thread dies, the unbounded mpsc grows until the
receiver drops. Bridge thread is `catch_unwind`-wrapped, so this needs a hard abort to
happen. Accepted.

**Ruling 5 — Task 11 Step 6 (end-to-end, real bot token) is NOT run this session.**
The user has no bot token to hand and asked to supply it later. Steps 1-5 and 7 of Task 11
are implemented and unit-tested; Step 6 is left unchecked in the plan and recorded in the
spec's unverified table as NOT RUN — not as "verified". Reason: the plan's own rule is
"没跑过的一律记成没跑过". Cost if wrong: nothing is claimed that was not run; the live
pairing path stays unproven until the user runs it.

**Ruling 6 — commit messages carry no AI signature line.**
The plan's Global Constraints say "提交信息用英文，不要 AI 署名行". The harness default
appends a Co-Authored-By trailer. Plan wins: it is project-specific and explicit.
Cost if wrong: commits lack attribution; amendable at any time.

**Ruling 7 (T8 note, not a conflict) — `deliver` needs an id→name source.**
T8's test asserts the receipt names the session ("修登录白屏") while `Route::To(7)` carries
only an id. The Bridge must therefore hold a way to resolve a session id to its name.
Carried into the Task 8 dispatch rather than ruled on now — the implementer picks the
mechanism that fits `Sessions`.

## Progress

Task 1: implementer DONE, commit 6a00dda (base 1a78887). 697 lib tests pass, full suite ok,
fmt + clippy clean, both prescribed mutations caught. Review dispatched.
Task 1: review clean — spec ✅, quality Approved, no findings at any severity.
  Its one ⚠️ (fmt/clippy not diff-verifiable) resolved by controller: `cargo fmt --check`
  clean, `cargo clippy --all-targets` 0 warnings/errors.
Task 1: complete (commits 1a78887..6a00dda, review clean)
Task 2: implementer DONE_WITH_CONCERNS, commit 6e01db5. 707 lib tests pass, fmt+clippy clean,
  3 mutations tried and all caught; the 403 mutation survived the brief's 8 tests exactly as
  the brief predicted, so a 403 test was added. Implementer also found a genuine defect in the
  brief's reference code: `pub type Send = dyn Fn(..) + Send + Sync` does not compile (the alias
  shadows `std::marker::Send`, E0404); fixed by fully qualifying.

**Ruling 8 (Task 2 concern — security-relevant) — the destination becomes an explicit
argument: `Channel::send(&self, to: i64, text: &str)`.**
The implementer had to invent a send target because Task 1's `send(&self, text)` carries no
destination, and chose "latch the first incoming chat_id inside `Telegram::poll()`". Task 5's
`Bridge` latches an owner on first message too. That is two independent first-sender-wins
latches in different layers, and they can diverge — re-pairing via the phone page's `r` key,
or one being restored across a daemon restart while the other is not. When they diverge the
adapter sends the user's session notifications to whichever chat it latched, which may not be
the owner the Bridge accepted. Ruling: the adapter holds no ownership state at all; Bridge
passes the destination on every send and remains the single source of truth.
Cost if wrong: a signature change that currently has zero call sites (Bridge is Task 5, unwritten),
versus a duplicated security latch that would have to be untangled after Tasks 5-8 build on it.
Also carried into Task 5's dispatch: Bridge owns pairing, exclusively.

Task 2: fix round 1/5 — original implementer STALLED mid-fix (watchdog, no progress 600s).
  It had applied only step 1 (the trait signature in src/channel/mod.rs) and left the tree
  non-compiling: telegram.rs still had the old `impl Channel`. Fresh implementer dispatched
  to finish, carrying the partial-state description + the ruling. Uncommitted at handoff.
Task 2 deferred (for final review, not blocking): `send_real`/the real ureq path has no test
  coverage by design, so the live wire format has never been exercised; and `timeout_from_url`
  parses the poll timeout back out of a URL string rather than threading it through the
  transport closure's signature — works, but unusual.
Task 2: fix round 1/5 completed by the fresh implementer, commit e100747. Ruling 8 fully
  applied (chat_id field and auto-latch removed, `send(to, text)` seam pinned by a new test,
  mutation on the `to` param caught). 708 lib tests pass, fmt+clippy clean.
  Scoped review of 6a00dda..e100747 dispatched.
Task 2: review clean — spec ✅ (Rulings 1 and 8 both verified fully applied, no residual
  latch fragment), quality Approved, no Critical/Important findings.
  ⚠️ (test/fmt/clippy not diff-verifiable) resolved by controller: fmt clean, clippy 0,
  full suite 746 passed / 0 failed (baseline was 738).
Task 2: minor (deferred): `max_update_id` re-parses the response body a second time inside
  `poll()` instead of `parse_updates` returning the max id alongside the Vec. Harmless —
  empty batch yields None so the cursor is correctly left untouched — but duplicated parsing.
Task 2: complete (commits 6a00dda..e100747, review clean, 1 minor deferred)

## Out-of-plan work requested by the user mid-run

**Border removal (user request, 2026-08-16).** Copying text out of dct picks up the vertical
border glyphs. User asked for left/right borders removed, and chose scope "every view".
Sites: attach.rs:269, board.rs:225, keys.rs:226, settings_view.rs:81, secret.rs:434 and :506,
pick.rs:399/:425/:526, mod.rs:2021, plus grid.rs:597-599 (BorderType, not Borders).
NOT a one-liner: border width is baked into attach.rs:278 (`screen_origin`, used for
mouse→cell mapping), attach.rs:282 (cursor placement), and a measured title-truncation
budget at attach.rs:251 that explicitly subtracts 2 columns for the border. Also need to
check wherever PTY columns are derived from the drawing area.
QUEUED behind Task 3 — it edits settings_view.rs, which Task 3 holds. Dispatch as its own
commit after Task 3 completes and before Task 4 starts.

## Progress (continued)

Task 3: implementer DONE_WITH_CONCERNS, commit 717b5d7. 754 tests pass (baseline 746 + 8 new),
  fmt + clippy clean. Three self-reported concerns, all carried into the review dispatch:
  (a) added `lang: Option<ListState>` to `View::Settings` and touched mod.rs + pick.rs beyond
      the brief's file list, because `Lang::all().len()` and `SettingsItem::all().len()` are
      both 2 today and one shared ListState would let the two lists collide;
  (b) the prescribed `move_sel_n` mutation SURVIVED — with both constants equal to 2, no test
      can distinguish them; the implementer added the arrow-key test anyway and disclosed the
      survivor rather than hiding it;
  (c) Step 5's manual `cargo run --release` language check not run (no interactive terminal),
      recorded as not run per the project's rule.
  Review of e100747..717b5d7 dispatched.
Task 3: review clean — spec ✅ (Ruling 2 verified: Phone arm inert, no View::Phone reference),
  quality Approved. Reviewer adjudicated concern (a) as genuinely required and NOT scope creep;
  concern (b) accepted as a documented survivor no test can distinguish today; concern (c)
  accepted as correctly disclosed. Pre-existing language tests verified relocated but
  unweakened — apply+persist, Esc no-op, and native-name rendering assertions all intact.
Task 3: minor (deferred): settings_view.rs:357 `draw()` clones the whole `View` each frame
  instead of matching by reference and cloning only the needed sub-field.
Task 3: complete (commits e100747..717b5d7, review clean, 1 minor deferred)

Border removal: complete, commit b6046ac (out-of-plan user request, own commit, ahead of Task 4).
  Every view now uses `Borders::TOP | Borders::BOTTOM`; no `Borders::ALL` remains in src/.
  Coupled geometry all moved with it: attach.rs derives from `Block::inner()` instead of
  hand-computed +1s, `screen_origin` / cursor / title budget updated together, mod.rs PTY
  column count widened to the full area, bottom-bar width math corrected, and the test helper
  that located the bottom bar by its `┌` corner reworked (that glyph no longer exists).
  754 passed / 0 failed (baseline count exactly), fmt clean, clippy 0.
  First implementer died mid-finish (machine slept) with the code complete but uncommitted;
  a finisher verified all 8 diffs, confirmed no test was weakened, and committed.
  NOT verified by a human eye: whether copy/paste now actually comes out clean, and whether
  the layout still reads right. Needs an interactive `cargo run --release` from the user.

Task 4: implementer DONE_WITH_CONCERNS, commit e082312. Controller-verified: 771 passed / 0
  failed, fmt clean, clippy 0. Rulings 2 and 3 both verified applied; Ruling 8 respected
  (no ownership state added to the channel layer).
Task 4: review — spec ✅, quality Approved. Adjudication of the four self-reported concerns:
  (1) `Broken(String)` write-only: NOT a defect. Making status_line/next_step structurally
      ignore the payload is the correct defensive posture — relying on the writer to sanitize
      breaks silently the first time daemon.rs is edited. The fixed "re-paste the token"
      next step is valid regardless of the underlying reason.
  (2) hardcoded-Chinese Broken string: Minor, latent only while (1) stands. Deferred.
  (3) restart → `WaitingForPairing{bot:None}`: UPHELD as Important and production-reachable.
      status_line falls back to `PhoneOffLine`, so a non-Off state announces itself as Off
      while next_step still says "go message the bot" naming nothing.
  (4) test-helper duplication: Minor. Deferred.
Task 4: minor (deferred): daemon.rs:325-333 `phone_set_token_failure_message` is unread today
  (YAGNI — could be deleted until Task 5/6 surfaces it).
Task 4: minor (deferred): test-helper duplication between settings_view.rs and mod.rs because
  the existing helper is private.
Task 4: fix round 1/5 dispatched for finding (3) — own honest line for bot:None, bilingual,
  plus a test and a mutation. Ruling: fix in Task 4 rather than deferring to Task 5, because
  the misleading text lives in this task's code and a state that lies about itself is exactly
  what gets forgotten. Task 5 must ALSO populate `bot` promptly — carried into its dispatch.
Task 4: fix round 1/5 (1 addressed, 0 open; commit c7b263a). Re-review: all five items
  ADDRESSED. New `PhoneReconnectingLine` / `PhoneNextStepReconnecting` keys, both languages,
  pinned by `waiting_without_a_bot_name_is_neither_off_nor_a_dangling_instruction` (asserts
  the line differs from the Off string AND the next step contains neither "@" nor "bot"),
  mutation applied and caught. No behaviour change to Off/Paired/Broken; `waiting_names_the_bot`
  still holds for the bot-present branch. 772 passing, baseline 771 held.
Task 4: complete (commits b6046ac..c7b263a, review clean after 1 fix round, 3 minors deferred)

Task 5: first implementer STALLED (watchdog, 600s) with the code complete but uncommitted and
  src/bridge.rs still untracked. Controller verified the uncommitted state: 787 passed / 0
  failed, fmt clean, clippy 0. NOTE: the verification run exceeded 600s and I initially
  suspected the bridge polling thread was blocking the suite — it was not; it was clippy plus
  a fresh compile of a new module. Corrected.
Task 5: finisher verified and committed as 9e551be. Reports: pairing match total/exhaustive,
  ownership only in `Bridge::owner` (Telegram-side latch confirmed still absent), a single
  Arc<Mutex<PhoneStatus>> slot used at both call sites, catch_unwind wrapping the whole thread
  body, backoff doubling capped at 5 min, `bot` populated via get_me() before polling starts,
  `Broken` prose structurally token-free (ChannelError carries no strings). Both required
  mutations caught (==→!= broke 6 tests; None-arm-drops-write broke 8). 10 adversarial tests
  added beyond the brief's 3. 787 passing.
Task 5: dispatched an independent security review on the stronger model (opus) — this is the
  boundary between a public bot username and the user's live shells, so it does not get the
  same review weight as ordinary application code.
Task 5: review — spec ❌, NOT APPROVED. Three Criticals, all at the lifecycle level around
  `accept` (which the reviewer independently confirmed correct: guard held across the whole
  match, exhaustive, poisoned lock preserves the owner, rejected strangers get total silence,
  Ruling 8 intact). The stronger model earned its cost here.
  C1: owner held only in memory, never persisted; daemon re-spawns a bridge every startup with
      owner:None, and a fresh Telegram starts at offset 0 so the first poll drains ~24h of
      backlog. A stranger who messages the public bot username while dct is down is paired on
      restart, and from Task 7 types into the user's terminals.
  C2: PhoneUnpair/PhoneDisable never reach the thread — no handle, no stop signal. Unpair
      leaves the old chat as the real owner while the UI claims otherwise; disable deletes the
      token from disk while the thread polls on with its in-memory copy.
  C3: PhoneSetToken spawns unconditionally alongside the startup spawn — two pollers with
      independent latches, so the owner pairs one and a stranger pairs the other.
  I1: none of C1-C3 is covered by a test; no concurrency test on accept; no panic-while-holding
      -the-lock test (session.rs has that shape for its own lock).
  I2: `broken_message_never_contains_anything_token_shaped` cannot fail — it scans const-derived
      literals. The real guarantee is structural (ChannelError carries no String).

**Ruling 9 (C1) — persist the owner chat id beside the token; open pairing only when no owner
is stored.** On startup with a stored owner, load it; do not re-enter pairing at all. Persisting
beats requiring an explicit re-pair because it also fixes the latch silently forgetting itself
across a restart, which is the deeper bug. Additionally, when pairing genuinely is open (fresh
token, no stored owner), discard Telegram's pre-existing backlog before accepting anyone —
otherwise the C1 attack survives on a fresh token.
Cost if wrong: the owner chat id becomes persisted state that must be cleared on unpair (covered
by the C2 fix) and migrated if the storage format changes; the alternative, an explicit re-pair
prompt after every restart, is safe but makes a restart feel broken.
C2+C3 fix directed structurally: one owned bridge handle with a stop signal; re-token stops the
existing bridge first; no path may leave an orphaned poller.
Task 5: fix round 1/5 dispatched to the finisher (it holds the bridge.rs context).
Task 5: fix round 1/5 (4 addressed, 1 partial, 3 new; commit d43b85e, 801 passing).
  C2, C3, I1, I2 fully closed — reviewer credited the 64-thread race test (exactly 1 Paired,
  63 Rejected) and the poll-count-freeze stop test as genuinely failing tests, not theatre.
  C1 partial: persistence, startup load, and skip-drain-when-owner-known all correct.
  New in the fix diff:
  F1 (Critical): `drain_backlog` terminates on the PARSED batch being empty, but parse_updates
      drops non-text updates while the offset still advances. 100 stickers then one text, sent
      while dct is down, makes batch 1 filter to empty → drain declares the backlog clear →
      the next poll hands the attacker's text to accept(). C1's impact restored via its own fix.
  F2 (Important): run()'s Ok(incoming) arm dispatches without rechecking stop, so a stopped
      thread can still pair someone up to 25s later — writing Paired into the shared slot and
      re-persisting __phone_owner__ after the handler deleted it.
  F3 (Important): startup_bridge_owner does `.parse().ok()`, so a corrupt owner field degrades
      to None and reopens pairing — what Ruling 9 forbids. Must distinguish "no owner stored"
      from "owner stored but unreadable".
Task 5: fix round 2/5 (3 addressed, 0 open; commit 60670e7, 804 passing).
  F1 ADDRESSED — new `Channel::drain` returns a raw update count (`result` array length, no
    text filter); drain_backlog terminates only on Ok(0). Reviewer traced the sticker attack
    concretely: the attacker's queued text is consumed by a drain batch and never parsed into
    an Incoming at all. Task 2's skip-non-text-during-poll rule left untouched. Offset
    bookkeeping verified across drain-then-poll — no update consumed by neither or delivered
    twice; a drain error leaves the offset unadvanced so the batch is re-fetched.
  F2 ADDRESSED — stop rechecked between poll() returning and dispatch; dispatch is the only
    caller of persist_owner and the only writer of Paired/owner.
  F3 ADDRESSED — StartupOwner::{None,Known,Corrupt}; the Corrupt arm returns before the channel
    is even built, so accept() is unreachable and cannot degrade to open pairing. Not a dead
    end: the Broken page offers Enter→set-token and x→disable, both of which clear the record.
Task 5: minor (deferred): post-stop `mark_broken` is not guarded by the stop recheck, so a
  dying thread can overwrite a PhoneDisable's `Off` with "token unusable". Cosmetic state
  confusion — no owner persisted, no Paired claimed.
Task 5: minor (deferred): ~8 duplicated lines of URL/offset logic between Telegram::drain and
  poll; reviewer agreed not worth factoring (the deliberate non-reuse of parse_updates is the
  entire point of the F1 fix).
Task 5: deferred note: drain_backlog has no iteration cap, so a continuously refilling backlog
  keeps pairing closed indefinitely. Denial-of-pairing, not escalation; runs on the bridge
  thread so the daemon never blocks; stop is checked every iteration. Pre-existing shape.
Task 5: complete (commits c7b263a..60670e7, review clean after 2 fix rounds, 3 minors deferred)

Task 6: implementer DONE, commit bdc5e9c. 812 passing (baseline 804 + 8), fmt + clippy clean.
  Both required mutations caught (deleting !first_input_empty fails both required tests;
  &&→|| fails 16). QUEUE_CAP = 32, a guessed constant with the same status as DEBOUNCE_WINDOW.
  Controller note: the agent appeared to end mid-verification, so I dispatched a finisher; the
  original then returned and committed. Finisher stopped via TaskStop before it could collide
  on the same files. Tree confirmed clean at bdc5e9c.

**Ruling 10 — the mpsc Receiver → Bridge wiring lands in Task 6.**
The implementer flagged that tick() produces events and Bridge can hold them, but nothing at
daemon runtime connects the two, so outbound notifications cannot reach a phone. This is a
PLAN DEFECT, not a scope error: Task 6's file list is session.rs + bridge.rs, Task 7 is
bridge.rs, Task 8 is bridge.rs + journal.rs — no task ever wires daemon.rs. Ruling: close it
here, because an outbound path that is not connected is not an outbound path, and deferring
means Tasks 7-11 build on a queue nothing drains.
Cost if wrong: daemon.rs churn lands in Task 6's commit rather than a later one; the
alternative (deferring) risks the gap being forgotten entirely, since no task claims it.
Fix round dispatched with the wiring, the no-live-bridge drop decision, lifecycle interaction
with stop_current/replace (no orphaned consumer), tests through the real wiring rather than
calling enqueue directly, and two mutations.
Task 6: wiring committed 463e10e, 814 passing (controller-verified; the agent kept ending while
  waiting on its own background suite run, so I ran it and handed back the numbers). Both
  wiring mutations caught.
Task 6: review — spec ✅, quality Approved, no Critical or Important. Rulings 4, 8 and 10 all
  verified applied. Reviewer credited a judgment call beyond the brief: notification was
  decoupled from the once-only auto-naming gate (`request_name`'s `name_attempted` no longer
  also gates notification), which a literal reading of the brief would have conflated.
  Drop-oldest confirmed genuinely oldest (pop_front before push_back); enqueue/queued public
  so Task 11 has a real hook. Panic safety confirmed: per-event work in catch_unwind, locks
  via recover(), send() failure ignored — nothing escapes to kill a session.
Task 6: minor (deferred): `events_are_dropped_without_blocking_when_no_bridge_is_live` uses a
  200ms sleep rather than condition-based waiting. Matches existing bridge.rs convention and
  the failure mode is self-diagnosing, but a processed-count wait would be strictly better.
Task 6: minor (deferred): `spawn_event_consumer`'s JoinHandle is fire-and-forget. Reviewer
  confirmed this is NOT a regression against Task 5's stop_current — that still synchronously
  stops the Bridge's own poll thread; only the always-alive router thread is unjoined.
Task 6: complete (commits 60670e7..463e10e, review clean, 2 minors deferred)

Task 7: implementer DONE, commit 7575b73. Controller-verified 824 passed / 0 failed (baseline
  814 + 10 new), fmt clean, clippy 0. The "background the suite, don't wait on it" instruction
  worked — first agent in a while to finish cleanly rather than dying mid-wait.
Task 7: review — spec ✅, quality Approved, no Critical or Important. `route()` is pure (no IO,
  no locks, no side effects, no new Bridge state). Rule 1 short-circuits via return inside the
  `if let`, so no fallthrough to a later rule is structurally possible, and `Gone` arises only
  from `map.get` returning None inside that same early return. Rules 4/5 are mutually exclusive
  on len(), so their relative order is semantically a no-op rather than a latent bug.
  All 3 named mutations map to distinct correctly-asserting tests (reviewer read the assertions
  rather than trusting the report). The 3 extra adversarial tests close the gap they were asked
  to close — a reordering that silently demotes rule 1 or Gone, which the named mutations alone
  would miss. No permutation of the five rules survives all 10 tests undetected.
Task 7: ⚠️ deferred to Task 8 — that `route()` is reachable ONLY downstream of `accept()`
  returning an owner verdict cannot be verified yet, because nothing calls it. Nothing in this
  diff violates the gate (route() takes no chat-id or ownership input at all). Carried into
  Task 8's dispatch as a must-verify.
Task 7: complete (commits 463e10e..7575b73, review clean, 0 minors)

Task 8: implementer DONE, commit 771ced2. 52 bridge:: + 7 journal tests pass targeted, fmt +
  clippy clean, both prescribed mutations caught. Full suite verification running controller-side.
  Controller removed a stray `full_test_task8.log` the agent left in the worktree root
  (untracked, never committed).
Task 8: OPEN ISSUE — deliver()/route() are implemented and tested but STILL NOT WIRED into
  dispatch(). The implementer's reasoning: RouteInput needs the message map (MsgId→session),
  the /use state, and the waiting list, and the map is only populated when outbound messages
  are actually sent, which is Task 9. Grep confirms nothing outside test code calls either
  function, so Task 7's carried security-ordering ⚠️ holds only TRIVIALLY — by absence, not by
  construction. This is the second wiring gap the plan does not assign to anyone (Ruling 10 was
  the first). Carry as a must-close into Tasks 9 and 10, and verify at Task 11 that a rejected
  stranger's message cannot reach route()/deliver() by construction rather than by nothing
  calling them. If no task claims it, rule it in before Task 11 closes.
Task 8: concern for review — phone-facing strings are Chinese-only, following the
  `broken_message` precedent rather than the bilingual i18n.rs system the global constraints
  require. Judgment call; flagged to the reviewer to adjudicate rather than pre-judged here.
Task 8: review — spec ✅, quality Approved, no Critical or Important. 836 passing (824 + 12).
  Routes verified by tracing: only `deliver_to` touches the writer, and it is structurally the
  only call site; Gone/Ask/NeedUse tests assert `spy.written().is_empty()` rather than just the
  returned variant. Ruling 7 resolved by reusing SessionInfo's existing tag-else-profile
  convention rather than inventing a second naming rule, falling back to "{id} 号会话" and never
  fabricating a name. Journal never carries message text (pinned). Both prescribed mutations
  caught, plus two permanent mutation-guard tests and separate pinning for Ask/NeedUse.
  Carried item A adjudicated: deferral is SOUND — wiring now with empty RouteInput state would
    make every genuine owner reply resolve to Gone/NeedUse, worse than not wiring. CONDITION for
    when it lands: the call must sit strictly inside the `if let Accepted::Paired(...)` arm of
    dispatch(), never on Rejected, with a test pinning that a rejected stranger cannot reach
    route()/deliver(). Reviewer names this as the one item the next task must land.
  Carried item B adjudicated: Chinese-only phone text is ACCEPTABLE, not a defect. A push
    channel has no per-request Lang to key off (unlike the daemon's client-facing i18n paths),
    the `broken_message` precedent in the same file already established this for this audience,
    and bilingual phone text is a contained 4-string follow-up.
Task 8: minor (deferred): the privacy test checks phone-facing strings for backticks/newlines —
  a heuristic, not a structural guarantee against a path-like session name reaching the wire.
  Low risk: names come from LLM titling, not filesystem paths.
Task 8: minor (deferred): deliver_to's "no writer wired" and "writer errored" paths produce
  nearly identical prose differing only in phrasing.
Task 8: complete (commits 7575b73..771ced2, review clean, 2 minors deferred)

Task 9: implementer DONE_WITH_CONCERNS, commit 1ef77df. merge/options_prompt/parse_options +
  9 tests, targeted bridge:: 63/63, fmt + clippy clean. Deferred the wiring to Task 10 (third
  deferral of the same seam), citing file scope: options_prompt needs live PTY screen text which
  is not in `Event`, and a Backend handle on Bridge, both requiring session.rs/daemon.rs changes.
  Also flagged `sighup_restores_the_terminal` as pre-existing flaky (real-PTY signal timing,
  passes standalone, untouched by the diff).

**Ruling 11 — integration becomes its own task, dispatched BEFORE Task 10's LLM work, with
daemon.rs and session.rs explicitly in scope.**
Controller grep confirms `route()` and `deliver()` are referenced ONLY from tests, and Task 9
added no outbound send loop, so the event queue fills and nothing ever sends. After nine tasks
the feature does not work in either direction: the components are individually correct and
well-tested, and no seam between them is owned by any task in the plan. Three unowned gaps:
  (a) outbound — drain the queue, merge, send via Channel::send, record the MsgId→session map;
  (b) inbound — inside the `Accepted::Paired` arm, build RouteInput, call route(), call deliver();
  (c) `/use` and `/ls` parsing, which Task 7's rules 2 and 5 depend on and which NO task assigns.
Ruling: stop deferring. Integration is its own dispatch with expanded file scope, and it must
satisfy the Task 8 reviewer's condition exactly — the route/deliver call sits strictly inside
`if let Accepted::Paired(...)`, never on `Rejected`, with a test pinning that a rejected stranger
cannot reach route() or deliver().
Cost if wrong: a larger, harder-to-review diff than one task's worth, touching three files.
The alternative is worse and nearly happened: Task 11's step 6 is the only check that would
catch a disconnected feature, and it CANNOT RUN (no bot token per the user's instruction), so
this would have shipped as ~845 green tests over a feature that does nothing.

Task 9: complete (commits 771ced2..1ef77df, 845 passing) — pending review alongside integration.
Integration: implementer DONE, commit 35e5efc, 852/853 passing (only the pre-existing
  `sighup_restores_the_terminal` real-PTY timing flake failed). All three required mutations
  performed and confirmed to fail their tests before revert. Notably it reported having to
  STRENGTHEN its own first-draft no-orphan test because the mutation initially survived it.
  Controller verified the security structure directly at src/bridge.rs:401-407:
    Accepted::Paired    => route_and_deliver(msg)   (the pairing message's own content is
                                                     still processed, deliberately)
    Accepted::FromOwner => route_and_deliver(msg)
    Accepted::Rejected  => {}   <- empty arm, with a comment stating nothing calling
                                   route/deliver may ever be added here
  That satisfies the Task 8 reviewer's condition by construction rather than by absence.
Integration: concern — a merged multi-session push cannot be replied to directly by design; it
  falls back to Route::Gone / "check /ls". Reasonable, but it means the merge feature and the
  long-press-reply feature are mutually exclusive for that message. Flag to review.
Integration: concern — the `/use` selection and the MsgId→session map are process-lifetime only
  and lost on daemon restart. Consistent with the existing Route::Gone semantics (a reply to a
  pre-restart message types nothing), so it degrades safely rather than wrongly.

Integration + Task 9: review (opus) — spec ❌, NOT APPROVED. One Critical.
  C1: `SessionWriter::type_into` (bridge.rs:200-202) calls send_input(id, text) and stops, but
      send_input only WRITES THE BYTES — pressing Enter is a separate call with an empty string
      (ui/grid.rs:683-685 does exactly that, with a comment that the steps must not be merged).
      So a phone reply lands in the agent's input buffer and sits there: no \r, no Working
      transition, no checkpoint, nothing runs. Meanwhile the phone gets "已经敲进「name」" and the
      journal records Typed(id) — both asserting a delivery that did not happen, to the one user
      who cannot glance at the terminal to notice. The new e2e test asserts only that characters
      appear on screen, which a typed-but-unsubmitted buffer passes. This is the feature's entire
      purpose failing silently behind 852 green tests.
  I1: only single-event batches enter outbound_map, so replying to a MERGED push yields
      Route::Gone — "这条消息对应的会话已经不在了" — while both sessions are alive and idle.
      Route::Ask exists for "several candidates, don't guess"; Gone is the wrong answer, not just
      a limitation. Worst in exactly the case where routing matters most.
  Everything the review was asked to attack came back CLEAN: no second entry point to
  route_and_deliver/route/deliver/send_input (grep over production lines); /use and /ls handled
  inside route_and_deliver, i.e. behind accept(); no pairing window (accept latches the owner
  under the lock before returning Paired); persist-before-slot is the correct order; lock order
  outbound→owner→outbound_map has no cycle; both threads catch_unwind-wrapped sharing one stop
  flag so replace/stop_current orphan nothing; map is Mutex<VecDeque> bounded MSG_MAP_CAP=256
  drop-oldest; unknown id → Gone with a mutation guard pinning it; post-restart reply → Gone,
  never a current-session fallback; merge has no llm reference at all.
  Reviewer also verified the strengthened no-orphan test genuinely fails under mutation by
  reading it, and judged the other two mutation-verified tests equally solid (they pin behaviour
  through public paths, not internal state).
Integration: fix round 1/5 dispatched — C1 (write + Enter, fail if either half fails, e2e
  asserts the session actually leaves Idle, mutation-test by dropping Enter), I1 (record every
  session a merged push covers and answer Ask), plus four small-but-not-cosmetic items:
  /ls@botname currently gets TYPED INTO THE TERMINAL; /use prefix swallows /user…; reply() holds
  the owner mutex across a 5s send; no stop check between drain_outbound and ch.send; and
  session.rs:1100-1103 falls back to dir.display(), putting a full path on the wire.
Integration: minor (deferred to final review): `deliver`/`enqueue` are pub with no callers
  outside the module — a pub unauthenticated delivery entry point is the shape the guarded arm
  defends against; BridgeHandle::accept is pub and latches ownership as a side effect.
Integration: minor (deferred): the new daemon e2e test spawns a real PTY with 3s/2s deadlines,
  same class as the known sighup flake — expect occasional load-dependent failures.
Task 9: OPEN — options_prompt/parse_options have no production caller yet; the async
  ask-the-model-with-fallback half is deferred to Task 10. Must not be lost before Task 11.
