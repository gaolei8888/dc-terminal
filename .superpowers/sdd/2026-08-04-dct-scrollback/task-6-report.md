# Task 6 report — PTY 保留 2000 行历史，加滚动 API

## What changed

- `src/pty.rs`
  - Added `pub const SCROLLBACK_ROWS: usize = 2000;` and `pub struct ScrollView { offset, max, agent_owns, alt_screen }`, placed right before `PtySession`, comments verbatim from the brief.
  - `PtySession::spawn`: `vt100::Parser::new(rows, cols, 0)` → `vt100::Parser::new(rows, cols, SCROLLBACK_ROWS)`.
  - Added `PtySession::scroll_by`, `scroll_to_bottom`, `scroll_state`, and the private module functions `probe_max` / `view_of`, all matching the brief's bodies and comments.
  - `resize()`: `parser.set_size(...)` → `parser.screen_mut().set_size(...)` (vt100 0.16 API change, see below).
  - `screen_spans()`: `cell.contents()` now returns `&str` in vt100 0.16 (was `String` in 0.15); the empty-cell branch already produced an owned `String`, so the non-empty branch now calls `.to_string()` to match.
  - Test module: added the nine tests from the brief verbatim (`spawn_lines`, `keeps_history_that_scrolled_off_the_screen`, `history_is_capped_at_the_configured_size`, `scrolling_past_the_top_stops_at_the_top`, `scrolling_below_the_bottom_stops_at_the_bottom`, `the_view_stays_put_when_new_output_arrives` + `wait_for_offset_to_grow`, `an_alternate_screen_app_has_no_history_to_scroll`, `a_scroll_region_swallows_the_history`, `a_plain_shell_does_not_own_the_scrolling`, `an_app_that_asks_for_the_mouse_owns_the_scrolling`).
- `Cargo.toml`: `vt100 = "0.15"` → `vt100 = "0.16"`, with a WHY comment (see below).
- `Cargo.lock`: updated accordingly (`vt100` 0.15.2 → 0.16.2, `vte` 0.11.1 → 0.15.0, `unicode-width` 0.1.14 → 0.2.2 as a transitive bump pulled in by vt100 0.16; `unicode-width = "0.1.14"` in our own `[dependencies]` is untouched and still resolves independently for our own direct use).

Commit: `6495e98` on branch `feat/scrollback` — "feat: PTY 保留 2000 行历史，加滚动 API" (message in Chinese, `feat:` prefix, ends with the required `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>` trailer, no other AI attribution line). Touched only `src/pty.rs`, `Cargo.toml`, `Cargo.lock` — did not stage the pre-existing unrelated working-tree changes (`.superpowers/sdd/.gitignore`, an untracked docs file) that were already dirty before this task started.

