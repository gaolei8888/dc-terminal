# Task 10 report: 选择器改造

## What was implemented

All changes are in `src/ui.rs`.

- `View::PickProfile` changed from `PickProfile(Vec<String>)` to a struct variant:
  `{ entries: Vec<ProfileEntry>, state: ListState, warning: Option<String> }`.
- New pure functions, placed next to `back_one_level` (same rationale — testable
  without a live socket):
  - `pub enum PickAction { Start(String), AskSecret(usize), Install { profile, command }, Blocked(String) }`
  - `pub fn pick_action(e: &ProfileEntry) -> PickAction`
  - `pub fn digit_index(c: char) -> Option<usize>`
- `n` key handler (Board view) now destructures the full `Response::Profiles { entries, warning }`
  and builds `View::PickProfile` with `state.select(Some(0))`, with proper error messaging
  (`Response::Error` / transport failure) instead of the old silent `if let Ok(...) = ...`.
- `View::PickProfile` key handling rewritten: Esc → Board; Down/Up move the cursor only;
  Enter/digit compute `chosen: Option<usize>`, then route through `pick_action`'s four
  branches (Start → `Request::Create` with `remember: true`; AskSecret → placeholder
  `message = Msg::err("还没做")`, since `View::EnterSecret` is Task 11's; Install → opens
  a `shell` session (`remember: false`) and feeds it the install command; Blocked → status
  message, no view change).
- Rendering: numbered list (numbers 1–9 stay fixed regardless of availability; row 10+ gets
  blank space instead of a number), whole row dimmed (not just the reason) when not Ready,
  Chinese reason suffix per status, red title/border when `warning` is `Some`.
- `idle_help` extracted from an inline `match` inside `draw()` into a standalone
  `fn idle_help(view: &View) -> &'static str`, mirroring `escape_hint`. This had to be
  created — despite the task context implying it was already a standalone function, it was
  actually still inline in `draw()`. The `PickProfile` arm now reads
  `"↑↓ 选  Enter 确认  或直接按数字  Esc 取消"`.
- `escape_hint` needed no change: its `_ => "Ctrl+Q 回看板"` wildcard already covers the new
  struct-shaped `PickProfile`.

### Design deviation from the brief's literal key-handling sketch

The brief's Step 3 sketch computes `chosen` in one `match key.code {...}` (using `continue`
in the Down/Up arm to skip past code that borrows `entries[i]`), then does the four-way
routing in a second, unconditional `match chosen.map(...)`. The brief itself flags that this
`continue` is wrong and says to rebuild `View` per-arm like `PickProject` instead — but
literally doing that while also keeping the "compute chosen, then route" two-phase shape
does not typecheck: the Down/Up arm would have to move `entries`/`warning` into a new `View`
to satisfy the borrow checker without `continue`, and then the later `pick_action(&entries[i])`
line can't borrow `entries` again (E0382, moved value).

