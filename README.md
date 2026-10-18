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
![version 0.2.24](https://img.shields.io/badge/version-0.2.24-444?style=flat-square)

[中文](README.zh-CN.md) · design notes in [`docs/superpowers/specs/`](docs/superpowers/specs/)

New session working on this repo? Read [`docs/HANDOFF.md`](docs/HANDOFF.md) first.

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

**Contents:** [What dct is](#what-dct-is) · [Which build has what](#which-build-has-what) ·
[Install](#install) · [Daily use](#it-never-asks-is-this-okay) ·
[Commands](#commands) · [`dct status --json`](#dct-status---json) ·
[Deciding for other programs](#deciding-for-other-programs) ·
[The `[llm]` connection](#the-llm-connection) · [`dct ask`](#dct-ask-the-read-only-chat-entry) ·
[Voice](#voice-dct-voice) · [Config reference](#config-reference) ·
[Files](#files-under-dct) · [Security and privacy](#security-and-privacy) ·
[Troubleshooting](#troubleshooting) · [Development](#development)

## What dct is

dct is a terminal session daemon with a terminal UI in front of it. In the code:

| Part | Where | What it does |
|---|---|---|
| pty | `src/pty.rs` | one pseudo-terminal per session, with a vt100 screen buffer |
| session | `src/session.rs`, `src/git.rs` | the session lifecycle, the 200 ms screen read that works out working / idle / stopped / failed, the hidden git snapshots |
| daemon | `src/daemon.rs`, `src/proto.rs` | owns every session. The UI and every `dct …` command are clients of it over a local socket (`~/.dct/daemon.sock`, one JSON object per line) |
| ui | `src/ui/` | the board, the tile grid and the full-screen session view |

**The boundary.** This is the line the project owner drew; no code enforces it.
dct is the entry point between a person and a model: it starts sessions, keeps
them alive, shows them, and carries what the person says to the model and what
the model asks back. It carries **no business- or application-specific
content**. **dcv** holds the rules and decisions; **dco** does the concrete
operation (looking at the screen, doing the input, output and UI actions). dct
calls them; their logic is not in dct.

- **dcv** is a separate program that stores what the user has confirmed (rules,
  procedures). dct reads and writes it: `dct do` drafts procedures into it, and
  approval goes through it.
- **dco** is a separate program on the same machine. dct talks to it over a
  local socket for `dct voice`, `dct game`, `dct do`, `dct dco-rules`, the
  board's `dco` label and the `dco` block in `dct status --json`. The commands
  that need it are Mac-only according to `dct --help`. Without dco those stop
  working and everything else keeps working.

Some commands in this tree **conflict with that boundary**: `dct do` and
`dct campaign` carry a table of social platforms, and `dct game` carries a
Candy Crush profile. They are documented below because they are in the code.
Nothing in this README says where they will end up.

---

## Which build has what

Two things are easy to mix up here: what the code in this branch does, and what
is on the published `origin/main`. This README describes the branch `dct-ask`
at `2c194da`. The comparison below is `git ls-tree origin/main` against that
commit, using the local `origin/main` ref (`542c53e`, version 0.2.24, socket
protocol 24); it was not fetched when this was written.

| | `origin/main` | branch `dct-ask` |
|---|---|---|
| Board, grid, attach, install, phone page, Telegram, live, several computers, `dct gate`, `dct game`, `dct dco-rules`, `dct llm check` | yes | yes |
| Session-switch keys: `F3` = next works; `F8` = previous is the reliable key; `Shift+F3` reported NOT working by the user in their terminal; `Ctrl+F3` is grabbed by macOS | yes | yes |
| `dct voice` (basic relay) | yes | yes |
| `[llm]` has no default `provider` | no (the default is `claude`) | yes |
| `dct judge`, `[judge]`, `dct judge up / down / status`, `[judge.serve]` | no | yes |
| `dct answer`, the danger gate, the model veto, `dct train record` | no | yes |
| `dco` label on the board and the `dco` block in `dct status --json` | no | yes |
| `dct campaign`, `dct do`, the platform table | no | yes |
| `scripts/jeff-exam`, `tests/fixtures/jeff-exam`, `tests/fixtures/jeff-dev` | no | yes |
| **`dct ask`, `[ask]`, socket protocol 25** | no | **yes, only on this branch** (not in `integration` either) |
| **`dct voice` reads dco's voice route, `voice.log` without transcripts, `voice.toml` `log_text`** | no | **yes, only on this branch** |

The rows between `[llm]` and `scripts/jeff-exam` are also on the local branch
`integration` (`14326d6`); none of them is on `origin/main`. Check it yourself
with `git log --oneline origin/main..HEAD`. Because the socket protocol is 25
here and 24 on `origin/main`, a daemon started from one build is "stale" for
the other: see [Troubleshooting](#troubleshooting).

---

## What's new

On this branch only (see [Which build has what](#which-build-has-what)):

- **[`dct ask`](#dct-ask-the-read-only-chat-entry).** One line of JSON in on
  stdin, one JSON event per line out, for dco's conversation feature. Off until
  you run `dct ask --enable`; memory lives in the daemon's RAM only; closing the
  connection cancels.
- **[Voice no longer logs what you said](#voice-dct-voice).** `voice.log` keeps
  counts and decisions by default; `log_text = true` in `voice.toml` turns the
  text back on. `dct voice` also reads dco's voice route and stays quiet while
  voice is going to chat.
- **[`dct answer`](#dct-answer-one-question-one-line-of-json).** One
  multiple-choice question in, one JSON answer out: Jeff, then the `[llm]`
  model, then "needs you". A rule-based danger gate stops money and permanent
  deletion whoever picked, and an optional model veto can add to it.
- **[dct runs Jeff for you](#running-the-local-judge).** `dct judge up | down |
  status` start and stop the local judge server from `[judge.serve]`.
- **[dco on the board and in `dct status --json`](#dct-status---json).**
- **[`dct train record`](#dct-train-record-saving-an-answer-someone-else-gave).**
- **[Post to a social platform (Mac, early)](#post-to-a-social-platform-mac-early).**
  `dct do` posts an approved text through dco. Not tried on a real screen yet.

On `origin/main` too:

- **[Telegram tells you when it's done](#telegram-tells-you-when-its-done).** A
  session that stops, fails or dies sends you a message; reply to it and your
  words go into that session.
- **[Sessions come back after a restart](#sessions-outlive-the-terminal).** When
  the background service starts over, dct offers to bring back what was running,
  and agents that can resume carry on the same conversation.
- **[A failure, in one sentence](#a-failure-in-one-sentence).** With `[llm]`
  set, a failed session's bottom bar says what went wrong in plain words.
  `dct llm check` tests that connection.
- **[A project picker you can search](#picking-a-project).** Type to filter,
  browse folders with a row that goes up a level, make a new project or paste a
  path.
- **[No more "press `g`"](#it-never-asks-is-this-okay).** A folder that isn't a
  git repository becomes one by itself, and a machine without git is offered it.
- **[Several computers](#several-computers).** Agents on different machines
  leave each other messages; the relay can't read them.
- **[Publish a live session to a fixed address](#live-to-a-room).** `p` in the
  live panel puts it on the relay's public list.
- **[dct in a browser](#dct-in-a-browser),** for machines that can't install
  anything: a container, a link instead of a password, an upload panel, and a
  classroom console.
- **[Previous session key](#the-board).** `F8` jumps to the previous
  session and `F3` to the next (`Shift+F3` / `Ctrl+F3` are not reliable, see below).
- **[Every command on one list](#commands).**
- The running version is on the board's title line and on the settings page.

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

Agents only run inside a git project — that's where undo comes from. **You don't
have to set that up.** Start an agent in a folder that isn't a repository yet and
dct runs `git init` there first. On a machine with no git at all it opens a
session running `dct install git` instead, so you can watch it: on Windows that
fetches a portable git into dct's own directory, elsewhere it names the one
command to run. Pick the agent again once it's done.

---

## Sessions outlive the terminal

The first run starts a background daemon. **The daemon is the product.** Close
the window, shut the lid, come back tomorrow — the sessions are still running
exactly where they were. `dct` itself is just the window you reattach with.

The board holds several agents at once, each in its own project directory, out
of each other's way.

**If the daemon itself goes away** — a reboot, a crash, `dct restart` — the next
`dct` lists what was open last time and asks whether to bring it back. Each line
says what you'll get: `continues` means the agent reopens the same conversation
(Claude, and the four agents that run through it, can), `starts fresh` means a
new session in the same folder with the same agent. Press Enter instead and you start with an empty board.

---

## Commands

`dct` on its own opens the board and starts the daemon if it isn't running.
Everything else:

**Sessions and the daemon**

| | |
|---|---|
| `dct ps` | list the sessions running in the background |
| `dct status [--json]` | is dct running, how many sessions are busy; `--json` is for other programs, see [`dct status --json`](#dct-status---json) |
| `dct stop <n>…` / `--all` | stop sessions, giving them time to wind down |
| `dct kill <n>…` / `--all` | kill sessions outright |
| `dct prune` | clear stopped sessions off the list |
| `dct restart` | swap in the current binary as the daemon (ends every session; `-y` skips the question) |
| `dct daemon` | run only the daemon, no board |
| `dct install <name>` | install an agent — `claude`, `codex`, `qwen`, `opencode` — or `git` |
| `dct llm check` | actually call the `[llm]` connection in your config once, and say whether it works; see [The `[llm]` connection](#the-llm-connection) |
| `dct --version` / `dct --help` | which build you have / the command list |

**Deciding for other programs** (this branch only)

| | |
|---|---|
| `dct judge [file]` / `--probe` | ask the `[judge]` backend (Jeff's `jeff-serve`) one multiple-choice question in Jev format and print each option's probability; `--probe` sends a fixed question to see if it answers. Exit 0 answered, 1 not configured or backend error, 2 the question is wrong |
| `dct judge up` / `down` / `status [--json]` | start, stop (only what dct started) or inspect the local judge server; see [Running the local judge](#running-the-local-judge) |
| `dct answer --json` | read one multiple-choice request on stdin, write one JSON line; see [`dct answer`](#dct-answer-one-question-one-line-of-json). Exit 0 answered or needs you, 1 config unreadable or invalid, 2 bad request |
| `dct train record --json` | record one question someone else answered as a training row; see [`dct train record`](#dct-train-record-saving-an-answer-someone-else-gave). Exit 0 recorded, 1 can't write, 2 bad request |

**Talking** (this branch only unless noted)

| | |
|---|---|
| `dct ask` | for dco's conversation: one JSON line on stdin, one JSON event per line on stdout; see [`dct ask`](#dct-ask-the-read-only-chat-entry) |
| `dct ask --enable` | turn on `[ask]` after you read where your words will go and type `y` |
| `dct ask --forget <conversation id>` | forget one conversation's memory |
| `dct voice` | click the dco octopus and speak; dct types it into your latest agent session as if you had. Mac only, needs dco. On `origin/main` too, without the route and log changes; see [Voice](#voice-dct-voice) |
| `dct dco-rules` | refresh `~/.dct/dco-rules.json` (the file the dc-vault rules page reads) right now; the daemon also does it at start and when dco changes version. On `origin/main` too |

**Several computers** (on `origin/main` too; see [Several computers](#several-computers))

| | |
|---|---|
| `dct login` | sign in with the DC account; the first computer to sign in creates the group |
| `dct invite` | on an old computer: a 6-digit code, valid 10 minutes, usable once |
| `dct join <code> [--name <computer name>]` | on the new computer: join with that code |
| `dct peers` | which computers are in the group and what sessions each has open |
| `dct peers remove <computer name>` | take a computer out of the group |
| `dct send <computer>/<session> "<text>"` | leave a line in a session on another computer (`#id` works as the session); typed now if it is idle, queued if busy |

**Application-specific** (Mac only; they conflict with [the boundary](#what-dct-is))

| | |
|---|---|
| `dct game play [--game candy-crush] [--steps 20] [--dry-run] [--auto-next] [--tries 5] [--ask-model] [--ask-every-step] [--goal "…"] [--goal-number 2]` | let dct play a match-3 level shown in iPhone Mirroring; see [Let the AI play a match-3 game](#let-the-ai-play-a-match-3-game-mac) |
| `dct game ask-bench <record file>… [--limit 40]` | ask the model about recorded positions to see how much better than random it is (touches no phone) |
| `dct game identify [--game candy-crush]` | say which kind of game is on screen now (recognised locally, nothing uploaded) |
| `dct game scene [--game <name>] [--steps 10] [--dry-run]` | hidden-object games: the model points at a place, dct checks it and taps (needs a recent dco) |
| `dct campaign approve <name> --param <platform>.<param>=<value>…` | approve one campaign's text for every platform at once; see [Post to a social platform](#post-to-a-social-platform-mac-early). This branch only |
| `dct do "<goal>" --platform <p> --campaign <name> [--window <id>] [--preview] [--json]` | post a campaign's approved text to one platform through dco. This branch only |

**The browser version** (on `origin/main` too)

| | |
|---|---|
| `dct gate [--bind <address>] [--port <port>] [--upstream <host:port>] [--url <public address>] [--link]` | the door in front of [dct in a browser](#dct-in-a-browser); `--link` only prints the link |

`ps`, `stop`, `kill` and `prune` never start a daemon: asking "is anything
running?" shouldn't turn the answer from no into yes. `restart` does start one,
because what you asked for is a daemon running the new binary. `status` and
`dct ask` don't start one either.

`dct --help` prints Chinese only, whatever `DCT_LANG` says. This README is the
English reference for it. The code also accepts `dct keys` (dct's two signing
keys, held in the Mac's secure enclave) and `dct procedure` (approving
procedures and issuing tickets); neither is in `dct --help` and neither is
described here.

---

## `dct status --json`

For other programs to poll (it is meant for a menu-bar helper). It always exits 0,
whatever state dct is in; only a wrong argument exits 2. It never starts the
daemon.

```json
{"schema_version": 1, "daemon": "up", "dct_version": "0.2.24",
 "sessions": {"busy": 1, "idle": 2, "total": 3}}
```

- `daemon` is `up`, `down` (no daemon answering) or `stale` (a daemon is
  running but speaks a different socket protocol, so dct will not read it).
  `down` carries only `schema_version` and `daemon`; `stale` adds
  `dct_version`; neither has `sessions`.
- `sessions.total` counts every session that has not stopped, so `busy` +
  `idle` can be smaller than `total` (sessions waiting for you, failed, or not
  readable are only in `total`).
- A key that is absent means "this version doesn't know", not zero.

`schema_version` stays 1. **This branch only:** if a local dco is installed
(`~/.dco/endpoint.json` exists), `dct status --json` adds an optional `dco`
block to all three shapes above, read-only, from dco's `status` tool and never
starting dco or the daemon:

```json
"dco": {"state": "waiting_user", "activity": "…", "runs": 1, "attention": ["needs_help"]}
```

`state` is one of `idle`, `running`, `waiting_user`, `paused`, `halted`,
`permission_missing`, `screen_locked`, `down` (dco is installed but can't be
reached) or `unknown` (it answered something we can't read, took longer than the
~0.8 s the whole read is allowed, or reported something we don't recognise). No
`dco` key means dco isn't installed.

`state` combines dco's own activity state with its attention list. Highest wins:
`halted` > `permission_missing` > `screen_locked` > `paused` > `waiting_user`
(an attention item of kind `dco.needs_help` exists, even while dco is running) >
`running` > `idle`. `waiting_user` is shown only for `dco.needs_help`, never for
the other kinds. Anything unrecognised (an unknown activity state or attention
kind, an odd-shaped attention list) never makes the state look healthier: it
turns `idle`/`running` into `unknown` and leaves the alert states as they are.

`attention` lists the recognised kinds with the `dco.` prefix removed
(`halted`, `permission_missing`, `screen_locked`, `paused`, `needs_help`),
deduplicated, most severe first, at most 8; unknown kinds are left out. It is
absent when nothing could be read. `runs` is the number of runs, never their
contents. `activity` is the text of the first `dco.needs_help` item only, capped
at 40 characters. **Only `idle` (with an empty attention list) has been seen on a
real dco.** Everything else, including the kind names and their priority, comes
from dco's own source as relayed by its session, and is unverified.

---

## Deciding for other programs

Everything in this section is **on this branch only**. It exists so another
program (dco, a script) can hand dct a multiple-choice question and get an
answer back without a window popping up.

The local small model is called **Jeff**. It is served by `jeff-serve`, which
answers Jev-format choice questions over HTTP and returns a probability per
option.

`dct judge`, `dct answer` and the Jeff step of `dct do` need a `[judge]` section in
`~/.dct/config.toml`, pointing at a running `jeff-serve`:

```toml
[judge]
base_url = "http://127.0.0.1:8765"
# model = "jeff-latest"      # the name jeff-serve answers to
# api_key_env = "JEFF_KEY"   # read a key from this variable; plain http only to this machine
# threshold = 0.8            # below this, dct answer / dct do go on to the [llm] model, then you
# timeout_ms = 5000
```

dct does not download weights. By default you start `jeff-serve` yourself (or
let dct do it, see [Running the local judge](#running-the-local-judge)). The weights
used so far are Jeff v1.2, `jeff-legacy/Jeff-Qwen3.5-0.8B` at revision
`d86f657` (about 1.6 GB). The server reports it accepts up to 254 options, but
dct itself sends 2 to 20 to `dct judge` and 1 to 20 to `dct answer`.

Without `[judge]` (or without a `base_url` in it), `dct answer` and `dct do`
skip Jeff; without `[llm]` they skip the model too. You are always the last
one asked.

### `dct answer`: one question, one line of JSON

`dct answer --json` reads one request from stdin (at most 1 MiB, one JSON
object, unknown fields refused) and writes one JSON line.

```json
{"state": "What the screen shows and what the goal is",
 "options": [{"key": "a", "text": "Post"}, {"key": "b", "text": "Delete account permanently"}],
 "allow": ["jeff", "llm", "user"],
 "forbid": ["a string that must never be sent to the model"]}
```

- `state`: at most 50 000 characters.
- `options`: 1 to 20 entries. `key` is non-empty, at most 64 characters,
  unique, and not `stuck`; `text` is at most 2 000 characters.
- `picture` (optional): a path of at most 4 096 characters. It is never sent
  to the `[llm]` model.
- `allow` (optional, default all three): which rungs may answer. It can only
  remove rungs, and it cannot switch the danger gate off.
- `forbid` (optional, at most 100 non-empty strings of at most 200
  characters): if any appears in `state` or an option's text, the request is
  never sent to the `[llm]` model (Jeff runs on your machine and is still asked);
  `reason` then ends with `llm_withheld: forbidden string in the request`.

It asks, in this order, and skips any rung that is not configured or cannot
answer:

1. **Jeff** (`[judge]`). The pick counts only when its confidence is at least
   `threshold` (default 0.8). dct recomputes the confidence from the returned
   probabilities and ignores the server's own number.
2. **The `[llm]` model**, with the text only. The same `[llm]` rules as
   everywhere apply: no `[llm]`, no rung.
3. **You.** dct does not ask anyone itself and shows nothing. It answers
   `by: "none"`, `reason: "needs_user"` and exits 0; the caller decides how to
   ask. With `"user"` left out of `allow` the reason is `no_answer`.

Whatever rung picked, the pick then goes through the [danger
gate](#the-danger-gate-dct-answer) and the model veto before it leaves.

Answers:

```json
{"key": "a", "by": "jeff", "confidence": 0.97}
{"key": "a", "by": "model", "confidence": null}
{"key": null, "by": "none", "confidence": null, "reason": "needs_user"}
{"key": null, "by": "none", "confidence": 0.991, "reason": "danger", "gated": {"key": "b", "tier": "critical"}}
```

`by` is `jeff`, `model` or `none`. `confidence` is Jeff's, or `null`. A pick the
model gave for an icon without text may also carry a `name`. Exit codes: 0
answered or needs you (including `danger`), 1 the config can't be read or is
invalid (a broken `[judge] gate_tiers` included), 2 the request is wrong. A
rung that is configured but can't answer (unreachable, too slow, unreadable
reply) is skipped; it never fails the command.

### Running the local judge

Add `[judge.serve]` and dct starts and stops the judge server for you. Everything
about *how* comes from this section; dct has no built-in path or program name.
Leave it out and dct never starts or stops anything.

```toml
[judge]
base_url = "http://127.0.0.1:8796"        # must be this machine

[judge.serve]
command = ["<path to the judge server program>"]   # argv array, no shell
env = { PORT = "8796" }                   # extra environment, strings only
# ready_timeout_secs = 60                 # how long to wait for GET <base_url>/health
# autostart = false                       # true: `dct answer` starts it when unreachable
# min_free_percent = 15                   # refuse to start below this much free memory
```

| Command | What it does |
|---|---|
| `dct judge up` | start it unless `/health` already answers (then it says so and starts nothing); waits up to `ready_timeout_secs`. Exit 0 ready, 1 not |
| `dct judge down` | stop **only the process dct started**: SIGTERM, then SIGKILL after 5 s. Exit 0 stopped or nothing to stop, 1 refused |
| `dct judge status [--json]` | healthy or not, whether dct started it, pid, uptime, memory, log path, last start error. Exit 0 healthy, 1 not |

With `autostart = true`, the first time `dct answer` finds the judge unreachable
it starts it once (one line on stderr, can take up to `ready_timeout_secs`) and
retries once. If that fails or times out, `dct answer` carries on as before:
next rung, then "needs you". Nothing ever starts while `autostart` is not `true`.

Safety:

- **Loopback only.** If `base_url` is not `127.0.0.1` / `localhost` / `::1`, dct
  refuses to start or stop anything.
- **Only its own process.** `up` records the pid, start time and the process's
  command line in `~/.dct/judge-serve.pid`. `down` checks the pid is alive and the
  command line still matches; if not, it does not signal it. A server dct did not
  start is never stopped. Two `dct` calls starting at once spawn one server.
- **Memory.** Below `min_free_percent` free memory (macOS `memory_pressure`, Linux
  `/proc/meminfo`) it refuses to start and says why. On other systems the free
  memory cannot be read and the check is skipped; `down` cannot verify the command
  line there either, so it refuses to stop anything (Windows: not supported yet).
- **No secrets in `command` or `env`.** Any user on the machine can read them with `ps`.
- **Log:** `~/.dct/judge-serve.log` (mode 0600, replaced on each start).
- Not yet: `idle_stop_minutes` (stop after a quiet period) is not implemented and is ignored.

### The danger gate (`dct answer`)

Whoever picks an option (Jeff, the `[llm]` model), `dct answer` looks at the
picked option's label with `dct-brain`'s tier rules before it lets the pick
out. This is rule-based and never reads confidence: Jeff at 0.99 on "Delete
account permanently" is stopped the same as Jeff at 0.5. Jeff still sees every
option; the gate only acts on the answer. Role prefixes (`按钮 `, `文字 `) are
stripped first, so the label is judged the way the exam scripts judge it.

**By default only money and critical (permanent deletion, erasing, factory
reset, authorisation) are stopped; ordinary delete/remove/send/publish/save
pass; add `"content"` to `gate_tiers` to stop those too.** The tiers are
**critical** (irreversible or authorising: delete account permanently, erase,
factory reset, Allow, …), **money** (buy, pay, …) and the opt-in **content**
(sends, posts, saves, deletes content). A gated pick comes back as:

```json
{"key": null, "by": "none", "confidence": 0.991, "reason": "danger",
 "gated": {"key": "b", "tier": "critical"}}
```

`reason` is `danger` (alongside `needs_user` / `no_answer`), the exit code is 0
like any other "needs you" answer, and `gated.key` is the key that was picked.
The JSON never carries label text. Fixed actions such as scroll and wait have
Read-tier text and pass. The request's `allow` only narrows which rungs may
answer; it cannot switch the gate off, and there is no per-request override.

```toml
[judge]
# gate_tiers = ["critical", "money"]              # the default when the key is absent
# gate_tiers = ["content", "critical", "money"]   # opt in: stop content picks too
```

Names are case-insensitive. `critical` and `money` can never be removed; a
list that leaves either out, or names an unknown tier, makes `dct answer` print
the reason and exit 1. Only `content` is optional, and it is off unless you
list it.

**Model veto (on when `[llm]` is configured).** After the rules let a pick
through, `dct answer` asks the `[llm]` model one fixed question about the
picked option's text only (no screen, no goal): does it spend or move money, or
permanently delete data? The model can only add a veto (`gated.source` is
`"model"`, tier `money` or `critical`); it never releases what the rules
stopped. If the model is unreachable, times out or errors, the pick is allowed
and one line goes to stderr; if its answer is unsure or unreadable the pick is
handed to you (`needs_user`, `"veto": "unsure"`). A request that leaves out
`"llm"` from `allow`, or whose text hits `forbid`, is never sent to the model.
Turn it off with:

```toml
[judge]
model_veto = false   # this section needs no Jeff address
```

**Limit, stated plainly: the gate only knows dct-brain's word list; words it
does not know are not stopped; callers should still filter options they know
are destructive.** (Examples it misses today: "Cancel subscription", "Clear
all", "Block", "Deactivate". With `"content"` opted in it also over-stops some
harmless ones, e.g. "Save", "Done", "Share", "Sign in", "Upload".)

### `dct train record`: saving an answer someone else gave

`dct train record --json` reads one request on stdin and appends one training
row (version 2) to `~/.dct/explore/answers.jsonl` (mode 0600). It is for
callers that asked someone else a question, so the answer can train Jeff
later. Nothing else is written.

```json
{"question_id": "q-17", "by": "user", "key": "b", "outcome": "good",
 "run_outcome": "published",
 "corrects_jeff": false, "jeff_asked": true,
 "shown": [{"key": "a", "text": "…"}, {"key": "b", "text": "…"}],
 "step": 3, "state": "…", "forbid": ["a string that must not be recorded"]}
```

- Required: `question_id`, `by` (`jeff` | `model` | `user`), `key`, `outcome`
  (`good` | `bad` | `unknown`) and `run_outcome` (`published` | `declined` |
  `halted` | `stopped`). The rest is optional: `corrects_jeff`, `jeff_asked`,
  `shown` (up to 254), `step`, `state`, `forbid`.
- **The caller must replace the user's own values with `{name}` placeholders
  first.** A request whose `state`, `key` or `shown` texts contain any `forbid`
  string is refused, not stored.
- `label` in the reply says whether the row counts as a training label: a
  user's answer always does; a model's answer only when `outcome` is `good`; a
  Jeff answer never does.
- Answer `{"recorded": true, "label": true|false}`. Exit 0 recorded, 1 the
  file can't be written, 2 the request is wrong.

---

## The `[llm]` connection

`[llm]` in `~/.dct/config.toml` is the one general-purpose model connection
dct itself uses. **It is off until you write the section.** With no `[llm]`
(or a config file that can't be parsed) nothing is ever sent anywhere; a
broken config never stops dct from starting.

Things that use it: naming sessions, [explaining a
failure](#a-failure-in-one-sentence), the second rung of `dct answer` and its
model veto, `dct do`, `dct game play --ask-model`, and (through `[ask]`) `dct ask`.

| Key | Default | Meaning |
|---|---|---|
| `provider` | none | the name of a profile (built in, or yours in `~/.dct/profiles/`). **No default**: dct never picks a vendor; without it the feature stays off and you get a one-line hint |
| `transport` | `"cli"` | `"cli"` runs that profile's own headless command; `"http"` calls the endpoint in the profile's `[api]` section directly |
| `model` | none | required for `http`; dct will not guess one |
| `base_url` | the profile's `[api].base_url` | optional override |

An example with an OpenAI-compatible server is under [A failure, in one
sentence](#a-failure-in-one-sentence). `dct llm check` makes one real call and
says whether it worked.

- **The DC profile.** The built-in `dc` profile points at the camp gateway,
  `https://dc-llm.tzspace.cn/v1` (OpenAI wire format). Pairing writes the key
  to `secrets.toml` and the model names to `pair-models.toml`. The ask design
  document records that chat on that gateway is answered by `qwen3.8:27b` on an
  Ollama server (its section 3.7, measured on 2026-10-16/17 and not re-checked
  since). If your old `[llm]` used `provider = "dc"` with a Claude model,
  change it to `provider = "qwen"`, `transport = "http"` and a model your
  account has. dct never rewrites an existing `[llm]`.
- **Only your own gateway, unless you write something else.** Nothing in the
  code sends to an external frontier model by itself, and there is no switch
  for one. `dct ask` accepts only an `http` `[llm]` whose host is in `[ask]
  allowed_hosts` (default `dc-llm.tzspace.cn`). `[llm]` itself also accepts a
  vendor CLI as provider (`claude`, `codex`, …), which talks to that vendor;
  that happens only if you write it.
- **Credentials.** A key you saved in dct is used for the provider you saved it
  for. A login borrowed from another program (Claude Code, Codex) is only ever
  sent back to that program's own vendor hosts, whatever the profile or
  `base_url` says.
- Pairing's tick box ("let AI explain errors you can't read") appends `[llm]`
  only if you leave it ticked and approve in the browser, and never touches an
  existing `[llm]`.

---

## `dct ask`: the read-only chat entry

**This branch only (`dct-ask`, not even on `integration`).** `dct ask` lets dco
hold a spoken or typed conversation with the model behind `[llm]`. It is a
chat: the reply is shown or spoken, never run. The ask service cannot see your
sessions, cannot type into them and has no tools.

**Turn it on once, yourself:**

1. Have a working `[llm]` with `transport = "http"` and an OpenAI-compatible
   endpoint (see [the `[llm]` connection](#the-llm-connection)).
2. Run `dct ask --enable` in a terminal. It tells you which host your words
   will go to, what is sent and what is not, and writes nothing unless you type
   `y`. It has no `--yes`, and refuses when stdin or stdout isn't a terminal
   (exit 2). It appends this to `config.toml` (it does not rewrite the file,
   and an existing `[ask]` section, on or off, is left alone):

   ```toml
   [ask]
   enabled = true
   allowed_hosts = ["dc-llm.tzspace.cn"]
   ```

   Exit codes of `dct ask --enable`: 0 turned on (or already on), 1 not
   turned on (you didn't say `y`, an `[ask]` section already exists, or the
   file can't be written), 2 not a terminal, 11 `[llm]` isn't usable for `ask`.

3. After installing this build, run `dct restart` once. A daemon from an older
   build doesn't know the `Ask` request (protocol 25) and `dct ask` answers
   `daemon_down` with `reason: "protocol"`. `dct restart` ends every running
   session, so pick a moment.

The `[llm]` consent only covers terminal error text; what you say needs its own
switch, which is why `[ask]` exists. `[ask]` is read on every request, so
turning it on or off needs no restart.

**Calling it.** The text goes on stdin, never on the command line (arguments
are visible to every user through `ps`):

```sh
echo '{"v":1,"text":"What is the weather like?","source":"voice","conversation_id":"c-7f3a"}' | dct ask
```

| Field | Rules |
|---|---|
| `v` | optional; if present it must be `1` |
| `text` | required, at most 500 characters after control characters become spaces; longer is refused, not cut |
| `source` | `"voice"` or `"typed"`; only a hint to the model (speech may be misheard, replies get read aloud). **It is never an authorization** |
| `conversation_id` | `[A-Za-z0-9_-]`, 1 to 64 characters; memory is kept per id |
| `persona` | optional, at most 300 characters, goes under the fixed rules and can't override them |
| `timeout_ms` | optional, default 8000, clamped to 1000–30000 |

stdin is read up to 16 KiB. One event per line comes back on stdout, every
event has `"v": 1`:

```jsonl
{"v":1,"type":"start","conversation_id":"c-7f3a","turn":3}
{"v":1,"type":"sentence","i":0,"text":"It is sunny."}
{"v":1,"type":"end","sentences":1,"truncated":false,"first_ms":1840,"total_ms":2100}
```

Every call ends with exactly one `end` or `error` (nothing if you closed the
connection). A request that is refused (bad input, not enabled, busy) has an
`error` and no `start`. If sentences were already sent when the model dies or
times out, you get `end` with `"truncated": true`, not an `error`. An `error`
carries `code`, `stage`, `retryable`, `after_ms` and a fixed `reason` word
(never any of your text). `dct ask` writes nothing to stderr.

| Exit | `code` | Meaning |
|---|---|---|
| 0 | (`end`) | finished, including `truncated` |
| 2 | `bad_request` | bad input; `reason` is one of `not_json`, `bad_version`, `empty_input`, `too_long`, `bad_source`, `bad_conversation_id`, `persona_too_long`, `usage` |
| 10 | `daemon_down` | no daemon answering (it is **not** started for you), or an old daemon: `reason` is `socket`, `closed` or `protocol` |
| 11 | `not_enabled` | `reason` is `ask_off`, `llm_missing`, `llm_not_http`, `llm_unusable` or `host_not_allowed` |
| 12 | `unreachable` | the gateway can't be reached (DNS, refused, TLS, connect timeout) |
| 13 | `auth` | the gateway said 401/403; retrying doesn't help |
| 14 | `busy` | the gateway said 429/503, or another conversation is already in flight (`reason` `in_flight`; only one at a time) |
| 15 | `slow` | no sentence by the deadline |
| 16 | `empty` | the stream ended with nothing visible to say |
| 17 | `bad_reply` | the response couldn't be understood |
| 18 | `cancelled` | a newer ask in the same conversation replaced this one |

**Cancelling is closing the connection.** dco kills the `dct ask` process; the
socket closes; the daemon stops generating and frees the in-flight slot at
once (the gateway may keep generating for a moment, at most `timeout_ms`). On
Windows the daemon can only notice by a failed write, which is weaker; dco is
Mac-only today.

**What it remembers.** The last 6 turns per conversation, at most 3 000
characters, 4 conversations at a time, dropped after 30 idle minutes. It lives
in the daemon's memory and is **never written to disk**. `dct ask --forget
<conversation id>` drops one. Restarting the daemon drops everything.

**What leaves the machine.** This turn's text, up to 6 turns of history, a
fixed system prompt and your optional `persona`, to the host of your `[llm]`
only. No screen content, no files, no session list. No tools in the request,
`max_tokens` 200, no thinking field.

**What it writes.** One metadata line per ask in `~/.dct/ask.log` (mode 0600,
replaced when it passes 256 KiB): `t`, `source`, `conv` (the first 4 hex
characters of a hash of the conversation id, not the id), `chars_in`,
`chars_out`, `first_ms`, `total_ms`, `outcome`. No text of what you or the
model said. Requests refused as `bad_request` are not logged.

Other uses of the same gateway inside the daemon (error explanations, session
names, the phone page) wait up to 20 seconds while an ask is in flight and then
give up; `dct answer`, `dct do`, `dct game` and `dct llm check` run in other
processes and don't wait.

Design, measurements and the open list: [`docs/2026-10-16-dct-ask-design.md`](docs/2026-10-16-dct-ask-design.md).
Not verified: cancelling while the gateway is still pre-filling, TLS close
behaviour, behaviour under load, and a run against the real gateway
(section 16 of that document says the real-gateway smoke test was not done).

---

## Voice: `dct voice`

Needs dco on a Mac. `dct voice` runs in the foreground (`Ctrl-C` quits) and
listens to what dco hears. Each sentence you speak after clicking the dco
octopus is typed into your most recently active agent session, as if you had
typed it and pressed Enter. Anything said before you started is skipped.

**Voice never counts as authorization.** A sentence is sent only if all of
these hold:

- dco says it was triggered by a click on the octopus (`octopus_click`, the
  only trigger accepted: a voice that sounds like you could be a television);
- confidence is at least 0.5;
- it is not just a bare confirmation ("好", "嗯", "ok", "yes", …);
- the target session is not waiting for an approval prompt (voice can't answer
  those) and is not a plain shell session;
- at least 2 seconds since the previous one;
- you don't say "取消", "算了", "别发" or "cancel" within the 2-second window
  after dct announces it. The state of the session is checked again when the
  window ends.

The text sent is prefixed with `（语音输入，可能有识别错误）` so the agent knows
to be forgiving. Say "给 X 说…", "对 X 说…" or "告诉 X…" (X is a session's
name or `#id`) to pick a session; two matches stop the send.

**Voice route** (this branch only). Before each sentence dct reads
`converse.voice_route` from dco's `status`, fresh every time. `chat`: dct
sends nothing and says so once (dco's own conversation, which calls `dct ask`,
handles the sentence). `agent`, or an old dco with no `converse` block: it
sends as before. A route that can't be read, or has an unknown value: it sends
as before and says so once; the cost is that words meant for chat can land in
an agent's terminal if the route is unreadable.

**`~/.dct/voice.toml`** (optional):

```toml
log_text = false          # true: also keep the words in voice.log
[alias]
章鱼 = ["张瑜", "张宇"]   # the right word = what the recogniser often writes instead
```

Without a file the alias table is the built-in one for 章鱼. A wrong type for
`log_text` is an error, never silently "on" or "off".

**`~/.dct/voice.log`** (mode 0600, one JSON per line) holds metadata only by
default: `t`, `trigger`, `session` (the number, not the name), `chars_raw`,
`chars_fixed`, `confidence`, `route`, `action`, `reason`. With `log_text =
true` it also has `text_raw` and `text_fixed`, **except** for sentences that
were skipped because the route was `chat`, which never carry text. Lines
written by an older build may contain your words; this build does not clean
them up.

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

The title line ends with the version that's running, and so does the settings
page — it's the first thing anyone helping you will ask.

If dco is installed, a small read-only label just left of the version shows what
it's doing (`dco idle`, `dco needs you`, `dco paused`, `dco halted`, `dco missing permission`, `dco screen locked` …). It's refreshed every few seconds in
the background, hidden when dco isn't installed, and drops out before anything
else on a narrow screen.

### Picking a project

`p` opens a list of the projects you've used, most recent first, with a search
box above it: start typing and the list narrows. Below the list are three rows
that never scroll away, however long it gets:

- **browse folders** — walk the disk from where you are. The top row goes up a
  level, and so does `←`; going up puts the cursor on the folder you just came
  out of, not back at the top. A "use this folder" row picks where you're
  standing.
- **new project** — its name starts as whatever you typed in the search box,
  because not finding a project is exactly when you want to make one.
- **type a path** — paste a path you already have.

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
stopped sessions included) and `F8` (also `Shift+F3` / `Ctrl+F3`, but `Shift+F3` was reported not working in the user's terminal and `Ctrl+F3` is grabbed by macOS; see below) does the same as `←`, `Enter` zooms into the focused tile, `g` goes back to
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
to the board, `F3` jumps straight to the next running session, `F8` jumps to
the previous one (so do `Shift+F3` and `Ctrl+F3`, but see below), `F4` toggles copy
mode, `F5` pastes an image, `F6` opens the colour picker. The bottom-left hint
(`F2 back`) is always there — a disconnect, an error, or a long message can't
push it off the line, because it's the only way out of a session.

Next/previous follow the board's order (project, then id), wrap around at the
ends and skip stopped sessions. The previous key is not on the hint bar (there
is no room); `Alt+F3` still means next.

**`F8` is the reliable previous key.** `Ctrl+F3` is claimed by macOS (the Dock)
before it reaches the terminal. `Shift+F3` is in the code and covered by unit
tests, but **it has been reported not to work in at least one real terminal,
and why is still being diagnosed**: don't count on it. Some terminals also
strip modifiers from function keys, and then `F3` simply means next. To see
what your terminal sends, run `cat -v` and press the key: `^[[1;2R` is
`Shift+F3`, `^[OR` or `^[[13~` is plain `F3`, `^[[19~` is `F8`. (Before this
change any modifier + `F3` meant next; now `Shift`/`Ctrl` + `F3` means
previous.)

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

## A failure, in one sentence

When a session fails, the bottom bar says why in words you can act on, instead
of leaving you to read a screen of someone else's error output. It asks the same
`[llm]` model the names come from, and only ever sends **the end of the failed
screen** — that's where the error is.

**This is off until you write an `[llm]` section yourself.** It isn't a default
anyone else can switch on for you, because the end of a failed screen is exactly
where `Invalid API key: sk-…`, `Authorization: Bearer …` and the contents of a
`.env` tend to appear. DC's pairing screen offers to add the section for you, as
a box you untick with `l`; it adds to your `config.toml` without rewriting it,
and leaves an existing `[llm]` alone.

An `[llm]` section has to say which backend to use: **`provider` has no default**,
and dct never picks a vendor for you. Without it, the feature stays off and you
get one line telling you what to add. `provider` is the name of a profile (a
built-in one, or your own in `~/.dct/profiles/`). `transport = "cli"` (the
default) runs that profile's own headless command. For any OpenAI-compatible
server, write a profile with an `[api]` section (here
`~/.dct/profiles/gateway.toml`) and connect directly. The address below is only
an example, not a real endpoint:

```toml
# ~/.dct/profiles/gateway.toml
name    = "gateway"
command = ["true"]                     # not an interactive agent; only [api] is used
[api]
base_url = "http://127.0.0.1:11435"
wire     = "openai"                    # "openai" = /v1/chat/completions
```

```toml
# ~/.dct/config.toml
[llm]
provider  = "gateway"                  # the profile name above
transport = "http"
model     = "<model name>"             # required for http; dct will not guess one
# base_url = "..."                     # optional; overrides the profile's [api] base_url
```

`dct llm check` makes one real call through that connection and says whether it
worked, so you find out now rather than on the next failure.

---

## On your phone

Settings has a "use your phone" switch. Turn it on and dct prints a QR code in
the terminal; scan it with a phone **on the same Wi-Fi** and you get your
sessions, each one's live screen, and a line to type into.

It is a page served by the daemon on your own network. Nothing goes to a server
— there isn't one — so this works with no internet at all, and stops working the
moment you leave the house. Out of the house, [Telegram](#telegram-tells-you-when-its-done)
reaches you instead. Opening this page itself from anywhere goes through a relay,
which is designed and partly written but not switched on: see
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

## Telegram tells you when it's done

The phone page needs the same Wi-Fi; this doesn't — though it does need Telegram,
which mainland China can't reach without a VPN. Make a bot with Telegram's
BotFather, then in settings open **phone notifications** and paste the token it
gave you. Send the bot any message and you're paired.

- **A message arrives when a session stops, fails, or dies**, with its name and
  project. Several at once are batched into one message rather than buzzing your
  pocket for each.
- **Reply to a message** and what you type goes into that session — no picking,
  no guessing. With `[llm]` set, a session waiting on a choice arrives with the
  choices already listed.
- A message that isn't a reply goes to the one session that's waiting. If
  several are waiting, the bot asks which rather than guessing: text typed into
  the wrong agent costs far more than one more question. `/ls` lists the
  sessions, `/use <n>` points your next messages at one of them.
- **The first person to message the bot after you paste the token becomes its
  owner, and everyone else is ignored for good.** Bot names are public and anyone
  can message one, so this is the line between "my phone" and "a stranger typing
  into my terminal". That owner is saved, so a restart doesn't reopen pairing;
  "re-pair" in settings does.
- Only text dct has already written for you leaves the machine — the name, the
  project, the choices. The raw screen never does.

## Live to a room

Press `L` in a session for the live panel: stage a few sessions, get a link and a
QR code, hand it out. Students open it in a browser and **can only watch**.

It needs a relay, and **dct ships without one**: set `DCT_RELAY` to the address of
a relay you run (`dct-srv` in this repo; setup in
[`docs/deploy-live-relay.md`](docs/deploy-live-relay.md)) before starting dct.
Without it, starting a broadcast says so instead of handing you a link that goes
nowhere. Where your screen travels is your call, not a default baked into the
binary.

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
- **Publish to a fixed address**: press `p` in the panel and give it a title; the session appears on the
  relay's public list, watchable by anyone without a link. The first time you'll be asked for the public
  live key your operator issued (`K` changes it later). Press `p` again to unpublish; private links keep
  working. While public, the bar reads "● LIVE PUBLICLY · <title>".
- Changing what's staged keeps the link; `r` is what mints a new one and kills the
  old immediately. After you stop, the student page says the session has ended.
- The QR code on the panel encodes the **full link, token and all** — students
  have to scan it. So don't project the live panel itself.
- Before putting the relay on the public internet, read
  [`docs/deploy-live-relay.md`](docs/deploy-live-relay.md): the reverse proxy must
  allow **`/live/*` and the public listing page `/`, and nothing else**. To enable
  "publish to a fixed address," the operator also issues and installs a publish
  key with `dct-srv key add`/`--publish-keys`.

## Let the AI play a match-3 game (Mac)

Open a Candy Crush level in iPhone Mirroring, then tell the AI in a dct session: "play a Candy Crush level for me".
dct reads the board, picks moves by its rules and has dco swipe them, under a second per move. For every move, the board,
every move that was possible and the choice are recorded in `~/.dct/games/log/`.
You can also run it yourself: `dct game play [--steps 20] [--dry-run]`. Where the board is on screen is written in `~/.dct/games/candy-crush.toml` (without that file, the built-in one is used).

Add `--auto-next` and, when a level is lost, it presses "Try again" and "Play" by itself to start over. For popups it only closes the safe ones, such as "Close", "Not now" and "No thanks".
It stops and presses nothing when it meets something that costs money, an ad, no lives left, a cleared level or a screen it does not know (`--tries` limits how many times it starts over, 5 by default; each try uses up one life).
After a win it does not go on to the next level by itself: the board sits in a different place on the next level, so that has to wait until "recognise a new board automatically" is built.

## Post to a social platform (Mac, early)

**This branch only, and it carries application-specific content that [the
boundary](#what-dct-is) says dct shouldn't hold.**

**Nothing here has run on a real screen yet.** Everything below is tested
against a fake dco; the first pilots (X, then TikTok) need you there.

**1. Approve the text once.**
`dct campaign approve launch --param x.post_text="…" --param tiktok.video=/path/a.mp4`
shows every platform's text in the terminal and saves it only when you type
`y` and then confirm with Touch ID (`~/.dct/campaigns/launch.json`). The
fingerprint signs the exact texts, so editing the file afterwards (or a file
from an older version, which has no signature) makes `dct do` refuse with exit 2
until you approve again. Each platform's text, and its opening
sentence, must differ from the others. `dct do` posts nothing else.

**2. Post it.** `dct do "post the launch text" --platform x --campaign launch`,
with dco running and the platform's browser open. Several browser windows open →
it lists them and you pass `--window <id>`.

- If dcv holds a confirmed procedure `promote-<platform>-<post|video>`, dco
  replays it. When dco can't find a button, dct answers for it the same way as
  below. When the procedure itself no longer works, dct carries on from that step
  and writes what it finds as a new draft; the confirmed one is left alone.
  `--preview` stops before the publish step.
- Otherwise dct works the page out: it opens the address in the platform table,
  turns the screen into a multiple-choice question, and asks Jeff (counted only
  above the threshold) → the `[llm]` model (text only, at most twice per screen)
  → you, with a numbered picture.
- **It stops and hands over** on login, QR-code, captcha, phone-verification and
  account-warning screens; that text goes to no model. It never taps follow,
  subscribe, promote, upgrade or delete, nothing that moves money or can't be
  undone, and nothing on the platform's own list. Limits: 40 steps, 10 minutes, 3 unchanged screens
  in a row.
- **It asks you before every publish tap.** Where a platform's publish button
  hasn't been checked on a real screen (all the Chinese platforms and
  xiaohongshu), it asks before every tap. On `wechat_mp` you always tap publish
  yourself.
- After a post goes out, dct writes the steps as a draft procedure (and rules
  for the popups it closed) into dcv, never approved by itself. If dcv refuses
  it, the `[llm]` model gets up to 3 tries to fix it. Then a card shows every
  line; yes → Touch ID, and the approval only holds for the exact text the card
  showed.
- Exit codes: 0 posted or previewed, 1 something broke (dco, dcv, the
  approval), 2 a wrong command, config, campaign or window, 3 stopped or you
  said no.
  `--json` prints one event per line (`ask`, `confirm_publish`, `card`, `note`,
  then `done` / `previewed` / `stopped` / `declined` / `error` with `exit`) and
  reads `{"key": "e3"}`, `{"stop": true}` or `{"verdict": "yes"}` from stdin.

**What it keeps** (all 0600, and the post text is written as `{post_text}`,
never the words): each run's steps in `~/.dct/explore/runs/<run>/trace.json`
with the pictures it showed you (the trace also records how the run ended),
and one line per answered question in `~/.dct/explore/answers.jsonl`, saved to
train Jeff later. The lines are written when the run ends; each says who
answered (`by`: user / model / jeff), which options Jeff was shown (`shown`),
which trace step it led to (`step`), how the run ended, whether the step was
confirmed to move closer to the goal (`outcome`: good / bad / unknown) and
whether it counts as a training label (`label`: user answers, and model
answers with outcome good; never Jeff's own).

**The platform table** has 16 built-in rows: x, tiktok, youtube, facebook,
instagram, xiaohongshu, weibo, bilibili, zhihu, zhihu_article, douyin, kuaishou,
wechat_channels, toutiao, wechat_mp, baijiahao. Fields nobody has checked on a
real screen are listed in each row's `unverified`; for the Chinese rows that is
nearly everything. Fix or add rows in `~/.dct/platforms.toml`; a row with the
same name replaces the built-in one.

**Not done yet:** the real-screen pilots; a stronger model for the middle step
(waiting for its address); dcv and dco changes it depends on, built in their own
repositories; and a
publish that takes two taps (Post, then a confirm dialog) counts as posted only
once the new screen has no publish button left or shows a verified "posted" text.
On X that text is still unverified and the home page keeps a Post button, so a
real X run stays pending after the first tap until the text is verified on a real
machine.

## Several computers

A Mac at home, a Windows box at work, a few agents on each: let them leave each
other messages. A session on one computer can drop a line into a session on
another, typed straight in if that session is idle.

```
dct login                       # on an existing computer: sign in with the DC account
dct invite                      # on an existing computer: show a 6-digit invite code (or press a on the board)
dct join 482913                 # on the new one: type that code (signs in first if needed)
dct peers                       # who is in the group, what each has open
dct send Mac/#3 "run the Windows tests"
```

- **Sign in**: `dct login` trades the paired DC account for a relay token, kept on
  that computer only. The first computer to sign in creates the group.
- **Adding a computer takes one code**: press `a` on the board of a computer that is
  already in (or run `dct invite`); it shows `Invite code 482 913 · valid for 10 minutes`.
  On the new computer run `dct join 482913` and it is in — the old one's board says
  "<name> has joined". Spaces, dashes and full-width digits in the code are fine. The
  new computer must be paired with the **same** DC account: computers on other accounts
  cannot even reach yours.
- **A code works once**: used once (right or wrong), 10 minutes old, or replaced by
  pressing `a` again — then it is gone. If someone tried a wrong code, the old
  computer says so; press `a` for a new one. Restarting the daemon also drops the code.
- **The relay can't guess it**: the code is the password of a PAKE (SPAKE2), so the
  relay sees every message and still learns nothing; impersonating a computer is a
  one-in-a-million shot per code.
- **Whoever sees the code can join** (if they also hold your DC account): mind screen
  sharing and screenshots. The code is the approval; the old computer does not ask again.
- **Names**: a computer is called Mac / Windows / Linux by default; a clash in the
  group is numbered by the old computer ("Mac 2"), even for a name set with `--name` —
  the new computer tells you what it ended up as. To pick your own:
  `dct join 482913 --name work-pc`.
- **Messages**: `dct send <computer>/<session> "<text>"`, the session can be `#id`.
  The other side sees a marker line (`[来自 home-mac/docs 的留言 #a1b2]`) and then
  the text. Idle session: typed now. Busy: queued, one at a time.
- **The relay can't read them**: sealed on your computer for the one recipient;
  the relay forwards, never writes to disk, and sees ciphertext only.
- The relay defaults to `https://dataclue.cn/dct-relay`; override with
  `[mesh] relay = "…"` in `~/.dct/config.toml`.

**Not yet:**

- the other computer offline → not sent (you are told); nothing is held for later;
- only agent sessions receive, not plain terminal sessions;
- queued messages and a live invite code are lost when the daemon restarts;
- two old computers each adding a newcomer at once can fork the group list, and an
  offline computer misses list changes;
- any computer on your account can keep burning your codes with wrong guesses
  (it cannot get in, but it is annoying); this version does not stop that;
- you can't open another computer's session yet — that's the next step.

### Deploying

Both the gateway and the relay have to be on. **This is the go-live checklist;
running it is going live, so check with whoever owns the servers first.**

1. **Gateway** (dc_llm): implements `POST /admin/api/relay/token` per the design
   appendix (done). Turn the flag on — its real name in the gateway is
   **`DC_ADMIN_RELAY_TOKENS_ENABLED`** (the design doc writes
   `DC_RELAY_TOKENS_ENABLED`; same thing) — with the signing key in
   `DC_ADMIN_RELAY_SIGNING_KEY`. `python -m scripts.relay_signing_key show`
   prints the public key, one base64 line.
2. **Reverse proxy**: add `/dct-relay` on dataclue.cn with the existing TLS cert.
   Pass only `/dct-relay/link/*`, prefix stripped, to the relay below —
   `--with-link` also mounts `/phone`, which this doesn't need; keep it closed.
3. **Relay**: put the public key in `/etc/dct-srv/gateway.pub` (one key per line;
   to rotate, add the new line first, wait for it to take effect, then switch the
   gateway's private key) and run

   ```
   dct-srv 127.0.0.1:<port> --with-link --relay-keys /etc/dct-srv/gateway.pub
   ```

   The address is the **first argument** — there is no `serve` subcommand and no
   `--addr`. The relay still listens on loopback only; public traffic comes in
   through the proxy.
4. **Check**: two real computers (a Mac and a Windows) go through `dct login` →
   `dct invite` → `dct join <code>` → `dct send`, and the message shows up.
5. **Check multi-line messages in real agents** (not verified yet): on the Windows
   box, `dct send` a two-line message into a Claude Code session and into a Codex
   session. Each must arrive as **one** turn — the marker line and both body lines
   together. If an agent submits at the first line break, the marker goes in alone
   and the body arrives as an unmarked turn the agent will take as the user's own
   words: stop the go-live and wrap the typed text in bracketed paste
   (`ESC[200~ … ESC[201~`) for that agent first. Record the result in the ledger.

To try the whole thing on one machine, no gateway, no internet:
`cargo test --test mesh_e2e -- --ignored --nocapture` starts a real relay and two
separate daemons, signs test tokens with `dct-srv token keygen`/`token mint` in
place of the gateway, walks login → invite → join → peers → send, and checks the
relay's output never contains the message text.

## dct in a browser

Everything above assumes a machine that meets [the hardware
table](#install). For the people it doesn't — an old laptop, a locked-down work
computer, a Chromebook, a tablet — [`container/`](container/README.md) runs the
whole board on a server, and they open a link. Closing the browser leaves the
sessions running, same as closing a terminal.

- **`dct gate` is the door.** The link carries its key, trades it for a 30-day
  cookie on first open, and wipes it from the address bar. Anything arriving
  without the key gets the same refusal whatever path it asked for. It does not
  encrypt, so it binds to loopback unless you say `--bind`, and **must sit
  behind TLS** before anyone else can reach it.
- **Beside the terminal is a panel for what a terminal can't carry:** paste or
  upload images (preview, add a note, then insert them into the session), upload
  files into the project's `uploads/` folder where the agent can read them,
  download a file or the whole project as a ZIP. Behind the classroom server
  there's also a microphone button that turns what you say into text in the
  input line. Ending a practice never
  deletes the code.
- **For a class**, [`container/classroom/`](container/classroom/README.md) is the
  teacher's console: add students by name, hand each one their own link, watch
  a student's terminal read-only or step in to help, and put a student's
  workspace live — publicly, if you choose.

Setup, the environment variables, and what gets worse in a container are all in
[`container/README.md`](container/README.md).

---

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

## Config reference

Everything lives under `~/.dct/` (see [Files](#files-under-dct)). A config that
can't be read or parsed never stops dct from starting; dct falls back to the
defaults below (every optional feature off) and prints one line on stderr.

**`~/.dct/config.toml`**

| Key | Default | What it does |
|---|---|---|
| `[llm]` `provider` | none (feature off) | profile name that serves the model; no vendor is ever chosen for you |
| `[llm]` `transport` | `"cli"` | `"cli"` or `"http"` |
| `[llm]` `model` | none | required for `http` |
| `[llm]` `base_url` | the profile's `[api].base_url` | override |
| `[judge]` `backend` | `"jev"` | the only value accepted |
| `[judge]` `base_url` | none | address of `jeff-serve`; without it Jeff is never asked (a `[judge]` with only `gate_tiers` / `model_veto` is fine) |
| `[judge]` `model` | `"jeff-latest"` | the model name `jeff-serve` answers to |
| `[judge]` `api_key_env` | none | name of an environment variable holding a key; sent as Bearer only to the `base_url` host; plain `http://` only to this machine |
| `[judge]` `threshold` | `0.8` | Jeff's answer counts only at or above this confidence |
| `[judge]` `timeout_ms` | `5000` | per request |
| `[judge]` `gate_tiers` | `["critical", "money"]` | which tiers `dct answer` stops; may add `"content"`; `critical` and `money` can't be removed |
| `[judge]` `model_veto` | on when `[llm]` is set | `false` turns the model veto off |
| `[judge.serve]` `command` | none | argv array (no shell) that starts the judge server; without it dct never starts or stops anything |
| `[judge.serve]` `env` | `{}` | extra environment, strings only |
| `[judge.serve]` `ready_timeout_secs` | `60` | how long to wait for `GET <base_url>/health` |
| `[judge.serve]` `autostart` | `false` | `true`: `dct answer` starts the server once when it is unreachable |
| `[judge.serve]` `min_free_percent` | `15` | refuse to start below this much free memory |
| `[ask]` `enabled` | `false` | `dct ask` works only when `true`; `dct ask --enable` writes it |
| `[ask]` `allowed_hosts` | `["dc-llm.tzspace.cn"]` | the only hosts `dct ask` will send to |
| `[menu]` `agents` | `[]` (show all) | which agent profiles the picker shows, in this order |
| `[mesh]` `relay` | `https://dataclue.cn/dct-relay` | the relay for several computers |

**`~/.dct/voice.toml`** (optional): `log_text` (default `false`), and an
`[alias]` table of `right word = ["what the recogniser writes", …]`.

**`~/.dct/profiles/*.toml`**: one agent per file. Keys read by dct: `name`,
`command`, `is_agent`, `pairable`, `idle_pattern`, `busy_pattern`,
`error_pattern`, `env`, `secret`, `install`, `login`, `headless`, `api`,
`label`, `note`, `resume_args`, `backend_only`. See [Your own
agents](#ten-agents-one-door).

**`~/.dct/platforms.toml`** (optional, for `dct do`): rows that replace or add to
the built-in platform table. **`~/.dct/games/candy-crush.toml`** (optional): where
the board is on screen for `dct game`.

**Environment variables** used by dct itself: `DCT_LANG` (`en` or `zh`; beats
the saved choice and the system locale), `NO_COLOR`, `DCT_RELAY` (the live relay; no default), `DCT_RELEASE_BASE`,
`DCT_INSTALL_DIR`, `DCT_MINGIT_URL`, `DCT_NODE_BASE`, `DCT_NPM_REGISTRY` (see
[Install](#install)).

---

## Files under `~/.dct`

All paths are derived from the daemon socket's folder, which is why tests can
move them to a temporary directory without touching yours.

| File | What it is |
|---|---|
| `daemon.sock` | the daemon's local socket (owner only) |
| `config.toml` | the config above; appended to (never rewritten) by pairing and `dct ask --enable` |
| `secrets.toml` | API keys and the `dct gate` key; **mode 0600**; never inside a profile file |
| `settings.json` | language, view mode, bar colour |
| `projects.json` | recent projects and each one's last agent |
| `last-sessions.toml` | what was running, so a restarted daemon can offer it back |
| `sessions.log` | how each session started and how it ended |
| `pair-models.toml` | the model names pairing wrote |
| `profiles/` | your own agent profiles |
| `runtime/` | dct's own Node and portable git, for dct's use only |
| `ask.log` | one **metadata** line per `dct ask` (mode 0600); no text |
| `voice.toml`, `voice.log` | voice settings; the voice log (mode 0600), metadata only by default |
| `judge-serve.pid`, `judge-serve.log` | the pid record of the judge server dct started, and its log (mode 0600, replaced on each start); `judge-serve.lock` and `judge-serve.err` are also written next to them |
| `dco-rules.json` | written verbatim from `dco rules --json` for the dc-vault rules page |
| `explore/answers.jsonl` | training rows from `dct do` and `dct train record` (mode 0600) |
| `explore/runs/<run>/trace.json` | each `dct do` run's steps and the pictures it showed you |
| `campaigns/<name>.json` | approved campaign texts with their fingerprint |
| `platforms.toml` | your platform rows for `dct do` |
| `games/log/<date>.jsonl` | one line per `dct game` move |

---

## Security and privacy

- **What was heard is not authorization.** `dct voice` sends nothing unless a
  click on the dco octopus triggered it, never answers an approval prompt and
  never sends a bare "yes". The `source` field of `dct ask` is a hint to the
  model and nothing else.
- **No transcripts by default.** `voice.log` and `ask.log` keep counts, timings
  and decisions, not words (`voice.toml` `log_text = true` is the only way to
  put words in `voice.log`). `dct ask` memory is in the daemon's RAM and is
  never written. Lines an older build wrote to `voice.log` may still contain
  words.
- **Off until you turn it on.** No `[llm]`, no model call. No `[ask]
  enabled = true`, no chat. Consent can't be given by another program:
  `dct ask --enable` and `dct campaign approve` refuse when they aren't on a
  terminal.
- **Only your own gateway.** `dct ask` sends only to hosts in `[ask]
  allowed_hosts` (default `dc-llm.tzspace.cn`), over the `http` transport; a
  vendor CLI as `[llm]` provider is refused for `ask`. A login borrowed from
  Claude Code or Codex is never sent anywhere but that vendor's own hosts.
- **What a failure explanation sends.** With `[llm]` set, the end of a failed
  session's screen (the last 2 000 characters) goes to your model. That is
  where keys and tokens tend to appear, which is why `[llm]` is opt-in.
- **Secrets.** `secrets.toml` is mode 0600 and profile files never hold keys. Don't
  put a secret in `[judge.serve]` `command` or `env`: any user on the machine
  can read them with `ps`. `[judge] api_key_env` names a variable; it never
  holds the key.
- **The danger gate is rules.** It stops money and permanent deletion by word
  list, not by confidence, but only the words `dct-brain` knows
  ([limits](#the-danger-gate-dct-answer)). Callers should still filter options
  they know are destructive.
- **Undo has a boundary.** Snapshots cover the project directory; an agent
  that writes outside it can't be undone ([Things that will
  annoy you](#things-that-will-annoy-you)).
- The phone page, the live link and the browser door each have their own
  token rules, described in their sections.

---

## Jeff's exam and dev sets

Training and evaluating Jeff is held to one rule: **the exam is never used to
choose anything.** Files in this branch:

- `tests/fixtures/jeff-exam/exam-v3.jsonl` (364 items) with `manifest-v3.json`.
  **Frozen** on 2026-10-14 by the owner and held out: never train, develop,
  calibrate or tune a threshold on it; evaluate once per candidate at the
  threshold already chosen; when exporting training rows, drop any whose
  `dedup_sha256` equals an exam item. A changed byte is a new exam version
  that needs the owner's agreement, and old and new scores are reported
  separately. `exam-v2.jsonl` and `manifest.json` stay as the historical
  record.
- `tests/fixtures/jeff-dev/` (`dev-v3.jsonl`, `dev-v4.jsonl`, their manifests
  and scenario files): the set a threshold is **chosen** on. The v3 manifest
  records `0.98` as the provisional threshold chosen on `dev-v4`; the code's
  default `[judge] threshold` is still `0.8`.
- `scripts/jeff-exam/`: `build_exam.py`, `check_overlap.py`, `diff_danger.py`,
  `exam_lib.py`, `danger_allowlist.json`, and `run_exam.py`, which asks a live
  `jeff-serve` every item (`python3 -I scripts/jeff-exam/run_exam.py --label
  v1.2 --out DIR [--set exam|exam-v3|dev|dev-v4] [--url …] [--seeds 5]
  [--threshold 0.8]`) and refuses to run when a set no longer matches its
  manifest.

Jeff v1.2 (`jeff-legacy/Jeff-Qwen3.5-0.8B`, revision `d86f657`) is the model in
use. The training design (`docs/superpowers/specs/2026-10-11-dct-jeff-auto-training-design.md`)
keeps it until a retrained one is approved by the owner. `docs/HANDOFF.md` says
that when it was written only the training-row format (`dct train record`,
`explore/answers.jsonl`) was built; the rest of that design has not been checked
against the code.

---

## Troubleshooting

- **Both builds say `dct 0.2.24`.** The version number can't tell `origin/main`
  from this branch. If `dct --help` has no `dct ask`, `dct answer`, `dct judge`
  or `dct do`, you are running the published build.
- **A daemon that is "stale".** Tell-tales: the board asks whether to restart
  the daemon, `dct status` says the daemon is stale, `dct ask` exits 10 with
  `reason: "protocol"`. The daemon was started from a build with a different
  socket protocol. `dct restart` swaps it, and **ends every running session**
  (file changes stay, the agents don't); `dct restart -y` skips the question.
  After rebuilding with the same version number nothing notices the difference,
  so run `dct restart` yourself.
- **`Shift+F3` doesn't go to the previous session.** Use **`F8`**, which is
  the reliable one. `Shift+F3` is implemented and unit-tested, but one user has
  reported it does nothing in their terminal. `Ctrl+F3` is taken by macOS. See
  the next entry for the suspected cause and [The board](#the-board) for how to
  see what your terminal sends.
- **Known issue: the `Shift+F3` previous-session key may never arrive as a key
  event.** *Diagnosis from reading code, not yet confirmed on a live terminal.*
  `dct` reads input through crossterm 0.28.1, which treats any `CSI … R` as a
  cursor-position reply and drops it, and xterm-style terminals (and tmux with
  `xterm-keys` on, `extended-keys` off) send `Shift+F3` as `ESC[1;2R`. `Ctrl+F3`
  (`ESC[1;5R`) is dropped the same way. The unit tests build `KeyEvent`s
  directly, so they don't cover decoding. Workaround: use `F8`. To check what
  your terminal sends, run `cat -v` and press the key (`^[[1;2R` confirms it).
- **`zsh: killed` right after copying a new `dct` over the old one.** Don't
  `cp` over an installed binary; see the **Don't `cp` over an installed binary** paragraph under [Install](#install).
- **`dct voice` says dco is missing or too old.** Start dco, or update it; the
  command needs dco's `hear` tool.
- **`dct judge up` refuses.** `[judge] base_url` must be `127.0.0.1`,
  `localhost` or `::1`; free memory must be above `min_free_percent`; `down`
  only stops a server dct started itself.
- **`dct answer` exits 1.** The config couldn't be read, or `[judge] gate_tiers`
  names an unknown tier or leaves out `critical` or `money`. The reason is on
  stderr.
- **A pty test fails once and passes on a rerun.** Tests that open real
  pseudo-terminals have failed intermittently when the suite runs in parallel,
  and the cause has not been pinned down. Run them one at a time:
  `cargo test -- --test-threads=1`.

---

## More documentation

- [`docs/HANDOFF.md`](docs/HANDOFF.md): where the explore-and-learn branch
  stands, and research notes on a local decision model. Read it first in a new
  session on this repo.
- [`docs/2026-10-16-dct-ask-design.md`](docs/2026-10-16-dct-ask-design.md): `dct ask`, with the
  measurements and the honest "not done" list.
- [`docs/2026-10-03-dct-dco-dcv-distributed-agent-runtime.md`](docs/2026-10-03-dct-dco-dcv-distributed-agent-runtime.md):
  the earlier architecture note on how dct, dco and dcv relate.
- [`docs/superpowers/specs/`](docs/superpowers/specs/): one design per feature (judge, brain,
  control layer, pairing, multi-machine, invite code, voice relay, match-3,
  Jeff auto-training, …), and [`docs/superpowers/plans/`](docs/superpowers/plans/) next to it.
- [`docs/deploy-live-relay.md`](docs/deploy-live-relay.md): putting the live relay on the public internet.
- [`container/README.md`](container/README.md): dct in a browser.
- [`docs/2026-09-04-gateway-findings.md`](docs/2026-09-04-gateway-findings.md): what was learned about the gateway.

---

<details>
<summary><b>Where this is going</b> — what isn't written yet</summary>

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
- **More phone channels.** [Telegram is done](#telegram-tells-you-when-its-done)
  — it went first because it's the only one that doesn't need a public callback
  address. Feishu, WeCom and SMS aren't. Nor is falling back: if the primary
  channel fails to send, the next one should carry the message and say so.
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

## Development

```sh
cargo build --release          # ./target/release/dct
cargo install --path . --root ~/.local   # puts the binary where scripts/install.sh does
cargo test -- --test-threads=1 # see Troubleshooting for why one at a time
cargo clippy --all-targets
cargo fmt --check
```

The repository is a Cargo workspace; the root package is `dct` itself and
`crates/` holds `dct-brain`, `dct-dco`, `dct-explore`, `dct-game`, `dct-link`,
`dct-mesh`, `dct-page` and `dct-srv`. Tests make real git repos, spawn real
processes and bind real sockets, but nothing touches your own `~/.dct` and
nothing hits the network. Tests that need a real dcv or a real relay are
`#[ignore]`d (for example `cargo test --test mesh_e2e -- --ignored`).
`cargo fmt --check` already fails on files that predate this branch
(`docs/HANDOFF.md` records 55 files, written 2026-10-11 and not re-checked
since), so don't fold a whole-repository format into a feature commit.

When a change touches the daemon socket, bump `PROTOCOL_VERSION` in
`src/proto.rs` (a test pins the wire shape to it); this branch is at 25. After
it changes, every running daemon is stale until `dct restart`.

<details>
<summary><b>For anyone working on the code</b> — the source layout and the house rules</summary>

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
src/last_sessions.rs  what was running, so a restarted daemon can offer it back
src/journal.rs     how each session started and how it ended (asked to stop, or vanished)
src/proto.rs       the wire contract
src/config.rs      ~/.dct/config.toml; [llm] is off unless written
src/llm/           the [llm] connection: CLI or HTTP, and which credential may go where
src/judge/         dct judge: Jev choice questions over HTTP to jeff-serve; confidence recomputed here;
                   serve.rs starts and stops the local judge server (dct judge up / down / status)
src/answer.rs      dct answer: the ladder, the request/answer JSON, the rule-based danger gate
src/answer_veto.rs the model veto after the gate (it can only add a veto)
src/train_record.rs  dct train record: one v2 training row per answered question
src/ask/           dct ask: input checks, the streaming turn, memory (RAM only), fixed prompt,
                   the daemon-side service, ask.log (metadata only). Never touches sessions
src/voice/         dct voice: gates, routing to a session, the dco voice route, voice.log
src/dco_rules.rs   dct dco-rules and the daemon's watcher that keeps ~/.dct/dco-rules.json fresh
src/mesh/          several computers: login, invite, join, peers, send
src/game/          dct game: match-3 and hidden-object play through dco
src/keys/, src/procedures.rs  dct keys and dct procedure (Touch ID signing keys, approvals, tickets)
src/ui/dco_cell.rs the board's read-only dco label
src/campaign.rs    dct campaign approve: one approval for every platform's text
src/explore_cli*   dct do: window, replay or explore, draft, card, exit codes, --json
src/explore_model.rs  the [llm] step of dct do: pick a key from the list, or fix a refused draft
src/learn.rs       drafts into dcv as import:agent, the confirmation card, Touch ID bound to the body
src/runtime.rs     dct's own Node and git, under ~/.dct/runtime
src/pair*.rs       DC pairing: the state machine, its HTTP, writing the result
src/channel/       phone channels; only Telegram so far. Knows nothing of sessions
src/bridge.rs      sessions <-> channel: who the owner is, which reply goes where
src/live.rs        pushes staged screens to the relay (DCT_RELAY)
src/gate.rs        dct gate, the key-checking door for the browser version
src/web/           the LAN phone client: a tiny HTTP server and one page
src/link.rs        dials out to a relay and long-polls it (no switch yet)
src/sys/           everything that differs between Unix and Windows
crates/dct-brain/  tiers for the danger gate, execution tickets, signature checks; no disk, no network
crates/dct-mesh/   machine ids, the 6-digit invite code (SPAKE2), key pairs, signed group lists
crates/dct-game/   match-3 move choice (pure), the dco status probe behind `dct status --json`
crates/dct-dco/    the dco client (unix socket MCP), shared by dct game and dct do
crates/dct-explore/  hands, platform table, stop rules, options, the Jeff → model → you ladder,
                   explore, trace and training rows, drafts, replay
crates/dct-link/   the envelope the daemon and the relay share; no Request
crates/dct-page/   the only two web pages: the phone client and the live viewer's
crates/dct-srv/    the relay. Only /live/* and the public listing page / by default (--with-link adds
                   the unauthenticated pairing routes, for local development; --publish-keys turns
                   on publishing). Phase one has no auth and no encryption, and
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

## License

[MIT](LICENSE)

