### Task 7: `dct __swap`：脱离会话的小进程换后台

**Files:**
- Modify: `src/client.rs`（新增 `spawn_swap`、`swap_daemon`）
- Modify: `src/main.rs`（`Some("__swap")` 分支，不进 `--help`）

**Interfaces:**
- Produces:
  - `pub fn spawn_swap(old_pid: u32, socket: &Path, exe: &Path) -> Result<()>`：用 `sys::proc::spawn_detached`（与 `spawn_daemon` 同法）起 `exe __swap <old_pid> <socket>`。
  - `pub fn swap_daemon(old_pid: u32, socket: &Path, exe: &Path, grace: Duration) -> Result<()>`：等 `old_pid` 退出最多 `grace`（真的是 10 秒），超时 `hard_kill` 再等 2 秒；然后 `spawn_daemon(exe, socket)`；等 `Hello` 版本对上最多 10 秒（**这里比 `restart_daemon` 的 5 秒长**：新后台要先把所有会话接回来才开始应答）。失败往 `<socket 目录>/daemon.log` 追加一行中文。

- [ ] **Step 1: 写失败的测试**（`client.rs`）

```rust
#[test]
fn swap_waits_for_old_process_then_hard_kills_it() {
    // 起一个 `sleep 30` 子进程当「旧后台」；swap_daemon 的等待部分抽成
    // `wait_old_gone(pid, grace) -> bool`（true = 自己走了，false = 被硬杀）单测：
    // grace = 200ms → 返回 false，且进程已不在。
    // 再起一个 `sleep 0.1` → 返回 true。
}
```

（起新后台那一半由 Task 10 的真机验收覆盖。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib client::`
Expected: 编译失败。

- [ ] **Step 3: 实现** `wait_old_gone`、`swap_daemon`、`spawn_swap`；`main.rs` 加：

```rust
        // 平稳重启的最后一步，由旧后台起、脱离任何会话。不给用户敲，不进帮助。
        Some("__swap") => {
            let pid: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let sock = args.get(2).map(PathBuf::from).unwrap_or_else(socket_path);
            let exe = std::env::current_exe()?;
            std::process::exit(match dct::client::swap_daemon(pid, &sock, &exe, Duration::from_secs(10)) {
                Ok(()) => 0,
                Err(_) => 1,
            })
        }
```

注意 `current_exe()`：用户刚把新二进制 `mv` 到 `~/.local/bin/dct` 时，旧后台的 `current_exe` 在 macOS 上仍指向那个路径（文件名没变），起的就是新文件——这是想要的。Linux 上 `current_exe` 可能带 ` (deleted)`：沿用 `restart_daemon` 调用方取 `exe` 的办法（先 grep `current_exe` 看仓库里怎么处理的）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib client::`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/client.rs src/main.rs
git commit -m "feat(restart): detached __swap helper waits for the old daemon to exit and starts the new one"
```

---

