# Residual fix pass: report (BASE 477befc)

Commits (on `feat/graceful-restart`):

- `9e91a04` fix(restart): the new-daemon guard trips on the first restart from an older dct (N1)
- `33092d2` fix(restart): wait for the handoff file and settled idle; widen codex busy signal (codex a–d, N3, N5, N6)
- `ca2a8a7` fix(restart): no handoff for unused sessions, keep --continue for the newest id-less record, fix doc comments (N4, N8, N7)

## Per item

Every mutation below was applied to the source, and the named test was run. All 13 mutants were killed: the build compiled and the test failed (`0 passed; 1 failed`). The source was then restored.

| Item | What changed | Pinning test | Mutation (killed) |
|---|---|---|---|
| N1 | `client.rs`: new `asked` flag. The guard trips on **any** change after the first ask (`self.asked && now != self.instance`), including no answer → answer. `RestartOldDaemonPlain` (zh/en) now says sessions come back by themselves with new numbers, and to close every open dct window before confirming and reopen afterwards. Both READMEs have the same first-restart bullet. | `client::instance_tests::a_reconnect_from_an_old_daemon_to_a_new_one_is_refused`: a fake old daemon answers `Instance` with an error. A reconnect to the same old daemon is let through. A reconnect to a real daemon is refused until acknowledged. | Old condition `self.instance.is_some() && …` |
| codex (a) | `drain.rs`: `Slot::HandoffAsked` → Done only when the handoff file exists **and** the session has been idle ≥3 s. The 10 s `HANDOFF_GRACE` is removed. If the file is never written, the session is interrupted at the hard deadline. `Drain::step` takes `written(&Path)`, and the daemon passes `is_file`. | `drain::tests::handoff_counts_as_written_only_once_the_file_exists_and_idle_settles` | `file_there = true` |
| codex (b) | A Waiting slot seen busy during the drain must be idle continuously for `IDLE_SETTLE` = 3 s before it counts as stopped. A session idle from the start is still done at once. | `drain::tests::a_busy_session_must_stay_idle_for_three_seconds_to_count_as_stopped` | `IDLE_SETTLE = 0` |
| codex (c) | `resume_notes`: `note_for` returns `Planned { text, needs_file }`. The read-handoff note is always queued with `needs_file = handoff path`. `step` sends it only once `written(path)` is true and keeps checking until the note deadline (`NOTE_WAIT`, 120 s). If the file never appears, the user gets a new `WarningCode::HandoffMissing`. | `resume_notes::tests::a_handoff_note_waits_for_its_file_until_the_deadline` (pure); `daemon::tests::a_read_handoff_note_waits_for_a_file_written_after_the_restart` (real pty: the file is written 600 ms after the restore and the note arrives); `daemon::tests::a_handoff_that_never_appears_becomes_a_handoff_missing_warning` | pure: `file_ok = true`; daemon: `written = \|_\| true` |
| codex (d) | `profiles/codex.toml` now has `busy_pattern = "esc to interrupt\|tab to queue\|· [⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]"`, with a dated comment (2026-10-15, v0.163.0, both modes). Real screens are in `tests/fixtures/codex-0.163/`. | `profile::tests::codex_busy_pattern_matches_real_v0_163_screens`: 5 busy screens match; 5 idle screens (including the startup logo, which is made of braille, and a text-filled idle box) do not. | Old pattern `esc to interrupt` |
| N3 | An unresumable session that is idle but never `ready_for_keys` is given up after `UNREADY_CAP` = 60 s: Done, interrupted, no handoff request. The timer resets if the session is seen busy. The warning is raised after restart: `resume_notes::lost_without_handoff` (interrupted, not resumed by id, not `--continue`d, no handoff, not the initiator) pushes `HandoffMissing` at restore. | `drain::tests::an_idle_session_that_never_gets_ready_is_given_up_after_a_minute`; `daemon::tests::an_interrupted_unresumable_session_without_a_handoff_is_reported` | `UNREADY_CAP` = 1e6 s; removing the `HandoffMissing` push |
| N4 | `drain_resumable` checks `!submitted` before `!by_id`, so an unused codex is not asked for a handoff. | `session::tests::an_unused_session_of_an_unresumable_profile_needs_no_handoff` | Old order |
| N5 | Cancel notes use the new `NoteKind::Cancel`. Their give-up warning is the new `WarningCode::CancelNoteDropped`: 「没能告诉 {dir} 的会话「dct 不重启了」，请手动告诉它接着做。」 / "The session in {dir} could not be told that dct is not restarting after all. Please tell it to carry on yourself." It does not say 「重启后」. The mapping is in `daemon::dropped_warning`. | `daemon::tests::a_dropped_cancel_note_has_its_own_warning` (also checks that no language contains 「重启后」 / "after the restart") | `Cancel → ResumeNoteDropped` |
| N6 | `resume_notes::step`: a `Message` note for a vanished or stopped session now goes to `dropped`, which raises `HeldMessageDropped`. dct's own notes are still dropped silently. In restore, a skipped record (directory gone, profile gone, start failed) raises one `HeldMessageDropped` per held message. The `HeldMessageDropped` wording no longer claims "it never got ready". | `resume_notes::tests::a_held_message_for_a_vanished_session_is_reported_dropped`; `daemon::tests::held_messages_of_a_skipped_session_are_reported_dropped` | Removing the push in `step`; removing `skips.extend(held_dropped(..))` |
| N7 | Moved `home_view`'s doc back onto `home_view`, and rejoined the doc of `disconnected_state_shows_warning_in_bottom_bar`. | n/a (comments only) | n/a |
| N8 | READMEs: the first (plain) restart brings sessions back by themselves with new numbers and asks nothing (same bullet as N1). Code: in `continue_flags`, an id-less record that has an id'd sibling keeps `--continue` when its `last_active` is strictly newer than every sibling's in its (dir, profile) group. `drain_resumable` is deliberately **not** relaxed to match (see Concerns). | `last_sessions::tests::the_newest_id_less_record_keeps_continue_even_beside_an_id_sibling` (newest → true; an id'd sibling newer → false; a tie → false) | Disabling the newest-in-group loop |

Existing drain tests that assumed "busy → idle = done in the same tick" now take one more step 3 s later. The old `handoff_not_considered_written_in_the_same_breath` test was replaced by the (a) test, because its rule (10 s grace without busy → done) no longer exists.

## Codex v0.163.0 busy-indicator measurements (2026-10-15)

Setup: `codex --dangerously-bypass-approvals-and-sandbox`, with and without `--no-alt-screen`, run side by side in a private `tmux -L rf1` server (110×32). HOME was `/private/tmp/rf1/h`, with `~/.codex` symlinked in. The screen was captured every ~0.4 s. There were three turns per mode:

- t1: write `notes.txt`, `sleep 12`, `cat`, two sentences.
- t2: the real Chinese handoff request asking for a ≥40-line handoff file.
- t3: `sleep 10`, and the user types into the box while codex is busy.

What was on screen and when (seconds after Enter; E = `esc to interrupt`, Q = `tab to queue`, spinner = `· ⠸` at the end of the footer):

| Turn | Mode | Turn length | Captures without E | What was on screen in those gaps |
|---|---|---|---|---|
| t1 | default | 0–21 s | 2.45, 2.85, 5.59 | 2.45: footer spinner `· ⠸`, streaming text `• I'll create \`notes`. 2.85 and 5.59: only streaming text (`• I'll create notes.txt…`, `• I`), with no marker at all. |
| t1 | inline | 0–25 s | 2.45, 2.85, 5.98 | The `• Working (…)` line is replaced by streaming text. No marker. |
| t2 | default | 0–25 s | 2.68, 3.06 | Streaming text 「我会把当前任务…写入 handoff.md」. No marker. |
| t2 | inline | 0–30 s | 2.68, 3.06 | Same. |
| t3 | default | 0–14 s | 2.09 | Streaming text. From 4.4 s (user typed): E **and** `tab to queue message` until the end. |
| t3 | inline | 0–15 s | 1.71 | Same. |

Findings:

- `esc to interrupt` is missing only while codex streams assistant text. Each gap was ≤ ~1.2 s (at most 2–3 consecutive captures) and appeared in both modes.
- `tab to queue message` appears exactly while busy with text in the box. It never appeared without E in these runs. Acceptance r14 saw it alone, so it is kept.
- The footer spinner `· <braille>` appears in the first ~2 s of a turn and covered one gap. It never appeared outside a turn (0 of all idle captures). The startup logo is made of braille, including `⠸`, so the pattern needs the `· ` prefix.
- **Not reproduced:** acceptance r15 (a whole turn with no E). The remaining sub-second gaps have no marker at all. They are covered by the 3 s idle settle (b). For a handoff, (a) also blocks an early swap whatever the screen shows.

Chosen pattern: `esc to interrupt|tab to queue|· [⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]`. Fixtures: `tests/fixtures/codex-0.163/{default,inline}-busy*.txt` and `*-idle-*.txt`.

## Real check (3 rounds)

Isolation:

- Each round had a fresh `HOME=/private/tmp/rf1/rN/h` (with `~/.codex` symlinked in) and a fresh git project `/private/tmp/rf1/rN/proj`.
- Every process was started with `env -i` (so no `DCT_SESSION_ID`) inside `tmux -L rf1`.
- The binary was this worktree's `target/release/dct`, by absolute path, built from `ca2a8a7`.
- A Python JSON-lines driver ran each round over the round's own socket:
  1. Create codex and accept the trust prompt.
  2. Submit "Create notes.txt containing ROUNDn-OK, cat it, reply DONE" and wait until it finishes.
  3. Run `dct restart -y --wait 1 --nudge-wait 1`, polling List and Screen every 0.3 s.
  4. Watch the new session for up to 170 s.
  5. Collect `~/.codex/history.jsonl` lines.
- Evidence is in `scratchpad/rc/ev/rN/` (timeline.log, old-screens.log, new-screens.log, history.jsonl).

| Round | Handoff request submitted | Mid-turn Idle blip (old daemon) | Handoff file written | New daemon started (swap) | Read-handoff note | Codex acted on it | Result |
|---|---|---|---|---|---|---|---|
| r1 | 20:03:50 | 20:03:55.8, one 0.3 s sample | 20:04:00.5 | 20:04:08 (idle from 20:04:05.1 + 3 s settle) | Typed 20:04:14, in codex history (submitted) | 「已读交接：上次已创建 notes.txt，内容为 ROUND1-OK…」, Worked for 7s | **PASS** |
| r2 | 20:04:57 | 20:04:59.6, ~0.6 s | 20:05:05.4 | 20:05:11 | 20:05:17, submitted | Codex read the handoff ("读取交接并继续处理"), turn finished | **PASS** |
| r3 | 20:05:46 | 20:05:48.3, ~0.6 s | 20:05:54.4 | 20:05:59 | 20:06:05, submitted | Codex read the handoff ("读取交接继续工作"), turn finished | **PASS** |

Every round showed the Idle blip right after the request. That blip is what ended drains early in the acceptance run (r1–3, r7, r8, r13, r16). It no longer does: the file was written before the swap in 3 of 3 rounds, and the note was delivered and submitted in 3 of 3. CLI output each time was `Waiting for 1 session (#1), 59s left … Switching to the new background service… Restarted.`, rc=0.

Parked item, Enter lost on the first ordinary send to a fresh codex: it did not happen in these 3 rounds. The driver's `Submit` came 5 s after the idle prompt appeared, and the task text was in codex history every time. There is no new evidence either way.

## Tests and clippy

- `cargo test`: all green. The lib suite has 1944 passed and 1 ignored; every integration binary reports ok.
- `cargo +1.99.0 clippy --all-targets -- -D warnings`: clean.
- `cargo fmt` was not run. My own hunks were formatted with rustfmt output applied only to the changed lines; a filtered `rustfmt --check` shows no diff in them.

## Cleanup proof

- `dct stop --all` was run against each round's socket (each printed "Stopped session 1"). The three isolated daemons (`…/graceful-restart/target/release/dct daemon /private/tmp/rf1/rN/h/.dct/daemon.sock`, pids 85722, 91494, 96516) were killed. The trial daemon (r0, pid 78242) was killed earlier.
- The measurement codex windows were closed. `tmux -L rf1 kill-server` was run, and a later `tmux -L rf1 ls` printed "no server running".
- No codex process is left with a cwd under `/private/tmp/rf1`. `ps | grep rf1` prints nothing.
- The `.codex` symlinks were deleted as links first (`find -type l -delete`). Then `/private/tmp/rf1` was removed. `~/.codex` is intact.
- `~/.codex/config.toml` was backed up to `scratchpad/rf/codex-config.toml.orig` (sha1 `4cc607ec…771608d`). Exactly the 5 added `[projects."/private/tmp/rf1/…"]` blocks were removed. `diff` reports the files identical, and both sha1 values are `4cc607ecf2596442e99de5929ceffd2d7771608d`.
- The test turns' rollouts and `history.jsonl` lines remain in the real `~/.codex`. Codex wrote them through the symlink, and they were not edited.
- The real daemon `/Users/lei/.local/bin/dct daemon /Users/lei/.dct/daemon.sock`, pid 26437 (started Sun 11 Oct 06:17:54), was never contacted and is still running. `~/.dct` and `~/.local/bin` were not touched.

## Concerns

- Under ruling (a), an agent that ignores the handoff request holds the drain until the hard deadline (`--wait` + `--nudge-wait`, 18 min by default) before it is recorded as interrupted. After the restart, the user gets `HandoffMissing`.
- `drain_resumable` is not relaxed for N8. An id-less, newest-in-group session is still asked for a handoff during a drain, and may then get both `--continue` and a "read your handoff" note. This is redundant but safe. Mirroring N8 there would let a session lose its conversation if a sibling becomes more recently active between the drain start and the final save.
- `HandoffMissing` and `CancelNoteDropped` are new `WarningCode` variants inside the unreleased protocol 25. No version bump was made, and no wire-shape test covers warnings.
- The whole-turn-without-marker case from acceptance r15 was not reproduced in 6 measured turns. If it happens, codex can still be typed into while busy (the busy signal is wrong). For a handoff, (a) and (c) still prevent losing it.
