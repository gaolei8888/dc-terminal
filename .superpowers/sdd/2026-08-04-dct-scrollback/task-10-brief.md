### Task 10: 会话视图接上滚动

**Files:**
- Modify: `src/ui/mod.rs`（`restore_terminal`、事件循环收 `Event::Mouse`）
- Modify: `src/ui/attach.rs`（路由、按键、底栏提示）
- Modify: `src/ui/app.rs`（存一份最近的 `ScrollState`）
- Test: `src/ui/attach.rs`

**Interfaces:**
- Consumes: `crate::session::ScrollState`（Task 7）、`crate::proto::{Request, ScrollBy, MouseForward, MouseForwardKind}`（Task 8）
- Produces:

```rust
/// 一次滚轮/翻页该做什么。纯函数，好测。
pub(crate) enum ScrollAction {
    /// 转发给 agent
    Forward,
    /// dct 自己滚这么多行
    Scroll(i32),
    /// 什么都不做
    Ignore,
}

pub(crate) fn wheel_action(st: &ScrollState, up: bool) -> ScrollAction;
pub(crate) fn key_scroll(st: &ScrollState, key: &KeyEvent, page: u16) -> Option<ScrollAction>;
pub(crate) fn scroll_hint(st: &ScrollState) -> Option<String>;
```

**步长：** 滚轮一格 3 行（终端惯例）；`PageUp`/`PageDown` 一屏减 2 行；
`End` 回底。三个都只在 `!agent_owns` 时由 dct 处理。

- [ ] **Step 1: 写失败的测试**

`src/ui/attach.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ScrollState;

    /// `key()` 在 Task 3 里跟着 View 的测试搬进了 `ui/view.rs`。这里要用，
    /// 就把它在 `view.rs` 的测试模块里标成 `pub(crate)`，别复制一份——
    /// 两份同名辅助函数迟早会漂成两个语义。
    use crate::ui::view::tests::key;

    fn own(agent_owns: bool, max: usize, offset: usize, new_lines: usize) -> ScrollState {
        ScrollState {
            agent_owns,
            alt_screen: false,
            max,
            offset,
            new_lines,
        }
    }

    #[test]
    fn an_agent_that_wants_the_mouse_gets_the_wheel() {
        assert!(matches!(
            wheel_action(&own(true, 0, 0, 0), true),
            ScrollAction::Forward
        ));
    }

    #[test]
    fn otherwise_dct_scrolls_three_rows_per_notch() {
        assert!(matches!(
            wheel_action(&own(false, 500, 0, 0), true),
            ScrollAction::Scroll(3)
        ));
        assert!(matches!(
            wheel_action(&own(false, 500, 10, 0), false),
            ScrollAction::Scroll(-3)
        ));
    }

    #[test]
    fn there_is_nothing_to_scroll_when_there_is_no_history() {
        assert!(matches!(
            wheel_action(&own(false, 0, 0, 0), true),
            ScrollAction::Ignore
        ));
    }

    #[test]
    fn page_keys_belong_to_the_agent_when_it_owns_the_viewport() {
        // None 表示「不归我管」，让它落到普通按键路径送给 agent
        assert!(key_scroll(&own(true, 0, 0, 0), &key(KeyCode::PageUp), 24).is_none());
        assert!(key_scroll(&own(true, 0, 0, 0), &key(KeyCode::End), 24).is_none());
    }

    #[test]
    fn page_keys_scroll_a_screen_minus_two() {
        let up = key_scroll(&own(false, 500, 0, 0), &key(KeyCode::PageUp), 24).unwrap();
        assert!(matches!(up, ScrollAction::Scroll(22)));
        let down = key_scroll(&own(false, 500, 30, 0), &key(KeyCode::PageDown), 24).unwrap();
        assert!(matches!(down, ScrollAction::Scroll(-22)));
    }

    #[test]
    fn a_tiny_window_still_scrolls_at_least_one_row() {
        let up = key_scroll(&own(false, 500, 0, 0), &key(KeyCode::PageUp), 2).unwrap();
        assert!(matches!(up, ScrollAction::Scroll(1)), "别算出 0 行或负数");
    }

    #[test]
    fn ordinary_keys_are_not_scroll_keys() {
        assert!(key_scroll(&own(false, 500, 10, 0), &key(KeyCode::Char('a')), 24).is_none());
    }

    #[test]
    fn the_hint_says_how_much_is_waiting_below() {
        let h = scroll_hint(&own(false, 500, 40, 12)).unwrap();
        assert!(h.contains("12"), "得说清有多少新东西: {h}");
    }

    #[test]
    fn the_hint_says_how_to_get_back_when_nothing_is_new() {
        let h = scroll_hint(&own(false, 500, 40, 0)).unwrap();
        assert!(h.contains("40"), "得说清翻了多远: {h}");
        assert!(h.contains("End"), "得说清怎么回去: {h}");
    }

    #[test]
    fn an_alt_screen_agent_that_ignores_the_mouse_gets_an_explanation() {
        let mut st = own(false, 0, 0, 0);
        st.alt_screen = true;
        let h = scroll_hint(&st).expect("这种情况谁都滚不了，必须说一声");
        assert!(!h.contains("End"), "都滚不了了就别提回底部: {h}");
        assert!(
            !h.contains("备用屏") && !h.contains("scrollback"),
            "不能有黑话: {h}"
        );
    }

    #[test]
    fn a_fresh_session_with_no_history_says_nothing() {
        assert!(scroll_hint(&own(false, 0, 0, 0)).is_none());
        assert!(scroll_hint(&own(true, 0, 0, 0)).is_none());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib ui::attach -- --test-threads=1`
