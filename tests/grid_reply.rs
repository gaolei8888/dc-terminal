//! 九宫格回复框依赖的那条发送路径，走真 socket 跑一遍。
//!
//! 界面侧 `grid::send_reply` 把一句话拆成**两次** `Input`：先送文字，再送一个
//! **空串**。空串在守护进程侧就是「按回车」（`session.rs::send_input`），而且
//! 回车那一步还会打检查点。这个约定全靠那一处实现，界面这边没有任何东西能
//! 证明它成立——`send_reply` 要连守护进程，UI 单测里的 `App` 是断连的，跑不到。
//!
//! 所以这条契约只能在这里钉：**文字 + 空串真的等于「打了一行字并回车」**。
//! 它一旦变了（比如哪天 `Input` 自己带上换行），界面会安静地退化成「字发过去
//! 了但 agent 一直在等回车」——用户以为自己回过话了，其实对面还停着。

use dct::client::Client;
use dct::profile::Profile;
use dct::proto::{Request, Response};
use dct::session::SessionManager;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod common;

/// 这条契约测过一遍守护进程真实的 socket 往返，所以起daemon 得走真 socket。
/// 但它**不能**用内置的 `shell` profile（`/bin/zsh`）——那是开发者自己的
/// 登录 shell，会 source 真实的 `~/.zshrc`，提示符画出来的时间取决于那份
/// rc 文件有多重，满载并行跑 `cargo test` 时经常输给固定的等待期限
/// （详见 `.superpowers/sdd/2026-08-09-dct-session-auto-name/followup-2-brief.md`）。
///
/// 换成一个测试自己注册的 profile：`/bin/sh --noediting`。选它是因为：
/// - `--noediting` 关掉 GNU Readline，shell 就不会在某个不确定的时刻把
///   终端切成 raw 模式——不然那次切换本身又是一个新的竞态窗口。
/// - `env.ENV = "/dev/null"`：sh 以 `sh` 这个名字启动、且是交互式时，会去读
///   `$ENV` 指向的文件当启动脚本（posix 模式下 sh 版本的「rc 文件」）。显式
///   摁死它，不管运行测试的机器上这个变量有没有被意外设置过。
/// - `env.PS1` 钉死成一个测试专用的固定串，`wait_for_prompt` 就不用再猜
///   「屏幕上随便出现点什么」，可以直接等这一句话。
///
/// 走的正是 `SessionManager::register_profile`（`session.rs:552`）——先把
/// profile 塞进一个 `SessionManager`，再拿它起 `daemon::run_with_manager`。
/// 这条路**隔着 socket 够得到**：`create()` 内部的 `resolve_profile`
/// （`session.rs:564-572`）查 `extra_profiles`（也就是 `register_profile`
/// 注册的那张表）这一步，跟请求是不是从 socket 来的无关。写这份测试之前
/// 有一版简报断言够不到、只能靠磁盘上的 profile 文件——读了
/// `resolve_profile` 才发现那是错的：`concurrency.rs`、`profiles_flow.rs`
/// 的 `two_projects_each_keep_their_own_agent_over_the_wire` 早就是「起
/// 真 socket + `register_profile`」这个用法，这里照抄的是那个已经验证过
/// 的现成手法，不是发明新机制，也不用再多一层磁盘 TOML 的间接。
const PROMPT: &str = "dct-test$ ";
const TEST_SHELL_PROFILE: &str = "grid-reply-test-shell";

fn test_shell_profile() -> Profile {
    let mut env = BTreeMap::new();
    env.insert("ENV".to_string(), "/dev/null".to_string());
    env.insert("PS1".to_string(), PROMPT.to_string());
    Profile {
        name: TEST_SHELL_PROFILE.into(),
        command: vec![common::posix_tool("sh"), "--noediting".into()],
        is_agent: false,
        idle_pattern: None,
        busy_pattern: None,
        error_pattern: None,
        env,
        secret: None,
        install: None,
        headless: None,
        api: None,
        label: Default::default(),
        note: Default::default(),
        resume_args: Default::default(),
        pairable: false,
        backend_only: false,
    }
}

