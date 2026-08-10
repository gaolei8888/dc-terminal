# Fix 1 report — control bytes in session names

Owner: fix-1 (of 4 blocking items from the final review). Scope: only Finding 1
(`src/session.rs`, the unsanitized-keystroke-bytes-become-the-name defect). The
other three findings are separate tasks and were not touched.

## What changed and why

The bug chain (see `fix-1-brief.md` for the full argument, verified against
`ratatui-0.28.1` source): the attach view forwards every keystroke verbatim
(`ui/mod.rs::key_to_input`) — arrows are `\x1b[A`..`\x1b[D`, Backspace is
`\x7f`, Esc is `\x1b`, Ctrl+letter is `\x01`..`\x1a`. `collect_first_input`
recorded these raw bytes into `first_input`, `request_name` copied them
straight into `name_slot` with no filtering, and `name_slot`'s content
(`SessionInfo.tag`) is rendered via `Line` → `Span::render_ref`, a path that —
unlike `Buffer::set_stringn`/`Paragraph` — does not drop control characters.
A backspace-laden or escape-prefixed name would therefore corrupt every
subsequent frame of the board/grid/attach title.

Fixes, all in `src/session.rs`:

1. **`is_backspace` / `apply_keystrokes`** (new, near `clean_name`): a shared
   character-processing core. `\x7f`/`\x08` pop the last character already
   accumulated (per the "recorded text should match what the user meant to
   type" decision — `"fix teh\x7f\x7f\x7fthe"` now records as `"fix the"`,
   not a literal byte replay). A full CSI escape sequence (`ESC '[' `
   parameter-bytes* terminator-byte, terminator = ASCII `0x40..=0x7E`) is
   consumed and dropped as one unit — dropping only the leading `ESC` and
   leaving `[A` behind was an early bug I caught with a mutation test (see
   below): `[` and the letter that follows it are *not* control characters
   individually, so a naive `char::is_control()` filter lets them through.
   Bare `Esc` (not followed by `[`) is dropped with no lookahead. Any other
   control character (Ctrl+letter) is dropped outright.
2. **`append_capped`** now delegates to `apply_keystrokes` with a cap, so
   `first_input` — the buffer collected incrementally, one `send_input` call
   per keystroke in the attach view — is clean at the point of collection.
   This also fixes the secondary problem the brief flagged: `first_input` is
   fed verbatim into `name_prompt`, so the model itself used to be handed a
   string full of `\x7f`.
3. **`sanitize`** (new, `pub(crate)`, one-shot version of the same filter):
   applied to the model's answer before it can reach `name_slot`, since
   `clean_name` only strips quotes/punctuation and was never meant to strip
   control bytes.
4. **`fallback_name` / `model_name`** (new, pure functions): the two
   decision points from the brief — "cap → sanitize → trim → treat empty as
   `None`" — extracted out of `request_name` into free functions, for the
   same reason `collect_first_input` was already a free function (see its
   doc comment): it's logic worth testing directly, decoupled from the
   locks/threads/PTY machinery in `request_name`. This extraction was not
   optional cosmetics — see mutation M3 below, it's the only way the
   fallback-side `sanitize` call turned out to be provably necessary.
   `request_name` now just calls `*recover(s.name_slot.lock()) =
   fallback_name(&s.first_input);` and, in the model thread, `if let
   Some(name) = model_name(&text) { ... }`.
5. Empty-after-sanitize handling: if `fallback_name`/`model_name` return
   `None`, `name_slot` is left/set to `None`, not `Some(String::new())`.
   `name_slot.is_none()` doubles as the "have we tried yet" gate in `tick()`,
   so a whitespace-only first message no longer permanently pins an
   invisible empty tag — the next Working→Idle transition gets another shot
   at naming the session.

## Mutation testing

Per the brief's instruction, "all green" was not the bar — every mutation
below was hand-introduced, the suite re-run, and the result recorded. Two of
them **survived** on the first pass; both led to either a stronger test or
(in one case) a refactor that made the surviving code path testable at all.
The exact code was restored from a saved copy (`diff` verified identical)
after every mutation.

