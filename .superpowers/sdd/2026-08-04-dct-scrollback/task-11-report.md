# Task 11 report: READMEs for the scrollback feature

## What the brief got wrong (superseded, per the task overrides)

- **`src/restart.rs` / `dct restart` command: does not exist.** Verified by reading
  `src/main.rs` (the `HELP` string and the `match args.first()` dispatch, lines ~9-70)
  and `grep -rn restart src/cli.rs src/main.rs` — no `restart` subcommand anywhere.
  The version-mismatch path that actually shipped is `offer_to_restart_stale_daemon`
  in `src/main.rs:132` (called from `run_ui` at `src/main.rs:113` when
  `daemon_status(...) == DaemonStatus::Stale`), which calls
  `dct::client::restart_daemon` (`src/client.rs:19`). This runs automatically at
  startup, before the TUI opens — it is not a command the user types. I did not
  document `dct restart` anywhere. I did not add `src/restart.rs` to the file
  inventory.
- **Line numbers**: the brief's `:90`/`:149`/`:104-116` are all stale. The
  "scrolling doesn't work" paragraph is at `README.md:133` / `README.zh-CN.md:133`
  (confirmed by `grep -n scrollback`). There was no separate "not done yet" bullet
  list containing a scrollback entry distinct from this paragraph — the "Things
  that will annoy you" section *is* that list, and the scrollback line inside it
  is the one edit needed; I did not find a second occurrence anywhere in either
  file (`grep -in "scrollback\|滚屏历史"` before editing found exactly one hit per
  file, both in that section).
- **File inventory**: `src/ui.rs` is already `src/ui/` (`mod.rs`, `view.rs`,
  `app.rs`, `board.rs`, `grid.rs`, `attach.rs`, `pick.rs`, `secret.rs`,
  `widgets.rs`) in both READMEs already — this must have landed in an earlier
  commit on this branch. I left it as-is (verified against `ls src/ui/`, which
  also shows `keys.rs` and `settings_view.rs` that aren't listed — the file list
  is a curated "worth knowing" set, not exhaustive; it already omits `main.rs`,
  `lib.rs`, `cli.rs`, `i18n.rs`, `config.rs`, `clipboard.rs`, `journal.rs`,
  `llm/`, `settings.rs` on both branches, before and after this task, so that's
  pre-existing curation, not something Task 11 was asked to fix).

## Every claim added, and what it was checked against

**README.md / README.zh-CN.md, "Getting it running" section (new paragraph)**
- "the next start notices ... explains ... and asks before touching anything" —
  `src/main.rs:112-114` (`daemon_status(...) == Stale` gates the call) and
  `offer_to_restart_stale_daemon` (`src/main.rs:132-163`), which prints
  `Key::StaleDaemonExplain` then `Key::StaleDaemonAsk` and reads a line before
  doing anything (`src/i18n.rs:431-446`).
- "restarting it will end whatever sessions are currently running (file changes
  stay, the agents don't)" — this is the literal content of
  `Key::StaleDaemonExplain` (`src/i18n.rs:431-440`) and matches
  `client::restart_daemon`'s doc comment (`src/client.rs:13-18`: "这会杀掉所有正在
  跑的会话，因为 pty 就在守护进程里").
- "say yes ... swaps the daemon in and reconnects; say no ... carries on with the
  old one" — `src/main.rs:146-162`: non-`y` answer returns the existing client
  unchanged; `y` drops it, calls `restart_daemon`, reconnects.
