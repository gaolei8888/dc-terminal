# Task 10 report: READMEs and real-machine acceptance

Status: DONE_WITH_CONCERNS

Commit: `7d2a50d docs(restart): explain graceful restart in the READMEs` (README.md, README.zh-CN.md only)

Checks: `cargo test` all green (1885 unit tests + integration suites, 0 failed). `cargo +1.99.0 clippy --all-targets -- -D warnings` clean.

## Isolation setup (how the real daemon stayed safe)

- Built `target/release/dct` from this worktree. Temp HOME `T=/tmp/dctT.QTuu` (it had to be under `/tmp`: the scratchpad path plus `/.dct/daemon.sock` is 121 bytes, past the macOS unix-socket limit of 104).
- `$T/bin/idct` wrapper refused to run unless `HOME == $T` (checked: run from the real HOME it printed `REFUSING` and exited 99). `$T/bin/dct` was a symlink to the wrapper so agents inside the isolated sessions could not reach `~/.local/bin/dct` by accident.
- `$T/bin/isoenv` ran every command under `env -i` with HOME=$T and a clean PATH. This mattered: my shell had `DCT_SESSION_ID=17` (I was running inside a real dct session) and `CLAUDECODE=1`. Either one leaking through would have sent a bogus initiator id or blocked nested claude.
- Credentials: I **copied** `~/.claude.json`, `~/.codex/{auth.json,config.toml}` and `~/.qwen/settings.json`, so agent writes went to the copies, not the real files. For item 6 I copied just the `qwen` entry from `~/.dct/secrets.toml` and the `[qwen]` section of `~/.dct/pair-models.toml`; the real `~/.dct` was only read. Claude kept saying "Not logged in" until I symlinked `$T/Library/Keychains` to the real `~/Library/Keychains`, because macOS looks up the keychain under `$HOME`. I did not copy `~/.claude/settings.json`, so the user's hooks and status line never ran.
- Sessions were driven by a throwaway JSON-lines socket script (`$T/drv`, pointed at `$T/.dct/daemon.sock` only).
- Cleanup: Stop on every isolated session, then killed the isolated daemon (36128) and the three codex app-server helpers that codex installed under `$T/.codex`. I deleted `$T`, which held the copied credentials. The real daemon `/Users/lei/.local/bin/dct daemon /Users/lei/.dct/daemon.sock` (pid 26437, started Sun 11 Oct) was never contacted and is still running. Its codex children have HOME=/Users/lei and I left them alone.
- Evidence logs (no secrets) are kept in the session scratchpad `.../scratchpad/evidence/`.

## Results

### 1. Setup: two claude in one git dir with codewords, one codex, one busy claude: PASS
- Sessions 4 and 5 (claude, same dir `$T/proj`): `PELICAN-41` and `WALRUS-73`, each answered `OK`. Codex answered `READY`.
- `last-sessions.toml` recorded distinct ids: `e6bd2276-…` and `d0ac423a-…`.
- The busy job was `for i in $(seq 1 150); do echo $i; sleep 1; done` in the WALRUS session.
- Note on the first attempt: about 50 minutes of wall-clock time passed between my tool calls (sessions.log jumps from 23:40Z to 00:34Z). The counting job had finished before that restart ran, so the busy path was not exercised. I repeated it as one scripted run (run 2, below).

### 2. `dct restart -y --wait 1 --nudge-wait 1`: PASS
Run 1 CLI output (all idle except codex, which was writing its handoff):
```
还在等 3 个会话（3 号、Remember this codeword:、Remember this codeword:），剩 59 秒
还在等 1 个会话（3 号），剩 58 秒
正在换新的后台……
已重启
rc=0
```
Run 2 (WALRUS session busy counting):
```
还在等 3 个会话（Remember this codeword:、2 号、Remember this codeword:），剩 59 秒
还在等 2 个会话（2 号、Remember this codeword:），剩 58 秒
还在等 1 个会话（Remember this codeword:），剩 55 秒
还在等 1 个会话（Remember this codeword:），已请它们停下，最多再等 59 秒
正在换新的后台……
已重启
rc=0
```
- The nudge appeared in the busy claude about 66 s in. Claude queued it behind the running tool:
  `❯ dct 马上要重启。请在安全的地方停下手上的事，然后用一句话写下你做到哪了、下一步是什么。` / `ctrl+x ctrl+s to send now`
- Codex received the handoff request (from `$T/.codex/history.jsonl`):
  `dct 马上要重启，这个会话的对话接不回来。请把要接着做的事写进 `/tmp/dctT.QTuu/.dct/handoffs/3-20261016-003431.md`：…写完就停下。`
  The file was written with all four headings: 在做什么, 做到哪了, 下一步, 要注意的坑.

