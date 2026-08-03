use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use dct::client::Client;
use dct::proto::{Request, Response};

fn start_daemon(sock: &PathBuf) {
    let s = sock.clone();
    std::thread::spawn(move || {
        let _ = dct::daemon::run(&s);
    });
    for _ in 0..50 {
        if sock.exists() {
            return;
        }
        sleep(Duration::from_millis(50));
    }
    panic!("守护进程没起来：{}", sock.display());
}

fn canon(p: &Path) -> String {
    std::fs::canonicalize(p).unwrap().display().to_string()
}

fn projects(c: &mut Client) -> Vec<String> {
    match c.call(Request::Projects).unwrap() {
        Response::Projects(v) => v,
        other => panic!("预期 Projects，实际 {other:?}"),
    }
}

#[test]
fn create_records_project_most_recent_first() {
    let home = tempfile::tempdir().unwrap();
    let sock = home.path().join("daemon.sock");
    start_daemon(&sock);

    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let mut c = Client::connect(&sock).unwrap();

    // shell profile 不要求 git 仓库，普通临时目录就够
    for d in [a.path(), b.path()] {
        match c
            .call(Request::Create {
                dir: d.display().to_string(),
                profile: "shell".into(),
            })
            .unwrap()
        {
            Response::Created { .. } => {}
            other => panic!("建会话失败：{other:?}"),
        }
    }

    assert_eq!(
        projects(&mut c),
        vec![canon(b.path()), canon(a.path())],
        "后建的项目必须排在前面"
    );
}

#[test]
fn failed_create_is_not_recorded() {
    let home = tempfile::tempdir().unwrap();
    let sock = home.path().join("daemon.sock");
    start_daemon(&sock);

    let mut c = Client::connect(&sock).unwrap();
    let missing = "/tmp/dct-这个目录不存在-9f3a2b";
    match c
        .call(Request::Create {
            dir: missing.into(),
            profile: "shell".into(),
        })
        .unwrap()
    {
        Response::Error(_) => {}
        other => panic!("目录不存在时应当报错，实际 {other:?}"),
    }

    assert!(projects(&mut c).is_empty(), "建失败的目录不能进最近项目");
}

#[test]
fn projects_is_empty_on_a_fresh_daemon() {
    let home = tempfile::tempdir().unwrap();
    let sock = home.path().join("daemon.sock");
    start_daemon(&sock);

    let mut c = Client::connect(&sock).unwrap();
    assert!(projects(&mut c).is_empty(), "全新守护进程的列表应为空");
}
