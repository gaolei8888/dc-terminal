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

// ---- dct judge up | down | status ----

fn write_config(home: &std::path::Path, text: &str) {
    std::fs::create_dir_all(home.join(".dct")).unwrap();
    std::fs::write(home.join(".dct/config.toml"), text).unwrap();
}

/// 假服务：这个测试二进制自己再起一份，只跑这个测试，监听 DCT_FAKE_JUDGE_PORT 并答 /health。
#[test]
fn fake_server_entry() {
    use std::io::{Read, Write};
    let Ok(port) = std::env::var("DCT_FAKE_JUDGE_PORT") else {
        return;
    };
    let l = std::net::TcpListener::bind(format!("127.0.0.1:{port}")).unwrap();
    for s in l.incoming() {
        let Ok(mut s) = s else { continue };
        let mut b = [0u8; 512];
        let _ = s.read(&mut b);
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
    }
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .unwrap()
        .success()
}

#[cfg(unix)]
#[test]
fn up_status_down_through_the_binary_in_a_sandbox_home() {
    let home = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let exe = std::env::current_exe().unwrap();
    write_config(
        home.path(),
        &format!(
            "[judge]\nbase_url = \"http://127.0.0.1:{port}\"\n[judge.serve]\ncommand = [{:?}, \"--exact\", \"fake_server_entry\", \"--test-threads=1\"]\nenv = {{ DCT_FAKE_JUDGE_PORT = \"{port}\" }}\nmin_free_percent = 0\nready_timeout_secs = 20\n",
            exe.to_str().unwrap()
        ),
    );
    let st = run(home.path(), &["judge", "status", "--json"]);
    assert_eq!(st.status.code(), Some(1), "not running yet");
    let v: serde_json::Value = serde_json::from_slice(&st.stdout).unwrap();
    assert_eq!(
        (v["healthy"].clone(), v["owned"].clone()),
        (false.into(), false.into())
    );

    let up = run(home.path(), &["judge", "up"]);
    assert_eq!(
        up.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&up.stderr)
    );
    let st = run(home.path(), &["judge", "status", "--json"]);
    assert_eq!(st.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&st.stdout).unwrap();
    assert_eq!(
        (v["healthy"].clone(), v["owned"].clone()),
        (true.into(), true.into())
    );
    let pid = v["pid"].as_u64().unwrap() as u32;
    let again = run(home.path(), &["judge", "up"]);
    assert!(String::from_utf8_lossy(&again.stdout).contains("already healthy"));

    let down = run(home.path(), &["judge", "down"]);
    assert_eq!(
        down.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&down.stderr)
    );
    assert!(!pid_alive(pid) && !home.path().join(".dct/judge-serve.pid").exists());
}

#[test]
fn up_refuses_a_remote_base_url_and_a_missing_command() {
    let home = tempfile::tempdir().unwrap();
    write_config(
        home.path(),
        "[judge]\nbase_url = \"http://192.0.2.1:8796\"\n[judge.serve]\ncommand = [\"/nope\"]\n",
    );
    let out = run(home.path(), &["judge", "up"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("loopback") && err.contains("回环"), "{err}");
    assert_eq!(run(home.path(), &["judge", "down"]).status.code(), Some(1));

    write_config(home.path(), "[judge]\nbase_url = \"http://127.0.0.1:1\"\n");
    let out = run(home.path(), &["judge", "up"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("[judge.serve] command"));
}
