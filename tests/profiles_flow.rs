//! Profiles / 密钥 / 上次用的 agent 走一遍真 socket。

use dct::profile::ProfileStatus;
use dct::proto::{Request, Response};

mod common;

#[test]
fn profiles_returns_entries_with_labels_and_status() {
    let h = common::start_daemon();
    let mut c = h.client();

    let Response::Profiles { entries, warnings } = c
        .call(Request::Profiles {
            lang: dct::i18n::Lang::Zh,
        })
        .unwrap()
    else {
        panic!("应当返回 Profiles");
    };
    assert!(warnings.is_empty(), "干净环境不该有告警");
    assert_eq!(entries.len(), dct::profile::Profile::builtin_names().len());
    assert_eq!(
        entries[0].name, "dc",
        "没写 [menu] 的机器上，菜单第一项就是内置清单的第一项"
    );
    assert_eq!(entries[0].label, "DC", "要带中文 label");
    let shell = entries.iter().find(|e| e.name == "shell").unwrap();
    assert_eq!(
        shell.status,
        ProfileStatus::Ready,
        "命令行走 login_shell()，最差也兜底到 /bin/sh"
    );
    let kimi = entries.iter().find(|e| e.name == "kimi").unwrap();
    assert!(
        kimi.secret.is_some(),
        "需要密钥的条目要把 hint / url 一起带过来，UI 才画得出输入界面"
    );
}

#[test]
fn set_secret_flips_kimi_off_needs_secret() {
    let h = common::start_daemon();
    let mut c = h.client();

    c.call(Request::SetSecret {
        profile: "kimi".into(),
        value: "sk-test".into(),
    })
    .unwrap();

    let Response::Profiles { entries, .. } = c
        .call(Request::Profiles {
            lang: dct::i18n::Lang::Zh,
        })
        .unwrap()
    else {
        panic!()
    };
    let kimi = entries.iter().find(|e| e.name == "kimi").unwrap();
    assert_ne!(
        kimi.status,
        ProfileStatus::NeedsSecret,
        "填了密钥就不该再报缺密钥"
    );
}

#[test]
fn delete_secret_puts_it_back() {
    let h = common::start_daemon();
    let mut c = h.client();
    c.call(Request::SetSecret {
        profile: "kimi".into(),
        value: "sk-test".into(),
    })
    .unwrap();
    c.call(Request::DeleteSecret {
        profile: "kimi".into(),
    })
    .unwrap();

    let Response::Profiles { entries, .. } = c
        .call(Request::Profiles {
            lang: dct::i18n::Lang::Zh,
        })
        .unwrap()
    else {
        panic!()
    };
    let kimi = entries.iter().find(|e| e.name == "kimi").unwrap();
    // claude 装没装取决于跑测试的机器，两种都算对——重点是密钥没了
    assert!(matches!(
        kimi.status,
        ProfileStatus::NeedsSecret | ProfileStatus::NeedsDependency { .. }
    ));
}

#[test]
fn create_with_remember_records_the_profile() {
    let h = common::start_daemon();
    let mut c = h.client();
    let dir = h.git_repo("proj");

    c.call(Request::Create {
        dir: dir.display().to_string(),
        profile: "shell".into(),
        remember: true,
    })
    .unwrap();

    assert!(matches!(
        c.call(Request::LastProfile { dir: dir.display().to_string() }).unwrap(),
        Response::LastProfile(Some(ref n)) if n == "shell"
    ));
}

