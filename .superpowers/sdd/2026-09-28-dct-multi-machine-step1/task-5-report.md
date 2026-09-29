# Task 5 report: machine keys on disk, generalised link, mesh::Net, daemon relay connection

## Status

Done. Commit `852e7e4` on feat/dct-multi-machine-step1 (base 453a1ea).

## What changed

- `src/mesh/store.rs` (new): `Store::at`, `default_dir()` (`~/.dct/mesh`), `dir_for_socket`,
  `load_or_create_keys(rand)`, `name`/`set_name`, `roster`/`save_roster`.
  - Seeds are stored as base64 in `sign.key`/`kx.key`, created with `create_private`. The directory goes through `restrict_dir_to_owner`.
  - Every write uses a temp file, then `sync_all`, then rename.
  - `kx.key` is written before `sign.key`, so if `sign.key` exists, both keys are complete. A leftover `kx.key` on its own gets regenerated. A `sign.key` without `kx.key` is an error; the code does not quietly create a new key.
  - A bad P-256 seed is retried up to 8 times.
  - The default name is the hostname, cleaned up (`.local` suffix and `/` removed, truncated to 32 characters, `dct` if empty). `set_name` enforces the same rules as the roster.
- `src/link.rs`:
  - New types and functions: `Handler`, `Link::with_handler`, `proto_handler`. `Link::new(cfg, dispatch)` is now `with_handler(cfg, proto_handler(dispatch))` and behaves as before.
  - A handler that returns `None` sends nothing back.
  - `LinkConfig.token` is now a shared `Token` cell (`Arc<Mutex<String>>`, and its `Debug` output hides the value). A renewal is therefore seen by both `Link` and `LinkNet`.
  - `Link::before_poll(hook)` runs on the link thread before each poll, wrapped in catch_unwind. Token renewal uses it.
  - New functions: `agent(cfg)`, `peers`, `send`, `ask` and `ask_within(timeout)`.
  - New `link::LinkError { Relay(dct_link::LinkError), Unreachable, BadEndpoint }`. Relay error bodies are parsed into their `ErrorBody` code.
- `src/mesh/net.rs` (new):
  - `Net` trait (`peers` / `send` / `ask(to, payload, timeout)`), following the controller ruling.
  - `LinkNet`: `seq` starts at unix milliseconds, so after a restart it doesn't collide with asks the relay is still holding. `ask` checks that the reply's from and seq match the request.
  - `#[cfg(test)] pub mod testing { FakeHub, FakeNet }`. Meshes register by endpoint, and `send`/`ask` call the target's `on_envelope` synchronously. There is also `set_online`, and `peers` returns the other online endpoints, sorted.
- `src/mesh/mod.rs`:
  - `Mesh` has fields `keys`, `me`, `roster`, `pending_joins`, `queues`, plus private `store`, `journal`, `seen`, `clock`, `rand`. Constructors and builders: `Mesh::new`, `Mesh::load(store)`, `with_store`, `with_journal`, `with_clock`, `with_rand`, `endpoint()`.
  - `on_envelope` handles each payload as follows:
    - `Roster`: kept only if `accept(Some(current), ..)` passes, then saved. If the save fails, the in-memory roster is not replaced either.
    - `Sealed`: opened with `seal::open`, then checked against a ±10-minute `sent_at` window, then deduplicated on `(from, id)` against a bounded set of the last 1024 ids.
      - `Msg`: added to `queues`, keyed by `#N`, or `UNRESOLVED_SESSION = 0` for names (Task 7 resolves those). The reply is a sealed `Receipt` with body `"queued"` and the original id.
      - `StatusRequest`: the reply is a sealed `Status` with body `{"os": ...}`.
      - `Receipt`/`Status`: ignored.
    - `Join`/`JoinPending`: ignored (Task 6).
    - Any failure: the envelope is dropped, one journal line is written (`mesh  dropped from=<ep> why=<reason>`), and no reply is sent.
  - `route(mesh, proto: Option<Handler>)` sends envelopes from `c-` endpoints to the mesh and everything else to `proto`.
  - Also: `DEFAULT_RELAY`, `COMPUTER_PREFIX`, `os_rand` (getrandom).
- `src/mesh/login.rs`:
  - `DC_PROFILE = "dc"`.
  - `http_transport`, the production ureq transport; tests never call it.
  - `renew_if_due(secrets, token, origin, endpoint, now, send) -> Renewal{NotDue, Renewed, Failed}`:
    - A missing or unparseable expiry counts as due.
    - The secrets lock is not held during the HTTP call.
    - On success, both the in-memory token cell and the secrets file are updated. On failure, the old token is kept.
