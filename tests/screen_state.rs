//! `Screen` 要把会话状态一起捎回来——贴在会话里时界面只调 `Screen`
//! （`List` 要逐个锁所有会话、取每个的最后一行，16ms 一轮太贵），所以这是
//! 它唯一能知道 agent 已经退出的途径。
//!
//! 走真 socket 而不是直接调 `SessionManager::screen()`：新加的 `state` 字段
//! 要经过 serde 编解码和 `handle()` 的线程边界，单元测试绕开了这两层，
//! 而这个修复的全部前提就是这个字段真能活着到界面手里。

use dct::proto::{Request, Response};
use dct::session::SessionState;
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

#[test]
fn screen_carries_the_session_state_over_the_socket() {
    let h = common::start_daemon();
    let workdir = tempfile::tempdir().unwrap();
    let mut c = h.client();
    let id = create_shell(&mut c, workdir.path());

    match c.call(Request::Screen { id }).unwrap() {
        Response::Screen { state, .. } => assert_ne!(
            state, SessionState::Stopped,
            "shell 刚起来就报 Stopped，界面会立刻把用户踢回看板"
        ),
        other => panic!("预期 Screen，实际 {other:?}"),
    }
}

/// 会话结束之后 `Screen` 必须报 `Stopped`。这条是空白页 bug 的回归测试：
/// 报不出来，界面就会一直画那张空缓冲——agent 在 alternate screen 里画，
/// 退出时恢复的主屏从来没被写过，所以「屏是空的」是正常现象，判死活只能靠状态。
#[test]
fn screen_reports_stopped_after_the_session_ends() {
    let h = common::start_daemon();
    let workdir = tempfile::tempdir().unwrap();
    let mut c = h.client();
    let id = create_shell(&mut c, workdir.path());

    assert!(matches!(
        c.call(Request::Stop { id }).unwrap(),
        Response::Ok
    ));

    // 守护进程的 tick 是定时跑的，状态落地要等一轮
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match c.call(Request::Screen { id }).unwrap() {
            Response::Screen { state, .. } => {
                if state == SessionState::Stopped {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "会话已经 stop 了，Screen 却一直报 {state:?}"
                );
            }
            other => panic!("预期 Screen，实际 {other:?}"),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
