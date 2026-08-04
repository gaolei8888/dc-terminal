## Task 10: 选择器改造

**Files:**
- Modify: `src/ui.rs:66-80`（`View`）、`src/ui.rs:314-318`（`n` 键取列表）、`src/ui.rs:376-405`（`PickProfile` 按键）、`src/ui.rs:920-935`（渲染）、`src/ui.rs:1034`（`idle_help`）
- Test: `src/ui.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 8 的 `ProfileEntry` / `ProfileStatus`
- Produces:
  ```rust
  View::PickProfile { entries: Vec<ProfileEntry>, state: ListState, warning: Option<String> }
  /// 纯函数，好单测：按下第 i 项时该干什么
  pub enum PickAction {
      Start(String),                      // 建会话，profile 名
      AskSecret(usize),                   // 切到填密钥视图，条目下标
      Install { profile: String, command: Vec<String> },
      Blocked(String),                    // 底栏说一句话，不切视图
  }
  pub fn pick_action(e: &ProfileEntry) -> PickAction;
  ```

- [ ] **Step 1: 写失败的测试**

```rust
fn entry(name: &str, status: ProfileStatus) -> ProfileEntry {
    ProfileEntry {
        name: name.into(),
        label: name.into(),
        note: String::new(),
        status,
        secret: None,
        install: None,
    }
}

#[test]
fn ready_entry_starts_a_session() {
    let e = entry("claude", ProfileStatus::Ready);
    assert!(matches!(pick_action(&e), PickAction::Start(n) if n == "claude"));
}

#[test]
fn needs_secret_entry_opens_the_secret_view() {
    let e = entry("kimi", ProfileStatus::NeedsSecret);
    assert!(matches!(pick_action(&e), PickAction::AskSecret(_)));
}

#[test]
fn not_installed_with_an_installer_offers_to_install() {
    let mut e = entry("codex", ProfileStatus::NotInstalled { command: "codex".into() });
    e.install = Some(InstallPrompt {
        command: vec!["npm".into(), "i".into(), "-g".into(), "@openai/codex".into()],
        note: String::new(),
    });
    match pick_action(&e) {
        PickAction::Install { profile, command } => {
            assert_eq!(profile, "codex");
            assert_eq!(command[0], "npm");
        }
        other => panic!("有安装命令就该给一条路，得到 {other:?}"),
    }
}

#[test]
fn not_installed_without_an_installer_just_explains() {
    let e = entry("weird", ProfileStatus::NotInstalled { command: "weird".into() });
    match pick_action(&e) {
        PickAction::Blocked(msg) => {
            assert!(msg.contains("weird"), "要说清是哪个命令找不到：{msg}");
            assert!(!msg.contains("PATH"), "别对非程序员说 PATH");
        }
        other => panic!("得到 {other:?}"),
    }
}

#[test]
fn missing_dependency_names_what_to_install_first() {
    let e = entry("kimi", ProfileStatus::NeedsDependency { label: "Claude".into() });
    match pick_action(&e) {
        PickAction::Blocked(msg) => {
            assert!(msg.contains("Claude"), "要点名先装什么：{msg}");
        }
        other => panic!("得到 {other:?}"),
    }
}

#[test]
fn digit_keys_still_pick_the_first_nine() {
    // 数字保留是因为快；置灰项也占编号——编号跳号比编号漂移更难受
    assert_eq!(digit_index('1'), Some(0));
    assert_eq!(digit_index('9'), Some(8));
    assert_eq!(digit_index('0'), None);
    assert_eq!(digit_index('a'), None);
}

#[test]
fn picker_help_mentions_both_ways_to_choose() {
    let help = idle_help(&View::PickProfile {
        entries: vec![],
        state: ListState::default(),
        warning: None,
    });
    assert!(help.contains("↑↓"));
    assert!(help.contains("数字"));
}

#[test]
fn back_one_level_from_picker_goes_to_board() {
    assert!(matches!(
        back_one_level(View::PickProfile {
            entries: vec![],
            state: ListState::default(),
            warning: None,
        }),
        Some(View::Board)
    ));
}
```

`idle_help` 目前返回 `&'static str`，`PickProfile` 那条改成常量串即可，签名不用动。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib ui`
Expected: FAIL，`pick_action` 不存在

