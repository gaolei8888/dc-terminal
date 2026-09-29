# Task 7 report: dct peers details, dct send, delivery into a session, busy queue

Status: DONE_WITH_CONCERNS

Worktree: /Users/lei/work/dc/dc-terminal-mesh, branch `feat/dct-multi-machine-step1`

Commits:
- `b0b2195` fix(mesh): hold an invite roster only from its signer, show endpoints for same-name responders, bound join wait by the invite
- `8a30be7` feat(mesh): dct peers and dct send, delivered into an idle session with a marker

Neither commit has a Co-Authored-By line or any AI attribution. `.superpowers/sdd/.gitignore` was already modified in the worktree before I started. I did not touch it and did not stage it.

## Part 1: carried fixes from Task 6 (commit b0b2195)

### m1: a held invite roster could be replaced by junk

- **Fix** (`src/mesh/mod.rs`, `take_roster`): while the machine is alone, a roster from a responder is held only if `env.from == incoming.signer`. The relay authenticates `env.from`. Anything else is dropped with the journal reason `invite_not_from_signer`.
- **Why this option:** it is the simpler of the two. The confirmed path is unchanged and still goes through `accept_invite`.
- **Test:** `group::tests::a_junk_roster_claiming_the_inviter_as_signer_does_not_clobber_the_held_one`.
  1. B joins, and both A and M respond.
  2. A approves, so B holds A's genuine roster.
  3. M sends a roster it signed itself, with `signer` set to A.
  4. B confirms A and ends up in A's group, with a roster equal to A's.

### m2: two responders with the same name

- **CLI** (`src/mesh/cli.rs`, `join`): when two or more responders share a name, each of their lines shows the endpoint, e.g. `  A (c-a1)：111111`. Unique names still print as `  C：333333`.
- **Ambiguous message:** typing just the shared name now prints a new message, `msg::mesh_ambiguous_responder`: 「不止一台叫 A 的电脑回应了。重新运行 dct join，输入数字对得上的那台后面括号里的 c-… 编号」. The old `MeshProblem::Ambiguous` text talked about `dct peers`, which does not apply to join.
- **Test:** `cli::tests::join_with_two_responders_of_the_same_name_shows_their_endpoints`. It checks the endpoint lines, that typing `c-a2` confirms that exact endpoint, and that typing only `A` sends no confirmation and prints the exact message.

### m3: the join wait could outlive the invite

- **Deadline:** the CLI deadline now starts before `MeshJoin`, the same moment the daemon stamps the invites in `set_invites`. It no longer starts after confirmation.
- **Early exit:** while polling, if the daemon's view is not grouped and `joining` is empty, the invite has been pruned, so the CLI stops immediately.
- **Message:** both paths print 「10 分钟里没等到同意，邀请已过期，请重新运行 dct join」.
- **Computer name:** `Mesh::confirm_inviter` looks up the name before pruning, so `NoSuchInviter` carries the computer name rather than the endpoint. It falls back to the endpoint when that computer never answered.
- **Tests:**
  - `cli::tests::the_join_wait_starts_when_the_invites_were_made_not_after_confirming`: a slow user plus a short wait. Exactly one poll is allowed.
  - `cli::tests::join_says_expired_as_soon_as_the_daemon_drops_the_invite`.
  - `join_gives_up_after_the_wait` now pins the exact message.
  - `group::tests::invites_expire_after_the_join_ttl` now expects `NoSuchInviter("A")`.
- **Fixture change:** the older join tests' fixtures now keep `joining` non-empty while waiting, because an empty `joining` now means "expired".

## Part 2: Task 7 (commit 8a30be7)

### Files

**`src/mesh/deliver.rs` (new)**

