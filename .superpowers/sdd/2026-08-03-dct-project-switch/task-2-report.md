# Task 2 报告：协议与守护进程接线

## 实现内容

严格按 brief 逐字实现：

1. `src/proto.rs`
   - `Request` 枚举新增 `Projects`（`Profiles` 之后一行）。
   - `Response` 枚举新增 `Projects(Vec<String>)`（`Profiles(Vec<String>)` 之后一行）。

2. `src/session.rs:65`
   - `fn recover` → `pub(crate) fn recover`，注释原样未动。

3. `src/daemon.rs`
   - `use` 块：`std::sync::Arc` → `std::sync::{Arc, Mutex}`；新增
     `use crate::projects::{store_path_for_socket, Store};`；
     `use crate::session::SessionManager;` → `use crate::session::{recover, SessionManager};`。
   - `run_with_manager` 里、`tick_mgr` 之前新建
     `let store = Arc::new(Mutex::new(Store::load(&store_path_for_socket(socket))));`，
     注释保持 brief 原文。
   - `incoming()` 循环里 `let s = store.clone();`，随连接线程一起传给 `serve`。
   - `serve` 签名加 `store: Arc<Mutex<Store>>` 参数，调用 `handle` 时传下去。
   - `handle` 签名加 `store: &Arc<Mutex<Store>>` 参数；新增
     `Request::Projects => Ok(Response::Projects(recover(store.lock()).list()))`；
     `Request::Create` 分支改写为：先 `mgr.create(..)` 拿到结果，仅当 `Ok` 时才
     `recover(store.lock()).touch(&dir)` 记账，失败的目录不进最近列表。

4. `tests/projects_flow.rs`（新建）—— 与 brief 给出的代码逐字一致，三个测试：
   - `create_records_project_most_recent_first`：建两个会话，断言 `Projects` 列表
     倒序（后建的在前）。
   - `failed_create_is_not_recorded`：对不存在的目录 `Create` 应报错，且不进列表。
   - `projects_is_empty_on_a_fresh_daemon`：全新守护进程列表为空。

## 架构约束核对

- store 锁只包住 `Store::list()` / `Store::touch()` 这类纯内存操作 + 一次小文件写，
  从未跨越 `SessionManager` 调用：`Request::Create` 分支里 `mgr.create(&dir, &profile)`
  先执行完拿到结果，*之后*才短暂拿 store 锁做 `touch`；`SessionManager` 本身完全不知道
  `Store` 的存在（`session.rs` 未改动任何业务逻辑，只是把 `recover` 可见性放宽）。
- 锁全部走 `session::recover()`，没有裸的 `.lock().unwrap()`。

## TDD 证据

### RED

```
export PATH="$HOME/.cargo/bin:$PATH" && cargo test --test projects_flow -- --test-threads=1
```

```
error[E0599]: no variant, associated function, or constant named `Projects` found for enum `dct::proto::Request` in the current scope
  --> tests/projects_flow.rs:27:27
   |
27 |     match c.call(Request::Projects).unwrap() {
   |                           ^^^^^^^^ variant, associated function, or constant not found in `dct::proto::Request`

error[E0599]: no variant, associated function, or constant named `Projects` found for enum `Response` in the current scope
  --> tests/projects_flow.rs:28:19
   |
28 |         Response::Projects(v) => v,
   |                   ^^^^^^^^ variant, associated function, or constant not found in `Response`

error: could not compile `dct` (test "projects_flow") due to 2 previous errors
```

失败符合预期：测试文件先于实现落地，`Request::Projects` / `Response::Projects` 尚不存在，
编译期即失败——证明测试确实在测「新协议变体是否接通」，而不是碰巧通过。

### GREEN

```
export PATH="$HOME/.cargo/bin:$PATH" && cargo test --test projects_flow -- --test-threads=1
```

```
running 3 tests
test create_records_project_most_recent_first ... ok
test failed_create_is_not_recorded ... ok
test projects_is_empty_on_a_fresh_daemon ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.17s
```

全量回归：

```
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -- --test-threads=1
```

全部 62 个测试通过（单元测试 45 + `cli.rs` 2 + `client_timeout.rs` 1 +
`concurrency.rs` 1 + `daemon_detach.rs` 1 + `daemon_roundtrip.rs` 2 +
`projects_flow.rs` 3 + `slow_input.rs` 1 + `socket_perms.rs` 1），0 failed。

## 涉及文件

- `/Users/lei/work/dc/dc-terminal/src/proto.rs`
- `/Users/lei/work/dc/dc-terminal/src/session.rs`
- `/Users/lei/work/dc/dc-terminal/src/daemon.rs`
- `/Users/lei/work/dc/dc-terminal/tests/projects_flow.rs`（新建）

## 自审

- **完整性**：brief 里 Step 1–5 全部完成，`Create` 失败不记账的边界情形有专门测试覆盖。
- **质量**：命名、结构与既有 `handle`/`serve` 模式一致；改动是外科手术式的，没有额外抽象。
- **纪律**：没有超出 brief 范围的改动；`SessionManager` 完全没被触碰业务逻辑（只放宽了
  `recover` 的可见性）；`git add` 只按文件名添加了 `src/daemon.rs`、`src/proto.rs`、
  `src/session.rs`、`tests/projects_flow.rs`，没有用 `-A`，`.superpowers/sdd/` 下的
  brief/report 文件未被这次 commit 带入（沿用 Task 1 的先例：文档由单独的 docs commit
  处理）。
- **测试**：`cargo fmt --check` 通过；`cargo clippy --all-targets -- -D warnings` 报的
  4 个错误（`SessionManager::new` 缺 `Default`、`session::screen` 返回类型复杂、
  `ui.rs` 的 collapsible-if 与 too-many-arguments）在 `git stash` 掉本次改动后于基线分支
  上原样复现，确认是本任务之外的既有技术债，不是本次改动引入的问题。测试输出干净，
  无额外 warning。

## 问题或顾虑

无。实现与 brief 逐字一致，测试真实覆盖了三条关键路径（记账顺序、失败不记账、
全新进程为空），且验证了架构约束（store 锁不跨 `SessionManager` 调用）确实成立。
