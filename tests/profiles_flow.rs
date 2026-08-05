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
    assert_eq!(entries.len(), 9);
    assert_eq!(entries[0].name, "claude");
    assert_eq!(entries[0].label, "Claude", "要带中文 label");
    let shell = entries.iter().find(|e| e.name == "shell").unwrap();
    assert_eq!(shell.status, ProfileStatus::Ready, "/bin/zsh 一定在");
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
        c.call(Request::LastProfile).unwrap(),
        Response::LastProfile(Some(ref n)) if n == "shell"
    ));
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
        c.call(Request::LastProfile).unwrap(),
        Response::LastProfile(None)
    ));
}
