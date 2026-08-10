# Fix 3 report — board list gets test coverage for session naming

Brief: `.superpowers/sdd/2026-08-09-dct-session-auto-name/fix-3-brief.md`
Target: `src/ui/board.rs:211,214`, the session-row line built by
`pad_to(&truncate(session_label(s), 15), 16) + truncate(&s.activity, 70)`.

Scope discipline: only tests were added. No production code line changed
(the four "mutations" below were introduced and reverted by hand during
verification, never left in place).

## Independent check of the brief's column arithmetic

I read `truncate`, `pad_to`, and `session_label` in `src/ui/widgets.rs`
(lines 127–204) directly before writing any test, per the brief's own
instruction not to trust its arithmetic blindly.

```rust
pub(crate) fn truncate(s: &str, max: usize) -> String {
    let mut w = 0;
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_control() { continue; }
        let cw = char_width(ch);
        if w + cw > max {
            out.push('…');
            return out;
        }
        w += cw;
        out.push(ch);
    }
    out
}

pub(crate) fn pad_to(s: &str, width: usize) -> String {
    let mut out = s.to_string();
    out.push_str(&" ".repeat(width.saturating_sub(display_width(s))));
    out
}
```

**The brief's summary is a simplification, not a bug.** It says
`truncate(s, 15)` returns "at most 15 columns, or exactly 16 when
truncation actually happens." That second half is only true when the
running width `w` happens to reach exactly `max` (15) right before the
character that triggers truncation — e.g. 15 one-column ASCII characters
followed by more input. If the trigger fires earlier (for example a
two-column CJK character pushes `w` from 14 straight past 15), `truncate`
returns `w + 1 = 15` columns, not 16. I confirmed this with a scratch
case: `truncate("A".repeat(14) + "一...", 15)` yields 15 display columns,
not 16.

This does **not** invalidate the brief's conclusion. The property that
actually matters for the row layout is the *composition* of `truncate`
and `pad_to`, not `truncate` alone:

- `truncate(s, 15)` output width is always **≤ 16** (proof: without
  truncation it's `display_width(s) ≤ 15`; with truncation it's
  `w + 1` where `w ≤ 15`, so `≤ 16`).
- `pad_to(x, 16)` pads any `x` with `display_width(x) < 16` up to
  exactly 16, and is a no-op when `display_width(x) ≥ 16` (guarded by
  `saturating_sub`).
- Since `truncate`'s output is always ≤ 16, `pad_to(..., 16)` always
  either pads it up to exactly 16, or leaves it at exactly 16 (the
  boundary case). **The field is always exactly 16 columns after
  `pad_to`, regardless of whether `truncate` alone produced 15 or 16.**

So: arithmetic is correct, `pad_to` is not redundant in every truncation
case (it can still be adding a column in the CJK-boundary case), and the
brief's directive to guard the composed invariant rather than rewrite it
stands. I did not change `truncate`, `pad_to`, or `session_label`.

`session_label` (widgets.rs:198–204) is a plain "tag if non-empty, else
profile" fallback — no surprises there.

## Test harness note

`board.rs:266`'s `screen_text()` helper strips all whitespace before
comparing, which is exactly right for "did this text render" checks but
structurally **cannot** detect a missing `pad_to` — removing padding only
changes how many blank cells separate two pieces of content, and blank
cells vanish under `screen_text`'s own whitespace filter. To kill that
mutation I added two small helpers in the same test module:

- `row_with(term, marker)` — same `TestBackend`/`buf.cell()` mechanics as
  `screen_text`, but returns one row **unfiltered** (spaces preserved),
  so column positions survive.
- `cols_between(row, from_marker, to_marker)` — locates two ASCII markers
  in that row and returns the **display width** (via
  `unicode_width::UnicodeWidthStr::width`, not byte length) of the slice
  between them. Byte length would be wrong here because `…` and CJK
  characters are multi-byte in UTF-8 while being 1–2 display columns;
  I hit this mismatch once while drafting the test and switched to
  `UnicodeWidthStr` to fix it.

Both helpers reuse the existing `TestBackend` + `term.draw()` +
`buf.cell()` idiom; they don't introduce a second test infrastructure.

## Tests added (`src/ui/board.rs`, in `mod tests`)

