# Final acceptance: codex "read your handoff" after a graceful restart

Branch `feat/graceful-restart`, HEAD `477befc`, `cargo build --release` from this worktree. Date 2026-10-15 (local, PDT).

## Verdict

- **The fix under test works.** The read-handoff note is now typed and submitted. In every round where the
  handoff file existed when the new daemon started (5 of 5), the new codex got
  「dct 刚重启。你上次的交接在 …，先读它，再接着做。」. It was submitted (it is in codex history and the
  rollout), and codex read the file and reported back. There were no lost notes and no text left sitting
  unsent in the input box.
- **The end-to-end flow still FAILS most of the time, for a different reason.** In 12 of the 17 valid
  rounds, codex had not written the handoff file before the old daemon swapped. `note_for` checks `is_file`
  once, at restore time, so no note was sent. The cause is codex v0.163.0's busy detection (details below).
  It is not the typing gate.
- `--now`: **PASS**.

## Method

Same approach as task-10 item 3, with these isolation changes:

- Each round got a fresh `HOME=/private/tmp/gr1/rN/h` and a fresh git project `/private/tmp/gr1/rN/proj`.
  Every command ran through a wrapper (`env -i HOME=<round home> …`, with a clean PATH), so
  `DCT_SESSION_ID` and `CLAUDECODE` were not inherited. The wrapper refused any HOME outside `/private/tmp/gr1/`.
- `~/.codex` was symlinked into each temp HOME. `/private/tmp/gr1/bin/dct` pointed to this worktree's
  `target/release/dct`.
- The daemon and `dct restart` ran in a private tmux server (`tmux -L gr1`). Sessions were driven over the
  isolated socket with a JSON-lines driver (Create / Submit / Input / Screen / List).
- Each round:
  1. Open a codex session and accept "Trust this folder".
  2. Ask codex to create `notes.txt` with `ROUNDn-OK`, `cat` it, and reply DONE. From r15 on, the script
     also waited for `task_complete` in the codex rollout plus 5 s.
  3. Run `dct restart -y --wait 1 --nudge-wait 1` in a tmux window.
  4. Poll `List` every ~0.15 s during the drain. From r11 on, also capture the old session's screen every
     0.3–0.5 s.
  5. Watch the new session's screen for 45–75 s.
  6. Collect `sessions.log`, the handoffs directory with mtimes, `~/.codex/history.jsonl` lines, and the
     codex rollout.
- No claude control session was run. Claude resumes by id, so it never gets the read-handoff note.

Evidence for every round (screens, state polls, restart output, handoff files, logs) is in the session
scratchpad: `/private/tmp/claude-502/-Users-lei-work-dc-dc-terminal/ddc91785-420b-43c3-9a68-f43f34bf555f/scratchpad/final-ev/rN/`.

## Per-round results

A round PASSES only if all of these hold: codex wrote the handoff before the swap, the new codex received the
read-handoff note, and the note was submitted (codex acted on it).

| Round | Restart (CLI) | Handoff file mtime vs. new daemon spawn | Read-handoff note | Result |
|---|---|---|---|---|
| r1 | 18:47:50 asked → swap 18:47:53 | written 18:47:57, **after** spawn 18:47:53 | none | FAIL (drain ended early) |
| r2 | 18:51:04 → 18:51:07 | 18:51:12, after spawn | none | FAIL (drain ended early) |
| r3 | 18:53:27 → 18:53:30 | 18:53:35, after spawn | none | FAIL (drain ended early) |
| r4 | 18:55:34 → 18:55:45 | 18:55:42, before spawn | typed 18:55:50 (5 s after spawn), submitted | **PASS** |
| r5 | 18:57:54 → 18:58:05 | 18:58:02, before | 18:58:13, submitted | **PASS** |
| r6 | 19:00:35 → 19:00:46 | 19:00:43, before | 19:01:02, submitted | **PASS** |
| r7 | 19:03:23 → 19:03:27 | 19:03:32, after | none | FAIL (drain ended early) |
| r8 | 19:05:18 → 19:05:21 | 19:05:26, after | none | FAIL (drain ended early) |
| r9 | 19:08:01 → 19:08:14 | before (test helper wrote it at 19:08:04, codex rewrote it at 19:08:17) | 19:08:19, submitted | PASS for delivery (assisted; see note) |
| r10 | 19:10:3x → 19:10:44 | never written | none | FAIL (request never reached codex) |
| r11 | 19:14:1x → 19:14:28 | never written | none | FAIL (request never reached codex) |
| r12 | — | — | — | INVALID (test setup: codex fell into "Agent Command Center" after the trust prompt) |
| r13 | 19:19:04 → 19:19:06 | 19:19:11, after | none | FAIL (drain ended early) |
| r14 | 19:21:58 → 19:22:40 | never written | none | FAIL (request typed into a busy codex, Enter did not submit) |
| r15 | 19:26:06 → 19:26:17 | 19:26:19, after | none | FAIL (busy marker never shown, 10 s grace ran out) |
| r16 | 19:31:18 → 19:31:22 | 19:31:33, after | none | FAIL (drain ended early; see the Submit note) |
| r17 | 19:33:08 → 19:33:23 | 19:33:17, before | 19:33:49, submitted | **PASS** |

