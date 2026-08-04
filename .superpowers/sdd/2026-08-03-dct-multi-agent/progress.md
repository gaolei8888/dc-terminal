# SDD ledger — plan: docs/superpowers/plans/2026-08-03-dct-multi-agent.md
Task 1: complete (commits 1c8b1de..214ae97, review clean)
Task 1: minor (deferred): fake_agent() 的 Profile 字面量在 src/session.rs / tests/concurrency.rs / tests/slow_input.rs 三处重复，加字段就要改三遍；先前就有的模式，未来再加字段时考虑抽共享 fixture
Task 2: complete (commits 214ae97..8719a17, review clean)
Task 2: minor (deferred): Profile::builtins() 用 filter_map 会静默丢掉解析不了的内置项；有测试兜底，但 expect 会更早暴露
Task 2: 实跑验证结果 —— codex v0.146.0 已验证（命令与 esc to interrupt 都对），claude 已装；opencode / qwen 本机未装，pattern 按约定留空；四个 base_url 需要真 key，未验证，设计文档的「未实测项」表已如实记录，无需改动
Task 3: fix round 1/5 (2 addressed, 1 open — describe_toml_error 仍可能吐多行：toml::de::Error::message() 自身可含换行，如非法转义序列；commits 4059948..c673212)
Task 3: minor (deferred): load_dir 的 filter_map(|e| e.ok()) 静默丢掉单个 DirEntry 失败；use std::path 插在文件中部；权限测试的 assert 失败时不会恢复 0o700
Task 3: fix round 2/5 (1 addressed, 0 open; commits c673212..9264a85)
Task 3: complete (commits fc25970..9264a85, review clean)
Task 4: fix round 1/5 (1 addressed, 0 open; commits a781cbe..8941ee0)
Task 4: complete (commits f1a867f..8941ee0, review clean)
Task 4: minor (deferred): 回滚只测到「原本没这条」分支；「恢复原值」分支靠代码审读，load_error 守卫拦掉所有写，测不出「先写成功再写失败」
Task 4: minor (deferred): bail! 把底层 io/toml 错误原文插进用户可见文案；Disk 的 Default derive 没人用；tmp 文件名固定，并发 set 会撞
Task 5: fix round 1/5 (1 addressed, 0 open; commits 405c6ee..e58da5a)
Task 5: complete (commits a746e81..e58da5a, review clean)
Task 5: minor (deferred): daemon 现在按请求里的原始 profile 名查密钥，而不是 resolve 之后的 Profile.name；当前九个内置和 register_profile 路径都保证两者一致，但这是新引入的耦合
Task 6: fix round 1/5 (1 addressed, 0 open; commits d874a04..7789467)
Task 6: complete (commits b9f68d6..7789467, review clean)
