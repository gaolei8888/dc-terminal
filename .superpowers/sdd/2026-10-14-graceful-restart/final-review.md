# Final whole-branch review: graceful restart (cd4cbc5..7d2a50d)

Reviewer: final whole-branch review (opus). Read-only on the checkout; no tests run (none needed to settle a doubt).

## Strengths

- **The drain state machine is pure and well-tested** (`src/drain.rs`). The `HandoffAsked { seen_busy }` + 10 s grace correctly avoids "asked and done in the same breath". Interrupted only goes to nudged sessions, to the initiator and to anything still unfinished at the hard deadline. All of these match the rulings.
- **Drain control is careful about concurrency** (`src/daemon.rs:338-597`):
  - A generation counter stops a stale drive thread from acting on a newer run.
  - `finishing` makes cancel a no-op once state is being written.
  - The lease is checked before `step`.
  - Lock order is always `run` → session locks; I found no inversion.
  - A failed swap unfreezes the list and abandons the run, so dct keeps running rather than killing everything.
- **The `last_sessions_frozen` mutex holds the lock across the write.** A slow normal persist cannot overwrite the final one, and `tick()` persisting dying sessions after the freeze is harmless.
- **Persistence is compatible both ways.** New fields are `#[serde(default)]` and there is no `deny_unknown_fields`, so a protocol-24 binary reads a new file and the new binary reads old files. `continue_flags` keeps old records on the old `--continue` grouping.
- **The protocol-24 fallback is correct.**
  - `plan_for` sends old daemons down the plain path and says so (`RestartOldDaemonPlain`).
  - Old daemons answer an unknown request with `BadRequest` and keep the connection, so the UI's `DrainStatus` poll against an old daemon is silent and harmless.
  - Voice falls back to `Input` + `Input("")` when protocol < 25.
- **Swapper design.** It is detached, takes the old pid, waits 10 s and then hard-kills. This answers the spec's "agent runs it from inside its own session" pitfall, and acceptance item 4 proved it.
- **`submitted` gating of `conversation_id`** prevents a `--resume` of a transcript that never existed.

## Issues

### Critical

**C1. After a restart, an open dct window silently switches to a different session, and the user's typing goes there.**
`src/session.rs:909` (persist order is `HashMap::values()`, so it is arbitrary), `src/session.rs:1097` (restore hands out new ids 1..N in file order), `src/client.rs:224-236` (`Client::call` reconnects on its own), and the attached view in `src/ui/mod.rs` keeps `View::Attached(id)` while disconnected.
- **What happens.** The new daemon numbers restored sessions from 1 in the order of `last-sessions.toml`, and that order is HashMap order. With 18 sessions, "7" after the restart is almost surely a different agent in a different project. The TUI never leaves the attached view while disconnected. Its next `Screen { id: 7 }` / `Input { id: 7 }` reaches the new daemon, so the screen changes under the user and keystrokes and Enter go to the other agent. The grid and board are reshuffled the same way.
- **Why it matters.** The mechanism is older than this branch. But this branch turns restart from a rare, openly destructive act into a routine one that you do with the UI open: there is a banner for exactly that. Spec §4 asked for the reconnect behaviour to be checked during implementation, and acceptance only used a socket script. This is the "type into the wrong session" risk in its most direct form.
- **Fix (pick one; (a) is cheapest and also keeps "3 号" meaning the same thing to the user):**
  - (a) Persist the session id in `RecordedSession` (`id: Option<u32>`, serde default). In `restore_sessions`, create with that id (add an `id` parameter to `create_inner`), then set `next_id` to max+1. Fall back to fresh ids for old files and collisions.
  - (b) Additionally, or instead: put a per-boot nonce (or the daemon pid) in `Hello`. When the TUI sees it change after a reconnect, it drops to the Board with a "dct 重启过了" message and clears `sent_size` and the grid.
  - Add a test: two sessions, persist, restore, ids unchanged.

### Important

