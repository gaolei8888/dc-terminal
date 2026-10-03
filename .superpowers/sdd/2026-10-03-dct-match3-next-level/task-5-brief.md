### Task 5: 命令层的集成测试、说明文档

**Files:**
- Modify: `tests/game_cli.rs`、`README.md`、`README.zh-CN.md`

- [ ] **Step 1: 把下面的内容追加到 `tests/game_cli.rs` 末尾**（它给假 dco 加了 `see`、`tap`，按「帧」往前走）

```rust
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
    assert_eq!(tools, vec!["see"]);
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
    assert_eq!(h.join().unwrap().0, vec!["read_grid"]);
}

#[test]
fn auto_next_arguments_are_checked() {
    let home = tempfile::tempdir().unwrap();
    for args in [&["game", "play", "--tries", "0"][..], &["game", "play", "--tries", "21"], &["game", "play", "--auto-next", "--tries"]] {
        let (_, err, code) = dct(home.path(), args);
        assert_eq!(code, 2, "{args:?} {err}");
    }
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test --test game_cli`
Expected: `12 passed`（第一轮 7 个 + 这里 5 个）。

- [ ] **Step 3: 再跑一遍全仓库**

Run: `cargo test --workspace`
Expected: 全绿。

Run: `cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: 通过。

- [ ] **Step 4: 文档**

在 `README.zh-CN.md` 的命令清单附近（跟 `dct send` 同一块）加：

```markdown
### 让 AI 玩三消游戏（Mac）

在 iPhone 镜像里打开 Candy Crush 的一关，然后在 dct 的会话里对 AI 说「帮我玩一关 Candy Crush」。
dct 读棋盘、按规则选步、让 dco 划，每步不到一秒；每一步的棋盘、所有能走的步和选择都记在 `~/.dct/games/log/`。
也可以自己运行：`dct game play [--steps 20] [--dry-run]`。棋盘的位置写在 `~/.dct/games/candy-crush.toml`（没有这个文件就用内置的）。

加上 `--auto-next`，一局没过时它会自己点「Try again」「Play」重来；弹窗只关「Close」「Not now」「No thanks」这类安全的；
遇到要花钱、广告、生命用完、通关、不认识的画面就停下，什么都不点（`--tries` 限制重来的次数，默认 5，每次都会用掉一条生命）。
打赢以后不会自动进下一关：下一关的棋盘位置不一样，要等「自动认新棋盘」做好。
```

`README.md`（英文）加同样内容的英文版，位置相同。

- [ ] **Step 5: 提交**

```bash
git add tests/game_cli.rs README.md README.zh-CN.md
git commit -m "test(game): auto-next against a fake dco that can see text and tap; docs for --auto-next"
```

---

