# Task 5 报告：协议与守护进程

## 结论

严格按 brief 步骤 1-8 执行，代码与 brief 给出的逐字内容完全一致（仅经 `cargo fmt` 重新排版），未做任何设计偏离。全部测试通过，`cargo fmt --check` 通过，已提交。

## 步骤与命令记录

### Step 1：先写失败的集成测试

创建 `tests/daemon_roundtrip.rs`，内容与 brief 逐字一致（两个测试：`daemon_serves_create_list_and_stop`、`unknown_session_returns_error_not_panic`）。

此时仓库尚未拆出 `lib.rs`，`Cargo.toml` 已含 `tempfile = "3"` 作为 dev-dependency，未新增依赖。

### Step 2：跑测试确认失败

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --test daemon_roundtrip
```

输出（关键部分）：

```
error[E0433]: cannot find module or crate `dct` in this scope
error[E0433]: cannot find module or crate `dct` in this scope
error[E0433]: cannot find module or crate `dct` in this scope
error[E0433]: cannot find module or crate `dct` in this scope
error: could not compile `dct` (test "daemon_roundtrip") due to 4 previous errors
```

符合 brief 预期："编译失败，找不到 crate `dct` 的 `client`/`proto`/`daemon`"。

### Step 3：拆出 lib.rs

- 新建 `src/lib.rs`，内容与 brief 一致：`pub mod client; pub mod daemon; pub mod git; pub mod profile; pub mod proto; pub mod pty; pub mod session; pub mod ui;`
- 新建占位 `src/ui.rs`，只有一行注释 `// Task 6 实现`（Task 6 会填内容，本任务不越界实现 UI）。
- `src/main.rs` 改为 brief 给的最简版本：

```rust
fn main() -> anyhow::Result<()> {
    println!("dct");
    Ok(())
}
```

原 `main.rs` 里的 `mod git; mod profile; mod pty; mod session;` 声明去掉了，因为这些模块现在由 `lib.rs` 统一声明，二进制 target 通过依赖同一 crate 的 lib 部分（Cargo 默认行为：同名 lib+bin 时 bin 可以 `use dct::...`，但当前 `main.rs` 还不需要引用任何模块，故未加 `use`）。

### Step 4-6：实现 proto.rs / daemon.rs / client.rs

三个文件的内容与 brief 给出的代码逐字一致，没有改动任何签名、字段名、错误文案。`Profile`、`SessionManager`、`FileStat`、`SessionInfo` 的字段/方法签名与仓库里已有的 `src/profile.rs`、`src/session.rs`、`src/git.rs` 核对过，完全匹配（`SessionManager::new/register_profile/create/list/send_input/screen/stop/undo/diff/tick`，`Profile::builtin_names()`，均无需改动即可编译通过）。

### Step 7：跑测试确认通过

```
cargo build
```
输出：`Finished \`dev\` profile [unoptimized + debuginfo] target(s) in ...`，无警告、无错误。

```
cargo test --test daemon_roundtrip -- --test-threads=1
```
输出：

```
running 2 tests
test daemon_serves_create_list_and_stop ... ok
test unknown_session_returns_error_not_panic ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
```

全量：

```
cargo test -- --test-threads=1
```
输出摘要：

```
running 23 tests   (src/lib.rs 单元测试：git/profile/pty/session 四个模块)
test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

running 0 tests    (src/main.rs 单元测试，本来就没有)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

running 2 tests    (tests/daemon_roundtrip.rs 集成测试)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

Doc-tests dct: 0 passed
```

拆分 `lib.rs` 没有破坏既有四个模块（git/profile/pty/session）的单元测试可见性——它们本来就在各自模块文件内用 `#[cfg(test)] mod tests { use super::*; ... }`，随模块一起迁到 lib.rs 下依旧可见，未做任何可见性/签名修改。

### fmt 与格式检查

```
cargo fmt
cargo fmt --check   # 通过，无输出
git diff --check    # 通过，无输出（无尾随空白/冲突标记）
```

