# SDD ledger — plan: docs/superpowers/plans/2026-08-04-dct-scrollback.md

Branch: feat/scrollback (created from main f33d608)
Baseline: 522 tests green, 0 failures.
Cargo is NOT on PATH by default: export PATH="$HOME/.cargo/bin:$PATH"

## Pre-flight: plan vs. main (2026-08-07)
Tasks 1-5 are ALREADY IN MAIN, superseded, NOT to be dispatched:
  T1 handshake  -> PROTOCOL_VERSION exists, now 4
  T2 dct restart -> replaced by client::restart_daemon + main.rs offer_to_restart_stale_daemon
  T3/4/5 ui.rs split -> src/ui/ is already 11 modules (app/attach/board/grid/keys/mod/pick/secret/settings_view/view/widgets)
Remaining scope: Task 6, 7, 8, 9, 10, 11.

Controller rulings on plan staleness (bind all implementers):
  R1 PROTOCOL_VERSION goes 4 -> 5, not 1 -> 2. The wire-shape test in proto.rs
     (pins (PROTOCOL_VERSION, shape)) must be updated in the same commit.
  R2 ScreenSnapshot today is `(Vec<Vec<ScreenSpan>>, (u16,u16), SessionState)`.
     The struct it becomes must keep `state` alongside lines/cursor/scroll.
  R3 The daemon reports ErrorCode only; the UI composes sentences via src/i18n.rs.
     Any new user-visible string goes through i18n::Key with BOTH zh and en,
     never a hardcoded literal. This overrides the plan's inline Chinese strings.
  R4 Grid (nine-tile) view is read-only and out of scope for scrolling.
  R5 All line numbers in the plan are stale; locate by symbol name.
  R6 All absolute test counts in the plan are stale (baseline 522). Verify
     "all green + my new tests present", never an absolute total.

## Task 6
- implementer agent ac411270adbaf0f94, commit 6495e98. Bumped vt100 0.15 -> 0.16
  (0.15.2 grid.rs:125 `rows_len - scrollback_offset` underflows and PANICS whenever
  the offset exceeds screen height — i.e. the feature's main use case; 0.16.2 uses
  saturating_sub). Reviewer independently confirmed against both crate sources.
  RULING: accepted — a version bump of an existing dep is not "a new dependency",
  and the bug is real and central.
- Task 6: review round 1 -> spec OK, 1 Critical: scroll_by calls probe_max BEFORE
  reading `cur`, so `cur` is always max; any scroll_by(n) jumps to the oldest line.
  PLAN-MANDATED (the bug is in the plan's own Step 3 snippet, task-6-brief.md:285-296).
  RULING: fix it. The plan's snippet is demonstrably wrong (reviewer reproduced
  empirically on a live PtySession); scroll_state two methods away has the right
  ordering. Also: add the regression test the brief's 9 tests all miss — every
  existing test overshoots max, where the buggy and correct computations coincide.
- Task 6: fix round 1/5 (2 addressed, 0 open; commits 6495e98..7000e57)
- Task 6: complete (commits f33d608..7000e57, review clean)
## Task 7
- implementer a325c40e8960b3f49, commit aebdec6. Rewrote the brief's new_lines test:
  the reference version polled screen_text_for_test for content vt100 deliberately
  keeps out of view while scrolled up, so it would hang forever. Reviewer
  independently confirmed the hang is real and the rewrite is not a weakening.
  Also: brief's helpers assumed a 5-arg create(); real signature is 4-arg.
- Task 7: complete (commits 7000e57..aebdec6, review clean)
## Task 8
- implementer a6d05168de8743595, commit cdc1c08. PROTOCOL_VERSION 4 -> 5; both
  shape-pin tests updated with real serialized output (reviewer verified the pin
  would still catch a future unversioned shape change, i.e. it was not silenced).
  Brief's backward-compat JSON literal predated the `state` field and had to be
  updated so the test isolates the one new field.
- Task 8: minor (deferred): only the Request side is byte-pinned against
  PROTOCOL_VERSION; Response::Screen / ScrollState have no equivalent shape pin.
  Pre-existing gap, acknowledged in a comment in proto.rs. For final-review triage.
- Task 8: complete (commits aebdec6..cdc1c08, review clean)
## Task 9
- implementer a1cfb5ec70514810a, commit 5536cca. vt100 0.16's mouse API matched the
  brief exactly (verified against crate source).
- Task 9: review round 1 -> spec OK, lock discipline OK, SGR path OK. 1 Critical +
  1 Important, BOTH inherited verbatim from the brief's reference code:
    C: legacy (?1000 non-SGR) release must use button code 3 (xterm ctlseqs), not the
       real button number. Reviewer proved Release(0) and Press(0) emit identical bytes.
    I: Utf8 (?1005) must not share the single-byte path — it emits raw >=128 bytes for
       any column >=96, which is not valid UTF-8 and can corrupt the agent's parse.
  RULING on both: the real xterm protocol governs, not the brief. Fix.
  Plus 2 Minors: no Default-encoding release test (the gap that let the Critical ship),
  and the single-byte boundary is untested (reviewer mutated >255 to >256, suite passed).
- Task 9: fix round 1/5 (4 addressed, 0 open; commits 5536cca..f59a98f)
- Task 9: complete (commits cdc1c08..f59a98f, review clean)
  Legacy release now uses Cb=3; Utf8 is its own arm emitting real multi-byte UTF-8
  with a 2015 ceiling; both sides of the single-byte boundary pinned by tests.
