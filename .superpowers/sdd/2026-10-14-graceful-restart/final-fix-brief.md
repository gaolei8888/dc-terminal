# Final fix wave — graceful restart (branch feat/graceful-restart, BASE 7d2a50d)

Source of findings: final-review.md (same folder; read it — it has file:line and suggested fixes) and task-10-report.md. Binding spec: docs/superpowers/specs/2026-10-14-graceful-restart-design.md. Rulings in progress.md override suggestions in the review.

## Fix all of these

### From final-review.md
- **C1** After a restart an open UI window reconnects to the same session number, now a different session. Required: the UI must detect it is talking to a new daemon instance (e.g. a daemon instance id / start time / pid in Hello or Screen) and go back to the board with a plain message (zh/en). Additionally restore each session's saved number on restore if that is cheap and safe (persist `id` in RecordedSession with serde default; fall back to fresh numbers on collision). Test both.
- **I1** codex post-restart note (and every drain-time typing: handoff request, nudge, continue note, read-handoff note) is typed before the agent is ready. Gate delivery on: session Idle continuously for ≥3 s AND ≥5 s since spawn; on Unix additionally require the pty is out of canonical mode (ICANON clear via portable-pty's termios access on the master) — verify that reading works on macOS with a focused test (spawn `sh -c 'stty raw -echo; sleep 5'` vs plain `sleep 5`); if it does not work reliably, keep only the timing rule and say so in the report. Do NOT retype text; never retry after a partial send (deferred minor from Task 9).
- **Unknown-state sessions (ruling):** sessions whose state is Unknown (profile cannot tell busy/idle, e.g. qwen) are not waited on and not nudged; record them interrupted=true. Their post-restart continue note is delivered once the screen has been unchanged for ≥5 s (and the I1 timing gate), not dropped with a warning.
- **I2** Claude sessions resumed via `--continue` (no id) can take over a sibling's conversation. Required: a recorded session with no conversation_id on a profile that has resume_id_args gets `--continue` only if no sibling in the same (dir, profile) group has an id; otherwise it starts fresh and is treated as unresumable for the drain (gets a handoff request). New sessions always get an id (already).
- **I4** Agent-run `dct restart` gets cancelled after ~2 min when the shell kills the CLI. Required: when the Drain carries an initiator session id, a CLI disconnect does NOT cancel the drain. Add `dct restart --cancel` (sends DrainCancel) and mention it in `--help` text and both READMEs.
- **I5** The initiator is told to continue (= run dct restart again). Required: persist a flag for the initiator; after restart send it instead (zh/en, plain): "你要的 dct 重启已经完成了，不用再重启。请接着做重启之前手上的事。" / English equivalent.
- **I6** Swap relaunches the daemon's own path. Required: the CLI sends `std::env::current_exe()` with the Drain request (optional field, serde default); swapper uses it; fallback = daemon's own path with a trailing " (deleted)" stripped.
- **I7** New daemon answers only after restoring all sessions; waits too short. Required: the new daemon must answer Hello before/while restoring (preferred), and the CLI/swapper waits ≥30 s for Hello. If answering early is not feasible, scale the wait with session count and print progress. Explain which in the report.
- **I8** Mesh messages held during drain are lost at restart. Required: persist held mesh messages with the final state and re-queue them after restore (target by the restored session), or if that is disproportionate, refuse new mesh deliveries during drain with the same "dct 正在准备重启，稍后再发" behaviour as phone/voice AND release the already-held ones before the final save. State which in the report.
- **M1** On cancel after nudge/handoff requests were typed, send those sessions (via the gated path) "dct 不重启了，请接着做刚才的事。" / English, and list them in the CLI cancel message.
- **M2** Re-check the drain generation under the lock immediately before each typed action.
- **M4** After the final save, stop every live session explicitly (kill_now) before the old daemon exits.
- **M6** If the initiator session's profile is unresumable, the CLI prints one line warning that this session's conversation won't come back and it should write its own handoff first.
- **M7** At daemon start, delete files in ~/.dct/handoffs older than 30 days.

### From task-10-report.md and Task 10 review
- `--now` must skip handoff requests and must not print a progress/"剩 0 秒" line (spec §3).
- Handoff filenames use local time.
- READMEs (both): say the first restart from an older dct (protocol < 25) is the old plain restart; add a `-y` row; quote the phone/voice refusal in English consistently; document `--cancel`; add the known limit that after `/clear` or `/resume` inside Claude a restart brings back the conversation dct started (parked I3).

### Deferred minors from earlier tasks (ledger)
- Ctrl+C during the swap wait: print once that it is too late to cancel.
- Unreadable old pid ⇒ premature RestartDone: fix.
- Swap-wait polling 50 ms → 250 ms.
- Comment in resume_notes noting a note can be delivered before the watcher window closes (harmless).

## Do NOT fix (parked by ruling)
I3 (/clear stale id — document only), M3 (stale DCT_SESSION_ID), M5 (half-typed draft).

## Constraints
- Rust; commit messages in English, NO Co-Authored-By or AI attribution lines.
- Before finishing: `cargo test` green and `cargo +1.99.0 clippy --all-targets -- -D warnings` clean. Do NOT run `cargo fmt` (repo is not fmt-clean; it reformats ~70 unrelated files) — format your own lines by hand.
- Generic code only: no branching on profile names.
- User-facing text plain Chinese for non-programmers + English.
- NEVER touch the real daemon (~/.dct/daemon.sock), ~/.dct, or ~/.local/bin. Any manual test uses HOME=<short temp dir> (macOS socket path ≤ ~100 chars).
- Tests that can't fail are defects: for each fix add a test that fails without it (mutation-check the important ones).
- Several small commits are fine.