1. `the_session_row_shows_the_name_not_the_profile`
2. `a_short_name_is_padded_out_to_the_full_sixteen_columns`
3. `an_oversized_name_is_truncated_before_it_can_eat_the_activity_budget`
4. `the_activity_column_still_truncates_at_seventy_not_seventy_six`

All four are purely synchronous (`App::test_app()` → `set_sessions()` →
`term.draw()`, no tick/sleep/poll), per the ledger's warning about the
six existing naming tests that degrade to false greens under a 50 ms
poll racing a 0.2 s window.

## Mutation kill table

Each mutation was introduced by hand with `Edit`, verified red with
`cargo test --lib ui::board::tests::<name> -- --test-threads=1`, then
reverted and verified `git diff` was byte-identical to before the
mutation (clean revert, no drift) before moving to the next one.

| # | Mutation | Test that kills it | Observed red output (assertion line) |
|---|---|---|---|
| 1 | `session_label(s)` → `s.profile` (list never shows the name) | `the_session_row_shows_the_name_not_the_profile` | `assertion failed: c.contains("改登录页文案")` — `会话行必须画出名字` — the tag never reached the screen because the name field rendered `s.profile` ("claude") instead. |
| 2 | delete `truncate(…, 15)` | `an_oversized_name_is_truncated_before_it_can_eat_the_activity_budget` | `assertion \`left == right\` failed` — `left: 20, right: 16` — the untruncated 20-column tag pushed the activity marker 4 columns further right than the 16-column budget allows. |
| 3 | delete `pad_to(…, 16)` | `a_short_name_is_padded_out_to_the_full_sixteen_columns` | `assertion \`left == right\` failed` — `left: 4, right: 16` — a 4-column tag with no padding left the activity marker sitting right after it instead of at column 16. |
| 4 | activity `70` → `76` | `the_activity_column_still_truncates_at_seventy_not_seventy_six` | `assertion failed: !c.contains("MARK")` — the `MARK` marker (columns 71–74 of the activity string) became visible once the truncate budget widened enough to include it. |

Full transcripts of each red run are below (commands run from
`/Users/lei/work/dc/dc-terminal`, `cargo` on PATH via `source
"$HOME/.cargo/env"`).

### Mutation 1 — `session_label(s)` → `&s.profile`

```
$ sed -i '' '211s/truncate(session_label(s), 15)/truncate(\&s.profile, 15)/' src/ui/board.rs
$ cargo test --lib ui::board::tests::the_session_row_shows_the_name_not_the_profile -- --test-threads=1
...
test ui::board::tests::the_session_row_shows_the_name_not_the_profile ... FAILED
thread '...' panicked at src/ui/board.rs:399:9:
会话行必须画出名字：┌dct会话看板...││▶┃1▾proj/var/folders/4t/…claude×1││┃1空闲claude│││...
test result: FAILED. 0 passed; 1 failed
```
Reverted (`cp` from a pre-mutation backup); `diff` against the backup was
empty afterward.

### Mutation 2 — delete `truncate(…, 15)`

```
$ # line 211 changed to: pad_to(session_label(s), 16)
$ cargo test --lib ui::board::tests::an_oversized_name_is_truncated_before_it_can_eat_the_activity_budget -- --test-threads=1
...
test ui::board::tests::an_oversized_name_is_truncated_before_it_can_eat_the_activity_budget ... FAILED
thread '...' panicked at src/ui/board.rs:458:9:
assertion `left == right` failed: `LONGNAME…` 到 `ACTV_TAIL` 应该正好隔 16 列；删掉 `truncate` 的话整条 20 列的 tag 会原样画出来，这里会变成 20："│  ┃    1  空 闲     LONGNAMEXXXXXXXXXXXXACTV_TAIL ...│"
  left: 20
 right: 16
test result: FAILED. 0 passed; 1 failed
```
Reverted; diff empty.

### Mutation 3 — delete `pad_to(…, 16)`

```
$ # line 211 changed to: truncate(session_label(s), 15).to_string()
$ cargo test --lib ui::board::tests::a_short_name_is_padded_out_to_the_full_sixteen_columns -- --test-threads=1
...
test ui::board::tests::a_short_name_is_padded_out_to_the_full_sixteen_columns ... FAILED
thread '...' panicked at src/ui/board.rs:429:9:
assertion `left == right` failed: `NAME` 到 `ACTV_TAIL` 应该正好隔 16 列（`pad_to` 补齐的宽度）；删掉 `pad_to` 的话这里会变成 4（tag 自己的宽度，一点没补）："│  ┃    1  空 闲     NAMEACTV_TAIL ...│"
  left: 4
 right: 16
test result: FAILED. 0 passed; 1 failed
```
Reverted; diff empty.

