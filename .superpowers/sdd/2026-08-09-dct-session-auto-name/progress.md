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