`cargo fmt` 对 `src/daemon.rs`、`src/client.rs`、`tests/daemon_roundtrip.rs` 做了重新换行（把一些单行的链式调用拆成多行），内容语义未变。这是预期的，因为 brief 里贴的代码是手写排版，不保证是 rustfmt 输出。格式化后重跑了一次全量 `cargo test -- --test-threads=1`，仍然全部 25 个测试（23 单元 + 2 集成）通过。

## 与 brief 的偏差

无实质偏差。唯一的“非逐字”之处是 `cargo fmt` 对 Step 5/6/1 给出的代码做了自动排版调整（换行位置），这是 brief 第 8 步明确要求的动作（提交前必须 `cargo fmt`），不算偏离。

`main.rs` 里没有保留原来的 `mod xxx;` 声明，改成了 brief 给的最简版本（无 `use dct::...`），因为 brief Step 3 明确给出了这个精确内容，且当前 `main` 不需要引用任何模块。

## 自查

- `git status`：只有 `src/main.rs` 被修改，`src/client.rs`、`src/daemon.rs`、`src/lib.rs`、`src/proto.rs`、`src/ui.rs`、`tests/` 是新增未跟踪文件，符合预期改动范围（没有动 `profile.rs`/`git.rs`/`pty.rs`/`session.rs`）。
- 未新增任何 Cargo 依赖，`tempfile` 已在 `[dev-dependencies]` 中。
- 协议按行分隔 JSON：`daemon.rs::serve` 用 `BufReader::lines()` 逐行读，`client.rs::call` 用 `writeln!` + `read_line`，与设计意图一致（协议自足，不依赖 TUI 状态）。
- 错误处理：`handle()` 把所有 `Result` 错误统一 `unwrap_or_else` 成 `Response::Error(e.to_string())`，未知会话 ID 会经 `SessionManager::get`/`get_mut` 里的 `anyhow!("没有这个会话: {id}")` 转成中文错误消息返回给客户端，集成测试 `unknown_session_returns_error_not_panic` 验证了这一点，未 panic、未断连。
- 未执行 `cargo clippy`（brief 未要求），仅执行了 brief 明确列出的命令。

## 提交

```
git add src/ tests/
git commit -m "feat: 守护进程、Unix socket 协议与客户端"
```

---

## 审查回归修复（第二轮）

审查发现 1 个 Critical + 2 个 Important，均已修复。下面按问题编号记录做了什么、跑了什么命令、实际输出是什么。

### 1. [Critical] `handle()` 持锁跨越慢操作

**采用路线 A（推荐路线）**：`SessionManager` 从"外面套 `Arc<Mutex<SessionManager>>`、方法 `&mut self`"改成"自己内部可变、方法全部 `&self`、以 `Arc<SessionManager>` 共享"。

具体设计（`src/session.rs`）：

- `next_id: u32` → `next_id: AtomicU32`，用 `fetch_add(1, Ordering::SeqCst)` 原子分配 id。刻意不是"锁内 peek、成功了才自增"的旧语义——旧语义在多线程下会导致两个并发 `create()` 拿到同一个 id、争抢同一个 worktree 分支名；`fetch_add` 保证每次分配都独一无二，代价是失败的 `create()` 会白白消耗一个 id（可接受，测试也不依赖 id 连续）。
- `sessions: HashMap<u32, Session>` → `sessions: Mutex<HashMap<u32, Arc<Mutex<Session>>>>`。两层锁：外层只用来"查一次/插一次/列一次" `Arc`，锁的持有时间跟 git 操作耗时无关；内层锁住单个会话自己的可变状态，不同会话之间互不阻塞。
- `create()` 里，`resolve_profile` → 校验目录 → 原子分配 id → **不持任何锁**地做 `create_worktree`/`PtySession::spawn`/`checkpoint`（这些正是审查报告里指出的慢操作）→ 最后才 `recover(self.sessions.lock()).insert(...)`，这一步只做一次 `HashMap` 插入。
- `list()`/`tick()` 用"锁外层拿一份 `Arc` 快照 → 释放外层锁 → 逐个锁内层读字段"的模式，同样不会被某个正在慢操作中的会话拖住（不过要澄清：`create()` 的慢操作发生在会话插入 `sessions` 之前，此时该会话对 `list`/`tick` 根本不可见，谈不上"拖住"；`list`/`tick` 唯一可能等待的是那次插入本身，是个 `O(1)` 的 `HashMap` 操作）。
- `send_input`/`screen`/`stop`/`undo`/`diff` 统一走新增的私有帮助方法 `with_session(id, f)`：查一次外层锁拿到会话的 `Arc`，锁释放后再锁这个会话自己的内层锁执行 `f`。这意味着 `send_input` 里的 `git::checkpoint`（回车时打检查点，也可能因为文件多而慢）虽然本任务的验收标准没有明确要求，但顺手也不再持有全局锁——同一类"跨连接头阻塞"的隐患没有留死角。

