# SDD ledger — plan: docs/superpowers/plans/2026-08-10-dct-phone-channel.md

BASE for Task 1 = d8ec739
Task 1: complete (commit d8ec739..1ff7575, review clean — spec ✅, quality Approved).
  Implementer caught a plan error: Step 3's reference code has `pub mod telegram;`, but
  telegram.rs is TASK 2's deliverable — copying it verbatim makes Task 1 fail to build on its own,
  contradicting its "no dependencies" premise. Correctly omitted.

CARRY-FORWARD TO TASK 2 (this line is the authoritative channel; the doc comment in mod.rs is not):
  Task 2 MUST add `pub mod telegram;` to src/channel/mod.rs. Its own brief never mentions touching
  mod.rs, so without this line nothing in the plan tells it to.
  Also: Task 2's Step 2 expects "compile error naming parse_updates". If the module is not declared
  the actual result is "0 tests matched" — which reads like an unremarkable green, not a red flag.
  Tell the implementer to expect that and treat it as the same signal.

CARRY-FORWARD TO TASK 6 (review finding, Important/plausible — a design gap in MY plan):
  debounce(last: Option<Duration>, now: Duration, window) works, but Task 6's real caller holds a
  per-session Instant, not a Duration sharing an origin. The plan shows no call-site code, so the
  conversion is undocumented. The danger: an implementer reaching for a wall-clock epoch
  (SystemTime::now().duration_since(UNIX_EPOCH)) reintroduces a NON-MONOTONIC-CLOCK bug into the
  debounce decision — a clock step backwards would suppress a real event, or unsuppress a burst.
  Task 6 must derive both Durations from a single monotonic origin (an Instant captured once per
  SessionManager, .elapsed() against it), and say so in its report.
Task 2: initial (commits 1ff7575..80f496c). Implementer caught TWO more plan errors:
  (a) the `Send` type alias does not compile — an explicit module item shadows the prelude, so
      `Send` in its own bound list resolves to the alias being defined (E0404). Renamed to `Sender`
      per llm/http.rs. Reviewer confirmed independently.
  (b) NEITHER the brief NOR Task 1's interface list ever says how Channel::send(&str) learns its
      destination chat_id. My interface gap. It invented first-write-wins-from-poll.
Task 2: review 1 — spec ✅, quality NOT APPROVED. 1 Critical + 4 Important + minors.
  CRITICAL: the invented destination tracker duplicates state Task 5's Bridge owns, and they can
  disagree in two reachable ways:
    - re-pair: Bridge.owner <- None but Telegram.chat_id has NO reset path (private field, no
      setter, its only writer refuses to overwrite). Outbound keeps going to the old account while
      send() returns Ok and the phone page reads Paired. The unpaired person keeps receiving
      session names, project names and agent output.
    - new token (THE SECURITY ONE): Telegram learns from the RAW poll stream, before authorisation
      (learning is inside poll(); Bridge.accept() is a separate call on the returned Vec). A
      stranger who found the public bot username and messaged first captures OUTBOUND permanently.
      Bridge correctly rejects their inbound — the security test still passes — so the one check
      the feature rests on guards one direction while its duplicate opens the other.
  Fixed now, not deferred, for three reasons: the fix is ~15 lines and touches only Task 2; there
  is currently a GREEN test pinning the duplicate tracker whose doc comment calls it equivalent to
  bridge.rs's pairing_happens_exactly_once, so reconciling later means deleting a test that reads
  like a security test; and Task 5's header already calls Bridge "唯一有状态的地方".
Task 2: fix round 1/5 dispatched (C1 + I1-I4 + 2 minors).

CARRY-FORWARD TO TASK 5: Channel gains `fn set_destination(&self, chat: Option<i64>)`. Bridge must
  call it where it already decides ownership: Accepted::Paired(id) -> Some(id); unpair/re-pair ->
  None. Telegram no longer learns anything from the poll stream.
CARRY-FORWARD TO TASK 4: 403 -> BadToken is WRONG for the most common 403, `Forbidden: bot was
  blocked by the user` (which is literally the test fixture). Telling that user to re-enter a
  perfectly good token fixes nothing. Task 4's PhoneState::Broken needs its own string for
  "the bot was blocked", distinct from "the token is bad". My brief mandated the bad mapping.