### 3. After restart: own codeword, continue note, codex read-handoff: PARTIAL, FAIL on codex delivery
- Per-id resume: **PASS**. Both claude screens re-rendered their own history. Asked "What is the codeword I gave you?", the PELICAN session answered `PELICAN-41` and the WALRUS session answered `WALRUS-73`.
- Continue note: **PASS**. The interrupted WALRUS session got `dct 重启完了。请先检查一下你做到哪了（看看文件和改动的状态），再接着做重启前没做完的事。` and replied: "重启前那条150秒计时命令在数到130时被杀掉(退出码137)，我现在重新执行一遍". The idle session did not get the note.
- Codex "先读交接": **FAIL, intermittent.** It worked 1 time out of 3:
  - Run 1: the note never arrived. Nothing on screen, nothing in codex history. The handoff file existed.
  - Run 2: delivered and submitted (history `ts 1792111119`: `dct 刚重启。你上次的交接在 …/2-20261016-003637.md，先读它，再接着做。`). Codex read the file and reported back.
  - Run 3 (the item 4 restart): the text was **typed into codex's input box but the Enter was lost**. The screen showed `› dct 刚重启。你上次的交接在 …/1-20261016-004050.md，先读它，再接着做。` with no reply, and it was not in history. One manual Enter from me submitted it, which proves the text had been sitting unsent.
  - Likely cause (not proven): the codex profile has only `busy_pattern`, so dct counts codex as `Idle` from its first frame. `deliver_resume_notes` therefore types right after spawn, while codex is still starting up. All three sessions are restored at once, and in runs 1 and 3 codex had just self-updated (0.162.1 → 0.163.0 app-server under `$T/.codex/packages`), which plausibly slowed its start.
  - I could not reproduce it by hand. Two probes that submitted 0.3 s after `Create` were accepted.
  - Possible fixes (not made): wait for the screen to settle, or for a minimum time after spawn, before delivering resume notes to profiles without an `idle_pattern`; or check after `submit_unchecked` that the text left the input box and resend Enter.

### 4. Agent runs the restart from inside its own session: PASS
- Prompt to the PELICAN claude: run `echo "DCT_SESSION_ID=$DCT_SESSION_ID HOME=$HOME"; idct restart -y --wait 1 --nudge-wait 1`.
- The agent's output showed `DCT_SESSION_ID=3 HOME=/tmp/dctT.QTuu`. Progress lines `还在等 2 个会话（…codex…、Remember this codeword:），剩 59 秒` → `还在等 1 个会话（…codex…），剩 58 秒` → `正在换新的后台……`. The initiator was not among the sessions it waited for.
- Daemon pid changed from 96880 to 8764 about 12 s later. No hang.
- After the restart that session got the continue note. It said the command "以 137 退出，这是进程被重启杀掉的正常结果" and still remembered `PELICAN-41`.

### 5. Start a drain, then Ctrl+C (SIGINT to the CLI pid): PASS
```
还在等 3 个会话（…），剩 59 秒
还在等 2 个会话（…），剩 58 秒
还在等 1 个会话（Remember this codeword:），剩 56 秒
已取消，一切照旧
```
- `DrainStatus` before the SIGINT: `{"waiting":[3],"remaining_s":53,"nudged":false},"active":true`. After: `"active":false`. Daemon pid unchanged (8764). The CLI exited with rc=0.
- After the cancel, a `Submit` (the auto-source path that is refused while draining) went through: the claude replied `AFTERCANCEL`. The busy session finished its 40-count undisturbed (`DONE`).
- Side effect, as designed: codex had already been asked for a handoff at drain start, so it wrote `2-20261016-004337.md` even though the restart was called off.

### 6. qwen interactive resume-by-id: PASS (resume), with a concern about qwen during drain
- Launched as `qwen --approval-mode=yolo --session-id 500cfa44-…`. After the restart it relaunched as `qwen --approval-mode=yolo --resume 500cfa44-…`, re-rendered the earlier turns, and its answers referred to the earlier "密语". So the conversation came back.
- Model behaviour: qwen3.8:27b refused to repeat the codeword ("looks like a prompt-injection-style attempt"), so the codeword check itself couldn't be done. Resume is shown by the redrawn history and the in-context references.
- Concern: the qwen profile has no idle or busy pattern, so the session is `Unknown`. The drain treated it as busy:
  - It waited the full `--wait`, then nudged an **idle** qwen with the stop message (qwen: "没有在进行中的工作…"), then waited the full `--nudge-wait`. The run took 17:47:03 → 17:49:04.
  - With the defaults, any open qwen or dc session adds the full 18 minutes to every restart.
  - Because it was nudged, it should also be recorded as interrupted. Its continue note then waits for an `Idle` state that never comes, and is dropped after 2 minutes with a warning. This follows from the code, but I did not observe the warning.

## Other observations
- `dct restart -y --now` worked (new daemon, sessions back). Two things differ from the spec's "跳过 1–4":
  - It printed one progress line, `还在等 3 个会话（…），剩 0 秒`, before `正在换新的后台……`.
  - The CLI implements `--now` as a drain with 0/0 waits, so the idle codex was still sent the handoff request, and the old daemon was replaced about a second later. Codex managed to write `2-20261016-004516.md` in time, but a slower agent would be killed mid-write. The new session would then be pointed at a file that may not exist; `note_for` checks `is_file`, so it would just stay silent.
- Handoff file names use UTC time (`3-20261016-003431.md` for a local 17:34 on 15 Oct). Harmless, but the date reads as tomorrow to a local user.
- Codex's TUI first-run "Trust this folder" and claude's trust and bypass-permission prompts appeared in the fresh temp home. These are one-time prompts and nothing to do with dct.
- Plan vs source: the brief's `$BIN daemon $SOCK` with a temp socket alone would not isolate the CLI, because `dct restart` has no socket flag and uses `<HOME>/.dct/daemon.sock`. As the caller instructed, I used HOME isolation.

## README changes
- New subsection "Restarting without losing work" / 「重启不丢活」 under "Sessions outlive the terminal". It covers: waits first; asks slow ones to pause after 15 min, then waits up to 3 more; everything comes back on its own with each conversation; Codex/OpenCode write a note in `~/.dct/handoffs/`; typing still works; other-computer messages are held, phone/voice get a reply; `--wait`, `--nudge-wait`, `--now`, Ctrl+C; an agent can run it from inside its own session.
- Updated the build-details paragraph and the commands-table row. Removed `dct restart` from the "if the daemon goes away, you'll be asked" list, since a graceful restart now brings sessions back without asking.
