# Final whole-branch review — dct multi-machine step 1

- Worktree: `/Users/lei/work/dc/dc-terminal-mesh`, branch `feat/dct-multi-machine-step1`
- Range: `8239173..b078195` (Tasks 5–9. Tasks 1–4 were already on main at base; I re-read them only where they meet the new code: relay `check/poll/send/ask/peers`, `seal`, `roster::accept*`, `sas`, `wire`)
- Method: superpowers `code-reviewer.md`, read-only. I did the review myself in several passes and dispatched no subagents.

## Verdict

**Needs fixes before go-live: 1 Critical, 5 Important, 7 Minor.**

The code is careful and well tested inside each task. Replay handling, lock discipline, account isolation at the relay and the from/to/id binding in `seal` all hold across task boundaries. The Critical finding is in the join design itself: the 6-digit compare does not deliver the spec's promise that "服务器就算伪造公钥，也凑不出一样的数字". The Important findings are cross-task seams that no single per-task review could see.

## Verification run

| Command | Result |
|---|---|
| `cargo test --workspace` (CARGO_TARGET_DIR=/tmp/dct-mesh-final-review) | **all green**: dct lib 1586 passed; dct-mesh 90; dct-srv 97; dct-link 13; dct-brain 49; every integration test binary passed; 4 ignored (manual-look tests + `mesh_e2e`) |
| `cargo test --test mesh_e2e -- --ignored` | **passed** (6.4 s) |
| `cargo clippy --workspace --all-targets -- -D warnings` | **clean** |
| `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc` | **passes**. One warning, `unused variable: path` at `src/student_projects.rs:669`, is pre-existing (the file is not in this diff) |

---

## Critical

### C1. The 6-digit compare can be ground by the relay; nothing commits either side before the code is fixed

- **Where**
  - `crates/dct-mesh/src/sas.rs:12-28`: the code is `SHA-256(sorted endpoints + sign_pub + kx_pub) mod 10^6`, with no nonce and no commitment.
  - `crates/dct-mesh/src/wire.rs:16-31`: `Join` and `JoinPending` carry only the static self-signed `Member`.
  - `src/mesh/mod.rs:385-418`: `take_join` shows the code as soon as a `Join` arrives.
  - `src/mesh/group.rs:120-135`: the joiner computes its code from the `JoinPending`.
- **Why it breaks the spec.** 第 1 段 says the relay "伪造公钥，也凑不出一样的数字". That holds only when neither side can choose its key after seeing the other's. Here every public key is visible to the relay in plaintext (`Payload::Join`, `Payload::JoinPending`, `Payload::Roster`), and existing members' keys are static and long-lived. That opens a key-grinding attack.
- **Failure scenario** (malicious or compromised relay on dataclue.cn — exactly what the SAS exists to stop):
  1. New machine B runs `dct join`. The relay learns B's `Member` from the `Join`.
  2. The relay answers B with a replay of A's genuine `JoinPending`. That record is static and self-signed, and any earlier join captured it. The relay can also simply forward B's Join once.
  3. B's screen shows `code(A,B)`.
  4. The relay generates X25519 keypairs for a fake machine X until `code(A,X) == code(A,B)`. That is about 10^6 X25519 keygens, only `kx_pub` needs to vary, and the endpoint stays fixed. It takes roughly a minute on one core and seconds on a laptop's cores. The relay can also pre-compute a code→key table for A, because A's keys never change.
  5. The relay sends A a `Join` from X, with `from` set freely (the relay is the one authenticating `from`) and `name` set to B's name. The name is not in the code.
  6. A shows "一台叫 公司Windows … 数字是 <same code>". The user compares, sees a match, and approves.
  7. X enters the signed roster and gets it broadcast to every member. It can then type marked messages into any idle agent session on every machine, read `dct peers` status (session names, directories, tentacles), and receive messages meant for "公司Windows". B just times out.