Expected: 编译失败，`cannot find function 'wheel_action'`。

- [ ] **Step 3: 实现三个纯函数**

`src/ui/attach.rs`：

```rust
/// 滚轮一格滚几行。3 是终端惯例，改了会跟用户在别处的肌肉记忆打架。
const WHEEL_ROWS: i32 = 3;

pub(crate) enum ScrollAction {
    Forward,
    Scroll(i32),
    Ignore,
}

/// 谁拿这一格滚轮。
///
/// 判据是「agent 有没有开鼠标上报」，不是「它在不在备用屏」——实测下来
/// Claude Code 备用屏 + 全套鼠标，codex 内联 + 完全不要鼠标，两个真实
/// agent 在这两个维度上正好相反。按鼠标分流恰好把两边都送到握着内容的
/// 那一方：Claude Code 自己管视口，codex 的历史在 dct 缓冲里。
pub(crate) fn wheel_action(st: &ScrollState, up: bool) -> ScrollAction {
    if st.agent_owns {
        return ScrollAction::Forward;
    }
    if st.max == 0 {
        return ScrollAction::Ignore;
    }
    ScrollAction::Scroll(if up { WHEEL_ROWS } else { -WHEEL_ROWS })
}

/// 翻页键归谁。`None` = 不归 dct 管，让它落到普通按键路径送给 agent。
pub(crate) fn key_scroll(st: &ScrollState, key: &KeyEvent, page: u16) -> Option<ScrollAction> {
    if st.agent_owns {
        return None;
    }
    // 一屏减 2 行：留两行重叠，翻页之后还能看到上一屏的尾巴，
    // 不然读长输出时每翻一页都要重新找位置。窗口太小时至少滚 1 行。
    let step = i32::from(page).saturating_sub(2).max(1);
    match key.code {
        KeyCode::PageUp => Some(ScrollAction::Scroll(step)),
        KeyCode::PageDown => Some(ScrollAction::Scroll(-step)),
        KeyCode::End if st.offset > 0 => Some(ScrollAction::Scroll(-i32::MAX)),
        _ => None,
    }
}

/// 底栏那一句。`None` = 不显示。
pub(crate) fn scroll_hint(st: &ScrollState) -> Option<String> {
    if st.offset > 0 && st.new_lines > 0 {
        return Some(format!("↓ 下面还有 {} 行新内容", st.new_lines));
    }
    if st.offset > 0 {
        return Some(format!("↑ 已往上翻 {} 行 · 按 End 回到底部", st.offset));
    }
    // agent 自己占着画面又不收鼠标：谁都滚不了。装死的话用户会以为
    // 滚轮坏了，一直试。
    if !st.agent_owns && st.alt_screen {
        return Some("这个 agent 自己管画面，翻不了历史".to_string());
    }
    None
}
```

> `End` 用 `Scroll(-i32::MAX)` 而不是单独一个 `Bottom` 分支：
> `ScrollBy::Rows` 在守护进程侧是 `saturating_sub` 之后再钳到 `[0, max]`，
> 结果跟 `ScrollBy::Bottom` 完全一样，多一个分支只是多一处要测的东西。

- [ ] **Step 4: 接上事件循环**

`src/ui/app.rs` 加一个字段：

```rust
    /// 最近一次 Screen 响应带回来的滚动状态。按键和滚轮都要看它分流，
    /// 而它每帧都会被刷新——滞后最多一帧，够用了。
    pub scroll: ScrollState,
```

`src/ui/mod.rs`：

1. `Event::Key` 之前先收 `Event::Mouse`：

```rust
        if let Event::Mouse(m) = ev {
            attach::handle_mouse(&mut app, m)?;
            continue;
        }
```

（这个 `continue` 在按键处理**之前**，不在任何按键分支里，不违反房规。
但循环末尾清理 `message` 的那段也会被它跳过——所以 `handle_mouse` 里
**不许**改 `app.message`。把这句话写成注释放在它上面。）

2. 进出会话时开关鼠标捕获：

```rust
// 进入 View::Attached 时
execute!(std::io::stdout(), EnableMouseCapture)?;
// 离开 View::Attached 时
execute!(std::io::stdout(), DisableMouseCapture)?;
```

只在会话里开：看板不需要滚，而开着捕获会让终端原生的选中复制失效——
把这个代价限制在真正需要它的地方。

3. `restore_terminal()` 里无条件加 `DisableMouseCapture`：

```rust
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
}
```

没开过捕获时多发一次关闭序列是无害的，而漏关会让用户的终端从此点哪儿都
冒出乱码。`TerminalGuard::drop` 和 `spawn_signal_restore` 都走这个函数，
所以所有退出路径（正常退出、`?` 提前返回、panic、SIGTERM）自动覆盖。

