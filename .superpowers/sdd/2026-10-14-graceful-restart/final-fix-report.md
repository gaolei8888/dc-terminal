# Final fix wave report — graceful restart

Status: DONE_WITH_CONCERNS (concerns at the end; none block merge)

BASE 7d2a50d. Commits:
- `5d14d0e fix(restart): final review wave - keep session numbers, gate typing, survive agent-run restarts`
- `477befc docs(restart): document --cancel, -y, agent-run hand-off, first restart from an older dct, and the /clear limit`

Checks: `cargo test` green (1931 lib + all integration suites, run twice in a row clean).
`cargo +1.99.0 clippy --all-targets -- -D warnings` clean. `cargo fmt` not run.

"Mutation-checked" = I changed the production line to the broken behaviour, ran the named test(s),
saw them fail, and restored the line (script-driven, every mutation listed was KILLED).

## Items

### C1 — UI types into the wrong session after a restart
- **Saved numbers.** `RecordedSession.id: Option<u32>` (serde default). `write_last_sessions` now sorts by
  id and writes `id`. Restore calls the new `SessionManager::restore(.., want_id)`; the id is used when
  free, `next_id` is pushed past it (`fetch_max`), otherwise a fresh number. Before the restore thread
  starts, `reserve_ids_up_to(max saved id)` so sessions the user opens during restore cannot take a
  saved number. The failed-resume replacement in `watch_resumed_by_id` deliberately keeps taking a
  **fresh** number: the pending "continue" note is queued by the old number and must not land in the
  new conversation.
- **New-daemon detection.** `Hello` is unchanged. New `Request::Instance` → `Response::Instance { id }`
  (random per boot, `getrandom`, falls back to time×pid). `Client::guard_instance()` (enabled only by
  `ui::run`) asks the instance on every reconnect; if it changed, that request is **not sent**, every
  call errors until `take_new_daemon()` is read. The UI loop checks it at the top of each iteration and
  calls `back_to_board_after_restart`: home view, `need_sessions`, clears `sent_size`/screen/grid,
  message `DaemonRestartedBackToBoard` (zh「dct 重启过了，会话编号可能变了，已经回到看板」/ en).
- Tests: `client::instance_tests::a_reconnect_to_a_new_daemon_is_refused_until_acknowledged`,
  `without_the_guard_reconnects_are_transparent`, `ui::tests::a_new_daemon_sends_the_ui_back_to_the_board`,
  `session::tests::restore_reuses_saved_ids_and_falls_back_on_collision`,
  `persisted_records_carry_ids_in_order`, `daemon::drain_tests::instance_ids_tell_daemons_apart`,
  integration `tests/cli.rs restart_from_inside_a_session_hands_off_and_keeps_session_numbers`
  (real binary, real `__swap`: sessions 1 and 3 come back as 1 and 3).
- Mutation-checked: guard never trips; UI stays attached; restore ignores saved id; id not persisted;
  no reservation.

### I1 — typing before the agent is ready
- `PtySession::accepts_keys()`: Unix `tcgetattr` on the master fd (`MasterPty::as_raw_fd`), true when
  `ICANON` is clear; true if it cannot ask, and on Windows.
- `SessionManager::ready_for_keys(id)`: ≥5 s since spawn **and** `accepts_keys` **and** (Idle →
  continuously Idle ≥3 s; Unknown → screen unchanged ≥5 s; Working etc. → no extra condition, so a nudge
  to a busy session is still possible). `tick()` records state changes (`seen_state`/`state_since`) at
  the top of each round and hashes the screen of Unknown sessions (`quiet_since`).
- Applied to: drain `AskHandoff` (state machine only emits it when `ready`), `Nudge` (queued in
  `nudge_due` at the deadline, emitted when ready; the session is marked interrupted at the deadline
  regardless), post-restart notes (`resume_notes::step` takes `ready`), cancel notes (M1) and held
  messages (I8). One note per session per tick.
- No retype: new `bridge::submit_once` → `Sent::{Yes, No, Partial}`; Partial (text in, Enter failed) is
  dropped, never retried; checkpoint-only failure counts as sent.
- Tests: `pty::tests::accepts_keys_follows_the_programs_terminal_mode`,
  `session::tests::ready_for_keys_waits_for_settle_raw_mode_and_steady_idle`,
  `resume_notes::tests::{idle_but_not_ready_waits, partial_send_is_never_retried, one_note_per_session_per_tick}`,
  `drain::tests::{handoff_request_waits_until_the_session_is_ready, nudge_waits_until_ready_but_interruption_is_recorded_at_the_deadline}`,
  `bridge::tests::sent_after_enter_never_calls_a_half_send_retryable`.
