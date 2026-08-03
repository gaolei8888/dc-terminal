# dct 逃生路径 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让用户在任何情况下都能退出 dct，并且退出后终端是干净的。

**Architecture:** 三处互不依赖的改动。(1) `ui::run` 屏蔽 SIGTERM/SIGINT/SIGHUP 并起一条 `sigwait` 线程，收到信号时在普通线程上下文里还原终端再 `_exit`；清理逻辑抽成 `restore_terminal()`，`TerminalGuard::drop` 和信号线程共用。(2) 在按键循环里、`match view.clone()` **之前**加一道 Ctrl+Q 全局拦截，语义是「退一层」。(3) 底部状态栏横向拆两段，左段是逃生键提示且永不让位，消息和断连提示只能占右段。

**Tech Stack:** Rust 2021、crossterm 0.28、ratatui 0.28、libc 0.2.189、portable-pty 0.8（仅测试）。**不新增任何依赖。**

**规格：** `docs/superpowers/specs/2026-08-03-dct-escape-hatch-design.md`

## Global Constraints

- **不新增依赖。** 需要的 crate（`libc`、`crossterm`、`ratatui`、`portable-pty`）都已在 `Cargo.toml` 里。
- **UI 文案不出现程序员黑话。** 用户是非程序员：写「退出」不写「terminate」，写「回看板」不写「detach」。
- **注释用中文，写「为什么」不写「是什么」。** 跟仓库现有风格一致——每处不显然的决定都要留下它的理由和被否掉的替代方案。
- **验收命令**（README 第 74-77 行）：
  ```
  cargo test -- --test-threads=1
  cargo fmt --check
  cargo clippy --all-targets
  ```
  `--test-threads=1` 不能省：集成测试会拉起真的守护进程和 pty，并发跑会互相干扰。
- **Ctrl+C 必须继续透传给 agent。** Claude Code 靠它中断。raw mode 下 Ctrl+C 不产生 SIGINT，所以屏蔽 SIGINT 不影响这条。
- **F2 保留。** 老用户的肌肉记忆，Ctrl+Q 是**增加**不是**替换**。

## File Structure

| 文件 | 责任 | 改动 |
|---|---|---|
| `src/ui.rs` | TUI 全部逻辑 | 修改：抽 `restore_terminal()`、加 `spawn_signal_restore()`、加 Ctrl+Q 拦截、底栏拆两段 |
| `tests/signal_restore.rs` | 「信号之后终端真的还回去了」的验收 | 新建 |
| `README.md` | 用户可见的按键表 | 修改：补 Ctrl+Q |

`src/ui.rs` 已经一千多行，但仓库的既定形态就是「TUI 全在 ui.rs」，本次三处改动都紧贴既有代码（`TerminalGuard` 旁边、按键循环里、`draw` 里），拆文件反而割裂上下文。不动结构。

---

### Task 1: 信号也能还原终端

**Files:**
- Create: `tests/signal_restore.rs`
- Modify: `src/ui.rs:83-99`（`TerminalGuard`）、`src/ui.rs:101-115`（`run` 开头）

**Interfaces:**
- Produces: `fn restore_terminal()`（模块私有，无参无返回，幂等）；`fn spawn_signal_restore()`（模块私有，无参无返回，在 `enable_raw_mode` 之前调用一次）
- Consumes: 无

**背景（实现者必读）：** `TerminalGuard` 靠 `Drop` 兜底，覆盖了提前 `return`/`?`/panic 三条路径。信号不走栈展开，`Drop` 根本不跑。于是用户「退不出去只好去别的窗口 kill」之后，原来那个终端停在 raw mode + alternate screen，回显和行缓冲全关，看上去像第二次卡死。关终端窗口和 tmux 杀 pane 走 SIGHUP，同样漏。

- [ ] **Step 1: 写会失败的集成测试**

新建 `tests/signal_restore.rs`。用 `libc::openpty` 而不是 `portable_pty`，因为我们需要**自己持有 slave 的 fd** 才能在信号之后读回 termios——`portable_pty` 不暴露原始 fd。

