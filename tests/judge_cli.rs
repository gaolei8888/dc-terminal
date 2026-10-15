//! `dct judge` 命令行：没配 `[judge]` 时给一句能照着改的话；不碰网络。
use std::process::Command;

fn run(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_dct"))
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .output()
        .unwrap()
}

#[test]
fn probe_without_judge_section_says_what_to_write() {
    let home = tempfile::tempdir().unwrap();
    let out = run(home.path(), &["judge", "--probe"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("[judge] base_url"), "{err}");
    assert!(err.contains("config.toml"), "{err}");
}

#[test]
fn unknown_flag_is_a_usage_error() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        run(home.path(), &["judge", "--nope"]).status.code(),
        Some(2)
    );
}
