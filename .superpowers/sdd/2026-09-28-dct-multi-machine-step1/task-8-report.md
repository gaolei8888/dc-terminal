# Task 8 report: my computers on the board, join prompt, message counts

Status: DONE_WITH_CONCERNS

Worktree: /Users/lei/work/dc/dc-terminal-mesh, branch `feat/dct-multi-machine-step1`

Commits (both in English, with no Co-Authored-By line and no AI attribution):
- `145e50f` fix(mesh): clear the in-flight mark when typing panics, neutralise fake markers behind invisible characters
- `b90f849` feat(ui): my computers on the board, join prompt and message counts

`.superpowers/sdd/.gitignore` was already modified in the worktree before I started (the sdd script had rewritten it to `*`). I did not touch it or stage it.

## Part 1: carried fixes from Task 7 (commit 145e50f)

### n1: `in_flight` is now cleared even when typing panics

- **New guard `deliver::InFlight<'a>`** (`src/mesh/deliver.rs`). It is created after the Mesh lock is released and before `type_into` runs.
  - The normal path is `guard.finish(|m| m.finish_typing(..))`. It takes the lock, runs the finish step (which clears the mark), releases the lock, and then disarms the guard.
  - If the guard is dropped without `finish`, `Drop` re-locks, removes the session from `in_flight`, and journals `typing_abandoned session=N`.
  - Poisoned locks are recovered with `into_inner`.
- **Why it cannot deadlock.** The guard only exists while the Mesh lock is not held. Inside `finish`, the `MutexGuard` is a local in an inner block. If `f` panics, that `MutexGuard` is dropped before `self`, so `Drop` never runs while this thread holds the lock.
- **Used on all three typing paths:**
  - `mesh::handle`;
  - `deliver::tick`. It arms a guard for every job it took before typing the first one, so if job 1 panics, jobs 2..n are cleared too. The messages in those later jobs are lost, because they were already popped from the queue.
  - the `is_me` branch of `deliver::send`.
- **Addition beyond the brief.** Without it, the guard would do nothing in production:
  - `link::spawn` wraps the whole `run()` in `catch_unwind`, so a panic in `handle` would end the link thread.
  - The daemon's delivery thread had no `catch_unwind` at all.

  So `mesh::route` now wraps `handle` in `catch_unwind`, and on a panic that envelope gets no answer. The daemon's 1-second delivery loop also wraps `tick` in `catch_unwind`. Each change is a few lines.
- **Tests:**
  - `a_panic_while_typing_an_incoming_message_does_not_wedge_the_session`. It goes through `route`, which returns `Some(None)` without panicking. The mark is cleared, and the next message is `Delivered`.
  - `a_panic_in_a_delivery_tick_clears_every_session_it_took`. Two sessions are queued, and the first one typed panics. Both marks are cleared, and both sessions then get `Delivered`.
  - `a_panic_while_typing_to_this_computer_does_not_wedge_the_session`.

### n2: fake markers behind invisible characters

- **Choice.** I trim before checking, instead of padding every line. Padding every line would change every message's text, for example by indenting code.
- **New `looks_like_marker(line)`.** It trims leading `char::is_whitespace` and Unicode `Cf` characters, then checks `starts_with("[来自")`.
- **New `is_format_char`.** It hard-codes the Unicode 15 `Cf` table: U+00AD, U+0600–0605, U+200B–200F, U+202A–202E, U+2060–2064, U+2066–206F, U+FEFF, U+FFF9–FFFB, the tag characters, and the rest. I did not add a new crate, because the pure-Rust rule applies and this is the only place that needs it.
- **Behaviour change.** A line that was already indented (`"  [来自 …"`) now also gets a padding space. The existing exact-output test `a_fake_marker_in_the_body_or_brackets_in_names_are_neutralised` pinned "indented lines are left alone", so I updated it to `"   [来自 缩进的也垫"`. The extra space does no harm, and the reviewer asked for exactly this check.
- **Tests:**
  - `a_fake_marker_behind_invisible_characters_is_neutralised` pins the exact output for U+200B, U+FEFF, U+2060+U+200D followed by a space, and U+3000. It also checks that a `[来自` that is not at the start of the line is left alone.
  - `a_received_fake_marker_behind_a_zero_width_space_is_neutralised` goes end to end through `receive` and `clean_body`.

