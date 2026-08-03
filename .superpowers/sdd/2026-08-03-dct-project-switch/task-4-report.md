# Task 4 补丁报告：底部栏收成一行

## 背景

Task 4 主体（`Msg` 类型、按视图给提示、F2 逆转键、标题栏文案）已在 commit
`00b4ce4` 完成并提交。用户看到真实界面后反馈：底部栏占了两行（`当前项目：X\n提示文字`），
要求收成一行。`task-4-brief.md` 已按此要求更新（步骤 3e），本次只补做这一处改动。

## 改了什么

只改了 `src/ui.rs`，两处：

1. **`draw()` 开头的 `Layout::vertical`**（原第 478 行）：
   `Constraint::Length(4)` → `Constraint::Length(3)`。底部框重新收回单行高度。

2. **底部栏渲染逻辑**（`draw()` 末尾，原第 587-613 行）：
   - `help` 字符串不再用 `format!("当前项目：{}\n{}", short_path(current), ...)`
     拼两行，改成只放提示或消息本身（`idle_help` / `message.text`）一行。
   - 当前项目从内容区搬进 `Block` 的边框标题：
     `.title(format!("当前项目：{}", short_path(current)))`。
   - 逻辑保持原来三态：断连 → 断连红字提示；`message` 为空 → 按视图给的 `idle_help`；
     `message` 非空 → 按 `message.error` 决定黑字还是红字。

改动内容与 `task-4-brief.md` 步骤 3e 给出的代码块逐字一致。

## 跑的命令与结果

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --check          # 无输出，无需改动
cargo test -- --test-threads=1
```

全量测试一次性通过：lib 53 passed（含新旧底部栏相关用例：
`bottom_bar_shows_current_project`、`error_message_is_red`、
`bottom_bar_help_follows_the_view`、`disconnected_state_shows_warning_in_bottom_bar`、
`msg_from_str_is_not_an_error`、`f2_is_not_forwarded_but_esc_is`、
`esc_is_forwarded_to_the_agent`、`draw_does_not_panic_for_all_views` 等），
另外 8 个集成测试文件全部通过，包括时序敏感的
`tests/client_timeout.rs::timeout_does_not_desync_the_protocol`（这次串行跑没有触发已知的偶发失败，未见其失败，不需要单独重跑）。

## 测试断言改动

**没有改动任何测试断言。** 原有的底部栏测试断言的都是"buffer 里出现某段文字"这种
弱耦合的方式（`content.contains(...)`），边框标题同样会被渲染进 buffer 的字符流，
所以：

- `bottom_bar_shows_current_project` 断言 `content.contains("dc-terminal")` ——
  现在这段文字来自标题而不是内容区，仍然出现在 buffer 里，照样通过。
- `disconnected_state_shows_warning_in_bottom_bar` 断言不包含"完成"——
  当前项目标题（"/tmp/proj"）不含"完成"，仍然通过。
- `bottom_bar_help_follows_the_view`、`error_message_is_red` 检查的是内容区的
  文字/颜色，不涉及标题，未受影响。

这些测试全部原样通过，不需要修改。

## 自查（`git diff` review）

- diff 只涉及 `src/ui.rs` 两处：`Layout::vertical` 的 `Length(4)` → `Length(3)`，
  以及底部栏那段渲染代码整体替换成单行 + 标题版本。
- 没有改动 `Msg`、按键处理、标题栏文案、`run()` 签名等 Task 4 已完成的部分。
- 工作区里另外还有三个文件处于已修改未提交状态
  （`.superpowers/sdd/2026-08-03-dct-project-switch/progress.md`、
  `task-4-brief.md`、`docs/superpowers/plans/2026-08-03-dct-project-switch.md`），
  这些不是本次改动产生的（本次会话没有编辑过它们），是此前会话/编排流程遗留的
  未提交编辑。按"只改 `src/ui.rs`"的约束，commit 时只 `git add src/ui.rs`，
  没有用 `git add -A`，没有把这三个文件带进本次 commit。它们仍处于工作区的
  已修改未提交状态，留给后续流程处理。

## 遗留疑虑

- 上面提到的三个非 `src/ui.rs` 文件目前仍未提交，是否需要在别的任务里一并提交，
  由调用方决定；本次严格遵守"只改 `src/ui.rs`"没有动它们。
- `dead_code`/`unused_variable` 警告（`expand_path`、`filter_projects`、
  `start_dir`）按 brief 说明是预期状态，本次未处理，等 Task 5 接线后自然消失。
