# Brief: make the dct board list (会话看板) tidy and good-looking

Worktree: /Users/lei/work/dc/dc-terminal-wt-board (branch feat/board-tidy, off main). Work ONLY here.
User approved the design below and added: 「做的漂亮一些」 — make it look polished, not just functional.

## Today (what the user called ugly)
Code: `src/ui/board.rs` (row rendering ~lines 168-290), `src/ui/view.rs` (`Row`, `flatten` ~1211),
activity text = `s.pty.last_line()` (`src/session.rs` ~1700).
Real screen today (22 sessions, nearly every project has exactly 1 session):
```
 2 ▾ dc-android        ~/work/dc         claude×1 codex×1
   47  干活中  claude    平板视频部署与…  ○ general-purpose  Running gradle assembleDebug build   … ✉ 1
   63  空闲    codex     阅读OpenAI手机   ← for agents · ? for shortcuts                          …
 3 ▾ dc-device         ~/work/dc         claude×1
    7  空闲    claude    现在项目什么样…  ⏵⏵ bypass permissions on (shift+tab to cycle) · ← 4 agents
   ... dc-thesis busy: ⏵⏵ bypass permissions on · 1 shell · esc to interrupt · ← 4 agents · ↓ to manage
```
Problems: the last column is the agent's own footer (same noise on every row); two lines per
project so 22 sessions don't fit; `~/work/dc` and `claude×1` add nothing; idle vs busy hard to tell;
stray `…` at line ends.

## Design (approved)
1. **One line for a project with one session.** Row shows: group number (1-9 as today, blank
   from 10), the project marker/current-project highlight, session id, status, agent (profile),
   project name, session name, activity, end marks (live mark, ✉ N). Projects with 2+ sessions
   keep a title line plus indented session lines. Navigation keeps working exactly as today
   (Enter/attach, collapse, number keys 1-9, Tab, current-project bar/chip, ▶ highlight). Likely
   a new `Row` variant (e.g. `Single(group)`) — every match on `Row` must handle it; Enter on it
   behaves like Enter on its session.
2. **Activity column shows only what the session is doing.**
   - New profile field `activity_noise: Vec<String>` (regexes, `#[serde(default)]`, documented
     like neighbours). The activity text is split into segments on ` · ` (and runs of 3+ spaces);
     segments fully matching a noise regex are dropped; leading arrows/glyph-only leftovers and
     trailing `…` are trimmed; remaining segments rejoined with ` · `.
   - Fill it in the builtin profiles from what is observed (claude + the claude-based kimi/glm/
     deepseek/qwen-api share the claude footer; codex its own): e.g. `bypass permissions on`,
     `shift\+tab to cycle`, `don't ask on`, `\? for shortcuts`, `← \d+ agents`, `← for agents`,
     `esc to interrupt`, `↓ to manage`, `⚠ \d+ warnings?`, `f2 to view`. Test against the real
     strings above. Never branch on profile names in code.
   - **Idle → activity blank.** Busy → cleaned activity (dc-thesis example → `1 shell`).
3. **Status tells the story at a glance.**
   - Idle: dim/grey, with how long: `空闲 3分钟` / `空闲 2小时` / `空闲 3天`, just `空闲` under a
     minute (English `idle 3m/2h/3d`). Use `last_active_ms` already in the session info; use i18n.
   - Busy: bright/accent. Failed/error stays red (danger). Waiting-for-user states keep whatever
     emphasis they have today.
   - A small state dot before the status is welcome (● busy, ○ idle) — plain text glyphs, NOT emoji.
4. **Project title line** (multi-session groups only): drop the `claude×1` counts and the parent
   dir column; show the parent dir only when two groups share a name. Keep the red 「N 个出错」
   — it must never be pushed off-screen (see the long comments in board.rs; keep that rule).
   For single-line rows the red failed state shows through the status column.
5. **Polish** (the user asked for 「漂亮」): consistent column alignment across single and
   multi-session rows (ids right-aligned, statuses, agents, names line up in one grid); one
   accent colour for busy, dim for secondary info (agent name, idle time), project name bold;
   no stray `…`; respect existing theme helpers (`accent()`, `dim()`, `danger()`, the six
   bottom-bar palettes) — no hard-coded colours. Respect CJK widths (`display_width`,
   `truncate`, `pad_to`). Must still look right at 80 columns (truncate the activity first,
   never the end marks or the failed count).
6. Out of scope: web/phone views, the grid view, ordering of groups.

## Tests (TDD; isolated, never the real ~/.dct)
- activity cleaning: each real string above → expected result (claude idle → empty after
  cleaning; dc-thesis busy → `1 shell`; #47 busy keeps `Running gradle assembleDebug build`).
- profile: claude-based builtins + codex have non-empty activity_noise; a TOML without it parses.
- board render (existing test style in board.rs, TestBackend): single-session group = 1 line;
  2-session group = title + 2 lines; no `claude×1`; parent dir only on name clash; idle shows
  duration and blank activity; failed count still visible at 80 cols; ✉ mark still visible.
- navigation: Enter/attach on a single row opens that session; number keys and collapse still work.
- Update existing tests that assert the old layout — change the assertion, don't delete the
  intent behind it (read their comments).
- Mutation check: disable the noise filter, confirm a test fails, restore. Say so in the report.

## Rules
- Do NOT run `cargo fmt`. Match the surrounding style (Chinese doc comments explaining why).
- Before finishing: full `cargo test` and `cargo +1.99.0 clippy --all-targets -- -D warnings` pass
  (run outside the sandbox if git snapshot tests fail inside it; two known flaky session tests:
  `whitespace_only_input_is_asked_about_exactly_once_not_forever`,
  `a_tag_born_from_control_bytes_never_carries_them_into_the_render_path`).
- Render the final board for the report: a TestBackend dump at 120 and 80 columns with a
  realistic set of sessions (like the screen above), pasted into the report.
- Commit messages in English, NO Co-Authored-By / AI attribution lines.
- Do not dispatch subagents. Do not merge, push, install, or restart anything.
- Put your full report in your final reply (report files may be blocked); start the reply with:
  status, commits, one-line test summary, concerns — then the report and the render dumps.
