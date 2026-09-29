# Task 4 report: gateway contract for relay tokens + dct-side exchange

## Status

Done. Contract appendix written into the design spec; `mesh::login::fetch_token` /
`needs_renewal` / `RELAY_TOKEN_KEY` / `RELAY_TOKEN_EXP_KEY` implemented with an
injected fake transport, TDD-first. All checks green.

## Commit(s)

- `70e0531` — `feat(mesh): gateway contract for relay tokens and the dct-side exchange`
  - `src/mesh/mod.rs` (new): `pub mod login;`
  - `src/mesh/login.rs` (new): `fetch_token`, `needs_renewal`, `RELAY_TOKEN_KEY`,
    `RELAY_TOKEN_EXP_KEY`, `Transport` type alias, 13 unit tests
  - `src/lib.rs`: added `pub mod mesh;`
  - `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`: appended
    "附录：网关签中转令牌（冻结的线上契约）" at the end of the file

Not staged/committed (per instructions): `devices/esp32-screen/`, `.superpowers/`.

## Test summary

13/13 new `mesh::login` unit tests pass; full workspace suite 1410/1410 (dct lib)
+ all other crates pass; clippy `-D warnings` clean; `cargo check` for
`x86_64-pc-windows-msvc` compiles clean (one pre-existing unrelated warning in
`student_projects.rs`). One integration test (`zombie_reaping.rs`) failed once
under full-suite parallel load and passed in isolation — pre-existing flake,
unrelated to this change (not touched by this task).

## Tests / commands / output

### rustfmt

```
$ ~/.cargo/bin/rustfmt --check --edition 2021 src/mesh/mod.rs src/mesh/login.rs
(no output, exit 0, after one auto-fix pass)
```

### cargo test --workspace

```
$ ~/.cargo/bin/cargo test --workspace 2>&1 > /tmp/full-test-output.txt; echo "exit=$?"
exit=0
```

Relevant excerpt (`grep mesh::login`):

```
test mesh::login::tests::a_200_returns_the_token_and_its_expiry ... ok
test mesh::login::tests::a_400_means_the_endpoint_was_rejected ... ok
test mesh::login::tests::a_401_means_the_login_has_gone_stale ... ok
test mesh::login::tests::a_404_means_the_gateway_has_multi_machine_switched_off ... ok
test mesh::login::tests::a_garbage_200_body_does_not_panic_and_is_reported_in_chinese ... ok
test mesh::login::tests::a_network_failure_is_reported_in_chinese_too ... ok
test mesh::login::tests::an_unexpected_status_code_is_reported_in_chinese_not_as_a_raw_number ... ok
test mesh::login::tests::needs_renewal_is_false_with_exactly_one_day_left ... ok
test mesh::login::tests::needs_renewal_is_false_with_plenty_of_time_left ... ok
test mesh::login::tests::needs_renewal_is_true_once_already_expired ... ok
test mesh::login::tests::needs_renewal_is_true_one_second_short_of_a_day ... ok
test mesh::login::tests::the_api_key_only_appears_in_the_bearer_slot_never_in_the_body ... ok
test mesh::login::tests::the_url_is_built_from_the_origin_with_no_double_slash ... ok
```

`dct` lib test result: `1410 passed; 0 failed`. One unrelated failure surfaced
in a separate integration binary:

```
running 1 test
test an_agent_that_exits_on_its_own_does_not_become_a_zombie ... FAILED
---- an_agent_that_exits_on_its_own_does_not_become_a_zombie stdout ----
thread ... panicked at tests/zombie_reaping.rs:111:9: shell 一直没起来
```

Re-ran in isolation to check for pre-existing flakiness (this task never touches
`pty.rs`/`session.rs`/zombie reaping):

```
$ ~/.cargo/bin/cargo test --test zombie_reaping 2>&1 | tail -10
running 1 test
test an_agent_that_exits_on_its_own_does_not_become_a_zombie ... ok
test result: ok. 1 passed; 0 failed
```

Confirms a pre-existing timing flake under full-suite parallel load, not caused
by this change.

### clippy

```
$ ~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -100
    Checking dct v0.2.18 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.80s
```

(First run flagged `clippy::type_complexity` on the raw `&dyn Fn(&str, &str, &str)
-> Result<(u16, String), String>` parameter; fixed by adding a `pub type
Transport<'a> = &'a dyn Fn(...) -> ...;` alias — same shape, just named. Clean
after that.)

### Windows cross-check

```
$ ~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc 2>&1 | tail -100
    Checking dct v0.2.18 (...)
warning: unused variable: `path` --> src/student_projects.rs:669:13   (pre-existing, unrelated)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.35s
```

### git diff --check

```
$ git diff --check
(no output, exit 0)
```

## Concerns