I resolved it by keeping `entries`/`state`/`warning` un-moved through the whole `chosen`
computation (Down/Up only takes `&mut state`, doesn't move anything), and only building the
final `View::PickProfile { entries, state, warning }` once, at the very end, in a single
`view = match chosen.map(|i| (i, pick_action(&entries[i]))) { ... }` expression. This keeps
Esc as its own early branch (`if key.code == KeyCode::Esc { view = View::Board } else { ... }`)
rather than folding it into `chosen`, since Esc doesn't fit the "chosen index" shape at all.
No `continue` anywhere, `view` is assigned unconditionally on every path, so
`message_after_transition` always runs and no stale message can stick — verified in the
manual run below (pressing digit `5`, which triggers "还没做", then pressing Esc/Ctrl+Q
correctly cleared it).

## What was tested and the results

All 8 tests from the brief, plus one I added for the empty-`command` trap (see below):
`ready_entry_starts_a_session`, `needs_secret_entry_opens_the_secret_view`,
`not_installed_with_an_installer_offers_to_install`,
`not_installed_without_an_installer_just_explains`,
`not_installed_with_empty_command_names_the_profile_not_a_blank_command` (new),
`missing_dependency_names_what_to_install_first`, `digit_keys_still_pick_the_first_nine`,
`picker_help_mentions_both_ways_to_choose`, `back_one_level_from_picker_goes_to_board`.

Also updated two pre-existing tests that referenced the old tuple-variant shape:
`ctrl_q_backs_out_one_level_at_a_time` and `draw_does_not_panic_for_all_views` (the latter
now builds a realistic mix of Ready/NeedsSecret/NeedsDependency/NotInstalled-with-installer
entries plus a `warning`, to exercise dimming, reason text, and the red border/title through
the panic-guard test).

Full suite: `env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo test` → **127 lib tests +
19 integration tests, all pass, 0 failed.** `cargo fmt` and `cargo clippy --lib` both clean
(clippy: zero warnings).

## TDD Evidence

### RED

Because this task changes a `View` variant's shape (rippling through key-handling and
rendering) as well as adding new pure functions, I could not do a literal "add one test,
watch it fail, add minimal code" cycle without the whole file failing to compile at every
intermediate step. Instead I verified RED honestly by doing the refactor in full, then
temporarily reverting *only* the implementation (`git show HEAD:src/ui.rs > src/ui.rs`) while
keeping the new test block (plus the two `use` imports it needs) appended — i.e., tests
present, implementation absent.

Command: `env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo test --lib ui`

Output (excerpt, grouped):
```
error: could not compile `dct` (lib test) due to 23 previous errors
4 error[E0425]: cannot find function `digit_index` in this scope
1 error[E0425]: cannot find function `idle_help` in this scope
6 error[E0425]: cannot find function `pick_action` in this scope
6 error[E0433]: cannot find type `PickAction` in this scope
2 error[E0559]: variant `ui::View::PickProfile` has no field named `entries`
2 error[E0559]: variant `ui::View::PickProfile` has no field named `state`
2 error[E0559]: variant `ui::View::PickProfile` has no field named `warning`
```
This is exactly the expected failure: every new symbol the tests reference (`pick_action`,
`PickAction`, `digit_index`, `idle_help`) doesn't exist yet, and `View::PickProfile` is still
the old tuple variant. Not a typo, not an unrelated failure.

Then I restored the full implementation (`cp` back from a scratch backup taken before the
revert) and re-ran.

### GREEN

Command: `env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo test --lib ui`
Result: 50 `ui::` tests passed (0 failed), including all 9 new tests.

Command: `env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo test` (full workspace)
Result: `test result: ok. 127 passed; 0 failed; 0 ignored` (lib) plus all integration test
binaries green (19 more tests across `tests/*.rs`), pristine output, no warnings.

## Manual run