Pure functions:
- `marker(from_machine, from_session, id, body)` gives exactly `[来自 <电脑名>/<会话名> 的留言 #<id前4位>]\n<body>`.
- `ready(state)` is true only for `Idle`.
- `clean_body` cleans each line with `session::sanitize` and keeps the line breaks, then refuses anything over 8000 chars as `TooLong`.
- `clean_label` does the same for session names, capped at 64 chars.
- `state_label` maps states to 忙 / 闲 / 等你回答 / 已停止 / 出错了 / 不清楚.
- `project_name` gives the last part of a directory, using the same rule as the board's group header.
- `resolve(sessions, want)` matches `#id`, or the board label (`ui::widgets::session_label`), or the project name. It returns `Err(vec![])` when nothing matches. When several sessions match, it returns the candidates formatted as `#id label（project）`.
- `split_address` splits at the first `/`.

Types:
- The wire types `Receipt` and `StatusBody` travel inside the sealed plaintext, so the relay never sees them.
- The `Inbox` trait has `sessions`, `type_into` and `tentacles`.
- `LocalInbox` wraps `SessionManager`. Its `type_into` is exactly `bridge::SessionWriter::type_into`, so no new way of typing keys was added.
- `read_tentacles` reads `~/.dco/endpoint.json` `capabilities`. A string entry is used as is; for an object it takes `name`, or `kind` if there is no `name`. A missing or malformed file gives an empty list with no error.

`impl Mesh`:
- `receive(&Message) -> Receipt` works as follows.
  - It refuses when there is no inbox or the body is too long.
  - If no session matches, or more than one does, it returns `NoSuchSession(candidates)`.
  - A `Stopped` or `Failed` session gives `SessionStopped`.
  - A non-agent session gives `Refused`.
  - An idle session with an empty queue gets the text typed in immediately (`Delivered`).
  - Otherwise the message is queued. The queue holds 50 per session; beyond that the answer is `Refused`, and old entries are never pushed out.
  - Every result is journaled.
- `deliver_queued()` runs once per tick. For each queued session that is ready, it sends exactly one message, the oldest first. When a session is gone or `Stopped`, its whole queue is dropped with the journal line `queue_dropped session=N count=K`.
- There are also `local_status`, `new_message` (a fresh random id on every call), `seal_for` and `open_answer`. `open_answer` requires a sealed answer of the expected kind, with the expected id, from the expected sender.

Free functions:
- **`peers(mesh, net)`**:
  - Builds one sealed `StatusRequest` per online member while holding the lock, then releases the lock.
  - Asks them in parallel with a 3-second limit, then opens the answers while holding the lock again.
  - This computer is included, built locally. A member that does not answer counts as offline.
- **`send(mesh, net, to, text, from_session)`**:
  1. Cleans the text and checks the length.
  2. Parses the address and looks up the machine by name or endpoint.
  3. For this computer itself, delivers locally through `receive` without going through the relay.
  4. Otherwise seals under the lock, releases it, and calls `ask` with a 10-second limit, then checks the receipt.
  5. On a relay `Busy`, retries once with a new id.
  6. Maps `Offline` to Offline, `NoAnswer` and `Unreachable` to `NoAnswer`, and any other error to `Refused`.
- `tick(mesh)` runs one delivery pass.
- A `testing::FakeInbox` is included for tests.

**`src/mesh/mod.rs`**
- New `inbox` field and `with_inbox`.
- `QueuedMsg` gained `text` (the precomputed marker).
- `Kind::Msg` now goes to `receive`, and the receipt body is the JSON of the `Receipt`. This happens after the existing roster, signature, time-window, before-start and `(from, id)` dedup checks.
- `StatusRequest` now answers with the JSON of `local_status()`.
- Removed `UNRESOLVED_SESSION` and `session_key`, which were placeholders from Task 5, and their test.
- The `pair_ab` fixture gives B a busy agent session #7, so the existing queue and replay tests still check what they did before.

