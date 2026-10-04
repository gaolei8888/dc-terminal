# Task 2 report
Commit: 6254362. Files: crates/dct-game/src/play.rs, play_tests.rs (Fake gets `settles` queue + swipe_settle override; empty queue = trait default behaviour).
RED: 5 new tests failed before implementation. GREEN: dct-game 222 pass; workspace tests pass; clippy -D warnings clean.
Mutations: (1) `!changed` reads grid instead -> dco_says_unchanged test red. (2) trust `settled` ignoring timed_out -> initially SURVIVED (my timed_out test used settled:false); added settled_flag_with_timed_out_is_still_not_trusted, now red.
Concerns: on the dco path timing_ms.settle is measured from swipe start (the wait happens inside swipe_settle), so swipe+settle double counts that time; rec.swipe.duration_ms likewise includes the wait. rustfmt not installed for 1.99.0 (fmt not checked). navigate.rs compiles unchanged.

## Fix round 1
Commit: see git log ("fix(game): on the dco path split the swipe call ..."). play.rs: dco path with settled_ms=Some(m): swipe_ms = call - m, settle = m + extra read time; settled_ms None / timed_out / SwipedNoSettle(non-"unsupported") fall back with swipe.duration_ms_includes_settle=true; default-trait path ("unsupported") unchanged, no flag. NotSwiped records call duration.
Tests (Fake now has shared clock, call_ms, read_ms): dco_path_splits_the_call_into_swipe_and_settle (swipe 80, settle 450, sum = 530 wall, duration_ms == timing swipe), fallback_paths_flag_..., poll_path_has_no_..._flag.
Mutation (swipe_ms = total again): dco_path_splits test FAILED (224 pass/1 fail); restored. Final: dct-game 225 pass, workspace pass, clippy -D warnings clean.
