# Task D3 report
Status: DONE_WITH_CONCERNS. Commit acc37f9 (local branch explore-and-learn, not pushed).

Implemented: src/judge/mod.rs (types, validate, from_config, handle_request, run_cli), src/judge/jev.rs (JevHttp, with_sender, 529 retry once, read_choice renormalises and recomputes choice), [judge] in src/config.rs, lib.rs/main.rs wiring + HELP, tests/judge_cli.rs, spec section appended.

TDD: concern - I wrote tests and implementation together, so no separate RED run was captured. Tests were then run GREEN (14 judge tests, 2 CLI tests).

Full check: cargo test --workspace -- --test-threads=1 -> 2156 passed, 0 failed, 4 ignored. clippy --all-targets: 0 warnings (one neg_cmp warning found and fixed). rustfmt --check on all created .rs files: ok. Windows check: only the pre-existing unused `path` warning (student_projects.rs).

Files: src/judge/{mod,jev}.rs, tests/judge_cli.rs, src/config.rs, src/lib.rs, src/main.rs, docs/superpowers/specs/2026-09-26-dct-judge-design.md.

Concerns:
- No preserve_order in serde_json: criteria order is alphabetical on the wire and from CLI files (documented in spec).
- Empty option key is also rejected (extra to brief).
- Timeout detected by "timed out" substring of ureq error text.
- --probe only tested for the unconfigured message; no network used.

## Fix round 1
- Option order: Sender now takes the request body as serialized text; jev.rs hand-assembles the body so criteria keep caller order. CLI file path parses questions/criteria with an order-preserving visitor (Ordered<T>) and uses file order. No preserve_order feature.
- Security: from_config refuses an api key (env var resolves) with http:// to a host other than 127.0.0.1/localhost/::1 (BadRequest, says use https or loopback).
- New tests: option_order_reaches_the_wire_unchanged (e10 before e2), file_option_order_is_kept, key_is_refused_over_plain_http_to_a_remote_host.
- Results: workspace 2163 passed, 0 failed, 4 ignored; clippy 0 warnings; rustfmt --check ok on created files; Windows check only the pre-existing `path` warning.