**I1. Root cause of the codex "read your handoff" failure: the note is typed before codex has switched the terminal to raw mode, so the tty line discipline turns Enter into Ctrl+J or throws the input away.**
`src/resume_notes.rs:64`, `src/daemon.rs:1167-1211`, `src/session.rs:1142-1146` + `classify` at `src/session.rs:113-117`.
- **Why dct thinks codex is ready.** The codex profile declares only `busy_pattern`. `classify` therefore returns `Idle` for any screen without "esc to interrupt", including the blank screen at spawn. The repo already documents this at `session.rs` (the `should_notify` / naming comments: "刚创建、还停在启动画面上的会话正是这样"). So the codex session is `Idle` on the first 200 ms `tick` after spawn. `deliver_resume_notes` starts right after restore, and `resume_notes::step` sends on the first `Idle` it sees. That can be well under a second after codex was exec'd, and much earlier when codex self-updates on start, as in runs 1 and 3.
- **What actually breaks.** Until codex (crossterm) enables raw mode, the pty slave is in canonical mode with `ICRNL` and `ECHO`:
  - The `"\r"` that `send_input(id, "")` writes is translated to `"\n"`. The line `text\n` waits in the tty buffer.
  - When codex later reads it in raw mode, crossterm 0.28 decodes a bare `\n` as **Ctrl+J**, not Enter. I checked `crossterm-0.28.1/src/event/sys/unix/parse.rs:99`: `b'\n' if !is_raw_mode_enabled() => Enter`, and otherwise it is a control character. Codex binds Ctrl+J to "insert newline". That gives run 3 exactly: the text sits in the composer and is not submitted, and one manual Enter sends it.
  - If codex (or its launcher or self-updater) reads or flushes stdin before or while switching modes, the canonical line is lost entirely. The echoed text is then wiped by the alt-screen. That gives run 1: nothing on screen, nothing in history.
  - Drain-time requests never hit this because those codex sessions have been in raw mode for minutes. The two hand probes 0.3 s after `Create` succeeded because codex had not just self-updated, which is a timing-dependent outcome.