`daemon.rs` 对应改动：去掉外层 `Mutex`，`run()`/`run_with_manager()` 直接 `Arc<SessionManager>`；`handle()` 不再 `mgr.lock().unwrap()`，直接 `mgr.list()` / `mgr.create(...)` 等；tick 后台线程也从 `tick_mgr.lock().unwrap().tick()` 简化成 `tick_mgr.tick()`。

**新增的可验证性设施**：`daemon.rs` 加了 `pub fn run_with_manager(socket: &Path, mgr: Arc<SessionManager>) -> Result<()>`，`run()` 现在只是 `run_with_manager(socket, Arc::new(SessionManager::new()))` 的薄封装。这是为了让回归测试能在起daemon之前 `register_profile` 一个测试专用的慢 profile，而不用碰内置的 `claude`/`shell`——**本机确实装了真的 `claude` CLI**（`which claude` → `/Users/lei/.local/bin/claude`），如果测试图省事直接用内置 `claude` profile 去触发慢 `Create`，一旦 `PtySession::spawn` 真的把它拉起来（带 `--dangerously-skip-permissions`），测试进程退出时未必能清理干净，是要极力避免的风险。`run()` 的对外签名/契约完全没变。

**新增回归测试** `tests/concurrency.rs :: list_is_not_blocked_by_slow_create`：

- 真造一个 8000 个文件的 git 仓库（不是用 `sleep()` 假装慢，是真实复现审查报告里"文件多导致 `git worktree add`/`checkpoint` 慢"的场景）。
- 注册一个自定义 profile（`command: ["cat"], is_agent: true`），不依赖任何外部 CLI。
- 用 `run_with_manager` 起 daemon，一个后台线程发 `Create`（会触发慢的 `create_worktree` + `checkpoint`），主线程 sleep 150ms 后（确保这时候 `Create` 还在慢操作里）发 `List` 并计时。
- 断言：`create_elapsed > 300ms`（证明场景确实慢，不是环境凑巧很快导致测试没测到东西）且 `list_elapsed < 100ms`（验收标准）。

**改代码前跑一遍，确认真的会失败**（用 8000 文件仓库 + 上面这套自定义 profile，此时 `daemon.rs` 已经加了 `run_with_manager` 但 `session.rs` 还是旧的 `Arc<Mutex<SessionManager>>` + `&mut self` 设计）：

```
$ cargo test --test concurrency -- --test-threads=1 --nocapture
running 1 test
test list_is_not_blocked_by_slow_create ... create_elapsed=1.058293917s list_elapsed=903.265292ms

thread 'list_is_not_blocked_by_slow_create' (42672720) panicked at tests/concurrency.rs:100:5:
List 被慢 Create 卡住了：耗时 903.265292ms（要求 < 100ms）
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.63s
```

复现数据：`create_elapsed=1.058s`，`list_elapsed=903ms`——`List` 几乎等了整个 `Create` 的时长，跟审查报告的 959ms/878ms 量级一致，坐实了问题。

**改完路线 A 之后，连跑 3 次确认修好且不是偶发**：

```
$ cargo test --test concurrency -- --test-threads=1 --nocapture   # 第 1 次
test list_is_not_blocked_by_slow_create ... create_elapsed=1.056340667s list_elapsed=56.5µs
ok

$ cargo test --test concurrency -- --test-threads=1 --nocapture   # 第 2 次
test list_is_not_blocked_by_slow_create ... create_elapsed=1.232503208s list_elapsed=60.458µs
ok

$ cargo test --test concurrency -- --test-threads=1 --nocapture   # 第 3 次
test list_is_not_blocked_by_slow_create ... create_elapsed=1.266504375s list_elapsed=120µs
ok
```

