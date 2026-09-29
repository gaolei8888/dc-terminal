# Task 3 report: relay — verify tokens, keep envelopes within an account, list online peers

**Status:** done

**Commit:** 16bcd63 — `feat(srv): verify relay tokens, keep envelopes within an account, list online peers`
(files: `crates/dct-link/src/lib.rs` modified — `LINK_VERSION` 3→4,
`PATH_PEERS`, `PeersResponse`; `crates/dct-page/page.html` modified — the
page's mirrored `LINK_VERSION` literal, kept in sync per the existing
`the_page_speaks_the_same_envelope_version_this_relay_does` guard test;
`crates/dct-srv/Cargo.toml` modified — added `dct-mesh`, `base64`, `p256`
dependencies; `crates/dct-srv/src/lib.rs` modified — `Relay::with_issuers`,
`Device.account`, `check()` now returns `Option<String>` account,
account isolation in `send`, `Relay::peers`, `POST /link/peers` route,
`bind_check`, `Cli::Serve.relay_keys`, `Cli::TokenKeygen`/`Cli::TokenMint`,
`parse_cli` updated; `crates/dct-srv/src/main.rs` modified — wires the new
CLI branches, only calls `bind_check` instead of `must_be_loopback`
directly; `crates/dct-srv/src/keys.rs` modified — `load_relay_keys` /
`parse_relay_keys`; `crates/dct-srv/src/issuer.rs` new — `token
keygen`/`token mint` logic: `generate`, `write_issuer_files`,
`signing_key_from_seed_b64`, `mint`.)

**Design notes / deviations:**
- `check()` changed signature from `Result<(), LinkError>` to
  `Result<Option<String>, LinkError>` (the verified account, or `None` for
  an unverified relay). `poll` writes it into `Device.account` on every
  poll (not just first contact), `send`/`peers` read it back. This wasn't
  spelled out in the brief's interfaces list but was needed to satisfy
  "每台设备记下它的 account" and the account-isolation/peers behavior
  together without a second lookup path.
- Account isolation in `send` only fires when the *recipient* is a
  registered online `Device` (i.e. the normal "not yours" case the brief's
  test describes). The `/link/ask` reply path (matched purely by `(to,
  seq)` in the waiting table, no `Device` entry for the asker) is left as
  before — it's already protected because the original `ask()` call goes
  through the normal `send()` account check against the being-asked
  device.
- Added `bind_check(addr, relay_keys: Option<&Path>)` in the lib (not
  `main`, which can't be tested) so `main.rs` calls it instead of
  `must_be_loopback` directly; `--relay-keys` present skips the loopback
  requirement entirely, matching "只有给了 --relay-keys 才允许绑非环回
  地址".
- `token keygen`/`token mint` needed a `p256::ecdsa::SigningKey` value to
  call `dct_mesh::relay_token::issue`, so `p256` (same version as
  dct-mesh) and `base64` were added to `dct-srv`'s `Cargo.toml` beyond the
  `dct-mesh` line the brief called out explicitly.
- `Cli` grew from five to seven shapes (`TokenKeygen`, `TokenMint`); renamed
  the existing parse test accordingly and extended it in place rather than
  duplicating it.
- Manually verified end-to-end outside the test suite: `token keygen`
  writes `issuer.key` (0600) and `issuer.pub`; `token mint` prints a
  token; `serve --with-link 0.0.0.0:0` without `--relay-keys` is refused,
  with `--relay-keys <issuer.pub>` it binds and logs "只收带着有效令牌的
  连接".

**Tests / commands / output:**

- `cargo test -p dct-srv -p dct-link` (after Step 1, before implementation):
  failed to compile as expected (missing `Relay::with_issuers`, `peers`,
  `PATH_PEERS`, `PeersResponse`, `bind_check`, `Cli::Serve.relay_keys`,
  `Cli::TokenKeygen`/`TokenMint`), confirming the new tests exercise
  not-yet-existing surface.
- After implementation, `cargo test -p dct-srv -p dct-link`: all green,
  including the new tests from the brief's Step 1 list
  (`with_issuers_a_missing_or_forged_token_is_unauthorized`,
  `a_token_for_another_endpoint_is_not_yours`,
  `an_expired_token_is_unauthorized`, `envelopes_do_not_cross_accounts`,
  `peers_lists_only_same_account_online_others`,
  `relay_new_without_issuers_still_accepts_anything`,
  `public_bind_needs_relay_keys`, `dct_srv_never_opens_mesh_payloads`) plus
  new coverage for `parse_relay_keys` (keys.rs) and `issuer` (keygen/mint
  round trip, expiry, file permissions).
- `cargo test --workspace`: **ok**, every crate's suite passed (dct,
  dct-brain, dct-link, dct-mesh, dct-page, dct-srv unit tests + `tests/serves.rs`
  integration tests), exit code 0. `dct-srv` lib alone: 89 passed, 0
  failed.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean, no
  warnings.
