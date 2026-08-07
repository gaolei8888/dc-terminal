# Task 8 report: 协议加 Scroll 与 Mouse，版本升到 5

## 什么改了、改在哪

### `src/proto.rs`
- `PROTOCOL_VERSION` 4 → 5，文档注释按已有的"第 N 版：xxx"惯例追加了一条
  第 5 版的说明。
- `use` 里从 `crate::session` 多引入了 `ScrollBy`、`ScrollState`——不在
  `proto.rs` 里另定义一份平行类型。
- `Request` 加两个变体：
  - `Scroll { id: u32, by: ScrollBy }`
  - `Mouse { id: u32, event: MouseForward }`
- 手写 `impl Debug for Request` 补了这两条 arm（穷举 match，漏一个编译不过）。
  `Mouse` 的字段没有密钥/自由文本，直接照常打印，不用脱敏。
- `Response::Screen` 加了 `#[serde(default)] scroll: ScrollState` 字段，
  `lines`/`cursor`/`state` 原样保留。
- `Response` 加了 `Scrolled(ScrollState)` 变体，对应 `Request::Scroll` 的回答。
- 新增 `MouseForward` 结构体（`col`/`row`/`kind`/`shift`/`alt`/`ctrl`）和
  `MouseForwardKind` 枚举（`WheelUp`/`WheelDown`/`Press(u8)`/`Release(u8)`），
  都是按 brief 给的形状照抄，放在 `ScreenEntry` 定义后面。
- `#[cfg(test)] mod tests`：
  - 三条新测试原样照抄 brief 给的名字和测试体：
    `a_screen_response_without_scroll_still_parses`、
    `scroll_requests_survive_a_round_trip`、`mouse_debug_has_no_surprises`。
  - `the_request_shape_is_pinned_to_the_protocol_version`：`all` 里追加了
    `Request::Scroll { id: 1, by: ScrollBy::Rows(3) }` 和
    `Request::Mouse { id: 1, event: MouseForward{...} }`，期望的 JSON 串按
    实际序列化结果更新，版本号从 4 改成 5。
  - `the_session_info_shape_is_pinned_too`：`SessionInfo` 的形状本身没变，
    但这条测试把 `PROTOCOL_VERSION` 也塞进了 `assert_eq!` 的元组里，所以
    单纯升版本号就会让它变红——把期望值从 `4` 改成 `5`，JSON 串不变。

### `src/session.rs`
- `ScrollBy` 补了 `Serialize`/`Deserialize` 派生，并在它上面加了一行注释
  说明原因：`Request::Scroll` 要把它整个嵌进线上请求。这不在 brief 的
  Interfaces 清单里，但 `Request` 要求全体字段都能序列化，`ScrollBy` 原来
  （Task 7 留下的）只有 `Debug, Clone, Copy, PartialEq, Eq`，不加这两个
  派生编译不过。
- `SessionManager` 加了占位实现 `forward_mouse(&self, id: u32, ev: crate::proto::MouseForward) -> Result<()>`，
  委托给 `s.pty.write_mouse(ev)`，注释写明真正的编码逻辑留给 Task 9。

### `src/pty.rs`
- `PtySession` 加了占位实现 `write_mouse(&self, _ev: crate::proto::MouseForward) -> Result<()> { Ok(()) }`，
  标了 `// Task 9 实现` 注释。

### `src/daemon.rs`
- `handle` 的 `Request::Screen` 分支构造 `Response::Screen` 时补上
  `scroll: snap.scroll`。
- 新增两条分派：
  ```rust
  Request::Scroll { id, by } => mgr.scroll(id, by).map(Response::Scrolled),
  Request::Mouse { id, event } => mgr.forward_mouse(id, event).map(|_| Response::Ok),
  ```