- [ ] **Step 3: 实现**

`View`：

```rust
    PickProfile {
        entries: Vec<ProfileEntry>,
        state: ListState,
        /// 密钥文件读不了、自定义 profile 写错了。顶部红字。
        warning: Option<String>,
    },
```

纯函数（放在 `back_one_level` 附近，和它一样是为了能单测才抽出来的）：

```rust
#[derive(Debug)]
pub enum PickAction {
    Start(String),
    AskSecret(usize),
    Install {
        profile: String,
        command: Vec<String>,
    },
    Blocked(String),
}

/// 按下某一项时该干什么。抽成纯函数是为了能单测——`run()` 的按键循环
/// 要连真 socket，测不了（同 `back_one_level`）。
pub fn pick_action(e: &ProfileEntry) -> PickAction {
    match &e.status {
        ProfileStatus::Ready => PickAction::Start(e.name.clone()),
        ProfileStatus::NeedsSecret => PickAction::AskSecret(0),
        ProfileStatus::NeedsDependency { label } => {
            PickAction::Blocked(format!("要先装 {label} 才能用 {}", e.label))
        }
        ProfileStatus::NotInstalled { command } => match &e.install {
            Some(i) => PickAction::Install {
                profile: e.name.clone(),
                command: i.command.clone(),
            },
            None => PickAction::Blocked(format!("本机没有找到 {command}")),
        },
    }
}

/// '1'..'9' → 0..8。'0' 不算——第 10 项要用 ↑↓ 选。
pub fn digit_index(c: char) -> Option<usize> {
    match c {
        '1'..='9' => Some(c as usize - '1' as usize),
        _ => None,
    }
}
```

`AskSecret(usize)` 的下标由调用方在按键分支里填成实际选中的行号；`pick_action` 里给 0 是占位，调用方一定会覆盖。**这个约定要写在 `PickAction::AskSecret` 的注释里**，否则下一个人会以为它有意义。

按键分支（替换 `src/ui.rs:376-405`）：

```rust
                View::PickProfile {
                    entries,
                    mut state,
                    warning,
                } => {
                    let chosen: Option<usize> = match key.code {
                        KeyCode::Esc => {
                            view = View::Board;
                            None
                        }
                        KeyCode::Down | KeyCode::Up => {
                            let d = if key.code == KeyCode::Down { 1 } else { -1 };
                            move_sel_n(&mut state, entries.len(), d);
                            view = View::PickProfile { entries, state, warning };
                            continue;
                        }
                        KeyCode::Enter => state.selected(),
                        KeyCode::Char(c) => digit_index(c).filter(|i| *i < entries.len()),
                        _ => None,
                    };
                    // ...（下面按 chosen 走 pick_action 的四个分支）
                }
```

四个分支的落点（`AskSecret` 那支在 Task 11 之前先写成 `Blocked("还没做")`，Task 11 再补上真视图）：

