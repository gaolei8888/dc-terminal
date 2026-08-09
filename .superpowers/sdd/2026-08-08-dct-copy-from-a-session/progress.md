# SDD ledger — plan: docs/superpowers/plans/2026-08-08-dct-copy-from-a-session.md

Branch: feat/copy-from-session
Branch base: 3902330 (docs: implementation plan for copying text out of a session)

Tasks: 4. Pre-flight conflict scan: clean — no task contradicts another or the
Global Constraints; no plan mandate collides with the review rubric.

Repo convention: this workspace is TRACKED, not deleted at the end. See
`.superpowers/sdd/.gitignore` — brief/report/progress are committed; only
`*.diff` is ignored. The sdd-workspace script rewrites that .gitignore to `*`
on every run; restore it before committing.

Task 1: implemented at 441ae41, review Approved with 1 Important. Fix round 1 dispatched:
  - a_failed_screen_call_does_not_flip_the_capture_state cannot fail — it recomputes a pure function with unchanged arguments and asserts the obvious. MY plan wrote that test; the implementer transcribed it faithfully. Production behaviour verified correct by trace (run()'s failure arm is `_ => app.connected = false`, touching neither app.scroll nor app.view).
Task 1: reviewer traced the entry-frame timing question and found no stale-scroll frame: view transitions happen after the capture check, and the next iteration's Screen block overwrites app.scroll before the check runs.

Task 1: fix round 1 at b62d7cb — chose "make it real": extracted
  `scroll_after_screen_call(previous, &Result<Response>) -> ScrollState` from run()'s
  Screen handling; run() now calls it (no divergent copy), and two real tests drive it
  (Err -> unchanged, Ok -> replaced). Mutation-proved red. Scoped re-review: all findings
  ADDRESSED, no new breakage. Minor (deferred): the test name still says
  "capture_state" though the body now exercises scroll_after_screen_call — stale name only.
Task 1: complete (441ae41, b62d7cb).

--- RESUME POINT (written before a context compaction) ---

Task 2: implemented dd5220f. Review Spec OK / Quality changes requested — my brief's
  "enter_session 是唯一漏斗" premise was FALSE (mod.rs:241-242 already said so; four sites
  reach View::Attached without it: pick.rs:81, pick.rs:137, mod.rs:326, mod.rs:1679,
  all right after a create_session call). Fix round 1 at 23eb2b8: kept the enter_session
  reset, added one to create_session (the common ancestor), rewrote both comments and the
  test docstring around the true claim (two entry constructors cover every path in;
  exit has three paths, one via the shared back_one_level). Re-review: both ADDRESSED,
  no new breakage. Verified no fifth entry path exists.
Task 2: minor (deferred): one-use attached_app() helper in attach.rs; the
  enter_session test lives in attach.rs rather than beside enter_session in mod.rs.
  Both brief-mandated, deliberately not actioned.
Task 2: complete (dd5220f, 23eb2b8).

State: branch feat/copy-from-session, base 3902330. Tasks 1-2 DONE.
Task 3: implemented at 74f40d8 (BASE 23eb2b8). Review: Spec OK, Quality Approved with one
  Important non-blocking finding I chose to fix anyway — the hint truncates below width 78
  (ACTION_MIN_COLS = 28 is the guaranteed floor; en 37 cols, zh 35), and the bar is the ONLY
  place in the whole UI that names F4 (view.rs:1121 lists only F3). Truncation there =
  a mode you can't see and can't leave, i.e. the exact thing this task exists to prevent.
Task 3: fix round 1 at 1e6a1b2 — Key::CopyModeShort + width-driven long/short pick, guard
  test, width-60 render test. Re-review: ADDRESSED, no new breakage. Independently
  recomputed: width 80 -> action 39 (long 37/35 fit); width 60 -> action 28 (short 20/18).
Task 3: minor (deferred): the implementer widened ACTION_MIN_COLS to pub(crate) so the
  guard test could live in i18n.rs. The sibling ESCAPE_HINT_COLS guard lives in mod.rs
  beside its constant and needs no widening — that placement was available and simpler.
  Stylistic, not harmful. Final review may triage.
Task 3: minor (deferred): below width ~39-41 (far under grid::MIN_COLS = 60) neither
  form fits and the hint truncates again. Same class as the rest of the right segment
  below its floor; out of the finding's scope.
Task 3: complete (74f40d8, 1e6a1b2).

Task 4: implemented at fed2898 (BASE 1e6a1b2). User's README install.sh edits verified
  byte-identical and unstaged; commit has zero install.sh content (I checked myself, not
  just the report). Review: Spec OK, Quality changes requested.
Task 4: fix round 1 at afe8db1 — the wheel sentence now hedges both ways (may do nothing
  OR send arrows, depending on the terminal), and 「鼠标一直归终端」 became 「只要会话里跑着
  的东西没自己伸手要鼠标」. Re-review: both ADDRESSED, no drift between languages.
  I re-verified the user's README edits byte-identical and absent from both commits.
Task 4: complete (fed2898, afe8db1).

FINAL whole-branch review (opus, base 3902330..afe8db1): approved except ONE blocking
  finding — F4 had no on-screen entry point. idle_help's Attached arm offered only F3,
  the ? overlay is unreachable there (all keys go to the agent), and the copy-mode hint
  only appears AFTER you press the key you cannot discover. For Claude Code sessions
  that meant the branch's own headline fix was reachable only by README readers.
  Triage of the six deferred findings: five fine to ship, F4-discoverability must-fix.
FINAL fix at f98cefe: ("F4", Key::EnterCopyMode) as the TAIL item (fit_help keeps the
  last item via split_last and drops from the head), label 复制/copy, ALL_KEYS + count,
  and a render test over {80,60} x {Zh,En}. Re-verified independently: action_cols
  39 @ 80 and 28 @ 60; F3+F4 costs 22 zh / 24 en — fits at the tightest width.
  Bundled three stale-comment fixes: wheel_action's doc described the world this branch
  ended; two comments disagreed on the narrowest supported width (60 is right); and
  App.copy_mode was pub among pub(crate) neighbours. Re-review: all ADDRESSED.

BRANCH COMPLETE. 643 lib + 38 integration tests green, fmt + clippy clean at f98cefe.

OPEN QUESTIONS FOR THE USER (not mine to decide):
  1. Wheel-becomes-arrow-keys in non-capturing sessions. dct is in the alt screen and
     never sends ?1007, so alternate-scroll terminals turn wheel notches into Up/Down
     that get forwarded to the agent. Documented in both READMEs this branch. Whether to
     suppress it (CSI ? 1007 l alongside EnterAlternateScreen) is a design decision.
  2. Second release: origin/main is at a553c57 (grouping, protocol 6). This branch adds
     no protocol change, so no daemon restart is forced by it.

Earlier detail, kept for the record:
  Task 4 fix round 1 blocking finding was: README claimed "the wheel does nothing in those sessions". FALSE on common
  terminal defaults. dct runs in the alt screen (mod.rs:234) and never sends ?1007, so
  with capture off the terminal's alternate-scroll turns wheel notches into cursor
  Up/Down; those miss key_scroll and fall through key_to_input (mod.rs:895-896) into
  Request::Input — i.e. the wheel now TYPES ARROW KEYS AT THE AGENT in codex/shell
  sessions. New behaviour created by Task 1 (capture used to be unconditional in a
  session). Fix = say the wheel no longer scrolls dct history and may do nothing or send
  arrows depending on the terminal. Plus a Minor: "鼠标一直归终端" overclaims, since
  agent_owns is per-frame (attach.rs:51-59) and a shell can launch a mouse-using TUI.
When it returns: review-package fed2898..<new>, scoped re-review, then the FINAL
whole-branch review (most capable model), then finishing-a-development-branch.

QUESTIONS FOR THE USER AT BRANCH END (do not decide these alone):
  1. F4 is undiscoverable: the ONLY place the UI prints it is the copy-mode hint, which
     you can only see after already pressing it. Not in bar_keys, not in the ? screen.
     After this branch the README is the sole place to learn the key exists.
  2. Wheel-becomes-arrow-keys in non-capturing sessions (above) is a real behaviour
     change. Documenting it is this branch's scope; suppressing it (sending ?1007l) or
     consuming bare Up/Down would be a new design decision.

  Fix was = long/short wording chosen by measured width, following bar_keys (mod.rs:2106-2126)
  which already does build-rich / measure / fall back. New key CopyModeShort
  (en "Copy mode · F4 exits" 20; zh "复制模式 · F4 退出" 18, both under 28), an i18n guard
  test that the short form is <= ACTION_MIN_COLS (modelled on the ESCAPE_HINT_COLS guard
  near mod.rs:3510), and render tests at width 80 (long) and 60 (short) in both languages.
Task 3: informational, pre-existing, for the final review: ALL_KEYS covers 97 of 109
  field-less Key variants (missing AllKeys, EnterFolder, GoUp, KeysGroup*, List, MoreKeys,
  NoSubfolders, RecentProjects, StatusFailed, SwitchPane), so the guard test's "exhaustive"
  comment overstates what it checks. Predates this branch.
Task 3: minor (deferred): with copy mode on, a stale non-empty app.message or a
  disconnect hides the hint entirely, and messages have no TTL. Judged acceptable — it is
  the mandated priority, and re-pressing F4 recovers. A message TTL is out of scope.

  Earlier in Task 3, the agent came back BLOCKED: my plan's English string
  "Copy mode · the mouse is the terminal's · F4 to exit" is ~52 display cols against a
  39-col right segment at width 80, and wrap_help only breaks on DOUBLE spaces, so a
  single-space sentence is atomic and gets silently truncated by Paragraph — the exact
  bug class that shipped in the grouping round. Chinese fits (35/39), unchanged.
  MY RULING: shorten English to "Copy mode · mouse released · F4 exits" (37 cols),
  keep the approved Chinese, keep single-space `·` style (house convention; double
  spaces would wrap and grow the bar, and bar height is load-bearing for grid MIN_ROWS),
  and ADD a width-80 both-languages regression test asserting the COMPLETE string is
  present. The missing test of exactly that shape is why this class shipped before.
When it returns: review-package 23eb2b8..&lt;new&gt;, task reviewer, then Task 4.

Each task: task-brief -> implementer -> review-package -> task reviewer -> fix loop.

Task order and what each does:
  1. wants_mouse_capture + App.copy_mode + swap the capture judgement   [DONE 441ae41, b62d7cb]
  2. F4 toggles copy_mode in attach.rs; reset in enter_session (the single funnel)
  3. Key::CopyMode both languages; bar priority error > copy mode > scroll hint
  4. Both READMEs; verify every documented key is actually bound

Hard rules for every task:
  - src/proto.rs, src/pty.rs, src/session.rs, src/daemon.rs stay UNTOUCHED.
    No protocol change is needed; App.scroll.agent_owns already carries the fact.
  - cargo fmt + cargo clippy --all-targets -- -D warnings + cargo test, all green.
  - English commit messages, no Co-Authored-By, no AI attribution.
  - The working tree has the user's own uncommitted README.md / README.zh-CN.md
    edits about scripts/install.sh. Never commit them.
  - This workspace is TRACKED. At the end, restore .superpowers/sdd/.gitignore
    (the sdd-workspace script rewrites it to `*`) and COMMIT the briefs, reports
    and this ledger. Do NOT delete the directory — the SDD skill says to, this
    repo says otherwise, and following the skill last time destroyed 10 reports.

Release state: origin/main is at a553c57 (grouping + scrollback, protocol 6).
This branch adds no protocol change, so it ships whenever it is ready.
