//! kill 掉 TUI 之后，终端必须还是能用的。
//!
//! `TerminalGuard` 的 `Drop` 盖不住信号：SIGTERM/SIGHUP 直接终止进程，不展开栈。
//! 少了这条保障，用户「退不出去只好去别的窗口 kill」之后会拿到一个停在
//! raw mode 的终端——回显和行缓冲全关，看上去像第二次卡死，得知道敲 `reset`
//! 才救得回来。而「知道敲 reset」正是不该要求用户具备的知识。

//! **这一整个文件只在 Unix 上有意义**：它要 `openpty` 造一个真终端、读
//! `termios` 看 raw mode 有没有还回去、发 SIGHUP 模拟「关掉窗口」。三样
//! Windows 都没有——那边同一件事由控制台处理函数负责（`sys::signal`），
//! 摆现场要另一套完全不同的装置。
#![cfg(unix)]

use std::os::unix::io::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// crossterm 0.28.1 的 `DisableMouseCapture::write_ansi`（src/event.rs）真正
/// 写出来的字节，逐条核实过，不是凭印象抄的字面量：`csi!("?1006l")` 展开成
/// `"\x1B[?1006l"`（`csi!` 定义在 crossterm 的 `macros.rs`，把参数拼在
/// `"\x1B["` 后面），五条 disable 序列按 `EnableMouseCapture` 的**逆序**
/// 拼在一起，一次 `write_ansi` 调用全部写出。这是 SIGTERM/SIGHUP 之后
/// `restore_terminal()` 必须真的送到用户终端上的那串字节——漏了它，用户
/// 退出 dct 之后终端会在每次点击/拖选时冒出这串乱码，直到他知道敲什么
/// 命令才能清掉，而多数用户不知道。
const DISABLE_MOUSE_CAPTURE_SEQ: &[u8] = b"\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// 把测试用的二进制复制成一个独一无二的名字，收尾时按名字杀进程
/// 不会误伤开发机上真正在跑的 dct 守护进程。
fn unique_binary(dir: &Path, tag: &str) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_BIN_EXE_dct"));
    let dst = dir.join(format!("dct-{tag}-probe-{}", std::process::id()));
    std::fs::copy(&src, &dst).unwrap();
    dst
}

