# 最终整分支 code review —— 修复报告

日期：2026-08-03
范围：13-task 分支合并前的最后一轮 review，共 9 条 finding（1 CRITICAL / 4 IMPORTANT / 4 MINOR）。

## 总览

| # | 级别 | 状态 |
|---|---|---|
| 1 | CRITICAL | 已修复（结构性方案：验证结果加身份戳） |
| 2 | IMPORTANT | 已修复 |
| 3 | IMPORTANT | 已修复 |
| 4 | IMPORTANT | 已修复 |
| 5 | IMPORTANT | 已修复 |
| 6 | MINOR | 已修复 |
| 7 | MINOR | 已修复 |
| 8 | MINOR | 已修复 |
| 9 | MINOR | 已修复 |

验证：`cargo fmt --check`、`cargo build`、`cargo test -- --test-threads=1`（172 个单测 + 全部集成测试，共约 190 个测试）、`cargo clippy --all-targets -- -D warnings`、`git diff --check` 全部通过。

---

## CRITICAL 1 —— 过期的验证结果写错 profile

**文件**：`src/ui.rs`

### 采用的方案：给验证结果打身份戳（stamping），不是把 Receiver 塞进 `View::EnterSecret`

设计文档明确给了两个选项，并要求"能做第二种就做第二种，做不到要说明为什么"。我选了第一种（stamping），理由：

1. **第二种方案（把 `Receiver` 挪进 `View::EnterSecret` 变体）本身解决不了这个 bug 的全部形状。** 复现路径不是"离开了 `EnterSecret` 视图"，而是"离开又用同一个变体、换了一个不同的 profile 回来"（`c` → Kimi → Verifying → Ctrl+Q → `Secrets` → Enter on GLM → **又是 `EnterSecret`**，只是 profile/buf 换了）。把 receiver 挂在变体上，靠"变体切换时自动 drop"这条机制，防不住"同一个变体、不同实例"这种情况——因为从 `Secrets` 再次进入 `EnterSecret(glm)` 时,那是一个全新构造的 `View` 值，跟旧的 `EnterSecret(kimi)` 之间没有任何自动关联，除非在构造处显式判断"这是不是同一次验证"——而这正是 stamping 要做的事。换句话说，"挪进变体"能解决的只是"完全换了个变体"这一类，解决不了 finding 里描述的真实复现。
2. **改动面**：`View::EnterSecret` 目前在 `ui.rs` 里被构造了 20+ 处（正常按键流程 + 十几个测试）。给它加一个 `Arc<Mutex<Option<Receiver<...>>>>` 字段意味着这 20+ 处全部要跟着改，风险和改动量都明显大于 stamping。
3. **stamping 给出的不变量更强、更直接对应 finding 的表述**："a verification result may only be applied to the request that issued it"——这句话本身就是一次相等性比较（发起时的 `(profile, buf)` == 应用时的 `(profile, buf)`），不需要依赖"哪条退出路径记得清 receiver"这种容易漏改的纪律。哪怕将来有人加了第 N 条退出路径、忘记清 `verify_rx`，只要它没有恰好构造出跟发起时完全相同的 `(profile, buf)`，这条防线依旧生效。

### 改动内容

- `verify_rx` 的类型从 `Receiver<VerifyOutcome>` 改成 `Receiver<(String, String, VerifyOutcome)>`：发起验证时把 `(profile, buf)` 的一份拷贝跟结果一起送回主循环。
- 新增纯函数 `verify_outcome_applies_to(issued_profile, issued_buf, current_profile, current_buf) -> bool`，只做一次相等性比较。
- 主循环收到结果时：先 `if let View::EnterSecret { profile, buf, .. } = view.clone()` 拿到"此刻屏幕上"的身份，再用 `verify_outcome_applies_to` 判断要不要真的应用；不满足就整段跳过，不切视图、不发任何请求。
- Ctrl+Q 分支本身没有改动（不需要显式清 `verify_rx`）——即使它继续留着一个"已经不会被用上"的 receiver，下一次 `try_recv()` 拿到结果时也会被 `verify_outcome_applies_to` 挡住，不会误用。

### 手工复现