- Mutation-checked: ICANON ignored; spawn age ignored; idle steadiness ignored; notes/handoff/nudge
  ignore `ready`; partial retried; partial mapped to No.
- Test fakes that need typing delivered now run `stty -icanon` first (as real agents do); tests that
  are not about timing set `set_type_waits_for_test(0,0,0)`.

#### ICANON on macOS (finding)
Reading `ICANON` through the **master** fd works on macOS (Darwin 27): `sleep 5` → ICANON set,
`sh -c 'stty raw -echo; …'` → clear. 8/8 repeated runs of the focused test passed. So the termios gate
is kept on Unix, not just the timing rule.

### Unknown-state ruling
- Drain: a Waiting session whose state is `Unknown` becomes Done and is added to `interrupted` (not
  waited for, not nudged). Restore notes are delivered to `Unknown` sessions once `ready_for_keys`
  (screen still ≥5 s + spawn + ICANON).
- Found and fixed a root cause: `send_input("")` set every session to `Working`, including pattern-less
  ones that `tick()` never touches again — a qwen session used once was "busy" forever (this is why the
  acceptance qwen run waited the full time and got nudged). Enter now flips to Working only for sessions
  with an idle or busy pattern. Two existing tests used that flip as their "Enter was sent" signal; they
  now check that `cat` printed the line a second time (only happens after Enter) — mutation-checked by
  removing the Enter.
- Tests: `drain::tests::unknown_state_sessions_are_not_waited_for_and_count_as_interrupted`,
  `resume_notes::tests::unknown_state_sessions_get_their_note_once_ready`,
  `daemon::tests::an_unknown_state_session_gets_its_note_once_its_screen_settles`,
  `session::tests::enter_keeps_a_pattern_less_session_unknown`. All mutation-checked.

### I2 — `--continue` taking a sibling's conversation
- `last_sessions::continue_flags`: an id-less record never gets `--continue` when any record in the same
  `(dir, profile)` resumes by id.
- `SessionManager::drain_resumable(id)` used for `Member.resumable`: profile can't resume by id → false;
  never submitted → true (nothing to lose; also stops the pointless handoff request to an unused codex);
  has an id → true; id-less and used → true only if no live sibling in the same dir/profile has a
  submitted id. Same rule as the next restart's `continue_flags`.
- Tests: `last_sessions::tests::{no_continue_when_a_sibling_resumes_by_id, continue_flags_skips_records_with_ids}`,
  `session::tests::drain_resumable_follows_what_the_next_restart_can_bring_back`,
  `daemon::drain_tests::an_id_less_session_with_an_id_sibling_is_asked_for_a_handoff`. All mutation-checked
  (including "daemon uses profile-level resumable").

### I4 — agent-run restart cancelled by the lease
- `DrainRun.initiator`; `drive` skips the lease check when it is set.
- CLI: with an initiator it sends `Drain`, prints `restart_handed_off` (zh/en: waits for others up to N
  min, this session will be told, no need to run again, "To call it off: dct restart --cancel"; a `--now`
  variant) and exits 0. `dct restart --cancel` (alone only): nothing running / old daemon / not draining →
  「dct 现在没在准备重启，不用取消」; otherwise `DrainCancel`.
- The CLI now only treats `DCT_SESSION_ID` as the initiator if it is a live session of this daemon (this
  also stopped my own shell's `DCT_SESSION_ID=17` from leaking into the integration tests; test harness
  also strips it).
- `--help` (main.rs) and both READMEs mention `--cancel`.
- Tests: `daemon::drain_tests::a_drain_with_an_initiator_survives_the_cli_going_away`,
  `cli::tests::parse_restart_cancel`, integration `restart_cancel_without_a_drain_changes_nothing`,
  `restart_from_inside_a_session_hands_off_and_keeps_session_numbers`. Mutation-checked (lease applied to
  initiator; CLI not handing off).

### I5 — initiator told to continue
- `RecordedSession.restart_initiator` (serde default) written from `Action::Finish { initiator }`;
  `note_for` gives it `INITIATOR_TEXT` 「你要的 dct 重启已经完成了，不用再重启。请接着做重启之前手上的事。」/
  English, before the continue rule.
- Tests: `resume_notes::tests::the_initiator_is_told_the_restart_is_done_not_to_continue`,
  `daemon::drain_tests::the_initiator_is_not_waited_for_and_is_interrupted` (asserts the flag on disk),
  `drain::tests::finish_names_the_initiator`. Mutation-checked.