- `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc`:
  clean except one pre-existing, unrelated warning in `dct`'s
  `src/student_projects.rs:669` (`unused variable: path`) — confirmed via
  `git stash` that this warning predates this task's changes and the file
  was never touched here.
- `cargo fmt --check -p dct-srv -p dct-link`: only reported diffs in lines
  this task didn't touch (confirmed pre-existing via `git stash` +
  `cargo fmt --check` on `HEAD`); every line this task added or edited is
  rustfmt-clean.
- `git diff --check`: clean, no whitespace errors.

**Concerns:**
- ~~The account-isolation gap on the `/link/ask` reply path~~ — closed in
  Fix round 1 below; the concern as originally written (from before that
  round) undersold the risk, see the round's notes.
- `LINK_VERSION` bump to 4 touches three pinned-shape tests in
  `dct-link` and the page's mirrored literal; anyone with an old `dct`
  build talking to a new relay (or vice versa) now gets `VersionMismatch`
  as intended — flagging in case task 4/5 need a migration note for
  already-deployed daemons.

## Fix round 1

Spec review of 16bcd63 raised seven findings; the controller ruled on
each and asked for all seven to be implemented. Commit: `70446a7` —
`fix(srv): close account-isolation gaps found in review of 16bcd63`.

**1. CRITICAL — ask-reply slot hijack.** `send()` matched a reply to a
pending `/link/ask` purely by `(to, seq)`, then delivered it
unconditionally. Since `seq` is chosen by the asker and not secret, an
endpoint on a *different* account that guessed a live `seq` could
deliver its own envelope into another account's pending ask. Fixed by
recording, at `ask()` time, who is expected to answer (the asked
endpoint) and the asker's own verified account in a new `WaitSlot`;
`send()` now only fills the slot when both `envelope.from` equals that
endpoint and the replier's verified account equals the asker's account.
Anything else leaves the slot alone and falls through to the normal
`send` path (which resolves to `Offline`, since the asker is never a
registered `Device`). New test
`ask_reply_slot_only_fills_for_the_asked_endpoint_and_its_account`
exercises all three cases the ruling asked for: wrong account, wrong
`from` (same account), then the real reply still landing.

**2. IMPORTANT — endpoint takeover while present.** `poll()` used to
unconditionally overwrite `Device.account` on every call, so a second
account minting a token for the same `endpoint` id could silently
relabel — and start receiving mail for — a device that was still
online under the first account. Fixed with a takeover guard: while an
id is present (survived the poll-time `retain`), a poll whose verified
account differs from the one already on file is refused with
`NotYours` and the block returns *before* touching `last_poll`,
`account`, or the mailbox. Once the id naturally expires (the next
`retain` drops it), a different account may register it again. Two new
tests:
`polling_with_a_different_account_while_present_is_refused_and_leaves_it_untouched`
(refusal + proof that account/mailbox/peers-visibility are untouched)
and `a_dropped_endpoint_can_be_re_registered_under_a_different_account`.

**3. IMPORTANT — presence oracle via NotYours.** `send()`'s
cross-account branch returned `NotYours` for a recipient that was
online but belonged to another account — which let a sender
distinguish "doesn't exist" (`Offline`) from "exists, online, not
yours" (`NotYours`) for an id it doesn't own, leaking presence across
accounts. Changed that branch to return `Offline`, identical to the
"never showed up" case. `envelopes_do_not_cross_accounts` updated to
expect `Offline` instead of `NotYours`.

