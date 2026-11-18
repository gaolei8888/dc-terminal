# Task 1 Report: Prompt and Reply Parser

## Status: DONE

## Test Results: GREEN

All 5 tests passing:
- `ask::tests::board_text_uses_letters_in_reading_order_and_hash_for_fixed` ✓
- `ask::tests::describe_counts_rows_and_columns_from_one_and_names_what_happens` ✓
- `ask::tests::prompt_lists_numbered_candidates_goal_and_failed_swaps` ✓
- `ask::tests::parse_reply_takes_the_first_number_inside_range` ✓
- `ask::tests::parse_reply_zero_out_of_range_empty_and_noise_are_none` ✓

Clippy: No warnings (checked with `-D warnings`).

## Files Changed

1. **Created**: `crates/dct-game/src/ask.rs` (180 lines)
   - `board_text()`: Renders grid as letters A-Z (by first-occurrence order) with '#' for fixed cells
   - `describe_move()`: 1-indexed row/column description in Chinese
   - `describe()`: Full candidate description with outcome effects (cleared, bombs, triggers)
   - `prompt_text()`: Formats numbered candidates, goal, and failed moves for model
   - `parse_reply()`: Extracts 1-based index from text, returns 0-based; reason text after number; handles edge cases (0, out-of-range, overflow, no digits)
   - Test module: 5 tests covering all functions with edge cases

2. **Modified**: `crates/dct-game/src/lib.rs`
   - Added `pub mod ask;` declaration

## Mutation Checks: PASSED

1. Changed `parse_reply()` range check from `(1..=n)` to `(0..=n)` → test failed ✓
2. Changed `board_text()` fixed-cell check from `fixed.contains(c)` to `false` → test failed ✓
3. Reverted both; all tests pass ✓

## Concerns

None. All requirements from brief met:
- Pure functions, no I/O
- Correct 1-based → 0-based index conversion in `parse_reply()`
- Letter assignment by appearance order (not class ID) in `board_text()`
- Chinese user-facing text throughout
- Test coverage includes boundary cases (0, overflow, empty, noise)
- No clippy warnings

## Commit

```
b1601d5 feat(game): text prompt and reply parser for asking a model to pick a move
```

Implementation ready for Task 2 (advisor trait + network integration).
