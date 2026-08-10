# Final whole-branch review — `feat/session-auto-name`

**Reviewed range:** `5da4ec4..10bea5f`, 13 commits.
**Review package:** `.superpowers/sdd/2026-08-09-dct-session-auto-name/review-5da4ec4..10bea5f.diff`
**Date:** 2026-08-09

## Reading this cold

The branch does two things.

1. **A bug fix.** The nine-tile grid's focus was a positional index that was only
   clamped, never re-anchored, when the session list changed. A session finishing
   drops out of the visible list, every tile after it shifts, and the focus
   silently lands on a different session — so a message typed into the reply box
   (`i`) could go to the wrong agent, and `s` (stop) / `u` (roll back), both
   irreversible, could hit the wrong one. The board's list cursor had always
   re-anchored by identity; the grid never did. Now it does.

2. **A feature.** Each agent session gets a short name (`SessionInfo.tag`),
   generated once by the configured `[llm]` backend the first time the session
   finishes a round of work, then pinned forever. It falls back to the user's
   truncated first line, then to the agent's profile name. `PROTOCOL_VERSION`
   deliberately stayed at 6.

Verification already done by the branch author, not repeated here: `cargo fmt
--check`, `cargo clippy --all-targets -- -D warnings`, and the full `cargo test`
were green at the branch head. Every task was individually reviewed and its
findings fixed. This review deliberately spent its effort on what per-task
reviews structurally cannot see: interactions between tasks, the branch as a
whole, and what narrow scoping let through.

**Caveat on line numbers.** During this review `HEAD` moved from `10bea5f` to
`f007276` — a docs-only commit made by another agent working in this repo
concurrently. All line references below describe the tree as it was read. Verify
before editing.

---

# Pre-merge blocking list

These four should be fixed before this branch merges.

1. **Finding 1 — the pinned name is built from raw keystrokes, not text.**
   `src/session.rs` (`collect_first_input` ~:202, `append_capped` ~:227,
   `request_name` ~:899). Control characters and escape sequences from the
   attached view end up in the permanent session name and are written verbatim
   to the terminal. Hits the documented common case (no `[llm]` configured).
2. **Finding 2 — the grid tile header evicts the status word.**
   `src/ui/grid.rs:475-486`. A generated name can consume the entire tile title
   at 60- and 80-column terminals, pushing off the one thing the grid exists to
   show.
3. **Finding 6 — the board list has zero test coverage for this feature.**
   `src/ui/board.rs:211,214`. Four separate mutations survive, including the one
   that makes the session list never show the name at all.
4. **Finding 7 — a fourth non-discriminating test.**
   `src/session.rs::recovering_from_a_failure_does_not_count_as_finishing_a_round`
   survives deletion of both guards it claims to pin. Delete it or rewrite it.

# Follow-up items (do not block merge)

- Finding 3 — what leaves the machine widened; document it or add an opt-out.
- Finding 4 — two user-facing surfaces bypass `session_label`.
- Finding 5 — the grid anchor is correct by an accident of layering; comment it.
- Finding 8 — duplicate tests, and the attach title's untested positive case.
- Finding 9 — surviving mutations / coverage holes worth closing.
- Finding 10 — the flaky pre-existing test `entering_a_session_always_lands_at_
  the_bottom_even_without_a_resize`; not this branch's fault, fix separately.

---

# Findings in detail

## Finding 1 — Important — the pinned name is built from raw keystrokes, not text

**Files:** `src/session.rs:202` (`collect_first_input`), `src/session.rs:227`
(`append_capped`), `src/session.rs:899` (`request_name`), `src/session.rs:136`
(`clean_name`). Rendering side: `src/ui/widgets.rs:137` (`truncate`),
`src/ui/widgets.rs:168` (`session_label`).

### The chain

`collect_first_input` records whatever `send_input` receives. In the **attached
view** every keystroke is forwarded individually — `src/ui/attach.rs:193` calls
`key_to_input` (`src/ui/mod.rs:874`), which produces:

