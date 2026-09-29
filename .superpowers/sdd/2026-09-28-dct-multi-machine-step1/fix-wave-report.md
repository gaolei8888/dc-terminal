# Fix wave report: dct multi-machine step 1

- Worktree: `/Users/lei/work/dc/dc-terminal-mesh`, branch `feat/dct-multi-machine-step1`
- Base: `b078195`. Head: `6d62005`. Nothing was pushed, merged or deployed.
- Scope: C1, I1, I2, I5, M1 and I4 from `final-review.md`. I did not touch code for M2–M7 or I3.
- Commits are in English, with no Co-Authored-By and no AI attribution.

## Commits

| Commit | Finding | Summary |
|---|---|---|
| `5883bc0` | M1 | `mesh::login::Renewer` makes attempts at least 10 min apart whatever the outcome, and 1 h apart after a failure. `NotDue` does not count as an attempt. The daemon's `before_poll` uses it. |
| `238435a` | I2 | `mesh::cli::dispatch` does a `Hello` handshake first (`daemon_status`). A stale daemon, or one that cannot answer `Hello`, gets `StaleDaemonExplain` plus 「要重启，运行：dct restart」 and exit 1. Nothing restarts automatically. |
| `7257f8c` | I5 | `LinkNet` has its own agent with an 8 s whole-request timeout (`net::CALL_TIMEOUT`); asks keep their per-call timeout. Roster broadcasts go out in parallel. `mesh::worst_case(req)` states the daemon's bound per `Mesh*` request (longest is `MeshJoin`, 34 s). The CLI waits 60 s for every `Mesh*` request (`cli::wait_for`); a test asserts it is at least 10 s above each worst case. The gateway timeout became the named `login::GATEWAY_TIMEOUT` (15 s). |
| `4402cf2` | I1 | `dct_mesh::roster::valid_name` also rejects Cc and Cf characters. `is_format_char` moved into dct-mesh so both sides share one table. It applies to roster acceptance, `take_join`, the joiner's `JoinPending` check (newly added there), `store::set_name` and `tidy_name`. `marker()` cleans both names again as a second line of defence. `group::view` and `joining` clean names once. The CLI cleans everything it prints from other computers through the shared `deliver::clean_name`, which the TUI now reuses too. |
| `2991d0e` | C1 | Commit–reveal inside the join round trips (dct-sas-v2); see below. |
| `6d62005` | I4 | Spec gains 「已知限制」 with 20 items, and the removal paragraph no longer implies relay revocation. The renewal note is updated. README.md and README.zh-CN.md get go-live step 5 (the LF check in Claude Code and Codex, with bracketed paste as the fallback). |

## C1 design as built

1. **Joiner.** It creates a fresh 32-byte nonce for each machine it asks; machines never share one. `Join` = {self-signed member, sig, `commit`}, where `commit = SHA-256(field("dct-sas-commit-v2") ‖ rec(joiner) ‖ field(b64 n_joiner))`.
2. **Inviter.** It stores the commit, draws a fresh `n_inviter` for each request, and replies `JoinPending {member, sig, nonce}`. There is no code yet. It keeps a snapshot of its own member record so the code uses exactly what it sent.
3. **Joiner, after the replies arrive.** It computes the code and calls `set_invites`, and only then sends `JoinReveal {nonce}` in parallel. Doing it in that order means a roster that comes back right away is already recognised as coming from an inviter. An invite whose reveal send fails is dropped from the joiner's list.
4. **Inviter, on reveal.**
   - It looks up the pending request by the relay-authenticated `from`.
   - A reveal that does not open the commit drops the whole request.
   - A second reveal is ignored.
   - At most `MAX_CODES_PER_TTL` = 20 codes are shown per `JOIN_TTL` (10 min). Beyond that the request is dropped.
   - Pending requests that have no code yet are neither shown nor approvable.
5. **The code.** `code = SHA-256(field("dct-sas-v2") ‖ rec(inviter) ‖ rec(joiner) ‖ field(b64 n_inviter) ‖ field(b64 n_joiner))`. Take the first 4 bytes big-endian, mod 10^6, zero-padded. `rec` = name, endpoint, sign_pub, kx_pub, so the machine name is covered.
6. **KAT.** An independent Python script computed commit = `1b37730e…434c50` and code = `204106`. The value is pinned in `sas::tests::commit_and_code_match_the_fixed_vectors` and written into the spec, which now defines dct-sas-v2 byte for byte. The v1 vector `280288` is gone. There was no appendix copy of the SAS definition to update: the gateway appendix never contained it.

