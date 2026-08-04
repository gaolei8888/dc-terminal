# dct 滚屏历史 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 dct 会话能往回翻看历史输出，同时把两个压在它下面的地基问题（协议无版本号、`ui.rs` 4061 行）先解决掉。

**Architecture:** vt100 解析器改成保留 2000 行 scrollback，渲染代码不动（`visible_rows()` 自带偏移）。谁拥有滚动由会话当前的 `mouse_protocol_mode()` 每帧决定：agent 要鼠标就把滚轮/翻页转发给它，不要就 dct 滚自己的缓冲。滚动状态住在守护进程（`vt100::Screen` 在那儿），随 `Response::Screen` 回给界面。

**Tech Stack:** Rust 1.80+，ratatui 0.28，crossterm 0.28，portable-pty 0.8，vt100 0.15.2，serde / serde_json。不引入新依赖。

**Spec:** `docs/superpowers/specs/2026-08-04-dct-scrollback-design.md`

## Global Constraints

- 用户不是程序员。每一句用户看得见的话都不能有黑话、栈追踪、操作系统原始报错；**一句没说清下一步该干嘛的错误提示，不算写完**。
- 界面文案一律中文。
- 不用 emoji 当图标。
- 注释解释为什么，不解释是什么。这个仓库的注释密度是刻意的，照着写。
- **按键分支里永远不要 `continue`**。它会跳过循环末尾清理陈旧 `message` 的那段。这个坑踩过一次：`e0ba1ec`。
- scrollback 行数 **2000，写死**，不做配置项。
- `PROTOCOL_VERSION` 只在破坏性改动时 +1；加 `#[serde(default)]` 的新字段不算。
- 每个任务结束前跑：`cargo test -- --test-threads=1`、`cargo fmt --check`、`cargo clippy --all-targets`。三个都要干净。
- 测试不碰网络，不碰真实 `~/.dct`（所有数据路径从 socket 路径推导，测试指向临时目录）。
- 提交信息用中文，`feat:` / `fix:` / `refactor:` / `docs:` 前缀，结尾带
  `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`。

## 文件结构

| 文件 | 职责 | 本计划里的变化 |
|---|---|---|
| `src/proto.rs` | 线上契约 | 加 `PROTOCOL_VERSION`、`Hello`、`Scroll`、`Mouse`、`ScrollState` |
| `src/client.rs` | 单条连接 | 连上之后先握手 |
| `src/daemon.rs` | 请求分发 | 处理三个新请求 |
| `src/restart.rs` | **新建** `dct restart` 子命令 | — |
| `src/main.rs` | 命令行入口 | 加 `restart` 分支 |
| `src/pty.rs` | PTY + vt100 缓冲 | scrollback 2000、滚动 API、鼠标序列编码 |
| `src/session.rs` | 会话生命周期 | `ScrollState` 贯通、输入/改尺寸时归零 |
| `src/ui/mod.rs` | **由 `src/ui.rs` 改名** 终端生命周期 + 主循环外壳 | — |
| `src/ui/app.rs` | **新建** `App`：主循环的全部状态 | — |
| `src/ui/view.rs` | **新建** `View` 枚举与其纯函数 | — |
| `src/ui/widgets.rs` | **新建** 排版与配色小工具 | — |
| `src/ui/board.rs` | **新建** 看板：按键 + 渲染 | — |
| `src/ui/attach.rs` | **新建** 会话视图：按键 + 渲染 + 滚屏 | — |
| `src/ui/pick.rs` | **新建** 选 agent / 选项目 | — |
| `src/ui/secret.rs` | **新建** 填密钥 / 密钥设置页 | — |
| `README.md` / `README.zh-CN.md` | 文档 | 滚屏可用；鼠标捕获让选中复制失效 |

任务顺序不能换：1→2 是地基，3→5 是重构（必须在往会话视图里加东西之前做完），6→10 才是滚屏本身。

---

### Task 1: 协议握手版本号

**Files:**
- Modify: `src/proto.rs`（`Request` / `Response` / 手写 `Debug`）
- Modify: `src/daemon.rs:100`（`handle` 的 match）
- Modify: `src/client.rs`（`reconnect`）
- Test: `src/client.rs` 的 `#[cfg(test)] mod tests`（新建，这个文件目前没有测试）

**Interfaces:**
- Consumes: 无
- Produces: `pub const PROTOCOL_VERSION: u32`（`src/proto.rs`）；`Request::Hello { version: u32 }`；`Response::Hello { version: u32 }`。Task 8 会把 `PROTOCOL_VERSION` 从 1 改成 2。

- [ ] **Step 1: 写失败的测试**

在 `src/client.rs` 末尾新增：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::PROTOCOL_VERSION;
    use std::io::BufRead;
    use std::os::unix::net::UnixListener;

    /// 起一个只会回固定内容的假守护进程。
    ///
    /// 用假的而不是真起 `daemon::run`：这里要测的是「对面回了个奇怪的东西
    /// 时客户端怎么办」，包括"老到不认识 Hello"这种真守护进程根本造不出来
    /// 的情况。假 socket 是唯一能覆盖它的办法。
    fn fake_daemon(reply: &Response) -> (tempfile::TempDir, PathBuf) {
        let reply = serde_json::to_string(reply).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("daemon.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut writer = stream;
                let mut line = String::new();
                // 每收一行回一行，直到对面关掉
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    let _ = writeln!(writer, "{reply}");
                    let _ = writer.flush();
                    line.clear();
                }
            }
        });
        (dir, sock)
    }

    #[test]
    fn handshake_succeeds_when_versions_match() {
        let (_dir, sock) = fake_daemon(&Response::Hello {
            version: PROTOCOL_VERSION,
        });
        assert!(Client::connect(&sock).is_ok());
    }

    #[test]
    fn version_mismatch_names_both_versions_and_how_to_fix_it() {
        // 对面报一个不可能等于当前值的版本号
        let (_dir, sock) = fake_daemon(&Response::Hello {
            version: PROTOCOL_VERSION + 100,
        });
        let err = Client::connect(&sock).unwrap_err().to_string();

        assert!(err.contains("dct restart"), "得告诉用户下一步干什么: {err}");
        assert!(
            err.contains(&(PROTOCOL_VERSION + 100).to_string()),
            "得说出后台是哪一版: {err}"
        );
        assert!(
            err.contains(&PROTOCOL_VERSION.to_string()),
            "得说出界面是哪一版: {err}"
        );
        assert!(
            !err.contains("Hello") && !err.contains("serde"),
            "不能把内部名字漏给用户: {err}"
        );
    }

    #[test]
    fn a_daemon_too_old_to_know_hello_gets_the_same_advice() {
        // 老守护进程收到不认识的请求会回 Error（daemon.rs:85），
        // 而 Error 变体新老都有，新界面解得开——这就是不用改老代码
        // 也能认出老代码的原因。
        let (_dir, sock) = fake_daemon(&Response::Error(
            "请求解析失败: unknown variant `Hello`".to_string(),
        ));
        let err = Client::connect(&sock).unwrap_err().to_string();
        assert!(err.contains("dct restart"), "{err}");
        assert!(!err.contains("unknown variant"), "别把原始报错甩给用户: {err}");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib client:: -- --test-threads=1`
Expected: 编译失败，`no variant or associated item named 'Hello'`。

- [ ] **Step 3: 加协议**

`src/proto.rs`，在 `Request` 定义之前加：

```rust
/// 线上契约的版本号。
///
/// 只在**破坏性**改动时 +1：改字段类型、删字段、把元组变体改成结构体变体。
/// 加一个带 `#[serde(default)]` 的新字段不算破坏性，不用动它。
///
/// 存在的理由是两次真实事故：多 agent 那轮把 `Response::Profiles` 从元组变体
/// 改成结构体变体，用户升级了界面没重启守护进程，界面报「拿不到 agent 列表」
/// ——既没说原因也没说下一步。裸 enum 的序列化不匹配是**静默**的，
/// 不主动握手就永远只能看到下游那句莫名其妙的失败。
pub const PROTOCOL_VERSION: u32 = 1;
```

`Request` 里加第一个变体（放最前面，它是连接上的第一个请求）：

```rust
    Hello {
        version: u32,
    },
```

`Response` 里加：

```rust
    Hello {
        version: u32,
    },
```

手写的 `impl Debug for Request` 里加一条 arm（漏了会编译不过）：

```rust
            Request::Hello { version } => {
                f.debug_struct("Hello").field("version", version).finish()
            }
```

- [ ] **Step 4: 守护进程回应握手**

`src/daemon.rs`，在 `handle` 的 match 里，`Request::List` 那条之前加：

```rust
        Request::Hello { .. } => Ok(Response::Hello {
            version: crate::proto::PROTOCOL_VERSION,
        }),
```

忽略客户端报来的版本号：**版本判定只在客户端做**。守护进程是长命进程，
可能同时被新老界面连上；让它去判断"对面太老要不要拒绝"，等于让一个界面的
版本影响另一个界面。客户端自己看得见自己的版本，自己决定。

- [ ] **Step 5: 客户端握手**

`src/client.rs`，`reconnect()` 里 `self.conn = Some(...)` 之后加一次握手：

```rust
        self.conn = Some(Conn {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        });
        self.handshake()
    }

    /// 连上之后的第一件事，先确认对面跟自己说的是同一种话。
    ///
    /// 放在 `reconnect()` 里而不是 `connect()` 里：`call()` 出错时会丢掉连接
    /// 自动重连，重连之后对面可能已经换成另一个进程了（用户正好在这一刻
    /// 重启了守护进程）。每次连上都问一遍才没有窗口。
    fn handshake(&mut self) -> Result<()> {
        let theirs = match self.try_call(&Request::Hello {
            version: PROTOCOL_VERSION,
        }) {
            Ok(Response::Hello { version }) => version,
            // 老守护进程不认识 Hello，会回一句 `Error("请求解析失败: …")`。
            // 它跟"版本对不上"是同一件事，给同一句提示，不要把原始报错
            // 甩给用户——那句话里全是 serde 的内部术语。
            Ok(_) | Err(_) => {
                self.conn = None;
                bail!(
                    "后台服务的版本跟界面对不上（界面是第 {PROTOCOL_VERSION} 版）。\n\
                     在终端里运行 dct restart，然后重新打开 dct。"
                );
            }
        };
        if theirs != PROTOCOL_VERSION {
            self.conn = None;
            bail!(
                "后台服务是第 {theirs} 版，界面是第 {PROTOCOL_VERSION} 版，两边对不上。\n\
                 在终端里运行 dct restart，然后重新打开 dct。"
            );
        }
        Ok(())
    }