| key | bytes sent |
|---|---|
| Backspace | `\x7f` |
| Up / Down / Left / Right | `\x1b[A` … `\x1b[D` |
| Esc | `\x1b` |
| Home / End / PageUp / PageDown / Delete / Insert | `\x1b[…` |
| Ctrl+a…Ctrl+z | `\x01`…`\x1a` |

The README states this outright: *"Inside a session every keystroke goes to the
agent, `Esc` included."* So for a user who edits their first message,
`first_input` becomes something like `"fix teh\x7f\x7f\x7fthe login bug"`.

`request_name` then pins the fallback with no sanitization at all:

```rust
let fallback: String = s.first_input.chars().take(NAME_MAX_CHARS).collect();
*recover(s.name_slot.lock()) = Some(fallback);
```

No `clean_name`, no trim, no control-character filter. `clean_name` — which is
applied only to *model* output — strips quotes and trailing punctuation but also
does not strip control characters.

### Why it reaches the terminal

Three links, each verified against the dependency sources in
`~/.cargo/registry/src/*/ratatui-0.28.1`:

1. `char_width` (`src/ui/widgets.rs:127`) returns `0` for control characters, so
   `truncate` neither drops them nor charges them against the width budget —
   they survive truncation intact.
2. `Span::render_ref` (ratatui `src/text/span.rs:396-400`) does **not** drop
   zero-width graphemes. It *appends* them to the previous cell's symbol:
   ```rust
   } else if symbol_width == 0 {
       // append zero-width graphemes to the previous cell
       buf[(x - 1, y)].append_symbol(grapheme.symbol)...
   ```
   (`Buffer::set_stringn` *does* filter control chars, and `Paragraph` skips
   zero-width symbols — but the board list item, the grid tile title and the
   attach block title all render through `Line` → `Span::render_ref`, which does
   not.)
3. The crossterm backend writes the cell symbol verbatim:
   `queue!(self.writer, Print(cell.symbol()))` (ratatui
   `src/backend/crossterm.rs:187`).

So an `\x1b[A` embedded in a name is a **cursor-up command emitted into the
board on every frame**.

### What the user experiences

- Most common: no `[llm]` configured — the README calls this *"the normal case,
  not a problem"* — so the fallback **is** the final name. The user types their
  first message in the attached view, backspaces over a typo, presses Enter. The
  board now shows a garbled name containing `\x7f` bytes, forever. There is no
  rename.
- Worse: the user presses Up (history recall) or Esc (agents use it for their own
  popups) before typing. The name begins with a real escape sequence and the
  board redraw is corrupted on every frame.
- A whitespace-only first input (space then Enter) produces a **non-empty but
  invisible** tag. `session_label` sees `!tag.is_empty()` and returns the blank
  tag, so the column that used to show the agent kind is permanently blank.
- Minor but related: `first_input` is also what gets embedded in `name_prompt`,
  so the model is asked to name a session based on a string full of `\x7f`.

### Security note

`name_prompt` feeds the last 2000 characters of the **agent's screen** to the
model. That screen can contain content the agent fetched from a repo or the web.
A model steered by hostile screen content can return up to 24 characters that
`clean_name` will not sanitize, which then render through the same unfiltered
path. Bounded, but it is an injection path into the user's terminal.

### What to change

In `append_capped`, skip `char::is_control()` characters (and ideally treat
`\x7f`/`\x08` as pop-last-char so the recorded text matches what the user
actually meant). Apply the same filter to `clean_name`'s output so the model path
is covered too — the cleanest single place is a shared `sanitize` applied to
**both** writes in `request_name`. Then re-check the fallback for blankness after
trimming: if the cleaned fallback is empty, **leave `name_slot` as `None`** so a
later, real input can still name the session, rather than pinning an invisible
name forever.

Add a test asserting a tag containing `\x1b[A` and `\x7f` never reaches the
rendered buffer.

---

## Finding 2 — Important — the grid tile header evicts the status word

**File:** `src/ui/grid.rs:475-486` (tile title construction in `draw_grid`).

The branch changed the tile title from `info.profile` to
`truncate(session_label(info), 20)`.

