## Task 11: 填密钥界面

**Files:**
- Modify: `src/ui.rs`（`View`、按键循环、粘贴分支、渲染、`escape_hint`、`back_one_level`）
- Test: `src/ui.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 9 的 `VerifyOutcome`、Task 10 的 `PickAction::AskSecret`
- Produces:
  ```rust
  View::EnterSecret {
      profile: String,
      label: String,
      prompt: SecretPrompt,
      buf: String,
      phase: SecretPhase,
  }
  pub enum SecretPhase { Typing, Verifying, Failed(String) }
  pub fn clean_secret(s: &str) -> String;
  pub fn verify_message(o: VerifyOutcome) -> Option<String>;  // None = 放行
  ```

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn paste_is_trimmed() {
    assert_eq!(clean_secret("  sk-abc\n"), "sk-abc");
}

#[test]
fn paste_strips_surrounding_quotes() {
    assert_eq!(clean_secret("\"sk-abc\""), "sk-abc");
    assert_eq!(clean_secret("'sk-abc'"), "sk-abc");
}

#[test]
fn paste_strips_bearer_prefix() {
    // 从接口文档里整段拷贝经常带上它
    assert_eq!(clean_secret("Bearer sk-abc"), "sk-abc");
    assert_eq!(clean_secret("\"Bearer sk-abc\"\n"), "sk-abc");
}

#[test]
fn paste_leaves_a_normal_key_alone() {
    assert_eq!(clean_secret("sk-abc123"), "sk-abc123");
}

#[test]
fn bad_key_gets_a_human_message() {
    let m = verify_message(VerifyOutcome::BadKey).unwrap();
    assert!(m.contains("密钥"));
    assert!(!m.contains("401"), "别把状态码甩给用户：{m}");
}

#[test]
fn unreachable_blames_the_network_not_the_key() {
    let m = verify_message(VerifyOutcome::Unreachable).unwrap();
    assert!(m.contains("网络"), "连不上要说是网络，不能让用户去怀疑密钥：{m}");
}

#[test]
fn ok_has_no_message() {
    assert!(verify_message(VerifyOutcome::Ok).is_none());
}

#[test]
fn secret_view_escapes_back_to_the_picker() {
    // 回选择器而不是回看板：用户可能只是选错了 agent
    let back = back_one_level(View::EnterSecret {
        profile: "kimi".into(),
        label: "Kimi".into(),
        prompt: SecretPrompt { hint: String::new(), url: None },
        buf: String::new(),
        phase: SecretPhase::Typing,
    });
    assert!(matches!(back, Some(View::PickProfile { .. })));
}

#[test]
fn secret_view_escape_hint_says_back_to_the_list() {
    let h = escape_hint(&View::EnterSecret {
        profile: "kimi".into(),
        label: "Kimi".into(),
        prompt: SecretPrompt { hint: String::new(), url: None },
        buf: String::new(),
        phase: SecretPhase::Typing,
    });
    assert!(h.contains("列表"), "底栏说什么就得真能做到什么：{h}");
}
```

⚠️ `back_one_level` 返回 `View::PickProfile` 需要一份条目列表，而它是纯函数拿不到。做法：返回 `View::PickProfile { entries: vec![], state: ListState::default(), warning: None }`，主循环在 `Ctrl+Q` 之后发现是空列表就重新拉一次 `Request::Profiles` 填上。这个约定要写进 `back_one_level` 的注释。

`ESCAPE_HINT_COLS` 是写死的 13 列（`src/ui.rs:847`）。「Ctrl+Q 回列表」正好 13 列，新文案别超。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib ui`
Expected: FAIL

- [ ] **Step 3: 实现纯函数**

```rust
/// 粘进来的密钥清洗一遍。用户从网页或接口文档里拷贝，经常带上引号、
/// `Bearer ` 前缀和尾随换行——让他自己发现并删掉是不现实的。
pub fn clean_secret(s: &str) -> String {
    let t = s.trim();
    let t = t.strip_prefix('"').unwrap_or(t);
    let t = t.strip_suffix('"').unwrap_or(t);
    let t = t.strip_prefix('\'').unwrap_or(t);
    let t = t.strip_suffix('\'').unwrap_or(t);
    let t = t.trim();
    t.strip_prefix("Bearer ").unwrap_or(t).trim().to_string()
}