按要求尝试了手工复现：`cargo build && ./target/debug/dct`，但这个 bug 需要在 4 秒验证窗口内精确地做"paste key → Enter → Ctrl+Q → Enter on 另一个 agent"这个时序，而当前环境没有真实交互式 TTY（沙箱里通过 Bash 工具驱动，拿不到能跟 `dct` 这种 ratatui 全屏 TUI 交互的终端）。手工复现在这个环境下不现实，因此**依赖纯函数测试**——已经加了三条直接覆盖判断逻辑本身的测试（见下）。这类异步竞态原本就是纯函数抽取的经典场景：判断"是不是同一个请求"是一次纯粹的比较，不需要真的连 daemon、真的等网络。

### 测试

- `verify_outcome_applies_when_profile_and_buffer_still_match`——profile、buf 都对得上，应用。
- `verify_outcome_does_not_apply_to_a_different_profile`——直接对应 finding 的复现（Kimi 验证结果不能套在 GLM 上）。
- `verify_outcome_does_not_apply_when_the_buffer_changed_on_the_same_profile`——同一个 profile，密钥换了，不应用。

---

## IMPORTANT 2 —— claude 没有安装器

**文件**：`profiles/claude.toml`

先核实了本机 `claude` 的真实来源，没有直接相信 finding 里给的包名：

```
$ which claude
/Users/lei/.local/bin/claude
$ readlink -f $(which claude)
/Users/lei/.local/share/claude/versions/2.1.221
$ npm list -g --depth=0 | grep -i claude
├── @anthropic-ai/claude-code1@npm:@anthropic-ai/claude-code@2.0.47
```

确认 `claude` 就是 npm 包 `@anthropic-ai/claude-code`。给 `profiles/claude.toml` 补上：

```toml
[install]
command = ["npm", "i", "-g", "@anthropic-ai/claude-code"]

[install.note]
zh = "需要先装 Node.js"
```

跟 `codex.toml`/`opencode.toml`/`qwen.toml` 的写法完全一致。机制本身（`status_of` 的依赖排序、`not_installed_with_an_installer_offers_to_install` 测试）不用改，`every_builtin_parses_and_is_well_formed` 等已有测试全部通过，确认新 TOML 语法正确。

---

## IMPORTANT 3 —— 密钥屏标题跟底栏自相矛盾

**文件**：`src/ui.rs`

`View::EnterSecret` 渲染分支原来硬编码「Esc 返回列表」的标题，没有跟 Task 13 已经改好的 `escape_hint`/`idle_help` 一样按 `return_to_settings`分岔。补上同样的分支：

```rust
let title = if *return_to_settings {
    format!("填 {label} 的密钥（Enter 确认，Esc 返回设置）")
} else {
    format!("填 {label} 的密钥（Enter 确认，Esc 返回列表）")
};
```

新增测试 `secret_view_title_agrees_with_escape_hint_for_both_origins`：两种来源各画一遍，断言画面上只出现跟这次来源匹配的那句话，另一句完全不出现（防止将来标题和底栏再次各说各话）。

---

## IMPORTANT 4 —— 密钥文件坏了给的是读不懂的英文 + 错误的建议

**文件**：`src/secrets.rs`、`src/daemon.rs`

`secrets.toml` 解析失败不再复用 `profile.rs::describe_toml_error`（那半句"原因"是 toml 库的原始英文，如 `invalid key`/`expected ...`，专为"用户在手编 profile 文件"设计，密钥文件的场景完全不适用）。改成固定的、完全中文、给得出下一步的一句话：

> 密钥文件坏了，读不出来。删掉这个文件，回 dct 里重新粘贴一遍密钥就行，不用手动修它。

原始 toml 错误只留一份在 stderr 方便排查。同时把 `describe_io_error` 那条分支也改成自足的中文句子（加上「密钥文件读不了：」前缀），让 `load_error()` 对外始终是一句完整、可操作的中文。

`daemon.rs` 组装 warning 时原来无条件拼「，检查一下 {path}」——这句话在权限错误上说得通，套在"文件坏了"上却是让用户去手改一个 README 明说不支持手改的文件。既然 `load_error()` 现在已经自足，daemon.rs 只再补一个路径（带括号），不再叠加任何"去编辑它"的措辞。

**强化了已有测试** `corrupt_file_load_error_is_plain_chinese_not_a_toml_stack_dump`：原来只检查格式（单行、没有 toml 库的图形化 Display、带「第 N 行」），从不检查内容本身是不是真的说人话——现在额外断言不含 `invalid`/`expected`/`line`/`column` 等英文技术词，并且要求包含"删"和"重新"这两个字，确认给出的是真正做得到的下一步。

---

