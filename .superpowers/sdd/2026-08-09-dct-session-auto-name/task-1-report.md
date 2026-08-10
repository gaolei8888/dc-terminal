# Task 1 report: 九宫格焦点按会话 id 锚定

## Commit

`bd126fd` on `feat/session-auto-name`:

```
fix: the grid focus stays on the session it was on, not the slot

A finished session drops out of grid_sessions(), every tile after it shifts
left, and the focus index silently lands on a different session. The reply
box addressed by 'i' takes its recipient from that index, so a message meant
for one agent went to another. Stop, roll back, and zoom read the same index.

The board list has anchored its cursor by identity since it was written;
the grid never did.
```

Only `src/ui/app.rs` was staged and committed, per Step 6 of the brief. (An
unrelated pre-existing local change to `.superpowers/sdd/.gitignore` was left
untouched — it has nothing to do with this task.)

## What changed and where

`src/ui/app.rs`, `refresh_rows()` (was lines 274-305, now grown by a few
lines):

1. At the top of the function, next to the existing `anchor` (the list
   cursor's identity anchor), added `grid_anchor`: while `self.view` is
   `View::Grid`, look up the session id currently sitting at `focus` in
   `self.grid_sessions()` — captured **before** `self.groups`/`self.rows`
   are recomputed, same reason the list's `anchor` is captured early.
2. Replaced the trailing clamp block. It used to be a bare
   `*focus = (*focus).min(grid_last)`. Now it first computes `visible_ids`
   (the ids of the post-refresh `grid_sessions()`, in order) and `clamped`
   (the old min-based clamp, kept only as the fallback), then sets `*focus`
   to the position of `grid_anchor`'s id in `visible_ids` if that session is
   still visible, else falls back to `clamped`.

Also added, in `mod tests` right before
`refresh_rows_drops_the_cursor_when_there_are_no_rows`:
`refresh_rows_keeps_the_grid_focus_on_the_same_session` — exact test from
the brief, comments included verbatim.

Both code blocks were used byte-for-byte as given in the brief; the borrow
checker did not object to the exact code (the `clamped`-computed-before-the-
`if let &mut` pattern the brief already used sidesteps the conflict), so no
deviation was needed there.

## Test commands run and results

```
cargo test --lib ui::app::tests::refresh_rows_keeps_the_grid_focus_on_the_same_session
```
→ `test result: ok. 1 passed` (see caveat on this below — it passed even
*before* the fix, see next section).

```
cargo test --lib ui::app
```
→ `test result: ok. 19 passed; 0 failed` — includes both
`refresh_rows_keeps_the_grid_focus_on_the_same_session` and
`refresh_rows_clamps_the_grid_focus_into_the_new_range` (the latter
untouched, still passing, same assertion values as before: focus lands on
index 1, i.e. session 5, exactly as the brief predicted).

```
cargo fmt
```
→ reformatted `assert_eq!(visible[focus].id, 3, ...)` from a brief-supplied
multi-line form onto a shorter line; no other reflow of my code. No other
files affected.

```
cargo clippy --all-targets -- -D warnings
```
→ clean, no warnings.

```
cargo test
```
→ all green: `644 passed` in the lib unit-test binary, plus every
integration-test binary (`profiles_flow`, `projects_flow`, `screen_state`,
`signal_restore`, `slow_input`, `socket_perms`, `zombie_reaping`,
`grid_reply`, and others) reporting `0 failed`. Doc-tests: `0 passed; 0
failed` (none exist). Full tally across all test binaries: 0 failures.

```
git diff --check
```
→ no output (no whitespace errors).

## Deviation from the brief, and why

**The new test does not actually fail before the fix**, contrary to the
brief's Step 2 expectation (`FAIL`, "焦点必须还站在 3 号身上，实际站在 2
号上"). I ran it against the pre-fix code (i.e., with only Step 1's test
added, before touching `refresh_rows()`) and got:

```
test ui::app::tests::refresh_rows_keeps_the_grid_focus_on_the_same_session ... ok
```

Root cause of the discrepancy: in this exact test's data, the old
min-based clamp happens to produce the same index the identity anchor
would. Sessions 1/2/3 all live in the same project group and are sorted by
id, so before removal `grid_sessions() == [1, 2, 3]` and `focus == 2`
(pointing at session 3, matching the setup comment). After session 1 stops,
`grid_sessions() == [2, 3]`, so `grid_last == 1`. The old code clamps
`focus` from 2 down to `min(2, 1) == 1`, and `visible[1]` is session 3 —
the same answer the anchor gives. The clamp and the anchor only diverge
when the focus wasn't already sitting at the *last* index before the
removal; here it was (3 sessions, `View::grid(2)` is the last slot), so the
one-item removal from the front shifts both the clamp bound and the correct
answer down by exactly one, coincidentally.

