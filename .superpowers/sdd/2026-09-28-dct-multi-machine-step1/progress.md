# SDD ledger — plan: docs/superpowers/plans/2026-09-28-dct-multi-machine-step1.md

Spec: docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md
Branch: feat/dct-multi-machine-step1 (from docs/dct-multi-machine 984607f)

## Preflight scan
| Pair / task | Produces → consumes | Finding |
|---|---|---|
| T1→T2 | canon::field, MachineKeys, keys::verify, Member, Roster | consistent |
| T2→T3 | relay_token::{issue,verify,Claims} | consistent; T3 adds dct-mesh dep to dct-srv |
| T2→T6 | wire::JoinPending{code_hint} | **gap**: new machine only gets endpoint ids from /link/peers, has no member pubkeys → cannot compute the 6-digit code |
| T1→T6 | roster::accept(None, incoming) requires version 1 single member | **conflict**: joining machine must accept a multi-member roster with no current roster |
| T4→T6 | login::fetch_token, RELAY_TOKEN_KEY | consistent |
| T5→T6/T7 | "Net trait" used by T6/T7 | **gap**: no task defines it |
| T5→T7 | on_envelope queues Msg + returns Receipt; T7 delivers | consistent |
| T6/T7→T8 | MeshView, PeerView, SendOutcome | consistent |
| T7 | marker format vs Global Constraints | identical |
| T9 | two daemons with separate homes | unknown whether dct has a home override env |
| Each task self-consistency | T1–T9 tests vs code | no contradictions found beyond the rows above |