### I6 — swap relaunches the daemon's own path
- `Request::Drain { exe: Option<String> }` (serde default); CLI sends `current_exe()`; `DrainRun.exe`
  passed to `SwapFn(&Path, Option<&Path>)`; fallback `own_exe()` = `current_exe()` with a trailing
  `" (deleted)"` stripped.
- Tests: `daemon::drain_tests::the_swap_uses_the_cli_binary`,
  `daemon::tests::a_deleted_exe_path_points_at_the_replacement`,
  `proto::tests::drain_without_new_fields_still_parses`. Mutation-checked.

### I7 — choice: answer Hello first (preferred option)
- `run_with_hooks` now loads the record list, reserves ids, sets `mgr.restoring`, and runs the whole
  restore (restore → `set_resume_skips` → clear `restoring` → watcher → notes) on a `restore` thread,
  then goes on to the accept loop. Secrets are looked up per record (`restore_sessions_with` takes a
  `secret_of` closure) so the restore never holds the secrets lock that `Profiles` needs.
- `DrainCtl::start` refuses with new `ErrorCode::StillRestoring` (zh「dct 刚重启，还在接回会话，过一会儿再试」/ en)
  while restoring — a drain then would persist a partial list and lose sessions.
- Waits: swapper waits 30 s for Hello (`client::NEW_DAEMON_HELLO_WAIT`), CLI 45 s total; both poll every
  250 ms.
- Tests: `daemon::tests::the_daemon_answers_while_it_is_still_restoring_sessions` (12 agent records in a
  git repo; Hello answers while `is_restoring()` and fewer than 12 are back; afterwards all 12 keep
  their saved ids), `daemon::drain_tests::a_drain_is_refused_while_sessions_are_being_restored`.
  Mutation-checked (joining the restore thread before serving; allowing a drain while restoring).

### I8 — choice: persist held mesh messages (option 1, not the fallback)
- `Mesh::held_texts()` copies (does not take) the queued, already-marked texts per session.
  `DrainCtl::set_held_source` is wired to the mesh slot in `run_with_hooks`; `finish` puts them in
  `RestartMarks.held` → `RecordedSession.held_messages` (serde default, omitted when empty).
- On restore each held message becomes a `Note { kind: Message }` queued after the continue note,
  delivered through the same gated, one-per-tick path, deadline 30 min (`MESSAGE_WAIT`); if dropped,
  a new `WarningCode::HeldMessageDropped { dir }` (zh/en) instead of the "couldn't tell it to continue" one.
- Copy-not-take means a failed swap leaves the queue intact. Known gap: a message that arrives in the
  ≈2 s between the final save and exit is lost (comment in `finish`).
- Option 2 was rejected because refusing would need a new mesh `Receipt` on the cross-machine wire,
  which older dct on other computers cannot parse.
- Tests: `mesh::tests::held_texts_copies_the_queues_without_taking_them`,
  `daemon::drain_tests::held_messages_are_saved_with_the_final_state`,
  `daemon::tests::held_messages_are_queued_after_the_continue_note` (both delivered on screen).
  Mutation-checked (not saved; not restored).

### M1 — cancel after nudge/handoff
- `DrainRun.told` records sessions whose nudge/handoff text actually went in. `cancel` (and lease expiry,
  swap failure) sends them `CANCEL_TEXT`「dct 不重启了，请接着做刚才的事。」/ English via
  `deliver_resume_notes` (gated). `DrainCancel` now answers `Response::DrainCancelled { told }`; the CLI
  (Ctrl+C and `--cancel`) prints `restart_cancelled_told`, naming them.
- Tests: `daemon::drain_tests::cancel_after_a_handoff_request_tells_the_session_to_carry_on`,
  `cli::tests::{the_cancel_line_names_sessions_that_were_already_asked, drain_loop_ctrl_c_cancels}`,
  `proto::tests::the_drain_cancelled_and_instance_shapes_are_pinned`. Mutation-checked (empty `told`;
  not told).

### M2 — cancel between step and typing
- `type_for_run` takes the run lock, checks the generation, types **while holding it**, then records
  `told`. A cancel is therefore either before the typing (nothing typed) or after it (session is in
  `told` and gets the M1 note). Lock order stays run → session.
- Test: `daemon::drain_tests::nothing_is_typed_for_a_run_that_is_no_longer_current`. Mutation-checked.

