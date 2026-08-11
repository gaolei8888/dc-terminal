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