- **Suggested fix.** Use a ZRTP/Bluetooth-style commit–reveal inside the existing round trips:
  1. The joiner sends `Join { member, sig, commit: H("dct-sas-commit-v1" ‖ member ‖ nB) }`.
  2. The inviter replies `JoinPending { member, sig, nA }`, with a fresh random `nA` per request.
  3. The joiner sends `JoinReveal { nB }`, which the inviter checks against `commit`.
  4. Both sides compute `code = H(sorted members ‖ nA ‖ nB)`.

  The inviter must not display a code until the reveal verifies. The joiner must not display one until it has `nA`. Neither party can then choose its contribution after seeing the other's, and a MITM gets one 10^-6 guess per attempt. Other changes:
  - Include `name` in the hashed transcript so the user's "是这台" also covers the name.
  - Add a `MAX_JOIN_ATTEMPTS` or rate limit per account, so repeated online guesses show up.
  - Add tests: a relay that substitutes X after seeing B's commit cannot make the codes match except by chance. Replaying an old `JoinPending` should also produce a different code, because `nA` is fresh.
  - Update the dct-sas-v1 → v2 KAT and the spec 第 1 段 text.

  If this is deferred, the spec must stop claiming the relay cannot forge a match, and "a compromised relay can join a machine" must go into 已知限制. Go-live on dataclue.cn should wait for the user's decision.

---

## Important

### I1. Machine names are never checked for control or format characters, and two sinks use them raw

- **Where**
  - `crates/dct-mesh/src/roster.rs:167-172` (`validate_name`), `src/mesh/mod.rs:611-613` (`valid_name`) and `src/mesh/store.rs:137` accept anything that is non-empty, has no `/`, and is 32 chars or fewer. That includes `\r`, `\n`, `\x03`, `\x1b[…`, U+202E and so on.
  - Sink 1, the marker. `src/mesh/deliver.rs:51` (`strip` removes only `[`/`]`) is fed at `deliver.rs:466-472` with the sender's roster name uncleaned. The body goes through `clean_body`/`sanitize` and the session label through `clean_label`, but the machine name goes to `bridge::submit → SessionManager::send_input`, which writes raw bytes to the agent's PTY.
  - Sink 2, the CLI. `src/mesh/group.rs:40,58,74` put raw names into `MeshView`. `src/mesh/cli.rs:262-264, 363, 372, 404, 450` print them with `writeln!` straight to the user's terminal. The TUI does clean them (`src/ui/computers.rs:236`, Task 8 M4); the CLI never does.
- **Failure scenarios**
  1. A machine whose `JoinRequest` name contains `\x03` or `\x1b` gets approved; the TUI shows the cleaned name, so the user sees nothing odd. Every message it sends then types Ctrl-C or Esc into the receiving agent: Esc cancels Claude Code's current turn. With `\r` it submits the marker line on its own.

     This breaks the invariant that "agents only get typed text with a marker". Only the part of the message that is supposed to be trustworthy, the header, is unfiltered.
  2. Anyone who can send a `Join` can inject terminal escape sequences into the terminal of whoever runs `dct peers` or `dct join`. That means any same-account token holder, or the relay via C1, and no approval is needed. The sequences can conceal or rewrite the displayed code line; `\x1b[8m` hides the rest of the line.
- **Suggested fix**
  - Make one `dct_mesh::roster` rule reject any char that is `is_control()` or in the `Cf` set: move `is_format_char` into dct-mesh. Apply it in `validate_name`; `accept`, `accept_invite`, `take_join` and `group::join`'s `JoinPending` check all go through it or `valid_name`. Use the same rule in `store::set_name`/`tidy_name`.
  - As a second line of defence, run `clean_label` on the machine name in `marker()`.
  - Clean names in `group::view`/`joining` once, rather than per sink, so the CLI and TUI share one cleaned view.
  - Tests: a roster or join with `"a\x1bb"` / `"a\u{202e}b"` is refused; `marker("a\x03b", …)` contains no control bytes.

### I2. `dct login | join | peers | send` skip the protocol handshake, so an old running daemon produces a raw serde error