Task 2: fix round 1/5 (C1 + I1-I4 + minors all ADDRESSED by scoped re-review; commits
  80f496c..22fbe60). Reviewer verified by reading: every write to the destination field is now the
  constructor and set_destination, nothing else; send re-reads per call so a clear really clears;
  reinterpret can never rewrite a BadToken (that value only comes from a parsed ok:false body, so
  it is never Malformed); &self confirmed as the only workable signature under Arc<dyn Channel>.
Task 2: fix round 2/5 dispatched — one Important that is small but guards the Critical:
  set_destination_can_be_cleared does NOT discriminate. Mutating set_destination back to
  first-write-wins keeps it green, because the exhausted FakeTransport returns Err and send maps
  ANY transport Err to Unreachable — the exact value the assertion expects. So the reset path has
  zero real coverage, and the Critical could be resurrected under a test named for preventing it.
  Fix is one line: assert the fake was never called.
CARRY-FORWARD TO TASK 5: send() snapshots the destination and releases the guard before the
  network call, so the guarantee is "no NEW sends after the reset", not "no sends to the old chat
  after the reset". One message can still land on the old chat after unpair. Holding the mutex
  across a 10s request would be worse; do not try to close it that way.
CARRY-FORWARD TO TASK 4: get_me is inherent on Telegram, not on the Channel trait. If Task 4 holds
  Arc<dyn Channel> rather than a concrete Telegram it cannot reach it, and the trait needs it.
Task 2: fix round 2/5 (all ADDRESSED, 0 open; commit 22fbe60..1a5344c). Reviewer walked the
  first-write-wins mutation itself: FakeTransport::sender pushes to `calls` BEFORE checking the
  replies queue, so the mutant records a call and the new assertion goes red; under the correct
  implementation send short-circuits before any transport call, so the assertion holds with no
  false-red. The staleness comment states the limit accurately and explicitly disclaims the
  stronger reading.
Task 2: complete (commits 1ff7575..1a5344c, review clean) — 24 adapter tests, 726 lib tests.
BASE for Task 3 = 1a5344c
Task 3: initial (commit 1a5344c..badc974). Implementer found a bug the brief never mentioned:
  open_settings() preselected the cursor with the LANGUAGE's index into Lang::all(), which after
  the refactor indexes SettingsItem::all(). Lang::all() is [En, Zh], so a Chinese-locale user —
  this repo's DOCUMENTED DEFAULT — got Some(1) = SettingsItem::Phone. Silently wrong, no panic,
  would have shipped to the majority of users. Fixed with a mutation-verified regression test.
  It also disclosed honestly that one brief-specified mutation is structurally uncatchable
  (SettingsItem::all().len() == Lang::all().len() == 2 by coincidence) instead of claiming a catch.
Task 3: review 1 — spec ✅, quality Approved. 1 Important + 3 Minor. Reviewer confirmed the
  Option<ListState> shape matches this codebase's governing precedent (PickProject/typing_path and
  Grid/reply, both Option-encoded second levels with guarded back_one_level + escape_hint arms).
  EnterSecret is NOT the precedent — its return_to_settings encodes provenance, not depth.
  MY ERROR, corrected by the reviewer: I told the implementer the length coincidence would end
  when Task 4 lands. It will not. Task 4 adds View::Phone (a page), not a third settings row, so
  SettingsItem::all().len() stays 2 and the blind spot outlives Task 4 unnoticed. Asked for a
  tripwire test instead of inventing a third enum entry to serve a test.
Task 3: fix round 1/5 dispatched (idle_help level-awareness + tripwire + i18n asymmetry + the
  missing escape_hint_cols entry).

*** MERGE GATE — DO NOT MERGE THIS BRANCH TO main WITHOUT TASK 4 ***
  The phone row is a visible, selectable dead end: Enter does nothing at all while the bottom bar
  advertises `Enter 确认`. view.rs:1154 states the rule it breaks —
  「屏幕上写着做不到的操作比不写更糟」. Correct for Task 3 in isolation, fine as an intermediate
  commit, unacceptable on main. Task 4 (View::Phone) closes it.