### `src/ui/mod.rs`（brief 之外，为了保持编译通过而做的最小改动）
- 唯一一处手写 `match` 了 `Response::Screen { lines, cursor, state }`
  （没用 `..`）的地方，加了 `scroll` 字段后穷举 pattern 编译不过。改成
  `Response::Screen { lines, cursor, state, .. }`，用 `..` 接住新字段，
  注释写明底栏画滚动提示是下一个任务的活，这里不提前实现。仓库里其余
  所有匹配 `Response::Screen` 的地方（`tests/screen_state.rs`、
  `tests/zombie_reaping.rs`、`tests/grid_reply.rs`、`tests/cli.rs`）本来就
  用了 `..`，不用动。

## 测试命令与结果

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -- --test-threads=1
```
`dct` 单测二进制：505 passed; 0 failed。九个集成测试二进制合计 35
passed; 0 failed。总计 540 passed，0 failed。按要求不追究跟基线 536 的
绝对数差异，只核实两件事都成立：全绿，且本任务新加的三条测试
（`a_screen_response_without_scroll_still_parses`、
`scroll_requests_survive_a_round_trip`、`mouse_debug_has_no_surprises`）
确实出现在输出里且通过。

```
cargo fmt --check   # 退出码 0
cargo clippy --all-targets -- -D warnings   # 退出码 0，无警告
```

三项全绿。

## 对线上形状钉子测试做了什么、为什么

`the_request_shape_is_pinned_to_the_protocol_version` 按它自己注释的规则走：
在 `all` 列表里给每个变体各加一条 `Request::Scroll`/`Request::Mouse` 实例，
重新拿 `serde_json::to_string` 跑一遍实际输出，把 assert 里的期望串换成这个
真实输出，版本号从 `4` 改成 `5`。这正是这条测试存在的目的——形状变了但
版本号没跟着变时它会红；这次是"形状变 + 版本号跟着变"，改完之后它验证的是
新形状钉在新版本号上，而不是绕开它。

`the_session_info_shape_is_pinned_too` 我也改了期望的版本号（4→5），虽然
`SessionInfo` 的字段这次完全没动。原因是这条测试的 `assert_eq!` 把
`PROTOCOL_VERSION` 和 JSON 串一起塞进同一个元组比较，单纯升版本号就会让它
挂红——不改的话它会给出一个跟本任务无关的假阳性。这不是绕过测试的意图，
是让它继续验证"数字对得上"这件事。

## brief 跟现实对不上的地方，以及怎么处理的

1. **`ScrollBy` 缺 `Serialize`/`Deserialize`。** brief 说
   "Consumes: `crate::session::{ScrollState, ScrollBy}`（Task 7）"，暗示
   这两个类型拿来就能用。但 `ScrollBy` 在 Task 7 留下的定义只有
   `Debug, Clone, Copy, PartialEq, Eq`——它原来只在进程内部用，没上过线。
   要把它嵌进 `Request::Scroll` 就必须能序列化。这是 brief 没写但代码
   意图（"滚动请求要走 socket"）必然要求的，于是在 `session.rs` 给
   `ScrollBy` 补了这两个派生，没有在 `proto.rs` 建平行类型——跟 override
   里"不要定义平行副本"的要求一致。

2. **`a_screen_response_without_scroll_still_parses` 的旧 JSON 字面量。**
   brief 给的是 `{"Screen":{"lines":[],"cursor":[0,0]}}`，这是 2 版之前
   （`Response::Screen` 还没有 `state` 字段时）的形状——brief 写于
   2026-08-04，"1→2"的版本号也印证了这一点。但 override 明确说
   "`Response::Screen` already exists and already carries `lines`,
   `cursor`, and `state`... `state` must survive"，而 `state` 在当前代码
   里没有 `#[serde(default)]`，是必填字段。照抄 brief 的字面量跑起来直接
   `missing field state`，报错原因跟这条测试想验证的东西（`scroll` 的
   向后兼容）完全无关。我把测试里的 JSON 字面量改成
   `{"Screen":{"lines":[],"cursor":[0,0],"state":"Idle"}}`，只省略
   `scroll`，让失败原因精确对上"新字段缺省"这件事，并在测试上方加了一句
   注释说明为什么跟 brief 字面量不同。断言逻辑（`scroll` 解出来等于
   `ScrollState::default()`）原样保留。

