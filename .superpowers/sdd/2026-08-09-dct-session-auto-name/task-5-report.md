# Task 5 report: 触发起名

## What changed, and where

All changes are in `src/session.rs` (277 lines changed: 258 insertions, 19 deletions). `src/proto.rs` was not touched.

1. **Removed the `expect(dead_code)` cfg_attrs** on `NAME_MAX_CHARS` and `clean_name` (they had a real caller as of this task, so the `expect` would have become a hard "unfulfilled expectation" compile error otherwise). Also dropped a stray trailing `///` doc line left over from the removed attribute's preceding paragraph on `NAME_MAX_CHARS`.

2. **Added `SessionManager::request_name(&self, s: &mut Session)`**, placed immediately after `request_explanation` — verbatim per the brief: writes a synchronous fallback (`first_input` capped to `NAME_MAX_CHARS`) into `name_slot` before doing anything else (this doubles as the "already named" flag), then spawns a background thread with a 15s timeout that overwrites the slot with `clean_name(...)` of the model's answer if it's non-empty. No generation counter, unlike `request_explanation` — the name is asked at most once per session by construction, so there's nothing to invalidate.

3. **Added the trigger in `tick()`**, inside the same `if let Some(next) = next` arm as the existing `Failed`-entry block, right after it: fires `request_name` when `was == SessionState::Working`, `next` is `Idle` or `Asking`, the session `is_agent`, and `name_slot` is still `None`.

4. **Test fixture fix (deviation from the brief — see below).**

## Deviation from the brief, and why

The brief's three tests use `fake_agent()` (`cat` with `idle_pattern: "READY"`) and send real first-line text (e.g. `"修一下登录白屏"`) followed by an empty-string enter, then poll `tick()` waiting for the tag to appear.

This cannot work: `cat`'s screen only ever contains an echo of whatever bytes were written to it. Nothing in these three tests ever writes the literal string `"READY"`, so `classify()`'s `idle_re` branch (`Idle` if the pattern matches, else `Working`) can never return `Idle` — the session is stuck at `Working` forever, `tick()` never sees a `Working → Idle` transition, and `request_name` is never called. This isn't a hypothetical: I ran the brief's tests verbatim against the fully-implemented production code and confirmed the resulting hang (`FAILED ... 一直没起出名字，最后是 ""`, timing out at the 5s deadline). Elsewhere in this same test module (`tick_marks_idle_when_pattern_matches`, the `flaky` profile in `a_second_failure_does_not_show_the_first_failures_stale_explanation`), the existing convention is to literally type `"READY"` or have the shell script `echo READY` itself — `fake_agent()` was never designed to reach `Idle` without one of those.

Fix: added a module-level test helper `finishing_agent()` — same shape as `failing_agent()` — running `/bin/sh -c "sleep 0.2; echo READY; sleep 30"`. It reaches `Idle` on its own timeline regardless of what's typed, so the tests can send a *real* first line (needed for the naming logic under test) while still reaching the `Idle` transition the trigger depends on. All three of the brief's tests were changed only to use `finishing_agent()` / profile name `"finishing"` in place of `fake_agent()` / `"fake"` — test bodies and assertions are otherwise verbatim from the brief.

A second, more substantive deviation: one of the three mandatory mutations (mutation 3, see below) did not turn any test red even after the `finishing_agent()` fix, because in every test's fixture the very first `Idle` classification is always immediately preceded by `was == Working` anyway (idle-pattern-only profiles start in `Working` at creation, per `create()`'s `state = if idle_re.is_some() || busy_re.is_some() { Working } else { Unknown }`, and can only ever alternate between `Working` and `Idle`). Per the task's explicit instruction — "如果一个 mutation 没让测试变红，说明那条测试没测到东西，当场把它修好再往下走" — I added a fourth test, `recovering_from_a_failure_does_not_count_as_finishing_a_round`, described in the mutation-3 section below.

## Test commands and pre-change failure lines

Before writing any production code, I added the brief's three tests verbatim (still using `fake_agent()`/"fake" at that point) to confirm a red baseline compiles and fails:

```
cargo test --lib session::tests::a_session_gets_named_after_its_first_round_of_work -- --nocapture
```
```
thread 'session::tests::a_session_gets_named_after_its_first_round_of_work' panicked at src/session.rs:1226:13:
一直没起出名字，最后是 ""
test session::tests::a_session_gets_named_after_its_first_round_of_work ... FAILED
```

