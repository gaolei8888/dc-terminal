# Task 1 report: 只在 agent 真要鼠标时才抓

Branch: `feat/copy-from-session`
Commit: `441ae41` — "feat: only grab the mouse when the agent actually asked for it"

## What changed, per file

### `src/ui/app.rs`
- Added `pub copy_mode: bool` field to `App`, placed right after `explained_failure` (adjacent to `scroll` conceptually, at the end of the struct in source order — the brief said "in the vicinity of `scroll`"; I put it at the end of the struct body since that's where the doc comment reads naturally after the last field, and `new_inner`'s initializer list follows the same struct order). Doc comment kept verbatim from the brief.
- Added `copy_mode: false,` to `new_inner`'s field-initializer list (used by both `new` and `new_disconnected`, per the module's existing "one source of truth for defaults" convention documented on `new_inner`).

### `src/ui/mod.rs`
- Added `fn wants_mouse_capture(attached: bool, agent_subscribed: bool, copy_mode: bool) -> bool` immediately after `mouse_capture_transition`, body `attached && agent_subscribed && !copy_mode`, doc comment verbatim from the brief.
- In `run()`'s main loop, replaced the capture-check block: `is_attached` is still computed the same way, but now feeds into `want = wants_mouse_capture(is_attached, app.scroll.agent_owns, app.copy_mode)`, and `mouse_capture_transition(mouse_captured, want)` replaces `mouse_capture_transition(mouse_captured, is_attached)`. `mouse_capture_transition`'s signature is untouched.
- Rewrote the comment above that block per the brief's instruction: kept the "why before `term.draw`" and "why one check instead of one per branch" reasoning intact, added a new paragraph up front explaining that the check is now a three-condition conjunction (codex/shell sessions with mouse capture on give up drag-select for a scroll wheel that PageUp/PageDown/End already provide), and appended a clause noting this frame's `Screen` response already landed in `app.scroll` so `agent_owns` is this frame's fact.
- Added three tests to `mod tests` at the end of the file, verbatim from the brief:
  - `mouse_is_captured_only_when_all_three_conditions_hold` — exhaustive 8-case truth table.
  - `a_failed_screen_call_does_not_flip_the_capture_state` — disconnection doesn't touch `app.scroll`, so the capture decision doesn't flip.
  - `a_fresh_app_is_not_in_copy_mode` — default is `false`.
- Left the existing `mouse_capture_toggles_only_on_a_real_transition` test completely untouched.

## Tests: names and results

Step 2 (pre-implementation, expected to fail):
```
cargo test --lib ui::tests
```
Failed with 13 compile errors: `cannot find function 'wants_mouse_capture' in this scope` (7 call sites) and `no field 'copy_mode' on type 'ui::app::App'` (3 sites) plus their function-not-found counterparts — exactly the expected failure mode (missing symbols, not a logic failure).

Step 6 (post-implementation):
```
cargo test --lib ui::
```
→ `test result: ok. 332 passed; 0 failed` (includes the 3 new tests and the untouched `mouse_capture_toggles_only_on_a_real_transition`).

Full suite:
```
cargo test
```
→ all binaries green: lib `633 passed; 0 failed`, plus every integration test file (`signal_restore`, `slow_input`, `socket_perms`, `zombie_reaping`, `screen_*`, etc.) `0 failed`. Doc-tests: `0 passed; 0 failed` (none exist).

## Exact commands run

```
cargo test --lib ui::tests        # confirmed the expected pre-implementation failure
cargo fmt                          # no diff produced
cargo clippy --all-targets -- -D warnings   # clean, "Finished ... 5.90s", zero warnings
cargo test --lib ui::             # 332 passed, 0 failed
cargo test                        # full suite, 0 failed across all files
git diff --check                  # no whitespace errors
```

## Deviations from the brief

None in code or test content — every snippet (field, doc comments, function, tests, loop replacement) was taken verbatim as specified. The only judgment call was where exactly in the `App` struct body to place `copy_mode` (brief said "近 `scroll` 字段附近" for the field but the code block it gave showed it standing alone); I placed it at the very end of the struct, after `explained_failure`, matching the position of its counterpart entry in `new_inner`'s initializer list added right after `explained_failure: None,`. This does not affect behavior, only source layout.

Did not touch `src/proto.rs`, `src/pty.rs`, `src/session.rs`, or `src/daemon.rs` — no need arose; `ScrollState`'s `Default` derive and `agent_owns` field already existed as described.