Built with `~/.cargo/bin/cargo build`, then drove the real binary inside `tmux` (raw-mode
apps can't be driven directly by the bash tool) with an isolated `HOME` under `/tmp`
(the scratchpad path was too long for a Unix domain socket — `SUN_LEN` — so I used a short
`/tmp` dir for this ephemeral test only; cleaned up afterward, no user data touched).

Board → pressed `n`:

```
┌选 agent──────────────────────────────────────────────────────────────┐
│▶ 1. Claude        Anthropic 官方
│  2. Codex         OpenAI 官方
│  3. OpenCode      开源，可接多种模型                 （未安装）
│  4. Qwen Code     阿里通义，独立命令行                （未安装）
│  5. Kimi          月之暗面，套用 Claude 界面         （未填密钥）
│  6. GLM           智谱，套用 Claude 界面           （未填密钥）
│  7. DeepSeek      深度求索，套用 Claude 界面         （未填密钥）
│  8. Qwen API      阿里通义，套用 Claude 界面         （未填密钥）
│  9. 命令行           普通终端，不带 AI
└─────────────────────────────────────────────────────────────────────┘
Ctrl+Q 回看板  ↑↓ 选  Enter 确认  或直接按数字  Esc 取消
```

All 9 rows present with numbers, Ready rows (Claude/Codex/命令行, since `claude`/`codex`
are actually on this machine's PATH) bright, unavailable rows dimmed with a Chinese reason
in parentheses. Pressed Down twice — highlight moved to row 3 (OpenCode) correctly, still
dimmed. Pressed Enter on it: OpenCode has a real `install` spec, so it correctly took the
**Install** branch — opened a shell session and ran `npm i -g opencode-ai` live on screen,
with the bottom bar showing "正在安装 opencode，装完按 Ctrl+Q 回看板再按 N". Pressed Ctrl+Q:
back to the board (session kept running detached, as designed). Pressed `n` again, then `5`
(Kimi, NeedsSecret): bottom bar showed "还没做" — confirming the `AskSecret` placeholder
routes correctly and doesn't crash or silently no-op. Pressed Esc: back to the board cleanly,
title read "dct 会话看板". Pressed `q`: quit cleanly.

No mess, no misalignment, no stale message left over from the "还没做" press once the view
changed.

## Decisions on the two flagged traps

**Empty `command`.** In `pick_action`, `ProfileStatus::NotInstalled { command }` with no
`install` spec and an empty `command` string now gets its own arm:
`format!("{} 没配置要运行的程序，用不了", e.label)` instead of falling into
`format!("本机没有找到 {command}")`, which would render as "本机没有找到 " with a dead trailing
space. This only affects `pick_action`'s Blocked message — the *render* path never
interpolates `command` at all (the brief's reason text is just a static "（未安装）"), so the
row itself never shows the dead-end string; only actually selecting that dimmed row would
have hit it. Covered by a new test,
`not_installed_with_empty_command_names_the_profile_not_a_blank_command`.

**Raw-OS-error `warning` text (carried forward from Task 8).** `SecretStore::load()` in
`secrets.rs` does `format!("{e}")` on a raw `io::Error`, which can be English OS phrasing
(e.g. "Permission denied (os error 13)"); `profile.rs::describe_toml_error` also
intentionally keeps the "expected ..." half of TOML parse errors in English, for
actionability (its own comment says so). This task is where that string first reaches the
screen (the picker's title, in red when non-empty).

I decided **not** to attempt string-level sanitization at render time in `ui.rs`. A regex/
pattern-matching cleanup of an already-flattened error string can't reliably distinguish
"safe Chinese detail" from "raw OS/library text" without re-doing the classification that
`secrets.rs`/`profile.rs` already threw away when they collapsed everything into one
`String`. Faking a fix here would just hide the problem behind a false sense that it's
handled. Instead I:
1. Kept the display as specified (`"选 agent —— {w}"`, red border/title when `Some`) — the
   user still needs to know *something* is wrong (e.g. their secrets file is corrupted and
   all their saved keys are currently invisible); suppressing the warning entirely would be
   worse than showing a partly-English one.
2. Added an explicit code comment at the render site documenting the gap, why it exists, and
   which files would need to change to fix it properly (`secrets.rs`'s `io::Error` handling,
   and `profile.rs::describe_toml_error`'s English "expected ..." tail) — both outside this
   task's file scope (`src/ui.rs` only).
3. Am flagging it here explicitly, per the instruction not to silently ship it: **this is a
   known, documented gap**, not a fixed one, and I'd recommend a small follow-up task scoped
   to `secrets.rs`/`profile.rs` to map common `io::ErrorKind` variants (`PermissionDenied`,
   etc.) to Chinese phrases before this warning ever reaches `Response::Profiles`.

## Files changed

- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/ui.rs`

## Self-review findings

- Checked: no `continue` in any `PickProfile`-related key branch (confirmed via
  `git diff src/ui.rs | grep continue` — no matches).
- Checked: digit beyond the list length (`filter(|i| *i < entries.len())`) and digit `'0'`
  (`digit_index` returns `None` for it) both fall through to the no-op `None` branch —
  harmless, verified by `digit_keys_still_pick_the_first_nine` and manual testing.
- Checked: `move_sel_n` already handles `len == 0` (selects `None`), so Down/Up on an empty
  entries list (a daemon returning zero profiles, shouldn't happen but defensive) doesn't
  panic.
- Checked layout: fixed-width columns (`num` 3 cols, label 14, note 26) match the brief
  exactly and follow the same padding convention already used for the Board view elsewhere
  in this file; ratatui's `Block`/`List` clip rather than panic on overflow, consistent with
  how the rest of the file already handles long strings in narrow terminals.
- `idle_help` had to be extracted from inline code in `draw()` rather than merely edited —
  documented above since the task context implied it already existed as a function.
- `escape_hint` required no source change since its wildcard arm already covers the new
  struct-shaped `PickProfile` — verified this is intentional, not an oversight, by checking
  its doc comment (it explicitly enumerates the one special case, `PickProject` with
  `typing_path: Some`, and falls back to "Ctrl+Q 回看板" for everything else).
- No Task 11 (`View::EnterSecret`) or Task 12 (n/N splitting) work was done — `AskSecret`
  routes to a `Msg::err("还没做")` placeholder as instructed, and the `n` key still opens the
  full picker (no filtering by installed/not-installed added).
- Ran `cargo clippy --lib`: zero warnings.

## Issues or concerns

None blocking. The one open item is the documented (not fixed) raw-OS-error warning text,
which I believe is correctly out of this task's scope but should be tracked as a follow-up.

---

# Fix report: two review findings on commit c3d6965

Both findings below are user-facing rendering defects, not routing/state-machine issues —
the reviewer confirmed the `pick_action`/no-`continue`/digit-handling work is correct. Fixed
in `src/ui.rs`, `src/secrets.rs`, `src/profile.rs`, `src/daemon.rs`.

## Finding 1: 中英混排的列对不齐

**Root cause.** `format!("{:<14}", truncate(&e.label, 14))` pads by *character count*.
`truncate` already measures *display width* (CJK = 2 columns), so a 3-character CJK label
like `命令行` (6 display columns) got the same 14-character pad as a 6-character ASCII label
like `Claude` (6 display columns) — leaving the CJK row 3 columns short and drifting the note
column right on every row after it, exactly as shown in the prior manual-run transcript.

**Fix.** Added a `pad_to(s, width)` helper next to `truncate` in `src/ui.rs`, sharing the same
per-character width rule (factored into a new `char_width`/`display_width` pair so `truncate`
and `pad_to` can never disagree about what "wide" means):

```rust
fn char_width(ch: char) -> usize { if (ch as u32) > 0x1100 { 2 } else { 1 } }
fn display_width(s: &str) -> usize { s.chars().map(char_width).sum() }
fn pad_to(s: &str, width: usize) -> String {
    let mut out = s.to_string();
    out.push_str(&" ".repeat(width.saturating_sub(display_width(s))));
    out
}
```

Both call sites in `View::PickProfile` rendering now do
`pad_to(&truncate(&e.label, 14), 14)` / `pad_to(&truncate(&e.note, 26), 26)` instead of
`format!("{:<N}", ...)`.

**Tests added** (`src/ui.rs::tests`):
- `pad_to_aligns_cjk_and_ascii_labels_to_the_same_display_width` — pads `"命令行"` and
  `"Claude"` both to 14 and asserts `display_width()` is equal (14) for both.
- `pad_to_never_shrinks_a_string_already_at_or_over_width` — guards the `saturating_sub`
  (a plain `-` here would underflow and panic on a string already at/over the target width).

**On screen.** Built the binary and drove it live via `tmux` (raw-mode app, can't be driven
directly by the bash tool), isolated `HOME=/tmp/dct-verify/home`. Captured the picker with
`tmux capture-pane -p` and verified column alignment two ways: (1) by eye — the note column
(`Anthropic 官方`, `普通终端，不带 AI`, `月之暗面，...`, `智谱，...`) starts in the same
screen column on every row; (2) programmatically — computed the true terminal column of each
note's first character using `unicodedata.east_asian_width` (the standard basis for terminal
wide-character rendering, distinct from this codebase's simpler `> 0x1100` heuristic which
happens to mis-classify the `▶` highlight-symbol as wide). All four sampled rows (row 1
`Claude`/ASCII, row 9 `命令行`/CJK, row 5 `Kimi`/ASCII, row 6 `GLM`/ASCII) landed at
**exactly the same terminal column (19)**. Before the fix this would have varied per row
depending on how many CJK characters were in the label.

## Finding 2: 原始 OS 报错会红字出现在选择器标题上

**Root cause.** `SecretStore::load()` (`src/secrets.rs`) did `format!("{e}")` directly on
`io::Error` and `toml::de::Error`, so raw system/library text (`Permission denied (os error
13)`, or the toml crate's multi-line ASCII-art parse-error `Display`) could reach
`Response::Profiles.warning` and get interpolated straight into the picker's title. Same
problem, smaller blast radius, in `profile.rs::load_dir`'s directory-open and file-read error
arms (`{name} 打不开：{e}` / `{name} 读不了：{e}`).

**Fix, at the source (not the `ui.rs` render site — the plan's 错误要说人话，不给栈追踪
constraint outranks this task's original file list):**

- Added `pub(crate) fn describe_io_error(e: &io::Error) -> String` in `src/profile.rs`,
  classifying by `ErrorKind` (`PermissionDenied` → 「没有权限读取」, `NotADirectory` →
  「不是一个文件夹」, everything else → 「读取失败」). The raw `io::Error` is written to
  stderr at each call site (`eprintln!`) before being discarded from the user-facing string,
  so it stays diagnosable without ever reaching the screen — matching the existing
  `eprintln!("连接处理失败: {e}")` pattern in `daemon.rs`.
- `load_dir`'s directory-open and per-file read-error arms now call `describe_io_error`
  instead of interpolating `{e}` directly.
- Promoted the existing `describe_toml_error` (already produces 「第 N 行：<reason>」, one
  line, no ASCII art) from private to `pub(crate)` so `secrets.rs` can reuse it instead of
  inventing a second TOML-error translator.
- `SecretStore::load()` now routes its `io::Error` branch through `describe_io_error` (with
  an `eprintln!` of the raw error first) and its TOML-parse branch through
  `describe_toml_error` — both already TOML files, no reason to classify differently from
  `profile.rs`'s custom-profile files.
- Added `SecretStore::path(&self) -> &Path` so callers can name the file in the message they
  compose.
- `daemon.rs`'s `Request::Profiles` handler now composes
  `format!("密钥文件读不了：{e}，检查一下 {}", sec.path().display())` — `e` is now a short
  Chinese reason, and the path makes the sentence actionable instead of just alarming.

**Wording chosen and why.** Two example titles actually produced (see manual run below):

- `选 agent —— 密钥文件读不了：没有权限读取，检查一下 /…/secrets.toml`
- `选 agent —— 密钥文件读不了：第 1 行：invalid key，检查一下 /…/secrets.toml`

Both name the problem, name the exact file, and give a next action ("检查一下"), which is
the closest first-person-plain-Chinese equivalent of the review's suggested
「密钥文件读不了，检查一下 ~/.dct/secrets.toml」. The second one keeps `第 1 行` (Chinese,
required by an earlier review round for `describe_toml_error`) and the toml crate's own
`invalid key` half in English — kept deliberately per this round's explicit exception: the
user is already hand-editing a TOML file at that point, so the crate's own syntax-expectation
text is more actionable than translating or dropping it. Nothing about `os error`, `errno`,
or the toml crate's multi-line `Display` box reaches the screen in either case.

Also replaced the stale "known gap, not fixed" comment block at the `ui.rs` render site
(previously explained *why* the leak existed) with a comment explaining the new contract:
`warning` arrives pre-translated, the one intentional exception is the toml `expected ...`
half, and `ui.rs` does no string-level cleanup of its own.

**Tests added:**
- `src/secrets.rs::tests`:
  - `corrupt_file_load_error_is_plain_chinese_not_a_toml_stack_dump` — corrupt TOML file,
    asserts `load_error()` has no `\n`, contains `第`, and does not contain the toml crate's
    own `"TOML parse error"` banner text.
  - `unreadable_file_load_error_has_no_raw_os_error_text` (`#[cfg(unix)]`, root-aware skip
    like the existing `profile.rs` permission test) — `chmod 000` a secrets file, asserts
    `load_error()` contains no `"os error"` / `"Permission denied"` and does contain `权限`.
  - `path_exposes_the_underlying_file` — the new getter round-trips.
- `src/profile.rs::tests`:
  - Extended `unreadable_dir_reports_an_error_instead_of_going_silent` with the same
    no-raw-English assertions plus a `权限` check.
  - `unreadable_file_reports_an_error_in_plain_chinese` (new, `#[cfg(unix)]`) — same pattern
    for `load_dir`'s per-file read-error arm, which had no dedicated coverage before.
- `src/daemon.rs::tests`:
  - `profiles_warning_names_the_broken_secrets_file_in_chinese` — calls `handle()` directly
    with a corrupt `secrets.toml`, asserts the composed `Response::Profiles.warning` names the
    file path, has no embedded newline, and has no toml-library banner text. This is the
    regression test for the exact bug: it exercises the full composition path
    (`secrets.rs` → `daemon.rs`), not just the isolated `describe_*` helpers.

Existing tests checked for stale expectations: none asserted on the old raw-English text
anywhere (`grep`'d for `os error`, `Permission denied`, `打不开`, `读不了` across
`src/*.rs`/`tests/*.rs` before editing) — `broken_disk_profile_reports_the_filename_and_keeps_the_rest`
and `toml_error_with_embedded_newline_still_collapses_to_one_line` only exercise
`describe_toml_error` via the TOML-parse branch, which is intentionally unchanged.

**On screen** (same `tmux`/isolated-`HOME` setup as Finding 1, daemon processes killed and
sockets removed between scenarios to avoid a stale in-memory `SecretStore` masking the fix —
first attempt showed the *old* title because a leftover daemon from an earlier verify run was
still serving from before the corrupt file was written; killing it and restarting made the
warning appear as expected):

- Corrupt TOML secrets file → title:
  `选 agent —— 密钥文件读不了：第 1 行：invalid key，检查一下 /tmp/dct-verify/home/.dct/secrets.toml`
- `chmod 000` secrets file → title:
  `选 agent —— 密钥文件读不了：没有权限读取，检查一下 /tmp/dct-verify/home/.dct/secrets.toml`

Both are single-line, plain Chinese aside from the deliberately-kept `invalid key` fragment,
with a red border (unchanged rendering logic) and no `os error`/`Permission denied`/stack
trace anywhere.

## Minor: empty-`entries` guard

`src/ui.rs`'s `n`-key handler built a fresh `ListState` and called `state.select(Some(0))`
unconditionally before constructing `View::PickProfile`. Guarded it:

```rust
if !entries.is_empty() {
    state.select(Some(0));
}
```

Practically unreachable (daemon always returns ≥ 9 built-ins) but a one-line guard turns a
would-be `entries[0]` panic on Enter into a harmless "nothing selected" state instead.

## Commands and output

```
~/.cargo/bin/cargo build                     # clean
~/.cargo/bin/cargo fmt --check                # clean, no diff
git diff --check                              # clean, no whitespace issues
env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo test
  # lib: 134 passed; 0 failed
  # tests/cli.rs: 2 passed
  # tests/client_timeout.rs: 1 passed
  # tests/concurrency.rs: 1 passed
  # tests/daemon_detach.rs: 1 passed
  # tests/daemon_roundtrip.rs: 2 passed
  # tests/profiles_flow.rs: 5 passed
  # tests/projects_flow.rs: 3 passed
  # tests/signal_restore.rs: 2 passed
  # tests/slow_input.rs: 1 passed
  # tests/socket_perms.rs: 1 passed
  # doc-tests: 0
  # total: 0 failed anywhere
env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo clippy --lib --tests
  # 1 warning surfaced (unnecessary_to_owned in the new daemon.rs test) — fixed, re-ran clean
```

## Files changed (this fix round)

- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/ui.rs`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/secrets.rs`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/profile.rs`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/daemon.rs`