**4. Peers self-exclusion test wasn't deterministic.** The original
`peers_lists_only_same_account_online_others` used sequential,
timeout-driven polls with a small `cfg`; by the time `peers()` ran, the
caller's own entry could already have aged past `presence_ttl` and been
swept by `peers()`'s own `retain` — at which point removing the
`**ep != auth.endpoint` filter wouldn't have changed the output, so the
test could pass without ever exercising self-exclusion. Replaced with
`peers_excludes_the_caller_even_though_they_are_present_too`, which
polls all three endpoints concurrently (spawned tasks + the existing
`until` helper) against a long `cfg(2000)`, so the caller is
provably still present — `until` only returns once all three device
entries exist — when `peers()` runs.

**5. Three previously-surviving mutants, now killed by dedicated
tests** (see Kill-check below for the manual mutation proof):
   - Removing the per-poll `d.account = account.clone()` refresh: now
     load-bearing, because the entry is created with a `None`
     placeholder and this line is the only place that ever writes the
     real value. Killed by 4 different tests once mutated (see below).
   - Removing `peers()`'s own `map.retain(...)`: new test
     `peers_does_not_list_a_device_that_stopped_polling` — a device
     expires with nobody else ever calling `poll()` again (which would
     otherwise trigger the sweep first), so only `peers()`'s own
     `retain` can keep it out of the list.
   - Removing `out.sort()`: new test
     `peers_come_back_sorted_lexicographically`, which brings 6 peers
     online in scrambled, non-alphabetical order (via concurrent polls)
     and asserts the exact lexicographic vector — `HashMap` iteration
     order is unspecified, so with 6 items the chance of an unsorted
     result coincidentally matching is ~1/720.

**6. Strengthened the dct-mesh source guard.** The old
`dct_srv_never_opens_mesh_payloads` test grepped for two hardcoded
substrings (`dct_mesh::wire`, `seal::open`), which a grouped import like
`use dct_mesh::{wire, relay_token};` would sail past (neither banned
substring appears literally). Replaced with
`dct_srv_only_ever_touches_dct_mesh_relay_token`, which parses every
occurrence of `dct_mesh` in `crates/dct-srv/src/**/*.rs`, resolves the
first path segment after `::` — including expanding `{a, b, c}` grouped
imports into their individual items — and asserts every one of them is
exactly `relay_token`.

**7. keys.rs — empty relay-keys file.** `parse_relay_keys` previously
returned `Ok(vec![])` for a file containing only comments/blank lines
(there was even a test asserting that). Since `Relay::with_issuers`
with an empty issuer list rejects every token
(`relay_token::verify`'s own `an_empty_issuer_list_never_verifies`),
loading such a file used to silently produce a relay that refuses
everyone with no diagnostic pointing at the cause. Now
`parse_relay_keys` returns a clear `Err` when the parsed list is empty;
updated the old test (`an_empty_relay_keys_file_is_an_empty_list_not_an_error`
→ `an_empty_relay_keys_file_is_refused_with_a_clear_error`).

### Kill-check (findings 1–5), on a scratch copy under `/private/tmp`

Per instruction, this was done on a copy of the repo — never on the
working tree — and the copy was deleted afterward. Method: `rsync`'d
the repo (excluding `target/`, `.git/`, `devices/`) to
`/private/tmp/.../scratchpad/killcheck`, then ran `cargo test` there
with `CARGO_TARGET_DIR` pointed back at the real repo's `target/` so
builds reused the existing cache. For each finding, reverted exactly
the fix in the scratch copy's `crates/dct-srv/src/lib.rs` (via a
scripted find/replace against a saved pristine copy of that file, so
each mutation started from a clean base), ran the specific new test(s),
confirmed `FAILED`, then restored the pristine file before the next
mutation. After all five, restored the file once more, diffed it
against the pristine copy to confirm it was byte-identical, and deleted
the whole scratch directory.

