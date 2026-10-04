# SDD ledger — plan: docs/superpowers/plans/2026-10-04-dct-match3-ask-model.md
Branch: feat/match3-ask-model (from main 6a74e8c). Spec: docs/superpowers/specs/2026-10-04-dct-match3-ask-model-design.md (reachable).

## Preflight scan
| Pair / task | Produces vs consumes / self-consistency | Finding |
|---|---|---|
| T1 -> T2 | AskInput/Advice/Advisor/board_text/describe/describe_move (T1) used by play.rs (T2) | match |
| T2 -> T3 | Options<'a>/NavOptions<'a> new fields (T2) built in src/game/cli.rs (T3) | match; T2 also edits src/game/cli.rs construction sites, T3 edits same file later: sequential, fine |
| T1 -> T3 | prompt_text/parse_reply used by LlmAdvisor | match |
| T3 -> T4 | load_llm_backend, LlmAdvisor used by bench | match |
| T2 own | tests use Fake/Clk/profile/grid/A/MOVED from play_tests.rs; placeholder test "single candidate" must be replaced with real test (plan says so) | note for implementer |
| T3 own | spec edit (drop no-moves trigger) listed in Files; skill.md edit | ok |
| Spec vs plan | spec trigger 2 (no moves) dropped by plan, T3 step 1 amends spec | Ruling: plan wins, spec amended in T3 — no candidates means nothing to choose |
Ruling: git commit messages in English with no Co-Authored-By (user standing rule overrides the attribution reminder) — if wrong, amend messages before push.
Task 1: Outcome Default confirmed by compile (tests pass); commit body has no AI line.
Task 1: minor (deferred): board_text letters wrap at 26 classes; prompt appends 。after goal (double punctuation if goal ends with one); parse_reply takes first digit run anywhere (prefer leading number); describe hard-codes candy terms (flag for final review); Advice/AskInput lack Debug/Clone.
Task 1: complete (commits 6a74e8c..b1601d5, review clean)
Task 2: octopus requirement added by user (show_status think/look around adv.pick) — implemented and tested.
Task 2: minor (deferred): offered[k] mapping with a failed candidate not pinned by a test; ASK_SHOWN=8 truncation untested; stale comment at play.rs ~269 about show_status "left for later"; ask_budget is per play() call (auto_next gives each level a fresh budget) — decide at final review whether whole-command budget is wanted.
Task 2: complete (commits def0fe4..b19559f, review clean)
Task 3: minor (deferred): ask_always true even when advisor None (harmless; could pass a.ask_model && advisor.is_some()); long llm_cannot_connect line (fmt if CI checks); LoadedLlm carries provider/http for llm_check.
Task 3: complete (commits b19559f..56c2cfa, review clean)
Task 4: minor (deferred): `answered` counted not printed; unreadable/invalid lines skipped silently (asked 0 gives no hint).
Task 4: complete (commits 56c2cfa..fc2bdd9, review clean)
Final review: no Critical. Important 1 (default refusal trigger unreachable: advisor only built with --ask-model and that flag also set ask_always), 2 (dead model asked every step: up to 30x20s per level), 3 (parse_reply takes first digit anywhere; <think> text can be parsed as a choice).
Ruling: Important 1 — split flags: `--ask-model` builds the advisor and asks only after a refused swap; new `--ask-every-step` (implies --ask-model) sets ask_always. Chosen over (a) auto-asking whenever [llm] is set (silent data-sending/latency for users who never asked for it) and (b) opt-in-only-every-step (makes the spec's main trigger unreachable). Costs: one more flag; spec/skill text updated; if wrong, flip default in cli.rs.
Ruling: Important 2 — after the first failed ask (advisor returned None) the LlmAdvisor stays down for the rest of the command, prints one plain line once, and play() skips the ask and the octopus think/look when `advisor.available()` is false; budget stays per play() call as spec says. Costs: a transient network blip disables the model for that command.
Ruling: Important 3 — strip closed <think>..</think>; unclosed <think> = no answer; accept only a number at the very start of the reply (after trimming) — "选 3" style replies are no longer accepted. Costs: some valid verbose replies become "no choice" (falls back to rules).
Ruling: deferred minors to fix before merge: offered-mapping test, stale comment; plus cheap extras: ask record gets `failed` and `goal`; help line lists --ask-model/--ask-every-step/--goal. ASK_SHOWN truncation test and ask-bench polish stay deferred.
Final fix wave: complete (commit f947325, scoped re-review clean; deferred minors: stray </think> without opener -> None; full-width digits rejected; println! in LlmAdvisor library layer; failed ask leaves no record line)
Final review: complete.