/// 起一个只在这个测试文件里活的守护进程，`SessionManager` 里预先注册好
/// `test_shell_profile()`。跟 `tests/common::start_daemon()` 长得像，但没有
/// 抽到那边去——`common::mod.rs` 头上的注释说得很清楚，那个共用脚手架是
/// 「零参数、内部自己 new 一个 manager」的形状，塞不下「起daemon 之前先
/// register_profile」这个需求，硬塞会让共用代码长出只有一个调用方用得到的
/// 分支。`concurrency.rs` 已经因为同样的理由自己长了一份，这里是第二份。
fn start_daemon() -> (tempfile::TempDir, std::path::PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let sock = home.path().join("daemon.sock");
    let mgr = Arc::new(SessionManager::new());
    mgr.register_profile(test_shell_profile());
    let s = sock.clone();
    std::thread::spawn(move || {
        let _ = dct::daemon::run_with_manager(&s, mgr);
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !sock.exists() {
        assert!(
            Instant::now() < deadline,
            "守护进程没起来：{}",
            sock.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    (home, sock)
}

fn create_shell(c: &mut Client, dir: &std::path::Path) -> u32 {
    match c
        .call(Request::Create {
            dir: dir.display().to_string(),
            profile: TEST_SHELL_PROFILE.into(),
            remember: false,
        })
        .unwrap()
    {
        Response::Created { id } => id,
        other => panic!("预期 Created，实际 {other:?}"),
    }
}

fn screen_text(c: &mut Client, id: u32) -> String {
    match c.call(Request::Screen { id }).unwrap() {
        Response::Screen { lines, .. } => lines
            .iter()
            .flat_map(|l| l.iter().map(|s| s.text.clone()))
            .collect(),
        other => panic!("预期 Screen，实际 {other:?}"),
    }
}

/// 等测试 shell 的提示符画出来。跟原来那版不一样的地方是：这里不用再猜
/// 「随便什么非空白内容」——`test_shell_profile()` 把 `PS1` 钉死成了
/// `PROMPT`，直接等这一句话，比“非空白”更准，也不会被半行没画完的回显
/// 骗过去。等的理由不变：提示符出来之前发的字会被吞掉。
fn wait_for_prompt(c: &mut Client, id: u32) {
    wait_for(c, id, PROMPT);
}

/// 等屏幕上出现某段文字。PTY 是异步的，写进去到画出来隔着几轮调度。
fn wait_for(c: &mut Client, id: u32, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = screen_text(c, id);
        if s.contains(needle) {
            return s;
        }
        if Instant::now() >= deadline {
            panic!("等不到「{needle}」，屏幕上是：{s}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 等某段文字出现到第 `n` 次。
///
/// 「命令被执行了」的判据是它出现**两次**（一次是回显，一次是输出），所以
/// 不能用 `wait_for`——回显那一次早就在屏幕上了，`wait_for` 会当场返回一张
/// 回车之前的旧屏，于是这条测试无论回车送没送到都「通过」。
fn wait_for_count(c: &mut Client, id: u32, needle: &str, n: usize) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = screen_text(c, id);
        if s.matches(needle).count() >= n {
            return s;
        }
        if Instant::now() >= deadline {
            panic!("「{needle}」没出现到 {n} 次，屏幕上是：{s}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 文字 + 空串 = 打了一行并回车。
///
/// 用 `echo` 是因为它的效果**不在输入行上**：光看见自己打的字不能证明回车
/// 送到了——那行字在按回车之前就已经回显在屏幕上了。只有 shell 真的执行了
/// 命令、把结果打印出来，才说明这一行被提交了。
#[test]
fn text_then_an_empty_input_submits_the_line() {
    let (_home, sock) = start_daemon();
    let workdir = tempfile::tempdir().unwrap();
    let mut c = Client::connect(&sock).unwrap();
    let id = create_shell(&mut c, workdir.path());

    // shell 起来、提示符画出来之前发的字会被吞掉
    wait_for_prompt(&mut c, id);

    // 界面侧 send_reply 的两步，顺序和内容都照抄
    assert!(matches!(
        c.call(Request::Input {
            id,
            text: "echo dct-reply-landed".into(),
        })
        .unwrap(),
        Response::Ok
    ));
    assert!(matches!(
        c.call(Request::Input {
            id,
            text: String::new(),
        })
        .unwrap(),
        Response::Ok
    ));

    // `echo` 的**输出**独占一行，跟回显的命令行不是同一行。只出现一次
    // 就说明命令回显了但没被执行——也就是回车没送到。
    wait_for_count(&mut c, id, "dct-reply-landed", 2);
}

/// 空框直接回车（只发空串，不发文字）也得真的按下去。
///
/// 这是九宫格里最高频的用法：批个计划、说声继续。它跟上面那条走的是不同
/// 分支——`send_reply` 在 `body` 为空时只发一次 `Input`，一次都不能少。
#[test]
fn an_empty_input_on_its_own_is_a_bare_enter() {
    let (_home, sock) = start_daemon();
    let workdir = tempfile::tempdir().unwrap();
    let mut c = Client::connect(&sock).unwrap();
    let id = create_shell(&mut c, workdir.path());
    wait_for_prompt(&mut c, id);

    // 先把命令打进去但**不**回车，模拟「agent 那边已经摆着一个待确认的东西」
    c.call(Request::Input {
        id,
        text: "echo bare-enter-works".into(),
    })
    .unwrap();
    // 先等回显画完再数。PTY 是异步的，写进去到画出来隔着几轮调度——
    // 不等就去数，数到的是「echo bare-en」这种画到一半的状态。
    let before = wait_for(&mut c, id, "bare-enter-works");
    assert_eq!(
        before.matches("bare-enter-works").count(),
        1,
        "这时候该只有回显那一次，命令还没被执行：{before}"
    );

    // 只发空串 = 只按回车
    assert!(matches!(
        c.call(Request::Input {
            id,
            text: String::new(),
        })
        .unwrap(),
        Response::Ok
    ));

    wait_for_count(&mut c, id, "bare-enter-works", 2);
}
