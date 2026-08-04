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
Task 7: complete (commits 071935a..bde09c6, review clean)
Task 7: minor (deferred): is_exec 用 mode & 0o111，任意 execute 位都算；0700 且属主不是 daemon 的二进制会误报可用。与 which(1) 同样的近似，可接受，值得加一行注释
Task 7: minor (deferred): status_of 对 command = [] 返回 NotInstalled { command: "" }，渲染层要特判，别显示空括号（Task 10）
Task 7: 注意 src/profile.rs 已 842 行（约 490 行是测试模块）；暂不拆，但后续任务再往里加东西要重新评估
Task 8: complete (commits a4fbf71..99dcb24, review clean)
Task 8: minor (deferred): warning 通道把 io::Error / 解析错误的原文（多半是英文 OS 文案，如 "No such file or directory (os error 2)"）直送用户可见红字；Task 10 负责渲染，要在那儿收口
Task 8: minor (deferred): client_timeout.rs 的偶发失败与本改动无逻辑因果，但新增的测试二进制提高了并行负载，可能是诱因
Task 9: fix round 1/5 (1 addressed, 1 new open — 注释仍承诺 DNS 也被兜住；commits 4d8b1de..3fb94b0)
Task 9: fix round 2/5 (内容已补，但开句中文不通；commits 3fb94b0..3f6aa78)
Task 9: fix round 3/5 (1 addressed, 0 open；commits 3f6aa78..9a3c0f5)
Task 9: complete (commits ba845e1..9a3c0f5, review clean)
Task 9: 已知限制（非缺陷，已在注释里写明）：ureq 2.12.1 无法给 DNS 查询设超时（stream.rs:364 自带 TODO），resolver 卡住时探测可能超过 5 秒；缓解是 UI 在后台线程验证，不冻界面
Task 9: 依赖增加 —— ureq + tls 引入 43 个传递 crate（rustls / ring / webpki / idna / icu）
Task 10: fix round 1/5 (2 addressed 1 minor addressed, 0 open; commits c3d6965..dd3db67)
Task 10: complete (commits 8fe647d..dd3db67, review clean)
Task 10: minor (deferred): char_width 的 (ch as u32) > 0x1100 => 2 是粗略近似，部分非 CJK 码位会被误判成双宽；旧逻辑，本轮只是提取成函数
Task 10: minor (deferred): load_dir 里 from_toml 错误的 unwrap_or_else(|| e.to_string()) 兜底目前不可达，但若 from_toml 的包装方式变了会漏原始文案
Task 10: 注意 src/ui.rs 已 2383 行，Task 11/12/13 还要往里加
Task 11: complete (commits 15c67f3..753e415, review clean)
Task 11: minor (deferred): Ctrl+Q 在 Verifying 期间会绕过「只有 Esc 能出去」的限制（全局退一层早于视图 match）；结果丢弃有守卫，不是缺陷，但与 brief 的字面表述不符
Task 11: minor (deferred): 空 PickProfile 的集中重取没有自动化回归测试（逻辑在 run() 里，要真 socket）；设计上靠状态判断而非枚举路径，风险已结构性缓解
Task 11: minor (deferred): Request::Profiles 持续失败时重取没有退避；沿用仓库既有模式，用户仍可 Ctrl+Q 脱身
Task 11: minor (deferred): SetSecret / Create 的 Response::Error 原文直接进 Failed(e) 给用户看，没有净化；沿用既有模式
Task 11: 注意 src/ui.rs 已 2948 行（本任务 +511），视图状态机 / 事件循环 / 纯函数 / draw / 90+ 内联测试全在一个文件；累积债务，留给最终评审判断
Task 12: complete (commits 3c13825..a771e45, review clean)
Task 12: minor (deferred): quick_start_target 的「非 Ready 就回退」只用 NeedsSecret 测过，NeedsDependency / NotInstalled 没覆盖；判定是整体相等比较，风险低
Task 12: minor (deferred): Ctrl+N 会落进和裸 n 相同的分支（看板上除 Ctrl+Q / Ctrl+O 外都不检查修饰键）；仓库既有行为，非本任务引入
