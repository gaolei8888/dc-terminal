# SDD ledger — plan: docs/superpowers/plans/2026-09-27-dct-brain-step1.md

Spec: docs/superpowers/specs/2026-09-27-dct-brain-design.md
Branch: feat/dct-brain-step1 (from docs/dct-brain-step1-plan @ 3825b05)

Ruling: restored .superpowers/sdd/.gitignore to the repo policy (track briefs/reports/ledgers, ignore *.diff) — ed0b078 had committed the script-rewritten "*" — if wrong, ledgers stop being tracked again, easy to revert

## Pre-flight scan

| Pair / task | Produces → consumes | Finding |
|---|---|---|
| T1 ↔ T3 | tier.rs: T1 Tier/SignerRole + tests module; T3 appends fns + tests into same module | consistent |
| T2 ↔ T4/T5/T6 | canon::{field, sha256_id} | consistent |
| T4 ↔ T5 | Ticket, TICKET_VERSION, digest/canonical_bytes | consistent |
| T5 ↔ T6 | make_signature (pub(crate)), verify_sig, VerifyError::StepsChanged (declared in T5) | consistent |
| T5 ↔ T7 | T7 adds make_signature_for_tests to sign.rs (soft-signer gated) | consistent; T7 commit includes sign.rs |
| T1 ↔ T7/T8 | dev-dep dct-brain features=["soft-signer"] → SoftSigner in dct tests | consistent |
| T7 ↔ T8 | KeyStore::{init, trusted, signer}, SecureEnclave, platform_enclave | consistent; T8 duplicates T7's test FakeEnclave |
| T1 | tests vs code | agrees |
| T2 | vectors verified independently (Python) | agrees |
| T3 | every test case traced through negated/hit/tier_for_step | agrees |
| T4 | canonical prefix "13:dct-ticket-v11:132:…" traced | agrees; goldens from independent Python |
| T5 | SoftSigner scalar valid; p256 API names may differ (plan allows docs.rs fixes, no new deps) | agrees |
| T6 | goldens from Python | agrees |
| T7 | NoEnclave unused on macOS → dead_code under clippy -D warnings locally | CONFLICT |
| T8 | approve signs before dcv.approve (test expects 0 calls on cancel) | agrees |

