# Final fix wave report

Commits (oldest first): 270e14d play loop, 5f91d68 dco client, text/log/cli commit, daemon/skill commit, spec docs commit, clippy test fix.

Red/green: play_tests were written first and run red (6 failed incl. steps==1 on refused swipe, StillMoving instead of NoGrid, flat features, backwards clock overflow panic) then green. For the dco client tests (I4, I5) and the text/log/skill/daemon tests I wrote tests and code in the same step, so I did not capture a separate red run for those; they exercise the new codes/paths and pass.

- I1: play.rs counts a step only after the swipe succeeded; record has `swiped` (false when refused, true after). text.rs step_line: refused -> "想走 A ↔ B，这一步没划成" (no "第 N 步", no "消 N 颗"); swiped but unsettled -> "第 N 步：…划了以后没等到画面停下" (no cleared claim). Tests: play_tests (halted_on_swipe..., a_swipe_that_happened..., settle_never_finishing...), text tests, game_cli halted test (stdout + "走了 0 步" + log). Side effect: a dry run now reports steps 0 (nothing swiped); game_cli dry-run test updated.
- I2: settle tracks readable frames; at the 8 s deadline with none readable it returns Stop::NoGrid(dco message). Tests: nothing_readable_after_a_swipe_is_no_grid..., a_screen_that_never_stops_changing_gives_up.
- I3/M6: should_install_skill pure fn + unit test; installer thread under cfg(macos), started in run_with_manager right after bind_private. `dct game play` unchanged (cfg(unix)).
- I4: DcoClient::connect_with_timeout (connect = 10 s); read/write timeouts map to dco_timeout with the specified sentence; text.rs passes it through. Test: a_dco_that_never_answers_times_out... (200 ms).
- I5: -32602 + "unknown tool" -> dco_too_old; other protocol errors keep numeric code in the message; PermissionDenied on connect -> dco_blocked; dco_refused text is fixed (no raw JSON). Tests in dco_tests.rs and text.rs dco_errors_are_translated.
- I6: candidates nest `features{...}` in play.rs; text.rs and tests read features.*; spec formula amended to (lowest_row + 1) / rows (the spec's log example already showed "features").
- M1: LogFile::open creates/appends the file at start, returns a plain Chinese String error with the path; cli exits 1. Tests: log.rs unit test, game_cli an_unwritable_log_refuses_to_start....
- M2: saturating_sub on every clock subtraction in play.rs (cli.rs has none). Test: a_clock_that_goes_backwards_does_not_panic.
- M3: Stuck wording now neutral; test the_stuck_sentence_does_not_claim_two_tries.
- M5: run_id (8 hex from time ms XOR pid) on every record and the stop line, stop line also has time_ms. Checked in game_cli halted test.
- M7: skill card written via temp file + rename (temp removed on failure); test installing_leaves_no_temp_file_behind.
- M8-card: "每步很快"; test the_card_does_not_promise_a_speed_we_have_not_measured; front-matter test still passes.

Results: cargo test --workspace all green (lib 1659 passed, game_cli 7, dct-game 43). cargo clippy --workspace --all-targets --locked -D warnings clean (after fixing two lints in my tests). cargo check --target x86_64-pc-windows-msvc -p dct: finished, only the pre-existing unused-variable warning (student_projects.rs).
Not done: nothing from the brief. Parked items untouched.
