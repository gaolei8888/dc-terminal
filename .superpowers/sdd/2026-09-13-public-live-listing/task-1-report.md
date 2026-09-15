# Task 1 Report: `dct-link` — public-live constants and the publish grant function

## Summary

Implemented exactly what the brief specified in `crates/dct-link/src/live.rs` and
`crates/dct-link/Cargo.toml`:

- `pub const MAX_PUBLIC_TITLE_CHARS: usize = 60;`
- `pub const RESERVED_LIVE_ID: &str = "public";`
- `pub const PATH_PUBLIC_LIST: &str = "/live/public";`
- `pub const PUBLISH_KEYS_RELOAD: Duration = Duration::from_secs(10);`
- `pub fn public_path(id: &str) -> String` → `"{LIVE_PREFIX}/{id}/public"`
- `pub fn push_hash(push_secret: &str) -> [u8; 32]` (SHA-256 via `sha2`)
- `pub fn publish_grant(push_hash: &[u8; 32], id: &str) -> String` (lowercase hex, 64 chars,
  HMAC-SHA256 via `hmac`, message `"publish:" + id`)

Added dependencies `sha2 = "0.10"` and `hmac = "0.12"` to `crates/dct-link/Cargo.toml`, with the
Chinese doc comment from the brief kept verbatim.

## Source check against real code (plan not authoritative)

Read `crates/dct-link/src/live.rs` before writing anything. Everything the brief referenced
matched the real source exactly:

- `MAX_LANE_NAME_CHARS` exists at the point the brief says to insert after.
- `LIVE_PREFIX = "/live"` exists and is what `public_path` should build on (matches the existing
  pattern used by `frame_path`/`page_path`).
- `use std::time::Duration;` is already imported at the top of the file.
- The `#[cfg(test)] mod tests { use super::*; ... }` block exists with the exact structure the
  brief assumes.

No discrepancies found — the brief's reference code needed no correction for this task.

## Independent verification of the fixed HMAC vectors

Before trusting the brief's test vectors, I computed them independently in Python
(`hashlib.sha256` + `hmac.new(..., hashlib.sha256)`) rather than assuming the brief got them
right:

```
python3 -c "
import hashlib, hmac
key = hashlib.sha256(b'p'*64).digest()
print(hmac.new(key, b'publish:abc', hashlib.sha256).hexdigest())
key2 = hashlib.sha256(b'q'*64).digest()
print(hmac.new(key2, b'publish:abc', hashlib.sha256).hexdigest())
"
```

Output:
```
74df63dc9e0620cba63084d7dbeec29068ecbf8a7ed31168d1c22895a5a10281
64b549e882f6a84b91dd4ffe27594e24667fa314ff7e0a236061114951ad5c79
```

Both match the brief's expected values exactly (each 64 lowercase hex chars, confirmed with
`len()`). The test as given is trustworthy.

## TDD evidence

### RED

Added the two test functions from the brief (`the_publish_grant_matches_a_fixed_vector`,
`the_public_path_is_built_in_exactly_one_place`) to `mod tests` before writing any
implementation. Ran:

```
cargo test -p dct-link
```

Result: compile failure, as expected —

```
error[E0425]: cannot find value `RESERVED_LIVE_ID` in this scope
error[E0425]: cannot find value `PATH_PUBLIC_LIST` in this scope
error[E0425]: cannot find function `push_hash` in this scope
error[E0425]: cannot find function `publish_grant` in this scope
error[E0425]: cannot find function `public_path` in this scope
error: could not compile `dct-link` (lib test) due to 11 previous errors
```

### GREEN

Added the dependencies, ran `cargo build -p dct-link` first (without `--locked`) to update
`Cargo.lock` — crates.io fetch succeeded, not blocked in this sandbox. Then implemented the
constants/functions and ran:

```
cargo test -p dct-link
```

Result: `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`, including
both new tests:

```
test live::tests::the_publish_grant_matches_a_fixed_vector ... ok
test live::tests::the_public_path_is_built_in_exactly_one_place ... ok
```

## Mutation check

Per the brief's Step 6: changed `mac.update(b"publish:")` to `mac.update(b"publish")` (dropped
the `:` separator) via `sed`, backing up the original first.

```
cargo test -p dct-link the_publish_grant_matches_a_fixed_vector
```

Result: FAILED —

```
thread 'live::tests::the_publish_grant_matches_a_fixed_vector' panicked at crates/dct-link/src/live.rs:162:9:
assertion `left == right` failed
  left: "d9e01eb72d5bb2ad86e11f171c8ad2bee74a8b59352e877b83e006c9c3d439fe"
 right: "74df63dc9e0620cba63084d7dbeec29068ecbf8a7ed31168d1c22895a5a10281"
```

Restored the original file from the backup, verified byte-for-byte the `"publish:"` literal was
back, then re-ran (after a `touch` to defeat a stale-mtime cargo cache artifact from the `mv`
restore) — all 13 tests green again.

## Full verification

```
env -u TERM cargo test --workspace --locked --no-fail-fast
```
→ all crates green, notably `dct-link` 13/13, `dct-srv` 48/48 + 3 integration tests, `dct`
1314/1314 (`--locked` succeeded, confirming `Cargo.lock` was correctly updated and committed).

```
cargo clippy --workspace --all-targets -- -D warnings
```
→ clean, no warnings.

Also ran `cargo fmt -p dct-link -- --check`, which found 3 pre-existing-style line-wrap
deviations in the code I just added (HMAC construction line, and two multi-line
`assert_eq!`/`assert_ne!` calls exceeding the configured width). Ran `cargo fmt -p dct-link` to
apply the project's formatting, then re-ran tests and clippy to confirm nothing broke. `git diff
--check` reports no whitespace errors.

## Global-constraints checks relevant to this task

- Credential formula matches exactly: `hex(HMAC-SHA256(key = SHA-256(push_secret), msg =
  "publish:" + id))`.
- `MAX_PUBLIC_TITLE_CHARS = 60`, `PUBLISH_KEYS_RELOAD = 10s`, `RESERVED_LIVE_ID = "public"` all
  match the constants table in `global-constraints.md` verbatim.
- No `Debug` derive was added to anything carrying a secret — `push_hash` returns a plain `[u8;
  32]`, `publish_grant` returns a plain `String`; neither type wraps a struct with a `Debug` impl
  that this task touches.
- Commit message in English, no AI attribution line, matches the brief's exact text.

## Files changed

- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/public-live/crates/dct-link/Cargo.toml`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/public-live/crates/dct-link/src/live.rs`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/public-live/Cargo.lock`

Commit: `63d3c11 feat(link): constants and the publish grant shared by relay and daemon`

## Concerns

None. The brief was accurate against the real source and the fixed vectors checked out against
an independent Python computation. The only deviation from the brief's literal instructions was
running `cargo fmt` afterward to match the crate's existing style — this is additive hygiene, not
a change in behavior, and tests/clippy were re-verified green after it.
