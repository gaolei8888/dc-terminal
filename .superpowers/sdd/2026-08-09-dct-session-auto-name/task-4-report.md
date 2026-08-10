# Task 4 report — 洗模型返回值 + 起名 prompt

## What changed and where

`src/session.rs`, right after `explain_prompt` (and before `FIRST_INPUT_MAX`):

- `const NAME_MAX_CHARS: usize = 24;`
- `pub(crate) fn clean_name(raw: &str) -> String`
- `pub fn name_prompt(first_input: &str, screen: &str) -> crate::llm::Prompt`

Both the const and `clean_name` carry `#[cfg_attr(not(test), expect(dead_code, reason = "..."))]`
(see "Deviations" below for why).

Tests added to `mod tests` (placed right before the pre-existing
`with_no_backend_the_explanation_stays_empty_and_nothing_breaks`):

- `clean_name_strips_quotes_punctuation_and_extra_lines` (verbatim from the brief)
- `clean_name_returns_empty_when_there_is_nothing_left` (verbatim from the brief)
- `clean_name_caps_a_runaway_answer` (verbatim from the brief)
- `clean_name_strips_a_quote_stacked_with_trailing_punctuation` (added — pins the
  bug fix, see below)
- `clean_name_keeps_a_quote_that_sits_in_the_middle` (added — pins verification
  item 1)
- `clean_name_keeps_a_name_exactly_at_the_cap_intact` (added — pins verification
  item 2)
- `name_prompt_carries_both_the_first_line_and_the_screen` (verbatim from the brief)

Nothing else in the file was touched. `src/proto.rs` was not touched;
`PROTOCOL_VERSION` stays 6. No call site, no background thread, no new
`Session`/`SessionManager` state was added — `clean_name` and `name_prompt`
have no caller yet, as instructed.

## Test commands run, with output summary

Red check (Step 2, before any implementation existed):

```
cargo test --lib session::tests::clean_name_strips_quotes_punctuation_and_extra_lines
```

Failed to compile, as expected:

```
error[E0425]: cannot find function `clean_name` in this scope
   --> src/session.rs:1043:20
...
error[E0425]: cannot find function `name_prompt` in this scope
   --> src/session.rs:1064:17
...
error: could not compile `dct` (lib test) due to 10 previous errors
```

Green check after implementing the brief's Step 3 code verbatim:

```
cargo test --lib session::tests::clean_name
```

```
running 3 tests
test session::tests::clean_name_returns_empty_when_there_is_nothing_left ... ok
test session::tests::clean_name_caps_a_runaway_answer ... ok
test session::tests::clean_name_strips_quotes_punctuation_and_extra_lines ... FAILED

thread 'session::tests::clean_name_strips_quotes_punctuation_and_extra_lines' panicked at src/session.rs:1087:9:
assertion `left == right` failed
  left: "修登录白屏」"
 right: "修登录白屏"
```

This is a genuine bug in the brief's reference implementation, not a test
artifact — see "Deviations" below. Fixed the implementation (single merged
`trim_matches`), then:

```
cargo test --lib session::tests::clean_name
cargo test --lib session::tests::name_prompt
```

```
running 6 tests
test session::tests::clean_name_returns_empty_when_there_is_nothing_left ... ok
test session::tests::clean_name_caps_a_runaway_answer ... ok
test session::tests::clean_name_strips_a_quote_stacked_with_trailing_punctuation ... ok
test session::tests::clean_name_keeps_a_quote_that_sits_in_the_middle ... ok
test session::tests::clean_name_strips_quotes_punctuation_and_extra_lines ... ok
test session::tests::clean_name_keeps_a_name_exactly_at_the_cap_intact ... ok

test result: ok. 6 passed; 0 failed

running 1 test
test session::tests::name_prompt_carries_both_the_first_line_and_the_screen ... ok

test result: ok. 1 passed; 0 failed
```

Full suite (Step 5):

