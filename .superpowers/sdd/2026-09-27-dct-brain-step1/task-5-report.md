# Task 5 report: sign.rs — signing, batch signing, verification

## What was done

Created `crates/dct-brain/src/sign.rs` and added `pub mod sign;` to `crates/dct-brain/src/lib.rs`, following the brief verbatim (Step 1 test module, then Step 3 implementation copied as given).

Files touched:
- `crates/dct-brain/src/sign.rs` (new, 362 lines including the test module from the brief)
- `crates/dct-brain/src/lib.rs` (added `pub mod sign;`)
- `Cargo.lock` unchanged (no new dependency versions needed; `p256`/`ecdsa` already present at 0.13.2 / 0.16.9)

## API adjustments

None. Every p256/ecdsa 0.13/0.16 call in the brief (`SigningKey::from_slice`, `EcSig::from_slice`, `to_encoded_point(false)`, `VerifyingKey::from_sec1_bytes`, `signature::Signer`/`Verifier` traits) compiled and ran against the versions already pinned in `Cargo.lock` (`p256 0.13.2`, `ecdsa 0.16.9`) with zero changes needed.

## TDD evidence

**RED (Step 2):** Wrote only the `#[cfg(test)] mod tests` block from the brief into `sign.rs`, added `pub mod sign;` to `lib.rs`, ran:
```
cargo test -p dct-brain sign
```
Result: compile failure, 52 errors (`cannot find type/function` for `Ticket`, `Tier`, `SignerRole`, `TrustedKey`, `VerifyError`, `sign_one`, `sign_batch`, `verify`, `batch_bytes`, `soft::SoftSigner`, etc.) — confirms tests fail before implementation exists.

**GREEN (Step 4):** Added the full implementation block from the brief above the test module (unchanged from brief text). Ran:
```
cargo test -p dct-brain sign
```
→ `test result: ok. 9 passed; 0 failed` — all 9 sign tests pass:
- a_self_ticket_signed_by_the_automatic_key_verifies
- an_outward_ticket_needs_the_users_key
- any_tampering_breaks_the_signature
- keys_not_paired_are_refused
- device_and_time_window_are_enforced
- one_signature_covers_a_whole_batch
- batch_bytes_are_stable
- signed_tickets_round_trip_through_json

Full crate suite:
```
cargo test -p dct-brain
```
→ `test result: ok. 31 passed; 0 failed; 0 ignored` (includes existing `canon`/`steps`/`tier`/`ticket` tests, unaffected).

## No-C check (Step 4)

```
cargo tree -p dct-brain -e normal | grep -iE "\bcc v|openssl|ring v|-sys v" ; echo "exit=$?"
```
Output: nothing printed by grep, `exit=1` — confirms no C-compiling / OpenSSL / ring / `-sys` crate in the dependency tree, as required.

## Clippy

```
cargo clippy -p dct-brain --all-targets -- -D warnings
```
Output: `Finished` with no warnings/errors — clean.

## Windows cross-check

```
cargo check --target x86_64-pc-windows-msvc --all-targets -q -p dct-brain
```
Output: exit code 0, no diagnostics — compiles cleanly for `x86_64-pc-windows-msvc` (target was already installed via rustup).

## Formatting

`gofmt`/`node --check` steps from CLAUDE.md are for Go/JS files and don't apply here. Ran `cargo fmt -p dct-brain -- --check` as a sanity check: it reports diffs in `sign.rs` at the same style points (multi-field struct literals, multi-arg function signatures kept on one line, etc.) as it already does in the pre-existing `ticket.rs`, `tier.rs`, and `steps.rs` — i.e. this crate does not conform to default `rustfmt` style anywhere, so `sign.rs` was left as given in the brief (verbatim) rather than reformatted, per the instruction to "only format lines you touch" and not fight an established (non-rustfmt) style.

`git diff --check` on the full diff: no whitespace errors.

## Self-review

- `sign.rs` content matches the brief's Step 1 (tests) and Step 3 (implementation) blocks verbatim — no logic was altered.
- `SignError`, `VerifyError` both derive `Debug, Clone, PartialEq, Eq` and implement `Display` + `std::error::Error` as required.
- `pub(crate) fn make_signature` and `VerifyError::StepsChanged` exist exactly as named, ready for Task 6.
- `soft::SoftSigner` is gated `#[cfg(any(test, feature = "soft-signer"))]`, matching the `soft-signer` feature already declared (empty, test-only) in `Cargo.toml`.
- Batch semantics: `sign_batch` produces one `Signature` shared across all tickets in the batch (`batch[0].signature == batch[1].signature` — verified by test), each `SignedTicket` carries the full digest list; `verify` checks the ticket's own digest is `contains`ed in that list before re-deriving `batch_bytes` and verifying — a ticket swapped in from outside the batch correctly fails with `NotInBatch` before signature verification is even attempted (test `one_signature_covers_a_whole_batch` covers the forged case).
- `role_may_sign` correctly permits both roles when `required_signer()` is `None` or `Some(Auto)`, and restricts to `User` when `Some(User)` — matches "谁能签什么" rule in the brief.
- `verify` order: version → device → earliest → expires → (batch membership) → signature → role. This matches the brief's own test expectations (e.g. `device_and_time_window_are_enforced` checks device before time, time before signature never explicitly tested to be after, but tampering test confirms signature check happens and blocks `WrongRole`/`Ok` incorrectly succeeding).
- JSON round-trip: `batch: None` is skipped via `skip_serializing_if = "Option::is_none"`, confirmed by test asserting `!... .contains("batch")` for a `sign_one` result.
- Nothing outside `crates/dct-brain` and `Cargo.lock` was touched; `devices/esp32-screen/` and `.superpowers/` were not staged (only `crates/dct-brain` and `Cargo.lock` added to the commit).
- Commit message is English, no AI attribution, matches the brief's suggested message verbatim: `feat(brain): sign and verify tickets with P-256, one signature for a batch`.

