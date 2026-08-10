# Fix 2 report — grid tile title loses the status word to a long name

Base: `12e5b0f` (Fix 1 complete, on `feat/session-auto-name`).
Commits: `a1f2070` (failing test), `7048a24` (implementation), `07862d8`
(round-1 fix: drop the name floor).

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

## Round 1 addendum — the floor was actively wrong

The reviewer rendered the floored code by hand at 60 columns, `Asking`,
English, and found the floor clipping `asking you` down to `asking yo` /
`asking y` / `asking ` for ids 1/10/100 — worse the wider the id got, and not
English-only (`等你回答` loses its last character at id >= 100 too). My own
concern note had understated this as "roughly 1 column, Working-only-safe";
it was actually up to 3 columns, and it hit the most important state
(`Asking` — the agent is blocked on the user) precisely because that state's
word is the widest.

### Verifying the claim before acting on it

Per `receiving-code-review`, worked the arithmetic by hand against the real
`truncate` (`src/ui/widgets.rs:164-183`) before touching code:

- `truncate(s, max)` never returns an empty string. When the first character
  doesn't fit, it returns exactly `"…"` (width 1) with no characters
  consumed — this is the reviewer's "truncate is its own floor" claim, and
  it holds regardless of how small `max` is, including 0.
- When truncation happens after `n` characters fit, the returned width is
  `w + 1` where `w <= max` (width already consumed before the character that
  overflowed) — so overshoot past `max` is **at most** 1 column, not always
  exactly 1 (it's 0 when the string's character widths land exactly on
  `max`, checked both with all-ASCII and all-CJK strings by hand).
- Reconstructed the reviewer's three id/asking-you renderings from the
  arithmetic (floored cap = 4 for all three ids because 18−15, 18−16, 18−17
  all round-trip through `.max(4)`; unfloored cap = 3/2/1) and the predicted
  clipped/fixed status text matched their claimed buffers exactly for
  id=1/10/100, confirming the claim rather than trusting it.
- Pushed the same arithmetic further than the review's examples and found a
  **new, smaller edge the review's proof doesn't cover**: the "status word
  fully visible" invariant only holds while `overhead <= budget`. At 60
  columns with `Asking`/English, `overhead = 14 + id_digits`, so the
  invariant holds through 4-digit ids (`9999`, overhead=18=budget, 1-column
  overshoot absorbed by the trailing space) but breaks at 5 digits
  (`10000`, overhead=19>budget=18): even the bare `"…"` name still pushes 2
  columns over budget, and the second of those 2 columns comes out of the
  status word itself (`asking you` → `asking yo`, verified by writing the
  test with id=10000 first, watching it fail, and reading the printed
  buffer). This is a real, narrower gap than what the floor was covering,
  discovered while verifying the fix rather than trusting the proof at face
  value. **Left it out of scope deliberately** — a 5-digit session id
  requires roughly 10,000 sessions created over a daemon's lifetime, well
  past what this round's brief or the coordinator's examples (id up to 100)
  called for, and unilaterally expanding a requested floor-removal into a
  budget-formula rewrite is scope creep this task shouldn't take on its own
  authority. Flagging it here as a candidate triage item alongside the three
  the coordinator already listed (project-name comment, per-frame
  allocation, tile/reply-row disagreement at 120 columns).

### What changed

- `src/ui/grid.rs`: deleted `MIN_TITLE_NAME_COLS` and the `.max(...)` call
  in `tile_title_name_cap`; `tile_title_name_cap` is now a straight
  `budget.saturating_sub(overhead)`.
- Replaced `tile_title_name_cap_has_a_floor_so_the_name_never_disappears`
  (self-referential — asserted `== MIN_TITLE_NAME_COLS`, so it would have
  stayed green if the constant itself were raised) with
  `tile_title_name_cap_can_reach_zero_when_the_id_is_wide`, asserting
  literal values (3, 2, 1, 0) for `Asking`/60-columns across 1-4 digit ids.
- Widened `a_long_name_never_pushes_the_status_word_off_the_tile_title` from
  `Working`-only to a loop over `Working`/`Asking`/`Idle`/`Failed`.
  `Stopped` is deliberately excluded: rendering 5 sessions that are all
  `Stopped` hits an unrelated pre-existing branch (`draw_grid` replaces the
  whole tile grid with a "they're all stopped, press g/n" screen — already
  covered by `a_grid_of_only_stopped_sessions_says_where_they_went`), not
  the tile-title code path this fix touches. Discovered this by running the
  loop with `Stopped` included first and reading the failure — the buffer
  contained the "all stopped" sentence, not a stale/clipped tile.
