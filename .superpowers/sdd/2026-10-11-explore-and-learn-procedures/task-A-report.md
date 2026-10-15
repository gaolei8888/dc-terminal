# Task A report: R17 two-stage publish

Commit: `67d358c` (local only, branch explore-and-learn). Only crates/dct-explore files were touched; no root files.

## How the loop tells a confirm step apart

- After a publish tap (dct tap or by-user tap) lands on a **new** screen, `Screen::landed` decides:
  - halt screen -> `Halted` (unchanged);
  - **sent** if the screen has no tappable publish-word UI element (same exact-match rule as `all_options`, value elements ignored), or shows the platform's `publish_done` text and that field is not in `unverified`. Value elements don't count, so post text can't fake it;
  - otherwise **pending**: `Run.pending = Some(<first publish word>)` and the loop keeps going.
- While pending:
  - **every Tap / TapIcon counts as a publish tap** (`opt.publish = true`). "继续" / "Continue" / "确定" aren't publish words, so this is the rule that catches them. Each one asks the user first, and the question names the word ("已经点了「Post」，tiktok 还要再确认一下：下一下要点「Post now」…"). On `publish_by_user_only` platforms the user taps it. Fill asks first through the 6b path. Nothing is tapped without asking.
  - the time and step budgets are waived. Unchanged-in-a-row still stops the run, and there is a backstop of `MAX_AFTER_PUBLISH = 10` actions. Any stop while pending is prefixed "点过「X」，但还没看到发出去（平台可能还在等确认）：…".
  - any action that lands on a new screen is checked again with `landed`.
  - `Done` while pending asks the user "发出去了吗？". Yes -> `Published`. No -> the pick counts as a failure on this screen (the ladder escalates), and the unchanged counter goes up.
  - if the user declines the confirm tap -> `DeclinedAtPublish`. `dct do` (src/explore_cli.rs, not touched) prints "你没让发，停在了发布前；这次不写草稿" and makes no "posted" claim.
  - looping back to a screen already seen restores `pending` from that screen's snapshot. For example, Cancel back to the composer clears it.
- `all_options` and the icon-publish check no longer require the post text on screen once `prog.published` is set. The confirm dialog usually hides the text.

## Draft

Both publish taps stay `publish: true` and both are copied into the draft. The completion marker (`完成标志`) now follows only the **last** publish step. Before, it went after every publish step, which would make replay wait for "posted" text after stage 1. `maybe_one_off` doesn't flag the confirm tap, and it still flags a non-dismiss tap after the last publish. The dcv body format is unchanged.

## Tests (tests/explore_loop.rs, plus one in draft.rs)

New: `x_single_tap_publish_ends_published_with_one_publish_step` (1), `tiktok_two_stage_publish_asks_again_and_learns_both_taps` (2), `a_chinese_confirm_dialog_with_continue_is_asked_and_learned` (3), `a_verified_publish_done_ends_at_once_even_with_a_post_button_left` (4), `an_unverified_done_line_with_a_post_button_left_asks_before_counting_it_sent`, `declining_the_second_confirmation_is_declined_not_published` (5), `the_ten_minute_budget_passing_during_confirmations_does_not_stop_a_two_stage_publish` (6), `on_a_by_user_platform_the_confirm_tap_is_left_to_the_user_too`, `a_confirm_dialog_that_never_goes_away_stops_without_claiming_it_was_sent`, `draft::a_two_stage_publish_keeps_both_taps_and_marks_only_the_last`.

No existing test needed changing. The existing "lands on a new screen -> Published at once" tests use a screen with no Post button left, so they still hold under the new rule.

RED (`cargo test -p dct-explore --features fake --no-fail-fast`, before the fix):
```
test draft::tests::a_two_stage_publish_keeps_both_taps_and_marks_only_the_last ... FAILED
test result: FAILED. 78 passed; 1 failed
test a_chinese_confirm_dialog_with_continue_is_asked_and_learned ... FAILED
test an_unverified_done_line_with_a_post_button_left_asks_before_counting_it_sent ... FAILED
test declining_the_second_confirmation_is_declined_not_published ... FAILED
test on_a_by_user_platform_the_confirm_tap_is_left_to_the_user_too ... FAILED
test tiktok_two_stage_publish_asks_again_and_learns_both_taps ... FAILED
test result: FAILED. 54 passed; 5 failed
```
Before the fix, (1), (4) and (6) passed, but only because the code returned Published right after stage 1. To show the budget waiver is load-bearing, I replaced the waiver condition with `if true`: (6) then fails. I reverted that afterwards.

GREEN: lib 79 passed, explore_loop 60 passed, hands_client 3, replay 26.

## Full check

- `cargo test --workspace -- --test-threads=1`: **2415 passed, 0 failed, 5 ignored**. No flaky failures this run. The count includes Task B's concurrent tests.
- `cargo clippy --all-targets`: 0 warnings, after fixing 3 doc-indent warnings I had introduced.
- `rustfmt --check --edition 2021` on all 7 changed files: clean.
- `cargo check --workspace --target x86_64-pc-windows-msvc`: only the pre-existing `src/student_projects.rs:669` unused `path` warning.

