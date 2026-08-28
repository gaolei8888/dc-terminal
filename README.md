<div align="center">

# dct

### Agents that keep working after you close the laptop.

A board for coding agents. Start a few, let them work in separate projects,
close the terminal. Come back, find one made a mess, press one key and it's gone.

![Rust 1.80+](https://img.shields.io/badge/rust-1.80%2B-b7410e?style=flat-square)
![macOS · Linux · Windows](https://img.shields.io/badge/macOS%20·%20Linux%20·%20Windows-005f87?style=flat-square)
![version 0.2.3](https://img.shields.io/badge/version-0.2.3-444?style=flat-square)

[中文](README.zh-CN.md) · design notes in [`docs/superpowers/specs/`](docs/superpowers/specs/)

</div>

```
dct sessions────────────────────────────────────────────────────────
  ┃ 1 ▾ ai-mania          ~/work            claude×1
▶ ┃    3  idle    rewrite the cra…
    2 ▾ dc-terminal       ~/work/dc         claude×1 codex×1
       1  working fix the login b…
       2  idle    port the picker…

────────────────────────────────────────────────────────────────────
q quit         ai-mania         Enter open  n new  Tab project  ? …
```

Grouped by project. The bar down the left marks where you are — that's the
project `n` opens in.

---

## Install

**macOS · Linux**

```sh
curl -fsSL https://raw.githubusercontent.com/gaolei8888/dc-terminal/main/scripts/install.sh | sh
```

**Windows** — native, no WSL. From PowerShell:

```
irm https://raw.githubusercontent.com/gaolei8888/dc-terminal/main/scripts/install.ps1 | iex
```

What comes down is a single executable of a few megabytes. **No Rust, no
compiler, no waiting for a build.** Open a fresh terminal window afterwards,
step into any folder, and run `dct`.

Before every turn `dct` takes a hidden snapshot of your project, and it takes it
with `git` — without git, undo is dead, and undo is the whole reason `dct` dares
run agents with permission prompts off. So on Windows, if the machine has no git
yet, the installer fetches a portable one for you (45 MB, unpacked in place,
living entirely inside `dct`'s own directory: it writes no registry keys and
touches nothing already on the system). macOS and Linux usually have git
already; when they don't, the installer names the one command to run.

<details>
<summary>When the classroom network can't reach GitHub</summary>

<br>

Put the release archives and `SHA256SUMS` anywhere your students can reach,
then have them set one environment variable first. **Their install command
stays exactly the same**, and checksums are still verified.

```sh
export DCT_RELEASE_BASE=https://your.host/dct
curl -fsSL https://your.host/install.sh | sh
```

```
$env:DCT_RELEASE_BASE = 'https://your.host/dct'
irm https://your.host/install.ps1 | iex
```

The portable git on Windows works the same way, through `DCT_MINGIT_URL`.

One more download happens later, when `dct` fetches the Node runtime the agents need. Both of
those have mirrors laid out exactly like the originals, so two environment variables move them:

```sh
export DCT_NODE_BASE=https://npmmirror.com/mirrors/node
export DCT_NPM_REGISTRY=https://registry.npmmirror.com
```

Two variables rather than one "mirror mode" switch, because they fail separately — a mirror may
carry only one of them, and then you want to move only that half. When a download does fail,
`dct` prints these two lines for you.

</details>

<details>
<summary>Installing elsewhere, and why not <code>cp</code></summary>

<br>

On Unix it lands in `~/.local/bin` by default; `--dir` or `DCT_INSTALL_DIR`
moves it. On Windows the default is `%LOCALAPPDATA%\Programs\dct`, changed with
`-InstallDir`. `--build` / `-Build` skips the download and compiles from source
(you need a checkout for that); `-NoPath` and `-NoGit` skip touching PATH and
skip the portable git.

**Don't `cp` over an installed binary.** On macOS, overwriting the file in place
while the daemon is still executing it leaves the kernel's cached code signature
pointing at content that no longer matches, and the next `dct` is killed during
exec — the terminal shows one line, `zsh: killed`. `codesign -v` will still call
the signature valid, because the copy on disk is. The installers write a new
file and rename it over the old one, so a new binary always lands on a fresh
inode. Windows is the same problem wearing different clothes: there you may not
write an image that is currently executing, so the installer renames the old one
out of the way and moves the new one in.

`dct --version` says which one you ended up with.

</details>

<details>
<summary>Windows toolchain (only if you build from source)</summary>

<br>

Skip this whole section if you used the command above — a prebuilt binary needs
no toolchain at all.

If you really want to build it yourself:

```
winget install --id Rustlang.Rustup -e
winget install --id BrechtSanders.WinLibs.POSIX.UCRT -e
rustup default stable-x86_64-pc-windows-gnu
git clone https://github.com/gaolei8888/dc-terminal
cd dc-terminal
scripts\install.cmd -Build
```

**No Visual Studio Build Tools required.** WinLibs is a mingw you unpack into
your own user directory — no gigabytes, no elevation. The one thing rustup's own
bundled mingw is missing is `as.exe`, and `dlltool` needs it to build the import
libraries for `windows-sys` and friends; without it the build dies on
`dlltool.exe: CreateProcess`, a line that names nothing you could act on. The
installer checks for this before it starts compiling and says what to install.

If you already have the MSVC Build Tools, `rustup default
stable-x86_64-pc-windows-msvc` works too and needs no `as`. Either way nothing in
the dependency tree compiles C: on Windows the TLS goes through the system's own
schannel rather than `rustls`, which drags in `ring`, which wants `lib.exe`. The
released binaries take the msvc road.

`scripts\install.cmd` exists so that PowerShell's default execution policy can't
stop the install with an error that has nothing to do with `dct`; it also reads
`install.ps1` as UTF-8 on your behalf, because that file carries no byte order
mark — it has to survive being piped through `irm ... | iex`. Use
`scripts\install.ps1` directly if you'd rather skip the `.cmd` layer.

WSL works too: run `scripts/install.sh` inside the distribution, exactly as on
Linux. On a fresh Ubuntu, run `scripts/install-wsl-deps.sh` first — it adds
`cc`, `git` and Rust, which `install.sh` does not install for you.

</details>

<details>
<summary>Building from source, and running the tests</summary>

<br>

```sh
cargo build --release
./target/release/dct
```

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets
```

Tests make real git repos, spawn real processes, and bind real sockets, so
they're steadier one at a time. Nothing hits the network. Nothing touches your
actual `~/.dct` either — every data path is derived from the socket path, and
tests point that at a temp directory.

If you rebuild `dct` while the daemon from before the rebuild is still running,
the next start notices, explains that restarting it will end whatever sessions
are currently running (file changes stay, the agents don't), and asks before
touching anything. Say yes and it swaps the daemon in and reconnects; say no and
it carries on with the old one.

`dct restart` does that swap on demand, without opening the board — for when the
rebuild produced the same version number and nothing notices anything is stale.
It asks the same question first, listing what will die; `dct restart -y` skips
the question. With nothing running it says so and starts nothing: restarting is
not starting.

</details>

---

## It never asks "is this okay?"

**Precisely because you can always take it back.** Before every turn, `dct`
takes a hidden snapshot of the project. That is what makes it safe to turn
permission prompts off entirely: agents don't stall waiting for you to say yes,
and whatever they break is one keypress away from undone.

Snapshots go through git, but on the side of git you never look at. Your
branches, your staging area and your `git log` stay clean.

| | |
|---|---|
| `u` | back to the last snapshot |
| `d` | what did this session actually change |
| `s` | stop it |

Agents only run inside a git project — that's where undo comes from. If the one
you picked isn't a repository yet, the agent picker says so before you choose,
and `g` creates one on the spot.

---

## Sessions outlive the terminal

The first run starts a background daemon. **The daemon is the product.** Close
the window, shut the lid, come back tomorrow — the sessions are still running
exactly where they were. `dct` itself is just the window you reattach with.

The board holds several agents at once, each in its own project directory, out
of each other's way.

---

## Nine agents, one door

Press `N` and you get all nine, **including the ones that don't work on this
machine**. Those are greyed out with the reason, and picking one takes you toward
fixing it instead of just saying no:

- not installed → `dct` opens a session and installs it, so you watch it work. **A machine
  with no Node.js is fine** — those agents come from npm, so `dct` first fetches a Node of
  its own into `~/.dct/runtime`, for its own use only: it never joins your system `PATH`
  and never touches a Node you already have
- no key → a box to paste into, with a link to wherever you get one (`Ctrl+O` opens it)
- keys get checked against the real endpoint before they're saved, so paste half
  a key and you find out immediately, not ten seconds later inside a session full
  of English error text

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
| Terminal | a plain shell | |

Those last four aren't separate programs at all. They're `claude` pointed at
somebody else's Anthropic-compatible endpoint, which is why they want both the
binary and a key.

Keys live in `~/.dct/secrets.toml`, mode 0600. They never go anywhere near the
profile files, which is deliberate: those you can copy between machines or hand
to a colleague.

<details>
<summary>Your own agents, no recompile</summary>

<br>

Drop a TOML file in `~/.dct/profiles/`. Nothing to rebuild, nothing to restart —
the directory gets re-read on every request. Use a built-in's name and yours
wins; use a new one and it joins the list.

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

Put the agent's own permission-bypass flag in `command`, or it'll stop and ask
you things. `is_agent = true` turns on snapshots and undo; leave it off for
anything that isn't really an agent.

The two pattern fields are how the board knows whether an agent is busy.
`busy_pattern` matches the screen while it's working; `idle_pattern` is the other
way round. Use `busy_pattern` when you can — "esc to interrupt" stays put,
whereas the placeholder text in an input box vanishes the second someone types.
If you give neither, the board shows `—`. That's on purpose. Making up a status
is worse than admitting you don't know.

There's also `env` for environment variables, `secret` if your agent needs a key
from the user, and `install` for how to install it. Get the TOML wrong and the
picker tells you which file and which line.

</details>

---

## The board

Sessions from the same project sit together under a header that names the agents
that project is running (`claude×2 codex×1`) and whether any of them failed.

| | |
|---|---|
| `Tab` `Shift+Tab` | switch project, one keypress |
| `1`…`9` | go straight to the Nth project |
| `n` | new session, this project's last agent |
| `N` | new session, pick the agent |
| `p` | put another project on the board — and start work in it |
| `x` | take a project with no sessions off the board |
| `←` `→` `Space` | fold / unfold the current project |
| `Enter` | open a session |
| `g` | tile grid: every session's live screen at once |
| `c` | API keys |
| `l` | settings |
| `?` | all keys |
| `q` | quit the board; sessions keep running |

**Every project remembers its own last agent.** Press `n` in project A and you
get claude, press `n` in project B and you get codex — and the bottom bar names
which one you're about to get before you press anything (`n new claude`).

`p` is the one place you say "I want to go to that project", so it goes on to ask
which agent and opens the session. `Tab` and the digits only move the cursor.

<details>
<summary>The bottom bar, the grid, and what a session keeps for you</summary>

<br>

The bottom bar is one line, and the keys that don't fit don't flicker in and out
with the window width: whatever the bar can't hold lives one keypress away behind
`?`, and that door (`? …`) is always the last thing on the line. That screen
lists only the keys that actually do something right now — no `Tab` when there's
only one project, no `x` when the group still has sessions in it.

The middle of the bar is the current project, reversed out of the bar's own
colours so it can't be mistaken for one more key name.

The grid is read-only — arrows move focus, `F3` does the same as `→` (next tile,
stopped sessions included), `Enter` zooms into the focused tile, `g` goes back to
the list, and `Tab`/`1`…`9`/`n`/`N`/`p`/`x`/`c`/`l`/`s`/`u`/`d`/`q` all do exactly
what they do on the board. Two differences: folding is list-only, because in the
grid the left and right arrows move the focus; and the digits work with no number
on screen, because tiles carry no numbering the way group headers do.

`i` is the grid's own key, and the one thing the board has no equivalent for: it
opens a one-line reply box on the focused tile, so you can answer an agent
without leaving the overview. Type and press `Enter` to send. Press `Enter` on an
empty box and it sends a bare Enter — that's how you approve a plan or say "carry
on". `Ctrl+C` interrupts the agent instead. While the box is open the whole
keyboard belongs to it.

Tiles are ordered by project, so one project's sessions stay next to each other,
and every tile says which project it belongs to. Nothing you type there ever
reaches an agent. Stopped sessions show a frozen last screen instead of nothing.
More than nine sessions get more pages, with a page indicator.

Inside a session every keystroke goes to the agent, `Esc` included — agents need
it for their own popups. `F2`…`F6` are the only keys `dct` keeps: `F2` backs out
to the board, `F3` jumps straight to the next running session, `F4` toggles copy
mode, `F5` pastes an image, `F6` opens the colour picker. The bottom-left hint
(`F2 back`) is always there — a disconnect, an error, or a long message can't
push it off the line, because it's the only way out of a session.

You can scroll back through what a session already printed, with
`PageUp`/`PageDown`/`End`. `dct` keeps roughly the last 2000 lines that scrolled
off the top; that's a ceiling, not a promise. A page moves a full screen minus two
lines so you keep your place, and `End` jumps straight back down. While you're up
looking at old output, new lines don't drag your view down with them — the bottom
bar counts how many are waiting and tells you how to get back. Type anything, or
resize the window, and you're snapped back to the bottom.

A session is stuck with the agent it was born with. There's no swapping Claude
for Codex halfway through; the whole conversation lives inside that process.
Press `N` and start another one.

</details>

---

## Sessions get a name

Three `claude` sessions in one project used to all read `3 claude`, `5 claude`,
`7 claude` — the same string with a different number, in every place you'd check
before deciding which one to open.

Now the daemon names each agent session for you: the first time it finishes a
round of work, it hands the model configured under `[llm]` what you said and
what's on screen, and asks for a short name. `3 claude` becomes `3 fix the login
blank screen`, and that's it for the life of the session; it's generated once and
never regenerated. The name is written in whatever language you typed in, not
whatever the interface happens to be showing.

It shows up everywhere a session does: the session list, the tile titles in the
grid, and the reply box's recipient line. There's no way to rename a session by
hand in this version.

---

## On your phone

Settings has a "use your phone" switch. Turn it on and dct prints a QR code in
the terminal; scan it with a phone **on the same Wi-Fi** and you get your
sessions, each one's live screen, and a line to type into.

It is a page served by the daemon on your own network. Nothing goes to a server
— there isn't one — so this works with no internet at all, and stops working the
moment you leave the house. Reaching your machine from anywhere is a separate
piece of work, designed but not built: see
[`docs/superpowers/specs/2026-08-23-dc-terminal-srv-design.md`](docs/superpowers/specs/2026-08-23-dc-terminal-srv-design.md).

- **The first time, your system asks whether to allow it.** Say yes for private
  networks, or the phone cannot connect. dct says so on the screen before you
  press the switch.
- The token lives in the URL fragment, so it goes into the code and never into
  the address written on screen — screens get photographed, projected and
  recorded, and whoever reads that line can type into your terminal.
- The phone never resizes the terminal. A PTY has one size, and two clients
  fighting over it would reflow the agent under the desktop too; the phone scales
  the canvas to fit instead.
- It stops asking for anything the moment the tab goes to the background, so a
  phone in a pocket isn't polling your laptop three times a second.
- Anyone on that network who has the token can type into your sessions. It is off
  by default and one keypress from off again.

## Colours

`F6` inside a session, or the settings page, opens the same list of fourteen
colours for the title and bottom bars. Arrows recolour the bar live against the
real agent screen, `Enter` keeps it, `Esc` puts the old one back, and the choice
survives a restart.

Each one is a background/foreground pair of 256-colour indices, never a named
0–15 colour that a terminal theme could redefine, and a test computes the WCAG
contrast of every pair and refuses anything under 4.5:1. `NO_COLOR` forces the
rules-only theme.

---

## Things that will annoy you

**The big one: the four vendor endpoints are copied out of public documentation
and have never been tested with a real account.** A key can verify fine and the
session still fail to start. Until somebody runs them with real credentials,
treat Kimi, GLM, DeepSeek and Qwen API as unverified.

**Permissions are auto-accepted, which means an agent can write outside the
project directory.** Those writes are outside the snapshot and undo won't bring
them back.

Two agents in one project will fight over the same files. Different projects, no
problem.

`opencode` and `qwen` are in the list but neither has ever actually been run, so
they have no screen patterns and their sessions just show `—`.

Naming a session needs an `[llm]` backend configured in `~/.dct/config.toml`, and
most people don't have one — that's the normal case, not a problem. Without it
the name falls back to the first thing you typed, trimmed short; nothing errors,
nothing interrupts.

Only the first nine groups get a number. From the tenth project on, `Tab` is the
only way there, one step at a time.

<details>
<summary>The mouse, copying, and pasting an image</summary>

<br>

`dct` only takes the mouse when **the agent itself wants it**. Claude Code does
(it uses the mouse to scroll its own screen); codex and plain command-line tools
don't — in those sessions the mouse stays with the terminal as long as nothing
running there asks for it, so click-and-drag text selection and copying work
exactly as they always do. The cost is that the wheel no longer scrolls `dct`'s
own history in those sessions; use `PageUp`/`PageDown`/`End` instead.

To copy inside a session where the agent wants the mouse, press `F4` to enter
copy mode: the mouse goes back to the terminal, the bottom bar says so, and
pressing `F4` again leaves it once you're done. You can also use your terminal's
own modifier (Option in iTerm2) without leaving the session at all. `dct` has no
copy of its own — copying uses whatever your terminal already gives you.

Pasting an image works the other way round, and it needs its own key: `F5`. A
terminal is a pipe for bytes, so a picture can't travel down it — your terminal's
own paste reads the clipboard, finds an image instead of text, and sends nothing
at all. `dct` never even learns you pressed paste, which is why the key can't be
`Ctrl+V`. `F5` makes `dct` read the clipboard itself: it saves the image to a
file under your temp directory and sends **the path** as if you had typed it, and
the agent reads the picture from there. It works with a screenshot
(Win+Shift+S, Cmd+Ctrl+Shift+4) or with an image file copied in Explorer or
Finder — that one is sent where it already is, not copied. Clipboard holding text,
or nothing? The bottom bar says so and nothing is sent. Windows and macOS only
for now.

</details>

The interface comes in Chinese and English. `l` switches it, `DCT_LANG=en`
overrides it for one run, and with neither it follows your system locale.

---

<details>
<summary><b>Where this is going</b> — none of it is written yet</summary>

<br>

It's here so the parts above make sense as a direction rather than a pile of
features.

The point was never "use your terminal from anywhere". It's that **development
keeps moving while you're not there**. You handle three things: state the goal,
make the calls, accept the result. The understanding, writing, testing and fixing
in between shouldn't need you watching.

- **Agents come find you instead of sitting there.** An `ask_human` tool: the
  agent calls it and blocks, the question goes to your phone, your answer comes
  back as the tool's return value, and it carries on.
- **Phone channels.** Telegram first, because it's the only one that doesn't need
  a public callback address; then Feishu, WeCom, SMS. If the primary channel
  fails to send, it falls back automatically and says so in the message.
  Fallbacks have to be chosen in advance — you can't ask someone which channel
  they'd like when the thing that's broken is how you ask them things.
- **Exactly one message format.** Outbound is always one sentence plus numbered,
  labelled options; inbound is always free text. The constraint comes from voice:
  the question has to survive being read aloud, and the answer is "the second one"
  rather than `2`. So outbound carries no file paths, no diffs, no code blocks.
- **Tasks replace sessions as the thing you deal with.** You say "fix the white
  screen after login on mobile" instead of first picking a PTY, an agent and a
  directory.
- **`dc_llm` stays resident doing the cheap work**: reading status, compacting
  context, classifying your replies, turning technical detail into a decision card
  you can read on a phone. The expensive frontier models get called only when
  there's actual code to write.
- **Done means the tests ran.** Detect the stack and the test command, run it, let
  the agent fix its own failures within a bounded number of rounds, and only hand
  it to you for acceptance once it passes.

</details>

<details>
<summary><b>For anyone working on the code</b></summary>

<br>

Two processes, newline-delimited JSON over a Unix socket at
`~/.dct/daemon.sock`, owner-only.

```
src/ui/mod.rs      the event loop, terminal lifecycle, key/render dispatch
src/ui/view.rs     the View enum and its pure functions
src/ui/app.rs      the loop's state, in one struct
src/ui/board.rs    the session list
src/ui/grid.rs     the tile grid — layout maths, cropping, rendering
src/ui/attach.rs   one session, full screen
src/ui/pick.rs     the agent and project pickers
src/ui/secret.rs   the key pages
src/ui/widgets.rs  padding, truncation, status colours
src/theme.rs       is the terminal light or dark, and the dim style
src/settings.rs    language, view mode and bar colour, on disk
src/client.rs      one connection, 5s read timeout, reconnects on any error
src/daemon.rs      request dispatch, thread per connection
src/session.rs     session lifecycle, 200ms tick that reads status off screen
src/pty.rs         PTY plus a vt100 screen buffer
src/profile.rs     profile schema, built-ins, disk loading, availability
src/secrets.rs     ~/.dct/secrets.toml
src/verify.rs      the API-key probe
src/git.rs         hidden snapshots
src/projects.rs    recent projects, last agent used
src/proto.rs       the wire contract
```

Three decisions worth knowing before you change things.

**Availability is computed in the daemon, never in the UI**, because the daemon's
`PATH` is the one the child actually gets spawned with. Ask the question anywhere
else and you can cheerfully report "ready" for something that then fails to start.

**Nothing holds a lock across `create()`.** Starting a session spawns a PTY and
shells out to git, and if you're holding a shared lock while that happens every
other client waits on you. There's a long comment in `src/session.rs` and a test
that measures it.

**The protocol carries strings that are already in the user's language.**
`ProfileEntry.label` is a `String`, not a `LocalizedText`. Exactly one place
decides how user-facing text gets built, and it's the daemon.

### House style

Comments explain why, not what. The density in this codebase is deliberate, and
it's saved us more than once; match it.

Every string a user can see is written for someone who has never programmed. No
jargon, no stack traces, no raw OS error text, and an error that doesn't tell you
what to do next isn't finished.

Never advertise a key that can't be pressed, and never leave a pressable key off
the screen.

No emoji as icons.

Never `continue` in a key-handling branch. It skips the bottom of the loop, which
is where stale status messages get cleared, and we've already shipped that bug
once — `e0ba1ec`, where a routine "switched to X" message covered up the only line
on screen telling the user how to quit.

</details>
