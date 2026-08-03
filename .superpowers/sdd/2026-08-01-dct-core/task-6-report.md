# Task 6 报告：TUI 会话看板

## 做了什么

按 brief 的步骤顺序实现了 `src/ui.rs`：

1. 先写 brief 给的失败测试（`status_labels_are_chinese`、`asking_and_working_use_different_colors`），跑 `cargo test ui` 确认因 `status_label`/`status_color` 未定义而编译失败（`E0425` ×6）。
2. 照 brief 给的实现代码原样写入 `run()`、`draw()`、`View`、辅助函数 `selected`/`move_sel`/`act`，`View` 按要求 `#[derive(Clone)]` 并在事件循环里 `match view.clone()`。
3. 跑测试确认通过。
4. 额外加了一个 `draw_does_not_panic_for_all_views` 测试（用 `ratatui::backend::TestBackend`），覆盖看板视图（含消息、空列表）、profile 选择弹窗、已进入会话的屏幕视图，确认 `draw()` 在这几种状态组合下都不 panic。
5. `cargo fmt`，`cargo fmt --check` 通过（fmt 对 `run()` 里两处 `act(...)` 调用做了换行调整，是 rustfmt 自动格式化，不是我手改的）。
6. `git diff --check` 无输出（无空白问题）。
7. 提交。

## 命令与输出摘要

```
$ export PATH="$HOME/.cargo/bin:$PATH" && cargo test ui
error[E0425]: cannot find function `status_label` in this scope   (×4)
error[E0425]: cannot find function `status_color` in this scope   (×2)
error: could not compile `dct` (lib test) due to 6 previous errors
```
（预期中的失败，符合 Step 2 的预期。）

写完实现后：

```
$ cargo test ui
running 6 tests
test ui::tests::asking_and_working_use_different_colors ... ok
test ui::tests::status_labels_are_chinese ... ok
test profile::tests::unknown_builtin_is_none ... ok
test profile::tests::builtin_names_lists_both ... ok
test profile::tests::builtin_shell_is_not_agent ... ok
test profile::tests::builtin_claude_uses_bypass_flag ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 21 filtered out
```

`cargo build --release`：

```
Compiling dct v0.1.0 (/Users/lei/work/dc/dc-terminal)
Finished `release` profile [optimized] target(s) in 13.24s
```
无警告、无错误。

加上 TestBackend smoke test 后：

```
$ cargo test ui
running 7 tests
... 全部 ok（含新加的 draw_does_not_panic_for_all_views）
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 21 filtered out
```

`cargo fmt && cargo fmt --check`：`FMT_OK`（无差异）。

`cargo build`（debug）：`Finished dev profile ... ` 无警告输出。

全量测试（`cargo test -- --test-threads=1`）：

```
running 28 tests   (src/lib.rs unittests，含 git/profile/pty/session/ui 全部模块)
test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.14s

tests/concurrency.rs: 1 passed
tests/daemon_roundtrip.rs: 2 passed

Doc-tests dct: 0 passed（没有 doctest，符合预期）
```

合计 31 个测试全部 PASS，前五个任务的模块（git 6、profile 6、pty 4、session 8、集成测试 3）都没有被这次改动跑挂。

`git diff --check`：无输出，退出码 0，无空白/行尾问题。

## 与 brief 的偏差及原因

- 唯一的偏差是在 brief 给的两个测试之外，**额外新增了 `draw_does_not_panic_for_all_views` 测试**（用 `ratatui::backend::TestBackend`）。brief 本身没要求这个测试，但任务说明里明确要求"自己确认代码真的能跑起来"，并建议这类验证"可以保留成正式测试"。我选择保留，理由见下一节。
- 实现代码（`run`/`draw`/`View`/辅助函数）与 brief 给的代码逐字一致，没有改动逻辑；`cargo fmt` 对两处 `act(&mut client, &sessions, &list_state, |id| Request::Undo { id })` 这类调用做了自动换行（把 `Request::Undo { id }` 拆到下一行），是格式化工具的选择，不影响语义。
- 没有修改 `session.rs`、`main.rs` 或任何其他文件，符合 brief"这个任务不需要改 session.rs"的说明；`main.rs` 里把 `ui::run`接入 CLI 属于 task-7 的范围（`task-7-brief.md` 已存在），本任务不涉及。

## 怎么验证 TUI 真的能用，以及为什么这么处理

TUI 本身是交互式的，没法在无人值守的测试里跑通完整的按键循环（`run()` 内部直接调用 `crossterm::event::read()` 阻塞读键盘、`enable_raw_mode()` 需要真实终端）。能做且做了的验证分三层：