- Added `multi_digit_session_ids_do_not_erode_the_status_words_budget`,
  reproducing the reviewer's exact id=1/10/100/`Asking`/60-column scenario
  plus the 1000/9999 boundary the arithmetic still guarantees (stopped at
  9999, not 10000+, for the reason above).
- Both new render tests initially had a real bug of their own, caught by
  running them before trusting them green: `status_label(Asking, En)` is
  `"asking you"`, the only status word with an internal space, but the
  assertion compared it against `squashed(&term)`, which strips **all**
  whitespace from the rendered buffer (documented on `squashed` itself,
  pre-existing). `c.contains("asking you")` can never match a string with no
  spaces in it — first run failed with a message claiming the status word
  was clipped when the buffer plainly contained `askingyou` intact. Fixed by
  stripping whitespace from the expected status string the same way before
  comparing, and left a comment explaining why (so the next person doesn't
  reintroduce the same false failure).

### Mutation testing (repeated on the amended code)

Same by-hand protocol as before: edit, run, observe, revert, confirm green.

1. **Acceptance criterion — delete `truncate()` from the tile title.**
   `truncate(session_label(info), name_cap)` → `session_label(info)`.
   Result: 2 tests failed (`a_long_name_never_pushes_the_status_word_off_the_tile_title`,
   `multi_digit_session_ids_do_not_erode_the_status_words_budget`). Reverted.
2. **Derivation → fixed value.** `tile_title_name_cap` body → `20`
   (unconditionally). Result: 4 tests failed (both render tests plus both
   pure-arithmetic tests). Reverted.
3. **Reintroduce the deleted floor** (`.max(4)` added back) — this is the
   exact regression the review found, run against the amended test suite to
   confirm it's now caught. Result: 3 tests failed
   (`a_long_name_never_pushes_the_status_word_off_the_tile_title` at the
   `Asking` iteration, `multi_digit_session_ids_do_not_erode_the_status_words_budget`,
   `tile_title_name_cap_can_reach_zero_when_the_id_is_wide`). Reverted.

### Test commands and output tails

```
$ cargo test --lib ui::grid:: -- --test-threads=1
test result: ok. 66 passed; 0 failed; 0 ignored; 0 measured; 628 filtered out; finished in 0.09s

$ cargo fmt --check
(clean, no output)

$ cargo clippy --all-targets
    Checking dct v0.1.0 (/Users/lei/work/dc/dc-terminal)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4s

$ cargo test -- --test-threads=1
test result: ok. 694 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in ~32-40s
(plus 15 integration-test binaries; one run showed `tests/grid_reply.rs`
FAILED on 2 tests — a real socket+PTY test that spawns a daemon and a login
shell and waits on wall-clock deadlines, unrelated to this file. Re-ran
`cargo test --test grid_reply -- --test-threads=1` in isolation immediately
after: 2 passed, 0 failed, confirming it was load-induced flake in the full
parallel run, not a regression from this change. Full suite re-run after
came back clean.)
```

### Files whose tests were re-run for this round

- `src/ui/grid.rs` (`ui::grid::tests::*`, 66 tests) — the only file this
  round's commit touches.
- Full workspace `cargo test` (694 lib tests + all `tests/*.rs` integration
  binaries) to catch any unexpected interaction.

### Concerns (superseding the previous round's concern)

- The previous round's stated concern ("the floor could still cost the
  status word ~1 column in extreme combinations") is retracted — that floor
  is now gone, and the invariant it was hedging against no longer needs
  hedging for any case the removal itself can guarantee.
- New concern, deliberately not fixed this round: **5+ digit session ids
  (>= 10000) can still clip the last 1-2 characters off the status word** at
  60 columns with the `Asking` state, because at that point `overhead` alone
  (before the name contributes anything) exceeds the tile's budget. This is
  a materially smaller and further-out edge than the floor bug — the review
  proof's own examples never reached it — but it is real, reproduced by
  hand, and worth a line in the final review's triage list alongside the
  three items already flagged (project-name comment overpromising,
  per-frame `id.to_string()` allocation, tile/reply-row disagreement at 120
  columns).
