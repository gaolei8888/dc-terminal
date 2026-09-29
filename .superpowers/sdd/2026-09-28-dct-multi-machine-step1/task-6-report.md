# Task 6 report — login, join (6-digit compare), approve, remove

Status: DONE_WITH_CONCERNS
Commit: 6d783ea (on feat/dct-multi-machine-step1)

Branch note: when I started, `feat/dct-multi-machine-step1` was at 453a1ea and `main` was at
9d3563f. Task 5's commits had landed on `main`. I fast-forwarded the feature branch to 9d3563f
(checked `merge-base --is-ancestor` first), checked it out, and committed there. `main` is
unchanged.

## What was built

- **dct-mesh `roster::accept_invite(incoming, me_endpoint, inviter)`** (ruling 2). Checks:
  - the same structural checks as `accept`, plus no duplicates;
  - `inviter.endpoint` is bound to `inviter.sign_pub`;
  - `signer == inviter.endpoint` (else `UnknownSigner`);
  - the signature verifies under the inviter's key;
  - the inviter is listed with exactly the keys that were compared (else `SignerRemoved` / `BadKey`);
  - the roster lists me (else the new variant `RosterError::NotForMe`).
- **`Mesh` (src/mesh/mod.rs)**:
  - New `invites: Vec<(Member, code)>`.
  - `Join` is verified: the self-signature, `member.endpoint == env.from`, the name is valid, `kx_pub` decodes, and the sender is not already a member. It is then stored in `pending_joins` (deduplicated by endpoint, capped at 16, expires after `JOIN_TTL` of 10 minutes) and answered with `JoinPending{member: me, sig}`. A `JoinPending` that arrives by poll is dropped.
  - A roster whose signer is in `invites` goes through `accept_invite` while the machine `is_alone()`. The joiner also checks that its own entry carries its own sign and kx keys.
  - `ensure_group` (genesis `mine-<endpoint>`), `rename`, `join_request`, `commit` (save first, then swap; clears pending for anyone now in the roster), `prune_pending`.
- **src/mesh/group.rs**:
  - `view`, `join` (asks all peers in parallel via `thread::scope`; a reply counts only if it is a verified `JoinPending` whose endpoint is the one asked), `approve` (by name or endpoint; `Ambiguous` / `NameTaken` / `NoSuchRequest`; `--no` only drops the pending entry), `remove` (refuses self).
  - Every `Net` call is made after the `Mesh` lock is released.
- **proto v20**:
  - Requests `MeshStatus`, `MeshLogin`, `MeshJoin{name}`, `MeshApprove{endpoint,yes}`, `MeshRemove{name}`.
  - `Response::Mesh(MeshView)`, plus `MemberView` and `PendingJoin`.
  - `ErrorCode::Mesh(MeshProblem)`.
  - Guard tests and every pinned `(19, …)` tuple are bumped.
- **daemon**:
  - `MeshCtl { socket, journal_path, slot: Mutex<Option<MeshRuntime>> }`. `MeshRuntime` gains `token`.
  - `serve` routes `Mesh*` to `handle_mesh`. From the HTTP path, `handle` refuses them with `BadRequest`.
  - `MeshLogin` does the following. It reads the `"dc"` api_key; if missing, it returns `NoDcAccount`. It gets the origin from `pair_origin`. It calls `fetch_token` for this machine's endpoint, with an injectable transport, and stores `__relay__` and `__relay_exp__`. If the link is running it swaps the token in the shared cell; otherwise it calls `start_mesh` into the slot. Finally it runs `ensure_group`.
  - `MeshStatus` without a runtime reads the disk only and never creates keys.
- **CLI** (src/mesh/cli.rs; main routes `login` / `join` / `peers`; HELP updated):
  - `dct login` prints 「这台电脑成了「我的电脑」组的第一台」 when a group was just created.
  - `dct join [--name X]` prints the code in the sentence for a single machine, or lists 「电脑名：数字」 per machine. It then polls status until the roster has another member, for up to 10 minutes.
  - `dct peers` lists members and pending joins, and prints the brief's y/n prompt with approve and refuse hints.
  - `dct peers approve <name|c-…> [--no]` and `dct peers remove <name>`.
  - `Client::call_within` was added because login and join exceed the 5s `READ_TIMEOUT`.
- **i18n**: all strings are `msg::mesh_*` functions plus `msg::error` arms. The brief's Chinese strings are pinned verbatim by a test.

## Deviations

