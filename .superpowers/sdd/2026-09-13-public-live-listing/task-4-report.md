# Task 4 report: `dct-srv` HTTP routes for publishing and key file reloading

## Summary

Implemented per the brief with one meaningful deviation from the reference code (noted below). All required routes, the `AppState.keys` field, the key-file reload task in `serve`, and `main.rs` wiring are in place. Controller Ruling 1 applied: `/` is now the public listing page on both `Routes` tiers, and the phone page moved to `/phone` under `Routes::WithLink`.

## Files changed

- `crates/dct-srv/src/lib.rs`
- `crates/dct-srv/src/main.rs`
- `crates/dct-srv/tests/serves.rs`

(`crates/dct-srv/src/live.rs` and `crates/dct-srv/src/keys.rs` untouched — confirmed via `git diff --stat` before commit.)

## What was implemented

1. `AppState` gained `pub keys: Arc<std::sync::RwLock<Option<crate::keys::PublishKeys>>>`.
2. `token_header()` and `control_header()` helpers next to `header()`.
3. `live_frame_route` now uses `token_header` (no `.ok_or(Unauthorized)`), passing `Option<&str>` straight to `live.frame`.
4. `live_lanes_route` returns `LiveLanesResponse { lanes, viewers, public: Option<PublicTitle> }`, reading `token_header` for auth (so token-less requests reach public rooms).
5. New routes/handlers:
   - `PUT/DELETE /live/{id}/public` → `live_publish_route` / `live_unpublish_route`
   - `GET /live/public` → `live_public_list_route`
   - `GET /` → `public_page_route` (placeholder `Html("<!doctype html><title>公开直播</title>")`, per brief — Task 5 will replace with `dct_page::public_page()`)
6. `router()`:
   - `/` and `/live/public` mounted unconditionally (both `Routes` tiers).
   - `/live/{id}/public` mounted unconditionally with its own 16 KB `DefaultBodyLimit` layer (same rationale as `PATH_START`).
   - `Routes::WithLink` branch: phone page moved from `/` to `/phone`; `/link/*` unchanged.
   - Updated doc comments on `router()` and the `Routes` enum variants to reflect the new `/` = public list, `/phone` = phone page assignment.
