# Final review fixes — report

Status: all seven ruling items implemented and verified.

Commits (feat/dct-brain-step1, on top of 3827849):
- `11fd836` fix(brain): never let negation suppress a money clause
- `700748c` feat(brain): verify a procedure ticket bound to its approval
- `24c6975` fix(procedures): namespace approval files by procedure, name tiers accurately, show human durations
- `2f885ff` fix(keys): correct the non-Mac comment and note macOS 11 minimum
- `5beeac4` fix(build): target macOS 11.0 for the Swift static library

Test summary: `cargo test --workspace` — all green (1397 in `dct`, 48 in `dct-brain`, rest of the workspace crates all pass, 0 failed); `cargo clippy --workspace --all-targets -- -D warnings` — clean; `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc` — clean (one pre-existing unrelated warning in `src/student_projects.rs::sync_dir`, not touched by this work); `cargo build --workspace` on macOS (real Swift compile at the new -macosx11.0 target) — succeeds.

What changed, mapped to rulings:

1. `crates/dct-brain/src/tier.rs`: added `effectively_negated`/`is_money_clause` so a clause containing a money word can never be hidden by negation; narrowed `NEGATION_ZH` from a bare `不` to an explicit list (不要/不用/不保存/不发/不了/不再/不需要/不同意/不允许/取消/暂不/以后再说/稍后/放弃/别). New tests cover 「不限量购买」→Money, "No-fee checkout"→Money, 「不要付款」→Money, plus existing negation/multi-clause tests still pass.
2. `crates/dct-brain/src/approval.rs`: added `verify_procedure_ticket(st, sa, trusted, device, now)`, which chains `sign::verify` (ticket signature/device/time/role), `verify_approval` (procedure name + steps_sha256 match), and a new `VerifyError::TierBelowApproval` check (`ticket.tier >= approval.run_tier()`). No canonical form or golden value touched. Tests: valid pair passes; self-tier ticket vs content approval fails with `TierBelowApproval`; wrong procedure/wrong steps both fail; tampered approval fails with `BadSignature`.
3. `src/procedures.rs` `Approvals::path`: file name is now `sha256(field(full_name) ‖ field(steps_sha256))` using `dct_brain::canon::field`/`sha256_id` (length-prefixed, unambiguous). `save`/`load` now take the procedure name. New test `two_procedures_with_identical_steps_keep_independent_approval_files` proves two procedures sharing a steps_sha256 get independent files.
4. `src/procedures.rs` ticket reason: `strict` now labels each qualifying step with its real tier word via `tier_zh` (e.g. "第4步动钱") instead of appending a blanket "对外" suffix to every step regardless of tier. Test `the_ticket_prompt_describes_money_steps_as_paying_money_not_outward` asserts the prompt says "第4步动钱" and never says "对外" when the step is money-tier.
5. `src/procedures.rs`: added `human_duration(secs) -> String` (秒 under a minute, then 分钟[+秒], 小时[+分钟], 天[+小时], two-level max) and used it for both halves of the ticket validity-period sentence. Unit test `human_duration_uses_minutes_hours_and_days` plus an integration test asserting the prompt shows "1 分钟 30 秒" / "1 小时 30 分钟" instead of raw seconds.
6. `src/keys/mod.rs`: rewrote the `platform_enclave` comment — non-Mac's `NoEnclave` makes `create`/`sign` fail unconditionally, so `init` refuses outright and **no** ticket at any tier (not just content/money) can be signed there yet. Also added "需要 macOS 11 或更新" to the module doc and the `run_cli` doc comment (`dct keys` isn't otherwise mentioned in README).
7. `build.rs`: lowered the Swift static-library target from macos13.0 to macos11.0. That target makes swiftc auto-link the Swift 5.6/concurrency/dynamic-replacement compatibility shims, which Command Line Tools doesn't ship (undefined `__swift_FORCE_LOAD_$_swiftCompatibilityConcurrency` etc. at link time); fixed by passing `-disable-autolinking-runtime-compatibility[-concurrency|-dynamic-replacements]`, verified both a bare `swiftc` compile (arm64 and x86_64) and a full `cargo build --workspace` succeed cleanly. No `@available` guards were needed in `swift/Keys.swift` — CryptoKit's `SecureEnclave` API predates macOS 11, and swiftc raised no availability diagnostics at this target. Item (8) from the review (step_tiers length unchecked) was explicitly parked to the dco verifier per the ruling and not touched here.

Concerns / residual risk:
- `verify_procedure_ticket` was placed in `approval.rs` rather than `ticket.rs` (both were offered as options) since it also needs `sign::verify` and `verify_approval`, both already imported there; this keeps `ticket.rs` free of a dependency on `sign`/`approval`.
- The Swift compatibility-shim autolinking fix is CLT-toolchain-specific behavior; if someone builds this with full Xcode instead of just Command Line Tools, the flags are harmless (they just suppress autolinking of libraries Keys.swift doesn't use), so it should be safe either way, but only CLT was available to test against here.
- Real Touch ID / real secure-enclave signing was not exercised, per instructions — all keys/procedures tests use the existing `FakeEnclave`/`SoftSigner` test doubles.
- `devices/esp32-screen/` was left untouched and unstaged throughout, as instructed.