1. **编译与类型检查**：`cargo build` / `cargo build --release` 全过，说明 `run()` 里所有对 `client.call`、`Request`/`Response` 变体、`SessionInfo` 字段的用法都和已有模块的真实签名对得上（不是照抄 brief 代码却对不上接口）。
2. **单元测试覆盖的纯函数**：`status_label`/`status_color` 是 brief 要求的两个测试，已跑通。
3. **`draw()` 的 TestBackend smoke test**：这是我自己加的部分。`run()` 里唯一不依赖真实终端事件循环、且逻辑最容易出问题的部分是 `draw()`（三个 View 变体的布局代码、`ListItem`/`Paragraph`/`Line`/`Span` 的构造、空列表场景）。用 `ratatui::backend::TestBackend` 把它单独摘出来渲染到内存缓冲区，构造了：看板视图（有消息/无消息）、看板为空列表（防止空列表导致的下标 panic）、profile 选择弹窗、已进入会话的屏幕视图（含多行文本）。这五种调用全部不 panic，说明 `draw()` 在这些状态组合下是安全的。

   这个测试**没有**覆盖的部分：`run()` 里的事件循环本身（键盘输入分发、`view.clone()` 后的状态转移、`enable_raw_mode`/`EnterAlternateScreen` 这些真实终端 side effect）。这部分只能靠人工在真实终端里跑（需要一个真实的 daemon socket，而 daemon 的 CLI 入口是 task-7 才接的），本次任务范围内没有可以启动的 `dct` 子命令来做端到端验证，所以没有做——这是已知的验证边界，不是遗漏。

   选择保留这个测试而不是删掉，理由：它是回归防护网——以后如果有人改 `draw()` 的布局代码（比如加新字段、调整 chunks），能立刻在 `cargo test` 里抓到 panic，而不用等到真人在终端里试出来。写起来和维护成本都很低（不依赖 client/daemon，纯内存渲染，几毫秒跑完）。

## 自查发现的问题

- 没有发现功能性问题。`draw()` 在空会话列表、多行屏幕文本、中文字符（宽字符）内容下都没有 panic。
- 一个已知的设计层面的观察（非 bug，brief 里也提到过）：`View::Attached` 分支里逐字符调用 `client.call(Request::Input{...})`，如果打字速度快而 socket 往返慢，`event::poll(150ms)` 之后每次 `read()` 只处理一个按键事件，理论上高频输入会被这个 150ms 的轮询间隔限速，但这是 brief 给定的实现，不在本任务修改范围内，仅记录在此供后续任务参考。
- `main.rs` 目前仍是占位符（`println!("dct")`），`ui::run` 还没有被任何地方调用，这是预期的（task-7 负责接线），不算本任务遗留问题。

---

## 审查回来后的修复（第二轮）

审查给了 1 个 Critical、3 个 Important。逐条处理如下。

### 1. [Critical] 循环体内的 `?` 跳过终端恢复

**根因**：`run()` 里 `disable_raw_mode()` / `LeaveAlternateScreen` 写在 `loop { ... }` 后面，循环体内任何一个 `?`（`term.draw(...)?`、`event::poll(...)?`、`event::read()?`）出错都是直接 `return` 出整个函数，根本走不到那两行清理代码；panic 展开同理不会执行。

**修法**：加一个零依赖的 RAII guard：

```rust
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    }
}
```

在 `enable_raw_mode()?` 成功之后、`EnterAlternateScreen`/`Terminal::new` 之前立刻 `let _guard = TerminalGuard;`（这样即便后两步失败，raw mode 也还能被 Drop 恢复）。原来循环后面显式的 `disable_raw_mode()?; execute!(term.backend_mut(), LeaveAlternateScreen)?;` 两行删掉，交给 Drop 统一处理——`?` 提前返回、正常 `break`、panic 展开，三条路径 Drop 都会跑。

**验证方法**：审查者用 Python `pty.openpty()` 起真伪终端跑了一遍，我照同样思路做，但用的是一个临时探针而不是改动 `main.rs`（`main.rs` 接线是 task-7 的范围，不该在这里提前做）：

1. 临时在 `src/ui.rs` 的 `term.draw(...)?;` 之后加一段被 `DCT_UI_TEST_FORCE_FAULT` 环境变量控制的注入代码（`err` 模式 `return Err(...)`，`panic` 模式 `panic!()`）。
2. 临时加了 `examples/pty_probe.rs`：起一个真实 `dct::daemon::run`（临时 socket）+ `Client::connect` + `dct::ui::run(client, default_dir)`，外层包一层 `catch_unwind` 保证探针进程自己总能退出（方便 Python 侧判断子进程已结束）。
3. 写了一次性的 Python 脚本（`/private/tmp/.../scratchpad/pty_verify.py`）：`pty.openpty()` 开一对真伪终端，把 probe 的 stdin/stdout/stderr 全部接到 slave 端跑起来，等子进程退出后用同一个 fd 号 `termios.tcgetattr` 读 lflags，对比 `ICANON`/`ECHO` 在跑之前和跑之后是否一致。

