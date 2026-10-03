# Final fix wave report

Commits (top three on feat/match3-next-level, not pushed):
1. stop sentences (text.rs)
2. navigation fixes C1/I2/I3 (navigate.rs, screen.rs)
3. spec update (I4 and the rest)

## Per finding
- C1: Unknown arm stops before read_grid when after_board; play with s.steps==0 and NoGrid/ClassesChanged stops UnknownScreen (no hot loop); Screen::Ambiguous (2+ exact Play) stops without grid read or tap; "board"/"entered" nav record before play. Tests: the four named tests. All were red before the fix (the no-move one hung, i.e. looped), green after.
- I2: budget spent after a level -> one see_text, classify, nav record (stopped, tapped null), stop per mapping; unreachable remaining==0 branch removed. Test a_step_budget_spent_at_the_end_of_a_level_looks_at_the_screen_once (4 screens); existing spent-budget test green.
- I3: is_level_start in screen.rs (unit tests); `retrying` removed; free Play only when free_play && is_level_start; free Play not counted as a try. Extra detail: after the free Play changes the screen, after_board is cleared (otherwise the board of the retried level, an Unknown screen, would hit the new C1 guard and stop). Frames of existing retry tests that had a bare Play now carry "Level 1712"/"Select boosters:". The nav-record order assertion in the first retry test now includes the "board" entered records.
- I4 and spec: done (steps default 20 text, Ambiguous row, record values, four rules, limitation).
- text.rs small items: done, with test.

## Mutation checks
- C1 guard disabled -> an_unknown_screen_that_reads_as_a_grid_is_not_swiped_after_a_level red. Restored.
- Free Play without is_level_start -> after_a_retry_a_play_that_is_not_the_level_start_box_is_refused red. Restored.

## Results
cargo test --workspace: 2109 passed, 0 failed. clippy -D warnings: clean. cargo check windows msvc -p dct: ok (only the pre-existing unused-variable warning). Not done: nothing outstanding.

## Fix round 1
- Split flags: `after_board` is never cleared by the free Play; new `expect_board` (set when the start-box Play changed the screen, cleared on board entry and by any other button) only relaxes the Unknown-arm guard.
- `is_level_start` now only checks for "select boosters"; unit tests updated (Level 12 / Level 1713+Play false). All start-box frames already contain "Select boosters:". Spec sentence updated.
- New test a_result_screen_after_the_start_box_play_is_not_pressed: red before (panic), green after.
- Mutation (free Play clears after_board again): that test red (1 failed, 91 passed); restored.
- `cargo test --workspace`: 0 failed (count in the status reply); clippy -D warnings clean; windows check ok.