```rust
//! kill 掉 TUI 之后，终端必须还是能用的。
//!
//! `TerminalGuard` 的 `Drop` 盖不住信号：SIGTERM/SIGHUP 直接终止进程，不展开栈。
//! 少了这条保障，用户「退不出去只好去别的窗口 kill」之后会拿到一个停在
//! raw mode 的终端——回显和行缓冲全关，看上去像第二次卡死，得知道敲 `reset`
//! 才救得回来。而「知道敲 reset」正是不该要求用户具备的知识。

use std::os::unix::io::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// 把测试用的二进制复制成一个独一无二的名字，收尾时按名字杀进程
/// 不会误伤开发机上真正在跑的 dct 守护进程。
fn unique_binary(dir: &Path, tag: &str) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_BIN_EXE_dct"));
    let dst = dir.join(format!("dct-{tag}-probe-{}", std::process::id()));
    std::fs::copy(&src, &dst).unwrap();
    dst
}

fn wait_for(mut cond: impl FnMut() -> bool, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        sleep(Duration::from_millis(100));
    }
    false
}

/// raw mode 的特征：回显关、行缓冲关。任一为真就说明终端还没还回来。
unsafe fn is_raw(fd: libc::c_int) -> bool {
    let mut t: libc::termios = std::mem::zeroed();
    assert_eq!(libc::tcgetattr(fd, &mut t), 0, "读不到 termios");
    (t.c_lflag & libc::ECHO) == 0 || (t.c_lflag & libc::ICANON) == 0
}

/// 在一个我们自己持有 fd 的 pty 里把 dct 跑起来，返回 (子进程, master fd, slave fd)。
///
/// 必须 `setsid` + `TIOCSCTTY`：crossterm 的 `enable_raw_mode` 优先操作
/// `/dev/tty`，也就是**控制终端**。不给子进程把这个 pty 设成控制终端的话，
/// 它动的是 cargo test 自己的终端，这个测试就测了个寂寞。
fn spawn_dct_in_pty(bin: &Path, home: &Path, cwd: &Path) -> (std::process::Child, libc::c_int, libc::c_int) {
    let (mut master, mut slave) = (0, 0);
    unsafe {
        assert_eq!(
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            0,
            "openpty 失败"
        );
    }

    let (si, so, se) = unsafe {
        (
            Stdio::from_raw_fd(libc::dup(slave)),
            Stdio::from_raw_fd(libc::dup(slave)),
            Stdio::from_raw_fd(libc::dup(slave)),
        )
    };

    let mut cmd = Command::new(bin);
    cmd.current_dir(cwd)
        .env("HOME", home)
        .env("TERM", "xterm-256color")
        .stdin(si)
        .stdout(so)
        .stderr(se);
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(slave, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().expect("拉不起 dct");
    (child, master, slave)
}

/// 等 TUI 真的进了 raw mode。不等就发信号的话，测试可能在它还没设置终端时
/// 就把它杀了——那样即使有 bug 也会「通过」。
fn wait_until_raw(slave: libc::c_int) -> bool {
    wait_for(|| unsafe { is_raw(slave) }, 20)
}

#[test]
fn sigterm_restores_the_terminal() {
    let home = tempfile::tempdir().unwrap();
    let workdir = tempfile::tempdir().unwrap();
    let bin = unique_binary(home.path(), "sigterm");

    let (mut child, master, slave) = spawn_dct_in_pty(&bin, home.path(), workdir.path());
    assert!(wait_until_raw(slave), "TUI 始终没进 raw mode，测试前提不成立");

    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    assert!(
        wait_for(|| child.try_wait().map(|s| s.is_some()).unwrap_or(false), 10),
        "SIGTERM 之后 TUI 应当退出"
    );

    assert!(
        !unsafe { is_raw(slave) },
        "SIGTERM 之后终端仍停在 raw mode——用户会拿到一个不回显、不换行的死终端"
    );

    unsafe {
        libc::close(master);
        libc::close(slave);
    }
}

#[test]
fn sighup_restores_the_terminal() {
    let home = tempfile::tempdir().unwrap();
    let workdir = tempfile::tempdir().unwrap();
    let bin = unique_binary(home.path(), "sighup");

    let (mut child, master, slave) = spawn_dct_in_pty(&bin, home.path(), workdir.path());
    assert!(wait_until_raw(slave), "TUI 始终没进 raw mode，测试前提不成立");

    // 关终端窗口、tmux 杀 pane 走的都是这条
    unsafe { libc::kill(child.id() as i32, libc::SIGHUP) };
    assert!(
        wait_for(|| child.try_wait().map(|s| s.is_some()).unwrap_or(false), 10),
        "SIGHUP 之后 TUI 应当退出"
    );

    assert!(
        !unsafe { is_raw(slave) },
        "SIGHUP 之后终端仍停在 raw mode"
    );

    unsafe {
        libc::close(master);
        libc::close(slave);
    }
}
```

- [ ] **Step 2: 跑测试，确认它以正确的理由失败**

```
cargo test --test signal_restore -- --test-threads=1
```

预期：两条都 FAIL，失败信息是 `SIGTERM 之后终端仍停在 raw mode…` / `SIGHUP 之后终端仍停在 raw mode`。