### M4 — kill sessions before exit
- `SessionManager::kill_all_now()` (marks `stopped_by_request`, `kill_now`, state Stopped) after the
  linger, just before `exit`. The list is frozen, so it does not rewrite the file.
- Tests: `session::tests::kill_all_now_stops_every_session`, assertion added to
  `daemon::drain_tests::drain_with_all_idle_persists_ids_and_reports_done`. Mutation-checked.

### M6 — unresumable initiator
- CLI `initiator_unresumable`: the initiator's profile (disk + builtin) is an agent without
  `resumes_by_id()` → prints `RestartInitiatorUnresumable` (one line, zh/en) before handing off.
  Capability-based, no profile names in code.
- Test: `cli::tests::initiator_unresumable_reads_the_profile_capability`. Mutation-checked.

### M7 — handoff pruning
- `prune_old_handoffs(handoffs dir, 30 days)` at daemon start.
- Test: `daemon::tests::old_handoffs_are_pruned`. Mutation-checked.

### Task-10 items
- `--now`: `DrainConfig.now` (sent as `Drain { now }`): no handoff requests, no nudges, busy sessions
  recorded interrupted, finishes on the first step; CLI `drain_loop(.., quiet)` prints no progress line.
  Tests: `drain::tests::now_skips_handoffs_and_nudges`, `cli::tests::drain_loop_quiet_prints_no_progress`.
  Mutation-checked (handoff still asked; still nudged; progress printed). Manually checked in an
  isolated HOME: output is just 「正在换新的后台…… / 已重启」.
- Handoff filenames use local time (`localtime_r`'s `tm_gmtoff` on Unix; UTC on Windows — flagged
  below). Tests: `drain::tests::{stamp_of_formats_civil_time, file_stamp_is_local_time}` (compares with
  `date`); mutation-checked with `TZ=Asia/Shanghai`.
- READMEs (both): first restart from an older dct (protocol < 25) is the old immediate restart; `-y` row;
  phone/voice refusal quoted exactly as sent (「dct 正在准备重启，稍后再发」, with an English gloss in the
  English README — both channels send that fixed Chinese text); `--cancel` row and paragraph; agent-run
  hand-off; the `/clear` / `/resume` known limit (I3); Unknown-state sessions are not waited for;
  sessions keep their numbers and the open board returns to the board; mesh limit line updated (queued
  messages now survive `dct restart`).

### Deferred minors
- Ctrl+C during the swap wait prints `RestartCancelTooLate` once (`drain_and_swap`). Not unit-tested
  (signal + real wait); not mutation-checked.
- Unreadable old pid ⇒ premature done: `OldDaemon::gone` — with no pid it only counts the old daemon as
  gone when the Instance id changed (old: always "gone"). Test
  `cli::tests::without_a_pid_the_old_daemon_is_gone_only_when_the_instance_changes`. Mutation-checked.
- Swap-wait polling 50 → 250 ms (`wait_up_to_every`, CLI and swapper). Not tested (timing only).
- Comment in `resume_notes::step` that a note may be delivered before the watcher window closes, and
  why it is harmless.

## Other changes worth knowing
- Agent-typed texts (nudge, handoff, continue, read-handoff, initiator, cancel) now have English
  versions. The language is the one saved in `settings.json` next to the socket, default Chinese
  (`drain::agent_lang`) — env vars are ignored because the daemon's environment is whoever spawned it.
- Protocol stays 25 (unreleased): added `Request::Instance`, `Drain.now`/`Drain.exe` (serde default),
  `Response::DrainCancelled`, `Response::Instance`, `ErrorCode::StillRestoring`,
  `WarningCode::HeldMessageDropped`; shape tests updated/added. `Hello` unchanged.
- `ALL_KEYS` count 219 → 222 (three new keys).

## Concerns
1. Local-time handoff names are Unix-only; Windows still uses UTC (no simple offset API without a new
   dependency).
2. Held mesh messages arriving in the ≈2 s between the final save and exit are lost (documented in code).
3. Message to an agent typed via `ready_for_keys` depends on the agent putting the terminal in raw mode.
   All real agents do (Ink/crossterm), but a hypothetical line-mode agent would never get notes — they
   then expire into the existing UI warnings rather than being typed wrongly.
4. Ctrl+C-too-late during the swap wait and the 250 ms polling have no automated test.
5. Not re-run on real claude/codex/qwen: the codex "read handoff" fix is verified by the focused ICANON
   test and the gated-delivery tests, not by a new real-machine run. A short isolated acceptance re-run of
   the codex case (Task 10 item 3) would confirm it end to end.
