# Task 3 report
RED: with play.rs reverted, `cargo test -p dct-game play_tests` -> E0425 `changed_cells` not found.
GREEN: play_tests 65 passed; workspace all green (dct 1683 passed); clippy -D warnings clean.
Mutations:
- brief's "COLOUR_SAME_DE = 0.0" does NOT turn the test red (identical colours give delta_e 0, not > 0). Brief's claim is wrong; replaced with:
  - compare class ids instead of colours -> changed_cells_compares_colours_not_class_ids FAILED
  - COLOUR_SAME_DE = 1e9 -> same test FAILED (second assertion)
- `?` -> unwrap_or([0,0,0]) -> changed_cells_is_none_when_colours_are_missing FAILED
All reverted.
Files: crates/dct-game/src/play.rs, play_tests.rs. ClassInfo.rgb was already pub.
Note: changed_cells compares by position on the same grid size; sizes differing -> None.
