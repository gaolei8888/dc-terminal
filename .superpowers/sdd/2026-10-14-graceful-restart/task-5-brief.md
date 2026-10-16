### Task 5: 排空期间，自动来源不往会话里打字

**Files:**
- Modify: `src/session.rs`（`SessionManager` 加 `draining: AtomicBool` 和 `set_draining(bool)` / `is_draining()`）
- Modify: `src/bridge.rs`（`submit` :214）
- Modify: `src/mesh/deliver.rs`（`Inbox` trait :318、`LocalInbox` :341、`tick` :834）
- Modify: `src/proto.rs`（`ErrorCode::RestartPending`）、`src/i18n.rs`（它的中文/英文说法）

**Interfaces:**
- Produces:
  - `SessionManager::set_draining(&self, on: bool)`、`SessionManager::is_draining(&self) -> bool`
  - `bridge::submit` 在 `is_draining()` 时返回 `coded(ErrorCode::RestartPending)`，不写任何字节。
  - `pub(crate) fn bridge::submit_unchecked(mgr, id, text) -> anyhow::Result<()>`：原来 `submit` 的本体，排空自己发话和重启后投递用。
  - `Inbox::draining(&self) -> bool`（默认 `false`；`LocalInbox` 返回 `self.mgr.is_draining()`）。

- [ ] **Step 1: 写失败的测试**

`bridge.rs` 测试模块：

```rust
#[test]
fn submit_refuses_while_draining_and_writes_nothing() {
    let (mgr, id) = /* 本文件已有的建会话辅助，起一个 shell 会话 */;
    mgr.set_draining(true);
    let e = submit(&mgr, id, "hello").unwrap_err();
    assert!(matches!(e.downcast_ref::<CodedError>(), Some(CodedError(ErrorCode::RestartPending))));
    // 屏幕上没有 hello
    mgr.set_draining(false);
    submit(&mgr, id, "echo ok").unwrap();
}

#[test]
fn submit_unchecked_ignores_draining() {
    let (mgr, id) = /* 同上 */;
    mgr.set_draining(true);
    submit_unchecked(&mgr, id, "echo ok").unwrap();
}
```

`mesh/deliver.rs` 测试模块（已有假 `Inbox`，约 :925）：给假 `Inbox` 加一个 `draining` 开关，测：

```rust
#[test]
fn tick_keeps_queue_while_draining_and_delivers_after() {
    // 排一条留言给一个 Idle 会话；inbox.draining = true；tick → 没有 type_into 调用，队列里还在；
    // draining = false；tick → 送达一次。
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge:: mesh::deliver::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

```rust
pub(crate) fn submit(mgr: &crate::session::SessionManager, id: u32, text: &str) -> anyhow::Result<()> {
    // 准备重启时，手机、语音这些自动来源一律不打：它们没有队列，当场
    // 告诉对方「稍后再发」比悄悄吞掉强。排空自己要说的话走 `submit_unchecked`。
    if mgr.is_draining() {
        return Err(crate::proto::coded(crate::proto::ErrorCode::RestartPending));
    }
    submit_unchecked(mgr, id, text)
}
```

`mesh::deliver::tick` 开头：

```rust
    // 准备重启时不取队列：留言原样留着，取消或者重启之后照常送。
    if lock(mesh).inbox.as_ref().is_some_and(|i| i.draining()) {
        return;
    }
```

`ErrorCode::RestartPending` 的用户说法：「dct 正在准备重启，稍后再发」。核对手机桥（Telegram）和语音把 `submit` 的错误转成给用户的话的那一处，确认这句能到用户眼前（不行就在那里补一个分支）。若 `ErrorCode` 有「必须穷举」的匹配（i18n、HTTP 状态映射），一并补上。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/
git commit -m "feat(drain): while dct prepares to restart, phone and voice are told to retry later and mesh messages stay queued"
```

---

