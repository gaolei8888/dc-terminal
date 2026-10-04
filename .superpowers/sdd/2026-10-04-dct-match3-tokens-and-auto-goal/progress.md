# SDD ledger — plan: docs/superpowers/plans/2026-10-04-dct-match3-tokens-and-auto-goal.md
Branch: feat/match3-tokens-goal (from main f54e8fe). Spec: docs/superpowers/specs/2026-10-04-dct-match3-tokens-and-auto-goal-design.md (reachable).

## Preflight scan
| Pair / task | Produces vs consumes / self-consistency | Finding |
|---|---|---|
| T1 -> T2 | Usage, complete_counted_with_timeout (T1) used by LlmAdvisor (T2) | match |
| T2 own | Advice gets `tokens`; every `Advice {..}` literal (play_tests Say, bench tests, advisor tests) must add `tokens: None`/Some | noted in plan |
| T2 -> T3 | independent files except play.rs (T2 adds two ask-record fields, T3 changes progress/goal blocks) | no overlap |
| T3 own | observe only on `moved`; refused arms must NOT observe; plan says add a test if existing ones miss it | noted |
| Spec vs plan | spec says GoalFinder locks once found and ignores list-length changes; plan matches | match |
Ruling: git commit messages in English with no Co-Authored-By (user standing rule).
Task 1: minor (deferred): no end-to-end test of HttpBackend::complete_counted with a malformed usage body; no float (12.0) case; doc comment that Anthropic input_tokens excludes cache tokens.
Task 1: complete (commits d189878..1980588, review clean)
Task 2: minor (deferred): the cli.rs summary-print wiring has no test; dry-run never asks so no summary (not traced by reviewer).
Task 2: complete (commits 1980588..a1751b6, review clean; commit bodies checked: no Co-Authored-By)
Task 3: minor (deferred): rename misleading test without_a_goal_index_a_flat_run_never_stalls_or_asks (a single flat number is now found as the goal after 4 samples; the test only passes because the run is short); if the first valid sample has a wrong length every later step is skipped (fails safe: never found); no test that stall counting continues after a refused step once found.
Task 3: complete (commits a1751b6..820ad2d, review clean)
Final review: no Critical. Important 1 (a constant number — level number, booster count — is picked as the goal by elimination; false stall flags + extra model asks, bounded by 30/level), Important 2 (no goal found => stall silently off; nobody told).
Ruling: Important 1 — GoalFinder only accepts a candidate that DROPPED at least once during sampling (ever_decreased); never-moving numbers are never picked. This reverses spec test a_goal_that_never_moves_is_still_found_by_elimination: spec section 二 and that test are changed. Chosen over keeping elimination-only (false stalls) because wrongly asking the model costs time/tokens and the printed sentence does not warn later; cost: a goal that stays flat for the first steps is not found until it moves (user can give --goal-number).
Ruling: Important 2 — skill.md gets one plain sentence: when it cannot tell, it does not judge progress; use --goal-number. No extra print.
Ruling: minor 4 — the first moved step is not counted as a sample (progress_prev can be read before the first board and capture a level-start popup). Cost: finding needs one more step (MIN_SAMPLES counts later steps).
Ruling: rename the misleading flat-run test and add an 11-step variant that does stall (with a number that dropped once, since flat-only numbers are no longer found).
Final fix wave: complete (commit c1392a5, scoped re-review clean; deferred: spec test list by behaviour not names; cosmetic markdown nit; goal flat for several steps before its first drop is not found until it drops — stated in spec)
Final review: complete.
