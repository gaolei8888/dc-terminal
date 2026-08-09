# dct 会话里能复制文字 —— 设计

**状态：** 待实现
**起因：** 2026-08-08 用户报「dc-terminal 目前没办法进行拷贝」，定位在会话视图内。
**关联：** `2026-08-04-dct-scrollback-design.md` —— 鼠标捕获是那一版为了滚轮翻历史引入的，
这一版收窄它的适用范围。

## 问题

贴进会话之后拖不动鼠标选文字，因此复制不了。

机制不是 bug，是滚屏功能的直接代价，而且 `README.zh-CN.md` 里已经写着这件事：
`src/ui/mod.rs` 的主循环一旦进入会话就发 `EnableMouseCapture`，退回看板才关
（`mouse_capture_transition`，判据只有一个——**在不在会话里**）。鼠标上报打开时，
终端把拖拽交给应用，不再做自己的选中，于是选不中也就复制不了。
`src/clipboard.rs` 只负责把图片粘给 agent，不管文字往外拷。

**但这个取舍下得太粗。** `src/pty.rs::write_mouse` 已经在读
`screen.mouse_protocol_mode()` —— 守护进程**知道**每个会话的 agent 有没有真的订阅鼠标：

- **Claude Code** 会发 `ESC[?1000h` 自己抓鼠标，捕获是它要的
- **codex / shell 会话**根本不订阅，dct 开捕获只为了自己的滚轮 ——
  而 `PageUp` / `PageDown` / `End` 已经能翻同一份历史（`src/ui/attach.rs:60-76`）

也就是说，**相当一部分会话是白白牺牲了复制**，换来一个键盘已经能做的事。

## 范围

**要做：**

- 只在 agent 真的订阅了鼠标时才捕获（协议要把这件事报给界面）
- `F4` 复制模式：临时放掉鼠标，底栏明确写着现在是什么状态

**不做：**

- **不做 dct 自己的复制功能**（选区、`y` 复制、写系统剪贴板）。终端自己的选中复制
  是用户已经会的东西，把它还回去就够了；自己实现一套选区要处理双宽字符、
  折行、滚动中的坐标换算，代价远大于收益。
- **不冻结画面。** 复制一段还在滚动的文字确实别扭，但冻结要引入一份快照状态和
  「什么时候解冻」的规则，为一个几秒钟的动作不值。
- **不动 `src/clipboard.rs`。** 那是图片粘贴，跟这件事无关。
- **不为「agent 订阅了鼠标」的会话取消捕获。** 那种会话里 `F4` 是唯一的出路，
  这是设计的一部分，不是遗漏。

## 架构

### 一、一条规则，不是两个开关

```
要不要抓鼠标 = 贴在会话里 && agent 订阅了鼠标 && 不在复制模式
```

现在的判据只有第一项。两个修法都落在这**同一个派生布尔值**上，所以不会出现
「两套逻辑各自开关鼠标、互相打架」——这正是 `mouse_capture_transition` 当初被抽成
纯函数的理由，它只在**翻转**时发转义序列。

```rust
// src/ui/mod.rs
/// 这一帧该不该抓鼠标。三个条件全真才抓。
///
/// 抽成纯函数的理由同 `mouse_capture_transition`：副作用（往 stdout 写转义序列）
/// 没法单测，判断能测——而且判断错了后果不轻，漏关会让用户连拖选复制都做不了，
/// 漏开会让 agent 收不到它订阅的鼠标事件。
fn wants_mouse_capture(attached: bool, agent_subscribed: bool, copy_mode: bool) -> bool {
    attached && agent_subscribed && !copy_mode
}
```

`mouse_capture_transition(was, wants)` 的签名不变，只是第二个参数从
「在不在会话里」换成这个函数的结果。

### 二、agent 订没订阅：**这件事已经在线上了，不用改协议**

初稿说要给 `Response::Screen` 加一个 `mouse: bool`、把 `PROTOCOL_VERSION` 推到 7。
**那是错的。** 这个事实早就在传了：

- `src/pty.rs::view_of` 里 `agent_owns: screen.mouse_protocol_mode() != MouseProtocolMode::None`
- 经 `session.rs::state_of` 进 `ScrollState`
- 随 `Response::Screen.scroll` 回到界面
- 界面每帧存进 `App.scroll`，`src/ui/attach.rs` 已经在五处读它

`ScrollState.agent_owns` 的语义就是「agent 自己攥着鼠标」，跟「要不要抓鼠标」问的是
**同一个事实**。所以这一版：

> **协议不变，`PROTOCOL_VERSION` 保持 6，守护进程一行不改。**

