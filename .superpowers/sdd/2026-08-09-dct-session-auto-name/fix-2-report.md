# Fix 2 report — grid tile title loses the status word to a long name

Base: `12e5b0f` (Fix 1 complete, on `feat/session-auto-name`).
Commits: `a1f2070` (failing test), `7048a24` (implementation).

## Option chosen: A (derive the cap from tile width)

Went with **A**, not B (reorder so the name is what gets clipped), because the
tile's available width is not constant the way the brief's arithmetic table
might suggest at a glance — `grid_shape` puts the grid into 1, 2, or 3 columns
depending on how many sessions are on the current page, so the *same* 60- or
80-column terminal gives a tile 40 columns wide with 2 sessions and only ~20
with 5+. A fixed cap (even a smaller fixed one, matching the pattern used in
`attach.rs` for the disconnected-title case) would either waste width in the
common 1–2 session case or still overflow in the 3-column case — it can't be
right for both without knowing the tile's real width. `tile.width` is already
in scope at the exact point the title is built (`draw_grid`, used one line later
for the focused-tile padding), so deriving from it costs nothing extra and
generalizes correctly across all three column counts, not just the two widths
the brief's table happens to enumerate.

Reordering (B) was rejected because it changes what the tile visually leads
with (status before name) for every tile, including the generously-wide
1-column case where nothing needs to be sacrificed at all — that's a bigger,
more visible interface change than the bug requires. A only changes behavior
in the tiles that are actually too narrow.

## Verifying the brief's arithmetic against real source

Read `grid_shape` (`src/ui/grid.rs:35-43`), `MIN_COLS` (`src/ui/grid.rs:130`
at the time of reading, now shifted by the new code), and the
`Layout::horizontal(Ratio(1, cols))` tile split (`draw_grid`, around
`src/ui/grid.rs:457` pre-change) directly, not from the brief.

- `grid_shape(5) == grid_shape(6) == (2, 3)`, `grid_shape(n >= 7) == (3, 3)` —
  confirmed. "5 个及以上会话 → 3 列" is correct.
- `MIN_COLS = 60`, checked as `area.width < MIN_COLS` before `draw_grid` ever
  builds tiles (`draw_grid` early-return branch) — confirmed 60 is fully
  supported, not a degraded/unsupported width.
- Tile width = terminal width / cols (via `Ratio(1, cols)`), minus 2 columns
  of border via `tile.width.saturating_sub(2)` (the same subtraction the
  existing focused-tile padding code already did one line below the title
  construction) — confirmed. 60÷3−2=18, 80÷3−2=24, 120÷3−2=38, matching the
  brief's table exactly.
- **One thing the brief got wrong, worth flagging like the ledger asked**: the
  brief claims the two pre-existing `干活中` assertions
  (`tiles_show_the_session_status_in_the_title`, `a_tile_without_a_screen_
  yet_still_draws_its_title`) render at "120×30 且 tag 为空". Actual source
  (`src/ui/grid.rs`, before this change): both use `TestBackend::new(80, 24)`,
  and the first has 2 sessions (`grid_shape(2) == (1, 2)`, tile width 40), the
  second has 1 session (`grid_shape(1) == (1, 1)`, tile width 80). Neither is
  120×30 and neither exercises the 3-column layout — but the brief's
  underlying conclusion ("these tests pass either way, budget is generous")
  still holds, it's just generous for a different reason (few columns, not a
  wide terminal). Doesn't change what to build; recorded per the "verify
  before trusting" instruction.
- Longest status word check (not in the brief, found while designing the
  floor): English `asking you` is 10 display columns — longer than
  `working` (7) — via `src/i18n.rs:426`. This is why the arithmetic function
  takes the actual status word rather than assuming Working is the worst case;
  it's also the scenario used to exercise `MIN_TITLE_NAME_COLS` in a unit
  test, since `Working` state never drives the cap low enough to hit the
  floor at any width `MIN_COLS` (60) actually allows.

## What changed

- Added `tile_title_name_cap(tile_width: u16, id: u32, status_word: &str) ->
  usize` as a pure function (module's "upper half is pure layout math" style,
  same neighborhood as `grid_shape`/`crop_line`). Budget = `tile_width - 2`
  (border columns) minus fixed overhead (focus marker, id digit width, two
  interior spaces, status word width, trailing space), floored at
  `MIN_TITLE_NAME_COLS = 4` so the name never vanishes entirely in extreme
  combinations. The project name is deliberately excluded from the overhead
  accounting — it doesn't go through `truncate` today (pre-existing gap at
  `src/ui/grid.rs:495`, tracked separately, not touched) and charging it here
  would only shrink the name budget for no actual guarantee.