After implementing `request_name` and the `tick()` trigger (production code only, tests still on `fake_agent()`), re-running the same test **still failed**, confirming the fixture defect independent of the feature:

```
thread 'session::tests::a_session_gets_named_after_its_first_round_of_work' panicked at src/session.rs:1254:13:
一直没起出名字，最后是 ""
```

That's the pre-fix failure line for all three brief tests (`a_name_is_pinned_and_never_asked_for_twice` and `a_dead_model_leaves_the_first_line_as_the_name` fail the same way, for the same reason — no `Working → Idle` transition ever happens with `cat`).

## Post-fix: all four tests green

```
cargo test --lib session::tests::a_session_gets_named_after_its_first_round_of_work
cargo test --lib session::tests::a_name_is_pinned_and_never_asked_for_twice
cargo test --lib session::tests::a_dead_model_leaves_the_first_line_as_the_name
cargo test --lib session::tests::recovering_from_a_failure_does_not_count_as_finishing_a_round
```

All four: `ok`, 1 passed each.

Full suite:

```
cargo fmt
cargo clippy --all-targets -- -D warnings   # clean, no warnings
cargo test                                  # 663 lib tests passed (was 659 before this task's 4 new tests),
                                             # all integration test binaries green, 0 failed
```

## Step 5: mutation testing (all three required mutations, plus one added)

### Mutation 1 — delete `&& recover(s.name_slot.lock()).is_none()` from the trigger

Expected: `a_name_is_pinned_and_never_asked_for_twice` must FAIL. Confirmed:

```
thread 'session::tests::a_name_is_pinned_and_never_asked_for_twice' panicked at src/session.rs:1321:9:
assertion `left == right` failed: 名字是钉死的，第二轮不该重起
  left: "第二个名字"
 right: "第一个名字"
test session::tests::a_name_is_pinned_and_never_asked_for_twice ... FAILED
```

Reverted; confirmed the test suite is green again with the guard restored.

### Mutation 2 — delete the two fallback lines at the top of `request_name`

Expected: `a_dead_model_leaves_the_first_line_as_the_name` must FAIL. Confirmed:

```
thread 'session::tests::a_dead_model_leaves_the_first_line_as_the_name' panicked at src/session.rs:1345:13:
兜底没生效，最后是 ""
test session::tests::a_dead_model_leaves_the_first_line_as_the_name ... FAILED
```

Reverted.

### Mutation 3 — replace `was == SessionState::Working` with `true`

Expected (per brief): at least one test FAILs, reasoning "起名会在第一次 tick 就发生，那时 first_input 还是空的".

**First attempt with only the brief's three tests: nothing failed.** All 56 `session::tests::*` passed including all three naming tests. Root cause: for every fixture used (idle-pattern-only profiles), `next == Idle` is only ever reachable from `was == Working` — there's no other reachable state to alternate from (creation already sets `state = Working` whenever `idle_re` is `Some`, and `classify()` for an idle-pattern-only profile can only return `Working` or `Idle`). So `was == SessionState::Working` is — for these three fixtures — always true whenever `next == Idle` fires, mutated or not; the guard never distinguishes anything observable in these three tests.

Per the task's mandatory instruction to fix a test that can't fail rather than move on, I added a fourth test that exercises the one case where `was` really can differ from `Working` at an `Idle`-entering tick: recovering from `Failed`. `recovering_from_a_failure_does_not_count_as_finishing_a_round` uses a profile with both `error_pattern = "BOOM"` and `idle_pattern = "READY"` (`echo BOOM; sleep 0.2; clear; echo READY; sleep 30`), sends **no input at all** while the session goes `Working → Failed → Idle` purely from the script's own output, then — only afterward — sends real input and drives a genuine `Working → Idle` round. The backend (`ByPrompt`) returns `"真实名字"` if the prompt it received contains the real first line, else `"提前起的名字"` — this ties the assertion to *which* transition actually produced the name (rather than to call-count ordering, which would be racy against the background thread).

- Unmutated: `was == Failed` at the recovery transition blocks `request_name`, so `name_slot` stays `None` until the real `Working → Idle` round; final tag is `"真实名字"`. **PASS.**
- With mutation 3 re-applied: `request_name` fires prematurely on the `Failed → Idle` recovery (with `first_input` still empty), pins `name_slot` to `"提前起的名字"`; the later real round is then blocked by the (still-intact) `name_slot.is_none()` guard. Confirmed red:

```
thread 'session::tests::recovering_from_a_failure_does_not_count_as_finishing_a_round' panicked at src/session.rs:1424:13:
真正干完一轮活之后应该起出真实名字，最后是 "提前起的名字"
test session::tests::recovering_from_a_failure_does_not_count_as_finishing_a_round ... FAILED
```

Reverted the mutation. Re-ran `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test` one final time on the fully-reverted tree — all green (663 lib tests, all integration binaries pass, no clippy warnings, `git diff --check` clean).

## Commit

```
git add src/session.rs
git commit -m "feat: name a session the first time it finishes a round of work
..."
```

The commit adds only `src/session.rs`. `.superpowers/sdd/.gitignore` and the untracked doc file present in the working tree before this task started were left alone.

---

## Fix round 1 (coordinator review)

Two findings from review, both in `src/session.rs`, on top of commit `3c64bad`.

### Critical — trigger fires on a freshly created agent's splash screen for every shipped profile

Confirmed independently: `create()` sets `state = Working` whenever `busy_re.is_some() || idle_re.is_some()` (line ~456). All shipped profiles (`profiles/claude.toml`, `codex.toml`, `glm.toml`, `kimi.toml`, `deepseek.toml`, `qwen-api.toml`) declare only `busy_pattern` (e.g. `"esc to interrupt"`), never `idle_pattern`. `classify()`'s `busy_re`-only branch reads `Idle` whenever the busy string is *absent* — true of a freshly spawned agent still on its splash screen. So the very first `tick()` after `create()` sees `was == Working` (the just-set initial value) → `next == Idle` (splash screen, busy string not printed yet), `is_agent == true`, `name_slot == None` — and fired `request_name` with `first_input == ""`, permanently pinning the tag to empty (or to a name invented from the splash screen, if a real backend was configured).

**Fix:** added `&& !s.first_input.is_empty()` to the trigger's condition list in `tick()`, and rewrote the accompanying comment to state the true condition ("干完一轮 **且用户已经说过话**") and explain why the busy-pattern-only shape of every real profile makes the second half load-bearing, not decorative. Deliberately used `first_input`, not `first_input_sealed`, per the coordinator's instruction — a user who pastes a long unsent brief and gets beaten to the punch by the agent finishing a round should still get named from that unsealed text.

**Coverage gap fix:** added `busy_only_agent()` — a `cat`-backed profile shaped like the real ones (`busy_pattern: Some("esc to interrupt")`, `idle_pattern: None`) — and a new test, `a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen`:
- Creates a session, ticks 5 times **without ever sending input**, asserts `tag == ""` every time.
- Then sends a real first line, drives a real `Working → Idle` round, asserts the tag eventually becomes the name a `ByPrompt` fake backend returns *only when its prompt contains the real first line* (otherwise it returns a distinguishable "splash misfire" string) — this ties the final assertion to *which* transition actually produced the name, immune to background-thread scheduling races.

Mutation (drop `&& !s.first_input.is_empty()`), red output:

```
thread 'session::tests::a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen' panicked at src/session.rs:1431:13:
assertion `left == right` failed: 没人跟它说过话，不该有名字
  left: "启动画面误触发"
 right: ""
test session::tests::a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen ... FAILED
```

It fails even earlier/more directly than anticipated — right at the first-phase "no premature name" assertion, not only at the later real-round assertion. Reverted the mutation before moving on; re-ran the test to confirm green again.

**Side effect on an earlier test's doc comment.** Adding this guard means `recovering_from_a_failure_does_not_count_as_finishing_a_round`'s premature-firing scenario (no input ever sent, `first_input` stays `""`) is now blocked by *both* guards (`was == Working` and `!first_input.is_empty()`) independently — I verified by mutating only `was == Working` → `true` (leaving the new guard intact) that the test still passes, i.e. it no longer isolates the `was == Working` mutation on its own. Its doc comment previously claimed it was "the only place `was == Working` is tested," which is no longer true, so I rewrote the comment to say the two guards are redundant in that scenario rather than leave a false claim in the code. I did not go further and try to construct a fixture that isolates `was == Working` from the new guard (e.g., a non-empty-`first_input` session recovering from an immediate post-input crash before ever reaching a real `Idle`) — that's a real, distinguishable scenario in principle, but it was out of scope for this review round and no reviewer asked for it; flagging it here rather than silently skipping it.

### Important — flaky script in `recovering_from_a_failure_does_not_count_as_finishing_a_round`