Task 3: fix round 1/5 (all ADDRESSED, 0 open; commit c15668e..adacd36). Reviewer verified arm
  ordering specifically (a guarded arm after the general one would be dead code that compiles),
  confirmed the new test discriminates (deleting the arm makes Key::Cancel fire, failing both
  assertions), and confirmed the tripwire's failure message says what to do next rather than just
  "these differ now".
Task 3: complete (commits 1a5344c..adacd36, review clean) — 740 lib tests.
BASE for Task 4 = adacd36
Task 4: first dispatch died on an API connection error mid-run. State when it died: commit 5754a82
  (protocol + token storage + daemon-side state) landed; src/ui/phone.rs untracked; app.rs and
  view.rs uncommitted; TREE DID NOT COMPILE (View::Phone draw arm was a todo!() at mod.rs:2048).
  Resumed the same agent from its transcript rather than re-dispatching — same handling as the
  Fix 3 interruption in the previous wave.
Task 4: initial (commits adacd36..12076c3). Review 1 — spec ❌, quality NOT APPROVED.
  3 Critical + 2 Important + 2 Minor. The three Criticals are one user's first three steps:
  C1 PASTING THE TOKEN DOES NOTHING, silently. Event::Paste (ui/mod.rs:715-750) has no
     View::Phone arm, and bracketed paste means Cmd+V never produces Char events. A bot token is
     46 chars copied from BotFather — nobody types it. The precedent is three lines above with the
     comment 「密钥十有八九是粘进来的，不是敲的」.
  C2 FOLLOWING THE ON-SCREEN INSTRUCTION LANDS ON "@?". Broken always carries bot: None, and
     PhoneUnpair leaves bot untouched, so Broken + `r` renders 「给 @? 发条消息」 — and `r` is what
     the Broken next-step tells the user to press. Deterministic. waiting_names_the_bot only ever
     passes Some(bot); the one unpair test starts from Paired, which cannot occur in production.
     Second half: a bad token is deliberately not saved, so `r` fabricates WaitingForPairing with
     NO TOKEN ON DISK; restart silently reports Off; the only way out is guessing `x`.
  C3 the two new integration tests pre-seed PHONE_TOKEN_KEY, so daemon startup fires
     spawn_phone_startup_refresh against the REAL api.telegram.org — breaks "tests touch no
     network", and is the SAME SHAPE this branch already fixed: assert a state while a background
     thread races to overwrite it. The claim that they follow three pre-existing tests is half
     true — start_daemon_for_test is pre-existing, but no existing test seeds a phone token.
  ROOT CAUSE tying the spec ❌ and I1 together: PhoneState::Broken(String) carries only prose, so
     next_step has nothing machine-readable to branch on. The implementer's reasoning for declining
     the 403 requirement was correct (error_from collapses 401/403 into a payload-free BadToken) —
     but it points at the fix, not at accepting the merge. Carry a cause alongside the sentence.
  I1: the report claims the only reachable Broken is "token invalid". WRONG — anyone offline or
     behind a proxy who types a PERFECTLY GOOD token gets Broken(phone_unreachable) and is told to
     re-type it or unblock a bot they never blocked.
  I2: PhoneState::Paired is never constructed on this branch but looks finished (i18n strings,
     idle_help branch, tests, and the only variant with no doc comment).
