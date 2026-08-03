# dct 逃生路径 —— 设计

**状态：** 已确认，待出实施计划
**前置：** `docs/superpowers/plans/2026-08-01-dct-core.md` 已完成（守护进程、会话、检查点、TUI 看板）

## 问题

用户报告「dct 在跑，我没法退出它」。追下去发现是两个独立的缺陷叠在一起，而且互相放大。

### 缺陷一：逃生提示会被正常操作顶掉

底栏的优先级是「断连提示 > 消息 > `idle_help` 按键表」（`src/ui.rs:904-914`）。消息一旦非空，
整张按键表就消失，包括其中唯一写着怎么退出的那一截。

而消息只在**切视图**时才清（`message_after_transition`，`src/ui.rs:693`），且本次切换的操作结果
会被刻意保留。于是：

- 在看板上按 `p` 换项目，落回看板时底栏显示 `已切到 ~/work/dc/dc-terminal`
- 这条消息是本次切换的结果，不清
- 看板的 `n 新建  p 换项目  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动  q 退出` 整行没了
- **只要不再切一次视图，`q 退出` 这几个字再也不出现**

一个完全正常的操作就能让退出提示永久消失。用户实拍的截图正是这一屏。

会话视图更糟：那儿的提示是 `F2 回看板（回看板后按 n 新建会话）　其余按键都发给 agent`，
而顶掉它的往往是 `守护进程连不上，刚才那次输入没发出去` 这类错误——**出事的那一刻，
唯一的逃生提示正好消失**。

### 缺陷二：信号盖不住，终端被留在 raw mode

`TerminalGuard`（`src/ui.rs:83-99`）已经覆盖提前 `return`/`?`/panic 三条路径，唯独盖不住信号：
SIGTERM 直接终止进程，不展开栈，`Drop` 不跑。

于是用户「退不出去 → 从另一个窗口 kill」之后，原来那个终端窗口停在 raw mode + alternate screen，
回显和行缓冲全关，看上去像第二次卡死。得知道敲 `reset` 才能救回来——而这正是不该要求用户知道的事。

关终端窗口、tmux 杀 pane 走的是 SIGHUP，同样漏。

### 根因不是「键失效」

用户从未按过 F2——他不知道有这个键。所以只加一个他同样不会知道的备用键并不解决问题；
**提示的可靠性和键本身同等重要**，这决定了下面第三节的存在。

## 范围

三件事：信号也能恢复终端；加一个好猜的全局逃生键；让逃生提示不可能被顶掉。

明确不做：改 F2（保留，老用户的肌肉记忆）；抢 Ctrl+C（必须继续透传给 agent，Claude Code 靠它中断）；
双击透传那种隐形状态（`src/ui.rs:469-474` 的注释已经否掉过一次，理由依然成立）。

## 一、终端恢复：清理收成一个函数 + 一条信号线程

### `restore_terminal()`

把 `TerminalGuard::drop` 的两步抽成一个自由函数：

```rust
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(std::io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
}
```

`TerminalGuard::drop` 调它，信号线程也调它。**清理逻辑从此只有一份**——这是选专用线程方案
而不是「在 handler 里直写 `libc::write`」的全部理由，后者会逼出第二份会各自漂移的清理代码。

两步都 `let _ =` 吞错的原因不变：`Drop` 里不能 panic。

### 信号线程

在 `ui::run` 里，**`enable_raw_mode()` 之前**安装：

```
pthread_sigmask(SIG_BLOCK, {SIGTERM, SIGINT, SIGHUP})   // 主线程屏蔽，新线程继承
thread: loop { sigwait(&set) } → restore_terminal() → libc::_exit(128 + signo)
```

装在 `enable_raw_mode()` 之前，跟 `TerminalGuard` 提前构造是同一个理由：装早了无害
（还没进 raw mode 时 `restore_terminal()` 无副作用，多发一次 `LeaveAlternateScreen` 也无害），
装晚了有一个真空窗口。

**为什么是专用线程而不是信号 handler。** handler 里能调的函数必须 async-signal-safe，
crossterm 的 `disable_raw_mode()` 内部要锁一个全局 Mutex 去取原始 termios——信号打断的正好是
持锁的主线程时就死锁。`sigwait` 在**普通线程上下文**里返回，之后跑的是普通代码，
这个约束整个消失。

**为什么不是「置标志位让主循环退出」。** 主循环卡在 `client.call` 上（守护进程死了、socket 不回）时
永远轮不到下一个 tick。而「卡住了所以我要 kill 它」正是要治的场景，标志位方案在这个场景下失效。

**为什么 `_exit` 而不是 `exit`。** `exit` 会跑 atexit 和静态析构，而主线程此刻还在跑自己的事，
两边可能同时清理终端或撞上同一把锁。终端已经在上一行还原好了，`_exit` 立刻走人。

**为什么退出码是 `128 + signo`。** shell 惯例，SIGTERM 是 143。脚本 `kill` 完还能判断死因。

### 两条不影响的事

- raw mode 下 Ctrl+C **不产生** SIGINT（termios 关了 ISIG），所以屏蔽 SIGINT 不影响 Ctrl+C 透传给 agent。
  SIGINT 这条只对外部 `kill -INT` 生效。
