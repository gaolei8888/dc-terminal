# Border removal (drop left/right border glyphs) — verification report

Scope: every bordered `Block` in `src/ui/` keeps `Borders::TOP | Borders::BOTTOM`
and drops `LEFT`/`RIGHT`, so selecting/copying text out of dct no longer picks
up vertical border characters. This report is the final check on
already-uncommitted work; I verified it, fixed one thing, and committed it.

## Sites changed (`Borders::ALL` → `Borders::TOP | Borders::BOTTOM`)

- `src/ui/attach.rs` — session content block (`draw`)
- `src/ui/board.rs` — session list block
- `src/ui/grid.rs` — per-tile block (was `Block::bordered()`, i.e. `ALL`, now
  explicit `Borders::TOP | Borders::BOTTOM`)
- `src/ui/keys.rs` — "all keys" help popup block
- `src/ui/mod.rs` — bottom bar block
- `src/ui/pick.rs` — profile list block, project-path input block, `pane_block`
  helper (used by both panes of the profile picker)
- `src/ui/secret.rs` — enter-secret block, secrets list block
- `src/ui/settings_view.rs` — settings list block, language list block

`grep -rn 'Borders::ALL' src/` returns nothing after the change.

## `grid.rs` and `BorderType`

`grid.rs` draws tiles with `Block::default().borders(Borders::TOP |
Borders::BOTTOM).border_type(Thick | Plain)`. `BorderType` only selects the
line-drawing character set used to render whatever borders are enabled
(`─`/`━` for TOP/BOTTOM here) — it is orthogonal to which edges are drawn.
Before this change the tile block was built with `Block::bordered()`, which
is shorthand for `Borders::ALL`; the diff replaces that with the explicit
`Borders::TOP | Borders::BOTTOM` builder while keeping the existing
`.border_type(...)` call untouched. This is consistent with the rest of the
change: focus is still shown by switching the line style (Thick vs Plain) of
the remaining top/bottom edges, not by anything to do with side borders (side
borders never conveyed focus).

## Geometry recalculations, and how each was verified

1. **`attach.rs` `draw`** — content block now built once and its `inner` rect
   (`Block::inner(area)`) used everywhere instead of hand-computed `area.x+1`
   / `area.y+1` offsets. Since only top/bottom borders remain,
   `inner.x == area.x` (no left border) and `inner.y == area.y + 1` (top
   border still there). `screen_origin`, the cursor-placement clamp, and the
   mouse-coordinate comment were all updated to match. Verified by
   `draw_records_the_bordered_content_corner_as_the_screen_origin`, which
   asserts `screen_origin == (area.x, area.y + 1)` for a deliberately
   offset, non-`(0,0)` `Rect` — this is exactly the kind of test that would
   catch a stray `+1` regression.

2. **`attach.rs` title truncation budget** — bilingual comment recomputed the
   60-column worst case: was 60 − 2 (side borders) − fixed text = budget 27,
   `truncate` argument 15; now 60 (no side borders) − fixed text = budget 29,
   `truncate` argument 17. The comment shows the arithmetic; I re-derived it
   by hand and it checks out (all the fixed-text widths — prefix 12, " · " 3,
   suffix 16 — are unchanged, so the budget simply gains back the 2 columns
   the side borders used to cost).

3. **`board.rs` room-for-truncation calc** — `room = area.width -
   HEADER_PREFIX_COLS` (was `... - 2 (borders) - HEADER_PREFIX_COLS`).
   Verified against the two tests that hard-code the resulting column math
   at 80 columns: `HEADER_PREFIX_COLS` is 44, so room is now 36 (was 34).
   The ellipsis-placement test's literal changed from `"lastusedan-absurd…"`
   (9 visible chars of the padded name before `truncate` cuts it) to
   `"lastusedan-absurdly…"` (11 visible chars) — I checked this by hand: `"no
   sessions · last used "` is 24 display columns, `truncate` gets `36 - 24 -
   1 = 11`, matching the new literal exactly.

4. **`grid.rs` `tile_title_name_cap`** — `budget = tile_width` (was
   `tile_width.saturating_sub(2)`). All five arithmetic assertions in
   `tile_title_name_cap_is_derived_from_tile_width_not_fixed` and all four
   (now five, see "found and fixed" below) in
   `tile_title_name_cap_can_reach_zero_when_the_id_is_wide` were recomputed
   with the new budget and match the documented overhead formula
   (`overhead = 4 + id-digit-count + status-word-width`, id/name/status
   layout unchanged). I re-derived each by hand against the stated overhead
   and all match.

