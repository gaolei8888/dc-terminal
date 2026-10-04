# Task 2 report
Status: DONE_WITH_CONCERNS
Files: crates/dct-game/src/{ask,play,play_tests}.rs, src/game/{advisor,bench,cli}.rs
Tests: workspace 1700 passed (after adding 5 new: 4 brief tests + a_failed_ask_is_not_counted_in_the_summary + play_tests token record test); clippy -D warnings clean.
Mutations: (1) `counted < asks` branch disabled -> summary_mixes_counted_and_uncounted_asks_honestly FAILED. (2) asks.fetch_add moved before the call -> a_failed_ask_is_not_counted_in_the_summary FAILED. Both restored.
Concern: I wrote implementation and tests in one pass, so the compile-failure RED was not observed separately; mutations stand in for it.
cli.rs: summary printed to stdout after the stop line (also when the stop line goes to stderr).