3. **`git add` 清单没提 `src/ui/mod.rs`。** brief 的 Step 8 只写了
   `git add src/proto.rs src/daemon.rs src/session.rs src/pty.rs`，但
   `Response::Screen` 加字段之后 `ui/mod.rs` 里那处没用 `..` 的穷举匹配
   编译不过。这是 brief 写作时假设的匹配方式（或者假设"UI 任务在下一个
   任务里统一处理"）跟当前代码实际写法不一致导致的编译阻塞，不属于
   "抢下一个任务的活"——只加了 `..`，没有让这个分支处理 `scroll`，滚动
   相关的 UI 行为完全留白给下一个任务。这个文件也进了本次提交。

## 我怎么核实新测试不是"怎么写都过"

- `a_screen_response_without_scroll_still_parses`：临时把 `scroll` 字段上的
  `#[serde(default)]` 去掉再跑这一条测试，得到
  `Error("missing field \`scroll\`", ...)`，确认测试在缺省失效时会真的红；
  加回来之后单独重跑通过。这个来回验证了它测的正是
  `#[serde(default)]` 这件事，不是空断言。
- `scroll_requests_survive_a_round_trip` / `mouse_debug_has_no_surprises`：
  两条都是 brief 原样给的测试体，前者要求序列化/反序列化后 `id` 和变体都
  保持不变（漏实现或字段错位会在 `matches!` 上直接失败），后者要求
  `Debug` 输出里出现字符串 `"Mouse"`——如果 `Debug` 实现漏了这条 arm，
  代码根本编译不过（穷举 match），所以它测的其实是"实现了但输出格式没有
  意外携带敏感信息"，跟它注释里说的意图一致，没有再额外验证。

## 版本不匹配提示路径的阅读结论

`src/main.rs` 里 `run()`（约第 112 行）用
`daemon_status(client.protocol()) == DaemonStatus::Stale` 判断要不要走
`offer_to_restart_stale_daemon`。`daemon_status` 本身（`proto.rs`）是纯粹
按数字比较：`Some(v) if v == PROTOCOL_VERSION => Same, _ => Stale`——
不针对任何具体版本号硬编码。这意味着这次把 `PROTOCOL_VERSION` 从 4 提到
5 之后，**旧守护进程（协议 4）连上新界面（协议 5）会被判定为 `Stale`**，
自动走到 `offer_to_restart_stale_daemon`：打印解释性文案，问用户要不要
重启守护进程；答 `y` 就调用 `client::restart_daemon` 换新进程再重连，
答别的就带着旧协议原样进 TUI（功能不全但不会被拦在门外）。这条路径在
Task 8 之前就已经是版本无关的通用逻辑，这次任务不需要，也没有对它做
任何修改——单纯的版本号提升就能被它正确处理。**没有按要求手工跑这个
验证**（会杀掉用户当前活着的会话），以上是纯代码阅读结论。

## 顾虑

- `ui/mod.rs` 里那处 `Response::Screen` 匹配目前用 `..` 吞掉了 `scroll`，
  UI 完全不知道有滚动状态这回事——这是有意留白给下一个任务，但如果下一个
  任务的人只搜索 "scroll" 关键字而不是重新审视这个匹配点，有可能漏掉
  这条已经存在但被 `..` 悄悄吃掉的字段。建议下一个任务开始时先看这一处。
- `MouseForward`/`MouseForwardKind` 的具体编码方式（col/row 是相对整个
  终端窗口还是相对 agent 显示区域，SGR 1006 vs 普通 X10 之类）完全没有
  定义——本任务只搭了线路。这是 brief 明确划给 Task 9 的范围，这里只是
  记录一下，免得下一个任务的人以为"字段都齐了就能直接编码"。
