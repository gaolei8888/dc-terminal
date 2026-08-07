### Task 7: `session.rs` 贯通滚动状态

**Files:**
- Modify: `src/session.rs`（`ScreenSnapshot`、`Session`、`screen`、`send_input`、`resize`，新增 `scroll`）
- Test: `src/session.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::pty::ScrollView`（Task 6）
- Produces:

```rust
/// 一屏文字 + 光标 + 滚动状态。
pub struct ScreenSnapshot {
    pub lines: Vec<Vec<ScreenSpan>>,
    pub cursor: (u16, u16),
    pub scroll: ScrollState,
}

/// 界面画底栏要用的全部滚动事实。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrollState {
    #[serde(default)] pub agent_owns: bool,
    #[serde(default)] pub alt_screen: bool,
    #[serde(default)] pub max: usize,
    #[serde(default)] pub offset: usize,
    #[serde(default)] pub new_lines: usize,
}

impl SessionManager {
    pub fn scroll(&self, id: u32, by: ScrollBy) -> Result<ScrollState>;
}

pub enum ScrollBy { Rows(i32), Bottom }
```

`ScreenSnapshot` 从元组别名改成结构体，是**破坏性**改动 —— Task 8 把
`PROTOCOL_VERSION` 从 1 改成 2。

**`new_lines` 怎么算：** vt100 的偏移只在两种情况下变——用户滚，或者新行
推入时自动 +1（`grid.rs:556-558`）。所以在 `Session` 里记一个
`scroll_mark: usize`：每次**用户主动**滚动之后把它设成当时的偏移，
`new_lines = offset.saturating_sub(mark)`。

- [ ] **Step 1: 写失败的测试**

```rust
    /// 造一个吐 N 行然后挂着的 shell 会话
    fn scrolling_session(mgr: &SessionManager, dir: &Path, n: usize) -> u32 {
        let mut p = fake_agent();
        p.is_agent = false;
        p.command = vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("i=1; while [ $i -le {n} ]; do echo line-$i; i=$((i+1)); done; sleep 30"),
        ];
        mgr.register_profile(p.clone());
        mgr.create(dir.to_str().unwrap(), &p.name, empty_secrets(), 24, 80)
            .unwrap()
    }

    #[test]
    fn typing_jumps_back_to_the_bottom() {
        let dir = init_repo();
        let mgr = SessionManager::new();
        let id = scrolling_session(&mgr, dir.path(), 100);
        wait_for_screen(&mgr, id, "line-100");

        mgr.scroll(id, ScrollBy::Rows(30)).unwrap();
        assert!(mgr.screen(id).unwrap().scroll.offset > 0);

        mgr.send_input(id, "x").unwrap();
        assert_eq!(
            mgr.screen(id).unwrap().scroll.offset,
            0,
            "一敲键就该回到底部，否则用户看不见自己打的字"
        );
    }

    #[test]
    fn resizing_jumps_back_to_the_bottom() {
        let dir = init_repo();
        let mgr = SessionManager::new();
        let id = scrolling_session(&mgr, dir.path(), 100);
        wait_for_screen(&mgr, id, "line-100");

        mgr.scroll(id, ScrollBy::Rows(30)).unwrap();
        mgr.resize(id, 40, 100).unwrap();
        assert_eq!(
            mgr.screen(id).unwrap().scroll.offset,
            0,
            "重排之后偏移的含义就失效了，只能回底"
        );
    }

    #[test]
    fn scroll_to_bottom_works() {
        let dir = init_repo();
        let mgr = SessionManager::new();
        let id = scrolling_session(&mgr, dir.path(), 100);
        wait_for_screen(&mgr, id, "line-100");

        mgr.scroll(id, ScrollBy::Rows(30)).unwrap();
        let st = mgr.scroll(id, ScrollBy::Bottom).unwrap();
        assert_eq!(st.offset, 0);
    }

    #[test]
    fn new_lines_counts_only_what_arrived_since_the_user_last_scrolled() {
        let dir = init_repo();
        let mgr = SessionManager::new();
        let mut p = fake_agent();
        p.is_agent = false;
        p.command = vec![
            "/bin/sh".into(),
            "-c".into(),
            "i=1; while [ $i -le 60 ]; do echo line-$i; i=$((i+1)); done; \
             sleep 1; i=1; while [ $i -le 5 ]; do echo new-$i; i=$((i+1)); done; sleep 30"
                .into(),
        ];
        mgr.register_profile(p.clone());
        let id = mgr
            .create(dir.path().to_str().unwrap(), &p.name, empty_secrets(), 24, 80)
            .unwrap();
        wait_for_screen(&mgr, id, "line-60");

        // 刚滚完，底下没有新东西
        let st = mgr.scroll(id, ScrollBy::Rows(20)).unwrap();
        assert_eq!(st.new_lines, 0);

        wait_for_screen(&mgr, id, "new-5");
        let st = mgr.screen(id).unwrap().scroll;
        assert_eq!(st.new_lines, 5, "5 行新内容进来了，得数得出来");

        // 用户再滚一次，计数重新归零
        let st = mgr.scroll(id, ScrollBy::Rows(1)).unwrap();
        assert_eq!(st.new_lines, 0);
    }

    #[test]
    fn scrolling_a_session_that_does_not_exist_says_so() {
        let mgr = SessionManager::new();
        assert!(mgr.scroll(999, ScrollBy::Rows(1)).is_err());
    }
```

