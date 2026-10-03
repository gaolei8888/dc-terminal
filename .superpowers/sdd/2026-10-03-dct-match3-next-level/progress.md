# SDD ledger — plan: docs/superpowers/plans/2026-10-03-dct-match3-next-level.md

Workspace: worktree /Users/lei/work/dc/dc-terminal-nextlevel, branch feat/match3-next-level (from main ed7a1e9). Spec: docs/superpowers/specs/2026-10-03-dct-match3-next-level-design.md (reachable). Tasks 1-5 run here by subagents; Task 6 (real Mac, user present) is done by the controller, not dispatched.
Models: implementers and reviewers sonnet; final whole-branch review opus.

## Preflight scan

| Pair / task | What one produces vs what the other consumes | Finding |
|---|---|---|
| T1 -> T2 | T1: `screen::Element`, `lib.rs` gets `pub mod screen;`. T2 imports `crate::screen::Element` in play.rs and dco.rs | consistent |
| T2 -> T3 | T2: `Seen`, `Dco::see_text/tap`, new `Stop` variants. T3 `navigate.rs` uses all of them; both edit lib.rs (T3 adds `pub mod navigate;`) | consistent; code in both came from one verified scratch copy |
| T2 -> T4 | T2 replaces `src/game/text.rs` (stop arms, `nav_line`); T4 replaces `cli.rs` which calls `text::nav_line` and the new stop codes | consistent; T2 moved text.rs so every commit builds |
| T3 -> T4 | `navigate::auto_next`, `NavOptions` used by cli.rs | consistent |
| T4 -> T5 | T4 `--auto-next/--tries`; T5 integration tests drive the binary | consistent |
| T1 self | 13 tests | matches (13 #[test]) |
| T2 self | +2 dco tests, text +3 tests => lib game:: 30; dct-game 64 after T2 (49 + 13 + 2) | matches |
| T3 self | navigate 15 tests => dct-game 79 | matches |
| T4 self | game:: 31 | matches (30 + cli 1) |
| T5 self | game_cli 12 | matches (7 + 5) |
| T6 | needs a real Mac and the user | not dispatched |

Rulings (before execution):
- Ruling: every task runs `cargo clippy --workspace --all-targets --locked -- -D warnings` (CI's command) and fixes warnings in files it wrote.
- Ruling: commits English, NO AI attribution (user's standing rule).
- Ruling: the SDD workspace is committed at the end, not deleted (user's standing rule).
- Ruling: merging to main and pushing happen only with the user's say-so at finish (the user already approved this for the last feature, not for this one).

Task 1: complete (commits ed7a1e9..d6a7f2b, review clean)
Task 1: minor (deferred, candidate for the final wave): `all.contains("0 lives")` also matches "10 lives"/"20 lives" (fails safe: stops); untested synonyms (ads/advert/advertisement, life, 其它通关/生命 words); the Payment/Pay comment in a test is not asserted; broad words (bar, gold, out of, get more) can stop a run on ordinary UI text (always fails safe)

Task 2: complete (commits d6a7f2b..9d2404e, review clean)
Task 2: minor (deferred): see_text returns "" for a missing snapshot_id instead of an error; the LevelEnded sentence names the English "Play" button; UnknownScreen with no text prints "画面上的字：。"

Task 3: review (opus): one Important, rest Minor.
Task 3: Ruling: Important 1 (steps budget exhausted exactly as a level ends: the loop still presses Try again and Play, burning a life, and only then notices the budget is zero) is real — fix in round 1: in the `NoGrid | ClassesChanged` arm, stop with StepsDone when the budget is spent, before any navigation.
Task 3: Ruling: Minor 2 (one retry through the level-start box costs two tries, so `--tries 1` can never finish a retry; the doc comment says each press uses a life) — fix in the same round: a Play pressed right after a Try again is the same retry and is not counted; the tries limit is not checked for it.
Task 3: Ruling: Minor 1 (TriesDone / TapLimit / StepsDone leave no `nav` record, so their screen text is lost) — fix in the same round for TriesDone and TapLimit (record the screen text with outcome "stopped"); StepsDone has no screen to record.
Task 3: Ruling: Minor 7 — add a test that a see_text error stops with Stop::Dco.
Task 3: parked (real, deferred) Minor 3: a readable board is played even right after a level ended (`after_board`) if the screen text is unknown — it needs the game to start the next level on its own, which it does not do without a Play press; the user can revisit when the real-device run shows it. Minor 4 is subsumed (the Retry path already clears after_board); Minor 5 (timers make "changed" noisy) is bounded by the 40-tap and tries limits; Minor 6 is a plan-table wording slip.

Task 3: fix round 1/5 (4 addressed, 0 open — budget-spent stop, free retry Play, nav records on TriesDone/TapLimit, see_text error test; commits 64f6faa..7b25907)
Task 3: complete (commits 9d2404e..7b25907, scoped re-review clean)
Task 3: minor (deferred): early StepsDone (budget spent as a level ends) does not look at the screen, so the record cannot say whether the level was won, lost or ended on a price; a no-effect free Play uses the flag up; the doc comment says "right after" but popups in between are also allowed

Task 4: complete (commits 7b25907..c5ebd2d, review clean)
Task 4: minor (deferred): inline `SystemClock` instances next to `clock`; the final `stop` line has no profile_sha256 (pre-existing); the card names English button labels (intentional)

Task 5: complete (commits c5ebd2d..cc83d44, review clean)
Task 5: minor (deferred): dry-run test has an unreachable trailing Frame::Board; price test does not assert the tool list; no integration test for the retry path, ads, lives-out or the Task 3 fix behaviours (unit tests in navigate.rs cover them); valid --tries values are not asserted accepted
Task 6: not dispatched (needs a real Mac and the user; the controller does it)

Final review (opus, ed7a1e9..cc83d44): Critical C1, Important I2-I4, Minors; verdict "with fixes".
Final: Ruling: C1 (an Unknown-text screen that read_grid can read as a grid is swiped as a board, including popups that spend gold; a regression from round one) — fix: after a level (`after_board`) never fall back to read_grid, stop with UnknownScreen; if `play` returns NoGrid/ClassesChanged with 0 steps stop (also prevents a hot loop); two exact Play buttons get their own Screen::Ambiguous that stops without reading the grid; record a `board` nav entry (outcome "entered") with the screen's text whenever a board is entered, as evidence for a future "looks like a board" check.
Final: Ruling: I2 (budget spent as a level ends tells the user "run again", and a re-run presses Play on the next level) — fix: when the budget is spent after a level, look once (no tap) and classify: Retry -> StepsDone, LivesOut -> LivesOut, Won -> Won, Money/Ad -> those, anything else -> LevelEnded; the cross-run bypass (user re-runs from Daily Stamps) stays as a documented limitation: crossing into a new level is the user's act until auto-calibration exists.
Final: Ruling: I3 (after a Try again any single Play is pressed, e.g. on Daily Stamps) — fix: a Try again no longer clears `after_board`; the free Play is allowed only when the screen looks like the level-start box (`is_level_start`: text contains "select boosters", or "level" followed by digits); `after_board` clears only when a board is entered.
Final: Ruling: I4 (`--steps` default 20 vs the spec's 60 under --auto-next) — keep 20: agent shell commands time out around 2 minutes and a step takes ~2.3 s including settle; a re-run continues the same level safely (a fresh start on a board just keeps playing). The spec text is corrected to 20; the card already suggests `--steps 50` for longer runs.
Final: Ruling: Minors ruled in: sentences say "这个画面上我没有点任何东西"; an unknown screen with no readable text prints "画面上没有读到字"; the dry-run closing line stops contradicting "会点它"; the unreachable `remaining == 0` branch is removed; the spec's nav `screen` values are corrected.
Final: parked (real): an Unknown screen at a FRESH start that happens to read as a grid is still played (same exposure as round one's `dct game play`; a text-based "looks like a board" check needs real board OCR — the new `board` nav entries collect it in Task 6); inside `play()` a popup adding at most one colour group is still swiped (round-one limit, to watch in Task 6); "Dismiss beats Ad/Money" (spec's explicit order); timers making "changed" noisy (bounded).

Final: re-review (opus) of the fix wave: C1, I2, I3 (as written), I4, minors all addressed; one Minor that touches a safety gate:
Final: Ruling: New Breakage 1 (the implementer clears `after_board` when the free Play changes the screen, so board -> Try again -> start-box Play -> a result screen with one Play presses that Play and enters the next level) — real, small to close: split the flags: `after_board` is kept until `play` actually enters a board (it alone gates Play), and a separate `expect_board` (set when the free Play changed the screen, cleared on entering a board or on any other press) only lifts the Unknown-arm guard so the retried level's board is not refused. Test: frames [board(A), Try again, start box, Text(["Daily Stamps","Play"])] must end with LevelEnded and taps ["Try again","Play"].
Final: Ruling: New Breakage 2 — tighten `is_level_start` to require "select boosters" (dc-octo's real start-box OCR has it); "Level N" alone can sit on a map label next to a single Play. Cost if wrong: if OCR misses "Select boosters:" the free Play is refused and the run stops with LevelEnded (safe; the user presses Play).
Final: parked: the two false stops (Try again that goes straight to a board with no start box; a free Play with no visible change in 8 s) — both stop, never press wrongly; a budget that ends on a `play` StepsDone while a cascade is still readable skips the early look (covered by the documented cross-run limitation).

Final fix wave: complete (commits cc83d44..e51ec36; scoped re-reviews: all findings addressed, no new Critical/Important)
Final: parked minor: a close-style popup between the start-box Play and the board now stops the run early (expect_board is cleared by any later tap): fails safe, nothing pressed wrongly; a free Play with no effect uses up the flag (safe early stop).
