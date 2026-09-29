# Fix-wave re-review: dct multi-machine step 1

- Worktree `/Users/lei/work/dc/dc-terminal-mesh`, range `b078195..6d62005` (6 commits). Read-only; no subagents.
- Scope: C1 (priority), I1, I2, I5, M1, I4 from `final-review.md`, against `fix-wave-report.md` and the spec.

## Verdict

C1's core commit-reveal is **sound**: the order is right and the commitment binds the right things. One **new Important** gap remains: the joiner side has no cap on how many inviter codes it shows. The spec's "at most 20 codes per window" bound is enforced only on the inviter, so a relay can buy guesses on the joiner's screen instead. Every other finding is ADDRESSED; I found only Minor residue.

**New: 0 Critical, 1 Important, 4 Minor.**

## Verification

`cargo test --workspace` (CARGO_TARGET_DIR=/tmp/dct-mesh-fixwave-review): 33 result lines, **1928 passed, 0 failed, 4 ignored**. This matches the report.

I recomputed the KAT independently in Python from the spec text, using `field` = decimal byte length + `:` + bytes:

- commit = `1b37730e640d4c4ee0d2b1915492a5c86aa2c67a66525eb3bc50e36983434c50`
- code = `204106`

Both equal `sas.rs:131-141` and spec lines 88-99. Spec and code agree byte for byte.

---

## C1: ADDRESSED (core), plus one NEW Important on the joiner side

### What holds

**Order.**

- Joiner B commits `commit(rec_B, nB)` in `Join` (`mod.rs:381-388`, `group.rs:114-124`).
- Inviter A draws `nA` only on receiving the commit (`mod.rs:453`).
- B computes its code and reveals only after it has `nA` (`group.rs:146-173, 176-187`).
- A shows no code until `opens()` verifies (`mod.rs:474-509`), and `approve` refuses code-less entries (`group.rs:224-230`).

The party that reveals last (B) is locked before it sees `nA`. The party that picks after seeing the commit (A) picks `nA` before `nB` is revealed. I tried every interleaving of the two relay sessions (relay↔B posing as A, relay-as-X↔A):

- **X session first.** `codeA` is fixed once `nA2` arrives, because `nX` is already committed. The relay must then pick `nA1` for B while seeing only B's commit.
- **B session first.** `codeB` is fixed. The relay must commit `nX` before A's fresh `nA2`.
- **Interleaved.** Same two constraints.

In every order the relay makes one blind 10⁻⁶ guess per attempt.

**Binding.** `rec` = name ‖ endpoint ‖ sign_pub ‖ kx_pub, all length-prefixed (`sas.rs:46-51`). The commitment therefore binds key, name and nonce. A reveal is looked up by the relay-authenticated `from`, and `opens(p.commit, p.req.member, n)` uses the member that was committed (`mod.rs:479-489`).

**Unsigned nonce and commit.** The `JoinPending.nonce` is not covered by A's signature, and `Join.commit` is not covered by B's signature. Both are harmless:

- The relay may choose `nA` for B, but it must choose it before `nB` is revealed.
- If the relay replays B's static member+sig with its own commit, it can only get A to approve **B's real keys**, which it cannot use. The worst case is that it replaces B's pending entry, which is DoS only.

**Replay.**

- An old `JoinPending` gets a fresh `nB`, so a new random code.
- An old `Join` plus its old reveal only reproduces a real machine's record under a fresh `nA`.
- A second reveal is ignored (`mod.rs:482`) before the does-not-open branch, so the relay cannot kill an entry that already has a code by sending a bad reveal.

**A withheld or dropped reveal.** A never shows a code, so nothing can be approved. This is a safe failure. The UX gap is noted in the report.

**The 20-codes/10-min limiter as DoS.** Any same-account token holder can exhaust it: a removed machine within token life, or the relay. The relay can always DoS by dropping traffic anyway. The limiter is honestly recorded in spec 已知限制 #6. Acceptable for step 1.

**Constant time.** Not relevant:

- `opens()` compares a hash of attacker-supplied inputs against an attacker-supplied commitment; there is no secret.
- The code compare in `approve` uses locally typed input and has no remote timing oracle.

**Per-peer nonces.** Present (`group.rs:114-124`), with a test. Without them, revealing to one peer would let the relay grind the answer of a later-replying fake peer.

