# Task D11 report
Implemented by a stale copy of the controller session (terminated); committed bea7cf4 without a report. Controller full check at 88a686d: 2220 passed, 0 failed, 4 ignored; clippy 0; rustfmt clean. No TDD evidence available.

## Fix round 1

Changes (crates/dct-explore/src/draft.rs only):
- Popup names are now `<platform>-<ascii slug of seen text, <=24>-<8-hex FNV-1a of seen+tap arg>` (`<platform>-popup-<hash>` when the slug is empty). Valid dcv slug (checked against dc-vault note::is_slug), stable across Rust versions. Dedupe only drops byte-identical popups (same hash means same name+body), so nothing distinct is lost; no note needed for true duplicates.
- New `DraftError::BadStepArg(String)`: a copied step (other than `back`) with empty or multi-line arg returns an error instead of a body dcv rejects.
- New tests: verified publish_done marker (3-space `   完成标志: verify_text 「…」`, via a cloned x row with unverified cleared, since the youtube seed row has no publish_done), brace marker omitted, NoPublish, Empty, short popup text -> notes only, two Chinese popups, two English popups sharing a 24-char prefix, same popup twice -> one stable draft, empty/newline arg error.

RED: `cargo test -p dct-explore draft` -> 3 failed (two_distinct_chinese..: left 1 right 2; two_english..: left 1 right 2; an_empty_or_multiline_step_arg_is_an_error panicked). The marker/NoPublish/Empty/short-popup/brace tests passed on the existing code (they were coverage gaps, no bug).
GREEN: `cargo test -p dct-explore` -> 66 + 12 + 3 passed, 0 failed.
Checks: clippy --all-targets warning-free; rustfmt --check on draft.rs ok; windows check only the pre-existing unused `path` warning.

Concerns: none blocking. Popup names changed shape (hash suffix), so any caller/test hardcoding old names would need updating (none found in the build).
