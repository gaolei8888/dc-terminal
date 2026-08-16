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

To put it on your `PATH` instead of running it out of `target/`:

```
./scripts/install.sh
```

That builds, installs to `~/.local/bin` (`--dir` or `DCT_INSTALL_DIR` to put it
elsewhere), and runs the result once to prove it starts. Use it rather than
copying the binary over an installed one yourself. On macOS, overwriting the file
in place while the daemon is still executing it leaves the kernel holding a stale
code signature for that inode, and the next `dct` you type dies at `exec` with
nothing but `zsh: killed` to show for it — `codesign -v` will insist the signature
is fine, because the copy on disk is. `install.sh` writes a new file and renames
it over the old one, so the new binary always gets a fresh inode.

If you rebuild `dct` while the daemon from before the rebuild is still running, the next start notices, explains that restarting it will end whatever sessions are currently running (file changes stay, the agents don't), and asks before touching anything. Say yes and it swaps the daemon in and reconnects; say no and it carries on with the old one.

## The board

The board is **grouped by project**. Sessions from the same project sit together under a header line that names the agents that project is running (`claude×2 codex×1`) and whether any of them failed. A vertical bar down the left marks the project your cursor is in — that's where `n` opens.

| Key | |
|---|---|
| `Tab` `Shift+Tab` | switch project, one keypress |
| `1`…`9` | go straight to the Nth project (the number is printed on the header; only the first nine get one) |
| `n` | new session in the current project, with that project's last agent |
| `N` | new session, pick the agent |
| `p` | put another project on the board |
| `x` | take a project with no sessions off the board |
| `←` `→` `Space` | fold / unfold the current project |
| `↑` `↓` | move around |
| `Enter` | open a session |
| `u` | undo, back to the last snapshot |
| `s` | stop a session |
| `d` | what did this session change |
| `c` | API keys |
| `l` | settings (interface language) |
| `g` | tile grid: every session's live screen at once; `Enter` zooms in |
| `?` | all keys |
| `q` | quit the board; sessions keep running |
| `Esc` | back out one level in the pickers, keys page and settings |

**Every project remembers its own last agent.** Press `n` in project A and you get claude, press `n` in project B and you get codex — and the bottom bar names which one you're about to get before you press anything (`n new claude`). A project that has never had a session just says `n new`, and pressing it opens the agent picker; so does a window too narrow to fit the name without pushing another key off the line.

The bottom bar is one line, and the keys that don't fit don't flicker in and out with the window width: whatever the bar can't hold lives one keypress away behind `?`, and that door (`? …`) is always the last thing on the line. That screen lists only the keys that actually do something right now — no `Tab` when there's only one project, no `x` when the group still has sessions in it.

The grid is read-only — arrows move focus, `F3` does the same as `→` (next tile, stopped sessions included), `Enter` zooms into the focused tile (the same attach view as `Enter` on the board), `g` goes back to the list, `?` opens the same all-keys screen, and `Tab`/`1`…`9`/`n`/`N`/`p`/`x`/`c`/`l`/`s`/`u`/`d`/`q` all do exactly what they do on the board. Two differences: folding is list-only, because in the grid the left and right arrows move the focus; and the digits work with no number on screen, because tiles carry no numbering the way group headers do — `1` is still the first project.

`i` is the grid's own key, and the one thing the board has no equivalent for: it opens a one-line reply box on the focused tile, so you can answer an agent without leaving the overview. Type and press `Enter` to send. Press `Enter` on an empty box and it sends a bare Enter — that's how you approve a plan or say "carry on". `Ctrl+C` interrupts the agent instead. While the box is open the whole keyboard belongs to it.

Tiles are ordered by project, so one project's sessions stay next to each other, and every tile says which project it belongs to. Nothing you type there ever reaches an agent. Stopped sessions show a frozen last screen instead of nothing. More than nine sessions get more pages, with a page indicator. The grid doesn't scroll a tile's history — zoom in (`Enter`) for that.

Inside a session every keystroke goes to the agent, `Esc` included — agents need it for their own popups. `F2`, `F3` and `F4` are the only three keys `dct` keeps: `F2` backs out to the board, `F3` jumps straight to the next running session without leaving the attach view, `F4` toggles copy mode (more on that below). The bottom-left hint (`F2 back`) is always there — a disconnect, an error, or a long message can't push it off the line, because it's the only way out of a session.

You can scroll back through what a session already printed, with `PageUp`/`PageDown`/`End`. `dct` keeps roughly the last 2000 lines that scrolled off the top; that's a ceiling, not a promise. A page moves a full screen minus two lines so you keep your place, and `End` jumps straight back down. The wheel only does something when the agent itself wants the mouse: Claude Code does, so there the wheel goes straight to Claude Code and `dct` stays out of it, no hint shown — you're scrolling its view, not `dct`'s. codex and plain command-line tools don't want the mouse, so `dct` doesn't capture it there either — the wheel no longer scrolls `dct`'s history, and depending on your terminal it may do nothing or send arrow keys straight to the agent; use `PageUp`/`PageDown`/`End` instead. While you're up looking at old output, new lines don't drag your view down with them — the bottom bar counts how many are waiting and tells you how to get back. Type anything, or resize the window, and you're snapped back to the bottom.

A session is stuck with the agent it was born with. There's no swapping Claude for Codex halfway through; the whole conversation lives inside that process. Press `N` and start another one.

## Sessions get a name

Three `claude` sessions in one project used to all read `3 claude`, `5 claude`, `7 claude` — the same string with a different number, in every place you'd check before deciding which one to open. Now the daemon names each agent session for you: the first time it finishes a round of work — the first `Working → Idle`/`Asking` transition after you've actually said something to it — it hands the model configured under `[llm]` what you said and what's on screen, and asks for a short name. `3 claude` becomes `3 fix the login blank screen`, and that's it for the life of the session; it's generated once and never regenerated. The name is written in whatever language you typed in, not whatever the interface happens to be showing — the two can drift apart because `l` switches the interface language at runtime, but a name, once made, doesn't move with it.

It shows up everywhere a session does: the session list, the tile titles in the grid, and the reply box's recipient line. The attached session's own title carries it too, with one exception — while the connection to the daemon is down, that title's space goes to the warning and the way out instead, not the name. One shared helper decides what to draw for a session, so these places can't quietly drift the way separately-formatted strings always do.

There's no way to rename a session by hand in this version.

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

`dct` only takes the mouse when **the agent itself wants it**. Claude Code does (it uses the mouse to scroll its own screen); codex and plain command-line tools don't — in those sessions the mouse stays with the terminal as long as nothing running there asks for it, so click-and-drag text selection and copying work exactly as they always do. The cost is that the wheel no longer scrolls `dct`'s own history in those sessions; use `PageUp`/`PageDown`/`End` instead.

To copy inside a session where the agent wants the mouse, press `F4` to enter copy mode: the mouse goes back to the terminal, the bottom bar says so, and pressing `F4` again leaves it once you're done copying. You can also use your terminal's own modifier (Option in iTerm2) without leaving the session at all.

`dct` has no copy of its own — copying uses whatever your terminal already gives you.

Naming a session needs an `[llm]` backend configured in `~/.dct/config.toml`, and most people don't have one — that's the normal case, not a problem. Without it, or if the model call times out, or the answer that comes back isn't usable, naming quietly steps back: the name falls back to the first thing you typed, trimmed short. Never typed anything either? Then there's no name at all, and the display shows exactly what it always showed before this feature existed — the agent's profile name. None of that interrupts the session or shows an error; it just runs.

Permissions are auto-accepted, which means an agent can write outside the project directory. **Those writes are outside the snapshot and undo won't bring them back.**

Two agents in one project will fight over the same files. Different projects, no problem.

`opencode` and `qwen` are in the list but neither has ever actually been run, so they have no screen patterns and their sessions just show `—`.

Only the first nine groups get a number. From the tenth project on, `Tab` is the only way there, one step at a time.

The interface comes in Chinese and English. `l` switches it, `DCT_LANG=en` overrides it for one run, and with neither it follows your system locale.

---

# For anyone working on the code

Two processes, newline-delimited JSON over a Unix socket at `~/.dct/daemon.sock`, owner-only.

```
src/ui/mod.rs    the event loop, terminal lifecycle, and the key/render dispatch
src/ui/view.rs   the View enum and its pure functions
src/ui/app.rs    the loop's state, in one struct
src/ui/board.rs  the session list
src/ui/grid.rs   the tile grid — layout maths, cropping, rendering
src/ui/attach.rs one session, full screen
src/ui/pick.rs   the agent and project pickers
src/ui/secret.rs the key pages
src/ui/widgets.rs  padding, truncation, status colours
src/theme.rs     is the terminal light or dark, and the dim style that follows
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
