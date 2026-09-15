# Task 7 report — daemon: publish intent, pusher PUT/DELETE, self-heal

Status: **DONE**. Commit `42e00dd` (on top of `f6b70ee`), message verbatim from the brief, no AI attribution.

## What was implemented

### `src/live.rs`
- `Room` gets `want_public`, `public_key`, `public`, `public_rev`. `start()` sets them to `None, None, Private, 0`, so a new live is always private. `info()` and `restage()` now report `room.public` (before this they hardcoded `Private`). `RoomSnapshot` also carries `want_public`, `public_key`, `public_rev`.
- `LiveState::publish(title, key) -> Option<LiveInfo>` sets Pending and bumps rev. `unpublish() -> Option<LiveInfo>` sets Private, clears key and title, and bumps rev. Both return `None` when nothing is live. `grant() -> Option<String>` computes `publish_grant(push_hash(push_secret), id)` under the one lock.
- `mark_public(id, rev, public)` only applies when both id and public_rev still match (same rule as `mark_ready`).
- `set_public_seen(id, rev, relay_title) -> bool` is guarded by id **and rev**. This differs from the brief's signature, which had no rev (see Deviations).
- Pure functions `PublicAction`, `public_action`, `seen_public` are exactly as the brief specifies.
- `PublishBody`, `put_public`, `delete_public`. `LanesBody` gets `#[serde(default)] public: Option<PublicTitleBody>`. `fetch_viewers` is renamed `fetch_lanes -> Option<(u32, Option<String>)>` and its two existing test call sites are updated.
- `pusher_loop`:
  - State: `public_applied`, `public_stuck` (both keyed `(id, public_rev)`), `public_retry: Option<((id, rev), Instant)>` and `public_fails`.
  - Publicity goes out after `start_room` succeeds and before the viewers read. A failed start `continue`s, so no PUT is ever sent before the room exists on the relay.
  - Retries use a retry-at instant, never a `nap`, so publicity never holds up frame pushing.
  - PUT 204 → `Listed` and applied. 401/403/413 → `Failed` and stuck, never retried. Anything else (unreachable, 429, 5xx) → `Failed { reason }` and retried on the `start_backoff` table.
  - Heal: when `set_public_seen` returns true, `public_applied = None`, so the next round PUTs once more.
  - A successful `start_room` clears `public_applied`, because the relay's reopen drops publicity (`dct-srv` `Live::start` inserts `public: None`, which I checked).
  - The not-live branch clears all four variables.
- `#[allow(dead_code)]` was removed from `Room::push_secret`, which `snapshot`/`grant` now read. It stays on the `push_secret()` method because only tests call it; clippy confirmed it is still needed. I rewrote the comment to say why.

### `src/daemon.rs`
- The three `TODO(Task 7)` placeholder arms are replaced.
  - `LivePublish` → `live_publish(live, secrets, title)`: stored key `secrets::LIVE_PUBLISH_KEY`, otherwise `LivePublishKeyMissing`. The title is trimmed, then cut to `MAX_PUBLIC_TITLE_CHARS` chars. Not live → `LiveStagingRejected(NotLive)`.
  - `LiveUnpublish` → `Live(info)` or `NotLive`.
  - `LivePublishGrant` → `LiveGrant(LiveGrantToken(g))` or `NotLive`.
- Signature check against source: `secrets: &Arc<Mutex<SecretStore>>`, `recover` comes from `crate::session`, `SecretStore::get(&str) -> Option<&str>`. The brief's code matched the source.

## Tests (TDD)

**RED**: after writing the tests, `env -u TERM cargo test --lib live` failed to compile with 26 errors (`unresolved import PublicAction`, and no method `publish`/`unpublish`/`grant`/`mark_public`/`set_public_seen` on `LiveState`).

**GREEN**:
- `env -u TERM cargo test --workspace --locked --no-fail-fast`: rc=0, **1492 passed**, 0 failed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: rc=0.
- `git diff --check`: clean.