Task 4: fix round 1/5 dispatched.
Task 4: fix round 1 — the implementer died THREE times (two API connection errors, then a 600s
  stall). Context had reached ~500k tokens. Not resumable; the work was recovered instead.
  CONTROLLER INTERVENTION, recorded because it deviates from the usual rule that the coordinator
  never touches the code:
   (a) A MUTATION WAS LEFT UNREVERTED in the working tree when it stalled mid-sweep:
       daemon.rs's Err(ChannelError::Blocked) arm carried reason: PhoneBrokenReason::BadToken
       while its message was phone_blocked. The test caught it correctly
       (phone_verify_token_marks_blocked_with_its_own_reason). I reverted it to BotBlocked.
       Left alone, the next implementer would most likely have "fixed" the failing TEST instead of
       the code — the mutation looks like a plausible mapping.
   (b) I committed the recovered work as e1694c7 rather than leaving it uncommitted for a fourth
       time. Three near-losses outweighed process purity here.
  Tree state at handoff: 790 lib tests pass, 0 failed; cargo check clean.
  What landed and looks right (verified by reading, not by report — there is no round-1 report):
   - paste_into_view extracted with a View::Phone arm (Critical 1)
   - Broken { reason: PhoneBrokenReason, message: String } — typed reason for branching, opaque
     message for display. BETTER than what I asked for: next_step reads only the closed enum, so
     defence in depth means even a poisoned message cannot reach the two most prominent lines.
   - error_from now splits 401 -> BadToken from 403 -> Blocked (Task 2's file, same branch)
   - PhoneState::Paired documented as unconstructed-on-this-branch (Important 2)
   - has-token-on-disk gating for r/x (Critical 2, second half)
  STILL UNVERIFIED — the next implementer owns these:
   - Critical 2 first half: is there a test starting from a REACHABLE Broken proving `r` no longer
     renders 「给 @? 发条消息」? The old coverage was blind (waiting_names_the_bot only passes
     Some(bot); the unpair test starts from Paired, which cannot occur in production).
   - Critical 3: do the two integration tests still reach api.telegram.org? Which of the three
     routes was taken? Do they pass with the network unplugged?
   - spawn_phone_startup_refresh: not in the brief, still unjustified.
   - The full mutation sweep was never completed.
   - No round-1 report exists.
Task 4: my "STILL UNVERIFIED" list above was STALE, not wrong — a fresh implementer verified that
  the agent which died three times had in fact finished all four items before stalling; it just
  never wrote them up. Verification pass: no source changes, 18 mutations run and reverted, git
  diff clean, and network-freedom shown DYNAMICALLY (proxies pointed at an unroutable address,
  both integration tests finish in 0.08s). spawn_phone_startup_refresh was REMOVED, not gated.
Task 4: scoped re-review of the recovered fix (commit 12076c3..e1694c7) — all 3 Criticals, the
  spec failure and both Importants ADDRESSED. Reviewer independently re-derived that
  Broken{BotBlocked} is unreachable today, and confirmed no path from daemon startup or either
  test reaches a real network call (the only live call site is PhoneSetToken -> get_me, and no
  test presses Enter in an open entry).
Task 4: fix round 2 dispatched — 2 Important + 3 Minor found in the fix diff itself:
  I1: THE FIX'S CENTRAL BEHAVIOUR HAS NO TEST. Three reasons producing three different next steps
      is the whole point of the round, and collapsing all three arms to PhoneNextStepBadToken
      leaves the ENTIRE SUITE GREEN — so "offline user told to re-type a perfectly good token"
      can regress silently.
  I2: apply_phone_set_token skips saving for EVERY Broken including BotBlocked, while
      has_confirmed_token() asserts BotBlocked means a token IS on disk. The only thing keeping
      this from being Critical 2 all over again is that Telegram happens never to answer getMe
      with 403 — an EXTERNAL SERVER'S BEHAVIOUR, not an invariant in our code. The comment at
      daemon.rs:481 also describes a world that does not exist (claims BotBlocked carries a
      previously-confirmed bot name; its only producer returns bot: None).
CARRY-FORWARD TO TASK 5: nothing re-verifies a saved token after a restart now that the startup
  refresh is gone. A revoked token shows WaitingForPairing forever, and since Enter is suppressed
  whenever has_confirmed_token(), the only way to a new token is `x` then `Enter`. Honest while no
  bridge exists (pairing never completes anyway); Task 5 must revisit.
Task 4: fix round 2/5 (all ADDRESSED, 0 open; commit 4cd7ef6..45cc5e0). The implementer's solution
  to I2 was better than either option I offered: map Blocked -> BadToken on the SET-TOKEN path,
  because that path never has a real bot name to save and getMe carries no chat context, so
  BotBlocked would not be a truthful answer there. Reviewer's summary of why it is right: it keeps
  has_confirmed_token()'s promise TRUE BY CONSTRUCTION rather than TRUE BY LUCK.
  Reviewer also verified the disclosed `git checkout --` incident lost nothing (test counts per
  file are consistent with N-1 existing + 1 new), and that the new shape pin is a real hardcoded-
  JSON pin rather than a self-consistent round-trip.
Task 4: complete (commits adacd36..cd4a6cb, review clean) — 793 lib tests.
  Cost note: this task burned ~900k subagent tokens across three agents, one of which died three
  times. The recovery pattern that worked: commit the tree first, then hand a FRESH agent the
  brief + report + an explicit list of what is unverified.
  Process lesson from the implementer, worth repeating in later tasks: it lost a real test to
  `git checkout --` while reverting a mutation, and switched to committing real changes BEFORE
  each mutate-and-revert cycle. Do that from the start.
BASE for Task 5 = cd4a6cb
Task 5: initial (commits cd4a6cb..360f8e8; the first three landed before the first dispatch was
  interrupted, the rest after re-dispatch). Implementer found a REAL SELF-DEADLOCK in the
  inherited wiring: `if let Some(b) = recover(slot.lock()).clone() { start_phone_bridge(..) }`
  extends the scrutinee's MutexGuard across the block and the callee re-locks the same mutex.
  No existing test reached that branch (all used an empty slot), so it shipped silently until new
  coverage hung cargo test for 7+ minutes.
Task 5: review 1 — spec ✅, quality NOT APPROVED. 2 Critical + 3 Important + 4 Minor.
  The deliverable itself (accept, rejection tests, set_destination monopoly) was verified by
  TRACING every path, not by reading the tests, and it holds. What fails is the lifecycle wiring.
  C1 *** PAIRING DOES NOT SURVIVE A DAEMON RESTART, AND THE REOPENED WINDOW IS RETROACTIVE ***
     owner lives only in memory and is never persisted, so every daemon start builds a fresh
     Bridge with owner: None -> pairing reopens with no user action and no announcement.
     Worse: Telegram::with_transport starts offset at 0, so getUpdates?offset=0 returns the whole
     ~24h unconfirmed backlog OLDEST FIRST. On the first poll after a restart, THE OLDEST
     UNCONSUMED MESSAGE WINS THE PAIRING — including one a stranger sent hours earlier while the
     machine was asleep. The stranger does not have to be present, or fast, or lucky.
     This contradicts the rationale written into pairing_happens_exactly_once itself
     (「配对必须是用户填完令牌后的一次显式动作，而不是长期开着的门」). Reviewer's phrasing:
     "a door that reopens on every boot and swings backwards in time."
     Fix: persist the owner chat id next to the token AND discard the backlog on a fresh Bridge.
  C2 Paired/BotBlocked can be written AFTER the token is deleted. "By construction" holds for
     creation ordering, not against concurrent deletion: send_pairing_confirmation does a 10s
     network send then writes Broken{BotBlocked} without re-checking is_retired(); and
     poll_forever checks is_retired() BEFORE blocking on phone.lock(), which PhoneDisable holds
     while it retires — so the guard releases and the thread writes Paired over Off.
     daemon.rs:643-649's comment promises exactly this cannot happen. Holding the phone guard
     across retire() LINES THEM UP rather than separating them.
  I1: PhoneSetToken replaces the slot without retiring the old Bridge. Survives only on an
      unwritten three-file argument enforced by the UI, not the daemon.
  I2: PhoneStatus.owner is never populated, so a wrong pairing is INVISIBLE — which is what makes
      C1 undetectable in practice. msg::phone_paired now has no producer at all.
  I3: a panic in the poll thread leaves the page asserting a promise nothing keeps (stderr goes
      to /dev/null when the TUI spawns the daemon).
Task 5: fix round 1/5 dispatched.
Task 5: fix round 1/5 (both Criticals + all 3 Importants ADDRESSED; commits 360f8e8..badd725).
  Two implementer judgement calls UPHELD by the reviewer: seating owner + calling set_destination
  inside the constructor genuinely closes the pre-thread window; and its STRONGER ordering for
  PhoneDisable (retire strictly before any phone lock) is the version that actually closes the
  race — the literal "drop the guard first" I suggested would have left a window.
Task 5: fix round 2/5 dispatched. The fix INTRODUCED a new Critical, and it is the subtlest bug
  found in either plan so far:
  *** discard_backlog stops at the first batch containing no TEXT, not the first empty batch. ***
    parse_updates silently skips non-text updates while max_update_id still advances the cursor,
    so an empty Vec<Incoming> means "this batch had no text", NOT "Telegram has nothing left".
    getUpdates has no limit and Telegram caps a batch at 100. So: SEND 100 STICKERS, THEN ONE TEXT.
    The drain sees [] and declares itself caught up; the main loop's first long poll returns
    update 101 and pairs the attacker. Needs nothing but the public bot username.
    THE UNIT TESTS CANNOT SEE IT: FakeChannel scripts Vec<Incoming> directly, so it can never
    produce "non-empty batch that parses to empty" — the fake collapses exactly the seam the bug
    lives in (transport's batch vs parser's message).
  I: the new on_owner_changed persistence is NOT retire-gated, so a retired bridge can write a
     stale chat id back to disk after it was cleared -> next start seats it as owner of the NEW
     token -> the legitimate user is Rejected forever while the UI says Paired.
  I: the test-network regression is worse than reported. The implementer said the bogus token
     fails fast with a terminal 401; Telegram actually answers 404, and error_from maps non-401/403
     to Unreachable, which IS worth_retrying — so each test leaves a thread retrying api.telegram.org
     with backoff for the life of the binary. Reviewer found a TWO-LINE test-side fix the
     implementer missed: neither test needs a token at all, seed only PHONE_BOT_KEY.
CARRY-FORWARD (track as real work, not a report line): phone_paired renders a raw chat id to a
  non-programmer (「通知发给 6285551234」). Needs from.first_name out of parse_updates.
Task 5: fix round 2/5 — the new Critical, both Importants and all four small items ADDRESSED
  (commits badd725..c321d00). Carried by THREE implementers: two were cut off by API errors and I
  committed their recovered trees (468b96c, 006d847) in between.
  MY FAILURE, recorded because it cost the record rather than the code: 468b96c bundles three
  separately-findable fixes (the Batch/raw_len change, the accept/unpair retire gate, and half the
  settings-view test fix) and 006d847 carries the other half — because I committed recovered work
  without having anyone write up what was in it. THE REPORT THEREFORE HAS NO MUTATION-TESTING
  RECORD FOR THE CRITICAL OR THE RETIRE GATE. The reviewer verified discrimination for each by
  reasoning through what the pre-fix code would do, and the evidence is in that review, not in the
  report. Next time a recovered tree is committed, dispatch the write-up with it.
  Reviewer's verification worth keeping: the fake CAN now express the seam the bug lived in
  (batch_with_raw_len builds messages: vec![] with raw_len: n), and the seam is pinned one layer
  down too — the_cursor_advances_past_a_text_less_update_too now asserts raw_len == 1 with empty
  messages against hardcoded Telegram JSON, so raw_update_count cannot silently collapse back onto
  the parser. The ordering pin is fail-closed by structure, and its assert releases the spawned
  thread on unwind (recover tolerates the poisoning), so a failure is ~2s red rather than a hang.
Task 5: fix round 3/5 dispatched — 2 Minors, one of them introduced by our own fix:
  the drain's termination now depends on the cursor advancing and the loop never checks it, so a
  batch with raw_len > 0 but no parsable update_id spins a tight, un-backed-off timeout=0 loop
  against Telegram until the user presses x. We traded a security hole for a hot loop.
  Second: the retire gate is asymmetric inside accept() — the disk write is gated but
  set_destination and send_pairing_confirmation are not, so a stranger can still receive the
  「已配对」 message in the same window. No durable harm; the file just now needs reading twice.
Task 5: fix round 3/5 (both Minors ADDRESSED, 0 open; commit c321d00..c951e0c).
  The busy-spin fix was better than the one-comparison guard I asked for: raw_len > 0 with a
  stuck cursor is now classified as a MALFORMED reply and goes through record_terminal_error, so
  the user sees an actionable line on the phone page instead of a silent hot loop. Reviewer
  confirmed cursor_advanced is written in the same branch that assigns offset, from the same
  max_update_id value, so it cannot disagree with whether the cursor really moved.
Task 5: complete (commits cd4a6cb..c951e0c, review clean) — 846 lib tests.
  DEFERRED (coverage blind spot, not a defect): the report claims FakeChannel::send mirrors
  Telegram's "no destination => Unreachable". It does not — it records unconditionally and returns
  Ok(0). So the full poll_forever -> accept -> send_pairing_confirmation path under retirement has
  no test; the new test asserts from the destinations side instead. Fine today, worth closing when
  someone next touches the fake.
BASE for Task 6 = c951e0c
