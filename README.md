# dc-terminal

`dct` is a board for coding agents. Start a few, let them work in separate projects, close the terminal, shut the laptop — they keep running. Come back, find one made a mess, press one key and it's gone.

**It never asks "is this okay?" precisely because you can always take it back.** Those two things are one thing: before every turn `dct` takes a hidden snapshot of the project, which is what makes it safe to turn permission prompts off entirely. Agents don't stall waiting for you to say yes, and whatever they break is one `u` away from undone. Your branches and your commit history never see any of it.

[中文](README.zh-CN.md) · design notes live in `docs/superpowers/specs/`

---

# What it does today

## Sessions outlive the terminal

The first run starts a background daemon. **The daemon is the product.** Close the terminal window, shut the lid, come back tomorrow — the sessions are still running exactly where they were. `dct` itself just reattaches.

The board holds several agents at once, each in its own project directory, out of each other's way.

## Let go, and take it back

Every agent starts with its own permission-bypass flag. Nothing asks, nothing stops, nothing waits.

Safety isn't about blocking things. It's about being able to reach back and pull them out:

- a hidden snapshot of the project, taken automatically before every turn
- `u` returns to the last one
- `d` shows what this session actually changed
- `s` kills it

Snapshots go through git, but on the side of git you never look at. Your branches, your staging area, your `git log` all stay clean.

## Nine agents, one door

Press `N` and you get all nine, **including the ones that don't work on this machine**. Those are greyed out with the reason, and picking one takes you toward fixing it instead of just saying no:

- not installed → `dct` opens a session running the installer, so you watch it work
- no key → a box to paste into, with a link to wherever you get one (`Ctrl+O` opens it)
- keys get checked against the real endpoint before they're saved, so paste half a key and you find out immediately, not ten seconds later inside a session full of English error text

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

Keys live in `~/.dct/secrets.toml`, mode 0600. They never go anywhere near the profile files, which is deliberate: those you can copy between machines or hand to a colleague.

## Your own agents, no recompile

Drop a TOML file in `~/.dct/profiles/`. Nothing to rebuild, nothing to restart — the directory gets re-read on every request. Use a built-in's name and yours wins; use a new one and it joins the list.

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

## Getting it running

Rust 1.80 or newer, and a C toolchain (Xcode command line tools, or `build-essential`) because the TLS stack compiles some C.

```
cargo build --release
./target/release/dct
```

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
| `g` | tile grid: every session's live screen at once; `Enter` zooms in |
| `q` | quit the board; sessions keep running |
| `Ctrl+Q` | back out one level, wherever you are |

The grid is read-only — arrows move focus, `Enter` zooms into the focused tile (the same attach view as `Enter` on the board), `F3` jumps to the next running session, and `n`/`N`/`p`/`c`/`s`/`u`/`d`/`q` all do exactly what they do on the board. Nothing you type there ever reaches an agent. Stopped sessions show a frozen last screen instead of nothing. More than nine sessions get more pages, with a page indicator.

Inside a session every keystroke goes to the agent, `Esc` included — agents need it for their own popups. `F2`, `F3` and `Ctrl+Q` are the only three keys `dct` keeps: `F2` and `Ctrl+Q` both back out to the board, `F3` jumps straight to the next running session without leaving the attach view.

A session is stuck with the agent it was born with. There's no swapping Claude for Codex halfway through; the whole conversation lives inside that process. Press `N` and start another one.

---

# Where this is going

**None of this is written yet.** It's here so the parts above make sense as a direction rather than a pile of features.

The point was never "use your terminal from anywhere." It's that **development keeps moving while you're not there**. You handle three things: state the goal, make the calls, accept the result. The understanding, writing, testing and fixing in between shouldn't need you watching.

- **Agents come find you instead of sitting there.** An `ask_human` tool: the agent calls it and blocks, the question goes to your phone, your answer comes back as the tool's return value, and it carries on.
- **Phone channels.** Telegram first, because it's the only one that doesn't need a public callback address; then Feishu, WeCom, SMS. If the primary channel fails to send, it falls back automatically and says so in the message. Fallbacks have to be chosen in advance — you can't ask someone which channel they'd like when the thing that's broken is how you ask them things.
- **Exactly one message format.** Outbound is always one sentence plus numbered, labelled options; inbound is always free text. The constraint comes from voice: the question has to survive being read aloud, and the answer is "the second one" rather than `2`. So outbound carries no file paths, no diffs, no code blocks.
- **Tasks replace sessions as the thing you deal with.** You say "fix the white screen after login on mobile" instead of first picking a PTY, an agent and a directory.
- **`dc_llm` stays resident doing the cheap work**: reading status, compacting context, classifying your replies, turning technical detail into a decision card you can read on a phone. The expensive frontier models get called only when there's actual code to write.
- **Done means the tests ran.** Detect the stack and the test command, run it, let the agent fix its own failures within a bounded number of rounds, and only hand it to you for acceptance once it passes.

---

# Things that will annoy you

**The big one: the four vendor endpoints are copied out of public documentation and have never been tested with a real account.** A key can verify fine and the session still fail to start. Until somebody runs them with real credentials, treat Kimi, GLM, DeepSeek and Qwen API as unverified.

Scrolling back doesn't work yet, and in iTerm2 it actively garbles the screen — scroll to the bottom and it repaints. The underlying reason is that `dct` currently keeps zero scrollback, so there's nothing to scroll to. The design is finished (`docs/superpowers/specs/2026-08-04-dct-scrollback-design.md`); the code isn't started.

Permissions are auto-accepted, which means an agent can write outside the project directory. **Those writes are outside the snapshot and undo won't bring them back.**

Two agents in one project will fight over the same files. Different projects, no problem.

`opencode` and `qwen` are in the list but neither has ever actually been run, so they have no screen patterns and their sessions just show `—`.

Switching projects is currently a "recently used" list plus pasting a path by hand. There's no directory browser. This part needs redoing.

The interface itself is Chinese-only. Profiles are already per-language; the UI strings aren't.

---

# For anyone working on the code

Two processes, newline-delimited JSON over a Unix socket at `~/.dct/daemon.sock`, owner-only.

```
src/ui.rs        the TUI — view state machine, event loop, rendering
src/client.rs    one connection, 5s read timeout, reconnects on any error
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
