# Task 3 report: automatic goal-number detection

RED: goal:: tests failed to compile (GoalFinder undefined); finder play test failed (assert at no goal_found); text step_line test failed.
GREEN: after implementation `cargo +1.99.0 test --workspace --locked` all pass (dct-game lib 194 tests), clippy -D warnings clean, git diff --check clean.

Mutations (all turned red as expected):
- MIN_SAMPLES=2: fewer_than_four, steps_with_different_list_lengths, new refused-wedge test, flat-run test
- no all_minus_one exclusion: step_counter..., never_moves..., once_found..., different_list_lengths, wedge test, finder play test
- left.len() >= 1: two_irregular_numbers_are_ambiguous
- observe called in both refused arms: only the NEW test a_refused_step_between_moved_steps_does_not_stop_the_finder_seeing_the_step_counter went red (brief's tests did not catch it)
- lock removal: simply deleting the `found.is_some()` early return is NOT caught (found is never cleared, so behaviour is equal). A real "recompute each time" mutation (early return removed + found=None when left.len()!=1) turns once_found_the_goal_stays_found_for_the_run red.

Added test: a_refused_step_between_moved_steps_does_not_stop_the_finder_seeing_the_step_counter (play_tests.rs). Script: A, (b,b), (A,A), A, 30xA (refused), (b,b), (A,A), A; texts [30,50],[29,50],[28,50],[28,50],[27,50],[26,50]. Passed first try.

Adjustments: none needed to the brief's fake-read scripts; both brief play tests passed as written.
Files: crates/dct-game/src/goal.rs (new), lib.rs, play.rs (update_stall, finder, goal_found, progress.goal_index), play_tests.rs, src/game/text.rs, src/game/skill.md.

Concerns: existing test without_a_goal_index_a_flat_run_never_stalls_or_asks still passes only because a single flat number is found after 4 samples and 8 steps leave just 4 no-progress steps (<6); with longer runs it would now stall (intended new behaviour). Test name is now somewhat misleading. rustfmt not installed for 1.99.0, so formatting hand-matched. observe is called only in moved arm and only when goal_index is None.

## Fix round 1
Changes: goal.rs adds ever_decreased (candidate = not all -1, never rose, dropped at least once); play.rs skips finder.observe for the first moved step (moved_seen counter); tests: never_moves -> a_number_that_never_moves_is_never_picked_as_the_goal (None), new a_goal_that_dropped_once_is_found, a_junk_first_step_is_not_a_sample, a_flat_only_number_is_never_picked_so_no_stall_and_no_ask (14 steps), a_goal_that_dropped_once_then_stays_flat_stalls_and_asks_the_model; existing finder/wedge play tests adjusted (goal drops once; wedge test gets one extra step because the first moved step is not a sample). skill.md sentence and spec section 2 + test list + limitation updated.
RED/GREEN: tests were adjusted together with the code; the mutations below prove each guard.
Mutations: no ever_decreased -> goal::a_number_that_never_moves... and play a_flat_only_number... red; first step counted (moved_seen > 0) -> a_junk_first_step_is_not_a_sample and the refused-wedge test red.
Final: cargo +1.99.0 test --workspace --locked all green (one earlier zombie_reaping failure was due to a compile-state/flake, passed on rerun), clippy -D warnings clean.
Note: a first attempt left a stray rename (without_X) which clippy caught; fixed before commit.