```

文件顶部的 import 补上 `PROTOCOL_VERSION`：

```rust
use crate::proto::{Request, Response, PROTOCOL_VERSION};
```

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 全绿，测试数从 172 变成 175。

- [ ] **Step 7: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`
Expected: 无输出。

- [ ] **Step 8: 提交**

```bash
git add src/proto.rs src/daemon.rs src/client.rs
git commit -m "feat: 协议加握手版本号，版本对不上时说人话

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `dct restart` 子命令

**Files:**
- Create: `src/restart.rs`
- Modify: `src/lib.rs`（加 `pub mod restart;`）
- Modify: `src/main.rs`（`HELP` 文本 + `restart` 分支）
- Test: `src/restart.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `Request::List` / `Response::Sessions(Vec<SessionInfo>)`（已有，新老守护进程都认）
- Produces: `pub fn run(sock: &Path) -> anyhow::Result<()>`；纯函数 `pub fn confirm_prompt(sessions: &[SessionInfo]) -> Option<String>`

**为什么不走协议：** 协议对不上正是它要解决的问题。按可执行文件绝对路径
`pkill` 是唯一对老守护进程也成立的办法。而 `Request::List` 在新老两版里是同一个
元组变体，对着老守护进程也问得出来——列会话这一步不需要握手成功，所以
`restart` 必须用**裸的 UnixStream**，不能用 `Client`（Task 1 之后 `Client::connect`
会在握手失败时直接报错返回）。

- [ ] **Step 1: 写失败的测试**

新建 `src/restart.rs`，先只写测试和签名：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionInfo, SessionState};

    fn info(id: u32, profile: &str, dir: &str, state: SessionState) -> SessionInfo {
        SessionInfo {
            id,
            profile: profile.to_string(),
            dir: dir.to_string(),
            state,
            activity: String::new(),
        }
    }

    #[test]
    fn no_sessions_means_no_question() {
        // 没有会话就没什么可失去的，不该拦着用户
        assert!(confirm_prompt(&[]).is_none());
    }

    #[test]
    fn prompt_lists_every_session_and_warns_about_losing_work() {
        let s = [
            info(4, "claude", "/Users/x/work/a", SessionState::Working),
            info(5, "codex", "/Users/x/work/b", SessionState::Idle),
        ];
        let p = confirm_prompt(&s).expect("有会话就该问");

        assert!(p.contains("2 个"), "得说清会关掉几个: {p}");
        assert!(p.contains("claude") && p.contains("codex"), "每个都要列出来: {p}");
        assert!(p.contains("work/a") && p.contains("work/b"), "得说是哪个项目: {p}");
        assert!(p.contains("中断"), "得说清代价: {p}");
        assert!(
            !p.contains("daemon") && !p.contains("SIGKILL") && !p.contains("进程"),
            "不能有黑话: {p}"
        );
    }

    #[test]
    fn prompt_says_working_sessions_are_working() {
        let s = [info(4, "claude", "/Users/x/work/a", SessionState::Working)];
        let p = confirm_prompt(&s).unwrap();
        assert!(p.contains("干活中"), "状态得让用户看出哪个正忙: {p}");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib restart:: -- --test-threads=1`
Expected: 编译失败，`cannot find function 'confirm_prompt'`。

- [ ] **Step 3: 实现纯函数与命令**

`src/restart.rs` 顶部（测试模块之前）：

```rust
//! `dct restart` —— 换掉正在跑的后台服务。
//!
//! 存在的唯一理由是「后台服务版本对不上」那句提示必须能照着做（房规：
//! 一句没说清下一步该干嘛的错误提示，不算写完）。
//!
//! 它**不能**走协议：协议对不上正是它要解决的问题。所以：
//! - 列会话用裸 `UnixStream` 发一条 `Request::List`——这个变体新老两版一样，
//!   老服务也答得上来。用 `Client` 不行，`Client::connect` 会先握手，
//!   握手失败就什么都问不到了。
//! - 停旧服务按可执行文件的绝对路径 `pkill`，不发任何请求。

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::proto::{Request, Response};
use crate::session::{SessionInfo, SessionState};
use crate::ui::status_label;

/// 重启前要不要问、问什么。没有会话就返回 `None`——没什么可失去的时候
/// 拦一下只是添堵。
pub fn confirm_prompt(sessions: &[SessionInfo]) -> Option<String> {
    if sessions.is_empty() {
        return None;
    }
    let mut s = format!("重启会关掉这 {} 个正在跑的会话：\n", sessions.len());
    for i in sessions {
        s.push_str(&format!(
            "  #{}  {}  {}  {}\n",
            i.id,
            i.profile,
            i.dir,
            status_label(i.state)
        ));
    }
    s.push_str("它们干到一半的活会中断。确定要重启吗？[y/N] ");
    Some(s)
}

/// 问一次老服务在跑什么。任何失败都当成「问不出来」：服务没起、太老、
/// 卡住，都不该挡着用户重启——重启正是这些情况的解法。
fn ask_sessions(sock: &Path) -> Vec<SessionInfo> {
    let Ok(stream) = UnixStream::connect(sock) else {
        return Vec::new();
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let Ok(mut writer) = stream.try_clone() else {
        return Vec::new();
    };
    let mut reader = BufReader::new(stream);
    let Ok(req) = serde_json::to_string(&Request::List) else {
        return Vec::new();
    };
    if writeln!(writer, "{req}").is_err() || writer.flush().is_err() {
        return Vec::new();
    }
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return Vec::new();
    }
    match serde_json::from_str::<Response>(&line) {
        Ok(Response::Sessions(v)) => v,
        _ => Vec::new(),
    }
}

pub fn run(sock: &Path) -> Result<()> {
    let sessions = ask_sessions(sock);

    if let Some(prompt) = confirm_prompt(&sessions) {
        print!("{prompt}");
        std::io::stdout().flush().ok();
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .context("读不到你的回答")?;
        // 默认「否」：默认值必须是不会毁东西的那个
        if !matches!(answer.trim(), "y" | "Y") {
            println!("没有重启，会话都还在。");
            return Ok(());
        }
    }

    let exe = std::env::current_exe().context("找不到 dct 自己在哪")?;
    let pattern = format!("^{} daemon$", regex::escape(&exe.display().to_string()));
    // -f 匹配完整命令行，锚定到自己的绝对路径：同名但装在别处的 dct 不受影响
    let status = std::process::Command::new("pkill")
        .arg("-f")
        .arg(&pattern)
        .status();
    match status {
        // 0 = 杀掉了，1 = 本来就没在跑。两种都算成功。
        Ok(s) if s.code() == Some(0) || s.code() == Some(1) => {}
        _ => anyhow::bail!("停不掉旧的后台服务。请重开一个终端窗口再试。"),
    }

    // socket 文件是旧进程留下的，不删掉新进程可能 bind 失败
    let _ = std::fs::remove_file(sock);

    println!("后台服务已经换新的了。重新运行 dct 就行。");
    Ok(())
}
```

> 新守护进程不在这里拉起：`dct` 无参启动时本来就会「连不上就自动拉一个」
> （`main.rs:run_ui`）。在这里再拉一个只会多一条不必要的路径，还得重复
> 那段 `setsid` 的处理。

- [ ] **Step 4: 接进命令行**

`src/lib.rs` 加：

```rust
pub mod restart;
```

`src/main.rs` 的 `HELP` 改成：

```rust
const HELP: &str = "\
dct —— vibe coding 终端

用法：
  dct           打开会话看板（守护进程没在跑就自动拉起）
  dct daemon    只跑守护进程，不开界面
  dct restart   换掉正在跑的后台服务（会关掉所有会话，会先问你）
  dct --help    看这段
";
```

match 里加：

```rust
        Some("restart") => dct::restart::run(&socket_path()),
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 全绿，178 个测试。

- [ ] **Step 6: 手工验一次**

```bash
cargo build --release
./target/release/dct restart
```
Expected: 有会话时列出来并问 `[y/N]`；直接回车什么都不杀，打印「没有重启，会话都还在。」

- [ ] **Step 7: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 8: 提交**

```bash
git add src/restart.rs src/lib.rs src/main.rs
git commit -m "feat: dct restart 子命令，先列会话再问

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `ui.rs` 拆出纯函数与小工具（纯搬家）

**Files:**
- Rename: `src/ui.rs` → `src/ui/mod.rs`
- Create: `src/ui/widgets.rs`
- Create: `src/ui/view.rs`
- Test: 测试跟着自己的代码一起搬，**不新增、不删除**

**Interfaces:**
- Consumes: 无
- Produces: `crate::ui::widgets::{status_label, status_color, Msg, to_color, to_style, screen_to_lines, char_width, display_width, truncate, pad_to, short_path}`；`crate::ui::view::{View, SecretPhase, PickAction, back_one_level, escape_hint, idle_help, pick_action, quick_start_target, digit_index, secret_rows, decide_delete_key, verify_outcome_applies_to, message_after_transition}`。`src/ui/mod.rs` 用 `pub use` 把它们原样再导出，**外部调用点一个都不用改**。

**这一步的唯一验收标准是「什么都没变」。** 不重命名、不改签名、不改注释、
不顺手修任何东西。`cargo test` 的数量必须还是 178。

- [ ] **Step 1: 建目录，原样改名**

```bash
mkdir -p src/ui
git mv src/ui.rs src/ui/mod.rs
cargo test -- --test-threads=1
```
Expected: 178 个测试全绿（Rust 的 `mod.rs` 约定，改名不影响任何东西）。

- [ ] **Step 2: 搬 widgets**

新建 `src/ui/widgets.rs`，从 `src/ui/mod.rs` **剪切**这些项过去（连注释一起，
一个字不改）：

`status_label`（22 行）、`status_color`（32 行）、`struct Msg` 与它的三个
`impl`（44-68 行）、`to_color`（1626 行）、`to_style`（1634 行）、
`screen_to_lines`（1660 行）、`char_width`（1676 行）、`display_width`（1684 行）、
`truncate`（1689 行）、`pad_to`（1709 行）、`short_path`（1716 行）。

搬测试：`pad_to_aligns_cjk_and_ascii_labels_to_the_same_display_width`、
`pad_to_never_shrinks_a_string_already_at_or_over_width`、
`status_labels_are_chinese`、`unknown_state_shows_a_dash`、
`asking_and_working_use_different_colors`、`msg_from_str_is_not_an_error`。

`src/ui/mod.rs` 顶部加：

```rust
mod widgets;
pub use widgets::{status_label, status_color, Msg};
use widgets::{
    char_width, display_width, pad_to, screen_to_lines, short_path, to_color, to_style, truncate,
};
```

被搬走的项在 `widgets.rs` 里维持原来的可见性；`pub fn` 的保持 `pub fn`，
私有的保持私有（同 crate 内跨模块要 `pub(crate)` 的，改成 `pub(crate)`）。

- [ ] **Step 3: 跑测试**

Run: `cargo test -- --test-threads=1`
Expected: 178 个，全绿。数量对不上就是搬漏了或搬重了。

- [ ] **Step 4: 提交**

```bash
git add -A src/ui
git commit -m "refactor: ui 的排版与配色小工具搬进 ui/widgets.rs

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 5: 搬 view**

新建 `src/ui/view.rs`，从 `src/ui/mod.rs` 剪切：

`enum View`（71 行）及其全部字段注释、`enum SecretPhase`、`enum PickAction`、
`is_ctrl_q`（1347 行）、`back_one_level`（1365 行）、`pick_action`（1417 行）、
`secret_rows`（1451 行）、`decide_delete_key`（1481 行）、
`quick_start_target`（1508 行）、`digit_index`（1517 行）、
`clean_secret`（1538 行）、`verify_message`（1553 行）、
`verify_outcome_applies_to`（1575 行）、`message_after_transition`（1845 行）、
`escape_hint`（1859 行）、`idle_help`（1882 行）、`filter_projects`（1747 行）、
`expand_path`（1725 行）。

对应的测试也一起搬（`ctrl_q_*`、`back_one_level_*`、`pick_action` 那一组、
`quick_start_*`、`digit_keys_*`、`paste_*`、`bad_key_gets_a_human_message`、
`unreachable_blames_the_network_not_the_key`、`ok_has_no_message`、
`verify_outcome_*`、`message_after_transition_*`、`secret_rows_*`、
`expand_path_*`、`filter_projects_*`）。

> **不要搬**那些用 `TestBackend` 画一帧再断言的测试（`draw_does_not_panic_*`、
> `escape_hint_survives_*`、`bottom_bar_*`、`secrets_view_renders_*` 等）。
> 它们依赖 `draw`，`draw` 还在 `mod.rs` 里，Task 5 才拆。

`src/ui/mod.rs` 顶部加：

```rust
mod view;
pub use view::{
    clean_secret, decide_delete_key, digit_index, pick_action, quick_start_target, secret_rows,
    verify_message, verify_outcome_applies_to, PickAction,
};
use view::{
    back_one_level, escape_hint, expand_path, filter_projects, idle_help, is_ctrl_q,
    message_after_transition, SecretPhase, View,
};
```

- [ ] **Step 6: 跑测试**

Run: `cargo test -- --test-threads=1`
Expected: 178 个，全绿。

- [ ] **Step 7: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 8: 提交**

```bash
git add -A src/ui
git commit -m "refactor: View 与其纯函数搬进 ui/view.rs

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: `run()` 的局部变量收进 `App`

**Files:**
- Create: `src/ui/app.rs`
- Modify: `src/ui/mod.rs`（`run()` 从 1130 行缩到外壳）
- Test: `src/ui/app.rs`

**Interfaces:**
- Consumes: `crate::ui::view::View`、`crate::ui::widgets::Msg`
- Produces:

```rust
pub struct App {
    pub client: Client,
    pub view: View,
    pub list_state: ListState,
    pub sessions: Vec<SessionInfo>,
    pub message: Msg,
    pub screen: Vec<Vec<ScreenSpan>>,
    pub screen_cursor: (u16, u16),
    pub sent_size: Option<(u32, u16, u16)>,
    pub connected: bool,
    pub need_sessions: bool,
    pub verify_rx: Option<(String, String, std::sync::mpsc::Receiver<VerifyOutcome>)>,
    pub start_dir: PathBuf,
    pub current_dir: PathBuf,
    pub quit: bool,
}

impl App {
    pub fn new(client: Client, default_dir: PathBuf) -> App;
}
```

**这一步有语义风险，跟 Task 3 分开做的原因就在这里。** 两条铁律：

1. `run()` 循环末尾清理陈旧 `message` 的那段（调用 `message_after_transition`
   的地方）必须**原样保留在循环末尾**，不能挪进任何按键分支。`e0ba1ec` 就是
   在这里翻的车：一句普通的「已切到 X」盖掉了屏幕上唯一告诉用户怎么退出的行。
2. 按键分支里仍然不许 `continue`，理由同上。

- [ ] **Step 1: 写失败的测试**

`src/ui/app.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 新建的 App 必须落在看板上、没有陈旧消息、认为自己是连着的。
    /// 这三个初值任何一个错了，用户开机第一眼看到的就是错的。
    #[test]
    fn a_fresh_app_starts_on_the_board_with_nothing_stale() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("nope.sock");
        // 连不上也要能构造：构造 App 不该有副作用
        let app = App::new_disconnected(sock, dir.path().to_path_buf());
        assert!(matches!(app.view, View::Board));
        assert_eq!(app.message.text, "");
        assert!(!app.quit);
        assert!(app.need_sessions, "开机第一轮必须拉一次会话列表");
    }

    /// start_dir 只用来解析用户敲的相对路径，永不改变；current_dir 是
    /// 「下一个会话开在哪」，选择器会改它。两者一开始相同但不是同一个东西，
    /// 合并成一个字段会让「换了项目之后再敲相对路径」解析到错的基准目录。
    #[test]
    fn start_dir_and_current_dir_are_separate_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new_disconnected(dir.path().join("s.sock"), dir.path().to_path_buf());
        app.current_dir = PathBuf::from("/somewhere/else");
        assert_eq!(app.start_dir, dir.path());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib ui::app -- --test-threads=1`
Expected: 编译失败，`cannot find struct 'App'`。

- [ ] **Step 3: 建 App**

`src/ui/app.rs` 写上面 Interfaces 里那个结构体，加两个构造函数：

```rust
impl App {
    pub fn new(client: Client, default_dir: PathBuf) -> App {
        App {
            client: Some(client),
            view: View::Board,
            list_state: ListState::default(),
            sessions: Vec::new(),
            message: "".into(),
            screen: Vec::new(),
            screen_cursor: (0, 0),
            sent_size: None,
            // 每轮循环开头的 List 调用是唯一的真相来源，它总在当次
            // term.draw 之前重新算一遍，所以这里给什么都会被立刻覆盖。
            connected: true,
            need_sessions: true,
            verify_rx: None,
            start_dir: default_dir.clone(),
            current_dir: default_dir,
            quit: false,
        }
    }