**如果失败信息是 `TUI 始终没进 raw mode，测试前提不成立`，先停下来修测试** —— 说明 pty/控制终端那套没搭对，此时即使实现写好了测试也不会变绿，它证明不了任何事。

- [ ] **Step 3: 抽出 `restore_terminal()`**

改 `src/ui.rs:83-99`。原来 `TerminalGuard::drop` 里的两步原样搬进新函数，`drop` 改成调它：

```rust
/// 还原终端：退出 raw mode、关掉括号粘贴、离开 alternate screen。
///
/// 抽成自由函数是因为有两个调用方——`TerminalGuard::drop` 和信号线程。
/// 两份各自维护的清理代码迟早会漂移，而漂移的后果是用户拿到一个半还原的终端。
///
/// 两步都 `let _ =` 吞错：`Drop` 里不能 panic，而且这里能做的补救本来就只有
/// 「尽量多还原一点」。
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
}

/// 兜底恢复终端状态。ratatui 的 `Terminal` 不会在 `Drop` 里自动退出 raw
/// mode / alternate screen；`run()` 的主循环里到处都是 `?`，一旦某次
/// `client.call`/`term.draw` 出错就会直接从函数返回，跳过写在循环末尾的清理代码，
/// 把用户的终端卡在 raw mode（回显、行缓冲全关）。这个 guard 保证不管是提前
/// `return`/`?`、正常 `break`，还是 panic 展开，`Drop` 都会跑一次。
///
/// 它盖不住的只剩信号——那条交给 `spawn_signal_restore`。
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}
```

- [ ] **Step 4: 加信号线程**

同样在 `src/ui.rs`，紧跟 `TerminalGuard` 之后：

```rust
/// 让 SIGTERM / SIGINT / SIGHUP 也能还原终端。
///
/// 为什么不是信号 handler：handler 里能调的函数必须 async-signal-safe，而
/// crossterm 的 `disable_raw_mode()` 内部要锁一把全局 Mutex 去取原始 termios——
/// 信号打断的正好是持锁的主线程时就死锁。`sigwait` 在普通线程上下文里返回，
/// 之后跑的是普通代码，这个约束整个消失，也才谈得上跟 `TerminalGuard` 共用
/// 同一个 `restore_terminal()`。
///
/// 为什么不是「置个标志位让主循环自己退」：主循环卡在 `client.call` 上
/// （守护进程死了、socket 不回）时永远轮不到下一个 tick，而那正是用户会去
/// 别的窗口 kill 的场景——恰好是最需要它工作的时候不工作。
///
/// 屏蔽掩码会被子进程继承（`execve` 之后仍保留），但这里不用担心：TUI 进程
/// 在 `run()` 里不 fork 任何东西，PTY 全在守护进程里（`src/pty.rs`），而守护
/// 进程在 `src/main.rs:60` 就已经拉起，早于 `src/main.rs:72` 的 `ui::run`。
///
/// raw mode 下 Ctrl+C 不产生 SIGINT（termios 关了 ISIG），所以屏蔽 SIGINT
/// 不影响 Ctrl+C 透传给 agent；这条只对外部 `kill -INT` 生效。
fn spawn_signal_restore() {
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGHUP);
        // 主线程先屏蔽，之后 spawn 出来的线程继承这份掩码，
        // 于是这三个信号只会被下面的 sigwait 取走，不会走默认处置直接杀进程。
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }

    std::thread::spawn(move || {
        let mut signo: libc::c_int = 0;
        if unsafe { libc::sigwait(&set, &mut signo) } != 0 {
            return;
        }
        restore_terminal();
        // 不能用 `exit`：它会跑 atexit 和静态析构，而主线程此刻还在跑自己的事，
        // 两边可能同时清理终端或撞上同一把锁。终端已经在上一行还原好了，立刻走人。
        // 退出码 128 + signo 是 shell 惯例，SIGTERM 就是 143，脚本还能判断死因。
        unsafe { libc::_exit(128 + signo) };
    });
}
```

- [ ] **Step 5: 在 `run()` 里装上**

改 `src/ui.rs` 里 `enable_raw_mode()?;` 那一行的**上方**，加一行调用：

```rust
    // 必须在 enable_raw_mode 之前装：装早了无害（还没进 raw mode 时
    // restore_terminal() 没有副作用，多发一次 LeaveAlternateScreen 也无害），
    // 装晚了就有一个「已经进 raw mode 但信号还没被接管」的真空窗口。
    // 跟 TerminalGuard 提前构造是同一个理由。
    spawn_signal_restore();
    enable_raw_mode()?;
```