Kept the pre-existing uncommitted `README.md` / `README.zh-CN.md` edits, the untracked `docs/...` file, and `scripts/` out of the commit — only `src/ui/app.rs` and `src/ui/mod.rs` were staged and committed.

## Uncertainties

None outstanding. The one thing worth flagging for later tasks (not this one): `copy_mode` is now a live field but nothing yet sets it to `true` (no F4 handler exists in `attach.rs` yet) — that wiring is presumably a later task in this 4-task plan, and this task's brief only asked for the field, the predicate, and the loop's consumption of it, not the key binding.

---

## Fix round 1: `a_failed_screen_call_does_not_flip_the_capture_state` could not fail

Commit: `b62d7cb` — "fix: make the disconnect-preserves-scroll test actually able to fail"

**Diagnosis, confirmed.** The original test set `app.connected = false` and then recomputed `wants_mouse_capture(true, app.scroll.agent_owns, app.copy_mode)` a second time with byte-identical arguments (`app.scroll` was never touched by the test, and `wants_mouse_capture` never reads `app.connected`). It was structurally guaranteed to pass regardless of what `run()`'s disconnect arm actually does.

**Option chosen: Make it real.** The `Screen`-response handling in `run()` turned out to have a small, cleanly separable piece: the line that assigns `app.scroll` from the `Response::Screen` match. I extracted it into:

```rust
fn scroll_after_screen_call(
    previous: crate::session::ScrollState,
    result: &Result<Response>,
) -> crate::session::ScrollState {
    match result {
        Ok(Response::Screen { scroll, .. }) => *scroll,
        _ => previous,
    }
}
```

In `run()`, the call site now reads the `Screen` response into a local (`screen_result`), computes `app.scroll = scroll_after_screen_call(app.scroll, &screen_result)` immediately (independent of the rest of the match), then matches on `screen_result` as before for the rest of the branch's work (`session_ended_notice`, `Failed`-state explanation caching, etc.) — that part of the match is untouched, just no longer also carries the `scroll` field (changed to `..` in the `Ok(Response::Screen { .. })` pattern since it's already handled).

This is a genuinely small extraction — one field's worth of logic — not a restructuring of the tangled surrounding match (session-ended notice, failure explanation caching, `sent_size` renegotiation all stayed exactly where they were).

Replaced the old test with two that call `scroll_after_screen_call` directly:
- `a_failed_screen_call_does_not_flip_the_capture_state` — feeds `Err(anyhow!(...))`, asserts the return equals the `previous` passed in.
- `a_successful_screen_call_replaces_the_scroll_state` — feeds `Ok(Response::Screen { scroll: fresh, .. })`, asserts the return equals `fresh`, not `previous`. Added so the pair can't both be satisfied by a constant-function cheat (e.g. `|previous, _| previous` would pass the first test but fail this one).

**Mutation verification.** Changed the failure arm from `_ => previous` to `_ => crate::session::ScrollState::default()` (simulating exactly the regression the coordinator described — the disconnect path resetting scroll state), reran `cargo test --lib ui::tests::a_failed_screen_call_does_not_flip_the_capture_state`:

```
thread 'ui::tests::a_failed_screen_call_does_not_flip_the_capture_state' panicked:
assertion `left == right` failed
  left: ScrollState { agent_owns: false, alt_screen: false, max: 0, offset: 0, new_lines: 0 }
 right: ScrollState { agent_owns: true, alt_screen: false, max: 10, offset: 3, new_lines: 0 }
test result: FAILED. 0 passed; 1 failed
```

Went red as expected. Reverted the mutation (`cp` from a pre-edit backup in the scratchpad dir), confirmed `_ => previous,` restored, and reran the full suite green.

**Verification commands, post-fix:**
```
cargo fmt                                    # no diff
cargo clippy --all-targets -- -D warnings    # clean
cargo test                                   # lib: 634 passed, 0 failed (was 633; net +1 test:
                                              # replaced 1, added 1 companion); every integration
                                              # test file 0 failed
git diff --check                             # clean
```

No other test claimed coverage of the disconnect-preserves-scroll path — grepped for other references to `agent_owns`/`app.scroll` assignment in tests and found none touching the `Screen`-call failure arm.

Kept README edits and other unrelated working-tree files out of this commit as before; only `src/ui/mod.rs` was staged.
