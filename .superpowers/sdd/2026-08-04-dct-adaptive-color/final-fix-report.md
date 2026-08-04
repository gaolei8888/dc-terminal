# Final fix wave — feat/adaptive-color

## Finding 1 — a late OSC 11 reply becomes injected keystrokes

### 1a. DA1 sentinel (`src/theme.rs`)

- `StdinReader::read_reply` now writes `\x1b]11;?\x07\x1b[c` in a single
  `write_all` (one write on purpose: nothing may be interleaved between the two
  queries or the ordering guarantee that the sentinel relies on is gone).
- New pure function `contains_da1(bytes: &[u8]) -> bool` recognizes a complete
  DA1 reply: `ESC [ ?` or `ESC [ >`, then a params run of digits/`;`, then a
  final `c`. It scans the whole accumulated buffer, and returns `false` while
  the trailing `c` is still missing — that is what makes a reply split across
  several `read`s safe (the loop keeps reading instead of treating half a reply
  as done). A `CSI` that isn't DA1 (e.g. `\x1b[6n`) doesn't abort the scan; it
  advances one byte and keeps looking.
- The read loop's exit condition changed from "buffer contains BEL or ST" to
  "buffer contains a DA1 reply". This is the point of the sentinel: with the BEL
  test, a stray `Ctrl-G` typed before startup cut the read short and left the
  real reply unread in the queue. (That was the separately deferred Minor; it is
  subsumed.)
- `QUERY_TIMEOUT` (150 ms) and all deadline arithmetic are unchanged — it is now
  documented as the outer backstop for a terminal answering neither query. The
  256-byte cap and the `n <= 0` arm are unchanged.
- Long WHY comment on `StdinReader` spelling out the failure chain (slow
  terminal → unread reply → crossterm reads it as `Alt+']'` + individual `Char`s
  → `c` opens Secrets, `d`+`d` deletes an API key) and the two terminal
  behaviours the sentinel leans on (every terminal answers DA1; terminals answer
  in order), plus an explicit "don't simplify this away".

### 1b. `isatty` guard (`src/theme.rs`)

First statement of `read_reply`:

```rust
if unsafe { libc::isatty(libc::STDIN_FILENO) } != 1 {
    return Vec::new();
}
```

Comment covers both reasons: don't let `libc::read` eat up to 256 bytes of a
redirected stdin, and — the bigger one — when stdin isn't a tty crossterm falls
back to `/dev/tty`, which turns the leak from a race into a certainty.
SAFETY note records the precondition: `isatty` reads only an fd number, touches
no memory and no process state, and `STDIN_FILENO` is the constant 0.

### 1c. Modifier guard on destructive / navigating keys (`src/ui.rs`)

New helper next to `is_ctrl_q`:

```rust
fn is_plain_key(key: &KeyEvent) -> bool {
    !key.modifiers.contains(KeyModifiers::ALT) && !key.modifiers.contains(KeyModifiers::META)
}
```

Guard added as a match-arm `if is_plain_key(&key)` to:

| View | Arm | Why it's in the set |
|---|---|---|
| Board | `Char('q')` | quits dct |
| Board | `Char('n')` / `Char('N')` | navigates away (creates a session) |
| Board | `Char('p')` | navigates away |
| Board | `Char('c')` | navigates to Secrets — the first half of the demonstrated chain |
| Board | `Char('u')` | destructive (Undo → daemon request) |
| Board | `Char('s')` | destructive (Stop → daemon request) |
| Board | `Char('d')` | in the leaked alphabet; explicitly named by the review |
| Secrets | `Char('d')` | the only arm in the whole key tree that actually deletes data |

**How I chose the set.** The review asked for `c` and `d` at minimum, plus
anything destructive or navigating. Every `Char` arm in the Board view is one or
the other, so the rule "all Board letter keys require a plain keypress" is
simpler to state and to keep true than a subset would be. I also added the
Secrets `Char('d')` arm, which the review didn't name: it is the actual payload
of the demonstrated chain (Board `c` only opens the view). With the guard, an
`Alt+d` there falls into the existing `_ => {}` arm, which *disarms*
`pending_delete` — strictly safer than being ignored.

