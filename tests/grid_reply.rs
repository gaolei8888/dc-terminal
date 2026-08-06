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

use dct::proto::{Request, Response};
use std::time::{Duration, Instant};

mod common;

fn create_shell(c: &mut dct::client::Client, dir: &std::path::Path) -> u32 {
    match c
        .call(Request::Create {
            dir: dir.display().to_string(),
            profile: "shell".into(),
            remember: false,
        })
        .unwrap()
    {
        Response::Created { id } => id,
        other => panic!("预期 Created，实际 {other:?}"),
    }
}

fn screen_text(c: &mut dct::client::Client, id: u32) -> String {
    match c.call(Request::Screen { id }).unwrap() {
        Response::Screen { lines, .. } => lines
            .iter()
            .flat_map(|l| l.iter().map(|s| s.text.clone()))
            .collect(),
        other => panic!("预期 Screen，实际 {other:?}"),
    }
}

/// 等 shell 的提示符画出来。**不认具体的提示符字符**——zsh 是 `%`、bash 是
/// `$`，而这里跑的是用户自己的登录 shell。等「屏幕上有了非空白内容」就够了，
/// 目的只是别在提示符出来之前就往里灌字（那些字会被吞掉）。
fn wait_for_prompt(c: &mut dct::client::Client, id: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if !screen_text(c, id).trim().is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "shell 的提示符一直没出来");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 等屏幕上出现某段文字。PTY 是异步的，写进去到画出来隔着几轮调度。
fn wait_for(c: &mut dct::client::Client, id: u32, needle: &str) -> String {
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
fn wait_for_count(c: &mut dct::client::Client, id: u32, needle: &str, n: usize) -> String {
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
    let h = common::start_daemon();
    let workdir = tempfile::tempdir().unwrap();
    let mut c = h.client();
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
    let h = common::start_daemon();
    let workdir = tempfile::tempdir().unwrap();
    let mut c = h.client();
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
