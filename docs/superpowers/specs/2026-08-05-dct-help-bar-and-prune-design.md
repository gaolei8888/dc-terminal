# dct：底栏按情境收窄 + `dct prune` / `dct kill`

**日期：** 2026-08-05
**状态：** 两部分都已实现，待 review（底栏 `5efc6f1`、prune/kill `f2f0fe7`）

**实现时跟本文的三处出入：**

- **底栏那条门印的是 `? …`，不是 `? 全部按键`。** 用户看到实现之后定的：
  底栏只有一行，「全部按键」四个字要占掉一个真按键的位置，而 `…` 本身
  就是「后面还有」的通用说法。
- **挪进 `?` 的键（`N` / `a` / `c` / `l` / `g`）没有从底栏表里删掉，而是排到了
  尾部。** 底栏改成「按顺序截断」之后，「删掉」和「排在后面」这两件事在
  窄终端上结果一样，在宽终端上后者更好——120 列放得下的时候没有理由不显示。
- **`n 新建` 排在 `s 停止` 之前**，跟本文那张优先级表相反：一个会话都没有时
  `n` 是屏幕上唯一能按的键，它不该排在一串会被隐藏的键后面。

## 起因

用户截了九宫格底栏的图，一句话：「太啰嗦」。截到的是 11 个按键提示折成两行：

```
arrow keys pick a tile  Enter zoom in  i reply  g list  n new  p switch project
a all projects  c keys  u undo  s stop  d changes
```

同一轮里还要求补两条命令：`dct prune`（清掉已停止的会话）和 `dct kill`（强杀）。

两件事共一个 spec，因为它们都在回答同一个问题：**已经不能用的东西不该继续占地方** ——
屏幕上的按键如此，`mgr` 里的 `Stopped` 会话也如此。

## 一、底栏

### 现在的问题

`idle_help()`（`src/ui/view.rs`）给每个视图写死一张按键表，跟当前能不能按无关：

- 一个会话都没有时照样写 `s 停止` `u 撤销` `d 改动` —— 没有会话可停可撤。
- 选中的是 `shell` 会话时照样写 `u 撤销` `d 改动` —— 那两条只对 agent 会话有效
  （`checkpoint_base()` 会返回 `NotAnAgentSession`）。**底栏在说谎。**
- 80 列下右段只有 `80 - 2 - ESCAPE_HINT_COLS(19) - 2 = 57` 列，11 个提示塞不下，
  于是折两行、吃掉内容区。代码里有一串注释在手工权衡「加了这个键就会折到第三行」
  —— 这是把布局约束交给人肉记忆在守。

### 设计

**1）按键表变成有优先级的列表，按宽度自动截断。**

新增 `i18n::help_line_fit(items: &[(&str, Key)], lang, cols) -> String`：按顺序拼，
拼不下就从尾部丢，**永远给尾部的 `? 全部按键` 留位置**。

这一条同时干掉两件事：底栏永远一行（不需要人肉数字宽），宽终端上自动多显示几个键。

**2）按键表按情境生成，不能按的不写。**

`idle_help` 多收一个上下文：

```rust
pub(crate) struct HelpCtx {
    /// 当前选中/聚焦的会话；一个会话都没有时是 None
    pub selected: Option<SelectedSession>,
}
pub(crate) struct SelectedSession {
    pub is_agent: bool,
    pub state: SessionState,
}
```

**看板**（优先级从高到低）：

| 提示 | 出现条件 |
|---|---|
| `↑↓ 选择` | 有会话 |
| `Enter 打开` | 有会话 |
| `s 停止` | 有选中且 `state != Stopped` |
| `n 新建` | 总是 |
| `d 改动` | 选中的是 agent 会话 |
| `u 撤销` | 选中的是 agent 会话 |
| `p 换项目` | 总是 |
| `? 全部按键` | 总是（保留位，永不被截断） |

**九宫格**：同一批，只换三处 —— `方向键 选格`、`Enter 放大`、多一条 `i 回一句`
（排在 `Enter 放大` 之后）。`reply` 开着时不变，仍是 `Enter 发送` `Ctrl+C 打断`。

**挪进 `?` 的键**：`N 换 agent`、`a 全部项目 / 只看本项目`、`c 密钥`、`l 设置`、
`g 列表 / 九宫格`。它们仍然能按，只是不再常驻底栏。

**3）新增 `?` 全部按键浮层。**

`View::Keys { from: Box<View> }`，`?` 开、`Esc` 回来。居中浮层，背后留着看板
（跟项目选择器同一种呈现，`src/ui/pick.rs` 已有这套画法可抄）。内容分三组：