- [ ] **Step 6: 跑测试，确认变绿**

```
cargo test --test signal_restore -- --test-threads=1
```

预期：`sigterm_restores_the_terminal` 和 `sighup_restores_the_terminal` 都 PASS。

- [ ] **Step 7: 跑全量检查**

```
cargo test -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets
```

预期：全绿。特别确认 `tests/daemon_detach.rs` 仍然通过——它同样依赖 TUI 在 SIGHUP 下的行为（`drop(pty.master)` 之后断言 TUI 退出），信号被接管后那条路径变成了「sigwait 收到 SIGHUP → `_exit(129)`」，仍然是退出，断言应当继续成立。

- [ ] **Step 8: 提交**

```bash
git add src/ui.rs tests/signal_restore.rs
git commit -m "fix: kill 掉 dct 之后终端不再停在 raw mode

TerminalGuard 的 Drop 盖不住信号。用户退不出去只好去别的窗口 kill，
之后原来那个终端回显和行缓冲全关，看上去像第二次卡死，得知道敲 reset
才救得回来。

屏蔽 SIGTERM/SIGINT/SIGHUP 并起一条 sigwait 线程，在普通线程上下文里
还原终端再 _exit——不用信号 handler 是因为 crossterm 的 disable_raw_mode
内部要锁 Mutex，在 handler 里锁可能死锁。"
```

---

### Task 2: Ctrl+Q 全局「退一层」

**Files:**
- Modify: `src/ui.rs:216-230`（按键循环，`match view.clone()` 之前）、`src/ui.rs:503-516`（`key_to_input`）、`src/ui.rs` 测试模块
- Modify: `README.md:36-48`（按键表）

**Interfaces:**
- Consumes: 无（不依赖 Task 1）
- Produces: `fn is_ctrl_q(key: &KeyEvent) -> bool`（模块私有）

**背景（实现者必读）：** 用户报告退不出去，根因不是 F2 失效——**他不知道有 F2 这个键**。`Q = quit` 猜得到，所以加 Ctrl+Q，语义是「退一层，一直按就退到头」。F2 保留不动。Ctrl+C 绝不能碰，Claude Code 靠它中断。

各视图的目标：

| 视图 | Ctrl+Q 之后 |
|---|---|
| `Board` | 退出 dct（等同 `q`） |
| `Attached` | → `Board` |
| `PickProfile` | → `Board` |
| `PickProject` 列表态 | → `Board` |
| `PickProject` 手输路径态 | → `PickProject` 列表态 |

- [ ] **Step 1: 写会失败的单元测试**

加到 `src/ui.rs` 的 `#[cfg(test)] mod` 里：

```rust
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_q_is_never_forwarded_to_the_agent() {
        // 调用点已经拦了 Ctrl+Q，这层是兜底：万一哪天调用点漏改，
        // 也不能把 0x11 悄悄发进 agent——那会变成一个「按了逃生键，
        // 结果字符落进了 Claude Code 输入框」的怪现象。
        assert_eq!(key_to_input(&ctrl('q')), None);
        assert_eq!(key_to_input(&ctrl('Q')), None);
    }

    #[test]
    fn other_ctrl_combos_still_reach_the_agent() {
        // 别误伤：Ctrl+C 是 Claude Code 的中断键，Ctrl+B 是它的「转后台」，
        // 两个都必须继续透传。
        assert_eq!(key_to_input(&ctrl('c')), Some("\u{3}".to_string()));
        assert_eq!(key_to_input(&ctrl('b')), Some("\u{2}".to_string()));
    }

    #[test]
    fn ctrl_q_is_recognised_in_both_cases() {
        // 有的终端在 Ctrl 组合里送大写字母
        assert!(is_ctrl_q(&ctrl('q')));
        assert!(is_ctrl_q(&ctrl('Q')));
        // 不带 Ctrl 的裸 q 不算——否则在项目选择器里打字过滤会退出界面
        assert!(!is_ctrl_q(&KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE
        )));
    }
```

- [ ] **Step 2: 跑测试，确认失败**

```
cargo test --lib -- --test-threads=1 ctrl_q
```

预期：编译失败，`cannot find function is_ctrl_q in this scope`。

- [ ] **Step 3: 加 `is_ctrl_q` 并让 `key_to_input` 拒绝它**

在 `src/ui.rs` 里 `key_to_input` 附近加：

```rust
/// Ctrl+Q —— dct 的全局逃生键。
///
/// crossterm 把它报成 `Char('q')` 带 `CONTROL` 修饰，有的终端送大写。
/// 判断必须放在任何 `Char(c)` 分支**之前**：项目选择器的打字过滤是靠
/// `Char(c)` 累加的，判晚了会往过滤框里塞一个 `q`。
fn is_ctrl_q(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
}
```