**RNG.** Production `rand` is `getrandom` (`mod.rs:230, 795-797`).

### NEW Important: the joiner shows an unbounded number of inviter codes

**Where.**

- `src/mesh/group.rs:111`: `let peers = net.peers().unwrap_or_default();`. The relay supplies this list, and nothing caps or deduplicates it.
- `group.rs:114-187`: one nonce, one thread and one ask per listed peer. Every valid reply becomes an `Invite` with a code (`mod.rs:512-520`, which has no cap).
- `cli.rs:311-318` prints them all.

**Scenario.**

1. The relay lets B's real Join and reveal reach A, so A's screen shows `cA`.
2. The relay reports failure to B's reveal `send` for the real A, while still delivering it. B then drops the real A from its list (`group.rs:190-193`).
3. The relay also lists N fake "inviters" in `peers`. Each has fresh keys and a self-signed `JoinPending`, and each is named like the user's real machine ("家里Mac"). Each fake is an independent 10⁻⁶ chance that `code(fake_i, B, nAi, nBi) == cA`.
4. If one matches, the user picks it on B. Picking is by name or endpoint (`cli.rs:194-205`), and the prompt prints the endpoint when names collide. B then accepts the relay's roster (`accept_invite`). The relay's machines become B's group and can type marked messages into B's agents.

The inviter-side `MAX_CODES_PER_TTL` never comes into play. Success is about N·10⁻⁶ per `dct join`, and only the user's patience with a long list limits N. A very large N also means one OS thread per fake peer (`group.rs:127-140`), which is a resource DoS on B's daemon.

In practice N in the tens is plausible on screen (≈10⁻⁵ to 10⁻⁴). N in the thousands is noticeable. That is why this is Important and not Critical. It still contradicts the spec's stated bound: spec line 70, "每冒充一次也只有一百万分之一…最多亮 20 个数字", and 已知限制 #6.

**Fix (small).**

- Deduplicate `peers` and cap the number asked, for example at `MAX_PENDING` (16) or smaller. A real group this size never has more machines online.
- If more than a handful reply, show a "too many computers answered, something is wrong" message instead of a code list.
- Add a test: a `Net` that lists 100 self-consistent fake peers yields at most the cap of invites.
- Spec: state that the per-join guess budget is bounded on **both** screens.

---

## I1: ADDRESSED, one Minor gap

**What is covered.**

- `roster::valid_name` rejects Cc (`char::is_control`: C0, DEL, C1 incl. U+0085) and the full Unicode 15 Cf table, including bidi U+202A-E/U+2066-9, zero-width U+200B-F/U+2060-4, U+FEFF and U+00AD (`crates/dct-mesh/src/roster.rs:171-211`).
- Entry points:
  - `validate_members` covers `accept` and `accept_invite`, which is every roster from a remote signer (`roster.rs:223-232, 290, 355`).
  - `take_join` (`mod.rs:433`).
  - The joiner's `JoinPending` check (new, `group.rs:156`).
  - `rename` (`mod.rs:360`), `store::set_name` and `tidy_name`.
- Second line of defence: `clean_name` runs in `marker()` (`deliver.rs:53`), in `group::view` and `joining`, in every CLI print via `c()`, including `PeerView` os/sessions/dir/tentacles (`cli.rs:494-512`), and in the TUI.

**Minor (new).** U+2028 LINE SEPARATOR and U+2029 PARAGRAPH SEPARATOR are Zl/Zp, not Cc or Cf, so they pass `valid_name` and `clean_name`. Reviewer asked explicitly for line/paragraph separators.

- **Impact.** Terminals render them harmlessly. However, JS-based agent CLIs and the model itself can treat them as line breaks, so an approved machine's name could visually split the marker line. The name is 32 chars at most and only an **approved** member can place one, so this is Minor.
- **Fix.** Add `'\u{2028}' | '\u{2029}'` to the reject set. Optionally also reject invisible fillers: U+115F, U+1160, U+3164, U+FFA0, U+2800. Add a test case.

Pre-existing and noted by the implementer: a hand-edited local name file (`store::name()`) is not validated. That is local only. Rosters loaded from disk are not re-validated either, but every display and marker path cleans them.

## I2: ADDRESSED

