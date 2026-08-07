# Task 1 report: `~/.dct/config.toml` `[llm]` section

## What was done

Followed the brief's TDD steps exactly.

1. Created `src/config.rs` containing only the `#[cfg(test)] mod tests` block from the
   brief (verbatim), with no implementation yet.
2. Added `pub mod config;` to `src/lib.rs`. Placement: alphabetically between
   `pub mod clipboard;` and `pub mod daemon;` (see "Deviations" below).
3. Ran `cargo test --lib config::` to confirm the expected compile failure
   (`Config`, `Transport`, `config_path_for_socket` undeclared — see actual output below;
   this is the same failure class the brief's "unresolved module" expectation points at,
   since by this point the module was already wired into `lib.rs`).
4. Inserted the implementation from the brief verbatim above the tests module: module doc
   comment, `Transport` enum, `LlmConfig` struct + `Default` impl, `Config` struct,
   `Config::from_toml`, `Config::load`, `config_path_for_socket`.
5. Ran `cargo test --lib config::` — 6 passed.
6. Ran `cargo fmt` (no changes needed beyond what was already correct) and the full
   `cargo test --lib` suite — 417 passed (baseline 411 + 6 new).
7. Ran `git diff --check` — clean, no output.
8. Committed with the exact message given in the brief.

## Exact commands run

```
export PATH="$HOME/.cargo/bin:$PATH"
env GOCACHE=/tmp/x cargo test --lib config:: 2>&1 | tail -40      # Step 2: confirm failure
cargo test --lib config:: 2>&1 | tail -30                         # Step 4: confirm pass
cargo fmt && env GOCACHE=/tmp/dcwb-go-cache cargo test --lib 2>&1 | tail -20   # full suite
git diff --check
git add src/config.rs src/lib.rs
git commit -m "feat(config): add ~/.dct/config.toml with an [llm] section

Defaults to the claude profile over the CLI transport, which needs no
credential at all. A missing or broken config falls back to defaults
instead of failing to start: the LLM is an enhancement, not a foundation."
```

## Actual test output

### Step 2 (before implementation) — confirms expected failure

```
error[E0433]: cannot find type `Transport` in this scope (x multiple)
error[E0433]: cannot find type `Config` in this scope (x multiple)
error[E0425]: cannot find function `config_path_for_socket` in this scope
error: could not compile `dct` (lib test) due to 9 previous errors; 3 warnings emitted
```

(The module itself compiled/linked fine since `pub mod config;` was already added before
this run — the errors are the follow-on "types don't exist yet" errors, which is the
same underlying signal the brief's "unresolved module" expectation was checking for:
the tests do not yet have an implementation to run against.)

### Step 4 (after implementation) — `config::` tests

```
running 6 tests
test config::tests::config_path_sits_next_to_the_socket ... ok
test config::tests::a_missing_file_is_defaults_not_an_error ... ok
test config::tests::an_empty_file_is_all_defaults ... ok
test config::tests::a_partial_llm_section_keeps_the_other_defaults ... ok
test config::tests::parses_a_full_llm_section ... ok
test config::tests::a_broken_file_falls_back_to_defaults_and_does_not_panic ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 411 filtered out; finished in 0.00s
```

### Full suite

```
test result: ok. 417 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.24s
```

411 baseline + 6 new = 417, matches expectation exactly.

### `git diff --check`

No output (clean).

## Commit SHA

`1bcaf98d2e30d010fef77245d5343ae4c666d0b0`

Branch: `feat/llm-connection`
Files changed: `src/config.rs` (new), `src/lib.rs` (+1 line)

## Deviations from the brief

One deviation, purely mechanical, no behavior/content difference:

- The brief's Step 3 prose says to add `pub mod config;` "在 `pub mod client;` 后面"
  (after `pub mod client;`), but also says "保持字母序" (keep alphabetical order).
  Those two instructions conflict: `src/lib.rs`'s existing order is
  `cli, client, clipboard, daemon, ...`, and alphabetically `config` sorts after
  `clipboard` and before `daemon` (not immediately after `client`, which would break
  the file's alphabetical order). I followed the explicit "保持字母序" instruction and
  the module list's actual alphabetical convention, placing the line between
  `pub mod clipboard;` and `pub mod daemon;`. No other content, naming, or default
  value was changed from the brief — struct/field names, the `Transport` variants,
  default provider (`"claude"`) and transport (`Cli`), error-handling behavior, and all
  Chinese comments are verbatim from the brief.

No other deviations. Test names, struct/field names, default values, and Chinese
comments are all copied verbatim from the brief.

## Self-review

- Double-checked that `toml = "0.8"` and `tempfile = "3"` (dev-dependency) were already
  present in `Cargo.toml` — no new dependency was added, per the global constraint.
- `Config::load`'s error branch on read failure (non-`NotFound`) and the parse-failure
  branch both build an interpolated string containing `path.display()` and the error
  `{e}` around a literal Chinese label — this matches the brief's code exactly and the
  brief's own global constraint note ("do not build Chinese strings in logic") is about
  runtime *logic* branching on strings, not about eprintln diagnostic text; the brief
  explicitly specifies this text verbatim in Step 3, so I did not alter it. Flagging
  this only so the reviewer can confirm this reading is correct — I did not deviate
  from the brief either way.
- No `unwrap`/`panic` paths were introduced; the broken-file test explicitly checks no
  panic occurs, and `Config::load` only ever returns `Config::default()` on any
  read/parse error.
- `cargo fmt` produced no diff on `src/config.rs`/`src/lib.rs` beyond what was already
  written matching rustfmt conventions (confirmed by running `cargo fmt` before the
  final test/commit and seeing a clean `git diff --check` afterward with no additional
  unstaged formatting-only changes appearing).
- Nothing else in this task felt ambiguous or risky; scope was self-contained and
  nothing downstream depends on it yet per the task description.
