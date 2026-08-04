# Task 11 报告：填密钥界面

## 实现内容

`src/ui.rs`：

- `View::EnterSecret { profile, label, prompt, buf, phase }` —— 新视图，从 `PickProfile` 选中「未填密钥」的行进入。
- `pub enum SecretPhase { Typing, Verifying, Failed(String) }`。
- `pub fn clean_secret(s: &str) -> String` —— 去首尾空白、脱引号（双/单）、脱 `Bearer ` 前缀，逐一验证顺序正确（含引号+Bearer+换行混在一起的用例）。
- `pub fn verify_message(o: VerifyOutcome) -> Option<String>` —— `Ok` 放行；`BadKey` 说密钥，不带状态码；`Unreachable` 说网络，不牵连密钥。
- 主循环 `run()`：
  - 新增局部 `verify_rx: Option<Receiver<VerifyOutcome>>` 和缓存的 `socket: PathBuf`（`socket_path()` 是纯函数，比从 `Client` 私有字段掏路径省事）。
  - 循环顶部（`term.draw` 之前）`try_recv` 一次：命中就清 `verify_rx`；仅当当前视图仍是 `EnterSecret` 才应用结果——通过则 `SetSecret` → `Create` → 直接进 `Attached`；失败则落回 `Failed(msg)`。
  - `PickProfile` 里 `PickAction::AskSecret(_)` 分支不再占位提示，改为用调用方拿到的真实下标 `i`（`AskSecret(usize)` 那个参数本身是占位，见其文档注释）从 `entries[i]` 取 `name`/`label`/`secret` 组出 `EnterSecret`。
  - `EnterSecret` 的按键分支：`Verifying` 阶段只认 `Esc`（退回选择器并清 `verify_rx`），其余按键原样忽略；`Typing`/`Failed` 阶段认 `Esc`（退）、`Enter`（开后台线程验证）、`Backspace`/字符输入（编辑，顺带把 `Failed` 的旧错误清成 `Typing`）、`Ctrl+O`（`open` 申领页链接，不动 `phase`）。
  - 粘贴分支加 `View::EnterSecret` 一支，`clean_secret` 洗一遍，`Verifying` 期间不接受粘贴。
  - 渲染加 `View::EnterSecret` 一支：密钥永远显示成圆点；`phase` 决定要不要多画一行「正在验证…」（青色）或失败原因（红色）；`prompt.url` 存在时提示 `Ctrl+O`。
  - `escape_hint` 加 `View::EnterSecret { .. } => "Ctrl+Q 回列表"`（与 `PickProject` 手输态共用同一句文案，13 列，不超 `ESCAPE_HINT_COLS`）。
  - `idle_help` 加两支：`Verifying` 时提示「正在验证，请稍候　Esc 可取消」，其余提示「粘贴或输入密钥　Enter 确认　Esc 返回列表」。
  - `back_one_level` 加 `View::EnterSecret { .. } => Some(View::PickProfile { entries: vec![], .. })`，文档注释写明这是个空壳，调用方必须补一次 `Request::Profiles`。
  - **新增（超出 brief 原文的一处修复，见下方"实现中发现并修的一个 bug"）**：把「发现空 `PickProfile` 就重新拉取」的逻辑从 Ctrl+Q 分支里挪出来，放到整个按键处理块之后统一收口，覆盖 `EnterSecret` 自己的本地 `Esc` 分支（它不走 `back_one_level`，直接手搭了同一个空壳）。

## 测试

TDD：先写测试，确认编译失败（RED），再实现（GREEN）。

### RED

命令：`~/.cargo/bin/cargo test --lib ui`

在只加入 9 个测试函数（brief 写的是"八个"，实际数出来是 9 个 `#[test]`，全部按原文实现）且未添加任何新类型/函数时运行，输出（节选）：

```
error[E0425]: cannot find function `clean_secret` in this scope
    --> src/ui.rs:2442:20
error[E0425]: cannot find function `verify_message` in this scope
    --> src/ui.rs:2465:17
error[E0599]: no variant named `EnterSecret` found for enum `ui::View`
    --> src/ui.rs:2484:41
error[E0433]: cannot find type `SecretPhase` in this scope
    --> src/ui.rs:2492:20
```

编译直接失败——这是预期的 FAIL：测试引用的 `clean_secret`/`verify_message`/`View::EnterSecret`/`SecretPhase` 在这一步都还不存在。

### GREEN

命令：`~/.cargo/bin/cargo test --lib ui`

```
test result: ok. 62 passed; 0 failed; 0 ignored; 0 measured; 82 filtered out; finished in 0.01s
```

