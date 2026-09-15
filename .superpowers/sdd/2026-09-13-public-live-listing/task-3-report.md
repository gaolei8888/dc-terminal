# Task 3 report: `dct-srv` — per-room public state

## Implemented

`crates/dct-srv/src/live.rs`:

- `Session` gained a `public: Option<Public>` field (`Public { title, key_name }`). `start` always sets `public: None` on insert, so reopening a room never inherits its old public state.
- `start`'s id-charset check now also rejects `id == RESERVED_LIVE_ID`.
- New `pub enum Control<'a> { Push(&'a str), Grant(&'a str) }` and `fn controls(room, id, &Control) -> bool` (constant-time, mirrors `same`).
- New `pub struct PublicEntry { id, title, lanes, viewers }` (`Debug, Clone, PartialEq, Eq, Serialize`).
- New `Live::publish(id, control, keys: Option<&PublishKeys>, key, title) -> Result<(), LinkError>`: checks control → total off-switch (`NotYours`) → key validity (`Unauthorized`) → blocklist (`NotYours`) → title length 1..=60 chars (`TooBig`), in that order, matching the spec's answer table.
- New `Live::unpublish(id, control) -> Result<(), LinkError>`: idempotent (no-op success when room isn't public or room doesn't... — actually room must exist, only lack of `public` is idempotent), requires `controls`.
- New `Live::public_list() -> Vec<PublicEntry>`, sorted by viewers desc then id, never includes `key_name`/publisher.
- New `Live::reconcile(keys: Option<&PublishKeys>)`: drops `public` when keys is `None` (feature off), key was revoked (name no longer in `keys.keys`), or room id is blocked.
- `authed` now takes `token: Option<&str>`: `None` succeeds only if `room.public.is_some()`; a present-but-wrong token is still rejected even on a public room.
- `Live::frame`/`Live::lanes` signatures changed to `token: Option<&str>`; `lanes` now returns `(Vec<String>, Option<String>)` (lane names, public title or `None`).
- `use` additions: `dct_link::live::{MAX_PUBLIC_TITLE_CHARS, RESERVED_LIVE_ID}`, `crate::keys::PublishKeys`.

`crates/dct-srv/src/lib.rs` (minimal, Task 4 will rewrite):
- `live_frame_route`: both `live.frame(&id, token, lane)` calls → `live.frame(&id, Some(token), lane)`.
- `live_lanes_route`: `live.lanes(&id, token)?` → `live.lanes(&id, Some(token))?.0` (keeps the existing `LiveLanesResponse { lanes, viewers }` shape; the tokenless/public path and the new title field are explicitly deferred to Task 4).

## Deviations from the plan's reference code

- None functionally — the real source matched the brief closely. Only adjustment: `unpublish`'s doc comment as written literally says "没公开也回成功" (returns success even if not public), which is accurate — the room must still exist (`rooms.get_mut(id).ok_or(Unauthorized)?`); "idempotent" refers to the public flag, not room existence. Implemented exactly as specified; no code deviation.
- Confirmed via source read (not the plan) that `Session` is constructed only once, in `start`'s `rooms.insert(...)`, and that `push`/`stop` return `Result<u64/(), LinkError>` — matches what the plan assumed, so `Control` and `controls` needed no adjustment.
- Confirmed `LinkError` already has `NotYours` (used elsewhere for "not your device"); no new variant needed.

## TDD evidence

RED (Step 2), compile failure as expected:
```
$ cargo test -p dct-srv --lib live::
error[E0599]: no method named `public_list` found for struct `live::Live` ...
error[E0433]: cannot find type `Control` in this scope
... (64 errors total, all "not found"/"mismatched types" for Control/publish/unpublish/public_list/reconcile/frame(Option)/lanes(Option))
```

GREEN (Step 4):
```
$ cargo test -p dct-srv --lib live::
running 27 tests
... all ok
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out
```

Full workspace:
```
$ env -u TERM cargo test --workspace --locked --no-fail-fast
... 0 failed anywhere (checked via grep -c " ... FAILED" => 0)
dct-srv lib: 63 passed; 0 failed
```

Clippy:
```
$ cargo clippy --workspace --all-targets --locked -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.5x s
```
(no warnings)

`git diff --check`: clean (no whitespace errors).

## Mutation checks (Step 5) — all confirmed RED, then reverted

