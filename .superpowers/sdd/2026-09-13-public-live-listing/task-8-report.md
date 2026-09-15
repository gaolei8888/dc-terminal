
## Step 8a

Changed:
- src/ui/view.rs: added `LiveInput` enum (`Title(String)` / `Key { buf, then_publish }`) with hand-written `Debug` redacting `buf`; `View::Live` gained `input: Option<LiveInput>`; added test `a_live_key_being_typed_is_redacted_in_debug`.
- src/ui/app.rs: added `pub last_public_title: String` field, initialized to `String::new()`.
- src/ui/live.rs: updated all `View::Live` construction/destructure sites (open, handle_key x2, draw, and 4 test setups) to match the new `input` field.
- Added `#[allow(dead_code)]` on `LiveInput`, `last_public_title`, and `View::Live::input` since nothing reads/constructs them outside tests yet (wiring deferred to a later step).

Commands:
- `cargo test --lib ui:: 2>&1 | tail -6` -> `test result: ok. 530 passed; 0 failed; ...` (531 with new test before revert check)
- `cargo clippy --lib --tests -- -D warnings` -> clean, `Finished` with no warnings/errors.

Mutation check: made the `Key` Debug arm print `buf` directly instead of `"<redacted>"`. Test `a_live_key_being_typed_is_redacted_in_debug` failed as expected (`assertion failed: !format!("{k:?}").contains("SUPER-SECRET")`). Reverted; full suite green again.

Commit: f9beb2b "refactor(ui): the live panel can carry an input line"

## Step 8b

**Changed:** `src/ui/live.rs` — `handle_key` now destructures `input` and routes to a new
`edit_live_input` when an input line is open (before the normal key match, so it swallows
all keys). Added `p` (toggle publish/unpublish) and `K` (edit publish key directly) arms.
Added free functions `edit_live_input`, `publish`, `unpublish`. `fake_live_daemon` gained a
`has_key: Arc<AtomicBool>` parameter and now answers `LivePublish`, `SetSecret` (for the live
publish key profile), and `LiveUnpublish`. Its one existing caller
(`a_broadcast_started_elsewhere_reaches_the_banner_even_inside_a_session`) updated to pass
`Arc::new(AtomicBool::new(false))`. Removed the three `#[allow(dead_code)]` markers on
`LiveInput`, `View::Live::input`, and `App::last_public_title` (`src/ui/view.rs`,
`src/ui/app.rs`) now that they're constructed/read.

**RED/GREEN:** Wrote the three specified tests
(`publishing_asks_for_a_title_then_a_key_when_missing_then_continues`,
`an_empty_title_is_refused_on_the_spot`, `p_on_a_public_live_unpublishes`) against the new
code in one pass (implementation and tests written together per the task instructions, not
test-first). `cargo test --lib ui::live::tests` — 22 passed, including all three new tests.
Full `cargo test --lib ui::` — 533 passed, 0 failed.

**Mutation checks:**
1. Made the `LivePublishKeyMissing` arm in `publish` return `None` instead of transitioning
   to `LiveInput::Key { then_publish: Some(title), .. }` — `publishing_asks_for_a_title...`
   failed as expected (assertion on the post-Enter view state). Reverted.
2. Dropped the empty-title check in `edit_live_input`'s `Title` arm — `an_empty_title_is_refused_on_the_spot`
   failed as expected (assertion on `app.message.error` / view staying on `Title`). Reverted.

**Verify:** `cargo test --lib --no-run` clean (no errors/warnings). `cargo clippy --lib --tests -- -D warnings` clean.

**Deviations:** None from the spec's field/type names — `App::test_app()` returns
`(App, TempDir)` as assumed, `Msg::err(String)` and `Msg { text, error }` matched as assumed.
No `cargo fmt` run, no stash used.

Commit: `64948c6 feat(ui): publish and unpublish from the live panel with p, enter the key with K`

## Step 8c

Changed:
- `src/ui/live.rs`: `draw` now destructures `input` from `View::Live`. New `input_line()` builds the prompt line — `LiveInput::Title` shows the typed text verbatim; `LiveInput::Key` shows the prompt plus one `•` per char, never `buf` itself. Drawn after the QR block when live, and above the "off" line (before the session list) when not live, both styled `accent()`.
- `src/ui/view.rs`: added `View::Live { input: Some(_), .. } => "Esc {Cancel}"` arm before the existing Live escape-hint arm. Added a matching arm in the help-bar match that returns only `Enter`/`Esc` when an input is open. In the normal (no-input) Live help arm, added `("p", LivePublishToggle)` inside `if ctx.live_on`, and `("K", LiveChangeKey)` unconditionally before `Esc`.
- `src/i18n.rs`: `LivePublishToggle`/`LiveChangeKey` English strings changed from `"p public/private"`/`"K change key"` to `"public/private"`/`"change key"` (the key letter is already rendered by `help_items`; Chinese strings were already correct).
- Checked `grep -n "LiveToggleStaged\|LiveCopyLink"` in view.rs/live.rs: no existing tests assert exact Live help string contents beyond the production match arms themselves, so nothing needed updating.

