# Final review fixes — report

**Status:** all four fixes applied, committed, verified green. Branch `feat/copy-from-session` unchanged aside from this one commit.

**Commit:** `f98cefe` — "fix: give the attached view an F4 entry point, and correct stale comments"

## i18n variant and labels

New `Key` variant: **`EnterCopyMode`** (chosen to be unambiguous next to `CopyMode`/`CopyModeShort` — those own the in-mode status line; this one is just the button label for the entry point, before you're in the mode).

Labels (verb only, `"F4"` supplies the key column via `("F4", Key::EnterCopyMode)`):
- zh: `复制`
- en: `copy`

Rendered bar item: `F4 复制` / `F4 copy`.

`ALL_KEYS` in `src/i18n.rs` updated (98 → 99), guard assertion bumped accordingly.

## Measured widths (computed from `bar_widths`, not assumed)

At width 80: `inner=78`, `escape=21`, `rest=57`, `project=18` → `action_cols = 39`.
At width 60: `inner=58`, `escape=21`, `rest=37`, `project=9` → `action_cols = 28` (= `ACTION_MIN_COLS`).

Item widths (`"F3"` + space + label, unicode-width-aware, CJK = 2 cols/char):
- zh: `F3 下一个会话` = 13, `F4 复制` = 7 → combined with 2-col separator = **22**
- en: `F3 next session` = 15, `F4 copy` = 7 → combined with 2-col separator = **24**

Both fit comfortably under 39 (width 80) and under 28 (width 60). `fit_help` budget check at width 60 confirms: budget for head items = `28 - (tail_width + 2)` = 19 (zh) / 19 (en); `F3`'s width (13 zh / 15 en) is ≤ 19, so it survives alongside the always-kept tail `F4`.

## Fixes applied

1. **F4 entry point** (`src/ui/view.rs`, `idle_help`'s `View::Attached` arm): now `help_items(&[("F3", Key::NextSession), ("F4", Key::EnterCopyMode)], lang)`. `F4` is placed last so `widgets::fit_help` (which always keeps the last item and drops earlier ones first) treats it as the least-droppable — it's the only "can't do the thing without this key" item at this layer; `F3` has `Ctrl+Q`/`F2` as an escape hatch and can yield first if ever needed (it never does at the supported floor).
2. **`wheel_action` doc comment** (`src/ui/attach.rs`): rewritten to state that mouse capture is now gated on `agent_owns`, so the wheel event physically never reaches `dct` for agents that don't subscribe to mouse reporting — the outer terminal handles it with its own native scroll/selection instead. The `ScrollAction::Scroll` branch and the `!app.scroll.agent_owns` click guard are now reachable only in a one-frame race right after an agent drops mouse ownership (state flips this frame, but `Enable/DisableMouseCapture` and the terminal's own catch-up haven't landed for an in-flight event yet). Branch and its tests (`otherwise_dct_scrolls_three_rows_per_notch`, `there_is_nothing_to_scroll_when_there_is_no_history`) left untouched — neither had a stale docstring to correct.
3. **80 vs. 60 column comment contradiction** (`src/ui/mod.rs`): the 80-column test's comment no longer claims 80 is the minimum; it now says 80 is the width at which the long-form copy-mode hint must still fit in full, and points at the 60-column test below for the actual floor (`grid::MIN_COLS`).
4. **`App.copy_mode` visibility** (`src/ui/app.rs`): changed `pub` → `pub(crate)` to match its struct neighbors. All external usages (`mod.rs`, `attach.rs`) are already within the crate, so this is purely a visibility tightening with no call-site changes.

## Tests

Added `ui::tests::attached_view_bar_keeps_both_f3_and_f4_at_eighty_and_sixty_columns` in `src/ui/mod.rs`, looping over width `{80, 60}` × lang `{Zh, En}`, asserting `bar_text` contains both `"F3"` and `"F4"` in the `View::Attached` bottom bar with `copy_mode` off (the idle-help state). `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` (643 lib tests + all integration suites) all pass clean after the change.

## Mutation evidence

Temporarily reverted the `idle_help` `View::Attached` arm to the pre-fix single-item form (`help_items(&[("F3", Key::NextSession)], lang)`) and re-ran the new test in isolation:

```
test ui::tests::attached_view_bar_keeps_both_f3_and_f4_at_eighty_and_sixty_columns ... FAILED
thread '...' panicked at src/ui/mod.rs:3885:17:
80 列 Zh 下 F4 不见了——这是这一层唯一能进复制模式的入口：┌会话1·/tmp/a——F2返回看板...
```

Failed exactly as expected, at the first (80, Zh) case. Reverted the mutation back to the real fix (verified via `git diff --stat` showing the file restored to its committed state) and re-ran the full `cargo fmt --check` / `cargo clippy` / `cargo test` gate green before committing.

## Concerns

None outstanding. Scope was held to the four requested fixes; did not touch `ACTION_MIN_COLS` visibility, the hint's narrow-width truncation, the pre-existing `ALL_KEYS` coverage gap beyond adding the one new key, the message/disconnect masking behavior, or anything about `?1007`/alternate scroll, per the explicit do-not-do list. `src/proto.rs`, `src/pty.rs`, `src/session.rs`, `src/daemon.rs` were not touched; `PROTOCOL_VERSION` unchanged. Staged and committed only the five source files (`src/i18n.rs`, `src/ui/app.rs`, `src/ui/attach.rs`, `src/ui/mod.rs`, `src/ui/view.rs`) — the user's uncommitted `README.md`, `README.zh-CN.md`, and `.superpowers/sdd/.gitignore` edits were left alone and unstaged.
