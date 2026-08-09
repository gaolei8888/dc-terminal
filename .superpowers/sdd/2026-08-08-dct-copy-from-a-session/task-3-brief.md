### Task 3: 底栏写清楚现在是什么状态

**Files:**
- Modify: `src/ui/mod.rs`（`draw` 里底栏内容的选择）
- Modify: `src/i18n.rs`（新词条）

**Interfaces:**
- Consumes: Task 1 的 `App.copy_mode`
- Produces: `Key::CopyMode`

- [ ] **Step 1: 写失败的测试**

追加到 `src/ui/mod.rs` 的 `mod tests`：

```rust
/// 模式看不见就是下一个隐形状态，而这个仓库刚花一整轮改造消灭掉那种东西。
#[test]
fn copy_mode_says_so_in_the_bar() {
    for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
        let (mut app, _d) = app_with_one_agent_session(View::Attached(1));
        app.lang = lang;
        app.copy_mode = true;
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();

        let bar = bar_text(&term);
        let hint = crate::i18n::text(crate::i18n::Key::CopyMode, lang);
        assert!(bar.contains(&hint.replace(' ', "")), "{lang:?} 下底栏要写着复制模式：{bar}");
    }
}

/// 优先级：错误消息 > 复制模式 > 滚动提示。
///
/// 复制模式压过滚动提示，是因为在复制模式下滚轮根本不归 dct 管，
/// 那条提示这时候是错的；而错误消息压过复制模式，是因为出错是一次性的、
/// 不说就再也没机会说，复制模式则是个持续状态，下一帧还会写。
#[test]
fn an_error_beats_copy_mode_which_beats_the_scroll_hint() {
    let (mut app, _d) = app_with_one_agent_session(View::Attached(1));
    app.copy_mode = true;
    app.scroll = crate::session::ScrollState {
        offset: 5,
        max: 100,
        ..Default::default()
    };
    let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();

    term.draw(|f| draw(f, &mut app)).unwrap();
    let hint = crate::i18n::text(crate::i18n::Key::CopyMode, app.lang);
    assert!(
        bar_text(&term).contains(&hint.replace(' ', "")),
        "复制模式压过滚动提示"
    );

    app.message = Msg::err("出事了".into());
    term.draw(|f| draw(f, &mut app)).unwrap();
    assert!(bar_text(&term).contains("出事了"), "错误消息压过复制模式");
}
```

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib ui::tests`
Expected: FAIL，`no variant named 'CopyMode'`。

- [ ] **Step 3: 加词条**

`src/i18n.rs` 的 `Key` 枚举：

```rust
    /// 复制模式下顶掉整条底栏右段的提示
    CopyMode,
```

译文表：

```rust
        CopyMode => t!(
            lang,
            en: "Copy mode · the mouse is the terminal's · F4 to exit",
            zh: "复制模式 · 鼠标已交还终端 · F4 退出"
        ),
```

并加进文件末尾那份穷举列表（漏了编译不过）。

- [ ] **Step 4: 插进底栏**

`src/ui/mod.rs::draw`，把算 `scroll_hint` 的那一段改成：

```rust
        // 复制模式压过滚动提示：这时候滚轮根本不归 dct 管，那条提示是错的。
        // 压不过错误消息——外层 if/else 链已经保证了这一点。
        let hint = match &app.view {
            View::Attached(_) if app.copy_mode => {
                Some(crate::i18n::text(crate::i18n::Key::CopyMode, app.lang).to_string())
            }
            View::Attached(_) => attach::scroll_hint(&app.scroll, app.lang),
            _ => None,
        };
        match hint {
            Some(h) => (BarContent::Text(h), Style::default()),
            None => (
                BarContent::Keys(idle_help(&app.view, app.lang, help_ctx(app))),
                Style::default(),
            ),
        }
```

- [ ] **Step 5: 跑测试，确认通过**

Run: `cargo test`
Expected: PASS。既有的 `a_scroll_hint_takes_over_the_bottom_bar_when_there_is_history` 和 `a_message_beats_the_scroll_hint` 都不该受影响——它们的 `copy_mode` 是 `false`。

- [ ] **Step 6: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui/mod.rs src/i18n.rs
git commit -m "feat: the bar says plainly when the mouse belongs to the terminal"
```

---