Tests added:

- **sas.rs**
  - KAT.
  - Roles are not interchangeable.
  - Every input changes the code (name, both keys, both nonces).
  - A reveal opens only its own commitment.
  - **Grinding.** A relay that learns `n_inviter` can find a nonce that matches the target. Nonce #1181180 was found offline and is only verified in the test. That nonce cannot open the commitment made earlier, and the committed nonce does not match.
  - With the commitment fixed, 1000 fresh inviter nonces give 0 hits.
  - A replayed `JoinPending` gives a different code.
- **group.rs**
  - No code before the reveal; approve gives `NoSuchRequest`; a wrong reveal kills the request.
  - A second reveal does not change the code.
  - A full relay-in-the-middle run: an old `JoinPending` from A is replayed to B, then a fake X named "B" retries 30 times. A never shows B's code, and shows exactly 20 codes.
  - Each machine asked gets its own commitment.

## Verification (final state, `6d62005`)

- `cargo test --workspace`: 33 test binaries all `ok`; 1928 passed, 4 ignored. That is the dct lib's 1605, dct-mesh's 97, and the rest.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --test mesh_e2e -- --ignored`: passed (real relay, two daemons, login → join → approve → peers → send, now over the three-step join).
- `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc`: passes. Its only warning (`src/student_projects.rs:669`, unused `path`) was there before. The new code adds no unix-only API; the new net test binds `127.0.0.1:0`, which works on Windows.

## Hand mutations: each fix was reverted once, and each test failed

| Mutation | Tests that failed |
|---|---|
| `Renewer` does not record a successful attempt | `a_short_lived_token_is_renewed_at_most_once_per_interval` |
| `dispatch` skips `daemon_is_current` | all 3 handshake tests |
| `LinkNet` uses the 40 s link agent again | `a_relay_that_never_answers_is_given_up_on_within_the_call_timeout` (the run took 80 s) |
| `wait_for` uses the old slow set | `the_cli_outwaits_the_daemon_on_every_mesh_request` |
| sequential broadcast (original code; failed before the fix) | `a_broadcast_takes_one_send_not_one_per_member` |
| `valid_name` without the Cc/Cf check | roster test; `mesh::valid_name` delegation (group tests) |
| `marker` without `clean_name` | 2 marker tests |
| `view` without cleaning | `the_view_hands_out_cleaned_names` |
| joiner without the `valid_name` check on `JoinPending` | `a_join_reply_whose_name_has_control_characters_is_not_an_invite` |
| CLI `c()` as identity | `names_from_other_computers_are_cleaned_before_printing` |
| inviter shows a code at `Join` time | 3 group tests, including the MITM test |
| the reveal is not checked against the commit | `the_inviter_shows_no_code_until_the_reveal_opens_the_commitment` |
| no rate limit | `a_relay_cannot_make_the_inviter_show_the_code_the_joiner_sees` |
| nonces left out of the code | 6 sas tests |
| name left out of the record | 4 sas tests |
| one nonce shared across peers | `each_computer_asked_gets_its_own_commitment` |

## Behaviour changes worth knowing

- **Marker line.** A newline inside a sender's session name is now stripped. The old test expected the marker head to span two lines, and I updated that expectation on purpose: the marker is always one line now.
- **Re-running `dct join`** gives a new code. The old code on the inviter's screen stops working and approving with it returns `CodeMismatch`. I updated the flooding test to match.
- **Mesh wire format.** It changed incompatibly (`JoinRequest.commit`, `JoinPending.nonce`, new `join_reveal`). Nothing is deployed, so there is nothing to migrate. The CLI↔daemon protocol is unchanged and `PROTOCOL_VERSION` stays 23.

## Concerns / open items

- **Lost reveals.** `JoinReveal` is fire-and-forget. If the relay accepts it and then drops it, the new machine shows a code while the old machine shows nothing. This is a safe failure (no approval is possible), but the user gets no explanation.
- **The rate limit is a DoS lever.** Any same-account machine can use up the 20 codes / 16 pending slots and block real joins for 10 minutes. That includes a removed machine whose token has not expired (M2) and the relay. It is documented as 已知限制 #6.
- **Timing tests.** `a_broadcast_takes_one_send…` has a 1.2 s bound for 6×300 ms sends, and the net timeout test has a 5 s bound for two 300 ms calls. They could flake on a very slow CI runner.
- **Unbounded local send.** `dct send` to a session on this same computer runs a git checkpoint before typing, and nothing bounds that. The CLI waits 60 s. This is documented as 已知限制 #20.
- **Not covered by this wave.**
  - The TUI's own `Mesh*` calls were not re-audited for I5. `ANSWER_TIMEOUT` is 40 s, above approve's 16 s worst case.
  - `ps`/`stop`/`kill`/`prune` still skip the handshake (from memory; out of scope).
  - `store::name()` does not validate a hand-edited name file. Other machines would refuse such a name.
- **I3 is still open.** Whether LF submits early in real agent CLIs is now go-live step 5 in both READMEs and 已知限制 #12. Checking it costs real prompts on the user's account, so it has to be done live.
- **Stray diff.** `.superpowers/sdd/.gitignore` shows as modified in the worktree. That change is not mine; I left it unstaged.

---

## Round 2 (after `fix-wave-review.md`)

Same rules as the first round: every fix went in test first, and I reverted each one by hand once to check that its test fails. Head is now `19456e3`.

| Commit | Item | Summary |
|---|---|---|
| `21027c0` | NEW Important (joiner cap) | `group::join` now dedupes the relay's online list. If more than `MAX_JOIN_ASK` (16) distinct machines remain, it asks none and shows no codes, and returns the new `MeshProblem::TooManyAnswered` ("一下子回应的电脑太多了，这不正常，这次不给你核对数字。过一会儿再试一次"). The spec now states the bound on both screens (line 70 and 已知限制 #6). I also reworded the reveal-order sentence the reviewer flagged. |
| `d2605aa` | Minor (separators) | New `dct_mesh::roster::is_hidden_char` covers Cc, Cf, U+2028/U+2029 and the invisible fillers U+115F/1160/3164/FFA0/2800. `valid_name`, `deliver::clean_name` (so the marker, CLI and TUI) and `store::tidy_name` all use it. |
| `19456e3` | Minor (expired-token retry) | In `Renewer`, if the stored `exp` is already past (or unreadable), a failed renewal is retried after `RENEW_RETRY_WHEN_EXPIRED` (1 min). Otherwise the rules are unchanged: 10 min after an attempt, 1 h after a failure. The spec's renewal notes are updated. |

**Tests added**

- **Joiner cap:**
  - `a_relay_listing_100_computers_gets_no_codes_shown_and_no_one_asked`: 100 self-signed fakes, all named 家里Mac. Result: 0 asked, no invites, `TooManyAnswered`.
  - `the_same_computer_listed_many_times_is_asked_once`: the same peer listed 50 times is asked once.
  - `up_to_the_cap_every_computer_is_asked`: 16 fakes plus 3 duplicates of one are all asked, 16 in total.
- **Separators:** the roster reject list and the marker/`clean_name` test gained U+2028, U+2029 and the fillers.
- **Expired token:** `an_expired_token_whose_renewal_fails_is_retried_after_a_minute`.

**Test change:** `flooding_joins_cannot_evict…` now takes its 21 flood nodes (and the impostor) offline after flooding. Otherwise B's re-join sees more than 16 peers online and is correctly refused.

**Mutations (each reverted afterwards):**

| Mutation | Tests that failed |
|---|---|
| Cap disabled | the 100-peer test |
| `dedup` removed | the repeat test and the cap test |
| Separator/filler set emptied | the roster test and the marker test |
| Expired branch forced off | the expired-retry test |

**Protocol note:** `MeshProblem` gained a variant. `PROTOCOL_VERSION` stays 23; this branch already bumped it from 19 and it has not shipped. The shape-pinning tests still pass.

**Verification (`19456e3`)**

- `cargo test --workspace`: 33 result lines, all ok; 1932 passed, 4 ignored.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --test mesh_e2e -- --ignored`: passed.
- Windows `cargo check --all-targets`: passes. Its only warning was already there before this work.

**Concerns**

- **The cap is a blunt refusal.** If the relay adds 17 or more fake online endpoints, it can block every `dct join` indefinitely. That is DoS, which the relay can do anyway by dropping traffic. I did not add it to 已知限制; it could be listed there.
- **Hung daemon misreported.** Left as is (out of scope): a daemon that is running but does not answer `Hello` within 5 s is still told it is "the old version".
- **Rust version.** The expired-check uses `Option::is_none_or`, which needs Rust 1.82. `src/live.rs` already uses it, and the crate declares no `rust-version`.
