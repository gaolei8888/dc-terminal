## Global Constraints

- 提交信息用英文；**不加** `Co-Authored-By` 或任何 AI 署名行（用户规矩）。
- 推送前必须 `cargo +1.99.0 clippy --all-targets -- -D warnings` 干净（CI 用最新 clippy）。
- 代码注释、面向用户的文字用中文，跟周围代码一个风格；面向用户的话要大白话，不出现 pty、SIGTERM 之类术语。
- dct 只放通用代码：**不许**在 Rust 里按 profile 名字（`"claude"`、`"codex"`）分支，能力一律由 profile 字段声明。
- 没实测过的 profile 不许声明恢复参数（`profile.rs` 里 `unverified_profiles_declare_no_resume_args` 那条规矩同样适用于新字段）。
- 新增 `Request` 变体要把 `PROTOCOL_VERSION` 从 24 升到 25（`src/proto.rs:145`），`Hello` 永远不改形状。
- 测试不许碰真实的 `~/.dct`：路径一律跟着 socket 走（同 `last_sessions_path_for_socket` 的模式）。
- **不许**在实施过程中重启用户真实的 dct 后台、不许装二进制到 `~/.local/bin`——用户有 18 个在跑的会话。真机验收只在 Task 10 用单独的 socket 做。
- 默认等待：`wait` 15 分钟，`nudge_wait` 3 分钟；重启后投递等空闲最多 2 分钟；`__swap` 等旧后台退出最多 10 秒；CLI 不来问进度超过 10 秒算取消。
- **本计划里的参考代码不是权威**：行号和函数名以实际源码为准，写之前先核对；发现计划和源码对不上，以源码为准并在报告里写明。


## Process rules for every implementer
- Work only in /Users/lei/work/dc/dc-terminal/.worktrees/graceful-restart (branch feat/graceful-restart). Never cd to the main checkout.
- Commit messages in English, NO Co-Authored-By / AI attribution lines.
- Run focused tests while iterating; full `cargo test` once before committing; `cargo +1.99.0 clippy --all-targets -- -D warnings` must be clean before you report.
- Never touch the user's real dct daemon, ~/.dct, or ~/.local/bin.
- Plan reference code is NOT authoritative: verify names/lines against source; when they differ, follow source and note it in the report.
- Do not dispatch subagents.