| # | Mutation | Test run | Result |
|---|---|---|---|
| 1 | `send()`'s waiting-slot match reverted to unconditional `(to, seq)` remove (old behavior) | `ask_reply_slot_only_fills_for_the_asked_endpoint_and_its_account` | **FAILED** — `left: Ok(()), right: Err(Offline)` on the "wrong account" case |
| 2 | Removed the endpoint-takeover guard block in `poll()` | `polling_with_a_different_account_while_present_is_refused_and_leaves_it_untouched` | **FAILED** — `left: Ok(None), right: Err(NotYours)` |
| 3 | Changed `send()`'s cross-account branch back to `Err(LinkError::NotYours)` | `envelopes_do_not_cross_accounts` | **FAILED** — `left: Err(NotYours), right: Err(Offline)` |
| 4 | Removed `**ep != auth.endpoint` from `peers()`'s filter | `peers_excludes_the_caller_even_though_they_are_present_too` | **FAILED** — `left: [a, b], right: [b]` |
| 5a | Removed `d.account = account.clone();` in `poll()` | full `dct-srv --lib` suite | **FAILED** — 4 tests broke: `ask_reply_slot_only_fills_for_the_asked_endpoint_and_its_account`, `peers_come_back_sorted_lexicographically`, `peers_excludes_the_caller_even_though_they_are_present_too`, `polling_with_a_different_account_while_present_is_refused_and_leaves_it_untouched` |
| 5b | Removed `map.retain(...)` inside `peers()` | `peers_does_not_list_a_device_that_stopped_polling` | **FAILED** — `left: [b], right: []` |
| 5c | Removed `out.sort();` in `peers()` | `peers_come_back_sorted_lexicographically`, run 5× (fresh process each time, so a fresh `HashMap` hasher seed each run) | **FAILED** every time |

All seven findings' fixes are covered by tests that provably fail
without the fix.

### Tests / commands / output (Fix round 1)

- `cargo test -p dct-srv -p dct-link` (scratch, before restoring each
  mutation): see kill-check table above — each mutation produced the
  expected `FAILED` with the expected left/right mismatch.
- `cargo test -p dct-srv -p dct-link` (real repo, after implementation):
  all green — dct-srv lib 94 passed (up from 89), dct-link 13 passed,
  `tests/serves.rs` 3 passed + 1 ignored. Re-ran 3× with
  `--test-threads=8` back to back: 94/94 passed every time, ~2.2s each
  (no flakiness observed in the new timing-sensitive tests).
