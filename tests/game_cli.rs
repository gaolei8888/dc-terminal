//! `dct game play` 对着一个假 dco 跑一遍：握手、读棋盘、选步、记录、退出码。假 dco 按 dco 真实的
//! 握手和回复形状说话（见 dc-octo `src/client.rs`、`src/mcp.rs`）。
#![cfg(unix)]
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::Command;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// 9 行 5 列，每个类别不止一格；(0,2)↔(1,2) 能换出第 0 行的四连。
fn board() -> Value {
    let mut cells: Vec<Vec<u64>> = (0..9).map(|r| (0..5).map(|c| ((r + c) % 3) as u64).collect()).collect();
    cells[0] = vec![0, 0, 1, 0, 2];
    cells[1] = vec![1, 2, 0, 1, 0];
    let mut counts = [0usize; 3];
    cells.iter().flatten().for_each(|&c| counts[c as usize] += 1);
    json!({
        "rows": 9, "cols": 5, "cells": cells, "odd": vec![vec![false; 5]; 9],
        "classes": (0..3).map(|i| json!({"id": i, "rgb": [i, i, i], "count": counts[i as usize]})).collect::<Vec<_>>(),
        "observation_id": "obs-0000000000000001", "observed_at_ms": 1790942400000u64, "frame_age_ms": 5, "elapsed_ms": 1
    })
}

fn fake_dco(home: &std::path::Path, swipe_error: Option<&'static str>) -> std::thread::JoinHandle<Vec<String>> {
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
        let mut tools = vec![];
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(line.trim()).unwrap()["dco_token"], TOKEN);
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return tools;
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let Some(id) = req.get("id") else { continue };
            let result = match req["method"].as_str().unwrap() {
                "tools/call" => {
                    let name = req["params"]["name"].as_str().unwrap().to_string();
                    tools.push(name.clone());
                    let (body, is_error) = match (name.as_str(), swipe_error) {
                        ("read_grid", _) => (board(), false),
                        ("swipe", Some(code)) => (json!({"error": {"code": code, "message": "x"}}), true),
                        ("swipe", None) => (json!({"swiped": true}), false),
                        ("show_status", _) => (json!({"ok": true}), false),
                        (other, _) => panic!("没想到会调 {other}"),
                    };
                    json!({"content": [{"type": "text", "text": body.to_string()}], "isError": is_error})
                }
                _ => json!({"protocolVersion": "2025-06-18"}),
            };
            writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap();
        }
    })
}

/// 状态章鱼（show_status）是 dct 顺带发的，跟这些测试要查的“划了什么、读了什么”无关，比较前先滤掉。
fn without_status(tools: Vec<String>) -> Vec<String> {
    tools.into_iter().filter(|t| t != "show_status").collect()
}

fn dct(home: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let o = Command::new(env!("CARGO_BIN_EXE_dct")).args(args).env("HOME", home).stdin(std::process::Stdio::null()).output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into(), o.status.code().unwrap_or(-1))
}

