# SDD ledger — plan: docs/superpowers/plans/2026-08-01-dct-core.md

branch: feat/dct-core
toolchain: rustup 安装于 ~/.cargo（shell 配置未改动）；所有 cargo 命令需 export PATH="$HOME/.cargo/bin:$PATH"
pre-flight: 移除了计划里未被调用的 SessionManager::shutdown()（YAGNI，会被 review 判为死代码）

Task 1: 实现完成 (commit 11e361d, 6/6 测试通过)
Task 1: review — 需求符合性 ✅；Critical 0；Important 1（报告自查声称无编译警告，实际有 4 条 dead_code）；Minor 2（cargo fmt 不通过；builtin 用 expect 会 panic，但代码是 brief 原样给出）
Task 1: minor (deferred): Profile::builtin 用 .expect，内置 TOML 损坏会 panic —— 静态嵌入内容且有测试覆盖，风险极低
Task 1: fix round 1/5 dispatched — 修报告准确性 + cargo fmt
Task 1: fix round 1/5 (2 addressed, 0 open; commits 11e361d..a5e907f)
Task 1: complete (commits 22e8cd7..a5e907f, review clean)

Task 2: 实现完成 (commit a8bee5b, 5/5 测试通过)
Task 2: review — 需求符合性 ✅；Critical 0；Important 2（diff_stat 漏报未跟踪新文件，探针验证；remove_worktree 静默吞掉 branch -D 失败，plan 原样代码需人工裁定）
Task 2: minor (deferred): git() 用 from_utf8_lossy 解析路径，非 UTF-8 仓库路径会被静默破坏
Task 2: minor (deferred): diff_stat 对二进制文件记成 added=0/removed=0，与"无改动"无法区分
Task 2: minor (deferred): 报告称用 --test-threads=1 是为避免竞态，理由不成立（各测试用独立 tempdir）
Task 2: 裁定 — remove_worktree 不再删分支（只删 worktree）。理由：产品靠 git 兜底才敢全自动接受权限，不能同时存在静默销毁 agent 工作的路径；且该函数当前无调用点，将来由调用方显式决定。
Task 2: fix round 1/5 dispatched — diff_stat 用 git add -N 覆盖新文件 + 移除 branch -D
Task 2: minor (deferred): diff_stat 里 `let _ = git(add -N .)` 静默吞掉失败，最坏退化为漏报、不丢数据
Task 2: fix round 1/5 (2 addressed, 0 open; commits a8bee5b..b98b487)
Task 2: complete (commits a5e907f..b98b487, review clean)

Task 3: 实现完成 (commit d2c7430, pty 3/3 + 全量 15/15 通过)
Task 3: review — 需求符合性 ✅；Critical 1（无 Drop，丢弃 PtySession 后子进程变僵尸，ps 实测确认）；Important 3（读线程可能永久悬挂；kill() 后可能仍是僵尸；is_alive() 把 try_wait 的 Err 当作存活）
Task 3: minor (deferred): 全模块 .lock().unwrap()，读线程 panic 会 poison 锁导致连锁 panic
Task 3: fix round 1/5 dispatched — 加 Drop 回收子进程 + is_alive 的 Err 语义
Task 3: fix round 1/5 (4 addressed, 0 open; commits d2c7430..d9fbc5e)
Task 3: 复审独立验证 — 删 Drop 后测试立即失败(Z 态)；mem::forget 探针 3→53 线程证明探针有区分力；实际 3→3 无泄漏。归因修正：唤醒读线程的是 master fd 析构的 hangup，非 kill() 本身
Task 3: complete (commits b98b487..d9fbc5e, review clean)

Task 4: 实现完成 (commit 18fe076, 全量 23/23 通过)
Task 4: review — 需求符合性 ✅；Critical 0；Important 1（stop() 不清理 worktree，SessionManager 无 Drop，worktree 目录无限堆积；brief 把 remove_worktree 列为 consumes 但从未调用）
Task 4: 复审探针独立验证三条设计意图全部做对（逐字符不提交/回车+1、undo 不弹栈、Asking 不被 idle 覆盖、非 git 目录真拒绝且不留痕）
Task 4: minor (deferred): 每次回车即使无改动也跑 3 次 git 子进程
Task 4: minor (deferred): Session.checkpoints 只增不减
Task 4: minor (deferred): main.rs 接线后需重跑 clippy，当前 dead_code 掩盖了其他 lint
Task 4: parked — stop() 不清理 worktree —— ruling: 有意为之，不修。会话停掉常常正是出事时刻，删目录等于清现场；分支捞得回代码但捞不回未提交状态。与 Task 2「不删分支」裁定一致。spec 已把「列出遗留 worktree 让用户决定」归到下一份计划。brief 的 Consumes 误列 remove_worktree，属计划文本瑕疵，不影响代码。
Task 4: complete (commits d9fbc5e..18fe076, 1 parked)

