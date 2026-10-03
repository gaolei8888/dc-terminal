# Task 3 report

Done: added crates/dct-game/src/navigate.rs (code extracted verbatim from the brief) and `pub mod navigate;` in lib.rs.
Tests: `cargo test -p dct-game` -> lib 79 passed, tests/real_board.rs 1 passed.
Clippy: `cargo clippy --workspace --all-targets --locked -- -D warnings` clean; no changes relative to the brief.
Mutations (each applied, lib tests run, restored; git diff clean afterwards):
1. after_board guard -> `if false`: red after_a_level_a_play_button_is_not_pressed..., the_guard_survives...
2. tries += 1 always: red dismissals_do_not_use_up_the_retry_budget only (popups_that_keep_coming_back_hit_the_tap_limit stayed green, brief expected it red; dismiss kinds are never checked against tries, so it can't)
3. no_effect >= 3: red a_button_that_does_nothing_stops_after_two_tries
4. drop no_effect = 0: red a_button_that_works_resets_the_no_effect_count
5. drop retry block: red a_failed_level_is_retried..., a_priced_failure_popup_with_no_thanks...
6. drop MAX_TAPS check: red popups_that_keep_coming_back_hit_the_tap_limit
7. drop after_board = true: red after_a_level_..., the_guard_survives...
Files: crates/dct-game/src/navigate.rs (new), crates/dct-game/src/lib.rs.
Concerns: brief's mutation 2 expectation slightly off (see above); cargo fmt --check shows pre-existing diffs in board.rs (not touched).

## Fix round 1
Done test-first (new/adjusted tests were red before the fixes: 5 failed). All in navigate.rs.
- Important: `steps >= o.max_steps` check in the NoGrid/ClassesChanged arm -> StepsDone (test a_spent_step_budget_stops_before_retrying_the_next_level).
- Minor 2: `free_play` flag; Play right after a changed Try again is not counted or checked; reset on entering a board; doc comment updated. Tests: a_retry_through_the_start_box_is_one_try (tries 1), a_second_failure_after_a_retry_is_refused_when_tries_is_one, two_retries_fit_in_a_budget_of_two_tries (added so the counting mutation is observable).
- Minor 1: nav "stopped" record before both TriesDone breaks and TapLimit; the tries-limit and tap-limit tests assert it (helper assert_stopped_with_texts).
- Minor 7: see_error on the World fake; test an_error_from_reading_the_screen_stops_with_it_and_taps_nothing.
Commands: `cargo test -p dct-game` -> lib 84 passed, real_board 1 passed. `cargo clippy --workspace --all-targets --locked -- -D warnings` clean (fixed my own test helper's `.last()` to `rfind`).
Mutations: remove steps>=max_steps check -> red a_spent_step_budget_stops_before_retrying_the_next_level. Free-play Play counted again -> red two_retries_fit_in_a_budget_of_two_tries (the tries:1 tests alone did not catch it, hence the extra test). Restored; tests green.
