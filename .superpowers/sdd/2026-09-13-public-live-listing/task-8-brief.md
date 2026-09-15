### Task 8: 界面 —— 直播面板 `p`/`K` 与底栏公开提示

**Files:**
- Modify: `src/ui/view.rs`（`View::Live` 加 `input`、`LiveInput`、按键表与逃生提示）
- Modify: `src/ui/live.rs`（按键、输入行、横幅文案与样式）
- Modify: `src/ui/mod.rs`（`View::Live` 的模式匹配、`bar_live_style`、实色底栏守卫加一屏）
- Modify: `src/ui/app.rs`（`last_public_title: String`）

**Interfaces:**
- Consumes: Task 6 的 `LivePublic`、新请求、`ErrorCode::LivePublishKeyMissing`、文案键；`secrets::LIVE_PUBLISH_KEY`。
- Produces:
  - `pub enum LiveInput { Title(String), Key { buf: String, then_publish: Option<String> } }`（`Clone, Debug, PartialEq`）
  - `View::Live { state: ListState, input: Option<LiveInput> }`

- [ ] **Step 1: 写失败的测试**

`src/ui/live.rs` 的 `mod tests` 里已有 `fake_live_daemon(answer)`（2026-09-13 的轮询测试用它）。给它加第二个参数 `has_key: std::sync::Arc<std::sync::atomic::AtomicBool>`，线程闭包里 `let has_key2 = has_key.clone();` 带进去，并把 `let resp = match req { ... }` 换成：

```rust
                    let resp = match req {
                        Request::LiveStatus => {
                            asked2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            Response::Live(answer.lock().unwrap().clone())
                        }
                        Request::LivePublish { title } => {
                            if has_key2.load(std::sync::atomic::Ordering::SeqCst) {
                                let mut info = answer.lock().unwrap().clone();
                                info.public = LivePublic::Pending { title };
                                Response::Live(info)
                            } else {
                                Response::Error(crate::proto::ErrorCode::LivePublishKeyMissing)
                            }
                        }
                        Request::SetSecret { profile, .. } if profile == crate::secrets::LIVE_PUBLISH_KEY => {
                            has_key2.store(true, std::sync::atomic::Ordering::SeqCst);
                            Response::Ok
                        }
                        Request::LiveUnpublish => {
                            let mut info = answer.lock().unwrap().clone();
                            info.public = LivePublic::Private;
                            Response::Live(info)
                        }
                        _ => Response::Ok,
                    };
```

既有两条调用它的测试补第二个实参 `std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))`。然后追加测试：

```rust
    fn press_live(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)).unwrap();
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            press_live(app, KeyCode::Char(c));
        }
    }

    /// 在播时按 `p` → 填标题 → Enter → 没密钥就转到填密钥 → Enter → 自动继续公开。
    #[test]
    fn publishing_asks_for_a_title_then_a_key_when_missing_then_continues() {
        let has_key = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let answer = Arc::new(Mutex::new(info(vec![(1, "一".into())], 0, LiveReadiness::Ready)));
        let (sock, _fake, _asked) = fake_live_daemon(answer, has_key.clone());
        let (mut app, _dir) = App::test_app();
        app.client = Some(crate::client::Client::connect(&sock).unwrap());
        app.connected = true;
        app.live = info(vec![(1, "一".into())], 0, LiveReadiness::Ready);
        app.view = View::Live { state: ListState::default(), input: None };

        press_live(&mut app, KeyCode::Char('p'));
        assert!(matches!(&app.view, View::Live { input: Some(LiveInput::Title(_)), .. }));
        type_text(&mut app, "第3课");
        press_live(&mut app, KeyCode::Enter);
        assert!(
            matches!(&app.view, View::Live { input: Some(LiveInput::Key { then_publish: Some(t), .. }), .. } if t == "第3课"),
            "没密钥要转去填密钥，并记着要继续公开"
        );
        type_text(&mut app, "the-key");
        press_live(&mut app, KeyCode::Enter);
        assert!(has_key.load(std::sync::atomic::Ordering::SeqCst), "密钥要存进守护进程");
        assert!(matches!(app.live.public, LivePublic::Pending { .. }), "存完密钥要自动继续公开");
        assert!(matches!(&app.view, View::Live { input: None, .. }));
        assert_eq!(app.last_public_title, "第3课");
    }

    /// 密钥输入不许出现在屏幕上。
    #[test]
    fn the_key_being_typed_is_never_drawn() {
        let (mut app, _dir) = App::test_app();
        app.live = info(vec![(1, "一".into())], 0, LiveReadiness::Ready);
        app.view = View::Live {
            state: ListState::default(),
            input: Some(LiveInput::Key { buf: "SUPER-SECRET".into(), then_publish: None }),
        };
        let screen = screen_of(&mut app, 100, 40);
        assert!(!screen.contains("SUPER-SECRET"), "{screen}");
    }

    #[test]
    fn an_empty_title_is_refused_on_the_spot() {
        let (mut app, _dir) = App::test_app();
        app.live = info(vec![(1, "一".into())], 0, LiveReadiness::Ready);
        app.view = View::Live { state: ListState::default(), input: Some(LiveInput::Title(String::new())) };
        press_live(&mut app, KeyCode::Enter);
        assert!(app.message.error, "空标题要当场说");
        assert!(matches!(&app.view, View::Live { input: Some(LiveInput::Title(_)), .. }), "输入行不关");
    }

    /// 公开中按 `p` 是取消公开，不再弹标题。
    #[test]
    fn p_on_a_public_live_unpublishes() {
        let has_key = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let answer = Arc::new(Mutex::new(info(vec![(1, "一".into())], 0, LiveReadiness::Ready)));
        let (sock, _fake, _asked) = fake_live_daemon(answer, has_key);
        let (mut app, _dir) = App::test_app();
        app.client = Some(crate::client::Client::connect(&sock).unwrap());
        app.connected = true;
        let mut live = info(vec![(1, "一".into())], 0, LiveReadiness::Ready);
        live.public = LivePublic::Listed { title: "课".into() };
        app.live = live;
        app.view = View::Live { state: ListState::default(), input: None };
        press_live(&mut app, KeyCode::Char('p'));
        assert_eq!(app.live.public, LivePublic::Private);
        assert!(matches!(&app.view, View::Live { input: None, .. }));
    }

    #[test]
    fn the_banner_says_public_with_the_title() {
        let mut live = info(vec![(1, "一".into())], 7, LiveReadiness::Ready);
        live.public = LivePublic::Listed { title: "第3课".into() };
        let s = live_banner(&live, Lang::Zh);
        assert!(s.contains("正在公开直播") && s.contains("第3课") && s.contains('7'), "{s}");
        live.public = LivePublic::Failed { title: "第3课".into(), reason: LiveFailure::Refused(401) };
        assert!(live_banner(&live, Lang::Zh).contains("吊销"));
    }
```

