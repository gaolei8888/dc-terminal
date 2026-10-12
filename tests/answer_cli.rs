//! `dct answer --json` 命令行：用沙箱 HOME 和本机回环上的假 Jeff / 假大模型（一个小脚本），
//! 不碰真模型、不出本机，也不碰真的 ~/.dct。
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run(home: &Path, args: &[&str], stdin: &str) -> Out {
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
    Out {
        code: o.status.code(),
        stdout: String::from_utf8_lossy(&o.stdout).into(),
        stderr: String::from_utf8_lossy(&o.stderr).into(),
    }
}

fn answer(home: &Path, stdin: &str) -> Out {
    run(home, &["answer", "--json"], stdin)
}

fn json(o: &Out) -> serde_json::Value {
    assert_eq!(
        o.stdout.lines().count(),
        1,
        "stdout 要正好一行：{:?}",
        o.stdout
    );
    serde_json::from_str(o.stdout.trim()).unwrap()
}

fn write_config(home: &Path, text: &str) {
    let d = home.join(".dct");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("config.toml"), text).unwrap();
}

const REQ: &str =
    r#"{"state":"pick one","options":[{"key":"a","text":"Alpha"},{"key":"b","text":"Beta"}]}"#;

/// 假 Jeff：本机回环上说 Jev 的小服务，`winner` 拿 `p`，其余平分。数一数被问了几次。
fn fake_jeff(winner: &'static str, p: f64) -> (String, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut c) = conn else { return };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let body = loop {
                let n = c.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break String::new();
                }
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(i) = text.find("\r\n\r\n") {
                    let len = text[..i]
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if text.len() >= i + 4 + len {
                        break text[i + 4..i + 4 + len].to_string();
                    }
                }
            };
            h.fetch_add(1, Ordering::SeqCst);
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let keys: Vec<String> = v["questions"]["q"]["criteria"]
                .as_object()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            let rest = (1.0 - p) / (keys.len().max(2) - 1) as f64;
            let probs: serde_json::Map<String, serde_json::Value> = keys
                .iter()
                .map(|k| {
                    (
                        k.clone(),
                        serde_json::json!(if k == winner { p } else { rest }),
                    )
                })
                .collect();
            let reply =
                serde_json::json!({"answers": {"q": {"type": "choice", "probabilities": probs}}})
                    .to_string();
            let _ = write!(
                c,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            );
        }
    });
    (url, hits)
}

