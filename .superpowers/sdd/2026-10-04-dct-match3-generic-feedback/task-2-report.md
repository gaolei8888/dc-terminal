# Task 2 Report: Tell the model only what can be seen

## Status
**DONE**

## Evidence
✓ Test `describe_counts_rows_and_columns_from_one_and_names_only_what_can_be_seen`: NEW test passes, asserts no "引爆" in output
✓ Test `prompt_lists_numbered_candidates_goal_and_failed_swaps`: Modified to assert "特殊标记不一定准" present, now passes
✓ Workspace tests: 97 passed (relay), 5 passed (dct-game ask), 0 failed
✓ Clippy: Zero warnings

## Commits
- `d8c0a0b` fix(game): do not tell the model about triggered special candies (a guess from a marker that cages also set); warn that markers may be wrong

## Files Changed
- `crates/dct-game/src/ask.rs`
  - Removed `if f.triggered > 0 { ... }` block from `describe()` function (lines 75-77)
  - Added marker warning to `prompt_text()`: "提醒：棋盘上的特殊标记不一定准，不要因为它就选某一步。\n"
  - Renamed and updated test to `describe_counts_rows_and_columns_from_one_and_names_only_what_can_be_seen`
  - Added assertion to `prompt_lists_numbered_candidates_goal_and_failed_swaps` that prompt contains "特殊标记不一定准"

## Concerns
None. The file `src/game/text.rs` contains a separate `step_line()` function that also outputs triggered special candies, but it is for user-facing game replay feedback, not model input. That function is correctly left unchanged.

## Notes
- No other tests in the workspace assert the old "引爆" wording from `describe()`
- All 5 ask module tests pass with the new implementation