`src/ui/mod.rs` 的 `nothing_on_a_solid_bar_takes_its_color_from_the_terminal_theme`：`cases` 数组长度 +2，加：

```rust
            ("正在公开直播", || {
                let (mut app, dir) = live(crate::proto::LiveReadiness::Ready);
                app.live.public = crate::proto::LivePublic::Listed { title: "第3课".into() };
                (app, dir)
            }),
            ("公开失败", || {
                let (mut app, dir) = live(crate::proto::LiveReadiness::Ready);
                app.live.public = crate::proto::LivePublic::Failed {
                    title: "第3课".into(),
                    reason: crate::proto::LiveFailure::Refused(401),
                };
                (app, dir)
            }),
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib ui::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

1. `src/ui/view.rs`：

```rust
/// 直播面板里正在填的那一行。
#[derive(Clone, Debug, PartialEq)]
pub enum LiveInput {
    /// 公开标题。
    Title(String),
    /// 公开直播密钥。`then_publish` = 填完之后继续用这个标题公开
    /// （按 `p` 时守护进程报 `LivePublishKeyMissing` 转过来的）。
    Key { buf: String, then_publish: Option<String> },
}
```

`View::Live { state: ListState }` 改成 `View::Live { state: ListState, input: Option<LiveInput> }`。全仓 `View::Live { state }` 的解构改成 `View::Live { state, .. }` 或按需取 `input`，构造处补 `input: None`（`grep -rn "View::Live" src` 列全）。
按键表（`help_of` 里 `View::Live` 那一支）：`input` 为 `Some` 时只给 `Enter 确认` / `Esc 取消`；否则在播时追加 `("p", Key::LivePublishToggle)` 与 `("K", Key::LiveChangeKey)`。逃生提示：`input` 为 `Some` 时是 `Esc 取消`。
2. `src/ui/app.rs`：`App` 加 `pub last_public_title: String,`，初始化为空串。
3. `src/ui/live.rs`：
- `handle_key` 开头改成解构 `let View::Live { mut state, input } = app.view.clone() else { return Ok(()) };`，并在处理普通按键之前：

```rust
    if let Some(field) = input {
        let next = edit_live_input(app, field, key);
        app.view = View::Live { state, input: next };
        return Ok(());
    }
```

- 普通按键里加：

```rust
        KeyCode::Char('p') if is_plain_key(&key) && is_live(&app.live) => {
            if matches!(app.live.public, LivePublic::Private) {
                app.view = View::Live { state, input: Some(LiveInput::Title(app.last_public_title.clone())) };
                return Ok(());
            }
            unpublish(app);
        }
        KeyCode::Char('K') if is_plain_key(&key) => {
            app.view = View::Live { state, input: Some(LiveInput::Key { buf: String::new(), then_publish: None }) };
            return Ok(());
        }
