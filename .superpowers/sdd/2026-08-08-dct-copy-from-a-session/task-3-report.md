# Task 3 report: 底栏写清楚现在是什么状态

## Update 2: post-review fix — hint degrades below width ~78

Review found an Important issue on top of the already-fixed width-80 bug:
`ACTION_MIN_COLS = 28` (`src/ui/mod.rs:1888`) is the floor the right segment
is actually guaranteed to have, not 39 — 39 was only true at width 80. Below
about width 76–78 the right segment shrinks under the 37-column long hint's
width, and since `wrap_help` can't break a single-space sentence, the tail
(`F4 exits` / `F4 退出`) gets silently truncated again — at width 60
(`grid::MIN_COLS`, a supported size and a routine tmux split), `action_cols`
drops to exactly 28 and the long form no longer fits either language.

Because the bar is the *only* place in the whole UI that ever names F4
(`src/ui/view.rs:1121` only lists F3 for the attached view; no other i18n
entry mentions F4), a truncated hint here leaves the user in a mode they
cannot see and cannot discover how to leave — the coordinator judged this
undermines the task's own point and asked me to fix it immediately rather
than defer.

### What changed

**`src/i18n.rs`**
- Added `Key::CopyModeShort` right after `CopyMode` (chose that name, no
  reason to deviate from the coordinator's suggestion) with a doc comment
  explaining it must fit `ui::ACTION_MIN_COLS` because it's the only F4
  mention on screen.
- Translation arm, verbatim from the resolution:
  - en: `"Copy mode · F4 exits"` (20 display columns)
  - zh: `"复制模式 · F4 退出"` (18 display columns)
  Same `·`-with-single-spaces style as `CopyMode` and its neighbours.
- Added `CopyModeShort` to `ALL_KEYS`; bumped the exhaustiveness count from
  97 to 98.
- New guard test `copy_mode_short_fits_the_action_floor_in_every_language`,
  modelled on the `ESCAPE_HINT_COLS` guard around `src/ui/mod.rs:3510`
  (`escape_hint_cols_fits_every_view`): for both languages, asserts
  `text(CopyModeShort, lang).width() <= ui::ACTION_MIN_COLS`, using
  `unicode_width::UnicodeWidthStr` — the same measuring method
  `widgets::display_width`/`item_width` ultimately use, not `.len()`.
  Unlike the escape-hint guard this doesn't also assert equality to the
  widest string — `ACTION_MIN_COLS` is a floor shared by other content
  (the scroll hint), not a constant sized exclusively for this string, so
  `<=` is the correct assertion here.

  This test needed `ui::ACTION_MIN_COLS` visible from `i18n.rs`, so I
  changed its visibility from private to `pub(crate)` in `src/ui/mod.rs`
  (with a comment explaining why) — the only non-additive change in this
  round, and it's a pure visibility widening with no behavior change.

**`src/ui/mod.rs`**
- In `draw()`'s copy-mode arm: measure the long `CopyMode` text with
  `widgets::display_width` (the same unicode-width-based function the file
  already uses for `item_width`/`help_width`) against `help_cols`. Use the
  long form if it fits, otherwise fall back to `CopyModeShort`. This
  mirrors `bar_keys` (`src/ui/mod.rs:2106-2126`), which measures a richer
  form with `help_width` and falls back to a plainer one when `cols` is
  too small. Commented in Chinese, in the surrounding style, explaining
  why: `底栏右段只保证 ACTION_MIN_COLS 列，而这条提示是全屏唯一写着 F4
  的地方，截掉尾巴等于把用户关在一个看不见也出不去的模式里。`
- Added `copy_mode_short_hint_survives_sixty_columns_in_both_languages`
  right after the existing width-80 test: renders both languages at width
  60 (`grid::MIN_COLS`) with `copy_mode = true`, asserts the bar contains
  the **complete** short-form string (whitespace-insensitive, same pattern
  as the width-80 test). The width-80 test itself needed no changes — it
  already asserts the complete long form at width 80, where the long form
  still fits (help_cols 39 > 37).

### Widths verified

- Width 80: `help_cols = 39`. Long form (37 cols English / 35 cols Chinese)
  fits and is shown complete — covered by the existing
  `copy_mode_hint_survives_eighty_columns_in_both_languages` (unchanged).
- Width 60: `help_cols = 28` (== `ACTION_MIN_COLS`, computed via
  `bar_widths(58) = (21, 9, 28)`). Long form (37/35) no longer fits; short
  form (20 English / 18 Chinese) fits and is shown complete — covered by
  the new `copy_mode_short_hint_survives_sixty_columns_in_both_languages`.

### Mutation evidence

Forced the long form unconditionally (`let chosen = if true { long } else
{ ... }`) and reran the new width-60 test:

```
cargo test --lib ui::tests::copy_mode_short_hint_survives_sixty_columns_in_both_languages
```
→ **FAILED**: `Zh 在 60 列下要完整显示复制模式的短文案：...复制模式·鼠标已交还终端·│` —
`F4 退出` visibly missing from the rendered bar (the long Chinese form
truncated at 28 columns), exactly the bug this test exists to catch.
Reverted the mutation (`widgets::display_width(long) <= help_cols`); reran
— passes again.

### Final verification (after the fix, before commit)

```
cargo test --lib ui::tests
```
62 passed (60 + the two width-boundary tests from Update 1's resolution,
+ this round's new width-60 test).

```
cargo test --lib i18n::tests
```
18 passed, including the new `copy_mode_short_fits_the_action_floor_in_every_language`.

```
cargo test
```
642 lib tests passed (640 + 2 new: the i18n guard and the width-60
render test), all integration suites unchanged and green, 0 failures.

```
cargo fmt --check
```
Clean.

```
cargo clippy --all-targets -- -D warnings
```
Clean, no warnings.

```
git diff --check
```
Clean.

### Commit

Staged only `src/i18n.rs` and `src/ui/mod.rs` by name (never `README.md`,
`README.zh-CN.md`, or `.superpowers/sdd/.gitignore`, all still dirty from
before this task and still untouched by it).

```
git commit -m "fix: copy-mode hint degrades to a short form on narrow terminals"
```

Commit: `1e6a1b2` on `feat/copy-from-session`, 2 files changed
(`src/i18n.rs`, `src/ui/mod.rs`), 72 insertions(+), 3 deletions(-).

`src/proto.rs`, `src/pty.rs`, `src/session.rs`, `src/daemon.rs` untouched.

### Concerns

None outstanding. The only structural change beyond straightforward
additions was widening `ACTION_MIN_COLS` from private to `pub(crate)` so
`i18n.rs` could reference the real constant instead of a duplicated magic
number — flagging it explicitly since it's the one edit that isn't purely
additive, though it's a visibility-only change with no behavioral effect.

---

## Update 1: resolved and committed

The coordinator resolved the blocking width-80 finding below: shorten the
English string only (Chinese stays, already reviewed/approved and it fits).
New English text, replacing the one in the brief:

```
en: "Copy mode · mouse released · F4 exits"
```

Applied to the `CopyMode` arm in `src/i18n.rs`. Display width 37 columns
(verified with the same east-asian-width accounting used below) against the
39-column budget at width 80 — matches the coordinator's number exactly.
Kept the same `·`-with-single-spaces house style as the neighbouring English
strings (`↓ {n} new below · press End`, `Session {id} · {project} —— F2 goes
back`). Did not introduce double-space separators — that would let
`wrap_help` split the line and grow the bar's height, which is load-bearing
for the grid view at 80×24 (one row from `MIN_ROWS`).

Added the required regression test right after the two brief tests in
`src/ui/mod.rs::mod tests`:

```rust
/// 80 是我们支持的最窄终端，右段只有 39 列，而 `wrap_help` 不拆单空格的
/// 句子——写长了不会折行，会被 `Paragraph` 悄悄切掉尾巴。两种语言都要
/// 在这个宽度下把复制模式的提示完整放出来，一个字都不能少。
#[test]
fn copy_mode_hint_survives_eighty_columns_in_both_languages() {
    use ratatui::backend::TestBackend;

    for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
        let (mut app, _d) = app_with_one_agent_session(View::Attached(1));
        app.lang = lang;
        app.copy_mode = true;
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();

        let bar = bar_text(&term);
        let hint = crate::i18n::text(crate::i18n::Key::CopyMode, lang);
        assert!(
            bar.contains(&hint.replace(' ', "")),
            "{lang:?} 在 80 列下要完整显示复制模式提示，不能被切掉尾巴：{bar}"
        );
    }
}
```

Modeled on `the_way_back_survives_a_narrow_terminal`, but checks both
languages at width 80 specifically (the narrowest supported terminal, 39
usable columns in the right segment) rather than one language at width 55.

### Mutation evidence for the new test

Temporarily restored the long English string
(`"Copy mode · the mouse is the terminal's · F4 to exit"`) and reran just
the new test:

```
cargo test --lib ui::tests::copy_mode_hint_survives_eighty_columns_in_both_languages
```
→ **FAILED**: `En 在 80 列下要完整显示复制模式提示，不能被切掉尾巴：...Copymode·themouseistheterminal's│` —
`· F4 to exit` visibly missing from the rendered bar, exactly the bug this
test exists to catch. Reverted to the short string; reran — passes again.

### Final verification (after the fix, before commit)

```
cargo test --lib ui::tests
```
61 passed → after adding the new test, still all green.

```
cargo test
```
640 lib tests passed (639 + 1 new), all integration suites unchanged and
green, 0 failures anywhere.

```
cargo fmt --check
```
Clean.

```
cargo clippy --all-targets -- -D warnings
```
Clean, no warnings.

```
git diff --check
```
Clean.

### Commit

Staged only `src/i18n.rs` and `src/ui/mod.rs` by name (never `README.md`,
`README.zh-CN.md`, or `.superpowers/sdd/.gitignore`, all of which remain
dirty from before this task and untouched by it).

```
git commit -m "feat: the bar says plainly when the mouse belongs to the terminal"
```

Commit: `74f40d8` — "feat: the bar says plainly when the mouse belongs to
the terminal" on `feat/copy-from-session`, 2 files changed
(`src/i18n.rs`, `src/ui/mod.rs`), 92 insertions(+), 4 deletions(-).

---

## Original report (the blocking finding, kept for the record)

## Status: BLOCKED (not committed)

Implementation is complete and all mandated tests pass, but the required
width-80 verification (the "extra check beyond the brief") surfaced a real
overflow bug in the approved English string. Per my instructions — "report
it and stop rather than silently shortening the approved English string" —
I stopped short of committing. The working tree currently holds the finished,
tested code changes, uncommitted, awaiting a decision on the wording (or on
a structural fix to the wrapping code, see below).

## What was changed

### `src/i18n.rs`
- Added `Key::CopyMode` to the `Key` enum (with the exact doc comment from
  the brief), right after `StaleData`.
- Added its translation arm to the exhaustive `text()` match, verbatim from
  the brief:
  - en: `"Copy mode · the mouse is the terminal's · F4 to exit"`
  - zh: `"复制模式 · 鼠标已交还终端 · F4 退出"`
- Added `CopyMode` to the `ALL_KEYS` exhaustive list (guarded by
  `every_key_is_listed_for_the_guards`, `no_english_entry_contains_han_characters`,
  `no_entry_is_empty_in_either_language`).
- Bumped the `ALL_KEYS.len()` assertion from 96 to 97.

### `src/ui/mod.rs`
- In `draw()`, inside the `app.message.text.is_empty()` branch: renamed the
  local binding from `scroll_hint` to `hint` and inserted a `View::Attached(_)
  if app.copy_mode` arm ahead of the existing `View::Attached(_) =>
  attach::scroll_hint(...)` arm, per the brief's Step 4. The live fallback
  arm was **left untouched** — it still calls `BarContent::Keys(bar_keys(app,
  help_cols))`, not the stale `idle_help`/`help_ctx` names from the brief's
  code block (those don't exist in this codebase; the parent's briefing
  note already flagged this as stale, and I followed the note, not the
  stale block).
- Added the two tests from Step 1, verbatim from the brief, to `mod tests`:
  - `copy_mode_says_so_in_the_bar` (loops both langs, asserts the bar
    contains the `CopyMode` text)
  - `an_error_beats_copy_mode_which_beats_the_scroll_hint` (asserts copy
    mode beats the scroll hint, then that an error message beats copy mode)
  - Both needed a local `use ratatui::backend::TestBackend;` that the
    brief's code block omitted (other tests in the file add this import
    locally rather than at module scope).

Neither `a_scroll_hint_takes_over_the_bottom_bar_when_there_is_history` nor
`a_message_beats_the_scroll_hint` was touched — both still pass unmodified.

## Test commands and output

```
cargo test --lib ui::tests
```
60 passed, 0 failed (includes the two new tests).

```
cargo test
```
639 lib tests passed + all integration test binaries (9, 1, 1, 1, 3, 3, 2,
6, 5, 2, 2, 1, 1, 1 across the various `tests/*.rs` files) — 0 failed
anywhere.

```
cargo fmt --check
```
Clean (I ran `cargo fmt` first, which only reformatted my own new code —
diff confirmed no unrelated reformatting).

```
cargo clippy --all-targets -- -D warnings
```
Clean, no warnings.

```
git diff --check
```
Clean, no whitespace errors.

## Width-80 verification — FOUND A BUG, did not paper over it

I added a temporary diagnostic test (`temp_width_80_probe`, removed before
finishing) that rendered both languages at `TestBackend::new(80, 24)` with
`copy_mode = true` and dumped the raw buffer.

**Chinese renders fine at width 80.** `help_cols` at width 80 is 39
(`bar_widths(78) = (21, 18, 39)`). The zh string's display width is 35
(`unicode-width` accounting for double-width CJK characters) — fits with
room to spare, one line, fully present.

**English does not.** The approved string is 51-52 chars (I measured 52
with a plain `len()` in Python; the brief said 51 — either count is far
past budget), which is its *display* width too since it's all single-width
ASCII. Against `help_cols = 39` at width 80, it is ~13 columns too wide.

The brief's own comment (and my briefing note) asserts `BarContent::Text`
"wraps rather than truncates," pointing at `widgets::wrap_help`. That
assumption does not hold for this string. `wrap_help` (`src/ui/widgets.rs:223`)
only splits on a **double** space or a full-width space (`\u{3000}`) — by
design, so it never breaks a "key label" pair apart (see its doc comment).
The copy-mode hint uses single spaces around a `·` bullet throughout, so
`wrap_help` treats the whole sentence as **one atomic item** and emits a
single line no matter how long. That single `Line` is then handed to
`Paragraph::new(help_lines)` at `src/ui/mod.rs:2093`, which has **no
`.wrap()`** call — by design, per the comment above it, because the key
table is supposed to always be exactly one line. The result: at width 80,
the line is silently right-truncated by the terminal buffer, not wrapped
to a second row.

Actual rendered bottom bar at 80×24, English, copy mode on:

```
│Ctrl+Q (F2) back     /tmp/a            Copy mode · the mouse is the terminal's│
```

`· F4 to exit` — the entire instruction for how to leave copy mode — is
silently missing. This is not a cosmetic wrapping-into-more-lines problem;
it is exactly the failure mode the task warned about and that the codebase
has an existing regression test for: see the comment directly above
`the_way_back_survives_a_narrow_terminal` (`src/ui/mod.rs`, right below
where I added my tests), which documents the *same* class of bug fixed
previously for the scroll-hint string by shortening it — "被 `Paragraph`
（没挂 `.wrap()`）直接从右边截断". My new string reproduces that exact bug.

I did not shorten the English wording myself, per the explicit instruction
that the wording is the user's to change. I also did not patch
`widgets::wrap_help` or add `.wrap()` to the bar `Paragraph` to structurally
fix this, because that function's current behavior (never break a key
away from its label) is deliberate and shared by every other consumer of
`BarContent::Text`/`BarContent::Keys` in the bar — changing it is out of
scope for this task and would need its own design/test pass.

