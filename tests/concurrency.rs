use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use dct::client::Client;
use dct::profile::Profile;
use dct::proto::{Request, Response};
use dct::session::SessionManager;

// 用 cat 冒充 agent：不依赖任何外部 CLI（不能用内置的 "claude" profile，那会真的把本机的
// claude 可执行文件拉起来），能收输入、不会自己退出。
fn fake_agent() -> Profile {
    Profile {
        name: "concurrency-fake".into(),
        command: vec!["cat".into()],
        is_agent: true,
        idle_pattern: None,
        busy_pattern: None,
        error_pattern: None,
        env: Default::default(),
        secret: None,
        install: None,
        label: Default::default(),
        note: Default::default(),
    }
}

/// 造一个有几千个文件的仓库，复现审查者报告里"create_worktree 里的 git checkout 因为文件多
/// 而变慢"的真实场景，而不是用 sleep() 假装慢。
fn init_big_repo(path: &Path, n: usize) {
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@example.com"]);
    run(&["config", "user.name", "t"]);
    std::fs::create_dir_all(path.join("files")).unwrap();
    for i in 0..n {
        std::fs::write(
            path.join("files").join(format!("f{i}.txt")),
            format!("{i}\n"),
        )
        .unwrap();
    }
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "init"]);
}

#[test]
fn list_is_not_blocked_by_slow_create() {
    let dir = tempfile::tempdir().unwrap();
    let sock: PathBuf = dir.path().join("slow.sock");

    let repo = tempfile::tempdir().unwrap();
    init_big_repo(repo.path(), 8000);

    let mgr = std::sync::Arc::new(SessionManager::new());
    mgr.register_profile(fake_agent());

    let s = sock.clone();
    let mgr_for_daemon = mgr.clone();
    std::thread::spawn(move || {
        dct::daemon::run_with_manager(&s, mgr_for_daemon).unwrap();
    });

    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        sleep(Duration::from_millis(50));
    }

    let repo_path = repo.path().display().to_string();
    let sock_for_create = sock.clone();
    let create_handle = std::thread::spawn(move || {
        let mut c = Client::connect(&sock_for_create).unwrap();
        let start = Instant::now();
        let resp = c
            .call(Request::Create {
                dir: repo_path,
                profile: "concurrency-fake".into(),
                remember: true,
            })
            .unwrap();
        (start.elapsed(), resp)
    });

    // 给慢 Create 一点时间真正进到耗时的 git 操作里
    sleep(Duration::from_millis(150));

    let mut list_client = Client::connect(&sock).unwrap();
    let list_start = Instant::now();
    let list_resp = list_client.call(Request::List).unwrap();
    let list_elapsed = list_start.elapsed();

    let (create_elapsed, create_resp) = create_handle.join().unwrap();

    eprintln!("create_elapsed={create_elapsed:?} list_elapsed={list_elapsed:?}");

    assert!(
        matches!(list_resp, Response::Sessions(_)),
        "List 应该正常返回，实际 {list_resp:?}"
    );
    assert!(
        create_elapsed > Duration::from_millis(300),
        "场景失真：Create 耗时应显著大于 300ms 才能验证不阻塞其它客户端，实际 {create_elapsed:?}"
    );
    assert!(
        list_elapsed < Duration::from_millis(100),
        "List 被慢 Create 卡住了：耗时 {list_elapsed:?}（要求 < 100ms）"
    );
    assert!(
        matches!(create_resp, Response::Created { .. }),
        "Create 应该最终成功，实际 {create_resp:?}"
    );
}
