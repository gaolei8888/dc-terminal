# Task C report: training rows v2 (auto-training spec task 1)

Status: DONE. Commits on `explore-and-learn` (local only):
- `202fab9` Training rows v2: who answered, whether the step was confirmed closer to the goal, label flag
- `64ac16b` Docs: training row v2 format and label rule as implemented; E1 numbers fixed

## Row format

Before (v1, no version field):
`ts_ms, run_id, platform, state, options[[key,text]], jeff[key,conf]|null, answer, by, corrects_jeff`.
Written immediately when an answer led to an unseen screen (or `done` on publish).

After (v2, additive; v1 fields unchanged):
`v: 2`, `shown` (shortlist keys sent to Jeff, in order, subset of `options`, <= 20; when Jeff was skipped
on that screen, the list it would have been sent), `step` (index of the first step of this answer in
`runs/<run_id>/trace.json` `steps`; `null` for `done`), `run_outcome` (`published|declined|halted|stopped`),
`outcome` (`good|bad|unknown`), `label` (bool).
Trace: new optional `outcome` (same enum), `#[serde(default, skip_serializing_if = "Option::is_none")]`.

Write timing (the choice): rows are buffered in memory during the run (`act::pending_row`) and written
once by `explore_from` after the loop returns (`act::write_rows`), so `outcome`/`label` are final and the
file is never rewritten. Cost: if the process is killed mid-run, that run's rows are lost (the trace is
also only saved at the end, so they would be unlinkable anyway).

## Label rule (`trace::grade`)

"Confirmed closer to the goal" = `good`: dco returned done, the screen moved to a screen unseen in this
run, the step is still `ok: true` in the FINAL trace (not reset by a later loop-back), and the run
published. `done` rows are only recorded on publish, so always good.
`bad`: screen did not move to a new screen (only possible for the user-taps-publish row on
`publish_by_user_only` platforms) or the step later became `ok: false`. Independent of run outcome.
`unknown`: still on the path but the run ended stopped/halted/declined.

label = `by: user` (always) OR (`by: model` AND outcome good). `by: jeff` never (outcome still recorded).
`corrects_jeff` unchanged. `trace::is_label(&Value)` for readers: v2 uses `label` (jeff never), rows
without `v` count only `by: user`.

Redaction unchanged: rows still built from already-redacted state/options; halted screens never produce
a row (test asserts halted-screen text absent); files 0600 via the same `open_private`.

## Changes

- `crates/dct-explore/src/trace.rs`: `RunOutcome`, `StepOutcome`, `QUESTION_ROW_V`, new row fields,
  `Trace.outcome`, `grade`, `is_label`; 3 unit tests.
- `crates/dct-explore/src/explore/act.rs`: `write_row` replaced by `PendingRow`/`pending_row`/`write_rows`.
- `crates/dct-explore/src/explore.rs`: `explore_from` = `drive(...)` + `settled()` (sets trace outcome)
  + `write_rows`; `Outcome::trace()/kind()`; early stops in `explore` also settled.
- `run.rs`, `draft.rs`: `outcome: None` in Trace literals.
- `tests/explore_loop.rs`: 6 integration tests.
- Docs: auto-training spec section 1 rewritten as implemented, task 1 marked done (`202fab9`),
  E1 numbers = E1 doc (242 rows / 16 steps / 164 s, 86.7% EN / 85.0% ZH) marked a single small smoke run
  conflicting with another smoke (226 model-written rows -> 67.5% vs v1.2 75.8%), controlled rerun in
  progress; open question 7 removed (renumbered). README.md / README.zh-CN.md and judge design doc
  row-format paragraph updated. `src/explore_cli.rs` needed no change (it saves the outcome-bearing trace).
- Replay (`replay.rs`) writes no rows (unchanged; spec already notes it).

## RED / GREEN