- `dispatch` does a `Hello` handshake before any subcommand (`cli.rs:108-110, 139-149`). On a mismatch it prints `StaleDaemonExplain` plus 「要重启，运行：dct restart」 and exits 1.
- There is **no auto-restart**: the message is only printed, with no y/n prompt and no `spawn`.
- `connect_or_start` spawns a daemon only when none is listening; it never replaces a running old one.
- A test covers all 6 command shapes with protocol 22.

**Minor wording note.** A daemon that is connected but fails to answer `Hello` within `READ_TIMEOUT` (5 s) is also told it is "the old version". That is a wrong diagnosis in the rare hung-daemon case. Consider mapping `Err`/timeout to `DaemonNotResponding` instead.

## I5: ADDRESSED; the arithmetic checks out

- `LinkNet` has its own agent with an 8 s overall timeout (`net.rs:26-55`).
- `ask_within` overrides it per request with `req.timeout(t)` (`link.rs:199-202`). ureq 2's request timeout replaces the agent's, so asks get their own bound and not 8 s.

Worst cases against the handlers (`daemon.rs:333-370`):

| Request | What it does | Worst case |
|---|---|---|
| Status | peers | 8 s |
| Login | gateway 15 + view 8 | 23 s |
| Join | peers 8 + parallel asks 10 + parallel reveals 8 + view 8 | 34 s |
| Approve / Remove | parallel broadcast 8 + view 8 | 16 s |
| Peers | 8 + parallel status asks 3 | 11 s |
| Send (remote) | 2 × 10 | 20 s |
| ConfirmInviter | no network (`group::confirm` takes no `net`) | 8 s budget is generous |

The CLI waits 60 s for every `Mesh*` request, which is at least 26 s of margin. A test pins it. The local-session send is unbounded, and that is documented as 已知限制 #20.

## M1: ADDRESSED; no regression

`Renewer` (`login.rs:175-220`) sets two intervals:

- 10 min after any real attempt;
- 1 h after a failure.

`NotDue` does not count as an attempt. With a short-TTL or past-`exp` gateway, renewal now happens at most every 10 min instead of every poll.

**The scenario you asked about** ("a failed first-ever renewal leaves no usable token for 1 h when it previously would retry"): this is **not a regression**. The old code (`RELAY_RENEW_RETRY` = 1 h on `last_failure`) had exactly the same 1 h hold after any failure, including the first. `Renewer::default()` has `last = None`, so the first attempt is immediate.

**Pre-existing Minor, worth a follow-up.**

- **Scenario.** The token is already **expired**, for example because the laptop slept past `exp`. The first renewal then fails transiently, for example because Wi-Fi is not up yet on wake.
- **Result.** The machine stays off the relay for a full hour. `Instant` on macOS does not advance during sleep, so it is an hour of awake time.
- **Suggestion.** When `exp <= now`, use a short retry (1-5 min) instead of `RENEW_RETRY_AFTER_FAILURE`.

## I4: ADDRESSED

- The spec now has 「已知限制」 with 20 items, covering roster forks, removed-machine rosters, token not revoked (#2), SAS DoS (#6), LF (#12) and local send (#20).
- The removal paragraph says revocation is not done (spec line 105).
- The old v1 text and KAT are gone, and dct-sas-v2 is defined byte for byte.

**Minor nit.** Spec line 86, 「加入方在拿到 n邀请 之前就收到了 JoinPending」, reads oddly. `nA` arrives *in* the `JoinPending`. Suggested: 「加入方收到 JoinPending（里面有 n邀请）之后先算数字、记下邀请，再揭晓 n加入」.

---

## Summary table

| Finding | Status | Note |
|---|---|---|
| C1 | ADDRESSED (core) + **NEW Important** | Joiner-side invite count uncapped (`group.rs:111`). Relay gets N·10⁻⁶ per join and one thread per fake peer. |
| I1 | ADDRESSED + NEW Minor | U+2028/U+2029 (and invisible fillers) pass `valid_name`/`clean_name`. |
| I2 | ADDRESSED + NEW Minor | A hung daemon on `Hello` is misreported as "old version". |
| I5 | ADDRESSED | Arithmetic verified; ureq per-request timeout overrides the agent timeout. |
| M1 | ADDRESSED | No regression. Pre-existing Minor: 1 h hold even when the token is already expired. |
| I4 | ADDRESSED + Minor nit | Spec line 86 wording. |

**New Critical: 0. New Important: 1. New Minor: 4** (U+2028/9, Hello misdiagnosis, expired-token retry, spec wording).
