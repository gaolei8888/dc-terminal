# Task D19 report

Status: DONE (explore_cli.rs wiring deliberately left to Task D16).

## Implemented
- src/explore_model.rs: LlmModel { backend, timeout } with Model::pick and Fixer::fix; prompt_for_pick, read_pick, prompt_for_fix; DEFAULT_TIMEOUT 60 s.
- Prompt structure: user message is a single serde_json object {"state", "options":[{key,text}]} (all options, no cap), so quotes/newlines in option text are escaped and cannot break out. System prompt marks it untrusted data. max_tokens 200 for pick.
- read_pick: first {...} object in reply; unknown key / "stuck" / non-string key / TapIcon without non-empty name -> None; name dropped for non-icon options.
- Fixer: prompt is JSON {body, rejection}; max_tokens 3000 (must re-emit the entire body; truncation only yields a dcv rejection and the 3-round cap handles it). Reply -> None if empty, identical to input (trimmed), contains extra ``` fences, or starts with commentary phrases (Here is/Sure/好的/以下/...). A single surrounding fence is stripped.
- picture_png ignored.
- src/lib.rs: `pub mod explore_model;` after `daemon`.

## RED
Stub impl + the 10 tests: `cargo test --lib explore_model` -> `3 passed; 7 failed` (reads_a_strict_json_pick, an_icon_pick..., the_first_json_object_wins, the_prompt_lists_every_option_not_just_twenty (assertion failed: p.user.contains("\"e44\"")), option_text_cannot_break..., fix-prompt, fixer-changed-body failed).
## GREEN
Real impl: `10 passed; 0 failed`.

## Full check
- cargo test --workspace -- --test-threads=1: 2301 passed, 0 failed, 5 ignored (daemon_upgrade did not flake).
- cargo clippy --all-targets: warning-free (only the pre-existing src/student_projects.rs:669 warning from the dct lib check).
- rustfmt --check --edition 2021 src/explore_model.rs: clean. Note: running it on src/lib.rs recurses into all modules and reports pre-existing diffs in other files (bridge.rs, clipboard.rs, ...); none in lib.rs or explore_model.rs. No other file was reformatted.
- cargo check --workspace --target x86_64-pc-windows-msvc: only the pre-existing student_projects.rs:669 warning.

## Files
src/explore_model.rs (new), src/lib.rs (1 line). Commit message as given by brief, no attribution.

## Self-review / concerns
- Chatty-prefix list is heuristic; a legitimate body beginning with e.g. "sure" would be rejected (-> None, safe direction).
- "first {...}": if commentary before the real JSON contains a stray `{`, parse fails -> stuck (safe).
- Not wired: D16 must construct LlmModel::new(backend, DEFAULT_TIMEOUT) in explore_cli.rs when [llm] is configured, else NoModel; and pass it as the Fixer.
