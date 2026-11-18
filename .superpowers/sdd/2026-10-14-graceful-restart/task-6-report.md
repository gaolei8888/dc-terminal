# Task 6 report: daemon-side drain (protocol 25)

Status: DONE. Commit `0ded584`.

## What was built

- **proto.rs**: `PROTOCOL_VERSION` 24 -> 25, with a comment that also counts Task 5's `Request::Submit`. New `Request::Drain { wait_s, nudge_wait_s, initiator }`, `DrainStatus { renew }`, `DrainCancel`, and `Response::DrainProgress { progress, done }`. Hand-written `Debug` arms. The request shape-pin test now includes the three new variants and pins 25. Seven other response pin tests had a literal `24`; all flipped to 25.
- **session.rs**:
  - `SessionManager::resumes_by_id(id) -> bool`.
  - `last_sessions_frozen: Mutex<bool>`. `persist_last_sessions_with` does nothing while frozen, and holds the lock while it writes.
  - `persist_final_last_sessions(interrupted, handoffs)` writes the file and then freezes it, all under that same lock. `unfreeze_last_sessions()` undoes the freeze.
  - `persist_last_sessions` is now `pub(crate)` so tests can call it.
- **daemon.rs**:
  - `run_with_manager` now calls the new `run_with_hooks(socket, mgr, DrainHooks)`.
  - `DrainHooks { lease, tick, linger, swap, exit }`. `DrainHooks::real()` uses lease 10s and tick 500ms. Its `swap` calls `client::spawn_swap(process::id(), socket, current_exe)` and its `exit` calls `process::exit(0)`.
  - `DrainCtl` (`Mutex<Option<DrainRun>>` plus a generation counter) handles start, status, cancel and the drain thread:
    - Members are built from `mgr.list()` and `resumes_by_id`.
    - `handoff_dir` is `<socket dir>/handoffs` (created with `create_dir_all`).
    - The thread runs `step` once per tick. `AskHandoff` and `Nudge` go through `bridge::submit_unchecked`; if sending fails it only logs to stderr.
    - When the lease expires it cancels.
    - On `Finish`: `persist_final_last_sessions` -> `swap`. If that succeeds it sets `done = true`, waits up to `linger` (2s) for a poll to see `done:true`, then calls `exit`. If it fails it logs, unfreezes, clears the run and sets `set_draining(false)`, and does **not** exit.
  - The Drain* requests are routed in `serve` (local socket only, the same as Mesh*). `handle` (the HTTP path) refuses them with `BadRequest`.
- **voice/mod.rs**:
  - New pure function `submit_requests(protocol, id, text)`. Protocol >= 25 gives `[Submit]`. Anything older, or `None`, gives `[Input(text), Input("")]`; an empty text gives just `[Input("")]`.
  - `SocketDaemon::type_into` asks `Hello` once on each connection (each type_into opens one), then sends the planned requests on that same connection. It asks every time because the daemon may have just been swapped.
- **UI** (`ui/mod.rs`, `ui/app.rs`, `i18n.rs`):
  - `poll_drain` sends `DrainStatus { renew: false }` once a second whenever connected.
  - `drain_banner_from` decides whether to draw: only when a `DrainProgress` comes back with a non-empty `waiting`. An error from an old daemon or a disconnect draws nothing.
  - The banner is drawn as an overlay on the top row of the content area, so the content area does not change size and the agent gets no resize.
  - Text comes from `msg::restart_pending_banner(lang, n)`: 「dct 准备重启：还在等 N 个会话」.

## TDD evidence

- RED: wrote the tests first. `cargo test --lib daemon::` then failed to compile with 45 errors: missing `Drain*` variants, `DrainHooks`, `run_with_hooks`, `submit_requests`, `App::drain`, `drain_banner_from`, and `persist_last_sessions` being private.
- GREEN: `cargo test --lib drain` gives 34 passed, including all 9 `daemon::drain_tests`. Ran the drain_tests 3 more times: 9/9 each time, about 2.4s.
- Mutation checks (each change reverted afterwards):
  - Disabling the freeze check makes `drain_nudges_busy_session_and_marks_it_interrupted` fail.
  - Making `renew:false` renew the lease makes `status_polls_without_renew_do_not_extend_the_lease` fail.
- Full `cargo test`: lib 1863 passed, 1 ignored; every integration test binary is green.
- `cargo +1.99.0 clippy --all-targets -- -D warnings` is clean. One `type_complexity` warning was fixed with a `SwapFn` alias.

Tests added:

- `daemon::drain_tests` (unix only):
  - all idle -> ids persisted, not interrupted, no handoff, swap and exit each called once
  - a second Drain returns progress and does not restart
  - cancel puts input back to normal
  - the lease expires when nobody polls
  - `renew:false` polls do not extend the lease
  - `renew:true` polls keep the drain alive
  - a busy session gets nudged and is marked interrupted, plus the freeze check
  - the initiator is not waited for or nudged, and is marked interrupted
  - a failed swap cancels the drain, does not exit, and unfreezes the file
- `daemon::tests::drain_requests_are_refused_off_the_local_socket`
- `voice::tests::submit_falls_back_to_two_inputs_for_an_old_daemon`
- `ui::tests::a_pending_restart_is_announced_on_the_top_row` (Board and Attached views)
- `ui::tests::no_pending_restart_draws_no_banner`
- `ui::tests::drain_banner_state_follows_the_daemon_answer`

## Deviations from the brief / plan (source wins)