Ruling: wire::JoinPending becomes `JoinPending { member: Member, sig: String }` (self-signed dct-join-v1 over the member's fields); the joining machine computes the 6-digit code from each JoinPending — T2 defines it, T6 uses it — cost if wrong: one extra round trip
Ruling: T6 adds `roster::accept_invite(incoming: &SignedRoster, me_endpoint: &str, inviter: &Member) -> Result<(), RosterError>` in dct-mesh: signer == inviter.endpoint, signature valid under inviter's key (the one the user compared), roster contains me and the inviter, no duplicates — the SAS compare is the trust root for a joining machine — cost if wrong: one more function
Ruling: T5 defines `mesh::Net` trait { peers() -> Result<Vec<String>, LinkError>; send(to, payload) -> Result<(), LinkError>; ask(to, payload, timeout) -> Result<Vec<u8>, LinkError> } with a LinkNet impl and a FakeNet (in-memory hub connecting several Mesh instances) for tests — T6/T7 tests use FakeNet — cost if wrong: trait shape tweak later
Ruling: T9 uses whatever home/socket override dct already has (grep socket_path / DCT_ env); if none exists, T9 adds `DCT_HOME` override for tests only — cost if wrong: small env var addition
Task 1: dispatched (BASE 984607f, implementer sonnet)
Task 1: implementer agent a576765caa2a72522
Task 1: implemented 7fb7cd4 (concerns: signer=endpoint, added SignerRemoved, from_slice fix); review dispatched (opus)
Task 1: review — spec ✅, Needs fixes; Important: (I1) accept() doesn't bind endpoint to endpoint_for(sign_pub); (I2) 5 surviving mutants (dup name, bad keys, genesis version/len/signer). Minor: M1 accept(None) trusts any genesis (doc); M2 names unvalidated; M3 version u64::MAX; M4 SignerRemoved
Ruling: SignerRemoved kept — a machine leaves via another online member (T6 already says 不能移除自己) — cost if wrong: last machine can't leave (it can just delete ~/.dct/mesh)
Ruling: fold M1 (doc) and M2 (name non-empty, no '/', ≤32 chars, validated in accept) into fix round 1; M3 parked (members trusted)
Task 1: minor (deferred): M3 version can be set to u64::MAX by a member, freezing the group
Task 1: fix round 1/5 dispatched (resume a576765caa2a72522)
Task 1: fix round 1/5 (I1,M1,M2 addressed; I2 partial — off-curve test masked by endpoint binding, decode_sig length untested; commits 7fb7cd4..f21a74d); round 2 dispatched
Task 1: fix round 2/5 (2 addressed, 0 open; commits f21a74d..2c888d2)
Task 1: complete (commits 984607f..2c888d2, review clean)
Task 2: dispatched (BASE 2c888d2, implementer sonnet; carries JoinPending ruling)
Task 2: implementer agent a07ca2e720882eae1
Task 2: implemented 72ffbdf (added MachineKeys::diffie_hellman; binding failure → UnknownSender); review dispatched (opus)
Task 2: review — spec ✅, Needs fixes; Important: (1) no forged-signature test; (2) binding test can't fail; (3) relay-token bytes not pinned (account/exp tamper, KAT); (4) eph_seed reuse → same key + zero nonce; (5) replay undocumented. Minor 6–13
Ruling: (4) derive the ephemeral secret from SHA-256("dct-seal-eph-v1" ‖ eph_seed ‖ plain) so a repeated seed with a different message still gets a fresh key; doc still requires fresh randomness — cost if wrong: none (deterministic per (seed,msg))
Ruling: fold cheap minors into round 1: 7 check Sealed.v == 1; 8 reject all-zero shared secret; 10 empty-issuer test; 11 fix module doc; 12 verify_member also checks endpoint == endpoint_for(sign_pub); 13 known-answer vectors for seal (fixed seeds) and relay token and join signature bytes — cost: a few tests
Ruling: (9) high-S malleability parked — document that signatures/tokens must never be used as ids; replay keys off (from, id) — same stance as dct-brain
Task 2: minor (deferred): 9 high-S signatures accepted (doc only); 10 token has no iat/max-lifetime/audience (bearer)
Task 2: fix round 1/5 dispatched (resume a07ca2e720882eae1)
Task 2: fix round 1/5 (all addressed, mutation-verified; commits 72ffbdf..e4d75b1); note: report's Fix round 1 section empty — diff is the record
Task 2: complete (commits 2c888d2..e4d75b1, review clean)
Task 3: dispatched (BASE e4d75b1, implementer sonnet)
Task 3: implementer agent a5bf911d872d83b0d
Task 3: implemented 16bcd63; review dispatched (opus)
Task 3: review — spec ❌; Critical: (1) /link/ask reply injectable cross-account (waiting slot matched by (to,seq) before account check); Important: (2) endpoint id takeover by another account's token (poll overwrites account); (3) Offline vs NotYours oracle. Minor: 4 peers self-exclusion untested/timing; 5 surviving mutants; 6 shallow guard; 7 expiry only at request start; 8 empty issuer file accepted; 9 no reload
Ruling: (1) a waiting slot records the asker's account AND the endpoint that was asked; a reply is accepted only if from == that endpoint and its account matches; anything else is treated as not matching (falls through to normal send rules) — cost: none
Ruling: (2) while a device is present (within presence_ttl), a poll with a token for a different account is refused NotYours and does not touch the device; after it expires the id may be re-registered — ids are hashes of keys so a squatter can't forge mesh messages — cost if wrong: brief DoS of a hijacked id, E2E still holds
Ruling: (3) a recipient in another account is reported Offline (same as absent) — cost: slightly less helpful error, only for cross-account attempts which are illegitimate
Ruling: fold minors 4, 5, 6 (guard: allow only dct_mesh::relay_token paths in dct-srv src), 8 (refuse issuer file with zero keys) into fix round 1; park 7 (expiry window ≤ presence_ttl) and 9 (restart to reload keys)
Task 3: minor (deferred): 7 token expiry enforced only at request start (window ≤ 3×poll timeout); 9 issuer keys load at startup only
Task 3: fix round 1/5 dispatched (resume a5bf911d872d83b0d)
Task 3: fix round 1/5 (7 addressed; new Important: cross-account ask cancellation — ask() inserts slot before from-check, overwrites, forget removes by key; guard misses `use dct_mesh::*`/rename; commits 16bcd63..70446a7)
Ruling: round 2 fixes: check from==caller before inserting; key slots by (account, endpoint, seq) or refuse to overwrite an existing slot; forget removes only its own slot (token/generation match); guard also rejects glob and renamed dct_mesh imports — cost: none
Task 3: minor (deferred): slot not cleaned on caller disconnect (only on timeout)
Task 3: fix round 2/5 dispatched
Task 3: fix round 2/5 (2 addressed, 0 open; commits 70446a7..192344e)
Task 3: minor (deferred): slot-id uniqueness untested (fetch_add→load mutant survives); guard can't see Cargo.toml renames
Task 3: minor (deferred): phone page uses a random phone: id with a fixed token → NotYours once issuers are on; phone-over-relay needs its own token flow before it is ever enabled (not live today)
Task 3: complete (commits e4d75b1..192344e, review clean after 2 fix rounds)
Task 4: dispatched (BASE 192344e, implementer sonnet)
Task 4: implementer agent a4d47b07dcda600a5
Task 4: implemented 70e0531; review dispatched (sonnet)
Task 4: review — spec ✅, Needs fixes; Critical: appendix references relay_token::bytes by name instead of spelling out the canonical bytes + tag; fix round 1 dispatched
Task 4: fix round 1/5 (addressed — independent Python impl from appendix alone issued a token dct accepts; commits 70e0531..453a1ea)
Task 4: complete (commits 192344e..453a1ea, review clean)
Task 4: outward step pending user approval — send appendix contract to dc-llm session
Task 5: dispatched (BASE 453a1ea, implementer opus — integration: link generalization + daemon wiring + Net/FakeNet)
Task 5: implementer agent a0c7f0b2af541c10a
Task 4: outward step done — user approved (发吧); contract sent to dc-llm-e4 (msg d4701bae), flag stays off in prod until user approves go-live
Task 5: implemented 852e7e4; deviations: out-of-group machine drops relay rosters (first roster only via T6 accept_invite); phone route unwired (None); link::LinkError wraps relay code + Unreachable/BadEndpoint; shared Token cell
Task 5: carry to T6: start_mesh only at daemon start → dct login must (re)start the link via a slot; carry to T7: retries need a fresh message id (dedup drops repeats silently); FakeNet is synchronous → never call Net while holding the Mesh lock
Task 5: review dispatched (opus)
Task 5: review — spec ✅, Approved-with-fixes; Important: (a) dedup in-memory only → replay after restart; (b) missing sign.key regenerates identity while roster.json exists. Minor: 1 key write order untested; 2 no cross-process lock on key creation; 3 journal/queue growth under replay; 4 second Journal on same file
Ruling: (a) reject messages with sent_at earlier than this process's start time (step 1 has no offline delivery, so nothing legitimate is lost); also evict the seen set by time (older than the window) not only by count — cost if wrong: a message sent in the second before a restart is refused and the sender sees a failure
Ruling: (b) load_or_create_keys refuses (clear Chinese error) when keys are missing/partial but roster.json exists — cost: user must remove ~/.dct/mesh to reset identity
Ruling: fold minors 1 and 2 (lock file or O_EXCL temp) into fix round 1; park 3 (T7 caps queues at 50) and 4
Task 5: minor (deferred): 3 journal lines per dropped envelope unbounded; 4 start_mesh opens a second Journal on the same file
Task 5: fix round 1/5 dispatched (resume a0c7f0b2af541c10a)
Note: dc-llm gateway side done at dc_llm d05aff1 (dev, not pushed/deployed, prod key not generated); appendix updated 5daf599 (account opaque ULID, flag DC_ADMIN_RELAY_TOKENS_ENABLED); tenant-shared keys → 401 confirmed
Task 5: fix round 1/5 (4 addressed, 0 open; commits 5daf599..9d3563f)
Task 5: complete (commits 453a1ea..9d3563f, review clean)
Task 6: dispatched (BASE 9d3563f, implementer opus; carries: login must (re)start link via a slot; never call Net while holding Mesh lock; first roster only via accept_invite)
Task 6: implementer agent a3384a73c067282c1
Task 6: implementer hit rate limit mid-mutation-pass, work uncommitted; resuming. Note: docs/superpowers/specs/2026-09-27-dct-brain-design.md has uncommitted edits from another session (dco pause/continue_run notes) — not ours, leave unstaged
Incident: another session shares /Users/lei/work/dc/dc-terminal; it switched HEAD to main mid-task, so Task 5's commits (852e7e4, 5daf599, 9d3563f) landed on local main. Fixed: local main reset to origin/main 8239173 (commits kept on feat); shared checkout left on main.
Ruling: from Task 6 review on, all implementation runs in worktree /Users/lei/work/dc/dc-terminal-mesh (branch feat/dct-multi-machine-step1); the ledger stays at the main checkout's .superpowers path — cost: separate target dir, slower first build
Task 6: implemented 6d783ea (deviations: login creates 1-machine group, "not in a group" = group of one; MeshView.joining; flows in src/mesh/group.rs; ErrorCode::Mesh). Concerns: concurrent approvals fork the roster; offline members go stale; cross-joins between two new machines; pending joins lost on restart
Task 6: review dispatched (opus)
Task 6: review — spec ❌; Critical C1: joiner adopts a roster from any responder (relay can add a machine that answers JoinPending then pushes its own roster; no human compare on the joiner side); invites never expire. Important I1: flood evicts pending, approve-by-name lands on an impostor (approve doesn't carry the displayed code). Minor: M1 flood bounded; M2 start_mesh under slot lock (comment)
Ruling: C1 — joiner-side confirmation: `dct join` prints the codes and asks the user which machine shows the same number (interactive prompt; non-interactive `dct join --confirm <电脑名>`); only that confirmed inviter's roster may pass accept_invite; invites expire after JOIN_TTL — cost: one extra keystroke on the new machine
Ruling: I1 — MeshApprove carries the endpoint and the code that were displayed; approve requires both to match the pending entry; when pending_joins is full new joins are refused (not evicting); CLI `dct peers approve` re-shows name+code and asks y/n — cost: none
Ruling: document (not fix) in step 1: concurrent approvals can fork the roster; offline members go stale (and a removed machine could push to a member that missed the removal); cross-joins between two new machines; pending joins lost on restart — add a 「已知限制」 section to the spec at finish
Task 6: fix round 1/5 dispatched (resume a3384a73c067282c1; work in worktree /Users/lei/work/dc/dc-terminal-mesh)
Task 6: re-review (opus) — C1, I1, M2 ADDRESSED; no new Critical/Important. Minors: m1 held roster keyed by unverified `signer` field (junk roster from same-account sender clobbers A's genuine held roster → "approve A first, then confirm" silently times out; DoS only); m2 duplicate responder names → Ambiguous but `dct join` never prints endpoints (dead end); m3 JOIN_WAIT starts after confirmation while invite TTL starts at join; NoSuchInviter shows endpoint not name; m4 refuse-when-full can be held by refreshing senders (accepted trade-off)
Ruling: fold m1, m2, m3 into Task 7's dispatch as a separate first commit (small, same files mod.rs/cli.rs) rather than a fix round — cost: Task 7 diff is a bit larger; m4 accepted and documented
Task 6: complete (commits 9d3563f..41a120b, review clean, minors carried to Task 7)
Task 7: dispatched (BASE 41a120b, implementer opus a6b2a5c5336b62dba; carries: m1-m3 from Task 6 as a separate first commit; fresh msg id on retry; FakeNet synchronous — never call Net under Mesh lock)
Note (from dc-llm session, 2026-09-28): production relay-token signing key generated on the gateway (private key in gpu.tzspace.cn deploy/production/.env, mode 600; never printed). Public key for `dct-srv --relay-keys` (SEC1 uncompressed, 65 B, std base64):
  BNEgy8UH1Twciieycr/xRP4lYu/Ks9nRwCbLGLeoCRvcis56hMc42dLDKVgg6/iSdg2RT+az0KqAFwznrDb5Ze0=
Cross-language probe token (claims account="usr_01PROBE", endpoint="c-0011223344556677889a", exp=4102444800), to verify with dct-mesh relay_token::verify in Task 9:
  eyJhY2NvdW50IjoidXNyXzAxUFJPQkUiLCJlbmRwb2ludCI6ImMtMDAxMTIyMzM0NDU1NjY3Nzg4OWEiLCJleHAiOjQxMDI0NDQ4MDAsInNpZyI6IkZsclBJbGpkTlpPN1J3NGlSTFZHYTdkRzMwN2lmdDNkZmV6SW93ZERJM2QwZzZralBlanRXMWhtcTlrWG84VHBjKzRGckJzSE1KYm5KdmZzUG80VmZBPT0ifQ
Gateway flag DC_ADMIN_RELAY_TOKENS_ENABLED is still OFF; flip only after dct-srv relay is up AND the user agrees. Key rotation order: add new pubkey to the relay --relay-keys list first, wait until live, then change the gateway private key (gateway holds one key, relay holds a list).
Task 7: implemented b0b2195 (m1-m3 fixes) + 8a30be7 (peers/send/delivery). Deviations: agent sessions only (shell → Refused); label = board session_label; NoAnswer outcome; retry once on relay Busy only (fresh id). Concerns: typing under Mesh lock (git checkpoint); idle-flip may release queue early; delivery thread + local-socket routing untested; queue lost on restart; newline in marker vs real agents unverified; ~/.dco/endpoint.json format unagreed
Task 7: review dispatched (opus, a2a50b4649506a223)
Task 7: review — spec ❌ / Needs fixes. Important: I1 typing + git checkpoint under Mesh lock on link thread; I2 queued msgs from a removed machine still typed (no roster re-check); I3 remote os / NoSuchSession candidates printed uncleaned. Minor: M1 checkpoint Err after Enter reported as Refused; M2 fake second marker in body; M3 no queue TTL; M4 idle-flip; M5 endpoint.json no size cap; M6 DCT_SESSION_ID test not agent path. Unverifiable: LF in marker vs real agent CLIs
Ruling: fix I1-I3, M1, M2 in round 1; leave M3/M5/M6 (documented); LF/newline: use existing bracketed-paste helper only if one exists, else keep behaviour and record which agent profiles are affected — cost: possible early-submit on some agent CLIs, to be checked live in Task 9
Task 7: fix round 1/5 dispatched (resume a6b2a5c5336b62dba)
Task 7: fix round 1 committed 995e9b9 (I1-I3, M1, M2 fixed; no bracketed-paste helper exists so LF behaviour unchanged, all agent profiles get raw LF). Re-review dispatched (opus a09214d13232bbe1f)
Task 7: re-review (opus) — I1, I2, I3, M1, M2, newline handling ADDRESSED; no new Critical/Important. Minors: n1 in_flight stuck if type_into panics (no drop guard); n2 M2 fake-marker check bypassable by leading invisible format chars (U+200B/U+FEFF); n3 bracket-only name leaves empty marker slot (cosmetic). Focused run: mesh:: 137 passed
Ruling: fold n1 (drop guard) and n2 (trim leading whitespace + format chars before the `[来自` check, or pad every body line) into Task 8's dispatch as a separate first commit; n3 left — cost: slightly bigger Task 8 diff
Task 7: complete (commits 41a120b..995e9b9, review clean, minors n1/n2 carried to Task 8)
Task 8: dispatched (BASE 995e9b9, implementer opus a1e96e41476565974; carries: n1 drop guard + n2 invisible-char marker bypass as separate first commit; MeshApprove must carry endpoint+code; cross-join hint on the confirm line)
Task 8: implemented 145e50f (n1,n2) + b90f849 (TUI). New src/ui/computers.rs; MeshStatus/MeshApprove on bg threads; protocol 23 (MeshView.messages); panic catches in route() and delivery loop; concerns: grid mode shows nothing, n dual-use, msgs lost after tick panic, 2 paths untested. Review dispatched
Task 8: review — spec ❌ / Needs fixes. Important I1: grid-home users never see or answer a pending join (poll only in Board view). Minors: M1 no settle delay (refresh swap A→B before keypress); M2 no journal line on caught panic; M3 typing_abandoned misleading log; M4 remote-name cleaning keeps Cf chars; M5 n2 trim misses U+2800/3164/115F; M6 late approve answer overwrites message in session view; M7 two paths untested; M8 fake-daemon tests use sleeps
Ruling: I1 — poll in grid too and show a one-line notice pointing back to the board; y/n stays board-only (key conflicts in grid) — cost: grid users need one extra key press. Fold M1, M2, M4, M6 into round 1; leave M3, M5, M7, M8
Task 8: fix round 1/5 dispatched (resume a1e96e41476565974)
Task 8: fix round 1 committed 0baa295 (I1 grid notice, M1 settle 500ms, M2 mesh_panic journal, M4 Cf clean, M6). Re-review dispatched (sonnet-tier not used; opus)
Task 8: re-review — I1, M1, M2, M4, M6 ADDRESSED; NEW Important: grid notice shrinks area so 80x24 grid (exactly MIN_ROWS) turns into "窗口太小" whenever a join is pending. Out-of-scope minor: `g` in the notice while grid reply box is open types into the reply
Task 8: fix round 2/5 dispatched (resume a1e96e41476565974): overlay the notice like draw_reply; 80x24 test
Task 8: fix round 2 committed 35923b7 (overlay grid notice when slicing would hide tiles; hidden when reply box open). Re-review dispatched (sonnet)
Task 8: re-review round 2 — ADDRESSED, no new Critical/Important. Minors: test comments overstate 80x24 grid rows (default theme gives 22, slicing path); overlay hides top-row tile titles in tight cases (accepted trade-off). Task 8: complete (commits 995e9b9..35923b7)
Task 9: implemented (71c9242, b078195). e2e passes (5/5 consecutive), production probe token verifies under dct-mesh. Non-outward parts only; go-live NOT done.
Ruling: brief's `dct-srv serve --addr` form is wrong; README/test use `dct-srv <addr> --with-link --relay-keys <file>` — that is the real CLI — cost if wrong: none.
Ruling: README.md gets English section, README.zh-CN.md gets Chinese — README.md is English.
Ruling: task 9 gets folded into the whole-branch final review (test+docs only, no separate task review) — diff is tests and docs; cost if wrong: final review catches it.
Open for final review: token renewal hammers gateway when token lifetime < 1 day (prod is 7d); relay-log privacy check is weak; LF-submit in real agents unverified; spec lacks 已知限制 section; same-relay vs second-instance on dataclue.cn undecided (go-live).
Final review (opus): 1 Critical, 5 Important, 7 Minor; full test suite green. Report: final-review.md.
Ruling: fix wave covers C1 (commit-reveal SAS + name in code), I1 (name sanitising at roster/join + CLI output), I2 (version check on mesh CLI cmds), I4 (spec 已知限制), I5 (CLI timeouts vs daemon), M1 (renew min interval). I3 (LF vs real agents) stays open: documented in 已知限制 and go-live checklist, since verifying costs real prompts on the user's account — cost if wrong: first real run may show body as separate turn.
Fix wave: 5883bc0 M1, 238435a I2, 7257f8c I5, 4402cf2 I1, 2991d0e C1 (dct-sas-v2 commit-reveal, KAT 204106), 6d62005 I4. Tests green. Scoped re-review next (C1 is crypto; opus).
Fix-wave re-review: C1,I1,I2,I5,M1,I4 ADDRESSED; new: 1 Important (joiner shows unlimited codes -> N-in-a-million grind by relay-supplied peer list), 4 Minor. Details fix-wave-review.md.
Ruling: one more small fix dispatch beyond the "one fix" guideline — the new Important is a security bound the spec promises; fix = dedupe+cap peers asked (~16) and refuse to show codes when more answer; also U+2028/2029 in name filter and short retry when token already expired. Verified by tests + my own run, no third re-review — cost if wrong: cap logic bug in a small diff.
Merged to main (ad574a2 + earlier merge) and pushed, user OK'd ("1 要").
Go-live (user: "可以", second relay instance): dct-srv-mesh built from ad574a2 on dataclue.cn (/usr/local/bin/dct-srv-mesh, systemd dct-srv-mesh, 127.0.0.1:8788 --with-link --relay-keys /etc/dct-srv/gateway.pub); Caddy dataclue.cn: handle_path /dct-relay/link/* -> 8788, other /dct-relay/* 404 (backup /opt/dc-terminal/deployment/before-dct-relay-20260929-073707). Verified publicly: prod probe token 200, forged 401, /phone 404, site root and live listing still 200. Existing dct-srv (8787) untouched.
Remaining: gateway flag DC_ADMIN_RELAY_TOKENS_ENABLED (dc-llm side, asked dc-llm-e4 for deploy state; NOT flipped), then two-real-computers acceptance + LF check (README step 4/5).
Gateway flag ON in prod (dc-llm-e4, its user approved): issuance 200 verified under prod pubkey (7d), negatives correct, regression OK, rollback = set flag false + up -d. Known trap: 3 personal accounts have keys with no created_by_user_id -> 401 -> "请重新配对" loop; dc-llm side to backfill. Remaining: user pairs DC account on this Mac, then real 2-computer acceptance (README step 4) + LF check (step 5).

## 2026-09-29 acceptance, this Mac
- DC account paired (phone 15313957725; dc-llm backfilled created_by_user_id on its key). `dc` key now in ~/.dct/secrets.toml.
- New dct 0.2.18 (main ad574a2) installed to ~/.local/bin/dct; old one saved as dct.0.2.17.bak. Daemon restart (kills running sessions) approved by user ("装").
- Next: dct login, join from a second computer, approve, send; then LF check in real Claude Code and Codex.
