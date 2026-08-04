# dct 滚屏历史设计

日期：2026-08-04
状态：设计已确认，待实施

## 要解决的是什么

会话里往回翻看不了。今天在 iTerm2 里滚滚轮，终端会按自己的 scrollback 去重绘，
把 dct 画在底部的状态条拽进内容区，画面花掉；滚回底部才恢复。

根因不在渲染，在 `src/pty.rs:91`：

```rust
let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
```

第三个参数是保留多少行历史，传的是 0。滚出屏幕的内容当场丢弃，dct 手里
**一行历史都没有**，所以它对滚轮无话可说，终端就自己拿自己的 scrollback 去糊了。

## 实测前提

以下不是从文档抄的，是抓 PTY 字节流数出来的：

| | 备用屏 | 鼠标上报 |
|---|---|---|
| Claude Code | 用 `?1049h` | 全开 `?1000` `?1002` `?1003` `?1006` |
| codex v0.146.0 | 不用（内联） | 完全没有，只有 `?1004` `?2004` `?2026` |

两个真实 agent 在这两个维度上正好相反。任何「agent 都用备用屏」或者
「内联的才要鼠标」的设计都会挑错一边。

## 一、谁拥有滚动

一条规则，每帧从会话当前状态读，不写死在 profile 里：

```
Screen::mouse_protocol_mode() != None  →  agent 拥有滚动，dct 把滚轮和翻页键转发给它
否则                                    →  dct 拥有滚动，滚自己的 scrollback
```

agent 运行中可以改模式（`?1000h` 随时能发），所以判断跟着会话状态走，
不是启动时定死。

落到四种情况：

| 情况 | 例子 | 结果 |
|---|---|---|
| 要鼠标 | Claude Code | 转发。dct 的缓冲对它本来就是空的（备用屏 scrollback 恒为 0），正合适 |
| 不要鼠标 + 内联 | codex、zsh | dct 自己滚，历史真在缓冲里 |
| 不要鼠标 + 备用屏 | 目前九个内置里没有 | 谁都滚不了。底栏必须说明白，不能装死 |
| 不要鼠标 + 内联但还没攒够历史 | 刚开的会话 | 什么都不做，不提示 |

第三种和第四种在协议上要能区分开，见第三节的 `alt_screen` 字段。

**转发要做的事比听起来多。** dct 开了鼠标捕获后拿到的是 crossterm 的
`MouseEvent`，要重新编码成 agent 要的格式，而且坐标要从终端坐标换算成
agent 画面里的坐标——减掉边框那一圈。编码格式由
`Screen::mouse_protocol_encoding()` 决定（Claude Code 要 SGR）。

**明确砍掉的**：Claude Code 开了 `?1003h`，连纯鼠标移动都要。那是每动一下一个
事件，全部经 socket 转发，量很大，换来的只是悬停高亮。**第一版只转发滚轮和
点击，不转发纯移动。** 这是有意的部分实现，不是遗漏。

## 二、dct 自己那条路

### 白送的两件事

改 `src/pty.rs:91` 的 `0` 为 `2000`，渲染代码一行不动——
vt100 的 `grid.rs:120-125`，`visible_rows()` 自带 `skip(scrollback_len - offset)`，
`screen_spans()` 走的 `Screen::cell()` 最终落到这里。

视图钉住也不用自己写。`grid.rs:556-558`：

```rust
if self.scrollback_offset > 0 {
    self.scrollback_offset = self.scrollback.len().min(self.scrollback_offset + 1);
}
```

每推入一行历史，偏移自动 +1。已经滚上去的时候来新输出，画面不动。

顺带白送「底下有多少新内容」：偏移只在两种情况下变——用户滚，或者新行推入。
在 `Session` 里记下用户上次手动滚完时的偏移 `mark`，`offset - mark` 就是新行数。

> 边界：偏移增长被 `scrollback.len()` 封顶。缓冲满 2000 行之后再来新内容，
> 偏移不再涨，`new_lines` 会少算，画面也会开始往上飘（最老的行被挤掉了）。
> 这是环形缓冲的固有代价，不修。

### 唯一的坑

`grid.rs:551`：

```rust
if self.scrollback_len > 0 && !self.scroll_region_active() {
```

程序设了滚动区（DECSTBM，`ESC [ r`）之后，滚出去的行直接丢弃，**什么都不进
scrollback**。设滚动区的一般是全屏 TUI，而全屏 TUI 通常也要鼠标、会被第一节的
规则路由走——但这不是保证。它落在第一节第三种情况里，靠底栏那句话兜住。

### 缓冲大小

**2000 行，写死，不做成配置项。** vt100 的 `Cell` 约 36 字节，120 列一行约
4.2 KB，满载约 8.4 MB/会话。`VecDeque` 按实际用量增长，2000 是天花板不是
预分配。用户不该被问这个数字。

## 三、状态放哪、协议怎么改

**只能放守护进程。** `scrollback_offset` 是 `vt100::Screen` 上的字段，Screen 在
守护进程里，界面手里只有渲染好的 span。

代价：两个 dct 连同一个会话会互相拽偏移。现实中一次只有一个人看，接受，
不为它加复杂度。

### 新增请求

```rust
Request::Scroll { id: u32, by: ScrollBy },
Request::Mouse  { id: u32, event: MouseForward },

pub enum ScrollBy {
    /// 正数往上翻进历史，负数往下。守护进程钳到 [0, max]，
    /// 界面不用自己算边界——它手上的 max 永远比屏幕状态晚一帧。
    Rows(i32),
    /// 直接回到底部。不用 Rows(i32::MIN) 表达，那种写法一年后没人看得懂。
    Bottom,
}

pub struct MouseForward {
    /// 已经是 agent 画面里的坐标，边框由界面减掉了
    pub col: u16,
    pub row: u16,
    pub kind: MouseForwardKind,   // WheelUp / WheelDown / Down(button) / Up(button)
    pub modifiers: MouseModifiers, // shift / alt / ctrl
}
```

