//! 一个会话的慢操作不能卡住整个看板。
//!
//! `list()` 要逐个锁会话取状态，所以 `send_input` 里那次可能很慢的快照
//! 如果持着会话锁做，大仓库上按一次回车就会让所有客户端的 `List` 一起等。

use std::path::Path;
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use dct::profile::Profile;
use dct::secrets::SecretStore;
use dct::session::SessionManager;

/// 造一个文件足够多的仓库，让快照慢到能测出来
fn big_repo(files: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(p)
            .output()
            .unwrap();
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@example.com"]);
    run(&["config", "user.name", "t"]);
    for i in 0..files {
        let sub = p.join(format!("d{}", i % 50));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(format!("f{i}.txt")), format!("内容 {i}\n")).unwrap();
    }
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "init"]);
    dir
}

fn fake_agent() -> Profile {
    Profile {
        name: "fake".into(),
        command: vec!["cat".into()],
        is_agent: true,
        idle_pattern: None,
        busy_pattern: None,
        env: Default::default(),
        secret: None,
        install: None,
        label: Default::default(),
        note: Default::default(),
    }
}

fn dirty_everything(p: &Path, files: usize) {
    for i in 0..files {
        let sub = p.join(format!("d{}", i % 50));
        std::fs::write(
            sub.join(format!("f{i}.txt")),
            format!("被 agent 改过 {i} {}\n", std::process::id()),
        )
        .unwrap();
    }
}

fn checkpoint_cost(dir: &Path) -> Duration {
    let t = Instant::now();
    dct::git::checkpoint(dir, 999, 0).unwrap();
    t.elapsed()
}

#[test]
fn slow_checkpoint_does_not_block_the_board() {
    let repo = big_repo(6000);

    let m = Arc::new(SessionManager::new());
    m.register_profile(fake_agent());
    let secrets_dir = tempfile::tempdir().unwrap();
    let secrets = SecretStore::load(&secrets_dir.path().join("secrets.toml"));
    let id = m.create(repo.path(), "fake", secrets.get("fake")).unwrap();

    // 模拟 agent 干了一大堆活：快照必须重新哈希这些文件，才会真的慢。
    // 不这么做的话 git 的索引缓存会让第二次快照快到测不出东西。
    dirty_everything(repo.path(), 6000);

    // 确认这个场景下的快照确实够慢，否则这个测试证明不了任何事
    let cost = checkpoint_cost(repo.path());
    assert!(
        cost > Duration::from_millis(150),
        "快照只花了 {cost:?}，测不出持锁的影响"
    );
    dirty_everything(repo.path(), 6000);

    // 后台线程发一次回车，触发慢快照
    let m2 = m.clone();
    let slow = std::thread::spawn(move || {
        let t = Instant::now();
        m2.send_input(id, "").unwrap();
        t.elapsed()
    });

    // 等它真的进到慢操作里
    sleep(Duration::from_millis(30));

    let t = Instant::now();
    let sessions = m.list();
    let list_cost = t.elapsed();

    let slow_cost = slow.join().unwrap();

    assert_eq!(sessions.len(), 1);
    assert!(
        slow_cost > Duration::from_millis(100),
        "回车本身应该是慢的（{slow_cost:?}），否则没测到并发"
    );
    assert!(
        list_cost < Duration::from_millis(100),
        "看板不能被一个会话的慢快照卡住：list 花了 {list_cost:?}，同期回车花了 {slow_cost:?}"
    );
}
