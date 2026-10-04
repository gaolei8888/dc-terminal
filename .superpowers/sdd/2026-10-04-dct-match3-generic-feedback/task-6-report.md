# Task 6 report
- RED: compile errors (no field labelled/picked_good/picked_bad).
- GREEN: game::bench 8 tests pass; workspace 1690 lib tests + all others pass; clippy -D warnings clean (fixed assert_eq!(.., true) in the brief's test -> assert!).
- Mutation: brief's mutation (good.contains(&order[0])) SURVIVED the brief's three tests (all-good/all-bad labels make any index match). Added test replay_maps_each_displayed_pick_back_to_exactly_one_original_candidate (loop over displayed picks 0..5, good=[0], bad=[1], expects totals (1,1)); with the mutation it fails; reverted.
- Also added report_text test for labelled line / caveat only when labelled == 0, and labelled==1 assert in out-of-range test.
- order[k] now uses order.get(k) (no panic on out-of-range advisor choice).
- Files: src/game/bench.rs only (report_text lives there; cli.rs untouched).
