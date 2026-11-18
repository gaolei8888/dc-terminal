//! `Screen` 要把会话状态一起捎回来——贴在会话里时界面只调 `Screen`
//! （`List` 要逐个锁所有会话、取每个的最后一行，16ms 一轮太贵），所以这是
//! 它唯一能知道 agent 已经退出的途径。
//!
//! 走真 socket 而不是直接调 `SessionManager::screen()`：新加的 `state` 字段
//! 要经过 serde 编解码和 `handle()` 的线程边界，单元测试绕开了这两层，
//! 而这个修复的全部前提就是这个字段真能活着到界面手里。

use dct::proto::{EndedHow, ErrorCode, Request, Response};
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
            state,
            SessionState::Stopped,
            "shell 刚起来就报 Stopped，界面会立刻把用户踢回看板"
        ),
        other => panic!("预期 Screen，实际 {other:?}"),
    }
}

/// 会话结束之后 `Screen` 必须说「已结束、怎么结束的」。这条是空白页 bug 的
/// 回归测试：说不出来，界面就会一直画那张空缓冲——agent 在 alternate screen
/// 里画，退出时恢复的主屏从来没被写过，所以「屏是空的」是正常现象，判死活
/// 只能靠守护进程的回答。会话一停就从名册上抹掉，所以回答是一个错误码
/// （`SessionEnded`），不是一张带 `Stopped` 的屏；也不能是「没有这个会话」，
/// 那会让人以为编号敲错了。
#[test]
fn screen_reports_ended_after_the_session_is_stopped() {
    let h = common::start_daemon();
    let workdir = tempfile::tempdir().unwrap();
    let mut c = h.client();
    let id = create_shell(&mut c, workdir.path());

    assert!(matches!(
        c.call(Request::Stop { id }).unwrap(),
        Response::Ok
    ));

    match c.call(Request::Screen { id }).unwrap() {
        Response::Error(ErrorCode::SessionEnded { id: got, how }) => {
            assert_eq!(got, id);
            assert_eq!(how, EndedHow::StoppedByUser);
        }
        other => panic!("预期 SessionEnded，实际 {other:?}"),
    }
    match c.call(Request::List).unwrap() {
        Response::Sessions(v) => assert!(v.iter().all(|s| s.id != id), "停了就该离开名册：{v:?}"),
        other => panic!("预期 Sessions，实际 {other:?}"),
    }
}

/// agent 自己退出：守护进程的 tick 发现进程没了、把它抹掉，`Screen` 说
/// 「agent 自己退出了」。tick 是定时跑的，要等一轮。
#[test]
fn screen_reports_ended_after_the_agent_exits_on_its_own() {
    let h = common::start_daemon();
    let workdir = tempfile::tempdir().unwrap();
    let mut c = h.client();
    let id = create_shell(&mut c, workdir.path());

    assert!(matches!(
        c.call(Request::Input {
            id,
            text: "exit\r".into()
        })
        .unwrap(),
        Response::Ok
    ));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match c.call(Request::Screen { id }).unwrap() {
            Response::Screen { .. } => assert!(
                Instant::now() < deadline,
                "shell 早该退出了，Screen 却还在给屏幕"
            ),
            Response::Error(ErrorCode::SessionEnded { how, .. }) => {
                assert_eq!(how, EndedHow::ExitedOnItsOwn);
                return;
            }
            other => panic!("预期 Screen 或 SessionEnded，实际 {other:?}"),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
