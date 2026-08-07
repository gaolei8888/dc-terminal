### Task 6: `pty.rs` 保留 2000 行历史，加滚动 API

**Files:**
- Modify: `src/pty.rs:91`（`Parser::new` 的第三个参数）
- Modify: `src/pty.rs`（加 `scroll_by` / `scroll_state`）
- Test: `src/pty.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: 无
- Produces:

```rust
/// 保留多少行滚出屏幕的内容。
pub const SCROLLBACK_ROWS: usize = 2000;

impl PtySession {
    /// 滚动并返回滚完之后的状态。正数往上翻进历史，负数往下。
    pub fn scroll_by(&self, rows: i32) -> ScrollView;
    /// 直接回到底部。
    pub fn scroll_to_bottom(&self) -> ScrollView;
    /// 不改变位置，只读当前状态。
    pub fn scroll_state(&self) -> ScrollView;
}

/// pty 层看到的滚动事实。协议层的 `ScrollState` 由它加上 `new_lines` 组成
/// （`new_lines` 要跨帧记忆，那是 session 层的事）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollView {
    pub offset: usize,
    pub max: usize,
    pub agent_owns: bool,
    pub alt_screen: bool,
}
```

**vt100 的三个事实（读过 0.15.2 源码，别再猜）：**
- `Parser::new(rows, cols, n)` 第三个参数是保留行数，现在传的 0。
- `Screen::cell()` → `visible_cell()` → `visible_rows()` 自带
  `skip(scrollback_len - offset)`（`grid.rs:120-125`），所以
  **`screen_spans()` 一行都不用改**。
- vt100 没有「现在攒了多少行历史」的公开接口。`set_scrollback` 内部会
  `rows.min(self.scrollback.len())` 钳一次（`grid.rs:183-185`），所以探测
  上限的办法是：设一个大得离谱的值，读回来就是真实上限。三次字段写，
  不分配不拷贝。

- [ ] **Step 1: 写失败的测试**

`src/pty.rs` 的测试模块里加（沿用已有的 `wait_for` 辅助函数）：

```rust
    /// 造一个吐 N 行然后挂着不退的会话
    fn spawn_lines(dir: &Path, n: usize) -> PtySession {
        PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                format!("i=1; while [ $i -le {n} ]; do echo line-$i; i=$((i+1)); done; sleep 30"),
            ],
            &Default::default(),
            dir,
            24,
            80,
        )
        .unwrap()
    }

    #[test]
    fn keeps_history_that_scrolled_off_the_screen() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 100);
        assert!(wait_for(&p, "line-100"));

        // 屏幕只有 24 行，line-1 早就滚出去了
        assert!(!p.screen_text().contains("line-1\n"));

        p.scroll_by(90);
        assert!(
            p.screen_text().contains("line-1\n"),
            "往上翻 90 行应该能看见最早那行"
        );
    }

    #[test]
    fn history_is_capped_at_the_configured_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), SCROLLBACK_ROWS + 500);
        assert!(wait_for(&p, &format!("line-{}", SCROLLBACK_ROWS + 500)));

        let st = p.scroll_state();
        assert_eq!(st.max, SCROLLBACK_ROWS, "上限就是上限，不能无限涨");
    }

    #[test]
    fn scrolling_past_the_top_stops_at_the_top() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 50);
        assert!(wait_for(&p, "line-50"));

        let st = p.scroll_by(i32::MAX);
        assert_eq!(st.offset, st.max, "翻到头就停在头，不能溢出");
    }

    #[test]
    fn scrolling_below_the_bottom_stops_at_the_bottom() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 50);
        assert!(wait_for(&p, "line-50"));

        p.scroll_by(10);
        let st = p.scroll_by(-1000);
        assert_eq!(st.offset, 0, "往下翻过头就停在底部");
    }

    /// 这条测的是 vt100 的行为，不是我们的代码——但整个「新输出时画面不动」
    /// 的设计都压在它上面（grid.rs:556-558）。它哪天变了，这里要第一个响。
    #[test]
    fn the_view_stays_put_when_new_output_arrives() {
        let dir = tempfile::tempdir().unwrap();
        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "i=1; while [ $i -le 60 ]; do echo line-$i; i=$((i+1)); done; \
                 sleep 1; echo MARKER-NEW; sleep 30".to_string(),
            ],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "line-60"));

        let before = p.scroll_by(30);
        assert!(wait_for_offset_to_grow(&p, before.offset));

        let after = p.scroll_state();
        assert!(
            after.offset > before.offset,
            "来了新行，偏移要跟着涨，画面才不动：{} -> {}",
            before.offset,
            after.offset
        );
    }

    fn wait_for_offset_to_grow(p: &PtySession, from: usize) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if p.scroll_state().offset > from {
                return true;
            }
            sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn an_alternate_screen_app_has_no_history_to_scroll() {
        let dir = tempfile::tempdir().unwrap();
        // ESC[?1049h 进备用屏，然后吐一堆行
        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf '\\033[?1049h'; i=1; while [ $i -le 60 ]; do echo alt-$i; \
                 i=$((i+1)); done; sleep 30".to_string(),
            ],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "alt-60"));

        let st = p.scroll_state();
        assert!(st.alt_screen, "应该认出它在备用屏上");
        assert_eq!(st.max, 0, "备用屏上没有历史，这跟真实终端一致");
    }

    /// 程序设了滚动区（DECSTBM）之后，vt100 不往 scrollback 里塞任何东西
    /// （grid.rs:551）。这不是我们能改的，但界面要能认出「这里翻不了」
    /// 而不是让用户对着一个没反应的滚轮猜。
    #[test]
    fn a_scroll_region_swallows_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf '\\033[1;20r'; i=1; while [ $i -le 60 ]; do echo rgn-$i; \
                 i=$((i+1)); done; sleep 30".to_string(),
            ],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "rgn-60"));
        assert_eq!(p.scroll_state().max, 0);
    }

    #[test]
    fn a_plain_shell_does_not_own_the_scrolling() {
        let dir = tempfile::tempdir().unwrap();
        let p = spawn_lines(dir.path(), 10);
        assert!(wait_for(&p, "line-10"));
        assert!(
            !p.scroll_state().agent_owns,
            "没开鼠标上报的程序，滚轮归 dct"
        );
    }

    #[test]
    fn an_app_that_asks_for_the_mouse_owns_the_scrolling() {
        let dir = tempfile::tempdir().unwrap();
        // ESC[?1000h 开鼠标上报，跟 Claude Code 实测抓到的一样
        let p = PtySession::spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf '\\033[?1000h'; echo mouse-on; sleep 30".to_string(),
            ],
            &Default::default(),
            dir.path(),
            24,
            80,
        )
        .unwrap();
        assert!(wait_for(&p, "mouse-on"));
        assert!(p.scroll_state().agent_owns);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib pty:: -- --test-threads=1`
Expected: 编译失败，`cannot find value 'SCROLLBACK_ROWS'`。

- [ ] **Step 3: 实现**

`src/pty.rs`，`PtySession` 定义之前：

```rust
/// 每个会话保留多少行滚出屏幕的内容。
///
/// 写死不做配置项：用户不该被问这个数字。vt100 的 `Cell` 约 36 字节，
/// 120 列一行约 4.2 KB，2000 行满载约 8.4 MB/会话。底下是 `VecDeque`，
/// 按实际用量增长，2000 是天花板不是预分配。
pub const SCROLLBACK_ROWS: usize = 2000;