```rust
                    match chosen.map(|i| (i, pick_action(&entries[i]))) {
                        None => {}
                        Some((_, PickAction::Start(name))) => {
                            match client.call(Request::Create {
                                dir: current_dir.display().to_string(),
                                profile: name,
                                remember: true,
                            }) {
                                Ok(Response::Created { id }) => {
                                    view = View::Attached(id);
                                    need_sessions = true;
                                }
                                Ok(Response::Error(e)) => {
                                    message = Msg::err(e);
                                    view = View::PickProfile { entries, state, warning };
                                }
                                _ => {
                                    message = Msg::err("创建失败".into());
                                    view = View::PickProfile { entries, state, warning };
                                }
                            }
                        }
                        Some((i, PickAction::AskSecret(_))) => {
                            // pick_action 里那个下标是占位，真下标只有这里知道
                            let e = &entries[i];
                            view = View::EnterSecret {
                                profile: e.name.clone(),
                                label: e.label.clone(),
                                prompt: e.secret.clone().unwrap_or(SecretPrompt {
                                    hint: String::new(),
                                    url: None,
                                }),
                                buf: String::new(),
                                phase: SecretPhase::Typing,
                            };
                        }
                        Some((_, PickAction::Install { profile, command })) => {
                            // 用命令行会话跑安装命令。remember: false ——
                            // 这不是用户选的 agent，记了下次按 n 会掉进命令行。
                            match client.call(Request::Create {
                                dir: current_dir.display().to_string(),
                                profile: "shell".into(),
                                remember: false,
                            }) {
                                Ok(Response::Created { id }) => {
                                    let line = format!("{}\n", command.join(" "));
                                    let _ = client.call(Request::Input { id, text: line });
                                    message = format!("正在安装 {profile}，装完按 Ctrl+Q 回看板再按 N").into();
                                    view = View::Attached(id);
                                    need_sessions = true;
                                }
                                _ => {
                                    message = Msg::err("开不了安装窗口".into());
                                    view = View::PickProfile { entries, state, warning };
                                }
                            }
                        }
                        Some((_, PickAction::Blocked(msg))) => {
                            message = Msg::err(msg);
                            view = View::PickProfile { entries, state, warning };
                        }
                    }
```

`SecretPrompt` 要 `#[derive(Clone)]`（Task 8 已经加了）。

⚠️ `continue` 会跳过循环末尾的 `message_after_transition`。上面 `Down/Up` 那支用 `continue` 是为了少写一遍重建 `View` 的样板，但它同时也跳过了消息清理——`PickProject` 的对应分支没有用 `continue`，是逐支重建 `View` 的。**照 `PickProject` 的写法逐支重建，不要用 `continue`**，否则光标一动消息就永远清不掉。

渲染（替换 `src/ui.rs:920-935`）：

```rust
        View::PickProfile {
            entries,
            state,
            warning,
        } => {
            let items: Vec<ListItem> = entries
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let num = if i < 9 {
                        format!("{}. ", i + 1)
                    } else {
                        "   ".to_string()
                    };
                    let reason = match &e.status {
                        ProfileStatus::Ready => String::new(),
                        ProfileStatus::NeedsSecret => "（未填密钥）".into(),
                        ProfileStatus::NeedsDependency { label } => {
                            format!("（需要先装 {label}）")
                        }
                        ProfileStatus::NotInstalled { .. } => "（未安装）".into(),
                    };
                    // 不可用的整行压暗，不只是把原因压暗——用户是先看名字再看原因的，
                    // 名字亮着会让他先以为能用
                    let base = if matches!(e.status, ProfileStatus::Ready) {
                        Style::default()
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(num, base),
                        Span::styled(format!("{:<14}", truncate(&e.label, 14)), base),
                        Span::styled(
                            format!("{:<26}", truncate(&e.note, 26)),
                            base.fg(Color::DarkGray),
                        ),
                        Span::styled(reason, base.fg(Color::DarkGray)),
                    ]))
                })
                .collect();

            let title = match warning {
                Some(w) => format!("选 agent —— {w}"),
                None => "选 agent".to_string(),
            };
            let border = if warning.is_some() {
                Style::default().fg(Color::Red)
            } else {
                border_style
            };
            let mut s = state.clone();
            f.render_stateful_widget(
                List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(border)
                            .title(title),
                    )
                    .highlight_symbol("▶ "),
                chunks[0],
                &mut s,
            );
        }
```

`idle_help` 的 `PickProfile` 一行改成 `"↑↓ 选  Enter 确认  或直接按数字  Esc 取消"`。

`n` 键取列表处（`src/ui.rs:314-318`）改成解构 `Response::Profiles { entries, warning }` 并建新 `View`，`state.select(Some(0))`。

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 5: 手动看一眼**

```bash
~/.cargo/bin/cargo build && ./target/debug/dct
```
按 `n`，确认九行都在、置灰项带原因、↑↓ 和数字都能选、Esc 能退。

- [ ] **Step 6: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/ui.rs
git commit -m "feat: agent 选择器列出全部九个，置灰项带原因，↑↓ 与数字都能选

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