5. **`grid.rs` focus-highlight width** (`draw_grid`) — `width =
   tile.width` (was `tile.width.saturating_sub(2)`), since the highlighted
   title bar now spans the whole tile, not just the space between two side
   borders. Verified by the focus-color-block test, whose `left`/`right`
   assertions moved from `(41, 78)` (one column in from each tile edge, to
   clear the side borders) to `(40, 79)` (the tile's own edges).

6. **`mod.rs` PTY size sent to the agent** — `cols = area.width` (was
   `area.width.saturating_sub(2)`); `rows` unchanged (still `height - 2 - 3`
   for top/bottom border + bottom bar, which was never a left/right-border
   calculation). Comment updated to explain sides are no longer subtracted
   and to point at `attach::draw`'s `Borders::TOP | Borders::BOTTOM` as the
   thing this must stay consistent with. Correct: the PTY column count must
   equal the actual rendered content width, and content width is now the
   full area width.

7. **`mod.rs` bottom-bar `action_cols`** — `bar_widths(f.area().width)` (was
   `bar_widths(f.area().width.saturating_sub(2))`), consistent with the bar
   block now being `Borders::TOP | Borders::BOTTOM`. Two tests
   (`bar_widths`-derived key-fitting checks) had their `width - 2` /
   `80 - 2` calls updated to plain `width` / `80` to match — same
   computation the production code now does, not a weakened assertion.

8. **`mod.rs` project-info block** — block changed to
   `Borders::TOP | Borders::BOTTOM`; `inner` is taken from `block.inner(...)`
   as before (was already inner-rect-based, no hand-computed offset to fix).

9. **`keys.rs` popup inner width** — `inner_w = max_w.saturating_sub(2)` (was
   `saturating_sub(4)`): the old `4` was "1 col border + 1 col breathing
   room" on each side; with side borders gone only the 2 columns of
   breathing room remain. `want_w = widest + 2` (was `+ 4`) for the same
   reason, with a comment cross-referencing the two so they can't drift
   apart independently. I checked the two constants are still paired
   correctly (both changed by exactly 2, matching "lost one border column on
   each side").

No other `saturating_sub(2)` / `saturating_sub(4)` sites remained that were
about side-border width; I grepped for both plus `Borders::ALL` and the box
corner characters (`┌└┐┘`) across `src/` and reviewed every hit
(`keys.rs:209`'s `saturating_sub(2)` is an unrelated 2-space text indent used
in the help-wrap row-width sum, not a border calculation; `mod.rs:2071`'s
`saturating_sub(2)` is the existing gap between the bar's left/right text
segments, also unrelated to side borders).

## Tests touched, before/after

- `attach.rs::draw_records_the_bordered_content_corner_as_the_screen_origin`:
  expected `screen_origin` before `(area.x + 1, area.y + 1)` → after
  `(area.x, area.y + 1)`. Strengthened comment, same rigor (still pins an
  exact coordinate, not a range).
- `board.rs` two "don't let the border interfere" tests: comments reworded
  (no longer claim a right border exists) but assertions unchanged in kind.
- `board.rs::...ellipsis...` test: literal `"lastusedan-absurd…"` →
  `"lastusedan-absurdly…"`, recomputed for the new 36-column room (see #3
  above) — this is a *correction*, not a weakening: it still asserts the
  ellipsis lands at an exact character position.
- `grid.rs::tile_title_name_cap_is_derived_from_tile_width_not_fixed`: five
  expected values increased by 2 each (budget gained back the 2 border
  columns), same structure.
- `grid.rs::tile_title_name_cap_can_reach_zero_when_the_id_is_wide`: the four
  existing cases had their expected values increased by 2 each (budget 18 →
  20), and — since a 4-digit id no longer drives the cap to exactly 0 at the
  new, bigger budget — a fifth case (6-digit id, `overhead = 20 == budget`)
  was added so the test still actually exercises the "budget reaches zero"
  claim in its name and doc comment, instead of silently losing that
  coverage. This was already done correctly in the uncommitted diff; I
  verified the arithmetic by hand and it checks out.
- `grid.rs::multi_digit_session_ids_do_not_erode_the_status_words_budget`:
  comment only, recomputed 60-column-tile budget from 18 to 20; assertion
  behavior unchanged.
- `grid.rs` focus-color-block test: `left`/`right` `(41, 78)` → `(40, 79)`,
  matching the new (bigger) highlighted span — a correction, still an exact
  pixel-column check, not loosened to a range.
- `mod.rs::bar_top` helper: previously located the bottom bar by scanning
  column 0 for the last `┌` (top-left corner) — that character no longer
  exists anywhere. Reworked to: assert the screen's last row is a `─`
  (bottom bar's bottom edge always touches the terminal's last row, since
  it's the last vertical layout slice), then scan upward from there for the
  nearest `─` (top edge of the bottom bar). I checked this can't collide
  with the content block's own top/bottom border lines: the scan starts at
  the very last screen row and stops at the *first* `─` found going up,
  which by construction is the bottom bar's own top edge (nothing else sits
  between the bottom bar and the bottom of the screen). All three tests that
  call `bar_top` still pass with `--test-threads=1`.
- `mod.rs` two `bar_widths(width - 2)` / `bar_widths(80 - 2)` call sites: `- 2`
  dropped, matching the production `bar_widths(f.area().width)` call.

## Found and fixed

Nothing needed fixing. I looked hard for the two things called out as risks:
a test that lost its "reaches zero" coverage when the `tile_title_name_cap`
budget grew (see the fifth case discussed above — already present and
correct in the uncommitted diff), and any missed side-border compensation
(`saturating_sub(2)`/`-2`-style arithmetic, stale `Borders::ALL`, stale box
corner characters `┌└┐┘` used as anchors). I did not find any instance of
either. The uncommitted work is complete and internally consistent as-is; I
made no source changes beyond what was already there.

## Commands run

```
cargo fmt --check                     # clean, no output
cargo clippy --all-targets            # 0 warnings
cargo test -- --test-threads=1        # 754 passed; 0 failed (matches the
                                       # pre-change baseline count)
```

Full per-binary breakdown of the 754: unit tests 716, doc-tests 0, and the
remaining 38 spread across the integration test binaries (9, 1, 1, 1, 3, 3,
2, 6, 5, 2, 2, 1, 1, 1, 0), all green.

## Commit

`src/ui/*.rs` changes committed. `.superpowers/sdd/.gitignore`'s uncommitted
modification (rewritten by a tooling script per instructions) was left
unstaged/untouched.
