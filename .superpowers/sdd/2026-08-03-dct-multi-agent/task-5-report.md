# Task 5 报告：环境变量注入到 PTY

## 实现内容

1. **`src/pty.rs`**
   - `PtySession::spawn` 签名新增 `env: &BTreeMap<String, String>` 形参，在 `builder.cwd(cwd)` 之后用 `builder.env(k, v)` 逐条加进去（只加不减，继承环境不会被清空）。
   - spawn 失败的错误上下文从 `"启动 {} 失败"` 改成 `"启动不了 {}，它可能装坏了"`。
   - 四处既有测试调用点第二个参数改传 `&Default::default()`，并新增 `spawn_passes_env_to_the_child` 测试。

2. **`src/session.rs`**
   - `SessionManager::create` 签名新增 `secrets: &SecretStore` 形参。
   - 组合规则：`profile.env` 打底，若 `profile.secret` 存在且 `secrets.get(&profile.name)` 有值，则把密钥写进 `env[spec.env]`；**密钥缺失不报错**，静默跳过（可用性判定留给后续任务）。
   - `Session` 结构体新增 `busy_re: Option<regex::Regex>` 字段，构造时用 `profile.busy_regex()?` 编译并填入；标了 `#[allow(dead_code)]`，因为本任务不实现读取它的 tick 逻辑（那是 Task 6 的范围）。
   - 新增 `SessionManager::screen_text_for_test`（`#[cfg(test)]`），直接读会话的 `pty.screen_text()`。
   - 新增 `empty_secrets()` 测试辅助（指向从未写过的路径，`SecretStore::load` 视为空、不是错误），用来给九处既有 `create(...)` 调用补上第三个参数，不改变它们原本要验证的行为。
   - 按 brief 原文加入三个新测试：`create_injects_the_secret_into_env`、`create_without_the_secret_still_starts`、`spawn_failure_says_what_to_do_not_enoent`。

3. **`src/daemon.rs`**
   - 新增 `use crate::secrets::{secrets_path_for_socket, SecretStore};`。
   - `run_with_manager` 里仿照 `store` 的模式，`SecretStore::load(&secrets_path_for_socket(socket))` 包进 `Arc<Mutex<_>>`，随每个连接 `clone()` 进 `serve`。
   - `serve` / `handle` 各加一个 `secrets: Arc<Mutex<SecretStore>>` / `&Arc<Mutex<SecretStore>>` 形参，`Request::Create` 分支里 `recover(secrets.lock())` 拿到 guard 后传给 `mgr.create(&dir, &profile, &secrets_guard)`。
   - 没有新增任何 `Request`/`Response` 变体——密钥的增删改（Task 8）还是没有入口，这一步只是把已有 `SecretStore` 接进 `create` 的读路径。

4. **`tests/slow_input.rs`**（brief 未列出，但不改会导致编译失败）
   - `SessionManager::create` 签名变了，这个集成测试直接调用它，补了一个从临时目录 load 出来的空 `SecretStore` 传进去。行为不变（该测试本来就不关心密钥）。

## 测试与结果

### TDD 证据

**RED**（Step 2）：先只加测试代码，不改产品代码，跑
```
env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo test --lib
```
结果：编译失败，`E0061`，报的正是预期的「参数个数不对」——`spawn` 少了 `env` 形参、`create` 少了 `secrets` 形参：
```
error[E0061]: this method takes 3 arguments but 2 arguments were supplied
error[E0061]: this method takes 2 arguments but 3 arguments were supplied
   --> src/session.rs:508:20 (create_injects_the_secret_into_env 等新测试)
error: could not compile `dct` (lib test) due to 26 previous errors
```
这确认了失败的原因是接口还没实现，不是测试写错了。

**GREEN**（Step 3/4/5 实现完之后）：
```
env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo test
```
结果：全部通过，`src` 单元测试 98 passed，各集成测试文件均 passed（含新加的 `pty::tests::spawn_passes_env_to_the_child`、`session::tests::create_injects_the_secret_into_env`、`session::tests::create_without_the_secret_still_starts`、`session::tests::spawn_failure_says_what_to_do_not_enoent`）。

```
env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo build --all-targets
```
无警告，干净编译。

```
~/.cargo/bin/cargo fmt && git diff --check
```
两条都无输出，格式和空白都干净。

## `format!("{e}")` vs `format!("{e:#}")` 的调查结果

`src/daemon.rs` 里 `handle()` 末尾的错误转换是：
```rust
r.unwrap_or_else(|e| Response::Error(e.to_string()))
```
`anyhow::Error` 的 `Display`（`.to_string()` 走的就是这个）只打印最外层的 context 消息，不会像 `{:#}`（alternate 格式）那样把整条 `Caused by:` source 链吐出来。这一点在 `spawn_failure_says_what_to_do_not_enoent` 测试里也间接验证了：`mgr.create(...)` 返回的 `anyhow::Error` 调 `.to_string()` 之后，既包含「启动不了」，又确认不包含（大小写不敏感）`"enoent"`——如果 `.to_string()` 真的把 source 链带出来，这条测试会失败，因为 `spawn_command` 底层 io error 的 message 里就有 `No such file or directory (os error 2)`（在部分平台/场景可能出现 ENOENT 字样或等价内容）。

