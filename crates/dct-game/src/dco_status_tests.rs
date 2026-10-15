use crate::dco_status::{probe, DcoState, BUDGET};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// 假 dco 对 `status` 的回法。
enum Reply {
    Body(Value),
    /// 原样写一行（用来喂读不懂的话）。
    Raw(&'static str),
    /// 先睡再答。
    Slow(Duration, Value),
}

/// 在 `home/.dco` 下摆好 endpoint.json 和钥匙，起一个只接一条连接的假 dco；返回它见到的方法名。
fn fake(home: &std::path::Path, reply: Reply) -> std::thread::JoinHandle<Vec<String>> {
    let dir = home.join(".dco");
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("dco.sock");
    std::fs::write(dir.join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.join("token"), TOKEN).unwrap();
    let l = UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let mut seen = vec![];
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let _ = writeln!(w, r#"{{"ok":true}}"#);
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                return seen;
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let method = req["method"].as_str().unwrap().to_string();
            if method == "tools/call" {
                seen.push(req["params"]["name"].as_str().unwrap().to_string());
            } else {
                seen.push(method.clone());
            }
            let Some(id) = req.get("id") else { continue };
            let result = if method == "tools/call" {
                let body = match &reply {
                    Reply::Raw(raw) => {
                        let _ = writeln!(w, "{raw}");
                        continue;
                    }
                    Reply::Slow(d, b) => {
                        std::thread::sleep(*d);
                        b.clone()
                    }
                    Reply::Body(b) => b.clone(),
                };
                json!({"content": [{"type": "text", "text": body.to_string()}], "isError": false})
            } else {
                json!({"protocolVersion": "2025-06-18"})
            };
            if writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).is_err() {
                return seen;
            }
        }
    })
}

fn body(state: &str) -> Value {
    with_attention(state, &[])
}

fn with_attention(state: &str, kinds: &[&str]) -> Value {
    let att: Vec<Value> = kinds.iter().map(|k| json!({"level": "yellow", "kind": k, "text": "need you"})).collect();
    json!({"activity": {"schema_version": 1, "state": state, "runs": [], "attention": att}})
}

#[test]
fn each_reply_shape_comes_through_and_only_status_is_called() {
    let cases: Vec<(Value, &str, Value)> = vec![
        (body("idle"), "idle", json!([])),
        (body("running"), "running", json!([])),
        (with_attention("running", &["dco.needs_help"]), "waiting_user", json!(["needs_help"])),
        (body("paused"), "paused", json!([])),
        (body("halted"), "halted", json!([])),
        (with_attention("halted", &["dco.needs_help"]), "halted", json!(["needs_help"])),
        (with_attention("running", &["dco.permission_missing"]), "permission_missing", json!(["permission_missing"])),
        (with_attention("idle", &["dco.screen_locked"]), "screen_locked", json!(["screen_locked"])),
        (with_attention("idle", &["dco.something_new"]), "unknown", json!([])),
        (body("sleeping"), "unknown", json!([])),
        (with_attention("running", &["dco.needs_help", "dco.needs_help", "dco.paused"]), "paused", json!(["paused", "needs_help"])),
        (with_attention("idle", &["dco.junk"; 12]), "unknown", json!([])),
        (json!({"activity": {"schema_version": 1, "state": "idle", "runs": [], "attention": [7, null, "x"]}}), "unknown", json!([])),
    ];
    for (reply, want_state, want_att) in cases {
        let home = tempfile::tempdir().unwrap();
        let h = fake(home.path(), Reply::Body(reply.clone()));
        let s = probe(home.path(), BUDGET).expect("endpoint file is there");
        let j = s.to_json();
        assert_eq!(j["state"], want_state, "{reply}");
        assert_eq!(j["attention"], want_att, "{reply}");
        let seen = h.join().unwrap();
        assert_eq!(seen, ["initialize", "notifications/initialized", "status"], "只许读 status：{seen:?}");
    }
}

#[test]
fn no_endpoint_file_means_not_installed() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(probe(home.path(), BUDGET), None);
}

#[test]
fn an_endpoint_with_nobody_listening_is_down() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".dco");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("endpoint.json"), json!({"socket": dir.join("gone.sock")}).to_string()).unwrap();
    std::fs::write(dir.join("token"), TOKEN).unwrap();
    assert_eq!(probe(home.path(), BUDGET).unwrap().state, DcoState::Down);
}

#[test]
fn a_malformed_reply_is_unknown() {
    for reply in [Reply::Body(json!({"nope": 1})), Reply::Body(body("???")), Reply::Body(Value::Null)] {
        let home = tempfile::tempdir().unwrap();
        let h = fake(home.path(), reply);
        assert_eq!(probe(home.path(), BUDGET).unwrap().state, DcoState::Unknown);
        let _ = h.join();
    }
    // 回的不是一行 JSON：连上了却读不懂 = unknown，不是 down，更不是 idle。
    let home = tempfile::tempdir().unwrap();
    let h = fake(home.path(), Reply::Raw("this is not json"));
    assert_eq!(probe(home.path(), BUDGET).unwrap().state, DcoState::Unknown);
    let _ = h.join();
}

#[test]
fn a_slow_dco_gives_unknown_within_the_budget() {
    let home = tempfile::tempdir().unwrap();
    let _h = fake(home.path(), Reply::Slow(Duration::from_secs(5), body("idle")));
    let t = Instant::now();
    let s = probe(home.path(), BUDGET).unwrap();
    let took = t.elapsed();
    assert_eq!(s.state, DcoState::Unknown);
    assert!(took < Duration::from_millis(1000), "总期限 800ms，实际 {took:?}");
}
