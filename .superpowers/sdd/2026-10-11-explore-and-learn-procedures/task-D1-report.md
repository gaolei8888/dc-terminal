# Task D1 report
Implemented: new crate crates/dct-dco (DcoClient, DcoError, Reply, call_full, RUN_TIMEOUT, IO_TIMEOUT, pub no_show_status, fake::serve behind feature "fake"). dct-game dco.rs now only has `impl Dco for DcoClient`; play.rs re-exports dct_dco::DcoError; dco_tests.rs uses dct_dco::fake::serve (handlers got a third `None`).
TDD: RED `cargo test -p dct-dco` -> compile errors (unresolved `fake`, `DcoClient`). GREEN -> 2 passed (call_full_returns_the_picture_next_to_the_json, a_tool_error_keeps_dcos_code).
Full check: cargo test --workspace -- --test-threads=1: 2128 passed, 0 failed, 4 ignored (baseline 2126 + 2 new). clippy --all-targets: clean.
rustfmt: crates/dct-dco/src/lib.rs (+fake.rs) formatted and clean. dct-game dco.rs/dco_tests.rs/play.rs were already rustfmt-dirty at baseline (verified via git show HEAD), left unformatted.
Files: Cargo.toml, Cargo.lock, crates/dct-dco/{Cargo.toml,src/lib.rs,src/fake.rs}, crates/dct-game/{Cargo.toml,src/dco.rs,src/dco_tests.rs,src/play.rs}.
Concerns: initialize clientInfo.version now reports dct-dco's CARGO_PKG_VERSION (0.1.0) instead of dct's; crate is unix-only like before (ungated). Brief's `body_of(_tool, ..)` unused param dropped.

## Fix round 1
- Windows regression fixed: dct-dco lib.rs keeps DcoError, Reply, IO_TIMEOUT, RUN_TIMEOUT on all platforms; DcoClient moved to new src/client.rs behind `#[cfg(unix)]` (re-exported from lib.rs); `fake` is `#[cfg(all(unix, any(test, feature = "fake")))]`.
- Verified: `cargo check -p dct-game -p dct-dco --target x86_64-pc-windows-msvc` (target installed) -> Finished OK, no errors.
- dco_timeout message now names the actual wait (DcoClient stores its timeout; "10 秒" / "0.2 秒"). New test the_timeout_message_names_the_real_wait.
- cargo test -p dct-dco -p dct-game: dct-dco 3 passed, dct-game 108 passed, 0 failed. clippy --all-targets clean. rustfmt --check clean on lib.rs, client.rs, fake.rs.