- `src/config.rs`: `[mesh] relay` section (`MeshConfig::relay()`, which falls back to `DEFAULT_RELAY` when the value is missing or blank).
- `src/journal.rs`: `Journal::mesh(event)`.
- `src/daemon.rs`: `start_mesh(socket, secrets, profiles_dir, journal_path) -> Option<MeshRuntime{mesh, net, link}>`, called in `run_with_manager` just before the tick thread starts.
  - It does nothing unless `__relay__` is in secrets.
  - Paths come from the socket location, so tests stay isolated.
  - The origin comes from `pair_origin(profiles_dir, "dc")`.
  - Renewal runs through `before_poll` on the link thread. After a failure it waits an hour before retrying, and every renewal outcome is logged to the journal.
  - Nothing runs on the 200ms tick.

## Tests / commands / output

```
$ ~/.cargo/bin/cargo test --workspace   -> exit=0
  dct lib: test result: ok. 1458 passed; 0 failed
  (every other crate and integration binary: ok, 0 failed)
$ ~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile ... (no warnings)
$ ~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
    warning: unused variable: `path` --> src/student_projects.rs:669:13   (pre-existing, unrelated)
    Finished `dev` profile ...
$ rustfmt --check --edition 2021 src/mesh/*.rs src/link.rs  -> exit 0
$ git diff --check -> clean
```

New tests (52 under `mesh`, plus the link and daemon tests below):
- store:
  - Keys: created once and loaded back the same, also by a reopened store; a bad seed is retried; a half-written pair is regenerated; `sign.key` without `kx.key` is an error.
  - Files are 0600 and the directory 0700 (unix).
  - Roster: reads back after saving; a failed write leaves the old roster intact (the tmp path is blocked by a directory); no tmp file is left behind.
  - Names: the default is valid; invalid names are refused; hostnames are cleaned up.
- link (all 7 existing tests unchanged):
  - `with_handler` replies with the same `seq`, addressed back to `from`.
  - A handler returning `None` sends nothing.
  - A renewed token is used on the next poll, and `before_poll` runs.
  - `peers`, `ask` (auth is carried), a relay refusal comes back as its code, an unreachable relay gives `Unreachable`.
  - The token doesn't appear in `Debug` output.
- mesh:
  - A `Msg` is queued and answered with a sealed `Receipt`; a `StatusRequest` gets a sealed `Status`.
  - A sealed message from a machine not in the roster is dropped with no reply and exactly one journal line.
  - Replays are dropped; `sent_at` outside the window (past and future) is dropped, and the edge value is accepted; the seen set is bounded.
  - A roster with a bad signature is dropped and the old one kept; a valid newer roster is taken and saved; a machine with no group won't take a roster from the network.
  - Garbage is dropped.
  - Routing: `c-` goes to the mesh, `phone:` to proto, and nothing is sent when `proto` is `None`.
  - FakeHub: ask/peers/offline.
  - `Mesh::load` keeps the same endpoint.
- net: envelope `from`/`seq`, `seq` doesn't restart from 0, a bad endpoint is refused.
- login renewal (5 tests): not due; renewed in memory and on disk; failure keeps the old token; missing expiry counts as due; no DC key or no origin never touches the network.
- config: relay default, override, and blank value.
- daemon (`mesh_tests`, using a local fake relay on 127.0.0.1): no token means no mesh and no files on disk; with a token, the daemon polls the configured relay with this machine's `c-` endpoint, the token and `kind=Computer`; after a restart the endpoint is the same.

Mutation checks:
- Making the roster write non-atomic makes `a_failed_roster_write_leaves_the_old_roster_intact` fail.
- Letting `take_roster` use `accept(None, ..)` makes `a_machine_with_no_group_does_not_take_a_roster_off_the_wire` fail.

## Deviations / decisions

1. **A machine not in a group never takes a roster from the network.** The brief says "keep it if `roster::accept(current, incoming)` passes". But `accept(None, ..)` only checks for a self-signed genesis roster, which anyone in the same account can make. So `on_envelope` drops any `Roster` when `current` is `None`. The first roster a machine takes has to come through Task 6's `accept_invite`.
2. **The phone path is not wired to the relay.** The daemon calls `route(mesh, None)`, so envelopes from other endpoints get no answer. The proto path still exists (`proto_handler` / `Link::new`) and is tested through `route`. This follows the brief's "手机那条路这一步不接，但保留" (the phone path isn't connected in this step, but is kept).
3. **`link::LinkError` is a new local enum** wrapping `dct_link::LinkError`, because network failure has no relay code. `Net` uses it.
4. `LinkConfig.token` changed from `String` to a shared `Token` cell. `LinkConfig::new` has the same signature.
5. **Placeholder bodies:** the `Receipt` body is `"queued"` and the `Status` body is `{"os":...}`. Task 7 defines the real content (`SendOutcome`, sessions, tentacles).
6. **A duplicate `(from, id)` is dropped silently**, with no receipt sent again. If Task 7 retries an `ask` with the same id, the retry will get no answer, so retries should use a new id.
7. `MeshRuntime` has `#[allow(dead_code)]` because `mesh` and `net` are only read from Task 6 on. It is held in `run_with_manager` as `_mesh`.

## Concerns

