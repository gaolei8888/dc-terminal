# Report: Tasks 1 and 2 (dct game identify)

## Task 1 - genre.rs (commit 2b71703)
- RED: with classify/sentence stubbed, 5 of 6 genre tests failed (the 6th passes trivially on a stub).
- GREEN: 6 passed. Mutations: weak >=1 -> one_weak_cue test red; board early-return removed -> a_readable_board test red; to_lowercase removed -> 3 tests red. All reverted.
- Workspace: dct-game 203 passed; only known-flaky dct lib tests failed (web_tests listener, recovering_from_a_failure...); clippy -D warnings clean.
- Files: crates/dct-game/src/genre.rs (new), crates/dct-game/src/lib.rs. normalise uses char::from_u32 only inside FF01..=FF5E (cannot be out of range) with unwrap_or(c).

## Task 2 - identify (subject: feat(game): dct game identify ...)
- Files: src/game/identify.rs (new: identify, run, run_identify), src/game/mod.rs, src/game/cli.rs (dispatch before parse), src/main.rs (help), src/game/skill.md.
- Adaptations: looks_like_board returns bool (not Option), so `Some(bool)` is built from it; json! does not accept `[false;4]`, replaced by literal arrays. Assertions unchanged. dct game play untouched.
- RED: stubbed identify, 3 of 4 tests failed. GREEN: 4 passed (68 in game::).
- Mutations: invert looks_like_board -> a_readable_board + text_unsupported red (the strong-cue test uses an Err grid so inversion cannot affect it; brief predicted it too); see_text unwrap() -> text_unsupported red. Reverted.
- Workspace: only flaky recovering_from_a_failure... failed; clippy clean.
## Concerns
- cargo fmt not installed for 1.99.0, so rustfmt not checked.
- `identify` CLI never run against real dco (as instructed).
- skill.md marker stays v1 though text changed (existing installs may not refresh; not checked).

## Review fix commit
- identify.rs: added popup_grid (11/12 cells one class) tests: with non-cue text -> Unknown; with "Inventory" -> HiddenObject. Mutation `map(|_g| true)` (stand-in for read_grid().is_ok()) turned both red; reverted.
- genre.rs normalise: U+2018/U+2019 -> ASCII apostrophe (full-width U+FF07 already covered); new test typographic_apostrophes_match_the_ascii_cue. No from_u32 added; panic-free.
- Workspace: only flaky tests failed (web_tests listener; polling_with_a_different_account... failed once under load, passed on rerun); clippy clean.
