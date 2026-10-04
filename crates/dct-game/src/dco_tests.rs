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
    Profile { window: json!({"app": "iPhone Mirroring"}), region: [0.2, 0.3, 0.5, 0.4], rows: 2, cols: 2, extra: json!({"class_de": 20.0}), fixed_rgb: vec![], match_de: 24.0, weights: crate::choose::Weights::default(), level_pattern: None }
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

/// 2026-10-03 真机通关后那一屏的 `see`（OCR）回复，只留 dct 用到的字段。
#[test]
fn see_text_reads_the_ocr_elements_and_tap_sends_the_snapshot_and_element_ids() {
    let dir = tempfile::tempdir().unwrap();
    let h = fake(dir.path(), TOKEN, |tool, args| match tool {
        "see" => {
            assert_eq!(args["source"], "ocr");
            assert_eq!(args["window"]["app"], "iPhone Mirroring");
            (json!({
                "snapshot_id": "s6", "observation_id": "obs-4adac3d30f33736a", "sources": ["ocr"],
                "elements": [
                    {"id": "e1", "role": "text", "source": "ocr", "text": "Daily Stamps", "bounds": {"x": 1.0, "y": 2.0, "w": 3.0, "h": 4.0}},
                    {"id": "e4", "role": "text", "source": "ocr", "text": "Play", "confidence": 1.0}
                ]
            }), false)
        }
        "tap" => {
            assert_eq!((args["snapshot_id"].as_str(), args["element_id"].as_str()), (Some("s6"), Some("e4")));
            (json!({"tapped": {"id": "e4", "text": "Play"}}), false)
        }
        other => panic!("没想到会调 {other}"),
    });
    let mut c = DcoClient::connect(dir.path()).unwrap();
    let seen = c.see_text(&profile()).unwrap();
    assert_eq!((seen.snapshot_id.as_str(), seen.observation_id.as_deref()), ("s6", Some("obs-4adac3d30f33736a")));
    assert_eq!(seen.elements.iter().map(|e| (e.id.as_str(), e.text.as_str())).collect::<Vec<_>>(), [("e1", "Daily Stamps"), ("e4", "Play")]);
    c.tap(&seen.snapshot_id, "e4").unwrap();
    drop(c);
    assert_eq!(h.join().unwrap().iter().filter(|m| *m == "tools/call").count(), 2);
}

#[test]
fn a_refused_tap_keeps_dcos_reason() {
    let dir = tempfile::tempdir().unwrap();
    let _h = fake(dir.path(), TOKEN, |_, _| (json!({"error": {"code": "needs_ticket", "message": "这个动作属于「付款」档"}}), true));
    let mut c = DcoClient::connect(dir.path()).unwrap();
    let e = c.tap("s1", "e2").unwrap_err();
    assert_eq!(e.code, "needs_ticket");
}

/// 假 dco：每个 show_status 之外的调用都正常回；show_status 按 `status_reply` 回（None=正常 {"ok":true}，
/// Some((code, message))=JSON-RPC 协议错误）。返回收到的每个 (工具名, 参数)。
fn fake_status(dir: &std::path::Path, status_reply: Option<(i64, &'static str)>) -> std::thread::JoinHandle<Vec<(String, Value)>> {
    let sock = dir.join("dco.sock");
    std::fs::write(dir.join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.join("token"), TOKEN).unwrap();
    let l = UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let mut calls = vec![];
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return calls;
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let Some(id) = req.get("id") else { continue };
            let reply = if req["method"] == "tools/call" {
                let name = req["params"]["name"].as_str().unwrap().to_string();
                calls.push((name.clone(), req["params"]["arguments"].clone()));
                let body = match name.as_str() {
                    "show_status" => {
                        if let Some((code, message)) = status_reply {
                            writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})).unwrap();
                            continue;
                        }
                        json!({"ok": true})
                    }
                    "read_grid" => json!({"rows":2,"cols":2,"cells":[[0,0],[1,1]],"odd":[[false,false],[false,false]],
                        "classes":[{"id":0,"rgb":[1,2,3],"count":2},{"id":1,"rgb":[4,5,6],"count":2}],"elapsed_ms":1}),
                    _ => json!({"swiped": true}),
                };
                json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": body.to_string()}], "isError": false}})
            } else {
                json!({"jsonrpc": "2.0", "id": id, "result": {}})
            };
            writeln!(w, "{reply}").unwrap();
        }
    })
}

fn status_calls(calls: &[(String, Value)]) -> Vec<&Value> {
    calls.iter().filter(|(n, _)| n == "show_status").map(|(_, a)| a).collect()
}

