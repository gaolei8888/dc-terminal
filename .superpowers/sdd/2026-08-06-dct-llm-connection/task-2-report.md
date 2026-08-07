# Task 2 report: profile 的 `[headless]` 与 `[api]` 块

## What was done

Followed the brief's TDD steps verbatim:

1. Appended the 5 given tests to `mod tests` in `src/profile.rs` (verbatim, no changes).
2. Ran `cargo test --lib profile::` — confirmed compile failure with
   `no field 'headless' on type 'profile::Profile'` (and `api`, `Wire` unresolved) as expected.
3. Inserted `HeadlessSpec`, `Wire`, `ApiSpec` (verbatim from the brief, including doc comments)
   right after `InstallSpec` in `src/profile.rs`, and added
   `pub headless: Option<HeadlessSpec>` / `pub api: Option<ApiSpec>` to `Profile` right after
   the `pub install: Option<InstallSpec>` field.
4. Appended the exact TOML blocks to `profiles/claude.toml`, `profiles/codex.toml`,
   `profiles/kimi.toml`, `profiles/glm.toml`, `profiles/deepseek.toml`, `profiles/qwen-api.toml`.
   `opencode.toml` and `qwen.toml` were left untouched (no `[headless]`, no `[api]`).
5. Ran `cargo test --lib profile::` — all green (46 tests, including the 5 new ones).
6. Ran `cargo fmt` and the full suite `cargo test --lib` — 422 passing (417 baseline + 5 new).
7. Ran `git diff --check` — clean.
8. Committed with the exact message given in the brief.

## Deviation from the brief (and why)

The brief's Step 3 only touched `src/profile.rs` and the six `profiles/*.toml` files. After
adding the two new fields to `Profile`, the crate failed to compile: two test-only struct
literals (`fake_agent()` in `src/daemon.rs:270` and in `src/session.rs:626`) construct `Profile`
field-by-field without `..Default::default()`, so they didn't automatically pick up the new
fields. I added `headless: None,` and `api: None,` to both literals — the minimal change needed
to make the crate compile, consistent with how every other `Option` field in those same literals
is already set to `None`. This was not mentioned in the brief but was required; I did not touch
anything else in either file. Both files were staged and committed alongside `src/profile.rs` and
`profiles/` (the brief's `git add src/profile.rs profiles/` alone would have left an uncompilable
tree).

No other deviation. Field placement, derives, doc comments, and TOML content match the brief
verbatim.

## Exact test commands run and output

Baseline (before any change):
```
$ cargo test --lib
test result: ok. 417 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.25s
```

Step 2 (tests written, implementation not yet added) — confirmed expected failure:
```
$ cargo test --lib profile::
error[E0433]: failed to resolve: use of undeclared type `Wire`
error[E0609]: no field `headless` on type `profile::Profile`
error[E0609]: no field `api` on type `profile::Profile`
error: could not compile `dct` (lib test) due to 9 previous errors
```

Step 4 (implementation added) — profile module green:
```
$ cargo test --lib profile::
running 46 tests
...
test profile::tests::claude_and_codex_declare_a_headless_command ... ok
test profile::tests::unverified_clis_declare_no_headless_command ... ok
test profile::tests::api_shaped_profiles_declare_an_api_block ... ok
test profile::tests::the_api_base_url_matches_the_env_base_url ... ok
test profile::tests::a_profile_without_the_new_blocks_still_parses ... ok
...
test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 376 filtered out; finished in 0.01s
```

Full suite after `cargo fmt`:
```
$ cargo fmt && cargo test --lib
test result: ok. 422 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.22s
```

`git diff --check`:
```
$ git diff --check
(no output, exit 0)
```

## Commit

SHA: `1ecceb485d57217ecd4f6b9ed955ef97cc5d9804` (short `1ecceb4`)
Branch: `feat/llm-connection`

```
feat(profile): declare headless commands and API endpoints on profiles

The provider registry lives on profiles/*.toml rather than beside it, so
there is never a second vendor list to keep in sync.

Only claude and codex get a [headless] block; opencode and qwen have never
been run non-interactively here, and inventing a command would create a path
that errors the moment a user takes it.
```

Files in the commit: `profiles/claude.toml`, `profiles/codex.toml`, `profiles/deepseek.toml`,
`profiles/glm.toml`, `profiles/kimi.toml`, `profiles/qwen-api.toml`, `src/daemon.rs`,
`src/profile.rs`, `src/session.rs`. 149 insertions, 0 deletions.

Note: `.superpowers/sdd/.gitignore` had a pre-existing uncommitted modification in this worktree
before I started (unrelated to this task — looked like leftover state from an sdd-workspace
script run). I left it untouched and unstaged; it is not part of this commit.

## base_url values copied verbatim

- `deepseek.toml`: existing `[env] ANTHROPIC_BASE_URL = "https://api.deepseek.com/anthropic"`
  → copied into `[api] base_url` unchanged.
- `qwen-api.toml`: existing
  `[env] ANTHROPIC_BASE_URL = "https://dashscope.aliyuncs.com/api/v2/apps/claude-code-proxy"`
  → copied into `[api] base_url` unchanged.

Both were read directly from the files with `cat` before typing, not from memory. The test
`the_api_base_url_matches_the_env_base_url` (which asserts `p.api.base_url == p.env["ANTHROPIC_BASE_URL"]`
for all four API-shaped profiles) passed, confirming the copies are exact for kimi, glm, deepseek,
and qwen-api.

## Self-review

- Struct/enum names, derives, field names, and doc comments match the brief character-for-character
  (I diffed my insertion against the brief text while writing it).
- `Wire` uses `#[serde(rename_all = "lowercase")]` as specified, and the TOML files write
  `wire = "anthropic"` (lowercase) accordingly — parses correctly, confirmed by the passing tests.
- Confirmed by grep that `opencode.toml` and `qwen.toml` have zero occurrences of `[headless]` or
  `[api]` after my edits — I only touched the six files named in the brief.
- The two `fake_agent()` fixture edits are the only place I deviated from "files to modify" in the
  brief; they're mechanical (`None` for two new `Option` fields, matching the pattern already used
  for `secret`/`install` in the same literals) and necessary for the crate to compile at all. No
  other logic, formatting, or unrelated code was touched.
- Did not touch `src/config.rs` or anything from task 1, per the instructions that task 2 doesn't
  depend on it.
