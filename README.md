# dc-terminal

I wanted to kick off a coding agent, shut the laptop, and keep an eye on it from my phone. That's the goal. Today `dct` does the local half of it well and the phone half not at all.

What works: a board with several agents on it, each one working in its own project directory. They run with permission prompts turned off, so they don't sit there waiting for you to say yes. Before every turn `dct` takes a hidden snapshot of the project, so if an agent makes a mess you press one key and it's gone. Your branches and your commit history never see any of this.

[中文](README.zh-CN.md) · design notes live in `docs/superpowers/specs/2026-08-01-dc-terminal-design.md`

## Getting it running

Rust 1.80 or newer, and a C toolchain (Xcode command line tools, or `build-essential`) because the TLS stack compiles some C.

```
cargo build --release
./target/release/dct
```

The first run starts a background daemon. That daemon is the point: close the terminal, close the laptop lid, come back tomorrow, your sessions are still running. `dct` on its own just reattaches to them.

## The board

| Key | |
|---|---|
| `n` | new session with whatever agent you used last, straight in, no menu |
| `N` | new session, pick the agent |
| `p` | switch project — sets where the *next* session opens |
| `↑` `↓` | move around |
| `Enter` | open a session |
| `u` | undo, back to the last snapshot |
| `s` | stop a session |
| `d` | what did this session change |
| `c` | API keys |
| `q` | quit the board; sessions keep running |
| `Ctrl+Q` | back out one level, wherever you are |

Inside a session every keystroke goes to the agent, `Esc` included — agents need it for their own popups. `F2` and `Ctrl+Q` are the only two keys `dct` keeps.

A session is stuck with the agent it was born with. There's no swapping Claude for Codex halfway through; the whole conversation lives inside that process. Press `N` and start another one.

## The agents

Press `N` and you get all nine, including the ones that don't work on this machine. Those show up greyed out with the reason, and picking one takes you toward fixing it instead of just saying no.

| | | needs |
|---|---|---|
| Claude | Anthropic's CLI | `claude` |
| Codex | OpenAI's CLI | `codex` |
| OpenCode | open source, many models | `opencode` |
| Qwen Code | Alibaba, its own CLI | `qwen` |
| Kimi | Moonshot, wearing Claude's face | `claude` + a key |
| GLM | Zhipu, same trick | `claude` + a key |
| DeepSeek | same trick | `claude` + a key |
| Qwen API | same trick | `claude` + a key |
| Command line | a plain shell | |

Those last four aren't separate programs at all. They're `claude` pointed at somebody else's Anthropic-compatible endpoint, which is why they want both the binary and a key.

Pick something that isn't installed and `dct` opens a session running the installer, so you can watch it work rather than being told to go away and come back. Pick something with no key and you get a box to paste into, plus a link to wherever you get one (`Ctrl+O` opens it). The key gets checked against the endpoint before it's saved — paste half a key and you find out immediately, not ten seconds later inside a session full of English error text.

Keys live in `~/.dct/secrets.toml`, mode 0600. They never go anywhere near the profile files, which is deliberate: those you can copy between machines or hand to a colleague.

## Your own agents

Drop a TOML file in `~/.dct/profiles/`. Nothing to recompile, nothing to restart — the directory gets re-read on every request. Use a built-in's name and yours wins; use a new one and it joins the list.

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

Put the agent's own permission-bypass flag in `command`, or it'll stop and ask you things. `is_agent = true` turns on snapshots and undo; leave it off for anything that isn't really an agent.

The two pattern fields are how the board knows whether an agent is busy. `busy_pattern` matches the screen while it's working; `idle_pattern` is the other way round. Use `busy_pattern` when you can — "esc to interrupt" stays put, whereas the placeholder text in an input box vanishes the second someone types. If you give neither, the board shows `—`. That's on purpose. Making up a status is worse than admitting you don't know.

There's also `env` for environment variables, `secret` if your agent needs a key from the user, and `install` for how to install it. Get the TOML wrong and the picker tells you which file and which line.

## Things that will annoy you

Two agents in one project will fight over the same files. Different projects, no problem.

Scrolling back doesn't work yet, and in iTerm2 it actively garbles the screen. Scroll to the bottom and it repaints. The underlying reason is that `dct` currently keeps zero scrollback, so there's nothing to scroll to; that's on the list.

Permissions are auto-accepted, which means an agent can write outside the project directory. Those writes are outside the snapshot and undo won't bring them back.

`opencode` and `qwen` are in the list but I've never run either one, so they have no screen patterns and their sessions just show `—`.

And the big one: **the four vendor endpoints are copied out of public documentation and have never been tested with a real account.** A key can verify fine and the session still fail to start. Until somebody runs them with real credentials, treat those four as untested.

---

# For anyone working on the code

Two processes, newline-delimited JSON over a Unix socket at `~/.dct/daemon.sock`, owner-only.

```
src/ui.rs        the TUI — view state machine, event loop, rendering
src/client.rs      one connection, 5s read timeout, reconnects on any error
src/daemon.rs    request dispatch, thread per connection
src/session.rs   session lifecycle, 200ms tick that reads status off the screen
src/pty.rs       PTY plus a vt100 screen buffer
src/profile.rs   profile schema, built-ins, disk loading, availability
src/secrets.rs   ~/.dct/secrets.toml
src/verify.rs    the API-key probe
src/git.rs       hidden snapshots
src/projects.rs  recent projects, last agent used
src/proto.rs     the wire contract
```

Three decisions worth knowing before you change things.

Availability is computed in the daemon, never in the UI, because the daemon's `PATH` is the one the child actually gets spawned with. Ask the question anywhere else and you can cheerfully report "ready" for something that then fails to start.

Nothing holds a lock across `create()`. Starting a session spawns a PTY and shells out to git, and if you're holding a shared lock while that happens every other client waits on you. There's a long comment in `src/session.rs` and a test that measures it.

The protocol carries strings that are already in the user's language. `ProfileEntry.label` is a `String`, not a `LocalizedText`. Exactly one place decides how user-facing text gets built, and it's the daemon.

## Building

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets
```

Tests make real git repos, spawn real processes, and bind real sockets, so they're steadier one at a time. Nothing hits the network. Nothing touches your actual `~/.dct` either — every data path is derived from the socket path, and tests point that at a temp directory.

## House style

Comments explain why, not what. The density in this codebase is deliberate, and it's saved us more than once; match it.

Every string a user can see is written for someone who has never programmed. No jargon, no stack traces, no raw OS error text, and an error that doesn't tell you what to do next isn't finished.

No emoji as icons.

Never `continue` in a key-handling branch. It skips the bottom of the loop, which is where stale status messages get cleared, and we've already shipped that bug once — `e0ba1ec`, where a routine "switched to X" message covered up the only line on screen telling the user how to quit.

## Not there yet

The `ask_human` bridge. Phone channels — Telegram, Feishu, WeCom, SMS. Context compaction through `dc_llm`. Commands from the phone side. Scrollback. And the interface itself is Chinese-only: profiles are already per-language, the UI strings aren't.