fn log_lines(home: &std::path::Path) -> Vec<Value> {
    let dir = home.join(".dct/games/log");
    let f = std::fs::read_dir(&dir).unwrap().next().expect("没有记录文件").unwrap().path();
    std::fs::read_to_string(f).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[test]
fn a_dry_run_prints_the_move_never_swipes_and_logs_it() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco(home.path(), None);
    let (out, err, code) = dct(home.path(), &["game", "play", "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("第 1 步：") && out.contains("试走，没有真划"), "{out}");
    assert!(out.contains("试走结束"), "{out}");
    let tools = h.join().unwrap();
    // 先读盘，再“想”“看”各一次（试走也发）
    assert_eq!(tools, ["read_grid", "show_status", "show_status"]);
    assert_eq!(without_status(tools), vec!["read_grid"], "试走不该划");
    let lines = log_lines(home.path());
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["game"], "candy-crush");
    assert_eq!(lines[0]["profile_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(lines[0]["observation_id"], "obs-0000000000000001");
    assert_eq!(lines[0]["outcome"], "dry_run");
    assert_eq!((lines[1]["stop"].as_str(), lines[1]["steps"].as_u64()), (Some("dry_run"), Some(0)));
}

#[test]
fn a_halted_dco_stops_the_game_with_a_plain_sentence_and_a_failing_exit_code() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco(home.path(), Some("halted"));
    let (out, err, code) = dct(home.path(), &["game", "play", "--steps", "3"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("急停"), "{err}");
    assert_eq!(without_status(h.join().unwrap()), vec!["read_grid", "swipe"]);
    assert!(err.contains("走了 0 步"), "{err}");
    assert!(out.contains("这一步没划成") && !out.contains("消 ") && !out.contains("第 1 步"), "{out}");
    let lines = log_lines(home.path());
    assert_eq!(lines.last().unwrap()["stop"], "dco");
    assert_eq!(lines.last().unwrap()["steps"], 0);
    assert_eq!(lines[0]["swiped"], false);
    assert!(lines[0]["candidates"][0]["features"]["cleared"].as_u64().is_some());
    let run = lines[0]["run_id"].as_str().unwrap();
    assert_eq!(run.len(), 8);
    assert!(lines.iter().all(|l| l["run_id"] == run), "同一次运行的记录要带同一个 run_id");
    assert!(lines.last().unwrap()["time_ms"].as_u64().is_some());
}

#[test]
fn without_dco_it_says_so_and_does_not_create_a_log() {
    let home = tempfile::tempdir().unwrap();
    let (out, err, code) = dct(home.path(), &["game", "play"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("dco 没在运行"), "{err}");
    assert!(!home.path().join(".dct/games/log").exists());
}

#[test]
fn bad_arguments_exit_2_and_show_usage() {
    let home = tempfile::tempdir().unwrap();
    for args in [&["game", "play", "--steps", "0"][..], &["game"], &["game", "play", "--nope"]] {
        let (_, err, code) = dct(home.path(), args);
        assert_eq!(code, 2, "{args:?} {err}");
        assert!(!err.is_empty());
    }
}

#[test]
fn a_broken_profile_file_names_itself() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".dct/games")).unwrap();
    std::fs::write(home.path().join(".dct/games/candy-crush.toml"), "rows = \"x\"").unwrap();
    let (_, err, code) = dct(home.path(), &["game", "play", "--dry-run"]);
    assert_eq!(code, 1);
    assert!(err.contains("candy-crush.toml"), "{err}");
}

#[test]
fn help_lists_the_game_command() {
    let home = tempfile::tempdir().unwrap();
    let (out, _, _) = dct(home.path(), &["--help"]);
    assert!(out.contains("dct game play"), "{out}");
}

#[test]
fn an_unwritable_log_refuses_to_start_with_a_plain_sentence() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco(home.path(), None);
    // 今天的记录文件该在的地方放一个文件夹
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let (y, m, d) = civil(secs / 86_400);
    std::fs::create_dir_all(home.path().join(format!(".dct/games/log/{y:04}-{m:02}-{d:02}.jsonl"))).unwrap();
    let (_, err, code) = dct(home.path(), &["game", "play", "--dry-run"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("记录文件开不了") && err.contains(".jsonl"), "{err}");
    drop(h);
}

/// 与 `journal::civil_from_days` 同一个算法（Hinnant），测试里自备一份。
fn civil(z: u64) -> (i64, u32, u32) {
    let z = z as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

// ---- `--auto-next`：假 dco 按“帧”往前走，点一下换下一帧 ----

enum Frame {
    Text(Vec<&'static str>),
    Board,
}

/// 假 dco：除了 read_grid / swipe，还会 see（OCR）和 tap。点或划一下就换到下一帧，最后一帧停在那里。
/// 返回它收到的工具调用名和 tap 的元素文字（按先后）。
fn fake_dco_world(home: &std::path::Path, frames: Vec<Frame>) -> std::thread::JoinHandle<(Vec<String>, Vec<String>)> {
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
        let (mut tools, mut taps, mut at) = (vec![], vec![], 0usize);
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return (tools, taps);
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let Some(id) = req.get("id") else { continue };
            let result = match req["method"].as_str().unwrap() {
                "tools/call" => {
                    let name = req["params"]["name"].as_str().unwrap().to_string();
                    tools.push(name.clone());
                    let args = &req["params"]["arguments"];
                    let (body, is_error) = match name.as_str() {
                        "see" => {
                            let texts: Vec<&str> = match &frames[at] {
                                Frame::Text(t) => t.clone(),
                                Frame::Board => vec!["38", "90"],
                            };
                            let elements: Vec<Value> = texts.iter().enumerate().map(|(i, t)| json!({"id": format!("e{}", i + 1), "text": t})).collect();
                            (json!({"snapshot_id": format!("s{at}"), "observation_id": format!("obs-{at}"), "elements": elements}), false)
                        }
                        "tap" => {
                            assert_eq!(args["snapshot_id"], format!("s{at}"));
                            let Frame::Text(t) = &frames[at] else { panic!("棋盘上不该点按钮") };
                            let n: usize = args["element_id"].as_str().unwrap().trim_start_matches('e').parse().unwrap();
                            taps.push(t[n - 1].to_string());
                            if at + 1 < frames.len() {
                                at += 1;
                            }
                            (json!({"tapped": true}), false)
                        }
                        "read_grid" => match &frames[at] {
                            Frame::Board => (board(), false),
                            Frame::Text(_) => (json!({"error": {"code": "not_a_grid", "message": "这块区域分不清类别"}}), true),
                        },
                        "show_status" => (json!({"ok": true}), false),
                        other => panic!("没想到会调 {other}"),
                    };
                    json!({"content": [{"type": "text", "text": body.to_string()}], "isError": is_error})
                }
                _ => json!({"protocolVersion": "2025-06-18"}),
            };
            writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap();
        }
    })
}