**New tests in `live.rs`**
- From the brief: `publishing_needs_a_room_and_records_pending`, `the_grant_is_the_shared_hmac_of_this_rooms_push_secret`, `the_publish_key_never_leaves_the_state`, `what_to_tell_the_relay_about_publicity`, `reading_publicity_back_from_the_relay`, `the_pusher_publishes_with_the_push_secret_and_the_key` (also asserts the path), `a_refused_publication_is_not_retried` (also asserts the reason is `Refused(401)`), `unpublishing_sends_a_delete` (also asserts the header and that there is exactly one DELETE), `a_publication_made_elsewhere_is_reflected_without_a_put`.
- Added by me: `restaging_keeps_the_public_state`, `a_new_live_starts_private`, `a_late_publicity_result_for_an_older_intent_is_ignored`, `pressing_publish_again_after_a_refusal_tries_again`, `an_unreachable_publication_is_retried_after_a_backoff`, `a_publication_the_relay_lost_is_put_once_more`, `restaging_sends_the_publication_again`, `restaging_does_not_retry_a_refused_publication`, `an_unpublish_the_relay_answered_is_not_retried`, `fetch_lanes_reads_the_public_title_back`.

**New tests in `daemon.rs`**
- From the brief: `live_publish_without_a_key_is_refused_with_a_code` (also asserts no intent is left behind), `live_publish_uses_the_stored_key_and_truncates_the_title` (also checks trimming and that the key is absent from the serialized response), `a_grant_needs_a_live_room` (also checks the grant value when live).
- Added by me: `live_publish_needs_a_live_room`, `live_unpublish_answers_the_private_state_and_needs_a_room`.

**FakeSrv changes**
- `public_status`, `lanes_public` as in the brief.
- I also added `forget_public` and `unpublish_status`, and made the fake stateful like the real relay: a PUT answered 204 stores the title into `lanes_public`, and a DELETE clears it. This was necessary. With the brief's stateless fake, the first lanes read after a successful PUT returns `public: null`, which correctly triggers the self-heal. `restaging_sends_the_publication_again` failed because of that (2 PUTs before the restage). The heal path is now tested on purpose through `forget_public`.

## Mutation checks (each reverted; `cmp` against the saved good copy confirmed the restore)

