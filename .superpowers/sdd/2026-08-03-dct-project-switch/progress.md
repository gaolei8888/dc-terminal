# SDD ledger — plan: docs/superpowers/plans/2026-08-03-dct-project-switch.md

branch: feat/dct-core（非 main，已有 24 个提交的既有特性分支）
toolchain: rustup 装在 ~/.cargo，所有 cargo 命令前需 `export PATH="$HOME/.cargo/bin:$PATH"`
spec: docs/superpowers/specs/2026-08-03-dct-project-switch-design.md

pre-flight 扫描：
- Task 3 与 Task 4 会各留下预期内的 dead_code / unused_variable 警告（新函数尚无调用点），
  Task 5 接线后消失。两个任务的 Step 4 都只跑 `cargo test`，不跑 `cargo clippy -- -D warnings`，
  与 Global Constraints 不冲突——clippy 只在「完成标准」处把关。已在计划正文写明，
  且明确禁止用 `#[allow(dead_code)]` 消警告。评审若据此提 finding，属计划已裁定事项。
- 无其它任务间矛盾。

注意：`scripts/sdd-workspace` 和 `scripts/task-brief` 每次运行都会把
`.superpowers/sdd/.gitignore` 重写成 `*`。本仓库按用户要求跟踪 ledger 与 brief/report
（只排除 `*.diff`），跑完脚本要改回来。**已跟踪的文件不受 gitignore 影响**，所以控制者
每个任务结束后要 `git add -f` 一次工作区的 .md，之后脚本再怎么重写都不会把它们弄丢。

Task 1: 实现完成 (commit 8263cab, 7/7 测试通过)
Task 1: review — 需求符合性 ❌（仅因越界改动）；Critical 0；Important 1
  （commit 8263cab 夹带了 `.superpowers/sdd/.gitignore` 从 `*.diff` 改成 `*`，
   越出 brief 的 Files 范围，且实测本任务自己的 brief/report/progress 已变成未跟踪）
  模块本身逐条对上 spec：socket 推导路径、canonicalize 失败存原样、20 条截断、
  临时文件 + rename 原子写、load 三种损坏都退化成空列表、Cargo.toml 未改
Task 1: 归因 — 该 .gitignore 改动是控制者跑的 SDD 脚本副作用，实现者 `git add -A` 扫进来的，
  不是实现者的判断失误。已在 fix 指令里写明。
Task 1: minor (deferred): 无「字段类型不对」的显式测试（如 `{"recent": 5}`），逻辑经审阅正确
Task 1: minor (deferred): `store_path_for_socket` 的 None-parent 分支未测
Task 1: minor (deferred): `save()` 在 touch 为空操作（已在队首）时仍重写文件
Task 1: fix round 1/5 dispatched — 恢复 .gitignore 为只排除 *.diff，单独提交
Task 1: fix round 1/5 (1 addressed, 0 open; commits 8263cab..1e22398)
Task 1: complete (commits bb0954f..1e22398, review clean)
Task 1: 后续操作提醒 — review-package 脚本同样会重写 .gitignore。每次跑完 SDD 脚本后
  `git checkout -- .superpowers/sdd/.gitignore` 复原；新产出的 .md 用 `git add -f` 入库。

Task 2: 实现完成 (commit 942a706, 62/62 通过，含 3 个新集成测试)
Task 2: review — 需求符合性 ✅；Critical 0；Important 0
  复审独立验证：store 锁只在 .list() 和 .touch() 两处单语句内持有，mgr.create() 完全返回后才取锁，
  没有跨慢 git 子进程持锁；无裸 .lock().unwrap()；Cargo.toml 未动；session.rs 只改了 recover 可见性
Task 2: minor (deferred): handle() 参数增至 3 个，将来再加状态应考虑上下文结构体而非继续加参数
Task 2: complete (commits c9eeb13..942a706, review clean)

计划修订 (commit 5742970)：用户指出会话视图缺明确操作提示。查证发现 ff1e37d 只改了标题栏文案和
  测试注释，没改按键处理——实际截走的是 Esc（Claude Code 里取消失灵），标题栏宣传的 Ctrl+B 按了没反应。
  用户裁定：逆转键改 F2，Esc 与 Ctrl+B 一律还给 agent；提示改成跟着视图走。两项并入 Task 4
  （Task 4 本就在重写同一段底部栏代码，分开做要改两遍）。spec 也已补记。

Task 3: 实现完成 (commit 4e1b518, 48/48 通过)
Task 3: 计划文本缺陷 — brief 的 filter_projects 测试断言 "WORK" 命中 3 条，实际只有 2 条含 work。
  实现者自行改成 2 并在报告里披露。复审逐条验证：算术正确、大写针匹配小写草堆仍能证明不区分大小写、
  且只改了那一个数字（diff:164），其余断言逐字未动。裁定：修正有效，非缺陷。计划正文已同步修正。