- `cargo test --workspace`: **ok**, 1397 + smaller per-crate suites all
  passed, exit code 0 (`dct` lib alone: 1397 passed).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc`:
  clean except the same pre-existing, unrelated warning in
  `src/student_projects.rs:669` (confirmed untouched by this change).
- `cargo fmt --check -p dct-srv -p dct-link`: the one new-code diff it
  found (a multi-line `vec![...]` in `peers_come_back_sorted_lexicographically`)
  was reformatted to match; all remaining reported diffs are in lines
  this round didn't touch (confirmed pre-existing).
- `git diff --check -- crates/dct-srv crates/dct-link`: clean.

### Concerns (Fix round 1)

- `ask()` now calls `self.check(&req.auth)` once to capture the asker's
  account for the `WaitSlot`, and `send()` (called internally right
  after) calls `check()` again on the same `AuthFrame`. Harmless
  (idempotent, no side effects) but it is a redundant signature
  verification on every `ask()`; worth collapsing if `check()` ever
  becomes non-trivial in cost.
- The lexicographic-order test's collision probability (~1/720 with 6
  peers) is not zero. If it's ever seen to flake, the fix is more
  distinct endpoint names, not a different assertion style.

## Fix round 2

Re-review of 70446a7 confirmed all 7 round-1 findings fixed, and raised
one new Important issue plus a guard gap. Commit: `192344e` —
`fix(srv): stop ask() from letting a forged from hijack another pending ask`.

**1. Important — cross-account ask cancellation.** `ask()` built its
`WaitSlot` and inserted it into the waiting table keyed only by
`(envelope.to, seq)` *before* anything checked that `envelope.from`
matched the caller's authenticated endpoint — that check only existed
inside the `send()` call that came after the insert. Consequences,
exactly as the finding described:
  - An attacker (any account, any endpoint of their own) could call
    `ask()` with a forged `envelope.from` equal to a victim's endpoint
    and a guessed live `seq`. The `insert()` would silently overwrite
    the victim's `WaitSlot`, dropping its `oneshot::Sender` — the
    victim's `rx.await` in its own `ask()` call would immediately see
    the sender dropped and return `NoAnswer`, with no warning.
  - The attacker's own `send(req)` call right after would then fail on
    the `envelope.from != req.auth.endpoint` check in `send()` (too
    late — the damage was done), and `ask()`'s failure path would call
    `forget(&key)`, which blindly removed whatever was at that key —
    at that point, the attacker's own bogus slot, but in a narrower
    timing window it could just as easily have removed a third party's
    freshly-inserted legitimate slot for the same key.
  - When the real reply eventually arrived, the slot was already gone,
    so `send()` fell through to the normal device-lookup path and
    returned `Offline` for what should have been a successful answer.

  Fixed in the three parts the ruling asked for:
  - **(a)** `ask()` now checks `req.envelope.from == req.auth.endpoint`
    (identical to the check `send()` already does) *before* creating
    the oneshot channel or touching the waiting table at all.
  - **(b)** Inserting into an already-occupied `(endpoint, seq)` key no
    longer overwrites it; `ask()` returns `Err(LinkError::Busy)`
    instead, and never mutates the slot that's already there. (Kept the
    two-element key rather than adding the account to it, per the
    ruling's "either/or" — simpler change, and the second test below
    confirms it's still safe when two different accounts collide on the
    same key.)
  - **(c)** `WaitSlot` gained a `next_wait_id`-derived `id: u64`, set
    once at insertion. `forget(&self, key, id)` now only removes the
    table entry if the slot currently at that key still has the id it
    was given — a late cleanup call from an `ask()` whose slot was
    already consumed and whose key was since reclaimed by a different,
    newer `ask()` is now a no-op instead of deleting someone else's
    live slot.

  Three new tests:
  - `a_forged_from_cannot_hijack_someone_elses_pending_ask` — reproduces
    the attack from the finding: victim's ask is pending, attacker
    calls `ask()` with a forged `from`/guessed `seq`, gets
    `Unauthorized` immediately, the victim's waiting-table entry is
    untouched, and the victim's `ask()` still receives the real reply
    (not `NoAnswer`).
  - `two_askers_colliding_on_the_same_endpoint_and_seq_do_not_corrupt_each_other`
    — two different (non-malicious) accounts whose tokens both
    legitimately claim the same endpoint string happen to pick the same
    `seq`; the second `ask()` gets `Busy`, the first is unaffected and
    still gets its real reply.
  - `forget_never_removes_a_slot_it_did_not_create` — a focused,
    white-box test for part (c) specifically: manually stages the exact
    race (old slot consumed and removed, key reclaimed by a new slot
    with a different id, then a `forget()` call carrying the *old* id)
    and asserts the new slot survives and still delivers its reply. This
    race isn't reachable deterministically through the public API, so
    the test pokes the private `waiting` map and calls the private
    `forget` directly (same pattern the file already uses elsewhere,
    e.g. `relay.devices.lock()` / `relay.waiting.lock()` in existing
    tests).

**2. Source guard gap.** The round-1 guard
(`dct_srv_only_ever_touches_dct_mesh_relay_token`) resolved the first
path segment after `dct_mesh::`, which correctly caught grouped imports
but had a blind spot the re-review found: `use dct_mesh::*;` and
`use dct_mesh as m;` don't have a "first segment after `::`" in the
shape the old parser expected, so neither was ever routed into the
segment check — both sailed through undetected. Replaced with the
simpler, stronger approach the ruling suggested: scan for every
*standalone* occurrence of the word `dct_mesh` (word-boundary checked,
so it doesn't fire on it being a substring of some other identifier),
and require the text at that position to literally be the exact prefix
`dct_mesh::relay_token` (followed by a non-identifier character, so
`relay_token` can't be a truncated match either). Every other
standalone occurrence — wildcard, rename, grouped import, or even a
bare `use dct_mesh;` with nothing after it — is now flagged. The
crate-name literal is split via `concat!` inside the test (same trick
as `the_relay_never_looks_inside_a_frame_either`) so the test doesn't
trip over its own definition of the marker string.

### Kill-check (both findings), on a scratch copy under `/private/tmp`

Same method as round 1: `rsync`'d the repo (excluding `target/`,
`.git/`, `devices/`) to a fresh scratch directory, ran `cargo test`
there with `CARGO_TARGET_DIR` pointed back at the real repo's `target/`
to reuse the build cache, mutated one thing at a time against a saved
pristine copy, ran the targeted test(s), then restored before the next
mutation. Deleted the scratch directory afterward.

| # | Mutation | Test run | Result |
|---|---|---|---|
| 1a | Removed the `req.envelope.from != req.auth.endpoint` check from the top of `ask()` | `a_forged_from_cannot_hijack_someone_elses_pending_ask` | **FAILED** — `left: Err(Busy), right: Err(Unauthorized)` (guard (b) alone still stops the overwrite, but not with the right error, and not before doing more work — confirms (a) is independently necessary, not redundant with (b)) |
| 1b | Removed the `waiting.contains_key(&key)` / `Busy` guard, restored unconditional `insert` | `two_askers_colliding_on_the_same_endpoint_and_seq_do_not_corrupt_each_other` | **FAILED** — `left: Err(Offline), right: Err(Busy)` (second asker's insert silently overwrote the first, whose eventual reply then fell through to `Offline`) |
| 1c | Reverted `forget()` to unconditionally remove by key, ignoring `id` | `forget_never_removes_a_slot_it_did_not_create` (and confirmed: survived the *entire* 96-test suite at that point — this specific test is the only one that catches it, which is exactly why it needed to exist) | **FAILED** — panics with "新挂号被一个迟到的、属于别人的 forget 误删了" |
| 2a | Injected `use dct_mesh::*;` (in an isolated inner module in `keys.rs`, to avoid namespace pollution) | `dct_srv_only_ever_touches_dct_mesh_relay_token` | **FAILED** — flagged the injected file/offset |
| 2b | Injected `use dct_mesh as m;` | same | **FAILED** — flagged the injected file/offset |
| 2c | Injected `use dct_mesh::{relay_token, wire};` (grouped import, re-confirming no regression from round 1) | same | **FAILED** — flagged the injected file/offset |

One operational note: after the round-2 scratch-copy runs (which shared
`CARGO_TARGET_DIR` with the real repo purely to reuse cached build
artifacts for speed), a subsequent `cargo test -p dct-srv --lib` in the
*real* repo intermittently hit a stale-cache artifact — the guard test
panicked with `NotFound` reading a path that turned out to belong to
the already-deleted scratch directory, meaning cargo had reused a test
binary built against the scratch tree's `CARGO_MANIFEST_DIR` value
baked in via `env!`. `cargo clean -p dct-srv` followed by a rebuild
resolved it and every subsequent run (including the full
`cargo test --workspace` used for final sign-off below) was clean. Not
a code bug — a caveat of sharing a target directory between two
different absolute-path checkouts of the same package name for build
speed. Noting it here in case round 3 (or anyone re-running this
kill-check recipe) sees the same thing and wonders whether it's real.

### Tests / commands / output (Fix round 2)

- Kill-check (scratch copy): see table above — every one of the six
  mutations produced the expected `FAILED`.
- `cargo test -p dct-srv --lib` (real repo, after `cargo clean -p
  dct-srv` per the note above): **97 passed, 0 failed** (up from 94 at
  the start of this round — 3 new tests).
- `cargo test --workspace`: **ok**, exit code 0, every crate's suite
  passed (`dct` lib 1397 passed; `dct-srv` lib 97 passed; the rest
  unchanged from round 1).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc`:
  clean except the same pre-existing, unrelated warning in
  `src/student_projects.rs:669`.