## Part 2: Task 8 (commit b90f849)

### Per-file changes

- **`src/proto.rs`**
  - `PROTOCOL_VERSION` 22 → 23, with a doc line.
  - `MeshView` gains `messages: BTreeMap<u32, u32>`. It maps session id to the number of messages typed into that session during this daemon run.
  - Every pinned `22` was bumped.
  - New test `the_mesh_view_shape_is_pinned`. It pins the JSON (`"messages":{"7":2}`) and checks the round trip.
- **`src/mesh/mod.rs`**: new `Mesh.delivered` and `delivered_counts()`.
- **`src/mesh/deliver.rs`**: `finish_typing` increments the count only when the receipt is `Delivered`, which includes `NoCheckpoint`. Queued, refused, and failed messages are not counted.
- **`src/mesh/group.rs`**: `view` fills `messages`.
- **`src/daemon.rs`**: the logged-out `mesh_view` sets `messages` to an empty map.
- **`src/mesh/cli.rs`**: a test fixture gets the new field.
- **`src/ui/computers.rs`** (new; see the deviations section for why)
  - `MeshPanel { view, status_rx, last_fetch, answer_rx, answering }`.
  - **`poll(app, now)`**, called once per main-loop turn next to `live::poll_status`:
    - It collects the approve answer and the status answer.
    - Every 5 s (`POLL_EVERY`) it starts `MeshStatus` on a background thread, using `Client::connect(app.socket)` and `call_within`.
    - It only polls when the view is Board, the app is connected, no status request is already in flight, and no answer is in flight.
    - When the approve answer arrives, any status request that was still in flight is discarded, because its result would be stale.
  - **`handle_key(app, &key)`**, which takes plain `y` or `n` only (no Ctrl, Alt or Meta):
    - With a prompt showing, it sends `MeshApprove { endpoint, code, yes }` for exactly the entry on screen (`pending[0]`, the oldest), on a background thread with a 40 s timeout.
    - While an answer is in flight, the prompt is hidden and y/n are swallowed, so a double press cannot land on the next request, and `n` cannot fall through to "new session".
    - With no prompt it returns false, so `n` is still "new session".
  - When the answer arrives:
    - On success, it replaces the view and shows 「X 已加入」 or 「已拒绝 X」.
    - On an error, it shows the Chinese error through `msg::error` and asks for a fresh status at once.
  - **`prompt_lines`**:
    - The prompt is `msg::mesh_join_prompt`, the brief's sentence verbatim, plus 「还有 N 条」 when more are waiting.
    - It uses the `asking()` theme style (the warm "needs you" colour: amber 179/94, or ANSI yellow when the background is unknown) in bold.
    - It is **wrapped by display width, never truncated**, so the code and `(y/n)` always stay visible. At 80 columns it fits on one line.
    - A second, dim line carries the cross-join hint, truncated to the width: 「一次只加一台新电脑：两台同时加入，可能互相批准进错组」.
  - **`section_lines`**:
    - If no status has arrived yet, nothing is drawn. The code does not guess "not logged in" (for example, with an old daemon).
    - When not logged in, it draws a single dim line: 「多电脑未开启 · 运行 dct login」.
    - Otherwise it draws a dim header 「我的电脑」, then up to 5 lines like `● 家里Mac  本机`, `● 公司Windows  在线`, `○ 云服务器  离线`, ordered this computer first, then online, then offline, then 「还有 N 台」.
    - ● uses the `idle()` theme colour and ○ is dim.
    - Names from other machines go through `session::sanitize` and are truncated to the width.
- **`src/ui/board.rs`**
  - `handle_key` gives y/n to `computers::handle_key` first.
  - `draw` renders the block on its own, then splits the inner area into prompt, list and section (see `split`). When height is short, the prompt comes first, the list keeps at least one row, and the section gets whatever is left.
  - Session rows get a trailing ` ✉ N` in the accent colour. When there are trailing marks (the live mark and/or ✉), the activity column shrinks so the marks stay on screen (`SESSION_PREFIX_COLS`). With no marks, the 70-column activity budget is unchanged.
