# Task 3 Report: Credential sourcing (`Inherit` / `Key` / `Bearer`)

## What was done

Created `src/llm/mod.rs` (doc comment + `pub mod creds;` only, per this
task's scope — later tasks fill in the rest) and `src/llm/creds.rs`,
implementing verbatim what the brief specified:

- `Credential` enum: `Inherit`, `Key(String)`, `Bearer(String)`, with a
  hand-written `Debug` impl (no `#[derive(Debug)]`) that redacts
  `Key`/`Bearer` payloads and prints `Inherit` for the no-secret variant.
- `non_empty` helper: treats an empty string as "field present but no
  value", returning `None` rather than an empty token.
- `parse_claude_oauth(json: &str) -> Option<String>`: pulls
  `claudeAiOauth.accessToken` out of the Claude Code OAuth JSON shape
  (same shape on macOS Keychain contents and Linux
  `~/.claude/.credentials.json`).
- `parse_codex_auth(json: &str) -> Option<Credential>`: reads
  `~/.codex/auth.json` shape — non-null `OPENAI_API_KEY` yields `Key`,
  otherwise falls back to `tokens.access_token` yielding `Bearer`.
- `read_claude_oauth() -> Option<String>`: real reader, `security
  find-generic-password -s "Claude Code-credentials" -w` on macOS,
  `~/.claude/.credentials.json` on other platforms. No unit test.
- `read_codex_auth() -> Option<Credential>`: real reader,
  `~/.codex/auth.json`. No unit test.

All four parsing/reading functions return `Option`, never `Result`, as
required — a vendor format change degrades to "no credential found",
never a propagated error.

Added `pub mod llm;` to `src/lib.rs`. This worktree does not yet have a
`journal` module (the brief's context implied one from a different
branch state), so `llm` was inserted in the correct alphabetical
position among the modules that actually exist here: after `i18n`,
before `profile`.

## TDD workflow followed

1. **Wrote the failing tests** — created `src/llm/creds.rs` containing
   only the `#[cfg(test)] mod tests { ... }` block from the brief
   (verbatim), plus the minimal skeleton (`src/llm/mod.rs` with `pub mod
   creds;`, and `pub mod llm;` in `lib.rs`) needed for the file to
   actually be part of the compiled crate.
2. **Confirmed the expected failure.** Ran:
   ```
   export PATH="$HOME/.cargo/bin:$PATH"
   cargo test --lib llm::creds
   ```
   Result: compile failure, 11 errors, all `cannot find type
   `Credential`` / `cannot find function `parse_claude_oauth`` /
   `cannot find function `parse_codex_auth`` in scope — i.e. the tests
   failed because the implementation didn't exist yet, which is the
   correct reason. (See "Deviation" note below — the brief's predicted
   error text was "unresolved module 'llm'"; actual text differed but
   the underlying cause, missing implementation, matches.)
3. **Wrote the implementation** — inserted the full `creds.rs` body from
   the brief verbatim above the test module.
4. **Confirmed green.** Ran:
   ```
   cargo test --lib llm::creds
   ```
   Output:
   ```
   running 6 tests
   test llm::creds::tests::an_empty_token_is_treated_as_absent ... ok
   test llm::creds::tests::any_unexpected_shape_yields_none_never_an_error ... ok
   test llm::creds::tests::debug_never_prints_the_secret ... ok
   test llm::creds::tests::codex_api_key_login_yields_a_key ... ok
   test llm::creds::tests::codex_sso_login_yields_a_bearer ... ok
   test llm::creds::tests::reads_the_claude_oauth_access_token ... ok

   test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 422 filtered out; finished in 0.00s
   ```
   Then ran `cargo fmt`, `git diff --check` (clean, no whitespace
   issues), and the full suite:
   ```
   cargo test --lib
   ```
   Output tail:
   ```
   test result: ok. 428 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.28s
   ```
   422 (baseline, verified before any change) + 6 (new) = 428, matching
   the brief's expectation exactly.
5. **Committed** with the exact message given in the brief:
   ```
   git add src/llm src/lib.rs
   git commit -m "feat(llm): source credentials from keys, CLI OAuth, or inheritance

   Parsing returns Option, never Result: vendor credential formats are
   undocumented and change without notice, so a format change must degrade the
   user to 'enter a key' rather than surface an error they cannot act on.

   Pure parsing is split from real reads so tests never touch the real Keychain,
   and Credential's Debug is hand-written to redact — credentials travel along
   error paths into stderr and logs."
   ```
   Commit SHA: `e8f2a1a47f6ad2343c3c5bc851f1f972db9c0f1b`
   (`git log --oneline -1` → `e8f2a1a feat(llm): source credentials from
   keys, CLI OAuth, or inheritance`)

   `git show --stat HEAD`:
   ```
   src/lib.rs       |   1 +
   src/llm/creds.rs | 172 +++++++++++++++++++++++++++++++++++++++++++++++++++++++
   src/llm/mod.rs   |   2 +
   3 files changed, 175 insertions(+)
   ```
   Only the three intended files are in the commit. An unrelated,
   pre-existing modification to `.superpowers/sdd/.gitignore` was left
   untouched and unstaged, as it is out of scope for this task.

## Deviations from the brief

1. **`src/llm/mod.rs` instruction location.** The brief said to add
   `pub mod llm;` "在 `pub mod journal;` 后" (after `pub mod journal;`),
   but this worktree's `src/lib.rs` (branch `feat/llm-connection`, after
   Task 1 and Task 2 commits) has no `journal` module — that must belong
   to a different branch/snapshot. I inserted `pub mod llm;` in the
   correct alphabetical position among the modules that do exist here:
   `i18n`, `llm`, `profile`. This preserves the letter of the rule
   (alphabetical order) even though the specific anchor module named in
   the brief doesn't exist in this worktree.