fn nav_records(home: &std::path::Path) -> Vec<Value> {
    log_lines(home).into_iter().filter(|r| r["kind"] == "nav").collect()
}

#[test]
fn auto_next_dry_run_says_what_it_would_press_and_presses_nothing() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco_world(home.path(), vec![Frame::Text(vec!["Daily Stamps", "Play"]), Frame::Board]);
    let (out, err, code) = dct(home.path(), &["game", "play", "--auto-next", "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("画面上有「Play」，会点它"), "{out}");
    let (tools, taps) = h.join().unwrap();
    assert_eq!(without_status(tools), vec!["see"]);
    assert!(taps.is_empty());
    let nav = nav_records(home.path());
    assert_eq!((nav[0]["screen"].as_str(), nav[0]["outcome"].as_str()), (Some("play"), Some("dry_run")));
    assert_eq!(nav[0]["texts"], json!(["Daily Stamps", "Play"]));
}

#[test]
fn auto_next_stops_on_a_price_without_pressing_anything() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco_world(home.path(), vec![Frame::Text(vec!["Out of moves", "Continue for 💎 5"])]);
    let (out, err, code) = dct(home.path(), &["game", "play", "--auto-next"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("要花钱") && err.contains("没有点任何东西"), "{err}");
    assert!(h.join().unwrap().1.is_empty());
    let lines = log_lines(home.path());
    assert_eq!(lines.last().unwrap()["stop"], "money");
    assert!(lines.iter().all(|l| l["run_id"].is_string()));
}

#[test]
fn auto_next_presses_a_safe_button_then_stops_on_a_screen_it_does_not_know_and_records_its_words() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco_world(home.path(), vec![Frame::Text(vec!["Music Season", "Not now"]), Frame::Text(vec!["Collect your daily treat!", "Claim"])]);
    let (out, err, code) = dct(home.path(), &["game", "play", "--auto-next"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("关掉了一个弹窗（Not now）"), "{out}");
    assert!(err.contains("不认识的画面") && err.contains("Collect your daily treat! / Claim"), "{err}");
    assert_eq!(h.join().unwrap().1, vec!["Not now"]);
    let nav = nav_records(home.path());
    assert_eq!(nav.len(), 2);
    assert_eq!((nav[0]["screen"].as_str(), nav[0]["tapped"].as_str(), nav[0]["outcome"].as_str()), (Some("dismiss"), Some("Not now"), Some("changed")));
    assert_eq!((nav[1]["screen"].as_str(), nav[1]["outcome"].as_str()), (Some("unknown"), Some("stopped")));
    assert_eq!(nav[1]["texts"], json!(["Collect your daily treat!", "Claim"]));
    assert_eq!(log_lines(home.path()).last().unwrap()["stop"], "unknown_screen");
}

#[test]
fn without_auto_next_the_screen_is_never_read_as_text() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco_world(home.path(), vec![Frame::Board]);
    let (out, err, code) = dct(home.path(), &["game", "play", "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(without_status(h.join().unwrap().0), vec!["read_grid"]);
}

#[test]
fn auto_next_arguments_are_checked() {
    let home = tempfile::tempdir().unwrap();
    for args in [&["game", "play", "--tries", "0"][..], &["game", "play", "--tries", "21"], &["game", "play", "--auto-next", "--tries"]] {
        let (_, err, code) = dct(home.path(), args);
        assert_eq!(code, 2, "{args:?} {err}");
    }
}