#[test]
fn show_status_sends_exactly_the_state() {
    let dir = tempfile::tempdir().unwrap();
    let h = fake_status(dir.path(), None);
    let mut c = DcoClient::connect(dir.path()).unwrap();
    c.show_status("think");
    c.show_status("look");
    drop(c);
    let calls = h.join().unwrap();
    assert_eq!(status_calls(&calls), [&json!({"state": "think"}), &json!({"state": "look"})]);
}

#[test]
fn an_old_dco_without_show_status_is_ignored_and_never_asked_again() {
    let dir = tempfile::tempdir().unwrap();
    let h = fake_status(dir.path(), Some((-32602, "unknown tool: show_status")));
    let mut c = DcoClient::connect(dir.path()).unwrap();
    c.show_status("think");
    c.show_status("look");
    c.read_grid(&profile()).unwrap();
    c.swipe(&profile(), (0.3, 0.4), (0.4, 0.5)).unwrap();
    drop(c);
    let calls = h.join().unwrap();
    assert_eq!(status_calls(&calls).len(), 1, "第二次不该再发");
    assert_eq!(calls.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["show_status", "read_grid", "swipe"]);
}

#[test]
fn another_show_status_error_does_not_stop_later_calls() {
    let dir = tempfile::tempdir().unwrap();
    let h = fake_status(dir.path(), Some((-32603, "boom")));
    let mut c = DcoClient::connect(dir.path()).unwrap();
    c.show_status("think");
    c.show_status("look");
    c.read_grid(&profile()).unwrap();
    drop(c);
    assert_eq!(status_calls(&h.join().unwrap()).len(), 2);
}

#[test]
fn a_show_status_timeout_is_not_paid_twice() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("dco.sock");
    std::fs::write(dir.path().join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.path().join("token"), TOKEN).unwrap();
    let l = UnixListener::bind(&sock).unwrap();
    let h = std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        let mut tool_calls = 0;
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return tool_calls;
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let Some(id) = req.get("id") else { continue };
            if req["method"] == "tools/call" {
                tool_calls += 1; // 不回话：让客户端超时
                continue;
            }
            writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "result": {}})).unwrap();
        }
    });
    let mut c = DcoClient::connect_with_timeout(dir.path(), std::time::Duration::from_millis(200)).unwrap();
    c.show_status("think");
    let t = std::time::Instant::now();
    c.show_status("look");
    assert!(t.elapsed() < std::time::Duration::from_millis(100));
    drop(c);
    assert_eq!(h.join().unwrap(), 1);
}

#[test]
fn a_see_text_timeout_is_not_paid_twice() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("dco.sock");
    std::fs::write(dir.path().join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.path().join("token"), TOKEN).unwrap();
    let l = UnixListener::bind(&sock).unwrap();
    let h = std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        let mut tool_calls = 0;
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return tool_calls;
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let Some(id) = req.get("id") else { continue };
            if req["method"] == "tools/call" {
                tool_calls += 1; // 不回话：让客户端超时
                continue;
            }
            writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "result": {}})).unwrap();
        }
    });
    let mut c = DcoClient::connect_with_timeout(dir.path(), std::time::Duration::from_millis(200)).unwrap();
    assert_eq!(c.see_text(&profile()).err().unwrap().code, "dco_timeout");
    let t = std::time::Instant::now();
    assert_eq!(c.see_text(&profile()).err().unwrap().code, "unsupported");
    assert!(t.elapsed() < std::time::Duration::from_millis(100));
    drop(c);
    assert_eq!(h.join().unwrap(), 1);
}

#[test]
fn a_late_reply_to_a_timed_out_call_is_not_taken_for_the_next_calls_reply() {
    let dir = tempfile::tempdir().unwrap();
    let _h = fake(dir.path(), TOKEN, |tool, _| match tool {
        "slow" => {
            std::thread::sleep(std::time::Duration::from_millis(300));
            (json!({}), false)
        }
        "read_grid" => (json!({"rows":2,"cols":2,"cells":[[0,0],[1,1]],"odd":[[false,false],[false,false]],
                "classes":[{"id":0,"rgb":[1,2,3],"count":2},{"id":1,"rgb":[4,5,6],"count":2}],"elapsed_ms":1}), false),
        other => panic!("没想到会调 {other}"),
    });
    let mut c = DcoClient::connect_with_timeout(dir.path(), std::time::Duration::from_millis(200)).unwrap();
    assert_eq!(c.call("slow", json!({})).unwrap_err().code, "dco_timeout");
    // 慢回复在这次调用等的时候才到，必须被跳过
    let g = c.read_grid(&profile()).unwrap();
    assert_eq!((g.rows, g.cols, g.classes.len()), (2, 2, 2));
}