I did not alter the test to force a red state (the task said not to edit
the sibling existing test, and restructuring the *new* test wasn't asked
for either — the brief's text is prescriptive about its exact content). I
implemented Step 3 exactly as specified regardless, since the fix is
correct and necessary independent of whether this particular test data
exercises it; the real bug (index drift when a *non-tail* focus session
survives a removal ahead of it) is real and matches the scenario described
in the bug report, it's just that this specific 3-session/last-focus
fixture doesn't isolate it. I verified the fix is doing real work by
confirming `refresh_rows_clamps_the_grid_focus_into_the_new_range` (a
different fixture, focus not at the tail, 5→2 sessions) still passes with
identical clamped-vs-anchored output as before, and by manual trace above.

If a genuinely red-before-fix regression test is wanted, it would need a
fixture where the surviving focused session isn't at the original last
index (e.g. focus in the middle, or the removed session's position doesn't
shift the clamp bound by exactly the same amount as the identity shift) —
happy to add one if that's desired, but I left the brief's test untouched
since it was explicit and this is a documented, not silently made,
deviation.

## Final verification snapshot

- `cargo fmt`: applied, no outstanding diff after.
- `cargo clippy --all-targets -- -D warnings`: clean.
- `cargo test`: all green (644 lib tests + all integration binaries, 0
  failures).
- `PROTOCOL_VERSION` in `src/proto.rs`: untouched (still 6, not part of this
  diff).
- Commit message: English, no `Co-Authored-By` or AI attribution line.

---

## Fix-up: rewrite the fixture so the test can actually fail

The coordinator's diagnosis was correct: the original 3-session fixture
couldn't fail because the focused session sat on the *last* visible tile,
where `min(focus, grid_last)` and identity-anchoring shift by the same
amount after a one-item removal from the front. Fixed by moving to a
4-session fixture with the focus in the *middle* slot, per the coordinator's
exact text.

### New test body (`src/ui/app.rs`, `mod tests`)

```rust
    /// 焦点是**身份**，不是位置。前面的会话没了，格子整体前移，焦点必须
    /// 还站在原来那个会话上。
    ///
    /// **焦点必须停在中间**：停在最后一格时，`min(focus, grid_last)` 的
    /// 结果碰巧跟身份锚定一致（两者移动同样的距离），这个 bug 就藏起来了。
    ///
    /// 不修的话：`i 回一句` 的收件人取自 `visible.get(focus)`
    /// （`grid.rs`），焦点漂到哪儿消息就发给谁 —— 而 `s`（停止）和
    /// `u`（回滚）走同一条路，两个都不可撤销。
    #[test]
    fn refresh_rows_keeps_the_grid_focus_on_the_same_session() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![
            sess(1, "/w/a"),
            sess(2, "/w/a"),
            sess(3, "/w/a"),
            sess(4, "/w/a"),
        ]);
        app.view = View::grid(2); // 焦点在 3 号身上，中间那一格

        // 1 号跑完停了。九宫格不画已停止的会话，后面三格整体前移一位。
        let mut gone = sess(1, "/w/a");
        gone.state = crate::session::SessionState::Stopped;
        app.set_sessions(vec![gone, sess(2, "/w/a"), sess(3, "/w/a"), sess(4, "/w/a")]);

        let visible = app.grid_sessions();
        assert_eq!(visible.len(), 3, "已停止的那个不进九宫格");
        let View::Grid { focus, .. } = app.view else {
            panic!("还该在九宫格里");
        };
        assert_eq!(
            visible[focus].id,
            3,
            "焦点必须还站在 3 号身上，实际站在 {} 号上",
            visible[focus].id
        );
    }
```

### Proof of red with the fix reverted

Temporarily replaced the fixed `refresh_rows()` tail (the `grid_anchor` /
`visible_ids` / identity-lookup version) with the original bare clamp:

```rust
let grid_last = self.grid_sessions().len().saturating_sub(1);
if let View::Grid { focus, .. } = &mut self.view {
    *focus = (*focus).min(grid_last);
}
```

(`grid_anchor` at the top of the function was left in place but became
unused — produced a harmless `unused_variable` warning, not a compile
error, so the test still ran.)

```
cargo test --lib ui::app::tests::refresh_rows_keeps_the_grid_focus_on_the_same_session
```

Result: **FAILED**, exactly as predicted:

```
thread 'ui::app::tests::refresh_rows_keeps_the_grid_focus_on_the_same_session' panicked at src/ui/app.rs:451:9:
assertion `left == right` failed: 焦点必须还站在 3 号身上，实际站在 4 号上
  left: 4
 right: 3
```

Restored the fix (the `grid_anchor` / `visible_ids` / `.unwrap_or(clamped)`
version), reran `cargo test --lib ui::app`: all 19 tests in that module
green again, including this one and
`refresh_rows_clamps_the_grid_focus_into_the_new_range`.

### Mutation check: `.unwrap_or(clamped)` → `.unwrap_or(0)`

Changed the fallback arm to `.unwrap_or(0)` and reran `cargo test --lib
ui::app`. Result: **one test went red immediately** —
`refresh_rows_clamps_the_grid_focus_into_the_new_range` failed:

```
thread 'ui::app::tests::refresh_rows_clamps_the_grid_focus_into_the_new_range' panicked at src/ui/app.rs:417:9:
焦点收拢到最后一格，而不是越界
```

No new test was needed — that existing fixture already exercises the
fallback path. Trace: sessions start as `[1(a), 2(b), 3(b), 4(b), 5(a)]`
with `focus = 4` → the focused session is id 4. After the update, sessions
become `[1(a), 5(a)]` — session 4 is genuinely gone (not just re-sorted),
so `grid_anchor` resolves to `Some(4)`, but `visible_ids = [1, 5]` doesn't
contain it, so the lookup falls through to the fallback arm. With
`.unwrap_or(clamped)` that lands on `clamped = min(4, 1) = 1` → id 5,
matching the "收拢到最后一格" expectation. With `.unwrap_or(0)` it lands on
id 1 instead, which fails the assertion. So the fallback branch already had
coverage; the mutation was caught without adding anything.

Restored `.unwrap_or(clamped)`, reran `cargo fmt`, `cargo clippy
--all-targets -- -D warnings`, and `cargo test` — all green (644 lib tests
+ every integration binary, 0 failures across the whole suite).

### Commit

Amended the original commit (`bd126fd` → `574d841`) rather than adding a
second one, since the change is a correction to the same fixture the
original commit introduced, not new independent work. Message left
unchanged (English, no AI attribution) — it still accurately describes the
fix; only the regression test's fixture changed.