在 `key_to_input` 的 `KeyCode::Char(c) if ctrl =>` 分支开头插一句（`src/ui.rs:507` 附近）：

```rust
        KeyCode::Char(c) if ctrl => {
            // Ctrl+Q 是 dct 自己的逃生键，绝不透传——见 is_ctrl_q 的注释
            if c.eq_ignore_ascii_case(&'q') {
                return None;
            }
            // Ctrl+A..Ctrl+Z -> 0x01..0x1a，其余 Ctrl 组合不转发
            let lower = c.to_ascii_lowercase();
            if lower.is_ascii_lowercase() {
                char::from(lower as u8 - b'a' + 1).to_string()
            } else {
                return None;
            }
        }
```

- [ ] **Step 4: 加 `back_one_level` 纯函数**

`run()` 要连真 socket，按键循环本身没法单测。把「退到哪一层」这个决定抽成纯函数，既好测也让循环体保持薄：

```rust
/// Ctrl+Q 从当前视图退到哪一层。`None` 表示「退到头了，该退出 dct」。
///
/// 抽成纯函数是为了能单测——`run()` 的按键循环要连真 socket，测不了。
fn back_one_level(view: View) -> Option<View> {
    match view {
        View::Board => None,
        View::PickProject {
            all,
            filter,
            state,
            typing_path: Some(_),
        } => Some(View::PickProject {
            all,
            filter,
            state,
            typing_path: None,
        }),
        _ => Some(View::Board),
    }
}
```

- [ ] **Step 5: 在按键循环里加全局拦截**

改 `src/ui.rs:230` 的 `match view.clone() {` —— 把它整个包进 `else`，前面加 Ctrl+Q 分支。三个快照变量（`view_kind_before` 等）必须留在**拦截之前**，这样循环末尾的 `message_after_transition` 对两条路径一视同仁：

```rust
        // 处理这次按键前拍个快照，处理完之后用来判断 message 该不该清——
        // 见 message_after_transition 的注释。
        let view_kind_before = std::mem::discriminant(&view);
        let message_text_before = message.text.clone();
        let message_error_before = message.error;

        // Ctrl+Q 在所有视图里都是「退一层，一直按就退到头」。
        //
        // 加这个键是因为真实事故：用户不知道有 F2，在会话里怎么按都出不去，
        // 只能去别的窗口 kill 进程。`Q = quit` 是非程序员唯一猜得到的组合，
        // 而 Claude Code 不占用它——代价只是从 agent 手里拿走 0x11。
        //
        // 拦截**必须**留在 `match view.clone()` 之前，别挪进去：`PickProject`
        // 的打字过滤和手输路径都靠 `Char(c)` 累加，而 Ctrl+Q 在 crossterm 里
        // 就是 `Char('q')` 带 CONTROL——挪进去就会往过滤框里塞一个 q。
        if is_ctrl_q(&key) {
            match back_one_level(view.clone()) {
                None => break Ok(()),
                Some(next) => {
                    // 回看板要重新拉一次会话列表，否则看板显示的是进会话之前的旧快照
                    need_sessions = matches!(next, View::Board);
                    view = next;
                }
            }
        } else {
            // 必须 clone：分支里要给 view 赋值，match &view 会被借用检查器拒掉
            match view.clone() {
                View::Board => match key.code {
                    // …既有的五个 View 分支整块原样搬进来，内部一个字不改，
                    //   只是多了一层缩进（cargo fmt 会处理）…
                }
            }
        }
```

**注意：** 既有的 `match view.clone() { … }` 整块搬进 `else`，**内部一个字都不要改**。这一步唯一的实质改动是外面多包了一层 `if is_ctrl_q(&key) { … } else { … }`。

- [ ] **Step 6: 加视图跳转的单元测试**

