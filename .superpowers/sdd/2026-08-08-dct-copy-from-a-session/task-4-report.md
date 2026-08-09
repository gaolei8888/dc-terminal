# Task 4 report: 两份 README 说实话

Branch: `feat/copy-from-session`
Commit: `fed2898` — "docs: the mouse stays yours unless the agent asked for it"

## What changed, per file

Both files got the same three edits (English in `README.md`, Chinese in `README.zh-CN.md`), all inside the "For anyone reading this" body — no other sections touched.

### 1. "Inside a session" key list (the F2/F3/Ctrl+Q sentence)

This sentence claimed `dct` keeps exactly three keys inside a session. That's now false: Task 2 made `F4` a fourth key `dct` swallows (toggles `copy_mode`, never forwarded — see `attach.rs::handle_key`'s `KeyCode::F(4)` arm and its own test `f4_toggles_copy_mode_and_is_never_forwarded_to_the_agent`). Updated the sentence to name four keys and describe what `F4` does, with a forward reference to the paragraph added later ("more on that below" / "见下文") rather than re-explaining copy mode twice.

### 2. The scroll-back paragraph (wheel / PageUp / PageDown / End)

This is the finding under "Step 3 matters most" — not explicitly named in the brief, but broken by Task 1's actual behavior change. Before this branch, `wants_mouse_capture` was just `is_attached` — the mouse was captured for *every* attached session regardless of what the agent wanted, so a codex/shell session (which doesn't subscribe to mouse reporting) still delivered wheel events to `dct`, and `wheel_action`'s local-scroll branch (`ScrollAction::Scroll`) was real, reachable behavior. That's what the old sentence "codex doesn't want the mouse, so `dct` scrolls what it kept instead" described, correctly, at the time.

Task 1 (`441ae41`) changed the formula to `attached && agent_subscribed && !copy_mode` — capture now only turns on when the agent itself wants the mouse. Tracing the consequence: `handle_mouse` (and therefore `wheel_action`) is only ever invoked while mouse capture is on, which now requires `agent_owns == true` — but `wheel_action` forwards to the agent whenever `agent_owns` is true (`if st.agent_owns { return ScrollAction::Forward; }`). So the branch that scrolls `dct`'s own kept history from the wheel is no longer reachable in production for *any* agent: whenever the wheel is captured, it's forwarded; whenever it isn't captured, `dct` never sees the event at all. `PageUp`/`PageDown`/`End` are unaffected — those are keyboard events, delivered regardless of mouse capture, and `key_scroll`'s gating on `agent_owns` predates this branch entirely (introduced in `3b16003`, untouched by Tasks 1-3).

I confirmed this reading against Task 1's own report (`task-1-report.md`) and its brief's inline comment: "codex/shell 会话里抓着它，唯一的效果是把终端的拖选复制废掉，换来一个 PageUp/PageDown/End 已经能做的滚轮" — i.e., the team already knew, when writing Task 1, that dropping mouse capture for non-owning agents costs nothing beyond a redundant wheel, because the keys already cover the same ground. That's exactly the fact the old README sentence no longer stated correctly, and it's exactly what the brief's own replacement text for the annoyances section (Step 1) independently asserts: "代价是那些会话里滚轮不再翻 `dct` 的历史，用 `PageUp`/`PageDown`/`End`。" Leaving the older scroll-back paragraph saying the opposite would have put two paragraphs of the same document in direct contradiction — a drift the task explicitly asked me to catch.

Rewrote the paragraph to: drop the "wheel moves 3 lines a notch" claim (that number described the now-unreachable local-scroll branch and is no longer something a user will ever observe), keep `PageUp`/`PageDown`/`End` as the described way to scroll `dct`'s own history, and state plainly that the wheel only does anything when the agent wants the mouse (goes straight to the agent) and does nothing in sessions where the agent doesn't (codex, plain command-line tools) — matching the annoyances-section text below it.

### 3. The annoyances-section paragraph (Step 1 / Step 2 of the brief)

