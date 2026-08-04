use dct::proto::{Request, Response};

mod common;

#[test]
fn daemon_serves_create_list_and_stop() {
    let h = common::start_daemon();

    let workdir = tempfile::tempdir().unwrap();
    let mut c = h.client();

    let resp = c
        .call(Request::Create {
            dir: workdir.path().display().to_string(),
            profile: "shell".into(),
            remember: true,
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
    let h = common::start_daemon();
    let mut c = h.client();
    match c.call(Request::Stop { id: 999 }).unwrap() {
        Response::Error(msg) => assert!(msg.contains("没有这个会话")),
        other => panic!("预期 Error，实际 {other:?}"),
    }
}