**`src/proto.rs`**
- `PROTOCOL_VERSION` is now 22.
- New requests `MeshPeers` and `MeshSend { to, text, from_session }`. The hand-written `Debug` prints only `text_chars`, not the message text.
- New responses `MeshPeers(Vec<PeerView>)` and `MeshSent(SendOutcome)`.
- New types `PeerView { name, online, os, sessions, tentacles }` and `SessionBrief { name, state, dir }`.
- New `SendOutcome { Delivered, Queued, Offline, NoSuchMachine, NoSuchSession(Vec<String>), SessionStopped, Refused, NoAnswer }`.
- `MeshProblem` gained `TooLong` and `BadAddress`.
- The pinned request shape and every pinned `21` tuple were updated to 22. There are new pin tests for the `MeshSent` / `MeshPeers` shapes and for the `Debug` redaction.

**`src/session.rs`**
- `sanitize` is now `pub(crate)`.
- New constant `SESSION_ID_ENV = "DCT_SESSION_ID"`. It is inserted last into every session's environment.
- New test `a_session_child_sees_its_own_session_id`, following `spawn_passes_env_to_the_child`. It creates two real sessions and checks that each prints its own id.

**`src/daemon.rs`**
- `MeshCtl.inbox` holds a `LocalInbox` built from `mgr`.
- `start_mesh(..., inbox)` passes it into the `Mesh`, including on the login path.
- A delivery thread calls `deliver::tick` every second on whatever mesh is in the slot.
- `handle_mesh` gained `MeshPeers` and `MeshSend` arms. `serve` routes both to the local socket only; `handle`, the HTTP path, refuses them.
- The refusal test and the not-logged-in test now include both requests.

**`src/mesh/cli.rs`**
- `dct send <addr> <text...>`: the words are joined with spaces. `from_session` comes from `DCT_SESSION_ID`. The exit code is 0 only for Delivered or Queued; bad usage exits 2.
- `dct peers` now calls `MeshStatus` and then `MeshPeers`. For each computer it prints one line with its OS; for online computers it then lists the sessions (name, state, dir) and the tentacles, or 「没有开着的会话」. If `MeshPeers` fails, it falls back to the old member lines.
- `MeshPeers` and `MeshSend` use the slow 40-second client timeout.

**`src/i18n.rs`**
- All strings from the brief, verbatim:
  - 「已送到 X」
  - 「对方正忙，已排队，忙完就送进去」
  - 「X 现在不在线，没送出去（离线留言下一步才做）」
  - 「太长了，请缩短或者改成派活（下一步）」
- Plus the lines for no such session, ambiguous, stopped, refused, no answer and usage, and the peer, session and tentacle lines.
- English versions have no Han characters, and this is tested.

**`src/main.rs`** routes `send` to `mesh::cli::run` and adds a HELP entry. **`src/ui/mod.rs`**: `widgets` is now `pub(crate)`, so that `deliver` can reuse `session_label`.

## Deviations from the brief, and why

1. **Messages are typed only into agent sessions.** A shell session answers `Refused`. Typing text plus Enter into a shell runs it as a command, which breaks the rule that a message carries no authority to run anything.
2. **Session names follow what the board shows.** The brief says to use the tag, falling back to the project directory name, but also to use the board's function. The board's row function, `ui::widgets::session_label`, falls back to the *profile*, while the project name appears as the group header. So `SessionBrief.name` is `session_label` (the board function), and addresses match either the row label or the project name. A match on more than one session gives `NoSuchSession` with candidates.
3. **New `SendOutcome::NoAnswer`.** When the envelope got through but no valid receipt came back, we do not know whether the message arrived. Reporting that as `Offline` ("没送出去") would be false. The CLI says 「X 没回话，不知道送到没有」.
4. **`NoSuchSession` carries candidates** (`NoSuchSession(Vec<String>)`) so the CLI can list them, as the brief requires.
5. **Retry policy.** Only a relay `Busy` is retried, because in that case the envelope was not accepted. It is retried once, with a fresh id. `NoAnswer` and timeouts are not retried: the first copy may have been typed in already, and a second id would not be deduplicated. Re-running `dct send` always creates a new id.
6. **Queued messages are dropped only when the session is gone or `Stopped`.** A `Failed` session keeps its queue, because it can return to Idle. A *new* message to a `Failed` session still gets `SessionStopped`, as the brief says.
7. **Message line breaks are kept.** Each line is cleaned separately and joined back with `\n`, which is consistent with the marker's own two-line format. All other control bytes and escape sequences are removed.
8. **Typing into the session happens while the `Mesh` lock is held**, inside `on_envelope` and inside the tick. The lock order is always Mesh → session, and nothing that holds a session lock takes the Mesh lock, so this cannot deadlock. `Net` is never called while the Mesh lock is held. The cost is latency: `type_into` runs a git checkpoint, and during it other Mesh requests (status, peers) wait.
9. **Sending to this computer** (`dct send 本机/会话`) is delivered locally without going through the relay. The brief did not cover this case.
10. **`dct peers` lists this computer too**, built locally. It is marked with 「这台」 using `MeshStatus`.