Replaced verbatim per the brief in `README.zh-CN.md`. Wrote the equivalent English in `README.md`, three paragraphs matching claim-for-claim:
- `dct` only takes the mouse when the agent itself wants it; Claude Code does, codex/plain CLIs don't, and in the latter the mouse stays with the terminal the whole time (click-and-drag, copying work as always); the cost is the wheel no longer scrolls `dct`'s own history there, use `PageUp`/`PageDown`/`End`.
- `F4` enters copy mode in agent-owning sessions: mouse goes back to the terminal, bottom bar says so, `F4` again leaves it; or use the terminal's own modifier (Option in iTerm2) without leaving the session.
- `dct` has no copy of its own — copying uses whatever the terminal gives you.

I deliberately avoided carrying forward the brief's "自查" claim that `enter_session` is the sole funnel into a session (that section was explicitly out of scope, and Task 2 established it's false — `create_session` is a second entry point, both reset `copy_mode`). Nothing in my README text makes that claim; the F4 sentence just says it toggles copy mode, with no statement about which functions reset it.

## Key verification (Step 3)

| Key | Checked against | Result |
|---|---|---|
| `F4` | `src/ui/attach.rs::handle_key`, `KeyCode::F(4)` arm (line 145) | Exists, toggles `app.copy_mode`, never reaches `key_to_input`/forwarding — confirmed by its own test `f4_toggles_copy_mode_and_is_never_forwarded_to_the_agent` |
| `PageUp` / `PageDown` | `src/ui/attach.rs::key_scroll` | Exist; consumed by `dct` only when `!st.agent_owns` and `st.max != 0`, otherwise fall through to the agent as ordinary keys |
| `End` | same | Exists; special-cased to always win when `st.offset > 0` (regardless of `agent_owns`), otherwise falls through like the other two |
| `F2`, `F3`, `Ctrl+Q` | `handle_key` | Unchanged by this branch, already correctly documented |

No documented key was found to be nonexistent. No key that exists and matters (in the mouse/copy context) was found undocumented.

## Mouse-release timing claim vs `wants_mouse_capture`

