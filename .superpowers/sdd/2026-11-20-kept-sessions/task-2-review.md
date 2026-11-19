# Task 2 review: `KeepWatch` state machine (4a9c030..d2c8c96)

Reviewer verified: `cargo test --lib keep` at d2c8c96 → 106 passed, 0 failed (filter also matches non-keep tests; the 17 `watch_tests` are among them).

**Spec verdict: ✅** (behaviour matches spec §2–§5 and the brief; deviations below are minor)
**Quality verdict: approved** (with minor follow-ups; nothing blocking inside Task 2's scope)

## Scenario traces (all checked against src/keep.rs step/step_one)

| Scenario | Result |
|---|---|
| Long-lived session stopped by hand | Death round: `lived >= 60` → `next_try = now + backoff(0)` = 5 s, failures untouched; reopen passes `w.id` and remembered conversation (from watcher memory, not roster — Review Focus #1 OK). |
| Opened, dies before next round | `seen = true` set by open, so next round is death detection, `lived < 60` → `fail()`, backoff grows (test `quick_deaths_...` asserts 10,20,40,80,160,300,300,300). |
| `open` Err repeatedly | `StartFailed` once, `fail()` each time, attempts at 0/10/30/70; `seen` stays false for a never-opened keep, but `next_try` gates it correctly. No panic. |
| Resumed by conversation, dies < 20 s | Conversation dropped at death detection; next open passes `None`, id kept. Opened without conversation (`resumed = false`) → conversation untouched. Shell (no resume-by-id) dies fast → conversation kept. |
| Keep removed, then re-added | Forgotten by `retain`; re-added is fresh → immediate `open(None, None)` if not alive, or adopted if the old session is still running. Matches spec §2.3 "配置刚加上 = 从没见过". Acceptable. |
| Duplicate names in `keeps` | Share one `Watched`; no panic. Second step_one in the same round would see `seen && next_try.is_none()` right after the first opened and count a quick death — harmless only because Task 1 drops duplicates. Fine. |
| Warning dedupe / re-arm | `warn_once` per reason; DirGone/ProfileGone cleared when the problem clears or the session is found; StartFailed/CrashLooping cleared on healthy; NameClash cleared when the clash goes away. |
| `2^failures` overflow | `checked_shl` → `u32::MAX` for ≥32, `checked_mul` fallback to CAP, then `min(CAP)`. No panic for any `u32`. `failures += 1` can't realistically overflow (one per ≥300 s). |
| Paused | Early return before `retain` and before any per-keep work. Mutation reported killed by two tests. |

## Findings

1. **minor — src/keep.rs:353-362: a death detected at exactly/after 60 s neither counts as a failure nor resets the count.**
   Healthy reset only happens in the *found* branch (keep.rs:345) on a round where `now - opened_at >= 60`. Rounds are 5 s apart, so a session opened at t that dies at t+58 is last found at t+55 (< 60, no reset) and detected at t+60 (`lived = 60`, not `< HEALTHY_AFTER`, so no `fail()`). Result: failures stay at their old value (e.g. 4 → next wait 160 s instead of 5 s per spec "活过 60 秒清零"), and CrashLooping/StartFailed stay suppressed. Fix: in the `else` branch at keep.rs:361 also set `failures = 0` and clear the non-NameClash warned flags (consistent with how `lived` is measured), or measure `lived` against the last time it was seen alive. Add a test: crash loop, then a session alive across rounds up to +55 and gone at +60 → expect wait 5 s.

2. **minor — src/keep.rs:354-356 (window precision): `lived` is measured to detection time, not death time.**
   With 5 s rounds the effective resume-fail window is 15–20 s and the healthy window 55–60 s. Within tolerance of the spec, but worth a one-line comment so Task 3 doesn't lengthen the tick without noticing.

3. **minor (test gap) — src/keep.rs:347 `w.warned.retain(|r| *r == NameClash)` is not pinned by any test.**
   The healthy reset runs on *every* found round after 60 s. If that line became `w.warned.clear()`, a persistent clash would re-warn `NameClash` every 5 s. `a_name_clash_warns_once_and_still_opens` only runs to s=31 (never reaches the 60 s healthy path). Extend the loop past 60 s (e.g. `1..120`) and keep the `== 1` assertion.

4. **minor (spec wording) — first wait after a quick death is 10 s, not 5 s.**
   `backoff` uses failures *after* increment, so a fresh keep that fails immediately retries at 0/10/30/70, and a crash loop is 5 (non-quick first death) → 10 → 20…. Spec §3 / Review Focus #5 say "5 s, 10 s, 20 s"; the brief's formula is what was implemented and the sequence from a healthy session does start at 5 s. Acceptable; just note it so the spec's test list ("按 5 秒、10 秒、20 秒……增长") isn't read literally in Task 3/4 tests.

5. **minor — `StartFailed` also feeds `CrashLooping`.** Three failed opens raise both warnings, and the CrashLooping text says "keeps stopping". Not wrong per brief (`open Err → failures += 1`), but the user sees two messages for one cause. Optional: only call `warn_once(CrashLooping)` from the death-detection path.

6. **should fix before merge (outside Task 2 scope) — report concern #1 is correct and worse than stated.**
   src/proto.rs:1483 doc says "反复崩溃，不再自动重启", and the **user-facing** i18n text at src/i18n.rs:2835-2838 says "it keeps stopping, so it will not be restarted again" / "它一直在停，不再自动重开了". The watcher (per spec §3) keeps reopening with backoff capped at 5 min, so the UI tells the user something false. Change both to e.g. "它一直在停，放慢了重开（最多每 5 分钟一次）" / "it keeps stopping; restarts are slowed down (at most every 5 minutes)". Also the enum doc at proto.rs:1474 "或者为什么停止重启它" should go. Assign to Task 3 or a fixup on Task 1's files.

7. **note — `Roster::open` returns `Result<u32, String>` instead of brief's `Result<u32, ()>`.** Justified (clippy `result_unit_err`, error text for logging). Task 3 must use this signature.

## Test quality

Good: deterministic manual clock, fake roster that does not auto-revive opened sessions (so "opened then died before next round" is expressible), assertions check exact `(name, conversation, want_id)` tuples and exact wait sequences rather than just counts, and the reported mutation runs (including the added `being_seen_alive_briefly_does_not_reset_the_backoff` that killed a surviving mutant) are convincing. Gaps: findings 1 and 3 above; nothing asserts that a failed open with a remembered conversation keeps it (it does — fine either way).