- **`src/ui/app.rs`**: new field `App.mesh: MeshPanel`.
- **`src/ui/mod.rs`**: adds `mod computers` and the `computers::poll` call. Nothing is added to the draw path.
- **`src/i18n.rs`**: new `mesh_more_requests`, `mesh_cross_join_hint`, `mesh_board_off`, `mesh_board_header`, `mesh_board_state`, `mesh_board_more` and `mesh_messages_mark`. The brief's strings are pinned verbatim in the existing test, and the English strings are checked to contain no Han characters.

### Deviations and why

1. **New module `src/ui/computers.rs`.** The brief lists only board.rs, mod.rs and i18n.rs. The polling state, key handling and line building are about 300 lines and belong together, and the 5 600-line `mod.rs` did not need another block.
2. **Both requests run on background threads.** The brief says "5 s, not in the draw path". The other periodic requests in `mod.rs` are synchronous calls in the main loop, but `MeshStatus` calls `net.peers()` (an HTTP round trip to the relay) and `MeshApprove` broadcasts the roster. Both can block for seconds, so I followed the `pair_start_rx` pattern instead.
3. **`n` declines on the daemon side.** `n` sends `MeshApprove{…, yes:false}` for the shown endpoint and code, which is what `dct peers approve --no` does. No local-dismiss fallback was needed.
4. **Protocol bumped to 23.** Task 7 had not added message counts to `MeshStatus`, so I added the `messages` field.
5. **The prompt uses two lines.** Line 1 is the yellow question, wrapped when needed. Line 2 is the dim hint. On narrow terminals the question wraps onto more lines; the hint is truncated.
6. **The section has a dim 「我的电脑」 header line when logged in.** The brief says to add a 「我的电脑」 section. When not logged in, only the single gray line is shown, as the brief says.
7. **Ordering.** Machines are listed this computer first, then online, then offline, so that with more than 5 machines the hidden ones are the offline ones.

### Mutations (each applied by script, tests run, file restored)

Fix commit:

| Mutation | Result: test that caught it |
|---|---|
| `handle`: guard defused (`mem::forget`) | KILLED: `a_panic_while_typing_an_incoming_message_does_not_wedge_the_session` |
| `route`: no `catch_unwind` | KILLED: same test |
| `tick`: guard created lazily per job (only the job being typed) | KILLED: `a_panic_in_a_delivery_tick_clears_every_session_it_took` |
| `send` `is_me`: guard defused | KILLED: `a_panic_while_typing_to_this_computer_does_not_wedge_the_session` |
| `Drop` does nothing | KILLED: all three panic tests |
| marker: no trim | KILLED: both n2 tests and the updated M2 test |
| marker: trim whitespace only | KILLED: both n2 tests |
| marker: trim Cf only | KILLED: `…behind_invisible_characters…`, the updated M2 test |

The first attempt at the tick mutant was not a real mutant: it still armed every guard up front, and it survived. I rewrote it as the real lazy variant, which was killed.

Feature commit:

| Mutation | Result: test that caught it |
|---|---|
| board does not hand y/n to the prompt | KILLED: y, n, and double-press tests |
| `asking()` shows the newest request | KILLED: `y_on_the_board_approves_exactly_the_request_shown`, n test, `several_pending_joins_are_asked_one_at_a_time` |
| approve sends an empty code | KILLED: y and n tests |
| `n` approves | KILLED: `n_on_the_board_refuses_the_request_shown_and_never_approves` |
| prompt keys taken in every view (in `dispatch_key`) | KILLED: `y_in_the_session_view_sends_no_approval` |
| second press not swallowed | SURVIVED at first. I added the assertion `handle_key(n)` is taken while answering, and it was then KILLED by `a_second_press_while_answering_sends_nothing_more` |
| answering does not hide the prompt | KILLED: y test |
| no 5 s throttle | KILLED: `status_is_asked_every_five_seconds_and_only_on_the_board` |
| poll in every view | KILLED: same test |
| poll while a request is in flight | SURVIVED at first. I added `no_second_status_request_while_one_is_in_flight`, and it was then KILLED |
| not logged in also draws the header | KILLED: `not_logged_in_shows_a_single_gray_line_at_the_bottom` |
| show 6 instead of 5 | KILLED: `more_than_five_computers_say_how_many_more` |
| no ordering | KILLED: `three_computers_are_listed_with_their_state` |
| offline drawn with ● | KILLED: same test |
| no 「还有 N 条」 | KILLED: several-pending test |
| prompt truncated instead of wrapped | KILLED: `narrow_and_tiny_terminals_keep_the_code_and_never_panic`, several-pending test |
| no hint line | KILLED: `a_pending_join_shows_the_confirm_line_under_the_title` |
| prompt not in the warm colour | KILLED: same test |
| activity keeps 70 columns when there are marks | KILLED: `a_session_that_got_messages_shows_the_count_at_the_end_of_its_row` |
| `messages_for` always 0 | KILLED: same test |
| draws 「多电脑未开启」 before the first answer | SURVIVED at first, because my first version of `nothing_is_drawn_before_the_first_answer` compared two identical renders. I rewrote the test, and it was then KILLED |
| the list is not guaranteed one row (section wins) | SURVIVED at first. I added an 80×5 assertion to the narrow test, and it was then KILLED |
| count every receipt | KILLED: `the_status_view_counts_messages_typed_into_each_session` |
| never count | KILLED: same test |
| view carries no counts | KILLED: same test |

