use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use dct::client::Client;
use dct::proto::{Request, Response};

#[test]
fn daemon_serves_create_list_and_stop() {
    let dir = tempfile::tempdir().unwrap();
    let sock: PathBuf = dir.path().join("d.sock");

    let s = sock.clone();
    std::thread::spawn(move || {
        dct::daemon::run(&s).unwrap();
    });

    // 等 socket 出现
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        sleep(Duration::from_millis(50));
    }

    let workdir = tempfile::tempdir().unwrap();
    let mut c = Client::connect(&sock).unwrap();

    let resp = c
        .call(Request::Create {
            dir: workdir.path().display().to_string(),
            profile: "shell".into(),
        })
        .unwrap();
    let id = match resp {
        Response::Created { id } => id,
        other => panic!("预期 Created，实际 {other:?}"),
    };

    match c.call(Request::List).unwrap() {
        Response::Sessions(v) => {
            assert_eq!(v.len(), 1);
            assert_eq!(v[0].id, id);
            assert_eq!(v[0].profile, "shell");
        }
        other => panic!("预期 Sessions，实际 {other:?}"),
    }

    assert!(matches!(
        c.call(Request::Stop { id }).unwrap(),
        Response::Ok
    ));
}

#[test]
fn unknown_session_returns_error_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let sock: PathBuf = dir.path().join("e.sock");
    let s = sock.clone();
    std::thread::spawn(move || {
        dct::daemon::run(&s).unwrap();
    });
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        sleep(Duration::from_millis(50));
    }

    let mut c = Client::connect(&sock).unwrap();
    match c.call(Request::Stop { id: 999 }).unwrap() {
        Response::Error(msg) => assert!(msg.contains("没有这个会话")),
        other => panic!("预期 Error，实际 {other:?}"),
    }
}