- 屏蔽掩码会被子进程继承（`execve` 之后仍然保留），但 TUI 进程在 `ui::run` 之后不 fork 任何东西：
  PTY 全在守护进程里（`src/pty.rs`），而守护进程在 `src/main.rs:60` 就已经拉起，早于
  `src/main.rs:72` 的 `ui::run`。

## 二、Ctrl+Q：全局「退一层」

| 视图 | Ctrl+Q | 原有键 |
|---|---|---|
| `Attached` | → 看板 | F2 保留 |
| `PickProfile` | → 看板 | Esc 保留 |
| `PickProject`（列表态） | → 看板 | Esc 保留 |
| `PickProject`（手输路径态） | → 列表 | Esc 保留 |
| `Board` | 退出 dct | `q` 保留 |

语义就一句：**Ctrl+Q 等同于当前视图的「后退」，一直按就退到头**。在看板上退出不杀会话，
守护进程照常跑，所以误触代价很低。

选 Ctrl+Q 而不是 Ctrl+\\ 的理由：根因是用户猜不到键，`Q = quit` 猜得到，Ctrl+\\ 猜不到。
Claude Code 不占用 Ctrl+Q，代价只是从 agent 手里拿走 `0x11`。

### 必须避开的坑

**crossterm 里 Ctrl+Q 是 `KeyCode::Char('q')` 带 `CONTROL` 修饰。** `PickProject` 的打字过滤靠
`Char(c)` 往 `filter` 累加，所以 ctrl 判断必须排在 `Char` 分支**之前**，否则按 Ctrl+Q 会往
过滤框里塞一个 `q`。同理 `Board` 的 `Char('q')` 分支也要先分辨有没有 CONTROL——虽然两者都是退出，
行为恰好一致，但不能靠这个巧合。

`key_to_input` 对 Ctrl+Q 改返回 `None`。调用点已经拦了，这层是兜底：万一哪天调用点漏改，
也不会把 `0x11` 悄悄发进 agent。

## 三、底栏拆两段

一行横向切开，左段固定宽度且**永不让位**：

```
┌─当前项目：~/work/dc/dc-terminal────────────────────────────┐
│ Ctrl+Q 回看板 │ 已切到 ~/work/dc/dc-terminal               │
└────────────────────────────────────────────────────────────┘
```

- **左段**：文案必须跟第二节的表逐行对上，说什么就真的能做到什么——

  | 视图 | 左段 |
  |---|---|
  | `Board` | `q 退出` |
  | `Attached` / `PickProfile` / `PickProject` 列表态 | `Ctrl+Q 回看板` |
  | `PickProject` 手输路径态 | `Ctrl+Q 回列表` |

  宽度取这三条显示宽度的最大值（`Ctrl+Q 回看板` = 13 列），硬编码成常量。中文双宽按现有
  `truncate`（`src/ui.rs:585`）的同一套规则算，不引新 crate。

- **右段**：优先级维持现状——断连提示 > 消息 > `idle_help`。`idle_help` 里跟左段重复的那一截
  去掉（看板去掉 `q 退出`，会话视图去掉 `F2 回看板`），其余原样。`PickProfile` / `PickProject`
  的 `Esc 取消` 保留——Esc 和 Ctrl+Q 是两个都能用的键，不是重复。超长用 `truncate` 截。

关键改动是 `!connected` 和 message 两个分支**只能吃掉右段**。第一节描述的
「按一次 `p` 就让 `q 退出` 永久消失」在结构上不再可能发生。

`message_after_transition` 不动——消息该不该留是另一件事，跟逃生提示的可见性已经解耦了。

## 测试

**单元**（`src/ui.rs` 的 `#[cfg(test)]`，沿用现有 `View::Attached(1)` 那套 draw 测试写法）：

- `key_to_input(Ctrl+Q) == None`
- 五种视图态各按一次 Ctrl+Q，`view` 落到上表的预期值
- `PickProject` 过滤态按 Ctrl+Q 后 `filter` **没有**多出 `q`（第二节那个坑的回归测试）
- 底栏渲染：在「message 超长」和「`connected == false`」两种情况下，左段文字仍完整存在

**集成**（新建 `tests/signal_restore.rs`，仿 `tests/daemon_detach.rs` 已有的 `portable_pty` 用法）：

- 在 pty 里拉起 `dct`，`kill -TERM`，读回 pty 的 termios 断言 `ECHO` / `ICANON` 已恢复
- SIGHUP 同理

集成这两条是整件事的验收标准——单元测试证明不了「终端真的还回去了」。

## 被否掉的方案

- **Ctrl+C 连按两下退出**：最符合本能，但要从 agent 手里抢 Ctrl+C 的一部分语义，
  且引入隐形的双击状态。`src/ui.rs:469-474` 的注释已经因为同样理由否掉过一次。
- **只修提示，不加键**：改动最小，但 F2 在部分 Mac 键盘/终端下本来就送不到（未开
  「将 F1、F2 用作标准功能键」时 F2 是亮度键），这个风险没消除。
- **进会话时弹几秒提示**：人是卡住的时候才找退路，那时候提示早没了。
- **在信号 handler 里直接 `libc::write` + `tcsetattr`**：不依赖主循环这点是对的，
  但会形成两份各自漂移的清理代码，且要自己把原始 termios 存进 static。专用线程两个好处都占。
- **标志位 + 主循环退出**：主循环卡死时失效，恰好在最需要它的场景下不工作。