**Not covered by a test:** dropping the stale in-flight status request when the approve answer arrives.

### Test list (new)

- **`ui::board`:**
  - `not_logged_in_shows_a_single_gray_line_at_the_bottom`
  - `nothing_is_drawn_before_the_first_answer`
  - `three_computers_are_listed_with_their_state`
  - `more_than_five_computers_say_how_many_more`
  - `a_pending_join_shows_the_confirm_line_under_the_title`
  - `several_pending_joins_are_asked_one_at_a_time`
  - `narrow_and_tiny_terminals_keep_the_code_and_never_panic`, which covers 80×24, 30×20, 30×10, 30×3, 10×5, 1×1, 2×2, 0×0 and 80×5
  - `a_session_that_got_messages_shows_the_count_at_the_end_of_its_row`
- **`ui::computers`** (fake daemon on `app.socket`, one thread per connection):
  - `y_on_the_board_approves_exactly_the_request_shown`
  - `n_on_the_board_refuses_the_request_shown_and_never_approves`
  - `a_second_press_while_answering_sends_nothing_more`
  - `y_in_the_session_view_sends_no_approval`: it also checks that the y went to the session as `Input`
  - `without_a_request_or_with_ctrl_the_keys_are_not_taken`
  - `status_is_asked_every_five_seconds_and_only_on_the_board`
  - `no_second_status_request_while_one_is_in_flight`
  - `wrapping_counts_wide_characters_as_two_columns`
- **`mesh::deliver`:** `the_status_view_counts_messages_typed_into_each_session`, plus the n1/n2 tests above.
- **`proto`:** `the_mesh_view_shape_is_pinned`.

## Commands (run in /Users/lei/work/dc/dc-terminal-mesh)

```
~/.cargo/bin/cargo test --workspace   -> passed 1893 failed 0 ignored 3
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings   -> clean
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
    -> Finished; only the existing warning (unused `path`, src/student_projects.rs:669)
git diff --check   -> clean
```

The fix commit alone was checked the same way: 1875 passed, clippy clean, and the Windows check showed only the old warning.

`rustfmt` was run only on files that were already rustfmt-clean: `src/ui/board.rs`, `src/ui/app.rs`, `src/ui/computers.rs`, `src/mesh/group.rs`, `src/mesh/cli.rs` and `src/mesh/deliver.rs`.

## Concerns

- **Grid mode users never see the join prompt or the computers section.** The brief scopes this to the board (list) view only. Anyone whose saved view mode is the grid would have to switch with `g` to see a pending join; `dct peers` still shows it.
- **`n` on the board is overloaded.** While a join prompt is showing, `n` declines it instead of opening a new session. That is what the brief specifies, and a decline can be recovered (the joiner simply retries), but it may surprise someone.
- **Messages lost after a panic.** Queued messages that `tick` popped in a pass where a panic happened are lost, and those senders were already told `Queued`. The sessions themselves are no longer wedged.
- **Untested wrap.** The daemon's `catch_unwind` around the delivery tick has no unit test, because the daemon thread itself has none.
- **Message counts are only as fresh as the 5 s poll.** They live in the `Mesh`, so they reset when the daemon restarts, which matches "本次 dct 运行期间" at the daemon level.
- **Behaviour change in the existing marker test.** An already indented fake marker line now also gets a padding space.

