# Task 10 report: 会话视图接上滚动

## Status

DONE_WITH_CONCERNS (concerns are about what I could not verify without a live daemon/terminal — see below; all automated gates are green).

## What changed, and where

### `src/i18n.rs`
Added three functions to `pub mod msg` (placed right after `session_title_disconnected`, next to the other attach-view sentences):

- `scroll_new_lines_below(lang: Lang, n: usize) -> String`
  - en: `"↓ {n} new line(s) below"`
  - zh: `"↓ 下面还有 {n} 行新内容"`
- `scrolled_up(lang: Lang, offset: usize) -> String`
  - en: `"↑ Scrolled up {offset} line(s) · press End to jump back down"`
  - zh: `"↑ 已往上翻 {offset} 行 · 按 End 回到底部"`
- `agent_owns_the_screen(lang: Lang) -> String`
  - en: `"This assistant controls its own screen here, so there's nothing to look back at"`
  - zh: `"这个 agent 自己管画面，翻不了历史"`

No jargon in either language ("scrollback"/"alt screen"/"buffer"/"备用屏" all absent) — enforced by a test (`an_alt_screen_agent_that_ignores_the_mouse_gets_an_explanation`).

### `src/ui/app.rs`
- Added `pub scroll: ScrollState` — refreshed every frame from `Response::Screen`'s new `scroll` field.
- Added `pub screen_origin: Option<(u16, u16)>` — the content area's top-left terminal coordinate, filled by `attach::draw`, consumed by `attach::handle_mouse`. `None` until the first frame is drawn (or when not in a session).
- Both initialized in `new_inner` (`ScrollState::default()`, `None`), so `App::new` and `App::test_app()` stay identical per the existing convention documented on `new_inner`.

