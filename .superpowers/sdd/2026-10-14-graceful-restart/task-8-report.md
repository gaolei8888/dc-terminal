# Task 8 report: `dct restart` new flow

Status: DONE
Commit: a534af4 feat(restart): dct restart waits for sessions to stop with live progress and Ctrl+C cancel; --now, --wait and --nudge-wait; falls back on old daemons

## What changed

- `src/cli.rs`
  - `Restart` is now `Run { ask, now, wait_s, nudge_wait_s } | Usage(String)`. Kept the source name `parse_restart_args(&[String], Lang)` (the brief's tests called it `parse_restart(&[&str])`). Defaults 900 / 180 s (`DEFAULT_WAIT_S`, `DEFAULT_NUDGE_WAIT_S`). `--wait`/`--nudge-wait` take minutes (checked_mul, so overflow → Usage); missing or non-numeric value → Usage; `--now` zeroes both waits no matter the order of flags; unknown arg → Usage (still never restarts).
  - `Plan { Graceful, Plain }` + `plan_for(Option<u32>)`: `>= 25` (`DRAIN_PROTOCOL`) → Graceful, `< 25` or `None` → Plain.
  - `initiator_from(Option<String>)` parses `DCT_SESSION_ID` (`session::SESSION_ID_ENV`).
  - `progress_line(&Progress, &HashMap<u32,String>, Lang)`: e.g. 「还在等 2 个会话（dc-octo、9 号），剩 9 分 12 秒」; nudged phase 「还在等 1 个会话（9 号），已请它们停下，最多再等 2 分 40 秒」; empty `waiting` (finishing window) → `RestartDrainFinishing`. Names are `SessionInfo::tag` from `List`; empty tag → number.
  - `ProgressOut<W>`: terminal → `\r` + line + space padding over the previous width, clipped to terminal width (long name lists would wrap and break `\r`). I used padding instead of an ANSI clear-line escape so old Windows consoles behave the same. Not a terminal → prints a new line only when (waiting set, nudged) changes. Literal text comparison would print one line per second, because the seconds change every tick.
  - `run_restart`: no daemon → `StartFresh` as before. Otherwise it sends `Hello` → plan. On Plain it prints 「这次后台太旧，不会等会话停下，直接重启」. It lists the sessions, and with `ask` prints the explanation: graceful and not `--now` uses 「会先等正在干活的会话停下（最多 N 分钟），再重启；重启后每个会话接回原来的对话。」 with N = ceil((wait+nudge)/60); otherwise it uses the old `RestartExplain`. Then y/n. Plain then goes to the old `restart_daemon` path unchanged.
  - `drain_loop` (testable core, takes a `call` closure, the cancel flag, an `old_gone` probe and the tick) + `drain_and_swap`. It sends `Drain`, then sends `DrainStatus{renew:true}` every 1 s, and checks Ctrl+C every 100 ms.
    - `done:true` → swap phase.
    - `active:false` → if the old daemon is gone, this is a missed `done` and the swap phase runs; otherwise it prints `RestartDrainAborted` and exits 1.
    - Ctrl+C → `DrainCancel`: `Ok` → 「已取消，一切照旧」, exit 0. `DrainProgress` (the daemon refuses once finishing) → 「已经在换了，取消不了，马上就好」 and it keeps waiting.
    - Connection error → swap phase if the old daemon is gone; after 3 consecutive errors while it is still alive → aborted.
    - Swap phase: prints 「正在换新的后台……」, then `wait_up_to(20s)` until the old daemon is gone and a new `Hello == PROTOCOL_VERSION` comes back. Then `RestartDone`, or `RestartNewDaemonNotUp`, exit 1.
- `src/sys/signal.rs`: new `catch_ctrl_c() -> &'static AtomicBool`. Unix uses `sigaction(SIGINT)` with `SA_RESTART` and only stores to an atomic in the handler. Windows uses `SetConsoleCtrlHandler` for CTRL_C/CTRL_BREAK and returns 1, leaving other events to the default. The existing raw-mode hook could not be reused because it `_exit`s.
- `src/client.rs`: `wait_up_to` is now `pub(crate)`.
- `src/i18n.rs`:
  - 7 new Keys with zh and en: RestartDrainFinishing, RestartOldDaemonPlain, RestartDrainCancelled, RestartCancelTooLate, RestartDrainAborted, RestartSwapping, RestartNewDaemonNotUp. Added to ALL_KEYS; the count went from 212 to 219.
  - New msg fns: `restart_bad_arg` (replaces `restart_takes_no_args`, which said "takes no arguments"), `restart_needs_minutes`, `restart_graceful_explain`, `session_number`, `drain_waiting`.
- `src/main.rs`: help text and comment for `restart`.

## Deviations from the brief (source/behaviour wins)

1. **Failure message after `done`.** The brief said to print `RestartFailed` plus 「看 ~/.dct/daemon.log」, but `RestartFailed` says 「旧的还在跑」. That is false after `done`, because the old daemon has already exited. I used the new key `RestartNewDaemonNotUp` instead: 「新的后台没起来，原因看 ~/.dct/daemon.log；再敲一次 `dct` 会照常起后台、接回会话。」
2. **"Old daemon is gone" check.** Liveness alone is not enough, and the existing integration test `tests/cli.rs::restart_with_yes_swaps_the_daemon_process` caught this. The old daemon there is an un-reaped child of the test, a zombie, so `alive(pid)` stays true forever. The old daemon still counts as gone if `!alive(pid)`, and now also if the pid answering on the socket differs from the old pid. Pid comparison is needed in any case: the old daemon lingers about 2 s after `done` and answers `Hello` with the same protocol number.
3. Test names: the parse test asserts more cases than the brief, including flag order with `--now`, a missing value, a negative value and overflow.

## Tests

- New unit tests in `cli::tests`:
  - `parse_restart_args_reads_flags_and_minutes`
  - `progress_line_names_sessions_and_time` (zh/en, nudged, under one minute, finishing)
  - `old_daemon_falls_back_to_plain_restart`
  - `initiator_comes_from_the_session_env`
  - `progress_out_without_a_terminal_prints_a_line_only_on_change`
  - `progress_out_on_a_terminal_rewrites_one_line`
  - `drain_loop_renews_until_done`
  - `drain_loop_ctrl_c_cancels`
  - `drain_loop_cancel_too_late_keeps_waiting`
  - `drain_loop_inactive_means_aborted_unless_the_old_daemon_is_gone`
  - `drain_loop_connection_loss`
- The existing integration test `restart_with_yes_swaps_the_daemon_process` uses a real daemon in a temp HOME. It now goes end to end through Drain → `__swap` → the new daemon, and passes in about 13 s.
- `cargo test`: all pass (lib 1877 passed, integration suites all ok).
- `cargo +1.99.0 clippy --all-targets -- -D warnings`: clean.
- Windows: `cargo clippy --all-targets --target x86_64-pc-windows-msvc` compiles. It reports 3 warnings, all in untouched files (`llm/cli.rs:48`, `student_projects.rs:669`, `game/identify.rs:22`). They are not from this task, but with `-D warnings` they would fail a Windows clippy run if CI does one. The 1.99.0 toolchain has no Windows target installed, so I checked with the default toolchain.
- rustfmt: my hunks match rustfmt. The remaining fmt diffs in cli.rs (6) and i18n.rs (2) were there before this task; no repo-wide fmt.

## Concerns

- In practice, `NEW_DAEMON_WAIT` = 20 s (per the brief) can be slightly shorter than the swapper's worst case: 10 s for the old daemon to exit, about 2 s for a hard kill, then 10 s for the new one. In that worst case the CLI reports failure while the new daemon is still coming up. This does no harm, because the swapper still finishes.
- Ctrl+C at the y/n prompt still kills the process the default way; the handler is installed only once the drain starts. This is harmless because nothing has been sent yet.
- Not run against a real multi-session daemon (that is Task 10).