## Mutations (each was applied, the tests run, and the file restored)

Fix commit:

| Mutation | Test that caught it |
|---|---|
| m1: drop the `env.from == signer` check | `a_junk_roster_claiming_the_inviter_as_signer_does_not_clobber_the_held_one` |
| m2: never print endpoints | `join_with_two_responders_of_the_same_name_shows_their_endpoints` |
| m3: ignore an empty `joining` | `join_says_expired_as_soon_as_the_daemon_drops_the_invite` |
| m3: move the deadline back to after confirmation | `the_join_wait_starts_when_the_invites_were_made_not_after_confirming` |
| m3: `NoSuchInviter` carries the endpoint again | `invites_expire_after_the_join_ttl` |

Task 7 (32 mutants; 31 killed, 1 equivalent):

| Mutation | Test that caught it |
|---|---|
| marker uses 6 id chars | `the_marker_is_the_fixed_two_line_format`, `an_idle_session_gets_the_marker_typed_in_at_once`, the e2e idle test |
| marker on one line | same tests, plus `…cleans_a_dirty_one` |
| `ready` also accepts Asking | `only_idle_is_ready`, `asking_is_not_ready_and_queues` |
| queue cap off by one | `the_queue_holds_at_most_50_and_refuses_the_next` |
| LIFO instead of FIFO | `a_busy_session_queues_first_in_first_out_one_at_a_time`, `a_new_message_waits_behind_queued_ones_even_when_idle` |
| flush the whole queue in one tick | `a_busy_session_queues_first_in_first_out_one_at_a_time` |
| an idle session jumps ahead of queued messages | `a_new_message_waits_behind_queued_ones_even_when_idle` |
| no Stopped/Failed check | `a_stopped_or_failed_session_says_so` |
| Failed not treated as stopped | `a_stopped_or_failed_session_says_so` |
| shell sessions allowed | `a_shell_session_never_gets_a_message` |
| receiver skips `clean_body` | `a_receiver_refuses_an_over_long_body_and_cleans_a_dirty_one` |
| receiver skips `clean_label` | `a_receiver_refuses_an_over_long_body_and_cleans_a_dirty_one` |
| 8000 limit off by one | `a_body_over_8000_chars_is_refused` |
| `sanitize` skipped | `a_body_is_cleaned_of_escapes…`, the 8000 test, the dirty-body test |
| first match wins, no ambiguity | `two_sessions_with_the_same_name_are_ambiguous_and_listed`, `an_ambiguous_or_missing_session_is_no_such_session` |
| no project-name match | resolve tests plus the e2e tests |
| queue never dropped (2 variants) | `a_queue_is_dropped_with_a_journal_line_when_its_session_goes_away` |
| retry reuses the first id | `a_busy_relay_is_retried_once_with_a_fresh_id` |
| no Busy retry | `a_busy_relay_is_retried_once_with_a_fresh_id` |
| receipt id not checked | `a_receipt_that_does_not_match_the_message_is_no_answer` |
| no answer still counted as online | `a_machine_that_does_not_answer_the_status_request_shows_as_offline`, `peers_shows_each_machine_with_its_sessions_and_tentacles` |
| object tentacles ignored | `tentacles_come_from_the_dco_endpoint_file_or_are_empty` |
| sender skips the TooLong check | `sending_to_an_offline_or_unknown_machine_says_so` |
| Offline mapped to NoAnswer | `sending_to_an_offline_or_unknown_machine_says_so` |
| `(from, id)` replay check removed | `a_duplicate_message_is_typed_only_once`, `mesh::tests::a_replayed_message_is_dropped_the_second_time` |
| `DCT_SESSION_ID` not set | `session::tests::a_session_child_sees_its_own_session_id` |
| Queued exits 1 | `send_says_what_happened_and_exits_0_only_when_delivered_or_queued` |
| peers detail not printed | `peers_shows_sessions_and_tentacles_of_each_machine` |
| receipt `from` not checked | **SURVIVED, equivalent.** `seal::open` already rejects `m.from != envelope_from` (`FromMismatch`), so the extra check is defence in depth only. |

