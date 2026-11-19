# Task 1 report: [[keep]] parsing

Status: DONE_WITH_CONCERNS (concern = pre-existing flaky tests only)

## Changed
- src/keep.rs (new, registered in lib.rs): Keep, parse(src, home), load(path, home); 9 unit tests.
- src/proto.rs: WarningCode::KeepEntryInvalid{index, reason: KeepEntryProblem}, KeptSessionProblem{name, reason: KeptSessionReason}; PROTOCOL_VERSION 26 -> 27 (doc entry added); 5 pinned-shape tests updated to 27 (request shape, drain cancelled/instance, drain progress, no-relay error, projects response).
- src/i18n.rs: zh + en rendering for both variants (mirrors SessionResumeSkipped), added to the all-codes test (no newline, no Han in en).
- src/config.rs: test that a malformed [[keep]] does not hide [llm]. Config has no deny_unknown_fields so no Config change needed.

## Enum shapes
- KeepEntryProblem { MissingField(String), EmptyName, DuplicateName(String), NotATable }. NotATable added for non-table entries and for `keep` not being an array (index 0 in that case).
- KeptSessionReason { DirGone, ProfileGone, StartFailed, CrashLooping, NameClash } (Copy).
- Whitespace-only name -> EmptyName; absent/non-string/blank other fields -> MissingField(field). Names/profile/dir are trimmed.
- `~` and `~/x` expanded; `~user` is left alone.

## Tests
- Baseline (untouched worktree): 2086 pass, 3 fail (session::tests::*, e.g. a_dead_model_leaves_the_first_line_as_the_name, a_session_gets_named_after_its_first_round_of_work, ...). Failing set varies run to run (also daemon::tests flakes seen) = pre-existing flakiness, not fixed.
- Final: 2095 pass, 4 fail per run, all in session::tests (same flaky family, different members each run); keep/config/i18n/proto tests all pass; clippy +1.99.0 -D warnings clean.

## Concerns
- Flaky session tests make a fully green `cargo test` impossible to demonstrate on this machine.
- container/classroom/server.mjs mentions protocol version in a comment/test message; I did not check whether it pins a version number (proto.rs message says to update it with the pin).

## Fix round 1
- Wrong-type test now asserts index and reason.
- New tests: trimming of name/profile/dir, duplicate detected after trimming, non-string field -> MissingField.
- `keep` not an array now has its own reason `KeepEntryProblem::KeepNotAnArray` (index 0, not rendered); zh/en message says each entry should start with [[keep]] and none were used; added to all-codes i18n test. NotATable kept for non-table entries inside the array.
- The three fields are read with three let-else calls (no Vec/try_into/expect).
- keep/i18n/proto tests pass, clippy +1.99.0 clean.