/// pty 层看到的滚动事实。
///
/// `agent_owns` 是整个滚屏设计的分流开关：agent 开了鼠标上报就说明它自己
/// 管视口（Claude Code 就是这样），滚轮该转发给它；没开就由 dct 滚自己的
/// 缓冲（codex、命令行）。这两个真实 agent 在「用不用备用屏」上正好相反，
/// 所以判据只能是鼠标，不能是备用屏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollView {
    pub offset: usize,
    pub max: usize,
    pub agent_owns: bool,
    pub alt_screen: bool,
}
```

`spawn()` 里改一个数字：

```rust
        let parser = Arc::new(Mutex::new(vt100::Parser::new(
            rows,
            cols,
            SCROLLBACK_ROWS,
        )));
```

加三个方法：

```rust
    /// 滚动并返回滚完之后的状态。正数往上翻进历史，负数往下。
    ///
    /// 钳位交给 vt100 自己做（`grid.rs:183-185` 会 `.min(scrollback.len())`），
    /// 我们只负责别让 i32 加法溢出。
    pub fn scroll_by(&self, rows: i32) -> ScrollView {
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let max = probe_max(&mut parser);
        let cur = parser.screen().scrollback();
        let target = if rows >= 0 {
            cur.saturating_add(rows as usize)
        } else {
            cur.saturating_sub(rows.unsigned_abs() as usize)
        };
        parser.set_scrollback(target.min(max));
        view_of(&parser, max)
    }

    pub fn scroll_to_bottom(&self) -> ScrollView {
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        // probe_max 会把偏移推到顶，所以归零必须在它之后
        let max = probe_max(&mut parser);
        parser.set_scrollback(0);
        view_of(&parser, max)
    }

    pub fn scroll_state(&self) -> ScrollView {
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let cur = parser.screen().scrollback();
        let max = probe_max(&mut parser);
        parser.set_scrollback(cur);
        view_of(&parser, max)
    }
```

模块级私有函数：

```rust
/// vt100 不公开「现在攒了多少行历史」。但 `set_scrollback` 内部会
/// `.min(scrollback.len())` 钳一次，所以设一个大得离谱的值再读回来，
/// 读到的就是真实上限。三次字段写，不分配不拷贝。
///
/// **调用方负责把偏移放回去**——这个函数会改变它。
fn probe_max(parser: &mut vt100::Parser) -> usize {
    parser.set_scrollback(usize::MAX);
    parser.screen().scrollback()
}

fn view_of(parser: &vt100::Parser, max: usize) -> ScrollView {
    let screen = parser.screen();
    ScrollView {
        offset: screen.scrollback(),
        max,
        agent_owns: screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None,
        alt_screen: screen.alternate_screen(),
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib pty:: -- --test-threads=1`
Expected: 全绿，pty 的测试从 5 个变成 14 个。

- [ ] **Step 5: 跑全量**

Run: `cargo test -- --test-threads=1`
Expected: 189 个，全绿。

- [ ] **Step 6: 格式与静态检查**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 7: 提交**

```bash
git add src/pty.rs
git commit -m "feat: PTY 保留 2000 行历史，加滚动 API

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