- **Where**: `src/mesh/cli.rs:43-65`. `Client::connect`/`connect_or_start` and then straight to `MeshStatus`. Compare the handshake precedent at `src/main.rs:206` (`daemon_status(client.protocol())`) and `PROTOCOL_VERSION = 23` at `src/proto.rs:136`.
- **Failure scenario**: this is the first real two-machine run. The user installs the new `dct` on the Mac, whose daemon from yesterday is still running, and types `dct login`. The old daemon cannot parse `MeshStatus`. The CLI then prints `请求解析失败：unknown variant \`MeshStatus\`, expected one of …` and exits 1. This violates the "说人话" rule, and nothing tells the user to run `dct restart`. It will happen on both machines on day one. It is the same class of bug the memory already records for `ps/stop/kill/prune`.
- **Suggested fix**: after connecting in `mesh::cli::run`, call `client.protocol()`. If `daemon_status(..)` is not `Same`, print the existing "守护进程是旧版本，运行 dct restart" style message in the i18n key the TUI path uses, and exit 1. Add a test with a fake daemon that answers `Hello` with protocol 22.

### I3. Raw LF in multi-line messages is still unverified against real agent CLIs, and the e2e test cannot catch it

- **Where**: `src/mesh/deliver.rs:50-70` (the marker is joined with `\n`) → `bridge::submit` (`src/bridge.rs:214-219`) writes the text raw. `tests/mesh_e2e.rs` uses `cat` as the "agent".
- **Status**: carried in the ledger ("LF-submit in real agents unverified", Task 7 ruling). It is not worse than ledgered, but it is still open and blocks go-live, because it decides whether the core property holds.
- **Failure scenario**: if any agent CLI treats LF as submit (Codex or opencode in some modes), the marker line is submitted alone as one turn. The body then arrives as a separate, unmarked turn that the agent reads as the user's own instruction. That is exactly what the marker exists to prevent.
- **Suggested fix**
  - Make it an explicit step in README go-live step 4: send a two-line message into Claude Code and into Codex on the Windows box, and check that each arrives as one turn.
  - Or, preferably, wrap the typed text in bracketed paste (`ESC[200~ … ESC[201~`) for agent profiles that support it, so newlines stay literal.
  - Record the result in the ledger.

### I4. The spec's 「已知限制」 section, promised by the Task 6 ruling "at finish", was never written

- **Where**: `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md` has no such section (grep finds nothing). The README has a user-facing list (`README.zh-CN.md` 「现在还做不到的」), but it leaves out the security-relevant items.
- **Failure scenario**: the next step's planner reads the spec, not the README, and treats these as solved:
  - roster forks under concurrent approvals;
  - a removed machine can still push a roster to a member that missed the removal;
  - cross-joins between two new machines;
  - pending joins lost on restart;
  - a removed machine's relay token stays valid (see M2);
  - C1, if it is deferred.
- **Suggested fix**: add the section to the spec, per the ledger ruling, listing each item with its cost and the step that will address it.

### I5. `MeshStatus`/`MeshApprove`/`MeshRemove`/`MeshConfirmInviter` do relay I/O but get the CLI's 5 s local timeout

- **Where**:
  - `src/mesh/cli.rs:70-76`: only Login, Join, Peers and Send count as "slow".
  - `src/mesh/group.rs:29`: `view()` calls `net.peers()` on every `MeshStatus`, using an agent with a 10 s connect timeout and a `POLL_TIMEOUT + 10 s` = 40 s read timeout (`src/link.rs:98-100`, `src/link.rs:120-126`).
  - `approve`/`remove` also `broadcast` over the network, then call `view()`.
- **Failure scenario**: on a network that black-holes dataclue.cn (corporate firewall, flaky Wi-Fi on the Windows box), `dct peers` and `dct join` fail on their first `MeshStatus` with "守护进程没响应", which is the wrong diagnosis. Worse, `dct peers approve X` can time out on the client after the daemon has already committed and broadcast the new roster. The user then sees an error for an approval that happened, and may retry or approve again.
- **Suggested fix**
  - Give `MeshStatus` a bounded, cached `online` set: have the link thread refresh it, or have `view()` use a 3 s `peers` call and mark members unknown on timeout.
  - Put Approve, Remove and ConfirmInviter in the `slow` set, or have them return before the broadcast and broadcast off-thread.

