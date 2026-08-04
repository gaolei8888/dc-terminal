# Task 3 Report: 带超时的 stdin 读取与四级探测链

## What I implemented

In `src/theme.rs`, following the brief verbatim:

- Added `use std::io::{Read, Write};` and `use std::time::{Duration, Instant};` to the top-of-file imports.
- `const QUERY_TIMEOUT: Duration = Duration::from_millis(150);`
- `pub(crate) trait ReplyReader { fn read_reply(&mut self, deadline: Duration) -> Vec<u8>; }`
- `pub(crate) struct StdinReader;` implementing `ReplyReader`: writes the OSC 11 query (`\x1b]11;?\x07`) to stdout, then loop-reads stdin via `poll(2)` bounded by the deadline, stopping on terminator (`\x07` or `\x1b\\`), EOF/error, deadline exhaustion, or a 256-byte cap.
- `fn stdin_is_readable(timeout: Duration) -> bool` wrapping `libc::poll` on `STDIN_FILENO`, rounding sub-millisecond remainders up to 1ms.
- `pub(crate) fn detect_with<R: ReplyReader>(reader: &mut R, dct_theme: Option<&str>, colorfgbg: Option<&str>) -> Theme` implementing the exact four-level chain: `DCT_THEME` override (short-circuits, no query) → OSC 11 reply → `COLORFGBG` → `Theme::Unknown`.
- `pub fn detect() -> Theme` reading real `DCT_THEME`/`COLORFGBG` env vars and wiring in `StdinReader`.

All code and comments are exactly as given in the brief, with one deliberate deviation described below (item 3).

## What I tested and the results

Added the brief's `CannedReader` test double and its 8 tests to the existing `mod tests` block (no second test module created). Also added one additional test of my own for the `parse_osc11` header-verification fix (see judgment below).

Final count: **22 tests in `theme::tests`, all passing** (13 pre-existing + 8 from the brief + 1 I added for the header fix).

Full workspace `cargo test`: all suites green, including `tests/slow_input.rs` and `tests/socket_perms.rs` (unaffected, unrelated to this change).

## TDD Evidence

**RED** — command: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme 2>&1 | head -30`, run after adding only the test block (Step 1), before any implementation:

```
error[E0405]: cannot find trait `ReplyReader` in this scope
   --> src/theme.rs:294:10
    |