Not touched: arrow keys, `Enter`, `Esc` (no leaked-byte exposure and no
destructive effect), the typing/filter paths in `PickProject` / `EnterSecret`
(they accumulate characters; changing their modifier handling is out of scope
and would risk eating legitimate input), and the existing `CONTROL` semantics
anywhere. Deliberately **not** guarding against `CONTROL`: `Ctrl+C` currently
reaches the Board `Char('c')` arm, and changing that is unrelated to this
finding and would alter behaviour users may rely on. `Ctrl+Q` never reaches the
match — `is_ctrl_q` intercepts it earlier.

## Finding 2 — design doc overstated the blast radius

`docs/superpowers/specs/2026-08-04-dct-adaptive-color-design.md` 起因 paragraph:
dropped 底部的操作提示 from the list of invisible text and added a parenthetical
saying the footer bar uses `Style::default()` / `Color::Cyan`, was never one of
the ten `DarkGray` sites, and there is no eleventh site to hunt for.

I also fixed the *same* factual claim where it is mirrored in the `src/theme.rs`
module doc comment (it said 底部提示全部隐形). Same one-line correction, same
reason; flagging it here because the review only named the design doc.

## Finding 3 — call-count assertion

`uses_osc11_reply_when_terminal_answers` now asserts `assert_eq!(dark.calls, 1)`
with a comment saying what it guards (a future refactor querying twice doubles
the startup cost, and the real reader writes a query and waits for a reply on
each call).

## Tests added (6 in `theme.rs`, 1 in `ui.rs`)

| Test | What it pins |
|---|---|
| `recognizes_both_da1_reply_forms` | primary `\x1b[?1;2c`, long `\x1b[?62;1;6;9;15;22c`, secondary `\x1b[>0;95;0c`, empty params `\x1b[?c` |
| `recognizes_da1_arriving_together_with_the_osc11_reply` | both replies in one buffer: sentinel recognized *and* the OSC 11 half still parses |
| `does_not_mistake_an_osc11_only_buffer_for_da1` | `\x1b]11;rgb:cdcd/dddd/dddd\x07` (the exact adversarial payload from the finding), empty, `c` not yet arrived, wrong final byte, `\x1b[6n`, bare `cccc` |
| `recognizes_da1_split_across_reads` | half a DA1 → false; after the second chunk → true (the partial-read case) |
| `detects_theme_from_a_buffer_that_includes_the_da1_sentinel` | end-to-end `detect_with` on the real-world buffer shape (OSC 11 + DA1 concatenated) |
| `da1_only_reply_falls_through_to_colorfgbg` | terminal answers DA1 only → `parse_osc11` returns `None` and detection degrades; DA1's digits are never read as a colour |
| `is_plain_key_rejects_alt_but_passes_normal_keypresses` | ALT and META rejected; `NONE`, `SHIFT`, and `CONTROL` all still pass (i.e. no legitimate keypress was eaten) |

## Test evidence

```
$ export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib theme::
running 28 tests
... (all listed ok, including the 6 new ones)
test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 175 filtered out; finished in 0.00s

$ cargo test --lib is_plain_key
running 1 test
test ui::tests::is_plain_key_rejects_alt_but_passes_normal_keypresses ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 202 filtered out; finished in 0.00s

$ cargo test
test result: ok. 203 passed; 0 failed; ...   (lib)
test result: ok. 2 passed; 0 failed; ...
test result: ok. 1 passed; 0 failed; ...
test result: ok. 1 passed; 0 failed; ...
test result: ok. 1 passed; 0 failed; ...
test result: ok. 2 passed; 0 failed; ...
test result: ok. 5 passed; 0 failed; ...
test result: ok. 3 passed; 0 failed; ...
test result: ok. 2 passed; 0 failed; ...
test result: ok. 1 passed; 0 failed; ...
test result: ok. 1 passed; 0 failed; ...
```

