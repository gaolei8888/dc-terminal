# dct 识别 agent 出错 —— 设计

**状态：** 已实现，待 review
**位置：** 换项目重做五步（D → A → B → C → E）的最后一步

## 问题

agent 在 PTY 里失败时，dct 一无所知。用户实际撞到的是这一句：

```
API Error: Connection closed mid-response. The response above may be incomplete.
```

会话还活着、进程还在、屏幕上还有输入框——`idle_pattern` 匹得上，于是 dct 把它标成
「空闲」。用户以为它在等自己，其实那一轮已经废了。

**这是「一屏管好几个 agent」这种工具最贵的失败模式**：你以为在跑，其实早就断了，
而你要等到下次去看那一格才发现。九宫格越有用，这个坑越深——正因为你不必盯着每一个。

## 范围

**要做：** profile 里声明 `error_pattern`；命中时进一个新的失败态；这个态在列表和
九宫格里都看得出来；**刚出错的那一刻主动说一句**，不用等用户自己去翻。

**不做：**

- **不自动重试、不自动恢复。** dct 不知道那一轮做到哪了，替用户重发是在赌。
- **不给没见过错误文案的 agent 硬编正则。** 见「哪些 profile 有」一节。
- **不做错误历史/日志页。** 状态 + 一句提示就够用户走到那个会话去看原文。

## 架构

### 状态

```rust
pub enum SessionState { Working, Asking, Idle, Stopped, Failed, Unknown }
```

`tick()` 的判定顺序加一档，**`Failed` 排在 Working/Idle 前面**：

```
Stopped → 进程没了 → Asking → Failed → Working / Idle
```

排在前面是因为出错时屏幕上**同时**有错误和输入框提示，`idle_pattern` 一样匹得上。
反过来的话，最要紧的那个事实会被一句「空闲」盖掉——那正是现在的 bug。

**匹的是当前可见屏幕**（`screen_text()`，跟 busy/idle 同一份文字，只扫一遍）。
所以错误滚出屏幕之后状态自然回到空闲/干活中——这是对的：状态描述的是「现在屏幕上
是什么」，用户往下走了、错误翻过去了，它就不该再报警。

### 提示

**在界面进程做，不改协议。** `App::set_sessions` 本来就同时拿得到新旧两份列表，
「谁刚进入失败态」是一次减法。守护进程侧不需要记「通知过没有」这种状态——
那会引出「通知给谁」的问题，而它可能同时服务多个界面。

一次转变只说一次：还在失败态里的会话不会每轮都再喊一遍。

### 哪些 profile 有 `error_pattern`

**只给见过真实错误文案的那些写。** 凭想象编正则会造出误报，而误报比不报更糟：
一个好端端的会话被标成失败，用户会跑去看一个根本没出事的东西，然后不再相信这个标记。

现在只有 Claude Code 那条文案是实见的（用户撞到的那句），所以：

| profile | 有没有 | 理由 |
|---|---|---|
| `claude` | 有 | `API Error` / `Connection closed mid-response` 实见 |
| `kimi` / `glm` / `deepseek` / `qwen-api` | 有 | 它们的 `command` 就是 `claude`，界面完全一样 |
| `codex` / `opencode` / `qwen` | **没有** | 没见过它们的错误文案。缺 `error_pattern` = 这个功能对它关着，行为跟改之前一模一样 |

补别的 agent 时，把它真实的错误行贴进对应的 `profiles/*.toml` 即可，不用改 dct。

## 测试

1. `error_pattern` 能从 TOML 解析出来，编译成正则
2. 没写 `error_pattern` 的 profile 行为不变（不会误判成失败）
3. 屏幕上出现错误文案 → 状态变 `Failed`
4. **错误和空闲提示同时在屏幕上时，判的是 `Failed`**（顺序的关键）
5. 错误滚出屏幕后回到正常态
6. 已停止的会话不会被改成 `Failed`
7. 列表里失败态有自己的文字和颜色（红）
8. 九宫格里失败的格子边框是红的
9. 新进入失败态 → 底栏说一句，点名是哪个会话
10. 同一个会话连续两轮都失败 → 只说一次
11. 会话从失败态恢复 → 不说话（没有「恢复了」的提示，那是噪音）
12. 状态文案两种语言都有

## 影响面

| 文件 | 改动 |
|---|---|
| `profiles/*.toml` | 五个 claude 系加 `error_pattern` |
| `src/profile.rs` | `error_pattern` 字段 + `error_regex()` |
| `src/session.rs` | `SessionState::Failed`；`tick()` 的顺序 |
| `src/i18n.rs` | 状态文案 + 提示文案 |
| `src/ui/widgets.rs` | `status_label` / `status_style` |
| `src/ui/app.rs` | `set_sessions` 里做转变检测 |
| `src/ui/grid.rs` | 失败格子的边框 |
