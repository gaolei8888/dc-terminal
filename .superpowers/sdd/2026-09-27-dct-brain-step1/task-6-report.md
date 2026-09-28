# Task 6: 流程批准记录 — Report

## Status: DONE

Commit: `5cb1fa5` feat(brain): signed procedure approvals that carry each step's tier

## Implementation

Created `crates/dct-brain/src/approval.rs` with:
- `APPROVAL_VERSION: u32 = 1`
- `Approval` struct with `canonical_bytes()` and `run_tier()` methods
- `SignedApproval` struct
- `sign_approval()` function (accepts only User role)
- `verify_approval()` function with checks for version, steps_sha256, and role

Added `pub mod approval;` to `crates/dct-brain/src/lib.rs`

## Test Results

All 5 approval tests PASS:
- `canonical_form_is_stable` ✓
- `the_run_needs_the_strictest_steps_signer` ✓
- `only_the_user_can_approve` ✓
- `editing_the_record_or_the_steps_voids_it` ✓
- `an_automatic_key_signature_is_not_an_approval` ✓

Full test suite: **41/41 PASS**

## Clippy Output

```
Checking dct-brain v0.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.08s
```
No warnings.

## Windows Check

```
cargo check --target x86_64-pc-windows-msvc --all-targets -q
```
No dct-brain-related warnings. (Pre-existing warning in student_projects.rs unrelated to this task.)

## Self-Review

✓ Golden value `sha256:4ba1f58585f9df3f798cc0178ea13e16f9c351aa0fc6adcc72a89ac17867432a` matches test expectation
✓ Canonical form follows spec: dct-approval-v1, v, procedure, steps_sha256, body_sha256, tier count, tier names, approved_at
✓ `run_tier()` correctly returns max tier or Read if empty
✓ `sign_approval()` rejects non-User signers with appropriate error message
✓ `verify_approval()` checks version, steps_sha256, and enforces User role
✓ Serialization derives (Serialize, Deserialize) present for both structs
✓ Cloning works for mutation tests (SignedApproval derives Clone)

## Concerns

None. All requirements met, golden value verified, tests passing.

---

## Fix Round 1: Bind Approvals to Specific Procedures

**Commit:** `748c231` fix(brain): bind approvals to specific procedures with WrongProcedure error

### Changes

1. **sign.rs**: Added `VerifyError::WrongProcedure` variant with Display message "这份批准记录是给另一条流程的".

2. **approval.rs**: 
   - Changed `verify_approval()` signature to accept `procedure: &str` parameter
   - Updated check order: version → procedure name → steps_sha256 → signature → role
   - Updated all 5 existing test calls to pass "social:edit-bio"
   - Added new test: `approval_for_one_procedure_does_not_verify_for_another` — verifies cross-procedure attacks are blocked

### Test Results

All 6 approval tests PASS (1 new test added):
- `canonical_form_is_stable` ✓
- `the_run_needs_the_strictest_steps_signer` ✓
- `only_the_user_can_approve` ✓
- `editing_the_record_or_the_steps_voids_it` ✓
- `an_automatic_key_signature_is_not_an_approval` ✓
- `approval_for_one_procedure_does_not_verify_for_another` ✓ (NEW)

Full test suite: **42/42 PASS**

### Clippy Output

```
Checking dct-brain v0.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.95s
```
No warnings.

### Windows Check

```
cargo check --target x86_64-pc-windows-msvc --all-targets -q
```
No dct-brain-related warnings. (Pre-existing warning in student_projects.rs unrelated.)

### Security Impact

Approvals are now cryptographically bound to their specific procedures. An approval for "social:edit-bio" cannot be reused for "social:edit-bio-copy" even if steps are identical, preventing unauthorized procedure substitution attacks.