**Options for whoever resolves this** (not applied by me):
1. Shorten the English string so its display width fits within
   `help_cols` at width 80 (39 columns) — the same fix applied previously
   to the scroll-hint string.
2. Make the bar's `Paragraph` for `BarContent::Text` wrap for real (e.g.
   give `wrap_help` a fallback that word-wraps on single spaces when the
   whole string is one atomic item over budget, or add `.wrap()` on that
   specific `Paragraph`) so overflow becomes an extra bar row instead of
   losing characters. This is a bar-wide change beyond Task 3's file list.
3. Accept that this hint is only ever safe above some minimum terminal
   width and special-case it — but nothing in `copy_mode` currently
   depends on terminal width, so this would need a new guard.

## Mutation evidence

Per the global constraint, I mutated the production code before finishing
verification (with the width-80 finding not yet resolved, I still ran this
to confirm the two new tests are real):

- Removed the `View::Attached(_) if app.copy_mode => { ... }` arm from
  `draw()`, leaving only `View::Attached(_) => attach::scroll_hint(...)`.
- `cargo test --lib ui::tests::copy_mode_says_so_in_the_bar` → **FAILED**:
  `Zh 下底栏要写着复制模式：...F3下一个会话...` (bar fell through to the key
  table, no copy-mode text at all).