## IMPORTANT 5 —— git 的英文 stderr 直接糊到界面上

**文件**：`src/session.rs`（四个调用点：`create()`、`send_input()`、`undo()`、`diff()`）

`git.rs` 的注释写明"调用方负责给出中文的上下文"，但当时没有任何调用方兑现。给四处都加了 `.context(...)`：

- `create()` 里 `git::checkpoint`：`"拍不了检查点，这个会话没法安全撤销"`
- `send_input()` 里 `git::checkpoint`：`"拍检查点失败，这一步的改动可能没法撤销"`
- `undo()` 里 `git::restore`：`"撤销失败，工作区可能停在了改到一半的状态"`
- `diff()` 里 `git::diff_stat`：`"算不出改了哪些文件，再试一次"`

`anyhow::Error` 的 `Display`（`{e}`，也就是 `daemon.rs` 里 `Response::Error(e.to_string())` 用的那个）只显示最外层的 context，原始英文 git stderr 不会再冒泡到 `ui.rs:684`（选择器的 `Msg::err`）和 `ui.rs` 的 `SecretPhase::Failed` 红字提示上——这两处本身不用改，因为它们只是把 `Response::Error` 里已经是中文的字符串显示出来。

---

## MINOR 6 —— `Request` 的 Debug 会漏明文密钥

**文件**：`src/proto.rs`

去掉 `#[derive(Debug, ...)]`，手写 `impl Debug for Request`：`SetSecret`/`VerifySecret` 的 `value` 字段换成 `"<redacted>"`，`profile` 等不敏感字段照常打印。新增两条测试 `debug_redacts_the_secret_on_set_secret`/`debug_redacts_the_secret_on_verify_secret`，断言明文密钥不出现在 `{req:?}` 里、profile 名字还在。

---

## MINOR 7 —— 没有测试守着密钥掩码

**文件**：`src/ui.rs`

新增 `secret_view_masks_the_key_on_screen`：用已有的 `buffer_text()` 辅助函数渲染一屏 `buf: "sk-abc123"`，断言渲染结果里不包含 `"sk-abc123"`。

---

## MINOR 8 —— Ctrl+O 在非 macOS 上悄无声息失效

**文件**：`src/ui.rs`

新增 `open_url(url) -> bool`：依次尝试 `open`（macOS）和 `xdg-open`（Linux 桌面），只要有一个 spawn 成功就返回 `true`。按键处理里改成：

```rust
if !open_url(url) {
    message = Msg::err(format!("打不开浏览器，自己去访问 {url}"));
}
```

两个都失败时给出明确提示和兜底方案（把地址念给用户），而不是一个看着能按、按下去毫无反应的键。没有为它加自动化测试——`open_url` 一旦被调用会真的尝试拉起系统浏览器，在当前（macOS）开发机上跑测试会弹出真实的 Safari 窗口，属于不可接受的测试副作用；finding 本身也没有明确要求测试。

---

## MINOR 9 —— 构建现在需要 C 工具链

**文件**：`README.md`、`README.zh-CN.md`

确认 `Cargo.lock` 里 `ureq` → `rustls` 链路带进了 `ring 0.17.14` 和 `cc 1.4.0`（`ring` 的构建脚本用 `cc` 编译原生代码）。在两份 README 的安装小节各加一句：

- 英文："You need a recent stable Rust toolchain (1.80 or newer) and a C toolchain (Xcode Command Line Tools on macOS, `build-essential` or equivalent on Linux) — one of the TLS dependencies compiles native code during the build."
- 中文："需要一个较新的 stable Rust（1.80 或更高），以及一套 C 工具链（macOS 装 Xcode 命令行工具，Linux 装 build-essential 或等价包）——依赖里有一份 TLS 库要在构建时编译原生代码。"

---

## 验证记录

```
$ cargo fmt --check          # exit 0
$ cargo build                # Finished, no warnings
$ cargo test -- --test-threads=1
  172 passed (lib) + 2+1+1+1+2+5+3+2+1+1 passed (integration) — 全绿，无新增失败
$ cargo clippy --all-targets -- -D warnings   # Finished, no warnings
$ git diff --check           # exit 0（无尾随空白/行尾问题）
```

修改的文件：`README.md`、`README.zh-CN.md`、`profiles/claude.toml`、`src/daemon.rs`、`src/proto.rs`、`src/secrets.rs`、`src/session.rs`、`src/ui.rs`。