## Task 10
- implementer ab5dea08936933fe8, commit 3fae13c. Four deliberate deviations, ALL
  judged correct by review: (a) brief's `app.client()?` in handle_mouse would have
  taken the whole TUI down on one mouse event while disconnected -> swallow idiom;
  (b) per-frame mouse_capture_transition reconciles state not edges, better than the
  brief's per-call-site enable/disable (4+ paths set View::Attached); (c) page height
  from app.screen.len(); (d) same for content_rows.
- Task 10: review round 1 (opus) -> spec OK. Exit-path enumeration for mouse capture
  clean on every path but SIGKILL/SIGSEGV. 3 Important + 5 Minor:
    I1 every mouse-MOTION event forces a Request::Screen + full redraw via the
       `continue`; ~80 refetch+redraw cycles per pointer sweep of an 80-col window.
    I2 zero automated coverage of DisableMouseCapture on exit — the worst failure in
       this change (leaked capture = junk on every click forever). signal_restore.rs
       already owns the pty master fd; one assertion away.
    I3 screen_origin border offset is mutation-invisible: reviewer changed it to
       (area.x, area.y) and 542/542 still passed.
  CONTROLLER RULINGS overriding the brief: M5 the "new lines below" hint must also say
  how to get back (the author's own doc comment one function later argues this);
  M6 key_scroll must decline PageUp/PageDown when max==0, matching wheel_action, so a
  fresh session's PageUp is not swallowed into a dead key.
- CONTROLLER ERROR (mine): I propagated the plan's stale commit convention (Chinese +
  Co-Authored-By). This repo's standing convention is ENGLISH commit messages with NO
  AI attribution. Reviewer caught it. Fix commit onward is English; the 6 earlier
  commits on this branch get reworded by the controller before the branch is finished.
- Task 10: fix round 1/5 (8 addressed, 0 open; commits 3fae13c..38c7135)
- Task 10: complete (commits f59a98f..38c7135, review clean)
  Drain loop uses a labelled 'main; re-reviewer checked it against swallowed-wheel,
  starved-capture-reconcile, stranded-message and spin-without-redraw and found it
  sound. Capture-disable is now proven on the SIGTERM path by tests/signal_restore.rs.
- Task 10: minor (deferred): attach.rs coordinate translation only bounds-checks the
  LOWER edges (checked_sub). A click on the bottom bar or past the right border still
  yields a plausible (col,row) and gets forwarded — contradicting the comment right
  above it ("点在了边框、底栏上就直接丢"). Pre-existing from 3fae13c. FOR FINAL REVIEW.
- Task 10: minor (deferred): the "don't forward clicks to an agent that doesn't want
  the mouse" guard has no test that dies when deleted (reviewer removed the else-if,
  249/0 still passed). FOR FINAL REVIEW.
- Task 10: minor (deferred): tests/signal_restore.rs hardcodes crossterm 0.28.1's
  byte string; a crossterm bump fails it loudly. Acceptable, note when the dep moves.
- Task 11: complete (commit fd49dbb -> reworded fcb13f4, review clean, zero factual errors)

## Commit-message rewrite (controller)
- The 7 Chinese commits with Co-Authored-By trailers were rewritten to English with no
  AI attribution, matching this repo's actual convention. Verified content-identical
  against a backup ref before deleting it. My error: I propagated the plan's stale
  convention into every dispatch; the Task 10 reviewer caught it.

## Final whole-branch review (opus)
- Verdict after one fix wave: SAFE TO MERGE. 1 blocking + 6 other findings, all fixed
  in commit e7f0d16 and confirmed by a scoped re-review that mutation-verified each of
  the three behavioral fixes.
- The blocker was a genuine seam artifact no per-task review could see: key_scroll
  bailed on agent_owns before considering End, but agent_owns is a PER-FRAME property
  read off the live parser, not a fixed per-agent trait. An agent turning on mouse
  reporting mid-session (a command finishing into a pager) left the user scrolled up
  with the bar telling them to press End while End was forwarded to the agent.
- Deferred minors triaged: all four can stand. Notably the Response-side shape pin is a
  pre-existing gap, and the direction that actually breaks users (v4 daemon -> v5 client)
  IS covered by a_screen_response_without_scroll_still_parses.
- Upgrade path 4->5 confirmed to degrade completely: an old daemon's Screen response has
  no scroll, serde default gives max=0/agent_owns=false, so wheel_action returns Ignore
  and key_scroll returns None — a v5 client never sends a request a v4 daemon would
  reject. The max==0 guard added as ruling M6 turns out to be load-bearing for this.

## Parked at the cap (no second fix wave; for a later touch-up)
- attach.rs comment says an out-of-range click overshoots "by one or two" rows; the real
  bound is bar_h-1, up to ~7 on a 24-row terminal. Substantive claim correct, magnitude
  understated. Ruling: park — comment nit, behavior is right.
- i18n.rs names htop as a full-screen program that ignores the mouse; htop enables mouse
  reporting by default (so does neovim). less and plain vim are correct examples.
  Ruling: park — one wrong example in a comment whose rule is stated correctly.
- The shortened English hint drops the unit ("Scrolled up 40 · press End"). Ruling: park
  — the Chinese keeps 行; "Up 40 lines · press End" would keep it and still fit 55 cols.
- Residual: below ~51 columns the English hint's "End" is still clipped. The attach view
  has no minimum-width guard. Pre-existing shape, materially improved (was ~76).

## Not done: interactive acceptance
- No live daemon pass was run at any point — restarting the daemon kills the user's
  running agent sessions, so every implementer and reviewer was forbidden from it.
  The checklist is in the final review and was handed to the user.