## Fix round 1 (commit 0baa295)

Status: DONE_WITH_CONCERNS. Worktree /Users/lei/work/dc/dc-terminal-mesh. The commit message is in English and has no Co-Authored-By line.

### I1: people who use the grid view now see a waiting computer

- **Polling.** `computers::poll` now also runs in `View::Grid`.
- **Notice.** `grid::draw` reserves lines at the top for `computers::grid_notice_lines` when a request is waiting. The notice is warm-coloured (`asking()`, bold) and wrapped by display width, never truncated. At 80 columns it takes 2 lines. The grid gets the remaining area.
- **Text.** It is `msg::mesh_grid_notice`:
  - zh: 「有电脑想加入「我的电脑」。按 g 切到看板确认（g 会把默认视图也换成看板，确认完再按 g 换回九宫格）」.
  - I checked the real key. In the grid, `g` is `toggle_view_mode`, which also saves the mode. The text says so rather than hiding it.
- **Keys.** y/n remain board-only. `grid::handle_key` is unchanged.
- **Tests:**
  - `the_grid_tells_you_to_go_to_the_board_when_a_computer_waits`: the notice is absent with no request; with a request, the first two rows are exactly the notice; no panic at 30×10, 10×3, 1×1 or 0×0.
  - `status_is_asked_in_the_grid_too`.
  - `y_and_n_in_the_grid_send_no_approval`.

### M1: settle delay

- **`MeshPanel::set_view(v, now)`** is now the only way the view is replaced (both the status path and the answer path use it).
  - It records the identity (endpoint, code) of `pending[0]`.
  - It restarts the clock only when that identity changes, including when the entry first appears.
- **`handle_key_at(app, key, now)`** treats y/n as not taken until `SETTLE` (500 ms) has passed.
  - "Not taken" means y does nothing and n stays "new session", so an `n` pressed as "new session" cannot decline a join the user never saw.
  - If `view` was replaced without going through `set_view`, the identity does not match and the key is never taken.
  - Production calls `handle_key`, which passes `Instant::now()`.
- **Test** `a_request_that_just_appeared_or_changed_is_not_answered_yet`. It uses explicit instants, with no sleeping on the logic. It checks:
  - a request that just appeared is refused;
  - a same-identity refresh does not restart the clock;
  - after an A→B swap, a press just after the swap does nothing, and after `SETTLE` it sends B's endpoint and code;
  - a view replaced without `set_view` is refused.

### M2: `mesh_panic` journal lines

- `route()` journals `mesh_panic where=route from=<endpoint>` when it catches a panic.
- New `deliver::tick_catching`, which the daemon's delivery thread now calls, journals `mesh_panic where=tick`.
- Both are asserted in the n1 panic tests. The tick test now goes through `tick_catching`.

### M4: remote names drop format characters

- `computers::clean` = `sanitize` plus a filter on `deliver::is_format_char`, which is now `pub(crate)`.
- It is used for the prompt name and for the machine names in the section.
- **Test** `format_characters_in_a_remote_name_never_reach_the_screen` uses U+202E, U+200B and U+2066.

### M6: a late answer while in the session view

- `say()` does not set `app.message` when the view is `Attached`. The message is dropped (the simplest option), but the fresh view is still applied.
- **Test** `a_late_answer_does_not_talk_over_the_session_view`.

### Mutations (each applied, tests run, file restored)

| Mutation | Result: test that caught it |
|---|---|
| no poll in the grid | KILLED: `status_is_asked_in_the_grid_too` |
| grid notice never drawn | KILLED: `the_grid_tells_you_to_go_to_the_board…` |
| grid notice drawn without a request | KILLED: same test |
| grid notice truncated instead of wrapped | KILLED: same test |
| grid takes y/n (`computers::handle_key` in `grid::handle_key`) | KILLED: `y_and_n_in_the_grid_send_no_approval` |
| `settled` check skipped | KILLED: `a_request_that_just_appeared_or_changed_is_not_answered_yet` |
| `set_view` always restarts the clock | KILLED: same test |
| `settled` ignores identity | KILLED: same test |
| `clean` keeps Cf characters | KILLED: `format_characters_in_a_remote_name_never_reach_the_screen` |
| late answer overwrites the session message | KILLED: `a_late_answer_does_not_talk_over_the_session_view` |
| route panic not journaled | KILLED: `a_panic_while_typing_an_incoming_message_does_not_wedge_the_session` |
| tick panic not journaled | KILLED: `a_panic_in_a_delivery_tick_clears_every_session_it_took` |