4. `attach.rs` 里实现 `handle_mouse`：

```rust
/// **这个函数里不许改 `app.message`。** 主循环在调用它之后直接
/// `continue`，跳过了循环末尾清理陈旧消息的那一段——在这里设一条消息，
/// 它会一直挂在屏幕上直到下一次按键。
pub(crate) fn handle_mouse(app: &mut App, m: MouseEvent) -> Result<()> {
    let View::Attached(id) = app.view else {
        return Ok(());
    };
    let (up, forwardable) = match m.kind {
        MouseEventKind::ScrollUp => (true, Some(MouseForwardKind::WheelUp)),
        MouseEventKind::ScrollDown => (false, Some(MouseForwardKind::WheelDown)),
        MouseEventKind::Down(b) => (false, Some(MouseForwardKind::Press(button_code(b)))),
        MouseEventKind::Up(b) => (false, Some(MouseForwardKind::Release(button_code(b)))),
        // 纯移动不转发：Claude Code 开了 ?1003h，每动一下就是一个事件，
        // 全部经 socket 转发过去量很大，换来的只是悬停高亮。这是有意的
        // 部分实现，不是遗漏。
        _ => (false, None),
    };
    let Some(kind) = forwardable else {
        return Ok(());
    };

    let is_wheel = matches!(kind, MouseForwardKind::WheelUp | MouseForwardKind::WheelDown);
    if is_wheel {
        match wheel_action(&app.scroll, up) {
            ScrollAction::Ignore => return Ok(()),
            ScrollAction::Scroll(n) => {
                let _ = app.client()?.call(Request::Scroll {
                    id,
                    by: ScrollBy::Rows(n),
                });
                return Ok(());
            }
            ScrollAction::Forward => {}
        }
    } else if !app.scroll.agent_owns {
        // 不收鼠标的 agent 收到点击事件只会看到一串乱码
        return Ok(());
    }

    // 终端坐标减掉边框，换算成 agent 画面里的坐标
    let Some((col, row)) = app.screen_origin.and_then(|(c0, r0)| {
        Some((m.column.checked_sub(c0)?, m.row.checked_sub(r0)?))
    }) else {
        return Ok(());
    };

    let _ = app.client()?.call(Request::Mouse {
        id,
        event: MouseForward {
            col,
            row,
            kind,
            shift: m.modifiers.contains(KeyModifiers::SHIFT),
            alt: m.modifiers.contains(KeyModifiers::ALT),
            ctrl: m.modifiers.contains(KeyModifiers::CONTROL),
        },
    });
    Ok(())
}

fn button_code(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}
```

`App` 再加一个字段 `pub screen_origin: Option<(u16, u16)>`，由 `attach::draw`
在每帧画完之后填上会话内容区左上角的终端坐标。**它必须由 `draw` 填，
不能在 `handle_mouse` 里硬算边框宽度**——布局改了硬算的数就错了，而且
错得很安静。

5. 会话按键路径里，在把按键交给 `key_to_input` 之前先问一次 `key_scroll`：

```rust
    if let Some(action) = key_scroll(&app.scroll, &key, content_rows) {
        if let ScrollAction::Scroll(n) = action {
            let _ = app.client()?.call(Request::Scroll {
                id,
                by: ScrollBy::Rows(n),
            });
        }
        return Ok(());
    }
```

6. `attach::draw` 把 `scroll_hint` 的结果画到底栏。它跟 `message` 抢同一行时
**`message` 优先**——消息是对用户刚才那个动作的回应，滚动提示是持续状态，
盖掉前者会让用户以为自己那步操作没反应。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 217 个，全绿。

- [ ] **Step 6: 手工验收（自动化不了，必须真跑）**

```bash
cargo build --release
./target/release/dct restart
./target/release/dct
```

1. 开一个 codex 会话，让它吐一屏以上的东西，滚轮往上：
   Expected: 看得到历史；画面不花；底部状态条不被拽进内容区；
   底栏出现「↑ 已往上翻 N 行 · 按 End 回到底部」。
2. 保持滚上去的状态，让 codex 再输出几行：
   Expected: 画面**不动**，底栏变成「↓ 下面还有 N 行新内容」。
3. 这时候敲一个字符：
   Expected: 立刻跳回底部，而且那个字符确实进了 codex 的输入框。
4. `PageUp` / `PageDown` / `End`：Expected: 分别翻一屏、翻回、回底。
5. 开一个 Claude Code 会话，滚轮往上：
   Expected: 滚的是 **Claude Code 自己的对话记录**，不是 dct 的缓冲；
   底栏不出现任何滚动提示。
6. 从会话退回看板，用鼠标在终端里拖选文字：
   Expected: 能选中（说明捕获确实关掉了）。
7. `Ctrl+C` 掉整个 dct，然后在终端里点几下：
   Expected: 不冒乱码（说明 `restore_terminal` 关掉了捕获）。

- [ ] **Step 7: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 8: 提交**

```bash
git add -A src/ui
git commit -m "feat: 会话里能往回翻历史了

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