```
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

- `cargo fmt`: applied (reformatted the new code's long lines and the
  `cfg_attr`/`expect` blocks — no logic changes, reviewed the diff after).
- `cargo clippy --all-targets -- -D warnings`: clean, `Finished` with no
  warnings or errors.
- `cargo test`: lib target `658 passed; 0 failed`; every integration test
  binary (`0 passed`/some N `passed; 0 failed` each) green; doc-tests
  `0 passed; 0 failed`. No `FAILED` or `error[` anywhere in the run.

`git diff --check`: no output (no whitespace errors).
`git diff --name-only`: only `src/session.rs` in the commit (a pre-existing
unrelated uncommitted change to `.superpowers/sdd/.gitignore` from before
this task started was left alone and not committed).

## Confirmation each new test genuinely failed before the change

- The three verbatim tests (`clean_name_strips_quotes_punctuation_and_extra_lines`,
  `clean_name_returns_empty_when_there_is_nothing_left`, `clean_name_caps_a_runaway_answer`)
  and `name_prompt_carries_both_the_first_line_and_the_screen` failed to
  **compile** before Step 3 (pasted above — `cannot find function 'clean_name'`
  / `'name_prompt'`), which is stronger than a runtime failure.
- `clean_name_strips_a_quote_stacked_with_trailing_punctuation` is a single
  assertion duplicating the first line of
  `clean_name_strips_quotes_punctuation_and_extra_lines`; it failed against
  the brief's own Step 3 code with the exact panic pasted above
  (`left: "修登录白屏」"`, `right: "修登录白屏"`) before I changed the
  implementation.
- `clean_name_keeps_a_quote_that_sits_in_the_middle` and
  `clean_name_keeps_a_name_exactly_at_the_cap_intact` did not fail against
  either implementation (both the buggy two-pass version and the fixed
  single-pass version handle these two cases identically) — they exist to
  pin down the two "verify rather than assume" behaviors the task asked
  about, not to catch a regression I found. I ran them against both
  implementations to confirm they pass either way, which is itself part of
  the verification.

## Findings on the two verification items

**1. `trim_matches` with a closure covering quotes and whitespace, applied
before a separate `trim_end_matches` for punctuation, on an answer like
`"「修登录白屏」。"`:**

This is not sane — it's a bug, and it's present in the brief's Step 3 code
verbatim. Traced character by character: `「修登录白屏」。` is
`「,修,登,录,白,屏,」,。`. The brief's first `trim_matches(QUOTES ∪
whitespace)` call strips the leading `「` (it's in `QUOTES`), but then looks
at the *last* character, `。`, which is not in `QUOTES` and not whitespace,
so it stops immediately — the trailing `」` is never reached because `。`
sits in front of it and doesn't match the predicate that call is using.
The result after that call is `"修登录白屏」。"`. The next call,
`trim_end_matches(TAIL)`, then strips just the `。` (it's in `TAIL`), but
stops at `」` because `」` isn't in `TAIL` either. Net result: `"修登录白屏」"`
— a stray closing quote survives, which is exactly the "「修登录白屏」。"
failure mode the doc comment on `clean_name` warns about. I confirmed this
by running the brief's code verbatim and pasting the actual panic above.

The fix: strip quotes and trailing punctuation in a **single** `trim_matches`
call over the union of both character sets (`QUOTES ∪ TAIL ∪ whitespace`).
A single call re-checks the union predicate against the new boundary
character after every strip, so it naturally handles arbitrary interleaving
of quotes and punctuation at each end, in one pass, with no fixed-point loop
needed. I verified this against all four brief assertions plus the "returns
empty" and "caps a runaway answer" cases, and added
`clean_name_strips_a_quote_stacked_with_trailing_punctuation` to pin it.

For a name that legitimately **contains** a quote in the middle (not at a
boundary), e.g. `修复 "login" 白屏`: `trim_matches` only strips from the two
ends inward and stops for good the moment it hits a non-matching character,
so an interior quote is never touched — the first character is `修` (not in
the union set) so the front trim does nothing at all, and the last character
is `屏` (also not in the union set) so the back trim does nothing either.
Confirmed with `clean_name_keeps_a_quote_that_sits_in_the_middle`, which
asserts the string passes through byte-for-byte.

**2. `chars().take(NAME_MAX_CHARS)` and multi-byte characters / panics:**

Sane, no bug. `str::chars()` yields `char`s (Unicode scalar values), already
decoded from UTF-8 — `take(n)` counts *characters*, not bytes, so it can
never stop mid-way through a multi-byte encoding, and collecting an iterator
of `char` into a `String` can't produce invalid UTF-8. There's no indexing
or byte-slicing anywhere in `clean_name`, so there's no panic surface for
this step. I added `clean_name_keeps_a_name_exactly_at_the_cap_intact`,
which builds a name of exactly `NAME_MAX_CHARS` `修` characters and asserts
`clean_name` returns it unchanged (both content and `chars().count()`),
confirming the boundary is `<=`, not off-by-one in either direction.

## Deviations from the brief, with reasoning

1. **`clean_name`'s body is not the brief's Step 3 code verbatim.** The
   brief's two-pass version (`trim_matches(QUOTES ∪ whitespace)` then
   `trim_end_matches(TAIL)` then `.trim()`) fails the brief's own first test
   assertion, as shown above. I replaced it with a single
   `trim_matches(QUOTES ∪ TAIL ∪ whitespace)` call (which also subsumes the
   final `.trim()`, since whitespace is already in the union set, so I
   dropped that line as redundant). This is the minimal change that makes
   the brief's own tests pass; all four of the brief's `clean_name`
   assertions and both "empty" tests pass identically before and after,
   except the one they were actually wrong about. I did not change
   `clean_name`'s signature, the constant, or `name_prompt`.

2. **Added three tests beyond the brief's Step 1 block** (`clean_name_strips_a_quote_stacked_with_trailing_punctuation`,
   `clean_name_keeps_a_quote_that_sits_in_the_middle`,
   `clean_name_keeps_a_name_exactly_at_the_cap_intact`). The brief's own
   ambiguity-resolution instructions asked me to verify the two "assume"
   items and, if I found nonsense, "add a test pinning the behavior you
   chose." I found nonsense in item 1, so I pinned both the fix and the
   adjacent "interior quote" case, and pinned item 2's boundary behavior
   even though it already worked, since the task asked me to *confirm*
   rather than assume it.

3. **Added `#[cfg_attr(not(test), expect(dead_code, reason = "..."))]` to
   `NAME_MAX_CHARS` and `clean_name`** — not in the brief's code block at
   all. `clippy --all-targets -- -D warnings` treats an uncalled
   `pub(crate)` item as a hard error (`-D dead-code`) in the plain lib
   build, and this task is explicitly required to leave both functions
   without a caller. `name_prompt` doesn't need this because it's `pub fn`
   inside a `pub mod session` — part of the crate's public surface, which
   clippy's dead-code lint doesn't flag as unreachable even with zero
   internal callers. `clean_name` is `pub(crate)` per the brief's own
   interface spec, so it doesn't get that exemption. I first tried a bare
   `#[expect(dead_code, ...)]`, which then failed in the *test* build
   instead (`this lint expectation is unfulfilled`, because the tests
   really do call `clean_name`, so it isn't dead code there) — that's why
   the attribute is gated to `cfg(not(test))`. This keeps `cargo clippy
   --all-targets -- -D warnings` clean in both the plain lib target and the
   lib-test target, without adding a call site, and the `expect` (rather
   than `allow`) means the moment Task 5 wires in a real caller, the
   now-unnecessary attribute turns into its own compile error instead of
   silently lingering.

4. **Commit message is in English with no AI attribution**, per this
   session's global constraint, overriding the Chinese message shown in the
   brief's Step 5 (that constraint was stated as binding for this task
   regardless of what any individual brief says). It also documents the bug
   fix and the `expect(dead_code)` rationale, which the brief's message
   didn't need to since its own reference code didn't have either issue.

---

## Fix round 1 (review findings)

Review of the initial commit (`4cdcda2`) raised two Important findings
about the replacement `clean_name` I wrote and a comment I carried over
from the brief without checking. Both are fixed in commit `a8ae456`.

### Finding 1 — `NAME_MAX_CHARS`'s doc comment stated something false

The comment said `24 是 12 个汉字，跟 prompt 里要的「不超过 12 个字」对得上`
(carried verbatim from the brief). That's GBK-era byte-counting logic and
directly contradicts the same comment's own preceding sentence
(`按字符数、不按显示宽度`): `.chars().take(24)` counts Unicode scalar
values 1:1, so 24 chars is 24 Chinese characters, not 12. I inherited this
without checking it, even though the task explicitly asked me to verify
the "24 chars = 12 汉字" claim in verification item 2 — I checked the
*mechanics* of `take()` (no panic, no mid-character split, exact boundary
correct) but did not check whether the *arithmetic in the comment* was
true, which is the gap the review caught.

Fixed by rewriting the comment to state what's actually true: 24 is a
character-count cap that gives a misbehaving model headroom beyond the
12-character target stated in the prompt (English answers need more
characters per word than Chinese needs per character), and it has nothing
to do with display width — that's still handled elsewhere by the UI. The
value itself (24) was not changed, per the review's instruction.

### Finding 2 — merging the trim character sets regressed leading punctuation

The single merged `trim_matches(QUOTES ∪ TAIL ∪ whitespace)` from the
initial commit fixed the interleaved-boundary bug, but as a side effect it
now also strips `TAIL` punctuation from the *front* of the string, which
the brief's original two-pass version never did (its front pass only ever
covered `QUOTES ∪ whitespace`). That regresses any name that legitimately
starts with a `TAIL` character, e.g. `.NET 迁移` → `NET 迁移`, `.env 权限`
→ `env 权限` — both plausible session names in this tool's domain (dotfiles,
.NET projects). My own verification for finding 1 of the original task
(item 1, "does a mid-string quote survive") only tested an *interior*
character, never a *leading* `TAIL` character, so this got through.

