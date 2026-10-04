# Final-fix report

Commits: F1 fcddadc, F2 (ask once per stall), F3 (lock fallback), F4 (wording/help) -- see `git log -4`.

## F1 dco.rs
`request()` now loops on `read_line` until the reply `id` equals the id sent; other ids and id-less notifications are skipped. A per-request deadline (new `timeout` field) keeps the overall timeout. Test `a_late_reply_to_a_timed_out_call_is_not_taken_for_the_next_calls_reply` (fake answers "slow" after 300 ms, client timeout 200 ms).
RED (change stashed): FAILED. GREEN: passed. Mutation = removing the id check (the stash run) is red.

## F2 play.rs
`stalled_now` computed before the ask; if the ask was triggered only by the stall (not ask_always, not had_failed_here), `no_progress = 0` right after (answered or not). The `stalled` flag uses `stalled_now`.
Test `a_stall_asks_the_model_once_then_counts_again`: 14 flat steps -> stalled and asked at indices [6, 12] exactly twice. Mutation (remove reset): red, stalled = [6..13].

## F3 play.rs
`pick` falls back (`or_else`) to the first candidate not exactly refused on this board (ignoring `locked`); all exactly refused -> Stuck. `offered` unchanged (still excludes locked). Comment on MAX_REFUSED_IN_A_ROW fixed (F4).
New test `a_move_blocked_only_by_a_lock_is_still_tried_and_stuck_means_all_refused` on board SHARED (2 candidates sharing a cell): 2 swipes then Stuck. Mutation (remove fallback): red.
Existing tests updated (board A has 4 candidates; now all 4 get tried before Stuck, previously 2):
- `two_unmoved_swipes_stop_and_the_second_try_is_a_different_move` -> renamed `unmoved_swipes_try_every_different_move_once_then_stop`: 4 distinct swipes, 4 no_change, Stuck (stronger: pairwise distinct).
- `permuted_ids_on_an_unchanged_screen_settle_as_no_change`: no_change count 2 -> 4.
- `a_changed_odd_flag_alone_is_not_a_changed_board`: no_change count 2 -> 4.

## F4
Comment on the locks (cleared when a move succeeds); `src/game/skill.md` sentence conditional on `--ask-model`, explains `--goal-number N` (plain numbers in order of appearance, first = 1, see `progress` in the log), redundant clause removed; `src/main.rs` help now lists `--auto-next` and `--tries 5` like USAGE.

## Commands
`cargo +1.99.0 test --workspace --locked`: all ok (dct-game 184 passed, root lib 1690 passed, no flaky failures this run).
`cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings`: clean. `git diff --check`: clean.
