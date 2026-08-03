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