- I deliberately did **not** claim it "lists whatever is still running" (the
  brief's wording for the old, non-existent `dct restart` command) — the actual
  explain text is generic ("the sessions running right now will end"), it does
  not enumerate session IDs. Checked `Key::StaleDaemonExplain` text directly;
  there is no session listing in that path.

**README.md / README.zh-CN.md, "Inside a session" (new paragraph, scroll behavior)**
- Wheel and `PageUp`/`PageDown`/`End` scroll — `src/ui/attach.rs:35-68`
  (`wheel_action`, `key_scroll`).
- "roughly the last 2000 lines ... a ceiling, not a promise" —
  `SCROLLBACK_ROWS: usize = 2000` (`src/pty.rs:67`) plus its use in
  `vt100::Parser::new(rows, cols, SCROLLBACK_ROWS)` (`src/pty.rs:140`); the
  "ceiling" framing matches the test `the_view_stays_put_when_new_output_arrives`
  and the comment on `st.max` capping in `key_scroll`'s doc.
- "wheel moves 3 lines a notch" — `const WHEEL_ROWS: i32 = 3;`
  (`src/ui/attach.rs:17`), used in `wheel_action` (`:42`).
- "a page moves a full screen minus two lines" — `key_scroll`
  (`src/ui/attach.rs:56`): `let step = i32::from(page).saturating_sub(2).max(1);`
  Comment there explicitly gives the "keep two lines of overlap" rationale,
  which I reused (in plain language, not "overlap" jargon).
- "`End` jumps straight back down" — `KeyCode::End if st.offset > 0 =>
  Some(ScrollAction::Scroll(-i32::MAX))` (`src/ui/attach.rs:65`).
- "Claude Code wants the mouse ... wheel goes straight to Claude Code ... no hint
  shown" — routing is on `ScrollState.agent_owns`
  (`wheel_action`/`key_scroll`, `src/ui/attach.rs:35-53`, `ScrollAction::Forward`
  when `agent_owns`); `agent_owns` is set from
  `screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None` (`src/pty.rs:509`).
  The "no hint" behavior — `scroll_hint` (`src/ui/attach.rs:71-84`) only emits
  the "agent owns the screen" hint when `!st.agent_owns && st.alt_screen`; when
  `agent_owns` is true, none of the three hint branches fire, so `None`. The doc
  comment on `wheel_action` (`src/ui/attach.rs:29-34`) is explicit that this was
  measured against real Claude Code (alt screen + mouse reporting) vs real codex
  (inline + no mouse), which is where "Claude Code wants the mouse, codex
  doesn't" comes from directly.
- "new lines don't drag your view down ... bottom bar counts how many are
  waiting" — `scroll_hint`'s first branch (`src/ui/attach.rs:72-74`,
  `st.offset > 0 && st.new_lines > 0` → `scroll_new_lines_below`); `new_lines`
  computed in `state_of` (`src/session.rs:726-733`,
  `v.offset.saturating_sub(mark)`) and the pinning behavior is exercised by
  `session.rs`'s `the view stays put...`-style tests
  (`new_lines_counts_only_what_arrived_since_the_user_last_scrolled`,
  `src/session.rs:1827+`).
- "type anything, or resize the window, snaps back to the bottom" —
  `src/session.rs:464-477` (`Input`: comment "一敲键就回到底部" +
  `scroll_to_bottom()` + `scroll_mark = 0` on every input write) and
  `src/session.rs:530-539` (`resize`: same reset, comment explains vt100
  re-wraps on width change so the old offset would point at the wrong line).

**README.md / README.zh-CN.md, grid paragraph (new clause)**
- "the grid doesn't scroll a tile's history" — `grep -n "PageUp\|PageDown\|wheel_action\|key_scroll" src/ui/mod.rs src/ui/grid.rs src/ui/board.rs`
  shows scroll routing (`wheel_action`/`key_scroll`) is only reachable from
  `attach.rs`'s `handle_key`/`handle_mouse`, which only fire when
  `View::Attached`; grid/board have no path into it.

**README.md / README.zh-CN.md, "Things that will annoy you" (replaced paragraph)**
- "dct grabs the mouse ... click-and-drag text selection stops working ...
  In iTerm2 you hold Option ... Back on the board the mouse is yours again" —
  `src/ui/mod.rs:537-545`: `is_attached = matches!(app.view, View::Attached(_))`
  gates `EnableMouseCapture`/`DisableMouseCapture` via `mouse_capture_transition`
  — capture is only on while attached, off everywhere else (board, grid). The
  Option-key / iTerm2 fact and "dct has no copy of its own yet" are product
  facts from the task brief, not independently re-derivable from source (no
  code implements a copy feature — confirmed by there being no
  clipboard-copy-from-scrollback code path in `src/clipboard.rs`, which is
  about profile/secrets clipboard use, not session-history copy).

## Claims I could not verify directly in code

- The Option-key-in-iTerm2 remedy for regaining native selection while `dct`
  holds mouse capture: this is standard terminal behavior (iTerm2's own
  modifier-to-bypass-mouse-reporting convention), not something `dct`'s source
  controls or asserts. Took it as given per the task's "what the feature
  actually does" brief.

## Concerns

- None outstanding. Both files were re-read end to end after editing; no
  banned words (checked programmatically:
  `git diff | grep -iE "seamless|powerful|effortless|robust|simply|无缝|强大|轻松|一键搞定"`
  → no match), no emoji, English and Chinese paragraphs say the same four
  things (daemon-restart offer, scroll mechanics, grid has no scroll, mouse
  capture cost) without being sentence-by-sentence translations of each other.
- One judgment call beyond the brief's literal scope: I added a short "the grid
  doesn't scroll" clause and expanded the scroll-mechanics description beyond
  what the brief's Step 1 asked for (which was cost-only). The task's own "WHAT
  THE FEATURE ACTUALLY DOES" section listed these as facts the README should
  be truthful about, so I documented them in the "Inside a session" prose
  rather than only in the annoyances list, on the reasoning that "how it works"
  belongs with the feature description and "what it costs you" belongs with
  the honest shortcomings list. Flagging this as a deviation from the brief's
  literal diff, in case the reviewer wanted the terser cost-only version.
