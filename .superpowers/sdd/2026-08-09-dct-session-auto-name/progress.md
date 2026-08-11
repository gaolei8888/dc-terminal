# SDD ledger — plan: docs/superpowers/plans/2026-08-09-dct-session-auto-name.md

Branch: `feat/session-auto-name`, branched from `5da4ec4` (main).

Task 1: implementer DONE_WITH_CONCERNS at `bd126fd` — fix correct, but the brief's
test fixture could not fail (focus sat on the last tile, where clamping and identity
anchoring coincide). Controller ruling: the brief's intent governs, its fixture is a
defect. Fix round 1 dispatched — 4-session fixture with focus in the middle, plus a
mutation check on the `unwrap_or(clamped)` fallback.


Task 1: fix round 1/5 (1 addressed, 0 open — always-passing fixture replaced with a
4-session/middle-focus one, proven red before green; commit amended to 574d841)
Task 1: minor (deferred): `refresh_rows` doc comment (src/ui/app.rs:270, pre-existing)
still describes only the list cursor, not the grid focus that is now also identity-anchored.
Task 1: complete (commits 5da4ec4..574d841, review clean — spec ✅, quality approved)

Task 2: implementer DONE_WITH_CONCERNS at 7bf31dd. Review: spec ✅, quality 1 Critical —
amending `the_session_info_shape_is_pinned_too` with `,"tag":""` silently defeats the test's
purpose (it exists to force a deliberate version-bump decision; `proto.rs:767-770` names this
exact move as "2026-08-05 那次事故的形状"). Controller ruling: the no-bump decision stands
(additive serde(default) read-only field is compatible both ways), but it must be made
explicit — carve-out comment at the test + a line in the proto changelog. Fix round 1 dispatched.
Task 2: fix round 1/5 (1 addressed, 0 open — carve-out comment states the non-generalization
rule, changelog records the no-bump entry under 6; commits 7bf31dd..5eafdc1)
Task 2: complete (commits 574d841..5eafdc1, review clean — PROTOCOL_VERSION still 6)

NOTE FOR FINISH: this repo KEEPS its SDD workspaces (see commit 2615e10). Do NOT `rm -rf`
the workspace at the end — `git add -f` it instead; `.superpowers/sdd/.gitignore` is `*`.
Task 3: complete (commits 5eafdc1..cdf0621, review clean — spec ✅, quality approved)
Task 3: minor (deferred): `a_pasted_wall_of_text_is_capped` is pure ASCII, so it cannot tell a
byte-count cap bug from a char-count one; a multibyte-paste-past-cap test would close it.
Task 3: minor (deferred): `append_capped` recomputes `buf.chars().count()` per iteration (O(n^2),
harmless at a 200-char cap).
Task 3: ⚠️ resolved by controller: the `is_agent` gate in `send_input` is verified only by code
reading, no test. Ruling: not a gap worth the fix loop — Task 5's trigger gates on `is_agent`
independently, so a broken gate here would only accumulate an unused buffer on shell sessions.
Left for the final review to triage.

Task 4: implementer DONE_WITH_CONCERNS at 4cdcda2 — found and fixed a real bug in the brief's
own `clean_name` reference code (two-pass scrub left a stray `」` and failed the brief's own
first assertion). Review: spec ✅, 2 Important — (a) the `NAME_MAX_CHARS` comment falsely claims
24 chars = 12 Chinese characters; (b) merging the trim sets stripped leading punctuation, so
`.NET 迁移` -> `NET 迁移`. Fix round 1 dispatched: correct the comment, split the trim
asymmetrically (front = quotes+ws, end = quotes+punct+ws), pin `.NET 迁移`.
Task 4: fix round 1/5 (2 addressed, 0 open — comment now truthful, trim split asymmetrically,
`.NET 迁移` pinned; commits 4cdcda2..a8ae456)
Task 4: complete (commits cdf0621..a8ae456, review clean)
Task 4: minor (deferred): `ui::tests::entering_a_session_always_lands_at_the_bottom_even_without_a_resize`
failed once in a full-suite run and passed in isolation and on rerun. Pre-existing, untouched by
this branch — but a flaky test in this repo is worth the final review's attention.