/// 假大模型：一个读标准输入、每次被叫就在记号文件里加一行、然后回固定答案的脚本。
#[cfg(unix)]
fn fake_llm(home: &Path, reply: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let marker = home.join("llm-called");
    let script = home.join("fake-llm.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncat >/dev/null\necho called >> '{}'\necho '{}'\n",
            marker.display(),
            reply
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let profiles = home.join(".dct").join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(
        profiles.join("fakellm.toml"),
        format!("name = \"fakellm\"\ncommand = [\"true\"]\nbackend_only = true\n[headless]\ncommand = [\"{}\"]\n", script.display()),
    )
    .unwrap();
    marker
}

#[cfg(unix)]
fn llm_calls(marker: &Path) -> usize {
    std::fs::read_to_string(marker)
        .map(|t| t.lines().count())
        .unwrap_or(0)
}

const LLM_CONFIG: &str = "[llm]\nprovider = \"fakellm\"\ntransport = \"cli\"\n";

#[test]
fn nothing_configured_says_needs_user_with_exit_zero() {
    let home = tempfile::tempdir().unwrap();
    let o = answer(home.path(), REQ);
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    let v = json(&o);
    assert!(v["key"].is_null());
    assert_eq!(v["by"], "none");
    assert_eq!(v["reason"], "needs_user");
}

#[test]
fn a_confident_jeff_answers() {
    let home = tempfile::tempdir().unwrap();
    let (url, hits) = fake_jeff("b", 0.95);
    write_config(home.path(), &format!("[judge]\nbase_url = \"{url}\"\n"));
    let o = answer(home.path(), REQ);
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    let v = json(&o);
    assert_eq!(
        (v["key"].as_str(), v["by"].as_str()),
        (Some("b"), Some("jeff"))
    );
    assert!(
        (v["confidence"].as_f64().unwrap() - 0.95).abs() < 1e-6,
        "把握由 dct 从概率重算"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[test]
fn a_hesitant_jeff_hands_over_to_the_model() {
    let home = tempfile::tempdir().unwrap();
    let (url, hits) = fake_jeff("a", 0.55);
    let marker = fake_llm(home.path(), r#"{"key":"b"}"#);
    write_config(
        home.path(),
        &format!("[judge]\nbase_url = \"{url}\"\n{LLM_CONFIG}"),
    );
    let o = answer(home.path(), REQ);
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    let v = json(&o);
    assert_eq!(
        (v["key"].as_str(), v["by"].as_str()),
        (Some("b"), Some("model"))
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(llm_calls(&marker), 1);
}

#[cfg(unix)]
#[test]
fn a_forbidden_string_never_reaches_the_model_but_jeff_is_still_asked() {
    let home = tempfile::tempdir().unwrap();
    let (url, hits) = fake_jeff("a", 0.55);
    let marker = fake_llm(home.path(), r#"{"key":"b"}"#);
    write_config(
        home.path(),
        &format!("[judge]\nbase_url = \"{url}\"\n{LLM_CONFIG}"),
    );
    let req = r#"{"state":"pick one","options":[{"key":"a","text":"Alpha"},{"key":"b","text":"Send to Acme"}],"forbid":["acme"]}"#;
    let o = answer(home.path(), req);
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    let v = json(&o);
    assert_eq!(v["by"], "none");
    assert!(v["reason"].as_str().unwrap().contains("llm_withheld"));
    assert_eq!(llm_calls(&marker), 0, "大模型一次都不该被叫");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[test]
fn allow_can_switch_the_model_off() {
    let home = tempfile::tempdir().unwrap();
    let marker = fake_llm(home.path(), r#"{"key":"b"}"#);
    write_config(home.path(), LLM_CONFIG);
    let req = r#"{"state":"s","options":[{"key":"a","text":"A"},{"key":"b","text":"B"}],"allow":["jeff","user"]}"#;
    let v = json(&answer(home.path(), req));
    assert_eq!(v["reason"], "needs_user");
    assert_eq!(llm_calls(&marker), 0);
}

#[test]
fn an_unreachable_jeff_is_skipped_not_fatal() {
    let home = tempfile::tempdir().unwrap();
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    write_config(
        home.path(),
        &format!("[judge]\nbase_url = \"http://{dead}\"\ntimeout_ms = 500\n"),
    );
    let o = answer(home.path(), REQ);
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    assert_eq!(json(&o)["reason"], "needs_user");
}

#[cfg(unix)]
#[test]
fn a_model_that_cannot_be_read_degrades_to_needs_user() {
    let home = tempfile::tempdir().unwrap();
    fake_llm(home.path(), "I would rather not say");
    write_config(home.path(), LLM_CONFIG);
    let o = answer(home.path(), REQ);
    assert_eq!(o.code, Some(0), "{}", o.stderr);
    assert_eq!(json(&o)["reason"], "needs_user");
}

#[test]
fn bad_input_is_exit_two_and_prints_nothing_on_stdout() {
    let home = tempfile::tempdir().unwrap();
    for input in ["", "not json", r#"{"state":"s","options":[]}"#] {
        let o = answer(home.path(), input);
        assert_eq!(o.code, Some(2), "{input}");
        assert!(o.stdout.is_empty());
        assert!(!o.stderr.is_empty());
    }
}

#[test]
fn an_oversized_request_is_exit_two() {
    let home = tempfile::tempdir().unwrap();
    let o = answer(home.path(), &"x".repeat(1 << 21));
    assert_eq!(o.code, Some(2));
}

#[test]
fn without_json_or_with_extra_args_it_is_a_usage_error() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(run(home.path(), &["answer"], REQ).code, Some(2));
    assert_eq!(
        run(home.path(), &["answer", "--json", "--x"], REQ).code,
        Some(2)
    );
}

#[test]
fn a_broken_config_file_is_exit_one_not_a_silent_skip() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "[judge\nnope");
    let o = answer(home.path(), REQ);
    assert_eq!(o.code, Some(1));
    assert!(o.stdout.is_empty());
}