Total 222 passed, 0 failed — baseline 215 plus the 7 new tests.

Note for whoever runs these: the unit tests live in the **lib** target
(`cargo test --lib ...`); `cargo test theme::` / `--bin dct` filters match
nothing, which is easy to misread as "no tests ran because they're broken".

```
$ touch src/theme.rs src/ui.rs && cargo build
   Compiling dct v0.1.0 (/Users/lei/Documents/work/dc/dc-terminal)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.50s
```

Zero warnings (forced recompile of both changed files).

## `ReplyReader` contract / `CannedReader`

**Signature unchanged, contract unchanged as documented**: "returns the bytes
read; empty `Vec` if nothing was read (timeout, not a tty, read failed)". The
real reader now writes an extra query and ends on a different condition, but the
value it hands back is still the same thing — a raw byte buffer that may contain
a reply, garbage, or nothing — so `detect_with` needed no change.

`CannedReader` therefore was **not** changed. It is still more forgiving than
reality (returns the whole reply in one call, ignores the deadline). I addressed
that honestly by adding two `detect_with` cases whose canned buffers are the
shapes the real reader now produces — OSC 11 + DA1 concatenated, and DA1 alone —
rather than pretending the double models chunking. Chunking itself is covered
where it actually matters, in `recognizes_da1_split_across_reads`, against the
pure function.

## Self-review

- **`unsafe` preconditions.** Two `unsafe` blocks in `read_reply`'s path, both
  one call wide. `isatty(0)`: no memory involved, no state change, an invalid fd
  only changes the return value. `read(0, chunk.as_mut_ptr(), chunk.len())`:
  unchanged, pointer and length come from the same live stack array. `poll` is
  unchanged. No new `unsafe`.
- **Can the loop hang past 150 ms?** No. Every iteration recomputes `left` from
  `deadline.checked_sub(start.elapsed())` and returns when it underflows; the
  `poll` timeout is `left` (rounded up to ≥1 ms). The DA1 exit only ever ends the
  loop earlier. Worst case is still one 150 ms wait, and only for a terminal that
  answers neither query.
- **Can it spin hot?** No. Each iteration either blocks in `poll` for the
  remaining budget or returns. The ≥1 ms floor still prevents the
  zero-timeout-busy-poll case. Bytes arriving in a flood are capped at 256.
- **DA1 recognition against a partial reply.** Covered by design (the final `c`
  must be present) and by test. Note the honest residual: if the user is
  hammering keys during startup, the 256-byte cap can still return before DA1
  arrives, which leaves a leak possible. That is why 1c exists — the leaked bytes
  become Alt-modified chars that now do nothing.
- **Did the guard change any legitimate keypress?** No. Plain letters, `Shift`
  (needed for `N`), and `Ctrl` combos all still pass. `Ctrl+Q` is intercepted
  before the match. Guarded-out keys fall through to the pre-existing `_` arm:
  no-op on Board, disarm-and-clear-message in Secrets.

## Concerns

- Nothing in the suite executes `StdinReader::read_reply` — still true after this
  change, and the change lands in exactly that region. The parsing half is now
  unit-tested; the I/O half (the write, the `isatty` guard, the poll/read loop's
  new exit condition) is verified by reading only. A manual run on a real
  terminal would be worth one minute before merge.
- The sentinel assumes DA1 is universally answered and that terminals answer in
  order. Both hold for xterm-family terminals, tmux, screen, and the mainstream
  Mac/Linux emulators. A terminal answering neither degrades exactly as before
  (150 ms, then `COLORFGBG`, then `Unknown`) — the fallback path is unchanged, so
  the downside of a wrong assumption is the old behaviour, not a new failure.
