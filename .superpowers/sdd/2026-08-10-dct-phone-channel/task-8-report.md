# Task 8 report — 入站落地：敲进 PTY、回执、journal

## What was implemented

In `src/bridge.rs` (after `route()`, before `struct Bridge`):

- `trait SessionWriter: Send + Sync` — `type_into(&self, id, text) -> Result<(), String>` and
  `name_of(&self, id) -> Option<String>`. The capability `deliver()` needs: type into a
  session's PTY, and resolve an id to a name a human can read.
- `impl SessionWriter for crate::session::SessionManager` — `type_into` calls the existing
  `send_input(id, text)` (mapping the `anyhow::Error` to `.to_string()`); `name_of` calls
  `list()`, finds the matching `SessionInfo`, and applies the same fallback rule
  `SessionInfo.tag` already documents (`tag` if non-empty, else `profile`). This is the answer
  to Ruling 7 — see below.
- `Delivered` enum exactly as specified: `Typed(u32)`, `AskedWhich(Vec<u32>)`, `SaidGone`,
  `SaidNeedUse`, `Failed(String)`.
- `fallback_name(id) -> String` — `"{id} 号会话"`, used only when a session id has no name
  available (writer not wired, or the session vanished between `route()` deciding and
  `deliver()` acting).

New `Bridge` fields:
- `writer: Mutex<Option<Arc<dyn SessionWriter>>>` — `None` by default (set via `set_writer`).
- `journal: Journal` (from `crate::journal`) — unset path by default (writes nothing), set via
  `set_journal_path`.

New `Bridge` methods: `set_writer`, `set_journal_path`, `deliver(&self, route, text) -> Delivered`,
`deliver_to` (private, the `To` branch), `ask_message` (private, builds the candidate-list text),
`reply` (private, sends to the current owner via `self.ch.send`, swallowing `Channel` errors).

In `src/journal.rs`: added `Delivery` enum (`Typed(u32)`, `Failed(u32)`, `Asked(usize)`, `Gone`,
`NeedUse`) with a `Display` impl, and `Journal::delivered(&self, d: Delivery)` which writes
`"inbound  {d}"` through the existing `write()` (same truncate-at-256KB, same swallow-all-IO-errors
path as `born`/`died`). **Deliberately does not take or log the message text** — only the outcome.

## Ruling 7 — how id → name resolution works, and why