1. `authed`: `None if room.public.is_some() => Ok(room)` → `None => Ok(room)` (drop the guard).
   Result: `tokenless_reads_only_reach_public_rooms` FAILED — `left: None, right: Some(Unauthorized)` on the "私密房间" assertion. Reverted.

2. `publish`: removed the `if keys.is_blocked(id) { return Err(LinkError::NotYours); }` block.
   Result: `publishing_follows_the_answer_table` FAILED — the blocklist assertion got `Ok(())` instead of `Err(NotYours)`. Reverted.

3. `reconcile`: collapsed `keep`'s match to `(_, None) => continue, _ => true`.
   Result: `reconcile_withdraws_revoked_blocked_and_disabled_publications` FAILED — "revoke 之后还挂在公开列表上". Reverted.

4. `start`: made it inherit the old room's `public` on reopen (`rooms.remove(&id).and_then(|r| r.public)` instead of `None`, since `insert` replaces the whole `Session`).
   Result: `reopening_a_room_does_not_inherit_its_public_state` FAILED — `public_list().is_empty()` assertion failed. Reverted.

5. `controls`: `Control::Grant(_grant) => true` (always controls).
   Result: `publishing_follows_the_answer_table` FAILED — the bad-grant assertion (`Control::Grant("00")`) got `Ok(())` instead of `Err(Unauthorized)`. Reverted.

## rustfmt handling

`crates/dct-srv` has pre-existing rustfmt drift (7 unformatted spots already in the file before this task, e.g. `starts.insert(...)` on one line, several test one-liners, the `PUT`-only route chain in `lib.rs`, and a `let seen` binding). Per instructions I did not run `cargo fmt` over the whole crate blindly. Process used:
1. Saved copies of both files, ran `cargo fmt -p dct-srv`, diffed before/after.
2. Confirmed the drift baseline is identical before and after my task's edits (same 7 `Diff in ...` locations reported by `cargo fmt -p dct-srv -- --check`), by comparing against the pre-task version of the file (retrieved via `git stash`/`stash apply`, immediately dropped after use — never a bare `git stash`/`pop`).
3. Kept rustfmt's reformatting for all of my own new code (the `publish`/`unpublish`/`public_list`/`reconcile`/`controls`/`authed` bodies, `Control`/`PublicEntry`/`Public`, and every newly-added test), and manually reverted the fmt tool's reformatting of five pre-existing, content-unchanged lines that weren't mine (the `starts.insert` one-liner and four `live.push(...).unwrap()` one-liners in pre-existing tests).
4. Final `cargo fmt -p dct-srv -- --check` reports exactly the same 7 pre-existing hunks as before this task — no new drift introduced, no drift fixed that wasn't mine to fix.

## Files changed

- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/public-live/crates/dct-srv/src/live.rs`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/public-live/crates/dct-srv/src/lib.rs`

## Commit

`cea5faa` — `feat(srv): rooms can be published, read without a token while public, and withdrawn` (no AI attribution, per brief/global constraints).

## Concerns for Task 4

- `live_lanes_route`'s response still requires a token (`header(...).ok_or(Unauthorized)?`) and drops the new public-title value (`.0`) — this is intentional per the brief ("Task 4 会重写"), but it means the tokenless/public-list HTTP surface doesn't exist yet after this task; that's expected, not a gap in Task 3.
- `PUBLISH_KEYS_RELOAD` / actually wiring `reconcile` into a periodic reload loop, and the `PUT/DELETE /live/{id}/public` + `GET /live/public` routes, are explicitly out of scope for Task 3 per the brief.

---

## Fix round 1 (review finding, Important)

**Finding:** `reconcile` kept a publication alive if *any* current key in the file had the same `name` as the one that published it (`k.keys.iter().any(|e| e.name == p.key_name)`). `PublishKeys::add` only rejects duplicate names among entries still present, and `revoke` removes the old entry outright. So the ordinary rotation sequence `key revoke 姜老师` then `key add 姜老师` (both well within one 10s reload window) produces a file that once again has an entry named `姜老师` — with a *different* hash — and `reconcile` treated that as "the same publisher is still valid," leaving every room the leaked/revoked key had published still public. This broke the spec guarantee that a revoked key's publications go private within 10s.

**Fix — what changed:**