| # | Mutation | Result | Test(s) that caught it |
|---|---|---|---|
| M1 | `apply_keystrokes`: negate `!ch.is_control()` → `ch.is_control()` | **Caught** | 21 tests failed (nearly everything touching `first_input`/`sanitize`) |
| M2 | `request_name`'s fallback write: drop the `is_empty()` → `None` branch, always `Some(fallback)` | **Survived first**, then caught | See below — required strengthening the whitespace test |
| M3 | `fallback_name`: remove the `sanitize(&capped)` call | **Survived first**, then caught | See below — required extracting `fallback_name` as a directly-testable function |
| M4 | `model_name`: remove the `sanitize(&clean_name(raw))` call (use `clean_name` alone) | Caught | `model_name_is_none_when_nothing_survives_the_wash`, `model_name_strips_control_bytes_after_clean_name`, `the_model_named_path_is_sanitized_too` |
| M5 | `fallback_name`: invert the emptiness check (`!cleaned.is_empty() → None`) | Caught | 6 tests failed, incl. `fallback_name_is_none_for_whitespace_only_input`, `whitespace_only_input_leaves_the_name_slot_open_for_a_later_real_attempt` |
| M6 | CSI terminator range narrowed to `0x40..=0x3f` (always empty ⇒ the loop never finds a terminator and silently eats the rest of the string) | Caught | `fallback_name_strips_control_bytes_even_if_first_input_somehow_carries_them` (its input has real text *after* the escape sequence, which is what exposes over-consumption) |
| M7 | Negate the CSI-introducer check (`chars.peek() == Some(&'[')` → `!=`) | Caught | 6 tests failed, incl. `sanitize_strips_escape_sequences_and_ctrl_codes`, `collect_first_input_drops_escape_sequences_and_control_codes` |
| M8 | Negate `is_backspace` (`==` → `!=` / `&&` instead of `||`) | Caught | 24 tests failed |

### M2 in detail — why it survived, and the fix

The first version of the "whitespace-only input" test only asserted that
`SessionInfo.tag` stayed `""`. That assertion is blind to the bug: `list()`
does `name_slot.clone().unwrap_or_default()`, so `None` and `Some("")` both
render as `""` — they're indistinguishable from the outside. The mutation
(always writing `Some(fallback)`, even when empty) pins `name_slot` to
`Some("")` forever, which the old test could not see.

The fix was to test the thing that actually depends on `None` vs `Some("")`:
whether the naming gate reopens. I rewrote
`whitespace_only_input_leaves_the_name_slot_open_for_a_later_real_attempt` to
force a *second* Working→Idle transition (using `fake_agent()` / `cat`, whose
screen content I control by writing `"READY"` to it directly, since
`finishing_agent()` only prints `READY` once and then sleeps 30s — it can't
be used to force a second transition). After the whitespace round, I swap in
a `FixedBackend` that returns a real name and drive a second round; the test
only passes if that second round can actually produce a name, which requires
`name_slot` to have been left `None`.

### M3 in detail — why it survived, and the fix

`s.first_input` is written *only* through `collect_first_input` →
`append_capped`, which (after this fix) already strips control bytes and
escape sequences at collection time. So by the time `request_name` reads
`s.first_input`, it is provably already clean — no test driven through the
public `SessionManager` API can make the `sanitize(&fallback)` call in
`request_name` matter, because nothing can get an unsanitized byte into
`first_input` in the first place under the current code.

