//! `dct game scene` 对着假 dco 和假大模型跑一遍（假模型是一个会回 JSON 的小脚本，走命令行那条后端）。
//! 不碰真 dco、手机和网关。
#![cfg(unix)]
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::process::Command;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// `private`: see 一律回 private_screen。回每次收到的 (工具名, 参数)。
fn fake_dco(home: &std::path::Path, private: bool) -> std::thread::JoinHandle<Vec<(String, Value)>> {
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
            let result = if req["method"] == "tools/call" {
                let name = req["params"]["name"].as_str().unwrap().to_string();
                let args = req["params"]["arguments"].clone();
                calls.push((name.clone(), args.clone()));
                match name.as_str() {
                    "see" if private => json!({"content": [{"type": "text", "text": json!({"error": {"code": "private_screen", "message": "私人"}}).to_string()}], "isError": true}),
                    "see" => {
                        let body = json!({"snapshot_id": "s1", "window": {"size": {"w": 400.0, "h": 800.0}}, "elements": [{"id": "e1", "text": "5", "role": "text", "source": "ocr"}]});
                        let mut content = vec![json!({"type": "text", "text": body.to_string()})];
                        if args["include_image"] == true {
                            content.push(json!({"type": "image", "data": "iVBORw0K", "mimeType": "image/png"}));
                        }
                        json!({"content": content, "isError": false})
                    }
                    "show_status" => json!({"content": [{"type": "text", "text": "{\"ok\":true}"}], "isError": false}),
                    other => panic!("没想到会调 {other}"),
                }
            } else {
                json!({"protocolVersion": "2025-06-18"})
            };
            writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap();
        }
    })
}

/// 在 home 里装一个会回 JSON 的假命令行模型，并在 config.toml 里指向它。返回它被调用时会创建的标记文件。
fn fake_llm(home: &std::path::Path) -> std::path::PathBuf {
    let dct = home.join(".dct");
    std::fs::create_dir_all(dct.join("profiles")).unwrap();
    let marker = home.join("llm-called");
    let script = home.join("agent.sh");
    std::fs::write(&script, format!("#!/bin/sh\ncat >/dev/null\ntouch '{}'\necho '{{\"name\":\"木箱\",\"x\":0.5,\"y\":0.5,\"why\":\"近\"}}'\n", marker.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(dct.join("profiles/fakeagent.toml"), format!("name = \"fakeagent\"\ncommand = [\"x\"]\n[headless]\ncommand = [\"{}\"]\n", script.display())).unwrap();
    std::fs::write(dct.join("config.toml"), "[llm]\nprovider = \"fakeagent\"\ntransport = \"cli\"\n").unwrap();
    marker
}

fn dct(home: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let o = Command::new(env!("CARGO_BIN_EXE_dct")).args(args).env("HOME", home).stdin(std::process::Stdio::null()).output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into(), o.status.code().unwrap_or(-1))
}

fn steps_file(home: &std::path::Path) -> std::path::PathBuf {
    let game = home.join(".dct/learn/candy-crush");
    let day = std::fs::read_dir(game).unwrap().next().expect("没有日期文件夹").unwrap().path();
    day.join("steps.jsonl")
}

#[test]
fn a_dry_run_asks_the_model_with_a_confirmed_screen_and_taps_nothing() {
    let home = tempfile::tempdir().unwrap();
    let marker = fake_llm(home.path());
    let h = fake_dco(home.path(), false);
    let (out, err, code) = dct(home.path(), &["game", "scene", "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("试走") && out.contains("木箱"), "{out}");
    assert!(marker.exists(), "模型该被问到一次");
    let calls = h.join().unwrap();
    let names: Vec<&str> = calls.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(names, ["see", "see"], "先认字、再取图，没有 tap_at 也没有章鱼：{names:?}");
    assert!(calls[0].1.get("include_image").is_none() && calls[1].1["include_image"] == true);
    let text = std::fs::read_to_string(steps_file(home.path())).unwrap();
    let rec: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!((rec["label"].as_str(), rec["action"]["x_bp"].as_u64()), (Some("dry_run"), Some(5000)));
    assert!(steps_file(home.path()).with_file_name("png/0001.png").exists());
}

#[test]
fn a_private_screen_never_reaches_the_model_or_the_screenshot() {
    let home = tempfile::tempdir().unwrap();
    let marker = fake_llm(home.path());
    let h = fake_dco(home.path(), true);
    let (out, err, code) = dct(home.path(), &["game", "scene"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("私人"), "{out}");
    assert!(!marker.exists(), "私人画面不能问模型");
    let calls = h.join().unwrap();
    let sees: Vec<_> = calls.iter().filter(|c| c.0 == "see").collect();
    assert_eq!(sees.len(), 1, "只该有一次认字：{calls:?}");
    assert!(sees[0].1.get("include_image").is_none());
    assert!(calls.iter().all(|c| c.0 == "see" || c.0 == "show_status"), "{calls:?}");
}

#[test]
fn without_a_model_it_says_so_and_exits_zero_before_touching_dco() {
    let home = tempfile::tempdir().unwrap();
    let (out, err, code) = dct(home.path(), &["game", "scene"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("没开大模型"), "{out}");
    assert!(!home.path().join(".dct/learn").exists());
}

#[test]
fn bad_arguments_exit_2() {
    let home = tempfile::tempdir().unwrap();
    for args in [&["game", "scene", "--steps", "0"][..], &["game", "scene", "--steps", "101"], &["game", "scene", "--nope"]] {
        let (_, err, code) = dct(home.path(), args);
        assert_eq!(code, 2, "{args:?} {err}");
    }
}
