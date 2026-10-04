# Final-review fix wave (one dispatch, all findings)

Repo /Users/lei/work/dc/dc-terminal, branch feat/match3-ask-model, head fc2bdd9. Spec: docs/superpowers/specs/2026-10-04-dct-match3-ask-model-design.md. Rulings already made by the controller (binding):

## F1 — flags (src/game/cli.rs, Args, USAGE, parse, run_parsed; src/main.rs help line; src/game/skill.md; the spec §1)
- `--ask-model`: build the advisor (as now) and ask ONLY after a refused swap on the current board (ask_always = false).
- New `--ask-every-step`: implies --ask-model AND sets ask_always = true.
- Update: parse tests (both flags; --ask-every-step alone also enables the advisor), USAGE, `dct --help` line for `dct game play` (list --ask-model, --ask-every-step, --goal), the skill card section (plain Chinese: 「卡住了才问大模型」/「每一步都问」), and spec §1 wording (the "用户要求" trigger now names `--ask-every-step`; `--ask-model` enables asking after a refusal).
- `ask_always` must be `a.ask_every_step && advisor.is_some()`.

## F2 — dead model (crates/dct-game/src/ask.rs trait, play.rs, src/game/advisor.rs)
- Add to trait Advisor: `fn available(&self) -> bool { true }` (default).
- LlmAdvisor: keep an `AtomicBool down`. In `pick`: if down → return None immediately (no backend call). If the backend call fails (the `.ok()?` path returns None) → set down and print ONCE (use an AtomicBool/Once) to stdout the plain line 「大模型没回应，后面只用规则。」. `available()` returns !down.
- play.rs: the ask block must also require `adv.available()`, so a down advisor causes no ask and NO show_status("think")/("look"). 
- Tests: advisor.rs — after one Err the next `pick` makes no backend call (count calls on the Fixed fake) and `available()` is false; play_tests.rs — an advisor whose `available()` is false is never called and no think/look events appear.

## F3 — reply parsing (crates/dct-game/src/ask.rs parse_reply)
- Remove every closed `<think>...</think>` block first; if an unclosed `<think>` remains (reply cut off), the result is (None, "") .
- Then trim; accept a number ONLY at the very start of the remaining text (digits immediately; allow an optional leading ASCII/full-width bracket-less form like "3" or "3)" or "3，理由"); anything else (e.g. "我选 3", "1) 和 3) 比，我选 3" → starts with "1" so that one still reads 1; that is accepted behaviour) → follow the existing range rules (1..=n else None).
- Update existing parse tests: `parse_reply("选 4 号：做出炸弹", 5).0` is now None; add tests: closed think block then "2" → Some(1); unclosed think → None; "<think>先看第 3 行" → None; "共 5 步，选 2" → None. Keep reason extraction for "2，因为…" .

## F4 — tests/comments/records
- play_tests.rs: strengthen `a_failed_swap_triggers_the_model_and_is_listed_but_not_offered` (or add a new test): after the refused first swipe, the second step's `chosen` index and swipe differ from the first step's refused swap, and `log[1]["ask"]["asked"]` does not contain the refused candidate's index; make the Say advisor choose offered index 0 so that mapping through `offered` (not the raw number) is what keeps it off the refused swap. Prove it with a mutation (use raw `k` instead of `offered[k]` → test red), then restore.
- Fix the stale comment near play.rs:269 about show_status ("留给以后慢的（模型）决策" → it is now used while the model is asked).
- The ask record in play.rs gets two more fields: `"failed": [describe_move strings of the refused swaps shown to the model]` and `"goal": o.goal`.

## Commands/rules
Commit messages English, NO Co-Authored-By/AI line; do not push. PATH must include $HOME/.cargo/bin; use `cargo +1.99.0 ...` with CARGO_TARGET_DIR=/private/tmp/claude-502/-Users-lei-work-dc-dc-terminal/d52f1f4f-cb49-40cc-ba88-c37403102743/scratchpad/target-199. TDD for each item; run `cargo +1.99.0 test --workspace --locked` and `cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings` before committing. Commit as one or a few commits. No subagents. Write the report (what changed, RED/GREEN evidence, mutation results, the commands you ran and their output) to /Users/lei/work/dc/dc-terminal/.superpowers/sdd/2026-10-04-dct-match3-ask-model/final-fix-report.md.
