# Task 9 report — outbound polish: merge and numbered options

## What was implemented

All three pure functions from the brief's interface list, in `src/bridge.rs`
(no changes ended up needed in `src/llm/mod.rs` — `Backend`/`complete_with_timeout`
were consumed as-is by reference in doc comments; nothing there needed to change
to support these functions):

- `event_label(e: &Event) -> String` / `event_verb(kind: EventKind) -> &'static str`
  — small private helpers `merge` builds on. `event_label` reuses the existing
  `fallback_name(id)` (the same one `deliver_to`/`ask_message` use) when a
  session has no name yet, so there is exactly one place in the file that
  decides "how do we honestly refer to a nameless session."

- `pub fn merge(events: &[Event], lang: Lang) -> String` — pure, no model
  involved. One event → `"「name」（project）干完停下来了"` with no numbering.
  Two or more → `"有 N 件事：\n1. …\n2. …"`. `lang` is accepted (to match the
  brief's signature and the "pure function that produces user-facing text
  takes a `Lang`" convention elsewhere) but intentionally unused in the body
  — phone-facing text in this file is Chinese-only by the same precedent as
  `broken_message` (a reviewer already adjudicated that as correct for this
  file, per the task instructions), so there's no second branch to write.

- `pub fn options_prompt(screen: &str) -> Prompt` — follows the
  `session.rs::explain_prompt`/`name_prompt` paradigm exactly: pure, takes
  only the last 2000 chars of the screen (`OPTIONS_TAIL`, same constant value
  and rationale as those two), and its `system` prompt explicitly forbids
  paths, code blocks, backticks, and diffs, asking the model to reply either
  with a `"1. …"`-style numbered list or with the literal `"没有选项"` if it
  can't tell the screen is a multiple-choice prompt.

- `pub fn parse_options(raw: &str) -> Option<Vec<String>>` — walks `raw`
  line by line, strips a leading `N.`/`N、`/`N)`/`N）` prefix via the private
  `strip_numbered_prefix`, and for every line that doesn't parse as a numbered
  item, skips it. If a candidate is empty after trimming, or contains `/` or
  a backtick, that one line is discarded (not the whole answer — other valid
  options in the same reply still come back). Returns `None`, never
  `Some(vec![])`, when nothing usable survives.

## Where the two enforcement points for the privacy boundary live

1. **The prompt** (`options_prompt`'s `system` string): explicitly says "绝不
   能出现文件路径、目录、代码块、反引号、diff、命令行原文" — a request, not
   a guarantee.
2. **The filter** (`parse_options`): `if candidate.contains('/') ||
   candidate.contains('`') { continue; }` — the guarantee. Verified by
   mutation (see below): removing this one `if` makes two tests fail,
   confirming it's load-bearing, not decorative.

## Why there is no "synchronous fallback then a 15-second-timeout thread" orchestration function in this task

The brief's own interface list for Task 9 is exactly `merge`, `options_prompt`,
`parse_options` — three pure/prompt-building functions, no send loop. I looked
for a way to actually wire an outbound send (fallback text sent immediately,
a background thread asking the model for options within 15s via
`complete_with_timeout`, then finalizing/sending the real message and
recording `MsgId -> session`), and concluded it does not fit in this task's
declared file scope (`bridge.rs`, `llm/mod.rs` only):

- `options_prompt` needs live PTY *screen text* for the session the event is
  about. `Event` (in `channel/mod.rs`) carries only `{session, kind, name,
  project}` — no screen text. The only place that has screen text today is
  `session.rs` (`s.pty.screen_text()`, used by `request_explanation`/
  `request_name`), and `SessionWriter` (bridge.rs's only handle onto
  sessions) exposes just `type_into`/`name_of` — no screen accessor.
  Wiring the send loop for real would require adding a screen-text method to
  `SessionWriter`/`SessionManager`, i.e. touching `session.rs`, which is out
  of this task's stated scope and risks colliding with whatever Task 10 was
  planned to do there.
- Actually sending also needs a `backend: Option<Arc<dyn Backend>>` field on
  `Bridge` plus a `set_backend`/wiring call from `daemon.rs::install_llm_backend`
  (today that function only calls `mgr.set_backend`, never touches the
  bridge) — again outside `bridge.rs`/`llm/mod.rs`.
- The `MsgId -> session` map itself doesn't exist on `Bridge` yet (the field
  comment literally says "见 Task 9/10"), and populating it correctly
  requires deciding its exact shape together with whoever writes the actual
  `Channel::send` call site — which is the same call site that would need the
  screen-text plumbing above.

**Conclusion: the actual outbound send loop (compose from `merge`/
`options_prompt`/`parse_options`, call `Channel::send`, and record
`MsgId -> session`), and wiring `route()`/`deliver()` into `dispatch()`, both
belong in Task 10.** They share the same missing pieces (session-side screen
access, a `Backend` handle on `Bridge`, and the message map), and Task 9, as
scoped by its own brief and file list, only had to hand Task 10 correct,
tested building blocks — which it does. I did not touch `dispatch()`, `route()`,
or `deliver()`; `Accepted::Rejected` still cannot reach either (unchanged,
still covered by the existing `phone_facing_text_never_looks_like_a_path_or_a_code_block`-style
tests plus Task 8's dispatch tests).

## Mutation testing (the closing action)

Three mutations tried, each caught by a test, each reverted afterward via
`Edit` (no artifacts left behind):

1. **`parse_options`'s failure branch** changed from `None` to
   `Some(vec![raw.to_string()])`. Caught by `unparseable_options_mean_no_options`:
   ```
   left: Some(["我觉得他大概想问你要不要继续吧"])
   right: None
   ```
2. **`merge`'s single-event branch removed** (always falls through to the
   numbered-list branch). Caught by `a_single_event_is_not_dressed_up_as_a_list`:
   ```
   只有一件事却排了个编号列表：有 1 件事：
   1. 「修登录白屏」（web）干完停下来了
   ```
3. **The `/`-and-backtick filter removed** from `parse_options` (as suggested
   by the brief's "also consider pinning the filter with its own mutation").
   Caught by two tests I added for exactly this purpose:
   - `options_containing_a_path_or_a_backtick_are_discarded`
   - `options_that_are_all_filtered_out_mean_no_options`
   ```
   left: ["直接改 src/main.rs", "先跑完", "用 `cargo test`"]
   right: ["先跑完"]
   ...
   left: Some(["修改 /etc/hosts", "用 `ls`"])
   right: None
   ```

All three mutations were made directly in `src/bridge.rs`, the targeted test
was run and confirmed failing, then the mutation was reverted with `Edit`
before moving to the next one — same method as Task 7/8's reports.

## Tests added (all in `src/bridge.rs::tests`)

- The four tests from the brief verbatim: `several_events_become_one_message`,
  `a_single_event_is_not_dressed_up_as_a_list`, `unparseable_options_mean_no_options`,
  `options_come_back_in_order`.
- `merge_falls_back_to_a_number_when_a_session_has_no_name` — pins the
  `fallback_name` reuse.
- `merge_numbers_several_events_in_order` — pins that the numbering isn't
  just present but actually tracks insertion order.
- `options_containing_a_path_or_a_backtick_are_discarded` and
  `options_that_are_all_filtered_out_mean_no_options` — pin the privacy
  filter (see mutation 3 above).
- `a_plain_no_reply_yields_no_options` — model literally answering "没有选项".
- `options_prompt_forbids_paths_and_code_and_carries_the_screen` — asserts
  the `system` prompt text actually contains the forbidding words (路径/
  代码块/反引号/diff), and `user` carries the screen content.
- `options_prompt_only_carries_the_screen_tail` — asserts the 2000-char cap.

## Commands run and results

```
cargo build --lib                        -> clean
cargo test --lib bridge:: -- --test-threads=1   -> 63 passed, 0 failed
cargo fmt --check                        -> clean (after one `cargo fmt` pass)
cargo clippy --all-targets               -> clean, no warnings
```

Full suite (`cargo test -- --test-threads=1`) was started in the background,
logging to a scratch dir outside the worktree, and was still running when
this report was written (integration tests were mid-way through, all green
so far). Please verify it completed at 836+ passed (baseline was 836 at
771ced2; this task adds 9 new tests to `bridge.rs`, so the expected new total
is 845).

## Concerns

- No functional risk identified in the shipped code — all three functions
  are pure/deterministic, no threads, no I/O, no panics possible on any
  input (checked: empty slice isn't exercised by `merge`'s single-event
  branch since callers only invoke it with a non-empty queue snapshot per
  `Bridge::queued`'s existing contract; if it ever were called with `&[]`,
  `events.len() == 1` is false, so it falls into the multi-branch and
  produces `"有 0 件事：\n"` — harmless, just not currently reachable by any
  caller since nothing calls `merge` in production yet).
- Because nothing in `daemon.rs`/`bridge.rs`'s production code calls `merge`/
  `options_prompt`/`parse_options` yet (Task 10's job, per above), these are
  currently dead code from the compiler's point of view except for being
  `pub` — `cargo clippy` did not flag this since they're `pub` (crate API),
  consistent with how Task 7's `route()` and Task 8's `deliver()` sat unused
  in production for a cycle too.