RED: `~/.cargo/bin/cargo test -p dct-explore` after adding tests:
```
error[E0432]: unresolved import `RunOutcome`
error[E0432]: unresolved import `StepOutcome`
error[E0433]: cannot find `RunOutcome` in `trace` (x4)
error[E0609]: no field `outcome` on type `&Trace` (x4)
error[E0609]: no field `outcome` on type `trace::Trace`
error: could not compile `dct-explore` (lib test) due to 3 previous errors
error: could not compile `dct-explore` (test "explore_loop") due to 8 previous errors
```
GREEN: same command: lib 82 passed; explore_loop 66 passed; replay 26; hands_client 3; 0 failed. New tests:
`the_label_rule_counts_users_and_confirmed_model_steps_only`, `old_rows_without_v_only_count_user_answers`,
`old_traces_without_outcome_still_load`, `model_answers_on_the_path_of_a_published_run_are_labels_with_outcome_good`,
`a_model_answer_that_later_loops_back_is_not_a_label`, `a_model_answer_still_on_the_path_of_a_stopped_run_is_unknown_and_not_a_label`,
`user_answers_are_labels_and_jeff_answers_never_are`, `a_halted_run_records_its_outcome_on_the_trace_and_the_rows`,
`a_declined_publish_records_declined` (all assert no param value on disk, v2 shape, shown subset, step link).
Note: RED was compile-level (new types/fields did not exist); behaviour tests passed on first GREEN build.

## Full check

- `cargo test --workspace -- --test-threads=1`: 2424 passed, 0 failed, 5 ignored (was 2415; +9). Flaky tests did not fail.
- `cargo clippy --all-targets`: 0 warnings.
- `rustfmt --check --edition 2021` on the 6 changed .rs files: clean.
- Windows `cargo check --workspace --target x86_64-pc-windows-msvc`: only the pre-existing `src/student_projects.rs:669` unused `path` warning.

## Residual risks

- Rows are lost if dct is killed mid-run (documented in code and spec).
- "Unseen screen" relies on dco scene_hash; a hash collision/instability would mislabel (pre-existing).
- `shown` is the shortlist even when Jeff was not asked that round (documented); `jeff` may hold an
  earlier failed Jeff answer on that screen (v1 behaviour).
- Model rows in published runs that reached a new screen but whose screen was later revisited by a
  different path keep `good` only if the trace still marks them ok — consistent with the draft rule.
- Spec task 3 (`src/jeff_train/rows.rs`) not started; it should use `trace::is_label`.

## Fix round 1 (commit `d356e8d`, on top of Task A's `c871fda`; A's changes and tests kept)

- Row field `jeff_asked: bool`: true when `Ladder::decide` actually asked Jeff this round (it asks iff
  `!tries.jeff_failed` on that screen; a Jeff that returns nothing still counts as asked). Computed in
  `drive` before `decide`, passed to `pending_row` together with the shortlist.
- Failed row write is no longer silent: `write_rows` prints `dct: 训练题没写下（<run_id>）：<io error>`
  to stderr (no state/options text, so no param values).
- Spec baseline table now gives each figure its source: E1 doc run (242 hand-labelled rows, 16 steps,
  164 s): v1.2 71.7% EN / 58.3% ZH vs adapter 86.7% / 85.0%; other smoke run (226 model-written rows,
  15 steps, 178 s): v1.2 75.8% vs adapter 67.5%. Both marked small smoke runs that conflict; controlled
  rerun in progress, no swap on either.
- Spec states ruling R18: model rows in a Declined run are `unknown`, not labels (deliberately
  conservative; `run_outcome` kept so a trainer can relabel). Also: a user answer is a label even when
  its outcome is bad (trainers may filter `outcome == bad`); "did nothing" answers (unchanged screen,
  loop-back, dco failed) are not recorded as negatives, because the cause is often not a wrong choice
  (page loading, popup, missed tap) and the same screen later gets a correct-answer row. `jeff_asked`
  added to the field table.

RED: `cargo test -p dct-explore --test explore_loop` with new `jeff_asked` assertions:
```
test model_answers_on_the_path_of_a_published_run_are_labels_with_outcome_good ... FAILED
test user_answers_are_labels_and_jeff_answers_never_are ... FAILED
test result: FAILED. 68 passed; 2 failed
```
GREEN: lib 82, explore_loop 70, hands_client 3, replay 26 passed; 0 failed.

Full check: `cargo test --workspace -- --test-threads=1` 2429 passed, 0 failed, 5 ignored (flaky tests
did not fail); clippy 0 warnings; rustfmt --check clean on the 4 changed .rs files; Windows check only
the pre-existing `src/student_projects.rs:669` warning. No test for the stderr log (would need forcing
an unwritable path; behaviour is a single eprintln).