Task 3: review — 需求符合性 ✅；Critical 0；Important 0
  复审具名风险核查：move_sel 委托给 move_sel_n 后，非空列表路径逐字符等价（同一 clamp 公式、
  同一 unwrap_or(0)）；空列表路径由「不动」变成「select(None)」，是 spec 明文要求的契约，
  且修掉了一个潜在的陈旧选中项问题，非回归
Task 3: minor (deferred): move_sel 空列表语义变化值得加一行注释，免得将来看 blame 误判为回归
Task 3: complete (commits 5742970..4e1b518, review clean)

Task 4: 实现完成 (commit 00b4ce4)。原实现者的 report 文件缺失（上一次会话在提交后中断），
  控制者已自行跑过全量测试：只有 tests/client_timeout.rs 偶发失败，单独跑通过，
  是既有的时序敏感测试，与本任务无关。
计划修订 2 (未提交时机: 与 Task 4 修复同批)：用户看到实际界面后指出底部栏占了两行，要求「用一行」。
  裁定：当前项目移到底部框的边框标题里，框内只留一行（提示或消息）。中文双宽，
  「当前项目：~/work/dc/dc-terminal」+ 看板按键表在 80 列里同一行必被截断，标题行本来就空着。
  Task 5 里选中项目后的提示随之从「当前项目：X」改成「已切到 X」，免得和标题重复。
  计划正文 Task 4 步骤 3e 与 Task 5 步骤 3c 已同步修改，task-4-brief.md 已重新生成。
Task 4: fix round 1/5 dispatched — 底部栏改一行（新实现者，原实现者已不在）
Task 4: fix round 1/5 (1 addressed, 0 open — 底部栏收成一行; commits 00b4ce4..a3015e1)
Task 4: review — 需求符合性 ✅；Critical 0；Important 0
  复审独立验证：View::Attached 只截 F2，Esc→"\x1b"、Ctrl+B→"\u{2}" 都确实进 key_to_input 转发给 agent；
  底部栏一行化后四态（断连/错误/普通消息/按视图提示）互斥且各有测试把关，不是同一断言换皮
Task 4: minor (deferred): `let mut current_dir` 目前触发 unused_mut，属计划预期警告但漏列在清单里，Task 5 接线后消失
Task 4: minor (deferred): task-4-report.md 只记了 a3015e1，主体实现 00b4ce4 的过程记录缺失（原实现者会话中断）
Task 4: 偏离已裁定为合理 — bottom_bar_help_follows_the_view 没照抄 brief，第二次 draw 换了新的 TestBackend。
  理由：ratatui 画宽字符只写首格、第二格留旧值，复用同一 backend 会把上一帧残字拼进来产生假阳性。未削弱断言。
Task 4: complete (commits 4e1b518..a3015e1, review clean)

Task 5: 实现完成 (commit 4b18a79, 全套测试通过；client_timeout 仍是既有偶发)
Task 5: review — 需求符合性 ✅；Critical 0；Important 1
  Important: 手输框空输入按 Enter 会静默把项目切回启动目录（expand_path("", base) == base 且 is_dir() 为真）
  复审独立验证：p 键只在看板生效（全文件 Char('p') 仅一处，Attached 只截 F2）；末行「手输路径…」三处口径一致、
  任何过滤词下都在；手输态 filter 只被搬运不被追加；只判 is_dir()，无 git 判断；12 处状态搬运无一漏；
  光标恒不越界（n 恒 ≥1，Enter 用 i >= shown.len() 兜底）；粘贴在会话视图行为等价
Task 5: 偏离已裁定为合理 — draw_does_not_panic_for_project_picker 每段新建 TestBackend，与 Task 4 同一裁定
Task 5: minor (deferred): 手输失败提示用裸绝对路径，同分支的成功路径与列表态都用 short_path，口径不一且可能被截断
Task 5: minor (deferred): 按 p 时补的是 start_dir 而非 current_dir，刚手输切过去、还没建过会话的项目不在列表里
Task 5: minor (deferred): 手输框没有横向滚动，长路径超出面板后右侧连同光标符一起看不见——正撞在「能粘贴长路径」这条设计理由上
Task 5: minor (deferred): KeyCode::Char(c) 不看修饰键，Ctrl+P 也会弹选择器、Ctrl+C 会往输入框塞字母 c（与看板既有 n/q/u/s/d 写法一致，非本次引入）
Task 5: minor (deferred): 按键状态机与新 idle_help 分支零测试覆盖（run() 不抽 reducer 测不了，结构性限制）
Task 5: 待办 — brief Step 5 的 14 条真人手动验证一条未跑，需在真终端由用户执行
Task 5: 计划完成标准冲突（待用户裁定）— `cargo clippy -- -D warnings` 有 4 条：
  session.rs 的 new_without_default / type_complexity（本计划之前就有）、ui.rs draw() 9 个参数
  （Task 4 按计划逐字写成这样）、tests/projects_flow.rs 的 &PathBuf（Task 2 的计划代码）。
  计划的「完成标准」要求 clippy 全绿，但其中两条正是计划自己逐字规定的代码所致。
Task 5: fix round 1/5 dispatched — 空输入按 Enter 改为无操作 + 补 expand_path 空串契约测试