`SessionManager::list()` already exists (Task 6/earlier) and returns `Vec<SessionInfo>`, each
carrying `id`, `profile`, and `tag` (the auto-generated name, empty string if not yet named —
`SessionInfo.tag`'s own doc comment says "界面遇到空串一律退回 profile"). I reused that exact
rule rather than inventing a new one: `SessionWriter::name_of` for `SessionManager` is
`list().find(|s| s.id == id).map(|s| if s.tag.is_empty() { s.profile } else { s.tag })`. This
keeps "how a session is named to a human" defined in exactly one place (`SessionInfo`'s
fallback convention), rather than duplicating a second name-resolution rule in the phone path
that could drift from what `dct ps`/the board show.

I introduced the `SessionWriter` trait (rather than depending on `SessionManager` directly in
`Bridge`) for the same reason `Channel` is a trait: `bridge.rs`'s existing tests never touch a
real PTY or network, and the brief's `for_test_with_writer()` needed a pure in-memory double.
`Bridge` holds it as `Mutex<Option<Arc<dyn SessionWriter>>>`, injected post-construction via
`set_writer()`, rather than as a required `Bridge::new` constructor argument. That was a
deliberate scope decision: `Bridge::new`/`spawn`/`replace` already have ~19 call sites (mostly
in `bridge.rs`'s own pre-existing tests, plus 4 in `daemon.rs`), none of which this task's file
list (`bridge.rs`, `journal.rs`) authorizes touching. A setter keeps every existing call site
compiling untouched, matches the `Option::None`-means-"not wired yet" pattern the file already
uses in a few places (e.g. `owner: Option<i64>` in `Bridge::new`), and leaves the actual
`daemon.rs` wiring (`bridge_handle.bridge.set_writer(mgr.clone())`,
`.set_journal_path(same path as mgr.journal)`) as a one-line follow-up for whichever task does
that (see Concerns).

`set_journal_path` follows the same shape, and for the same reason journals should share a
path with `SessionManager::journal`: the module's whole reason to exist is correlating "a
session died" with "why," and a message typed into a session from the phone belongs on that
same timeline.

## Carried ⚠️ — verifying `route()`/`deliver()` are unreachable except from `accept()`'s `FromOwner`/`Paired` path

I checked this by grep, not by reasoning about intent: `grep -n "route(\|\.deliver(" src/bridge.rs
src/daemon.rs` shows every non-doc-comment call to `route()` and `deliver()` is inside
`#[cfg(test)] mod tests`. Neither `dispatch()` nor `run()` calls either function, and
`daemon.rs` has zero references to `Route`, `Delivered`, or `deliver` at all.

This is intentional, not an oversight I'm flagging as incomplete: building a real
`RouteInput` requires state (`MsgId → session id` map, `/use` state, the waiting-session set)
that does not exist in `Bridge` yet. Task 7's own report says so explicitly ("Did not touch
`Bridge`'s internal state ... those don't exist in `Bridge` yet"), and the plan document
(`docs/superpowers/plans/2026-08-10-dct-phone-channel.md` line ~1434) places that state's
construction in **Task 9** ("合并" — merging the message map) and **Task 10** ("`narrow` 只作用于
`Ask`" — narrowing candidates), both after this one. Wiring `dispatch()` to call `route()` with
today's empty state would mean every real reply from the paired owner immediately resolves to
`Gone` or `NeedUse` — a materially wrong (and untested-for-the-real-shape) behavior shipped to
users before the state that makes routing meaningful exists. So the invariant holds today for
the simplest possible reason: **there is no call path from any input into `route()`/`deliver()`
at all outside test code** — a rejected stranger's message, an owner's message, and a
between-restart backlog message are all equally unable to reach them right now. When Task 9/10
does add the message-map/`/use`/waiting state and wires `dispatch()` → `route()` → `deliver()`
for real, the single correct hook point is inside `dispatch()`'s `if let Accepted::Paired(...) =
...` / the `FromOwner` arm — i.e. strictly downstream of `accept()`'s verdict, never on
`Accepted::Rejected`. I did not add that wiring myself (out of this task's scope per the file
list and per the state genuinely not existing yet), but I verified there is nothing today that
could accidentally create a shortcut around `accept()`.

## What each route writes / does not write

| Route | Types into a session? | Sends a phone reply? | Journals? |
|---|---|---|---|
| `To(id)` (success) | yes, via `writer.type_into(id, text)` | yes — `已经敲进「{name}」` | `Delivery::Typed(id)` |
| `To(id)` (writer errors, or no writer wired) | **no** | yes — a plain "didn't work, try again" sentence, never the raw error string | `Delivery::Failed(id)` |
| `Ask(ids)` | **no** | yes — lists candidates by name where known, else by number, asks for a reply-to-a-push or `/use` | `Delivery::Asked(ids.len())` |
| `Gone` | **no** | yes — "这条消息对应的会话已经不在了...先发 /ls 看看现在有哪些会话" (uses the brief's exact required phrase) | `Delivery::Gone` |
| `NeedUse` | **no** | yes — "先发 /ls 看看有哪些会话...或者发 /use 加编号指定一个" | `Delivery::NeedUse` |

Journal entries never contain the message text (`Delivery`'s doc comment states this
explicitly, and `delivered_records_the_outcome_but_never_the_message_text` pins it) — only
session id / candidate count / outcome kind, mirroring `Death`'s existing "record what
happened, not the payload" shape.

Phone-facing replies never contain the raw `writer.type_into` error string (`Err(_)` branch
discards it and substitutes a fixed human sentence) and never contain a fabricated session name
(`fallback_name` is only used when `name_of` returns `None`).

## Language

Text is hardcoded Chinese, matching the existing precedent already merged in this exact file:
`broken_message()` (used by `mark_broken`, i.e. the phone-facing "手机通知的令牌不能用了..."
messages from Task 4/5) is Chinese-only prose, not routed through `i18n.rs`'s bilingual
`Key`/`Lang` machinery, and is tested only for containing Han characters, not for having an
English counterpart. `deliver()` has no `Lang` parameter available (the brief's fixed call shape
is `deliver(&self, route, text)`), and nothing in `daemon.rs`'s existing non-test code ever
picks a `Lang` on the daemon's behalf — there's an explicit test,
`profiles_are_labelled_in_the_language_the_client_asked_for`, whose comment says the daemon
"不该替用户决定语言" for client request/response text, specifically because a request carries
its own `lang`. The phone channel has no request to carry one. Given the existing merged
precedent in this file is Chinese-only for exactly this audience (phone/Telegram messages), I
followed it rather than inventing a bilingual phone-text scheme unreviewed by any earlier task.
**Flagging this as a judgment call**, not a certainty — see Concerns.

## Privacy boundary

`phone_facing_text_never_looks_like_a_path_or_a_code_block` asserts none of the four
routes' phone replies contain a triple-backtick or a newline. None of the four hardcoded
messages contain a path or diff by construction; the only variable content is the session name
(`writer.name_of`, an LLM-generated short title like "修登录白屏", not a path) and, for `Ask`,
session ids. The user's own typed `text` is echoed only into the PTY (`writer.type_into`), never
back into a phone reply — the receipt says *where* it went, not an echo of *what* was sent.

## Mutations tried (both prescribed, both caught)

Performed by hand-editing `src/bridge.rs` in place, saving a known-good copy first
(`bridge.rs.orig` in the scratchpad dir), running the targeted tests, confirming failure, then
restoring and re-verifying full pass + `cargo fmt --check` + clippy clean.

1. **`Gone` branch also calls `type_into(0, text)`** before replying:
   ```
   test bridge::tests::a_gone_route_writes_nothing_at_all ... FAILED
     panicked: "旧消息被敲进了会话"
   test bridge::tests::mutation_guard_gone_must_never_call_type_into ... FAILED
     panicked: "Gone 分支一旦调用了 type_into，这里就会看到写入记录"
   ```
   Both the brief's required test and my added pinning test failed, as specified.

2. **Receipt session name replaced with `id.to_string()`** (`let name = id.to_string();`
   instead of `writer.name_of(id).unwrap_or_else(...)`):
   ```
   test bridge::tests::typing_it_in_sends_a_receipt_naming_the_session ... FAILED
     panicked: "回执里没说敲给了谁"
   test bridge::tests::mutation_guard_receipt_must_name_the_session_not_just_the_number ... FAILED
     panicked: "回执必须点名会话叫什么：已经敲进「7」"
   ```
   Both failed, as specified.

Both mutations were reverted; `diff` against the saved pre-mutation copy confirmed the restored
file is byte-identical.

### Extra pinning beyond the brief's mutations

Per the global constraint ("consider what else deserves pinning: in particular that `Ask` and
`NeedUse` also write nothing"), added:
- `a_need_use_route_writes_nothing_at_all` — the brief's three tests only cover `To`/`Gone`/`Ask`;
  `NeedUse` was the one route left unpinned.
- `gone_ask_and_need_use_all_still_reply_something` — the flip side of "writes nothing": all
  three no-op-on-the-PTY routes must still produce a phone reply, or the user gets silence
  indistinguishable from a lost message.
- `a_write_failure_is_reported_honestly_not_swallowed` and `deliver_to_without_a_writer_fails_honestly`
  — pin `Delivered::Failed` for both ways `To` can fail (writer errors; writer never wired).
- `all_four_routes_are_journaled` — pins "全部记 journal" against a real temp-file `Journal`,
  not just checking the `Delivery` enum shape.
- `mutation_guard_gone_must_never_call_type_into` / `mutation_guard_receipt_must_name_the_session_not_just_the_number`
  — permanent guards for the two prescribed mutations, so a future regression is caught by CI
  without anyone re-running the manual mutation by hand.

## Commands run and results

```
cargo build --lib                                          → clean
cargo test --lib bridge:: -- --test-threads=1               → 52 passed, 0 failed
cargo test --lib journal -- --test-threads=1                 → 7 passed, 0 failed
cargo fmt --check                                            → clean (after one `cargo fmt` run)
cargo clippy --all-targets                                   → clean, no warnings
```

Baseline was 824 tests at 7575b73. This task adds 6 tests to `journal.rs` (net +1, since 6 total
minus... actually: journal went from 6 to 7 tests, +1) and bridge went from 41 to 52 tests
(+11). Total added: 12. Expected full-suite count: 836 passed, 0 failed.

The full suite (`cargo test -- --test-threads=1`) was started in the background
(`full_test_task8.log` in the worktree root) and was still running when this report was
written, per instructions not to wait on it.

## Concerns

1. **Production wiring is not done in this task**, by design (see Ruling 7 section above):
   `daemon.rs` never calls `Bridge::set_writer` or `Bridge::set_journal_path`, so `deliver()` is
   fully implemented and tested but dead in production until some future task calls
   `bridge_handle.bridge.set_writer(Arc::clone(&mgr) as Arc<dyn SessionWriter>)` (this requires a
   way to reach the inner `Bridge` from `BridgeHandle`, which today only exposes `stop`/`unpair`/
   `accept` — that accessor doesn't exist yet either) and `.set_journal_path(...)` with the same
   path used by `SessionManager::journal`. `impl SessionWriter for SessionManager` is ready and
   tested via the trait's contract (through `Spy`, not through a real `SessionManager` in this
   task's tests — there's no test in `bridge.rs` that exercises the real
   `SessionManager`-backed impl end-to-end, since doing so would require `session.rs` test
   scaffolding out of this task's file scope).
2. **`route()`/`deliver()` remain called only from tests**, confirmed by grep (see the carried
   ⚠️ section) — this is expected given Task 9/10 haven't landed the state `RouteInput` needs,
   but means the phone reply/journal behavior described here isn't reachable by a real Telegram
   message yet.
3. **Chinese-only phone text is a judgment call**, not a certainty — see the Language section.
   If a future review wants bilingual phone text, the four hardcoded strings in
   `deliver`/`deliver_to`/`ask_message` are the only places to change, plus deciding where a
   `Lang` would come from for a push-style channel that has no per-request client to ask.