Ruling: T7 — gate `struct NoEnclave` and its impl with `#[cfg(not(target_os = "macos"))]` — otherwise clippy -D warnings fails on the Mac where the user builds — cost if wrong: none, it only compiles where used.
Ruling: T8's test-only FakeEnclave duplicates T7's — keep the duplication (two small test-local fakes in different modules) rather than add a shared test-util module the plan doesn't have — cost if wrong: a reviewer Minor.
Task 1: dispatched (BASE 3825b05, implementer haiku)
Note: sdd scripts rewrite .superpowers/sdd/.gitignore to '*'; restore from 03617cc before committing the ledger at the end.
Task 1: minor (deferred): clippy run not evidenced in report (reviewer ran it: clean)
Task 1: complete (commits 3825b05..0354544, review clean)
Task 2: dispatched (BASE 0354544, implementer haiku)
Task 2: ⚠️ items resolved — controller recomputed A/B/C hashes with an independent Python impl before writing the plan (match); tests pass per report
Task 2: complete (commits 0354544..1f959b7, review clean)
Task 3: dispatched (BASE 1f959b7, implementer haiku)
Task 3: review — Important: curly-apostrophe test case dropped (duplicate straight quote); adversarial: a leading negation suppresses a later outward word ("放弃草稿，直接发布", "No, post anyway" → self/Read), plan-mandated code
Ruling: judge negation per clause, not per label — split on ，,;；、。.!！?？ ; a clause starting with a negation contributes nothing, the label's tier is the max over clauses; tier_for_step returns SelfOnly for negation only when every clause is negated — spec §4 says button text must only raise and negations are not what they negate; a negated first clause must not hide an outward second clause — cost if wrong: labels like "Don't save and post" (one clause) still slip through; residual risk, still backed by intent approval
Task 3: fix round 1/5 (2 addressed, 0 open; commits 4c41fa1..87838d6)
Task 3: minor (deferred): "Not now, maybe later" test passes via tap default, not via the all-clauses-negated branch (informational)
Task 3: complete (commits 1f959b7..87838d6, review clean)
Task 4: dispatched (BASE 87838d6, implementer haiku)
Task 4: minor (deferred): canonical_json float formatting not canonicalized (1.0 vs 1) — fine for current integer/string args
Task 4: ⚠️ resolved — dco is Rust and will verify with dct-brain itself (one implementation), per design §1
Task 4: complete (commits 87838d6..da253a0, review clean)
Task 5: dispatched (BASE da253a0, implementer sonnet — p256 API may need adjusting)
Task 5: review — spec ✅, quality Approved; Important: (1) high-S malleability → doc must say replay is tracked by nonce/digest, never signature bytes; (2) same public key paired twice under different roles → first match wins, could lift auto to User
Ruling: fix both in dct-brain now — doc comment for (1); verify_sig refuses a key_id that matches more than one trusted entry (new VerifyError::AmbiguousKey) for (2) — cheap here, and pairing code doesn't exist yet to rely on — cost if wrong: one extra error variant
Task 5: ⚠️ resolved — required_signer mapping verified in Task 1 review; Task 6 approvals use their own "dct-approval-v1" prefix (plan); WebAuthn DER/authData adapter is a later slice (phone approval page); dco nonce tracking → tell dc-octo session at finish
Task 5: minor (deferred): unbounded batch length (DoS via dco only); no deny_unknown_fields (unsigned fields ignored)
Task 5: fix round 1/5 (2 addressed + 5 tests, 0 open; commits d755ed7..6eef79a)
Task 5: complete (commits da253a0..6eef79a, review clean)
Task 6: dispatched (BASE 6eef79a, implementer haiku)
Task 6: review — spec ✅, Approved; Important: verify_approval doesn't bind the procedure name, an approval verifies for any procedure with identical steps
Ruling: add `procedure: &str` to verify_approval (checked before the signature; mismatch → new VerifyError::WrongProcedure) — binding the name is cheap and the consumer (Task 8) doesn't exist yet — Task 8 must call verify_approval(&sa, &trusted, full_name, &steps_sha) — cost if wrong: an extra parameter
Task 6: fix round 1/5 (1 addressed, 0 open; commits 5cb1fa5..748c231)
Task 6: minor (deferred): verify_approval has no doc comment
Task 6: complete (commits 6eef79a..748c231, review clean)
Task 7: dispatched (BASE 748c231, implementer sonnet; carries preflight ruling NoEnclave cfg)
Task 7: step 6 blocked — dct keys init from the agent shell (a dct/daemon-spawned PTY) fails: CryptoKit OSStatus -25308 errSecInteractionNotAllowed; SE available. Risk to carry: the dct daemon may hit the same when auto-signing — verify when wiring daemon signing; user must run init/show/test from a normal Terminal
Task 7: review — spec ✅, quality Needs fixes; Important: (1) every sign error maps to 3=Cancelled (biometry-changed, lockout, -25308 all read as "没有通过指纹确认"); (2) create errors all → 5 and printed as "签名失败", -25308 undiagnosable. Minor: load failure always blamed on "别的电脑"; missing public.json + existing handles silently re-keys; public.json not atomic; no partial-failure test
Ruling: FFI gains an out-param `status: *mut i32` on create/sign/load; Swift returns 3 only for LAError userCancel/systemCancel/appCancel; any other error returns new code 6 with status = NSError code; Rust maps 6+(-25308) to a plain message ("请在已解锁的 Mac 上、从普通终端窗口运行"), other 6 to "安全芯片出错（代码 N）"; create errors say "创建钥匙失败" not "签名失败"; load failure message no longer asserts another computer; init refuses when a handle file exists but public.json is missing; public.json written via temp+rename; add a fake-enclave create-failure test — codes carry the real cause at small cost — cost if wrong: an extra FFI param
Ruling: local public.json is forgeable by same-account processes → trust anchor for content/money is the key dco pins at pairing, not ~/.dct/keys/public.json; document beside the auto-key gap in final report — cost if wrong: none now (no verifier trusts the local file yet)
Ruling: auto key kSecAttrAccessibleWhenUnlockedThisDeviceOnly kept for now; locked-screen daemon signing is a known risk alongside -25308, revisit when daemon signing is wired
Task 7: fix round 1/5 dispatched (fresh sonnet implementer; original agent id lost to compaction)
Task 7: fix round 1 implementer agent a44983ed2d5ec3904 (BASE 150b103)
Task 7: fix round 1/5 committed 06399b4 (fmt --check fails identically on HEAD, pre-existing)
Task 7: re-review — all 6 addressed; notes: chip_message (Apple -25308 constant) lives in dct-brain sign.rs; two unreachable status-0 paths; load message still mentions "不属于这台 Mac"
Ruling: keep chip_message in dct-brain — SignError::Display needs a message and the function is pure; one Apple constant is not platform I/O; "不属于这台 Mac" is hedged and true for SE blobs — cost if wrong: a later move of ~10 lines
Task 7: complete (commits 748c231..06399b4, review clean after fix round 1)
Task 8: dispatched (BASE 06399b4, implementer sonnet; carries verify_approval(sa,trusted,procedure,steps) + SignError::Chip + keys API as of 06399b4)
Task 8: implementer agent a76829b6a4ac0d9d2
Task 8: implemented 3e9b896 (fixed brief's 3-arg verify_approval call); review dispatched (opus)
Task 8: review — spec ✅, Needs fixes; Important: (1) Touch ID prompt hides lowered tiers, lowering a money step makes money auto-signed forever; (2) unchecked time arithmetic + unbounded window not shown in prompt. Minor: prompt says 对外 for money; old-approval replay; Shown::parse lax; CLI numbering/errors; test gaps
Ruling: a step proposed as money can never be lowered (refuse with plain message) — spec: money needs fingerprint every time; other lowering stays allowed under Touch ID but the prompt must name every overridden step "第N步「…」原为X，改为Y"; tier words in prompts accurate per tier — cost if wrong: user has to use a different flow to relax money, which they never should
Ruling: checked_add everywhere; start_in ≤ 30 days, window ≤ 24 h, plain Chinese errors beyond; the ticket Touch ID prompt states the validity period — cost if wrong: long-lived tickets need re-issue
Ruling: fix now — Shown::parse rejects n out of u32 range, missing arg, duplicate n; overrides naming a nonexistent step error; CLI numbers by n, plain Chinese errors, duplicate --param errors; approvals dir 0700 / files 0600 via existing private-file helper; add tests: start_in>0, changed-after-approval → no ticket, approval for A refused for B, nonexistent override, money-lowering refused
Ruling: park old-approval replay (newest-wins) — needs a monotonic store or dco-side pinning; same-account attacker already out of scope for this slice (see public.json ruling) — cost if wrong: a same-account agent can restore looser tiers until dco pins
Task 8: fix round 1/5 dispatched (resume a76829b6a4ac0d9d2)
Task 8: fix round 1/5 committed 3827849; re-review dispatched
Task 8: re-review — Approve; minor (deferred): cap boundary (== cap accepted) untested; CLI approve re-fetches dcv show after save, a failing second fetch reports error though approval is saved
Task 8: complete (commits 06399b4..3827849, review clean after fix round 1)
Final review: dispatched (opus, range 3825b05..3827849)
Final review: not ready; Important: (1) negation hides money (「不限量购买」, "No-fee checkout" → SelfOnly, escapes money guard and prompt); (2) procedure ticket not bound to approval, dco can't check ticket tier ≥ approval; (3) approvals keyed by steps_sha only, identical procedures overwrite each other. Minor: ticket prompt 对外 for money; seconds shown raw; keys comment misstates non-Mac; Swift built for macOS 13 hard-links CryptoKit; step_tiers length unchecked
Ruling: (1) a clause containing a money word is never suppressed by negation; 不 negation narrowed to explicit negation words (不要/不用/不保存/不发/不了/不再/不需要…), English negation matched per clause on whole-word tokens — cost if wrong: some negated money labels propose money (safe direction)
Ruling: (2) add dct-brain `verify_procedure_ticket(ticket, approval, trusted, now)` that verifies both signatures, requires subject procedure+steps_sha == approval's, and ticket.tier >= approval.run_tier(); no canonical/golden change — dco will call it with the approval shipped alongside; freezing dct-ticket-v1 stays safe — cost if wrong: dco must carry the approval with each ticket
Ruling: (3) approval file name = sha256(procedure name ‖ steps_sha) — cost: old files (none exist yet) orphaned
Ruling: minors 4/5/6 fix; (7) Swift built for macOS 11.0 with @available guards as needed, README/keys doc states minimum macOS 11 — x86_64 Macs older than 11 out of scope; (8) parked to dco verifier (it has the steps)
Final fix: dispatched (sonnet, BASE 3827849)
Final fix: committed 11fd836..5beeac4 (macOS 11 needed -disable-autolinking-runtime-compatibility* flags); re-review dispatched
Final re-review: ready to merge; minors fixed by controller: added 不分享/不上传/不转发/不提交/不确认/不完成 negations, money words 付费/结算/订阅/payment/subscribe (+tests); verify_procedure_ticket doc says dco must recompute steps_sha of what it runs; Keys.swift warns against async/Task/actor (shims disabled)
Parked (accepted): one-clause outward labels behind a leading negation ("Skip and publish", 「不要紧发布」) still SelfOnly — backed by intent approval; step_tiers length check → dco verifier; old-approval replay → dco pinning
Branch complete; user said 做完了合并吧