## Residual risks

- **X today**: X's `publish_done` is unverified and the home page keeps a left-nav "Post" button, so a real X post will usually stay pending. If Jeff then picks "done", the user is asked "发出去了吗" (fine). If Jeff instead picks the inline Post, the user is asked to confirm a second publish. Declining gives `DeclinedAtPublish`, so the post went out but no draft is written. Verifying X's `publish_done` on real hardware removes this. The same applies to any platform whose home screen keeps a publish button.
- A confirm dialog that **hides** the original publish button and offers only non-publish words ("继续/取消") counts as sent after stage 1. That is the user's rule taken literally. The fix there is to add the confirm word to `publish_words` or verify `publish_done`.
- While pending, a Cancel/dismiss tap is also treated as publish-class. It is asked first, but if the user says yes and it lands on a new screen with no publish button, that screen counts as sent.
- Replay `publish_step` still stops before the **first** publish step, which is correct for preview and by-user platforms. On by-user replay, the user handles the second tap.
- The declined message in explore_cli says "停在了发布前" even when stage 1 was already tapped. It is slightly imprecise but doesn't claim the post went out. It's a root file (Task B's), so I left it alone.

## Fix round 1 (commit `c871fda`, on top of Task C's 202fab9 / 64ac16b; C's code and tests kept)

Changes:
- **Confirm tap flagged as one-off** (draft.rs): a publish step that follows an earlier publish step is now pushed to `maybe_one_off`, unless it is a dismiss word. The first publish is never flagged.
  - Updated expectations: the TikTok and Chinese loop tests now expect `maybe_one_off == [4]`; `draft::a_two_stage_publish_keeps_both_taps_and_marks_only_the_last` now expects `[6, 7]`.
  - The old fixture `a_non_dismiss_popup_step_is_flagged_for_the_user` now records "Post now" as `publish: true`, matching what the loop records; it still expects `[3, 5]`.
- **Honest decline in `dct do`** (src/explore_cli.rs, a root file, edited with permission this round): when `DeclinedAtPublish` and the trace already has an `ok && publish` step, the message is "点过「X」以后你没让再点；帖子可能已经发出去了，请到平台上看一眼；这次不写草稿". Without such a step the old message is kept.
- **Same-word warning**: while pending, if the next publish word folds to the same text as the first publish tap (X's left-nav Post), the question no longer says "还要再确认一下". It warns "再点可能会再发一次". The by-user variant asks the user to tap it themselves only if they really want to.
- **Minor fixes**:
  - while pending, `landed` is checked only after publish-class taps (dct or user); scroll or wait reaching a new screen no longer counts as sent;
  - `back_to` resets `after_publish` to 0 when the restored snapshot has no `pending`;
  - Attach while pending asks first;
  - declining a Fill or Attach while pending gives `DeclinedAtPublish`, not Stopped.

RED (before the fix): lib 80 passed / 2 failed (both draft tests); explore_loop 64 passed / 6 failed (the two updated expectations plus `tapping_the_same_publish_word_again_warns_it_may_post_twice`, `a_scroll_while_pending_reaching_a_screen_without_post_is_not_sent`, `declining_a_fill_while_pending_is_declined_at_publish`, `going_back_out_of_the_confirm_dialog_resets_the_after_publish_count`). The last one failed for the intended reason, stopping at the 10-action cap ("点过发布以后又走了 10 下"). The CLI test `explore_cli::tests::declining_after_a_first_publish_tap_does_not_say_it_stopped_before_publishing` failed: its output showed "下一下要点「Post」把它发出去" followed by "你没让发，停在了发布前".

GREEN: dct-explore lib 82, explore_loop 70, hands_client 3, replay 26; root `explore_cli` tests 36 passed.

Full check: `cargo test --workspace -- --test-threads=1` **2429 passed, 0 failed, 5 ignored**. Clippy: 0 warnings. rustfmt `--check`: clean on draft.rs, explore.rs, explore/run.rs, tests/explore_loop.rs and src/explore_cli.rs. Windows check: only the pre-existing `src/student_projects.rs:669` warning.

Notes:
- **Attach while pending has no loop test.** It can't be reached today: the first publish already requires every file to be attached, and `attached` only shrinks through `back_to`, which also clears `pending`. The guard is there for safety.
- **Platform-table item (recorded, not fixed):** a confirm dialog that hides the original publish button and offers only 继续/取消 counts as sent after the first tap. That is the user's rule taken literally. Per platform, the fix is to add the confirm word (e.g. 确认发布) to `publish_words`, or to verify `publish_done` on real hardware.
- **X today:** every real X run still goes pending, because of the left-nav Post and the unverified `publish_done`. The user now gets the same-word warning, and declining gives the "可能已经发出去" message. Verifying X's `publish_done` removes the pending state entirely.
