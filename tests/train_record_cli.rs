//! `dct train record --json` 命令行：沙箱 HOME，真的 ~/.dct 一个字节都不碰。
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn run(home: &Path, args: &[&str], stdin: &str) -> (Option<i32>, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dct"))
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // 对面读够了就退出，写一半被断开是正常的（超长请求那条测试）。
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    let o = child.wait_with_output().unwrap();
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stdout).into(),
        String::from_utf8_lossy(&o.stderr).into(),
    )
}

fn rec(home: &Path, stdin: &str) -> (Option<i32>, String, String) {
    run(home, &["train", "record", "--json"], stdin)
}

fn answers(home: &Path) -> std::path::PathBuf {
    home.join(".dct").join("explore").join("answers.jsonl")
}

fn req(by: &str, outcome: &str) -> String {
    format!(
        r#"{{"question_id":"q9","by":"{by}","key":"e3","outcome":"{outcome}","run_outcome":"published"}}"#
    )
}

#[test]
fn every_by_and_outcome_lands_in_the_sandbox_file_with_the_shared_label() {
    let home = tempfile::tempdir().unwrap();
    let mut n = 0;
    for by in ["jeff", "model", "user"] {
        for outcome in ["good", "bad", "unknown"] {
            let (code, out, err) = rec(home.path(), &req(by, outcome));
            assert_eq!(code, Some(0), "{by} {outcome}: {err}");
            let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
            let want = by == "user" || (by == "model" && outcome == "good");
            assert_eq!(v["recorded"], true);
            assert_eq!(v["label"], want, "{by} {outcome}");
            n += 1;
        }
    }
    let text = std::fs::read_to_string(answers(home.path())).unwrap();
    assert_eq!(text.lines().count(), n);
    for l in text.lines() {
        let row: serde_json::Value = serde_json::from_str(l).unwrap();
        assert_eq!(row["v"], 2);
    }
}

#[cfg(unix)]
#[test]
fn the_file_is_mode_0600() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    rec(home.path(), &req("user", "good"));
    let mode = std::fs::metadata(answers(home.path()))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_forbidden_string_is_exit_two_and_nothing_is_written() {
    let home = tempfile::tempdir().unwrap();
    let input = r#"{"question_id":"q","by":"user","key":"a","outcome":"good","run_outcome":"published","state":"mail Acme","forbid":["acme"]}"#;
    let (code, out, err) = rec(home.path(), input);
    assert_eq!(code, Some(2));
    assert!(out.is_empty() && err.contains("forbid"));
    assert!(!answers(home.path()).exists());
}

#[test]
fn bad_input_and_usage_errors_are_exit_two() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(rec(home.path(), "nope").0, Some(2));
    assert_eq!(rec(home.path(), &req("robot", "good")).0, Some(2));
    assert_eq!(
        run(home.path(), &["train", "record"], &req("user", "good")).0,
        Some(2)
    );
    assert_eq!(run(home.path(), &["train"], "").0, Some(2));
    assert!(!answers(home.path()).exists());
}
