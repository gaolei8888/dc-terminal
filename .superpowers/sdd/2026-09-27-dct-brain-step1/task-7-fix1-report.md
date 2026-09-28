# Task 7 fix round 1: `dct keys` review findings

## Status: DONE

Commit: `06399b40f3fe014902f9d634950cb2f93d0bca9d`
("fix(keys): map chip errors honestly, refuse orphaned handle files")

Base: `feat/dct-brain-step1` at `150b103` (Task 7's own commit).

## What changed, mapped to the six rulings

1. **Error status out-param.** `swift/Keys.swift`'s `dct_se_create` and
   `dct_se_sign` now take a trailing `statusOut: UnsafeMutablePointer<Int32>`.
   It is set to `(error as NSError).code` on every path that returns a
   non-zero result from a caught `Error`, and left at 0 for the
   no-enclave/buffer-too-small paths that never construct an `NSError`. The
   Swift extern declarations in `src/keys/mac.rs` grew the matching
   `status_out: *mut i32` parameter; `MacEnclave::create`/`sign` pass a local
   `status` and feed it into the new `status_error(code, status)` mapper.
   Load-failure inside `dct_se_sign` (loading the blob before signing) is
   covered by the same sign-call status, per the brief's "if load is a
   separate step inside sign, the sign status covers it."

2. **Cancel mapping.** Added a private Swift `classify(_:_:)` helper used by
   both `dct_se_create` and `dct_se_sign`'s catch blocks: it returns 3 only
   when the caught error is an `LAError` with code `.userCancel`,
   `.systemCancel`, or `.appCancel`; everything else returns 6 with the
   status set. Added `SignError::Chip(i32)` in
   `crates/dct-brain/src/sign.rs`.

3. **Messages.**
   - `SignError::Chip(status)` displays via a new pure function
     `dct_brain::sign::chip_message(status: i32) -> String`: status
     `-25308` → "安全芯片现在不能用：请在已解锁的 Mac 上、从普通终端窗口运行";
     any other status → "安全芯片出错（代码 N）".
   - `KeyStore::init` failures now go through a new `create_failed_message`
     in `src/keys/mod.rs` that always prefixes "创建钥匙失败：" (bypassing
     `SignError::Other`'s "签名失败：" Display wording, which is still used
     as-is for the actual `sign()` path, e.g. `dct keys test`).
   - Code 2 (load/handle failure) in `src/keys/mac.rs` now reads "钥匙文件
     打不开（可能已损坏或不属于这台 Mac）", with the status code appended in
     parentheses when nonzero, instead of claiming the file came from
     another computer.

4. **Refuse on orphaned handle file.** `KeyStore::init` now calls a new
   `stray_handles()` before doing anything else: if `auto.se` or `user.se`
   exists but `public.json` does not, it returns an error naming the stray
   file(s) and the directory to check, and creates nothing.

5. **Atomic public.json write.** New `KeyStore::write_public_atomic` writes
   `.public.json.tmp.<pid>` in the same directory, then `rename`s it over
   `public.json`. Permission behaviour is unchanged (plain `std::fs::write`
   on the temp file — public key material, not secret).

6. **Tests** (all in `src/keys/mod.rs` unless noted):
   - `FakeEnclave` grew a `fail_user_create: bool` field; `create()` fails
     with `SignError::Other(...)` when `biometric && fail_user_create`.
   - `a_failed_create_leaves_no_trace_and_a_retry_succeeds`: auto key
     creation succeeds, user key creation fails; asserts no `public.json`,
     no `auto.se`, no `user.se` remain after the failed `init`, then clears
     the flag and asserts a fresh `init` succeeds with two distinct
     `key_id`s and all three files present. This exercises the new rollback
     in `init` (on any error from `create_both`, both blob files are
     removed so the retry doesn't trip the new orphaned-handle guard).
   - `init_refuses_when_a_handle_file_exists_without_public_json`: pre-seeds
     a stray `auto.se`, asserts `init` fails with a message containing
     "auto.se", and that nothing on disk changed.
   - `chip_message_maps_locked_terminal_status_to_an_actionable_sentence`
     (here) and `chip_message_calls_out_the_locked_terminal_case_by_name`
     (in `crates/dct-brain/src/sign.rs`, alongside the other `sign` tests)
     both check the pure `chip_message` function for `-25308` and a generic
     code.

## Verification

- `~/.cargo/bin/cargo build -q` — succeeds (Swift compiles into
  `libDctMac.a` and links).
- `~/.cargo/bin/cargo test --workspace` (via `env GOCACHE=/tmp/dcwb-go-cache`)
  — full workspace green, exit code 0. Targeted re-runs for close
  inspection: `cargo test --lib keys::` → 23 passed (7 in `keys::tests`, all
  new/changed ones passing: `chip_message_maps_locked_terminal_...`,
  `init_refuses_when_a_handle_file_exists_without_public_json`,
  `a_failed_create_leaves_no_trace_and_a_retry_succeeds`,
  `no_keys_yet_means_no_signer`, `a_cancelled_touch_id_is_reported_as_cancelled`,
  `init_creates_both_keys_once`,
  `signers_sign_with_the_right_role_and_verify_against_the_public_file`);
  `cargo test -p dct-brain chip_message` → 1 passed.
- `~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings` —
  clean, exit 0.
- `~/.cargo/bin/cargo fmt --check` — **fails**, but this is a pre-existing
  condition, not something this change introduced: `git stash` back to
  HEAD (`150b103`, Task 7's own unmodified commit) and running the same
  check fails identically (exit 1, ~1923 lines of diff across nearly every
  source file in the repo, including files this task never touches, e.g.
  `src/daemon.rs`, `src/live.rs`, `src/pty.rs`). The repo has no
  `rustfmt.toml`/`.rustfmt.toml` at any point in its history, and its
  existing code (including the rest of Task 7's own untouched lines in the
  same files) is hand-formatted denser than stable rustfmt's 100-column
  default — e.g. the original `extern "C" { fn dct_se_create(...) -> i32; }`
  one-liners, or `PublicEntry { key_id: ..., public_key: ... }` struct
  literals. I did not run a repo-wide `cargo fmt -w`, since that would
  reformat dozens of unrelated files/functions untouched by this task and
  blow up the diff far outside its scope. My new/changed lines follow the
  same ambient hand-formatted style as the surrounding code they sit next
  to, not stable rustfmt's output.
- `git diff --check` — clean, exit 0.

## Not done / explicitly out of scope

- Did not run `dct keys init` for real (per instructions — it needs a
  normal Terminal session and would fail here with -25308, which is
  exactly the case this fix now reports with an actionable message instead
  of a bare "签名失败：安全芯片返回 5").
- Did not touch `devices/esp32-screen/` (untracked, unrelated) — confirmed
  not staged or committed.
- Did not address the pre-existing `cargo fmt --check` failure (see above).

## Concerns

None beyond the pre-existing fmt-check condition noted above, which is
informational, not a regression from this change.