Totals:
- Unassisted end to end: **4 PASS of 16** (r4, r5, r6, r17).
- Read-handoff delivery whenever the file existed at restore: **5 of 5 typed AND submitted** (r4, r5, r6,
  r9, r17). This was the original bug, and it no longer happens.

r9 note: a helper wrote the handoff file within 0.2 s of the request, to exercise delivery. In r4 the helper
was also on, but codex wrote its own file before the swap anyway, so r4 counts as unassisted.

PASS evidence, r17 new-session screen:
```
› dct 刚重启。你上次的交接在 /private/tmp/gr1/r17/h/.dct/handoffs/1-20261015-193306.md，先读它，再接着做。
• 我先读取上次交接，确认进度和待办，再继续处理。
• Explored
  └ Read 1-20261015-193306.md, SKILL.md (superpowers:using-superpowers skill)
• 已读交接：上次任务已完成，notes.txt 已写入 ROUND17-OK 并验证，已回复 DONE。目前没有待继续的工作。
  Worked for 13s • 19:33
```
r4, r5, r6 and r9 look the same (codex history has the note, and codex's reply cites the file).

## Why the end-to-end flow still fails: codex v0.163.0 busy detection

The codex profile only has `busy_pattern = "esc to interrupt"`, which was measured on v0.146. On the codex
installed here (v0.163.0), that line is missing for long stretches of a running turn.

1. **Short Idle gaps in the middle of a turn end the drain early** (r1–3, r7, r8, r13, r16).
   - After the handoff request, codex prints a sentence (「我会把…写入交接文件…」). The "Working (…esc to
     interrupt)" line then disappears for about 1–2 s before the tool call.
   - `Slot::HandoffAsked` treats `seen_busy && stopped` as done straight away, so the drain finishes about
     3 s after asking and the old codex TUI is killed.
   - State poll r8: `19:05:18.48 Working → 19:05:20.47 Idle` → swap 19:05:21.
   - State poll r7: `Working 19:03:22.80 → Idle 19:03:25.22` → swap 19:03:27.
   - Codex still wrote the file a few seconds later. Its TUI talks to the shared managed `codex app-server`
     under `~/.codex`, and that process outlives the TUI, so the turn finished there (r1 rollout: tool call at
     01:47:57Z, daemon swapped at 01:47:53Z).
   - By then the new daemon had already run `note_for`, the file did not exist yet, and no note was queued.
   - Earlier, outside the drain, a 0.25 s poll of an ordinary codex turn also showed `Working, Working, Idle,
     Working, …`.
2. **No busy marker during a whole turn** (r15).
   - The handoff request was submitted at 02:26:07Z. Codex worked until 02:26:23.9Z (rollout: message, tool
     call, "已写入交接文件").
   - All 11 old-screen captures 19:26:06–19:26:16 contain **no** "esc to interrupt". Only the footer spinner
     (`· ⠹`) showed.
   - dct never saw Working, `HANDOFF_GRACE` (10 s) ran out, and the drain finished at 19:26:17. The file was
     written at 19:26:19, too late.
3. **Codex reported Idle while really busy, so the request was typed into a running turn** (r14).
   - The original task's turn ran 02:21:47–02:22:32Z (40 s waiting on the model after the tool call), but
     dct listed the session Idle from 19:21:57.
   - After the ≥3 s idle gate, the handoff request was typed at about 19:22:06. Codex was mid-turn, so the
     text stayed in the input box with the footer hint `tab to queue message`, and Enter did not submit it.
   - The drain then saw Working, followed by Idle when the turn ended, and finished at 19:22:40. The request
     was never sent, and no file or note followed.
   - The ready gate cannot help here, because the busy state it relies on is wrong.
