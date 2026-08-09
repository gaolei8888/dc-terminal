### Task 1: 只在 agent 真要鼠标时才抓

**Files:**
- Modify: `src/ui/app.rs`（加 `copy_mode` 字段）
- Modify: `src/ui/mod.rs`（新增 `wants_mouse_capture`，换掉捕获判据）

**Interfaces:**
- Consumes: `App.scroll: ScrollState`，其 `agent_owns: bool` 由守护进程每帧带回（`src/pty.rs::view_of` → `session.rs::state_of` → `Response::Screen.scroll`）
- Produces: `fn wants_mouse_capture(attached: bool, agent_subscribed: bool, copy_mode: bool) -> bool`；`App.copy_mode: bool`

- [ ] **Step 1: 写失败的测试**

追加到 `src/ui/mod.rs` 的 `mod tests`：

```rust
/// 三个条件的真值表，八种组合全列。
///
/// 穷举而不是挑几个代表：这个函数错一格的后果是「用户在会话里复制不了」
/// 或者「agent 收不到它订阅的鼠标」，两种都不会 panic、不会报错，只会让人
/// 觉得工具坏了却说不清哪儿坏。八行断言比一句「应该没问题」便宜得多。
#[test]
fn mouse_is_captured_only_when_all_three_conditions_hold() {
    // attached, agent_subscribed, copy_mode -> want
    assert!(wants_mouse_capture(true, true, false));
    assert!(!wants_mouse_capture(true, true, true), "复制模式一票否决");
    assert!(
        !wants_mouse_capture(true, false, false),
        "agent 不要鼠标就别抓——抓了用户就白白丢了拖选复制"
    );
    assert!(!wants_mouse_capture(true, false, true));
    assert!(!wants_mouse_capture(false, true, false), "看板上永远不抓");
    assert!(!wants_mouse_capture(false, true, true));
    assert!(!wants_mouse_capture(false, false, false));
    assert!(!wants_mouse_capture(false, false, true));
}

/// 断连时 `Screen` 拉不到，`app.scroll` 保持上一帧的值，所以捕获状态不该翻转。
/// 翻转要往 stdout 写转义序列，断连时反复翻转是最吵的一种失败。
#[test]
fn a_failed_screen_call_does_not_flip_the_capture_state() {
    let (mut app, _d) = App::test_app();
    app.view = View::Attached(1);
    app.scroll = crate::session::ScrollState {
        agent_owns: true,
        ..Default::default()
    };
    let before = wants_mouse_capture(true, app.scroll.agent_owns, app.copy_mode);

    // 一次失败的 Screen 调用不会碰 app.scroll
    app.connected = false;

    assert_eq!(
        wants_mouse_capture(true, app.scroll.agent_owns, app.copy_mode),
        before
    );
}

#[test]
fn a_fresh_app_is_not_in_copy_mode() {
    let (app, _d) = App::test_app();
    assert!(!app.copy_mode);
}
```

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib ui::tests`
Expected: FAIL，`cannot find function 'wants_mouse_capture'`。

- [ ] **Step 3: 加 `App.copy_mode`**

`src/ui/app.rs`，在 `scroll` 字段附近加：

```rust
    /// 用户按 `F4` 打开的复制模式：**临时**把鼠标交还给终端，好让人用
    /// 终端自己的拖选去复制。
    ///
    /// 它是「此刻正在复制」的临时状态，不是配置——离开会话一律复位
    /// （见 `attach::handle_key`）。跨会话粘着的话，用户会在另一个会话里
    /// 发现鼠标莫名其妙不归 agent 管，而屏幕上没有任何东西解释为什么。
    pub copy_mode: bool,
```

`new_inner` 的字段初值里加 `copy_mode: false,`。

- [ ] **Step 4: 写 `wants_mouse_capture`**

`src/ui/mod.rs`，紧挨着 `mouse_capture_transition`：

```rust
/// 这一帧该不该抓鼠标。三个条件全真才抓。
///
/// 抽成纯函数的理由同 `mouse_capture_transition`：副作用（往 stdout 写转义
/// 序列）没法单测，判断能测——而且判断错了两个方向都难受：漏关，用户在会话里
/// 连拖选复制都做不了；漏开，agent 收不到它明明订阅了的鼠标事件。
///
/// `agent_subscribed` 来自 `App.scroll.agent_owns`，**不新开一条判据**。
/// 那个字段的语义就是「agent 自己攥着鼠标」，跟这里问的是同一个事实；
/// 各读各的，迟早会分叉成「dct 抓着鼠标却不肯滚」这种自相矛盾的状态。
fn wants_mouse_capture(attached: bool, agent_subscribed: bool, copy_mode: bool) -> bool {
    attached && agent_subscribed && !copy_mode
}
```

- [ ] **Step 5: 换掉主循环里的判据**

`src/ui/mod.rs` 的 `run()`，把原来那段（`let is_attached = matches!(app.view, View::Attached(_));` 起）改成：

```rust
        // 抓不抓鼠标不再只看「在不在会话里」：agent 没订阅鼠标的会话
        // （codex、shell）里抓着它，唯一的效果是把终端的拖选复制废掉，
        // 换来一个 PageUp/PageDown/End 已经能做的滚轮。
        //
        // 放在 `term.draw` 之前的理由不变（见下一段注释）；而且这一轮的
        // `Screen` 响应已经落进 `app.scroll` 了，`agent_owns` 就是这一帧的事实。
        let is_attached = matches!(app.view, View::Attached(_));
        let want = wants_mouse_capture(is_attached, app.scroll.agent_owns, app.copy_mode);
        if let Some(enable) = mouse_capture_transition(mouse_captured, want) {
            let _ = if enable {
                execute!(std::io::stdout(), EnableMouseCapture)
            } else {
                execute!(std::io::stdout(), DisableMouseCapture)
            };
            mouse_captured = enable;
        }
```

原来那段「检查一次『在不在会话里』有没有变」的注释要跟着改——它现在检查的是三个条件的**合取**，不是「在不在会话里」。保留它关于「为什么放在 draw 之前」和「为什么不在每个分支各开关一次」的两条理由，那两条依然成立。

- [ ] **Step 6: 跑测试，确认通过**

Run: `cargo test`
Expected: PASS。既有的 `mouse_capture_toggles_only_on_a_real_transition` 不受影响——`mouse_capture_transition` 的签名没变。

- [ ] **Step 7: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui/app.rs src/ui/mod.rs
git commit -m "feat: only grab the mouse when the agent actually asked for it"
```

---