294 |     impl ReplyReader for CannedReader {
    |          ^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find type `Duration` in this scope
   --> src/theme.rs:295:45
error[E0425]: cannot find function `detect_with` in this scope
   --> src/theme.rs:306:20
```

Expected and matches the brief's Step 2 prediction: compile error, trait/function not found.

**GREEN** — command: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme`, run after Step 3 implementation:

```
running 21 tests
test theme::tests::luminance_separates_real_terminal_backgrounds ... ok
...
test theme::tests::osc11_wins_over_colorfgbg ... ok

test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 172 filtered out; finished in 0.00s
```

(21 at that point = 13 pre-existing + 8 new; after later adding the header-gap regression test it became 22, still all green — reconfirmed above.)

## `cargo build` warning state

`export PATH="$HOME/.cargo/bin:$PATH" && cargo build 2>&1 | grep -i warning` → empty output, both immediately after Step 3 and again after the `parse_osc11` header fix. Task 2's five `dead_code` warnings (`is_light`, `parse_osc11`, `parse_colorfgbg`, `theme_from_override`, `parse_hex_component`) are gone — `detect_with` now calls all four public-in-crate functions, and `parse_hex_component` is called transitively by `parse_osc11`. No new warnings appeared. Build is clean.

## Judgment on item 3 — the `parse_osc11` header gap

**Fixed.** I judged this a real risk given who fills the buffer.

`StdinReader::read_reply` reads raw stdin in raw-mode with no line discipline, so its buffer can contain:
- leftover bytes from an earlier, unrelated escape-sequence exchange that never got fully drained, or
- literal characters the user typed before the terminal answered (if the terminal is slow, or if focus is momentarily on the app and the user is impatiently mashing keys).

Before the fix, `parse_osc11` located its payload purely via `s.split_once("rgb:")`, with no check that an OSC 11 header (`\x1b]11;`) preceded it. A buffer containing the literal substring `rgb:` anywhere — coincidentally, e.g. as part of typed text, a paste, or debris from another escape sequence — would be accepted and its bytes parsed as if they were the terminal's real background color. Given that a wrong "answer" here silently produces a `Dark`/`Light` misdetection rather than falling through to the safer `COLORFGBG`/`Unknown` levels, this is worth closing off, and the fix is cheap and doesn't touch behavior for well-formed replies.

**The fix:** `parse_osc11` now requires the literal OSC 11 header to appear before `rgb:`:

```rust
let after_header = s.split_once("\x1b]11;")?.1;
let after = after_header.split_once("rgb:")?.1;
```

All 8 pre-existing `parse_osc11` tests already include the `\x1b]11;` prefix in their fixtures, so none needed changes and all still pass. I added one new test, `rejects_coincidental_rgb_without_osc11_header`, asserting that a bare `rgb:...` payload or a `rgb:` substring embedded in unrelated stray text (with no OSC header) is rejected — i.e. exactly the buffer shape a stray keystroke or a leftover fragment could produce.

## Files changed

- `/Users/lei/Documents/work/dc/dc-terminal/src/theme.rs` — all changes for this task (trait, `StdinReader`, `stdin_is_readable`, `detect_with`, `detect`, the header-verification fix in `parse_osc11`, and all new tests).

## Self-review findings

- **Completeness against the brief:** implemented exactly the code given, with the one intentional addition (header check) called out above, plus its accompanying test. No scope creep into `ui.rs`.
- **Every failure path degrades, none panics:** verified by reading the code path by path —
  - `write_all`/`flush` failure in `StdinReader` → empty `Vec` → `parse_osc11(&[])` → `None` → falls to level 3.
  - `poll` returning `<= 0` (timeout or EINTR/error, both treated the same) → `stdin_is_readable` returns `false` → loop returns whatever partial `buf` it has.
  - `std::io::stdin().read()` returning `Ok(0)` (EOF) or `Err(_)` → returns `buf` immediately.
  - Malformed/garbage reply bytes → `parse_osc11` returns `None` at every internal `?` — no `unwrap`/`expect`/`panic!` anywhere in the parse path.
  - `theme_from_override`/`parse_colorfgbg` return `Option`, chained with `if let`/`and_then`, no panicking paths.
  - No `?` operator used anywhere that could propagate an error out of `detect`/`detect_with` (both return `Theme` directly, never `Result`).
- **`unsafe` block preconditions:** `libc::pollfd` is fully initialized (`fd`, `events`, `revents: 0` — no uninitialized fields); `fd: libc::STDIN_FILENO` is always a valid, open file descriptor for the process's stdin; `nfds` argument is `1`, matching the single-element pointer passed (`&mut fd`, i.e. a pointer to one `pollfd`, count 1) — no buffer/count mismatch.
- **Read loop cannot spin or hang:** each iteration either (a) returns due to deadline exhaustion (`checked_sub` returning `None`), (b) blocks inside `poll` for at most `left` (monotonically shrinking as wall time advances, hard-capped by the original 150ms `QUERY_TIMEOUT`), or (c) makes forward progress on a successful read and then re-checks the terminator/cap conditions. There is no branch that both consumes zero time and loops back without reducing the deadline.
- **256-byte cap and terminator check are both reachable:** confirmed via the `uses_osc11_reply_when_terminal_answers`/`parses_*` tests (terminator path, well within 256 bytes) and by inspection of the cap branch, which is guarded independently after the terminator check in the same `Ok(n)` arm — a sufficiently long non-terminated garbage stream would hit it. (Not exercised by a dedicated unit test since `CannedReader` returns its whole canned reply in one `read_reply` call rather than looping through `StdinReader`'s internal `read()` loop; the cap logic lives entirely inside `StdinReader`, which by design isn't exercised against real stdin in unit tests. This mirrors the brief's own scope — `StdinReader` is deliberately excluded from unit-testability, it's the real-I/O side the trait exists to isolate.)

## Concerns

None blocking. The one non-brief change (header verification in `parse_osc11`) was explicitly flagged as an open judgment call in the task instructions, so I made a call and documented the reasoning above rather than silently doing nothing or silently changing more than necessary.

Note: `git status` at the start of this task showed `.superpowers/sdd/.gitignore` as already modified (unrelated to this task, pre-existing in the working tree). I left it untouched and only staged/committed `src/theme.rs`, per the brief's Step 5 instruction (`git add src/theme.rs`).

---

## Fix report: review Important #1 — poll(2)/std::io::stdin() layering mismatch

**Finding (verbatim from review):** `StdinReader::read_reply` polled raw fd 0 via `libc::poll` but read via `std::io::stdin()`, which sits on the standard library's global `BufReader`. A single syscall behind that `BufReader` can pull more than the 64-byte destination chunk off the tty; anything beyond the chunk gets stranded inside the `BufReader`, invisible both to the next `poll` call (kernel queue looks empty → spurious "not readable" → false-negative degrade on a code path with no unit test) and to crossterm's event source (which reads fd 0 directly, bypassing `std::io::stdin` entirely — so those stranded bytes would never reach the TUI's event loop once it starts, i.e. swallowed keystrokes by a different route than the reader-thread design already guarded against).

**What I changed:**

In `src/theme.rs`, `StdinReader::read_reply`'s read step now calls the raw fd primitive directly instead of going through the buffered `std::io::stdin()`:

```rust
let n = unsafe {
    libc::read(libc::STDIN_FILENO, chunk.as_mut_ptr().cast(), chunk.len())
};
match n {
    i if i <= 0 => return buf,
    n => {
        let n = n as usize;
        buf.extend_from_slice(&chunk[..n]);
        // ... (terminator check, 256-byte cap — unchanged)
    }
}
```

replacing the previous:

```rust
match std::io::stdin().read(&mut chunk) {
    Ok(0) | Err(_) => return buf,
    Ok(n) => { buf.extend_from_slice(&chunk[..n]); ... }
}
```

`libc::read` returns `isize`; `n <= 0` covers both EOF (`0`) and error (`-1`, e.g. `EINTR`), which is the same fold the old `Ok(0) | Err(_)` arm did — no new failure-handling behavior, only the layer the bytes travel through.

Added a Chinese WHY comment directly above the call explaining the buffering hazard (why `std::io::stdin()` is deliberately not used) and connecting it back to the reasoning already given for `poll` over a reader thread, so a future reader doesn't "simplify" this back to `std::io::stdin().read(...)`.

**`unsafe` preconditions for the new block**, matching the standard already met by the existing `libc::poll` call:
- `libc::STDIN_FILENO` is a valid, open fd for the process's stdin — same fd already trusted by the adjacent `poll` call.
- `chunk.as_mut_ptr()` is a pointer into a live, stack-allocated `[u8; 64]` that outlives the call (`chunk` isn't dropped or moved before `read` returns).
- `chunk.len()` (64) is passed as the length, matching the buffer's actual capacity — `read` cannot write past what `chunk` owns.

**Import cleanup:** removed `Read` from `use std::io::{Read, Write};` (now `use std::io::Write;`) since nothing in the file calls the `Read` trait's methods anymore.

**Covering tests re-run** (per reviewer instruction — `theme::tests` module):

Command:
```
export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme 2>&1 | tail -35
```

Output:
```
   Compiling dct v0.1.0 (/Users/lei/Documents/work/dc/dc-terminal)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.84s
     Running unittests src/lib.rs (target/debug/deps/dct-b4ebaea020bee378)

running 22 tests
test theme::tests::luminance_weights_are_not_transposed ... ok
test theme::tests::each_theme_has_a_distinct_dim_style ... ok
test theme::tests::parses_theme_override_leniently ... ok
test theme::tests::ignores_invalid_theme_override ... ok
test theme::tests::dark_gets_a_lighter_gray_than_light ... ok
test theme::tests::luminance_separates_real_terminal_backgrounds ... ok
test theme::tests::override_wins_over_colorfgbg ... ok
test theme::tests::rejects_coincidental_rgb_without_osc11_header ... ok
test theme::tests::override_wins_and_skips_the_query ... ok
test theme::tests::osc11_wins_over_colorfgbg ... ok
test theme::tests::rejects_malformed_osc11_replies ... ok
test theme::tests::unknown_never_pins_a_foreground_color ... ok
test theme::tests::garbage_at_every_level_lands_on_unknown ... ok
test theme::tests::falls_back_to_colorfgbg_when_reply_is_garbage ... ok
test theme::tests::parses_four_digit_osc11_reply ... ok
test theme::tests::scales_short_hex_components_to_full_range ... ok
test theme::tests::falls_back_to_colorfgbg_when_terminal_is_silent ... ok
test theme::tests::parses_colorfgbg ... ok
test theme::tests::parses_st_terminated_reply ... ok
test theme::tests::unknown_when_nothing_answers ... ok
test theme::tests::uses_osc11_reply_when_terminal_answers ... ok
test theme::tests::rejects_malformed_colorfgbg ... ok

test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 172 filtered out; finished in 0.00s
```

All 22 pass, same count as before the fix (the changed code path — `StdinReader::read_reply` itself — is exercised only through real stdin in `detect()`, not the `CannedReader`-based unit tests, consistent with the brief's own design: `StdinReader` is the real-I/O implementation the `ReplyReader` trait exists to keep out of unit tests. No test regressed or was newly required by this change since it's a pure implementation-detail swap with identical externally observable `Vec<u8>` semantics).

**Build warning state — re-verified zero-warning:**

Command:
```
export PATH="$HOME/.cargo/bin:$PATH" && cargo build 2>&1 | grep -i warning; echo "--- warnings above (none expected)"
```

Output:
```
--- warnings above (none expected)
```

Confirms the `Read` import removal didn't leave anything else depending on it, and no new warning was introduced by the `unsafe` block or the `match n { i if i <= 0 => ..., n => ... }` arm.

Also re-ran the full workspace suite (`cargo test`) to check for unrelated regressions — all suites (including `tests/slow_input.rs`, `tests/socket_perms.rs`, signal-handling tests) still pass.

**Deferred minors — confirmed untouched:** the BEL-anywhere terminator scan, `from_utf8` strictness, the 上取整 comment wording, uninspected `revents`, and the missing "reader called exactly once" assertion were left exactly as they were; only the `libc::read`/import change and its comment were made.

**Files changed in this fix round:** `/Users/lei/Documents/work/dc/dc-terminal/src/theme.rs` (commit `b2212ac`).
