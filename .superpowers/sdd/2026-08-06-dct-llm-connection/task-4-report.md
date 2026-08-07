# Task 4 report: Backend trait, Prompt, hard call timeout

## What I did

Followed the brief's TDD steps exactly, in `src/llm/mod.rs`:

1. Appended the exact test module from the brief (Step 1) to the existing
   file, which at that point only had the doc comment and `pub mod creds;`.
2. Ran `cargo test --lib llm::tests` and confirmed it failed to compile with
   `cannot find type 'LlmError'` / `cannot find function 'complete_with_timeout'`
   (the brief's Step 2 expected reason, expressed across several E0433/E0425
   errors rather than a single one, since the test module references all
   four missing items).
3. Replaced the file with the brief's Step 3 implementation verbatim: module
   doc comment, `pub mod creds;`, `Prompt`, `LlmError`, `Backend` trait, and
   `complete_with_timeout` using `std::sync::mpsc::channel` + `recv_timeout`.
4. Ran `cargo test --lib llm::` — all 9 tests green (6 from Task 3's
   `creds` module + 3 new).
5. Ran `cargo fmt`, then the full suite `cargo test --lib` — 431 passed.
   `cargo fmt` reformatted the test module's multi-line `assert_eq!`/`assert!`
   calls and the `Prompt` struct literal in `p()` onto multiple lines (the
   brief's markdown had them on single lines); this is rustfmt's own
   formatting, not a content change — logic and names are untouched.
6. `git diff --check` was clean.
7. Committed with the exact message given in the brief.

## Exact test commands and output

```
$ export PATH="$HOME/.cargo/bin:$PATH"
$ cargo test --lib llm::tests    # after Step 1, before Step 3 (expected failure)
error[E0433]: cannot find type `LlmError` in this scope
...
error[E0425]: cannot find function `complete_with_timeout` in this scope
...
error: could not compile `dct` (lib test) due to 18 previous errors; 1 warning emitted
```

```
$ cargo test --lib llm::         # after Step 3
running 9 tests
test llm::creds::tests::debug_never_prints_the_secret ... ok
test llm::creds::tests::an_empty_token_is_treated_as_absent ... ok
test llm::creds::tests::any_unexpected_shape_yields_none_never_an_error ... ok
test llm::creds::tests::codex_api_key_login_yields_a_key ... ok
test llm::creds::tests::codex_sso_login_yields_a_bearer ... ok
test llm::tests::a_fast_backend_returns_its_answer ... ok
test llm::creds::tests::reads_the_claude_oauth_access_token ... ok
test llm::tests::a_backend_error_passes_through ... ok
test llm::tests::a_slow_backend_gives_up_instead_of_blocking_forever ... ok

test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 422 filtered out; finished in 0.16s
```

```
$ cargo fmt && cargo test --lib
test result: ok. 431 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.32s
```

(428 before this task's change, per the task instructions; 431 after — +3
new tests, matches expectation exactly.)

```
$ git diff --check
(no output, exit 0)
```

## Commit

SHA: `2bf06ddcf64d81bbc3ca602eb96c00590d12fb08`

```
feat(llm): add the Backend trait and a hard call timeout

complete_with_timeout is how the 'never block the TUI' constraint is
enforced: the worst a caller can experience is a Timeout after the budget.
A frozen dct and a dead agent look identical on screen, which makes blocking
the redraw loop the most expensive failure this tool has.
```

1 file changed, 110 insertions(+) — `src/llm/mod.rs` only.

## Deviations

None in substance. The only difference from the brief's literal text is
rustfmt's line-wrapping of the test module (multi-line `Prompt { ... }`
literal, multi-line `assert_eq!`/`assert!` calls) — required by the
"`cargo fmt` clean" global constraint, and it does not change any name,
type, or behavior from the brief.

## Self-review

- `Prompt`, `LlmError`, `Backend`, `complete_with_timeout` all match the
  brief's signatures and derive lists exactly: `Prompt` is
  `Debug, Clone` only (no `PartialEq`, matching the brief — tests never
  compare `Prompt` values, only `Result<String, LlmError>`); `LlmError` is
  `Debug, Clone, Copy, PartialEq, Eq`.
- Neither `Prompt` nor `LlmError` can hold a credential, so the Task 3
  redaction concern (carried forward in the instructions) doesn't apply
  here — confirmed by re-reading both definitions after implementing.
- `complete_with_timeout` does not attempt to cancel or kill the spawned
  thread on timeout, per the explicit instruction not to "fix" this. The
  thread sends into `tx` after the parent's `rx` may already be dropped;
  `let _ = tx.send(...)` discards the resulting `SendError` silently, which
  is exactly the documented "sends into a channel nobody is listening to
  and exits" behavior.
- The timing test
  (`a_slow_backend_gives_up_instead_of_blocking_forever`) ran in well under
  2s each time (part of a 5.3s full-suite run across 431 tests, no
  flakiness observed across the two full-suite runs I did). I did not
  loosen the assertion.
- `pub mod creds;` line preserved unchanged, per instructions.
- No new crate dependencies added; only `std::sync::mpsc`, `std::sync::Arc`,
  `std::time::Duration`, `std::thread` are used, all already available via
  std.
- Did not touch any other file; `git status` before commit showed only
  `src/llm/mod.rs` as modified by this task (the worktree's pre-existing
  modified files from earlier tasks/branches were left untouched).
