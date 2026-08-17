# Task 3 report: settings is a list of settings, not a list of languages

Commit: `717b5d7`

## What changed

- `src/ui/settings_view.rs`: added `SettingsItem { Language, Phone }` with
  `all()` / `at()` exactly as specified in the brief. Split `handle_key`/`draw`
  into a top-level "settings item list" (`handle_top_key` / `draw_settings_items`,
  mapped over `SettingsItem::all()`) and a "language sub-list"
  (`handle_language_key` / `draw_language_list`, mapped over `Lang::all()`,
  behaviourally identical to the old code). `Enter` on `Language` opens the
  sub-list with the cursor pre-set to the current language; `Enter` on `Phone`
  is inert per the controller ruling — a comment says the phone page arrives
  with Task 4, no TODO. `Esc` inside the language sub-list returns to the
  settings item list (one level), not straight to the board.
- `src/ui/view.rs`: `View::Settings` gained a `lang: Option<ListState>` field
  (`None` = top-level list, `Some` = inside the language sub-list). This was
  necessary because two logically different lists (settings items vs.
  languages) need two independent cursors, and the brief forbids adding a
  new `View` variant for this (only `View::Phone` in Task 4 is a new variant,
  and even that is out of scope here). Doc comments explain why one `View`
  variant carries two layers instead of nesting via a new variant.
- `src/i18n.rs`: added `Key::Phone` ("phone notifications" / "手机通知") for
  the settings-item label, wired into `text()` and `ALL_KEYS` (count bumped
  100 → 101).
- `src/ui/mod.rs`: `open_settings()` used to pre-select the *current
  language's* index into what was then a flat language list. That index no
  longer means anything against `SettingsItem::all()`, so it now just selects
  index 0 (the "Language" item). Also updated a `View::Settings` struct
  literal in a test to add `lang: None`. Not in the brief's file list, but the
  refactor doesn't compile/behave correctly without this fix — flagged
  because "plan reference not authoritative" invites judging each line, and
  this is a straightforward consequence of the field added in `view.rs`.
- `src/ui/pick.rs`: same mechanical fix — one `View::Settings` struct literal
  in a test needed `lang: None`.

## Why a `lang` field instead of a literal port of the brief's Enter dispatch

The brief's prose says "`Language` 进语言列表 (把今天的语言选择逻辑原样搬进去)"
— selecting Language enters an actual language list, not that Enter directly
re-applies the previously selected index. Since `Lang::all().len() ==
SettingsItem::all().len() == 2`, a naive "reuse the same index for both
lists" design would silently break switching to the *second* language
(pressing Enter at index 1 would dispatch to `Phone`, not apply `Lang::Zh`).
A genuine two-level list needs two cursors; since the brief's Files list and
Interfaces section rule out a new `View` variant for this, the extra field on
`View::Settings` was the smallest change that keeps behaviour correct.

## Tests

New tests in `src/ui/settings_view.rs` (11 total in that module now):
- `the_first_item_is_language`, `phone_is_a_settings_item_too`,
  `an_out_of_range_index_selects_nothing` — verbatim from the brief.
- `arrow_down_moves_from_language_to_phone` — added per Step 6's instruction
  ("if no test fails [for the `move_sel_n` mutation], add one for arrow-key
  movement to Phone").
- `entering_language_opens_the_sub_list_on_the_current_language`,
  `choosing_phone_does_nothing_yet`,
  `esc_in_the_language_list_returns_to_the_settings_item_list`,
  `the_top_level_page_lists_settings_items_not_languages` — added to cover
  the new two-level structure and the inert Phone arm, since the brief only
  gave test bodies for the `SettingsItem` type itself.

Existing tests were **rewritten, not deleted**, to exercise the same
guarantees one level down:
- `choosing_a_language_applies_it_and_writes_it_to_disk` — same assertions
  (`app.lang == En`, saved to disk), but the setup now places the cursor
  directly in the language sub-list (`on_language_list`) rather than the
  now-repurposed top-level list, since the top-level list's index no longer
  maps onto `Lang::all()`.
- `escaping_out_of_settings_changes_nothing` — same assertions, cursor set
  via `on_settings_items` (top level) instead of the old `on_settings`.
- `every_language_is_listed_in_its_own_language` — same assertion (buffer
  contains every language's native name), draws the language sub-list
  instead of the old single-level settings screen.

I could not literally freeze these three test bodies unchanged: with a real
two-level list, a single `Enter` from the top no longer both opens the
sub-list *and* commits a language in one step, so the old call sequences
don't type-check/behave the same way against the new `SettingsItem`-mapped
top list. I preserved every assertion's *intent* (language applies + persists,
Esc is a no-op, every language shows in its own tongue).

## Mutation testing

1. `at()`: changed `.get(i)` to `Some(SettingsItem::all()[i.min(1)])`
   (clamp out-of-range to the last item). Ran
   `cargo test --lib ui::settings_view -- --test-threads=1`:
   `an_out_of_range_index_selects_nothing` failed
   (`left: Some(Phone), right: None`) as required. Reverted.
2. `move_sel_n`'s length argument in the top-level arrow-key branch: changed
   `SettingsItem::all().len()` back to `Lang::all().len()`. Ran the same
   command: **no test failed**, including the new
   `arrow_down_moves_from_language_to_phone`. Root cause: `Lang::all().len()
   == SettingsItem::all().len() == 2` right now, so the two constants are
   numerically interchangeable — moving down one step clamps to index 1
   either way. I did add the arrow-key test the brief asks for (this was not
   skipped), but it can't distinguish these two particular constants while
   they happen to coincide in value; only a future third settings item (or a
   third language) would make this mutation observable. Documenting this
   explicitly per the instructions rather than claiming false coverage.
   Reverted.

## Commands run (final state, mutations reverted)

- `cargo build --all-targets` — clean.
- `cargo test -- --test-threads=1` — all passed: 716 (lib) + 9 + 1 + 1 + 1 +
  3 + 3 + 2 + 6 + 5 + 2 + 2 + 1 + 1 + 1 across integration binaries = 754
  total (baseline 746 + 8 new tests in `settings_view.rs`; no reduction).
  One run showed `slow_checkpoint_does_not_block_the_board` (in
  `tests/slow_input.rs`, unrelated to this change — a timing-sensitive
  daemon test) fail once; re-ran in isolation and as part of the full suite
  again and it passed both times, consistent with pre-existing flakiness,
  not something this refactor touches.
- `cargo fmt` then `cargo fmt --check` — clean (fmt made one whitespace-only
  pass over `settings_view.rs` before the check).
- `cargo clippy --all-targets` — clean, no warnings.

## Not run

Step 5 of the brief (`cargo run --release`, manually opening the board,
pressing `l`, picking a language, verifying the switch persists across
restart) was **not run** — this session is non-interactive and cannot drive
a TUI. This remains for a human to verify before merging.

## Concerns for review

- The addition of `lang: Option<ListState>` to `View::Settings` goes beyond
  the brief's literal "Produces: `View::Settings { state: ListState }`"
  (unchanged shape) and beyond the brief's Files list (touched `mod.rs` and
  `pick.rs` in addition to the three listed files). I believe this is
  required for correctness (see rationale above) rather than scope creep,
  but it's the one place I deviated from the letter of the interface
  contract and it's worth the reviewer's attention, especially against
  Task 4's `Consumes: SettingsItem::Phone` (Task 4 does not depend on
  `View::Settings`'s exact field list, so this should be safe for it).
  Same holds for `open_settings()` in `mod.rs`, which is a real behavioural
  fix (previously pre-selected the wrong thing after this refactor) not
  called out in the brief's Files list.