1. **Login always creates a solo group if none exists** (as the brief says). As a result, "join requires not in a group" is implemented as "join requires `is_alone()`": no roster, or a roster containing only me. Otherwise a second machine could never join after `dct login`. On join, the solo roster is replaced by the invite roster.
2. **`MeshView` has an extra field, `joining: Vec<PendingJoin>`.** These are the joiner's own inviters and their codes. The CLI needs the codes back from `MeshJoin`.
3. **`Some("send")` is not routed in main.** That command belongs to Task 7; a stub would only print nothing.
4. **Removal broadcasts the new roster to the remaining members only.** The removed machine keeps its stale roster. Its sealed messages are dropped everywhere else, which is tested.
5. **The flows live in a new `src/mesh/group.rs`**, not in `mod.rs`.
6. **Errors are codes (`MeshProblem`), not sentences**, following the daemon rule. `LoginFailed(String)` carries `fetch_token`'s Chinese reason through, like `Git(raw)`.

## Tests / commands / output

Multi-machine tests use `FakeHub` / `FakeNet` (src/mesh/group.rs, 17 tests):

- **`three_computers_join_one_after_another`**: A login → B login and join → A approve → B joined → C join → B approves → A receives v3.
  - Codes match on both sides.
  - The roster is unchanged before approval.
  - A's pending entry for C clears once C is in.
- **`a_join_with_swapped_keys_shows_a_different_code_and_never_enters_unapproved`**: an impostor named "B" shows a different code. Approving by the ambiguous name is refused. The impostor's self-signed roster is dropped. `--no` removes only that request.
- **Join edge cases**:
  - `a_join_that_is_not_self_signed_by_the_sender_is_dropped_silently` covers a bad signature from B's own endpoint and B's real signature sent from another endpoint.
  - `a_join_reply_must_come_from_the_machine_that_was_asked` covers a swapped reply and a bad reply signature.
- **Roster acceptance**:
  - `a_joining_machine_only_takes_a_roster_from_a_machine_it_compared_codes_with` also covers a swapped kx for me.
  - `a_grouped_machine_never_takes_an_invite_roster`.
- **Removal**: `a_removed_machine_is_dropped_and_self_removal_is_refused` checks that a sealed message from removed C is dropped, with a positive control against the pre-removal roster.
- **Other cases**: name taken, no answer, already grouped, rename via `--name`, pending cap, duplicate join, and a save failure that changes nothing and sends nothing.
- **dct-mesh**: 8 `accept_invite` tests.
- **daemon**:
  - Mesh requests are refused off the local socket.
  - Status before login creates no keys; join, approve and remove return `NotLoggedIn`.
  - No DC key returns `NoDcAccount`.
  - Login stores the token, starts the link, creates the group, and a second login swaps the token in the running cell.
  - A gateway 404 reason is passed through.
- **cli**: 9 scripted-dialogue tests. **proto**: v20 guard. **i18n**: brief strings verbatim, and no Han characters in the English strings.

Hand mutation pass (script in the scratchpad; each mutant applied, tests run, file restored and verified with `cmp`). 17 mutants, all KILLED after adding 2 tests for the first-round survivors (join reply signature check; `is_alone` guard on the invite path):

- **group.rs**: reply endpoint check, reply signature check, ambiguous-name refusal, name-taken refusal, join-when-grouped refusal.
- **mod.rs**: `take_join` signature check, `take_join` from check, invite kx check, already-member drop, pending cap, invite path, `is_alone` guard.
- **roster.rs (`accept_invite`)**: signer check, inviter keys check, inviter binding, me-listed check, signature verify.

```
~/.cargo/bin/cargo test --workspace                     -> passed 1812 failed 0 ignored 3
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings  -> Finished, no warnings
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
                                                        -> Finished; one warning, already there
                                                           (unused `path` in src/student_projects.rs:669, not touched)
```
`cargo fmt --all` was not run. `rustfmt` was applied only to the two new files.

## Concerns

- **Concurrent approvals.** If two members approve different joiners from the same version at the same moment, each produces a vN+1 and the rosters fork. Nothing reconciles them in step 1.
- **Offline members miss roster updates.** There is no offline delivery and no roster pull. A member that was offline during an approval or removal keeps a stale roster. It will drop messages from the new member until another roster reaches it.
- **Join-to-join between solo machines.** When two machines both run `dct login` and then `dct join`, their Join requests go to each other too. Each shows a code, and the user must approve on the machine intended as the group owner. This is by design, but it deserves a line in the TUI (Task 8).
- **The pending list is in memory only.** A daemon restart drops pending joins; the joiner simply retries.

## Fix round 1 (commit 41a120b, worktree /Users/lei/work/dc/dc-terminal-mesh)

### C1: the joiner only accepts a roster from the inviter the user confirmed

- `Mesh.invites` is now `Vec<Invite{member, code, at}>`. `at` comes from the Mesh clock, and `prune_invites` drops any invite older than `JOIN_TTL`.
- New fields: `confirmed: Option<endpoint>` and `held: Vec<SignedRoster>`. The held list keeps at most one roster per responder.
- `take_roster` on a machine that is still alone:
  - If the signer is the confirmed inviter, the roster goes through `take_invite` → `accept_invite`, together with the check that my own entry has my keys.
  - If the signer only responded to the join, the roster is held and never taken directly.
  - Otherwise, the normal path applies.
