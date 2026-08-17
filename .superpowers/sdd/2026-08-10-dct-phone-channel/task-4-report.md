# Task 4 report: protocol, token storage, phone settings page

Commit: `e082312` on branch `worktree-phone-channel`.

## What was implemented

**`src/proto.rs`**
- `PhoneState` (`Off`/`WaitingForPairing`/`Paired`/`Broken(String)`) and `PhoneStatus { state, bot, owner }`, verbatim per the brief.
- `Request::PhoneStatus`, `PhoneSetToken { token }`, `PhoneUnpair`, `PhoneDisable`; `Response::Phone(PhoneStatus)`.
- Hand-written `Debug` for `Request` redacts `PhoneSetToken`'s token (same treatment as `SetSecret`/`VerifySecret`).
- `PROTOCOL_VERSION` bumped 6 → 7 with a dated changelog entry; the two shape-pinning tests (`the_request_shape_is_pinned_to_the_protocol_version`, `the_session_info_shape_is_pinned_too`, `projects_response_carries_both_lists`) updated to the new version and the new JSON shape.
- New test `debug_redacts_the_token_on_phone_set_token`.

**`src/secrets.rs`**
- `pub const PHONE_TOKEN_KEY = "__phone__"` with the comment explaining why it is invisible to the `c` key-settings page (that page iterates profiles + `has_secret`, not the keys of `secrets.toml` — see `ui::view::secret_rows`).
- Test `phone_token_key_round_trips_like_any_other_secret`.

**`src/channel/telegram.rs`**
- Added `Telegram::get_me(&self) -> Result<String, ChannelError>`, a thin wrapper around the already-existing `parse_get_me` that uses the instance's injected transport (so it's testable without touching the network). Test `telegram_get_me_uses_the_injected_transport`.