    /// 只给测试用：不需要一个活的守护进程就能构造。
    #[cfg(test)]
    pub fn new_disconnected(_sock: PathBuf, default_dir: PathBuf) -> App { /* 同上，client: None */ }
}
```

> `client` 改成 `Option<Client>` 是为了测试能构造一个没有连接的 App。
> 所有用到它的地方走一个 `fn client(&mut self) -> Result<&mut Client>`，
> `None` 时返回「守护进程连不上」——这跟真实断线是同一条路径，不新增分支。

- [ ] **Step 4: 把 `run()` 的局部变量换成 `app.<字段>`**

`src/ui/mod.rs` 的 `run()`：删掉 210-250 行那一堆 `let mut`，改成
`let mut app = App::new(client, default_dir);`，然后把函数体里每一处
裸变量名换成 `app.` 前缀。终端生命周期那部分（`spawn_signal_restore`、
`enable_raw_mode`、`TerminalGuard`、`execute!`、`Terminal::new`）留在 `run()` 里，
它们不是状态。

**逐字保留**这些注释（它们记录的是踩过的坑，不是废话）：`start_dir`/`current_dir`
的区别、`sent_size` 为什么存在、`connected` 为什么不预置初值、`need_sessions`
为什么只在看板拉、`verify_rx` 为什么带着 `(profile, buf)`。把它们搬到
`App` 对应字段的上方。

- [ ] **Step 5: 跑测试**

Run: `cargo test -- --test-threads=1`
Expected: 180 个（178 + 新增 2 个），全绿。

- [ ] **Step 6: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 7: 提交**

```bash
git add -A src/ui
git commit -m "refactor: run() 的状态收进 App 结构

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: 按视图把按键与渲染搬进各自模块

**Files:**
- Create: `src/ui/board.rs`、`src/ui/attach.rs`、`src/ui/pick.rs`、`src/ui/secret.rs`
- Modify: `src/ui/mod.rs`

**Interfaces:**
- Consumes: `App`（Task 4）
- Produces: 每个模块两个函数，签名完全一致：

```rust
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> anyhow::Result<()>;
pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App);
```

`board.rs` 管 `View::Board`；`attach.rs` 管 `View::Attached`；
`pick.rs` 管 `View::PickProfile` 与 `View::PickProject`；
`secret.rs` 管 `View::EnterSecret` 与 `View::Secrets`。

- [ ] **Step 1: 先把 `run()` 的 `match view` 抽成函数（不换文件）**

在 `src/ui/mod.rs` 里，把 `match view.clone()` 的每个 arm 原样抽成一个
`fn handle_board(app: &mut App, key: KeyEvent) -> Result<()>` 之类的私有函数，
`match` 变成只负责分派。**这一步不动任何一行分支内部的代码**，只是把它包进函数。

Run: `cargo test -- --test-threads=1`
Expected: 180 个，全绿。

- [ ] **Step 2: 提交这一半**

