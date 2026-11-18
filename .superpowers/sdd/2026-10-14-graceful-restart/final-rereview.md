# Final fix wave re-review: graceful restart (7d2a50d..477befc)

Scope: only the findings the fix wave was asked to fix, plus new breakage in the diff. Read-only. I ran two focused tests: `pty::tests::accepts_keys_follows_the_programs_terminal_mode` and `session::tests::restore_reuses_saved_ids_and_falls_back_on_collision`. Both pass on macOS, so ICANON can be read through the master here.

## Verdict per finding

| Item | Status | Evidence |
|---|---|---|
| C1 | **partially** | **Restored numbers are correct.** <br>- Ids are saved in id order: `session.rs:1065-1075`, plus `id: Some(s.id)`. <br>- `create_inner` uses the saved id when it is free and pushes `next_id` past it. <br>- `reserve_ids_up_to` runs before the restore thread starts (`daemon.rs:165-172`), so a session the user opens during restore always gets a number above the highest saved one. No collision is possible: the only other creators use `fetch_add`, and `next_id` is already above every saved id. <br>- Old files without ids get fresh numbers. <br>- The failed-resume replacement takes a fresh number on purpose. <br>**The UI guard is wired correctly** (`client.rs:273-292`, `ui/mod.rs:583-587`) and nothing bypasses it on retry. **But it never trips for the user's first real restart.** See N1. |
| I1 | addressed | `pty.rs:443-462` checks ICANON on Unix. On Windows and on any error it answers true, so it can never block forever; only the timing rule applies there. <br>`session.rs:1026-1060` `ready_for_keys` requires ≥5 s since spawn, ICANON clear, and either Idle for ≥3 s or an Unknown screen unchanged for ≥5 s. <br>The gate is applied to `AskHandoff` (`drain.rs:160-166`), to nudges through `nudge_due` (`drain.rs:220-230`), to resume, cancel and held notes (`resume_notes.rs:134-138`), and to the drain `ready` closure (`daemon.rs` in `drive`). <br>No retype: `bridge::submit_once` returns `Partial`, and Partial is dropped (`resume_notes.rs:144-148`). |
| Unknown ruling | addressed | `drain.rs:180-183`: Unknown → Done and interrupted, no wait and no nudge. The note is delivered once the screen has been quiet for 5 s (`resume_notes.rs:135`). The Enter change in `session.rs:1599-1608` is the right root-cause fix. I checked the other readers of `Working`: mesh delivers only to `Idle` (`mesh/deliver.rs:90`); notifications and auto-naming need a Working→Idle edge that pattern-less sessions never had; `last_active` and `submitted` are still set unconditionally (`session.rs:1556`, `1562`). Nothing regresses. |
| I2 | addressed (per the brief's rule) | `last_sessions.rs` `continue_flags` uses `sibling_has_id`. `session.rs:990` `drain_resumable` is used for `Member.resumable` (`daemon.rs` in `start`). See N4 for an inaccuracy in the report, and N8 for what this means on the plain first restart. |
| I4 | addressed | `drive` skips the lease when `run.initiator.is_some()`. The CLI hands off and exits 0 (`cli.rs` `hand_off`). `--cancel` exists (`cli.rs` `cancel_restart`) and is in `--help` (`main.rs:29-32`) and both READMEs (`README.md:462,475`). <br>`DCT_SESSION_ID` is only trusted when it names a live session of this daemon (`cli.rs` `run_restart`, `.filter(\|me\| live.iter().any(..))`). The agent-run path still works: a restored session is a new process spawned with its own (now stable) id in its environment, and the integration test `restart_from_inside_a_session_hands_off_and_keeps_session_numbers` covers it. |
| I5 | addressed | `RecordedSession.restart_initiator`; `note_for` checks the initiator before the continue rule (`resume_notes.rs:71-79`). |
| I6 | addressed | `Request::Drain { exe }`; `DrainRun.exe` is passed to the swap; `own_exe` strips `" (deleted)"` (`daemon.rs` `own_exe` / `strip_deleted`). |
| I7 | addressed | The restore runs on a `restore` thread and the accept loop starts right away (`daemon.rs:173-206`). Secrets are looked up per record, so the restore never holds the secrets lock. `Drain` is refused while restoring (`StillRestoring`), and the CLI prints the error in plain words (`drain_loop` `Response::Error` arm; `hand_off`). Hello, List and New are safe while restoring (see C1). The swapper waits 30 s (`client.rs` `NEW_DAEMON_HELLO_WAIT`) and the CLI 45 s. |
| I8 | addressed (option 1) | Held messages are copied, not taken (`mesh/mod.rs` `held_texts`). They are saved with the final state (`daemon.rs` `finish` → `RestartMarks.held`) and re-queued against the **restored** record's id (`daemon.rs` `restore_sessions_with`), so renumbering cannot misroute them. <br>No double delivery: mesh keeps holding through the 2 s linger (draining stays true), and a failed swap leaves the queue alone and clears the file on the next persist. <br>Known gaps: the ≈2 s window between the final save and exit is acknowledged; senders never get a "Delivered" receipt; and see N6. |
| M1 | addressed | `DrainRun.told`; `cancel`, lease expiry and swap failure all call `tell_called_off`; the CLI names those sessions (`cli.rs` `cancelled_line`). Small issues in N5. |
| M2 | addressed | `daemon.rs:625` `type_for_run` checks the generation and types while holding the run lock. The cost of that is N2. |
| M4 | addressed | `session.rs` `kill_all_now`, called after the linger and before `exit` (`daemon.rs` `finish`). |
| M6 | addressed | `cli.rs` `initiator_unresumable` reads the profile's capability; no profile names in code. |
| M7 | addressed | `daemon.rs:158-163` `prune_old_handoffs` keeps 30 days. |
| Task-10: `--now` | addressed | `drain.rs` `cfg.now`: no handoff, no nudge, finishes on the first step. `drain_loop(.., quiet)` prints no progress. |
| Task-10: local-time filenames | addressed on Unix | `drain.rs` `utc_offset_secs` (`localtime_r` `tm_gmtoff`). Windows still uses UTC; this is flagged in the report and acceptable. |
| Task-10: READMEs | addressed, one inaccuracy | Both READMEs now have the protocol-24 first-restart bullet, a `-y` row, the refusal quoted exactly, `--cancel`, and the `/clear` limit. But the first-restart bullet is partly wrong; see N8. |
| Deferred: Ctrl+C too late during swap | addressed | `cli.rs` `drain_and_swap`, `told_too_late`. |
| Deferred: unreadable pid ⇒ premature done | addressed | `cli.rs` `OldDaemon::gone` falls back to comparing instance ids. |
| Deferred: 50 → 250 ms polling | addressed | `client.rs` `wait_up_to_every`, `SWAP_POLL`. |
| Deferred: resume_notes comment | addressed | `resume_notes.rs:124-126`. |

## New issues

### Important

**N1. The new-daemon guard cannot trip on the user's first real restart, and the README says it will.**
`client.rs:283-289`: the guard trips only when `self.instance.is_some() && now != self.instance`.

- **New-build UI on an old daemon.** A UI that started against the running protocol-24 daemon asked it `Instance` and got `BadRequest`. Its `instance` is therefore `None`. After the plain restart it reconnects to the new daemon, and `None → Some(x)` does not trip.
- **Windows opened before upgrading.** Every `dct` window the user opened before upgrading runs the old binary and has no guard at all.
- **What the first restart does.** It goes through the plain path. The old daemon's file has no ids, so the 18 sessions are renumbered 1..18 in the old HashMap order. Any open window attached to "7" silently shows a different agent and types into it. That is the original C1 harm, on exactly the restart the user is about to do.
- **The docs promise the opposite.** `README.zh-CN.md` now says 「开着的看板会回到看板首页」 unconditionally.

Fix:
- (a) In `ensure_conn`, remember whether the instance was asked. Trip on any change, including `None → Some`:

  ```rust
  if self.asked && now != self.instance
  ```

  Old → old (`None → None`) still cannot be detected. That case is acceptable.
- (b) In `RestartOldDaemonPlain` (`i18n.rs:1159`), and in the first-restart bullet of both READMEs (`README.md:478`, `README.zh-CN.md:402`), tell the user to close every open dct window before confirming and reopen afterwards: 「先关掉所有开着的 dct 窗口，重启完再打开，不然打的字可能落进别的会话」.
- (c) Add a test: a guard created against a daemon that answers `Instance` with an error, then a reconnect to one that answers it, must trip.

### Minor

- **N2. `type_for_run` holds the drain run lock across `submit_once`, and that includes a `git::checkpoint` for agent sessions.** `daemon.rs:625-640`. While it runs, `DrainStatus` is blocked: the UI's `poll_drain` makes that call on the render thread, so the UI freezes, and CLI renewals and `cancel` wait too. In a big repo that is seconds per typed line. Correctness holds: the lease is skipped for the initiator, and a CLI poll times out only after 5 s, three times in a row. Fix: a separate `typing: Mutex<()>`. `type_for_run` takes it around the generation check and the typing, and `cancel` and `abandon` take it before the run lock. `status` does not take it.
- **N3. An unresumable Idle session that never becomes `ready_for_keys` holds the whole restart to the 18-minute hard deadline.** `drain.rs:160-166`. Examples: an agent whose idle prompt stays in canonical mode, or a flickering idle screen that never stays Idle for 3 s. It stays `Waiting` but counts as stopped, so it is never nudged. Before this wave it was asked at once. Fix: bound the wait, for example 60 s of Idle without becoming ready → ask anyway or mark Done, and say so in progress.
- **N4. The report says `drain_resumable` makes an unused codex skip its handoff request ("never submitted → true … stops the pointless handoff request to an unused codex"). The code does not.** `session.rs:1003-1008` returns `false` on `!by_id` before it checks `!submitted`, so an unused codex is still asked. This is unchanged from before and the brief did not require it. Fix: move `if !submitted { return true; }` above the `by_id` check, or correct the report.
- **N5. Cancel notes reuse `NOTE_WAIT` (120 s).** `daemon.rs:602-621`. A nudged session still busy after 2 min drops its note, and the dropped warning is `ResumeNoteDropped` 「…的会话重启后一直没准备好…」, but no restart happened. Fix: use `MESSAGE_WAIT`, or a dedicated `CancelNoteDropped` text.
- **N6. Held mesh messages are dropped silently when the session they belong to is replaced.** `resume_notes.rs:134` (`None | Some(Stopped) => continue`). If the by-id resume fails and the watcher replaces the session, the messages vanish with no `HeldMessageDropped` warning. The same happens when the record is skipped on restore. Fix: for `NoteKind::Message`, push the message to `dropped` instead of `continue`.
- **N7. Two doc comments are spliced.**
  - `ui/mod.rs:2058-2066`: `home_view`'s doc ("用户选的那个模式对应的视图 … `sync_board_cursor_from_grid` 负责") now sits on top of `back_to_board_after_restart`'s doc, and `home_view` has none.
  - `ui/mod.rs:4098-4101`: the doc of `disconnected_state_shows_warning_in_bottom_bar` is cut in half by the C1 test's doc.

  Fix: move the inserted blocks below the existing docs.
- **N8. The first-restart README bullet says the user is asked whether to bring sessions back, but the plain `dct restart` path never asks.** `README.md:480` ("you are asked about bringing them back, as after a crash") and `README.zh-CN.md:403`. The path calls `client::restart_daemon` → `spawn_daemon`, and the new daemon restores `last-sessions.toml` by itself. The question exists only in `main::run_ui` when no daemon is running. Fix: say "they come back by themselves, with new numbers".
  - Also on that path: an id-less claude record whose (dir, profile) group has an id'd sibling now starts **fresh, with no handoff**, because there is no drain to ask. That is the brief's I2 rule, but it is a real loss on the first restart.
  - Optional: in `continue_flags`, keep `--continue` when the id-less record's `last_active` is newer than every by-id sibling's.

## Things checked and found fine

- **Lock order is still run → sessions map → session.** `drain_resumable` and `ready_for_keys` under the run lock take only session locks. `finish` reads `held` and persists outside the run lock. No path takes the run lock while holding a session or mesh lock.
- **The restore thread versus requests.**
  - `List`, `Screen` and `New` during restore are fine.
  - Persists during restore write a partial list. That was true before too, and a drain that would freeze a partial list is refused.
  - `set_restoring(false)` comes before the watcher. That ordering is pre-existing and fine.
- **The first real restart from protocol 24.** The new daemon answers `Hello` at once, so `restart_daemon`'s 5 s wait is no longer at risk. An old file without ids gives fresh numbers. The protocol-24 fallback is unchanged. The remaining harm is N1.
- **Windows.** `accepts_keys` returns true, so only the timing gate applies, and notes are not blocked forever. `utc_offset_secs` returns 0.

## Verdict

**Needs fixes.** N1 only: it is small (a one-line guard change plus one sentence in the plain-restart message and both READMEs), and without it the first real restart on the user's 18 sessions keeps the C1 wrong-session risk that this wave was meant to remove. N2–N8 are Minor and can be done in the same pass or parked.
