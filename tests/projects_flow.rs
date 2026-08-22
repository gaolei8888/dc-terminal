use dct::client::Client;
use dct::proto::{Request, Response};

mod common;

/// 跟守护进程用的是同一种归一（`projects::key_of` → `sys::fs::canonicalize`）。
/// 直接用标准库那一个的话，macOS 上会少解一层 `/private`，Windows 上会多一个
/// `\\?\` 前缀——两边都会让断言因为跟被测行为无关的原因失败。
fn canon(p: &std::path::Path) -> String {
    dct::sys::fs::canonicalize(p).unwrap().display().to_string()
}

fn projects(c: &mut Client) -> Vec<String> {
    match c.call(Request::Projects).unwrap() {
        Response::Projects { recent, .. } => recent,
        other => panic!("预期 Projects，实际 {other:?}"),
    }
}

#[test]
fn create_records_project_most_recent_first() {
    let h = common::start_daemon();

    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let mut c = h.client();

    // shell profile 不要求 git 仓库，普通临时目录就够
    for d in [a.path(), b.path()] {
        match c
            .call(Request::Create {
                dir: d.display().to_string(),
                profile: "shell".into(),
                remember: true,
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
    let h = common::start_daemon();

    let mut c = h.client();
    let missing = "/tmp/dct-这个目录不存在-9f3a2b";
    match c
        .call(Request::Create {
            dir: missing.into(),
            profile: "shell".into(),
            remember: true,
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
    let h = common::start_daemon();

    let mut c = h.client();
    assert!(projects(&mut c).is_empty(), "全新守护进程的列表应为空");
}

fn pinned(c: &mut Client) -> Vec<String> {
    match c.call(Request::Projects).unwrap() {
        Response::Projects { pinned, .. } => pinned,
        other => panic!("预期 Projects，实际 {other:?}"),
    }
}

/// `p` / `x` 的整条来回，跨 socket 走一遍。
///
/// 单测只盖到 `projects::Store`；`PinProject` / `UnpinProject` 两个请求到
/// 守护进程 handler 这一段，在这个 Task 之前从来没有调用方，也就从来没被
/// 端到端跑过。这里跑一遍：摆上去要**落盘**（重新连一次还在——不然重启
/// dct 那个项目就自己没了，而规矩说的是「只有 `x` 能移除」），拿下来也要
/// 落盘。
#[test]
fn pinning_a_project_survives_a_reconnect_and_unpinning_removes_it() {
    let h = common::start_daemon();
    let d = tempfile::tempdir().unwrap();

    let mut c = h.client();
    assert!(
        pinned(&mut c).is_empty(),
        "全新守护进程（projects.json 还不存在）一条 pinned 都没有"
    );

    assert!(matches!(
        c.call(Request::PinProject {
            dir: d.path().display().to_string()
        })
        .unwrap(),
        Response::Ok
    ));

    // 换一条连接再问，确认答案来自落盘的那份而不是这条连接的内存。
    // 回来的必须是**用户敲的那条路径**：界面拿这一份当组头名字的显示来源，
    // 存/回归一化结果的话，重启一次 dct 项目就自己改了名（macOS 上还会
    // 冒出 `/private/var/…`）。见 `projects::Store::pin`。
    let mut c2 = h.client();
    assert_eq!(pinned(&mut c2), vec![d.path().display().to_string()]);

    assert!(matches!(
        c2.call(Request::UnpinProject {
            dir: d.path().display().to_string()
        })
        .unwrap(),
        Response::Ok
    ));
    assert!(pinned(&mut h.client()).is_empty(), "`x` 之后要真的没了");
}

/// 一个从没开过会话的项目问 `LastProfile`，守护进程要能干脆地答「没有」。
/// 界面靠这个答案缓存负结果（见 `ui::profiles_to_fetch`）；如果它在这里
/// 报错而不是 `LastProfile(None)`，界面就会认为「还没问到」，每 150ms
/// 重问一次。
#[test]
fn a_project_with_no_history_answers_last_profile_with_none() {
    let h = common::start_daemon();
    let d = tempfile::tempdir().unwrap();

    match h
        .client()
        .call(Request::LastProfile {
            dir: d.path().display().to_string(),
        })
        .unwrap()
    {
        Response::LastProfile(None) => {}
        other => panic!("预期 LastProfile(None)，实际 {other:?}"),
    }
}