Fixed by splitting the trim asymmetrically instead of merging into one
call:

```rust
let line = line.trim_start_matches(|c: char| QUOTES.contains(&c) || c.is_whitespace());
let line = line
    .trim_end_matches(|c: char| QUOTES.contains(&c) || TAIL.contains(&c) || c.is_whitespace());
```

- Front: only quotes and whitespace — never touches `TAIL` punctuation, so
  a leading `.` in `.NET 迁移` survives.
- End: quotes, `TAIL` punctuation, and whitespace together in one
  `trim_end_matches` call — this is still what's needed to fix
  `「修登录白屏」。`, where the closing `」` and the trailing `。`
  interleave and must be stripped in the same pass (a single
  `trim_end_matches` re-checks the union predicate against the new
  boundary character after every strip, so it handles the interleaving
  without a loop).

Behavior before/after, confirmed by test:

| Input | Before fix (single merged `trim_matches`) | After fix (asymmetric) |
|---|---|---|
| `「修登录白屏」。` | `修登录白屏` (correct) | `修登录白屏` (still correct) |
| `.NET 迁移` | `NET 迁移` (regression — leading `.` eaten) | `.NET 迁移` (correct, unchanged) |
| `.env 权限` | `env 权限` (regression) | `.env 权限` (correct, unchanged) |

