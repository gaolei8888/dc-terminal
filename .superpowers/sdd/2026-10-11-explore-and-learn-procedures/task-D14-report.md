# Task D14 report

Implemented crates/dct-explore/src/replay.rs (replay, publish_step, ReplayInput incl. `platform` per R2, ReplayOutcome) and tests/replay.rs (14 tests: the 7 required + 7 extra).

Rules: draft/non-true `executable` -> Stopped, no dco call. needs_help -> scrubbed elements (Scrub), screen_halt -> cancel_run + Stopped (no ladder member asked); options = tappable elements + not_there + stuck (D7 wording, neighbour_hint for icons); Ladder::decide; Jeff/model -> By::Model, user -> By::User; not_there -> NotThere; max 5 answers. failed with no fixed word (超时/急停/锁屏/私人/run_timeout/halted/screen_locked/private_screen) -> NeedsRepair with prefix of steps 1..at_step-1 (from_procedure, by User, ok). blocked -> Blocked. R11: publish_by_user_only forces preview; no publish step -> Stopped. Preview where dco returns done -> Stopped (honest warning).

RED: `cargo test -p dct-explore --test replay` with unimplemented stubs: 0 passed; 14 failed ("not implemented").
GREEN: 14 passed; 0 failed.
Full: `cargo test --workspace -- --test-threads=1` all ok (root crate 1707 passed, 1 ignored; no failures; daemon_upgrade did not flake). clippy --all-targets warning-free. rustfmt --check ok on touched files. Windows check: only pre-existing src/student_projects.rs:669 warning.

Files: new replay.rs, tests/replay.rs; edited lib.rs (`pub mod replay;`), explore.rs (`pub(crate) mod screen`), explore/screen.rs (Scrub + new/text/line/els pub(crate)), options.rs (neighbour_hint pub(crate)).

Concerns: visibility edits to 3 existing files were needed to reuse Scrub/neighbour_hint. Over 20 elements: Jeff sees the first 18 (intent matches sorted first). Unmodified working-tree changes in src/learn.rs, src/campaign.rs, src/lib.rs are not mine and were not staged.

## Fix round 1
- Fault words now include `stale`, 窗口关了, 窗口已经关了, 要帮忙超过 (dco run/mod.rs, intents.rs, popups.rs; `not_found` left out: dco uses it for missing elements/runs, not windows). Matched on the failing step's note and raw `code` only.
- answer_run Err now cancels the run; already_done on a preview says the completion marker was already on screen and nothing ran.
- RED: 4 of 5 new behaviour tests failed (window/helps, earlier-note, answer_run cancel, already_done); GREEN: 22 passed. Added also: stopped on non-preview, >20 help elements (Jeff 20, user 27).
- Full workspace tests all ok, clippy 0 warnings, rustfmt ok, Windows check only the pre-existing warning.
