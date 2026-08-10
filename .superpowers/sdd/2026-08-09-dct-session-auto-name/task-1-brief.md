### Task 1: 九宫格焦点按会话 id 锚定

这是用户报的「一个 session 的消息发给了另一个 session」的根因，**跟命名无关，独立可上线**。

**Files:**
- Modify: `src/ui/app.rs:274-305`（`refresh_rows`）
- Test: `src/ui/app.rs` 的 `mod tests`（同文件内联，跟仓库其余测试一致）

**Interfaces:**
- Consumes: 无
- Produces: 无新公开接口。`refresh_rows()` 的行为契约变成「光标**和**九宫格焦点都按会话身份找回原位」

- [ ] **Step 1: 写失败测试**

加在 `src/ui/app.rs` 的 `mod tests` 里，紧挨着已有的 `refresh_rows_clamps_the_grid_focus_into_the_new_range`：

```rust
    /// 焦点是**身份**，不是位置。前面的会话没了，格子整体前移，焦点必须
    /// 还站在原来那个会话上。
    ///
    /// 不修的话：`i 回一句` 的收件人取自 `visible.get(focus)`
    /// （`grid.rs`），焦点漂到哪儿消息就发给谁 —— 而 `s`（停止）和
    /// `u`（回滚）走同一条路，两个都不可撤销。
    #[test]
    fn refresh_rows_keeps_the_grid_focus_on_the_same_session() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/a"), sess(3, "/w/a")]);
        app.view = View::grid(2); // 焦点在 3 号身上

        // 1 号跑完停了。九宫格不画已停止的会话，后面两格整体前移一位。
        let mut gone = sess(1, "/w/a");
        gone.state = crate::session::SessionState::Stopped;
        app.set_sessions(vec![gone, sess(2, "/w/a"), sess(3, "/w/a")]);

        let visible = app.grid_sessions();
        assert_eq!(visible.len(), 2, "已停止的那个不进九宫格");
        let View::Grid { focus, .. } = app.view else {
            panic!("还该在九宫格里");
        };
        assert_eq!(
            visible[focus].id,
            3,
            "焦点必须还站在 3 号身上，实际站在 {} 号上",
            visible[focus].id
        );
    }
```

- [ ] **Step 2: 跑它，确认它红**

```bash
cargo test --lib ui::app::tests::refresh_rows_keeps_the_grid_focus_on_the_same_session
```

预期：FAIL，`焦点必须还站在 3 号身上，实际站在 2 号上`。

- [ ] **Step 3: 最小实现**

`src/ui/app.rs` 的 `refresh_rows()`：在函数开头（取列表光标锚点的**旁边**）加上焦点锚点 ——

```rust
    pub fn refresh_rows(&mut self) {
        let anchor = self
            .list_state
            .selected()
            .and_then(|i| super::view::anchor_of(&self.groups, &self.rows, i));
        // 九宫格焦点也要按身份锚定。**必须在重算之前取**，理由同上面那行：
        // 重算之后取到的是新列表里的东西，等于没锚。
        let grid_anchor = match &self.view {
            View::Grid { focus, .. } => self.grid_sessions().get(*focus).map(|s| s.id),
            _ => None,
        };
```

然后把函数末尾那段夹取整个换掉：

```rust
        // 焦点是身份，不是位置。会话增删会让格子整体平移，只夹取的话
        // 焦点会静默指到别的会话上 —— 而 `i` 的收件人、`Enter` 放大的
        // 那一格、`s`/`u` 作用的对象全都取自它，后两个不可撤销。
        // 锚点找不回来（那个会话真没了）才退回夹取。
        let visible_ids: Vec<u32> = self.grid_sessions().iter().map(|s| s.id).collect();
        let grid_last = visible_ids.len().saturating_sub(1);
        if let View::Grid { focus, .. } = &mut self.view {
            let clamped = (*focus).min(grid_last);
            *focus = grid_anchor
                .and_then(|id| visible_ids.iter().position(|x| *x == id))
                .unwrap_or(clamped);
        }
```

（`clamped` 先算出来再赋值，是为了绕开借用检查器 —— 闭包里再读 `*focus` 会跟 `&mut` 撞上。）

- [ ] **Step 4: 跑测试**

```bash
cargo test --lib ui::app
```

预期：新测试 PASS，`refresh_rows_clamps_the_grid_focus_into_the_new_range` 仍然 PASS
（它的旧断言在锚定下答案不变：焦点原本在 5 号身上，5 号在新列表里是第 1 格）。

- [ ] **Step 5: 全量跑一遍**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

预期：全绿。

- [ ] **Step 6: 提交**

```bash
git add src/ui/app.rs
git commit -m "fix: the grid focus stays on the session it was on, not the slot

A finished session drops out of grid_sessions(), every tile after it shifts
left, and the focus index silently lands on a different session. The reply
box addressed by 'i' takes its recipient from that index, so a message meant
for one agent went to another. Stop, roll back, and zoom read the same index.

The board list has anchored its cursor by identity since it was written;
the grid never did."
```

---