Tried but not a useful mutant: removing `MeshSend` from the HTTP refusal arm does not compile, because the match must be exhaustive.

## Commands (all run in /Users/lei/work/dc/dc-terminal-mesh)

```
~/.cargo/bin/cargo test --workspace   -> passed 1859 failed 0 ignored 3
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings   -> Finished, clean
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
    -> Finished; only the existing warning (unused `path`, src/student_projects.rs:669)
git diff --check   -> clean
```

Two tests failed once under full parallel load, then passed on rerun and when run alone:
- `daemon::web_tests::enabling_starts_a_listener_and_disabling_stops_it`, a port race;
- `tests/zombie_reaping.rs`, a 10-second shell start-up deadline.

Neither is related to this change. `rustfmt` was applied only to `src/mesh/deliver.rs` and `src/mesh/cli.rs`.

## Concerns

- **Typing while holding the Mesh lock.** For an agent session, `type_into` runs a git checkpoint, which can take seconds on a large repository. Meanwhile the link thread and CLI mesh requests wait. If it ever takes longer than the 10-second send limit, the sender reports `NoAnswer` even though the text was delivered. Splitting receive into two phases (verify under the lock, type after releasing it) would fix this, but it would need `FakeHub` to go through a lock-aware entry point.
- **"One at a time" depends on the Working/Idle detection.** After typing, `send_input` sets `Working`. But `SessionManager::tick` can flip the session back to `Idle` from a stale screen before the agent redraws. In that case the next queued message could be sent about a second later, while the agent is actually busy. The phone bridge has the same limit.
- **The daemon's 1-second delivery thread and the local-socket routing in `serve` have no unit tests.** `tick` and `handle_mesh` do.
- **Queued messages live in memory only**, and they have no age limit apart from the session going away. A daemon restart loses them, and the senders were already told `Queued`.
- **Does a raw `\n` in the marker insert a newline or submit, in each agent CLI?** I did not check this against real Claude, Codex or Qwen sessions. The two-line format is fixed by the spec, and the final Enter is always sent separately.
- **`~/.dco/endpoint.json` has no agreed schema yet.** The parser accepts strings and `{name|kind}` objects. When dco settles the format, this should be revisited.

## Fix round 1 (commit 995e9b9)

Status: DONE_WITH_CONCERNS. Worktree /Users/lei/work/dc/dc-terminal-mesh. English commit message, no Co-Authored-By.

### I1: typing now happens outside the Mesh lock

- **Split into two steps.** `Mesh::on_envelope` was split into `step()` and a finish step.
  - `step()` runs under the lock: verify, deduplicate, then `decide`.
  - `decide` either answers with a `Receipt`, or returns `Decision::TypeNow(Typing { session, text, msg })` and marks that session in `Mesh.in_flight`.