`Scroll` 走 dct 自己的路；`Mouse` 是转发路径——**界面负责换算坐标**（它知道
边框在哪），**守护进程负责编码**（它知道当前是 SGR 还是 X10）。职责按知识
所在地切分，不把 `mouse_protocol_encoding()` 的语义泄漏到界面。

不复用 `Request::Input`：那是发给 agent 的用户输入，塞控制序列进去会让
「打字跳回底部」这条规则变得没法讲清楚。

### 响应带回状态

`Response::Screen` 加一个字段，`ScreenSnapshot` 从元组别名改成结构体：

```rust
pub struct ScrollState {
    /// agent 自己管视口（它开了鼠标上报），滚轮和翻页键要转发给它
    pub agent_owns: bool,
    /// agent 在备用屏上。配合 agent_owns=false 才是「谁都滚不了」
    pub alt_screen: bool,
    /// dct 缓冲里最多能往上翻多少行
    pub max: usize,
    /// 当前往上翻了多少行，0 表示在底部
    pub offset: usize,
    /// 上次用户手动滚之后，底下新增了多少行
    pub new_lines: usize,
}
```

界面据此决定底栏显示什么：

| 条件 | 底栏 |
|---|---|
| `offset > 0 && new_lines > 0` | `↓ 下面还有 N 行新内容` |
| `offset > 0 && new_lines == 0` | `↑ 已往上翻 N 行 · 按 End 回到底部` |
| `!agent_owns && alt_screen` | `这个 agent 自己管画面，翻不了历史` |
| 其余 | 不显示 |

### 契约又变了

老守护进程 + 新界面 = 反序列化失败，跟上一轮「拿不到 agent 列表」是同一个坑。
两手都要：

1. 新字段全部 `#[serde(default)]`，能兜一半；
2. 客户端认出反序列化失败时，错误文案改成说人话的
   「后台服务还是旧版本，重启一下就好」，并给出怎么重启。

第 2 条本来就是待办，跟这件事捆在一起做。

## 四、归零的两个时刻

- **打字**：`SessionManager::send_input` 里先把偏移归零再写 PTY。用户一敲键就
  回到底部，字符照常送进去，不吞。转发过来的鼠标事件走 `Request::Mouse`，
  不经过这里，所以不会误触发。
- **改窗口大小**：`SessionManager::resize` 里归零。vt100 会重排，偏移的含义
  当场失效。直接回底，不提示。

## 五、键位和步长

| | |
|---|---|
| 滚轮一格 | 3 行（终端惯例） |
| `PageUp` / `PageDown` | 一屏减 2 行 |
| `End` | 回到底部 |

这三个都只在 `agent_owns == false` 时由 dct 处理；`agent_owns == true` 时
`PageUp`/`PageDown`/`End` 照旧当普通按键送给 agent，滚轮走 `Request::Mouse`。

鼠标捕获**只在会话里开，退回看板就关**。看板不需要滚，而开着捕获会让终端
原生的选中复制失效——把这个代价限制在真正需要它的地方。
`restore_terminal()` 里无条件加 `DisableMouseCapture`，`TerminalGuard::drop`
和 `spawn_signal_restore` 都走它，所有退出路径自动覆盖。

**这个代价要写进 README。** 会话里选文字复制会失灵，iTerm2 要按住 Option 拖选，
其他终端一般也有类似的修饰键。dct 目前没有自己的复制功能，所以这是真的功能
倒退，不能只在设计文档里提一句。

## 六、怎么测

**`src/pty.rs`（真 PTY，跟现有测试一个路子）**

- 推超过 2000 行，`max` 停在 2000，最老的行确实没了
- 设了偏移之后再推新行，偏移跟着涨——钉住行为是 vt100 给的，但它是这个设计的
  地基，塌了要有测试告诉我们
- 备用屏下 `max == 0`
- 设了滚动区之后推行，`max` 不涨（把 `grid.rs:551` 那条固定下来）

**`src/session.rs`**

- `send_input` 之后偏移归零
- `resize` 之后偏移归零
- `new_lines` 的算法：滚上去 → 推新行 → 读到正确的差值 → 再滚 → 归零

**`src/ui.rs`（纯函数，不起终端）**

- `mouse_action(&ScrollState, MouseEventKind) -> MouseAction`
  ——`Forward` / `Scroll(i32)` / `Ignore` 三分支各一个用例
- `key_scroll(&ScrollState, KeyEvent) -> Option<ScrollAction>`
  ——`agent_owns` 为真时对 `PageUp` 返回 `None`（要让它落到普通按键路径）
- `scroll_hint(&ScrollState) -> Option<String>`——上面那张表四行各一个用例
- SGR 编码函数：滚轮上/下、左键按下/抬起，各比对一次期望字节串

**手工验收**（这两条自动化不了，必须真跑）

- codex 会话里滚轮往上，能看到历史，画面不花，状态条不被拽走
- Claude Code 会话里滚轮往上，是 Claude Code 自己的对话记录在滚，不是 dct 的

## 七、明确不做

- 不转发纯鼠标移动（见第一节）
- 不做鼠标选中复制。dct 自己实现选区是另一个量级的工作，这一版就让用户用
  终端的修饰键拖选
- 不做缓冲大小配置项
- 不做「多个客户端各自独立的滚动位置」
- 不做搜索历史。有了缓冲之后它才谈得上，但它是另一份设计