```rust
    #[test]
    fn ctrl_q_backs_out_one_level_at_a_time() {
        // 会话 / 两个选择器 -> 看板
        assert!(matches!(
            back_one_level(View::Attached(1)),
            Some(View::Board)
        ));
        assert!(matches!(
            back_one_level(View::PickProfile(vec!["claude".into()])),
            Some(View::Board)
        ));
        assert!(matches!(
            back_one_level(View::PickProject {
                all: Vec::new(),
                filter: String::new(),
                state: ListState::default(),
                typing_path: None,
            }),
            Some(View::Board)
        ));
    }

    #[test]
    fn ctrl_q_leaves_the_typing_state_before_leaving_the_picker() {
        // 手输路径态退一层是回列表，不是一步退回看板
        let back = back_one_level(View::PickProject {
            all: vec!["/tmp/a".into()],
            filter: "a".into(),
            state: ListState::default(),
            typing_path: Some("/tmp/b".into()),
        });
        match back {
            Some(View::PickProject {
                typing_path,
                filter,
                all,
                ..
            }) => {
                assert_eq!(typing_path, None, "应当退出手输态");
                assert_eq!(filter, "a", "退一层不该顺手清掉过滤词");
                assert_eq!(all, vec!["/tmp/a".to_string()], "项目列表不该丢");
            }
            other => panic!("手输态应当退回列表态，实际是 {:?}", other.is_some()),
        }
    }

    #[test]
    fn ctrl_q_on_the_board_quits() {
        // 退到头了。看板上退出不杀会话，守护进程继续跑。
        assert!(back_one_level(View::Board).is_none());
    }
```

- [ ] **Step 7: 跑测试，确认全过**

```
cargo test --lib -- --test-threads=1
```

预期：本任务新增的 6 个测试全 PASS，既有测试无回归。

**关于「Ctrl+Q 不能被当成过滤字符」这条：** 规格里点名要一条回归测试，但它测不了——那条路径在 `run()` 的按键循环里，而循环要连真 socket。这里靠的是**结构保证**：拦截在 `match view.clone()` 之前，`Char(c)` 分支根本收不到 Ctrl+Q。Step 5 的注释写明了「别挪进去」以及挪进去的后果，`ctrl_q_is_recognised_in_both_cases` 保证裸 `q` 不会被误判成逃生键。如果将来有人重构掉这个结构，靠的是那条注释，不是测试——这一点要如实知道，别以为有网。

- [ ] **Step 8: 更新 README 按键表**

`README.md` 第 36-48 行那张表，在 `q` 那一行下面补一行，并改会话屏幕那段说明：

```markdown
| `q` | 退出看板（守护进程继续跑） |
| `Ctrl+Q` | 退一层：会话里回看板，看板上退出 |
```

以及第 50 行那句改成：

```markdown
进入会话屏幕后打字直接送给 agent，`Esc` 也会送给 agent（它靠这个键取消/清空/关弹窗）；回看板用 `F2` 或 `Ctrl+Q`。
```

- [ ] **Step 9: 提交**

```bash
git add src/ui.rs README.md
git commit -m "feat: Ctrl+Q 全局退一层

用户报告在会话里退不出去。根因不是 F2 失效，是他不知道有 F2 这个键。
Q = quit 是非程序员唯一猜得到的组合，Claude Code 不占用它。

拦截必须排在 match view 之前：项目选择器的打字过滤靠 Char(c) 累加，
拦晚了 Ctrl+Q 会往过滤框里塞一个 q。F2 保留，Ctrl+C 继续透传给 agent。"
```

---

### Task 3: 底栏拆两段，逃生提示永不让位

**Files:**
- Modify: `src/ui.rs:893-925`（`idle_help` 与底栏渲染）、`src/ui.rs` 测试模块

**Interfaces:**
- Consumes: `View`（已有）、`is_ctrl_q` 无关；本任务与 Task 1/2 无代码依赖，但文案要跟 Task 2 的键位一致
- Produces: `fn escape_hint(view: &View) -> &'static str`、`const ESCAPE_HINT_COLS: u16`

**背景（实现者必读）：** 底栏现在的优先级是「断连提示 > 消息 > `idle_help` 按键表」（`src/ui.rs:904-914`）。消息一非空，整张按键表就消失，包括其中唯一写着怎么退出的那一截。而消息只在切视图时才清，且本次切换的结果会被刻意保留。于是：**在看板上按一次 `p` 换项目，落回看板时底栏显示「已切到 …」，`q 退出` 四个字再也不出现**——除非用户再切一次视图，而他正是因为不知道怎么切才卡住。用户实拍的截图就是这一屏。

会话视图更糟：顶掉提示的往往是「守护进程连不上，刚才那次输入没发出去」这类错误，也就是**出事的那一刻，唯一的逃生提示正好消失**。

- [ ] **Step 1: 写会失败的测试**

加到 `src/ui.rs` 的 `#[cfg(test)] mod` 里：

