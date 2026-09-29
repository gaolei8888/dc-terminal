# Task 1 report — dct-mesh crate

## What was built

New pure-logic crate `crates/dct-mesh`, added to the workspace and as a root `dct` dependency (Cargo.toml unused today but wired for later tasks, matching how `dct-brain` was added).

- `src/canon.rs` — `field()`, the same length-prefixed encoding as `dct-brain::canon::field`. Copied rather than shared, since the crate must not depend on `dct-brain` and it's a few lines.
- `src/id.rs` — `endpoint_for(sign_pub: &[u8; 65]) -> String` (`"c-"` + first 10 bytes of SHA-256 as lowercase hex, 22 chars total, fits `dct-link::EndpointId`'s `[A-Za-z0-9_.:-]` / 64-char rules without depending on `dct-link`); `Address { machine, session }` and `Address::parse` / `AddrError`.
- `src/keys.rs` — `MachineKeys::from_seeds`, `sign_pub`, `kx_pub`, `sign`, and free function `verify`. `KeyError::BadSeed` for a bad P-256 seed (e.g. all-zero).
- `src/sas.rs` — `sas::code(a: &Member, b: &Member) -> String`, the 6-digit compare code, `dct-sas-v1` canonical form.
- `src/roster.rs` — `Member`, `Roster` (+ `member`/`by_name`), `SignedRoster`, `bytes()`, `sign()`, `genesis()`, `accept()`, `RosterError`.
- `src/lib.rs` — module wiring plus a small `pub use` re-export set (`Address`, `AddrError`, `KeyError`, `MachineKeys`, `Member`, `Roster`, `RosterError`, `SignedRoster`) for ergonomic `dct_mesh::X` access; every interface point the brief specified is still reachable as `dct_mesh::<module>::<item>` too.

No network, disk, env, or process access anywhere in the crate; every dependency in `Cargo.toml` is pure Rust (p256, x25519-dalek, chacha20poly1305, hkdf, sha2, base64, serde/serde_json — chacha20poly1305 and hkdf aren't used by any Task-1 code yet; they're in the Cargo.toml verbatim per the brief for later tasks in this plan, same pattern dct already uses for keeping the C-free property explicit).

## Tests (TDD)

Wrote all the tests from the brief's Step 2 first, ran `cargo test -p dct-mesh` to confirm they failed to compile (no crate existed yet), then implemented. Added several extra tests beyond the brief's list to make the suite mutation-resistant:
- `keys.rs`: cross-key/cross-message negative verification, SEC1 shape check, distinct kx keys, bad-seed rejection, and a garbage-input fail-closed check.
- `id.rs`: determinism/distinctness of `endpoint_for`, and the full accept-rule set for `Address::parse`.
- `sas.rs`: a zero-padding check (loops over 256 candidate keys to find one whose code needs a leading zero, asserting the string is still 6 chars) — the brief's own padding claim ("补零到 6 位") had no test that would catch a missing `:06`.
- `roster.rs`: canonical-byte member-order independence, and `Roster::member`/`by_name` lookups.

Final run: **25/25 passed**, including the exact fixed vector `"280288"` from the brief on the very first compile of `sas::code` — confirms both the encoding and the reference algorithm are correct as given.

## Commands run

```
~/.cargo/bin/cargo test -p dct-mesh                                              # 25 passed; 0 failed
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings               # clean, no warnings
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
  # clean for dct-mesh; one PRE-EXISTING warning in src/student_projects.rs
  # (`unused variable: path` in sync_dir, windows-only cfg branch) — not
  # touched by this task, confirmed unrelated by `git status`/`git diff`
  # scope (dct-mesh files only).
~/.cargo/bin/cargo fmt -p dct-mesh                                               # applied, then --check passed
git diff --check                                                                  # no whitespace errors
```

## Deviations from the brief's reference code (and why)

1. **`keys.rs` signing-key construction**: brief said `SigningKey::from_bytes(&sign_seed.into())`. `[u8; 32]` does not implement `Into<FieldBytes>` (`GenericArray<u8, U32>`) in the `generic-array`/`p256` versions pinned here, so that line does not compile. Used `SigningKey::from_slice(&sign_seed)` instead — same fallible behavior (`Result`, errors on a non-canonical/zero scalar), and it's exactly what `dct-brain::sign::soft::SoftSigner::from_seed` already does in this repo, so it's the locally-idiomatic form too.
2. **`x25519_dalek::StaticSecret::from(kx_seed)`**: verified correct as written in x25519-dalek 2.0.1 (`From<[u8; 32]>` clamps the scalar; infallible, matching `KeyError` having only `BadSeed` for the *signing* seed).
3. **`roster::SignedRoster.signer`**: the brief's interface list types it as `String` but doesn't say whether it's the signer's name or endpoint. Chose **endpoint** (the crate's canonical, collision-resistant identifier, consistent with `Roster::member(endpoint)` being the primary lookup and with how `id::endpoint_for` is described as the machine's identity) and documented the choice in the doc-comment above `sign()`.
4. **Added `RosterError::SignerRemoved`**, not listed among the brief's six bullet `accept` rules but required to make the brief's own test name meaningful: `the_signer_may_not_be_removed_in_the_same_version_it_signs`, annotated in the brief with "签名者必须在上一版名单里". Read literally, the six listed rules only require the signer to be *in `current`* — they don't forbid a version that removes the signer while they're the one signing it. Implemented the literal test-name rule as an extra defense-in-depth check in `accept()`: the signer must still be a member of `incoming.roster` (self-exit must be signed by someone else), matching task-6's CLI rule "不能移除自己" being enforced at the protocol/crate layer too, not just the CLI. Flagging this because it's a judgment call filling a gap between the brief's rule list and its own test name — worth the plan owner double-checking against task 6's `MeshRemove` flow.
5. `keys::b64` — considered adding a small base64 helper, removed it again since nothing outside tests needed it and the brief's interface list doesn't mention it; kept the crate's public surface exactly matching the brief instead.

## Concerns

- Point 4 above (`SignerRemoved`) is an interpretive addition beyond the brief's explicit `accept` rule list. If task 6 or a later ruling intends self-removal-while-signing to be *allowed* (e.g. a member voluntarily leaving in a version it signs itself), this rule needs to be relaxed or removed; nothing else in dct-mesh depends on it.
- `chacha20poly1305` and `hkdf` are unused today (no code references them) since no Task-1 interface needs session encryption yet; that's expected per the brief's own Cargo.toml, just noting it so it isn't mistaken for a leftover.

## Fix round 1 (code review response)

Coordinator review of `7fb7cd4` asked for five things, all addressed in `f21a74d`:

**I1 — endpoint must be bound to the signing key.** `accept()` previously only checked that a member's `sign_pub`/`kx_pub` *decode*; it never checked that `member.endpoint` actually equals `id::endpoint_for(&sign_pub)`. That meant a signed roster (genesis or a later version) could claim any endpoint string for a member regardless of what key that endpoint's owner actually holds — the SAS 6-digit code is computed from the claimed keys, so a forged binding wouldn't even show up as a mismatch there. Fixed by adding a new `validate_members()` step, run for every member on every `accept()` call (genesis and later versions alike), that decodes `sign_pub`/`kx_pub` and requires `member.endpoint == id::endpoint_for(&sign_pub)`, erroring `BadKey` otherwise.

**I2 — mutation-survivor tests.** Added 15 new tests in `roster.rs`:
- `duplicate_names_with_different_endpoints_are_refused` — kills removal of the *name* half of `has_duplicates`'s `||` (the pre-existing duplicate test only varied endpoint, so a mutant dropping the name check would have survived).
- `a_sign_pub_of_the_wrong_length_is_a_bad_key`, `a_sign_pub_without_the_0x04_prefix_is_a_bad_key`, `a_sign_pub_that_is_not_on_the_curve_is_a_bad_key`, `a_kx_pub_of_the_wrong_length_is_a_bad_key` — cover every way `decode_sign_pub`/`decode_kx_pub` can fail. The "not on the curve" case required a real fix, not just a test: `decode_sign_pub` previously only checked length and the `0x04` prefix, so a syntactically-shaped-but-off-curve "key" would have decoded as `Some(..)`; if it belonged to a non-signer member it would never have been rejected at all (only a signer's off-curve key would eventually surface, and only as `BadSignature` from `p256`'s own parse failure inside `keys::verify`). Now `decode_sign_pub` calls `p256::ecdsa::VerifyingKey::from_sec1_bytes` for real and rejects on failure, caught as `BadKey`.
- `a_signature_with_invalid_base64_is_refused`, `a_signature_of_the_wrong_length_is_refused` — cover `decode_sig` failure modes (were untested).
- `a_genesis_roster_must_be_version_1`, `a_genesis_roster_must_have_exactly_one_member`, `a_genesis_roster_signer_field_must_equal_its_member` — each isolates one condition of the `current == None` three-part check (version, member count, signer identity), which previously only had one combined "happy path" test.
- `a_non_member_cannot_add_itself_and_self_sign` — a non-member adding itself into the incoming member list and self-signing; still caught by `UnknownSigner` (looked up against `current`, not `incoming`), now with an explicit test for that specific attack shape.
- `a_members_endpoint_must_match_its_own_signing_key`, `a_new_members_endpoint_must_match_its_own_signing_key_in_a_later_version` — the direct I1 regression tests, for both the genesis and later-version paths.

**M1 — doc comment.** Expanded the doc comment on `accept()` to state explicitly that `current = None` is only for a genesis roster this machine created itself, and that a joining machine's first roster goes through a separate `accept_invite` (a later task), not this path.

**M2 — member name validation.** Added `validate_name()` (non-empty, no `/`, at most `MAX_NAME_LEN` = 32 chars) called from `validate_members()`, plus `RosterError::BadName`. Tests: `an_empty_member_name_is_refused`, `a_member_name_with_a_slash_is_refused`, `a_member_name_over_32_chars_is_refused`, `a_member_name_of_exactly_the_max_length_is_accepted`.

Also reordered `accept()`'s checks so `validate_members` (name/key/endpoint-binding) and the duplicate check run before the genesis/current-version-specific logic — this keeps `BadKey`/`BadName`/`Duplicate` failures reported precisely even when the roster would otherwise also fail a later check, which matters for the new tests to actually exercise the code path they claim to.

### Commands and output

```
~/.cargo/bin/cargo test -p dct-mesh
# test result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

~/.cargo/bin/cargo fmt -p dct-mesh   # applied
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
# Finished `dev` profile [unoptimized + debuginfo] target(s) — no warnings

~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
# Finished `dev` profile [unoptimized + debuginfo] target(s)
# (one pre-existing, unrelated warning in src/student_projects.rs::sync_dir,
# a windows-only cfg branch not touched by this task)

git diff --check
# no whitespace errors
```

Commit: `f21a74d` — "fix(mesh): bind roster endpoints to signing keys, validate names, close mutation gaps"

## Fix round 2 (re-review response)

Re-review of `f21a74d` found two of the round-1 mutation-coverage tests didn't actually isolate what they claimed to.

**1. Off-curve `sign_pub` test didn't isolate the curve check.** The round-1 test set `a.sign_pub` to an off-curve (but correctly shaped) 65-byte value while leaving `a.endpoint` as the endpoint of `ka`'s real key. So the roster failed the I1 endpoint-binding check (`endpoint != id::endpoint_for(sign_pub)`) regardless of whether the curve check in `decode_sign_pub` ran at all — a mutant deleting `p256::ecdsa::VerifyingKey::from_sec1_bytes(&raw).ok()?;` would still leave the test green. Fixed by setting `a.endpoint = id::endpoint_for(&off_curve)` too, so the binding check passes and only the curve parse can reject it. Also added `decode_sign_pub_rejects_wrong_length_missing_prefix_and_off_curve_points`, a direct unit test on the private decoder (wrong length, wrong prefix, off-curve, and a real-key round trip), independent of `accept`'s call graph entirely.

**2. `decode_sig`'s length check had no test that isolated it from `keys::verify`.** The existing `a_signature_of_the_wrong_length_is_refused` test only asserted `accept(...)` returns `Err(BadSignature)` for a 63-byte "signature" — but a mutant that guts `decode_sig` (e.g. replaces its body with a constant `Some([0; 64])`, ignoring the input) would still make `accept` fail with `BadSignature`, just via a different route: an all-zero (or any bogus) 64-byte value is never a valid ECDSA signature, so `keys::verify` fails it downstream regardless of whether the length check ever ran. Added `decode_sig_rejects_anything_that_is_not_exactly_64_bytes`, a direct unit test on the private decoder covering 63/64/65-byte inputs and invalid base64, using a non-zero fill byte (`7u8`) specifically so a constant-valued mutant is caught even on the *valid*-length case (`Some([0;64]) != Some([7;64])`).

### Mutation verification (scratch copies, not the repo)

Per the coordinator's instruction, verified both fixes by hand-mutating a **scratch copy** of the crate (under the session scratchpad, never inside the repo) and re-running `cargo test` there, then discarding the copies.

**Mutant A — delete the curve-validity parse in `decode_sign_pub`:**
```
- p256::ecdsa::VerifyingKey::from_sec1_bytes(&raw).ok()?;
```
Result in the scratch copy: 2 failed, 42 passed —
```
roster::tests::a_sign_pub_that_is_not_on_the_curve_is_a_bad_key
  left: Err(BadSignature)  right: Err(BadKey)
roster::tests::decode_sign_pub_rejects_wrong_length_missing_prefix_and_off_curve_points
  left: Some([4, 4, ..., 4])  right: None
```
Both new/rebuilt tests catch it, exactly as predicted (the accept-level test degrades to `BadSignature` since the bogus "key" now reaches `keys::verify`, which does its own — unmutated — curve parse and fails there instead).

**Mutant B — gut `decode_sig`, two variants tried:**

- *Whole body replaced with a constant* (`Some([0u8; 64])`, ignoring input): 5 tests failed in the scratch copy, including `decode_sig_rejects_anything_that_is_not_exactly_64_bytes` (`Some([0,0,...]) != Some([7,7,...])`) and several previously-`Ok(())` tests that now got `Err(BadSignature)` (since every real signature got replaced by the same invalid dummy).
- *Just the length-check condition deleted*, keeping `copy_from_slice`: this panics for non-64-byte input (`copy_from_slice: source slice length (63) does not match destination slice length (64)`), which `cargo test` reports as a failed test — `decode_sig_rejects_anything_that_is_not_exactly_64_bytes` fails (as a panic) while the unrelated `decode_sign_pub` test still passes, confirming the failure is attributable to `decode_sig` specifically.

Both mutation shapes for `decode_sig` are killed by the new direct unit test either way.

### Commands and output (real repo, after the fix)

```
~/.cargo/bin/cargo fmt -p dct-mesh
~/.cargo/bin/cargo test -p dct-mesh
# test result: ok. 44 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
# Finished `dev` profile [unoptimized + debuginfo] target(s) — no warnings

~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
# Finished — same one pre-existing, unrelated warning in src/student_projects.rs::sync_dir

git diff --check
# no whitespace errors
```

Commit: `2c888d2` — "test(mesh): isolate two decode mutants the previous round missed"