（62 = 原有 53 条 ui 测试 + 9 条本任务新增）全部通过，含全部 9 条 brief 里的测试：`paste_is_trimmed`、`paste_strips_surrounding_quotes`、`paste_strips_bearer_prefix`、`paste_leaves_a_normal_key_alone`、`bad_key_gets_a_human_message`、`unreachable_blames_the_network_not_the_key`、`ok_has_no_message`、`secret_view_escapes_back_to_the_picker`、`secret_view_escape_hint_says_back_to_the_list`。

### 补充测试（自查阶段加的，超出 brief 的 9 条）

自查清单要求验证「密钥比终端还宽时圆点行会不会出问题」，我没有只凭肉眼看，补了两处：

- `draw_does_not_panic_for_all_views` 里加了一个循环，把 `EnterSecret` 的三个阶段（`Typing`/`Verifying`/`Failed`）各画一遍，带真实的 hint + URL，确认不 panic。
- 新增 `secret_view_dots_line_does_not_panic_when_wider_than_the_terminal`：40 列窄终端 + 200 字符的假密钥，确认 `Paragraph` 会正常裁剪，不越界不 panic。

### 全量回归

```
~/.cargo/bin/cargo fmt
~/.cargo/bin/cargo test
```

lib 144 passed；`tests/*.rs` 全部集成测试（cli、client_timeout、concurrency、daemon_detach、daemon_roundtrip、profiles_flow、projects_flow、signal_restore、slow_input、socket_perms）逐个 `ok`；doc-tests 0/0。`cargo clippy --all-targets` 干净，零警告。`git diff --check` 无尾随空白。

## 手动走一遍

用 tmux 起了一个隔离的 `HOME=/tmp/dct-mt-home`（避免碰到这台机器上真实在跑的 dct daemon 和它的项目/密钥数据），`cd` 到一个空 git 仓库，跑 debug build：

1. `n` → 选择器列出九个内置 agent，Kimi 一行显示「（未填密钥）」，编号 5。
2. 按 `5` 进入填密钥视图：标题「填 Kimi 的密钥（Enter 确认，Esc 返回列表）」，hint 行「在 platform.moonshot.cn 开通后复制 API Key」，下面提示「Ctrl+O 打开申领页面」。
3. 打入假密钥 `sk-bogus-fake-key-123`：屏幕上只看到一串 `•`，没有明文。
4. 按 Enter：立刻（约 50ms 内）出现「正在验证…」，底栏左段变成「Ctrl+Q 回列表」，右段「正在验证，请稍候　Esc 可取消」。
5. **在验证仍在飞的时候按 Esc**：界面立刻（毫秒级）切回选择器，且选择器列表是完整的九条（不是空的）——这是本次自查抓到并修掉的那个 bug 的回归证据，见下节。
6. 重新走一遍 4→ 让验证跑完（这个沙箱环境能连到 Kimi 的真实端点）：约几秒后收到 401，界面自动切成红字「这个密钥用不了，可能是复制的时候少了一段」，全程没有卡顿——中途没有再按任何键，说明后台线程独立跑完、主循环全程还在正常刷新。
7. 在 `Failed` 状态下敲一个字符：红字立刻消失，回到正常打字态，圆点数量加一——验证了「编辑清除旧错误」的设计。
8. 再按 Esc：回到选择器，九条仍然都在。

**Esc 在「正在验证…」期间确实生效**，且不需要等验证结束——这正是 trap 2/3 要证明的东西。

## 实现中发现并修的一个 bug

手动测试第 5 步最初复现了一个真实的空白屏 bug：`EnterSecret` 视图里，本地 `Esc`（在 `Typing`/`Failed`/`Verifying` 三个分支里）直接手搭了一个 `View::PickProfile { entries: vec![], .. }`，跟 `back_one_level` 给 `Ctrl+Q` 用的是同一个「空壳」写法。但我最初只把「空列表就重新拉取」的刷新逻辑焊在 `Ctrl+Q` 的处理块里，没意识到本地 `Esc` 也会产出同样的空壳、却没人去刷新它——用 tmux 实测出来，退回选择器后屏幕是空的，九个 agent 一个都不显示。

修法：把刷新逻辑从 `Ctrl+Q` 分支里挪出来，放到整个按键处理块（`if is_ctrl_q {...} else {...}`）之后统一做一次「视图变成空 `PickProfile` 就补一次 `Request::Profiles`」的检查，覆盖所有会产出这个空壳的路径，而不是每加一条新路径就得记得再补一次同样的判断。`back_one_level` 的文档注释和这段代码上方的注释都同步更新，把「谁都可能产出这个空壳」写清楚。

