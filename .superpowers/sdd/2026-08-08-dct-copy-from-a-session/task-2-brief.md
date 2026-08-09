### Task 2: `F4` 复制模式

**Files:**
- Modify: `src/ui/attach.rs`（`F4` 分支；离开会话时复位）

**Interfaces:**
- Consumes: Task 1 的 `App.copy_mode`
- Produces: 无新公开接口

- [ ] **Step 1: 写失败的测试**

追加到 `src/ui/attach.rs` 的 `mod tests`（若该文件的测试模块里还没有构造 `App` 的助手，用 `App::test_app()`）：

```rust
fn attached_app() -> (App, tempfile::TempDir) {
    let (mut app, d) = App::test_app();
    app.view = View::Attached(1);
    (app, d)
}

#[test]
fn f4_toggles_copy_mode_and_is_never_forwarded_to_the_agent() {
    let (mut app, _d) = attached_app();
    assert!(!app.copy_mode);

    handle_key(&mut app, key(KeyCode::F(4))).unwrap();
    assert!(app.copy_mode, "第一下打开");

    handle_key(&mut app, key(KeyCode::F(4))).unwrap();
    assert!(!app.copy_mode, "第二下关掉");

    // F4 是 dct 自己吃掉的键，一个字节都不能落进 agent 的输入
    assert_eq!(super::super::key_to_input(&key(KeyCode::F(4))), None);
}

/// 复制模式是「此刻正在复制」的临时状态，不是配置。**进会话时复位**——
/// 不管上一个会话是怎么离开的（F2、Ctrl+Q、agent 自己退出），下一个会话
/// 一定从「鼠标归 agent」开始。
///
/// 在**进入**这一侧复位，而不是在三条离开的路上各写一次：`enter_session`
/// 是所有进会话路径的唯一漏斗（看板 Enter、九宫格 Enter、F3 都走它），
/// 而离开有三条路，其中 Ctrl+Q 那条走的是 `back_one_level`——一个所有视图
/// 共用的纯函数，为这一个字段改它的签名不值。漏斗上写一次，结构上就漏不掉。
#[test]
fn entering_a_session_always_starts_outside_copy_mode() {
    let (mut app, _d) = App::test_app();
    app.copy_mode = true;

    super::super::enter_session(&mut app, 1);

    assert!(!app.copy_mode, "上一个会话的复制模式不能粘到下一个会话");
}
```

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib ui::attach::tests`
Expected: FAIL，`no field 'copy_mode'` 已经在 Task 1 解决，这里会是 `F4` 没有被处理（`copy_mode` 仍为 `false`）以及 `leave_session` 不存在。

- [ ] **Step 3: 在进会话的漏斗上复位**

`src/ui/mod.rs::enter_session`。一行，加在已有的 `app.explained_failure = None;` 旁边：

```rust
    // 上一个会话的复制模式不能粘到这一个来。**在「进入」这一侧复位**，
    // 不在三条离开的路上各写一次：`enter_session` 是所有进会话路径的唯一
    // 漏斗（看板 Enter、九宫格 Enter、F3 都走它），而离开有三条路，其中
    // Ctrl+Q 那条走的是 `back_one_level`——一个所有视图共用的纯函数，
    // 为这一个字段改它的签名不值。漏斗上写一次，结构上就漏不掉。
    //
    // 留在看板上的那个 `copy_mode` 是无害的：`wants_mouse_capture` 的第一个
    // 条件就是「贴在会话里」，不在会话里时它压根不参与判断。
    app.copy_mode = false;
```

**不要**去重构 `attach::handle_key` 的 `F2` 分支、主循环里 `session_ended_notice` 之后那一段、或者 `back_one_level` 的落点。那三处各自还做着别的事（设消息、清 `explained_failure`、`sent_size = None`），为这一个 `bool` 把它们收成一个函数，风险远大于收益。

- [ ] **Step 4: 加 `F4` 分支**

`src/ui/attach.rs::handle_key`，插在 `F3` 分支之后、`key_scroll` 之前：

```rust
    } else if key.code == KeyCode::F(4) {
        // F4 = 复制模式：临时把鼠标交还给终端，用终端自己的拖选去复制。
        // 挑 F4 沿用 F2/F3 的理由：没有 CLI agent 在用 F 功能键，偷它不踩
        // 任何人，也不用搞双击透传那种隐形状态。
        //
        // 这里只翻转状态，真正开关鼠标在主循环里统一做（见
        // `mod.rs::wants_mouse_capture`）——在这儿直接 execute! 的话，
        // 就有两处在写同一个终端状态，而它们对「现在开着没有」的记忆会分叉。
        app.copy_mode = !app.copy_mode;
    } else if let Some(action) = key_scroll(
```

`key_to_input` 不用改：它的通配臂对所有 `KeyCode::F(_)` 返回 `None`，F4 天然不会被转发。上面那条断言把这件事钉住，免得以后有人给 F 键加编码时不小心让它开始转发。

- [ ] **Step 5: 跑测试，确认通过**

Run: `cargo test`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui/attach.rs src/ui/mod.rs
git commit -m "feat: F4 hands the mouse back to the terminal so you can select and copy"
```

---