/// 验证结果给用户看的话。`None` 表示放行。
pub fn verify_message(o: VerifyOutcome) -> Option<String> {
    match o {
        VerifyOutcome::Ok => None,
        VerifyOutcome::BadKey => Some("这个密钥用不了，可能是复制的时候少了一段".into()),
        VerifyOutcome::Unreachable => Some("连不上服务器，检查一下网络".into()),
    }
}
```

- [ ] **Step 4: 接上视图与后台验证**

`View` 加：

```rust
    EnterSecret {
        profile: String,
        label: String,
        prompt: SecretPrompt,
        buf: String,
        phase: SecretPhase,
    },
```

```rust
#[derive(Clone)]
pub enum SecretPhase {
    Typing,
    Verifying,
    Failed(String),
}
```

⚠️ **`View` 要 `Clone`（`run()` 里 `match view.clone()`），所以 `mpsc::Receiver` 不能放进 `View`。** 在 `run()` 的局部变量区另起一个：

```rust
    // 密钥验证是网络调用，不能在按键循环里直接跑——会话视图 16ms 一刷，
    // 一次阻塞就是整个界面冻住。丢给后台线程，主循环每轮 try_recv。
    // 放在 View 外面是因为 View 要 Clone，而 Receiver 不能 Clone。
    let mut verify_rx: Option<std::sync::mpsc::Receiver<VerifyOutcome>> = None;
```

按 Enter 时：

```rust
                            let (tx, rx) = std::sync::mpsc::channel();
                            let sock = socket.to_path_buf();
                            let p = profile.clone();
                            let v = buf.clone();
                            std::thread::spawn(move || {
                                // 另开一条连接：主循环那条还要继续画界面
                                let outcome = Client::connect(&sock)
                                    .and_then(|mut c| {
                                        c.call(Request::VerifySecret { profile: p, value: v })
                                    })
                                    .map(|r| match r {
                                        Response::Verify(o) => o,
                                        _ => VerifyOutcome::Unreachable,
                                    })
                                    .unwrap_or(VerifyOutcome::Unreachable);
                                let _ = tx.send(outcome);
                            });
                            verify_rx = Some(rx);
                            phase = SecretPhase::Verifying;
```

主循环开头（`term.draw` 之前）收结果：

```rust
        if let Some(rx) = &verify_rx {
            if let Ok(outcome) = rx.try_recv() {
                verify_rx = None;
                // 通过就存盘 + 开会话 + 进去；不通过就留在原地显示原因
            }
        }
```

`Verifying` 期间不接受输入（Enter / 字符都忽略），只有 Esc 能退——退出时把 `verify_rx` 置 `None`，迟到的结果直接丢掉。

`Ctrl+O` 打开申领页：

```rust
                        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // 用 Ctrl+O 不用 o：o 得留给密钥输入
                            if let Some(url) = &prompt.url {
                                let _ = std::process::Command::new("open").arg(url).spawn();
                            }
                        }
```

粘贴分支（`src/ui.rs:258-275`）加一支：

```rust
                View::EnterSecret { buf, .. } => buf.push_str(&clean_secret(&text)),
```

渲染：

```rust
        View::EnterSecret {
            label,
            prompt,
            buf,
            phase,
            ..
        } => {
            let mut lines: Vec<Line> = Vec::new();
            if !prompt.hint.is_empty() {
                lines.push(Line::from(Span::styled(
                    prompt.hint.clone(),
                    Style::default().fg(Color::DarkGray),
                )));
                lines.push(Line::from(""));
            }
            // 显示成圆点：密钥不该以明文停在屏幕上，用户可能在录屏或在办公室
            lines.push(Line::from(format!("{}▌", "•".repeat(buf.chars().count()))));
            lines.push(Line::from(""));
            match phase {
                SecretPhase::Typing => {}
                SecretPhase::Verifying => lines.push(Line::from(Span::styled(
                    "正在验证…",
                    Style::default().fg(Color::Cyan),
                ))),
                SecretPhase::Failed(m) => lines.push(Line::from(Span::styled(
                    m.clone(),
                    Style::default().fg(Color::Red),
                ))),
            }
            if prompt.url.is_some() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Ctrl+O 打开申领页面",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            f.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style)
                        .title(format!("填 {label} 的密钥（Enter 确认，Esc 返回列表）")),
                ),
                chunks[0],
            );
        }
```

`escape_hint` 加一支：`View::EnterSecret { .. } => "Ctrl+Q 回列表"`。

- [ ] **Step 5: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 6: 手动走一遍**

```bash
~/.cargo/bin/cargo build && ./target/debug/dct
```
`n` → 选 Kimi → 粘一个假 key → 回车。确认「正在验证…」出现、界面**不冻**（这期间还能按 Esc）、最后红字说密钥用不了。

- [ ] **Step 7: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/ui.rs
git commit -m "feat: 就地填密钥，粘贴自动清洗，存盘前后台验证不冻界面

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

