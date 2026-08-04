# dc-terminal

An agent terminal built for vibe coding. Start a task, walk away from the computer, keep steering it from your phone.

[中文文档](README.zh-CN.md) · Design notes: `docs/superpowers/specs/2026-08-01-dc-terminal-design.md`

`dct` runs several coding agents side by side, each in its own project directory, and shows them on one board. Agents run with permission prompts turned off, so they don't stall waiting for you. Before every turn it takes a hidden snapshot, so one keystroke undoes whatever an agent just did — without touching your branches or your commit history.

---

# For users

## Install

You need a recent stable Rust toolchain (1.80 or newer).

```
cargo build --release
./target/release/dct
```

That's the whole install. `dct` starts a background daemon the first time you run it; closing the terminal window does not stop your sessions.

## The board

Running `dct` opens the board — one line per session, with what each agent is doing right now.

| Key | What it does |
|---|---|
| `n` | Start a new session with the agent you used last time (no menu) |
| `N` | Start a new session and pick which agent |
| `p` | Switch project — which directory the next session opens in |
| `c` | Manage keys — change or delete a saved one |
| `↑` `↓` | Move between sessions |
| `Enter` | Open a session; `F2` gets you back to the board |
| `u` | Undo — roll back to the last checkpoint |
| `s` | Stop a session |
| `d` | Show which files this session changed |
| `q` | Quit the board (the daemon and your sessions keep running) |
| `Ctrl+Q` | Back out one level. In a session it returns to the board; on the board it quits. |

Inside a session, everything you type goes to the agent — including `Esc`, which agents use to cancel and to close their own popups. `F2` and `Ctrl+Q` are the two keys `dct` keeps for itself.

## Agents

Press `N` and you get all of them, whether or not they work on this machine. Ones that can't run yet are dimmed with the reason, and choosing one takes you toward fixing it rather than turning you away. The first time you pick one, `n` remembers it — press `n` next time and you go straight back in, no menu.

| Agent | What it is | Needs |
|---|---|---|
| Claude | Anthropic's own CLI | `claude` installed |
| Codex | OpenAI's own CLI | `codex` installed |
| OpenCode | Open source, works with many models | `opencode` installed |
| Qwen Code | Alibaba Qwen, its own CLI | `qwen` installed |
| Kimi | Moonshot AI, through the Claude interface | `claude` installed + an API key |
| GLM | Zhipu AI, through the Claude interface | `claude` installed + an API key |
| DeepSeek | DeepSeek, through the Claude interface | `claude` installed + an API key |
| Qwen API | Alibaba Qwen, through the Claude interface | `claude` installed + an API key |
| Command line | A plain shell, no AI | — |

The last four aren't separate programs. They run `claude` pointed at that vendor's Anthropic-compatible endpoint, which is why they need `claude` installed *and* a key of their own.

**Not installed?** Pick it anyway. If `dct` knows how to install it, it opens a session and runs the installer where you can watch it.

**No key yet?** Pick it anyway. You get a box to paste the key into, with a link to the page where you get one — press `Ctrl+O` to open that page in your browser. The key is checked before it's saved, so a bad paste is caught right there instead of turning into a wall of English errors ten seconds later.

Keys are stored in `~/.dct/secrets.toml`, owner-readable only. They are never written into profile files, so those stay safe to copy and share.

**Changed your mind, or a key stopped working?** Press `c` on the board. It lists only the agents that actually need a key, with each one marked configured or not — pick one and `Enter` to replace it, or `d` to delete it. This is the only place you should ever touch a saved key; editing `secrets.toml` by hand isn't necessary and isn't supported.

## Adding your own agent

Drop a TOML file in `~/.dct/profiles/`. No code changes, no restart — `dct` re-reads the directory on every request. A file whose `name` matches a built-in replaces it; any other name is added to the list.