```

- 新函数：

```rust
/// 输入行的一次按键。回下一步的输入状态（`None` = 关掉输入行）。
fn edit_live_input(app: &mut App, field: LiveInput, key: KeyEvent) -> Option<LiveInput> {
    let buf_of = |f: &LiveInput| match f {
        LiveInput::Title(b) => b.clone(),
        LiveInput::Key { buf, .. } => buf.clone(),
    };
    let with_buf = |f: LiveInput, b: String| match f {
        LiveInput::Title(_) => LiveInput::Title(b),
        LiveInput::Key { then_publish, .. } => LiveInput::Key { buf: b, then_publish },
    };
    match key.code {
        KeyCode::Esc => None,
        KeyCode::Backspace => {
            let mut b = buf_of(&field);
            b.pop();
            Some(with_buf(field, b))
        }
        KeyCode::Char(c) if !key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            let mut b = buf_of(&field);
            b.push(c);
            Some(with_buf(field, b))
        }
        KeyCode::Enter => match field {
            LiveInput::Title(t) => {
                let t = t.trim().to_string();
                if t.is_empty() {
                    app.message = Msg::err(text(Key::LiveTitleEmpty, app.lang).into());
                    return Some(LiveInput::Title(t));
                }
                publish(app, t)
            }
            LiveInput::Key { buf, then_publish } => {
                let saved = app.client().and_then(|c| {
                    c.call(Request::SetSecret { profile: crate::secrets::LIVE_PUBLISH_KEY.into(), value: buf })
                });
                match saved {
                    Ok(Response::Ok) => {
                        app.message = text(Key::LiveKeySaved, app.lang).into();
                        match then_publish {
                            Some(t) => publish(app, t),
                            None => None,
                        }
                    }
                    _ => {
                        app.message = Msg::err(text(Key::RequestFailed, app.lang).into());
                        None
                    }
                }
            }
        },
        _ => Some(field),
    }
}

/// 发 `LivePublish`。没密钥就转去填密钥，并记着标题。
fn publish(app: &mut App, title: String) -> Option<LiveInput> {
    match app.client().and_then(|c| c.call(Request::LivePublish { title: title.clone() })) {
        Ok(Response::Live(info)) => {
            app.live = info;
            app.last_public_title = title;
            None
        }
        Ok(Response::Error(ErrorCode::LivePublishKeyMissing)) => {
            Some(LiveInput::Key { buf: String::new(), then_publish: Some(title) })
        }
        Ok(Response::Error(e)) => {
            app.message = Msg::err(msg::error(app.lang, &e));
            None
        }
        _ => {
            app.message = Msg::err(text(Key::RequestFailed, app.lang).into());
            None
        }
    }
}

fn unpublish(app: &mut App) {
    match app.client().and_then(|c| c.call(Request::LiveUnpublish)) {
        Ok(Response::Live(info)) => {
            app.live = info;
            app.message = text(Key::LiveUnpublished, app.lang).into();
        }
        _ => app.message = Msg::err(text(Key::RequestFailed, app.lang).into()),
    }
}
```

- `live_banner` 改成：

```rust
pub(crate) fn live_banner(info: &LiveInfo, lang: Lang) -> String {
    let base = match &info.public {
        LivePublic::Listed { title } | LivePublic::Pending { title } | LivePublic::Failed { title, .. } => {
            msg::live_on_air_public(lang, title, info.staged.len(), info.viewers)
        }
        LivePublic::Private => msg::live_on_air(lang, info.staged.len(), info.viewers),
    };
    let base = match &info.public {
        LivePublic::Pending { .. } => format!("{base} · {}", text(Key::LivePublicPending, lang)),
        LivePublic::Failed { reason, .. } => format!("{base} · {}", msg::live_publish_failed(lang, reason)),
        _ => base,
    };
    match &info.readiness {
        LiveReadiness::Ready => base,
        LiveReadiness::Pending => format!("{base} · {}", text(Key::LiveConnectingToRelay, lang)),
        LiveReadiness::Failed(why) => format!("{base} · {}", msg::live_start_failed(lang, why)),
    }
}
```

- `banner_style`：`LivePublic::Failed { .. }` 时返回 `danger()`（面板内），其余照旧。
- `draw`：在 `lines` 开头（`is_live` 分支里横幅之后）若 `input` 为 `Some`：`Title(b)` 画 `format!("{}{}▏", text(Key::LiveTitlePrompt, lang), b)`；`Key { buf, .. }` 画 `format!("{}{}▏", text(Key::LiveKeyPrompt, lang), "•".repeat(buf.chars().count()))`——**绝不画 `buf` 本身**。
4. `src/ui/mod.rs`：`bar_live_style` 在实色档下，`LivePublic::Failed { .. }` 返回 `bar_danger(t)`，其余按原 `readiness` 规则。`View::Live` 相关匹配补 `..`。

- [ ] **Step 4: 跑测试确认通过**

Run: `env -u TERM cargo test --workspace --no-fail-fast && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: PASS。

- [ ] **Step 5: 变异检查**

`draw` 里把圆点改回 `buf`（`the_key_being_typed_is_never_drawn` 红）；`publish` 的 `LivePublishKeyMissing` 分支改成 `None`（`publishing_asks_for_a_title...` 红）；`bar_live_style` 的 `Failed` 改成 `danger()`（实色底栏守卫红）。改回。

- [ ] **Step 6: Commit**

```bash
git add src/ui
git commit -m "feat(ui): publish and unpublish from the live panel, and say so on the bar"
```

---

