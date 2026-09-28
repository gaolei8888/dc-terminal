# Task 3 Report: Tier Proposals and Label-Based Floors

## Status
DONE

## Commit
- **4c41fa1**: feat(brain): propose tiers from each step's intent; button text only raises

## Test Summary
All 13 new tests pass + 4 existing tests remain passing = 17 total tests passing.

## Implementation Details

### Code Added
Modified: `/Users/lei/work/dc/dc-terminal/crates/dct-brain/src/tier.rs`

Implemented four public functions:
1. **`tier_for_label(label: &str) -> Tier`**: Determines tier based on button text
   - Checks against Chinese and English word lists
   - Returns `Read` for empty, negated, or unrecognized text
   - Returns `Money` if word lists contain payment keywords
   - Returns `Content` if word lists contain content keywords

2. **`floor_for_element(label: &str, confirmed: bool) -> Tier`**: Unconfirmed elements default to at least Content tier
   - If confirmed: returns result from `tier_for_label`
   - If unconfirmed: returns at least `Tier::Content` (enforced via `max`)

3. **`raise(approved: Tier, label: &str, confirmed: bool) -> Tier`**: Runtime floor enforcement
   - Combines approved tier with floor_for_element result via `max`
   - Never lowers the approved tier

4. **`tier_for_step(action: &str, arg: &str) -> Tier`**: Proposes tier for procedure steps
   - `verify_text`, `wait`, `read_value` → `Read`
   - Navigation/UI actions → `SelfOnly`
   - `pick_file` → `Content`
   - `tap_by_intent`, `confirm_dialog` → intelligent matching with text analysis
   - Unknown actions → `Content` (default to stricter)

Helper functions and constants:
- `words()`: Tokenizes label into lowercase alphanumeric words
- `hit()`: Checks if label matches Chinese substring or English whole-word lists
- `negated()`: Detects negation patterns (Chinese prefix, English first word)
- Five word lists (CONTENT_ZH, CONTENT_EN, MONEY_ZH, MONEY_EN, AMBIGUOUS_ZH, AMBIGUOUS_EN, NEGATION_ZH, NEGATION_EN)

### Tests Added
All tests added to existing `mod tests` block in tier.rs:

1. ✓ `reading_steps_are_read`: verify_text/wait/read_value → Read
2. ✓ `moving_around_and_filling_in_only_affect_yourself`: navigation actions → SelfOnly
3. ✓ `outward_words_and_handing_over_files_are_content`: save/publish/pick_file → Content
4. ✓ `ambiguous_last_buttons_are_proposed_as_content`: Next/Continue/OK → Content
5. ✓ `negations_are_not_the_thing_they_negate`: Don't save/Cancel → SelfOnly
6. ✓ `money_words_are_money`: Payment keywords → Money
7. ✓ `unknown_actions_are_treated_as_outward`: Unknown actions → Content
8. ✓ `label_floor_only_speaks_when_the_text_says_something`: tier_for_label behavior
9. ✓ `raising_never_lowers`: raise never decreases approved tier
10. ✓ `unconfirmed_elements_count_as_outward_at_least`: unconfirmed defaults to Content+

## TDD Evidence

### Step 1: RED Phase
```
$ cargo test -p dct-brain tier
error[E0425]: cannot find function `tier_for_step` in this scope
error[E0425]: cannot find function `tier_for_label` in this scope
error[E0425]: cannot find function `floor_for_element` in this scope
error[E0425]: cannot find function `raise` in this scope
```
✓ Tests failed as expected

### Step 2: Implementation
Added 184 lines: word lists, helper functions, and four public functions

### Step 3: GREEN Phase
```
$ cargo test -p dct-brain tier
running 13 tests
test result: ok. 13 passed; 0 failed
```

### Step 4: Full Test Suite
```
$ cargo test -p dct-brain
running 17 tests
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured
```

### Step 5: Clippy (strict)
```
$ cargo clippy -p dct-brain --all-targets -- -D warnings
Checking dct-brain v0.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.88s
```
✓ No warnings

### Step 6: Windows Cross-Compile
```
$ cargo check --target x86_64-pc-windows-msvc --all-targets -q
```
✓ Passed (unrelated warning from student_projects.rs)

## Self-Review

