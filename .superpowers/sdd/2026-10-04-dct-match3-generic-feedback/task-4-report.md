# Task 4 report
RED: with test changes but without impl, `cargo test -p dct-game play_tests` fails to compile (`numbers` not found). GREEN: 69 play_tests pass.
Mutations: removing `.trim()` -> numbers_keeps_only_whole_number_elements_in_order red; removing `progress_prev = progress_after` -> added test the_after_of_one_step_is_the_before_of_the_next red. Both reverted.
Files: crates/dct-game/src/play.rs, play_tests.rs, tests/game_cli.rs.
Extra: tests/game_cli.rs fake dco panicked on the new `see` call; it now answers `see` with an unsupported error and the halted test expects tools ["see","read_grid","swipe"] (initial read happens before the loop).
Reads happen only for non-dry-run, in the moved / no_change branches (not stopped). Workspace tests and clippy -D warnings green. rustfmt not installed for 1.99.0, not run.

## Fix round 1
- dco.rs: `no_see_text` flag (like `no_show_status`); `see_text` returns "unsupported" immediately once set, set after dco_timeout/dco_too_old. Test `a_see_text_timeout_is_not_paid_twice` (RED first: second call returned dco_timeout); mutation removing the flag check -> red.
- play.rs `numbers`: only non-empty all-ASCII-digit trimmed texts; new test `numbers_rejects_signs_separators_non_ascii_digits_and_overflow` ("+7","1,234","-5","٣","", 25 digits dropped, "007"->7); mutation removing digit guard -> red.
- Command: `cargo +1.99.0 test --workspace --locked` -> 2211 passed, 0 failed; clippy -D warnings clean.