## Test command and results

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -- --test-threads=1
```
→ every `test result: ok` across the run; lib suite: **496 passed, 0 failed**, plus 8 integration-test binaries and one doctest binary all green (9, 1, 1, 1, 3, 3, 2, 5, 3, 2, 2, 1, 1, 1, 0 across the various `tests/*.rs` files). Total passing = 531 = the controller's stated baseline of 522 + the 9 new tests this task added (pty went from 6 pre-existing tests to 15). 0 failures anywhere.

```
cargo test --lib pty:: -- --test-threads=1
```
→ `test result: ok. 15 passed; 0 failed`, all nine new scroll tests present by name and green, alongside the six pre-existing pty tests.

```
cargo fmt --check   → exit 0, no diff
cargo clippy --all-targets -- -D warnings   → clean, no warnings
git diff --check   → exit 0, no whitespace errors
```

## Where the brief didn't match reality (beyond the stale line numbers/counts already flagged by the controller)

Step 2 went as expected — `cargo test --lib pty::` failed to compile with `cannot find value 'SCROLLBACK_ROWS'` / `no method named scroll_by/scroll_state`, confirming the tests actually exercise the new API.

Step 4 (after implementing verbatim per the brief, on vt100 0.15.2 as the brief specifies) did **not** go as expected: 14/15 pty tests passed, but `keeps_history_that_scrolled_off_the_screen` panicked:

```
thread 'pty::tests::keeps_history_that_scrolled_off_the_screen' panicked at
.../vt100-0.15.2/src/grid.rs:125:42:
attempt to subtract with overflow
```

I isolated this with a standalone repro outside the workspace (a scratch `Cargo.toml` + `main.rs` calling `vt100::Parser::new(24,80,2000)`, pushing 100 lines, then `set_scrollback(90)`, then `.screen().contents()`) and confirmed it panics deterministically, single-threaded, with no race involved. Root cause: `Grid::visible_rows()` in vt100 0.15.2 computes

```rust
.chain(self.rows.iter().take(rows_len - self.scrollback_offset))
```

where `rows_len` is the fixed live-screen height (24 here) and `scrollback_offset` is the current scroll offset. Whenever the offset exceeds the screen height — which is the entire point of scrolling into multi-thousand-line history — this subtraction underflows. This is hit by *any* read of screen content (`contents()`, and `cell()` too, since both funnel through `visible_rows()`), not just our new test. So the brief's claim "`screen_spans()` 一行都不用改" is only true for offsets ≤ screen height; beyond that, vt100 0.15.2 itself is broken, independent of anything in our implementation.

I confirmed this is a known, already-fixed upstream bug: vt100 0.16.2's `grid.rs` has the identical code path but with an explicit comment ("when scrollback_offset > rows_len ... the skip(...) / take(...) would panic") and uses `rows_len.saturating_sub(self.scrollback_offset)` instead. I bumped `vt100` to `"0.16"` and adjusted the two call sites whose API shape changed:
- `Parser::set_scrollback` was removed in 0.16; it now lives on `Screen` only, reached via `parser.screen_mut().set_scrollback(...)`. Same for `Parser::set_size` → `parser.screen_mut().set_size(...)`.
- `Cell::contents()` changed from returning `String` (0.15) to `&str` (0.16), requiring one `.to_string()` in `screen_spans()`.

No other call site in the codebase touches `vt100::` (grepped), and no other behavior changed. After the bump, all 9 new tests plus the 6 pre-existing pty tests pass, and the full suite is green.

I did not change any test bodies, constants, function signatures, or the brief's Chinese comments — they're reproduced verbatim. The only genuinely new prose is the WHY comment I added above `vt100 = "0.16"` in `Cargo.toml`, explaining the version bump in the same comment style/density as the rest of the repo.

## Concerns

- **Dependency bump risk for the other 5 tasks in this SDD**: this task raises `vt100` from 0.15 to 0.16 workspace-wide. I verified (via source inspection) that every vt100 API this codebase currently touches (`Parser::new/process/screen/screen_mut`, `Screen::size/contents/contents_between/cell/cursor_position/alternate_screen/mouse_protocol_mode/set_scrollback/set_size/scrollback`, `Cell::contents/is_wide_continuation/fgcolor/bgcolor/bold/italic/underline/inverse`, `Color`, `MouseProtocolMode`) still exists with compatible signatures in 0.16.2, and the full test suite (531 tests) is green under it. But the five downstream tasks that build on this API should be aware the dependency moved, in case they lean on any vt100 behavior not exercised by today's tests.
- The brief's Step 3 code, taken completely verbatim against vt100 0.15.2, would panic in production the first time a real user scrolled more than one screenful into history — this wasn't a hypothetical edge case, it's the primary use case of the whole feature. Worth flagging to whoever wrote the brief that it was evidently drafted/tested against 0.15.2 without exercising an offset larger than the terminal height.
- I did not audit vt100 0.16's CHANGELOG for unrelated behavioral changes beyond the scrollback fix and the two signature changes I hit at compile time; the test suite is the evidence I'm relying on for "nothing else broke."

## Fix round 2 — review finding: `scroll_by` broken for incremental scrolling

Review caught a real bug in `scroll_by` (src/pty.rs), transcribed verbatim from the brief's own Step 3 snippet: it called `probe_max(&mut parser)` *before* reading `cur = parser.screen().scrollback()`. `probe_max`'s own doc comment says it mutates the live offset to the max as a side effect (that's how it discovers the upper bound — `set_scrollback(usize::MAX)` then read back what vt100 clamped it to). So by the time `scroll_by` read `cur`, the parser's offset had already been shoved to `max` by the probe — `cur` was never the caller's actual prior position, it was always `max`. Every call to `scroll_by` computed its target relative to `max`, not relative to where the view actually was. Two consecutive `scroll_by(5)` calls would both land at (approximately) `max`, not at 5 then 10 — every wheel tick would jump straight to the oldest line in history instead of advancing incrementally.

`scroll_state()`, two methods below, already had the correct order (read `cur`, then `probe_max`, then restore `cur`) — `scroll_by` just didn't follow the same pattern.

### Fix

Reordered `scroll_by` to read `cur` before calling `probe_max`, and added a comment explaining why the order is load-bearing so it doesn't get "simplified" back:

```rust
pub fn scroll_by(&self, rows: i32) -> ScrollView {
    let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
    // 顺序不能反：probe_max 会把偏移拨到顶当副作用（见它自己的文档），
    // 先读 cur 再 probe_max，不然 cur 读到的永远是上一次探测剩下的
    // max，而不是调用方真正的当前位置——增量滚动会变成每次都跳到顶。
    let cur = parser.screen().scrollback();
    let max = probe_max(&mut parser);
    let target = if rows >= 0 {
        cur.saturating_add(rows as usize)
    } else {
        cur.saturating_sub(rows.unsigned_abs() as usize)
    };
    parser.screen_mut().set_scrollback(target.min(max));
    view_of(&parser, max)
}
```

### Regression test

The brief's original nine tests all miss this because every one of them either scrolls past `max` in a single call (`i32::MAX`, `-1000` after a prior `scroll_by(10)` — the `-1000` overshoots the bottom which is 0, not `max`) or calls `scroll_by(90)` once from a fresh session against a `max` of roughly 76 — in every case the buggy "always relative to max" computation and the correct "relative to cur" computation happen to converge on the same clamped answer, so the bug never surfaces.

Added `scrolling_by_a_small_amount_twice_advances_instead_of_jumping_to_the_top`, right after `scrolling_below_the_bottom_stops_at_the_bottom`: spawns a session with 200 lines (comfortably larger than the 5-row step, so neither call comes close to `max`), calls `scroll_by(5)` twice, and asserts `offset == 5` after the first call and `offset == 10` after the second.

I verified the test actually catches the bug before finalizing: temporarily reverted the ordering fix (restored `probe_max` before `cur`) and reran just this test —

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --lib pty::tests::scrolling_by_a_small_amount_twice_advances_instead_of_jumping_to_the_top -- --test-threads=1
```

failed as expected:

```
thread '...' panicked at src/pty.rs:637:9:
assertion `left == right` failed: 第一次滚 5 行应该刚好停在 5
  left: 177
 right: 5
```

(177 was that run's `max` — the offset jumped straight to the top on the very first call, exactly the reported symptom.) Restored the fix and reran.

### Verification (after the fix was back in place)

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --lib pty:: -- --test-threads=1
```
→ `test result: ok. 16 passed; 0 failed` — all pty tests, including the new regression test, green.

```
cargo test -- --test-threads=1
```
→ every `test result: ok` across the whole run; lib suite now **497 passed, 0 failed** (up from 496 — the one new test), all integration-test binaries and the doctest binary still green, 0 failures anywhere.

```
cargo fmt --check     → exit 0, no diff
cargo clippy --all-targets -- -D warnings   → clean, no warnings
git diff --check      → exit 0
```

### Commit

`7000e57` on `feat/scrollback` — "fix: scroll_by 增量滚动前先读当前偏移，不被 probe_max 的副作用冲掉" (Chinese, `feat:`-repo-style but this one is a `fix:` prefix since it's a bug fix on top of the feature commit, same trailer convention: `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`, no other AI attribution line). Only `src/pty.rs` staged and committed; the same pre-existing unrelated dirty files (`.superpowers/sdd/.gitignore`, an untracked docs file) were left alone again.

No further concerns from this round — the fix is narrow (one reordering + one comment), the regression test is proven to catch the exact reported failure mode, and the full suite plus fmt/clippy are clean.