Confirmed the mechanism: the profile's script did `echo BOOM; sleep 0.2; clear; echo READY; sleep 30` — `clear` and `echo READY` are two separate process invocations, hence two separate `write()`s to the pty. A `tick()` landing in the window between them would see neither `BOOM` nor `READY`, classify as `Working`, and the *next* tick would then see a genuine (but spurious, for this fixture's purposes) `Working → Idle` transition with an empty `first_input` — failing the test for the wrong reason (a scheduling race, not the thing under test).

**Fix:** switched the test's profile from a `Profile::from_toml` raw-TOML block to a `Profile` struct literal (consistent with `finishing_agent()`/`busy_only_agent()`/`failing_agent()`), and replaced the `clear; echo READY` pair with a single `printf '\033[2J\033[HREADY\n'` — ED2 (clear), cursor-home, and the marker all emitted from one shell command, landing in (for all practical purposes) one `write()`. The struct-literal form was necessary anyway: TOML basic strings interpret `\0` as an escape sequence and would either reject or mis-decode the ANSI octal escape; a Rust string literal doesn't have that problem.

Checked `finishing_agent()` for the same hazard: its script is `sleep 0.2; echo READY; sleep 30` — a single content-producing command with no preceding `clear`, so there's no two-write race to begin with. No change needed there.

Stress-tested the fixed test directly against the compiled test binary (bypassing `cargo test`'s per-run build overhead): **0 failures in 155 consecutive runs** (15 via `cargo test`, then 40 + 100 more direct binary invocations). The original flake was estimated at 2-5%; 0/155 is strong evidence the race is gone (at a true 3% rate, seeing zero failures in 155 tries has under 1% probability).

### Full verification, re-run twice per the coordinator's request

```
cargo fmt
cargo clippy --all-targets -- -D warnings
git diff --check -- src/session.rs
```
All clean, no output, exit 0.

```
cargo test
```
Run 1: `test result: ok. 664 passed; 0 failed; ...` for the lib target, plus every integration test binary green (17 `test result: ok` lines total, 0 failures).

Run 2: identical — `664 passed; 0 failed`, all integration binaries green, 0 failures.

(664 = 663 from the prior round + 1 new test, `a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen`.)

### Commit

```
git add src/session.rs
git commit -m "fix: don't name a session before it's heard from the user
..."
```

---

## Fix round 2 (coordinator review)

**Adjudication:** `was == SessionState::Working` is load-bearing and stays, but round 1 left it untested — `recovering_from_a_failure_does_not_count_as_finishing_a_round` turned out to be doubly guarded (both `was == Working` and the round-1 `!first_input.is_empty()` independently block its scenario, since it never sends input), so it no longer isolates `was == Working` on its own.

**The reachable bug this guard prevents:** user sends a first line (`first_input` non-empty), agent hits `API Error` → `Failed`, error text scrolls off → `classify()` reads `Idle`. Five of six shipped profiles declare `error_pattern` (only `codex.toml` doesn't) — this is a real path. Without the guard, that `Failed → Idle` satisfies every other condition (including the round-1 `!first_input.is_empty()` check, since `first_input` is already non-empty here) and burns the once-per-session `name_slot` on an error-recovery screen, before the genuine round of work.

### New test: `recovering_from_a_failure_after_real_input_still_does_not_count`

Added to `src/session.rs`, right after `recovering_from_a_failure_does_not_count_as_finishing_a_round`. Structure, per the coordinator's spec:

- Profile with `error_pattern = "BOOM"`, `idle_pattern = "READY"`, script `echo BOOM; sleep 1; printf '\033[2J\033[HREADY\n'; sleep 0.5; cat` (clear + marker combined into one `printf` write, same discipline as round 1).
- Sends a real first line **before** the recovery happens, so `first_input` is already sealed and non-empty at the `Failed → Idle` transition — this is what makes the scenario distinct from `recovering_from_a_failure_does_not_count_as_finishing_a_round`, where `first_input` stays empty throughout.
- A fake backend (`ByScreenTail`) keys its answer on the prompt's **screen-tail** substring specifically — not on `p.user` as a whole, since `first_input` is embedded verbatim in every `name_prompt`'s `user` text regardless of what's on screen, so a whole-string `contains()` check would spuriously match both firings and hide the bug. `ByScreenTail` splits on the literal separator `name_prompt` uses ("屏幕上的最后一段内容：\n\n") and inspects only what follows it: `"真实名字"` if that tail contains `cat`'s echo of the user's line, `"恢复期间误触发"` if it doesn't.
- Waits for the natural `Failed → Idle` recovery, ticks a further 10 rounds to give a wrongful early firing room to happen, then waits for `cat` to actually echo the queued line onto the screen (confirms the "genuine round" evidence is now present), then forces a second `Working → Idle` round and asserts the tag ends up `"真实名字"`.

**Two timing issues found and fixed while building this, both fixture-only (not production bugs):**

1. **`cat` could echo the queued input before the "recovery" screen snapshot was taken**, collapsing the discriminator (both firings would then see the same screen and produce the same name, hiding the bug regardless of which condition is present). Fixed by delaying `cat`'s start by 0.5s after the `READY` marker appears, giving `request_name`'s synchronous screen read (at the `Failed → Idle` tick) a wide, reliable window before the echo can appear.
2. **A subtler one, caught by a real 1/30 spurious failure under *correct* code during stress-testing:** the test's own polling loop (`tick()` every 20ms) can, under scheduling jitter, skip observing the `Failed` state entirely and land straight on the `READY`-triggered `Idle` read. When that happens, `s.state` was never updated away from its *initial* value — which is also `Working` (`create()` sets `state = Working` whenever `idle_re` or `busy_re` is `Some`) — so `was == Working` reads spuriously true even though a `Failed` state genuinely occurred in between; the test just never observed it. This is a fixture-granularity problem, not a production defect: production's real `tick()` cadence (200ms) is comparable to my original `sleep 0.2` BOOM-visible window, so the two could tie under jitter. Fixed by widening the BOOM-visible window from `sleep 0.2` to `sleep 1` — about 50x the 20ms poll interval — so a single scheduling hiccup can no longer skip the entire window.

Debugged both by temporarily adding an `eprintln!` inside the fake backend to dump the exact screen tail at each call, confirmed the mechanism (`"BOOM\n修一下登录白屏"` at the explanation call when `Failed` is first entered — proving local pty echo makes typed text visible almost immediately — then `"READY"` alone at the naming call once the screen was cleared and before `cat` had run), then removed the debug print before finalizing.

**Stress-test results** (direct invocation of the compiled test binary in a loop, bypassing `cargo test`'s per-run rebuild, run sequentially without other concurrent load):

- Correct code: **0 failures in 40 runs** (after both timing fixes; an earlier run, before the `sleep 0.2 → sleep 1` fix, had shown 1 spurious failure in 30 under correct code — root-caused as above and fixed, not shipped).
- Mutated code (see below): **20 failures in 20 runs.**

### Mutation: delete `was == SessionState::Working &&` from the trigger

Mutated:
```rust
if matches!(next, SessionState::Idle | SessionState::Asking)
    && s.is_agent
    && !s.first_input.is_empty()
    && recover(s.name_slot.lock()).is_none()
{
    self.request_name(&mut s);
```

Red output (`cargo test --lib session::tests::recovering_from_a_failure_after_real_input_still_does_not_count -- --nocapture`):

```
thread 'session::tests::recovering_from_a_failure_after_real_input_still_does_not_count' panicked at src/session.rs:1678:13:
真正干完一轮活之后应该起出真实名字，最后是 "恢复期间误触发"
test session::tests::recovering_from_a_failure_after_real_input_still_does_not_count ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 664 filtered out; finished in 6.88s
```

Restored the `was == SessionState::Working &&` line; re-ran the same command, green:

```
test session::tests::recovering_from_a_failure_after_real_input_still_does_not_count ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 664 filtered out; finished in 1.88s
```

### Verification

```
cargo test --lib session   # 111 passed, 0 failed
cargo fmt                  # clean
git diff --check -- src/session.rs   # clean
```

An earlier attempt at this round also ran the full `cargo test` suite (not just the `session` module) twice as originally instructed; both full runs were green (665 lib tests total — the 3 additions this round: `recovering_from_a_failure_after_real_input_still_does_not_count` plus the timing-fix iterations landing on the same test — passed alongside everything else), and one intermediate full run hit `ui::tests::entering_a_session_always_lands_at_the_bottom_even_without_a_resize` — confirmed pre-existing and unrelated to this change (last touched in commit `7bf31dd`, long before this task; reproduces only under full-suite concurrent load, passes reliably in isolation both with and without this round's changes). Those full runs are not repeated here per the coordinator's follow-up instruction to run only `cargo test --lib session` this round, since the long full-suite runs were tripping a process watchdog.

### Commit

Only `src/session.rs` is in this round's commit, same as prior rounds.

Only `src/session.rs` is in this commit, same as round 1.