Task 5: 实现完成 (commit f853b58, 25/25 通过：23 单元 + 2 集成)
Task 5: review — 需求符合性 ✅；Critical 1（handle() 持锁跨越慢操作：实测 Create 持锁 959ms，并发 List 被卡 878ms，tick 线程同样被拖住）；Important 2（Mutex 无 poison 恢复，一次持锁 panic 会让 daemon 对所有后续请求永久失效但进程不退出；shell profile 不校验目录存在，坏目录返回成功但会话空转）
Task 5: 协议 fuzz 通过 —— 8 类畸形输入（非法 JSON/空行/5MB 超长行/中途断连/NUL 路径）daemon 均正常返回 Error 或不崩
Task 5: minor (deferred): 无自动化测试覆盖并发卡顿、中途断连、畸形长行等被点名场景
Task 5: fix round 1/5 dispatched — 缩小锁范围 + poison 恢复 + 目录校验
Task 5: fix round 1/5 (3 addressed, 0 open; commits f853b58..8038ad6)
Task 5: 复审独立验证 — 临时改回粗粒度锁复现 list_elapsed=755ms 证明测试有区分力；修复后 98.667µs；tick 线程实测未被拖住；12 线程并发 create id 无重复且 worktree 名一一对应；Task 4 的 7 个测试逐字保留仅 let mut m -> let m
Task 5: minor (deferred): pty.rs 仍有 5 处裸 .lock().unwrap()，同类 poison 风险，本轮范围外
Task 5: minor (deferred): create() 中 create_worktree 成功但 spawn 失败会留孤儿 worktree（Task 4 起既有，非本轮引入）
Task 5: complete (commits 18fe076..8038ad6, review clean)

Task 6: 实现完成 (commit 13bbc84, 31/31 通过，含自加的 TestBackend draw smoke test)
Task 6: review — 需求符合性 ✅；Critical 1（循环体内 ? 直接返回函数，跳过 disable_raw_mode/LeaveAlternateScreen，真 pty 实测 raw mode 残留，终端卡死）；Important 4（List/Screen 失败静默吞掉显示陈旧状态；Input 用 let _ 丢错误；Client::call 无读超时会冻结整个 TUI 含 q 键；空闲时仍每 150ms 轮询）
Task 6: minor (deferred): Asking 颜色区分当前走不到，为将来 Bridge 准备
Task 6: fix round 1/5 dispatched — RAII 终端恢复 + 连接错误可见 + 读超时
Task 6: fix round 1/5 (4 addressed; commits 13bbc84..28b0f8e) — 实现者用真 pty 验证 ?-return 与 panic 两条路径改前复现、改后恢复
Task 6: complete (commits 8038ad6..28b0f8e)
Task 7: complete (commit b66a406) — 由控制者本人实现（用户指示），非子 agent
Task 7: 端到端真 pty 验证全绿 — 自动拉起守护进程/看板渲染/q 退出码 0/终端状态恢复/退出 alt screen/无残留进程
Task 7: 探针教训 — pty.openpty() 默认 0x0 需 TIOCSWINSZ；ratatui 宽字符被光标定位转义打断，匹配前必须剥 ANSI
Task 7: minor (deferred): clippy 剩 2 条 —— SessionManager 缺 Default 实现；ui.rs 有个 if 可折叠进外层 match

FINAL REVIEW (against b66a406): 有阻塞项。4 Critical 均探针复现：
 C1 main.rs:40-46 spawn 无 setsid，daemon 与 TUI 同 process group，SIGHUP 一起死 —— 否掉「关窗口不影响会话」
 C2 client.rs:33-41 读超时后协议永久错位（迟到响应留在 socket，之后每次差一格），ui 从不重连
 C3 session.rs:143-188 with_session 持锁跑慢 git + 每次回车都 checkpoint；Task 5 的细粒度锁只覆盖 create()
 C4 session.rs:104 next_id 重启归 1 + worktree/分支故意不清理 => dct/s1 撞名，重启后 100% 失败
 I5 第二个 daemon 静默劫持 socket；I6 仓库本身是 worktree 时开不了会话；I7 PTY 固定 40x120 从不 resize；
 I8 shell 会话永远「干活中」；I9 README「加 TOML 即可」是假的；I11 pty.rs 5 处裸 unlock；I12 socket 默认权限可被同组用户执行任意命令
裁定：pty.rs recover()、回车快路径、clippy 2 条 —— 必须修；其余 Minor 可留
控制者决定：C1/C2/C3/C4/I12 由控制者本人直接修（用户要求提速），不再派子 agent
