### Task 6: 后台执行排空：协议、线程、落盘、交给小进程、退出

**Files:**
- Modify: `src/proto.rs`（`Request` :309、`Response` :717、`PROTOCOL_VERSION` :145）
- Modify: `src/daemon.rs`（`handle` :939；新增排空线程）
- Modify: `src/client.rs`（新增 `spawn_swap`，见 Task 7 的 `__swap`）

**Interfaces:**
- Consumes: Task 2 的 `persist_last_sessions_with`、`conversation_of`；Task 4 的 `Drain`/`Action`/`Progress`/`NUDGE_TEXT`/`handoff_text`；Task 5 的 `set_draining`、`submit_unchecked`。
- Produces（协议 25）：

```rust
// Request
Drain { wait_s: u64, nudge_wait_s: u64, initiator: Option<u32> },
/// `renew = true` 只有发起重启的 CLI 发（续租约）；界面画横幅时发 `false`，不续。
DrainStatus { renew: bool },
DrainCancel,
// Response
DrainProgress { progress: crate::drain::Progress, done: bool },
```

  - `Drain` 已在排空时再发一次：返回当前进度，不重新开始。
  - `DrainStatus { renew: true }`：返回进度，同时刷新「CLI 还在」的租约。超过 10 秒没收到 `renew: true` → 取消。`renew: false` 只读不续。
  - `DrainCancel`：取消，`set_draining(false)`，返回 `Ok`。
  - 没在排空时 `DrainStatus` 返回 `DrainProgress { done: false, progress: 空 }`——CLI 据此知道被取消了（见 Task 8）。
  - `done: true` 表示已经落盘、`__swap` 已起，后台马上退出；CLI 收到后去等新后台。

- [ ] **Step 1: 写失败的测试**（`daemon.rs` 测试模块；照已有的 `run_with_manager` + 临时 socket 测试的写法，用 Task 2 那种回显参数的测试 profile，再加 `busy_pattern = "BUSY"`、`idle_pattern = "IDLE"`，命令是一个小 shell 脚本：读一行输入就打印 BUSY、睡 1 秒、再打印 IDLE）

```rust
#[test]
fn drain_with_all_idle_persists_ids_and_reports_done() {
    // 起两个会话，等它们 Idle；发 Drain{wait_s:5,nudge_wait_s:2,initiator:None}；
    // 轮询 DrainStatus 直到 done；读 last-sessions.toml：两条都有 conversation_id，interrupted 都是 false。
    // 测试模式下不真的起 __swap、不退出进程（见 Step 3 的 `DrainHooks`）。
}

#[test]
fn drain_cancel_restores_normal_input() {
    // 发 Drain；发 DrainCancel；mgr.is_draining() == false；DrainStatus 返回 done:false 且 waiting 为空。
}

#[test]
fn drain_lease_expires_without_status_polls() {
    // 用可注入的时钟或把租约改小（DrainHooks.lease = 200ms）；发 Drain 后不再发 DrainStatus；
    // 等 500ms，mgr.is_draining() == false。
}

#[test]
fn drain_nudges_busy_session_and_marks_it_interrupted() {
    // 一个会话一直 BUSY（脚本不打印 IDLE）；Drain{wait_s:1,nudge_wait_s:1}；
    // 等 done；屏幕上出现 NUDGE_TEXT 的开头；last-sessions.toml 里它 interrupted = true。
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib daemon::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

- `PROTOCOL_VERSION` 改 25，注释写「25：平稳重启（Drain/DrainStatus/DrainCancel）」。
- 后台持有 `Arc<Mutex<Option<DrainRun>>>`，`DrainRun { drain: Drain, last_poll: Instant }`。
- 收到 `Drain`：若已有就返回进度；否则从 `mgr.list()` 建 `Member`（`resumable` = 该会话 profile 的 `resumes_by_id()`，需要 `SessionInfo` 或 `SessionManager` 提供 profile 能力——加一个 `SessionManager::resumes_by_id(id) -> bool`），`handoff_dir = <socket 所在目录>/handoffs`（`create_dir_all`），`mgr.set_draining(true)`，起一个线程每 500ms：
  1. 取各会话状态 → `drain.step(now, &states)`；
  2. `AskHandoff` → `submit_unchecked(&mgr, id, &handoff_text(&path))`；`Nudge` → `submit_unchecked(&mgr, id, NUDGE_TEXT)`；发话失败只记 stderr，不中断；
  3. 租约过期 → 取消（同 `DrainCancel`）并结束线程；
  4. `Finish` → `mgr.persist_last_sessions_with(&interrupted, &handoffs)`，然后调 `hooks.after_finish()`。
- `DrainHooks`（trait 或函数指针结构体，`run()` 里装真的、测试装假的）：真的 `after_finish` = `crate::client::spawn_swap(std::process::id(), socket, current_exe)` 成功后 `std::process::exit(0)`；若 `spawn_swap` 失败：写 stderr，`set_draining(false)`，把 run 清掉，**不退出**（宁可不重启也不能把所有会话杀了却起不来新的）。测试的假 hooks 记一下「被调用了」。
- **界面横幅**（spec 第 3 节第 1 步）：界面（`src/ui/` 里每帧发 `List` 的地方，先 grep `Request::List`）每秒发一次 `DrainStatus { renew: false }`，有排空在跑就在顶上画一行「dct 准备重启：还在等 N 个会话」（新 `Key`）；后台是旧协议、不认这条请求时安静地不画。补一条测试：`renew: false` 的轮询不会让租约续上（照 `drain_lease_expires_without_status_polls`，期间每 100ms 发一次 `renew: false`，租约照样过期）。
- **落盘之后到退出之前不许再落盘**：`persist_last_sessions` 在 `is_draining()` 且已 finish 时直接返回（加一个 `frozen: AtomicBool`），防止会话被 SIGHUP 时 tick 把 `interrupted` 冲掉。为此补一条测试：finish 后调一次 `mgr.persist_last_sessions()`，文件内容不变。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/
git commit -m "feat(daemon): Drain request runs the graceful-restart state machine, persists ids and interruptions, then hands over to a detached swapper (protocol 25)"
```

---