### Mutation 4 — activity `70` → `76`

```
$ # line 214 changed to: truncate(&s.activity, 76)
$ cargo test --lib ui::board::tests::the_activity_column_still_truncates_at_seventy_not_seventy_six -- --test-threads=1
...
test ui::board::tests::the_activity_column_still_truncates_at_seventy_not_seventy_six ... FAILED
thread '...' panicked at src/ui/board.rs:490:9:
70 列预算下 MARK 连一个字都不该露出来；如果预算被改回 76，MARK 会整个冒出来：┌dct会话看板...││▶┃1▾proj/var/folders/4t/…claude×1││┃1空闲claudeAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAMARK│││...
test result: FAILED. 0 passed; 1 failed
```
Reverted; diff empty (`diff /tmp/board.rs.bak src/ui/board.rs` printed
nothing after the final revert, confirming a clean return to the
committed-to-be-committed source).

All four mutations are dead. None survived.

## Final verification commands and output tails

Targeted board tests, real (unmutated) code:

```
$ cargo test --lib ui::board:: -- --test-threads=1
running 19 tests
test ui::board::tests::a_group_header_calls_out_how_many_sessions_failed ... ok
test ui::board::tests::a_group_whose_folder_is_gone_says_so_instead_of_vanishing ... ok
test ui::board::tests::a_long_cjk_project_name_never_pushes_the_failure_count_off_screen ... ok
test ui::board::tests::a_short_name_is_padded_out_to_the_full_sixteen_columns ... ok
test ui::board::tests::a_tag_with_control_bytes_never_reaches_the_rendered_buffer ... ok
test ui::board::tests::an_empty_project_names_the_agent_it_last_used ... ok
test ui::board::tests::an_empty_project_with_no_recorded_agent_says_only_what_it_knows ... ok
test ui::board::tests::an_over_long_agent_name_is_cut_visibly_not_by_the_border ... ok
test ui::board::tests::an_oversized_name_is_truncated_before_it_can_eat_the_activity_budget ... ok
test ui::board::tests::collapsing_a_group_hides_its_sessions_and_keeps_the_cursor_on_the_header ... ok
test ui::board::tests::digits_go_straight_to_a_project_and_ignore_the_ones_that_are_not_there ... ok
test ui::board::tests::enter_on_a_group_header_does_nothing ... ok
test ui::board::tests::entering_a_session_never_announces_a_project_change ... ok
test ui::board::tests::g_enters_the_grid_focused_on_the_selected_session ... ok
test ui::board::tests::no_builtin_agent_name_is_wider_than_the_header_budget_allows ... ok
test ui::board::tests::tab_jumps_to_the_next_project_and_wraps ... ok
test ui::board::tests::the_activity_column_still_truncates_at_seventy_not_seventy_six ... ok
test ui::board::tests::the_board_groups_every_session_under_its_own_project ... ok
test ui::board::tests::the_session_row_shows_the_name_not_the_profile ... ok

test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 679 filtered out; finished in 0.04s
```

Full suite, single-threaded (as required — the ledger's six naming tests
are timing-sensitive under parallel load):

```
$ cargo test -- --test-threads=1
...
Running unittests src/lib.rs ...
test result: ok. 698 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 32.13s
...
Running tests/grid_reply.rs (target/debug/deps/grid_reply-a0dfc86024059b18)
running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.96s
...
[15 more integration-test binaries, all "test result: ok", 0 failed]
$ echo $?
0
```
17 `test result:` blocks total, all green, exit 0. `grid_reply.rs` — the
one the ledger flagged as flaky under parallel load — passed cleanly
here running serially; no re-run in isolation was needed.

Formatting and lints:

```
$ cargo fmt --check
(no output, exit 0)

$ cargo clippy --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.11s
(no warnings, exit 0)

$ git diff --check -- src/ui/board.rs
(no output, exit 0)
```

## Diff summary

`src/ui/board.rs`: +163 lines, 0 deletions — test-only, matches the
brief's "only add tests" constraint. No other file touched by this task.

## Concerns

None. All four named mutations are killed, the brief's arithmetic holds
(with the one clarification above, which changes no conclusion), and the
full suite plus fmt/clippy are clean.
