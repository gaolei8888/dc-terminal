### Task 8: `dct restart` 新流程

**Files:**
- Modify: `src/cli.rs`（`Restart` 参数解析、`run_restart` :386）
- Modify: `src/i18n.rs`（新 `Key`）
- Modify: `src/main.rs`（`restart` 参数解析处，若在 main 里）

**Interfaces:**
- Consumes: Task 6 的三条请求。
- Produces: `Restart` 参数变为 `{ ask: bool, now: bool, wait_s: u64, nudge_wait_s: u64 }`（`-y` → `ask=false`；`--now`；`--wait <分钟>`；`--nudge-wait <分钟>`；坏参数 → `Usage`）。

流程：
1. 没有后台 → 照旧 `StartFresh`。
2. `ask` → 照旧列会话、问 y/n。说明文字改成：「会先等正在干活的会话停下（最多 N 分钟），再重启；重启后每个会话接回原来的对话。」`--now` 时用旧说明。
3. 发 `Drain { wait_s, nudge_wait_s, initiator }`，`initiator` 取环境变量 `DCT_SESSION_ID`（`session::SESSION_ID_ENV`）解析成 `u32`；`--now` 时 `wait_s = nudge_wait_s = 0`。
4. **旧后台回退（Review Focus 1）**：先 `Hello` 拿协议号；`< 25` → 打印「这次后台太旧，不会等会话停下，直接重启」，走原来的 `restart_daemon`。
5. 每秒发 `DrainStatus { renew: true }`，打印进度一行（同一行刷新：`\r` + 清行），例如「还在等 2 个会话（dc-octo、dc_llm），剩 9 分 12 秒」，兜底阶段说「已请它们停下，最多再等 2 分 40 秒」。会话名字从 `List` 里取，取不到用编号。
6. Ctrl+C：装 `SIGINT` 处理（看 `sys::signal` 现有的封装能不能用于非 raw 模式；不能就用 `AtomicBool` + `libc::signal`，Windows 用 `SetConsoleCtrlHandler`——先 grep 仓库里 CLI 有没有现成的 Ctrl+C 处理），收到就发 `DrainCancel`，打印「已取消，一切照旧」，退出 0。
7. `done: true` → 打印「正在换新的后台……」，用 `wait_up_to(20s)` 等新后台 `Hello` 协议号对上；成功打印 `RestartDone`，失败打印 `RestartFailed`（附一句「看 ~/.dct/daemon.log」）。
8. 若命令是在 dct 会话里敲的（有 `DCT_SESSION_ID`），第 7 步这个进程大概率会随后台退出而被杀掉——这是预期的，`__swap` 会完成剩下的事。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn parse_restart_args() {
    assert_eq!(parse_restart(&[]), Restart::Run { ask: true, now: false, wait_s: 900, nudge_wait_s: 180 });
    assert_eq!(parse_restart(&["-y", "--wait", "5"]), Restart::Run { ask: false, now: false, wait_s: 300, nudge_wait_s: 180 });
    assert_eq!(parse_restart(&["--now"]), Restart::Run { ask: true, now: true, wait_s: 0, nudge_wait_s: 0 });
    assert!(matches!(parse_restart(&["--wait", "abc"]), Restart::Usage(_)));
}

#[test]
fn progress_line_names_sessions_and_time() {
    let p = Progress { waiting: vec![2, 9], remaining_s: 552, nudged: false };
    let names = HashMap::from([(2, "dc-octo".to_string())]);
    assert_eq!(progress_line(&p, &names, Lang::Zh), "还在等 2 个会话（dc-octo、9 号），剩 9 分 12 秒");
}

#[test]
fn old_daemon_falls_back_to_plain_restart() {
    // 抽出 `fn plan_for(protocol: Option<u32>) -> Plan { Graceful, Plain }`：
    // Some(24) → Plain；None → Plain；Some(25) → Graceful。
}
```

（`Restart` 原来是 `Usage/Ask/Yes` 三态，改形状时把原有测试一起改；类型名以源码为准。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib cli::`
Expected: 编译失败。

- [ ] **Step 3: 实现** 上面的流程和三个可测函数（`parse_restart`、`progress_line`、`plan_for`）。i18n 新增 key 只写中文和英文两份（其他语言照仓库现有做法：看 `i18n.rs` 是否要求每个 key 都有全语言）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib cli::`，再 `cargo test`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/
git commit -m "feat(restart): dct restart waits for sessions to stop with live progress and Ctrl+C cancel; --now, --wait and --nudge-wait; falls back on old daemons"
```

---