- `draw_grid`'s tile title now calls `truncate(session_label(info),
  tile_title_name_cap(tile.width, info.id, status_word))` instead of the
  fixed `truncate(session_label(info), 20)`.
- Left the *other* `truncate(session_label(s), 20)` call site alone — the one
  in `draw()` building the reply-row "to" prefix (`src/ui/grid.rs`, ~line
  400). That row is a full-width overlay (`draw_reply`'s `row.width =
  area.width`, not per-tile), already has its own width-sensitive test
  (`a_long_name_never_pushes_the_draft_off_the_reply_row`), and isn't part of
  this brief's scope (`src/ui/grid.rs:475-486` only). Its neighboring comment
  used to claim "same cap as the tile title (20)", which became false once
  the tile cap went dynamic — updated the comment only, not the logic, so it
  doesn't keep asserting something no longer true.

## Tests added

1. `tile_title_name_cap_is_derived_from_tile_width_not_fixed` — pure
   arithmetic, five assertions covering 60/80/120-column, 3-column-layout
   budgets in both languages, checked by hand against the derivation above.
2. `tile_title_name_cap_has_a_floor_so_the_name_never_disappears` — pure
   arithmetic, exercises the `MIN_TITLE_NAME_COLS` floor with the
   `asking you` / 60-column combination that actually drives the raw
   subtraction negative (`saturating_sub` would give 0 without the floor).
3. `a_long_name_never_pushes_the_status_word_off_the_tile_title` — full
   render (`TestBackend`), 5 sessions (forces the 3-column layout), **all
   five** tiles given the same 24-character tag (not just one — see the
   test's own doc comment for why: with only one long-named tile, the other
   four short-named tiles would show the status word regardless of the bug,
   making the assertion pass vacuously), asserted for both `Lang::Zh` and
   `Lang::En` at both 60 and 80 columns.

## TDD

Wrote test 3 first against the unmodified tile title code (still
`truncate(session_label(info), 20)`), ran it, confirmed it failed:

```
$ cargo test --lib ui::grid::tests::a_long_name_never_pushes_the_status_word_off_the_tile_title -- --test-threads=1
test ui::grid::tests::a_long_name_never_pushes_the_status_word_off_the_tile_title ... FAILED
panicked at src/ui/grid.rs:1011:17:
Zh 语言、60 列下，24 字符的名字把状态词挤出了格子：┏▶1修修修修修修修━┓...
```

Committed that red test on its own (`a1f2070`), then implemented
`tile_title_name_cap` and wired it in (`7048a24`), confirming green.

## Mutation testing

Per the brief's explicit list, ran each of the following by hand: edit,
`cargo test --lib ui::grid::`, observe, revert, confirm green again.

1. **Derivation → fixed value.** Replaced the function body with a constant
   `20` (params prefixed `_` to keep it compiling). Result: **3 tests failed**
   — both pure-arithmetic tests (expected values didn't match 20) and the
   integration test (`a_long_name_never_pushes_the_status_word_off_the_tile_
   title`, status word pushed off the 60-column tile again).
2. **Remove the floor** (`.max(MIN_TITLE_NAME_COLS)` deleted). Result: **1
   test failed** — `tile_title_name_cap_has_a_floor_so_the_name_never_
   disappears` (got 3 instead of 4). The integration test stayed green
   because `Working` state never drives the cap that low at any width
   `MIN_COLS` permits — this is expected and documented in that test's own
   comment; the floor is a genuine edge-case guard, not something the primary
   scenario exercises.
3. **Reorder name/status** — not applicable; Option A was chosen, nothing in
   the diff reorders the title spans. Skipped per the brief's own phrasing
   ("哪个选了哪个" implies this mutation targets Option B).
4. **Acceptance criterion — delete the `truncate()` call from the tile title
   entirely.** Replaced `truncate(session_label(info), name_cap)` with plain
   `session_label(info)`. Result: `a_long_name_never_pushes_the_status_word_
   off_the_tile_title` **failed** (status word pushed off at 60 columns,
   same failure shape as the original TDD red). Reverted immediately after
   confirming.

All four mutations were caught by at least one test; reverted each back to
the committed state and reran the full grid suite (65/65 green) before moving
on.

## Verification commands and output tails

```
$ cargo fmt --check
(clean, no output)

$ cargo clippy --all-targets
    Checking dct v0.1.0 (/Users/lei/work/dc/dc-terminal)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.25s

$ cargo test --lib ui::grid:: -- --test-threads=1
test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 628 filtered out; finished in 0.07s

$ cargo test -- --test-threads=1
test result: ok. 693 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 32.51s
(plus 15 integration-test binaries, all "0 failed")

$ git diff --check
(clean, no output)
```

## Concerns

- None blocking. The `MIN_TITLE_NAME_COLS = 4` floor means that in a
  hypothetical combination worse than anything the brief's test matrix
  covers (very long status word, e.g. `asking you`, combined with a
  multi-digit session id, at exactly 60 columns), the status word could
  still lose ~1 column to the name floor. This is documented in the
  function's doc comment and in `MIN_TITLE_NAME_COLS`'s own doc comment as a
  deliberate, bounded trade-off (name must show *something*), not an
  oversight — and it doesn't regress anything the old fixed-20 code handled,
  since the old code had no floor logic and no name-vs-status budget
  awareness at all in that combination either.
- Did not touch `src/ui/grid.rs:495` (project name bypassing `truncate`) or
  the reply-row `who` construction in `draw()` — both out of scope per the
  brief and the task instructions, confirmed by direct reading rather than
  taking the brief's scope note on faith.
