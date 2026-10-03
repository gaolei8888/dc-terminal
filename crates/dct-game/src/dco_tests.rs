use crate::dco::DcoClient;
use crate::play::{Dco, Profile};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

/// 假 dco：按 dco 真实的握手和回复形状说话。`handler(tool, args)` 返回 (JSON, 是不是错误)。
fn fake(dir: &std::path::Path, token: &str, handler: impl Fn(&str, &Value) -> (Value, bool) + Send + 'static) -> std::thread::JoinHandle<Vec<String>> {
    let sock = dir.join("dco.sock");
    std::fs::write(dir.join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.join("token"), token).unwrap();
    let l = UnixListener::bind(&sock).unwrap();
    let token = token.to_string();
    std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let mut seen = vec![];
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let hello: Value = serde_json::from_str(line.trim()).unwrap();
        seen.push(hello.to_string());
        if hello["dco_token"] != token {
            writeln!(w, r#"{{"ok":false}}"#).unwrap();
            return seen;
        }
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return seen;
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            seen.push(req["method"].as_str().unwrap().to_string());
            let Some(id) = req.get("id") else { continue };
            let result = match req["method"].as_str().unwrap() {
                "initialize" => json!({"protocolVersion": "2025-06-18"}),
                "tools/call" => {
                    let (body, is_error) = handler(req["params"]["name"].as_str().unwrap(), &req["params"]["arguments"]);
                    json!({"content": [{"type": "text", "text": body.to_string()}], "isError": is_error})
                }
                _ => json!({}),
            };
            writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap();
        }
    })
}

fn profile() -> Profile {
    Profile { window: json!({"app": "iPhone Mirroring"}), region: [0.2, 0.3, 0.5, 0.4], rows: 2, cols: 2, extra: json!({"class_de": 20.0}) }
}

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn reads_a_grid_with_the_extra_parameters_and_swipes() {
    let dir = tempfile::tempdir().unwrap();
    let h = fake(dir.path(), TOKEN, |tool, args| match tool {
        "read_grid" => {
            assert_eq!(args["rows"], 2);
            assert_eq!(args["class_de"], 20.0);
            assert_eq!(args["region"]["w"], 0.5);
            assert_eq!(args["window"]["app"], "iPhone Mirroring");
            (json!({"rows":2,"cols":2,"cells":[[0,0],[1,1]],"odd":[[false,false],[false,false]],
                    "classes":[{"id":0,"rgb":[1,2,3],"count":2},{"id":1,"rgb":[4,5,6],"count":2}],"elapsed_ms":1}), false)
        }
        "swipe" => {
            assert_eq!(args["from"]["x"], 0.3);
            assert_eq!(args["to"]["y"], 0.5);
            (json!({"swiped": true}), false)
        }
        other => panic!("没想到会调 {other}"),
    });
    let mut c = DcoClient::connect(dir.path()).unwrap();
    let g = c.read_grid(&profile()).unwrap();
    assert_eq!((g.rows, g.cols, g.classes.len()), (2, 2, 2));
    c.swipe(&profile(), (0.3, 0.4), (0.4, 0.5)).unwrap();
    drop(c);
    let seen = h.join().unwrap();
    assert_eq!(&seen[1..4], ["initialize", "notifications/initialized", "tools/call"]);
}

#[test]
fn a_tool_error_keeps_dcos_error_code() {
    let dir = tempfile::tempdir().unwrap();
    let _h = fake(dir.path(), TOKEN, |_, _| (json!({"error": {"code": "halted", "message": "急停中"}}), true));
    let mut c = DcoClient::connect(dir.path()).unwrap();
    let e = c.swipe(&profile(), (0.3, 0.4), (0.4, 0.5)).unwrap_err();
    assert_eq!((e.code.as_str(), e.message.as_str()), ("halted", "急停中"));
}

#[test]
fn a_wrong_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let _h = fake(dir.path(), TOKEN, |_, _| (json!({}), false));
    std::fs::write(dir.path().join("token"), "f".repeat(64)).unwrap();
    assert_eq!(DcoClient::connect(dir.path()).err().unwrap().code, "dco_refused");
}

#[test]
fn no_endpoint_file_means_dco_is_not_running() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(DcoClient::connect(dir.path()).err().unwrap().code, "dco_down");
}

#[test]
fn a_dead_socket_means_dco_is_not_running() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("endpoint.json"), json!({"socket": dir.path().join("none.sock")}).to_string()).unwrap();
    std::fs::write(dir.path().join("token"), TOKEN).unwrap();
    assert_eq!(DcoClient::connect(dir.path()).err().unwrap().code, "dco_down");
}

#[test]
fn a_dco_that_never_answers_times_out_with_its_own_code() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("dco.sock");
    std::fs::write(dir.path().join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.path().join("token"), TOKEN).unwrap();
    let l = UnixListener::bind(&sock).unwrap();
    let _h = std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(3));
        drop(s);
    });
    let t = std::time::Instant::now();
    let e = DcoClient::connect_with_timeout(dir.path(), std::time::Duration::from_millis(200)).err().unwrap();
    assert_eq!(e.code, "dco_timeout");
    assert!(e.message.contains("没有回应"));
    assert!(t.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn an_unknown_tool_error_means_dco_is_too_old() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("dco.sock");
    std::fs::write(dir.path().join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.path().join("token"), TOKEN).unwrap();
    let l = UnixListener::bind(&sock).unwrap();
    let _h = std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return;
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let Some(id) = req.get("id") else { continue };
            let reply = match req["method"].as_str().unwrap() {
                "tools/call" => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "unknown tool: read_grid"}}),
                _ => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            };
            writeln!(w, "{reply}").unwrap();
        }
    });
    let mut c = DcoClient::connect(dir.path()).unwrap();
    assert_eq!(c.read_grid(&profile()).err().unwrap().code, "dco_too_old");
}

#[test]
fn another_protocol_error_keeps_its_numeric_code_in_the_message() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("dco.sock");
    std::fs::write(dir.path().join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.path().join("token"), TOKEN).unwrap();
    let l = UnixListener::bind(&sock).unwrap();
    let _h = std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return;
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let Some(id) = req.get("id") else { continue };
            let reply = match req["method"].as_str().unwrap() {
                "tools/call" => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": "boom"}}),
                _ => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            };
            writeln!(w, "{reply}").unwrap();
        }
    });
    let mut c = DcoClient::connect(dir.path()).unwrap();
    let e = c.read_grid(&profile()).err().unwrap();
    assert_eq!(e.code, "dco_error");
    assert!(e.message.contains("boom") && e.message.contains("-32603"), "{}", e.message);
}

#[test]
fn a_socket_we_may_not_connect_to_is_blocked_not_down() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("dco.sock");
    let _l = UnixListener::bind(&sock).unwrap();
    std::fs::write(dir.path().join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.path().join("token"), TOKEN).unwrap();
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o000)).unwrap();
    // root 不受文件权限限制：那种环境下连得上，这条没法测。
    if std::fs::OpenOptions::new().read(true).open(&sock).is_ok() || std::os::unix::net::UnixStream::connect(&sock).is_ok() {
        return;
    }
    assert_eq!(DcoClient::connect(dir.path()).err().unwrap().code, "dco_blocked");
}
