# Task 1 report

- Created crates/dct-game/src/screen.rs (extracted verbatim from the brief's code block, no changes) and added `pub mod screen;` to lib.rs.
- Tests: `cargo test -p dct-game screen::tests` -> 13 passed (the plain `screen` filter shows 17 because 4 play_tests names also match).
- Clippy (`cargo clippy --workspace --all-targets --locked -- -D warnings`): clean, no changes needed.
- Mutations (each restored; final diff vs original identical):
  - plays.len()==1 -> !is_empty(): two_exact_play_buttons_... red
  - MONEY_WORDS minus gold / gem / bar / checkout / purchase: every_money_word_counts red
  - minus buy: every_money_word_counts and a_price_without_a_safe_close_is_money red
  - minus the diamond mark: a_price_without_a_safe_close_is_money red
  - delete lives_out check: lives_out_beats_everything_below_it red
  - swap dismiss/money blocks: a_price_with_an_exact_no_thanks_... red
  - word-split in exact(): 5 red (matching_ignores..., no_thanks, ad, retry, two_exact_play)
- Files changed: screen.rs (new), lib.rs (+1 line).
- Concerns: none. `cargo fmt --check` reports diffs in pre-existing board.rs (not touched; CI uses clippy only per ruling).