- `cargo fmt --check -p dct-srv`: the one new-code diff it found (an
  `until(...)` closure call in
  `two_askers_colliding_on_the_same_endpoint_and_seq_do_not_corrupt_each_other`)
  was reformatted to match; every other reported diff is in lines this
  round didn't touch (confirmed against the pre-round-2 baseline).
- `git diff --check -- crates/dct-srv crates/dct-link`: clean.

### Concerns (Fix round 2)

- `two_askers_colliding_on_the_same_endpoint_and_seq_do_not_corrupt_each_other`
  documents current behavior (second legitimate asker gets `Busy` on a
  same-key collision) rather than resolving it — two different accounts
  that happen to pick the same self-chosen endpoint string can still
  transiently block each other's `ask()` calls. That's a availability
  hiccup, not a correctness/isolation bug (no data crosses accounts),
  and the ruling explicitly accepted this as one of two valid fixes, but
  flagging in case task 4/5's endpoint-naming scheme wants to rule it
  out entirely (e.g. by making phone-side ids less likely to collide, or
  by moving to an account-qualified key later).
- `forget_never_removes_a_slot_it_did_not_create` is a white-box test
  (it manipulates the private `waiting` map and calls the private
  `forget` method directly) because the race it covers isn't reachable
  deterministically through `poll`/`send`/`ask` alone. Flagging per the
  project's usual preference for black-box tests — this one is an
  intentional, documented exception, consistent with how this file
  already peeks at `relay.devices`/`relay.waiting` for synchronization
  in a few other tests, but it's the first time a private method itself
  is called directly.