- **New entry point `mesh::handle(&Mutex<Mesh>, env)`.** It runs `step` under the lock, releases the lock, calls `inbox.type_into`, then takes the lock again for `finish_incoming`. That call clears the in-flight mark, journals the result and builds the sealed receipt. `route()` and `FakeHub` now both go through `handle`. `on_envelope(&mut self)` remains as the all-in-one version for tests.
- **The delivery tick is split the same way.** `deliver::tick` calls `take_queued()` under the lock. It pops at most one message per session, only for sessions that are ready and not in flight, and marks each one in flight. It then types outside the lock and calls `finish_typing` under the lock for each message.
- **Sending to this computer** uses the same decide / type / finish sequence.
- **Ordering guarantees.**
  - While a message is being typed into a session, a new message to that session is queued behind it, and an overlapping tick skips that session. So delivery stays one at a time and first in, first out.
  - Deduplication still happens in `step`, before anything is decided, so a duplicate is still typed only once.
- **Tests:**
  - `typing_happens_outside_the_mesh_lock`: the fake's `type_into` calls `try_lock` on the mesh, and it must succeed on both the immediate path and the queued path.
  - `a_message_arriving_while_another_is_being_typed_waits_behind_it`: a second message arrives through `handle` and a tick runs, both from inside the first message's typing. The second message is `Queued`, nothing extra is typed, and after the session is idle again the typed order is m1 then m2.
  - `two_overlapping_ticks_still_send_one_at_a_time_in_order`.
  - `a_duplicate_through_handle_is_typed_once`.
- **`FakeInbox` changes.** It now takes its callback out of the lock before calling it. The nested tests check `try_lock` first, so a lock-holding mutant fails an assertion instead of deadlocking the test run.

### I2: a machine removed from the group can no longer have its queued messages typed

- **At delivery:** `take_queued` checks that `next.msg.from` is still a member of the current roster. If it is not, the message is dropped and journaled as `queued_msg_dropped … why=not_a_member`, and the next queued message is tried.
- **On roster change:** `commit()` calls `purge_non_members()`, which removes queued messages from senders that are no longer in the roster, with one journal line each.
- **Tests:**
  - `queued_messages_from_a_removed_machine_are_purged_when_the_roster_changes`:
    1. Messages from C and from A are queued.
    2. B receives A's v3 roster, which no longer has C.
    3. Only A's message is left.
    4. The session goes idle, and ticking types only A's message.
  - `a_queued_message_is_checked_against_the_current_roster_before_typing`: the roster is swapped without calling `commit`; ticking types nothing and writes the journal line.

### I3: remote text is cleaned before it reaches the terminal

- **OS name:** `PeerView.os` goes through `clean_text(os, 32)`.
- **Candidates:** on the sender side, `NoSuchSession` candidates go through `clean_text(…, 128)`, and at most 10 are kept (`MAX_CANDIDATES`).
- **Test:** `remote_os_and_candidates_are_cleaned_before_they_reach_the_terminal` uses a fake network whose correctly sealed Status and Receipt from B contain escape codes, 20 candidates and a 100-character OS name.

### M1: a failed checkpoint after Enter still counts as delivered

- **One keystroke path.** New `bridge::submit(mgr, id, text) -> anyhow::Result<()>` is the single place that types text and presses Enter. `SessionWriter::type_into` now calls it, so the keystroke path is still one function.
- **Classifying the result.** `Inbox::type_into` now returns `Result<Typed, String>`, and `deliver::typed_from` maps:
  - `CodedError(OperationFailed(Checkpoint))` → `Typed::NoCheckpoint`, because Enter has already been sent;
  - any other error → `Err`.
- **Effect.** `finish_typing` treats `NoCheckpoint` as `Delivered`, with journal result `delivered_no_checkpoint`, on both the immediate and the queued path.
- **Tests:**
  - `a_missed_checkpoint_still_counts_as_delivered`: both paths are covered, no `type_failed` appears, and a real error still gives `Refused`.
  - `only_the_checkpoint_error_is_treated_as_typed`.

### M2: fake markers are neutralised

- `marker()` removes `[` and `]` from the machine name and the session name.
- Any body line that starts with `[来自` gets a leading space.
- **Tests:**
  - `a_fake_marker_in_the_body_or_brackets_in_names_are_neutralised` pins the exact output.
  - `a_received_fake_marker_is_neutralised_end_to_end` runs through the receive path and pins the exact typed text.