`cargo fmt` 之后又补跑了 3 次（确认格式化没有改变运行时行为）：

```
=== run 1 (post-fmt) ===
test list_is_not_blocked_by_slow_create ... create_elapsed=1.053669084s list_elapsed=79.167µs
ok
=== run 2 (post-fmt) ===
test list_is_not_blocked_by_slow_create ... create_elapsed=1.520272166s list_elapsed=111.416µs
ok
=== run 3 (post-fmt) ===
test list_is_not_blocked_by_slow_create ... create_elapsed=1.1149045s list_elapsed=50.833µs
ok
```

6 次运行里 `create_elapsed` 全部在 1.05s~1.52s 之间（证明场景稳定复现慢操作），`list_elapsed` 全部在 50~120 微秒量级——比 100ms 的验收线快了三个数量级，不是"压线过"，是"根本不在一个数量级"。

### 2. [Important] Mutex 没有 poison 恢复

在 `session.rs` 里加了一个小工具函数：

```rust
fn recover<T>(r: std::sync::LockResult<T>) -> T {
    r.unwrap_or_else(|poisoned| poisoned.into_inner())
}
```

路线 A 里新增的两把锁（`sessions: Mutex<HashMap<...>>`、`extra_profiles: Mutex<HashMap<...>>`）、以及每个会话自己的 `Mutex<Session>`，所有 `.lock()` 调用统一走 `recover(...)`，全文件搜索确认没有裸 `.lock().unwrap()` 残留（见下面 grep 结果）。`daemon.rs` 因为路线 A 之后不再有任何 `Mutex<SessionManager>`，这一条对它已经不适用（没有锁可中毒）。

```
$ grep -rn "\.lock()" src/ tests/
src/session.rs:76:        recover(self.extra_profiles.lock()).insert(p.name.clone(), p);
src/session.rs:80:        if let Some(p) = recover(self.extra_profiles.lock()).get(name) {
src/session.rs:130:        recover(self.sessions.lock()).insert(id, Arc::new(Mutex::new(session)));
src/session.rs:135:        recover(self.sessions.lock())
src/session.rs:145:        let mut guard = recover(arc.lock());
src/session.rs:151:            recover(self.sessions.lock()).values().cloned().collect();
src/session.rs:156:                let s = recover(s.lock());
src/session.rs:233:            recover(self.sessions.lock()).values().cloned().collect();
src/session.rs:236:            let mut s = recover(s.lock());
src/pty.rs:...                                                    （见下方"未处理的残留"说明）
```

**补了一个构造性验证测试**（跟审查者的做法一样：故意让持锁线程 panic 把锁弄"中毒"，再验证系统还能正常用），`session.rs::tests::recovers_from_poisoned_sessions_lock`：

```rust
let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
    let _guard = m.sessions.lock().unwrap();
    panic!("模拟持锁期间的 panic，用来验证锁中毒后还能恢复");
}));
assert!(result.is_err());

let id = m.create(plain.path(), "shell")
    .expect("锁中毒之后 create() 应该还能正常工作，而不是永远失败");
```

跑出来：

```
$ cargo test --lib session:: -- --test-threads=1 --nocapture
test session::tests::recovers_from_poisoned_sessions_lock ...
thread 'session::tests::recovers_from_poisoned_sessions_lock' (42919692) panicked at src/session.rs:345:13:
模拟持锁期间的 panic，用来验证锁中毒后还能恢复
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
ok
```

（打印的 panic 是被 `catch_unwind` 接住的那个人为制造的 panic，属于测试的一部分，最终测试结果是 `ok`，证明中毒之后 `create()` 依旧成功。）