| # | Mutation | Result |
|---|---|---|
| M1 (brief) | `applied \|\| stuck` → `applied` | RED: `a_refused_publication_is_not_retried`, `what_to_tell_the_relay_about_publicity` |
| M2 (brief) | `seen_public` `(None, Some(t))` → `(Private, false)` | RED: `a_publication_made_elsewhere_is_reflected_without_a_put`, `reading_publicity_back_from_the_relay` |
| M3 (brief) | delete `public_applied = None;` in the `start_room` success branch | RED: `restaging_sends_the_publication_again`. The brief allowed reasoning instead; I wrote a permanent test, so this is proven by a test, not argued. |
| M4 (brief) | `live_publish` without `.take(MAX_PUBLIC_TITLE_CHARS)` | RED: `live_publish_uses_the_stored_key_and_truncates_the_title` |
| D2 | no `.trim()` | RED: same test |
| D3 | grant value altered in `handle` | RED: `a_grant_needs_a_live_room` |
| B1 | no retry-at on unreachable (retry every round) | RED: `an_unreachable_publication_is_retried_after_a_backoff` |
| B2 | heal result ignored | RED: `a_publication_the_relay_lost_is_put_once_more` |
| B3 | `set_public_seen` ignores rev | RED: `a_late_publicity_result_for_an_older_intent_is_ignored` |
| B4 | `mark_public` ignores rev | RED: same test |
| B5 | `delete_public` treats HTTP status as retryable | Survived at first. I added `an_unpublish_the_relay_answered_is_not_retried`, and it is now RED. |
| B6 | reset `public_stuck` on `start_room` success (the brief's version) | RED: `restaging_does_not_retry_a_refused_publication` |

Process note: my first batch of mutations in one shell ran past the tool's 600s limit (each mutation recompiles). I killed a test binary that was running normally, which lost the M6/M7 output from that batch. I re-ran every affected mutation on its own under a watchdog (`perl -e 'alarm N'`), and the results above come from those runs. No hang was ever reproduced.

## Flakiness

The thread tests use `until` (2s deadline) plus bounded sleeps (at most `PUSH_INTERVAL*6` = 3s).
- 10 sequential runs of `cargo test --lib -- live::`: 10/10 green, about 3.04s each.
- 3 rounds of 6 concurrent runs of the test binary (18 runs under load): 18/18 green.
- Plus 2 full workspace runs, both green.

The backoff test has timing margin on both sides:
- With backoff, `elapsed` is at least about 980ms; the threshold is `elapsed + 250ms >= 1s`.
- Without backoff, `elapsed` is about 500ms, so the mutant fails clearly.
- The `until` deadline covers the worst case of about 1.5s.

## Deviations from the brief (source and spec win)

1. **`set_public_seen(id, rev, relay_title)`** takes a rev, while the brief has no rev. Without it, a lanes read that started before the teacher pressed "unpublish" writes `Listed` back over `Private`. Nothing corrects that until the next `KEEPALIVE` read 20s later, because the DELETE path doesn't mark state. The task prompt asks for id + public_rev guarding.
2. **`public_stuck` is not reset when `start_room` succeeds.** The brief resets both variables. The spec says 401/403/413 are "不再重试，直到老师自己再按一次 `p`". A restage reopens the room but is not a `p` press, so resetting would re-PUT a revoked or taken-down publication on every restage. `public_applied` is still reset, as the brief says. Pinned by `restaging_does_not_retry_a_refused_publication`.
3. **`delete_public` returns `Result`, and any HTTP answer counts as delivered.** Only a transport error retries, with backoff. The brief's `is_ok()` plus "retry next round" would send a DELETE every 500ms forever against a room the relay answers 401 for (room gone after a relay restart or TTL). A room that doesn't exist can't be public, and the relay's DELETE route has no 429.
4. **The retry-at state is keyed by `(id, rev)`**: `public_retry: Option<((String,u64), Instant)>` instead of a bare `Option<Instant>`. Backoff restarts when the teacher presses `p` again, cancels, or a new live starts, so a new intent doesn't wait out an old intent's 15s backoff. Unreachable and any other non-401/403/413 answer (429, 5xx) back off. This matches the brief's catch-all `Err(reason)` branch.
5. **The Put and Delete outcomes share one match**, so success, stuck, and backoff bookkeeping lives in one place. The behavior is otherwise as in the brief.
6. **The daemon tests don't use `one_staged_session`.** `LiveState::start` accepts any ids, and the existing `stopping_the_broadcast...` test does the same. This avoids spawning a real shell. I added a small `live_call` helper around `handle`.
7. **The stateful FakeSrv** (explained under Tests).
8. **Formatting**: `live.rs` and `daemon.rs` already had rustfmt drift (12 and 4 hunks at HEAD). I formatted only my additions by running rustfmt, then reverse-applying the baseline drift patch. The remaining diff against HEAD contains only lines I touched.

## Concerns

- **"Relay restarted" heal depends on something reopening the room.** The existing `pusher_loop` never resets `started` when pushes start failing, so after a real relay restart it does not call `POST /live/start` again. Pushes fail and the lanes read gets 401, so the publicity heal never triggers either. The spec's "房间因为中转重启或回收被重开后同样再调一次" works for reopens that do happen (restage), and the lanes-based heal covers revocation or takedown while the room exists. Recovering the room itself after a relay restart is a pre-existing gap outside this task, and worth a follow-up.
- **Heal timing**: relay-side loss of publicity is noticed at the viewers-read cadence (`KEEPALIVE`, 20s), as the spec states.
- **Empty title after trim**: not rejected in the daemon. The relay answers 413 and the state goes `Failed(Refused(413))`, stuck. The UI task (title input) should prevent sending an empty title.
- **`handle` is also dispatched from the LAN web path** (`web: None`). Today `web::routes` only issues specific requests, so `LivePublishGrant` is not reachable from the phone page, but any future generic passthrough there would expose the grant.

## Fix round 1

**Change** (`src/live.rs`, commit `c76c6eb`): when a frame push or the lanes
read for the *current* room gets HTTP 401, the pusher now clears `started` so
the next `PUSH_INTERVAL` tick re-registers the room via `start_room` (same
id, viewer token, push secret). The existing success path already resets
`public_applied`, so publicity is re-applied automatically. Transport errors
(`LiveFailure::Unreachable`) and other refusal codes do **not** clear
`started` — they fall through unchanged, still governed by `start_backoff`.

- `push_frame` now returns `Result<(), LiveFailure>` instead of `bool`.
- `fetch_lanes` now returns `Result<(u32, Option<String>), LiveFailure>`
  instead of `Option<...>`.
- `attempt_lane` now returns `(Option<(u64, Instant)>, bool)` — the second
  element says whether this attempt was refused with 401, so `pusher_loop`
  can OR it across all lanes in a round (`saw_401`) and clear `started` once
  after the loop.
- `pusher_loop`'s lanes-fetch branch matches on `Ok`/`Err(Refused(401))`/`Err(_)`
  directly, with the 401 arm clearing `started`.
- Doc comments on `push_frame`, `fetch_lanes`, `attempt_lane`, and the new
  `saw_401` block were updated (Chinese, matching file style) to explain the
  401-vs-transport-error distinction and why it matters.
- Updated all existing callers/tests of `push_frame`/`fetch_lanes`/`attempt_lane`
  for the new signatures (fetch_lanes/push_frame unit tests now compare
  `Result`s; attempt_lane unit tests destructure the tuple).
- `FakeSrv`/`FakeState` in the test module gained a `forget_room: bool` flag:
  while set, `POST /live/frame` and `GET /live/{id}/lanes` answer 401;
  `POST /live/start` is unaffected and clears the flag on receipt (simulating
  successful re-registration after a relay restart).

**New tests** (`src/live.rs`):
1. `a_relay_that_forgets_the_room_is_reregistered_and_republished` — starts a
   live with a real ticking-shell session (so frame pushes fire every
   `PUSH_INTERVAL`, not gated by the 20s `KEEPALIVE` lanes-read cadence),
   publishes it, waits for the first `start`+`PUT`+`Listed`, then flips
   `forget_room = true` (and drops `lanes_public`) to simulate a relay
   restart. Asserts a second `POST /live/start`, a second `PUT /public`, and
   `live.info().public` returning to `Listed`.
2. `a_transport_failure_does_not_trigger_reregistration` — same ticking
   session, but uses the existing `FakeState.fail` connection-drop counter
   instead of 401s. Asserts the `POST /live/start` count stays at 1 across
   several `PUSH_INTERVAL` ticks.

A `ticking_session` test helper spawns a real `/bin/sh` loop via
`crate::profile::Profile::from_toml` + `crate::sys::testing::toml_with_sh`
(same pattern as `session.rs` fixtures) because pusher_loop only re-attempts
a frame push when the screen hash changes or `KEEPALIVE` (20s) elapses —
using a real changing session was the only way to exercise the 401 path
inside a `until()`-scale (2s) test instead of waiting out `KEEPALIVE`.

**Commands + results:**
- `cargo test --lib --no-run` — clean compile, no warnings.
- `cargo test --lib -- live::tests::a_relay_that_forgets_the_room_is_reregistered_and_republished live::tests::a_transport_failure_does_not_trigger_reregistration --nocapture`
  → `test result: ok. 2 passed; 0 failed; ... finished in 2.05s`
- `cargo test --lib live::` → `test result: ok. 80 passed; 0 failed; 0 ignored; 0 measured; 1263 filtered out; finished in 3.04s`
  (78 pre-existing + 2 new), run 3 times consecutively, identical result each
  time — no flakiness observed.
- `cargo test --lib daemon::` → `test result: ok. 42 passed; 0 failed`
- `cargo clippy --lib --tests -- -D warnings` → no output (clean).
- `git diff --check` → clean (no whitespace issues).

**Mutation check:** temporarily replaced the `started = None;` inside the
`if saw_401 { ... }` block (the one actually exercised by the ticking-session
test, since the lanes-read 401 path is gated by the 20s `KEEPALIVE` and never
fires within the test window) with a no-op. Re-ran the relay-restart test:
it failed as expected —
`panicked at src/live.rs:1841:9: 等了两秒也没等到：中转忘了房间之后重新 start`.
Restored the fix; full `live::` suite green again afterward.

**Left to the controller:** the full workspace test suite
(`cargo test --workspace` / unfiltered `cargo test`) was not run per the
task's hard constraints — only `--lib live::` and `--lib daemon::` were run
here.