```rust
    /// 底栏左段的文字。宽字符在 TestBackend 里只占首个 cell，
    /// 所以统一滤掉空白再找子串，跟既有的 bottom_bar_help_follows_the_view 一致。
    fn bar_text(term: &Terminal<ratatui::backend::TestBackend>) -> String {
        buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    #[test]
    fn escape_hint_survives_a_long_message() {
        use ratatui::backend::TestBackend;

        // 真实事故：在看板上按 p 换项目，「已切到 …」这条消息把整张按键表
        // 顶掉，其中就包括「q 退出」。用户从此没有任何地方能看到怎么退出。
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut st = ListState::default();
        let long = Msg::from("已切到 ~/work/dc/dc-terminal，这条消息故意写得很长很长很长很长很长".to_string());
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Board,
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &long,
                    connected: true,
                    current: "/tmp",
                },
            )
        })
        .unwrap();
        let c = bar_text(&term);
        assert!(
            c.contains("q退出"),
            "消息再长也不能把退出提示挤掉——这正是用户卡住的那一屏：{c}"
        );
    }

    #[test]
    fn escape_hint_survives_a_disconnect() {
        use ratatui::backend::TestBackend;

        // 出事的那一刻恰恰是最需要逃生提示的时候，断连提示不能把它顶掉。
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut st = ListState::default();
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Attached(1),
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: false,
                    current: "/tmp",
                },
            )
        })
        .unwrap();
        let c = bar_text(&term);
        assert!(
            c.contains("Ctrl+Q回看板"),
            "断连时逃生提示必须还在：{c}"
        );
        assert!(c.contains("连不上"), "断连提示本身也要显示：{c}");
    }

    #[test]
    fn escape_hint_matches_what_the_key_actually_does() {
        // 底栏说什么就必须真能做到什么。手输路径态的 Ctrl+Q 是回列表
        // 不是回看板（见 back_one_level），文案不能写成「回看板」。
        assert_eq!(escape_hint(&View::Board), "q 退出");
        assert_eq!(escape_hint(&View::Attached(1)), "Ctrl+Q 回看板");
        assert_eq!(
            escape_hint(&View::PickProject {
                all: Vec::new(),
                filter: String::new(),
                state: ListState::default(),
                typing_path: None,
            }),
            "Ctrl+Q 回看板"
        );
        assert_eq!(
            escape_hint(&View::PickProject {
                all: Vec::new(),
                filter: String::new(),
                state: ListState::default(),
                typing_path: Some(String::new()),
            }),
            "Ctrl+Q 回列表"
        );
    }
```

- [ ] **Step 2: 跑测试，确认失败**

```
cargo test --lib -- --test-threads=1 escape_hint
```

预期：编译失败，`cannot find function escape_hint in this scope`。

- [ ] **Step 3: 加 `escape_hint` 与宽度常量**

在 `src/ui.rs` 的 `draw` 附近加：

```rust
/// 底栏左段：逃生键提示。
///
/// 这是唯一一条「不管出什么事都必须还在」的信息——用户找不到它就只能去
/// 别的窗口 kill 进程，而 kill 会把终端留在 raw mode。文案必须跟
/// `back_one_level` 逐行对上：底栏说什么就得真能做到什么，
/// 手输路径态退的是一层（回列表），不能写成「回看板」。
fn escape_hint(view: &View) -> &'static str {
    match view {
        View::Board => "q 退出",
        View::PickProject {
            typing_path: Some(_),
            ..
        } => "Ctrl+Q 回列表",
        _ => "Ctrl+Q 回看板",
    }
}

/// 左段固定占的列数：「Ctrl+Q 回看板」= 6 + 1 + 中文 3 字 × 2 = 13。
/// 三条文案里最长的就是它（「Ctrl+Q 回列表」同宽，「q 退出」更短）。
/// 写死而不是每帧算：左段宽度跟着文案跳动会让右段的消息忽宽忽窄。
const ESCAPE_HINT_COLS: u16 = 13;
```

- [ ] **Step 4: 底栏改成横向两段**

替换 `src/ui.rs:893-925` 那一整块。`idle_help` 里跟左段重复的那一截去掉：