**结论：现有代码已经是对的（`e.to_string()` 等价于 `format!("{e}")`），没有改动这一段。**

## 文件改动

- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/pty.rs`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/session.rs`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/daemon.rs`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/tests/slow_input.rs`（非 brief 列出的文件，但是既有的 `SessionManager::create` 调用点，签名变了必须跟着改，否则整个 workspace 编译不过）

提交：`405c6ee feat: profile 的 env 与密钥注入子进程`

## 自查发现

- **密钥泄露排查**：`Session`（内部结构体，含 `profile: Profile`）和 `PtySession` 都没有 `#[derive(Debug)]`，走公网协议的 `SessionInfo` 只有 `profile: String`/`dir`/`state`/`activity`，不含 env 或密钥。全仓库 grep `{:?}`、`eprintln!`、`println!`，daemon.rs 唯一一处日志是连接层错误 `"连接处理失败: {e}"`，跟密钥无关。`create()` 里组装的 `env`（含密钥值）只流向 `PtySession::spawn` 的 `builder.env()`，不会被打印或写进错误消息。结论：密钥没有意外出现在 Debug 输出、错误消息或日志行里。
- **`create` 不因缺密钥而拒绝启动**：`create_without_the_secret_still_starts` 测试专门验证了这一点，实现里用 `if let Some(key) = secrets.get(...)` 静默跳过，不 `bail!`。
- **没有超范围实现**：`busy_re` 字段加了但没有任何代码读它（`#[allow(dead_code)]` 标注原因），tick 逻辑分毫未动；`daemon.rs` 没有新增 `Request`/`Response` 变体，密钥的存取接口留给 Task 8。

## 遗留的顾虑（未在本任务修复，值得记录）

`daemon.rs::handle()` 里，`Request::Create` 分支对 `secrets` 的锁是这样拿的：
```rust
let secrets_guard = recover(secrets.lock());
let r = mgr.create(&dir, &profile, &secrets_guard)...
```
这把 `secrets` 的锁**持有到 `mgr.create()` 整个调用结束**——包括 PTY spawn 和（agent profile 时）`git::checkpoint`，也就是这个代码库其它地方一直刻意避免「持锁做慢操作」的那类操作。`store` 的锁用法不同：只在 `create()` 跑完之后才短暂拿一次锁调用 `touch()`。

现状下影响有限：daemon.rs 里目前没有别的请求类型会去抢 `secrets` 的锁（`SecretStore` 的写入接口是 Task 8 才接进来），所以实际能造成的争用只是「两个并发的 `Create` 请求互相等」。`concurrency.rs` 里已有的 `list_is_not_blocked_by_slow_create` 测试不受影响（`List` 完全不碰 `secrets`），依旧通过。

按 brief 的原文（"和 store 一样放进 `Arc<Mutex<_>>`"）我是照做了，但 `store` 和 `secrets` 的锁持有时长其实不对称——`create()` 签名要求 `&SecretStore`，daemon.rs 除了在整个调用期间持锁别无选择（`SecretStore` 没有实现 `Clone`，也没有「只读快照」之类的轻量接口）。Task 8 接入密钥的增删改请求时，这个点值得重新审视：如果那时候的 `SetSecret`/`RemoveSecret` 请求处理也要抢同一把锁，一个慢 `Create`（比如超大仓库打检查点）会让用户设置密钥的操作卡住。本任务没有改这个设计，因为改动会超出 brief 指定的接口范围；仅在此记录供后续任务参考。

## 修复报告：Important 发现「密钥仓的锁被握过了整个 create()」

审查确认了上面「遗留的顾虑」一节自查发现的问题，要求本次直接修，不再拖到 Task 8。

### 选的方案：(b) 只传已查好的那一条密钥

在 (a)「给 `SecretStore` 派生 `Clone`，克隆整仓密钥」和 (b)「只传调用方查好的那一条 `Option<&str>`」之间选了 (b)：

- `create()` 从来只需要一条密钥（`profile.secret` 对应的那个 env 变量），让它的形参类型是"整个密钥仓"本身就是依赖面比实际需要更宽——(a) 能让签名维持 brief 原文，但代价是把与本次会话无关的所有密钥都复制一份，纯属浪费，而且复制的还是密钥这种敏感材料，不是普通数据。
- Task 8 本来就要再改一次这个签名（加 `profiles: &[Profile]`），所以 (b) 带来的签名改动不是额外成本，只是提前发生。
- (b) 让 `create()` 的依赖更诚实：它读的就是"这一条密钥要不要注入"，不是"整仓密钥"。

### 具体改动

