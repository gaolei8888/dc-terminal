# Task 1 report
- RED: 5 of 6 new swipe_settle tests failed with the default impl (the NotSwiped test passed trivially).
- GREEN: all 6 pass. Workspace: 1707 + others all pass, clippy -D warnings clean.
- Mutations: (1) drop `no_swipe_settle = true` on missing settle -> old-dco test red; (2) set the flag on settle.error too -> wait-error test red. Both reverted.
- Files: crates/dct-game/src/play.rs (SETTLE_* consts as u32, SwipeSettle, SwipeOutcome, trait default), dco.rs (no_swipe_settle flag, swipe_settle), dco_tests.rs (6 tests).
- Concern: none. A non-object `settle` (e.g. null) is treated as old dco.

## Fix round 1
- swipe_settle now temporarily sets timeout + socket read timeout to `settle_call_timeout` (max(timeout, SETTLE_TIMEOUT_MS+4s) = 12s by default), restored after the call.
- `dco_timeout` on that call -> SwipedNoSettle(err), flag not set. Other transport errors (disconnect etc.) cannot be cleanly split into before/after write, so they stay NotSwiped.
- `settle` missing `changed` or `settled` -> SwipedNoSettle(bad_settle), flag not set. Doc comment added on `no_swipe_settle`.
- Tests: swipe_settle_client_timeout_is_swiped_no_settle_and_late_reply_is_skipped, swipe_settle_malformed_report_is_not_a_bounce_back (8 swipe_settle tests pass).
- Mutations: removing the dco_timeout arm -> timeout test FAILED; accepting `{}` -> malformed test FAILED. Reverted.
- Workspace tests: no failures; clippy -D warnings clean.