Task 5: implementer DONE at 3c64bad. It found that the brief's three tests could never pass
(`fake_agent()` runs `cat`, which never emits the "READY" the idle_pattern needs) and that the
brief's mutation #3 turned nothing red; it added `finishing_agent()` and a fourth test.
Review (opus): spec ✅, 1 Critical + 1 Important + 1 Minor.
  CRITICAL (verified by controller): no shipped profile declares `idle_pattern` — all six agent
  profiles carry `busy_pattern` only. `classify()` with busy_re Some returns Idle when the busy
  pattern is absent, i.e. on a freshly booted splash screen. So the first tick after create is a
  Working->Idle transition and names every session with an empty string, permanently. The brief's
  four trigger conditions are the root cause.
  Fix dispatched: add `&& !s.first_input.is_empty()` (NOT `first_input_sealed` — the spec ruled
  sealing must not gate naming), plus a test using a busy_pattern-only profile like the real ones.
  IMPORTANT: `recovering_from_a_failure...` flakes ~2-5% (separate `clear` and `echo` writes).
Task 5: minor (deferred): `matches!(next, Idle | Asking)` — `SessionState::Asking` is never
assigned anywhere in the daemon; `classify()` cannot return it. Harmless forward-compat, but no
comment should imply it happens today.
Task 5: fix round 1/5 (2 addressed, 1 new open — commits 3c64bad..19ba916). The Critical fix is
verified genuinely tested (busy_pattern-only fixture traced, fails without the guard) and the
flake is gone. New open item, self-reported by the implementer and confirmed by re-review:
`was == SessionState::Working` is now isolated by no test, yet still load-bearing — a session
that already has a first line, goes Failed on an API error, then has the error scroll off, would
burn its once-per-session slot on an error screen. Fix round 2 dispatched with the isolating
fixture (discriminator must be the screen, not first_input).
Task 5: fix round 2 agent stalled on a watchdog AFTER writing the isolating test but BEFORE
committing. Work was intact in the working tree (`recovering_from_a_failure_after_real_input_
still_does_not_count`, src/session.rs:1573). Resumed with a narrowed instruction: one scoped
test run, the mutation check, commit. Lesson: the long full-suite runs are what trips the
watchdog on this task.
Task 5: fix round 2/5 (1 addressed, 0 open — isolating test added, mutation red confirmed
("恢复期间误触发"), and the implementer's own stress run caught a 1/30 flake in it and widened
the BOOM window from 0.2s to 1s; commits 19ba916..e49e036)
Task 5: complete (commits a8ae456..e49e036, review clean)

Task 6: implementer DONE at 5e2675a. Review: spec ✅, 1 Important + 1 Minor.
  IMPORTANT: two of the four display sites never truncate the tag — the reply box's `who` and the
  attached view's title. NAME_MAX_CHARS is 24 *characters*, so an all-CJK name is 48 columns. In
  the reply box `to` is drawn before `body`, so a long name pushes the user's own draft and cursor
  off screen while composing; in the title it eats the only on-screen exit hint. Fix round 1
  dispatched: bound both, plus 80/60-column tests mirroring the existing bar-hint tests.
  MINOR: the report's worst-case width analysis used 12 CJK chars (the prompt target) instead of
  24 (the real cap) — which is what let the two unbounded sites pass unnoticed.