而且共用同一个字段是对的，不只是省事：滚轮该归谁、鼠标要不要抓，本来就必须给出
一致的答案。各读各的判据，迟早会分叉成「dct 抓着鼠标却不肯滚」这种自相矛盾的状态。

**它会中途变，不是开会话时判一次。** Claude Code 弹出可滚动菜单时开鼠标、收起来就关。
会话视图本来就每帧拉 `Screen`，所以这个值天然跟着刷新；`mouse_capture_transition`
只在翻转时动作，agent 反复开关也不会每帧刷屏。

### 三、`F4` 复制模式

会话里所有键都转发给 agent，dct 只留了 `F2` / `F3` / `Ctrl+Q`（见 `src/ui/attach.rs`），
所以新键必须是功能键。选 `F4`：紧挨着已有的两个，且没有 agent 用它做常用操作。

- `App` 加 `copy_mode: bool`
- `F4` 翻转它。**不转发给 agent**，跟 `F2`/`F3` 同列
- 离开会话（`F2` / `Ctrl+Q` / 会话结束）一律复位成 `false`——它是「此刻正在复制」
  的临时状态，不是配置，更不该跨会话粘着
- 复制模式下 dct 自己也收不到滚轮，`PageUp`/`PageDown`/`End` 照常可用

**底栏必须写清楚。** 模式看不见就是下一个「隐形状态」，那正是这个仓库刚花一整轮
改造消灭掉的东西。复制模式下右段整条换成一句醒目提示（「复制模式 · 鼠标已交还终端 ·
F4 退出」），优先级高于滚动提示，低于错误消息。

### 四、要动的文件

**守护进程侧一个文件都不动**（`proto.rs` / `pty.rs` / `session.rs` / `daemon.rs` 全部不变）。
这一版整个活在界面进程里：

| 文件 | 改什么 |
|---|---|
| `src/ui/app.rs` | `copy_mode: bool`（`agent_owns` 走既有的 `App.scroll`，不加字段） |
| `src/ui/mod.rs` | `wants_mouse_capture`；捕获判据换掉；`F4` 分支 |
| `src/ui/attach.rs` | `F4` 不转发；离开会话时复位 |
| `src/ui/view.rs` | 复制模式的底栏文案 |
| `src/i18n.rs` | 新词条，`en:` / `zh:` 各一，并进穷举表 |
| `README.md` / `README.zh-CN.md` | 重写那段「拖选失灵」的说明 |

## 错误处理

- **`Screen` 拉不到**（断连）：`agent_wants_mouse` 保持上一帧的值，不要因为一次失败
  就翻转捕获状态——翻转会往终端写转义序列，断连时反复翻转是最吵的一种失败。
- **旧守护进程**：走现有握手，版本不匹配时提示重启。
- **复制模式下会话结束**：跟正常离开一样复位，用户回到看板时鼠标状态是干净的。
- **`restore_terminal` 不变**：它已经无条件关捕获，不管这次运行有没有开过。

## 测试

- `wants_mouse_capture` 的真值表（8 种组合全覆盖）
- `mouse_capture_transition` 只在翻转时返回 `Some`（既有测试保留）
- agent 中途开关鼠标：连续几帧 `mouse` 变化，断言只在翻转处动作
- `F4` 翻转 `copy_mode` 且**不**产生转发给 agent 的字节
- 离开会话复位 `copy_mode`（三条路径：`F2`、`Ctrl+Q`、会话结束）
- 断连时 `agent_wants_mouse` 不变
- 底栏：复制模式提示压过滚动提示，但压不过错误消息
- 协议：新 `Response::Screen` 的 JSON 快照，与 `PROTOCOL_VERSION` 一起断言

## 破坏性变更

**没有。** 协议不变（见 §二），守护进程不用重启，正在跑的会话不受影响。

> **发布节奏的前提变了。** 用户在 2026-08-08 拍板「两版一起发」，理由是当时以为
> 这一版会把协议从 6 推到 7、分开发就要用户重启两次守护进程。**那个前提不成立**——
> 这一版一次协议变更都没有。
>
> 所以现在只剩 grouping 那一次 5 → 6，什么时候发、跟这一版一起还是先发，
> 是纯粹的排期选择，不再有「省一次重启」这个技术理由。这个决定退回给用户。

## 已知上限

- **agent 自己抓鼠标的会话（Claude Code）里，拖选复制仍然要么按 `F4`、要么按终端的
  修饰键**（iTerm2 是 Option）。这是没法绕开的：那种会话里鼠标事件是 agent 要的，
  捕获着就是对的。
- **codex / shell 会话的滚轮不再翻 dct 的历史**，因为不再捕获。键盘照常。
  这是本设计明确接受的交换：拖选复制是每天都用的，滚轮翻历史键盘能替代。
