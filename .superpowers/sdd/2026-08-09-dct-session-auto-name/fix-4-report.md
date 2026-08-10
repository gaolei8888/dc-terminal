# Fix 4 report — the fourth non-discriminating test

Brief: `.superpowers/sdd/2026-08-09-dct-session-auto-name/fix-4-brief.md`
Target: `src/session.rs::recovering_from_a_failure_does_not_count_as_finishing_a_round`,
plus the "顺带" item `a_fresh_session_has_no_tag`.

## Option chosen: A (delete), not B (rewrite)

The brief's own instruction was to read the current code first, because
the brief was written against the pre-Fix-1 tree. I did, and the ground
had shifted in a way the brief could not have anticipated: Fix 1 replaced
the `name_slot.is_none()` gate with a synchronous `name_attempted: bool`
set as the *first statement* of `request_name`. That single change turns
out to make the mutation table in the brief (and in the final review's
Finding 7) stale, not just pessimistic.

I re-ran all three mutations from the brief's table against the **current**
tree, on just the target test, before deciding anything:

| Mutation | Brief's claim (pre-Fix-1) | Actual result (current tree) |
|---|---|---|
| delete `!s.first_input.is_empty()` alone | passes (survives) | **passes (survives)** — confirmed |
| delete `was == SessionState::Working` alone | passes (survives) | **passes (survives)** — confirmed |
| delete both together | passes (survives) | **FAILS (caught)** — brief is stale here |

The "both together" line no longer holds: with `name_attempted` set
synchronously at function entry, a premature `request_name` call during
the BOOM→Idle recovery (first_input still empty) burns the one-shot latch
before the real Working→Idle transition ever fires, so the real name
never lands and the test's final-state assertion (`tag == "真实名字"`)
times out red. I verified this directly — see the mutation runs below.

So the honest picture, post-Fix-1, is: **the test still provides zero
unique coverage**, just for a different reason than the brief describes.
Its own doc comment already conceded this before I touched anything
(lines 1972–1982, pre-edit): both guards are individually redundant in
this test's specific scenario (`first_input` is empty for the whole
recovery arc, so either guard alone already blocks it), and the test's
real target was the `name_attempted` latch, not these two guards. The
question the brief actually asks — "if I delete this test, do the two
named guards remain covered by *other* tests, individually and in
combination?" — I answered by mutation, not by trusting either the
brief's table or the test's own doc comment. All three answers are yes.

I did not attempt option B. There is nothing to make "reliable" here:
option B would mean asserting on `name_attempted`/`name_slot` at the
moment of recovery instead of at the end, for a scenario (empty
`first_input` recovery) whose two guards are already each pinned,
independently and deterministically (no sleep-race), by two other tests
already on the branch. Writing a third racy-or-not observation point for
already-covered ground would just be the fifth non-discriminating test
the brief warns against adding. A is correct on its own terms, not as a
fallback from a failed B.

## Verification: are the two guards still covered after deletion?

Both mutations, and their combination, were re-run against the **final**
tree (after both deletions and the doc-comment rewrite below), targeting
the whole `session::tests::` module so any test could catch them, not
just the deleted one.

**Mutation 1 — delete `was == SessionState::Working &&`** (src/session.rs,
the naming-gate `if` in `tick()`):

```
$ cargo test --lib session::tests:: -- --test-threads=1
...
failures:
---- session::tests::recovering_from_a_failure_after_real_input_still_does_not_count stdout ----
thread '...' panicked at src/session.rs:2102:13:
真正干完一轮活之后应该起出真实名字，最后是 "恢复期间误触发"
test result: FAILED. 72 passed; 1 failed
```
Caught by `recovering_from_a_failure_after_real_input_still_does_not_count`
— the sibling test added in Fix 1, which isolates exactly this guard by
sealing `first_input` *before* the BOOM→recovery arc (so the other guard
is already satisfied) and distinguishing "which trigger fired" via the
model prompt's screen-tail content, not sleep timing.

