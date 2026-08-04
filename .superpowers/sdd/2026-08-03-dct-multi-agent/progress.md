# SDD ledger — plan: docs/superpowers/plans/2026-08-03-dct-multi-agent.md
Task 1: complete (commits 1c8b1de..214ae97, review clean)
Task 1: minor (deferred): fake_agent() 的 Profile 字面量在 src/session.rs / tests/concurrency.rs / tests/slow_input.rs 三处重复，加字段就要改三遍；先前就有的模式，未来再加字段时考虑抽共享 fixture
Task 2: complete (commits 214ae97..8719a17, review clean)
Task 2: minor (deferred): Profile::builtins() 用 filter_map 会静默丢掉解析不了的内置项；有测试兜底，但 expect 会更早暴露
Task 2: 实跑验证结果 —— codex v0.146.0 已验证（命令与 esc to interrupt 都对），claude 已装；opencode / qwen 本机未装，pattern 按约定留空；四个 base_url 需要真 key，未验证，设计文档的「未实测项」表已如实记录，无需改动