- **Fix.** Concrete, and it extends the existing ruling:
  1. Add `SessionManager::accepts_keys(id) -> bool`. On Unix it returns `pty.master.get_termios()` (portable-pty 0.8 `MasterPty::get_termios`, already available on `PtySession::_master`) with `ICANON` **cleared**. On Windows (ConPTY, no termios) it returns `true`. Only deliver a resume note when `accepts_keys` is true **and** the ruling's settle condition holds: Idle continuously for ≥ 3 s **and** ≥ 5 s since spawn. Track `idle_since` per note in `resume_notes::Note`, and keep `step` pure by passing it a `ready(id) -> bool` closure.
     - Check once on macOS that `tcgetattr` on the master reflects the slave. If it does not, keep only the settle window there.
  2. Treat `Unknown` like `Idle` for delivery once the settle window has passed and the screen has stayed unchanged for it (hash of `screen_text`). Otherwise the Unknown ruling (qwen and dc get `interrupted = true`) produces a continue note that can never be sent, plus a `ResumeNoteDropped` warning on every restart for every qwen session.
  3. Keep the ruled "no retry after partial send" (text typed, Enter failed means don't resend the text). Optionally, if the session has not left Idle within ~5 s after a successful send, send **one** extra `send_input(id, "")` only (Enter, never text). That is safe because an empty-composer Enter does nothing in claude and codex.
  4. Apply the same `accepts_keys` gate to drain `AskHandoff` and `Nudge` (`src/daemon.rs:547-558`). A codex session opened seconds before `dct restart` has the same problem, and today it would just fall through the 10 s grace with no handoff.

**I2. Claude sessions resumed with `--continue` never get an id, and later they can take over a sibling's conversation.**
`src/session.rs:1188-1199`, `src/last_sessions.rs:199-212` (`continue_flags`), `src/daemon.rs:440` (`Member.resumable` comes from the profile, not the session).
- **What happens.** The user's first restart goes through the plain path and gives each `(dir, claude)` group one `--continue` session (A, no id) plus fresh siblings (B, with ids). A has no id from then on: it gets `--continue` on every later restart. `claude --continue` resumes the **most recently written conversation in the directory**. Once B is the most recently used, A's next restart resumes B's conversation. Two panes then write into one transcript, and A's own conversation is gone.
- **The drain gets it wrong too.** `Member.resumable` is true for A because the *profile* resumes by id. So A is never asked for a handoff, and with `resumed_by_id == false` it gets no continue note either.
- **Fix.**
  - (a) In `Drain` members, `resumable = !submitted || conversation_of(id).is_some()`. Id-less but used claude sessions are then asked for a handoff like codex. Unsubmitted ones need nothing, which also drops the pointless handoff request to a never-used codex.
  - (b) In `continue_flags`, don't give `--continue` to an id-less record when a by-id record in the same `(dir, profile)` has a newer `last_active`. Start it fresh instead; with (a) it has a handoff.
  - (c) Optional, verify on real claude first: `resume_args` + `--fork-session --session-id {id}` gives a continued session a new id, so this ends after one restart.

**I3. Once the user runs `/clear` (or `/resume`) inside claude, resume-by-id brings back the old conversation and can tell it to "continue".**
`src/session.rs:1181-1199`, `src/resume_notes.rs:42-44`.
- **What happens.** dct records only the id it passed at spawn. Claude Code starts a new session id on `/clear`, and its own idle screen keeps suggesting "new task? /clear". After a restart, `--resume <old id>` brings back the pre-`/clear` conversation. If the session was interrupted, the continue note tells the agent to resume the *old* task. A non-programmer will not understand why the agent suddenly redoes last week's work.
- **Fix.**
  - Learn the real current id. Pass claude `--settings '{"hooks":{"SessionStart":[…"dct __conv-id <dct id>"…]}}'`. The hook receives `session_id` and `source` (startup/resume/clear) on stdin and reports them to the daemon.
  - Minimal stop-gap: when a submitted line starts with `/clear` or `/resume`, mark the session's `conversation_id` as unknown. It then gets a handoff instead of a by-id resume, and never a continue note.

**I4. When an agent runs `dct restart` from inside its own session, the restart is silently cancelled after about 2 minutes whenever any session is busy.**
`src/daemon.rs:530`, `src/cli.rs:641-653`.
- **What happens.** The drain is tied to a 10 s CLI lease. Agent shells kill long commands: Claude Code's Bash tool defaults to 2 minutes and allows at most 10. With the default 15 + 3 minute waits, the agent's tool times out, the CLI dies, the lease expires and the drain is cancelled. Nudges and handoff requests may already have gone out. The agent then reports a timeout and may retry with `--now`. The README now advertises "an agent can run it from inside its own session". Acceptance item 4 only used `--wait 1 --nudge-wait 1`.
- **Fix.**
  - When `initiator.is_some()`, the daemon does not apply the lease to that run. The CLI prints that the restart will happen once the other sessions stop (≤ N min) and that this session will be told afterwards, then exits 0.
  - Add `dct restart --cancel` (it sends `DrainCancel`) so the user can still stop it.

**I5. The initiator is told "continue what you didn't finish", and its unfinished work is "run `dct restart`", which risks a restart loop.**
`src/drain.rs:105`, `src/resume_notes.rs:42-44`.
- **What happens.** Acceptance item 4 passed with a strong model, which recognised exit 137 as expected. Weaker profiles (qwen, deepseek, glm) can just re-run `dct restart`. With everyone idle that restarts again in seconds and repeats.
- **Fix.** Persist `restart_initiator: bool` (serde default) and give that session a different note: "dct 已经按你的要求重启完了，你刚才运行的 dct restart 已经完成，不要再运行它；接着做别的没做完的事。"

**I6. On Linux, a restart after upgrading the binary in place fails, and the graceful path does not switch to the binary the user ran.**
`src/daemon.rs:360`, `src/main.rs:76`.
- **Linux.** The swap uses the **old daemon's** `current_exe()`. If the binary was replaced by unlink and rename (cargo install, most installers), `/proc/self/exe` reads `…/dct (deleted)` and `spawn_swap` fails with ENOENT. The drain is then abandoned after every session was already nudged or asked for a handoff, and the user gets "重启没进行下去" on exactly the restart that was meant to pick up the new build.
- **All platforms.** It also breaks the `run_restart` contract ("把守护进程换成当前这个二进制"). The plain path uses the CLI's exe, but the graceful path relaunches whatever path the old daemon came from. If the CLI is `target/release/dct` and the daemon was `~/.local/bin/dct`, the CLI then waits for a protocol that never comes.
- **Fix.** Add `exe: Option<String>` to `Request::Drain`. Protocol 25 is unreleased and Drain is local-only, so the same user supplies the path. The CLI sends `current_exe()`, and the daemon uses it, falling back to its own `current_exe()` with a trailing `" (deleted)"` stripped.

**I7. The new daemon only answers once every session is restored, and the waits do not allow for 18 sessions in big repos.** This is worse than the deferred "20 s → 30 s" minor.
`src/client.rs:135` (swapper waits 10 s for `Hello`), `src/cli.rs:656` (`NEW_DAEMON_WAIT` = 20 s), `src/daemon.rs:162-188` (restore runs synchronously before `listener.incoming()`, and each agent session does a first `git::checkpoint`, `session.rs:1204-1214`).
- **What happens.** With 18 sessions in the dc monorepo, restore can take well over 10–20 s. The swapper then writes a false "新的后台起来后一直没有应答" to `daemon.log`, and the CLI prints `RestartNewDaemonNotUp` and exits 1 even though the restart is fine. A non-programmer reads that as "it broke".
- **Fix.**
  - Preferred: start answering `Hello` before restoring. Run restore on a thread, after `bind_private` and before the accept loop starts serving everything else.
  - At minimum: scale the CLI and swapper waits with the number of records (for example 10 s + 3 s per session, capped at 120 s) and print "正在接回 N 个会话……".

**I8. Mesh messages held "until after the restart" are lost at restart.**
`src/mesh/deliver.rs:471` and `:841-847`; queues live in memory in `Mesh.queues` (`src/mesh/mod.rs:123`).
- **What happens.** Spec §3.1 says held mesh messages are delivered "取消或重启后照常投递". On cancel that is true. On restart the old daemon exits with the in-memory queue, and the sender already got `Queued`. This branch makes queueing much more likely (up to 18 minutes of drain) and promises delivery after the restart in the README.
- **Fix.**
  - Option 1: at `Finish`, serialise non-empty queues into the matching `RecordedSession` (`pending_messages`, serde default). Re-enqueue them on the restored session id, and they go out through the normal idle delivery.
  - Option 2, simpler but a spec deviation: during the drain, refuse new mesh messages like phone and voice ("dct 正在准备重启，稍后再发"). Also journal the drop of anything already queued at `Finish`.

### Minor

- **M1. Cancel after the nudge leaves agents stopped while dct says "一切照旧".** `src/daemon.rs:492-503`, `src/cli.rs:747-751`. Sessions that received `NUDGE_TEXT` (or a handoff request) stop and wait. Fix: on cancel, list the nudged sessions in the CLI message, or queue "dct 不重启了，请接着做" to them via the resume-note path.
- **M2. A cancel can land between `step()` and typing.** `src/daemon.rs:525-559`: `step` runs under the lock and the actions run after it is released, so an `AskHandoff`/`Nudge` can still be typed after a cancel. Fix: re-check `generation` under the lock right before each `submit_unchecked`.
- **M3. A stale `DCT_SESSION_ID` can make a different session the initiator.** `src/cli.rs:641`. Session ids are reassigned on every restart (C1), and a terminal or editor launched from inside a dct session keeps the variable. Running `dct restart` there makes some unrelated busy session the initiator, so it is not waited for and is cut off. Fix: the daemon verifies the initiator by walking the connecting peer's parent pids up to that session's child pid, and otherwise ignores it. Fixing C1 (stable ids) only narrows this.
- **M4. Agents are not killed explicitly before exit.** `src/daemon.rs:595`: `process::exit(0)` relies on SIGHUP (Unix) or the job object (Windows) to end the agents. Now that the new daemon resumes **the same conversation id**, an agent or tool child that survives SIGHUP would write to the same transcript alongside the resumed one. Fix: call `kill_now` on every live session after `persist_final_last_sessions` and before `exit`.
- **M5. Drain typing merges into the user's own half-typed draft and submits it.** `AskHandoff` and `Nudge` go through `submit_unchecked`. If the user is mid-sentence in that pane, the request is appended to their draft and sent. Mesh delivery behaves the same way, and that is older than this branch. Fix later: before typing, send a clear-line key for profiles that support it, or skip sessions with keystrokes in the last N seconds.
- **M6. An unresumable initiator (codex or opencode running `dct restart`) loses its conversation with no handoff.** Fix: the CLI prints a one-line warning so the agent can write its own handoff first.
- **M7. `~/.dct/handoffs/` is never cleaned up,** and cancelled drains also leave files there (acceptance item 5). Fix: prune files older than about 30 days at daemon start.

### Deferred minors re-checked (not re-reported)

All parked minors stay minor, with one exception: **"raise CLI new-daemon wait 20 s → 30 s" is worse than minor.** The problem is restore time growing with session count, not 2 s of swapper slack (see I7). The Windows-only clippy warnings in `llm/cli.rs`, `student_projects.rs` and `game/identify.rs` are pre-existing: none of those files is touched in cd4cbc5..7d2a50d.

## Declined to judge

- Whether Windows `GetModuleFileNameW` still returns the original path after `dct.exe` is renamed during an upgrade: this cannot be tested here, so I raised the Linux case only (I6).
- Whether `qwen --resume <missing id>` exits (the watcher assumes a failed resume exits): this was not verified on real qwen. The spec only required the happy path, and acceptance item 6 covers that.
- Whether `claude --continue --fork-session --session-id` is accepted (I2c): this is from memory of the claude CLI, so I2c is marked "verify first".
- Whether macOS `tcgetattr` on the pty master reflects the slave's `ICANON` (I1 fix): Linux does. I asked for a check and gave a settle-window fallback.
- Windows swapper and job objects: the swapper is spawned by the daemon, which is not in a per-session job, so it should survive. I can't exercise it, and the old `restart_daemon` path has the same Windows shape.
- The UI's `poll_drain` blocking call on the render thread: already a ruled and deferred minor.
- The Unknown-state wait, `--now` handoff and "剩 0 秒", and UTC handoff names: already ruled for the fix wave. I only added that the Unknown ruling needs I1.2, or its continue notes can never be delivered.
- Web phone page typing during a drain: ruled ungated as the user's own typing.
- The first real restart against the protocol-24 daemon kills all 18 sessions immediately: that is the documented fallback, and the user is told before confirming. Its follow-on consequences are I2 and C1.
- The plain path run from inside a dct session leaves no daemon, because the CLI is SIGHUPed before `spawn_daemon`. This is old behaviour and only applies until the first restart. The cheap hardening, which I did not grade: in `Plan::Plain`, spawn the detached `__swap` from the CLI before SIGTERMing the old daemon. That matters more now that the README invites agents to run `dct restart`.
- A session created during the ≤ 2 s finishing window is not persisted: the window is tiny and the session is new.
- Interrupted flags left on disk if the daemon crashes right after a failed swap and before the next normal persist: rare, and the consequence is one stray "continue" note.

## Assessment

**Ready to merge: With fixes.** C1 must be fixed before this is used on the user's 18 sessions. Among the Important items:
- I1, I2, I6 and I7 have direct, observable effects on the first real graceful restarts. I1 is already in the ruled fix wave and only needs the concrete mechanism above.
- I3, I4, I5 and I8 can share the same fix wave. Each one is small.

The drain state machine, the cancel and lease handling, persistence compatibility and the protocol-24 fallback themselves look correct.