- `confirm_inviter(endpoint)` fails with `NoSuchInviter` unless that endpoint is a live responder. If it succeeds, it verifies any roster held from that inviter right away. This covers the case "the other computer clicked approve before this side confirmed".
- `group::confirm` and a new `Request::MeshConfirmInviter { endpoint }`, handled on the local socket only.
- **CLI:** `dct join` lists 「电脑名：数字」 for every responder, including when there is only one. It then asks which computer shows the same number, and the user types a name or `c-…`. `dct join --confirm <name>` skips the question. Pressing Enter, EOF, or naming a machine that never responded confirms nothing and exits 1.

### I1: approval is tied to the endpoint and code the user saw

- `MeshApprove { endpoint, code, yes }`. The daemon looks up the pending request by exact endpoint, and approving requires the code to match (else `CodeMismatch`). Refusing needs only the endpoint.
- `dct peers approve <name|c-…>` reads the current status and resolves the argument to a single request. More than one match gives `Ambiguous` locally. It shows the brief's y/n prompt with that name and code, and sends the endpoint and code it just showed. `--no` refuses without asking.
- When `pending_joins` is full, a new endpoint is refused with no reply. An endpoint that is already pending can still refresh its request.

### M2: the lock comment in `mesh_login`

A comment in `mesh_login` explains why `start_mesh` under the slot lock is safe:
- it makes no network calls;
- the only other locks it takes are secrets (never held together with the slot elsewhere) and its own brand-new `Mesh`;
- the spawned link thread never touches the slot;
- holding the lock prevents two concurrent logins from each starting a link.

### Protocol

`PROTOCOL_VERSION` 21. The shape guard is updated, and all pinned tuples are bumped. There are new `MeshProblem::NoSuchInviter(String)` and `MeshProblem::CodeMismatch`, and new i18n strings: `mesh_which_computer`, `mesh_not_a_responder`, `mesh_join_cancelled`, `mesh_not_approved`.

### New and updated tests

**group.rs**
- `a_second_responder_that_was_never_compared_cannot_pull_the_joiner_in` (reviewer's C1 PoC): M's roster is refused both before and after the user confirms A. B then joins A's group.
- `a_roster_from_the_inviter_that_arrives_before_confirmation_is_taken_on_confirm`.
- `invites_expire_after_the_join_ttl` covers three cases: confirming after the TTL is refused, a confirmation that has since expired rejects the roster, and a control case one second before the TTL still joins.
- `confirming_a_machine_that_never_answered_is_refused`.
- `flooding_joins_cannot_evict_a_pending_request_or_slip_in_an_impostor` (reviewer's I1 PoC): B's request survives a flood of cap+5, the impostor named "B" gets no reply, a re-join while the list is full still works, and approving B's shown endpoint and code succeeds.
- `approving_with_a_code_other_than_the_pending_one_is_refused`.
- The swapped-keys test now also sends the impostor's endpoint with B's code and gets `CodeMismatch`.
- Existing flows now confirm the inviter explicitly.

**cli.rs**
- Join asks the user and confirms the chosen computer, not the first in the list.
- Join still asks when there is only one responder, and joins immediately if the roster is already held.
- `--confirm` skips the question.
- Enter, EOF, or an unknown name confirms nothing.
- `peers approve` shows the code again, sends the endpoint and code, answering n sends nothing, and `--no` refuses.
- Two pending requests with the same name make approve ask for the endpoint.

**daemon, i18n**
- The daemon refusal and not-logged-in lists include `MeshConfirmInviter`.
- The i18n lists include the new codes and strings.

### Mutation pass on the new logic

10 mutants, all KILLED:
- only the confirmed inviter's roster is accepted;
- invite TTL;
- confirm requires a responder;
- a full list refuses instead of evicting;
- re-join allowed when full;
- held roster taken on confirm;
- code must match;
- the CLI approve asks y/n;
- the CLI confirms the chosen computer.

This took two fixes. The first run had two survivors:
- **The CLI test** picked the first responder, so it could not tell "the chosen one" from "the first one". The test now picks the second.
- **`held` pruned together with invites** was redundant code. `set_invites` clears the held list, and a held roster is only used when its invite is still live. I removed the line and documented why.

### Commands

```
cargo test --workspace   -> passed 1822 failed 0 ignored 3
cargo clippy --workspace --all-targets -- -D warnings   -> Finished, clean
cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
                         -> Finished; one existing warning (src/student_projects.rs:669 unused `path`)
```
All three were run in /Users/lei/work/dc/dc-terminal-mesh. `cargo fmt --all` was not run; rustfmt was applied to `src/mesh/cli.rs` and `src/mesh/group.rs` only.

Not changed, per the rulings: concurrent approvals, stale offline members, cross-joins, and pending joins lost on restart.