1. **`src/session.rs`**
   - `SessionManager::create` 签名从 `secrets: &SecretStore` 改成 `secret: Option<&str>`；函数体里原来的 `secrets.get(&profile.name)` 直接替换成传入的 `secret`，行为完全不变（调用方负责在传入前用同一个 key 查过）。
   - 顶部加了一段注释解释这个形参为什么只是"一条密钥"而不是整仓：因为 `create()` 接下来要做的是慢操作（PTY spawn、agent profile 的 git checkpoint），指回同一段"以下全是慢操作"的注释和调用方 `daemon.rs::handle` 的注释，保持两处的推理是同一条链。
   - 去掉了不再需要的 `use crate::secrets::SecretStore;`（生产代码里）。
   - 测试：`empty_secrets()` 辅助从"建一个指向空路径的 `SecretStore`"简化成直接返回 `None: Option<&'static str>`，九处调用点相应去掉取址 `&`；三处真正用到密钥的测试（`create_injects_the_secret_into_env`、`create_without_the_secret_still_starts`、`spawn_failure_says_what_to_do_not_enoent`）改成先 `SecretStore::load` 再 `secrets.get("...")` 传进去,验证的行为不变。

2. **`src/daemon.rs`**
   - `Request::Create` 分支：把「拿 `secrets_guard`、整段传给 `create()`」改成「在极短的 `recover(secrets.lock())` 作用域内只读一条 `.get(&profile).map(str::to_string)`，锁立刻释放，再把 `Option<String>` 转成 `Option<&str>` 传给 `create()`」。锁的生命周期现在被 `secret` 这个 `Option<String>` 绑定截断在那一行语句内，不会跨越 `mgr.create()`。
   - `store.lock()`（给 `touch()` 用）本来就在 `create()` 跑完之后才发生，且和 `secrets` 锁不再有任何先后依赖或嵌套关系，顺手把这点写进了新加的注释（对应 brief 里"Minor: narrow the secrets_guard binding"那条——现在锁作用域本身就窄到只剩一条语句，两把锁也确认不嵌套）。
   - 新增一段回归测试 `daemon::tests::create_does_not_hold_the_secrets_lock_across_the_slow_work`：仿照 `tests/concurrency.rs::list_is_not_blocked_by_slow_create` 的手法（8000 文件的仓库，让 agent 会话建立时的首次 git checkpoint 真的慢），在一个线程里直接调用模块私有的 `handle()` 触发慢 `Create`，另一个线程在给慢操作 150ms 头启动时间之后，单纯尝试 `recover(secrets.lock())`（不经过任何请求类型，纯粹测这把锁本身）并计时。断言：`Create` 本身耗时 > 300ms（证明场景确实慢），而拿 `secrets` 锁耗时 < 100ms（证明没有被慢 `Create` 卡住）。因为 `SetSecret`/`DeleteSecret` 这两个真正会跟 `Create` 抢 `secrets` 锁的请求类型是 Task 8 才加的，此刻还没有能通过 wire protocol 端到端验证的入口，所以选了直接测锁本身这个更底层但一样能证伪原 bug 的方式——旧代码在这个测试下会因为 `lock_wait` 远大于 100ms 而失败（等价于等到 `create_elapsed` 那么久）。测试跑了 3 次确认不 flaky（都在 4.6~4.9s 内稳定通过）。
   - 同步改了 `tests/slow_input.rs` 里唯一一处直接调用 `SessionManager::create` 的地方（签名变了，不改会编译不过），传法是 `secrets.get("fake")`，行为不变。

### 覆盖的测试

- `session::tests::create_injects_the_secret_into_env`
- `session::tests::create_without_the_secret_still_starts`
- `daemon::tests::create_does_not_hold_the_secrets_lock_across_the_slow_work`（新增的回归测试）
- 全量 `cargo test`（lib + 全部 `tests/*.rs` 集成测试 + doc-tests）

### 命令与结果

```
~/.cargo/bin/cargo fmt
env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo test --lib
```
→ `test result: ok. 99 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`（含新的 `daemon::tests::create_does_not_hold_the_secrets_lock_across_the_slow_work`）。

```
env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo test
```
→ lib 99 passed；`tests/cli.rs` 2 passed；`tests/client_timeout.rs` 1 passed；`tests/concurrency.rs` 1 passed（`list_is_not_blocked_by_slow_create` 依旧绿）；`tests/daemon_detach.rs` 1 passed；`tests/daemon_roundtrip.rs` 2 passed；`tests/projects_flow.rs` 3 passed；`tests/signal_restore.rs` 2 passed；`tests/slow_input.rs` 1 passed（`slow_checkpoint_does_not_block_the_board` 依旧绿）；`tests/socket_perms.rs` 1 passed；doc-tests 0。全部通过，0 failed。

```
git diff --check
```
无输出，空白干净。

新增的回归测试单独重复跑了 3 次（`cargo test --lib daemon::tests::create_does_not_hold_the_secrets_lock_across_the_slow_work`），耗时稳定在 4.6~4.9 秒之间，均通过，判断不 flaky。

### 文件改动

- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/session.rs`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/daemon.rs`
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/tests/slow_input.rs`