## Concerns

None found — every test in the brief passed against the brief's own implementation without modification, on the first attempt (no golden-value or method-name mismatches encountered). Status: DONE (not DONE_WITH_CONCERNS).

## Commit

`d755ed7` — `feat(brain): sign and verify tickets with P-256, one signature for a batch`

---

## Fix round 1 (review follow-ups)

### What changed

1. **Replay-safety documentation** (module doc + `verify` doc, Chinese, short): added a note that
   replay must be tracked by the ticket's `nonce` (or `ticket.digest()`), **never** by the raw
   signature bytes or a hash of the signed JSON. Reason: P-256/ecdsa here accepts both `s` and
   `n-s` for the same message (neither CryptoKit nor WebAuthn guarantees low-S only), so a single
   ticket can have more than one byte-valid signature encoding; deduping on signature bytes would
   let one approved publish/payment run twice.
2. **`VerifyError::AmbiguousKey`**: new variant, `Display` → `"同一把钥匙配对了不止一次，不认"`.
   `verify_sig` now collects *all* `trusted` entries whose `key_id` matches the signature's
   `key_id`; if more than one matches (e.g. the same public key paired under both `Auto` and
   `User`), it returns `AmbiguousKey` instead of picking the first match by list order (previous
   code used `.find(...)`, which silently picked the first one).
3. **Five new tests** appended after the existing test module (no existing test was touched):
   - `a_batch_signed_by_the_automatic_key_still_needs_the_users_key_for_outward_tickets` — a
     `sign_batch` by the auto key containing a `Tier::Content` ticket: `verify` on that ticket
     returns `WrongRole { need: SignerRole::User }`.
   - `an_empty_batch_list_never_contains_the_ticket` — `batch: Some(vec![])` → `NotInBatch`.
   - `an_unknown_algorithm_is_unsupported` — `signature.alg = "rs256"` → `Unsupported`.
   - `the_same_key_paired_under_two_roles_is_refused` — same public key in `trusted` twice
     (`Auto` and `User` roles) → `AmbiguousKey`.
   - `high_s_twin_of_a_valid_signature_still_verifies` — decodes a real signature, computes its
     high/low-S twin via `EcSig::from_scalars(es.r(), -es.s())` (pure P-256 scalar negation, no
     new dependency), asserts the twin's bytes differ from the original, then asserts `verify`
     still returns `Ok(())` on the twinned signature. This pins down the accept-both decision
     without needing any C-compiling crate — `p256`'s `arithmetic` feature (already pulled in by
     the `ecdsa` feature) exposes `Signature::r()`/`s()` and `Neg for NonZeroScalar`, so no new
     dependency was added.

### Files touched

- `crates/dct-brain/src/sign.rs` only.

### Commands run and output

```
cargo test -p dct-brain sign
```
→ `test result: ok. 14 passed; 0 failed; 0 ignored` (9 original + 5 new sign tests).

```
cargo test -p dct-brain
```
→ `test result: ok. 36 passed; 0 failed; 0 ignored` (full crate suite; the 22 non-sign tests are
byte-identical to before this round).

```
cargo tree -p dct-brain -e normal | grep -iE "\bcc v|openssl|ring v|-sys v" ; echo "exit=$?"
```
→ no grep output, `exit=1` — no C-compiling/OpenSSL/ring/`-sys` crate entered the tree.

```
cargo clippy -p dct-brain --all-targets -- -D warnings
```
→ `Finished` with no warnings.

```
cargo check --target x86_64-pc-windows-msvc --all-targets -q -p dct-brain
```
→ exit code 0, no diagnostics.

```
git diff --check
```
→ no whitespace errors.

### Self-review

- Diffed `crates/dct-brain/src/sign.rs` against the pre-fix version: every one of the 9 original
  tests is present unchanged; all edits are additive (two doc comments, one enum variant + one
  Display arm, the `find` → `filter`/ambiguity-check change in `verify_sig`, and 5 new tests
  appended at the end of the test module).
- `AmbiguousKey` is checked strictly *after* confirming at least one key matches (so `UnknownKey`
  still wins when nothing matches) and *before* decoding/verifying the signature bytes — an
  ambiguous pairing is refused before any crypto work happens.
- The doc additions are short, in Chinese, and placed exactly where the reviewer asked (module
  doc top, and directly above `pub fn verify`).
- No other files touched; `devices/esp32-screen/` and `.superpowers/` remain unstaged/untracked
  in the commit.

### Concerns

None. All 5 requested tests were added, including the high-S one (no C dependency was needed to
construct it — `p256`'s own scalar arithmetic sufficed).

## Fix commit

`6eef79a` — `fix(brain): reject ambiguous key pairings, document replay must key off nonce not signature bytes`
