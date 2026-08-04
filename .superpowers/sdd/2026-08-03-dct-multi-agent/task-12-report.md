# Task 12 报告：n 直连上次的 agent，N 才进选择器

## 实现内容

1. **`pub fn quick_start_target(last: Option<&str>, entries: &[ProfileEntry]) -> Option<String>`**（`src/ui.rs`，紧挨在 `pick_action` 之后、`digit_index` 之前）
   与 brief 给的实现逐字一致：`last` 为 `None` 直接 `None`；否则在 `entries` 里找名字匹配且 `status == ProfileStatus::Ready` 的那条，返回它的名字。密钥被删、CLI 被卸、自定义 profile 被删/改坏都不是 `Ready`，一律回退到 `None`（进选择器）。

2. **看板按键分支** `KeyCode::Char('n') | KeyCode::Char('N')`（原来只有 `'n'` 一支，替换在原位置）：
   - 先调一次 `Request::Profiles`，拿到 `entries`/`warning`——n 和 N 共用这一次网络往返。
   - 只有按的是小写 `n` 才再调 `Request::LastProfile`；大写 `N` 直接把 `last` 设成 `None`，保证「N 一定进选择器」不受上次记录影响。
   - 用 `quick_start_target(last.as_deref(), &entries)` 判断能不能直开：
     - `Some(name)` → 调 `Request::Create { dir, profile: name, remember: true }`，成功则 `view = View::Attached(id)`（同 `PickAction::Start` 分支的落点）；失败（`Response::Error` 或其它）则设错误消息并落回 `View::PickProfile`（用已经拿到的 `entries`/`warning`，不用再拉一次）。
     - `None` → 落到 `View::PickProfile`（新建/选中第一项）。
   - 把「新建 `ListState` + 选中第 0 项 + 组装 `View::PickProfile`」这段原来只在一处出现的代码，抽成分支内的局部闭包 `picker`，因为这次它在三处落点（选择器为空、Create 失败两种）都要用到，抄三遍风险更大。
   - 拿不到 profile 列表的兜底分支（`Ok(Response::Error(e))` / 其它）**没有用 `continue`**——只是设 `message`，`view` 保持 `View::Board` 不变，正常走到循环尾部的 `message_after_transition`。因为 `view_changed == false`，该函数会原样保留这条消息（见其文档注释里"视图没变：原样保留消息"那一条），效果和 brief 草稿里想用 `continue` 达到的效果完全一样，但不会有跳过收尾逻辑的副作用。加了一段注释解释为什么这里不用 `continue`。

3. **看板 `idle_help`**：
   ```
   "n 新建  N 换 agent  p 换项目  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动"
   ```
   `q 退出` 仍由 `escape_hint` 单独管，未动。

4. **README.md / README.zh-CN.md**：按键表拆成 `n`（直连上次）和 `N`（选 agent）两行；"## Agents" / "## agent" 段落里"按 n 会列出全部"改成"按 N"，并补一句"第一次选完之后 n 就记住了它"。安装完成后的提示"装完按 Ctrl+Q 回看板再按 N"本来就是大写 N，不用改。

## 是否依赖了集中式的空 `PickProfile` 重拉

**没有**，是有意的。`n`/`N` 分支自己就要一次真实的 `Request::Profiles` 结果——n 要拿它判断上次那个 agent 现在是不是 `Ready`，N 要拿它渲染选择器——这份 `entries` 从头到尾都是非空的正常返回（daemon 目前总有九个内置 profile），所以走的是"直接组装带内容的 `View::PickProfile`"这条路，跟 `back_one_level`/`EnterSecret` 的 Esc 分支那种"给个空壳等下一轮统一重拉"的模式不是一回事。之前 `KeyCode::Char('n')` 单独一支的写法本来就是这样（我只是把它推广到 n/N 共用），集中重拉的检查（`src/ui.rs:927-959`）只会在 `entries` 真的返回空表时才触发，两条路不冲突。

## TDD 证据

### RED

命令：
```
~/.cargo/bin/cargo test --lib ui
```
输出（节选）：
```
error[E0425]: cannot find function `quick_start_target` in this scope
    --> src/ui.rs:2848:13
     |
2848 |             quick_start_target(Some("kimi"), &entries),
     |             ^^^^^^^^^^^^^^^^^^ not found in this scope
... (4 处同样的错误)
error: could not compile `dct` (lib test) due to 4 previous errors
```
先加测试、`quick_start_target` 还没实现，编译期就失败——四个用到它的测试全部落空；`board_help_mentions_both_n_and_capital_n` 那条断言当时还没跑到（同一编译单元先挂了），符合预期的"先失败"。

### GREEN