Rather than accept an untestable "defense in depth" line (which the brief's
own process explicitly disallows — "no test failure = the test wasn't written
well enough, go add more"), I extracted the fallback-computation logic into
`fallback_name(first_input: &str) -> Option<String>`, a pure function with no
`Session`/PTY dependency. That makes it possible to test the sanitize step in
isolation, by handing it a string that *deliberately* bypasses
`append_capped` — `fallback_name("fix\x1b[A the bug")` must equal
`Some("fix the bug".to_string())`. This also matches the file's existing
convention (`collect_first_input`'s own doc comment gives the identical
rationale for being a free function). The same extraction was applied
symmetrically to the model-name path as `model_name`.

## Test commands and output

TDD: the new tests were written first and confirmed to fail to compile
(`sanitize` didn't exist yet), then the implementation was added.

Full session module suite, single-threaded (the timing-sensitive naming
tests share real subprocesses and must not run concurrently):

```
$ cargo test --lib session:: -- --test-threads=1
...
test result: ok. 75 passed; 0 failed; 0 ignored; 0 measured; 611 filtered out; finished in 16.49s
```

Full workspace suite (unit + all integration test binaries):

```
$ cargo test -- --test-threads=1
...
test result: ok. 686 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 26.95s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   (doctests)
test result: ok. 9 passed; ...
test result: ok. 1 passed; ...   (x several integration binaries)
test result: ok. 3 passed; ...
test result: ok. 6 passed; ...
test result: ok. 5 passed; ...
test result: ok. 2 passed; ...
test result: ok. 1 passed; ...
... (17 test binaries total, all "0 failed")
```

Formatting and lints:

```
$ cargo fmt --check
(no output — clean)

$ cargo clippy --all-targets
    Checking dct v0.1.0 (/Users/lei/work/dc/dc-terminal)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.7s
(no warnings)

$ git diff --check
(no output — clean)
```

One clippy warning was fixed along the way: `while let Some(next) =
chars.next()` inside the CSI-consuming loop → `for next in chars.by_ref()`
(clippy::while_let_on_iterator).

## New tests added (all in `src/session.rs`, `mod tests`)

Pure-function level:
- `sanitize_strips_escape_sequences_and_ctrl_codes`
- `sanitize_pops_the_previous_character_on_backspace`
- `sanitize_backspace_on_an_empty_buffer_does_nothing`
- `sanitize_keeps_ordinary_text_untouched`
- `fallback_name_strips_control_bytes_even_if_first_input_somehow_carries_them`
- `fallback_name_is_none_for_whitespace_only_input`
- `fallback_name_caps_and_trims`
- `model_name_strips_control_bytes_after_clean_name`
- `model_name_is_none_when_nothing_survives_the_wash`
- `collect_first_input_applies_backspace_as_the_user_intended`
- `collect_first_input_backspace_reaches_across_calls`
- `collect_first_input_drops_escape_sequences_and_control_codes`
- `collect_first_input_backspace_on_an_empty_buffer_does_nothing_bad`

Full `SessionManager` integration level (real subprocess, real `tick()`,
50ms-poll deadline loops — the same pattern the file already uses everywhere
else; per the brief's known-flake warning, none of these depend on landing a
tick inside a narrow timing window, they poll to a 5s deadline):
- `a_tag_born_from_control_bytes_never_carries_them_into_the_render_path` —
  the brief's required core regression test: feeds `\x1b[A` (arrow-up,
  simulating "pressed up-arrow to recall history before typing") plus a
  typo-and-backspace sequence through `send_input` one keystroke at a time,
  asserts the resulting `tag` never contains a control character at any
  point during polling, and lands on the exact backspace-corrected string
  `"fix the"`.
- `the_model_named_path_is_sanitized_too` — model returns a name containing
  `ESC` and `DEL`, asserts the stored tag is clean.
- `whitespace_only_input_leaves_the_name_slot_open_for_a_later_real_attempt`
  — see M2 above.

## Concerns / follow-ups

None blocking. Two notes for whoever reviews this:

- `append_capped`'s CSI-consumption loop has no explicit upper bound on how
  many bytes it will consume looking for a terminator; if it ever finds
  none, it silently consumes the rest of the current `text` chunk. This is
  safe in practice given `FIRST_INPUT_MAX` (200 chars) and the 24-char
  `name_prompt` model-name cap bound the blast radius, and it's the same
  trade-off any ANSI-stripper makes on malformed input — but it's worth
  knowing about if this function is ever reused somewhere with an unbounded
  input size.
- I extracted `fallback_name`/`model_name` beyond what the brief's "要改什么"
  section literally asked for (it only asked for a shared `sanitize` called
  at the two write points). I judged this necessary rather than optional
  because mutation M3 could not otherwise be honestly closed — the
  alternative was leaving a genuinely dead/untestable line in `request_name`,
  which conflicts with the brief's own "no test failure = test not written
  well enough" instruction. Flagging this as a deliberate scope decision in
  case the reviewer disagrees with the trade-off.

## Round 2 — fixes from the task review (spec ❌, two Important findings)

The task review confirmed the sanitizing filter itself (`apply_keystrokes`/
`sanitize`) was correct and left it untouched. Two Important findings were
about consequences and coverage, not the filter, plus two minor cleanups.

### Important 1 — the reopened naming gate was unbounded

The review's diagnosis was correct and its root-cause analysis of its own
brief's error was correct too: leaving `name_slot` as `None` on an empty
fallback does *not* give a later real user input a second chance, because
`collect_first_input` returns early once `first_input_sealed` is set — no
later input ever reaches `first_input`. Only the model can ever name such a
session, and gating on `name_slot.is_none()` reopened `request_name` on
*every* subsequent Working→Idle transition, which meant: an unbounded
stream of 15-second LLM calls for a session that will never usefully
produce a name, and a real lost-update race — a second `request_name`
firing before the first one's background thread completes would
synchronously overwrite `name_slot` back to `None` (since a still-empty
fallback recomputes to `None` every time), discarding a real name the first
thread had already written.

Fix: added a separate `Session::name_attempted: bool` field, set to `true`
as the very first statement in `request_name` (before the fallback write,
before the backend check — so there is no window where two calls could both
see "not yet attempted"). `tick()`'s gate now checks `!s.name_attempted`
instead of `name_slot.is_none()`. This restores true "ask once, ever"
semantics — matching what the original (pre-existing, pre-fix-1) code
comments claimed but, after my first pass, no longer delivered.

Comments corrected to stop claiming `name_slot`'s `None`-ness is the "have
we tried" signal:
- `SessionInfo::tag`'s doc (was line 376-377, the "一次干完活时起一次，
  之后不变" claim) — now points at `Session::name_attempted` as the
  mechanism that actually guarantees this.
- `request_name`'s doc and the `Session::name_slot` field doc (was line
  1019-1021 in the reviewed diff) — rewritten to state the gate is
  `name_attempted`, not `name_slot`.