---

## Minor

### M1. Token renewal hammers the gateway whenever the issued lifetime is 1 day or less (question 4)

- **Where**: `src/daemon.rs:521-557` (the `before_poll` closure) and `src/mesh/login.rs:80-82` (`needs_renewal`), `:127-165` (`renew_if_due`).
- **Behaviour**
  - `before_poll` runs before every poll. After a successful renewal, `last_failure` is reset to `None`, so the only guard left is `needs_renewal(exp, now)`.
  - If the gateway issues tokens with lifetime ≤ 86400 s, every poll renews. It also renews after every envelope, since each envelope ends the long poll early, and on every backoff retry, from 0.5 s up to 30 s, while the relay is down. Each renewal is a gateway HTTP call plus a `secrets.toml` rewrite plus a journal line.
  - The same happens if the gateway returns an `exp` in the past (clock skew, or a misconfigured TTL).
- **Severity**: Minor. Production TTL is 7 days (ledger), so this fires once in the last day and then stops. It only becomes real if the gateway TTL is ever lowered below a day or the gateway's clock is badly skewed. It is still a small, cheap fix, and worth doing so a gateway misconfiguration cannot turn every dct install into a load generator.
- **Suggested fix**
  - Keep a `last_attempt: Instant` and skip if `elapsed < RELAY_RENEW_MIN_INTERVAL` (for example 10 min), whether or not the last attempt succeeded. The existing 1 h is for failures.
  - Optionally renew at `min(1 day, (exp - issued_at)/2)` remaining, if the response ever carries `iat`.
  - Test: a transport that always returns `exp = now + 3600` is called once across many `before_poll` calls within the interval.

### M2. Spec 第 1 段 "它的账号凭据也在中转上作废" on removal is not implemented

`src/mesh/group.rs:203-232` only re-signs the roster. The removed machine keeps a valid relay token for up to 7 days. With it, it can:

- list which of the account's endpoints are online (`/link/peers`);
- flood `Join`s, capped at 16 pending (`MAX_PENDING`), which re-prompts the user;
- push rosters to members that missed the removal (already ledgered).

This needs gateway and relay revocation, which is out of scope for step 1. Record it in 已知限制 (I4) and make sure the README's removal wording does not imply revocation.

### M3. A clock-skewed sender's messages are silently dropped and look like "no answer"

- **Where**: `src/mesh/mod.rs:552-557`.
- **What happens**
  - `stale` fires when skew exceeds 10 min.
  - `before_start` fires when the sender's clock is behind the receiver's restart time; this rejects every message sent in the first few seconds or minutes after the receiver restarts.
  - Both are journal-only. The sender sees `NoAnswer` after 10 s, with no hint.
- **Fix**: this is acceptable for step 1. Add "check both clocks" to the `NoAnswer` user text, or have `Status` replies carry the peer's `now`, so that `dct peers` can warn when skew exceeds 60 s.

### M4. The 401 text is misleading for tenant-shared API keys

`src/mesh/login.rs:71` returns "登录已失效，请先在 dct 里重新配对 DC 账号" for every 401. The gateway notes (spec appendix, 网关实现备注) say tenant-shared keys always get 401, so re-pairing with the same kind of key never helps. That could be the very first `dct login` outcome for a teacher-provisioned key.

The text is frozen contract wording, so the fix is to have the gateway return a distinct `{"error": "shared_key"}` body and dct map it to a clear message. Until then, add it to the README troubleshooting notes.

### M5. The relay-log privacy assertion in e2e is vacuous

In `tests/mesh_e2e.rs`, "the relay output does not contain the message body" cannot fail: the body is encrypted, and the relay never logs payloads. This is ledgered as weak.

A meaningful version would also assert that the relay output contains no base64 `ct`/`eph` from the envelope and no machine names or session names. Machine names are plaintext in `Join` and `Roster` payloads, so a future relay debug log could leak them.

### M6. `Mesh::reply` answers from the receiver's `to_session` string as sent, not the resolved session

