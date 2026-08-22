//! 守护进程必须脱离终端存活——"关掉终端窗口不影响会话"是这个产品的立身之本。
//!
//! 之前 `dct` 用普通 `spawn` 拉起守护进程，两者在同一个终端 session 里，
//! 关掉窗口时 SIGHUP 会把守护进程一起带走。

//! **这一整个文件只在 Unix 上有意义**，但它守的东西两个平台都要。装置是
//! Unix 的：`pgrep` 按命令行找进程、`ps -o pgid=` 看它有没有自成一组——
//! 「自成一组」正是 `setsid` 的痕迹。Windows 上这两样都没有对应物（那边
//! 靠的是 `DETACHED_PROCESS`：不继承调用者的控制台，见 `sys::proc`），
//! 要验它得换一套完全不同的装置：起一个中间进程、让它拉起守护进程、
//! 杀掉中间进程、再看守护进程还在不在。
#![cfg(unix)]

use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use dct::client::Client;
use dct::proto::{Request, Response};

/// 把测试用的二进制复制成一个独一无二的名字，这样收尾时按名字杀进程
/// 不会误伤开发机上真正在跑的 dct 守护进程。
fn unique_binary(dir: &std::path::Path) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_BIN_EXE_dct"));
    let dst = dir.join(format!("dct-detach-probe-{}", std::process::id()));
    std::fs::copy(&src, &dst).unwrap();
    dst
}

/// 这份测试二进制起的守护进程的 pid（名字独一无二，不会误抓开发机上真正的 dct）
fn pgrep_daemon(bin: &std::path::Path) -> Option<u32> {
    let out = std::process::Command::new("pgrep")
        .args(["-f", &format!("{} daemon", bin.display())])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

/// 进程组 id。macOS 的 `ps -o sess=` 只会打 0，读不出会话号，
/// 所以用进程组来判断：setsid 之后守护进程会自成一组，pgid 等于它自己的 pid。
fn process_group(pid: u32) -> Option<u32> {
    let out = std::process::Command::new("ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
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

#[test]
fn daemon_survives_terminal_close() {
    let home = tempfile::tempdir().unwrap();
    let workdir = tempfile::tempdir().unwrap();
    let bin = unique_binary(home.path());
    let sock = home.path().join(".dct").join("daemon.sock");

    // 在一个真 pty 里跑 TUI，模拟用户开着终端窗口
    let pty = NativePtySystem::default()
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(&bin);
    cmd.cwd(workdir.path());
    cmd.env("HOME", home.path());
    cmd.env("TERM", "xterm-256color");
    let mut child = pty.slave.spawn_command(cmd).unwrap();
    drop(pty.slave);

    assert!(
        wait_for(|| sock.exists(), 15),
        "守护进程没能把 socket 建出来"
    );

    // 确认它此刻是活的
    {
        let mut c = Client::connect(&sock).unwrap();
        assert!(matches!(
            c.call(Request::List).unwrap(),
            Response::Sessions(_)
        ));
    }

    // 核心断言：守护进程必须自成一个终端会话（session），不能和 TUI 同属一个。
    // 这才是 setsid 真正做的事，也是它能扛住 SIGHUP 的原因。只断言"关掉 pty
    // 之后还活着"是不够的——那个断言在没有 setsid 时也会通过，区分不出来。
    let daemon_pid = pgrep_daemon(&bin).expect("找不到守护进程");
    let tui_pid = child.process_id().expect("拿不到 TUI 的 pid");
    let daemon_pgid = process_group(daemon_pid).expect("读不到守护进程的进程组");
    let tui_pgid = process_group(tui_pid).expect("读不到 TUI 的进程组");
    assert_ne!(
        daemon_pgid, tui_pgid,
        "守护进程不能和 TUI 同属一个进程组，否则终端的 SIGHUP 会把它一起带走"
    );
    assert_eq!(
        daemon_pgid, daemon_pid,
        "setsid 之后守护进程应当自成一个进程组（pgid == 自己的 pid）"
    );

    // 关掉终端窗口 = 释放 pty master，内核给这个终端会话发 SIGHUP。
    // 不能自己先 kill TUI，那样就绕过了真正的路径。
    drop(pty.master);
    assert!(
        wait_for(
            || child.try_wait().map(|s| s.is_some()).unwrap_or(false),
            10
        ),
        "释放 pty 之后 TUI 应当退出，否则这个测试没有真的模拟关窗口"
    );
    sleep(Duration::from_secs(1));

    let mut c = Client::connect(&sock).expect("关掉终端后守护进程必须还能连上");
    assert!(
        matches!(c.call(Request::List).unwrap(), Response::Sessions(_)),
        "关掉终端后守护进程必须还能正常服务"
    );
    drop(c);

    // 收尾：只杀这个测试自己那份二进制起的守护进程
    std::process::Command::new("pkill")
        .args(["-f", &format!("{} daemon", bin.display())])
        .output()
        .ok();
}
