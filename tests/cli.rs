use std::process::Command;

#[test]
fn daemon_subcommand_is_recognized() {
    // --help 必须提到 daemon 子命令
    let out = Command::new(env!("CARGO_BIN_EXE_dct"))
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("daemon"), "帮助里应当有 daemon: {text}");
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let out = Command::new(env!("CARGO_BIN_EXE_dct"))
        .arg("bogus")
        .output()
        .unwrap();
    assert!(!out.status.success());
}

/// `dct ps` / `dct stop` 走真二进制 + 真守护进程。
///
/// 不复用 `common::start_daemon`：那个是在测试进程里起线程，socket 落在临时
/// 目录里；而这两条命令是从 `HOME` 推 socket 路径的（`proto::socket_path`），
/// 只有把 `HOME` 换掉、再用同一个 `HOME` 起一个**真正的守护进程进程**，测到的
/// 才是用户真会走的那条路。
mod ps_and_stop {
    use std::path::Path;
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    /// 起一个守护进程，`HOME` 指向临时目录。
    ///
    /// 返回的 `Child` 必须被 kill——测试漏掉的守护进程会一直挂在开发机上
    /// （这条测试文件自己就是在修那类问题，不能再制造一个）。
    struct Daemon {
        home: tempfile::TempDir,
        child: Child,
    }

    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    impl Daemon {
        fn start() -> Daemon {
            let home = tempfile::tempdir().unwrap();
            let child = Command::new(env!("CARGO_BIN_EXE_dct"))
                .arg("daemon")
                .env("HOME", home.path())
                .spawn()
                .unwrap();
            let sock = home.path().join(".dct").join("daemon.sock");
            let deadline = Instant::now() + Duration::from_secs(10);
            while !sock.exists() {
                assert!(
                    Instant::now() < deadline,
                    "守护进程没起来：{}",
                    sock.display()
                );
                std::thread::sleep(Duration::from_millis(50));
            }
            Daemon { home, child }
        }

        fn home(&self) -> &Path {
            self.home.path()
        }

        fn run(&self, args: &[&str]) -> (String, String, i32) {
            run_with_home(self.home(), args)
        }
    }

    fn run_with_home(home: &Path, args: &[&str]) -> (String, String, i32) {
        let out = Command::new(env!("CARGO_BIN_EXE_dct"))
            .args(args)
            .env("HOME", home)
            .output()
            .unwrap();
        (
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
            out.status.code().unwrap_or(-1),
        )
    }

    /// **没有守护进程时 `dct ps` 不许拉起一个。**
    ///
    /// 「问问有没有东西在跑」不该把「没有」变成「现在有了」——那样每敲一次
    /// ps 都在开发机上多留一个常驻进程。这条同时也是在验证退出码是 0：
    /// 「后台没东西」是个正常答案，不是错误。
    #[test]
    fn ps_without_a_daemon_says_so_and_starts_nothing() {
        let home = tempfile::tempdir().unwrap();
        let (out, _, code) = run_with_home(home.path(), &["ps"]);
        assert_eq!(code, 0, "「没东西在跑」是正常答案，不该是错误");
        assert!(!out.trim().is_empty(), "得说一句话");
        assert!(
            !home.path().join(".dct").join("daemon.sock").exists(),
            "ps 把守护进程拉起来了——它不该有这个副作用"
        );
    }

    #[test]
    fn ps_lists_a_session_that_is_actually_running() {
        let d = Daemon::start();
        let workdir = tempfile::tempdir().unwrap();

        let mut c =
            dct::client::Client::connect(&d.home().join(".dct").join("daemon.sock")).unwrap();
        let id = match c
            .call(dct::proto::Request::Create {
                dir: workdir.path().display().to_string(),
                profile: "shell".into(),
                remember: false,
            })
            .unwrap()
        {
            dct::proto::Response::Created { id } => id,
            other => panic!("预期 Created，实际 {other:?}"),
        };

        let (out, _, code) = d.run(&["ps"]);
        assert_eq!(code, 0);
        assert!(out.contains(&id.to_string()), "ps 里该有会话号 {id}：{out}");
        assert!(out.contains("shell"), "ps 里该有 agent 名字：{out}");
    }

