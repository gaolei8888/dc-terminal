# Final fix wave — report (base 5ed4666, head 360d010)

Commits (oldest first, none pushed, no AI attribution):

| commit | items |
|---|---|
| 0dfcceb | 11 |
| 277b5ef, bfdf93c | 2, 3 (bfdf93c only rustfmt-formats the 277b5ef test; 277b5ef went in before the fmt check ran) |
| df88ae6 | 4 (repair draft) |
| df7564f | 1, 6 |
| a4e885a | 5 |
| 2ff3b4e | 12, 13 |
| 2a39b6e | 7 |
| 79e3b29 | 9, 10, D13 |
| 360d010 | D10 |

RED/GREEN logs in this session's scratchpad `log/` (red-*.txt / green-*.txt); the key lines are quoted below.

## CRITICAL 1 — icon names: FIXED
- `gate.rs`: new `icon_name_shape` (no platform: not empty, ≤ 40 chars (`MAX_NAME_CHARS`), only alphanumeric + space + a few punctuation marks, so every control/format/bidi char is refused; generic never-tap + tier ≥ Critical refused) and `check_icon_name(name, &Platform) -> IconName::{Ok, Publish, Refused}` (adds the platform's never_tap; a publish word as a whole word (any containment for CJK) → `Publish`). `tappable` now = platform never_tap + `tappable_anywhere`. Behaviour of `tappable` unchanged.
- `explore.rs`: after naming a `TapIcon`, `Refused` → `Stopped` (name not echoed); `Publish` → `opt.publish = true` (asks `Confirm::publish_ok`; on `publish_by_user_only` the user taps it), refused when the text/file params are not all on screen yet. The by-user summary names the icon name.
- `explore/act.rs` help answers: a user-given name is checked; `Refused` → stop + cancel_run; a publish word only when the option was already the confirmed publish. `answer_for` now returns `Result<HelpAnswer, String>`.
- `replay.rs` help answers: `Refused` or `Publish` → cancel_run + `Stopped`.
- `src/explore_model.rs::read_pick`: icon name must pass `icon_name_shape`, else None (read as stuck).
- Tests: explore_loop `an_icon_named_with_a_publish_word_asks_before_it_is_tapped`, `..._is_published_after_yes`, `..._on_a_by_user_platform_is_never_tapped`, `an_icon_named_with_a_banned_word_or_a_bad_name_is_refused` (Follow, Boost, 关注, Delete, 63 chars, U+202E, U+200B), `an_icon_named_publish_before_the_post_is_on_screen_is_not_tapped`, `a_help_answer_naming_an_icon_with_a_banned_or_publish_word_is_refused`; replay `a_replay_help_name_that_is_banned_or_a_publish_word_is_refused`; gate `icon_names_follow_the_same_rules_as_screen_text`; explore_model `an_icon_name_that_is_banned_too_long_or_hidden_is_read_as_stuck`.
- RED: `cargo test -p dct-explore --test explore_loop icon` → 6 FAILED, every one panicking in FakeHands `run_step`/`answer_run` (it tried to tap); replay → `answer_run 被调用的次数比脚本里排的 answers 多`; explore_model → panicked `"Follow"`. GREEN: all pass (two of them only after item 6, which they also depend on).
- Note: a newline in a name is flattened to a space by `scrub.line` before the check, so "Add\nphoto" becomes "Add photo" (allowed). That is why the explore test uses U+200B instead.

## IMPORTANT 2 — popup `应用`: FIXED
- `draft.rs`: body is `应用: {p.name}`. Fixtures updated (draft.rs, learn.rs ×2). The card never showed 应用, so the D13 wording did not change. Also added: a 看到 text with a URL (`://` or `www.`, like dc-vault recipe.rs `has_url`) goes to notes instead of becoming a rule.
- RED: `a_dismiss_tap_becomes_a_popup_draft_not_a_step` left `应用: Google Chrome` vs right `应用: tiktok`; contract test `x: 应用 \`Google Chrome\` 不过 check_platform`. GREEN: 16/16 draft tests.

## IMPORTANT 3 — slug names: FIXED
- `draft.rs`: `name_part` maps `_` → `-`; new `pub fn procedure_name(p, video)` is used by draft and by `explore_cli::drive` for the lookup, so both use the same name (`promote-wechat-mp-post`). Popup names use `name_part`. `--platform` / 应用 keep `wechat_mp`.
- Contract test `every_generated_name_and_popup_body_passes_dcvs_checks` copies the dc-vault checks: `is_slug` (src/note.rs:302), `check_platform` (src/procedure.rs:101), and the popup body rules (src/popup.rs:296-400: 应用 first, 看到 4–200 chars and no URL (popup.rs:20-23, recipe.rs:137), 点 ≤ 40 chars). It runs them for every seed platform, post and video, and two popups (English and Chinese). It also checks that 应用 equals the platform name, which `dcv popups --app` filters on (popup.rs:487).
- RED: `left: "promote-wechat_mp-post" right: "promote-wechat-mp-post"`; explore_cli replay fixture with the new name → FakeHands panic (explored instead of replaying). GREEN: all draft and explore_cli tests pass.

## IMPORTANT 4 — repairs: repair-path draft FIXED; "repairs never take effect" NOT A BUG (checked against dcv)
- dcv check (read-only): `dcv approve` of a draft that has `draft_of` replaces the target. dc-vault src/show.rs:966-1006 `approve_procedure` sets `final_name = target` (when the target exists, `check_draft_target` 915-935) and writes the approved file at the target's path. `write_approved` (940-964) deletes the draft's own file ("approve X (replaces Y)"). The same holds when the target is an older declined draft: it still exists, so `same = true`. After dct approves `promote-x-post-0001`, `dcv show promote-x-post` returns the repaired, executable body, and `drive` replays it. No dct-side resolution is needed and nothing is BLOCKED.
- Fixed: `draft()` detects a repair trace (steps 0 and 1 are `from_procedure` `open_url` + `verify_text`). It keeps those two steps once, as written in the procedure, and does not check the login gate against `first_lines`. A prefix without the gate step still has to see the gate. Repair fixtures in explore_cli now include step 2 and fail at step 3.
- Tests: draft `a_repair_keeps_the_procedures_gate_step_once_without_the_gate_on_the_stuck_screen`, `a_repair_whose_prefix_lacks_the_gate_step_still_needs_the_gate_on_screen`; explore_cli `a_late_repair_after_typing_still_writes_a_draft_with_one_gate_step` (exact body), and `a_repaired_draft_is_approved_...` now asserts exactly one verify_text.
- RED: draft test panicked (LoginGateNotSeen); explore_cli `left: 0 right: 1` (no draft written, LoginGateNotSeen) and `left: 2 right: 1` (two verify_text). GREEN: 18 draft and 34 explore_cli tests pass.

## IMPORTANT 5 — unverified publish words: FIXED
- platforms.toml: xiaohongshu `unverified` now includes `publish_words` (plus a header comment).
- `explore.rs` step 6b: when the row lists `publish_words` as unverified, every Tap / TapIcon / Fill asks `Confirm` first. The summary tells the user why ("…的发布按钮叫什么还没核实，dct 认不准哪一个是发布，所以每点一下都先问你…"). On `publish_by_user_only`, once `published` is set, every further tap asks. A no → `Stopped`.
- Tests: platform `every_chinese_rows_placeholder_publish_word_is_marked_unverified`; explore_loop `with_unverified_publish_words_every_tap_asks_first` (yes and no), `on_a_by_user_platform_every_tap_after_publishing_asks_first`.
- RED: `xiaohongshu 的「发布」是凑数的占位…`; both loop tests panicked in run_step (tapped without asking). GREEN: 78 lib + 48 loop tests pass.

## IMPORTANT 6 — publish ends Stopped: FIXED
- A publish tap (or a by-user publish the user confirmed) that lands on a new, unseen screen returns at once. The new screen still goes through `halt`: a login wall → `Halted`, otherwise `Published`. No later `done` pick and no budget check.
- Tests: `a_publish_that_lands_on_a_new_screen_is_published_at_once` (no further looks or steps), `a_slow_publish_confirmation_does_not_turn_a_sent_post_into_a_stop` (clock +11 min inside `publish_ok`). Existing `a_value_split_or_recased…` now expects 2 Jeff questions instead of 3, because no `done` question is asked.
- RED: both panicked in FakeHands `look` (looped again after publishing). GREEN: pass.

## IMPORTANT 7 — private_screen run_id: FIXED
- `DcoError` gains `run_id: Option<String>` (Debug/Clone/PartialEq kept). `client.rs::body_of` copies `body["run_id"]`, which dco puts next to `error` (dc-octo tools/runs.rs `with_help_image`). All constructors are updated (dct-dco, dct-explore, dct-game, src/game/text.rs, tests).
- explore: `act::cancel_parked` on run_step errors (including open_url). An answer_run error cancels `e.run_id` or the run being answered. replay: a run_procedure error cancels `e.run_id`; an answer_run error cancels `e.run_id` or the run.
- Tests: dct-dco `a_private_screen_error_keeps_the_parked_run_id`; explore_loop `a_private_screen_on_a_step|an_answer|open_url_cancels_the_parked_run`; replay `a_private_screen_error_from_run_procedure_cancels_the_parked_run`.
- RED: `no field run_id on type DcoError`; calls lists without `cancel_run`. GREEN: dct-dco 4, explore 51, replay 26, dct-game 108 pass.
- dct-game / src/game/text.rs literals keep those files' existing one-line style. Those exact lines were already in rustfmt's diff before this change (counted: same number of `DcoError {` lines in the diff before and after).

## IMPORTANT 9 — Touch ID bound to the card: FIXED
- `learn::body_sha256` reproduces dcv exactly (dc-vault procedure.rs:397-407: `\r\n`→`\n`, trim `\n`, `sha256:<hex>`). Test vector: sha256("abc"), with and without CRLF/blank lines.
- `apply_confirmation(Yes)`, before Touch ID: refuses hidden characters in the final body or popup bodies. It then `show()`s the procedure and every popup and compares dcv's `body_sha256` with the sha of `w.final_body` and of each popup draft body (`Written.popup_bodies`, new). A mismatch is an error and nothing is approved. Only after that does it call `approve_proc(name, sha)`; popups are approved with the card's sha.
- `procedures::approve_expecting` refuses before the signer/Touch ID if dcv's current `body_sha256` differs from the shown one. `dcv approve` then carries that sha, so dcv checks it again under its lock. `real.rs` uses it, and `approve` still works for `dct run`.
- Tests: learn `the_body_fingerprint_is_dcvs`, `touch_id_is_bound_to_the_body_the_card_showed`, `a_procedure_changed_in_dcv_after_the_card_is_not_approved`, `a_popup_changed_…`; procedures `approving_a_body_other_than_the_one_shown_is_refused_before_touch_id`; explore_cli `a_draft_changed_in_dcv_after_the_card_is_never_approved` (exit 1, approve_proc never called).
- RED: `unwrap_err() on an Ok`, `assertion failed: r.is_err()`, `approve_expecting(...).is_err()` failed. GREEN: learn 23, procedures 28, explore_cli 35 pass.

## IMPORTANT 10 — raw card text: FIXED
- `campaign::is_hidden_or_control` is now `pub(crate)` and reused (not duplicated). The card also refuses `\t`; only `\n` is allowed. `render_card` returns `Result<Card, String>`. It refuses a final body or popup body with such characters (and checks the finished text again), and `drive` then exits 1 with nothing approved.
- Every body line is rendered: steps in plain words; `   完成标志: verify_text 「X」` as "（做完后确认看得到「X」）"; any other line (indented, unknown) as "（原文）…". A numbered step whose action has no plain-word translation shows the whole `action: arg` instead of only the arg. The JSON gains `body_lines`.
- Tests: `a_body_with_hidden_or_control_characters_is_neither_shown_nor_approved` (U+202E, ESC, \r, \t, U+200B, U+2028, popup body), `every_line_of_the_body_is_on_the_card_even_indented_or_unknown_ones`. RED: panicked `"\u{202e}"` and text without the indented line. GREEN: pass.

## MINOR 11 — FIXED (0dfcceb). The RED was the rustfmt diff (`pub mod trace` after `testing`); GREEN: `rustfmt --check` is clean.

## MINOR 12 — FIXED
- replay halts report only the kind (`halt_text`: 登录 / 验证码 / 警告), never the line and never `{:?}`. This matches explore's `halted()`. The earlier R14-era test that expected "Log in to X" in the message now asserts it is absent.
- RED: `重放时画面变成了登录 / 验证码 / 警告（Login("Log in to X")）…`. GREEN: pass.

## MINOR 13 — FIXED
- `publish_step` returns the FIRST publish-word tap, for every preview and not only by-user platforms. Stopping at a later one would mean an earlier publish tap had already run, so taking the first is the safe choice in both cases.
- Test `preview_and_by_user_platforms_stop_before_the_first_publish_tap`. RED: `left: Some(5) right: Some(3)`. GREEN: pass.

## Deferred D13 — FIXED
- When the final body differs from the draft body, each one-off step is found again in the final body by its exact `action: arg` content. If any step is missing or matches more than one line, no step numbers are shown and the card says the body was fixed and the numbers no longer match. JSON `maybe_one_off` follows the same rule. `p.notes` are about popup taps by text, not by number, so they are kept.
- Test `one_off_step_numbers_follow_the_fixed_body` (shifted to 4、6; then dropped with the 修过 note). RED: card still showed 第 3、5 步.

## Deferred D10 — FIXED (doc only): the scrub.rs header now describes the element-has-≥2-words / word-boundary rule and the `link_like` inner-dot rule (a trailing dot does not count).

## Item 8 — not touched (user decision pending).

## Full check (head 360d010)
- `cargo test --workspace -- --test-threads=1`: first run stopped on 1 failure in the root lib, `session::tests::recovering_from_a_failure_after_real_input_still_does_not_count` (src/session.rs:2900; file untouched; timing). Run alone 3 times: passes every time. Second full run with `--no-fail-fast`: **2391 passed, 0 failed, 5 ignored** (was 2357; +34 new tests). `tests/daemon_upgrade.rs` did not flake in either run.
- `cargo clippy --all-targets` and `cargo clippy --workspace --all-targets`: no warnings.
- `rustfmt --check --edition 2021` clean: crates/dct-explore/src/lib.rs (whole crate), crates/dct-explore/tests/{explore_loop,replay}.rs, crates/dct-dco/src/lib.rs (with client.rs), src/explore_cli.rs (with io.rs, real.rs), src/learn.rs, src/explore_model.rs, src/campaign.rs. src/procedures.rs was dirty before: the new functions/test were copied from rustfmt output (one `bail!` string line rustfmt cannot break).
- `cargo check --workspace --target x86_64-pc-windows-msvc`: only the existing src/student_projects.rs:669 warning.

## Residual risks
- Item 4: `procedures::approve` signs and saves the Approvals record under the draft name (`…-0001`), but dcv stores the approved procedure under the original name. `dct do` replay only needs `executable` and is fine. `dct run promote-x-post` / tickets would not find that approval record and would ask for approval again. That is a pre-existing approve-of-draft_of behaviour, not changed here.
- Item 4: the draft-name tail is the last 4 chars of the run id. A collision with an older undeleted draft makes `dcv add` fail (reported, no harm).
- Item 5: Chinese platforms (all `publish_words` unverified) now ask before every tap, including tapping the composer for typing. That is noisier until each row is checked on a real machine.
- Item 13: all previews now stop at the first publish-word tap. On a platform whose first "Post" only opens a composer, a preview stops earlier than before (safer, but shows less).
- Item 1: publish detection on names is whole-word for ASCII and plain containment for CJK, so "发布笔记" counts as publish and "Repost" does not. dco's own fuzzy matching of an `Ok` name could still land on a nearby text button. The name check narrows this but cannot rule it out.
- Item 9: binding relies on dcv's `body_sha256` staying `sha256(normalize(body))`. If dcv changes the formula, approval fails closed (mismatch error), never open.
- Flaky `session::tests::recovering_from_a_failure_after_real_input_still_does_not_count` under `--test-threads=1` load (unrelated file).