2. **Step 2's predicted error text.** The brief predicted "编译失败，
   unresolved module 'llm'" for the pre-implementation test run. Because
   I wired the minimal `mod.rs`/`lib.rs` skeleton first (necessary for
   `cargo test --lib llm::creds` to even locate the test module), the
   actual compile failure was "cannot find type/function ... in this
   scope" for `Credential`, `parse_claude_oauth`, `parse_codex_auth`
   instead. This is the same underlying situation (tests fail because
   the implementation is absent) — no functional deviation, just a
   difference in the literal compiler wording.

Neither deviation touches any of the three strict rules (Option-only
returns, pure/real split, hand-redacted Debug) or the exact test/
implementation code, which were used verbatim from the brief.

## Self-review: no test touches real credentials

Verified by reading the final `src/llm/creds.rs`:

- The `#[cfg(test)] mod tests` block only references the three
  hand-written constants `CLAUDE_SAMPLE`, `CODEX_SSO_SAMPLE`,
  `CODEX_KEY_SAMPLE` (all inline fake JSON strings, `-fake` suffixed
  tokens) and the `junk` array of literal strings. No test calls
  `read_claude_oauth()` or `read_codex_auth()`.
  `grep -n "read_claude_oauth\|read_codex_auth" src/llm/creds.rs` shows
  these two functions are defined once each, in non-test code, and never
  invoked anywhere in the file — including the test module.
- No test references `std::process::Command`, `security`, `$HOME`,
  `~/.claude`, or `~/.codex`. Those only appear inside
  `read_claude_oauth`/`read_codex_auth`, both outside `#[cfg(test)]` and
  both explicitly documented in the code comment as "没有单元测试覆盖
  （会碰真实凭据），只在 `dct llm check` 这条手动路径上跑" (no unit
  test coverage — touches real credentials — only exercised via the
  manual `dct llm check` path).
- `Credential` has no derived `Debug`; the hand-written impl was used
  verbatim, and `debug_never_prints_the_secret` passed, confirming
  redaction works by construction rather than by a weak test.