Task 6: minor (deferred): `ui::tests::entering_a_session_always_lands_at_the_bottom_even_without_a_resize`
flaked a SECOND time (message "没等到滚屏内容攒够"), again passing on rerun. Two sightings on this
branch now; the final review should decide whether it is a real intermittent bug.
Task 6: fix round 1/5 (1 addressed, 1 residual — commits 5e2675a..56e69ad). Both sites bounded and
both new width tests verified discriminating; the implementer's first reply-box test passed even
with the truncation reverted (1-column margin) and it caught that itself by mutation and rewrote it.
Residual Important: `session_title_disconnected` adds a long warning clause, so the disconnected
title is ~73 cols (zh) / ~85 (en) even after truncation — English overflows 80, both overflow 60,
and the casualty is the exit hint. Fix round 2 dispatched: omit the name entirely when disconnected.
Task 6: deferred (pre-existing, NOT this branch): the disconnected title overflowed ~106 columns
before this work and still will not fit 60 columns with the name removed. Out of scope here —
fixing it means reworking `session_title_disconnected` and its own width tests. Final review to triage.
Task 6: fix round 2/5 (1 addressed, 0 open — name omitted entirely when disconnected, test
discriminating, scope boundary judged honest; commits 56e69ad..df32dbd)
Task 6: complete (commits e49e036..df32dbd, review clean)
Task 6: deferred (pre-existing, name-independent): the ENGLISH disconnected title overflows 60
columns with an EMPTY tag — proven by probe. Nothing on this branch can close it; it needs
`session_title_disconnected` reworded. Final review to triage.
Task 7: complete (commits df32dbd..10bea5f, review clean — spec ✅, every documented claim
verified against source, both READMEs agree)
Task 7: minor (deferred): the no-rename limitation landed in the new feature section rather than
the "things that will annoy you" section; the fallback paragraph did land there correctly.

FINAL WHOLE-BRANCH REVIEW (opus, 34-mutation audit): **NOT MERGEABLE AS-IS.**
Full report being written to `final-review-report.md` in this directory — read that first.
Blocking:
  1. CODE — unsanitized keystroke bytes become the pinned name (`collect_first_input` accumulates
     raw bytes from the per-keystroke attached-view path: backspace, control chars, escapes).
  2. CODE — the grid tile header lets the name evict the status word, and dropping
     `truncate(session_label(info), 20)` (src/ui/grid.rs:475) turns NO test red.
  3. COVERAGE — the board list has zero coverage for this feature: swapping `session_label(s)`
     back to `s.profile` (the list never shows the name) survives, as do dropping truncate/pad_to
     and reverting the 70/76 activity re-budget.
  4. TEST — `recovering_from_a_failure_does_not_count_as_finishing_a_round` survives deleting
     either guard AND both at once; zero unique coverage. Delete it or assert on the slot at the
     moment of recovery instead of on the final value.
Follow-up (non-blocking): `is_agent` gate unpinned in BOTH places; grid re-anchoring tested only
for session removal (not addition ahead of focus, not across a page boundary); attach title never
asserts the positive; duplicate tests; `name_prompt`'s 2000-char cap unasserted.
Flake shape worth remembering: the six new naming tests degrade toward FALSE GREENS under load,
not false reds — they need a tick to land inside a 0.2s window polled at 50ms.
Note: HEAD moved 10bea5f -> f007276 (the docs commit keeping this workspace) during the review,
so its line numbers describe the tree as read. The `.gitignore` item it flagged is resolved.

FIX WAVE (post-final-review, 4 blocking items) — base 7898caa
Fix 1: review 1 — spec ❌, 2 Important.
  I1: empty-after-sanitize leaves the naming gate open FOREVER (first_input_sealed never resets,
      so fallback_name returns None every retry) -> one 15s LLM thread per Working->Idle turn,
      plus a lost-update race that falsifies request_name's "只问一次" doc at :1019-1021.
      ROOT CAUSE IS MY BRIEF: it asserted "a later real input can still name the session",
      which collect_first_input's early-return-when-sealed (:325) makes impossible.
      Same lesson as the plan-reference-code memory: my own brief text is not authoritative.
  I2: the brief's core regression test was written at the data layer (SessionInfo.tag), not the
      render buffer. board.rs:266 already has a TestBackend+screen_text harness. Matters because
      tag crosses the socket as #[serde(default)] and an older daemon can hand the UI a dirty tag.