- `crates/dct-srv/src/keys.rs`:
  - Added `PublishKeys::entry_for(&self, key: &str) -> Option<(String, String)>` — same constant-time (`same_hex`) scan as the old `name_for`, but returns `(name, hash)` instead of just `name`. `name_for` is now implemented in terms of it (`self.entry_for(key).map(|(name, _)| name)`), so existing callers/tests of `name_for` are unaffected.
  - Added `PublishKeys::has_hash(&self, hash: &str) -> bool` — plain `==` scan over `self.keys`. Per the controller's ruling, this is fine without constant-time comparison because the digest being tested (`Public::key_hash`, read from the room, not attacker input at this call site) and the file's stored digests are not caller-controlled at the point `reconcile` calls it — there's no secret being guessed here, unlike `name_for`/`entry_for` where `key` is a caller-supplied plaintext.
- `crates/dct-srv/src/live.rs`:
  - `Public` gained a `key_hash: String` field (the publishing key's `KeyEntry.hash`). `key_name` stays, marked `#[allow(dead_code)]` with a comment — it's kept for a future logs/admin surface (nothing in this crate logs yet), so `reconcile` intentionally never reads it.
  - `Live::publish` now calls `keys.entry_for(key)` (was `name_for`) to capture both `key_name` and `key_hash`, and stores both in `Public`.
  - `Live::reconcile`'s `(Some(k), Some(p))` arm is now `!k.is_blocked(id) && k.has_hash(&p.key_hash)` — keeps a publication only if the *exact* key that published it is still present in the file (by digest), regardless of what name is attached to it now.
  - Added test `reconcile_revokes_by_key_not_by_name`: publishes with a key named "姜老师", revokes "姜老师", adds a *new* key also named "姜老师" (different secret ⇒ different hash), calls `reconcile`, and asserts `public_list()` is empty and `frame("abc", None, 0)` is `Err(Unauthorized)`.

**Mutation check (this fix's new test):** reverted `reconcile`'s hash check back to the old name-matching (`!k.is_blocked(id) && k.keys.iter().any(|e| e.name == p.key_name)`) — `reconcile_revokes_by_key_not_by_name` failed:
```
thread 'live::tests::reconcile_revokes_by_key_not_by_name' panicked at crates/dct-srv/src/live.rs:1032:9:
吊销后同名重开，不该借新钥匙的条目继续公开
test result: FAILED. 0 passed; 1 failed
```
Reverted back to the hash-based check; test passes again.

**A second issue found and fixed while implementing this:** capturing `key_hash` alone (without touching `key_name`'s usage) left `key_name` completely unread, which `cargo clippy -- -D warnings` correctly flagged as `dead_code` and turned into a hard build failure (`error: field 'key_name' is never read`). Fixed with `#[allow(dead_code)]` plus a comment explaining why the field is kept unused for now (reserved for a future admin/audit surface, per the controller's "key_name stays for logs" ruling) rather than removing it or fabricating a spurious reader.

**Covering tests run:**
```
$ cargo test -p dct-srv
... (lib) test result: ok. 64 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
... (main) test result: ok. 0 passed; 0 failed
... (tests/serves.rs) test result: ok. 3 passed; 0 failed
... (doc-tests) test result: ok. 0 passed; 0 failed
```
```
$ cargo clippy --workspace --all-targets --locked -- -D warnings
    Checking dct-srv v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.45s
```
Also re-ran the full workspace suite (`env -u TERM cargo test --workspace --locked --no-fail-fast`): 0 failures anywhere, and `git diff --check` is clean.

**rustfmt drift:** the new test (`reconcile_revokes_by_key_not_by_name`) initially left 2 more unformatted lines beyond the crate's known 7-hunk baseline; reformatted those two by hand to match what `cargo fmt -p dct-srv` would produce (multi-line `live.publish(...)` call and multi-line `assert_eq!` for the frame check), then re-checked — `cargo fmt -p dct-srv -- --check` now reports exactly the same 7 pre-existing hunks as before this fix (same content, shifted line numbers), no new drift introduced.

**Commit:** `285f05f` — `fix(srv): reconcile revoked publications by key digest, not name` (no AI attribution). Used `git status`/`git diff` throughout to inspect state; did not touch the shared stash stack for this round.

**Files changed (fix round 1):**
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/public-live/crates/dct-srv/src/keys.rs`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/public-live/crates/dct-srv/src/live.rs`