命令：
```
~/.cargo/bin/cargo test --lib
```
输出（尾部）：
```
test result: ok. 149 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.12s
```
单独确认新增的五条：
```
~/.cargo/bin/cargo test --lib quick_start
test ui::tests::quick_start_falls_back_when_the_last_agent_is_gone ... ok
test ui::tests::quick_start_uses_the_last_agent_when_it_is_ready ... ok
test ui::tests::quick_start_falls_back_on_first_ever_run ... ok
test ui::tests::quick_start_falls_back_when_the_last_agent_is_no_longer_usable ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 145 filtered out

~/.cargo/bin/cargo test --lib board_help
test ui::tests::board_help_mentions_both_n_and_capital_n ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 148 filtered out
```

全量：
```
~/.cargo/bin/cargo test
```
结果：所有 crate（lib + 9 个集成测试文件）全绿，`149 passed` in lib，集成测试各自 1-5 条全 ok，无失败无忽略。

`cargo fmt -- --check` 干净；`git diff --check`（对已提交的 commit `git show --check`）无告警。

## 手动走一遍

用 `~/.cargo/bin/cargo build` 编译 debug 版，在 `tmux` 里起了一个短路径的隔离 `HOME`（`/tmp/dct-manual-home`——Unix socket 路径长度有限制，scratchpad 那条路径太长会导致 `path must be shorter than SUN_LEN`），跑全新守护进程：

1. **首次开局，`n`（回退路径）**：看板上按小写 `n`——因为这是全新 daemon，没有 `LastProfile` 记录——直接弹出了「选 agent」选择器，九项列表都在，`Claude` 因为没装密钥/CLI 而置灰。符合"第一次永远进选择器"的预期。
2. 选了第 9 项「命令行」（`shell` profile，不需要真实 API key，安全地验证机制），成功进入「会话 2」（这是这台机器上第二次跑，第一条是之前测试留下的编号）。
3. `Ctrl+Q` 回看板，看板底部提示已经是新的 `n 新建  N 换 agent  p 换项目 ...`。
4. **关键验证**：按小写 `n`——**没有弹任何菜单**，直接进入了新的「会话 3」（同样是命令行 shell），标题栏显示"会话 3 · .../multi-agent —— F2 返回看板"。这就是 quick start 生效：daemon 记住了上次用的是 `shell`，这次直接复用。
5. 再 `Ctrl+Q` 回看板，按大写 `N`——即便上次的 `shell` 依然 `Ready`，还是照常弹出了完整的九项选择器，证明大写 `N` 不查 `LastProfile`、永远进选择器。

两条路径（quick start 直开 + 首次/大写回退到选择器）都跟预期一致。测试完毕后杀掉了 dct 进程、tmux 会话，清理了 `/tmp/dct-manual-home`。

## 自查结论

- **完整性**：`quick_start_target`、n/N 分支、看板 `idle_help`、两份 README 都改了；五条测试全部按 brief 原文加上（`entry` 助手函数已存在，未重复添加）。
- **质量**：`quick_start_target` 命名和行为一致；n/N 分支里用局部闭包收敛了三处重复的 `ListState` 初始化，没有引入不必要的顶层辅助函数（怕跟已有的集中重拉逻辑打架，克制在分支内部）。
- **纪律**：没有碰 `View::Secrets`/`c` 键（那是 Task 13 的范围），没有碰 `.superpowers/sdd/.gitignore`，commit 只 `git add` 了三个改动文件，没有用 `git add -A`。
- **测试**：先写测试→确认编译期失败（RED，函数不存在）→实现→全绿（GREEN），过程干净，没有中途混用没有测过的代码路径。
- **`continue` 陷阱**：确认过，新代码里所有分支都没有用 `continue`；拿不到 profile 列表那支只是设 `message`、让 `view` 保持 `Board` 不变，正常流到循环尾部走 `message_after_transition`，行为等价于 brief 草稿想用 `continue` 达成的效果，但不会跳过后面的收尾逻辑。
- **"n 首次运行" / "Create 失败后用户去哪"**：手动验证了首次运行 `n` 会回退到选择器（不是静默失败或崩溃）；代码里 `quick_start_target` 返回 `Some` 之后如果 `Request::Create` 失败（`Response::Error` 或连接失败），会设错误消息并把用户放回一个已经填好内容的 `View::PickProfile`——用户能立刻看到出错原因，并且还能直接在选择器里再选一次，不会卡在一个无法操作的空白视图。

## 文件改动

- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/ui.rs`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/README.md`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/README.zh-CN.md`

提交：`a771e45 feat: n 直连上次的 agent，N 才进选择器`

## 遗留问题

无。范围内的验收点（quick_start_target、n/N 分支、看板 idle_help、两份 README、五条测试）均已完成并验证；未涉及 Task 13 的密钥设置页。
