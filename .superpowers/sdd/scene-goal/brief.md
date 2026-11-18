# Task: `dct game scene --goal "<text>"` (generic goal for the scene loop)

Repo: /Users/lei/work/dc/dc-terminal (Rust workspace; use `export PATH="$HOME/.cargo/bin:$PATH"` and `cargo +1.99.0`). Start from `main`, create branch `feat/scene-goal`. Commit in English, NO Co-Authored-By / AI attribution lines.

## Why
`dct game scene` asks the vision model "what is the next most worth tapping spot". There is no goal, so it can't be used to measure "how many model calls/tokens/seconds to get from A to B". We want an optional, GENERIC goal sentence (e.g. "find the honey") passed to the model. dct must stay generic: no game-specific text in code.

## Requirements
1. New flag `--goal <text>` on `dct game scene` (parse in `src/game/scene.rs` `parse`, USAGE string updated). Empty / missing value = error with a one-line plain-Chinese message like the other flags. Default: no goal (behaviour and prompt byte-identical to today).
2. With a goal, the user prompt sent to the model gets one extra line stating the goal (Chinese, plain), and tells the model it may answer a JSON with `"done": true` when the goal is already visibly achieved on screen. Extend `parse_pick`/`Pick` in `crates/dct-game/src/scene.rs` minimally to carry `done: bool` (default false); a pick with `done:true` ends the run with a new `SceneStop::GoalReached` (plain-Chinese summary line, exit code 0). Without `--goal`, `done` is ignored.
3. Records (`steps.jsonl`): add `"goal": <text or null>` to every step record; keep all existing fields.
4. Summary at the end of the run (the existing one that prints model-call counts/tokens): also print total model time in seconds (sum of the existing `model_ms`) and number of steps taken, in plain Chinese, one line. Do not add price conversion.
5. Tests (in the existing test modules): parse of `--goal`; prompt contains the goal only when given and is unchanged without it; `done:true` stops with GoalReached only when a goal was given; record has `goal`; summary line has time. Run `cargo +1.99.0 test --workspace --locked` twice and `cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings`. Run one mutation (ignore `done` when a goal is given) and confirm a test fails.
6. Do not touch dco, dcv or any game-specific config.

## Report
Write the full report to /Users/lei/work/dc/dc-terminal/.superpowers/sdd/scene-goal/report.md. Return only: status, commits, one-line test summary, concerns. Do not spawn subagents. Do not push or merge.