1. **No daemon test used `run_with_manager` with a temp socket** as the brief assumed. Tests start `run_with_hooks` in a thread on `<tempdir>/d.sock`, the same way `ui/settings_view.rs` starts `daemon::run`. The sessions are created directly on the shared `Arc<SessionManager>`.
2. **Hooks are a struct with boxed closures, not a trait.** They have two parts, `swap` and `exit`, plus `linger`. That way the fake can let the "after finish" path run all the way through without exiting the process.
3. **The freeze is a separate `Mutex<bool>`, not "`is_draining()` and finished".** Normal persists check it and write while holding it. The final write sets it under the same lock, so a normal persist that is already running cannot overwrite the final file.
4. **The banner text is a `msg::` function, not a new `Key`**, because it takes a number (the existing pattern, like `live_on_air`).
5. **`DrainCancel` after Finish has started** returns the current `DrainProgress` and does not cancel. The spec says "第 5 步开始后不能取消"; the brief only described the normal `Ok` case.
6. **The spec talks about `wait_ms` / `nudge_wait_ms`**, but the brief's interface uses `wait_s` / `nudge_wait_s`. I followed the brief.
7. **`drain_tests` are `#[cfg(all(test, unix))]`.** They switch BUSY/IDLE with clear-screen escapes, and the Windows ConPTY redraws the screen, so the timing there is not reliable. The drain logic is covered on both platforms by the pure `drain.rs` tests, and the HTTP-refusal test is cross-platform.

## Self-review

- Lock order: the drain run lock is never held while calling into submit or persist. `step` runs under the lock, and the actions run after it is released. `persist_final_last_sessions` takes frozen -> sessions -> session, the same order as normal persists. `persist_last_sessions` is never called while a session lock is held (existing invariant).
- When a run is cancelled or replaced, its thread sees the generation has changed and exits. It never touches a newer run.
- `start` takes the member snapshot and calls `set_draining(true)` before the thread starts. The response is computed from the new run (`done:false`).
- `done_seen` is set when a `DrainStatus` sees `done`. The real exit waits at most 2s for it. `__swap` waits up to 10s for the old daemon to leave, so this fits.
- Nothing touches `~/.dct`. All paths come from the temp socket, and no real `__swap` is spawned in tests.

## Concerns for the controller

- **The "not draining" reply looks the same as "draining with nobody left to wait for".** Both are `DrainProgress{done:false, waiting:[]}`, as the brief specified. In the gap between the last session stopping and the next tick reaching Finish (at most one tick, 500ms), a running drain answers with an empty `waiting`. If the Task 8 CLI treats "empty waiting and not done" as cancelled, it can stop early by mistake. Suggestions for Task 8: also look at `remaining_s > 0` (a running drain almost always has some), or only conclude "cancelled" after two empty answers in a row. If you would rather fix it in the protocol, a `#[serde(default)] active: bool` field would need a decision now, while we are already on 25.
- **After the binary is installed and before the daemon restarts, an old 24 daemon** gets the new UI's `DrainStatus` poll and answers with an error, so no banner is drawn (as intended). The CLI (Task 8) still has to handle an old daemon that rejects `Drain` itself.
- **The UI does not cancel on disconnect.** Only the CLI lease does. The spec says "CLI 断开连接…算取消"; with the lease, that takes effect at most 10s after the CLI goes away, not instantly.

---

## Fix round 1

Coordinator ruling: add `active` to `DrainProgress`. This resolves concern 1 above.

- **[Important] `Response::DrainProgress { progress, done, active }`** (still protocol 25).
  - `active: true` means a drain run exists, including the finishing window. The daemon's `progress_of` always sets it to true.
  - `active: false` means no run exists: nothing ever started, it was cancelled, the lease expired, or the swap failed.
  - What `done` means is unchanged.
  - The proto doc comment now says the CLI must use `active`, not an empty `waiting`, to tell that a drain was cancelled.
  - New pin test: `proto::tests::the_drain_progress_shape_is_pinned`.
- **[Minor] UI banner.** `drain_banner_from` now shows the banner whenever `active` is true.
  - While sessions are still running: `msg::restart_pending_banner` 「dct 准备重启：还在等 N 个会话」.
  - When `waiting` is empty (the finishing window): the new `msg::restart_now_banner` 「dct 正在重启……」 (EN "dct is restarting…").
- **[Minor]** Removed the fixed 300ms sleep in `drain_with_all_idle_persists_ids_and_reports_done`; `wait_state` already polls until the session is Idle.

Tests:

- RED: the new and updated tests failed to compile (8 errors, no field `active`).
- GREEN:
  - `daemon::drain_tests`: 11/11, ran 3 times.
  - `--lib drain`: 37 passed.
  - `proto::`: 29 passed.
  - `ui::tests`: 89 passed.
  - Full `cargo test`: lib 1867 passed with 1 ignored, every binary green.
  - `cargo +1.99.0 clippy --all-targets -- -D warnings`: clean.
- New or changed tests:
  - The reply to `Drain` when all sessions are already idle has `active: true`.
  - `not_running` now also requires `active: false`. It is used after cancel, after the lease expires, and after a failed swap.
  - `poll_until_done` requires `active: true`.
  - `nothing_started_means_not_active`: covers both `renew:false` and `renew:true` polls.
  - `the_finishing_window_is_still_active`: the fake swap blocks on `Calls::hold_swap`. While it is blocked, `DrainStatus` returns `active:true, done:false, waiting:[]`, and `DrainCancel` is refused (it returns progress with `active:true`). After releasing the swap, the drain reaches `done`.
  - `ui::tests::drain_banner_state_follows_the_daemon_answer`: updated for `active`.
  - `ui::tests::the_finishing_window_says_restarting`: new.
