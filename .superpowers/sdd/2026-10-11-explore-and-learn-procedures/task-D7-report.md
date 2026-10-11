# Task D7 report: options

Status: DONE. Commit: see `git log -1` on branch explore-and-learn ("dct-explore: options from the element list ...").

Implemented: crates/dct-explore/src/options.rs (Act, Opt, Goal, Progress, all_options, shortlist, state_text, question, steps_for + 12 tests);
lib.rs: `pub mod options` + `JevChoice` (same shape as root ChoiceQuestion); gate.rs: `fold` made pub(crate) (no duplicate).

TDD: tests written with the 5 public fns stubbed `todo!()`:
`cargo test -p dct-explore options` -> 11 failed, 1 passed (the pre-existing gate test matching the filter). With impl: 12 passed, 0 failed.

Full check: workspace tests 2184 passed, 0 failed, 4 ignored (--test-threads=1); clippy --all-targets warning-free after fixing 2 lints
(is_ascii, filter_map/then); rustfmt --check clean on options.rs, lib.rs (gate.rs change is one token); Windows check: only the pre-existing warning at src/student_projects.rs:669.
(Tests/Windows ran before the 2 clippy-only edits; dct-explore tests re-run after: 34+3 pass.)

Ambiguity decisions:
- Fill offered only if param not in prog.filled (spec literal); a wiped field must be handled by the caller clearing `filled` (D10).
- Textless input fields get no Fill (no intent to tap by); they appear as TapIcon.
- Tap key = element id (prefixed with `e` if missing); Fill key `fill_<param>_<id>`, Attach key `attach_<param>`; duplicate element ids skipped; `question` suffixes `_2` on any remaining key clash.
- Tap text kind: role button -> 按钮, anything else -> 文字. Option order: elements, Fill, Attach, fixed; shortlist preserves it. `done` counted as fixed in shortlist.
- Fixed fill/attach slots are capped so the total stays <= 20 even then (fixed first).
- TapIcon with no name / Done / Stuck -> steps_for returns empty.
- Textless element with no text neighbour: text "没有字的图标，画面上没有其他字可参照".
- question criteria descriptions = Some(text); instructions is one fixed English sentence.

Concerns: none blocking. Extra test publish_also_waits_for_files covers file params gating publish.
