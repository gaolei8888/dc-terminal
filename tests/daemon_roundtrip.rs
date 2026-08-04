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
fn screens_request_returns_batch_over_socket() {
    let h = common::start_daemon();
    let mut c = h.client();

    let create = |c: &mut dct::client::Client| -> u32 {
        let workdir = tempfile::tempdir().unwrap();
        match c
            .call(Request::Create {
                dir: workdir.path().display().to_string(),
                profile: "shell".into(),
                remember: true,
            })
            .unwrap()
        {
            Response::Created { id } => id,
            other => panic!("预期 Created，实际 {other:?}"),
        }
    };
    let id1 = create(&mut c);
    let id2 = create(&mut c);

    match c
        .call(Request::Screens {
            ids: vec![id1, id2],
        })
        .unwrap()
    {
        Response::Screens { screens } => {
            assert_eq!(screens.len(), 2);
            assert_eq!(screens[0].id, id1);
            assert_eq!(screens[1].id, id2);
        }
        other => panic!("预期 Screens，实际 {other:?}"),
    }
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