```bash
git add -A src/ui
git commit -m "refactor: 按键分支抽成函数，match 只负责分派

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 3: 同样处理 `draw`**

`draw`（原 1932 行，约 390 行）内部也是按视图分的。把每一段抽成
`fn draw_board(f: &mut Frame, area: Rect, app: &mut App)` 之类。
`DrawInput` 结构体删掉——它存在的理由是「`draw` 参数太多」，现在参数就是 `App`。

Run: `cargo test -- --test-threads=1`
Expected: 180 个，全绿。

- [ ] **Step 4: 把配对的 handle + draw 一起搬进新文件**

`handle_board` + `draw_board` → `src/ui/board.rs`，各自改名成 `handle_key` / `draw`。
其余三组照做。用 `TestBackend` 画一帧的那些测试跟着自己那一组走。

`src/ui/mod.rs` 顶部：

```rust
mod app;
mod attach;
mod board;
mod pick;
mod secret;
mod view;
mod widgets;
```

在每个新文件的 `handle_key` 上方，把这条房规抄一份（拆开之后它更难被看见）：

```rust
/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
```

- [ ] **Step 5: 跑测试**

Run: `cargo test -- --test-threads=1`
Expected: 180 个，全绿。

- [ ] **Step 6: 确认瘦身有效**

Run: `wc -l src/ui/*.rs`
Expected: 没有一个文件超过 1200 行。超了就说明还有一段没归位。

- [ ] **Step 7: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 8: 提交**

```bash
git add -A src/ui
git commit -m "refactor: 每个视图的按键与渲染搬进自己的模块

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: `pty.rs` 保留 2000 行历史，加滚动 API

**Files:**
- Modify: `src/pty.rs:91`（`Parser::new` 的第三个参数）
- Modify: `src/pty.rs`（加 `scroll_by` / `scroll_state`）
- Test: `src/pty.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: 无
- Produces:

```rust
/// 保留多少行滚出屏幕的内容。
pub const SCROLLBACK_ROWS: usize = 2000;

impl PtySession {
    /// 滚动并返回滚完之后的状态。正数往上翻进历史，负数往下。
    pub fn scroll_by(&self, rows: i32) -> ScrollView;
    /// 直接回到底部。
    pub fn scroll_to_bottom(&self) -> ScrollView;
    /// 不改变位置，只读当前状态。
    pub fn scroll_state(&self) -> ScrollView;
}

/// pty 层看到的滚动事实。协议层的 `ScrollState` 由它加上 `new_lines` 组成
/// （`new_lines` 要跨帧记忆，那是 session 层的事）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollView {
    pub offset: usize,
    pub max: usize,
    pub agent_owns: bool,
    pub alt_screen: bool,
}
```

**vt100 的三个事实（读过 0.15.2 源码，别再猜）：**
- `Parser::new(rows, cols, n)` 第三个参数是保留行数，现在传的 0。
- `Screen::cell()` → `visible_cell()` → `visible_rows()` 自带
  `skip(scrollback_len - offset)`（`grid.rs:120-125`），所以
  **`screen_spans()` 一行都不用改**。
- vt100 没有「现在攒了多少行历史」的公开接口。`set_scrollback` 内部会
  `rows.min(self.scrollback.len())` 钳一次（`grid.rs:183-185`），所以探测
  上限的办法是：设一个大得离谱的值，读回来就是真实上限。三次字段写，
  不分配不拷贝。

- [ ] **Step 1: 写失败的测试**

`src/pty.rs` 的测试模块里加（沿用已有的 `wait_for` 辅助函数）：

```rust
    /// 造一个吐 N 行然后挂着不退的会话
    fn spawn_lines(dir: &Path, n: usize) -> PtySession {
        PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                format!("i=1; while [ $i -le {n} ]; do echo line-$i; i=$((i+1)); done; sleep 30"),
            ],
            &Default::default(),
            dir,
            24,
            80,
        )
        .unwrap()
    }

    #[test]
    fn keeps_history_that_scrolled_off_the_screen() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 100);
        assert!(wait_for(&p, "line-100"));

        // 屏幕只有 24 行，line-1 早就滚出去了
        assert!(!p.screen_text().contains("line-1\n"));

        p.scroll_by(90);
        assert!(
            p.screen_text().contains("line-1\n"),
            "往上翻 90 行应该能看见最早那行"
        );
    }

    #[test]
    fn history_is_capped_at_the_configured_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), SCROLLBACK_ROWS + 500);
        assert!(wait_for(&p, &format!("line-{}", SCROLLBACK_ROWS + 500)));

        let st = p.scroll_state();
        assert_eq!(st.max, SCROLLBACK_ROWS, "上限就是上限，不能无限涨");
    }

    #[test]
    fn scrolling_past_the_top_stops_at_the_top() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 50);
        assert!(wait_for(&p, "line-50"));

        let st = p.scroll_by(i32::MAX);
        assert_eq!(st.offset, st.max, "翻到头就停在头，不能溢出");
    }

    #[test]
    fn scrolling_below_the_bottom_stops_at_the_bottom() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 50);
        assert!(wait_for(&p, "line-50"));

        p.scroll_by(10);
        let st = p.scroll_by(-1000);
        assert_eq!(st.offset, 0, "往下翻过头就停在底部");
    }

    /// 这条测的是 vt100 的行为，不是我们的代码——但整个「新输出时画面不动」
    /// 的设计都压在它上面（grid.rs:556-558）。它哪天变了，这里要第一个响。
    #[test]
    fn the_view_stays_put_when_new_output_arrives() {
        let dir = tempfile::tempdir().unwrap();
        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "i=1; while [ $i -le 60 ]; do echo line-$i; i=$((i+1)); done; \
                 sleep 1; echo MARKER-NEW; sleep 30".to_string(),
            ],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "line-60"));

        let before = p.scroll_by(30);
        assert!(wait_for_offset_to_grow(&p, before.offset));

        let after = p.scroll_state();
        assert!(
            after.offset > before.offset,
            "来了新行，偏移要跟着涨，画面才不动：{} -> {}",
            before.offset,
            after.offset
        );
    }

    fn wait_for_offset_to_grow(p: &PtySession, from: usize) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if p.scroll_state().offset > from {
                return true;
            }
            sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn an_alternate_screen_app_has_no_history_to_scroll() {
        let dir = tempfile::tempdir().unwrap();
        // ESC[?1049h 进备用屏，然后吐一堆行
        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf '\\033[?1049h'; i=1; while [ $i -le 60 ]; do echo alt-$i; \
                 i=$((i+1)); done; sleep 30".to_string(),
            ],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "alt-60"));

        let st = p.scroll_state();
        assert!(st.alt_screen, "应该认出它在备用屏上");
        assert_eq!(st.max, 0, "备用屏上没有历史，这跟真实终端一致");
    }

    /// 程序设了滚动区（DECSTBM）之后，vt100 不往 scrollback 里塞任何东西
    /// （grid.rs:551）。这不是我们能改的，但界面要能认出「这里翻不了」
    /// 而不是让用户对着一个没反应的滚轮猜。
    #[test]
    fn a_scroll_region_swallows_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf '\\033[1;20r'; i=1; while [ $i -le 60 ]; do echo rgn-$i; \
                 i=$((i+1)); done; sleep 30".to_string(),
            ],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "rgn-60"));
        assert_eq!(p.scroll_state().max, 0);
    }

    #[test]
    fn a_plain_shell_does_not_own_the_scrolling() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 10);
        assert!(wait_for(&p, "line-10"));
        assert!(
            !p.scroll_state().agent_owns,
            "没开鼠标上报的程序，滚轮归 dct"
        );
    }

    #[test]
    fn an_app_that_asks_for_the_mouse_owns_the_scrolling() {
        let dir = tempfile::tempdir().unwrap();
        // ESC[?1000h 开鼠标上报，跟 Claude Code 实测抓到的一样
        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf '\\033[?1000h'; echo mouse-on; sleep 30".to_string(),
            ],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "mouse-on"));
        assert!(p.scroll_state().agent_owns);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib pty:: -- --test-threads=1`
Expected: 编译失败，`cannot find value 'SCROLLBACK_ROWS'`。

- [ ] **Step 3: 实现**

`src/pty.rs`，`PtySession` 定义之前：

```rust
/// 每个会话保留多少行滚出屏幕的内容。
///
/// 写死不做配置项：用户不该被问这个数字。vt100 的 `Cell` 约 36 字节，
/// 120 列一行约 4.2 KB，2000 行满载约 8.4 MB/会话。底下是 `VecDeque`，
/// 按实际用量增长，2000 是天花板不是预分配。
pub const SCROLLBACK_ROWS: usize = 2000;

/// pty 层看到的滚动事实。
///
/// `agent_owns` 是整个滚屏设计的分流开关：agent 开了鼠标上报就说明它自己
/// 管视口（Claude Code 就是这样），滚轮该转发给它；没开就由 dct 滚自己的
/// 缓冲（codex、命令行）。这两个真实 agent 在「用不用备用屏」上正好相反，
/// 所以判据只能是鼠标，不能是备用屏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollView {
    pub offset: usize,
    pub max: usize,
    pub agent_owns: bool,
    pub alt_screen: bool,
}
```

`spawn()` 里改一个数字：

```rust
        let parser = Arc::new(Mutex::new(vt100::Parser::new(
            rows,
            cols,
            SCROLLBACK_ROWS,
        )));
```

加三个方法：

```rust
    /// 滚动并返回滚完之后的状态。正数往上翻进历史，负数往下。
    ///
    /// 钳位交给 vt100 自己做（`grid.rs:183-185` 会 `.min(scrollback.len())`），
    /// 我们只负责别让 i32 加法溢出。
    pub fn scroll_by(&self, rows: i32) -> ScrollView {
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let max = probe_max(&mut parser);
        let cur = parser.screen().scrollback();
        let target = if rows >= 0 {
            cur.saturating_add(rows as usize)
        } else {
            cur.saturating_sub(rows.unsigned_abs() as usize)
        };
        parser.set_scrollback(target.min(max));
        view_of(&parser, max)
    }

    pub fn scroll_to_bottom(&self) -> ScrollView {
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        // probe_max 会把偏移推到顶，所以归零必须在它之后
        let max = probe_max(&mut parser);
        parser.set_scrollback(0);
        view_of(&parser, max)
    }

    pub fn scroll_state(&self) -> ScrollView {
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let cur = parser.screen().scrollback();
        let max = probe_max(&mut parser);
        parser.set_scrollback(cur);
        view_of(&parser, max)
    }
```

模块级私有函数：

```rust
/// vt100 不公开「现在攒了多少行历史」。但 `set_scrollback` 内部会
/// `.min(scrollback.len())` 钳一次，所以设一个大得离谱的值再读回来，
/// 读到的就是真实上限。三次字段写，不分配不拷贝。
///
/// **调用方负责把偏移放回去**——这个函数会改变它。
fn probe_max(parser: &mut vt100::Parser) -> usize {
    parser.set_scrollback(usize::MAX);
    parser.screen().scrollback()
}

fn view_of(parser: &vt100::Parser, max: usize) -> ScrollView {
    let screen = parser.screen();
    ScrollView {
        offset: screen.scrollback(),
        max,
        agent_owns: screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None,
        alt_screen: screen.alternate_screen(),
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib pty:: -- --test-threads=1`
Expected: 全绿，pty 的测试从 5 个变成 14 个。

- [ ] **Step 5: 跑全量**

Run: `cargo test -- --test-threads=1`
Expected: 189 个，全绿。

- [ ] **Step 6: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 7: 提交**

```bash
git add src/pty.rs
git commit -m "feat: PTY 保留 2000 行历史，加滚动 API

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: `session.rs` 贯通滚动状态

**Files:**
- Modify: `src/session.rs`（`ScreenSnapshot`、`Session`、`screen`、`send_input`、`resize`，新增 `scroll`）
- Test: `src/session.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::pty::ScrollView`（Task 6）
- Produces:

```rust
/// 一屏文字 + 光标 + 滚动状态。
pub struct ScreenSnapshot {
    pub lines: Vec<Vec<ScreenSpan>>,
    pub cursor: (u16, u16),
    pub scroll: ScrollState,
}

/// 界面画底栏要用的全部滚动事实。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrollState {
    #[serde(default)] pub agent_owns: bool,
    #[serde(default)] pub alt_screen: bool,
    #[serde(default)] pub max: usize,
    #[serde(default)] pub offset: usize,
    #[serde(default)] pub new_lines: usize,
}

impl SessionManager {
    pub fn scroll(&self, id: u32, by: ScrollBy) -> Result<ScrollState>;
}

pub enum ScrollBy { Rows(i32), Bottom }
```

`ScreenSnapshot` 从元组别名改成结构体，是**破坏性**改动 —— Task 8 把
`PROTOCOL_VERSION` 从 1 改成 2。

**`new_lines` 怎么算：** vt100 的偏移只在两种情况下变——用户滚，或者新行
推入时自动 +1（`grid.rs:556-558`）。所以在 `Session` 里记一个
`scroll_mark: usize`：每次**用户主动**滚动之后把它设成当时的偏移，
`new_lines = offset.saturating_sub(mark)`。

- [ ] **Step 1: 写失败的测试**

```rust
    /// 造一个吐 N 行然后挂着的 shell 会话
    fn scrolling_session(mgr: &SessionManager, dir: &Path, n: usize) -> u32 {
        let mut p = fake_agent();
        p.is_agent = false;
        p.command = vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("i=1; while [ $i -le {n} ]; do echo line-$i; i=$((i+1)); done; sleep 30"),
        ];
        mgr.register_profile(p.clone());
        mgr.create(dir.to_str().unwrap(), &p.name, empty_secrets(), 24, 80)
            .unwrap()
    }

    #[test]
    fn typing_jumps_back_to_the_bottom() {
        let dir = init_repo();
        let mgr = SessionManager::new();
        let id = scrolling_session(&mgr, dir.path(), 100);
        wait_for_screen(&mgr, id, "line-100");

        mgr.scroll(id, ScrollBy::Rows(30)).unwrap();
        assert!(mgr.screen(id).unwrap().scroll.offset > 0);

        mgr.send_input(id, "x").unwrap();
        assert_eq!(
            mgr.screen(id).unwrap().scroll.offset,
            0,
            "一敲键就该回到底部，否则用户看不见自己打的字"
        );
    }

    #[test]
    fn resizing_jumps_back_to_the_bottom() {
        let dir = init_repo();
        let mgr = SessionManager::new();
        let id = scrolling_session(&mgr, dir.path(), 100);
        wait_for_screen(&mgr, id, "line-100");

        mgr.scroll(id, ScrollBy::Rows(30)).unwrap();
        mgr.resize(id, 40, 100).unwrap();
        assert_eq!(
            mgr.screen(id).unwrap().scroll.offset,
            0,
            "重排之后偏移的含义就失效了，只能回底"
        );
    }

    #[test]
    fn scroll_to_bottom_works() {
        let dir = init_repo();
        let mgr = SessionManager::new();
        let id = scrolling_session(&mgr, dir.path(), 100);
        wait_for_screen(&mgr, id, "line-100");

        mgr.scroll(id, ScrollBy::Rows(30)).unwrap();
        let st = mgr.scroll(id, ScrollBy::Bottom).unwrap();
        assert_eq!(st.offset, 0);
    }

    #[test]
    fn new_lines_counts_only_what_arrived_since_the_user_last_scrolled() {
        let dir = init_repo();
        let mgr = SessionManager::new();
        let mut p = fake_agent();
        p.is_agent = false;
        p.command = vec![
            "/bin/sh".into(),
            "-c".into(),
            "i=1; while [ $i -le 60 ]; do echo line-$i; i=$((i+1)); done; \
             sleep 1; i=1; while [ $i -le 5 ]; do echo new-$i; i=$((i+1)); done; sleep 30"
                .into(),
        ];
        mgr.register_profile(p.clone());
        let id = mgr
            .create(dir.path().to_str().unwrap(), &p.name, empty_secrets(), 24, 80)
            .unwrap();
        wait_for_screen(&mgr, id, "line-60");

        // 刚滚完，底下没有新东西
        let st = mgr.scroll(id, ScrollBy::Rows(20)).unwrap();
        assert_eq!(st.new_lines, 0);

        wait_for_screen(&mgr, id, "new-5");
        let st = mgr.screen(id).unwrap().scroll;
        assert_eq!(st.new_lines, 5, "5 行新内容进来了，得数得出来");

        // 用户再滚一次，计数重新归零
        let st = mgr.scroll(id, ScrollBy::Rows(1)).unwrap();
        assert_eq!(st.new_lines, 0);
    }

    #[test]
    fn scrolling_a_session_that_does_not_exist_says_so() {
        let mgr = SessionManager::new();
        assert!(mgr.scroll(999, ScrollBy::Rows(1)).is_err());
    }
```

`wait_for_screen` 是新的辅助函数（`screen_text_for_test` 已经有了）：

```rust
    fn wait_for_screen(mgr: &SessionManager, id: u32, needle: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if mgr.screen_text_for_test(id).contains(needle) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("等不到 {needle}");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib session:: -- --test-threads=1`
Expected: 编译失败，`no method named 'scroll'`。

- [ ] **Step 3: 实现**

`src/session.rs`：把 `ScreenSnapshot` 从

```rust
pub type ScreenSnapshot = (Vec<Vec<ScreenSpan>>, (u16, u16));
```

改成 Interfaces 里那个结构体，并加 `ScrollState` / `ScrollBy`。

`Session` 加一个字段：

```rust
    /// 用户上次**主动**滚动时的偏移。`new_lines` 靠它算：vt100 会在新行
    /// 推入时自动把偏移 +1（grid.rs:556-558，画面因此不动），所以
    /// 「偏移 - 这个标记」就正好是用户没看过的行数。
    ///
    /// 边界：偏移增长被历史总行数封顶，缓冲满 2000 行之后 new_lines 会
    /// 少算，画面也会开始往上飘（最老的行被挤掉了）。这是环形缓冲的
    /// 固有代价。
    scroll_mark: usize,
```

三个方法：

```rust
    pub fn scroll(&self, id: u32, by: ScrollBy) -> Result<ScrollState> {
        self.with_session(id, |s| {
            let v = match by {
                ScrollBy::Rows(n) => s.pty.scroll_by(n),
                ScrollBy::Bottom => s.pty.scroll_to_bottom(),
            };
            // 用户主动滚过了，「没看过的行数」从这一刻重新算
            s.scroll_mark = v.offset;
            Ok(state_of(v, s.scroll_mark))
        })
    }

    pub fn screen(&self, id: u32) -> Result<ScreenSnapshot> {
        self.with_session(id, |s| {
            let v = s.pty.scroll_state();
            Ok(ScreenSnapshot {
                lines: s.pty.screen_spans(),
                cursor: s.pty.cursor(),
                scroll: state_of(v, s.scroll_mark),
            })
        })
    }
```

`send_input` 里，写 PTY **之前**：

```rust
            // 一敲键就回到底部。滚上去的时候打字，字会落在看不见的地方，
            // 用户会以为键盘坏了。归零之后字符照常送出去，不吞。
            s.pty.scroll_to_bottom();
            s.scroll_mark = 0;
```

`resize` 里，改尺寸**之后**：

```rust
            // vt100 会按新宽度重排，偏移指向的行跟改之前不是同一行了。
            // 与其显示一个错位的画面，不如老老实实回到底部。
            s.pty.scroll_to_bottom();
            s.scroll_mark = 0;
```

模块级私有函数：

```rust
fn state_of(v: crate::pty::ScrollView, mark: usize) -> ScrollState {
    ScrollState {
        agent_owns: v.agent_owns,
        alt_screen: v.alt_screen,
        max: v.max,
        offset: v.offset,
        new_lines: v.offset.saturating_sub(mark),
    }
}
```

改完 `ScreenSnapshot` 之后，`daemon.rs` 里构造 `Response::Screen` 的地方
会编译不过——Task 8 会处理，本任务先让它编译通过（照结构体字段取值即可）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 194 个，全绿。

- [ ] **Step 5: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 6: 提交**

```bash
git add src/session.rs src/daemon.rs
git commit -m "feat: 会话层贯通滚动状态，打字和改尺寸都回到底部

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: 协议加 `Scroll` 与 `Mouse`，版本 +1

**Files:**
- Modify: `src/proto.rs`（`PROTOCOL_VERSION`、`Request`、`Response::Screen`、手写 `Debug`）
- Modify: `src/daemon.rs`（`handle` 的两条新 arm、`Response::Screen` 的构造）
- Test: `src/proto.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::session::{ScrollState, ScrollBy}`（Task 7）
- Produces:

```rust
pub const PROTOCOL_VERSION: u32 = 2;   // 从 1 改过来

Request::Scroll { id: u32, by: ScrollBy },
Request::Mouse  { id: u32, event: MouseForward },

Response::Screen {
    lines: Vec<Vec<ScreenSpan>>,
    cursor: (u16, u16),
    #[serde(default)]
    scroll: ScrollState,
}
Response::Scrolled(ScrollState),

pub struct MouseForward {
    pub col: u16,
    pub row: u16,
    pub kind: MouseForwardKind,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

pub enum MouseForwardKind {
    WheelUp,
    WheelDown,
    Press(u8),    // 0=左 1=中 2=右
    Release(u8),
}
```

- [ ] **Step 1: 写失败的测试**

`src/proto.rs` 的测试模块：

```rust
    /// 新加的滚动字段必须能从「没有这个字段」的旧 JSON 里解出来。
    /// 这是 `#[serde(default)]` 的意义所在：往后再加字段就不用再动版本号。
    #[test]
    fn a_screen_response_without_scroll_still_parses() {
        let old = r#"{"Screen":{"lines":[],"cursor":[0,0]}}"#;
        let r: Response = serde_json::from_str(old).unwrap();
        match r {
            Response::Screen { scroll, .. } => {
                assert_eq!(scroll, crate::session::ScrollState::default());
            }
            _ => panic!("解成了别的变体"),
        }
    }

    #[test]
    fn scroll_requests_survive_a_round_trip() {
        for by in [ScrollBy::Rows(3), ScrollBy::Rows(-3), ScrollBy::Bottom] {
            let req = Request::Scroll { id: 7, by };
            let s = serde_json::to_string(&req).unwrap();
            let back: Request = serde_json::from_str(&s).unwrap();
            assert!(matches!(back, Request::Scroll { id: 7, .. }));
        }
    }

    /// 手写的 Debug 漏一条 arm 会编译不过，但漏了密钥脱敏不会。
    /// 顺手确认新变体没有把什么敏感东西带进 Debug。
    #[test]
    fn mouse_debug_has_no_surprises() {
        let req = Request::Mouse {
            id: 1,
            event: MouseForward {
                col: 10,
                row: 20,
                kind: MouseForwardKind::WheelUp,
                shift: false,
                alt: false,
                ctrl: false,
            },
        };
        let s = format!("{req:?}");
        assert!(s.contains("Mouse"));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib proto:: -- --test-threads=1`
Expected: 编译失败，`no variant named 'Scroll'`。

- [ ] **Step 3: 改协议**

`src/proto.rs`：`PROTOCOL_VERSION` 改成 `2`，并在它的文档注释末尾追加一行：

```rust
/// 第 2 版：`Response::Screen` 加了 `scroll`，`ScreenSnapshot` 从元组改成结构体。
```

加 Interfaces 里列的两个 `Request` 变体、`Response::Scrolled`、
`Response::Screen` 的 `scroll` 字段（带 `#[serde(default)]`）、
`MouseForward` / `MouseForwardKind`。手写 `Debug` 补两条 arm。

- [ ] **Step 4: 守护进程分派**

`src/daemon.rs` 的 `handle`：

```rust
        Request::Scroll { id, by } => mgr.scroll(id, by).map(Response::Scrolled),
        Request::Mouse { id, event } => mgr.forward_mouse(id, event).map(|_| Response::Ok),
```

`Response::Screen` 的构造改成取结构体字段：

```rust
        Request::Screen { id } => mgr.screen(id).map(|s| Response::Screen {
            lines: s.lines,
            cursor: s.cursor,
            scroll: s.scroll,
        }),
```

`mgr.forward_mouse` 在 Task 9 实现；本步先加一个占位实现让它编译：

```rust
    /// 把界面转发过来的鼠标事件按 agent 当前的编码写进 PTY。
    pub fn forward_mouse(&self, id: u32, ev: MouseForward) -> Result<()> {
        self.with_session(id, |s| s.pty.write_mouse(ev))
    }
```

`PtySession::write_mouse` 也在 Task 9 实现，本步给一个直接返回 `Ok(())`
的空壳并加 `// Task 9 实现` 注释。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 197 个，全绿。

- [ ] **Step 6: 手工验一次版本提示**

```bash
cargo build --release
# 让旧守护进程还跑着（第 1 版），用新界面连
./target/release/dct
```
Expected: 立刻打印「后台服务是第 1 版，界面是第 2 版……运行 dct restart」，
不是「拿不到 agent 列表」。

- [ ] **Step 7: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 8: 提交**

```bash
git add src/proto.rs src/daemon.rs src/session.rs src/pty.rs
git commit -m "feat: 协议加 Scroll 与 Mouse，版本升到 2

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: 鼠标事件编码

**Files:**
- Modify: `src/pty.rs`（`write_mouse` 与纯函数 `encode_mouse`）
- Test: `src/pty.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::proto::{MouseForward, MouseForwardKind}`（Task 8）
- Produces:

```rust
impl PtySession {
    pub fn write_mouse(&self, ev: MouseForward) -> Result<()>;
}

/// 纯函数，好测。`None` 表示这个 agent 现在不收鼠标，什么都别发。
pub fn encode_mouse(
    mode: vt100::MouseProtocolMode,
    enc: vt100::MouseProtocolEncoding,
    ev: &MouseForward,
) -> Option<Vec<u8>>;
```

**SGR 编码（`?1006`，Claude Code 用的就是这个）：**
`ESC [ < <按钮码> ; <列+1> ; <行+1> M`（按下/滚轮）或 `m`（抬起）。
按钮码：左 0、中 1、右 2；滚轮上 64、滚轮下 65。
修饰键往按钮码上加：Shift +4、Alt +8、Ctrl +16。

**默认编码（`?1000` 不带 `?1006`）：**
`ESC [ M <32+按钮码> <32+列+1> <32+行+1>`，三个都是单字节。
列或行超过 223 就编不下，这种情况**不发**（返回 `None`）——
发一个截断的坐标比不发更糟，agent 会以为你点在别的地方。

- [ ] **Step 1: 写失败的测试**

```rust
    use crate::proto::{MouseForward, MouseForwardKind};

    fn ev(kind: MouseForwardKind, col: u16, row: u16) -> MouseForward {
        MouseForward {
            col,
            row,
            kind,
            shift: false,
            alt: false,
            ctrl: false,
        }
    }

    #[test]
    fn sgr_encodes_a_wheel_scroll() {
        let out = encode_mouse(
            vt100::MouseProtocolMode::AnyMotion,
            vt100::MouseProtocolEncoding::Sgr,
            &ev(MouseForwardKind::WheelUp, 10, 20),
        )
        .unwrap();
        // 坐标是 1 起算的，所以 10,20 变成 11,21
        assert_eq!(out, b"\x1b[<64;11;21M".to_vec());
    }

    #[test]
    fn sgr_wheel_down_uses_a_different_button_code() {
        let out = encode_mouse(
            vt100::MouseProtocolMode::AnyMotion,
            vt100::MouseProtocolEncoding::Sgr,
            &ev(MouseForwardKind::WheelDown, 0, 0),
        )
        .unwrap();
        assert_eq!(out, b"\x1b[<65;1;1M".to_vec());
    }

    #[test]
    fn sgr_marks_release_with_a_lowercase_m() {
        let out = encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Sgr,
            &ev(MouseForwardKind::Release(0), 4, 5),
        )
        .unwrap();
        assert_eq!(out, b"\x1b[<0;5;6m".to_vec());
    }

    #[test]
    fn modifiers_are_added_to_the_button_code() {
        let mut e = ev(MouseForwardKind::Press(0), 0, 0);
        e.shift = true;
        e.ctrl = true;
        let out = encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Sgr,
            &e,
        )
        .unwrap();
        // 0 + 4(shift) + 16(ctrl) = 20
        assert_eq!(out, b"\x1b[<20;1;1M".to_vec());
    }

    #[test]
    fn default_encoding_uses_the_single_byte_form() {
        let out = encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Default,
            &ev(MouseForwardKind::Press(0), 10, 20),
        )
        .unwrap();
        // 32+0, 32+11, 32+21
        assert_eq!(out, vec![0x1b, b'[', b'M', 32, 43, 53]);
    }

    #[test]
    fn default_encoding_refuses_coordinates_it_cannot_express() {
        // 单字节形式最多到 223；发一个截断的坐标会让 agent 以为你点在别处
        assert!(encode_mouse(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Default,
            &ev(MouseForwardKind::Press(0), 300, 5),
        )
        .is_none());
    }

    #[test]
    fn nothing_is_sent_when_the_agent_does_not_want_the_mouse() {
        assert!(encode_mouse(
            vt100::MouseProtocolMode::None,
            vt100::MouseProtocolEncoding::Sgr,
            &ev(MouseForwardKind::WheelUp, 1, 1),
        )
        .is_none());
    }

    /// X10（`?1000` 不带 release）只上报按下。发一个抬起事件过去，
    /// agent 会收到一个它没订阅的东西。
    #[test]
    fn x10_mode_drops_release_events() {
        assert!(encode_mouse(
            vt100::MouseProtocolMode::Press,
            vt100::MouseProtocolEncoding::Sgr,
            &ev(MouseForwardKind::Release(0), 1, 1),
        )
        .is_none());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib pty:: -- --test-threads=1`
Expected: 编译失败，`cannot find function 'encode_mouse'`。

- [ ] **Step 3: 实现**

`src/pty.rs`：

```rust
/// 把一个鼠标事件编码成 agent 当前订阅的那种格式。
///
/// `None` 表示「什么都别发」，有三种情况：agent 根本没开鼠标上报；
/// 它只订阅了按下（X10）而这是个抬起；坐标大到默认编码装不下。
/// 三种都是「发出去比不发更糟」——agent 会收到它没订阅的东西，
/// 或者一个指向别处的坐标。
pub fn encode_mouse(
    mode: vt100::MouseProtocolMode,
    enc: vt100::MouseProtocolEncoding,
    ev: &crate::proto::MouseForward,
) -> Option<Vec<u8>> {
    use crate::proto::MouseForwardKind as K;
    use vt100::MouseProtocolMode as M;

    if mode == M::None {
        return None;
    }
    let is_release = matches!(ev.kind, K::Release(_));
    if is_release && mode == M::Press {
        return None;
    }

    let mut button = match ev.kind {
        K::WheelUp => 64,
        K::WheelDown => 65,
        K::Press(b) | K::Release(b) => u32::from(b),
    };
    if ev.shift {
        button += 4;
    }
    if ev.alt {
        button += 8;
    }
    if ev.ctrl {
        button += 16;
    }

    // 终端协议的坐标是 1 起算的，我们内部是 0 起算的
    let col = u32::from(ev.col) + 1;
    let row = u32::from(ev.row) + 1;

    match enc {
        vt100::MouseProtocolEncoding::Sgr => {
            let end = if is_release { 'm' } else { 'M' };
            Some(format!("\x1b[<{button};{col};{row}{end}").into_bytes())
        }
        // Utf8 跟 Default 的差别只在坐标怎么编码。按下界处理成
        // 「装不下就不发」对两者都成立，也就不用分开写。
        _ => {
            let b = 32u32.checked_add(button)?;
            let c = 32u32.checked_add(col)?;
            let r = 32u32.checked_add(row)?;
            if b > 255 || c > 255 || r > 255 {
                return None;
            }
            Some(vec![0x1b, b'[', b'M', b as u8, c as u8, r as u8])
        }
    }
}
```

`PtySession` 加：

```rust
    /// 把鼠标事件按 agent 当前的模式写进 PTY。它不收鼠标就什么都不做——
    /// 这是正常情况，不是错误。
    pub fn write_mouse(&self, ev: crate::proto::MouseForward) -> Result<()> {
        let bytes = {
            let parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
            let screen = parser.screen();
            encode_mouse(
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
                &ev,
            )
        };
        match bytes {
            Some(b) => self.write(&b),
            None => Ok(()),
        }
    }
```

> 锁在 `encode_mouse` 之后就放掉了（那个块结束），`self.write()` 拿的是另一把锁。
> 同时握两把是死锁的开始，这个仓库在 `create()` 上已经吃过一次「持锁做慢操作」
> 的亏，别在这里重犯。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 205 个，全绿。

- [ ] **Step 5: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 6: 提交**

```bash
git add src/pty.rs
git commit -m "feat: 鼠标事件按 agent 订阅的编码转发

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: 会话视图接上滚动

**Files:**
- Modify: `src/ui/mod.rs`（`restore_terminal`、事件循环收 `Event::Mouse`）
- Modify: `src/ui/attach.rs`（路由、按键、底栏提示）
- Modify: `src/ui/app.rs`（存一份最近的 `ScrollState`）
- Test: `src/ui/attach.rs`

**Interfaces:**
- Consumes: `crate::session::ScrollState`（Task 7）、`crate::proto::{Request, ScrollBy, MouseForward, MouseForwardKind}`（Task 8）
- Produces:

```rust
/// 一次滚轮/翻页该做什么。纯函数，好测。
pub(crate) enum ScrollAction {
    /// 转发给 agent
    Forward,
    /// dct 自己滚这么多行
    Scroll(i32),
    /// 什么都不做
    Ignore,
}

pub(crate) fn wheel_action(st: &ScrollState, up: bool) -> ScrollAction;
pub(crate) fn key_scroll(st: &ScrollState, key: &KeyEvent, page: u16) -> Option<ScrollAction>;
pub(crate) fn scroll_hint(st: &ScrollState) -> Option<String>;
```

**步长：** 滚轮一格 3 行（终端惯例）；`PageUp`/`PageDown` 一屏减 2 行；
`End` 回底。三个都只在 `!agent_owns` 时由 dct 处理。

- [ ] **Step 1: 写失败的测试**

`src/ui/attach.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ScrollState;

    /// `key()` 在 Task 3 里跟着 View 的测试搬进了 `ui/view.rs`。这里要用，
    /// 就把它在 `view.rs` 的测试模块里标成 `pub(crate)`，别复制一份——
    /// 两份同名辅助函数迟早会漂成两个语义。
    use crate::ui::view::tests::key;

    fn own(agent_owns: bool, max: usize, offset: usize, new_lines: usize) -> ScrollState {
        ScrollState {
            agent_owns,
            alt_screen: false,
            max,
            offset,
            new_lines,
        }
    }

    #[test]
    fn an_agent_that_wants_the_mouse_gets_the_wheel() {
        assert!(matches!(
            wheel_action(&own(true, 0, 0, 0), true),
            ScrollAction::Forward
        ));
    }

    #[test]
    fn otherwise_dct_scrolls_three_rows_per_notch() {
        assert!(matches!(
            wheel_action(&own(false, 500, 0, 0), true),
            ScrollAction::Scroll(3)
        ));
        assert!(matches!(
            wheel_action(&own(false, 500, 10, 0), false),
            ScrollAction::Scroll(-3)
        ));
    }

    #[test]
    fn there_is_nothing_to_scroll_when_there_is_no_history() {
        assert!(matches!(
            wheel_action(&own(false, 0, 0, 0), true),
            ScrollAction::Ignore
        ));
    }

    #[test]
    fn page_keys_belong_to_the_agent_when_it_owns_the_viewport() {
        // None 表示「不归我管」，让它落到普通按键路径送给 agent
        assert!(key_scroll(&own(true, 0, 0, 0), &key(KeyCode::PageUp), 24).is_none());
        assert!(key_scroll(&own(true, 0, 0, 0), &key(KeyCode::End), 24).is_none());
    }

    #[test]
    fn page_keys_scroll_a_screen_minus_two() {
        let up = key_scroll(&own(false, 500, 0, 0), &key(KeyCode::PageUp), 24).unwrap();
        assert!(matches!(up, ScrollAction::Scroll(22)));
        let down = key_scroll(&own(false, 500, 30, 0), &key(KeyCode::PageDown), 24).unwrap();
        assert!(matches!(down, ScrollAction::Scroll(-22)));
    }

    #[test]
    fn a_tiny_window_still_scrolls_at_least_one_row() {
        let up = key_scroll(&own(false, 500, 0, 0), &key(KeyCode::PageUp), 2).unwrap();
        assert!(matches!(up, ScrollAction::Scroll(1)), "别算出 0 行或负数");
    }

    #[test]
    fn ordinary_keys_are_not_scroll_keys() {
        assert!(key_scroll(&own(false, 500, 10, 0), &key(KeyCode::Char('a')), 24).is_none());
    }

    #[test]
    fn the_hint_says_how_much_is_waiting_below() {
        let h = scroll_hint(&own(false, 500, 40, 12)).unwrap();
        assert!(h.contains("12"), "得说清有多少新东西: {h}");
    }

    #[test]
    fn the_hint_says_how_to_get_back_when_nothing_is_new() {
        let h = scroll_hint(&own(false, 500, 40, 0)).unwrap();
        assert!(h.contains("40"), "得说清翻了多远: {h}");
        assert!(h.contains("End"), "得说清怎么回去: {h}");
    }

    #[test]
    fn an_alt_screen_agent_that_ignores_the_mouse_gets_an_explanation() {
        let mut st = own(false, 0, 0, 0);
        st.alt_screen = true;
        let h = scroll_hint(&st).expect("这种情况谁都滚不了，必须说一声");
        assert!(!h.contains("End"), "都滚不了了就别提回底部: {h}");
        assert!(
            !h.contains("备用屏") && !h.contains("scrollback"),
            "不能有黑话: {h}"
        );
    }

    #[test]
    fn a_fresh_session_with_no_history_says_nothing() {
        assert!(scroll_hint(&own(false, 0, 0, 0)).is_none());
        assert!(scroll_hint(&own(true, 0, 0, 0)).is_none());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib ui::attach -- --test-threads=1`
Expected: 编译失败，`cannot find function 'wheel_action'`。

- [ ] **Step 3: 实现三个纯函数**

`src/ui/attach.rs`：

```rust
/// 滚轮一格滚几行。3 是终端惯例，改了会跟用户在别处的肌肉记忆打架。
const WHEEL_ROWS: i32 = 3;

pub(crate) enum ScrollAction {
    Forward,
    Scroll(i32),
    Ignore,
}

/// 谁拿这一格滚轮。
///
/// 判据是「agent 有没有开鼠标上报」，不是「它在不在备用屏」——实测下来
/// Claude Code 备用屏 + 全套鼠标，codex 内联 + 完全不要鼠标，两个真实
/// agent 在这两个维度上正好相反。按鼠标分流恰好把两边都送到握着内容的
/// 那一方：Claude Code 自己管视口，codex 的历史在 dct 缓冲里。
pub(crate) fn wheel_action(st: &ScrollState, up: bool) -> ScrollAction {
    if st.agent_owns {
        return ScrollAction::Forward;
    }
    if st.max == 0 {
        return ScrollAction::Ignore;
    }
    ScrollAction::Scroll(if up { WHEEL_ROWS } else { -WHEEL_ROWS })
}

/// 翻页键归谁。`None` = 不归 dct 管，让它落到普通按键路径送给 agent。
pub(crate) fn key_scroll(st: &ScrollState, key: &KeyEvent, page: u16) -> Option<ScrollAction> {
    if st.agent_owns {
        return None;
    }
    // 一屏减 2 行：留两行重叠，翻页之后还能看到上一屏的尾巴，
    // 不然读长输出时每翻一页都要重新找位置。窗口太小时至少滚 1 行。
    let step = i32::from(page).saturating_sub(2).max(1);
    match key.code {
        KeyCode::PageUp => Some(ScrollAction::Scroll(step)),
        KeyCode::PageDown => Some(ScrollAction::Scroll(-step)),
        KeyCode::End if st.offset > 0 => Some(ScrollAction::Scroll(-i32::MAX)),
        _ => None,
    }
}

/// 底栏那一句。`None` = 不显示。
pub(crate) fn scroll_hint(st: &ScrollState) -> Option<String> {
    if st.offset > 0 && st.new_lines > 0 {
        return Some(format!("↓ 下面还有 {} 行新内容", st.new_lines));
    }
    if st.offset > 0 {
        return Some(format!("↑ 已往上翻 {} 行 · 按 End 回到底部", st.offset));
    }
    // agent 自己占着画面又不收鼠标：谁都滚不了。装死的话用户会以为
    // 滚轮坏了，一直试。
    if !st.agent_owns && st.alt_screen {
        return Some("这个 agent 自己管画面，翻不了历史".to_string());
    }
    None
}
```

> `End` 用 `Scroll(-i32::MAX)` 而不是单独一个 `Bottom` 分支：
> `ScrollBy::Rows` 在守护进程侧是 `saturating_sub` 之后再钳到 `[0, max]`，
> 结果跟 `ScrollBy::Bottom` 完全一样，多一个分支只是多一处要测的东西。

- [ ] **Step 4: 接上事件循环**

`src/ui/app.rs` 加一个字段：

```rust
    /// 最近一次 Screen 响应带回来的滚动状态。按键和滚轮都要看它分流，
    /// 而它每帧都会被刷新——滞后最多一帧，够用了。
    pub scroll: ScrollState,
```

`src/ui/mod.rs`：

1. `Event::Key` 之前先收 `Event::Mouse`：

```rust
        if let Event::Mouse(m) = ev {
            attach::handle_mouse(&mut app, m)?;
            continue;
        }
```

（这个 `continue` 在按键处理**之前**，不在任何按键分支里，不违反房规。
但循环末尾清理 `message` 的那段也会被它跳过——所以 `handle_mouse` 里
**不许**改 `app.message`。把这句话写成注释放在它上面。）

2. 进出会话时开关鼠标捕获：

```rust
// 进入 View::Attached 时
execute!(std::io::stdout(), EnableMouseCapture)?;
// 离开 View::Attached 时
execute!(std::io::stdout(), DisableMouseCapture)?;
```

只在会话里开：看板不需要滚，而开着捕获会让终端原生的选中复制失效——
把这个代价限制在真正需要它的地方。

3. `restore_terminal()` 里无条件加 `DisableMouseCapture`：

```rust
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
}
```

没开过捕获时多发一次关闭序列是无害的，而漏关会让用户的终端从此点哪儿都
冒出乱码。`TerminalGuard::drop` 和 `spawn_signal_restore` 都走这个函数，
所以所有退出路径（正常退出、`?` 提前返回、panic、SIGTERM）自动覆盖。

4. `attach.rs` 里实现 `handle_mouse`：

```rust
/// **这个函数里不许改 `app.message`。** 主循环在调用它之后直接
/// `continue`，跳过了循环末尾清理陈旧消息的那一段——在这里设一条消息，
/// 它会一直挂在屏幕上直到下一次按键。
pub(crate) fn handle_mouse(app: &mut App, m: MouseEvent) -> Result<()> {
    let View::Attached(id) = app.view else {
        return Ok(());
    };
    let (up, forwardable) = match m.kind {
        MouseEventKind::ScrollUp => (true, Some(MouseForwardKind::WheelUp)),
        MouseEventKind::ScrollDown => (false, Some(MouseForwardKind::WheelDown)),
        MouseEventKind::Down(b) => (false, Some(MouseForwardKind::Press(button_code(b)))),
        MouseEventKind::Up(b) => (false, Some(MouseForwardKind::Release(button_code(b)))),
        // 纯移动不转发：Claude Code 开了 ?1003h，每动一下就是一个事件，
        // 全部经 socket 转发过去量很大，换来的只是悬停高亮。这是有意的
        // 部分实现，不是遗漏。
        _ => (false, None),
    };
    let Some(kind) = forwardable else {
        return Ok(());
    };

    let is_wheel = matches!(kind, MouseForwardKind::WheelUp | MouseForwardKind::WheelDown);
    if is_wheel {
        match wheel_action(&app.scroll, up) {
            ScrollAction::Ignore => return Ok(()),
            ScrollAction::Scroll(n) => {
                let _ = app.client()?.call(Request::Scroll {
                    id,
                    by: ScrollBy::Rows(n),
                });
                return Ok(());
            }
            ScrollAction::Forward => {}
        }
    } else if !app.scroll.agent_owns {
        // 不收鼠标的 agent 收到点击事件只会看到一串乱码
        return Ok(());
    }

    // 终端坐标减掉边框，换算成 agent 画面里的坐标
    let Some((col, row)) = app.screen_origin.and_then(|(c0, r0)| {
        Some((m.column.checked_sub(c0)?, m.row.checked_sub(r0)?))
    }) else {
        return Ok(());
    };

    let _ = app.client()?.call(Request::Mouse {
        id,
        event: MouseForward {
            col,
            row,
            kind,
            shift: m.modifiers.contains(KeyModifiers::SHIFT),
            alt: m.modifiers.contains(KeyModifiers::ALT),
            ctrl: m.modifiers.contains(KeyModifiers::CONTROL),
        },
    });
    Ok(())
}

fn button_code(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}
```

`App` 再加一个字段 `pub screen_origin: Option<(u16, u16)>`，由 `attach::draw`
在每帧画完之后填上会话内容区左上角的终端坐标。**它必须由 `draw` 填，
不能在 `handle_mouse` 里硬算边框宽度**——布局改了硬算的数就错了，而且
错得很安静。

5. 会话按键路径里，在把按键交给 `key_to_input` 之前先问一次 `key_scroll`：

```rust
    if let Some(action) = key_scroll(&app.scroll, &key, content_rows) {
        if let ScrollAction::Scroll(n) = action {
            let _ = app.client()?.call(Request::Scroll {
                id,
                by: ScrollBy::Rows(n),
            });
        }
        return Ok(());
    }
```

6. `attach::draw` 把 `scroll_hint` 的结果画到底栏。它跟 `message` 抢同一行时
**`message` 优先**——消息是对用户刚才那个动作的回应，滚动提示是持续状态，
盖掉前者会让用户以为自己那步操作没反应。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 217 个，全绿。

- [ ] **Step 6: 手工验收（自动化不了，必须真跑）**

```bash
cargo build --release
./target/release/dct restart
./target/release/dct
```

1. 开一个 codex 会话，让它吐一屏以上的东西，滚轮往上：
   Expected: 看得到历史；画面不花；底部状态条不被拽进内容区；
   底栏出现「↑ 已往上翻 N 行 · 按 End 回到底部」。
2. 保持滚上去的状态，让 codex 再输出几行：
   Expected: 画面**不动**，底栏变成「↓ 下面还有 N 行新内容」。
3. 这时候敲一个字符：
   Expected: 立刻跳回底部，而且那个字符确实进了 codex 的输入框。
4. `PageUp` / `PageDown` / `End`：Expected: 分别翻一屏、翻回、回底。
5. 开一个 Claude Code 会话，滚轮往上：
   Expected: 滚的是 **Claude Code 自己的对话记录**，不是 dct 的缓冲；
   底栏不出现任何滚动提示。
6. 从会话退回看板，用鼠标在终端里拖选文字：
   Expected: 能选中（说明捕获确实关掉了）。
7. `Ctrl+C` 掉整个 dct，然后在终端里点几下：
   Expected: 不冒乱码（说明 `restore_terminal` 关掉了捕获）。

- [ ] **Step 7: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 8: 提交**

```bash
git add -A src/ui
git commit -m "feat: 会话里能往回翻历史了

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: 两份 README 跟上

**Files:**
- Modify: `README.md:90`、`README.md:149`、`README.md:104-116`
- Modify: `README.zh-CN.md:90`、`README.zh-CN.md:149`、`README.zh-CN.md:104-116`

**Interfaces:**
- Consumes: 前十个任务的成果
- Produces: 无代码

**语气要求：** 这两份 README 刚重写过，就是为了不像 AI 写的。第一人称、
承认缺点、不堆形容词、不用「无缝」「强大」「轻松」这类词。改动要跟周围
一个调子。

- [ ] **Step 1: 删掉「滚不了」那条，换成新的代价**

`README.md:90` 现在是：

```
Scrolling back doesn't work yet, and in iTerm2 it actively garbles the screen.
Scroll to the bottom and it repaints. The underlying reason is that `dct`
currently keeps zero scrollback, so there's nothing to scroll to; that's on the list.
```

换成：

```
Scrolling back works now, but it cost you something: while you're inside a
session, dct grabs the mouse, so your terminal's own click-and-drag text
selection stops working. In iTerm2 you hold Option to get it back; most
terminals have some equivalent. dct has no copy of its own yet. Back on the
board the mouse is yours again.
```

`README.zh-CN.md:90` 换成：

```
往回滚屏能用了，但它是有代价的：进了会话之后 dct 会接管鼠标，终端自己的
拖动选中就失灵了。iTerm2 里按住 Option 能拿回来，别的终端一般也有对应的
修饰键。dct 目前还没有自己的复制功能。退回看板鼠标就还给你了。
```

- [ ] **Step 2: 「还没做的」里删掉滚屏**

两份文件的最后一段（`:149`）里都有 `Scrollback` / `滚屏历史`，删掉这一项，
其余不动。

- [ ] **Step 3: 文件清单补三个新模块**

两份文件的 `src/` 清单（`:104-116`）里，`src/ui.rs` 那一行换成：

```
src/ui/          the TUI — one module per view, plus App and the shared widgets
src/restart.rs   dct restart
```

中文版：

```
src/ui/          界面：一个视图一个模块，外加 App 和公用的小部件
src/restart.rs   dct restart
```

- [ ] **Step 4: 看板键表加一行**

两份文件的键表里加 `dct restart` 说明不合适（那是命令不是键），改为在
「跑起来」那一节末尾加一句：

英文：

```
If you upgrade dct and it tells you the background service is out of date,
run `dct restart`. It lists whatever is still running and asks before killing anything.
```

中文：

```
升级完 dct 之后如果它说后台服务版本对不上，运行 `dct restart`。
它会先把还在跑的会话列出来，问过你再动手。
```

- [ ] **Step 5: 通读一遍**

Run: `git diff README.md README.zh-CN.md`
检查：两份内容对得上；没有 emoji；没有「无缝」「强大」这类词；
中文版读起来不像从英文直译的。

- [ ] **Step 6: 提交**

```bash
git add README.md README.zh-CN.md
git commit -m "docs: README 跟上滚屏与 dct restart

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## 自查记录

**规格覆盖**

| 设计文档 | 任务 |
|---|---|
| 0.1 协议握手版本号 | Task 1 |
| 0.1 `dct restart` | Task 2 |
| 0.2 第一步：模块拆分 | Task 3、Task 5 |
| 0.2 第二步：`App` | Task 4 |
| 一、路由规则 | Task 6（`agent_owns`）、Task 10（`wheel_action` / `key_scroll`） |
| 一、第四种情况的提示 | Task 10（`scroll_hint`） |
| 一、坐标换算与编码分工 | Task 9（编码）、Task 10（换算） |
| 一、不转发纯移动 | Task 10（`handle_mouse` 的 `_ => (false, None)`） |
| 二、2000 行 | Task 6（`SCROLLBACK_ROWS`） |
| 二、钉住 | Task 6（`the_view_stays_put_when_new_output_arrives`） |
| 二、`new_lines` | Task 7 |
| 二、滚动区的坑 | Task 6（`a_scroll_region_swallows_the_history`） |
| 三、状态在守护进程 | Task 7 |
| 三、`Scroll` / `Mouse` / `ScrollState` | Task 8 |
| 四、打字与改尺寸归零 | Task 7 |
| 五、键位步长 | Task 10 |
| 五、捕获只在会话里开 | Task 10 |
| 五、代价写进 README | Task 11 |
| 六、全部测试项 | 各任务的 Step 1 |
| 七、明确不做 | 无任务（就是不做） |

无遗漏。

**类型一致性**

- `ScrollView`（pty 层，Task 6）→ `ScrollState`（协议层，Task 7 用 `state_of` 转换，多一个 `new_lines`）。两个名字不同是故意的：`new_lines` 要跨帧记忆，pty 层没有那个上下文。
- `ScrollBy` 在 Task 7 定义（`session.rs`），Task 8 在协议里引用同一个类型，不另建。
- `MouseForward` / `MouseForwardKind` 在 Task 8 定义（`proto.rs`），Task 9、10 引用。
- `PROTOCOL_VERSION` Task 1 建为 1，Task 8 改为 2。Task 1 的测试用的是常量本身，不写死数字，改版本不会让它们变红。

**测试数量推演**：172（起点）→ 175（T1）→ 178（T2）→ 178（T3 纯搬家）→ 180（T4）→ 180（T5 纯搬家）→ 189（T6）→ 194（T7）→ 197（T8）→ 205（T9）→ 217（T10）。每个任务的 Step「跑测试」里都写了预期数字，对不上就说明搬漏了或漏写了。