### The arithmetic

- `grid_shape` (`src/ui/grid.rs:35`) returns **3 columns** for 5 or more sessions
  on a page.
- `MIN_COLS` is 60 (`src/ui/grid.rs:130`); below that the grid is replaced by a
  "window too small" message, so 60 is a fully supported width.
- Tile areas come from `Layout::horizontal(Ratio(1, cols))` over the full width
  (`src/ui/grid.rs:457`), so a tile is `width / 3`. Two of those columns are the
  block's left and right border, leaving `width/3 - 2` for the title.

The title is built as: `"▶"` (1 col) + `"{id} {name} "` + `"{status} "` +
`"{project} "`. Status labels are `干活中` (6 columns) or `working` (7).

| terminal width | tile width | title budget | `"▶" + "1 " + 20-col name` | left for status + project |
|---|---|---|---|---|
| 60 | 20 | 18 | 23 | **nothing — name itself is clipped** |
| 80 | 26 | 24 | 23 | **1 column** |
| 120 | 40 | 38 | 23 | 15 — fits |

Before this branch the field was `profile`, at most 8 columns, so
`1 + 2 + 8 + 1 + 7 = 19` fit comfortably even in a 20-column tile.

### What the user experiences

On an 80×24 terminal — a very common size — with five or more sessions running
and any name of roughly ten CJK characters or twenty Latin characters, the tile
header is entirely consumed by the name. The status word (`干活中` / `working` /
`出错` / `failed`) and the project name are pushed past the tile's right border
and clipped away by ratatui's line-truncation branch. The nine-tile grid exists
to answer "which agents are working and which have stopped or failed at a
glance"; that answer disappears exactly when the feature is working as designed.

No overflow into neighbouring tiles — ratatui clips at the area — so this is
information loss, not visual corruption.

### Why no test caught it

The branch added width-adversarial tests for the reply row
(`a_long_name_never_pushes_the_draft_off_the_reply_row`, at 60 and 80 columns)
and for the attach title, but none for the tile header. The two existing tests
that assert `干活中` appears (`src/ui/grid.rs:1718` and `:2197`) render at
120×30 with an empty tag, where everything fits, so they pass regardless.

The mutation audit confirmed this independently: **dropping
`truncate(session_label(info), 20)` entirely fails no test.**

### What to change

Derive the name's cap from the tile width rather than using a fixed 20 — subtract
the known fixed costs (focus marker, id, status label, project) from
`tile.width - 2` and give the name the remainder, with a small floor. Or reorder
the title so status precedes the name, making the name the thing that gets
clipped instead of the status. Then add a tile-header test at 60 and 80 columns
with a 24-character name, asserting the status label is still present.

---

## Finding 3 — Important (judgment) — what leaves the machine widened

**Files:** `src/session.rs:173` (`name_prompt`), `src/session.rs:899`
(`request_name`), `README.md:182`.

Before this branch, the `[llm]` backend saw screen content only when a session
**failed** (`request_explanation`). Now `request_name` ships the user's first
message plus the last 2000 characters of the screen for **every agent session's
first completed round**, automatically.

`[llm]` is a single switch (`src/config.rs:40` `LlmConfig` — provider, model,
base_url, transport) with no per-feature opt-out. A user who configured it for
error explanations did not opt into having every session's first round uploaded.
The branch's own history notes that four vendor `base_url`s ship with the tool,
so the destination is frequently a third party.

The README's candid section (`README.md:182`) explains the *fallback* behaviour
well but never says the screen contents go to the model for naming.

**What to change:** add a sentence to the candid section, and ideally a
`[llm] naming = false` (or `explain_only = true`) escape hatch.

---

## Finding 4 — Minor — two user-facing surfaces bypass `session_label`

`src/ui/widgets.rs:168` introduces `session_label` as the single answer to "what
text represents this session", with a comment explaining that separately
formatted strings always drift. Two places still format their own:

