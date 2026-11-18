### Task 9: 重启后投递「继续」和「先读交接」

**Files:**
- Create: `src/resume_notes.rs`（`lib.rs` 加 mod）
- Modify: `src/daemon.rs`（`restore_last_sessions` 收集待投递；投递线程）
- Modify: `src/proto.rs`（`WarningCode::ResumeNoteDropped { dir: String }`）、`src/i18n.rs`

**Interfaces:**
- Consumes: Task 2 的 `RecordedSession.interrupted/handoff/conversation_id`；Task 5 的 `submit_unchecked`。
- Produces:

```rust
pub const CONTINUE_TEXT: &str = "dct 重启完了。请先检查一下你做到哪了（看看文件和改动的状态），再接着做重启前没做完的事。";
pub fn read_handoff_text(path: &Path) -> String; // 「dct 刚重启。你上次的交接在 <路径>，先读它，再接着做。」

pub struct Note { pub id: u32, pub text: String, pub deadline: Instant, pub dir: String }

/// 一条恢复出来的会话该收到什么话：
/// - interrupted 且按 id 接回了 → CONTINUE_TEXT
/// - 有 handoff 且文件存在 → read_handoff_text
/// - 其余 → None
pub fn note_for(record: &RecordedSession, resumed_by_id: bool) -> Option<String>;

/// 每拍：Idle 的投递并移除；过了 deadline 的移除并返回它们（给警告用）。
pub fn step(notes: &mut Vec<Note>, now: Instant, states: &HashMap<u32, SessionState>,
            send: &mut dyn FnMut(u32, &str) -> bool) -> Vec<Note>;
```

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn note_for_rules() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path().join("h.md");
    let mut r = /* 一条 RecordedSession，conversation_id = Some("a") */;
    assert_eq!(note_for(&r, true), None);
    r.interrupted = true;
    assert_eq!(note_for(&r, true).as_deref(), Some(CONTINUE_TEXT));
    assert_eq!(note_for(&r, false), None); // 没接回原对话，叫它「继续」它也不知道继续什么
    r.interrupted = false;
    r.handoff = Some(h.clone());
    assert_eq!(note_for(&r, false), None); // 文件不存在
    std::fs::write(&h, "x").unwrap();
    assert_eq!(note_for(&r, false), Some(read_handoff_text(&h)));
}

#[test]
fn step_waits_for_idle_and_drops_after_deadline() {
    let t0 = Instant::now();
    let mut notes = vec![
        Note { id: 1, text: "a".into(), deadline: t0 + Duration::from_secs(120), dir: "/x".into() },
        Note { id: 2, text: "b".into(), deadline: t0 + Duration::from_secs(120), dir: "/y".into() },
    ];
    let mut sent = vec![];
    let mut send = |id: u32, t: &str| { sent.push((id, t.to_string())); true };
    let states = HashMap::from([(1, SessionState::Working), (2, SessionState::Idle)]);
    assert!(step(&mut notes, t0, &states, &mut send).is_empty());
    assert_eq!(sent, vec![(2, "b".to_string())]);
    let dropped = step(&mut notes, t0 + Duration::from_secs(120), &states, &mut send);
    assert_eq!(dropped.len(), 1);
    assert!(notes.is_empty());
}

#[test]
fn failed_send_keeps_note_for_next_tick() {
    // send 返回 false → 留在 notes 里，下拍再试。
}
```

`daemon.rs`：

```rust
#[test]
fn restore_queues_continue_for_interrupted_sessions_only() {
    // 两条记录，都有 conversation_id，一条 interrupted；恢复后（脚本 profile 一起来就 Idle）
    // 只有 interrupted 那个会话的屏幕上出现 CONTINUE_TEXT 的开头。
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib resume_notes:: daemon::`
Expected: 编译失败。

- [ ] **Step 3: 实现**：`restore_last_sessions` 在每条成功恢复后算 `note_for(record, resumed_by_id)`（`resumed_by_id` = 记录有 `conversation_id` 且 profile `resumes_by_id()`），收集成 `Vec<Note>`（deadline = now + 2 分钟）返回给调用方；`run_with_manager` 有待投递时起一个线程每 500ms `step`，`send` 用 `submit_unchecked`；被丢弃的变成 `WarningCode::ResumeNoteDropped`（走 `set_resume_skips` 那条路顶到界面，说法：「<目录> 的会话重启后一直没准备好，没能叫它继续，请手动告诉它接着做」）。投递完把对应记录的 `interrupted`/`handoff` 清掉——最简单的办法是恢复完成后立刻 `persist_last_sessions()` 一次（新会话的记录本来就不带这两个标记）；核对恢复路径上 `create_resuming` 已经触发了落盘，若是，就不用另加。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/
git commit -m "feat(restart): after a graceful restart, interrupted sessions are told to continue and unresumable ones to read their handoff"
```

---