- Two test doc comments (`recovering_from_a_failure_does_not_count_as_finishing_a_round`,
  and the test formerly named
  `whitespace_only_input_leaves_the_name_slot_open_for_a_later_real_attempt`)
  also asserted the old, wrong mechanism and were rewritten.

**The whitespace-only test had to be inverted, not just relabeled.** The
old test proved the gate *reopens* after an empty fallback — that was
exactly the bug. It's renamed
`whitespace_only_input_is_asked_about_exactly_once_not_forever` and now
proves the opposite: after the first (empty-fallback) attempt, a second
forced Working→Idle transition, even with a backend that *would* produce a
real name, must **not** change the tag. Verified this test actually
discriminates: reverted to the old assertion direction against the fixed
code and confirmed it fails (see mutation table below, M9/M10 use the same
mechanism).

Mutation testing on the new gate:

| # | Mutation | Result | Test(s) that caught it |
|---|---|---|---|
| M9 | Negate the tick() gate: `!s.name_attempted` → `s.name_attempted` | Caught | 8 tests failed, incl. `a_session_gets_named_after_its_first_round_of_work`, `a_name_is_pinned_and_never_asked_for_twice` |
| M10 | Remove `s.name_attempted = true;` from `request_name` | Caught | `a_name_is_pinned_and_never_asked_for_twice`, `whitespace_only_input_is_asked_about_exactly_once_not_forever` |

### Important 2 — the core regression test was at the wrong layer

Correct finding, and the review's prediction was right: I added
`ui::board::tests::a_tag_with_control_bytes_never_reaches_the_rendered_buffer`
in `src/ui/board.rs`, using the existing `TestBackend` + `screen_text()`
harness (`board.rs:266`), setting `SessionInfo.tag = "\x1b[Afix\x7f"`
directly (bypassing the daemon entirely, simulating what a UI talking to an
unpatched daemon would receive) and rendering a real frame.

**The test fails.** `screen_text()`'s output contains the literal bytes
`\u{1b}[Afix\u{7f}` — the board's list-item rendering path
(`Span::raw` → `List`/`ListItem` → `Line` → `Span::render_ref`, `board.rs:211`)
has no control-character filter of its own, exactly as the review predicted
from reading the `ratatui` source. The daemon-side `sanitize` added in
Round 1 is necessary and correct for the primary path (new daemon → new
UI), but does not close the socket-boundary path (old/unpatched daemon →
new UI) the review pointed at — `SessionInfo.tag` crosses that boundary as
`#[serde(default)]` JSON with no guarantee about what produced it.

Per the review's explicit instruction, I did **not** silently add a filter
to `truncate`/`session_label`/the render path — that code is `grid.rs:475`
/ `board.rs` territory that the review's own message describes as the
subject of a *different* Important finding (about `grid.rs:475` dropping
`truncate(session_label(info), 20)`), i.e. arguably someone else's fix task.
Instead: the test is marked `#[ignore = "..."]` with a reason string that
states the gap and cites this report, so `cargo test` stays green while the
test remains in the tree, discoverable, and immediately runnable
(`cargo test -- --ignored`) by whoever picks up UI-side filtering — they
won't need to reconstruct the threat model, just delete the `#[ignore]` and
watch it turn green once fixed.

