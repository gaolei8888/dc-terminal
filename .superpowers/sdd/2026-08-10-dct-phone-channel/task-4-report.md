# Task 4 report: protocol, token storage, phone notification page

**Status:** complete. Commits on `feat/phone-channel` (base `adacd36`):
- `5754a82` — phone status protocol, token storage, daemon-side state
- `01eb34c` — the phone page itself (`ui/phone.rs`), settings wiring
- `0708163` — daemon/mod.rs wiring closed out, mutation-driven fixes

The run that produced these three commits was interrupted once by an API
connection error partway through wiring `mod.rs` (between commits 1 and 2,
the tree briefly did not compile — two `todo!()`-shaped holes in the draw/key
dispatch). The coordinator's resumption message is accurate; I picked up
exactly where it said and finished from there. Nothing was redone or lost.

## Brief inaccuracies found, and what I did about them

### 1. `Request::PhoneSetToken` needed `lang`, which the brief's interface line omits

The brief's Interfaces line lists `PhoneSetToken { token }`. But
`PhoneState::Broken(String)` is defined (by the brief itself, verbatim) to
carry **already-composed human text**, not a code — and composing a correct
sentence requires knowing which language to write it in. The daemon is a
long-lived process that doesn't know the UI's language on its own; the one
place it *can* know is a request that tells it, exactly like
`Request::Profiles { lang }` already does (and exactly the convention
`proto.rs`'s own `SecretPrompt` doc comment names: "组句发生在哪一侧必须
一致... daemon 端已经知道用户语言"). Without `lang`, `PhoneSetToken` would
have no way to pick between "这个令牌用不了..." and its English
equivalent when composing the `Broken` message.

I added `lang: crate::i18n::Lang` to the request, updated the protocol
pinning test (`the_request_shape_is_pinned_to_the_protocol_version`), and
redacted `token` (not `lang`) in the hand-written `Debug` impl, mirroring
`SetSecret`/`VerifySecret`.

### 2. `View::Phone { status: PhoneStatus }` is missing the field that makes `Enter` (fill in token) possible

The brief's Produces line gives `View::Phone` exactly one field. But Step 3
also asks for `handle_key()` with `Enter` 填令牌 (fill in token) — and there
is nowhere in a single `status: PhoneStatus` field to hold what the user is
typing, or which phase (typing / verifying / failed) that input is in. This
is the same class of gap Task 3's own report flagged for `View::Settings`
(which needed a `lang: Option<ListState>` field beyond what its brief
specified, for the identical reason: one view needs to distinguish two
layers, and a single flat field can't hold that).

I added:
```rust
Phone {
    status: PhoneStatus,
    entry: Option<PhoneEntry>,  // Some = filling in a token; None = just watching status
},
```
with `PhoneEntry { buf: String, phase: SecretPhase }`, reusing `SecretPhase`
verbatim per the brief's own instruction ("复用 EnterSecret 的
SecretPhase::Verifying 反馈").

## The 403 case ("bot was blocked by the user")

**What I did NOT need to do:** touch `channel/telegram.rs`. Reading it before
writing anything, `error_from` already maps both 401 and 403 to
`ChannelError::BadToken`, and a test (`a_403_is_also_bad_token_not_unreachable`)
already pins that mapping — this was already in place when I started Task 4
(per the standing context, Task 2 left it there deliberately: the channel
layer's job is "worth retrying or not," not "what sentence to show," and both
401 and 403 are equally "not worth retrying").

**Why the 403/blocked case cannot actually arise in Task 4's own code path:**
`getMe` (what `PhoneSetToken` calls to validate a freshly-typed token) has no
chat context — it just asks "is this token valid," and Telegram has no reason
to return "Forbidden: bot was blocked by the user" for it. That error is
specific to `sendMessage`/`getUpdates`-style calls that target a chat — which
only happen once a chat is paired, i.e. inside Task 5's Bridge, not here. So
the *only* Broken case Task 4 exercises is "token invalid," and
`daemon.rs::phone_verify_token` composes exactly that
(`i18n::msg::phone_token_invalid`), never `phone_unreachable` or anything
resembling a "blocked" message, for a `BadToken` result.

**What I built so the eventual blocked-bot case (Task 5's job) isn't
foreclosed, and what the user sees today:**
- `PhoneState::Broken(String)` is architected as an **opaque** payload as far
  as the two functions that render the page's headline are concerned.
  `ui/phone.rs::status_line`/`next_step` never read the string inside
  `Broken` — not even to decide phrasing. This is enforced by a real test
  (`the_token_never_appears_in_any_status_text`, from the brief, plus my own
  `daemon::phone_broken_text_never_contains_the_token`) and means the daemon
  is free to compose *any* text for *any* future Broken cause (including a
  "you blocked this bot" message Task 5 will add) without the UI needing to
  change.
- `next_step` for any `Broken` state returns one fixed, honest sentence that
  covers **both** real causes at once, since the compact status line
  deliberately can't (and, given `ChannelError::BadToken` erasing the
  401/403 distinction by design, structurally shouldn't try to) tell which
  one actually happened: *"如果是令牌本身失效了，按 Enter 重新填一遍；如果
  是你在 Telegram 里把这个机器人拉黑了，解除拉黑后按 r 重新配对"* (en: "if
  the token itself stopped working, press Enter to type a new one; if you
  blocked this bot in Telegram, unblock it and press r to pair again"). Both
  actions it names are real and both work today: `Enter` re-opens the token
  entry, `r` sends `Request::PhoneUnpair` (Off is excluded from both).
- The daemon's own composed reason (for now, always "令牌用不了...") is not
  thrown away — it's shown as a separate detail line in `draw()`, styled red,
  below the generic headline and next-step. `draw_shows_the_broken_reason_as_a_detail_line`
  covers that it actually renders.
- I did **not** invent a `phone_blocked_by_user` message function, because
  Task 4 has no call site that would ever construct it — that would be
  guessing at Task 5's Bridge/send-path wiring I haven't seen. `i18n.rs`'s
  `msg::phone_token_invalid`/`msg::phone_unreachable` are the two the daemon
  actually uses; Task 5 adds its own when its `send()` failure path needs one,
  using the same `PhoneState::Broken(String)` shape and the same
  never-echoed-verbatim UI contract.

## `Telegram::get_me` reachability

Reached directly, no trait change needed. `daemon.rs`'s `PhoneSetToken`
handler (via the extracted `apply_phone_set_token`) calls
`Telegram::new(token).get_me()` straight from a fresh, short-lived `Telegram`
built for that one verification — it never holds an `Arc<dyn Channel>`. The
long-lived, shared `Channel` object (the one that would need `get_me` on the
trait to be reachable through it) is the Bridge's, and the Bridge doesn't
exist yet — that's Task 5. So the brief's warning ("if you find yourself
holding an `Arc<dyn Channel>` and unable to reach it...") never triggered;
Task 4's own code never needed to hold one. `phone_verify_token`'s transport
is injected as `&dyn Fn(&str) -> Result<String, ChannelError>` (same pattern
as `verify.rs::verify_with`), letting the decision logic be tested with a
fake closure while the real path (`|t| Telegram::new(t).get_me()`) stays
untested at the unit level — deliberately, matching the project's existing
line for real-transport code (`send_real`, `verify.rs::send_probe`).

## What I built beyond the brief's two Step-5 mutations, and why

Working through mutation testing surfaced a real design gap: the "can `Enter`/
`r`/`x` do anything right now" rule was written twice — once as a guard in
`phone::handle_status`, once as the advertising condition in
`view::idle_help`. A disconnected test `App` (`App::test_app()`) can't tell
"the guard blocked the call" from "the call ran and failed anyway, so the
screen looks the same" — both produce an unchanged `PhoneStatus`. I extracted
both copies into one pure function, `view::phone_key_has_effect(state, code)`,
used by both call sites, and backed it with a truth-table test plus two
real-daemon integration tests that only a genuine call-vs-hardcode
distinction could pass. Same story for the async-result race in the
token-verify flow: extracted `view::phone_verify_outcome_applies_to`,
mirroring the existing `verify_outcome_applies_to` for `EnterSecret`.

## Mutation table

Brief-named (Step 5), both against `ui/phone.rs`:

| # | Mutation | Test that should catch it | Result |
|---|---|---|---|
| 1 | `next_step`'s `Paired` branch changed to also return `Some(...)` | `every_state_tells_the_user_what_to_do_next` (last assertion) | **RED** — caught |
| 2 | `status_line`'s `WaitingForPairing` branch: drop the bot-name interpolation | `waiting_names_the_bot` | **RED** — caught |

Mine, against the logic Task 4 added beyond those two:

| # | Mutation | Where | Test | Result |
|---|---|---|---|---|
| 3 | `phone_verify_token`: `BadToken` composes `phone_unreachable` instead of `phone_token_invalid` | `daemon.rs` | `bad_token_and_network_trouble_produce_different_messages` | **RED** |
| 4 | `initial_phone_status`: flip the has-token ternary | `daemon.rs` | `initial_phone_status_is_off_without_a_saved_token` + `..._is_waiting_for_pairing_with_a_saved_token` | **RED** (both) |
| 5 | `apply_phone_set_token`: drop the "don't save on Broken" guard, always save | `daemon.rs` | `apply_phone_set_token_does_not_save_a_bad_token` | **RED** |
| 6 | `PhoneUnpair`: drop the `Off`-stays-`Off` guard | `daemon.rs` | `phone_unpair_on_off_stays_off` | **RED** |
| 7 | `PhoneDisable`: skip `secrets.remove`, only reset memory | `daemon.rs` | `phone_disable_deletes_the_token_and_resets_to_off` | **RED** |
| 8 | `back_one_level`: delete the `View::Phone { entry: Some(_) }` special case (falls through) | `view.rs` | `ctrl_q_leaves_the_phone_entry_before_leaving_the_page` | **RED** |
| 9 | `back_one_level`: `View::Phone { entry: None }` target uses `ListState::default()` instead of `settings_state_on_phone()` | `view.rs` | `ctrl_q_from_the_phone_status_page_goes_back_to_settings_on_the_phone_row` | **RED** |
| 10 | `phone_key_has_effect`: `Enter` condition narrowed to just `Off` (drops `Broken`) | `view.rs` | `phone_key_has_effect_matches_the_documented_truth_table`; also `phone.rs::enter_on_broken_also_opens_the_token_entry` | **RED** (both) |
| 11 | `phone_key_has_effect`: `r`/`x` condition replaced with `true` | `view.rs` | `phone_key_has_effect_matches_the_documented_truth_table` | **RED** |
| 12 | `idle_help`'s `View::Phone` arm: always push `r`/`x` regardless of state | `view.rs` | `phone_idle_help_only_advertises_keys_that_actually_work` | **RED** |
| 13 | `escape_hint`'s two `View::Phone` arms swapped | `view.rs` | `phone_escape_hint_distinguishes_entering_a_token_from_just_looking` | **RED** |
| 14 | `phone_verify_outcome_applies_to` body replaced with `true` | `view.rs` | `phone_verify_outcome_does_not_apply_when_the_token_changed` | **RED** |
| 15 | `fetch_phone_status`: ignore the `fallback` parameter, always return `Off` | `mod.rs` | `fetch_phone_status_falls_back_when_disconnected` (fallback deliberately set to `Paired`, not `Off`, so the mutation can't hide behind a coincidental match) | **RED** |
| 16 | `settings_view.rs`'s `Some(SettingsItem::Phone)` arm: hardcode `PhoneStatus { Off, .. }` instead of calling `fetch_phone_status` | `settings_view.rs` | `entering_the_phone_item_reaches_the_real_daemon_not_a_hardcoded_default` (real daemon, pre-seeded token, asserts `WaitingForPairing` — the disconnected-client sibling test can't tell these apart, this one can) | **RED** |

All 16 mutations were applied by hand, confirmed red, then reverted; the
tree was rebuilt clean (`cargo check --all-targets`, then the full suite)
after every revert before moving to the next one. No mutation required
adding more than the tests already listed above.

## Test commands and output tails

Step 2 (before implementation), per the brief:
```
$ cargo test --lib ui::phone -- --test-threads=1
error[E0432]: unresolved import `crate::proto::PhoneStatus`
```
(`cannot find type PhoneStatus`, as the brief predicted — module didn't
exist yet.)

Step 4, after implementation:
```
$ cargo test --lib ui::phone -- --test-threads=1
running 14 tests
test ui::phone::tests::draw_does_not_panic_and_masks_the_token ... ok
test ui::phone::tests::draw_shows_the_broken_reason_as_a_detail_line ... ok
test ui::phone::tests::enter_on_broken_also_opens_the_token_entry ... ok
test ui::phone::tests::enter_on_off_opens_the_token_entry ... ok
test ui::phone::tests::enter_while_waiting_does_not_reopen_the_entry ... ok
test ui::phone::tests::esc_during_verifying_drops_the_pending_receiver ... ok
test ui::phone::tests::esc_on_the_status_page_goes_back_to_settings_on_the_phone_row ... ok
test ui::phone::tests::esc_while_typing_cancels_back_to_the_status_page ... ok
test ui::phone::tests::every_state_tells_the_user_what_to_do_next ... ok
test ui::phone::tests::r_and_x_on_off_are_no_ops ... ok
test ui::phone::tests::the_token_never_appears_in_any_status_text ... ok
test ui::phone::tests::typing_and_backspace_edit_the_buffer ... ok
test ui::phone::tests::typing_during_verifying_is_frozen ... ok
test ui::phone::tests::waiting_names_the_bot ... ok
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 765 filtered out; finished in 0.00s
```

```
$ cargo test --lib daemon:: -- --test-threads=1
running 21 tests
[... all 21 listed ...]
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 758 filtered out; finished in 5.33s
```

Full library suite:
```
$ cargo test --lib -- --test-threads=1
test result: ok. 779 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 29.45s
```

Full workspace (lib + 15 integration binaries + doctests), all `0 failed`:
```
$ cargo test -- --test-threads=1
Running unittests src/lib.rs ... 779 passed
Running unittests src/main.rs ... 0 passed
Running tests/cli.rs ... 9 passed
Running tests/client_timeout.rs ... 1 passed
Running tests/concurrency.rs ... 1 passed
Running tests/daemon_detach.rs ... 1 passed
Running tests/daemon_roundtrip.rs ... 3 passed
Running tests/daemon_upgrade.rs ... 3 passed
Running tests/grid_reply.rs ... 2 passed
Running tests/profiles_flow.rs ... 6 passed
Running tests/projects_flow.rs ... 5 passed
Running tests/screen_state.rs ... 2 passed
Running tests/signal_restore.rs ... 2 passed
Running tests/slow_input.rs ... 1 passed
Running tests/socket_perms.rs ... 1 passed
Running tests/zombie_reaping.rs ... 1 passed
Doc-tests dct ... 0 passed
```

```
$ cargo fmt --check
(exit 0)

$ cargo clippy --all-targets
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s
(no warnings)

$ git diff --check
(exit 0)
```

## Files touched

- `src/proto.rs` — `PhoneState`, `PhoneStatus`, four `Request::Phone*`
  variants (`PhoneSetToken` carries `lang`), `Response::Phone`,
  `PROTOCOL_VERSION` 6→7, Debug redaction, pinning-test updates, new tests.
- `src/secrets.rs` — `PHONE_TOKEN_KEY` (verbatim from the brief).
- `src/i18n.rs` — 9 new `Key` variants, `msg::phone_waiting`,
  `msg::phone_paired`, `msg::phone_token_invalid`, `msg::phone_unreachable`;
  `ALL_KEYS` bumped 101 → 110.
- `src/daemon.rs` — `handle()`/`serve()`/`run_with_manager()` thread a shared
  `Arc<Mutex<PhoneStatus>>`; `initial_phone_status`, `phone_verify_token`,
  `apply_phone_set_token`, `spawn_phone_startup_refresh`; four new match arms.
- `src/ui/view.rs` — `View::Phone`, `PhoneEntry`, `settings_state_on_phone`,
  `phone_key_has_effect`, `phone_verify_outcome_applies_to`; `back_one_level`/
  `escape_hint`/`idle_help` arms for the new view; guard tests extended.
- `src/ui/phone.rs` (new) — `status_line`, `next_step`, `handle_key`, `draw`,
  27 tests.
- `src/ui/settings_view.rs` — the `TODO(task-4, dct-phone-channel)` no-op
  replaced with a real dispatch to `View::Phone`; module doc comment updated
  to match; one test rewritten, one real-daemon test added.
- `src/ui/mod.rs` — `mod phone;`, key/draw dispatch, per-tick status polling,
  the `phone_verify_rx` collection block, `fetch_phone_status` helper; three
  new tests (two real-daemon).
- `src/ui/app.rs` — `phone_verify_rx` field, mirroring `verify_rx`.

## Concerns for whoever reviews this / writes Task 5

- `spawn_phone_startup_refresh` (daemon startup background thread) is
  deliberately untested at the unit level — it calls the real
  `Telegram::new(t).get_me()`, same convention as `send_real`/`send_probe`.
  If a bot name persisted across daemon restarts turns out to matter more
  than I judged, that's the function to revisit; today a restarted daemon
  with an existing token shows `WaitingForPairing` with `bot: None` for the
  few seconds until that thread's one `getMe` call lands.
- `Request::PhoneUnpair` folds `Broken` into `WaitingForPairing` unconditionally
  (see the comment on that arm in `daemon.rs`) — this is honest given
  `ChannelError::BadToken` already erased the 401/403 distinction, but if
  Task 5's Bridge starts tracking *why* a token broke with more precision
  than that enum currently allows, this arm is the one to reconsider.
- I did not implement any pairing/Bridge logic, `Channel::set_destination`
  call sites, or an owner-display-name source — `PhoneStatus.owner` never
  becomes `Some(_)` anywhere in this branch today. That's entirely Task 5.
- The two real-daemon tests I added spin `crate::daemon::run` on a temp
  socket the same way three pre-existing `mod.rs` tests already do
  (`start_daemon_for_test` / the manual inline version in
  `settings_view.rs`) — never touches the user's real `~/.dct`.

---

## Round 2 (fix round 1 recovery): verification pass, no code changes

**Status: nothing in this pass required a code change.** The implementer who
died three times (context ~500k tokens, two API connection errors then a
600s stall) got further than the coordinator's recovery note assumed —
before it stalled it had already fixed Criticals 2 and 3 and removed
`spawn_phone_startup_refresh` outright, but it never wrote a report saying
so, which is why the ledger still listed all four items as unverified. My
job this round was to read the code (not the stale ledger entry) and check
each of the four items against what commit `e1694c7` actually contains, then
prove the load-bearing tests are real by mutating the guarded code and
confirming red, then reverting. `git diff` is clean; nothing below is a new
commit.

### 1. Critical 2 first half — proof from a reachable state

`proto.rs::PhoneState::has_confirmed_token()` now exists and is the single
guard shared by `daemon.rs`'s `Request::PhoneUnpair` handler and
`ui/view.rs::phone_key_has_effect` (both call sites are commented as
required to agree, and do). `Broken`'s reason is now typed
(`PhoneBrokenReason::{BadToken, BotBlocked, Unreachable}`), not a bare
`String`, and only `BotBlocked` counts as "confirmed" — `BadToken` and
`Unreachable` do not, because `apply_phone_set_token` never saves on a
failed verification.

Of the three `Broken` reasons, only `BadToken` and `Unreachable` are
reachable by an actual user today — `BotBlocked` requires Telegram to answer
`getMe` with a 403, which the code's own comment (`daemon.rs` above
`phone_verify_token`) documents cannot happen: `getMe` carries no chat
context, so 403 (`bot was blocked by the user`) is a `sendMessage`-only
error that only becomes reachable once Task 5's Bridge exists. So the test
that actually proves Critical 2's first half is
`daemon::tests::phone_unpair_on_a_bad_token_is_a_no_op`
(`src/daemon.rs:1332`), which constructs `Broken { BadToken | Unreachable,
bot: None }` — exactly the state a user reaches by typing an invalid token,
or having no network when they type a valid one — and asserts `PhoneUnpair`
leaves it untouched.

I verified this by mutation (not just by reading): reverting
`if ph.state.has_confirmed_token()` in the `PhoneUnpair` arm to
`if true` turns that state into `WaitingForPairing` with `bot` still `None`
— which is precisely the `status_line` render that produces "等你在
Telegram 里给 @? 发条消息" (`status_line`'s `WaitingForPairing` arm does
`status.bot.as_deref().unwrap_or("?")`). That mutation turned two daemon
tests red (`phone_unpair_on_a_bad_token_is_a_no_op`,
`phone_unpair_on_off_stays_off`). I also mutated
`view.rs::phone_key_has_effect`'s `r`/`x` arm to `true` and its `Enter` arm
to `state.has_confirmed_token()` (inverted) separately; each turned
`phone_key_has_effect_matches_the_documented_truth_table` red, and the
`Enter` mutation additionally turned three `ui/phone.rs` tests red
(`enter_on_broken_also_opens_the_token_entry` among them — a test that
starts from `Broken { BadToken }`, not `Paired`).

One thing worth flagging precisely: `phone_key_has_effect`'s own doc comment
already explains why there is no equivalent "press r on Broken, assert
no-op" test *inside* `ui/phone.rs` itself (mirroring
`r_and_x_on_off_are_no_ops`) — `App::test_app()` is disconnected, so a
mutated (guard removed) `r`/`x` still ends up calling a client that errors,
and `apply_phone_response`'s fallback produces the same unchanged status as
the guard blocking it outright. I confirmed this experimentally: mutating
the `r`/`x` guard to `true` left `r_and_x_on_off_are_no_ops` green. Adding
such a test to `phone.rs` would be exactly the kind of test this house
flags as suspect — looks like coverage, discriminates nothing. The
`phone_key_has_effect` truth-table test in `view.rs` is the one place this
guard can actually be pinned, and it already is. I did not add a
non-discriminating test to satisfy the brief's file-location preference.

**Conclusion: already fixed and already covered as of `e1694c7`.** No code
or test changes made this round for item 1.

### 2. Critical 3 — network reachability of the two integration tests

Both tests (`ui/mod.rs::fetch_phone_status_reaches_the_real_daemon_when_connected`,
`ui/settings_view.rs::entering_the_phone_item_reaches_the_real_daemon_not_a_hardcoded_default`)
now pre-seed `PHONE_TOKEN_KEY` on disk *before* starting the daemon (via the
shared `start_daemon_at` helper, not a duplicated one — see Minor 2 below),
and both carry a comment stating the fix: "删掉 `spawn_phone_startup_refresh`
之后这条测试不再跟一次真实的 `getMe` 请求赛跑". I confirmed this two ways:

- **Static**: `grep`'d every call site of `Telegram::new(_).get_me()` in the
  crate. There is exactly one, inside `daemon.rs`'s `Request::PhoneSetToken`
  arm (`&|t| Telegram::new(t).get_me()`), reached only when a client sends
  `PhoneSetToken`. Neither test sends that request — they only read status
  (`Request::PhoneStatus`, or `Enter` on the settings row, which calls
  `fetch_phone_status`). `run_with_manager` itself no longer spawns any
  background thread that touches the network (I read it in full; the only
  spawned thread is the 200ms session-tick loop).
- **Dynamic**: ran both tests with `https_proxy`/`http_proxy` pointed at
  `127.0.0.1:1` (nothing listening — any real HTTP attempt would either
  connection-refuse immediately or hang) and, separately, with no proxy
  vars set at all. Both runs: `2 passed; finished in 0.08s`. A real call to
  `api.telegram.org`, successful or not, does not complete in 0.08s
  end-to-end including daemon startup — this is consistent with zero
  network attempts, not just a fast one.

The race the task warned about (asserting `WaitingForPairing` while a
background thread overwrites it with `Broken`) no longer applies, for the
same reason: the background thread that would have raced
(`spawn_phone_startup_refresh`) does not exist in this tree. It was
removed, not gated. There is nothing left to race.

**Conclusion: already fixed as of `e1694c7`.** No code changes made this
round.

### 3. `spawn_phone_startup_refresh`

It is gone. `grep -n spawn_phone_startup_refresh src/daemon.rs` matches only
a doc comment (`run_with_manager`'s header and `initial_phone_status`'s)
explaining that it used to exist and was deleted for being unrequested,
untested, and network-touching. `run_with_manager` now calls
`initial_phone_status(&secrets)` directly — disk-only, no thread, no
`getMe`. This resolves the concern by removal, one of the two options the
task offered.

**Conclusion: already resolved as of `e1694c7` (by deletion). No action
taken this round.**

### 4. Mutation sweep, applied by hand, all reverted

Beyond the two mutations already covered under item 1, I swept the rest of
the logic this fix round touched or that the earlier mutation table (in the
first half of this report) covered, re-verifying against the *current*
shape of the code (`Broken` now carries `{ reason, message }`, not a bare
`String`; `has_confirmed_token` is new). Each mutation was applied with
`Edit`, confirmed red with a targeted `cargo test --lib -- --test-threads=1
<name>`, then reverted with `git checkout -- <file>`.

| # | Mutation | File | Test(s) that catch it | Result |
|---|---|---|---|---|
| 1 | `has_confirmed_token`: drop the `BotBlocked` arm | `proto.rs` | `phone_unpair_on_a_blocked_bot_repairs_and_keeps_the_bot_name`, `phone_idle_help_only_advertises_keys_that_actually_work`, `phone_key_has_effect_matches_the_documented_truth_table` | RED (3) |
| 2 | `phone_verify_token`: `BadToken` composes `reason: Unreachable` instead of `BadToken` | `daemon.rs` | `phone_verify_token_marks_a_bad_token_broken` | RED |
| 3 | `apply_phone_set_token`: drop the "don't save on Broken" guard | `daemon.rs` | `apply_phone_set_token_does_not_save_a_bad_token` | RED |
| 4 | `initial_phone_status`: invert the has-token ternary | `daemon.rs` | 3 tests (`initial_phone_status_is_off_without_a_saved_token`, `..._is_waiting_for_pairing_with_a_saved_token`, `..._reads_the_bot_name_from_disk_without_any_network_call`) | RED (3) |
| 5 | `PhoneDisable`: skip removing `PHONE_BOT_KEY` | `daemon.rs` | `phone_disable_deletes_the_token_and_resets_to_off` | RED |
| 6 | `PhoneUnpair`: drop the `has_confirmed_token()` guard | `daemon.rs` | `phone_unpair_on_a_bad_token_is_a_no_op`, `phone_unpair_on_off_stays_off` | RED (2) |
| 7 | `phone_key_has_effect`: invert the `Enter` condition | `view.rs` | 5 tests (truth table, idle_help, 3 `phone.rs` Enter tests) | RED (5) |
| 8 | `phone_key_has_effect`: `r`/`x` condition replaced with `true` | `view.rs` | `phone_key_has_effect_matches_the_documented_truth_table`, `phone_idle_help_only_advertises_keys_that_actually_work` | RED (2) — `phone.rs`'s `r_and_x_on_off_are_no_ops` stayed green (see item 1's discussion of why) |
| 9 | `back_one_level`: delete the `View::Phone { entry: Some(_) }` arm | `view.rs` | `ctrl_q_leaves_the_phone_entry_before_leaving_the_page` | RED |
| 10 | `back_one_level`: `View::Phone { entry: None }` target uses `ListState::default()` instead of `settings_state_on_phone()` | `view.rs` | `ctrl_q_from_the_phone_status_page_goes_back_to_settings_on_the_phone_row` | RED |
| 11 | `idle_help`'s `View::Phone` arm: always push Enter/r/x | `view.rs` | `phone_idle_help_only_advertises_keys_that_actually_work` | RED |
| 12 | `escape_hint`'s `entry: Some(_)` arm made identical to the `entry: None` arm | `view.rs` | `phone_escape_hint_distinguishes_entering_a_token_from_just_looking` | RED |
| 13 | `phone_verify_outcome_applies_to` body replaced with `true` | `view.rs` | `phone_verify_outcome_does_not_apply_when_the_token_changed` | RED |
| 14 | `fetch_phone_status`: ignore `fallback`, always return `Off` | `mod.rs` | `fetch_phone_status_falls_back_when_disconnected` | RED |
| 15 | `status_line`'s `WaitingForPairing` arm: drop the bot-name interpolation | `phone.rs` | `waiting_names_the_bot` | RED |
| 16 | `next_step`'s `Paired` arm: return `Some(...)` instead of `None` | `phone.rs` | `every_state_tells_the_user_what_to_do_next` | RED |
| 17 | `status_line`'s `Broken` arm: render `message` instead of the opaque headline | `phone.rs` | `the_token_never_appears_in_any_status_text` | RED |
| 18 | `settings_view.rs`'s `Some(SettingsItem::Phone)` arm: hardcode `Off` instead of calling `fetch_phone_status` | `settings_view.rs` | `entering_the_phone_item_reaches_the_real_daemon_not_a_hardcoded_default` | RED |

