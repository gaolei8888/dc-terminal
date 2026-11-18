# Task 5 report
- RED: tests added first; dct-game failed to compile (decreased not found), text.rs step_line_says_so_when_stalled FAILED.
- GREEN: dct-game 178 passed; game::text 15 passed; workspace tests all green; clippy -D warnings clean.
- Mutations (all reverted): STALL_STEPS=1 -> six_steps... red; remove len guard -> decreased_needs_same_length... red; remove `|| no_progress >= STALL_STEPS` -> six_steps... red.
- Deviation: `decreased` is `pub(crate)` (brief: private) because play_tests.rs reaches it via `use crate::play::*`. Fake-read scripts needed no adjustment.
- Files: crates/dct-game/src/play.rs, crates/dct-game/src/play_tests.rs, src/game/text.rs

## Fix round 1
- Changed: `goal_index: Option<usize>` on Options/NavOptions (navigate passes through; all construction sites updated); `decreased` replaced by `goal_dropped(before, after, goal) -> Option<bool>` (None on length mismatch / index out of range); counter only advances when goal_index is Some. CLI `--goal-number N` (1..=20, plain-Chinese error), USAGE, `dct --help` line in src/main.rs, skill.md paragraph, parse test.
- Tests (play_tests.rs): goal_dropped unit test; six-flat -> first stalled at index 6; None never stalls/asks; dropping goal never stalls; moves-left dropping + goal flat (Some(1)) stalls at 6 while Some(0) does not; (a) mid-run drop restarts count, first stalled exactly index 10; (b) unreadable list in the middle shifts first stalled from 6 to 8.
- Mutations: remove reset-to-0 -> a_drop_in_the_middle_restarts_the_count red; compare any element -> the_moves_left... and goal_dropped... red. Reverted.
- Command: `cargo +1.99.0 test --workspace --locked` all green; clippy -D warnings clean.