7. `serve()`:
   - New 5th parameter `keys: Option<crate::keys::KeyFile>`.
   - Builds `shared: Arc<RwLock<Option<PublishKeys>>>` seeded from the initial key file (or `None`).
   - If a key file was given, spawns a 10s-interval (`PUBLISH_KEYS_RELOAD`) reload task: on change, calls `live.reconcile(Some(&fresh))` **before** publishing the new keys into `shared` (comment explains why: otherwise there's a window where a revoked key is still visible to the publish route); on parse failure, logs and keeps the last-good keys; on no-change, does nothing.
   - Constructs `AppState { relay, live, keys: shared }`.
8. `main.rs`: replaced `let _ = publish_keys;` with passing `publish_keys` (the opened `KeyFile`) into `serve(...)` as the 5th argument. Captured the original path (`publish_keys_path`) before it's consumed by `KeyFile::open`, and added a startup message segment: `；已开启公开直播（密钥文件：<path>）` when a key file was configured.
9. `tests/serves.rs`: both `dct_srv::serve(...)` calls updated with a trailing `None` (no key file — these tests don't exercise publishing).

## Deviations from the reference code (brief said it's not authoritative)

- **`Rejected`/`LinkError` mapping**: verified against the real `impl IntoResponse for Rejected` — no changes needed; `NotYours → 403`, `Unauthorized → 401`, `TooBig → 413`, `Busy → 429` already matched the brief's answer table exactly.
- **`AppState` derive**: already `#[derive(Clone)]`; `PublishKeys` already derives `Clone` in `keys.rs`, so `Arc<RwLock<Option<PublishKeys>>>` clones cheaply as required by `serve`'s reload task closure.
- **`page.html` `/` dependency check** (explicitly asked for in the task): grepped `page.html` for `location.` and `fetch(`. All `fetch()` calls target absolute paths (`/link/ask`, a `path` variable built from `/api/...`/`/link/...` constants). The only `location.pathname` use is `history.replaceState(null, "", location.pathname)`, which strips the URL hash after claiming a cookie token — it does not hardcode or assume `/`. Confirmed no relative-path dependency on being served at `/`; moving the phone page to `/phone` is safe.
- **`router()`/`app()`/`app_with_live()` construction**: matched the brief's reference code closely; only change was adding the `keys` field to `AppState` literals in test helpers, exactly as specified.
- **rustfmt scope accident (caught before committing)**: I initially ran `rustfmt --edition 2021 crates/dct-srv/src/lib.rs crates/dct-srv/src/main.rs` to sanity-check formatting of my own additions. Because `lib.rs` declares `mod live;`, rustfmt treated it as a crate root and reformatted all of `live.rs` too (which I did not intend to touch and which has pre-existing drift per the task's warning). I reverted `live.rs` with `git checkout --` before staging/committing, and reverted `lib.rs`/`main.rs` to my hand-written versions from a `/tmp` backup rather than keeping the auto-formatted versions, per the instruction not to run `cargo fmt`/`rustfmt` over the whole crate. `git diff --check` is clean; `cargo clippy -D warnings` is clean without any reformatting.
- No other functional deviations: `Live::note_start` is `pub fn` as expected; `FromRef<AppState> for Arc<Live>`/`Arc<Relay>` already existed and needed no changes; `State<AppState>` extraction in `live_publish_route` works because `AppState: Clone` and matches the router's declared state type exactly.

## TDD evidence

I implemented the routes and their tests together (both new tests and the required `AppState`/helper changes were added as part of the same edit pass, since the brief's Step 1/3 code is tightly coupled — the test helpers reference the exact struct shape being introduced). To provide genuine RED/GREEN evidence I ran targeted **mutation checks** (Step 5) that reintroduce exactly the bugs the tests are meant to catch, confirmed each fails, then reverted:

### Mutation 1 — `token_header` drops `.filter(|t| !t.is_empty())`

```
fn token_header(headers: &HeaderMap) -> Option<&str> {
    header(headers, "x-live-token")
}
```
```
$ cargo test -p dct-srv --locked --no-fail-fast an_empty_token_header_counts_as_no_token
failures:
    tests::an_empty_token_header_counts_as_no_token
test result: FAILED. 0 passed; 1 failed; ...
```
Reverted → passes.

### Mutation 2 — `live_publish_route` ignores the loaded keys (`keys.as_ref()` → `None`)

```
state.live.publish(&id, control, None, &req.key, &req.title)?;
```
```
$ cargo test -p dct-srv --locked --no-fail-fast publishing_over_http_end_to_end
thread 'tests::publishing_over_http_end_to_end' panicked:
assertion `left == right` failed
  left: 403
 right: 204
test result: FAILED.
```
Reverted → passes.

### Mutation 3 — `LiveLanesResponse` drops the `public` field

```
struct LiveLanesResponse { lanes: Vec<String>, viewers: u32 }
```
```
$ cargo test -p dct-srv --locked --no-fail-fast publishing_over_http_end_to_end
thread 'tests::publishing_over_http_end_to_end' panicked:
{"lanes":["前端"],"viewers":0}
test result: FAILED.
```
(The assertion checking `lanes.contains(r#""public":{"title":"第3课"}"#)` failed as expected.) Reverted → passes.

All three mutations confirmed RED, all three reverts confirmed GREEN (verified by rerunning `cargo test -p dct-srv` and the full workspace suite after each revert).

## Full verification run (after final revert, before commit)

```
$ env -u TERM cargo test --workspace --locked --no-fail-fast
... (all crates) ...
test result: ok. 1314 passed; 0 failed; 0 ignored; ...   # dct
test result: ok. 69 passed; 0 failed; 0 ignored; ...      # dct-srv lib
test result: ok. 3 passed; 0 failed; 0 ignored; ...       # dct-srv tests/serves.rs
test result: ok. 13 passed; 0 failed; 0 ignored; ...      # dct-link
test result: ok. 7 passed; 0 failed; 0 ignored; ...       # dct-page
(all other test binaries: ok, 0 failed)

$ cargo clippy --workspace --all-targets --locked -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.55s   # clean, no warnings

$ git diff --check
(clean, no output)
```

dct-srv lib test count: 69 (was 64 before this task — 5 new tests: `publishing_over_http_end_to_end`, `a_grant_header_can_publish`, `without_a_key_file_publishing_is_forbidden_and_the_list_is_empty`, `publishing_without_proof_of_control_is_unauthorized`, `an_empty_token_header_counts_as_no_token`).

## Controller Ruling 1 — applied

- `the_default_relay_does_not_answer_the_unauthenticated_routes` rewritten: now asserts `PATH_POLL`, `PATH_SEND`, `PATH_ASK`, and `/phone` are 404 on the default (`LiveOnly`) relay; `/` returns 200 (public page); `/live/abc` still returns 200. Doc comment updated to explain `/` is the deliberate exception (public listing, meant to be readable by anyone).
- `the_relay_serves_the_very_same_page_the_daemon_does` updated to request `/phone` instead of `/` (this test uses `Routes::WithLink` via the `app()` helper).
- `Routes` enum doc comments updated: `LiveOnly` now documents it includes `/` and `/live/public`; `WithLink` now says `/phone` instead of `/`.
- `router()`'s top doc comment and `page_route`'s doc comment updated to say the phone page is `/phone`, not `/`.

## Concerns / notes for follow-on tasks

- `public_page_route` is an explicit placeholder (`Html("<!doctype html><title>公开直播</title>")`) per the brief — Task 5 must replace it with `dct_page::public_page()` and should double check the placeholder text doesn't leak into any snapshot/golden test in the meantime (none currently assert on `/`'s body content in `dct-srv`, confirmed by grep).
- `live_publish_route` calls `note_start` (rate limiting) **before** checking `control_header`, matching `live_start_route`'s existing pattern from the reference code and the brief — this means a request with no proof of control still consumes a rate-limit slot from the same per-source bucket used by `POST /live/start`. This is intentional (per brief: "跟建房共用按来源的限流（同一本账）") but worth flagging since it means an attacker without any credentials can still exhaust a legitimate publisher's start/publish quota from the same source IP bucket — this is the existing `MAX_STARTS_PER_WINDOW`/`client_key` design, not something new introduced here.
- Protocol version bump (18→19) is explicitly out of scope for this task per global constraints ("Task 6 一次到位") — not touched.

## Fix round 1 (review findings)

Commit: `56c6e42 fix(srv): reorder key reload to close a publish race, add tokenless frame HTTP tests`

### Finding 1 — reload ordering let a revoked/blocklisted room stay public

**Root cause confirmed as described.** The reload task previously called `live.reconcile(Some(&fresh))` (which only touches the `rooms` lock) and *then* swapped `fresh` into `shared` under the `keys` write lock. Between those two steps, a `live_publish_route` in flight — holding the *old* `keys` read guard while it called `Live::publish` — could validate against a since-revoked/blocked key and set `room.public`, and no further `reconcile` would run until the next file change, breaking the "revoked/blocked within 10s" guarantee.

**Fix — swapped the order** in `crates/dct-srv/src/lib.rs`'s `serve()` reload task:

```rust
Ok(true) => {
    let fresh = file.keys().clone();
    // 先换密钥，再 reconcile...
    *shared.write().expect("keys 锁") = Some(fresh.clone());
    live.reconcile(Some(&fresh));
}
```

Now the write lock on `shared` blocks until every in-flight `live_publish_route` call (which holds the `keys` read guard across the whole `Live::publish` call — verified unchanged, `crates/dct-srv/src/lib.rs:552-565`) has released it. Once the write lock is acquired and the fresh keys are installed, `reconcile` runs with those same fresh keys, withdrawing anything a request managed to publish with the old (now-superseded) keys while queued behind the write lock. Rewrote the surrounding comment to explain this ordering and why the reverse order is unsafe (matches the controller ruling verbatim).

No test was written to *observe* this race directly (it would require synchronizing a slow request against a concurrent reload, i.e. a timing-dependent integration test) — the fix is verified by code inspection matching the ruling's described mechanism, plus the existing `live::tests::reconcile_*` unit tests (unaffected, still pass) confirming `reconcile`'s own semantics are unchanged.

### Finding 2 — tokenless frame reads untested over HTTP

Added two new tests in `crates/dct-srv/src/lib.rs` (`mod tests`), using the `app_with_keys`/`app_with_live`/`call` helpers already in place:

- `a_published_rooms_frame_can_be_read_without_a_token_over_http`: publishes room `abc`, pushes a frame, then asserts `GET /live/abc/frame?lane=0` returns 200 with no `x-live-token` header, and 200 again with `x-live-token: ""`.
- `a_private_rooms_frame_stays_401_without_a_token_over_http`: starts room `abc` without publishing, asserts the same two requests (no header, empty header) both return 401.

**Mutation check** (Step 5 style, as requested): reverted `live_frame_route`'s token handling from `token_header(&headers)` back to `header(&headers, "x-live-token").ok_or(LinkError::Unauthorized)?` (wrapping the two `live.frame(&id, token, lane)` call sites back in `Some(token)`):

```
$ cargo test -p dct-srv --locked --no-fail-fast a_published_rooms_frame_can_be_read_without_a_token_over_http
thread 'tests::a_published_rooms_frame_can_be_read_without_a_token_over_http' panicked at crates/dct-srv/src/lib.rs:1854:9:
assertion `left == right` failed: 公开房间没带令牌该读得到帧
  left: 401
 right: 200
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 70 filtered out; finished in 0.01s
```

Confirmed RED, then reverted the mutation back to `token_header`.

### Covering test run (after both fixes, mutation reverted)

```
$ env -u TERM cargo test -p dct-srv --locked --no-fail-fast
running 71 tests
... (all pass, including the two new tests) ...
test result: ok. 71 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.06s
     Running tests/serves.rs ...
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s

$ env -u TERM cargo test --workspace --locked --no-fail-fast
... all 1314+ tests across the workspace: ok, 0 failed ...

$ cargo clippy --workspace --all-targets --locked -- -D warnings
    Checking dct-srv v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.56s   # clean

$ git diff --check
(clean)
```

Only `crates/dct-srv/src/lib.rs` changed in this fix round (`git status` confirmed before commit); no unrelated reformatting.

dct-srv lib test count: 69 → 71 (the two new tokenless-frame HTTP tests from Finding 2).