All 18 caught. After every mutation, `git checkout -- <file>` restored the
original; `git status --porcelain` and `git diff --stat` were both empty
before starting the next one and at the end of the sweep. `cargo test --lib
-- --test-threads=1` afterward: **790 passed, 0 failed** (unchanged from the
tree at handoff). `cargo fmt --check` and `cargo clippy --all-targets`: both
clean, no changes needed.

### Minors

- **"27 tests" in `ui/phone.rs`**: still wrong, still 14
  (`grep -c '#\[test\]' src/ui/phone.rs` → 14). The claim in the "Test
  commands and output tails" section above (line 189 area) was accurate at
  the time it was written (14 passed, correctly listed) — the "27" is only
  in the "Files touched" bullet's prose and was never true; I'm not
  overwriting that line per the append-only instruction, but flagging it
  here as wrong. Total test count across the four phone-related files as of
  this commit: `proto.rs` 16, `daemon.rs` 26, `ui/view.rs` 102 (not all
  phone-specific — this file covers the whole UI), `ui/phone.rs` 14.
- **Duplicated `start_daemon_for_test`**: also already fixed as of
  `e1694c7`. `src/ui/mod.rs:2296` defines `pub(crate) fn start_daemon_at`
  (the shared primitive `start_daemon_for_test` now calls), and
  `settings_view.rs`'s test calls `super::super::start_daemon_at` directly
  — no inlined copy remains. The doc comment on `start_daemon_for_test`
  even names this as the reason the split exists ("那边曾经内联抄了一份
  一模一样的实现——…C3 的修复要改两处而不是一处正是这份重复的直接代价").

### What this round actually did

Read every line the four work items pointed at, ran the daemon and reached
it over its real Unix socket to rule out the network claim empirically
rather than trust a comment, and ran an 18-mutation sweep (2 of which
directly reproduce Critical 2's original failure mode) confirming every
guard this task named is load-bearing. No source file changed. Verified
final state: `git status --porcelain` shows only the pre-existing untracked
`docs/dc-terminal_产品改进与自动化开发方案.md` (unrelated, not part of this
branch's work); `git diff` is empty; `cargo test --lib -- --test-threads=1`
is 790 passed / 0 failed; `cargo fmt --check` and `cargo clippy
--all-targets` are clean.
