# Task 3 report
RED: `cargo test --lib game::` failed to compile (LlmAdvisor not found). GREEN: workspace tests all pass (1679 lib tests + integration), clippy -D warnings clean.
Files: src/game/advisor.rs (new), src/game/{mod,cli,text}.rs, src/game/skill.md, src/cli.rs (load_llm_backend), spec doc.
Deviations: NotEnabled carries the config path and Problem is a struct variant {error, provider, http}, LoadedLlm also has provider/http, so llm_check prints byte-identical output (the "using" line is printed before the cannot-connect line).
Smoke: with an empty HOME, `dct game play --ask-model --dry-run` and without --ask-model both print "dco 没在运行" rc=1 (dco connect happens before the advisor is built), so the "没开大模型" line could not be observed. Not faked.
