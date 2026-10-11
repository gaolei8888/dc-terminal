# Task D5 report: platform table

Status: DONE.

## Implemented
- crates/dct-explore/src/platforms.toml: seed, verbatim from brief (6 platforms).
- crates/dct-explore/src/platform.rs: Platform, Table (seed/with_override/get), validate.
  - name rule replicates dcv procedure.rs check_platform (lowercase start, [a-z0-9_-], max 32).
  - entry_url: https:// only, host (up to first / ? #) must equal domain exactly; '@' and ':' (port) rejected.
  - publish_words non-empty (no blank entries), browser/vault/domain non-empty.
  - override: same name replaces whole row, new name appends, every row validated; bad TOML or unknown fields -> Err.
- lib.rs: `pub mod platform;`

## TDD
RED: `cargo test -p dct-explore --lib platform -- --test-threads=1` with todo!() stubs: 0 passed, 4 failed ("not yet implemented").
GREEN: same command: 4 passed, 0 failed.
Tests: the 4 from the brief, extended with extra URL cases (userinfo '@', port, fragment ok), replace-not-append row count, append of new name, malformed TOML.

## Full check
- cargo test --workspace -- --test-threads=1: 2160 passed, 0 failed, 4 ignored (was 2156).
- clippy --all-targets: 0 warnings.
- rustfmt --check on platform.rs, lib.rs: clean.
- cargo check --workspace --target x86_64-pc-windows-msvc: only the pre-existing unused `path` warning.

## Files
platform.rs, platforms.toml (new), lib.rs (modified). Committed locally only, nothing from .superpowers.

## Concerns
None. Note: override validation errors name the platform but not the TOML row index.

## Addition: Chinese platforms (finished by controller after a session restart)
The implementer was stopped by a parent-session restart after writing platform.rs/platforms.toml edits but before testing/committing/reporting. Controller verified and committed them.
- Rows: weibo, bilibili, zhihu, zhihu_article, douyin, kuaishou, wechat_channels, toutiao, wechat_mp, baijiahao (+ xiaohongshu login_qr). Fields `login_qr`, `publish_by_user_only` (serde default false).
- Tests: `cargo test -p dct-explore platform` → 9 passed (new: chinese_platforms_are_seeded_and_valid, qr_login_and_publish_by_user_flags, override_without_the_new_fields_defaults_both_to_false). rustfmt --check platform.rs clean.
- TDD RED evidence: not available (lost with the restarted implementer).

## Addition fix round 1

Changes (commit 0c1ac74, two files only):
- `crates/dct-explore/src/platform.rs`: test `chinese_platforms_are_seeded_and_valid` now asserts `entry_url`, `login_gate`, `publish_words`, `publish_done`, `vault`, and `never_tap` are all in each new row's `unverified`.
- `crates/dct-explore/src/platforms.toml`: `"never_tap"` added to `unverified` on the 10 new rows (weibo, bilibili, zhihu, zhihu_article, douyin, kuaishou, wechat_channels, toutiao, wechat_mp, baijiahao). Older rows unchanged.
- Comment fixes: weibo now reads "据说首页直接发，未核实"; wechat_mp now reads "推广设计第 3 批说编辑器难驱动，最后发布由用户点（未在本机核实）".

Commands and output:

```
$ cargo test -p dct-explore platform 2>&1 | grep -E "^test |test result"
test platform::tests::seed_has_the_six_platforms_and_all_rows_are_valid ... ok
test platform::tests::qr_login_and_publish_by_user_flags ... ok
test platform::tests::chinese_platforms_are_seeded_and_valid ... ok
test platform::tests::entry_url_must_be_https_on_the_fixed_domain ... ok
test platform::tests::a_row_without_publish_words_is_refused ... ok
test platform::tests::override_without_the_new_fields_defaults_both_to_false ... ok
test gate::tests::follow_subscribe_boost_and_platform_never_tap_are_not_options ... ok
test gate::tests::platform_never_tap_is_enforced_on_its_own ... ok
test platform::tests::override_replaces_a_row_and_adds_new_ones_with_the_same_checks ... ok
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 41 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out

$ rustfmt --check --edition 2021 crates/dct-explore/src/platform.rs
(no output, exit 0)
```

## Addition: Chinese platforms
- Schema: Platform gains `login_qr: bool` and `publish_by_user_only: bool` (both serde default, doc-commented).
- Seed: 10 new rows (weibo, bilibili, zhihu, zhihu_article, douyin, kuaishou, wechat_channels, toutiao, wechat_mp, baijiahao), all fields unverified, no login_gate/publish_done, placeholder publish_words ["发布"]; xiaohongshu gets login_qr = true; wechat_mp has publish_by_user_only; header comment says Chinese gate texts and publish words are unknown placeholders. Seed is now 16 rows.
- Tests added: chinese_platforms_are_seeded_and_valid, qr_login_and_publish_by_user_flags, override_without_the_new_fields_defaults_both_to_false. The old override test's hard-coded 6/7 row counts became relative to the seed size.
- RED 1: tests first -> compile error E0609 (no field login_qr / publish_by_user_only).
- RED 2: fields added, no data -> `chinese_platforms_are_seeded_and_valid` and `qr_login_and_publish_by_user_flags` FAILED (8 passed, 2 failed in the first run of the data step before data; after data the only failure was the stale 6/7 count assertion, fixed).
- GREEN: workspace `cargo test --workspace --no-fail-fast -- --test-threads=1`: 2206 passed, 0 failed, 4 ignored. (One earlier run hit a transient failure in `an_agent_that_exits_on_its_own_does_not_become_a_zombie` under concurrent machine load; rerun green.)
- clippy 0 warnings; rustfmt --check platform.rs clean; Windows check only the pre-existing `path` warning.
- Staged only platform.rs and platforms.toml.
