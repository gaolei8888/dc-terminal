<div align="center">

# dct

### Stop babysitting your agents.

A coding agent needs you sitting there because every few minutes it stops to ask
"is this okay?". dct answers all of those yes — **and can afford to, because it
takes a hidden snapshot of your project before every single turn.** If one makes
a mess, `u` puts it back.

With nobody to babysit, one person can run several at once: a board, a nine-up
grid, one glance to see who's working and who's stuck. Close the terminal and
they keep going. Leave the house and check them from your phone. Teaching a class,
hand out one link and a roomful of people watch — read-only.

![Rust 1.80+](https://img.shields.io/badge/rust-1.80%2B-b7410e?style=flat-square)
![macOS · Linux · Windows](https://img.shields.io/badge/macOS%20·%20Linux%20·%20Windows-005f87?style=flat-square)
![version 0.2.16](https://img.shields.io/badge/version-0.2.16-444?style=flat-square)

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

## What this adds over an agent in tmux

Keeping a process alive is something `tmux` already does, and dct is no better
at it. The difference is the other four things:

**It never asks permission, because it can undo.** A hidden git snapshot before
every turn, `u` to go back one, `d` to see which files a session actually
touched. **Without that the other three don't matter** — you'd still be sitting
there clicking "allow", and running several agents would only make you busier.

**State is read, not guessed.** Every 200ms dct reads each session's screen and
works out what it's doing: working, idle, stopped, failed. Nine agents on one
board, and you can tell which is stuck without opening any of them. When it
can't tell, it prints "—" instead of guessing — `shell` sessions used to get
guessed as "working".

**Sessions belong to the daemon, not the terminal.** Close the terminal, drop
the SSH connection, shut the lid: they keep running, and typing `dct` puts you
back exactly where you left.

**Ten agents, one installer.** `dct install claude` brings the Node it needs
with it, and keys all live in `~/.dct/secrets.toml` — no remembering what each
vendor calls its environment variable.

What it costs is in [Things that will annoy
you](#things-that-will-annoy-you) — **read that before you decide**, especially
what "accepts every permission" means outside the project directory.

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

Windows 10 and 11 are what this is built for, and both ship the PowerShell the
installer needs. On anything older it stops with one sentence telling you what
to do, rather than failing halfway down with a wall of red.

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
<summary>Will this machine do?</summary>

<br>

**Recommended** — buy to this column. It's what "ten agents, one front door"
costs when you actually open ten:

| | |
|---|---|
| OS | Windows 11 / macOS 14 / Ubuntu 24.04 |
| CPU | 8 cores |
| RAM | **16 GB** |
| Disk | SSD, 20 GB free |

**Minimum** — the floor for *running at all*, **not a recommendation**: two or
three agents fill it, and the fourth starts swapping.

| | |
|---|---|
| OS | Windows 10 1809 (build 17763) / macOS 12 / Ubuntu 22.04 |
| CPU | dual core, x86_64 or Apple Silicon |
| RAM | **8 GB** |
| Disk | 5 GB free |

The RAM row is **measured, not guessed**. With five claude sessions open at once
on one machine: each `claude.exe` holds 250–400 MB resident (301 MB average), the
daemon itself 23 MB, plus a ten-odd MB shell process per session. That's about
**320 MB per agent**. On 8 GB, after the OS and a browser, two or three is the
honest number; a full board needs 16 GB.

**CPU is not the bottleneck — don't size the machine by it.** dct itself burns
almost none, and agents spend most of their time waiting on an API. What actually
eats CPU is the builds and tests the agent runs for you, so pick core count by
how long *your project* takes to compile, not by how many agents you want.

Disk is dominated by the layer underneath dct, not dct itself (a 5.5 MB
executable). Measured here: 95 MB for the Node runtime, 416 MB for the `claude`
npm package alone, plus 45 MB for the portable git on Windows machines that
lack one. Around 600 MB installed; the rest is your projects, the git snapshots,
and the transcripts — and those grow: `~/.claude` reached 185 MB in a week here.

**These are hard floors, not preferences:**

- **Windows 10 1809 (build 17763).** dct's pseudo-terminal is ConPTY, a system
  API that arrived in 1809, and there is no winpty fallback in the code. Note
  that **the installer only checks the PowerShell version** (5.0+), so 1607-era
  Windows 10 installs fine and then fails to start. Check `winver` first.
- **64-bit only.** The published builds are x86_64 and Apple Silicon. No 32-bit.
- **No native Windows on ARM build.** It should run under the x64 compatibility
  layer, but that is untested — don't standardize a classroom on it.
- **Linux is x86_64 only, and needs glibc 2.35+** (the package is built on
  Ubuntu 22.04). Ubuntu 20.04 won't start, and the error it prints is a dynamic
  linker message with nothing to do with dct. No prebuilt package for ARM boards.
- **Older macOS is untested, not ruled out.** Apple Silicon is 11+ by
  construction; the Intel build is cross-compiled against the macOS 14 SDK with
  no deployment target pinned, so below 12 is unknown territory.
- **git is required, not optional** — the pre-turn snapshot is built on it, and
  that snapshot is the whole reason dct dares turn permission prompts off.

**The machine running the models is not in this table.** Everything above sizes
the machine that *drives* the agents. The model lives behind the gateway, so
this machine **needs no GPU**.

</details>

<details>
<summary>When the classroom network can't reach GitHub</summary>

<br>

Put the release archives and `SHA256SUMS` anywhere your students can reach,
then have them set one environment variable first. **Their install command
stays exactly the same**, and checksums are still verified.

```sh
export DCT_RELEASE_BASE=https://your.host/dct
curl -fsSL https://your.host/dct/install.sh | sh
```

```
$env:DCT_RELEASE_BASE = 'https://your.host/dct'
irm https://your.host/dct/install.ps1 | iex
```

**Build that mirror with `scripts/mirror-sync.sh`** rather than by hand:

```sh
./scripts/mirror-sync.sh --out ./mirror     # fetch latest, verify, lay it out
./scripts/mirror-sync.sh --tag v0.2.14      # or a specific version
```

It puts the four platform archives, `SHA256SUMS`, and `install.sh` /
`install.ps1` in one directory. The installer scripts have to live in the mirror
too — a student who can't reach GitHub can't reach `raw.githubusercontent.com`
either, and would be stuck on the very first command.

Checksums are verified **on the mirror side as well**, and a mismatch refuses to
publish. Students verify too, but that check happens on forty machines, mid-class,
where you can't debug it; verifying here moves the same failure to one machine,
one person, and a moment where you can just run it again.

To ship a new version, run the same command and re-upload. **Asset names carry no
version number** — that is deliberate in `release.yml`, so releasing on a mirror
means overwriting files of the same name, and the student's command never changes.

<br>

**Where to host it?** Any static directory reachable over HTTP: object storage,
the school's own nginx, even a Gitee repo. `install.sh` makes no GitHub-specific
assumption about its source — it builds `$base/<name>` and `$base/SHA256SUMS`
and runs two `curl`s.

On Gitee, **use a raw path, not Releases**:

```sh
export DCT_RELEASE_BASE=https://gitee.com/<you>/<repo>/raw/main/dist
curl -fsSL https://gitee.com/<you>/<repo>/raw/main/dist/install.sh | sh
```

Gitee's release attachments are addressed per tag and have **no `latest`
equivalent** (`/releases/latest/download/<name>` there returns a generic JSON 404
— the route simply doesn't exist), and fixed-name-plus-latest is the whole basis
of this design. A raw path has no such problem: it is an ordinary static directory.

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
the question. With nothing running there is no old daemon to swap and nothing to
ask about, so it just starts a fresh one — the board, or in a script (no
terminal) the background service alone.

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

## Ten agents, one door

Press `N` and you get all ten, **including the ones that don't work on this
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
| DC | the training camp's gateway, using Qwen Code by default | `qwen` + a key |
| Claude | Anthropic's CLI | `claude` |
| Codex | OpenAI's CLI | `codex` |
| OpenCode | open source, many models | `opencode` |
| Qwen Code | Alibaba, its own CLI | `qwen` |
| Kimi | Moonshot, wearing Claude's face | `claude` + a key |
| GLM | Zhipu, same trick | `claude` + a key |
| DeepSeek | same trick | `claude` + a key |
| Qwen API | same trick | `claude` + a key |
| Terminal | a plain shell | |

DC now runs Qwen Code by default and reuses Qwen models saved by earlier automatic
pairing. A custom `profiles/dc.toml` still overrides the builtin. If your existing
`[llm]` uses `provider = "dc"` with a Claude model, change it to `provider = "qwen"`,
`transport = "http"`, and an OpenAI-compatible `model` available to your account.
Existing `[llm]` settings are not rewritten automatically.

The last four — Kimi, GLM, DeepSeek and Qwen API — aren't separate programs at
all. They're `claude` pointed at another Anthropic-compatible endpoint, which is
why they want both the binary and a key. DC is the same trick on the other side
of the fence: it runs `qwen` against the camp gateway's OpenAI-compatible API.
The dialect differs because the models behind that gateway speak that one, and
Claude Code can't.

**DC needs no copy-pasted key**: pick it while this machine has no key configured
yet, hit enter, and it goes straight into pairing — a confirmation page opens in
your browser, you click approve once, and the key plus model names get written
back automatically, no account page to open or paste into. Manual entry is still
there if you want it: press `p` from any step of the pairing screen to switch to
it. The pairing screen also carries one ticked box — "let AI explain errors you
can't read" — and it states its cost rather than its benefit, because turning it
on means the raw error text from your terminal gets sent to the camp gateway.
Press `l` to untick it before you approve in the browser; if your
`~/.dct/config.toml` already has an `[llm]` section, pairing says so and leaves
your own setting alone. Pairing is why DC sits first in the list — for a student on their first day
it's the one line in that table that needs no account, no card and no VPN.
Everyone else can ignore it, or point it somewhere else: it's an ordinary
profile, and a `dc.toml` of your own in `~/.dct/profiles/` replaces it.

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

<details>
<summary>Handing out machines: showing fewer than ten</summary>

<br>

Ten choices is a lot to put in front of someone on their first day, and most of
those rows ask them to go and register somewhere. If you're setting up machines
for a class or a team, name the ones you want in `~/.dct/config.toml`:

```toml
[menu]
agents = ["dc", "shell"]
```

The order you write is the order they appear in. Leave the section out — which is
what everybody else has — and nothing is trimmed. A name that matches no profile
is skipped, and if none of them match, the full list comes back rather than an
empty menu you can't start anything from.

This is read on every request, so editing the file is enough; there's nothing to
restart, and nobody has to lose their running sessions over a menu change.

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
- **The type sizes itself**: the page picks a size that makes the whole screen
  fit, `+` and `−` adjust it, and what you chose is remembered. The bottom bar is
  pinned there and doesn't scroll away with the content.
- **Plug in a keyboard and just type**: light up the keyboard button top right
  and a physical keyboard on an iPad or phone goes straight into the session —
  arrows, `Ctrl+C`, and IME composition included. `PageUp`, `PageDown` and `End`
  still scroll history, the same rule as the desktop.
- **Tap any line on screen** and it lands in the input box, ready to edit and
  send.
- The phone can pick its own colours, screen included, without touching the
  desktop's.
- Anyone on that network who has the token can type into your sessions. It is off
  by default and one keypress from off again.

## Live to a room

Press `L` in a session for the live panel: stage a few sessions, get a link and a
QR code, hand it out. Students open it in a browser and **can only watch**.

It works out of the box — frames travel through the relay at `live.dataclue.cn`
(point `DCT_RELAY` at your own to use a different one).

"Only watch" is not a check somewhere in the code — the pipe runs one way. The
daemon pushes the staged screens to a relay twice a second and students read from
the relay. **Their side has GET and nothing else; no route carries a byte back to
your terminal.**

- **50–200 watching at once.** Your machine's load does not depend on how many —
  one student and two hundred get the same frame. Bandwidth is held down by three
  things: unchanged screens aren't pushed, frames are gzipped, and a request hangs
  waiting for a change (near-zero traffic while you think, everyone lights up the
  moment the screen moves).
- **Text goes over the wire, not pictures.** A screen is 3–5 KB gzipped, and
  students get real text rendering — their own font size, their own dark mode.
- **Two keys**: the students' one only reads, yours only writes, and yours
  **never travels over the protocol**. Knowing the link doesn't let anyone push a
  fake screen or stop your broadcast.
- **Only the staged sessions leave the building**, under names you chose — session
  titles and project paths never go out. Switching to another session doesn't
  broadcast it.
- **The screen keeps saying you are live**: `● live · 2 lanes · 7 watching`, not
  dismissible, and it outranks every other bar message. The dangerous failure was
  never a leaked link — it's forgetting you are broadcasting.
- Changing what's staged keeps the link; `r` is what mints a new one and kills the
  old immediately. After you stop, the student page says the session has ended.
- The QR code on the panel encodes the **full link, token and all** — students
  have to scan it. So don't project the live panel itself.
- Before putting the relay on the public internet, read
  [`docs/deploy-live-relay.md`](docs/deploy-live-relay.md): the reverse proxy must
  allow **`/live/*` and nothing else**.

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

**DC shows no status on the board, and has no resume.** It runs Qwen Code, whose
screen tells and resume flag nobody has measured yet — and this repo does not
invent either, so DC's cell reads `—` forever and closing a DC session and
reopening it starts a fresh one (the interface says as much). The endpoint itself
is up and keys are obtainable; it's these two conveniences that are missing until
someone measures that TUI once.

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

Visible `http://` and `https://` addresses within a screen row are emitted as terminal
hyperlinks. Use your terminal's open-link gesture (usually Cmd-click or Ctrl-click).
If the agent captures the mouse, press `F4` first. This also works in browser
terminals that support OSC 8. Links hidden behind labels and URLs split across
rows are not recovered from the current text-only screen snapshots.

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
src/web/           the LAN phone client: a tiny HTTP server and one page
src/link.rs        dials out to a relay and long-polls it (no switch yet)
crates/dct-link/   the envelope the daemon and the relay share; no Request
crates/dct-srv/    the relay. Phase one has no auth and no encryption, and
                   refuses to bind anything but loopback
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