Verified against `src/ui/mod.rs::wants_mouse_capture(attached, agent_subscribed, copy_mode) = attached && agent_subscribed && !copy_mode`, `agent_subscribed` fed from `App.scroll.agent_owns`. The new text's claims all check out:
- Mouse is taken only when the agent wants it (`agent_subscribed` required) — true.
- `codex`/plain CLIs never get their mouse taken (they don't subscribe) — true, and this also means `dct`'s own scroll wheel stops working in those sessions, which the new text says explicitly.
- `F4` (`copy_mode`) hands the mouse back even in an agent-owning session — true, `!copy_mode` is a hard veto in the conjunction, confirmed by the truth-table test `mouse_is_captured_only_when_all_three_conditions_hold`'s `"复制模式一票否决"` case.
- The bottom bar states the copy-mode status while active, long or short form depending on width — confirmed against `src/ui/mod.rs` `draw()`'s `View::Attached(_) if app.copy_mode` arm, which picks `CopyMode` or `CopyModeShort` by measured width.

## Staging-gate checks (Step 4)

**Gate 1** — `git diff --cached` after applying only my hunks: contains exactly the three paragraph edits per file (six hunks total across both files), nothing else. Grepped the staged diff for `install.sh|~/.local/bin|DCT_INSTALL_DIR|codesign|inode` — zero matches, confirmed clean.

**Gate 2** — unstaged `git diff README.md README.zh-CN.md` after staging, diffed against the Step-1 backup patch (`/private/tmp/claude-502/.../scratchpad/user-readme-edits.patch`): the only differences were the `index <hash>..<hash>` lines (expected — those track blob hashes, and the "before" blob changed once my hunks were staged into the index). Every `+`/`-`/context line, i.e. the actual content of the user's `install.sh` blocks in both languages, is byte-for-byte identical to the backup.

Both gates passed; committed without `git add`, using only the already-correct staged content.

## Procedure used

1. Backed up `git diff README.md README.zh-CN.md` to the scratchpad before touching anything.
2. Made all edits with the Edit tool directly on the working files (which already carried the user's uncommitted install.sh blocks).
3. Took a fresh full diff, hand-built a patch containing only my hunks (second and third hunk in each file; skipped the first hunk, which was the user's install.sh block), and applied it with `git apply --cached`. Verified with `--check` first.
4. Ran both gate checks (above) before committing.
5. Committed with `git commit -m "docs: the mouse stays yours unless the agent asked for it"` — no `git add`, no `Co-Authored-By`.

Never staged `.superpowers/sdd/.gitignore` — confirmed by `git diff --cached --name-only` showing only the two README files.

## Deviations from the brief

- Extended the fix beyond the brief's two explicitly named steps: also updated the "F2/F3/Ctrl+Q" key-count sentence (now four keys, F4 added) and the scroll-back paragraph's wheel claim (now dead code per the mouse-capture formula, as traced above). Both are claims Tasks 1-3 made false; the brief's own Step 3 instruction ("核对文档里的每个键" / verify every key and the mouse timing) plus my task instructions' "fix it too and say so" directive cover this.
- Did not carry forward the brief's "自查" claim about `enter_session` being the only funnel into a session, per my task instructions flagging it as false and out of scope.

No source files were touched — only `README.md` and `README.zh-CN.md`.

---

## Fix round 1: alt-screen wheel claim overreached; mouse-ownership claim overstated permanence

Commit: `afe8db1` — "docs: acknowledge terminal-dependent wheel behavior, per-frame mouse ownership"

The coordinator's review caught two remaining problems in text I wrote in the first pass, both in the same two paragraphs.

### Blocking finding: "the wheel does nothing in those sessions" was false on common terminal defaults

My extension (b) correctly traced the capture-ON case (`wheel_action` forwards to the agent whenever `agent_owns`, so `dct`'s own local-scroll branch is unreachable) but stopped there and asserted the wheel is inert whenever capture is off. The reviewer traced the capture-OFF case, which I'd missed: `dct` runs in the alternate screen and never sends `?1007h` (checked — no occurrence of `?1007` anywhere in `src/`), so on any terminal that implements alternate-scroll-as-arrow-keys and defaults it on (iTerm2, Terminal.app, VTE/GNOME, Alacritty, kitty, WezTerm, xterm with `alternateScroll`), a wheel notch with mouse tracking off becomes an `Up`/`Down` key event. That event is not `F2`/`F3`/`F4`/`Ctrl+Q`, gets `None` from `key_scroll` (which only claims `End`/`PageUp`/`PageDown`), and falls through `key_to_input` straight to `Request::Input` — sent to the agent as a real keystroke. So in exactly the sessions the paragraph calls out (codex, plain shells), rolling the wheel can type arrow keys at the agent (walking shell history, moving through codex's input) — the opposite of "nothing happens," and new behavior introduced by Task 1 (before this branch, capture was always on, so the terminal never got a chance to translate the notch itself).

**Fix**, both files, scroll-back paragraph:

- English (`README.md`, was ending "...so `dct` doesn't capture it there either — the wheel does nothing in those sessions, use `PageUp`/`PageDown`/`End` instead."):
  > "...so `dct` doesn't capture it there either — the wheel no longer scrolls `dct`'s history, and depending on your terminal it may do nothing or send arrow keys straight to the agent; use `PageUp`/`PageDown`/`End` instead."

- Chinese (`README.zh-CN.md`, was ending "...`dct` 也就不去抓它，滚轮在那些会话里不起作用，翻历史用 `PageUp`/`PageDown`/`End`。"), written in the document's own voice rather than translated literally:
  > "...`dct` 也就不去抓它——滚轮不再翻 `dct` 的历史，至于滚轮本身会怎样，看你用的终端：有的什么反应都没有，有的会把它当成方向键直接送给 agent，稳妥的办法还是 `PageUp`/`PageDown`/`End`。"

Kept the true, verified part (the wheel no longer scrolls `dct`'s own kept history — that follows from the capture formula and is unaffected by this fix) and added the honest uncertainty rather than picking a side.

### Minor finding: "the mouse stays with the terminal the whole time" overstated permanence

`agent_owns` is a per-frame fact read from the live vt100 parser, not a fixed trait of an agent — `attach.rs:51-59`'s own comment on `wheel_action` says so explicitly (a plain command finishing and being replaced by a mouse-reporting pager/TUI flips `agent_owns` mid-session, no automatic reset of anything else). Saying the mouse "stays with the terminal the whole time" in a codex/shell session promises something that isn't guaranteed: a shell that launches `less -M`, `htop`, or any other mouse-reporting program mid-session gets its mouse captured just like Claude Code, no different mechanism.

**Fix**, both files, annoyances-section paragraph:

- English (`README.md`, was "...codex and plain command-line tools don't — in those sessions the mouse stays with the terminal the whole time, so click-and-drag text selection and copying work exactly as they always do."):
  > "...codex and plain command-line tools don't — in those sessions the mouse stays with the terminal as long as nothing running there asks for it, so click-and-drag text selection and copying work exactly as they always do."

- Chinese (`README.zh-CN.md`, was "...codex 和普通命令行不要——那些会话里鼠标一直归终端，拖动选中文字、复制，跟平时完全一样。"):
  > "...codex 和普通命令行不要——只要会话里跑着的东西没自己伸手要鼠标，鼠标就归终端，拖动选中文字、复制，跟平时完全一样。"

Nothing else in either file was touched — the paragraphs the reviewer passed (Step 1's verbatim Chinese replacement structure, the F2/F3/F4/Ctrl+Q key-count sentence, the copy-mode/F4 paragraph, the "`dct` has no copy of its own" line) are untouched from the prior commit.

### Procedure

Same as the first pass, run twice to avoid manual-transcription risk after a first attempt at a hand-typed hunk-only patch failed `git apply --check --cached` with "corrupt patch" (a transcription slip in a hand-copied hunk, not a real patch-format problem — confirmed by then generating the isolated patch mechanically instead):

1. Backed up `git diff README.md README.zh-CN.md` to the scratchpad (`user-readme-edits-round2.patch`) before making any edits — confirmed identical in content to the very first backup from the initial pass (only blob-hash lines in `index ...` headers differed, which is expected since `HEAD` had moved).
2. Made both edits with the Edit tool directly on the working files.
3. Took a fresh full diff (`full-diff-round2.patch`) and this time built the isolated my-hunks-only patch **programmatically** (a short Python script splitting the diff into per-file blocks and per-hunk chunks, then dropping any hunk containing `install.sh`/`DCT_INSTALL_DIR`/`codesign`/`inode`) rather than hand-retyping hunk headers and content — this produced `mine-only-round2-auto.patch`, which passed `git apply --check --cached` on the first try.
4. Applied it with `git apply --cached`.
5. Ran both gate checks (below) before committing.
6. Committed with `git commit -m "docs: acknowledge terminal-dependent wheel behavior, per-frame mouse ownership"` — no `git add`, no `Co-Authored-By`. Confirmed `.superpowers/sdd/.gitignore` was never staged (`git diff --cached --name-only` showed only the two README files, both before and after commit).

**Gate 1** (`git diff --cached` contains only my changes): the staged diff showed exactly the two paragraph edits in each file (four hunks total). Grepped it for `install.sh|~/.local/bin|DCT_INSTALL_DIR|codesign|inode` — zero matches.

**Gate 2** (unstaged diff still matches the backup): `git diff README.md README.zh-CN.md` after staging, diffed against `user-readme-edits-round2.patch` — the only differences were the two `index <hash>..<hash>` header lines (expected, since the "before" blob in the index changed once my hunks were staged); every content line of the user's install.sh blocks in both languages is byte-for-byte identical to the backup.

Commit: `afe8db1`.