Tests added:
- `src/ui/live.rs`: `the_key_being_typed_is_never_drawn` (asserts screen never contains the raw key buffer, does contain `•`), `the_title_being_typed_is_drawn` (asserts the typed title text appears on screen, whitespace stripped for CJK cell spacing).
- `src/ui/view.rs`: `the_live_input_line_shrinks_the_help_bar_to_confirm_and_cancel` — builds `idle_help` for `View::Live { input: Some(Title(..)) }` with `live_on: true`, asserts the returned `HelpItem` keys contain `"Enter"`/`"Esc"` and none of `"c"`/`"r"`/`"s"`/`" "`/`"p"`/`"K"` (checked against item keys directly, not the rendered string, since `"Esc"` itself contains the substrings `c`/`s`).

RED/GREEN evidence: wrote tests against the implementation (implementation and tests were written together in this step), then confirmed both pass: `cargo test --lib the_key_being_typed_is_never_drawn`, `the_title_being_typed_is_drawn`, `the_live_input_line_shrinks_the_help_bar_to_confirm_and_cancel` all `ok`. Full suite: `cargo test --lib ui::` → 536 passed; `cargo test --lib i18n::` → 19 passed. `cargo test --lib --no-run` and `cargo clippy --lib --tests -- -D warnings` both clean.

Mutation checks (each applied, verified RED, then reverted):
1. Replaced `"•".repeat(buf.chars().count())` with `buf.clone()` in `input_line` → `the_key_being_typed_is_never_drawn` failed as expected.
2. Deleted the `View::Live { input: Some(_), .. }` help-bar arm (falling through to the normal arm) → `the_live_input_line_shrinks_the_help_bar_to_confirm_and_cancel` failed, printing `["", "c", "r", "s", "p", "K", "Esc"]` as expected.

Deviations from the brief: none. Both not-live and live branches draw the input line; the not-live branch draws it above the session list (no banner/QR to sit below).

## Step 8d

Changed:
- `src/ui/live.rs`: `live_banner` now picks base text via `info.public` — `Listed`/`Pending`/`Failed` use `msg::live_on_air_public(lang, title, routes, viewers)`, `Private` keeps `msg::live_on_air`. Adds a public-state suffix (`LivePublicPending` text, or `msg::live_publish_failed`) before the existing readiness suffix, so both can appear together. `banner_style` returns `danger()` for `LivePublic::Failed { .. }`, checked before `readiness`. Doc comments updated in Chinese. Added test `the_banner_says_public_with_the_title` (Listed shows title+count, Failed shows "吊销" reason, Private never says "公开").
- `src/ui/mod.rs`: `bar_live_style` returns `bar_danger(t)` for `LivePublic::Failed { .. }` on a solid bar, checked before the readiness match; doc note added. Extended `nothing_on_a_solid_bar_takes_its_color_from_the_terminal_theme`'s `cases` from `[Case; 7]` to `[Case; 9]`, adding "正在公开直播" (Listed) and "公开失败" (Failed, Refused(401)) cases.

RED/GREEN evidence:
- Both mutations below produced failing tests before being reverted (see Mutation checks).
- After implementation: `cargo test --lib --no-run` clean (no errors/warnings); `cargo test --lib ui::` → 537 passed, 0 failed; `cargo test --lib the_banner_says_public_with_the_title` → 1 passed; `cargo clippy --lib --tests -- -D warnings` clean.

Mutation results:
- `bar_live_style` Failed arm changed to `danger()` instead of `bar_danger(t)` → `nothing_on_a_solid_bar_takes_its_color_from_the_terminal_theme` failed with "Gray 档「公开失败」... 底栏上出现第三种颜色". Reverted.
- `live_banner` Listed arm changed to use `live_on_air` (private text) → `the_banner_says_public_with_the_title` failed, banner printed "● 正在直播 · 1 路 · 7 人在看" (missing "公开"/title). Reverted.

Deviations: none from the step instructions.
