# Task 7 report: `dct keys` — Mac Secure Enclave, an automatic key and a Touch ID key

## What was done

Implemented exactly what the brief specified, using the brief's own code verbatim for
`build.rs`, `swift/Keys.swift`, `src/keys/mac.rs`, and `src/keys/mod.rs`, with one deliberate
deviation (see below).

Files:
- Created `build.rs` (root, macOS-only Swift compile, no-op elsewhere)
- Created `swift/Keys.swift` (CryptoKit/LocalAuthentication/Security FFI surface)
- Created `src/keys/mod.rs` (`SecureEnclave` trait, `platform_enclave()`, `KeyStore`,
  `PublicKeys`/`PublicEntry`, `EnclaveSigner`, `run_cli`, plus the brief's test module)
- Created `src/keys/mac.rs` (extern "C" bindings to the Swift library)
- Modified `src/lib.rs`: added `pub mod keys;` in alphabetical position
- Modified `src/main.rs`: added `Some("keys") => std::process::exit(dct::keys::run_cli(&args[1..])),`
  immediately before the `Some("llm")` arm
- Modified `Cargo.toml`: added `base64 = "0.22"` to `[dependencies]` (it was not previously a
  direct dependency of the `dct` package, only transitive via `dct-brain`/`ureq`); `anyhow` and
  `serde_json` were already present
- Modified `Cargo.lock` (adds `base64` as a direct dependency edge for `dct`, no version change)
- Modified `crates/dct-brain/src/sign.rs`: added `make_signature_for_tests`, gated
  `#[cfg(any(test, feature = "soft-signer"))]`, placed above the `soft` module as specified

## Deviation from the brief

Per the controller's ruling (given as a known fact, not discovered independently): gated
`struct NoEnclave` and its `impl SecureEnclave for NoEnclave` with
`#[cfg(not(target_os = "macos"))]` in `src/keys/mod.rs`, since on macOS `platform_enclave()`
never constructs it and it would otherwise be dead code, failing
`cargo clippy --workspace --all-targets -- -D warnings` on the Mac. No other deviations.

## TDD evidence

Step 1/2: wrote `src/keys/mod.rs` with only `use dct_brain::tier::SignerRole;` and the brief's
`#[cfg(test)] mod tests` block (referencing `SecureEnclave`, `SignError`, `KeyStore`, which did
not exist yet), added `pub mod keys;` to `src/lib.rs`, then ran:

```
cargo test --lib keys::
```

Result: compile failure, 9 errors, all `cannot find type 'SignError'/'KeyStore' in this scope`
— confirming the tests fail to even compile before implementation, as expected.

Step 3/4: added the real implementation (trait, `KeyStore`, `EnclaveSigner`, `run_cli`) above the
test module, added `swift/Keys.swift`, `build.rs`, `src/keys/mac.rs`.

Step 5: ran `cargo test --lib keys::` again:

```
test keys::tests::no_keys_yet_means_no_signer ... ok
test keys::tests::a_cancelled_touch_id_is_reported_as_cancelled ... ok
test keys::tests::init_creates_both_keys_once ... ok
test keys::tests::signers_sign_with_the_right_role_and_verify_against_the_public_file ... ok
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 1349 filtered out
```
(the other 16 of the 20 are pre-existing, unrelated `ui::keys::tests` / `web::keys::tests`
modules matched by the same `keys::` filter substring — all 4 new `src::keys` tests pass).

## Step 5 check outputs

```
cargo test --workspace -q
```
All green: `dct` lib 1369 passed; every other crate (dct-link, dct-page, dct-srv, dct-brain,
integration test binaries) passed; 0 failed across the whole run (exit code 0).

```
cargo clippy --workspace --all-targets -q -- -D warnings
```
Clean, no output, exit code 0.

```
cargo check --target x86_64-pc-windows-msvc --all-targets -q
```
Exit code 0. One warning, pre-existing and unrelated to this change:
`warning: unused variable: 'path'` at `src/student_projects.rs:669` (`fn sync_dir(path: &Path)`).
Not touched by this task; `cargo check` (not `-D warnings`) doesn't fail on it.

Also ran `git diff --check` (exit 0, no whitespace errors) before committing.

## Step 6: real Secure Enclave on this Mac

```
$ cargo build -q
(succeeds — build.rs compiled swift/Keys.swift into libDctMac.a and linked it)

$ ./target/debug/dct keys init
签名失败：安全芯片返回 5
(exit code 1)

$ ./target/debug/dct keys show
还没建钥匙，先运行 dct keys init
(exit code 1)
```

`init` failed. Per the task instructions I did not work around this, but I did add temporary
diagnostic-only instrumentation (a standalone Swift script run with `xcrun swift`, never part of
the committed code, deleted afterward) to see what the generic "5" was hiding, since the brief's
`dct_se_create` collapses every non-Cancelled Swift error to status 5. That probe called the
exact same CryptoKit API the shipped code calls:

```
isAvailable: true
create failed: Error Domain=NSOSStatusErrorDomain Code=-25308
  "<sepk:* kid=0000000000000000>: unable to generate key"
  UserInfo={NSDebugDescription=..., AKSError=-536870174}
localizedDescription: The operation couldn't be completed. (OSStatus error -25308.)
```

-25308 is `errSecInteractionNotAllowed`. This machine is a real Apple Silicon Mac with a Secure
Enclave (`Mac16,13`, macOS 26/"27.0", `system_profiler SPiBridgeDataType` shows a real Secure
Enclave controller) — `SecureEnclave.isAvailable` correctly returns `true`. The failure is not
"no Secure Enclave"; it's the OS refusing key generation for this process's session (this shell
runs under the Claude Code agent harness, not a normal interactive Terminal.app/login-session
context with a keychain/AKS session attached to it). `~/.dct/keys` was empty before this run and
still is — `init` created nothing, consistent with "never overwrites, and here it never
succeeded to begin with."

I did not attempt further workarounds (e.g., re-running through `osascript`/Terminal.app to get
a different session context) since that's outside this task's scope and risks masking a real
issue. The user should run `./target/debug/dct keys init` themselves from a normal Terminal
window; if it succeeds there, the code is correct and the failure above is purely an artifact of
the harness's process/session. `dct keys test` was not run (per instructions — it needs a real
Touch ID prompt).