**`src/daemon.rs` (Ruling 3 — the shared status slot)**
- Added `phone: Arc<Mutex<PhoneStatus>>` alongside the existing `store`/`secrets` slots in `run_with_manager`, threaded through `serve()`/`handle()` exactly like those two. It is constructed once at daemon startup by `initial_phone_status(&secrets)`: `WaitingForPairing` if a token is already on disk, `Off` otherwise — deliberately does **not** call `getMe` at startup (no network dependency to get the daemon running, and it keeps daemon startup testable without a network). Task 5's bridge thread is expected to write into this exact same `Arc<Mutex<PhoneStatus>>` — it's the last argument to `handle()`, easy to find.
- `Request::PhoneStatus` just clones the slot.
- `Request::PhoneSetToken` calls `Telegram::new(&token).get_me()` (real network — analogous to `VerifySecret`'s `send_probe`); on success it saves the token via `secrets.set(PHONE_TOKEN_KEY, ...)` first, then writes `WaitingForPairing{bot, owner: None}`; on failure it writes `Broken(<generated message>)` and does **not** touch the secrets file. Both paths update the shared slot and return `Response::Phone`.
- `Request::PhoneUnpair` clears `owner`, sets `WaitingForPairing`, leaves the token alone (re-pairing, not disabling).
- `Request::PhoneDisable` deletes the token from `SecretStore` and resets the slot to `Off`.
- New tests: `phone_status_reads_the_shared_slot`, `phone_unpair_forgets_the_owner_but_keeps_the_token`, `phone_disable_deletes_the_token_and_resets_the_slot`, `initial_phone_status_follows_whether_a_token_is_stored`. (No test hits the real Telegram network for `PhoneSetToken`'s success/failure branches — that would violate the "tests must not touch the network" constraint; the `get_me` unit test in `telegram.rs` already covers the parsing/transport contract that branch depends on.)

**`src/i18n.rs`**
- New `Key` variants for the phone page's fixed strings (`PhoneOffLine`, `PhonePairedLine`, `PhoneBrokenLine`, three `PhoneNextStep*`, `PhoneEnterToken`, `PhoneRepair`, `PhoneTurnOff`, `PhonePasteToken`) plus `msg::phone_waiting_for_pairing(lang, bot)` and `msg::phone_paired(lang, owner)` for the two lines that need interpolation.

**`src/ui/view.rs`**
- `View::Phone { status: PhoneStatus }` — exactly the one field named in the brief. The "typing a token" / "verifying" temporary states deliberately do **not** live on `View::Phone`; they live on `App` (`phone_buf`, `phone_verify_rx`) for the same reason `verify_rx` already lives on `App` and not on `View::EnterSecret`'s enclosing enum: `View` is `Clone`d on every key-dispatch and draw, and an `mpsc::Receiver` can't be.
- `escape_hint` and `idle_help` (both exhaustive matches over `View`, no wildcard) got explicit `View::Phone` arms: escape hint is always "Esc → 设置" (this page's only entry point is Settings), and `idle_help` lists `Enter`/`r`/`x`/`Esc` conditionally based on `status.state` — never showing a key that would currently just produce an error (same "can't press it, don't print it" rule `board_keys` already follows).

**`src/ui/settings_view.rs` (Ruling 2)**
- Replaced the inert `Some(SettingsItem::Phone) | None => { ... }` arm with `Some(SettingsItem::Phone) => open_phone(app, state)`, `None` split off on its own. `open_phone` synchronously calls `Request::PhoneStatus` (same pattern as `mod.rs::open_secrets`: on failure it stays on `Settings` with a red error message instead of entering a page with no data).
- Test ownership: replaced the old `choosing_phone_does_nothing_yet` with `choosing_phone_without_a_daemon_stays_on_settings_with_an_error` (disconnected `App`, confirms it doesn't silently transition), and added `choosing_phone_enters_the_phone_page_and_escape_returns_to_settings`, which starts a real in-process daemon (same pattern as `mod.rs::start_daemon_for_test`, duplicated locally since that helper is private to `mod.rs`'s test module), confirms `Enter` on "Phone" lands in `View::Phone { status }` with the daemon's real initial status (`Off`), and confirms `Esc` (dispatched through `ui::phone::handle_key`, not this module's own `handle_key`) returns to `View::Settings`.

**`src/ui/phone.rs` (new file)**
- `status_line`/`next_step`: the two pure functions from the brief's Step 1 test, copied verbatim (including the three tests). Both **deliberately ignore the contents of `PhoneState::Broken(String)`** — they only match on the enum variant, never interpolate the string. This is what makes `the_token_never_appears_in_any_status_text` pass unconditionally rather than by luck: the test constructs `Broken("123456:AAH-SECRET".into())`, a string that looks exactly like a real token, and asserts it never reaches the rendered text; if either function ever read that string, the test would fail immediately regardless of what the real `daemon.rs` writes into `Broken`.
- `handle_key`/`draw`: `Enter` (only when `Off`/`Broken`) starts typing into `app.phone_buf`; `Enter` while typing spawns a background thread (own connection, same pattern as `secret.rs`'s `VerifySecret` thread) that calls `Request::PhoneSetToken` and reports back through `app.phone_verify_rx`; `r` (only when `Paired`) sends `PhoneUnpair` synchronously (no network — local state/secrets writes only); `x` (whenever not `Off`) sends `PhoneDisable` synchronously; `Esc` behaves like the language sub-list: one level at a time (cancel typing → view status → back to Settings).
- `mod.rs` wiring: registered `mod phone;`, added `View::Phone` arms to the two exhaustive matches (key dispatch and draw dispatch), added a `View::Phone` case to `escape_hint_cols_fits_every_view`'s enumerated view list, and added a `phone_verify_rx` drain (mirrors the existing `verify_rx` drain, same ordering rationale: before `term.draw`) plus a throttled (300ms, same idea as the grid's `grid_last_fetch`) auto-refresh of `Request::PhoneStatus` while sitting on `View::Phone` and not mid-type/verify — this is what makes the async pairing actually observable without the user having to leave and re-enter the page.

## Mutations tried (both from the brief, both caught)

1. Changed `next_step`'s `Paired` arm to `Some("mutated".to_string())` → `every_state_tells_the_user_what_to_do_next`'s last assertion failed as required.
2. Removed the bot-name interpolation from `status_line`'s `WaitingForPairing` arm (made it return the fixed `PhoneNextStepWaiting` text) → `waiting_names_the_bot` failed as required.

Both mutations were reverted (verified with a diff-free restore) before the final commit.

## Commands run

- `cargo build --lib` — clean, iterated a few times while wiring things up.
- `cargo test --lib -- --test-threads=1` — 733 passed, 0 failed (baseline for `--lib` alone, checked via `git stash`, was 716; net +17 new tests, no regressions).
- `cargo test -- --test-threads=1` (lib + all integration binaries) — every suite passed; combined total 771 (up from the stated baseline of 754).
- `cargo fmt` then `cargo fmt --check` — clean.
- `cargo clippy --all-targets` — one warning (`type_complexity` on a test helper's tuple return type) fixed with a local type alias; final run clean.

## Concerns

- `PhoneState::Broken`'s inner `String` is currently write-only in practice: `daemon.rs` composes a real (Chinese-only) sentence for it, but `ui::phone::status_line`/`next_step` never read it back (by design, see above). This is intentional given the exact test the brief specifies, but it means the specific reason for a `Broken` state (bad token vs. unreachable vs. malformed) is not yet visible to the user — they only see a generic "手机通知这会儿连不上" plus "按 Enter 重新粘贴一遍令牌". If a future task wants to surface the specific reason, it will need a different mechanism than reading the `Broken` payload directly (e.g. a separate, explicitly-not-secret reason code).
- The daemon's `phone_set_token_failure_message` and the i18n phone strings are Chinese/English via the normal `Key`/`msg` machinery, but the *daemon-composed* `Broken(String)` itself is hardcoded Chinese (no `Lang` is threaded through `Request::PhoneSetToken`, matching the brief's exact type shape). This is currently harmless only because that string is never displayed (see above); it would need attention if that decision is revisited.
- Task 5's bridge is expected to also write into the same `phone: Arc<Mutex<PhoneStatus>>` slot and to fill in `bot`/`owner` after the daemon restarts with an existing token (today's `initial_phone_status` leaves `bot: None` in that case, so a restarted daemon's `WaitingForPairing` status won't name the bot until the bridge runs and updates it).
- `settings_view.rs`'s new daemon-backed test duplicates the small `start_daemon_for_test`-style setup that already exists (privately) in `ui/mod.rs`'s test module, since that helper isn't `pub(crate)`. Minor duplication, consistent with other small test-helper duplication already in this codebase.

## Files touched

- `/Users/lei/Documents/work/dc/dc-terminal/.claude/worktrees/phone-channel/src/proto.rs`
- `/Users/lei/Documents/work/dc/dc-terminal/.claude/worktrees/phone-channel/src/secrets.rs`
- `/Users/lei/Documents/work/dc/dc-terminal/.claude/worktrees/phone-channel/src/channel/telegram.rs`
- `/Users/lei/Documents/work/dc/dc-terminal/.claude/worktrees/phone-channel/src/daemon.rs`
- `/Users/lei/Documents/work/dc/dc-terminal/.claude/worktrees/phone-channel/src/i18n.rs`
- `/Users/lei/Documents/work/dc/dc-terminal/.claude/worktrees/phone-channel/src/ui/view.rs`
- `/Users/lei/Documents/work/dc/dc-terminal/.claude/worktrees/phone-channel/src/ui/app.rs`
- `/Users/lei/Documents/work/dc/dc-terminal/.claude/worktrees/phone-channel/src/ui/settings_view.rs`
- `/Users/lei/Documents/work/dc/dc-terminal/.claude/worktrees/phone-channel/src/ui/phone.rs` (new)
- `/Users/lei/Documents/work/dc/dc-terminal/.claude/worktrees/phone-channel/src/ui/mod.rs`

## Fix round: review finding on concern 3

Review verdict: spec pass, quality approved. Concerns 1, 2, 4 accepted as-is (the
"structurally ignore `Broken`'s payload" design was explicitly endorsed as the
correct defensive posture, not dead code). Concern 3 was upheld as an Important,
reachable-in-production finding, addressed here.

**Finding:** `status_line`'s `WaitingForPairing` arm fell back to `PhoneOffLine`
when `bot` was `None`. That combination is reachable in production, not just in
theory: `daemon::initial_phone_status` puts a restarted daemon straight into
`WaitingForPairing` whenever a token is already on disk, but leaves `bot: None`
until Task 5's bridge thread runs and re-queries `getMe`. Opening the phone page
in that window showed "手机通知还没打开" ("phone notifications are off") for a
state that is very much not off, while `next_step` simultaneously told the user
to go message a bot it never named — exactly the failure `waiting_names_the_bot`
exists to prevent.

**Fix:**
- Added `Key::PhoneReconnectingLine` ("正在重新接上，请稍候" / "Reconnecting, one
  moment") and `Key::PhoneNextStepReconnecting` ("稍等一下，过会儿再回来看看" /
  "Give it a moment, then check back here") to `src/i18n.rs`, both languages.
- `status_line`'s `WaitingForPairing` arm now renders `PhoneReconnectingLine`
  (not `PhoneOffLine`) when `bot` is `None`.
- `next_step`'s `WaitingForPairing` arm now branches on `bot` too: names the bot
  and says "go message it" only when a name is present; otherwise says "wait a
  moment" — never referencing a bot name that isn't on screen.
- New pinning test in `src/ui/phone.rs`:
  `waiting_without_a_bot_name_is_neither_off_nor_a_dangling_instruction` — asserts
  the status line is not equal to the `Off` line, and that the next-step text
  contains neither `@` nor the word "bot".

**Mutation performed and its output:** reverted the `WaitingForPairing`/`bot: None`
arm of `status_line` back to `text(Key::PhoneOffLine, lang).to_string()` (the
pre-fix behavior) and re-ran only the new test:

```
running 1 test
test ui::phone::tests::waiting_without_a_bot_name_is_neither_off_nor_a_dangling_instruction ... FAILED

---- ui::phone::tests::waiting_without_a_bot_name_is_neither_off_nor_a_dangling_instruction stdout ----
thread '...' panicked at src/ui/phone.rs:307:9:
assertion `left != right` failed: 令牌还在，不该说成关着的：手机通知还没打开
  left: "手机通知还没打开"
 right: "手机通知还没打开"

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 733 filtered out; finished in 0.00s
```

The mutation was caught, confirming the test pins the right thing. The mutation
was then reverted (fix restored) before verifying and committing.

**Verification after the fix:**
- `cargo test --lib -- --test-threads=1 ui::phone::` — 10 passed, 0 failed (was 9;
  the new test added).
- `cargo test -- --test-threads=1` (lib + every integration binary) — all green,
  734 lib tests + 38 integration tests = 772 total (baseline to hold was 771; net
  +1, no regressions).
- `cargo fmt --check` — clean.
- `cargo clippy --all-targets` — clean.

Concerns 1, 2, and 4 from the original report stand as previously disclosed and
were not touched in this round, per the review's instruction.