4. **Request never reached codex, cause unclear** (r10, r11).
   - The drain took about 12 s, which matches the 10 s grace after an AskHandoff. Nothing reached codex
     history or the rollout.
   - The r11 screens (12-line captures) never showed the text in the input box. About 4 s after the restart
     started, the footer went `? for shortcuts` → `GPT-6.1-Sol default` → back to `medium`, which looks like
     the codex TUI re-initialising.
   - Not diagnosed further.

Suggested directions (not implemented):
- Make the codex busy signal reliable on current versions. Options: a pattern for the footer spinner or
  `· <task title>`, or an `idle_pattern` so that `classify` returns `None` for "can't tell".
- Debounce `HandoffAsked` → Done: require Idle to hold for a few seconds, like the ready gate does.
- Or evaluate `note_for`'s `is_file` lazily at delivery time and keep the note pending for a while, so a
  file written a few seconds after the swap still gets a note. Today's codex keeps writing after the TUI
  dies, so this alone would have turned r1–3, r7, r8, r13, r15 and r16 into passes.

## Other oddities

- **Driver `Submit` lost its Enter on a fresh codex twice** (r9 and r16, the first task right after the
  trust prompt). This path is not part of the restart feature.
  - The task text stayed in the box, the drain's handoff request was appended to it, and both were submitted
    as one message (history r9: `"Create a file notes.txt … reply DONE.\ndct 马上要重启，…"`).
  - So the handoff request can be typed onto a non-empty input box. The plain Submit path (text, then a
    separate Enter, with no ready gate) shows the same symptom the fix addressed for resume notes.
- Handoff file names now use local time (`1-20261015-185534.md` for 18:55 PDT), as the final fix intended.

## `dct restart --now -y`: PASS

r18: codex idle with a finished task, then `dct restart --now -y`. Full CLI output:
```
正在换新的后台……
已重启
rc=0
```
- No 「剩 0 秒」 line (`grep -c "剩 0 秒"` = 0).
- No handoff request typed:
  - 0 lines for r18 in `~/.codex/history.jsonl`
  - 0 occurrences of 「马上要重启」 in the r18 rollout and in all old- and new-session screen captures
  - handoffs directory empty
- Old daemon swapped within 2 s (born 02:36:06Z, then 02:36:33Z).

## Codex config restore proof

- Before the run: copied `~/.codex/config.toml` to
  `scratchpad/codex-config.toml.orig` (sha1 `4cc607ecf2596442e99de5929ceffd2d7771608d`).
- After the run, `diff` showed only 17 added blocks `[projects."/private/tmp/gr1/rN/proj"]` +
  `trust_level = "trusted"` (r1–r18 except r12). I removed exactly those blocks with an anchored regex.
- Result: `diff orig current` → identical; both sha1 `4cc607ecf2596442e99de5929ceffd2d7771608d`.
- Left behind in the real `~/.codex`: the test threads' rollouts under `sessions/2026/10/15/` and their
  lines in `history.jsonl`. Codex writes these through the symlink. I did not edit them.

## Cleanup proof

- `dct stop --all` against each round's own socket (18 rounds, each 「已停止 N 号会话」). Then SIGTERM to the
  18 isolated daemons (`/private/tmp/gr1/bin/dct daemon /private/tmp/gr1/rN/h/.dct/daemon.sock`).
- All 36 recorded codex child pids of those daemons confirmed gone. `ps | grep /private/tmp/gr1` shows
  nothing but the grep.
- `tmux -L gr1 kill-server` → `no server running on /private/tmp/tmux-502/gr1`.
- `/private/tmp/gr1` deleted. The `.codex` symlinks inside it were removed as links; `~/.codex` is intact.
- Codex app-server processes: both running ones have the real HOME and were started before this run
  (11 Oct and 15 Oct 15:24). Neither is mine, and both were left alone.
- Real daemon `/Users/lei/.local/bin/dct daemon /Users/lei/.dct/daemon.sock`, pid 26437 (started Sun 11 Oct
  06:17), was never contacted and is still running. `~/.dct` and `~/.local/bin` were not touched.
- No source changes and no commits. The only new file in the worktree is this report.