`src/mesh/mod.rs:582-598` uses `from_session: to_what.to_session` rather than the resolved board label. This is cosmetic. It becomes relevant only when step 2 starts using the reply's `from_session` for `dct task`.

### M7. `route()` catches panics but still unwinds through `Mesh` with state half-updated

`src/mesh/mod.rs:652-669`. `InFlight` covers `in_flight`. A panic inside `step()` itself (a bug) leaves the lock poisoned. That is recovered via `into_inner` everywhere, so it is fine today; note it for step 2 when `step()` grows.

---

## Cross-task seams checked and found sound

- **Account isolation at the relay across T3/T5.**
  - `check()` binds token→endpoint.
  - `poll` refuses re-registration by another account while the id is present.
  - `send` reports Offline across accounts.
  - Ask slots record `(asker account, asked endpoint)`, and `send` fills a slot only when both match.
  - `LinkNet::ask` re-checks `reply.from`/`seq` (`src/mesh/net.rs:77-86`).
  - `seq` starts at epoch milliseconds, so restarts do not collide with lingering slots.
- **Token sharing and renewal (T4/T5).** The `Token` cell is shared by `Link` and `LinkNet`. Re-login swaps the cell. `Debug` redacts the token. The api_key goes only to the gateway, in the Bearer header.
- **Roster acceptance (T1/T5/T6).**
  - A machine that is not in a group never takes a roster from the wire; only `accept_invite` from a human-confirmed inviter does.
  - A held invite roster is keyed by the relay-authenticated sender.
  - `commit` purges pending joins and queued messages from non-members.
  - `take_queued` re-checks membership.
- **Replay window and restart (T5).** `(from,id)` dedup plus the ±10 min window plus `sent_at >= started_at` blocks replaying messages accepted before a restart. Answers to `ask` are matched by fresh random id + kind + from (`deliver.rs:666-673`).
- **Sealing and AAD (T2/T7).** The signature covers id, kind, from, from_session, to, to_session, body and sent_at. `open` checks `from == envelope.from` and `to == me`, and rejects an all-zero shared secret. The constant AAD is fine because the key is per message.
- **Marker injection through the session label.** Covered: `clean_label` plus bracket stripping. The body's fake markers are neutralised, including behind Cf/whitespace. The machine-name path is the gap (I1).
- **"Relay never parses payloads".** dct-srv is untouched in this range, and its guard test still pins `dct_mesh::relay_token` as the only dct-mesh use.
- **"Default deny" and "agents only get typed messages".**
  - Shell sessions are refused.
  - `Asking` is not ready.
  - `Mesh*` requests over the relay proto path are refused (`src/daemon.rs:1249-1257`).
  - `route(.., None)` means phone envelopes get no reply.
- **Windows portability.**
  - Key files use `sys::fs::create_private` (0600/ACL) plus `restrict_dir_to_owner`.
  - Key creation is locked with `create_new`.
  - Writes are temp + `rename`, which uses `MoveFileExW(REPLACE_EXISTING)`.
  - `hostname()` is `cfg(unix)` via `libc::gethostname`, `cfg(windows)` via `COMPUTERNAME`.
  - The permission test is `#[cfg(unix)]`.
  - e2e uses `common::posix_tool` and is `#[ignore]`.
  - The Windows `cargo check --all-targets` passes.

  I found no unix-only API without a cfg gate in the new code, and no new test that would fail on windows-latest. Daemon tests pointing at `127.0.0.1:9` only affect the background link thread, which is slower to get "refused" on Windows but asserts nothing.

## Things that would stop the first real two-machine run

In order of likelihood:

1. **I2**: an old daemon still running gives a raw serde error.
2. **Deployment**: dataclue.cn currently runs a pre-mesh dct-srv. The relay must be the `LINK_VERSION`-matching build with `--relay-keys`, and the reverse proxy must strip `/dct-relay`. The README covers this.
3. **M4**: a tenant-shared DC key gets 401 forever.
4. **I5**: slow or black-holed networks make approve report failure after it succeeded.
5. **I3**: the LF behaviour in real agents.
