# Task 2 report: dct-mesh signed/sealed messages, wire payloads, relay tokens

**Status:** done

**Commit:** 72ffbdf — `feat(mesh): signed and sealed messages, wire payloads, relay tokens`
(files: `crates/dct-mesh/src/seal.rs`, `crates/dct-mesh/src/wire.rs`,
`crates/dct-mesh/src/relay_token.rs` new; `crates/dct-mesh/src/lib.rs` and
`crates/dct-mesh/src/keys.rs` modified — the latter gained a
`MachineKeys::diffie_hellman` method, needed by `seal` but not called out in
the brief's file list, since Task 1 only exposed `kx_pub`, not a way to
actually run X25519 DH with the private key.)

**Test summary:** `cargo test -p dct-mesh` — 68 passed, 0 failed (up from 25
before this task). `cargo clippy --workspace --all-targets -- -D warnings`
clean. `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc`
clean (one pre-existing unrelated warning in `dct`'s `student_projects.rs`,
not touched by this task). `cargo fmt -p dct-mesh -- --check` clean.

**Concerns:**
- Implemented the Controller ruling as stated: `Payload::JoinPending {
  member: Member, sig: String }`, tag `join_pending`, and `wire::sign_member`
  / `wire::verify_member` shared between `JoinRequest` and `JoinPending`.
- `seal::open`'s `OpenError` has no dedicated variant for "roster entry fails
  endpoint binding" (the brief's `OpenError` list doesn't include one), so
  that case is folded into `UnknownSender` — a roster entry whose claimed
  `endpoint` doesn't match its own `sign_pub` is treated as if the sender
  weren't in the roster at all. Covered by its own test
  (`a_sender_whose_roster_entry_fails_endpoint_binding_is_refused`).
  Worth confirming this mapping is what later tasks (e.g. the relay/router)
  expect.

Fix round 1