修复后已用同一套 tmux 手动流程重新走了一遍第 5 步，确认选择器完整显示九条。测试方面：`secret_view_escapes_back_to_the_picker`（覆盖 `back_one_level`）和 `secret_view_escape_hint_says_back_to_the_list` 这两条不测 `run()` 里的刷新逻辑本身（`run()` 要连真 socket，单元测试测不到，跟 `back_one_level`/`escape_hint` 抽成纯函数是同一个理由），刷新逻辑的正确性是靠上面的手动 tmux 复现 + 修复验证的，不是自动化测试覆盖的——这是本次实现里没有自动化测试兜底的一块，如果后续要重构这段按键循环，建议先把这个刷新检查也抽成可单测的纯函数。

## 六个陷阱怎么处理的

1. **`View` 要 `Clone`，`Receiver` 不能塞进去**：`verify_rx` 放在 `run()` 的局部变量里，不进 `View`。
2. **验证不能在按键循环线程跑**：`Enter` 时 `std::thread::spawn`，线程内部 `Client::connect(&sock)` 开一条新连接，跟主循环画界面用的 `client` 完全分开。
3. **`Verifying` 期间忽略输入**：`EnterSecret` 按键分支里 `SecretPhase::Verifying` 单独一支，只认 `KeyCode::Esc`，其余原样忽略；`Esc` 时立即 `verify_rx = None`，粘贴分支也判断了 `phase` 不是 `Verifying` 才接受。
4. **`Ctrl+O` 不是 `o`**：guard 写的是 `KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL)`，且这个 arm 排在通用的 `KeyCode::Char(c) => buf.push(c)` 之前，裸 `o` 照常落进密钥缓冲区。
5. **`back_one_level` 是纯函数，拿不到条目列表**：返回 `entries: vec![]` 的空壳，文档注释写明约定；主循环统一收口检测并重新拉取（见上面「发现并修的 bug」一节，这也是本次对 brief 原始设计的一处必要扩展——原文只提到 Ctrl+Q 这一条路径会产出空壳，实际还有 `EnterSecret` 自己的本地 Esc 也会）。
6. **`ESCAPE_HINT_COLS` 是写死的 13**：`EnterSecret` 复用了跟 `PickProject` 手输态完全相同的字符串字面量 `"Ctrl+Q 回列表"`，本来就在 13 列以内，没有新增更长的文案。

## 文件与体量

- `src/ui.rs`：2437 → 2948 行，净增 511 行（实现约 300 行，测试约 211 行）。
- 只改了这一个文件，`git diff --check` 干净。

## 自查发现

- **正确性**：如上，找到并修了「本地 Esc 产出空壳选择器不刷新」的 bug，已用手动 tmux 复现 + 修复验证，全量测试仍绿。
- **安全**：`buf`（明文密钥）从未进入任何 `Debug`/日志/错误消息；唯一走明文的地方是 `Request::SetSecret`/`Request::VerifySecret` 的 IPC payload（协议本身如此，超出本任务范围）和渲染时的圆点计数（`buf.chars().count()`，不暴露内容）。
- **迟到的验证结果**：`verify_rx` 在应用结果前先判断当前视图是否仍是 `EnterSecret`，不是就静默丢弃；`Esc` 路径额外提前清空，`Ctrl+Q` 路径虽不清空但结果到达时视图已经不是 `EnterSecret`，同样被丢弃，逻辑上已覆盖所有退出路径（这一点在报告里单独确认过，`Ctrl+Q` 是全局逃生键，在 `Verifying` 期间依然可用，不受本视图内 `Esc`-only 限制，这是既有设计，不是新引入的例外）。
- **超宽密钥**：专门加了窄终端 + 长密钥的渲染测试，确认 `ratatui::Paragraph` 正常裁剪不 panic。
- **范围纪律**：没有碰 `View::Secrets`/设置页（Task 13）、没有做任何 `n`/`N` 拆分相关的改动（Task 12）；brief 提到的 `return_to_settings` 字段按要求没有加，因为 Task 11 的流程里 `EnterSecret` 只从选择器进入，退出也只回选择器，不需要它。

## 问题与关注点

- brief 的任务描述说"the brief has eight [tests]"，但正文实际列出的是 9 个 `#[test]`；已按 9 个原文照抄实现，不算阻塞项，仅在此指出以免和别的产物对不上数。
- 「空 `PickProfile` 自动刷新」这段逻辑本身没有单元测试覆盖（原因见上，`run()` 要连真 socket），只有手动验证。如果这个仓库后续想让这类主循环副作用也能被单测覆盖，可以考虑把「视图是否需要重新拉取」抽成一个独立的纯函数（类似 `back_one_level`），但那已经超出本任务范围，没有在这次一并做。