Added two new assertions in `clean_name_keeps_a_leading_tail_punctuation_character`
pinning `.NET 迁移` and `.env 权限` surviving intact, and confirmed
`clean_name_strips_a_quote_stacked_with_trailing_punctuation` (the
`「修登录白屏」。` case from the first round) and all four of the brief's
original verbatim assertions in `clean_name_strips_quotes_punctuation_and_extra_lines`
still pass unchanged.

`name_prompt` and both `#[cfg_attr(not(test), expect(dead_code, ...))]`
attributes were not touched in this round, per the review's instruction.

### Commands run and output

```
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --lib session
cargo test
```

- `cargo fmt`: applied, reformatted the new lines only (reviewed the diff
  after — no logic changes).
- `cargo clippy --all-targets -- -D warnings`:

  ```
      Checking dct v0.1.0 (/Users/lei/work/dc/dc-terminal)
      Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.43s
  ```

  Clean, no warnings.

- `cargo test --lib session`: `105 passed; 0 failed` (this filter also
  matches unrelated `ui::*` tests that happen to have "session" in their
  name/path; the `session::tests::clean_name_*` and `session::tests::name_prompt_*`
  tests are a subset of that run). Ran the narrower filters directly too:

  ```
  cargo test --lib session::tests::clean_name
  ```
  ```
  running 7 tests
  test session::tests::clean_name_returns_empty_when_there_is_nothing_left ... ok
  test session::tests::clean_name_strips_a_quote_stacked_with_trailing_punctuation ... ok
  test session::tests::clean_name_caps_a_runaway_answer ... ok
  test session::tests::clean_name_strips_quotes_punctuation_and_extra_lines ... ok
  test session::tests::clean_name_keeps_a_quote_that_sits_in_the_middle ... ok
  test session::tests::clean_name_keeps_a_leading_tail_punctuation_character ... ok
  test session::tests::clean_name_keeps_a_name_exactly_at_the_cap_intact ... ok

  test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 652 filtered out; finished in 0.00s
  ```

- `cargo test` (full suite): first run showed one failure unrelated to
  `session.rs` — `ui::tests::entering_a_session_always_lands_at_the_bottom_even_without_a_resize`
  — which passed in isolation (`cargo test --lib ui::tests::entering_a_session_always_lands_at_the_bottom_even_without_a_resize`
  → `1 passed; 0 failed`) and passed on a full rerun (`659 passed; 0 failed`
  across the lib target, plus every integration binary green). This is a
  pre-existing flaky test unconnected to this change — it lives in
  `ui::tests`, not `session::tests`, and this round of fixes touched only
  `clean_name`'s trim logic and a doc comment. Final full-suite run: all
  green, 659 lib tests + all integration binaries + doc-tests, 0 failures.

### Commit

`a8ae456` — "fix: correct a false doc claim and a leading-punctuation
regression in clean_name" (English, no AI attribution). Only
`src/session.rs` changed; `src/proto.rs` untouched, `PROTOCOL_VERSION`
still 6.