- **移动**：↑↓ / 方向键、Enter、`g` 列表↔九宫格、`i` 回一句
- **会话**：`n` 新建、`N` 换 agent、`s` 停止、`u` 撤销、`d` 改动
- **配置**：`p` 换项目、`a` 全部项目、`c` 密钥、`l` 设置、`q` 退出

浮层里 `c` 的文案写「密钥 / API keys」，不写「keys」—— 底栏的 `? 全部按键` 已经占了这个词。

### 风险与取舍

`g` 是列表↔九宫格的唯一入口，挪进 `?` 后新用户只能靠撞见 `?` 才发现。
已经跟用户确认接受。缓解办法是浮层第一组第一屏就列 `g`。

### 测试

- `the_bottom_bar_is_always_one_line` —— 每个视图 × 每种语言 × 每种 `HelpCtx`
  组合下，`idle_help` 在 57 列内不换行。
- `help_line_fit_never_drops_the_question_mark` —— 宽度压到极小时 `? 全部按键` 仍在。
- `the_bottom_bar_never_offers_undo_on_a_shell_session` —— `is_agent: false` 时
  底栏不含 `u` / `d`。
- `the_bottom_bar_offers_nothing_to_act_on_when_there_are_no_sessions` ——
  `selected: None` 时不含 `s` / `u` / `d` / `Enter`。
- 保留现有的 `the_bottom_bar_never_squeezes_the_grid_off_the_screen`。

## 二、`dct prune` / `dct kill`

### 现在的问题

`Manager::stop()` 只把 `state` 改成 `Stopped`，**没有任何地方把它从 map 里删掉**
（`src/session.rs:363`）。守护进程活得很久，于是 `dct ps` 会越积越多的墓碑，
九宫格和看板要靠 `visible` 过滤去躲它们。

`pty.kill()` 走的是 portable-pty 的 SIGHUP → 约 200ms 宽限 → SIGKILL。
**所以 `dct kill` 跟 `dct stop` 的差别只有「不给那 200ms」**。差别真实但很窄，
已跟用户确认照做。

### 设计

**`Request::Prune` → `Response::Pruned(u32)`**

`Manager::prune()`：两趟，跟 `list()` 一样的锁纪律 —— 先逐个短暂拿会话锁挑出
`state == Stopped` 的 id，再拿 map 锁删。被删的 `Session` 落地时 `Drop` 会兜底回收
子进程（已经死了，是空操作）。返回删掉几个。

**`Request::Kill { id }` → `Response::Ok`**

`PtySession::kill_now()`：取 `process_id()`，`libc::kill(pid, SIGKILL)`，然后
`child.wait()` 收尸，`alive` 置 false。`libc` 已经是依赖（`Cargo.toml:17`）。
拿不到 pid（进程已经没了）不算错误，直接置 `Stopped` 返回 Ok。
`Manager::kill()` 之后把 `state` 置 `Stopped`，跟 `stop()` 一致。

**CLI**

```
dct prune          清掉已停止的会话
dct kill <会话号>  强制杀掉，可以给多个
dct kill --all     强制杀掉全部
```

- `parse_stop_args` 泛化成 `parse_target_args(args, lang, cmd: &str)`，`cmd` 只影响
  用法提示里印的是 `dct stop 3` 还是 `dct kill 3`。`StopTarget` 改名 `Target`。
  对应两条 i18n 文案从 `Key` 常量改成 `msg::needs_a_target(lang, cmd)` /
  `msg::all_takes_no_ids(lang, cmd)`。
- **`dct kill` 不给参数不等于全杀**，跟 `stop` 同一条规矩，理由也同一条：撤不回来。
- `dct kill --all` 的目标是所有 `state != Stopped` 的会话，跟 `stop --all` 一致。
- `dct prune` 输出：清掉了印「清掉 N 个已停止的会话」，没有可清的印「没有要清理的会话」。
- **三条命令都不拉起守护进程**，连不上就如实说没有 —— 跟 `ps` / `stop` 同一条模块规矩。
- `main.rs` 的 `HELP` 补上这两条。

### 测试

- `prune_removes_stopped_sessions_and_leaves_the_rest`
- `prune_on_a_clean_manager_removes_nothing`
- `a_bare_kill_asks_what_to_kill_instead_of_killing_everything`
- `kill_all_takes_no_ids`
- `the_usage_hint_names_the_command_you_actually_typed` —— `kill` 的提示里印
  `dct kill 3`，不是 `dct stop 3`
- `kill_now_leaves_no_zombie` —— 起一个 `cat`，`kill_now()` 后 `is_alive()` 为 false

## 不做

- 不自动 prune。守护进程定时清墓碑会让「刚才那个会话去哪了」变成新问题。
- 界面里不加 prune / kill 入口。看板已经有 `s 停止`，再加两个近义动作只会更糊。
- 不动 `stop` 的语义。