/// 这份二进制起的守护进程还在不在。按完整路径匹配，只认这个测试自己那一份，
/// 不会看见开发机上真正在跑的 dct。
fn daemon_alive(bin: &Path) -> bool {
    Command::new("pgrep")
        .args(["-f", &format!("{} daemon", bin.display())])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

/// 只杀这个测试自己那份二进制起的守护进程。同 `daemon_detach.rs` 的收尾。
fn reap_daemon(bin: &Path) {
    Command::new("pkill")
        .args(["-f", &format!("{} daemon", bin.display())])
        .output()
        .ok();
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
///
/// 读的是 **master** 那一端，不是 slave。macOS（BSD）在会话首进程退出时会对
/// 控制终端做一次 `revoke()`，之后 slave 的 fd 全部失效，`tcgetattr` 直接返回
/// ENOTTY——而「进程死了之后终端什么状态」恰好是这个测试唯一想问的事，
/// 从 slave 上根本问不出来。master 不受 revoke 影响，且 pty 会把 TIOCGETA
/// 转给 slave 的 termios，读到的就是用户那一端的真实状态。
unsafe fn is_raw(master: libc::c_int) -> bool {
    let mut t: libc::termios = std::mem::zeroed();
    assert_eq!(libc::tcgetattr(master, &mut t), 0, "读不到 termios");
    (t.c_lflag & libc::ECHO) == 0 || (t.c_lflag & libc::ICANON) == 0
}

/// 在一个我们自己持有 fd 的 pty 里把 dct 跑起来，返回 (子进程, master fd,
/// dct 写给终端的全部字节)。
///
/// 必须 `setsid` + `TIOCSCTTY`：crossterm 的 `enable_raw_mode` 优先操作
/// `/dev/tty`，也就是**控制终端**。不给子进程把这个 pty 设成控制终端的话，
/// 它动的是 cargo test 自己的终端，这个测试就测了个寂寞。
fn spawn_dct_in_pty(
    bin: &Path,
    home: &Path,
    cwd: &Path,
) -> (std::process::Child, libc::c_int, Arc<Mutex<Vec<u8>>>) {
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

    // 还没拉起任何进程，此刻必然不是 raw mode。这条断言是 `is_raw` 从 master
    // 读 termios 这个做法的守门人：万一哪天 master 不再转发 TIOCGETA、恒返回
    // 一份全零的 termios，下面的 `wait_until_raw` 会立刻为真，两个测试都会
    // 变成永远通过的假绿。这里先钉死「空 pty 读出来是非 raw」。
    assert!(
        !unsafe { is_raw(master) },
        "刚开出来的 pty 就被判成 raw mode，说明 master 读到的不是真 termios"
    );

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
    // 父进程这份 slave 没用了，早点还掉：留着它，子进程死后 master 读不到 EOF，
    // 下面那条 drain 线程就永远挂着。
    unsafe { libc::close(slave) };

    // 必须有人一直读 master，否则 TUI 画第一帧就把 pty 缓冲写满、卡在 write 里
    // 出不来——实测那样连 SIGTERM 都收拾不干净，进程会停在 ps 的 `?E`
    //「正在退出」状态上十几秒不动。真终端本来就一直在读，这条只是把它补上。
    // 读 dup 出来的副本：收尾时主线程 close(master) 不会跟这个线程抢同一个 fd。
    //
    // 原来这里读完就扔——draining 只是为了不卡住子进程。现在把读到的字节
    // 攒进一份共享缓冲区带出去：`restore_terminal()` 在 SIGTERM/SIGHUP 之后
    // 写的 `DisableMouseCapture` 序列也在这条流里，不攒下来就只能「看代码
    // 相信它会发生」，测不出「真的发生了」。
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_for_thread = Arc::clone(&captured);
    let drain = unsafe { libc::dup(master) };
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe { libc::read(drain, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n <= 0 {
                break;
            }
            captured_for_thread
                .lock()
                .unwrap()
                .extend_from_slice(&buf[..n as usize]);
        }
        unsafe { libc::close(drain) };
    });

    (child, master, captured)
}

/// 等 TUI 真的进了 raw mode。不等就发信号的话，测试可能在它还没设置终端时
/// 就把它杀了——那样即使有 bug 也会「通过」。
fn wait_until_raw(master: libc::c_int) -> bool {
    wait_for(|| unsafe { is_raw(master) }, 20)
}

/// 等排空线程把 `needle` 攒进 `captured` 里。子进程退出（`try_wait` 返回
/// `Some`）不代表排空线程已经把它写的最后几个字节读完并追加到缓冲区——
/// 两者是不同线程，中间有个真实但很短的窗口，所以要轮询，不能读一次就断言。
fn wait_until_captured_contains(captured: &Arc<Mutex<Vec<u8>>>, needle: &[u8], secs: u64) -> bool {
    wait_for(
        || contains_subsequence(&captured.lock().unwrap(), needle),
        secs,
    )
}

#[test]
fn sigterm_restores_the_terminal() {
    let home = tempfile::tempdir().unwrap();
    let workdir = tempfile::tempdir().unwrap();
    let bin = unique_binary(home.path(), "sigterm");

    let (mut child, master, captured) = spawn_dct_in_pty(&bin, home.path(), workdir.path());
    assert!(
        wait_until_raw(master),
        "TUI 始终没进 raw mode，测试前提不成立"
    );

    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    assert!(
        wait_for(
            || child.try_wait().map(|s| s.is_some()).unwrap_or(false),
            10
        ),
        "SIGTERM 之后 TUI 应当退出"
    );

    assert!(
        !unsafe { is_raw(master) },
        "SIGTERM 之后终端仍停在 raw mode——用户会拿到一个不回显、不换行的死终端"
    );
    // raw mode 恢复只说明 `disable_raw_mode()` 跑了；`DisableMouseCapture`
    // 是另一条独立的 `execute!`，漏发不会影响上面那条断言——这里直接盯着
    // 它真的写进了终端那条流，而不是从"raw mode 好了"倒推"鼠标捕获也关了"。
    assert!(
        wait_until_captured_contains(&captured, DISABLE_MOUSE_CAPTURE_SEQ, 5),
        "SIGTERM 之后没看到 DisableMouseCapture 序列——用户以后点哪儿终端都会冒乱码"
    );

    unsafe { libc::close(master) };

    // TUI 一起来就会 `setsid` 拉起一个守护进程，而 setsid 的全部意义就是
    // 「杀 TUI 杀不到我」。这两个测试只 kill TUI，守护进程就活到开发机重启
    // 为止——实测一台机器上攒了 124 个，全是历次 `cargo test` 留下的，每个
    // 都还锁着一份早就该被删掉的临时二进制。
    reap_daemon(&bin);
    assert!(
        wait_for(|| !daemon_alive(&bin), 10),
        "测试自己拉起的守护进程没被收掉：{}",
        bin.display()
    );
}

#[test]
fn sighup_restores_the_terminal() {
    let home = tempfile::tempdir().unwrap();
    let workdir = tempfile::tempdir().unwrap();
    let bin = unique_binary(home.path(), "sighup");

    let (mut child, master, captured) = spawn_dct_in_pty(&bin, home.path(), workdir.path());
    assert!(
        wait_until_raw(master),
        "TUI 始终没进 raw mode，测试前提不成立"
    );

    // 关终端窗口、tmux 杀 pane 走的都是这条
    unsafe { libc::kill(child.id() as i32, libc::SIGHUP) };
    assert!(
        wait_for(
            || child.try_wait().map(|s| s.is_some()).unwrap_or(false),
            10
        ),
        "SIGHUP 之后 TUI 应当退出"
    );

    assert!(!unsafe { is_raw(master) }, "SIGHUP 之后终端仍停在 raw mode");
    // 同 SIGTERM 那条：raw mode 恢复不等于鼠标捕获也关了，两条是
    // `restore_terminal()` 里两次独立的 `execute!`，分别断言。
    assert!(
        wait_until_captured_contains(&captured, DISABLE_MOUSE_CAPTURE_SEQ, 5),
        "SIGHUP 之后没看到 DisableMouseCapture 序列——用户以后点哪儿终端都会冒乱码"
    );

    unsafe { libc::close(master) };

    // TUI 一起来就会 `setsid` 拉起一个守护进程，而 setsid 的全部意义就是
    // 「杀 TUI 杀不到我」。这两个测试只 kill TUI，守护进程就活到开发机重启
    // 为止——实测一台机器上攒了 124 个，全是历次 `cargo test` 留下的，每个
    // 都还锁着一份早就该被删掉的临时二进制。
    reap_daemon(&bin);
    assert!(
        wait_for(|| !daemon_alive(&bin), 10),
        "测试自己拉起的守护进程没被收掉：{}",
        bin.display()
    );
}