- `src/ui/app.rs:266` — the failure toast, via
  `i18n::msg::session_failed(lang, id, &s.profile)` (`src/i18n.rs:779`). The user
  sees `会话 3（claude）出错了` while every other surface calls it by name.
- `src/cli.rs:96` — `dct ps` prints `s.profile`. `tag` is now on the wire and
  available here.

`src/ui/view.rs:683` also uses `profile`, but that is the group header's
"claude ×3" count, which is genuinely about agent kind, not identity. Correct as
is.

**What to change:** route both through `session_label`. `session_failed`'s
signature already takes a `&str`, so it is a call-site change.

---

## Finding 5 — Minor — the grid anchor is correct by an accident of layering

**File:** `src/ui/app.rs:277-283` and `:307-318` (`refresh_rows`).

`set_sessions` (`src/ui/app.rs:233`) assigns `self.sessions = v` **before**
calling `refresh_rows()`. Inside `refresh_rows`, the grid anchor is read at the
top:

```rust
let grid_anchor = match &self.view {
    View::Grid { focus, .. } => self.grid_sessions().get(*focus).map(|s| s.id),
    _ => None,
};
```

This is correct only because `grid_sessions()` (`src/ui/app.rs:325`) derives from
`self.groups`, which is still the *old* grouping at that point —
`self.sessions` has already been replaced. If anyone ever simplifies
`grid_sessions()` to read `self.sessions` directly (which looks equivalent), the
anchor silently becomes the post-shift session and the fix reverts to the bug.

The new test `refresh_rows_keeps_the_grid_focus_on_the_same_session` does catch
that regression, so this is a comment request, not a defect. The doc comment on
`refresh_rows` also still describes only the list cursor.

---

## Finding 6 — Important — the board list has zero test coverage for this feature

**File:** `src/ui/board.rs:211,214`.

The branch changed the board's session row from
`pad_to(&s.profile, 10)` + `truncate(&s.activity, 76)` to
`pad_to(&truncate(session_label(s), 15), 16)` + `truncate(&s.activity, 70)`,
re-budgeting six columns from activity to the name.

Mutation testing found **four separate mutations that fail no test**:

| mutation | result |
|---|---|
| `session_label(s)` → `s.profile` (list never shows the name) | no test fails |
| drop `truncate(…, 15)` | no test fails |
| drop `pad_to(…, 16)` | no test fails |
| revert activity `70` → `76` (breaks the column budget) | no test fails |

The README explicitly promises the name shows up in "the session list". Nothing
pins that, nor the column arithmetic the branch rewrote.

The arithmetic itself is correct as written: `truncate(s, 15)` returns at most 15
display columns, or exactly 16 when it truly truncates (the `…` is appended after
the width check), so `pad_to(…, 16)` yields exactly 16 and the row total is
unchanged at 8 + 16 + 70. But it is unguarded.

**What to change:** add board-row tests — one asserting the name is drawn and the
profile is not, one asserting a 24-character name does not push the activity
column out of alignment.

---

## Finding 7 — Important — a fourth non-discriminating test

**File:** `src/session.rs::recovering_from_a_failure_does_not_count_as_finishing_a_round`.

Its doc comment claims it pins the `Failed → Idle` misfire. It survives **every**
guard mutation:

| mutation | result |
|---|---|
| delete `&& !s.first_input.is_empty()` (`src/session.rs:851`) | **passes** |
| delete `&& was == SessionState::Working` (`src/session.rs:849`) | **passes** |
| delete **both** at once | **still passes** |

Reason: with the guards gone, `request_name` is simply called *again* on the real
`Working → Idle` transition, and the later call — whose prompt contains the real
first input — overwrites the slot with the correct name. The test's final
assertion inspects only the end state, so it can never observe a misfire. Every
mutation it does catch is caught by four other tests as well; it has zero unique
coverage.

Three tests on this branch were already found non-discriminating and rewritten
during the per-task reviews. This is the fourth.

**What to change:** delete it, or rewrite it to assert on `name_slot` **at the
moment of recovery** rather than on the final value.

`src/session.rs::a_fresh_session_has_no_tag` is also killed by no mutation — it
asserts a struct field starts empty. Harmless, but it buys nothing.