**改之前（临时注释掉 `let _guard = TerminalGuard;`，模拟修复前的行为），`DCT_UI_TEST_FORCE_FAULT=err`：**

```
BEFORE lflags: {'ICANON': True, 'ECHO': True}
AFTER  lflags: {'ICANON': False, 'ECHO': False}
probe output: [?1049h[39m[49m[59m[0m[?25l[?25hprobe: run() 返回错误: 测试注入的错误
RESULT: 终端状态未恢复 (raw mode 残留)
exit_code=0
```

跟审查者报的现象完全一致：`ICANON`/`ECHO` 从 `True` 变成 `False` 之后再也没恢复。确认复现。

**改之后（恢复 `let _guard = TerminalGuard;`），`DCT_UI_TEST_FORCE_FAULT=err`：**

```
BEFORE lflags: {'ICANON': True, 'ECHO': True}
AFTER  lflags: {'ICANON': True, 'ECHO': True}
probe output: [?1049h[39m[49m[59m[0m[?25l[?25h[?1049lprobe: run() 返回错误: 测试注入的错误
RESULT: 终端状态已恢复
exit_code=0
```

**改之后，panic 路径（`DCT_UI_TEST_FORCE_FAULT=panic`）：**

```
BEFORE lflags: {'ICANON': True, 'ECHO': True}
AFTER  lflags: {'ICANON': True, 'ECHO': True}
probe output: [?1049h[39m[49m[59m[0m[?25l
thread 'main' (47281368) panicked at src/ui.rs:117:17:
测试注入的 panic
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
[?25h[?1049lprobe: run() panic
RESULT: 终端状态已恢复
exit_code=0
```

`?` 提前返回和 panic 展开两条路径都验证了终端能正确恢复。

**清理**：验证完之后把三处临时改动全部撤掉——`src/ui.rs` 里的环境变量注入代码块删除、`examples/pty_probe.rs` 整个文件删除（连同空的 `examples/` 目录）、`let _guard = TerminalGuard;` 的临时注释恢复成正常代码。`git status --short` 确认最终只有 `src/client.rs`、`src/ui.rs` 两个文件有改动，没有遗留探针文件。

### 2 & 3. [Important] 连接错误被静默吞掉 / Attached 视图逐字符输入错误被 `let _ =` 吞掉

这两条用同一套机制解决：给 `run()` 加一个 `connected: bool`，每轮循环开头对 `Request::List`（以及 `View::Attached` 下的 `Request::Screen`）的调用结果做一次判定——`Ok(Response::Sessions(_))` / `Ok(Response::Screen(_))` 才算 `connected = true`，其它一律 `connected = false`。这个值是当前唯一的“连接是否正常”的真相来源，每一轮循环都会在 `term.draw` 之前重新算一遍。

`draw()` 新增一个 `connected: bool` 形参：

- 断连时 List/PickProfile/Attached 三个视图的边框都改成红色（`border_style`），看板标题追加"（连接已断开，数据可能已过期）"，Attached 视图标题追加"（连接已断开，画面可能过期）"。
- 底部提示栏：断连时无条件显示"守护进程连不上，界面数据可能已过期"，盖过任何残留的旧 action 消息（比如上一次按 `u`/`s`/`d` 留下的"完成"字样），避免用户盯着一句过期的成功提示误以为一切正常。

`View::Attached` 里逐字符/回车发送不再用 `let _ = client.call(...)` 完全丢弃错误：改成 `if client.call(...).is_err() { message = "守护进程连不上，'{c}' 没发出去" 或 "...那次回车没发出去"; }`。注意这里没有再单独维护一份 `connected` 状态（一开始写了 `Ok(_) => connected = true, Err(_) => connected = false`，但 `cargo build` 报了 `unused_assignments` 警告——因为下一轮循环顶部的 `List`/`Screen` 判定必然先于下一次 `draw` 执行，会把这里设的值直接覆盖掉，等于是死代码。改成只在失败时写 `message`，`connected` 的计算完全交给循环顶部的探测，逻辑更简单也没有警告。

