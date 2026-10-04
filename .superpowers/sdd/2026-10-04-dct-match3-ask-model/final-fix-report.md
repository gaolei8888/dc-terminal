# Final-fix report (F1-F4)

Commit: see `git log -1` on feat/match3-ask-model (single commit "fix(game): ask flags split, ...").

## Changes
- F1: cli.rs `--ask-every-step` (implies ask_model); `ask_always = a.ask_every_step && advisor.is_some()`; USAGE, `dct --help`, skill.md, spec §1 updated; parse tests extended.
- F2: `Advisor::available()` default true; LlmAdvisor `down` AtomicBool, one-time stdout line, no backend call once down; play.rs ask block requires `adv.available()`.
- F3: parse_reply strips closed `<think>` blocks, unclosed -> (None,""), number only at start (optional trailing `)`/`）`/punctuation before reason).
- F4: strengthened `a_failed_swap_triggers_the_model_and_is_listed_but_not_offered`; stale show_status comment fixed; ask record has `failed` and `goal`.

## Evidence
Tests were written alongside the implementation in one pass (no separate RED run); proof of bite is by mutation:
- `.map(|k| offered[k])` -> `.map(|k| k)`: `a_failed_swap_triggers_...` FAILED (play_tests.rs:778). Restored.
- removing `adv.available() &&` in play.rs: `an_unavailable_advisor_is_never_asked_...` FAILED. Restored.
- removing the `down` early return in advisor.rs: `after_one_backend_error_...` FAILED. Restored.
- ask.rs: old parse would fail "选 4 号" -> None test by construction (first-digit search).

## Commands
- `cargo +1.99.0 test --workspace --locked`: final run all ok (1683 main-crate tests, 0 failed). Two earlier runs each had one different failure (daemon::web_tests::enabling_starts_a_listener_and_disabling_stops_it, session::tests::recovering_from_a_failure_after_real_input_still_does_not_count): unrelated flaky tests (known sporadic), passed on rerun.
- `cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings`: clean.
- `dct --help` shows the new `game play` line.