**未处理的残留（如实报告，不是修复范围）**：`src/pty.rs` 里有 5 处裸 `.lock().unwrap()`（`process_id`/`write`/`screen_text`/`is_alive`/`kill` 内部各自的 `Mutex`）。这是 Task 3 交付的既有模块，审查意见原文明确限定"`daemon.rs` 里的 `lock().unwrap()`"和"路线 A 里 `session.rs` 内部新增的锁"，没有把 `pty.rs` 划进本轮范围，我也没有在未获授权的情况下去动一个已交付模块的既有实现。这是同一类风险（某次 pty 操作里 panic 会让那个会话的 pty 锁永久中毒），但目前局限在单个会话内部，不会像本轮修的 `sessions`/`extra_profiles` 锁那样让*所有*会话的*所有*请求一起瘫痪。建议后续任务单独跟进。

### 3. [Important] shell profile 不校验目录

在 `create()` 最开头（`resolve_profile` 拿到 `profile` 之后，做任何 git/PTY 操作之前）加了统一校验，覆盖 agent 和非 agent 两种 profile：

```rust
if !dir.is_dir() {
    bail!("目录不存在: {}", dir.display());
}
```

放在 `is_agent` 分支之外、对所有 profile 生效，而不是只判断 `!profile.is_agent` 的情况——因为 agent 分支下 `git::is_repo(dir)` 对一个不存在的目录本来就会返回 `false`（`git rev-parse` 在不存在的 cwd 下跑不起来），走的是已有的"不是 git 仓库"报错路径，行为一直是对的；只是 shell（非 agent）分支完全没做这个检查，会返回一个"成功"的 `Created`，实际是个空转僵尸会话。统一放在最前面校验更简单，也不会跟已有的 agent 测试冲突（`rejects_agent_session_outside_repo`/`agent_session_runs_in_worktree_not_main_tree` 等测试用的都是真实存在的临时目录，不受影响）。

**新增测试** `session::tests::rejects_shell_session_with_missing_dir`：

```rust
let m = SessionManager::new();
let missing = std::path::PathBuf::from("/definitely/does/not/exist/dct-test-dir");
let err = m.create(&missing, "shell").unwrap_err().to_string();
assert!(err.contains("目录不存在"), "实际错误: {err}");
```

跑通，见下面全量测试输出。

### 全量验证

```
$ cargo fmt --check
FMT_CHECK_OK
```

```
$ cargo test -- --test-threads=1
running 25 tests   (src/lib.rs：git 6 + profile 6 + pty 4 + session 9，session 里含 Task 4 原有 7 个
                     + 本轮新增 rejects_shell_session_with_missing_dir、recovers_from_poisoned_sessions_lock)
test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.53s

running 0 tests   (src/main.rs)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

running 1 test    (tests/concurrency.rs)
test list_is_not_blocked_by_slow_create ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.59s

running 2 tests   (tests/daemon_roundtrip.rs)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.17s

Doc-tests dct: 0 passed
```

Task 4 原本的 7 个 `session::tests` 全部还在且全绿：`agent_session_runs_in_worktree_not_main_tree`、`rejects_agent_session_outside_repo`、`shell_session_runs_in_place`、`tick_marks_idle_when_pattern_matches`、`undo_restores_last_checkpoint`、`diff_reports_agent_changes`、`stop_marks_stopped`——只是测试体里把 `let mut m = SessionManager::new();` 改成了 `let m = SessionManager::new();`（因为方法签名从 `&mut self` 变成 `&self`），没有改测试的断言逻辑。

```
$ git diff --check
（无输出，exit 0）
```

### 与审查要求的偏差

无实质偏差。唯一主动做的额外决定：

1. 把"原子分配 id"和"目录校验放在所有 profile 前面而不只是非 agent 分支"这两处，审查意见给了方向但没给到逐字实现，是我按"两个并发 create() 不能撞 id"和"代码尽量简单、行为对 agent 分支无影响"这两个判断标准做的选择，已在上面分别说明理由。
2. 新增了 `daemon::run_with_manager` 这个测试专用的注入点，审查意见没有要求但没有它就无法在不依赖真实 `claude` 二进制、也不用 `sleep()` 假装慢的前提下，写出一个确定性复现"文件多导致 git 操作慢"这个真实场景的集成测试。`daemon::run(&Path) -> Result<()>` 的对外契约保持不变。

### 提交

```
git add src/daemon.rs src/session.rs tests/concurrency.rs
git commit -m "fix: SessionManager 内部可变化解决锁粒度问题，补 poison 恢复与目录校验"
```