---

## Finding 8 — Minor — duplicates and an untested positive case

- **`clean_name_strips_a_quote_stacked_with_trailing_punctuation`**
  (`src/session.rs:1248`) is a literal duplicate: its single assertion
  `clean_name("「修登录白屏」。") == "修登录白屏"` is byte-identical to the first
  line of `clean_name_strips_quotes_punctuation_and_extra_lines`
  (`src/session.rs:1222`). No mutation kills one without the other.
- **`a_newline_inside_one_chunk_seals_at_the_newline`**
  (`src/session.rs:1199`) is subsumed by
  `a_multibyte_utf8_sentence_before_the_newline_does_not_panic`
  (`src/session.rs:1210`) — same three calls, same two assertions, ASCII vs CJK.
- **The attach title's positive case is untested.** Replacing the whole name
  branch in `src/ui/attach.rs:222-256` with bare `project` — so the attach title
  never shows the name — breaks no test.
  `a_long_name_never_pushes_the_way_back_off_the_title` asserts only that
  `F2返回看板` is present;
  `a_disconnected_title_drops_the_name_to_save_room_for_the_way_out` asserts only
  that the name is *absent*. Neither ever asserts the name *appears* in a
  connected title. Both are individually discriminating for the behaviours they
  do target.
- Soft overlap: the "tag stays empty" loop inside
  `a_freshly_created_busy_pattern_agent_…` re-asserts `a_fresh_session_has_no_tag`
  and is itself vacuous — the misfire would write `Some("")`, which `list()`
  renders as `""` anyway. Only the second half of that test does real work.

---

## Finding 9 — Minor — surviving mutations worth closing

Each of these mutations fails **no** test:

- `src/session.rs:850` — narrowing `matches!(next, Idle | Asking)` to
  `matches!(next, Idle)`. Naming on `Working → Asking` is untested.
- `src/session.rs:580` — removing `if s.is_agent` from `send_input`'s collection.
- `src/session.rs:851` — removing `&& s.is_agent` from the tick trigger.
  Together with the previous item: **nothing anywhere pins that a plain shell
  session never collects a first input or gets a name.** (Behaviour is currently
  correct — the two gates are redundant with each other — but neither is held.)
- `src/session.rs:903` — dropping `.chars().take(NAME_MAX_CHARS)` from the
  fallback. A 200-character pasted first input would become a 200-character tag,
  and the two display sites that must absorb it (board, tile) are themselves
  untested per findings 2 and 6.
- `src/session.rs:174-179` — removing `name_prompt`'s 2000-character screen tail
  slice. `explain_prompt` has the equivalent assertion
  (`p.user.chars().count() < 2500`); `name_prompt_carries_both_the_first_line_and
  _the_screen` has no size assertion at all.
- **Grid focus re-anchor** (`src/ui/app.rs:311-318`): the one new test covers
  *removal* of an earlier session. A session *added* ahead of the focused one
  (the `n` new-session case, equally common) and re-anchoring across a nine-tile
  page boundary are both uncovered.

---

## Finding 10 — the flaky test: verdict

**Test:** `src/ui/mod.rs::entering_a_session_always_lands_at_the_bottom_even_
without_a_resize`. Untouched by this branch and predating it; flaked four times
during the branch's work, always passing on rerun or in isolation.

**Verdict: a test-harness fixture artifact, not a production bug. Does not block
merge.** Reproduced 3 times in ~6 full-suite runs; 0 failures in 30 isolated runs
(~2.48s each); fails roughly 30–50% under full-suite parallelism.

The panic is **always** at the *setup* wait, never at either assertion the test
exists to make:

```
panicked at src/ui/mod.rs:2598:13:
没等到滚屏内容攒够
```

The real assertions (`:2608` `offset > 0`, `:2642` "重新进入会话必须落在底部")
never failed in any run.

### Mechanism

