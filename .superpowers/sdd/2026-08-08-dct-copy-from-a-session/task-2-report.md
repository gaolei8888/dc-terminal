# Task 2 report: F4 复制模式

## What changed

- `src/ui/attach.rs::handle_key` — added an `F(4)` branch between the `F3`
  branch and the `key_scroll` branch that toggles `app.copy_mode`:
  `app.copy_mode = !app.copy_mode;`, with the comment text from the brief
  explaining the F4 choice and why the toggle doesn't touch the terminal
  directly.
- `src/ui/mod.rs::enter_session` — added `app.copy_mode = false;` right after
  the existing `app.explained_failure = None;` line, with the brief's comment
  explaining why the reset lives in the entry funnel rather than on the three
  exit paths.
- `src/ui/attach.rs` test module — added:
  - `attached_app()` helper (`App::test_app()` + `app.view = View::Attached(1)`).
  - `f4_toggles_copy_mode_and_is_never_forwarded_to_the_agent` — presses F4
    twice, asserts `copy_mode` flips true then false, and asserts
    `crate::ui::key_to_input(&key(KeyCode::F(4)))` is `None` (F4 must never
    reach the agent).
  - `entering_a_session_always_starts_outside_copy_mode` — sets
    `app.copy_mode = true`, calls `crate::ui::enter_session(&mut app, 1)`,
    asserts `copy_mode` is false afterward.

Path spelling: used `crate::ui::key_to_input` / `crate::ui::enter_session`
instead of the brief's `super::super::` form (both compile; `crate::ui::`
reads more clearly from inside `attach::tests`, per the task instructions
that this substitution is allowed).

No changes to `src/proto.rs`, `src/pty.rs`, `src/session.rs`, `src/daemon.rs`.
Did not touch `attach::handle_key`'s `F2` branch, the main loop's
`session_ended_notice` region, or `back_one_level`. No `leave_session`
function created (confirmed it doesn't exist and isn't needed — the brief's
mention of it was a stale draft artifact per the task instructions).

## Test commands run

1. Before Step 3/4 (tests only, to confirm red):
   `cargo test --lib ui::attach::tests`
   → `f4_toggles_copy_mode_and_is_never_forwarded_to_the_agent` FAILED
     (panicked "第一下打开"); `entering_a_session_always_starts_outside_copy_mode`
     FAILED (panicked "上一个会话的复制模式不能粘到下一个会话"). 26 passed, 2 failed.

2. After Step 3/4 (full suite):
   `cargo test`
   → 636 passed in the lib target, 0 failed; all integration-test binaries
     (attach_grid, disconnect_*, signal_restore, slow_input, socket_perms,
     zombie_reaping, etc.) also 0 failed.

3. `cargo fmt` — ran once (no diff produced beyond the intended edits);
   `cargo fmt --check` after mutation-revert — clean.

4. `cargo clippy --all-targets -- -D warnings` — clean, no warnings.

5. `git diff --check src/ui/attach.rs src/ui/mod.rs` — clean (no whitespace
   errors).

## Mutation test evidence

**Mutation 1** — replaced `app.copy_mode = !app.copy_mode;` in the F4 branch
with a no-op comment (`// MUTATION TEST: no-op instead of toggling`).
Ran `cargo test --lib ui::attach::tests::f4_toggles_copy_mode_and_is_never_forwarded_to_the_agent`:

```
thread '...f4_toggles_copy_mode_and_is_never_forwarded_to_the_agent' panicked at src/ui/attach.rs:365:9:
第一下打开
test result: FAILED. 0 passed; 1 failed;
```

Confirmed red, then reverted the mutation back to `app.copy_mode = !app.copy_mode;`.

**Mutation 2** — replaced `app.copy_mode = false;` in `enter_session` with a
no-op comment (`// MUTATION TEST: reset removed`).
Ran `cargo test --lib ui::attach::tests::entering_a_session_always_starts_outside_copy_mode`:

```
thread '...entering_a_session_always_starts_outside_copy_mode' panicked at src/ui/attach.rs:389:9:
上一个会话的复制模式不能粘到下一个会话
test result: FAILED. 0 passed; 1 failed;
```

Confirmed red, then reverted the mutation back to `app.copy_mode = false;`.

After both reverts: `cargo fmt --check`, `cargo clippy --all-targets -- -D
warnings`, and `cargo test` all green again (636 lib tests passed, 0 failed).

## Commit

- Staged only `src/ui/attach.rs` and `src/ui/mod.rs` by name (never touched
  `README.md` / `README.zh-CN.md`, which carry the user's own uncommitted
  edits; also left the pre-existing uncommitted `.superpowers/sdd/.gitignore`
  change untouched — not part of this task).