- `cargo test --lib ui::tests::an_error_beats_copy_mode_which_beats_the_scroll_hint`
  → **FAILED**: `复制模式压过滚动提示` (bar showed the scroll hint instead).
- Reverted the mutation; reran `cargo test` — 639 passed, 0 failed, both
  tests green again.

Both tests can fail and do fail when the arm they cover is removed.

## Concerns

1. **Primary concern (blocking):** the approved English `CopyMode` string
   overflows and gets silently truncated at width 80, dropping the "F4 to
   exit" instruction entirely. See the width-80 section above. I stopped
   before committing, per instruction, rather than shorten the string
   myself or patch the shared wrapping code.
2. The repo's working tree already carries unrelated uncommitted changes to
   `.superpowers/sdd/.gitignore` (rewritten to `*` — a pre-existing
   artifact of the sdd-workspace tooling, not something I touched),
   `README.md`, and `README.zh-CN.md`. None of these were staged or
   touched by me; I did not `git add` any of them, per the constraint
   about the two README files, and left `.gitignore` alone since it wasn't
   part of my task and was already dirty before I started.
3. No commit was made. `src/i18n.rs` and `src/ui/mod.rs` hold the
   finished, tested, fmt/clippy-clean diff, ready to `git add` and commit
   once the wording/wrapping question above is resolved.
