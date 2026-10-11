# Task B report: campaign text approval requires Touch ID

## Design
- `campaign::approve(dir, name, per, ask, signer: &dyn dct_brain::sign::Signer, now)`; `campaign::load(dir, name, trusted: &[TrustedKey])`. Signer and verifier keys are injected, so tests use dct-brain's SoftSigner (root dev-dep already has `soft-signer`). No new signing mechanism: real path is `KeyStore::signer(SignerRole::User, platform_enclave())` (same as `procedures::approve`) and `KeyStore::trusted()`.
- Order: validation, distinct check, user types yes, then signer (Touch ID), then write. No yes or cancelled Touch ID: nothing stored. Non-user (auto) key refused.
- File = campaign JSON + `signature` (flattened `Stored`; `Campaign` struct unchanged).

## What is signed / verified
- Bytes: dct-brain length-prefixed fields (`canon::field`): "dct-campaign-v1", name, platform count, then per platform (sorted) name, param count, each param name and value. `approved_at` is not signed (does not change what is posted). Version prefix gives domain separation from tickets and approval records.
- Load refuses: no signature (legacy), file name != stored name (copied file), signature not from a trusted key, signer role not User, content changed (name / value / added or removed platform or param). Existing hidden-character checks kept. All messages tell the user to re-approve.
- `dct do` (src/explore_cli/real.rs): loads trusted keys then campaign before connecting to dco; failure is exit 2 (a JSON error event with --json).

## RED / GREEN
- RED: `cargo test --lib campaign` failed to compile (29 errors: new signature of approve/load not yet implemented).
- GREEN: `cargo test --lib campaign`: 26 passed. `cargo test --test campaign_cli`: 4 passed (tampered, tampered --json, unsigned legacy: exit 2 and no ~/.dco touched; untouched signed passes the gate then fails at missing dco, exit 1).

## Full check
- `cargo test --workspace -- --test-threads=1`: 2405 passed, 0 failed, 5 ignored (2391 before + 14 new). No flaky failures seen.
- clippy --all-targets: warning-free. rustfmt --check on src/campaign.rs, tests/campaign_cli.rs, and my hunk in real.rs: clean.
- Windows (`cargo check --workspace --target x86_64-pc-windows-msvc`): only the pre-existing src/student_projects.rs:669 warning. On non-Mac, `NoEnclave` means keys cannot exist, so approve fails at `keys.signer` (same as procedures::approve); `dct do` on non-unix exits 2 before this anyway.

## Files
src/campaign.rs, src/explore_cli/real.rs, tests/campaign_cli.rs, README.md, README.zh-CN.md (the "not done yet" note about campaign Touch ID removed).

## Residual risks
- Anyone who can write ~/.dct/keys/public.json can swap the trusted key (same trust base as procedure approvals).
- Replay: an old signed version of a campaign file can be restored and still verifies (no revocation/version counter; approved_at not signed).
- Existing campaign files stop working until re-approved (intended).
- Real Touch ID path not exercised in tests (by instruction).
