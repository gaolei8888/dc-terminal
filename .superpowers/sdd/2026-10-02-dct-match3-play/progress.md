# SDD ledger — plan: docs/superpowers/plans/2026-10-02-dct-match3-play.md

Workspace: worktree /Users/lei/work/dc/dc-terminal-match3, branch feat/match3-play (from main 366362e). Spec: docs/superpowers/specs/2026-10-02-dct-match3-play-design.md (reachable). Tasks 1-5 run here; Task 6 (real Mac, user present) is NOT run by the controller.
Models: implementers and reviewers sonnet (plan code is complete but every task has run-and-mutate steps, so mid-tier floor); final whole-branch review opus.

## Preflight scan (shared file / interface pairs, then per-task self-consistency)

| Pair / task | What one produces vs what the other consumes | Finding |
|---|---|---|
| T1 -> T2 | T1: Board::from_read, choose, GridRead, lib.rs ends with `#[cfg(test)] mod tests;`. T2 consumes them, edits lib.rs (adds `pub mod play;` and `mod play_tests;`) | consistent; code in both came from one verified crate |
| T2 -> T3 | T2: Dco trait, Profile, DcoError. T3 impls Dco, edits lib.rs again | consistent |
| T3 -> T5 | T3: DcoClient::connect(&Path). T5 cli.rs calls it with ~/.dco | consistent |
| T4 -> T5 | T4: profile::load, LogFile, mod.rs v1 (profile, log). T5 rewrites mod.rs adding cli/skill/text | consistent; T5 Step 1 replaces mod.rs wholesale |
| T4 shares src/lib.rs, src/journal.rs with nobody else | pub(crate) civil_from_days | ok |
| T5 shares main.rs, daemon.rs with nobody else in this plan | one wiring each | ok |
| T1 self | 15 tests listed vs tests.rs | matches (15 #[test]) |
| T2 self | 13 play tests; total 28 | matches |
| T3 self | 5 dco tests; total 33 | matches |
| T4 self | 5 profile + 2 log = 7 | matches |
| T5 self | 6 text + 5 skill + 3 cli = 14 lib tests; plan says "库里 21" counting T4's 7 | consistent (21 = T4 + T5) |
| T6 self | needs real Mac + user | not run |

Rulings (before execution):
- Ruling: every task also runs `cargo clippy --workspace --all-targets --locked -- -D warnings` (CI's exact command, .github/workflows/ci.yml:45) and fixes what it finds, although the plan's commit steps only say `cargo clippy` — CI fails on any warning; cost if wrong: a few clippy-driven edits to the plan's verbatim code (e.g. `len() >= 1` -> `!is_empty()`), harmless.
- Ruling: in Task 5, `src/game/cli.rs` imports and helpers used only by the unix path (`SystemClock`, `play`, `Options`, `Clock`, `profile`, `text`, `LogFile`) must live under `#[cfg(unix)]` so a Windows build has no unused/dead-code warnings; behavior unchanged; cost if wrong: Windows CI warnings.
- Ruling: commits are English with NO AI attribution line (user's standing rule, overrides the harness default).
- Ruling: the SDD workspace is committed at the end, not deleted (user's standing rule: dct keeps SDD workspaces in the repo).
- Ruling: work happens in the worktree on branch feat/match3-play; merging to main and pushing are the user's call at finish.

Task 1: complete (commits 366362e..85a8dd4, review clean)
Task 1: minor (deferred): choose.rs comment says tied scores share a row — not strictly true for small `rows` (order is still deterministic)
Task 1: minor (deferred): test `equal_scores_prefer_the_lower_row` checks the lowest_row score term, not the tie-break; nothing pins the tie-break order (leftmost column, horizontal first, upper row)
Task 1: minor (deferred): Board::parse is pub but test-only; ClassInfo not re-exported in lib.rs

Task 2: Ruling: the reviewer's Important finding (dry-run and swipe-error records lack `timing_ms`, plan-mandated) is real but small — fix it: every record gets `timing_ms` with `read`/`choose` measured and `swipe`/`settle` 0 where they did not happen; `after_observation_id` stays absent on records with no settled frame (StillMoving, Failed, dry-run, swipe error) — the reviewer agreed this is defensible. Cost if wrong: a tiny schema difference nobody reads yet.
Task 2: Ruling: "more than 2 above the start's" in the review prompt was my sloppy wording; the spec rule is "one extra class allowed, two or more extra stop" and the code implements that.
Task 2: minor (deferred): no test pins NO_CHANGE_MS / GIVE_UP_MS values; `clock.now_ms() - t` can underflow if a Clock is not monotonic (use saturating_sub); `Summary` lacks #[derive(Debug)]; one-line comment for "unreadable the whole window -> StillMoving"
Task 2: plan defect (noted): brief's mutation row 5 used `break Settle::Failed(e)` which does not compile inside `settle` (needs `return`)

Task 2: fix round 1/5 (1 addressed, 0 open — timing_ms on dry-run and swipe-error records; commits a200885..104ad03)
Task 2: complete (commits 85a8dd4..104ad03, review clean)

Task 3: complete (commits 104ad03..714f0f7, review clean; 34 tests in dct-game = 33 + the one Task 2's fix round added)
Task 3: minor (deferred, candidate for the final fix wave): DcoClient sets no read timeout, so a stalled dco blocks `dct game play` forever (dco's own server uses 5 s in the handshake); JSON-RPC protocol errors drop error.code; non-JSON tool text becomes Null and hides the cause; `let _ = init;` leftover; fake dco uses {"ok":false} not the real {"error":"unauthorized"} (same code path)

Task 4: complete (commits 714f0f7..34ccb7d, review clean)
Task 4: minor (deferred): toml crate's English error text is appended to the Chinese config message; out-of-range test only asserts is_err (does not check the message names region/rows/cols, no y+h>1 or negative case)

Task 5: complete (commits 34ccb7d..cc51b86, review clean)
Task 5: minor (deferred, candidate for the final fix wave): the skill-installer thread (daemon.rs) also runs on Windows/Linux where `dct game play` only says "只支持 Mac" — gate it on macOS; gate has no test (extract should_install_skill(socket)); skill.md written non-atomically (temp+rename); final stop-line write ignored; println! panics on a closed pipe
Task 6: not run (needs the user at a real Mac with dco and the three agents)

Final review (opus, over 366362e..cc51b86): no Critical; Important I1-I6, Minor M1-M9; verdict "with fixes". Full text in the task output; the fix brief below carries each finding.
Final: Ruling: I1 (a refused swipe is counted and printed as a move) — fix: count a step only after a successful swipe; give outcome `stopped` its own sentence — a user-facing false statement, cheap to fix.
Final: Ruling: I2 (level ends, board unreadable, message says "screen still moving") — fix: track "no readable frame since the swipe"; at the deadline stop with NoGrid, not StillMoving — this is the normal end of a level.
Final: Ruling: I3 — gate the skill-installer thread on `cfg(target_os = "macos")` only; the command itself stays `cfg(unix)` (so the fake-dco integration tests still run on Linux CI) — the harm is the card on machines with no dco, not the command; cost if wrong: Linux says "dco 没在运行" which is true enough.
Final: Ruling: I4 — read timeout on the dco connection (10 s on reads) mapped to a plain sentence.
Final: Ruling: I5 — unknown-tool JSON-RPC error -> "dco 版本太旧"; PermissionDenied on connect gets its own sentence.
Final: Ruling: I6 — the SPEC is binding: nest the feature fields under `candidates[].features` in play.rs and read them from there in text.rs; log shape is permanent once logs exist.
Final: Ruling: M9 — keep `(lowest_row+1)/rows` in code; amend the spec formula to match (ranking identical, top row no longer scores 0).
Final: Ruling: M1 open the log file once in LogFile::open; M2 saturating_sub everywhere a clock difference is taken; M3 neutral Stuck sentence (it can stop after one try); M5 add run_id (to every record and the stop line) and time_ms to the stop line; M6 start the installer thread after the daemon's bind, inside run_with_manager under the same default-socket condition; M7 atomic card write (temp + rename); M8 plain sentence for dco_refused, and soften the card's "每步不到一秒" to "每步很快" until acceptance item 2 measures it.
Final: parked (not fixed, real): M4 (board that caused a stop is not logged), the rest of M8 (bad_request wording, dry-run final line, println! on closed pipe), no opt-out for the skill card, Codex/Qwen sandbox and card-loading (acceptance item 3), real-board fixture test (Task 6), pre-existing Windows clippy error at src/student_projects.rs:669 on main.

Final fix wave: complete (commits cc51b86..cd7ae0f; scoped re-review: all findings addressed, no new Critical/Important)
Final: parked minor: settle's "nothing readable since the swipe" rule means one early readable frame followed by ~8 s of unreadable still reports StillMoving ("画面还在动") — plausible when a celebration overlay precedes the result page; refine to "last reads were unreadable" later. Ruling: parked, because it is a refinement of a case the user will see only on some level ends, and the real-device run (Task 6) will show how often it happens.
Final: parked minor: tests/game_cli.rs carries its own copy of civil_from_days (could drift from journal.rs)