### Commands

```
cargo test --workspace   -> passed 1899 failed 0 ignored 3
cargo clippy --workspace --all-targets -- -D warnings   -> clean
cargo check --workspace --all-targets --target x86_64-pc-windows-msvc -> only the existing student_projects.rs:669 warning
git diff --check -> clean
```

`rustfmt` was applied to `src/ui/computers.rs`, `src/mesh/deliver.rs` and `src/ui/grid.rs`. All three were rustfmt-clean before this change.

### Known and left as is (ruling)

M3, M5, M7 and M8 from the review are unchanged.

### Remaining concerns

- **Pressing g changes the saved default.** The only way from the grid to the board is `g`, which also saves the board as the default view. The notice says so, but a user who confirms a join ends up in board mode until they press `g` again.
- **The daemon side of the delivery thread has no unit test.** The daemon now only calls `tick_catching`, which is tested.

## Fix round 2 (commit 35923b7)

Status: DONE_WITH_CONCERNS. English commit message, no Co-Authored-By line.

### The regression: the grid notice pushed the tiles below MIN_ROWS

- `grid::draw` now takes rows away from the tiles for the notice **only if at least `MIN_ROWS` remain**.
- Otherwise the tiles are drawn on the full area, and the notice is then drawn on top of the top `nh` rows, using `Clear` and then a `Paragraph`. This follows the reasoning of `draw_reply`.
- **Cost in the tight case:** the notice covers the first row of tiles' top border and title line (at 80 wide with 4 sessions, the names of tiles 1 and 2). Every tile is still rendered, and the second row is fully visible.
- With the test app, `ui::draw` at 80×24 gives the grid 22 rows, so there the notice is sliced off and every title stays. The reviewer's 20-row case is covered by calling `grid::draw` directly at 80×20 and 80×21.

### The small extra fix (a few lines)

The notice is not shown while the grid reply box is open. At that point `g` types into the box, so "press g" would be wrong.

### Tests

- **`the_grid_notice_keeps_the_tiles_on_an_80_by_24_terminal`**:
  - The full `ui::draw` at 80×24 with a pending request shows the full notice and tiles `tile2` and `tile4`, and does not show 「窗口太小」.
  - `grid::draw` at 80×20 and 80×21 (the overlay path) shows the notice and `tile3` and `tile4`, with no 「窗口太小」.
  - 80×40 shows the notice and all tiles.
- **`the_grid_notice_is_not_shown_while_the_reply_box_is_open`.**
- **The tiny-size no-panic loop is kept** in `the_grid_tells_you_to_go_to_the_board_when_a_computer_waits`: 30×10, 10×3, 1×1 and 0×0.

### Mutations

| Mutation | Result: test that caught it |
|---|---|
| always slice (the round-1 behaviour) | KILLED: `…80_by_24_terminal` (80×20/21 turn into 「窗口太小」) |
| always overlay | KILLED: `…80_by_24_terminal` (at 80×24 the first row's titles would be covered when there is room to slice) |
| notice drawn before the grid, so the grid paints over it | KILLED: `…80_by_24_terminal`, `the_grid_tells_you_to_go_to_the_board…` |
| notice shown with the reply box open | KILLED: `the_grid_notice_is_not_shown_while_the_reply_box_is_open` |

At first, "always slice" survived, because the full-screen test only exercised a 22-row grid. The direct 80×20 and 80×21 checks were added for that reason.

### Commands

```
cargo test --workspace   -> passed 1901 failed 0 ignored 3
cargo clippy --workspace --all-targets -- -D warnings   -> clean
cargo check --workspace --all-targets --target x86_64-pc-windows-msvc -> only the existing student_projects.rs:669 warning
git diff --check -> clean
```

### Concerns

- **Covered tile titles.** In the tight case (grid content area under `MIN_ROWS + 2`), the notice covers the first row of tiles' titles while a join is pending. The alternative would be to overlay at the bottom, but the ruling said the notice goes at the top.