**Recommendation for whoever owns the UI side**: the fix likely belongs in
`session_label` (`widgets.rs:168`) — a single choke point for both
`board.rs` and `grid.rs`'s title rendering — running the tag through the
same kind of control-character strip before it's ever handed to `Span`.
Whether to reuse `session::sanitize`-shaped logic or something UI-local is
a decision for that task, not this one.

### Minor findings

- `sanitize` changed from `pub(crate)` to private (`fn sanitize`, was
  `pub(crate) fn sanitize`) — no caller outside `session.rs` (verified with
  `grep -rn "session::sanitize\|::sanitize(" src/` excluding `session.rs`
  itself: no matches). Tests still see it fine since `mod tests` is a
  submodule.
- Added a paragraph to `model_name`'s doc comment explaining that its call
  to `sanitize` borrows keystroke semantics (backspace-pops) for text that
  isn't a keystroke stream, and why that's harmless here (a model answer
  containing `\x7f`/`\x08` is already an edge case — screen content the
  model is echoing back after being manipulated — and either "pop" or
  "drop" reads leave the same safety property: the byte never survives
  into the name; "pop" was kept only so `sanitize` stays a single
  implementation instead of one per caller).

### Re-verification

```
$ cargo test --lib session:: -- --test-threads=1
test result: ok. 75 passed; 0 failed; 0 ignored; 0 measured; 611 filtered out

$ cargo test --lib ui::board:: -- --test-threads=1
test result: ok. 14 passed; 0 failed; 1 ignored; 0 measured; 672 filtered out
(the 1 ignored is the new, deliberately-red-if-run render-layer test — see
Important 2 above)

$ cargo test -- --test-threads=1        # full workspace: unit + all integration binaries
test result: ok. 686 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
... (16 more "ok" lines, one per integration test binary, all 0 failed)

$ cargo fmt --check
(clean)

$ cargo clippy --all-targets
(clean, no warnings)

$ git diff --check
(clean)
```

Files touched this round: `src/session.rs` (the `name_attempted` fix and
comment corrections), `src/ui/board.rs` (the new render-layer test).

### Concerns

- The `#[ignore]`d test in `board.rs` is, as far as I could find, the first
  use of `#[ignore]` in this codebase (`grep -rn "#\[ignore" src/` had no
  prior hits). I judged it the right tool for "known failing, deliberately
  not fixed here, must stay discoverable" rather than inventing a
  codebase-local convention, but flagging the precedent in case the
  reviewer prefers a different way to record an intentionally-still-broken
  regression test (e.g. tracking it purely in this report with no test
  artifact at all).
- The UI-side gap Important 2 surfaced is real and is not fixed by this
  task. `tag` reaching the UI unsanitized from an old daemon is a genuine,
  if narrower, instance of the same injection class Finding 1 was about.

## Round 3 — closing the render-path gap (Important 2, acted on)

The coordinator clarified that "say so in your report rather than adding it
silently" meant *report before acting*, not *leave it broken* — and set the
scope explicitly to include the UI-side filter. Implemented it.

### Where the filter went, and why

Added the filter inside `truncate` (`src/ui/widgets.rs`), not
`session_label`. Reasoning:

1. **Coverage.** `truncate` is the single choke point every rendered title
   in this codebase passes through, not just the tag: `board.rs:211`
   (`truncate(session_label(s), 15)`), `grid.rs:369` and `grid.rs:481`
   (`truncate(session_label(info), 20)`), and `attach.rs:257`
   (`truncate(&s.tag, 15)`). Filtering in `session_label` would only cover
   `tag` — `truncate`'s other callers (`board.rs`'s group name/parent dir,
   `s.activity` which is a rendering of live agent screen content, `pick.rs`'s
   labels/notes, `secret.rs`'s labels) get hardened for free, and none of
   those are more trustworthy than `tag` — a directory name or an agent's
   last screen line can contain arbitrary bytes too.
2. **No signature change.** `session_label` currently returns a zero-copy
   `&str` (either `&s.tag` or `&s.profile`); filtering there means
   allocating and changing the return type to `String`, rippling through
   every call site. `truncate` already takes and returns an owned `String` —
   the fix is entirely internal to the function.