Fix 1 round 1 (in flight): implementer added the render-buffer test in board.rs but marked it
  #[ignore] — it FAILS when run, and the ignore reason says the UI-side render path has no
  control-char filter at all; sanitize lives entirely daemon-side, so new-UI + old-daemon still
  renders raw control bytes. Honest, but an #[ignore]d red test is not coverage.
  CONTROLLER TO ADJUDICATE when the round returns.
Fix 1: fix round 1/5 (I1 addressed, I2 parked as #[ignore] by implementer; commits 5c862f2..65bea36)
Fix 1: RULING on the #[ignore] — rejected, sent back as round 2. Reasons: terminal-injection path
  deserves defence in depth and the other side (an older daemon) is not under our control; an
  #[ignore]d red test is not coverage on a branch whose history is exactly "tests that looked like
  coverage"; and it would be this repo's FIRST #[ignore], i.e. a new convention for "deliberately
  broken test" — a bigger change to the codebase's habits than a ten-line filter.
  Note: half the confusion was my round-1 wording ("say so rather than adding it silently" was
  meant as report-before-acting, read as do-not-act). Corrected in the round-2 message.
Fix 1: fix round 2/5 dispatched — UI-side filter in widgets.rs, remove the #[ignore].
Fix 1: fix round 2/5 (I1+I2 both ADDRESSED by scoped re-review, 2 minors ADDRESSED;
  commits 65bea36..69b4899). truncate filter placement independently re-derived: controls are
  0-width so filtering cannot move the width budget or cut a CJK char mid-way.
Fix 1: fix round 3/5 dispatched — 2 new minors, both the false-green class, overriding the
  usual "minors are deferred" rule because that class is exactly what got this branch rejected:
  (a) session.rs:1915 whitespace test is a negative assertion on a 300ms budget observing a
      background thread -> make it channel-deterministic;
  (b) board.rs:301 asserts only absence, stays green vacuously if the row stops rendering
      -> add a positive assertion.
Fix 1: DEFERRED TO FINAL REVIEW (triage): truncate is NOT the choke point for every UI string.
  grid.rs:495, attach.rs:231/:241 (short_path(&s.dir) — the ENTIRE title in the disconnected
  branch) and board.rs:189 reach Span::render_ref unfiltered, and a POSIX directory name may
  legally contain 0x1b (only / and NUL are forbidden). Same injection class, different route,
  pre-existing. NOT fixed by fix-1.
Fix 1: fix round 3/5 (2 addressed, 0 open; commits 69b4899..12e5b0f)
Fix 1: complete (commits 7898caa..12e5b0f, review clean) — 690 tests, 0 ignored.
FIX WAVE base for Fix 2 = 12e5b0f
Fix 2: review 1 — spec ✅, quality NOT approved. 2 Important + 1 Minor.
  I1: MIN_TITLE_NAME_COLS=4 floor is the ONLY thing breaking the invariant. Reviewer RENDERED it:
      en/60col/Asking/24-char name loses 1, 2, 3 cols at id=1/10/100; zh loses 「答」 at id>=100.
      Ids come from a never-recycling AtomicU32 on a long-lived daemon, so 2-3 digit ids are normal.
      Why unfloored is exact, not lucky: truncate overshoots max by exactly 1 col when it truncates
      (pushes the ellipsis after w reaches max), and overhead includes the trailing space AFTER the
      status word, which absorbs exactly that column. cap = budget - overhead holds for every id,
      every label, every width.
  I2: the render test only exercises Working — the one status that provably cannot reach the floor.
      Implementer's own mutation note observed this and drew the backwards conclusion from it.
  m3: floor test asserts == MIN_TITLE_NAME_COLS (self-referential); constant 6 keeps it green while
      clipping asking you by 3 cols.
  Report DID independently verify the brief and caught a real error in it (existing tests render at
  80x24, not the 120x30 my brief claimed). Credit where due.
Fix 2: deferred to final review (triage): grid.rs:527-529 project-name comment now overpromises;
  grid.rs:155 allocates per tile per frame; tile title and reply row disagree on name width at 120.
Fix 2: fix round 1/5 dispatched.
Fix 2: fix round 1/5 (I1+I2+m3 all ADDRESSED, 0 open; commits 698a7c5..b9c1347)
Fix 2: complete (commits 12e5b0f..b9c1347, review clean) — 694 lib tests.
Fix 2: deferred to final review (triage), 5th item: at 60 cols/Asking/English the invariant holds
  only through 4-digit ids; a 5-digit id (>=10000, ~10k session creations since daemon restart)
  saturates name_cap to 0 and still overshoots 2 cols into the status word. NOT a regression --
  the old floored code overshot 6 cols at the same boundary, so this fix made it smaller, not new.
Fix 2: deferred (triage): tests/grid_reply.rs (real daemon+PTY) failed twice under full parallel
  load, clean in isolation and under --jobs 1. Structurally disjoint from this change (no refs to
  draw_grid/tile_title/ui::grid/TestBackend). SECOND known load-flake on this branch, after
  entering_a_session_always_lands_at_the_bottom_even_without_a_resize.
FIX WAVE base for Fix 3 = b9c1347
Fix 3: first dispatch died on an API connection error mid-run (163 uncommitted lines in board.rs,
  no commits, no report). Resumed the same agent from its transcript rather than re-dispatching.
INFRA NOTE: the sdd-workspace helper script rewrites .superpowers/sdd/.gitignore to `*` on EVERY
  run, which un-tracks the whole workspace. That is the mechanism behind the earlier permanent
  loss of 10 reports: workspace silently untracked, then deleted per the skill's teardown step.
  Restored it here. Correct contents: comment block + the single line `*.diff`.
  Check `git diff .superpowers/sdd/.gitignore` after every review-package run.
Fix 3: complete (commits b9c1347..2e0b85e, review clean — spec ✅, quality Approved).
  All 4 named mutations independently re-derived by the reviewer and confirmed to fail for the
  RIGHT reason (column position/count, not "the whole row vanished"). No production code changed.
  Implementer caught a real error in my brief for the THIRD time this wave: truncate(s,15) is NOT
  always exactly 16 cols on truncation (a wide char can push w from max-1 past max, giving 15).
  The composed invariant the row actually depends on -- pad_to(...,16) yields exactly 16 because
  truncate's output is unconditionally <=16 -- does hold. Reviewer verified both halves.
  Minor (no action): the claude-count assertion is redundant with the tag-text assertion.
FIX WAVE base for Fix 4 = d6177bc (2e0b85e + the phone-spec turn-ended commit)
Fix 4: complete (commits d6177bc..249409e, review clean — spec ✅, quality Approved, 0 findings).
  Chose A (delete). Implementer caught my brief's mutation table as STALE for the FOURTH
  brief-error this wave: post-Fix-1's name_attempted latch, "delete both guards together" IS now
  caught by the target test (premature request_name latches the flag, so the real transition can
  never re-fire naming) — only "delete each guard alone" still survives. Reviewer re-derived this.
  Coverage after deletion verified disjointly and independently by the reviewer:
    was == Working      -> recovering_from_a_failure_after_real_input_still_does_not_count
    !first_input.empty  -> a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen
  a_fresh_session_has_no_tag also deleted: name_slot is a fixed struct literal, no branch to mutate.

ALL FOUR BLOCKING ITEMS CLOSED. Moving to final whole-branch review.
Triage list for the final review (deferred/parked across the wave):
  T1 [Fix 1] truncate is NOT the choke point for every UI string. grid.rs:495, attach.rs:231/:241
     (short_path(&s.dir) — the ENTIRE title in the disconnected branch) and board.rs:189 reach
     Span::render_ref unfiltered. A POSIX dir name may legally contain 0x1b. INJECTION PATH,
     pre-existing. Judge whether it blocks merge.
  T2 [Fix 2] 60 cols/Asking/English: invariant holds only through 4-digit ids; a 5-digit id
     (>=10000) overshoots 2 cols into the status word. Old floored code was worse (6 cols).
  T3 [Fix 2] tests/grid_reply.rs fails under full parallel load, clean serially. SECOND load-flake
     on this branch, after entering_a_session_always_lands_at_the_bottom_even_without_a_resize
     (original Finding 10). Both are load-red; the six naming tests are load-GREEN — opposite and
     more dangerous direction.
  T4 [Fix 2] grid.rs:527-529 project-name comment overpromises (says unconditional; now clipped
     to nothing at 60 cols with a long name).
  T5 [Fix 2] grid.rs:155 display_width(&id.to_string()) allocates per tile per frame.
  T6 [Fix 2] tile title and reply row disagree on name width at 120 cols (documented trade).
  T7 [Fix 3] redundant claude-count assertion; comment overstates why it is needed. No action.
  T8 Original final-review follow-ups, never addressed: Finding 3 (what leaves the machine
     widened), 4 (two surfaces bypass session_label), 5 (grid anchor correct by accident),
     8 (duplicate tests, attach title's untested positive case), 9 (surviving mutations).

FINAL WHOLE-BRANCH REVIEW (opus): **MERGEABLE.** All four blocking items genuinely closed;
nothing the wave introduced contradicts anything else it introduced.
Cross-task interaction no per-task review could see: Fix 4's deletion is only safe BECAUSE of
  Fix 1's name_attempted latch. Pre-latch, a premature request_name was invisible (the later real
  transition just overwrote the bad name) — which is exactly why the deleted test survived every
  mutation. Post-latch, a misfire permanently burns the once-per-session slot, so the POSITIVE
  assertions in the two surviving tests detect it.
Also confirmed: Fix 1's filter cannot perturb Fix 2/3's width algebra (char_width already scored
  controls as 0, so the accumulator is bit-identical).
Deliberate asymmetry on record: daemon-side sanitize eats the whole CSI sequence (\x1b[Afix -> fix);
  UI-side truncate drops only is_control() (-> [Afix). Both safe; same session renders differently
  depending on which side cleaned it.
Triage results — NONE block:
  T1 injection paths: narrower than the ledger read. The one route where an attacker-controlled
    name renders without the user choosing it (the project browser, pick.rs:494) is ALREADY
    filtered by Fix 1. Every remaining route needs a name the user selected or authored. The
    branch NARROWS this class (activity, the widest agent-controlled string, is now filtered).
    Residual: the picker sanitizes what it displays but pins the RAW name, so a hostile dir name
    is invisible at the moment of choice. TOP OF FOLLOW-UP LIST.
  T2: worst case is currently UNREACHABLE — SessionState::Asking is never assigned anywhere in
    the daemon (read at :961, matched at :1013, never written).
  T3: suite is trustworthy. The load-GREEN direction was closed by the wave (channel rewrite +
    the latch converting a transient timing observation into permanent state). The two load-RED
    tests share a root cause that predates this branch: they spawn the developer's real login
    shell and race a fixed deadline. False-red is the honest direction. Fix before next branch.
  T4/T5/T6/T7: no action or cheap follow-up.
  T8: Finding 3's documentation half is CLOSED by the README. Finding 8 has teeth — deleting the
    whole name branch at attach.rs:240-258 still breaks no test. Finding 4's failure toast
    (app.rs:266) names the profile while every other surface uses the name.
