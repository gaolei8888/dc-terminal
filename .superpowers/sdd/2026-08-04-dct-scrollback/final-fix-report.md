# Final fix wave — scrollback branch review findings

Branch: `feat/scrollback`. All seven findings from the whole-branch review addressed in one commit.

## B1 — End became a dead key after agent_owns flipped mid-session (BLOCKING)

`src/ui/attach.rs`, `key_scroll`: moved the `KeyCode::End if st.offset > 0` check to the top of the
function, before the `agent_owns` bail. `End` is now claimed whenever dct still owes the user a
scroll-back, regardless of which way `agent_owns` reads on this frame. Only when `offset == 0` does
`agent_owns` get a chance to hand `End` to the agent (as an ordinary editing key).

**Verification**: added `end_is_claimed_even_when_the_agent_now_owns_the_screen` (`agent_owns: true,
offset: 40` → asserts `End` still produces `Scroll(-i32::MAX)`). Confirmed it fails against the
pre-fix code: reverted just the reorder (kept the new test), reran — `unwrap()` panicked on `None`
because `key_scroll` bailed out on `agent_owns` before ever looking at `End`. Restored the fix, test
passes, existing `page_keys_belong_to_the_agent_when_it_owns_the_viewport` (offset 0) and
`end_with_nothing_scrolled_is_not_a_scroll_key` still pass unchanged.

## F2 — comment overclaimed the mouse bounds check

`src/ui/attach.rs`, `handle_mouse`: rewrote the comment above the `checked_sub` coordinate
translation. It no longer claims clicks on the border/bottom bar are dropped — `checked_sub` only
catches the top/left side (negative results); a click below or right of the content origin still
produces a valid non-negative (col, row) and gets forwarded, off by whatever the border/bottom-bar
overhang is. Explained why that's tolerated (agents ignore out-of-range coordinates) and what a
real fix would need (a `Rect`, not just an origin point) — out of scope for this wave, so behavior
unchanged.

**Verification**: comment-only change; confirmed by rereading the code path and the existing test
`a_click_with_no_known_screen_origin_is_dropped_not_guessed`, which only exercises the "no origin at
all" case and was never exercising the bottom-right overhang the old comment claimed to guard.

## F3 — Response::Scrolled doc claimed a consumer that doesn't exist

`src/proto.rs`: rewrote the doc comment on `Response::Scrolled`. It no longer says the UI reads it
to refresh the bottom bar directly. Now states plainly that every current call site discards the
result with `let _ = ...` and that the bar refresh comes from the next 16ms `Screen` poll's `scroll`
field instead, and explains why the variant is still worth keeping in this shape.

**Verification**: comment-only; confirmed via `grep -n "Request::Scroll" src/ui/*.rs` — both call
sites (`attach::handle_key`'s scroll branch and the new `enter_session` call added for F7) discard
the response with `let _ =`.

## F4 — i18n comment named the wrong agent as the example

`src/i18n.rs`, `agent_owns_the_screen`: rewrote the doc comment. It no longer offers Claude Code as
the example of an agent this hint applies to. Now states the exact trigger condition
(`!agent_owns && alt_screen`) and names the real occupants (`less`, `vim`, `htop` — full-screen
programs that ignore the mouse), explicitly calling out that Claude Code takes the mouse and goes
through a different branch. User-facing strings unchanged (they were already correct — this was a
comment-only fix, and the task explicitly said that distinction is the routing rule so it had to be
right).

**Verification**: comment-only; cross-checked against `wheel_action`'s actual judgment (`agent_owns`
= mouse reporting is on) and `scroll_hint`'s guard (`!st.agent_owns && st.alt_screen`) in
`src/ui/attach.rs` to confirm the corrected description matches the real branch condition.

## F5 — SCROLLBACK_ROWS memory comment was wrong, and silent about retention

`src/pty.rs`: corrected the arithmetic — vt100 0.16.2's `Cell` is 32 bytes, not 36 (confirmed by
reading `vt100-0.16.2/src/cell.rs`, which has `const _: () = assert!(std::mem::size_of::<Cell>() ==
32)` baked into the crate itself). Recomputed: 120 cols × 32 B ≈ 3.75 KB/row, 2000 rows ≈ 7.5
MB/session (was: 4.2 KB/row, 8.4 MB/session). Also confirmed and documented that `vt100::Row::new`
is `vec![Cell::new(); cols]` — a row's cells are allocated eagerly, not grown lazily by character
count; "grows with usage" only describes row *count*, not per-row bytes.

