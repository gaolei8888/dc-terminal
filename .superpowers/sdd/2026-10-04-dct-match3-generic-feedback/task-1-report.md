# Task 1 report

Status: DONE_WITH_CONCERNS

Process note: implementation was written before the new tests were run (no separate RED run); RED was demonstrated afterwards via mutations.

Changes (crates/dct-game/src/play.rs, play_tests.rs): MAX_REFUSED_IN_A_ROW=5, `locked` cells, `blocked` closure used for pick and offered; board_key hoisted; locked.clear() on success.

Existing tests: the "two refusals stop" tests (two_unmoved_swipes..., permuted_ids_on_an_unchanged_screen..., a_changed_odd_flag..., a_swap_the_game_bounces_back...) pass UNCHANGED: on the tiny 3x4 board A the two refused swaps lock all candidates, so pick is None -> Stuck after 2 swipes. Not edited.
One test had to change: a_failed_swap_triggers_the_model_and_is_listed_but_not_offered. After locking, board A has <2 offered candidates so the model was not asked. Switched its board to MANY (6x4) and a 6x4 `moved` board; assertions unchanged.

New tests: after_a_refused_swap_no_later_candidate_touches_its_two_cells (strengthened: MANY board, 5 steps, all earlier steps), five_refusals_in_a_row_stop_the_run_not_two, a_move_that_works_unlocks_the_cells (brief version, weak), a_refused_swap_locks_both_of_its_cells_even_when_better_candidates_share_only_one (OVERLAP board found by random search), a_move_that_works_unlocks_cells_that_the_only_remaining_move_needs.

Mutations (brief's versions survived with the brief's tests; added tests above to kill them):
- drop locked.contains(b): killed by OVERLAP test. drop a: killed by several.
- remove locked.clear(): killed by ..._the_only_remaining_move_needs (brief's unlock test survives it).
- const = 2: killed by five_refusals test.
Final: dct-game 165 pass; workspace test all green (no flakes hit); clippy -D warnings clean. cargo fmt unavailable on 1.99.0 toolchain (not run).