- **Logging in (`dct login`, Task 6) will need to start the link while the daemon is running.** `start_mesh` currently runs only at daemon startup. Task 6 should keep the runtime in a slot and call `start_mesh` again after it writes the token.
- **Dedup can be beaten by volume.** The seen set holds 1024 ids. A sender pushing more than 1024 messages inside the 10-minute window could push an id out of the set and replay it. This was accepted by the controller.
- **Holding locks while calling `Net` can deadlock under FakeHub.** FakeHub calls `on_envelope` synchronously while holding the target's lock. Tasks 6/7 must not call `Net` while holding their own `Mesh` lock. This is documented in `net.rs`.
- **`cargo fmt --all` reformats unrelated files in this repo** (stable rustfmt formats differently from what's committed). I ran it by accident, then reverted every unrelated file and hunk. Only my files are in the commit.

## Fix round 1

Commit `9d3563f`: fix(mesh): refuse pre-restart replays and never silently change machine identity.

1. **Replay after a restart (Important).**
   - `Mesh` now has a `started_at` field. It is set to unix now in `new`, and `with_clock` resets it to that clock's "now".
   - `take_sealed` drops any message whose `sent_at` is earlier than `started_at` (journal reason `before_start`). This check runs after the window check and before dedup.
   - `Seen` now stores `sent_at`. On every insert it first evicts entries whose `sent_at` is earlier than `now - SENT_AT_WINDOW_SECS`. The eviction scans the whole queue, because a future-dated entry must not block older ones behind it. The count cap is still there as a backstop.
   - Test helper `pair_ab` uses a test-only `with_started_at(NOW - 2*window)` so the existing window tests still test the window itself.
2. **Silent identity change (Important).** Two changes in `store.rs`:
   - New helper `existing_keys()`:
     - Both key files present: load them.
     - Any key missing while `roster.json` exists: bail with 「这台电脑的多电脑钥匙丢了，但组名单还在；要重置请删除 <dir> 后重新 dct login」. This covers both files missing, sign.key only, and kx.key only.
     - sign.key without kx.key and no roster: the previous error, unchanged.
     - Neither file, or kx.key alone, with no roster: `None`, so keys can be generated.
   - `Mesh::load` reads keys before the roster, so this check guards it.
3. **Write order (Minor).** New test `kx_key_is_written_before_sign_key`: it blocks `kx.key.tmp` with a directory, then asserts that `sign.key` never appears.
4. **Cross-process key creation (Minor).**
   - Generation runs inside a `KeysLock`: `.keys.lock` in the mesh dir, created with `create_new`, removed on drop.
   - If the lock is taken, it retries every 20ms for up to 10s, then errors with a Chinese message.
   - A lock older than 60s counts as a crash leftover and is removed.
   - After acquiring the lock it runs `existing_keys()` again, so a waiter reuses the pair the other process wrote.

New tests:
- `mesh::tests::a_message_accepted_before_a_restart_is_refused_after_it`
- `mesh::tests::the_seen_set_forgets_entries_older_than_the_window`
- `mesh::store::tests::missing_keys_with_a_roster_present_is_an_error_not_a_new_identity` (3 cases: both missing, sign missing, kx missing; also asserts no key file is regenerated)
- `mesh::store::tests::kx_key_is_written_before_sign_key`
- `mesh::store::tests::a_held_keys_lock_makes_the_other_creator_wait_and_reuse_the_keys`
- `mesh::store::tests::a_stale_keys_lock_left_by_a_crash_is_broken`
- `mesh::store::tests::concurrent_creators_end_up_with_one_matching_pair` (20 rounds, two threads each)

Mutation checks (each mutation applied on its own, then reverted):

| Mutation | Failing test(s) |
|---|---|
| `sent_at < started_at` check disabled | `a_message_accepted_before_a_restart_is_refused_after_it` |
| Age eviction removed | `the_seen_set_forgets_entries_older_than_the_window` |
| kx/sign write order swapped | `kx_key_is_written_before_sign_key` |
| Lock removed | `concurrent_creators_end_up_with_one_matching_pair`, `a_held_keys_lock_makes_the_other_creator_wait_and_reuse_the_keys` |
| Roster-present check disabled | `missing_keys_with_a_roster_present_is_an_error_not_a_new_identity` |

Commands and output:

```
$ ~/.cargo/bin/cargo test --workspace   -> exit=0
  dct lib: test result: ok. 1465 passed; 0 failed   (no failures in any other binary)
$ ~/.cargo/bin/cargo test --lib mesh    -> test result: ok. 59 passed; 0 failed
$ ~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile ... (clean)
$ ~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
    warning: unused variable: `path` --> src/student_projects.rs:669:13   (pre-existing, unrelated)
    Finished `dev` profile ...
$ git diff --check -- src -> clean
```

Formatting: I ran `rustfmt` on `src/mesh/mod.rs` and `src/mesh/store.rs` only, not `cargo fmt --all`. Only those two files changed.

Trade-off: a legitimate message whose sender clock runs behind by N seconds is refused during the first N seconds after this daemon starts. Step 1 has no offline delivery, so nothing is lost; the sender just sees no answer.
