## Task 6: 出站 —— tick 投事件、三道门、防抖

**Files:**
- Modify: `src/session.rs`（`tick()`）
- Modify: `src/bridge.rs`（消费队列）
- Test: `src/session.rs` 内

**Interfaces:**
- Consumes: Task 1 的 `Event`/`EventKind`/`debounce`、Task 5 的 `Bridge`
- Produces: `Sessions::set_event_sink(mpsc::Sender<Event>)`、`should_notify(is_agent, first_input_empty, has_channel) -> bool`

- [ ] **Step 1: 写失败测试**

```rust
/// 三道门。第二道是关键：真实 profile（claude/codex/glm/kimi/deepseek/
/// qwen-api）**全都只声明 busy_pattern**，`classify()` 在 busy 串不在屏幕上
/// 时就判 Idle，而刚创建、还停在启动画面上的会话正是这样。没有这道门，
/// **每开一个会话手机就响一次**。
#[test]
fn a_brand_new_session_does_not_page_you() {
    // 是 agent、有渠道，但用户还没说过话
    assert!(!should_notify(true, true, true));
}

#[test]
fn a_plain_shell_never_pages_you() {
    assert!(!should_notify(false, false, true));
}

#[test]
fn no_channel_means_no_page() {
    assert!(!should_notify(true, false, false));
}

#[test]
fn an_agent_you_have_talked_to_pages_you() {
    assert!(should_notify(true, false, true));
}
```

再写一条走完整 tick 的集成测试：造一个假 profile（只有 `busy_pattern`），`create()` 之后立刻 `tick()`，**断言事件队列是空的**。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib session::tests::a_brand_new -- --test-threads=1`
Expected: `cannot find function should_notify`

- [ ] **Step 3: 实现**

`should_notify` 三个条件与；`tick()` 在三处投递事件：

1. 已有的 `was == Working && matches!(next, Idle | Asking)` 分支 → `EventKind::Stopped`
2. 已有的 `next == Failed && was != Failed` 分支 → `EventKind::Failed`
3. 已有的收尸分支（`journal.died(..., Vanished, ...)` 旁边）→ `EventKind::Vanished`

**投递用 `try_send` 语义，队列满了就丢，绝不阻塞 tick。** 防抖状态（每会话上次发送时刻）记在 `Session` 上。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把 `should_notify` 的 `!first_input_empty` 那一项去掉 —— `a_brand_new_session_does_not_page_you` 和那条 tick 集成测试**都**必须失败。把三个条件的 `&&` 改成 `||`，至少两条测试必须失败。

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/session.rs src/bridge.rs
git commit -m "feat: page the user when an agent stops, fails or dies

The transition that auto-naming already hangs on is the same one worth
sending to a phone, so there is no new detection here -- just a second
consumer and a queue.

The gate that matters is the one on first_input: every real profile declares
only busy_pattern, so a session still sitting on its splash screen reads as
'finished a round of work'. Without that gate your phone buzzes every time
you open a session.

tick never blocks on the queue. A full queue drops the event; a slow send
would freeze the daemon, and a frozen dct looks exactly like a dead agent."
```

---

