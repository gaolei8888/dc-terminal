# Final whole-branch review — fixes report

Commit: `a9564a2` on `worktree-phone-channel` (parent `5e6ff78`).
Single commit, all seven fixes, message in English, no AI signature/trailer.

## Fix 1 (Important) — dead `has_channel` gate

**Route taken: the real fix**, not the doc-only fallback. Investigated and it
was cheap enough:

- `src/daemon.rs::run_with_manager` now only calls `mgr.set_event_sink(...)`
  at startup when the secrets store already has `PHONE_TOKEN_KEY` (mirrors
  the check `initial_phone_status`/`start_phone_bridge` already do).
- `Request::PhoneSetToken`'s success arm re-arms it with
  `mgr.set_event_sink(event_tx.clone())` — this is the path a user takes the
  first time they ever configure the feature from `Off`, so it must be armed
  there too. Idempotent if a token was already configured (e.g. re-pasting a
  new token while a bridge is already running).
- `Request::PhoneDisable` calls a new `SessionManager::clear_event_sink()`
  (mirrors `set_backend`'s `Option`-based pattern) so the gate goes back to
  false when the feature is actually turned off — otherwise `tick()` would
  keep screen-capturing and queueing events into a sink nobody drains, which
  is wasted work and leaves the same doc-comment lie for anyone who ever
  enabled-then-disabled.
- `handle()`/`serve()` gained an `event_tx: &Sender<Event>` parameter to
  reach `mgr.set_event_sink` from the `PhoneSetToken` request handler; all in
  a single unbounded `mpsc::channel` created once in `run_with_manager` and
  cloned per connection (cheap, `mpsc::Sender` clone is Arc-based).
- Rewrote the doc comments on `should_notify`'s `has_channel` bullet and
  `SessionManager::event_tx`'s field doc to describe what's true now (and to
  say plainly what was wrong before), instead of leaving a stale "试都不用
  试" that was never actually being enforced.

Test-site churn: `handle()` gained a required parameter, so all 9 existing
test call sites needed a `&test_event_tx()` argument (new helper: an
unreceived `mpsc::channel().0`, since `send()` never fails just because
nobody's listening).

## Fix 2 (Important) — narrow the security-bypass surface

- `Bridge::enqueue` and `Bridge::deliver`: `pub fn` → `fn` (private to the
  module). Grepped for callers outside `bridge.rs` first — none exist; all
  call sites are the in-module `spawn_event_consumer`, `dispatch`/`route()`,
  and the module's own tests.
- `BridgeHandle::accept`: `pub fn` → `#[cfg(test)] pub(crate) fn`. Its only
  non-test caller was daemon.rs's own `#[cfg(test)] mod tests` (verified by
  line numbers — both call sites are inside the `#[cfg(test)]` block starting
  at daemon.rs:616).
- Zero behaviour change; only visibility.

**Mutation-tested (fix 2):** temporarily appended a probe function to
`src/main.rs` (a separate binary crate consuming `dct::` as a library, so a
genuine external caller) that called `b.deliver(...)`, `b.enqueue(...)`, and
`h.accept(...)`. `cargo build --bin dct` failed with:
```
error[E0624]: method `deliver` is private
error[E0624]: method `enqueue` is private
error[E0599]: no method named `accept` found for reference `&BridgeHandle`
```
Confirms the narrowing is real and structural, not just a comment. Probe was
then reverted (`git diff --stat src/main.rs` shows no changes after revert).

## Fix 3 (Important) — no cue on the options list

`Bridge::compose_outbound` now appends a short Chinese line after the
numbered list: "回数字就行，或者直接说说你自己的想法也可以" ("just reply
with the number, or just say it in your own words is fine too"). Matches the
register of the surrounding phone strings (plain, no jargon, no imperative
bark).

Updated `bridge::tests::options_from_the_push_are_used_to_map_the_next_reply`
to also assert the cue text is present (`回数字` and `自己的想法` substrings)
alongside the existing numbered-list assertion.

## Fix 4 (Minor, real bug) — `PhoneUnpair` leaves old routing state behind

`Bridge::clear_owner` (the landing point for `PhoneUnpair`/`BridgeHandle::
unpair`) now also clears `used`, `replied_since_use`, `outbound_map`,
`ambiguous_pushes`, and `pending_options` — not just `owner`. Left `outbound`
(the not-yet-sent push queue) alone deliberately: those notifications don't
belong to any particular phone and should still reach whoever pairs next.

`PhoneDisable` needs no equivalent change: it discards the whole `Bridge`
object (`stop_current` + the next `PhoneSetToken` constructs a fresh
`Bridge::new()`), so all this state is naturally empty on the next bridge
anyway — there's no live `Bridge` instance carrying stale state across a
disable/re-enable cycle the way there is across an unpair/re-pair cycle.

**New test:** `bridge::tests::unpairing_does_not_leave_the_old_use_target_for_the_next_phone`.
Old phone (chat id 999, `for_test_with_writer`'s default owner) sends
`/use 3`; `clear_owner()` runs (the `PhoneUnpair` landing point); new phone
(chat id 222) pairs and sends a bare "继续" with only session 9 waiting.
Asserts it lands on 9 (the "only one waiting" rule), not 3 (the stale
`/use` target).

**Mutation-tested (fix 4):** reverted `clear_owner` to only clear `owner`,
reran the new test — it failed:
```
assertion `left == right` failed: 新手机不该继承旧手机的 /use 目标：[(3, "继续")]
  left: [(3, "继续")]
 right: [(9, "继续")]
```
Restored the fix, test passes again.

## Fix 5 (Minor) — `.lock().unwrap()` on telegram.rs's offset

All four sites (`poll`'s read, `poll`'s write, `drain`'s read, `drain`'s
write) switched from `self.offset.lock().unwrap()` to
`recover(self.offset.lock())`, importing `crate::session::recover` (already
`pub(crate)`). No other `.lock().unwrap()` on `offset` remained (grepped the
whole file).

## Fix 6 (Minor) — bottom bar lies while the token field is open

- Added `HelpCtx::phone_editing: bool` (mirrors the existing "don't pass the
  whole `App` in, just the couple of booleans idle_help needs" pattern
  documented on `HelpCtx` itself).
- `mod.rs::help_ctx_for` sets it from `app.phone_buf.is_some()`.
- `view.rs::idle_help`'s `View::Phone` arm now checks `ctx.phone_editing`
  first and, if true, returns the editing-state keys (`Key::PasteOrTypeKey`,
  `Enter`→`Key::Confirm`, `Esc`→`Key::Cancel`) before falling through to the
  status-derived keys.
- No `continue` involved anywhere — this is a pure render function
  (`idle_help`), not a key-handling loop, so the project's "never `continue`
  inside a key-handling branch" rule (which the review flagged as a risk)
  didn't end up applying to the actual fix location; I didn't touch
  `phone.rs::handle_key`/`handle_typing` at all.
- Fixed two other `HelpCtx` literal construction sites (`ui/keys.rs`) that
  the compiler caught once the field was added (no logic change, just
  `phone_editing: false`).

**New test:**
`view::tests::the_phone_bar_shows_editing_keys_while_the_token_field_is_open`
— for all four `PhoneState` variants, with `phone_editing: true`, asserts
the bar shows `Enter`+确认 and `Esc`+取消, and does **not** show any of
"填令牌"/"关掉"/"重新配对".

## Fix 7 (Minor) — status write ordering in `PhoneSetToken`

Restructured the success arm so `*recover(phone.lock()) = status.clone()`
happens immediately after building `status`, **before** `crate::bridge::
replace(...)` starts the new bridge thread. Also cleaned up the `Err` arm to
follow the same "build status, write status, return status" shape instead of
writing once after the whole `match`.

## Commands run

```
cargo fmt --check                       # clean after one `cargo fmt` pass
cargo clippy --all-targets              # clean, no warnings
cargo build --all-targets               # clean

cargo test --lib bridge:: -- --test-threads=1        # 101 passed
cargo test --lib daemon:: -- --test-threads=1        # 16 passed
cargo test --lib session:: -- --test-threads=1       # 79 passed
cargo test --lib channel::telegram:: -- --test-threads=1  # 14 passed
cargo test --lib ui::view:: -- --test-threads=1      # 83 passed
cargo test --lib ui::keys:: -- --test-threads=1      # 11 passed
cargo test --lib phone -- --test-threads=1           # 26 passed (cross-module phone-related tests)
```

Full suite (`cargo test -- --test-threads=1`) launched in the background,
logging to
`/private/tmp/claude-502/.../scratchpad/full_test_run.log` (outside the
worktree), not waited on per instructions. Baseline was 885 tests passing at
5e6ff78; this branch adds 3 new tests (fix 3's assertion is inside an
existing test, not a new one) — fix 4's and fix 6's new tests — so the full
run should land at 887 passed, 0 failed, plus the known
`sighup_restores_the_terminal` flake if it fires.

## Concerns

- None structural. The one judgment call worth flagging: fix 1's re-arm in
  `PhoneSetToken` required threading a new `event_tx` parameter through
  `handle()`/`serve()`, which touched 9 existing test call sites purely
  mechanically (added `&test_event_tx()`). No test's assertions changed
  because of this — only the call signature.
- `.superpowers/sdd/2026-08-10-dct-phone-channel/progress.md` was left
  modified in the working tree and deliberately not staged/committed, per
  instructions.
