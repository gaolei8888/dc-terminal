### Task 8: 协议加 `Scroll` 与 `Mouse`，版本 +1

**Files:**
- Modify: `src/proto.rs`（`PROTOCOL_VERSION`、`Request`、`Response::Screen`、手写 `Debug`）
- Modify: `src/daemon.rs`（`handle` 的两条新 arm、`Response::Screen` 的构造）
- Test: `src/proto.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::session::{ScrollState, ScrollBy}`（Task 7）
- Produces:

```rust
pub const PROTOCOL_VERSION: u32 = 2;   // 从 1 改过来

Request::Scroll { id: u32, by: ScrollBy },
Request::Mouse  { id: u32, event: MouseForward },

Response::Screen {
    lines: Vec<Vec<ScreenSpan>>,
    cursor: (u16, u16),
    #[serde(default)]
    scroll: ScrollState,
}
Response::Scrolled(ScrollState),

pub struct MouseForward {
    pub col: u16,
    pub row: u16,
    pub kind: MouseForwardKind,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

pub enum MouseForwardKind {
    WheelUp,
    WheelDown,
    Press(u8),    // 0=左 1=中 2=右
    Release(u8),
}
```

- [ ] **Step 1: 写失败的测试**

`src/proto.rs` 的测试模块：

```rust
    /// 新加的滚动字段必须能从「没有这个字段」的旧 JSON 里解出来。
    /// 这是 `#[serde(default)]` 的意义所在：往后再加字段就不用再动版本号。
    #[test]
    fn a_screen_response_without_scroll_still_parses() {
        let old = r#"{"Screen":{"lines":[],"cursor":[0,0]}}"#;
        let r: Response = serde_json::from_str(old).unwrap();
        match r {
            Response::Screen { scroll, .. } => {
                assert_eq!(scroll, crate::session::ScrollState::default());
            }
            _ => panic!("解成了别的变体"),
        }
    }

    #[test]
    fn scroll_requests_survive_a_round_trip() {
        for by in [ScrollBy::Rows(3), ScrollBy::Rows(-3), ScrollBy::Bottom] {
            let req = Request::Scroll { id: 7, by };
            let s = serde_json::to_string(&req).unwrap();
            let back: Request = serde_json::from_str(&s).unwrap();
            assert!(matches!(back, Request::Scroll { id: 7, .. }));
        }
    }

    /// 手写的 Debug 漏一条 arm 会编译不过，但漏了密钥脱敏不会。
    /// 顺手确认新变体没有把什么敏感东西带进 Debug。
    #[test]
    fn mouse_debug_has_no_surprises() {
        let req = Request::Mouse {
            id: 1,
            event: MouseForward {
                col: 10,
                row: 20,
                kind: MouseForwardKind::WheelUp,
                shift: false,
                alt: false,
                ctrl: false,
            },
        };
        let s = format!("{req:?}");
        assert!(s.contains("Mouse"));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib proto:: -- --test-threads=1`
Expected: 编译失败，`no variant named 'Scroll'`。

- [ ] **Step 3: 改协议**

`src/proto.rs`：`PROTOCOL_VERSION` 改成 `2`，并在它的文档注释末尾追加一行：

```rust
/// 第 2 版：`Response::Screen` 加了 `scroll`，`ScreenSnapshot` 从元组改成结构体。
```

加 Interfaces 里列的两个 `Request` 变体、`Response::Scrolled`、
`Response::Screen` 的 `scroll` 字段（带 `#[serde(default)]`）、
`MouseForward` / `MouseForwardKind`。手写 `Debug` 补两条 arm。

- [ ] **Step 4: 守护进程分派**

`src/daemon.rs` 的 `handle`：

```rust
        Request::Scroll { id, by } => mgr.scroll(id, by).map(Response::Scrolled),
        Request::Mouse { id, event } => mgr.forward_mouse(id, event).map(|_| Response::Ok),
```

`Response::Screen` 的构造改成取结构体字段：

```rust
        Request::Screen { id } => mgr.screen(id).map(|s| Response::Screen {
            lines: s.lines,
            cursor: s.cursor,
            scroll: s.scroll,
        }),
```

`mgr.forward_mouse` 在 Task 9 实现；本步先加一个占位实现让它编译：

```rust
    /// 把界面转发过来的鼠标事件按 agent 当前的编码写进 PTY。
    pub fn forward_mouse(&self, id: u32, ev: MouseForward) -> Result<()> {
        self.with_session(id, |s| s.pty.write_mouse(ev))
    }
```

`PtySession::write_mouse` 也在 Task 9 实现，本步给一个直接返回 `Ok(())`
的空壳并加 `// Task 9 实现` 注释。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 197 个，全绿。

- [ ] **Step 6: 手工验一次版本提示**

```bash
cargo build --release
# 让旧守护进程还跑着（第 1 版），用新界面连
./target/release/dct
```
Expected: 立刻打印「后台服务是第 1 版，界面是第 2 版……运行 dct restart」，
不是「拿不到 agent 列表」。

- [ ] **Step 7: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 8: 提交**

```bash
git add src/proto.rs src/daemon.rs src/session.rs src/pty.rs
git commit -m "feat: 协议加 Scroll 与 Mouse，版本升到 2

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