## Self-review

- `SecureEnclave` trait, `KeyStore`, `EnclaveSigner`, `run_cli` match the brief's interface list
  exactly (`available`, `create`, `sign`; `at`, `default_dir`, `init`, `public_keys`, `trusted`,
  `signer`).
- `KeyStore::default_dir()` derives from `crate::proto::socket_path()`'s parent (`~/.dct`) +
  `keys`, matching "跟 daemon.sock 同一个目录下".
- Blob files are written via `crate::sys::fs::create_private`, so owner-only from creation (no
  window). `public.json` is a plain `std::fs::write` (public key material, not secret), per spec.
- `init` is idempotent: returns the existing `PublicKeys` unchanged if `public_keys()` already
  finds a file, never regenerating — verified by `init_creates_both_keys_once`'s second call
  equality assertion.
- `verify_sig` in `dct-brain` refuses an ambiguous key_id (`VerifyError::AmbiguousKey`); auto and
  user get distinct SEC1 public keys from two separate `se.create()` calls, so this never
  triggers here — confirmed by `signers_sign_with_the_right_role_and_verify_against_the_public_file`.
- `NoEnclave` is `cfg`-gated off macOS only, so it compiles on Linux/Windows targets and is dead
  code on macOS (where `mac::MacEnclave` is used instead) — this is exactly the controller's
  ruling, and clippy under `-D warnings` on this Mac passing confirms it worked.
- `EnclaveSigner::sign` forwards `reason` straight to the enclave; on non-macOS `NoEnclave::sign`
  always returns `Unavailable`, so `content`/`money` tier tickets can't be signed at all off Mac
  — matches the design note "对外和动钱两档直接关闭，不退化成「点一下同意」".
  Windows/Linux is out of scope for the Touch ID key entirely for now (only macOS has the real
  enclave binding); that's expected per the brief.
- `dct_se_sign`'s `reason` is passed as a `CString` with embedded NULs stripped in Rust before
  crossing the FFI boundary, avoiding an `unwrap()` panic on malicious/accidental NUL bytes in a
  user-supplied reason string.
- Diffed `Cargo.toml`/`Cargo.lock`: only `base64` added as a new direct edge; no other dependency
  changed versions.
- `git diff --check`: clean. Did not touch `devices/esp32-screen/` or `.superpowers/` in the
  commit (only this report file was written under `.superpowers/`, as instructed, and it is not
  part of the git commit).

## Concerns

1. **The known design gap the brief calls out**: the automatic key's blob (`auto.se`) is
   file-permission-protected only, not keychain-access-group-protected. Any process running as
   the same OS user can read it and forge `self`/`physical` tier tickets. This requires an Apple
   developer certificate + provisioning profile to fix (out of scope here, per the brief). The
   user key is unaffected — Touch ID is required at sign time regardless of who holds the blob.
2. **Step 6 did not succeed** on this machine due to `errSecInteractionNotAllowed` (-25308), which
   looks like a session/process-context limitation of the harness this agent runs under, not a
   hardware or code defect. The user needs to personally run
   `./target/debug/dct keys init && ./target/debug/dct keys show && ./target/debug/dct keys test`
   from a normal Terminal window to get real confirmation (and to answer the Touch ID prompt for
   `test`, which this agent was told not to run in any case).
3. Windows/Linux builds never construct a working Touch ID/enclave signer at all (`NoEnclave`
   always `Unavailable`) — this is intended per the brief's design ("没有 passkey 的平台上，对外和
   动钱两档直接关闭"), just flagging it as a real, expected limitation rather than a bug.