1. **`RELAY_TOKEN_EXP_KEY` naming is my addition.** The brief's Interfaces
   section only lists `RELAY_TOKEN_KEY` explicitly; the adjacent
   `"__relay_exp__"` key is mentioned only in prose. I added a matching
   `pub const RELAY_TOKEN_EXP_KEY: &str = "__relay_exp__"` for symmetry (Task 6
   will need something to store expiry under). Worth a quick nod from whoever
   picks up Task 6 that this is the intended name.
2. **Error strings for 400 / network-failure / unexpected-status are my own
   wording**, not verbatim from the brief — it only gave 401 and 404 "for
   example" (`例如`). I kept them in the same terse, non-technical register as
   existing dct Chinese error copy (e.g. `daemon.rs`'s "连不上 Telegram，检查
   一下网络，然后重试" pattern). Flagging in case the controller wants to align
   wording with what dc_llm ends up actually returning for 500s etc.
3. **Real HTTP transport (`ureq`-based `send`) is intentionally not built in
   this task** — brief's file list only asked for `mesh/mod.rs` + `mesh/login.rs`
   + `lib.rs`, and Task 5/6 briefs show the daemon-side wiring (including the
   real transport and where the fetched token gets persisted into
   `secrets.toml`) lands later. `fetch_token` is pure/injectable only, per the
   `pair_http.rs` precedent but without its own `_http.rs` counterpart yet.
4. Ready for the controller to send the appendix contract to the dc_llm
   session — **pending user consent**, per the brief's closing instruction.

## Fix round 1

Coordinator review of `70e0531` passed the spec check and found no code
problems, but flagged one Critical gap: the appendix pointed at
`relay_token::bytes` instead of being self-contained, and a separate (e.g.
Python) gateway implementation needs to be buildable from the appendix text
alone.

**Changes:**

- `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`: expanded
  the appendix into five numbered subsections:
  1. Canonical signed bytes — the exact `<decimal length>:<bytes>` encoding,
     field order, and a worked example (claims → the literal 67-byte string).
  2. Signature — P-256 ECDSA over SHA-256, 64-byte `r||s`, standard base64
     with padding; explicit note that Python's `cryptography` library returns
     DER and must be converted via `decode_dss_signature` + 32-byte
     big-endian `r`/`s`; a note on ECDSA malleability and what that implies
     for comparing signatures across implementations.
  3. Token string — base64url without padding of the JSON `{account,
     endpoint, exp, sig}`; states field order doesn't matter since only the
     canonical bytes are verified, not the JSON bytes.
  4. Public key format the relay trusts — SEC1 uncompressed, 65 bytes,
     `0x04` prefix, standard base64, one per `--relay-keys` line.
  5. A known-answer vector: fixed test-only 32-byte issuer key (32 × `0x01`,
     marked never-for-production), fixed claims, the exact canonical bytes,
     the issuer's SEC1/base64 public key, and an example token, plus the
     exact check a Python implementation should perform
     (`relay_token::verify` must return `Ok(claims)`), with a note that
     comparing raw signature/token bytes across implementations is the wrong
     check (ECDSA is randomized/malleable).
- `crates/dct-mesh/src/relay_token.rs`: added
  `relay_token::tests::the_appendix_known_answer_vector_is_accepted`, which
  hardcodes the appendix's exact canonical bytes, public key (base64), and
  token string, and asserts `verify(token, &[issuer_pub], 0)` returns the
  expected `Claims` — so the doc and the code cannot silently drift apart.

**How the vector was generated:** a throwaway `#[cfg(test)]` module was
temporarily added to `relay_token.rs` to print the canonical bytes, the
issuer's public key (base64), and a signed token for the fixed test-only key
and claims; the printed values were copied verbatim into the appendix and
into the permanent KAT test, then the throwaway module was deleted.

### Tests / commands / output (fix round 1)

```
$ ~/.cargo/bin/cargo test -p dct-mesh relay_token -- --nocapture
running 11 tests
test relay_token::tests::the_appendix_known_answer_vector_is_accepted ... ok
... (11 passed; 0 failed)

$ ~/.cargo/bin/cargo test --workspace > /tmp/full-test-2.txt 2>&1; echo exit=$?
exit=0
(all crates: 0 failed, including the zombie_reaping test that flaked once
under load in the original round and is clean here)

$ ~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 18.13s
(no warnings)

$ ~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
warning: unused variable: `path` --> src/student_projects.rs:669:13   (pre-existing, unrelated)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 16.80s

$ ~/.cargo/bin/rustfmt --check --edition 2021 crates/dct-mesh/src/relay_token.rs src/mesh/login.rs src/mesh/mod.rs
(no output, exit 0)

$ git diff --check
(no output, exit 0)
```

### Commit

`453a1ea` — `docs(mesh): make the relay-token appendix self-contained with a KAT`
(`crates/dct-mesh/src/relay_token.rs`, `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`).
`.superpowers/` and `devices/esp32-screen/` left unstaged, as before.
