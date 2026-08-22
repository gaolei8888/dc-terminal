//! 界面和守护进程是分开升级的两个东西，早晚会撞上「新界面 + 旧守护进程」。
//!
//! 2026-08-05 的现场：守护进程从 8/4 17:47 一直活着，用户当天中午装了新的
//! dct。协议在这期间改过（`Profiles` 加了 `lang`），于是按 n 只弹一句
//! 「拿不到 agent 列表」——一个既不说明原因、也不告诉他怎么办的死胡同。
//! 而守护进程活得久正是这个产品存在的理由，所以这个局面不是意外，是常态。
//!
//! 这个文件测的是撞上之后能不能自己走出来：认出对面是旧的，然后换掉它。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use dct::client::Client;
use dct::proto::{daemon_status, DaemonStatus, Request, Response};

/// 把测试用的二进制复制成一个独一无二的名字，收尾时按名字杀进程
/// 不会误伤开发机上真正在跑的 dct 守护进程。
fn unique_binary(dir: &Path, tag: &str) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_BIN_EXE_dct"));
    let dst = dir.join(format!("dct-upgrade-{tag}-probe-{}", std::process::id()));
    std::fs::copy(&src, &dst).unwrap();
    dst
}

fn wait_for(mut cond: impl FnMut() -> bool, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        sleep(Duration::from_millis(50));
    }
    false
}

/// 直接把守护进程当子进程起来——不经过 TUI。这两条测试要的是「socket 那头
/// 是一个**别的**进程」，而 `common::start_daemon()` 是在测试进程自己的线程
/// 里跑的：拿它的 peer pid 会拿到测试进程自己，一 kill 就把测试跑崩。
fn spawn_daemon(bin: &Path, home: &Path, sock: &Path) -> std::process::Child {
    let child = Command::new(bin)
        .arg("daemon")
        // socket 路径是从 HOME 推出来的（`proto::socket_path`），
        // 给个临时 HOME 就不会碰到开发机上真正的那个守护进程。
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("拉不起守护进程");
    assert!(
        wait_for(|| Client::connect(sock).is_ok(), 10),
        "守护进程没起来：{}",
        sock.display()
    );
    child
}

/// 握手：新守护进程报得出自己是几号协议。
#[test]
fn a_fresh_daemon_reports_the_protocol_it_speaks() {
    let home = tempfile::tempdir().unwrap();
    let bin = unique_binary(home.path(), "hello");
    let sock = home.path().join(".dct").join("daemon.sock");
    let mut child = spawn_daemon(&bin, home.path(), &sock);

    let mut c = Client::connect(&sock).unwrap();
    assert_eq!(
        daemon_status(c.protocol()),
        DaemonStatus::Same,
        "刚起来的守护进程必须跟界面是同一号协议"
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// socket 那头是谁，问内核要。
///
/// 不靠进程名匹配：旧守护进程可能是从别的路径起的（现场那个跑的是
/// `target/release/dct`，而界面装在 `~/.local/bin/dct`）。也不能靠守护进程
/// 自己配合——**需要换掉的恰恰是那些老到不认识任何新请求的**。
#[test]
fn peer_pid_names_the_process_behind_the_socket() {
    let home = tempfile::tempdir().unwrap();
    let bin = unique_binary(home.path(), "peer");
    let sock = home.path().join(".dct").join("daemon.sock");
    let mut child = spawn_daemon(&bin, home.path(), &sock);

    let c = Client::connect(&sock).unwrap();
    assert_eq!(
        c.peer_pid(),
        Some(child.id()),
        "socket 那头应当正是我们刚拉起来的那个守护进程"
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// 换掉旧的：老的必须真的死掉，新的必须真的能服务。
///
/// 「能服务」不是「socket 文件还在」——旧进程死掉时留下的 socket 文件照样
/// 摆在那，连上去却没人应答。所以这里要真发一条请求。
#[test]
fn restarting_replaces_the_daemon_with_one_that_serves() {
    let home = tempfile::tempdir().unwrap();
    let bin = unique_binary(home.path(), "restart");
    let sock = home.path().join(".dct").join("daemon.sock");
    let mut old = spawn_daemon(&bin, home.path(), &sock);
    let old_pid = old.id();

    dct::client::restart_daemon(&sock, &bin).expect("重启守护进程失败");

    // 老的必须真的走了。收尸一下，免得留个僵尸。
    let _ = old.wait();
    assert!(
        !process_alive(old_pid),
        "旧守护进程还活着，socket 那头还是它"
    );

    let mut c = Client::connect(&sock).expect("重启之后连不上");
    assert!(
        matches!(c.call(Request::List).unwrap(), Response::Sessions(_)),
        "重启之后守护进程必须能正常服务，不能只是留了个 socket 文件"
    );
    assert_eq!(
        daemon_status(c.protocol()),
        DaemonStatus::Same,
        "换上来的必须是跟界面同一号协议的那个"
    );

    let new_pid = c.peer_pid().expect("拿不到新守护进程的 pid");
    assert_ne!(new_pid, old_pid, "根本没换人");

    dct::sys::proc::hard_kill(new_pid);
}

fn process_alive(pid: u32) -> bool {
    // Unix 上这是 0 号信号的存在性探测，Windows 上是问进程句柄的退出码。
    // 两边同一个意思：这个 pid 现在还在不在。
    dct::sys::proc::alive(pid)
}