    /// `dct stop <id>` 真的把会话停掉。
    #[test]
    fn stop_by_id_actually_stops_it() {
        let d = Daemon::start();
        let workdir = tempfile::tempdir().unwrap();
        let sock = d.home().join(".dct").join("daemon.sock");

        let mut c = dct::client::Client::connect(&sock).unwrap();
        let id = match c
            .call(dct::proto::Request::Create {
                dir: workdir.path().display().to_string(),
                profile: "shell".into(),
                remember: false,
            })
            .unwrap()
        {
            dct::proto::Response::Created { id } => id,
            other => panic!("预期 Created，实际 {other:?}"),
        };

        let (_, err, code) = d.run(&["stop", &id.to_string()]);
        assert_eq!(code, 0, "停成功该是 0，stderr：{err}");

        match c.call(dct::proto::Request::Screen { id }).unwrap() {
            dct::proto::Response::Screen { state, .. } => {
                assert_eq!(state, dct::session::SessionState::Stopped, "会话没真停")
            }
            other => panic!("预期 Screen，实际 {other:?}"),
        }
    }

    /// 停一个不存在的会话要**非零退出**。
    ///
    /// `dct stop 3 && 干别的` 是很自然的写法；停不成只写进 stderr 的话，
    /// 后面那半句照样会跑。
    #[test]
    fn stopping_a_session_that_is_not_there_exits_nonzero() {
        let d = Daemon::start();
        let (_, err, code) = d.run(&["stop", "4242"]);
        assert_ne!(code, 0, "停不成必须让脚本看得出来");
        assert!(!err.trim().is_empty(), "得说清为什么没停成");
    }

    /// 光敲 `dct stop` 不能等于全停——停会话撤不回来。
    #[test]
    fn a_bare_stop_refuses_instead_of_stopping_everything() {
        let d = Daemon::start();
        let workdir = tempfile::tempdir().unwrap();
        let sock = d.home().join(".dct").join("daemon.sock");
        let mut c = dct::client::Client::connect(&sock).unwrap();
        let id = match c
            .call(dct::proto::Request::Create {
                dir: workdir.path().display().to_string(),
                profile: "shell".into(),
                remember: false,
            })
            .unwrap()
        {
            dct::proto::Response::Created { id } => id,
            other => panic!("预期 Created，实际 {other:?}"),
        };

        let (_, err, code) = d.run(&["stop"]);
        assert_eq!(code, 2, "该是用法错误");
        assert!(!err.trim().is_empty());

        match c.call(dct::proto::Request::Screen { id }).unwrap() {
            dct::proto::Response::Screen { state, .. } => assert_ne!(
                state,
                dct::session::SessionState::Stopped,
                "光敲 dct stop 就把会话停了——这正是不能有的行为"
            ),
            other => panic!("预期 Screen，实际 {other:?}"),
        }
    }

    #[test]
    fn stop_all_stops_every_session() {
        let d = Daemon::start();
        let workdir = tempfile::tempdir().unwrap();
        let sock = d.home().join(".dct").join("daemon.sock");
        let mut c = dct::client::Client::connect(&sock).unwrap();

        let mut ids = Vec::new();
        for _ in 0..2 {
            match c
                .call(dct::proto::Request::Create {
                    dir: workdir.path().display().to_string(),
                    profile: "shell".into(),
                    remember: false,
                })
                .unwrap()
            {
                dct::proto::Response::Created { id } => ids.push(id),
                other => panic!("预期 Created，实际 {other:?}"),
            }
        }

        let (_, err, code) = d.run(&["stop", "--all"]);
        assert_eq!(code, 0, "stderr：{err}");

        for id in ids {
            match c.call(dct::proto::Request::Screen { id }).unwrap() {
                dct::proto::Response::Screen { state, .. } => assert_eq!(
                    state,
                    dct::session::SessionState::Stopped,
                    "{id} 号没被 --all 停掉"
                ),
                other => panic!("预期 Screen，实际 {other:?}"),
            }
        }
    }

    /// 帮助里必须写出这两条命令，否则等于没加。
    #[test]
    fn help_mentions_the_new_commands() {
        let home = tempfile::tempdir().unwrap();
        let (out, _, _) = run_with_home(home.path(), &["--help"]);
        assert!(out.contains("dct ps"), "帮助里该有 ps：{out}");
        assert!(out.contains("dct stop"), "帮助里该有 stop：{out}");
    }
}