### `src/ui/attach.rs`
- `ScrollAction` enum, `wheel_action`, `key_scroll`, `scroll_hint` — pure functions, as specified in the brief, with `scroll_hint` taking an explicit `Lang` parameter (override #1).
- `handle_key`: added a branch between F3 and the general `key_to_input` fallback that calls `key_scroll(&app.scroll, &key, app.screen.len() as u16)`. **Deviation from the brief**: the brief's snippet references a `content_rows` value without showing where it comes from. `handle_key`'s signature (`app: &mut App, key: KeyEvent`) has no terminal-size parameter, so I used `app.screen.len()` — `PtySession::screen_spans()` (src/pty.rs:211) always returns exactly `rows` lines (the vt100 parser's configured height), and that height is exactly what was last negotiated via `Request::Resize`. This avoids threading terminal size into `handle_key`'s signature and is documented inline.
- `draw`: now records `app.screen_origin = Some((area.x + 1, area.y + 1))` right after the bordered block is rendered — the `+1` is the border width, matching the existing cursor-mapping code two lines below it (the brief's requirement in override #3).
- `handle_mouse` + `button_code`: implemented per the brief, with one correctness fix (see "What didn't match reality" below).

### `src/ui/mod.rs`
- Imports: added `EnableMouseCapture`, `DisableMouseCapture`.
- `restore_terminal()`: unconditionally emits `DisableMouseCapture` now, alongside the existing `DisableBracketedPaste`/`LeaveAlternateScreen`. Reached by `TerminalGuard::drop` and `spawn_signal_restore`, so it covers normal quit, `?` early return, panic, and SIGTERM.
- New pure function `mouse_capture_transition(was_attached: bool, is_attached: bool) -> Option<bool>` — decides whether to flip capture on/off this frame.
- **Design deviation from the brief**: the brief said to open capture "on entering `View::Attached`" and close it "on leaving" — implying per-transition call sites. I instead added one check per loop iteration, right before `term.draw`, comparing `matches!(app.view, View::Attached(_))` against a `mouse_captured` flag carried across iterations. Reason: there are at least four distinct code paths that can set `app.view = View::Attached(id)` (`enter_session`, the `verify_rx` secret-verification success path, and two others found via grep), and enumerating them all is exactly the kind of "miss one, get a silent leak" bug the rest of this codebase's comments warn about repeatedly. A single per-frame check is provably exhaustive and is itself unit-tested (`mouse_capture_toggles_only_on_a_real_transition`).
- `Event::Mouse` is intercepted right after `event::read()`, before the `Event::Paste` check, with `continue` — per the brief, with the required comment about why this `continue` doesn't violate the no-`continue`-in-key-handling rule, and why `handle_mouse` must never touch `app.message`.
- `Response::Screen` handling: now binds `scroll` (previously discarded via `..`) into `app.scroll`.
- `draw()`'s bottom-bar composition: when `app.message` is empty (the branch where `idle_help` was unconditionally shown), it now first asks `attach::scroll_hint(&app.scroll, app.lang)` for `View::Attached`, and only falls back to `idle_help` if that returns `None`. `message` still wins overall because this whole branch is only reached when `message.text.is_empty()`.

## i18n keys added (both languages)

See the three `msg::` functions above — Chinese and English text are both shown there.

## Test command and results

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

- `cargo test --lib`: **542 passed, 0 failed** (baseline was 520; +22 new tests: 19 in `ui::attach`, 3 in `ui::mod`).
- Integration test binaries (`daemon_*`, `screen_state`, `signal_restore`, `slow_input`, `socket_perms`, `zombie_reaping`, `grid_reply`, `profiles_flow`, `projects_flow`, `list_is_not_blocked_by_slow_create`, etc.): all green, unchanged from baseline (none of them touch scroll/mouse — this task didn't add integration coverage there since it's UI-layer wiring, not protocol/daemon logic, which was already covered by earlier tasks in this plan).
- `cargo fmt --check`: clean (exit 0).
- `cargo clippy --all-targets -- -D warnings`: clean, zero warnings.
- `git diff --check`: no whitespace errors.

### Mutation-tested the routing logic, as required
Before finalizing, I deliberately broke `wheel_action` (forced it to always return `Forward`) and reran `cargo test --lib ui::attach` — `otherwise_dct_scrolls_three_rows_per_notch` and `there_is_nothing_to_scroll_when_there_is_no_history` both failed as expected, then restored and reverified green. Separately broke `key_scroll`'s step formula (dropped the `saturating_sub(2).max(1)`) — `page_keys_scroll_a_screen_minus_two` and `a_tiny_window_still_scrolls_at_least_one_row` both failed as expected, then restored and reverified green. This confirms the brief's own tests (kept close to verbatim, only `scroll_hint` tests gained a `Lang` parameter) have real teeth, not just line coverage.

I also added two `handle_key`-level tests (`a_page_key_is_consumed_silently_when_dct_owns_scrolling`, `a_page_key_falls_through_to_the_agent_when_it_owns_the_viewport`) that exercise the *wiring*, not just the pure function: they use the fact that the disconnected test `App`'s `Input` path sets `app.message` on failure but the scroll path deliberately doesn't, so a wrong implementation that either never wires `key_scroll` in, or wires it in unconditionally regardless of `agent_owns`, fails one of these two tests.

## What did not match the brief, and how I resolved it

1. **`handle_mouse`'s final forward used `app.client()?.call(...)` in the brief's snippet.** Using `?` on `app.client()` would propagate a "daemon unreachable" error out of `handle_mouse`, through `attach::handle_mouse(&mut app, m)?` in `run()`'s main loop, and crash the entire TUI on a single failed mouse event while disconnected — a real correctness bug (this is one of the three defects the task instructions warned the brief ships). I changed every `app.client()` call in `handle_mouse` (and the new branch in `handle_key`) to the existing codebase idiom of `app.client().and_then(|c| c.call(...))` wrapped in `let _ =`, matching how `attach::handle_key`'s existing `Input` branch is *structured* (though that one does report the error — intentionally, since scroll/mouse failures should be silent per the "never touch `app.message`" rule, while `Input` failures are reported because the user is actively typing and needs to know).
2. **Mouse-capture enable/disable placement**: the brief suggested per-entry/exit-point calls; I used one per-frame check instead, for the exhaustiveness reason described above. This is a design choice, not a bug fix, but I'm flagging it as a deviation since it changes *where* the capture toggling code lives relative to what the brief described.
3. **`content_rows` for `key_scroll`**: the brief's Step 4 snippet references `content_rows` without showing its derivation. I derived it from `app.screen.len()` as described above.
4. Everything else (step sizes: wheel=3, page=screen−2 clamped to ≥1, End via `Scroll(-i32::MAX)`; the "who owns the wheel" judged by `agent_owns` not `alt_screen`; the no-jargon requirement; the message-wins-over-hint priority; `screen_origin` computed only in `draw`) matches the brief's stated intent and was kept as close to verbatim as the overrides allowed.

## What I could NOT verify (Step 6 skipped per instructions)

I did not run `dct restart` or drive a live UI, per the explicit instruction not to risk the user's running agent sessions. Concretely, unverified:

- That a real Claude Code session (alt-screen + full mouse reporting) actually receives forwarded wheel/click events and the dct-side buffer stays untouched, with no scroll hint shown.
- That a real codex session (inline, no mouse) actually shows history when the wheel is scrolled, that the screen freezes while scrolled up and the "↓ N new lines below" hint appears as it produces new output, and that pressing any character key snaps back to the bottom **and** the character reaches codex's input.
- `PageUp`/`PageDown`/`End` behavior against a live PTY (the pure functions and the `handle_key` wiring are tested, but not the full round trip through the daemon and vt100 parser).
- That leaving a session actually restores native terminal text selection (mouse capture truly off) and that `Ctrl+C` mid-session followed by clicks in the terminal doesn't produce garbage (mouse capture truly off on that exit path too). The `signal_restore` integration test (`sigterm_restores_the_terminal`, `sighup_restores_the_terminal`) does verify raw-mode restoration end-to-end but does not (and did not, before this task) assert on mouse-capture escape sequences specifically.

A human needs to do the interactive pass described in the brief's Step 6.

## Concerns

- The mouse-capture-restoration correctness rests on `restore_terminal()` being reachable on every exit path, which was already true before this task (the doc comments on `TerminalGuard`/`spawn_signal_restore` establish that) — I only added one more `execute!` call inside a function that was already proven to run everywhere. I did not add new exit paths, so I believe the existing guarantee extends cleanly, but this is inference from reading the code, not an observed live test.
- `app.screen.len() as u16` as a stand-in for "current screen height" is correct today because `screen_spans()` always returns exactly `rows` lines regardless of content — if that invariant ever changes (e.g., someone makes `screen_spans()` trim trailing blank lines), `key_scroll`'s page-size math would silently degrade (still `.max(1)`-clamped, so it can't panic or go negative, but the "screen minus two" step would become wrong). Worth a comment cross-reference if `pty.rs` changes.

## Files touched

- `/Users/lei/work/dc/dc-terminal/src/i18n.rs`
- `/Users/lei/work/dc/dc-terminal/src/ui/app.rs`
- `/Users/lei/work/dc/dc-terminal/src/ui/attach.rs`
- `/Users/lei/work/dc/dc-terminal/src/ui/mod.rs`

---

# Fix report: review findings (round 2)

Addresses the coordinator's review of the original commit: 3 Important findings, 5 Minors. All eight are fixed. Files touched this round: `src/ui/mod.rs`, `src/ui/attach.rs`, `src/i18n.rs`, `tests/signal_restore.rs`.

## IMPORTANT 1 — motion events forced a refetch+redraw per event

**Approach taken: drain pending mouse events before continuing the outer loop** (the brief's second suggested option), not "skip the refetch/redraw for that iteration" as a separate flag.

Why: `handle_mouse` (`src/ui/attach.rs`) now returns `bool` — whether it actually sent a request (MINOR 8 folded this in naturally). In `run()` (`src/ui/mod.rs`), the event-read site became:

```rust
let mut ev = event::read()?;
while let Event::Mouse(m) = ev {
    let acted = attach::handle_mouse(&mut app, m);
    if acted {
        continue 'main;   // state may have changed — do the full refetch+redraw cycle
    }
    if !event::poll(Duration::from_millis(0))? {
        continue 'main;   // nothing else queued — end this iteration normally
    }
    ev = event::read()?;  // more already buffered — handle it right here, no full cycle
}
// ev is guaranteed non-Mouse past this point; existing Paste/Key handling follows unchanged
```

The outer `loop` was given a label (`'main:`) so this nested `while` can jump back to the top of the outer loop specifically (not the `while` itself) when a request was actually sent. All pre-existing unlabeled `continue;` statements elsewhere in the function are untouched — they were never inside this new `while`, so their target (the outer loop) is unaffected by adding the label.

I chose draining over a "skip this iteration" flag because:
- It requires no new field or flag threaded through the rest of the loop body — the fix is fully contained in the event-read site.
- It collapses an entire burst of motion events (e.g., sweeping the pointer across an 80-column window) into at most one refetch+redraw at the end of the burst, rather than one per individual event — strictly better than "skip this one iteration" for a fast sweep, which would still refetch+redraw once per iteration, just doing nothing productive when it does.
- When nothing is queued, it falls through to `continue 'main`, so a session with no pending events behaves exactly as before (same natural 16ms/150ms tick cadence) — no correctness change on the steady-state path, and I could point at the diff and show the untouched code was moved, not altered.

`event::poll(Duration::from_millis(0))` is a non-blocking check (matches the existing `event::poll(Duration::from_millis(tick))` pattern already used for the main tick, just with a zero timeout), so this never introduces a wait.

The no-`app.message` invariant is untouched — `handle_mouse` still never writes it, and the only new code around it is control flow.

Not unit tested: this loop lives inside `run()`, which drives real terminal I/O via crossterm's blocking `event::read()`/`event::poll()`, and no existing test in this codebase exercises `run()` directly (it isn't testable without a real or synthetic event source, which is a bigger lift than this fix warrants). `wheel_action`/`key_scroll`/`handle_mouse`'s `bool` return are unit tested as before; the draining control flow itself is covered by inspection and by the fact that it doesn't touch anything the existing 544 tests already assert on.

## IMPORTANT 2 — no coverage for the mouse-capture-leak exit path

Added assertions to both `tests/signal_restore.rs` tests. First verified what crossterm 0.28.1 actually emits by reading `DisableMouseCapture::write_ansi` (`src/event.rs`) and the `csi!` macro (`src/macros.rs`): `csi!("?1006l")` expands to `"\x1B[?1006l"`, and the five disable sequences are written in one call, in the reverse order of `EnableMouseCapture`:

```
\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l
```

That's `DISABLE_MOUSE_CAPTURE_SEQ` in the test file now, with a comment citing exactly where it came from (not "the reviewer's literals" trusted blind — I re-derived them from source and they matched).

`spawn_dct_in_pty`'s drain thread previously read-and-discarded the master fd's output (needed so the child doesn't block on a full pty buffer). It now accumulates everything it reads into a shared `Arc<Mutex<Vec<u8>>>` and returns that handle alongside the child and master fd. Both `sigterm_restores_the_terminal` and `sighup_restores_the_terminal` (the reviewer asked for SIGTERM specifically; I added the identical assertion to SIGHUP too since it's the same `restore_terminal()` code path and the marginal cost was one function call) now assert, after confirming raw mode is off:

```rust
assert!(
    wait_until_captured_contains(&captured, DISABLE_MOUSE_CAPTURE_SEQ, 5),
    "... 之后没看到 DisableMouseCapture 序列——用户以后点哪儿终端都会冒乱码"
);
```

`wait_until_captured_contains` polls (same `wait_for` helper already used throughout the file) rather than checking once, because the drain thread reading the final bytes and the main thread observing `child.try_wait() == Some(_)` are different threads with no ordering guarantee between them — a bare one-shot check would be flaky.

**Verified this actually catches the regression it's meant to catch**: temporarily removed `DisableMouseCapture` from the `execute!` call in `restore_terminal()` (`src/ui/mod.rs`), rebuilt, and reran `cargo test --test signal_restore -- --test-threads=1` — both tests failed with the expected message. Reverted, reran, both green again.

## IMPORTANT 3 — `screen_origin` off-by-one was invisible

Added `draw_records_the_bordered_content_corner_as_the_screen_origin` to `src/ui/attach.rs`'s test module. It calls `attach::draw` directly (not through the full `ui::draw` layout) with a deliberately non-`(0, 0)` `Rect { x: 3, y: 2, width: 40, height: 12 }`, and asserts `app.screen_origin == Some((area.x + 1, area.y + 1))`.

**Verified against the reviewer's exact mutation**: changed `app.screen_origin = Some((area.x + 1, area.y + 1))` to `Some((area.x, area.y))`, reran the new test alone — it failed (`left: Some((3, 2)), right: Some((4, 3))`), confirming it catches the dropped-border-offset bug that the full 542-test suite missed. Reverted, reran, green.

Chose a non-`(0, 0)` origin specifically so the test can't pass by coincidence (a mutation that hardcodes `(0, 0)` or forgets the offset would still slip through an origin-at-the-corner test).

## MINOR 4 — misplaced rationale

Moved the `End` → `Scroll(-i32::MAX)` / daemon-side-clamping paragraph from `scroll_hint`'s doc comment to `key_scroll`'s, where the encoding decision it explains actually lives. `scroll_hint`'s doc comment is now just the one line describing what it does.

## MINOR 5 — "new lines below" hint now includes the End instruction (controller ruling applied)

`i18n::msg::scroll_new_lines_below` (`src/i18n.rs`) now reads:
- zh: `"↓ 下面还有 {n} 行新内容 · 按 End 回到底部"`
- en: `"↓ {n} new line(s) below · press End to jump back down"`

Updated `the_hint_says_how_much_is_waiting_below` (`src/ui/attach.rs`) to assert both languages contain `"End"` in addition to the line count.

## MINOR 6 — `key_scroll` now agrees with `wheel_action` on empty history (controller ruling applied)

`key_scroll` gained a guard clause: `KeyCode::PageUp | KeyCode::PageDown if st.max == 0 => None`, placed before the unconditional `PageUp`/`PageDown` arms, with a comment cross-referencing `wheel_action`'s identical `max == 0` check. Added `page_keys_are_not_scroll_keys_when_there_is_no_history`, asserting both `PageUp` and `PageDown` return `None` when `max == 0`.

## MINOR 7 — weak digit-soup assertion replaced

`a_scroll_hint_takes_over_the_bottom_bar_when_there_is_history` (`src/ui/mod.rs`) now asserts the actual whitespace-stripped rendered sentence, `c.contains("已往上翻40行·按End回到底部")`, instead of `c.contains('4') && c.contains('0')`. `App::test_app()` defaults to `Lang::Zh`, confirmed by reading `App::new_disconnected`.

## MINOR 8 — `handle_mouse` signature closed structurally

`handle_mouse` is now `pub(crate) fn handle_mouse(app: &mut App, m: MouseEvent) -> bool` (was `-> Result<()>`). Every internal `return Ok(())` became `return false`, and the two paths that send a request return `true`. The call site in `run()` no longer uses `?` — this is also what made IMPORTANT 1's fix straightforward, since the caller now gets the "did it act" signal it needs from the type itself, not a side channel.

Updated all three existing `handle_mouse` tests to drop `.unwrap()` and, where it added a real assertion, check the returned `bool`. `handle_mouse_never_touches_the_message_even_when_the_call_fails` was also strengthened: it previously used `app.scroll` at its default value, under which every event in the loop short-circuited before attempting any network call (so despite its docstring, it never actually exercised "the call fails" for click events). It now sets `app.scroll.agent_owns = true`, so `ScrollUp`/`ScrollDown`/`Down`/`Up` all genuinely reach the failing `app.client()` call (and are asserted to return `true`), while `Moved` still short-circuits (asserted `false`) — the test's docstring now matches what it actually does.

## Test commands and results (this round)

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --lib ui:: -- --test-threads=1
cargo test --test signal_restore -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

- `cargo test --lib ui:: -- --test-threads=1`: **249 passed, 0 failed**.
- `cargo test --lib -- --test-threads=1` (full lib, for completeness): **544 passed, 0 failed** (was 542 before this round; +2 new tests: `page_keys_are_not_scroll_keys_when_there_is_no_history`, `draw_records_the_bordered_content_corner_as_the_screen_origin`).
- `cargo test --test signal_restore -- --test-threads=1`: **2 passed, 0 failed**, both now with the `DisableMouseCapture`-sequence assertion.
- `cargo test` (full suite, all binaries + integration tests): all green, same as before this round.
- `cargo fmt --check`: clean (exit 0).
- `cargo clippy --all-targets -- -D warnings`: clean, zero warnings.
- `git diff --check`: no whitespace errors.

Mutation checks performed this round (each: mutate → confirm red → revert → confirm green):
1. Dropped `DisableMouseCapture` from `restore_terminal()`'s `execute!` call → both `signal_restore` tests failed with the expected message.
2. Dropped the `+1` border offset in `attach::draw`'s `app.screen_origin` assignment → `draw_records_the_bordered_content_corner_as_the_screen_origin` failed with the expected mismatch.

## What I still could not verify

Unchanged from the original report: no interactive/live-daemon pass was run, per the standing instruction not to risk the user's real sessions. The event-draining control flow in `run()` (IMPORTANT 1) in particular has no automated coverage of its own — see the note in that section above.

(Two unrelated pre-existing local changes — `.superpowers/sdd/.gitignore` and an untracked docs file — were present in the working tree before this task started and were intentionally excluded from the commit.)
