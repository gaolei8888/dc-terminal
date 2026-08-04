//! 集成测试共用的「起一个真守护进程」脚手架。
//!
//! `tests/projects_flow.rs` 和 `tests/profiles_flow.rs` 都要走真实的 unix
//! socket——协议层的往返、锁的粒度这些东西只有连真 daemon 才测得出来，
//! 直接调用内部函数会绕过 `serve()`/`handle()` 的编解码和线程边界。这份代码
//! 原来在 `projects_flow.rs` 里单独长了一份，`profiles_flow.rs` 需要一模一样
//! 的起法，所以抽到这里供两边共用。
//!
//! 没有抽给 `concurrency.rs`：那边要在起daemon 之前先往 `SessionManager`
//! 里 `register_profile` 一个测试专用的慢 profile，`start_daemon()` 这种
//! 「零参数、内部自己 new 一个 manager」的形状塞不下那个需求，硬塞会让这个
//! 共用脚手架长出一个只有一个调用方用得到的分支。

// 每个用到 `mod common` 的集成测试文件都会把这份代码单独编译成一个 crate，
// 而不是每个文件都用得上全部方法（比如 `daemon_roundtrip.rs` 不需要
// `git_repo()`）。逐个文件加 `#[allow(dead_code)]` 太啰嗦，这里整体放开。
#![allow(dead_code)]

use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use dct::client::Client;

/// 一个已经起来的守护进程。`home` 是它的 `~/.dct` 替身，跟着这个 handle 的
/// 生命周期走——测试结束、handle 被 drop，临时目录才被清掉，所以只要 handle
/// 还活着，`sock` 和 `git_repo()` 建的目录就都还在。
pub struct DaemonHandle {
    home: tempfile::TempDir,
    pub sock: PathBuf,
}

/// 起一个全新的守护进程，用临时目录当它的 `~/.dct`——projects.json /
/// secrets.toml / profiles/ 全部落在这个临时目录里，不会碰到真实用户的数据。
pub fn start_daemon() -> DaemonHandle {
    let home = tempfile::tempdir().unwrap();
    let sock = home.path().join("daemon.sock");
    let s = sock.clone();
    std::thread::spawn(move || {
        let _ = dct::daemon::run(&s);
    });
    for _ in 0..50 {
        if sock.exists() {
            return DaemonHandle { home, sock };
        }
        sleep(Duration::from_millis(50));
    }
    panic!("守护进程没起来：{}", sock.display());
}

impl DaemonHandle {
    /// 每次都开一条新连接，跟真实 TUI 的用法一致：一条 `Client` 对应一条
    /// TCP/Unix 连接，不同测试里的 `c` 互不相扰。
    pub fn client(&self) -> Client {
        Client::connect(&self.sock).unwrap()
    }

    /// 建一个已初始化的 git 仓库，目录建在这个 handle 的临时 home 下面，
    /// 生命周期跟着 handle 走。`name` 只是给目录起个可读的名字，不影响内容——
    /// agent 会话要求是 git 仓库，shell 会话不要求，测试用哪个看具体场景。
    pub fn git_repo(&self, name: &str) -> PathBuf {
        let dir = self.home.path().join("repos").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }
}