/// **这一版的招牌功能，端到端跑一遍：每个项目各记各的 agent。**
///
/// 单测（`projects::Store` 那条）只盖到 store 本身，而 `profiles_flow` 里
/// 原有的那条只用了**一个**目录——一个目录分不出「按项目记」和「记一个全局
/// 值」，因为 `last_profile_for` 找不到项目记录时会退回那个全局兜底，两种
/// 实现给的答案一模一样。于是把 `daemon.rs` 里那一行改成往一个固定的假目录
/// 记账（也就是「按项目记」彻底失效），全部 17 个测试二进制照样全绿。
///
/// 两个目录、两个 agent，才问得出这个问题。
#[test]
fn two_projects_each_keep_their_own_agent_over_the_wire() {
    use dct::profile::Profile;
    use dct::session::SessionManager;
    use std::sync::Arc;

    // 用 `cat` 冒充第二个 agent：内置 profile 里只有 `shell` 一定装得上，
    // 而这条测试的全部意义就是**两个不同的名字**。非 agent，所以不要求 git 仓库。
    let fake = Profile {
        name: "profiles-flow-fake".into(),
        command: vec![common::posix_tool("cat")],
        is_agent: false,
        idle_pattern: None,
        busy_pattern: None,
        error_pattern: None,
        env: Default::default(),
        secret: None,
        install: None,
        headless: None,
        api: None,
        label: Default::default(),
        note: Default::default(),
        resume_args: Default::default(),
        pairable: false,
        backend_only: false,
    };

    let home = tempfile::tempdir().unwrap();
    let sock = home.path().join("daemon.sock");
    let mgr = Arc::new(SessionManager::new());
    mgr.register_profile(fake);
    let s = sock.clone();
    std::thread::spawn(move || {
        let _ = dct::daemon::run_with_manager(&s, mgr);
    });
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut c = dct::client::Client::connect(&sock).unwrap();

    let a = home.path().join("proj-a");
    let b = home.path().join("proj-b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();

    for (dir, profile) in [(&a, "shell"), (&b, "profiles-flow-fake")] {
        match c
            .call(Request::Create {
                dir: dir.display().to_string(),
                profile: profile.into(),
                remember: true,
            })
            .unwrap()
        {
            Response::Created { .. } => {}
            other => panic!("{profile} 建会话失败：{other:?}"),
        }
    }

    let last = |c: &mut dct::client::Client, d: &std::path::Path| match c
        .call(Request::LastProfile {
            dir: d.display().to_string(),
        })
        .unwrap()
    {
        Response::LastProfile(x) => x,
        other => panic!("预期 LastProfile，实际 {other:?}"),
    };

    // 两条断言缺一不可：只问 b 的话，全局兜底也会答对（b 是最后写的那个）。
    assert_eq!(
        last(&mut c, &a).as_deref(),
        Some("shell"),
        "先建的那个项目必须还记得自己的 agent，而不是被后一次覆盖"
    );
    assert_eq!(last(&mut c, &b).as_deref(), Some("profiles-flow-fake"));
}

#[test]
fn create_without_remember_does_not_record() {
    // 「帮你装 CLI」开的那个 shell 会话不能变成「上次用的 agent」——
    // 否则用户下次按 n 会直接掉进一个命令行。
    let h = common::start_daemon();
    let mut c = h.client();
    let dir = h.git_repo("proj");

    c.call(Request::Create {
        dir: dir.display().to_string(),
        profile: "shell".into(),
        remember: false,
    })
    .unwrap();

    assert!(matches!(
        c.call(Request::LastProfile {
            dir: dir.display().to_string()
        })
        .unwrap(),
        Response::LastProfile(None)
    ));
}

/// `[menu]` 走一遍真 socket。
///
/// 单元测试证明的是「裁剪函数按清单裁」，这一条证明的是另一件事：daemon
/// 真的**找得到**那份配置。菜单那条路拿不到 socket，配置文件的位置是从
/// profiles 目录反推的（`config_path_for_profiles_dir`），推错的话什么都
/// 不会报错——发机器的人写下的 `[menu]` 只是静静地不生效。
#[test]
fn the_menu_section_trims_the_list_the_daemon_hands_back() {
    let h = common::start_daemon();
    std::fs::write(
        dct::config::config_path_for_socket(&h.sock),
        "[menu]
agents = [\"shell\", \"dc\"]
",
    )
    .unwrap();

    let mut c = h.client();
    let Response::Profiles { entries, .. } = c
        .call(Request::Profiles {
            lang: dct::i18n::Lang::Zh,
        })
        .unwrap()
    else {
        panic!("应当返回 Profiles");
    };
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["shell", "dc"], "只留清单里那两项，顺序照清单");
}