3. **Confirmed no CJK/width regression** (this was the specific thing I was
   asked to check for before proceeding, and did before writing the fix):
   `char_width` already returns 0 for every control character (see its own
   doc comment — this was already true before this round). The old loop
   pushed 0-width control characters into `out` without them ever
   contributing to `w`; the new code just skips the `push` for those same
   characters via `continue` before reaching the width/cap logic. `w`'s
   accumulated value is byte-for-byte identical with or without the
   control characters in the input, so the truncation point for the
   surrounding real characters cannot move. Added
   `truncate_control_byte_stripping_does_not_disturb_cjk_width_accounting`
   and `truncate_dropping_control_bytes_does_not_shift_the_width_budget` to
   pin exactly this — both check that a control byte's presence/absence
   changes nothing about where the *next* real character gets cut.

The filter drops any `char::is_control()` character outright — it does
*not* do the daemon-side `sanitize`'s full CSI-sequence consumption (ESC +
`[` + params + terminator, all treated as one unit). That asymmetry is
deliberate, not an oversight: the daemon-side function additionally has to
reconstruct "what the user meant to type" (so a stray `[A` left over from a
half-processed escape sequence would be a real, if minor, correctness bug
there). At the render layer the only property that needs to hold is "no
raw control byte reaches the terminal" — and dropping just the `ESC` byte
already breaks the sequence: a terminal emulator needs the leading `ESC` to
recognize `[A` as a live cursor-movement command; without it, `[A` is inert
printable text. Per-character `is_control()` filtering is sufficient for
that property and is what `Buffer::set_stringn`/`Paragraph` already do
elsewhere in `ratatui`, so this keeps the same simple model rather than
importing a second, heavier CSI-aware state machine into a generic
string-truncation utility.

### Grid tile title / attach block title coverage (asked, not tested here)

Both route through `truncate` and are therefore covered by this fix:

- **Grid tile title**: `grid.rs:369` (compose-reply header) and
  `grid.rs:481` (the tile's own title bar) both call
  `truncate(session_label(info), 20)` unconditionally — `truncate` always
  runs the same per-character loop regardless of whether the string
  actually needs cutting, so the filter applies even to tags that fit
  comfortably under the width cap.
- **Attach block title**: `attach.rs:257` calls `truncate(&s.tag, 15)`
  directly, same unconditional path.

Grepped every `.tag`/`session_label(` occurrence in `src/ui/*.rs` to check
for a path that reads the tag without going through `truncate`
(`attach.rs:240` is the only other production use, and it's an
`is_empty()` check, not a render) — found none. No new tests added for the
grid title or attach title per the instruction; Fix 2 owns `grid.rs` and
was asked to be told rather than have tests added preemptively.

### Test the coordinator asked to see turn green

`#[ignore]` removed from
`ui::board::tests::a_tag_with_control_bytes_never_reaches_the_rendered_buffer`.
It now passes in the default suite with no flag needed.

### Mutation testing

| # | Mutation | Result | Test(s) that caught it |
|---|---|---|---|
| M11 | Negate the filter condition: `if ch.is_control()` → `if !ch.is_control()` (keeps control chars, drops everything else) | Caught | 20 tests failed across `board`, `grid`, `keys`, `pick`, `widgets` — including `a_tag_with_control_bytes_never_reaches_the_rendered_buffer` |
| M12 | Remove the filter entirely (revert to the old loop body) | Caught | `a_tag_with_control_bytes_never_reaches_the_rendered_buffer` plus the three new `truncate_*` tests |

### Re-verification

```
$ cargo test --lib ui::widgets:: -- --test-threads=1
test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 665 filtered out

$ cargo test --lib ui::board:: -- --test-threads=1
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 675 filtered out
(a_tag_with_control_bytes_never_reaches_the_rendered_buffer is in this 15,
 no longer ignored)

$ cargo test -- --test-threads=1        # full workspace
test result: ok. 690 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
... (16 more "ok" lines, one per integration test binary, all 0 failed)

$ cargo fmt --check
(clean)

$ cargo clippy --all-targets
(clean, no warnings)

$ git diff --check
(clean)
```

Files touched this round: `src/ui/widgets.rs` (the `truncate` filter + 3
new tests), `src/ui/board.rs` (removed `#[ignore]`, updated its doc
comment to point at where the defense actually lives now).

### Concerns

None. The render-path gap Important 2 identified is now closed with
coverage that discriminates (verified by mutation), and both dependent
titles (grid, attach) were confirmed covered by inspection rather than
assumed.