新增了两个测试：
- `draw_does_not_panic_for_all_views` 扩了两个 `connected=false` 的 case（看板、Attached 视图），确认新增的 `connected` 形参和红色边框逻辑不会 panic。
- 新增 `disconnected_state_shows_warning_in_bottom_bar`：用 `TestBackend` 渲染一次断连状态，把 buffer 逐 cell 读出来拼成字符串，断言里面包含"守护进程连不上"且不包含旧的"完成"提示。这里有个小坑：ratatui 给宽字符（每个汉字占 2 个 cell）后面那个 cell 塞的是 `Cell::reset()` 产生的单个空格 `" "`，不是空串，所以逐 cell 拼出来的字符串里每个汉字后面都夹了一个空格（"守 护 进 程..."）。测试里对拼出来的字符串和目标子串都做了「去掉所有空白字符」的归一化处理再比较，避免误判。同时把 `buf.get(x, y)`（ratatui 0.28 里已 deprecated）换成不告警的 `buf.cell((x, y))`。

### 4. [Important] `Client::call` 没有读超时

`client.rs` 的 `connect()` 里，`UnixStream::connect` 成功之后立刻 `stream.set_read_timeout(Some(READ_TIMEOUT))?`，`READ_TIMEOUT` 设为 5 秒（一个模块级 `const`）。超时之后 `read_line` 会返回 `io::Error`（`WouldBlock`/`TimedOut`），走的是 `call()` 里已有的 `.context("守护进程没有回应")` 这条错误路径，不需要再改 `call()` 本身——这条错误现在会被 `run()` 里的 `connected=false` 分支接住，显示成"守护进程连不上，界面数据可能已过期"。

超时值选 5 秒的依据：`tests/concurrency.rs` 里那个用 8000 个文件的仓库故意制造的慢 `Create`，实测跑了两次分别是 `1.008s` 和 `1.049s`，5 秒留了将近 5 倍余量，不会把这个合法的慢操作误判成超时断连；同时 5 秒对真实断连场景也不算长到让用户干等太久。

### 二轮修复后的验证结果

```
$ cargo build 2>&1 | tail -5
   Compiling dct v0.1.0 (/Users/lei/work/dc/dc-terminal)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.36s
```
（无警告。）

```
$ cargo fmt && cargo fmt --check
FMT_OK
```

```
$ cargo build --release
   Compiling dct v0.1.0 (/Users/lei/work/dc/dc-terminal)
    Finished `release` profile [optimized] target(s) in 2.50s
```

```
$ cargo test -- --test-threads=1
running 29 tests   （src/lib.rs：git 6 + profile 6 + pty 4 + session 9 + ui 4 = 29）
test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.10s

tests/concurrency.rs:
running 1 test
test list_is_not_blocked_by_slow_create ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.07s

tests/daemon_roundtrip.rs:
running 2 tests
test daemon_serves_create_list_and_stop ... ok
test unknown_session_returns_error_not_panic ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s

Doc-tests dct: 0 passed
```

合计 32 个测试全部 PASS（比第一轮多了 1 个：新增的 `disconnected_state_shows_warning_in_bottom_bar`）。`concurrency` 和 `daemon_roundtrip` 两个集成测试都没有被读超时改动影响；单独用 `--nocapture` 复核了一次 `concurrency`，`create_elapsed=1.049476917s`，远小于 5 秒超时。

`git diff --check`：无输出，退出码 0。`git status --short` 最终只有 `src/client.rs`、`src/ui.rs` 两个文件被修改，没有探针文件残留。

### 与本轮审查要求的偏差

无实质偏差。唯一需要说明的一点：审查建议的验证方式是"先跑一遍确认能复现，改完再跑确认恢复"，但由于 guard 修复和这次验证是在同一轮里连续做的，我采用的做法是——先完整应用修复，再临时注释掉 `let _guard = TerminalGuard;` 这一行来还原"修复前"的行为、跑一遍确认复现，然后取消注释、重新构建、再跑一遍确认修复生效——而不是先跑一个真正意义上"从未加过 guard"的历史版本。效果等价（两次跑的是同一份探针代码、同一个 fault-injection 机制，唯一变量就是 guard 是否生效），但流程上是"临时禁用再启用"而不是"先于修复之前跑"，在此如实说明。

### 顾虑

- 读超时 5 秒是一个经验值，不是从需求文档里抠出来的精确数字；如果以后真实场景里出现比 8000 文件仓库更慢的操作（比如更大的仓库、更慢的磁盘），5 秒可能不够，需要重新评估。
- "连接断开"和"请求本身返回业务错误（`Response::Error`）"目前被合并成同一个 `connected=false` 状态处理，没有区分"守护进程没反应"和"守护进程活着但拒绝了这次请求"。审查要求里没有要求区分这两种情况（"保持简单,不要做重连退避之类的复杂机制"），所以按最简单的方式处理了，但如果后续有真实场景需要区分，这里可能要再拆一次。