**Mutation 2 — delete `!s.first_input.is_empty() &&`**:

```
$ cargo test --lib session::tests:: -- --test-threads=1
...
failures:
---- session::tests::a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen stdout ----
thread '...' panicked at src/session.rs:1767:13:
assertion `left == right` failed: 没人跟它说过话，不该有名字
  left: "启动画面误触发"
 right: ""
test result: FAILED. 72 passed; 1 failed
```
Caught by `a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen`
— exercises `was == Working` (the session's own initial state) with
`first_input` empty (splash screen, nobody has typed anything), and
distinguishes via the model prompt content rather than final tag state.

**Mutation 3 — delete both together**:

```
$ cargo test --lib session::tests:: -- --test-threads=1
...
failures:
    session::tests::a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen
    session::tests::recovering_from_a_failure_after_real_input_still_does_not_count
test result: FAILED. 71 passed; 2 failed
```
Both surviving tests catch it independently — no unique coverage was
lost by deleting the third test that used to (weakly) cover this case.

After each mutation the file was restored from a saved copy and `diff`
confirmed byte-identical to the pre-mutation tree before moving to the
next one.

This directly answers what the brief's "如果选 A" test requirement asks
for: it does **not** just accept the review report's "caught by four
other tests" claim, it re-derives which tests catch which mutation, by
name, with the actual panic output.

## What I changed

`src/session.rs`:

1. Deleted `recovering_from_a_failure_does_not_count_as_finishing_a_round`
   in full (doc comment + body, ~96 lines) — zero unique coverage per the
   mutation runs above.
2. Rewrote the doc comment on
   `recovering_from_a_failure_after_real_input_still_does_not_count`
   (the surviving sibling) so it no longer refers to a test that doesn't
   exist anymore. The new comment explains, in its own right, which guard
   it pins, which guard `a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen`
   pins, and *why* the deleted third test's job is now fully absorbed by
   the two of them (the WHY the house style asks for, not a "what changed"
   changelog entry).
3. Updated one other comment (`a_freshly_created_busy_pattern_agent_...`'s
   body, line ~1740) that referenced the deleted test by name to instead
   reference `recovering_from_a_failure_after_real_input_still_does_not_count`,
   since that's the sibling still using the same "observe via prompt
   content, not final tag" idiom.
4. Deleted `a_fresh_session_has_no_tag`.

## `a_fresh_session_has_no_tag`

Deleted, per the brief's "unless you can name what it guards" test. I
looked for a code path that could initialize `name_slot` to something
other than empty at construction time — there isn't one:
`name_slot: Arc::new(Mutex::new(None))` (session.rs:658) is a fixed
struct-literal initializer, not conditional logic, so there is no branch
or guard for a mutation to remove. The property it asserts (tag is ""
right after `create()`) is also already exercised as a precondition
inside `a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen`,
which asserts the same thing for five ticks before doing anything else.
I could not name a regression this test would catch that the other one
wouldn't, so it goes.

## Final verification

```
$ cargo test -- --test-threads=1
...
Running unittests src/lib.rs ...
test result: ok. 696 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 30.74s
...
[16 more integration-test binaries, all "test result: ok", 0 failed]

$ cargo fmt --check
(no output, exit 0)

$ cargo clippy --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s)
(no warnings, exit 0)
```

696 = 698 (previous total) − 2 deleted tests. No new tests were added —
this task's job was to stop claiming coverage that didn't exist, not to
manufacture new coverage for ground that two other tests already hold.

## Diff summary

`src/session.rs`: 18 insertions, 120 deletions. Two tests removed, one
doc comment rewritten to stand on its own, one cross-reference in another
test's body comment repointed to the surviving sibling. No production
code touched.

## Concerns

None. The two guards named in the brief remain independently covered,
by name, with mutation evidence for each — including for the combined
deletion, which the brief predicted would survive but which the current
(post-Fix-1) code actually catches via the two remaining tests. Full
suite, fmt, and clippy are clean.