`wait_for_screen` 是新的辅助函数（`screen_text_for_test` 已经有了）：

```rust
    fn wait_for_screen(mgr: &SessionManager, id: u32, needle: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if mgr.screen_text_for_test(id).contains(needle) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("等不到 {needle}");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib session:: -- --test-threads=1`
Expected: 编译失败，`no method named 'scroll'`。

- [ ] **Step 3: 实现**

`src/session.rs`：把 `ScreenSnapshot` 从

```rust
pub type ScreenSnapshot = (Vec<Vec<ScreenSpan>>, (u16, u16));
```

改成 Interfaces 里那个结构体，并加 `ScrollState` / `ScrollBy`。

`Session` 加一个字段：

```rust
    /// 用户上次**主动**滚动时的偏移。`new_lines` 靠它算：vt100 会在新行
    /// 推入时自动把偏移 +1（grid.rs:556-558，画面因此不动），所以
    /// 「偏移 - 这个标记」就正好是用户没看过的行数。
    ///
    /// 边界：偏移增长被历史总行数封顶，缓冲满 2000 行之后 new_lines 会
    /// 少算，画面也会开始往上飘（最老的行被挤掉了）。这是环形缓冲的
    /// 固有代价。
    scroll_mark: usize,
```

三个方法：

```rust
    pub fn scroll(&self, id: u32, by: ScrollBy) -> Result<ScrollState> {
        self.with_session(id, |s| {
            let v = match by {
                ScrollBy::Rows(n) => s.pty.scroll_by(n),
                ScrollBy::Bottom => s.pty.scroll_to_bottom(),
            };
            // 用户主动滚过了，「没看过的行数」从这一刻重新算
            s.scroll_mark = v.offset;
            Ok(state_of(v, s.scroll_mark))
        })
    }

    pub fn screen(&self, id: u32) -> Result<ScreenSnapshot> {
        self.with_session(id, |s| {
            let v = s.pty.scroll_state();
            Ok(ScreenSnapshot {
                lines: s.pty.screen_spans(),
                cursor: s.pty.cursor(),
                scroll: state_of(v, s.scroll_mark),
            })
        })
    }
```

`send_input` 里，写 PTY **之前**：

```rust
            // 一敲键就回到底部。滚上去的时候打字，字会落在看不见的地方，
            // 用户会以为键盘坏了。归零之后字符照常送出去，不吞。
            s.pty.scroll_to_bottom();
            s.scroll_mark = 0;
```

`resize` 里，改尺寸**之后**：

```rust
            // vt100 会按新宽度重排，偏移指向的行跟改之前不是同一行了。
            // 与其显示一个错位的画面，不如老老实实回到底部。
            s.pty.scroll_to_bottom();
            s.scroll_mark = 0;
```

模块级私有函数：

```rust
fn state_of(v: crate::pty::ScrollView, mark: usize) -> ScrollState {
    ScrollState {
        agent_owns: v.agent_owns,
        alt_screen: v.alt_screen,
        max: v.max,
        offset: v.offset,
        new_lines: v.offset.saturating_sub(mark),
    }
}
```

改完 `ScreenSnapshot` 之后，`daemon.rs` 里构造 `Response::Screen` 的地方
会编译不过——Task 8 会处理，本任务先让它编译通过（照结构体字段取值即可）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 194 个，全绿。

- [ ] **Step 5: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 6: 提交**

```bash
git add src/session.rs src/daemon.rs
git commit -m "feat: 会话层贯通滚动状态，打字和改尺寸都回到底部

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