1. `profiles/shell.toml` is `command = ["/bin/zsh"]`, so the fixture spawns a
   **real interactive zsh with the developer's real `$HOME`**, which sources
   `~/.zshrc` — sdkman, nvm, `compinit`, `pyenv init`. Dozens of forks, hundreds
   of ms to seconds.
2. The test writes its 200-iteration echo loop into the PTY immediately after
   `Request::Create` returns (`src/ui/mod.rs:2571-2576`), i.e. as typeahead while
   zsh is still sourcing rc files. Anything in that chain that reads stdin (e.g.
   compinit's "insecure directories" prompt) eats it outright.
3. zsh spawn + rc files + 200 echoes + the pty reader feeding the vt100 parser +
   `scroll.max > 0` must all land inside a **5-second** deadline. In isolation
   that already consumes ~2.5s — a 2× margin.
4. Under default `cargo test --lib` parallelism the same binary concurrently runs
   4 more daemons, ~30 real PTY spawns, git-subprocess tests, and **three other
   tests that also spawn `/bin/zsh`** (`src/ui/mod.rs:2394, 2424, 2456`). Those
   three only assert on `Create`'s return value, so zsh's slowness is invisible to
   them; this is the only test that needs zsh to actually *execute* something.
   Suite wall time was ~12s idle vs 30–54s under load. The 5s window loses.
5. Aggravating: nothing stops the sessions or shuts the daemons down, so each run
   **orphans interactive zsh processes that outlive the test binary** and pile up
   across runs — a positive feedback loop.

### Ruled out, with evidence

- **Shared statics / env vars:** only `THEME: OnceLock` (`src/ui/mod.rs:52`,
  unused here) and `clipboard::SEQ`. All per-daemon state derives from the socket
  path (`store_path_for_socket`, `secrets_path_for_socket`,
  `profiles_dir_for_socket`, `config_path_for_socket`), and every daemon gets its
  own `tempfile::tempdir()` socket.
- **`env::set_var`:** only `src/session.rs:2226/2266`
  (`CLAUDE_CODE_CHILD_SESSION`), not on this test's path. Worth removing anyway —
  it is a process-global mutation in a multithreaded test binary.
- **Terminal size:** the PTY is spawned at fixed rows/cols by the daemon.
- **The behaviour under test:** `enter_session` (`src/ui/mod.rs:1319-1324`)
  issues a **synchronous** `Request::Scroll { by: Bottom }` via `c.call`, so the
  daemon has already zeroed scrollback under the session lock before the call
  returns. `tick()` and the pty reader never touch scrollback. The "异步落地"
  comment above the final poll loop is inaccurate — that loop is not racy.

### Fix

Stop typing into an interactive login shell; let the spawned process produce the
scrollback itself. The hook exists and is documented for this:
`daemon::run_with_manager` (`src/daemon.rs:32`) + `SessionManager::register_
profile` (`src/session.rs:396`). Replace the daemon spawn at
`src/ui/mod.rs:2544-2551` with a registered fixture profile whose command is
`/bin/sh -c "i=1; while [ $i -le 200 ]; do echo line-$i; i=$((i+1)); done; sleep 300"`,
create with that profile, and **delete the `Request::Input` block**
(`:2571-2576`). Precedents: `src/session.rs:2542 scrolling_session` and
`src/daemon.rs:489-495`. Optionally `Request::Stop` the session at the end so
orphan shells stop accumulating.

Band-aid alternative: raise both 5s deadlines to ~30s and wait for a shell prompt
before sending input — hides the slowness but not the typeahead race.

---

# What is clean

## Concurrency

Sound. `request_name` writes the fallback under the session lock, then spawns a
thread that later overwrites the same `Arc<Mutex<Option<String>>>`.

- **Lock ordering is consistent.** `list()` takes sessions-map → session →
  name_slot. `tick()` takes session → name_slot. The spawned thread takes
  name_slot only. `set_backend` (`src/session.rs:364`) takes the backend lock and
  never a session lock, so the backend-lock acquisition inside `request_name`
  (held while holding a session lock) cannot deadlock.
- **No thread leak.** The thread captures only the slot `Arc` and the backend
  `Arc`, never the `Session`. A session that is stopped, killed or pruned while
  the model call is in flight drops cleanly; the late write lands in an orphan
  slot and is harmless. The 15s timeout bounds the thread's life (deliberately
  shorter than the explanation path's 30s).
- **No lost update.** Exactly one thread can ever exist per session, so unlike
  `request_explanation` no generation counter is needed — the reasoning in the
  doc comment is correct.
- Session ids are never recycled, and there is no session persistence or restore
  path (`SessionManager`'s public API has no restore), so no cross-session
  confusion.

## Exactly-once

Confirmed by tracing every access. `name_slot` has exactly four sites:

| line | access |
|---|---|
| `src/session.rs:502` | init to `None` in `create()` |
| `src/session.rs:550` | read in `list()` |
| `src/session.rs:851` | `is_none()` guard in `tick()` |
| `src/session.rs:903` | write `Some(fallback)` in `request_name` |
| (+ `:909`) | the cloned `Arc`, written once by the spawned thread |

The guard check and the fallback write both happen under the session lock that
`tick()` already holds, so they are atomic with respect to each other. Nothing
ever resets the slot to `None`. The thread's write only ever replaces the
fallback with a cleaned, non-empty name. No path can re-trigger naming or lose a
name.

## Failure modes

- **No `[llm]` configured** (the common case): `request_name` returns early after
  writing the fallback; the session runs on. Correct — but see finding 1 for what
  that fallback contains.
- **Model returns garbage:** `clean_name` takes the first non-empty line, strips
  quotes and trailing punctuation asymmetrically (deliberately keeping leading
  `.` so `.NET 迁移` survives), caps at 24 chars, and an empty result leaves the
  fallback in place.
- **Session never receives input:** `!s.first_input.is_empty()` blocks naming.
  Correct, and load-bearing — all real profiles declare only `busy_pattern`, so
  `classify()` reports `Idle` for a session still on its startup screen, and
  without this guard the very first tick would pin an empty name forever.
- **Shell session:** double-gated by `is_agent` in both `send_input` and `tick`.
- **Created and stopped within one tick:** `tick` skips `Stopped` sessions;
  `name_slot` stays `None`; `session_label` falls back to the profile.

## The two parts do not interfere

- `Draft.id` (`src/ui/view.rs:106`) captures the recipient by identity when `i`
  is pressed, and the reply box resolves it with
  `visible.iter().find(|s| s.id == draft.id)` — no positional assumption.
- `View::Grid` stores no page; the page is derived from `focus` via
  `page_of` (`src/ui/grid.rs:45`), so re-anchoring across a page boundary cannot
  desync page and focus.
- `session_action`, `Enter`, `s`, `u`, `d` and `help_ctx_for`
  (`src/ui/mod.rs:933`) all read `grid_sessions().get(focus)`, which is now
  identity-anchored.
- `draw_grid` indexes `page_sessions` positionally, but from the same `visible`
  list `focus` indexes into. Consistent.

## Protocol

The decision to hold `PROTOCOL_VERSION` at 6 is sound and, unusually, is argued
in the test that would otherwise be the tripwire
(`the_session_info_shape_is_pinned_too`, `src/proto.rs:677+`). The two stated
conditions — `#[serde(default)]` on a read-only field, and no new or changed
`Request` variant — are both actually satisfied, and the comment explicitly
refuses to generalise the exception. Bumping would have broken
`ps`/`stop`/`kill`/`prune` against a running old daemon (those four never joined
the version handshake) and forced a restart that kills live sessions.

---

# Known items triaged before this review

None of these block merge.

- **`refresh_rows`'s doc comment still describes only the list cursor.**
  Confirmed; fold into finding 5.
- **`matches!(next, Idle | Asking)` is half-dead** — `SessionState::Asking` is
  never assigned and `classify()` cannot return it. Harmless. One thing to note
  for whoever wires `Asking` up later: `tick()` has an early `continue` for
  `state == Asking` (`src/session.rs:804`), so that state would be *terminal* —
  a session entering it would never be re-classified. Pre-existing, not this
  branch's doing.
- **`append_capped` recomputes `buf.chars().count()` per iteration** (O(n²),
  capped at 200 chars) — negligible, but it is exactly where finding 1's
  control-character filter goes, so fix both in one edit.
- **The `is_agent` gate in `send_input` is verified only by code reading.**
  Confirmed worse than "untested": see finding 9 — removing it fails no test, and
  neither does removing the tick-side gate.
- **The attached view's disconnected title overflows 60 columns in English even
  with an empty name.** Predates the branch and is name-independent; the branch
  correctly stopped adding to it by dropping the name entirely when
  disconnected. `session_title_disconnected` still needs rewording, separately.
- **The no-rename limitation is documented in the README's feature section
  rather than the candid "things that will annoy you" section.** Move it — given
  finding 1, a permanent bad name is the failure mode users actually hit.

---

# Test-suite health

Method: 34 targeted mutations to production code, one at a time, full
`cargo test --lib` after each, reverted with `git checkout -- src/` between.
Isolated `CARGO_TARGET_DIR` (other agents were building in this repo
concurrently). All mutations reverted; `git diff` verified empty afterwards.

Beyond findings 6–9, one structural observation worth internalising:

**The six new naming tests degrade toward false *greens*, not false reds.**

- `recovering_from_a_failure_does_not_count_as_finishing_a_round` needs a tick to
  land inside a 0.2s `Failed` window polled at 50ms. Miss it and `was` stays
  `Working` and the test passes for the wrong reason.
- `recovering_..._after_real_input_still_does_not_count` has the same shape: its
  "10 extra ticks × 20ms" misfire window must close before `cat` starts at ~1.5s,
  or the discriminator (screen tail containing the echo) is gone.
- `a_name_is_pinned_and_never_asked_for_twice` waits 20×50ms and concludes the
  name never changes. Correct today because the `name_slot` guard blocks the
  second call, but the bound is arbitrary.

The test comments do document these windows, which is good practice, but the
consequence is that **load makes these tests weaker rather than noisier** — the
opposite failure mode from finding 10's fixture flake, and much harder to
notice.

Cost: real `git init` + real `git::checkpoint` subprocesses per test, real PTY
children with hard-coded sleeps (`finishing_agent` = `sleep 0.2; echo READY;
sleep 30` ×3 tests; `flaky-name` = `sleep 30`; `flaky-name-2` = `sleep 1; …;
sleep 0.5; cat`). Roughly 2.5s of unavoidable script sleep plus three 5s
deadlines, and the `sleep 30` children outlive the assertions. The branch adds
~6s of PTY/git work to a suite that already flakes under load.

No real network or LLM calls — every backend in the new tests is a fake and
`complete_with_timeout` resolves instantly. No shared global state, no `HOME` or
env mutation, no shared temp dirs; each test gets its own `TempDir` and
`SessionManager`.

Pre-existing failures observed under heavy concurrent load, none introduced by
this branch: `entering_a_session_always_lands_at_the_bottom_even_without_a_resize`
(4×, see finding 10), `busy_pattern_marks_working_then_idle`,
`busy_pattern_wins_over_idle_pattern`, `create_injects_the_secret_into_env`
(once each).

---

# Verdict

**Not mergeable as-is.**

The design is sound — the concurrency, the exactly-once guarantee, the failure
degradation, the protocol decision, and the interaction between the bug fix and
the feature are all correct, and several of them are correct for well-argued
reasons rather than by accident. The two parts genuinely do not interfere.

What blocks merge is small and concrete:

1. **Finding 1** is a real defect the branch introduces in its own documented
   common path, and the fix is a control-character filter plus a blank check —
   perhaps ten lines and a test.
2. **Finding 2** is a visible regression in the grid's core purpose at two
   common terminal widths, unguarded by any test.
3. **Finding 6** — board-list coverage — is the largest hole, and covers the
   surface the README leads with.
4. **Finding 7** — delete or rewrite the fourth non-discriminating test.

Findings 3–5 and 8–10 can all follow in separate work.