### Newline / bracketed paste

- `src/` has no bracketed-paste helper (no match for `200~` or `bracketed`), so I did not add one, and the current behaviour is unchanged.
- The session receives the marker line, `\n`, the body (which keeps its own `\n`s), and then a separate Enter (`send_input(id, "")`).
- **Affected built-in agent profiles:** every agent profile receives the raw LF bytes:
  - `claude`;
  - `kimi`, `glm`, `deepseek` and `qwen-api`, which all run the claude CLI;
  - `codex`;
  - `qwen`;
  - `opencode`;
  - `dc`.
- **Not verified:** I did not check against a live claude, codex, qwen or opencode session whether a raw LF inserts a newline or submits the input early. The code alone cannot tell.
- **If an agent submits on LF:** the marker line would arrive as its own turn, and the body would follow as one or more further turns. Nothing gets the right to run commands, because shell sessions are refused. But the message may be split, and the body lines would lose the marker in front of them.

### Known and left as is (ruling)

- **M3:** queued messages have no expiry time.
- **M5:** `~/.dco/endpoint.json` is read with no size cap.
- **M6:** no test checks that `DCT_SESSION_ID` reaches an *agent*-profile session. The existing test uses a non-agent profile, and the variable is set on the shared path in `create_inner`.

### Mutations (each applied, tests run, file restored)

| Mutation | Result |
|---|---|
| `handle` types under the lock (goes back to `on_envelope` under the lock) | KILLED: `typing_happens_outside_the_mesh_lock`, `a_message_arriving_while_another_is_being_typed_waits_behind_it` |
| `tick` types while holding the lock | KILLED: `typing_happens_outside_the_mesh_lock` |
| `decide` ignores `in_flight` | KILLED: `a_message_arriving_while_another_is_being_typed_waits_behind_it` |
| `take_queued` ignores `in_flight` | KILLED: `…waits_behind_it`, `two_overlapping_ticks_still_send_one_at_a_time_in_order` |
| `in_flight` never cleared | KILLED: 5 tests, including FIFO, overlapping ticks and checkpoint |
| no member check at delivery | KILLED: `a_queued_message_is_checked_against_the_current_roster_before_typing` |
| no purge on commit | KILLED: `queued_messages_from_a_removed_machine_are_purged_when_the_roster_changes` |
| OS not cleaned | KILLED: `remote_os_and_candidates_are_cleaned_before_they_reach_the_terminal` |
| candidates not capped | KILLED: same test |
| candidates not cleaned | KILLED: same test |
| `NoCheckpoint` treated as refused | KILLED: `a_missed_checkpoint_still_counts_as_delivered` |
| `typed_from` treats the checkpoint error as a failure | KILLED: `only_the_checkpoint_error_is_treated_as_typed` |
| fake marker line not padded | KILLED: both M2 tests |
| brackets kept in names | KILLED: both M2 tests |

In the first run, the four lock and in-flight mutants were caught only by the test runner hanging on a deadlock. I then changed `FakeInbox` to release its callback lock before calling it, and made the nested tests check `try_lock` first. After that, all four fail on an assertion instead of hanging, as shown in the table.

### Commands

```
cargo test --workspace   -> passed 1870 failed 0 ignored 3
cargo clippy --workspace --all-targets -- -D warnings   -> clean
cargo check --workspace --all-targets --target x86_64-pc-windows-msvc -> only the existing student_projects.rs:669 warning
git diff --check -> clean
```

### Remaining concerns

- **"One at a time" still depends on Working/Idle detection.** `send_input` sets `Working`, but a stale screen can flip the session back to `Idle` before the agent redraws.
- **The daemon's 1-second thread and the local-socket routing in `serve` are still not unit-tested.**
- **The raw-LF question above is unverified.**
- **A panic inside `type_into` would leave the session marked in flight**, so it would never be delivered to again until the daemon restarts. `type_into` has no known way to panic.