### Code Quality
- All helper functions (words, hit, negated) are private, not exported
- Word lists match dcv design exactly; constants clearly marked as copy-paste
- Unicode handling correct: right single quotation mark (\u{2019}) properly escaped
- Chinese text detection uses substring matching; English uses whole-word matching after tokenization
- Negation detection: Chinese checks prefix, English checks first word only

### Design Adherence
- ✓ Word lists only grow, no removals/edits
- ✓ Ambiguous buttons (Next/Continue/OK) treated as Content in tier_for_step but not in tier_for_label
- ✓ Unconfirmed elements always floor at Content minimum
- ✓ raise() never lowers approved tier
- ✓ tier_for_label() returns Read for anything it can't classify
- ✓ Unknown actions default to Content (stricter interpretation)

### Test Coverage
- All test cases from brief implemented verbatim
- Expected values match brief exactly
- Edge cases covered: empty labels, whitespace, negations, mixed case

## Concerns
None. All tests pass, all requirements met, code clean.

---

# Fix Round 1 Report

## Issues Fixed

### Issue 1: Apostrophe Test Coverage
**Finding**: Test `negations_are_not_the_thing_they_negate` had two instances of "Don't save" with the same U+0027 (straight apostrophe), missing coverage of the U+2019 (right single quotation mark) path that `words()` function handles.

**Fix**: Changed second instance to `"Don\u{2019}t save"` (U+2019 right single quotation mark).

**Impact**: Now `words()` function's `.replace('\u{2019}', "'")` is properly tested.

### Issue 2: Multi-Clause Negation Suppression
**Finding**: Labels with multiple clauses separated by punctuation (e.g., "放弃草稿，直接发布" = "abandon draft, post anyway") were incorrectly suppressed if the first clause contained negation. A leading negation should not suppress content tiers from later non-negated clauses.

**Fix**: Refactored negation logic to be per-clause:
1. Added `split_clauses()` helper: splits on ，,;；、。.!！?？ (trim each, skip empty)
2. Added `score_clause()` helper: scores a single clause with money/content/read logic
3. Updated `tier_for_label()`: returns max tier across all non-negated clauses (Read if nothing)
4. Updated `tier_for_step()`: for tap_by_intent/confirm_dialog:
   - Returns SelfOnly only if ALL clauses are negated
   - Otherwise applies per-clause scoring
   - Ambiguous-word default (Content) applies only to non-negated clauses

**Test Cases Added**:
- `tier_for_step("tap_by_intent", "放弃草稿，直接发布")` → Content (second clause "直接发布" is content)
- `tier_for_step("tap_by_intent", "No, post anyway")` → Content (second clause "post anyway" is content)
- `tier_for_label("放弃草稿，直接发布")` → Content
- `tier_for_label("No, post anyway")` → Content
- `raise(Tier::SelfOnly, "No, post anyway", true)` → Content
- `tier_for_step("tap_by_intent", "Not now, maybe later")` → SelfOnly (all clauses negated)

**Code Changes**:
- Lines 86-97: Added clause-splitting logic with Unicode char array (fixed clippy warning)
- Lines 99-109: New `score_clause()` for single-clause scoring
- Lines 111-132: Refactored `tier_for_label()` with per-clause logic and Chinese doc comment
- Lines 153-182: Refactored `tier_for_step()` tap_by_intent/confirm_dialog with per-clause negation check

## Tests

### RED → GREEN → Verify Cycle
```
$ cargo test -p dct-brain
running 18 tests
test result: ok. 18 passed; 0 failed
```

All 17 previous tests remain passing; 1 new test added (`multi_clause_labels_judge_negation_per_clause`).

### Clippy (Strict)
Initial: Manual char comparison warning on split_clauses
```
error: this manual char comparison can be written more succinctly
  --> crates/dct-brain/src/tier.rs:88:16
  help: consider using an array of `char`: ['，', ',', ';', '；', '、', '。', '.', '!', '！', '?', '？']
```

After fix:
```
$ cargo clippy -p dct-brain --all-targets -- -D warnings
Checking dct-brain v0.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.81s
```
✓ No warnings

### Windows Check
```
$ cargo check --target x86_64-pc-windows-msvc --all-targets -q
```
✓ Passed (unrelated warning from student_projects.rs)

## Commit
- **87838d6**: fix: judge negation per clause in multi-clause labels like "abandon draft, post anyway"

## Summary
Fixed apostrophe test coverage (U+2019 path) and multi-clause negation suppression. All 18 tests pass, code clean with no clippy warnings. Both findings from controller review fully addressed.
