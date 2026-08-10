# Fix 1 — 会话名由原始按键字节拼成，控制字符会写进终端

来源：`final-review-report.md` 的 Finding 1（Important，四条合并阻塞项之一）。
判定为 **Important**：它命中的是 README 明确记录的常见路径（没配 `[llm]`），
而且是一条通往用户终端的注入路径。

## 缺陷链条

`collect_first_input`（`src/session.rs:202`）记录的是 `send_input` 收到的**原始
字节**。在**附着视图**里每一次按键都被单独转发（`src/ui/attach.rs:193` 调用
`key_to_input`，`src/ui/mod.rs:874`），产出的是：

| 按键 | 送出的字节 |
|---|---|
| Backspace | `\x7f` |
| 上/下/左/右 | `\x1b[A` … `\x1b[D` |
| Esc | `\x1b` |
| Home / End / PageUp / PageDown / Delete / Insert | `\x1b[…` |
| Ctrl+a…Ctrl+z | `\x01`…`\x1a` |

README 原话：**「Inside a session every keystroke goes to the agent, `Esc` included.」**

所以一个改过错字的用户，`first_input` 会变成
`"fix teh\x7f\x7f\x7fthe login bug"`。

`request_name`（`src/session.rs:899`）把兜底名字**一点没洗**就钉死了：

```rust
let fallback: String = s.first_input.chars().take(NAME_MAX_CHARS).collect();
*recover(s.name_slot.lock()) = Some(fallback);
```

没有 `clean_name`、没有 trim、没有控制字符过滤。而 `clean_name`
（`src/session.rs:136`，只作用于**模型**输出）会去引号、去尾部标点，
**但同样不过滤控制字符**。

## 为什么它能到达终端

三个环节，报告已逐个对着 `ratatui-0.28.1` 的源码核实过：

1. `char_width`（`src/ui/widgets.rs:127`）对控制字符返回 `0`，所以 `truncate`
   既不丢弃它们、也不把它们算进宽度预算 —— 它们**原样穿过截断**。
2. `Span::render_ref`（ratatui `src/text/span.rs:396-400`）**不丢弃零宽字素**，
   而是把它 append 到前一个单元格的 symbol 上。
   （`Buffer::set_stringn` 确实过滤控制字符，`Paragraph` 也跳过零宽 symbol，
   **但看板列表项、九宫格标题、附着视图的块标题走的都是 `Line` → `Span::render_ref`**，
   那条路不过滤。）
3. crossterm 后端把单元格 symbol 原样写出：`queue!(self.writer, Print(cell.symbol()))`。

结论：名字里嵌的 `\x1b[A` 就是**每一帧都往看板里发一次的光标上移命令**。

## 用户会遇到什么

- **最常见**：没配 `[llm]`（README 称之为「正常情况，不是问题」），于是兜底名
  **就是**最终名字。用户在附着视图里打第一句话、退格改掉一个错字、回车。
  看板上从此永久显示一个含 `\x7f` 的乱名字。**没有改名的办法。**
- **更糟**：用户在打字前按了上箭头（调历史）或 Esc（agent 自己的弹窗要用），
  名字以一个真的转义序列开头，**看板每一帧重绘都被打乱**。
- 只输入空白（一个空格加回车）会产生一个**非空但不可见**的 tag。
  `session_label` 只看 `!tag.is_empty()`，于是返回那个空白 tag，
  原本显示 agent 种类的那一列**永久变空**。
- `first_input` 也是塞进 `name_prompt` 的东西，所以模型是拿着一串
  `\x7f` 在给会话起名。

## 安全性

`name_prompt` 把 **agent 屏幕**的最后 2000 个字符喂给模型，而那块屏幕可能含有
agent 从仓库或网上取回的内容。被恶意屏幕内容操纵的模型可以返回至多 24 个
`clean_name` 不会清洗的字符，走同一条不过滤的渲染路径。**有界，但这是一条
通往用户终端的注入路径。**

## 要改什么

1. `append_capped`（`src/session.rs:227`）跳过 `char::is_control()` 的字符。
   **更好的做法**：把 `\x7f` / `\x08` 当作「弹掉上一个字符」，让记录下来的文本
   与用户真正想打的话一致。
2. 同一个过滤也要作用到 `clean_name` 的输出，**模型那条路也得覆盖**。
   最干净的落点是一个共享的 `sanitize`，作用在 `request_name` 里的**两处写入**上。
3. 洗完之后**重新检查兜底是否为空**：清洗后的兜底若为空，
   **`name_slot` 保持 `None`**，让之后一次真正的输入还能给这个会话起名，
   而不是永久钉死一个看不见的名字。

## 测试要求

- 断言一个含 `\x1b[A` 和 `\x7f` 的 tag **永远不会到达渲染出的 buffer**。
  这是本条修复的核心回归测试。
- 退格语义：`"fix teh\x7f\x7f\x7fthe"` 记录下来应当是 `"fix the"`（若采纳弹出语义）。
- 只输入空白 → `name_slot` 仍是 `None`，之后一次真正的输入还能起名。
- `clean_name` 的输出同样过滤（模型返回带控制字符时）。

## 全局约束

- **本简报里的引用代码不是权威**，是意图说明。照抄之前先想它对不对。
- **收尾动作是变异测试，不是「测试全绿」**：把过滤条件取反、把空值检查删掉、
  把 `sanitize` 的某一处调用去掉，各跑一次测试。**没有测试失败 = 测试没写够，回去补。**
- 已知陷阱（上一轮 ledger 记的）：**那六个命名测试在负载下退化成「假绿」而不是
  「假红」** —— 它们需要一个 tick 落进 0.2 秒的窗口，而轮询间隔是 50ms。
  **别被绿骗了**，新写的测试不要依赖同样的时序。
- 每个用户能看到的字符串写给没编过程序的人：不用黑话、不给栈追踪、不给原始 OS 错误。
- key 处理分支里永远不要 `continue`。
- 提交信息用英文，**不要 AI 署名行**。
- 收尾跑：`cargo test -- --test-threads=1`、`cargo fmt --check`、`cargo clippy --all-targets`。
- 测试不碰网络、不碰真实 `~/.dct`。
