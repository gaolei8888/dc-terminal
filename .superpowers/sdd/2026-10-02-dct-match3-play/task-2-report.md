# Task 2 report
Status: DONE
- Created play.rs and play_tests.rs verbatim from brief; added `pub mod play;` and `#[cfg(test)] mod play_tests;` to lib.rs.
- Deviation 1: play_tests.rs `len() >= 1` -> `!is_empty()` (clippy len_zero, same intent).
- Deviation 2 (mutation only): row 5 mutated to `return Settle::Failed(e)` instead of the brief's `break Settle::Failed(e)` (break with value does not compile in that loop shape).
- `cargo test -p dct-game`: 28 passed, 0 failed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: clean (after the fix above).
Mutations (each red, then restored; git diff clean):
1. streak>=3 -> two_unmoved_swipes_stop_and_the_second_try_is_a_different_move
2. base+2 -> many_new_classes_mean_something_else_is_on_screen
3. removed streak=0 (failed.clear kept) -> a_move_that_works_resets_the_unmoved_counter
4. removed failed.push -> two_unmoved_swipes_stop_and_the_second_try_is_a_different_move
5. not_a_grid -> Failed -> not_a_grid_during_the_animation_is_waited_out
6. if false -> dry_run_never_swipes
Files: crates/dct-game/src/{play.rs,play_tests.rs,lib.rs}
Concerns: none.

## Fix round 1
- play.rs: dry-run record gets timing_ms {read, choose, swipe:0, settle:0}; swipe-error record gets {read, choose, swipe: time before error, settle:0}. after_observation_id stays absent on those.
- New test `dry_run_and_swipe_error_records_also_carry_timing` (required keys, timing_ms sub-keys, no after_observation_id).
- Mutation: removed the dry-run timing line -> new test FAILED; restored.
- `cargo test -p dct-game play_tests`: 14 passed, 0 failed. Clippy `--workspace --all-targets --locked -- -D warnings`: clean.
- Commit 104ad03.
