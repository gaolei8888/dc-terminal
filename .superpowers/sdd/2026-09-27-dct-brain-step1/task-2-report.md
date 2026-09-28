# Task 2 Report: dcv-steps-v1 Step Fingerprint

## Summary
Successfully implemented Task 2 of the dct-brain step-1 plan by adding shared field encoding (`canon.rs`) and the `dcv-steps-v1` procedure-step fingerprint (`steps.rs`), with all test vectors verified against dcv's three cases.

## Files Changed

### Created
1. **crates/dct-brain/src/canon.rs** (16 lines)
   - Implements `field(out: &mut Vec<u8>, s: &str)` for TLV-style encoding
   - Implements `sha256_id(bytes: &[u8]) -> String` for sha256 hashing with "sha256:" prefix
   - Uses sha2 crate (already in Cargo.toml)

2. **crates/dct-brain/src/steps.rs** (75 lines)
   - Defines `Step` struct with fields: `n: u32`, `action: String`, `arg: String`, `done_when: Option<String>`
   - Implements `steps_bytes(params: &[String], steps: &[Step]) -> Vec<u8>` for byte encoding
   - Implements `steps_sha256(params: &[String], steps: &[Step]) -> String` for fingerprinting
   - Includes 4 test cases from dcv (case_a, case_b, case_c, and mutation test)

### Modified
1. **crates/dct-brain/src/lib.rs**
   - Added `pub mod canon;` and `pub mod steps;` declarations

## TDD Evidence

### Step 1: Tests Written
All 4 test cases from the brief were implemented in `steps.rs`:
- `case_a_chinese_and_steps_without_done_marks` - verifies UTF-8 Chinese handling and byte-level encoding
- `case_b_no_params_and_an_empty_arg` - verifies edge case with no params and empty arg
- `case_c_two_params_and_fullwidth_brackets` - verifies fullwidth bracket handling with multiple params
- `any_change_to_a_step_changes_the_fingerprint` - mutation test verifying fingerprint sensitivity

### Step 2-4: Test Execution
```bash
$ cargo test -p dct-brain steps
```
Result:
```
running 4 tests
test steps::tests::case_b_no_params_and_an_empty_arg ... ok
test steps::tests::case_c_two_params_and_fullwidth_brackets ... ok
test steps::tests::case_a_chinese_and_steps_without_done_marks ... ok
test steps::tests::any_change_to_a_step_changes_the_fingerprint ... ok

test result: ok. 4 passed; 0 failed
```

Full test suite:
```bash
$ cargo test -p dct-brain
```
Result:
```
running 7 tests
test steps::tests::case_a_chinese_and_steps_without_done_marks ... ok
test tier::tests::names_match_dcv_and_dco ... ok
test steps::tests::any_change_to_a_step_changes_the_fingerprint ... ok
test steps::tests::case_b_no_params_and_an_empty_arg ... ok
test tier::tests::stricter_tiers_sort_higher ... ok
test steps::tests::case_c_two_params_and_fullwidth_brackets ... ok
test tier::tests::who_signs_each_tier ... ok

test result: ok. 7 passed; 0 failed
```

## Quality Checks

### Clippy
```bash
$ cargo clippy -p dct-brain --all-targets -- -D warnings
Checking dct-brain v0.1.0 (/Users/lei/work/dc/dc-terminal/crates/dct-brain)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.86s
```
**Result: PASS** - No warnings.

### Windows Target Check
```bash
$ cargo check --target x86_64-pc-windows-msvc --all-targets -q
```
**Result: PASS** - No errors or warnings for dct-brain. (One unrelated warning in student_projects.rs which is outside scope.)

## Commit

```
commit 1f959b7
Author: lei <lei@...>
Date:   [today]

    feat(brain): dcv-steps-v1 step fingerprint, checked against dcv's three cases
```

## Self-Review

### Implementation Correctness
- The implementation exactly matches the specification in the brief
- `field()` function correctly encodes strings as `<utf8_byte_count>:<raw_bytes>`
- `steps_bytes()` correctly encodes the full structure with version marker, param count, param names, step count, and step details
- `steps_sha256()` correctly computes and formats SHA256 digest
- All three dcv test vectors produce byte-identical output and matching SHA256 hashes

### Test Coverage
- Case A: Validates UTF-8 Chinese character handling, multiple steps, done_when flag presence
- Case B: Validates minimal case (one step, no params, empty arg)
- Case C: Validates fullwidth bracket characters and multiple params (uses only sha256 assertion)
- Mutation test: Verifies fingerprint changes for each field modification (arg, n, done_when flag, params)

### Byte-Level Verification
Spot-checked case_a output:
- `12:dcv-steps-v1` → 12 bytes: "dcv-steps-v1"
- `1:1` → 1 byte: "1" (one param)
- `3:bio` → 3 bytes: "bio"
- `1:4` → 1 byte: "4" (four steps)
- ... (rest correctly encoded)

## Concerns
None. The implementation:
- Follows the brief exactly with zero deviations
- Passes all dcv test vectors (3 cases + mutation test)
- Produces no clippy warnings
- Passes Windows target check
- Uses only dependencies already in Cargo.toml (sha2, serde)
- Follows Rust idioms and conventions