Added the missing consequence: `SessionManager::stop` (src/session.rs:542-553) only kills the child
and flips state to `Stopped` — the `Session` and its parser (and its 2000-row buffer) live on until
someone calls `prune` (src/session.rs:577), which has no automatic caller. So a stopped-but-unpruned
session now holds several MB instead of the ~90 KB it held before scrollback. Documented as an
intentional trade-off (stopped sessions stay attachable for `u`/`d` history review), not a leak, per
the instruction not to change retention behavior.

**Verification**: comment-only; confirmed the 32-byte assert and eager `Vec` allocation by reading
the vendored vt100 0.16.2 source directly, and confirmed the stop/prune gap by reading
`SessionManager::stop` and `SessionManager::prune` in `src/session.rs`.

## F6 — English scroll hints got clipped on narrower terminals

`src/i18n.rs`: shortened `scroll_new_lines_below` and `scrolled_up`'s English text (chose the
"shorten the wording" option over "let it wrap", since these hints go through `BarContent::Text` →
`wrap_help`, which only splits on double-spaces — these strings are all single-spaced, so they were
never actually wrapping, just getting truncated by `Paragraph`'s right-edge clip).

- `scrolled_up`: `"↑ Scrolled up {offset} line(s) · press End to jump back down"` (54 cols) →
  `"↑ Scrolled up {offset} · press End"` (≤31 cols for a 4-digit offset, the max — `SCROLLBACK_ROWS`
  caps offset at 2000).
- `scroll_new_lines_below`: similarly shortened to `"↓ {n} new below · press End"`.

Chinese strings untouched (already ~30 cols, safe per the review).

**Verification**: added `the_way_back_survives_a_narrow_terminal` in `src/ui/mod.rs` — renders the
bottom bar at a 55-column terminal (`help_cols = 55 − 23 = 32`; chosen because at that width the old
54-col string clips exactly inside the word "press" and drops "End" entirely, while the new ≤31-col
string still fits whole — verified this boundary arithmetic in Python before writing the test).
Confirmed it fails against the pre-fix strings: temporarily restored the old long-form English text
(kept the new test), reran — panicked with the rendered bar ending in `···pressEnd` cut down to
`···press` (no "End" anywhere on screen). Restored the shortened strings, test passes, and the full
`ui::` test suite (252 tests, includes all the other scroll-hint content assertions) still passes
unchanged.

## F7 — re-entering a session gave two different results depending on the route

`src/ui/mod.rs`, `enter_session`: added an explicit `Request::Scroll { id, by: ScrollBy::Bottom }`
call at the end of `enter_session`, with a comment recording the controller ruling (entering a
session view always lands at the bottom) and explaining why it must not be achieved by perturbing
`sent_size` (that's exactly the accidental mechanism that produced the F7 bug: `SessionManager::resize`
happens to reset scroll as a side effect, and whether `Resize` even fires depends on whether
`sent_size` still matches — a memoization key, not a decision). Failure is swallowed silently, same
pattern as the existing scroll-request handling in `handle_key`.

`enter_session` is the single choke point for every route that re-enters an *existing* session (F3
direct-switch in `attach.rs`, Enter-from-board in `board.rs`, Enter-from-grid in `grid.rs`) — checked
this by grepping every `View::Attached(id)` assignment in `src/ui/*.rs`; the two call sites that don't
go through `enter_session` (`pick.rs` and the two `Request::Create` branches in `mod.rs`) are all
"just created a brand-new session" paths, which start at offset 0 by construction and don't need the
fix.

**Verification**: added `entering_a_session_always_lands_at_the_bottom_even_without_a_resize` in
`src/ui/mod.rs` — spins up a real daemon on a temp socket, creates a shell session, drives it to
generate 200 lines of scrollback, scrolls up 20 rows over the wire, then calls `enter_session`
directly (bypassing `run()` entirely, so no `Resize` request is ever sent), and polls `Screen` to
confirm `scroll.offset` returns to 0. This deliberately proves the reset isn't resize-dependent.
Confirmed it fails against the pre-fix code: removed the new `Request::Scroll{Bottom}` call from
`enter_session` (kept the test), reran — failed with `offset=20` (never reset, since without a
`Resize` there was nothing to trigger the old accidental behavior). Restored the fix, test passes.

## Verification commands (all clean)

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -- --test-threads=1   # 547 unit tests + all integration test files: all pass
cargo fmt --check                # clean
cargo clippy --all-targets -- -D warnings   # clean
git diff --check                 # no whitespace errors
```

No interactive daemon pass was run (per instructions — would kill live agent sessions). The two new
integration-style unit tests (`the_way_back_survives_a_narrow_terminal`'s rendering path and
`entering_a_session_always_lands_at_the_bottom_even_without_a_resize`'s real daemon) spin up their
own throwaway daemons on temp sockets inside the test process; they don't touch the user's running
daemon or `~/.dct`.