```toml
name = "myagent"
command = ["myagent", "--yolo"]
is_agent = true
busy_pattern = "esc to interrupt"

[label]
zh = "我的 agent"
en = "My agent"

[note]
en = "What this one is good at"
```

| Field | Meaning |
|---|---|
| `command` | Include the agent's own permission-bypass flag, or it will still stop and ask |
| `is_agent` | `true` gets snapshots and undo; `false` (a plain shell, say) doesn't |
| `busy_pattern` | Regex matched against the screen. Matches → working, doesn't → idle |
| `idle_pattern` | The other direction: matches → idle. `busy_pattern` wins if both are set |
| `env` | Environment variables for the process — a different base URL, for instance |
| `secret` | Declares that this agent needs a user-supplied key, and which variable it goes into |
| `install` | How to install it, if it isn't on this machine |

Prefer `busy_pattern` over `idle_pattern` when you can. An agent's "press esc to interrupt" line is stable; the placeholder text in its input box disappears the moment the user types. With neither, the board honestly shows `—` rather than inventing a status.

If a file you wrote doesn't show up, the picker says why, with the line number.

## Known limits

- Two agents in the same project will step on each other's edits. Different projects are fine.
- A session is bound to one directory for its lifetime. Switching projects means a new session.
- Permissions are auto-accepted, so an agent can write outside the project directory. Those changes are outside the snapshot and undo will not bring them back.
- `opencode` and `qwen` ship without screen patterns — nobody has observed their interfaces yet, so their sessions show `—` instead of a status.
- The four vendor endpoints are written from public documentation and **have not been tested against live accounts**. See `docs/superpowers/specs/2026-08-03-dct-multi-agent-design.md`.

---

# For developers

## Shape of the thing

Two processes talking newline-delimited JSON over a Unix socket at `~/.dct/daemon.sock`, owner-only:

```
src/ui.rs        TUI: view state machine + rendering (ratatui + crossterm)
src/client.rs      |  one connection, 5s read timeout, reconnects on any error
src/daemon.rs    request dispatch, one thread per connection
src/session.rs   session lifecycle, 200ms tick deriving status from screen text
src/pty.rs       PTY + vt100 screen buffer
src/profile.rs   profile schema, built-ins, disk loading, availability
src/secrets.rs   ~/.dct/secrets.toml, 0600, atomic replace
src/verify.rs    API-key probe, injectable transport
src/git.rs       hidden snapshots
src/projects.rs  recent projects, last agent used
src/proto.rs     the wire contract
```

The daemon outlives the UI. Kill the terminal, reattach later, sessions are still there.

**Why the daemon computes availability.** Whether `codex` is on `PATH` is answered where the child will actually be spawned. A UI-side check could report "ready" for something that then fails to start.

**Why no lock is held across `create()`.** Starting a session spawns a PTY and shells out to git. Holding a shared lock across that stalls every other client — `src/session.rs` has the long version, and there's a regression test.

**Why the protocol carries resolved strings.** `ProfileEntry.label` and friends are `String`, already picked for the current language daemon-side, rather than `LocalizedText`. One place decides how user-facing text gets composed.

## Build and test

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets
```

Tests create real git repositories, spawn real child processes, and bind real Unix sockets, so they are steadier run serially. No test touches the network, and none touches your real `~/.dct` — data paths are derived from the socket path, which tests point at a temp directory.

## Conventions

- Comments explain **why**, not what. The density here is deliberate; match it.
- Every user-facing string is written for someone with no programming background. No jargon, no stack traces, no raw OS error text. Errors name the next step.
- **No emoji as icons.**
- In the UI's key-handling branches, never `continue` — it skips the loop tail that clears stale status messages, and this repo has already shipped and fixed that bug once (`e0ba1ec`).

## Not built yet

The `ask_human` bridge; phone channels (Telegram / Feishu / WeCom / SMS); context compaction and classification via `dc_llm`; phone-side commands; interface languages beyond Chinese (the profile schema is already per-language, the interface strings are not).