```rust
    // 提示必须跟着视图走。底部栏原来不分视图，进了会话仍写着看板的按键表，
    // 而那些键在会话视图里全部被转发给 agent——用户照着按 n，字母 n 会落进
    // Claude Code 的输入框。显示做不到的操作比不显示更糟。
    //
    // 逃生键那一截已经挪进左段常驻，这里不再重复。
    let idle_help = match view {
        View::Attached(_) => "F2 同效　回看板后按 n 新建会话　其余按键都发给 agent",
        View::PickProfile(_) => "按数字选 agent，Esc 取消",
        View::PickProject {
            typing_path: Some(_),
            ..
        } => "输入路径后 Enter 确认，Esc 返回列表",
        View::PickProject { .. } => "↑↓ 选  Enter 确认  直接打字过滤  Esc 取消",
        View::Board => "n 新建  p 换项目  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动",
    };

    let (help, style) = if !connected {
        (
            "守护进程连不上，界面数据可能已过期".to_string(),
            Style::default().fg(Color::Red),
        )
    } else if message.text.is_empty() {
        (idle_help.to_string(), Style::default())
    } else if message.error {
        (message.text.clone(), Style::default().fg(Color::Red))
    } else {
        (message.text.clone(), Style::default())
    };

    // 当前项目放在边框标题里，框内只留一行字。中文是双宽字符，
    // 「当前项目：~/work/dc/dc-terminal」加上按键表在 80 列终端里放不下同一行，
    // 挤在一起会被 Paragraph 直接截断——标题行本来就空着，正好用它。
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("当前项目：{}", short_path(current)));
    let inner = block.inner(chunks[1]);
    f.render_widget(block, chunks[1]);

    // 横向拆两段：左段是逃生键，永不让位；断连提示和消息只能吃掉右段。
    //
    // 拆之前的写法是一整行按优先级二选一，于是「已切到 X」这类完全正常的
    // 操作反馈会把整张按键表连同「q 退出」一起顶掉，而消息只在切视图时才清——
    // 用户不知道怎么切视图正是他卡住的原因，于是退出提示永久消失。
    // 拆成两段之后这件事在结构上不可能再发生。
    let bar = Layout::horizontal([
        Constraint::Length(ESCAPE_HINT_COLS + 2), // +2 是和右段之间的间隔
        Constraint::Min(0),
    ])
    .split(inner);
    f.render_widget(
        Paragraph::new(escape_hint(view)).style(Style::default().fg(Color::Cyan)),
        bar[0],
    );
    f.render_widget(
        Paragraph::new(truncate(&help, bar[1].width as usize)).style(style),
        bar[1],
    );
```

- [ ] **Step 5: 修既有测试的断言**

`bottom_bar_help_follows_the_view` 里那句 `assert!(c.contains("F2回看板"))` 会挂——会话视图的 `idle_help` 已经改成 `F2 同效…`，「回看板」三个字挪到了左段。改成：

```rust
        let c = text_of(&term);
        assert!(c.contains("Ctrl+Q回看板"), "会话视图要给出逆转键提示：{c}");
        assert!(c.contains("F2同效"), "F2 是老用户的肌肉记忆，也要留在提示里：{c}");
        assert!(c.contains("新建会话"), "还要说清新建会话怎么走：{c}");
        assert!(!c.contains("u回滚"), "会话视图不能显示看板按键表：{c}");
```

看板那半段里 `assert!(c.contains("q退出"))` 之类的断言如果存在，现在由左段满足，仍然成立，不用改。跑一遍看实际报什么再动手，**不要盲改**。

- [ ] **Step 6: 跑测试，确认全过**

```
cargo test --lib -- --test-threads=1
```

预期：本任务 3 个新测试 PASS，`bottom_bar_help_follows_the_view` 和 `draw_does_not_panic_for_all_views` 继续 PASS。

- [ ] **Step 7: 在真终端上眼看一遍**

```
cargo build --release && ./target/release/dct
```

按 `p` 换一个项目，落回看板。确认底栏**同时**显示左边的 `q 退出` 和右边的 `已切到 …`——这就是用户截图里坏掉的那一屏，现在应该两者并存。然后按 `q` 退出，确认终端正常。

- [ ] **Step 8: 全量检查并提交**

```
cargo test -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets
```

```bash
git add src/ui.rs
git commit -m "fix: 逃生提示不再被消息顶掉

底栏原来一整行按优先级二选一，于是在看板上按一次 p 换项目，
「已切到 X」就把整张按键表连同「q 退出」一起顶掉；而消息只在切视图时
才清，用户不知道怎么切视图正是他卡住的原因，退出提示就此永久消失。
会话视图更糟：顶掉提示的往往是「守护进程连不上」——出事那一刻逃生
提示正好消失。

横向拆两段，左段逃生键永不让位，消息和断连提示只能吃掉右段。"
```

---

## 完成后

三个任务各自独立，可按任意顺序做，但建议按 1 → 2 → 3：Task 3 的文案（`Ctrl+Q 回看板`）要跟 Task 2 的键位对上，反过来做会有一段时间底栏写着一个还不存在的键。

全部做完后跑一次完整验收：

```
cargo test -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets
```

并手工确认这条端到端路径：开 `dct` → 进一个会话 → 按 Ctrl+Q 回看板 → 按 `p` 换项目 → 确认底栏左边仍写着 `q 退出` → 按 `q` 退出 → 终端正常。再从另一个窗口 `kill` 一个开着的 dct，确认那个终端也不需要 `reset`。
