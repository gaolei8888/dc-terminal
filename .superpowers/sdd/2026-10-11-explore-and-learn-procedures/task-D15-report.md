# Task D15 report
Implemented src/campaign.rs (Campaign, parse_params, check_distinct, approve, load, run_cli), wired Some("campaign") in main.rs + HELP, pub mod in lib.rs.
- approve: validates name (no empty, /, \, .., leading dot, spaces, >64), requires non-empty params, distinct check BEFORE asking, store only after yes, file 0600 (cfg(unix)). CLI requires stdin IsTerminal.
- Platform/param names: small local check (dct_explore check_name is private; crates/ off-limits).
- check_distinct compares post_text only: identical (case-insens.) or same opening sentence (up to . ! ? 。！？ or newline).
## RED
Stub impl: 7 failed, 0 passed (all campaign tests FAILED).
## GREEN
campaign tests 7 passed. Full workspace --test-threads=1: passed 2261 failed 0. clippy --all-targets: no warnings. rustfmt --check src/campaign.rs ok. Windows check: only pre-existing src/student_projects.rs:669 warning.
## Concerns
Opening-sentence rule only applies to post_text. Store dir = parent of daemon socket path + /campaigns (same as approvals dir convention).

## Fix round 1
- parse_params refuses control chars except \n and \t (ANSI/\r/BEL/U+0085).
- render: continuation lines of multi-line values are indented with `      | `, so a value cannot forge a `[platform]` header.
- lib.rs: `pub mod campaign;` moved after `bridge` (rustfmt order); src/lib.rs rustfmt-clean (HEAD~1 was not clean overall because rustfmt follows mods, but lib.rs itself had no diff).
- opening_sentence: `.` only ends a sentence when followed by whitespace/end; inner whitespace collapsed.
- RED: `cargo test --lib campaign` -> 7 passed, 3 failed (control_characters..., a_multiline_value..., opening_sentence_ignores...). Two tests then had mistakes of mine (counted continuation line; `e.g. ` is a sentence end) and were corrected.
- GREEN: 10 passed. Workspace: 2282 passed, 0 failed. clippy clean, rustfmt campaign.rs/lib.rs clean, Windows check only the student_projects.rs:669 warning.

## Fix round 2
- is_hidden_or_control: Cc (except \n, \t) plus explicit Cf/Zl/Zp ranges (U+00AD, 0600-0605, 061C, 06DD, 070F, 180E, 200B-200F except 200D, 2028-202E, 2060-206F, FEFF, FFF9-FFFB, 1BCA0-1BCA3, 1D173-1D17A, tags E0000-E007F). U+200D ZWJ is ALLOWED so combined emoji work; variation selectors are not Cf and pass. Tag characters are refused (so subdivision flag emoji are not accepted).
- Used by parse_params and by load() (also re-validates platform/param names), so hand-edited files cannot smuggle characters into dct do.
- RED (range check disabled): 2 failed (invisible_and_bidi_characters_are_refused, a_hand_edited_file_..._refused_by_load), 10 passed. GREEN: 12 campaign tests pass; workspace 2301 passed, 0 failed; clippy clean; rustfmt clean; Windows only the student_projects.rs:669 warning.

## Fix round 3
- Added U+0890-0891, U+08E2, U+110BD, U+110CD, U+13430-1343F (the others named were already covered). Comment now says it is a hand-written Unicode 15 table, not "all Cf".
- approve() runs validate_per (valid_ident for platform/param names + check_value) before the distinct check and before asking; load() uses the same fn.
- RED (new ranges + approve check removed): 2 failed (more_format_characters_are_refused, approve_refuses_what_load_would_refuse), 12 passed. GREEN: 14 campaign tests; workspace and checks below.