- Commit: `dd5220f` — "feat: F4 hands the mouse back to the terminal so you
  can select and copy" (English message, no Co-Authored-By line).
  `2 files changed, 57 insertions(+)`.

## Concerns

None. The diff matches the brief's Step 3/Step 4 code blocks verbatim
(comments and all), both new tests are demonstrated to be able to fail via
mutation, and the full test suite plus fmt/clippy are green.

---

## Review fix round 1

Coordinator relayed a code review with one blocking finding (the brief's
"`enter_session` is the only funnel" premise was wrong — its own author
confirmed this against `src/ui/mod.rs:241-242` and the four production call
sites that reach `View::Attached` via `create_session` instead) plus two
Minors explicitly left alone (the one-use `attached_app()` helper, and the
`enter_session` test living in `attach.rs` rather than `mod.rs`).

### What changed

- `src/ui/mod.rs::create_session` (the common ancestor of `pick.rs:78`
  Start, `pick.rs:129` Install/shell, `mod.rs:323` secret-verified create,
  and `mod.rs:1676` quick-start `n`/`N`) — added `app.copy_mode = false;`
  right before the `r` return, unconditional on whether creation actually
  succeeded (a failed create never reaches `View::Attached` anyway, so
  resetting regardless is harmless and avoids an extra branch). New comment
  explains this is one of the *two* entry constructors, not a standalone fix.
- `src/ui/mod.rs::enter_session` — rewrote the comment above
  `app.copy_mode = false;` to drop the false "only funnel" claim. It now
  says the entry side has exactly two constructors (`enter_session` and
  `create_session`) that together cover every path into `View::Attached`,
  while the exit side has three paths, one of which (Ctrl+Q) goes through
  the shared `back_one_level` — not worth re-signing for one bool.
- `src/ui/attach.rs::entering_a_session_always_starts_outside_copy_mode`
  docstring — same correction: removed the "only funnel" wording, now
  points at `create_session_resets_copy_mode_for_a_freshly_created_session`
  as the sibling test covering the other constructor. Test itself (and its
  location in `attach.rs`) left untouched per the coordinator's Minor
  exclusion.
- `src/ui/mod.rs` test module, next to the other `create_session` tests —
  added `create_session_resets_copy_mode_for_a_freshly_created_session`:
  starts a real daemon (`start_daemon_for_test`), sets `app.copy_mode = true`,
  calls `create_session(&mut app, &dir, "shell", false)`, asserts the create
  succeeded and `app.copy_mode` is now `false`.

No changes to `src/proto.rs`, `src/pty.rs`, `src/session.rs`, `src/daemon.rs`.

### Test commands run

- `cargo test --lib create_session_resets_copy_mode_for_a_freshly_created_session`
  → 1 passed (after the fix).
- `cargo fmt` — reformatted (collapsed one `assert!` onto one line; no
  semantic change). `cargo fmt --check` clean after mutation-revert.
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test` (full suite) → lib target 637 passed, 0 failed (up from 636 —
  the one new test); all integration-test binaries also 0 failed.
- `git diff --check src/ui/attach.rs src/ui/mod.rs` — clean.

### Mutation test evidence

Replaced `app.copy_mode = false;` in `create_session` with
`// MUTATION TEST: reset removed`. Ran
`cargo test --lib create_session_resets_copy_mode_for_a_freshly_created_session`:

```
thread 'ui::tests::create_session_resets_copy_mode_for_a_freshly_created_session' panicked at src/ui/mod.rs:2431:9:
上一个会话的复制模式不能粘到新建的这一个上
test result: FAILED. 0 passed; 1 failed;
```

Confirmed red, then reverted the mutation back to `app.copy_mode = false;`
with its comment. Re-ran `cargo fmt`, `cargo clippy --all-targets -- -D
warnings`, and `cargo test` — all green (637 lib tests passed).

### Commit

- Staged only `src/ui/attach.rs` and `src/ui/mod.rs` by name (README.md /
  README.zh-CN.md / the pre-existing `.superpowers/sdd/.gitignore` change
  left untouched, same as round 1).
- Commit: `23eb2b8` — "fix: reset copy_mode in create_session, not just
  enter_session" (English message, no AI attribution).
  `2 files changed, 52 insertions(+), 8 deletions(-)`.

### Concerns

None. Did not act on the two Minor findings (both brief-mandated, per the
coordinator's instruction to leave them). All four previously-leaking sites
(`pick.rs:78`, `pick.rs:129`, `mod.rs:323`, `mod.rs:1676`) now route through
`create_session`, which resets `copy_mode` unconditionally, so the leak
described in the review's repro (`F4` → `F2` → `n` landing with stale
`copy_mode = true`) is closed.
