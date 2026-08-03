### Task 2: 协议与守护进程接线

**Files:**
- Modify: `src/proto.rs`
- Modify: `src/session.rs:65`（`fn recover` → `pub(crate) fn recover`）
- Modify: `src/daemon.rs`
- Create: `tests/projects_flow.rs`

**Interfaces:**
- Consumes: `projects::{Store, store_path_for_socket}`（Task 1）、`session::SessionManager`
- Produces:
  - `proto::Request::Projects`
  - `proto::Response::Projects(Vec<String>)`
  - `session::recover` 变为 `pub(crate)`

**说明：** `Create` **失败不记账**。目录不存在或不是 git 仓库的路径进了「最近项目」，
下次还会被选中、还会失败，等于给用户埋一颗定时哑弹。

- [ ] **Step 1: 写失败的测试**

新建 `tests/projects_flow.rs`：

```rust
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use dct::client::Client;
use dct::proto::{Request, Response};

fn start_daemon(sock: &PathBuf) {
    let s = sock.clone();
    std::thread::spawn(move || {
        let _ = dct::daemon::run(&s);
    });
    for _ in 0..50 {
        if sock.exists() {
            return;
        }
        sleep(Duration::from_millis(50));
    }
    panic!("守护进程没起来：{}", sock.display());
}

fn canon(p: &Path) -> String {
    std::fs::canonicalize(p).unwrap().display().to_string()
}

fn projects(c: &mut Client) -> Vec<String> {
    match c.call(Request::Projects).unwrap() {
        Response::Projects(v) => v,
        other => panic!("预期 Projects，实际 {other:?}"),
    }
}

#[test]
fn create_records_project_most_recent_first() {
    let home = tempfile::tempdir().unwrap();
    let sock = home.path().join("daemon.sock");
    start_daemon(&sock);

    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let mut c = Client::connect(&sock).unwrap();

    // shell profile 不要求 git 仓库，普通临时目录就够
    for d in [a.path(), b.path()] {
        match c
            .call(Request::Create {
                dir: d.display().to_string(),
                profile: "shell".into(),
            })
            .unwrap()
        {
            Response::Created { .. } => {}
            other => panic!("建会话失败：{other:?}"),
        }
    }

    assert_eq!(
        projects(&mut c),
        vec![canon(b.path()), canon(a.path())],
        "后建的项目必须排在前面"
    );
}

#[test]
fn failed_create_is_not_recorded() {
    let home = tempfile::tempdir().unwrap();
    let sock = home.path().join("daemon.sock");
    start_daemon(&sock);

    let mut c = Client::connect(&sock).unwrap();
    let missing = "/tmp/dct-这个目录不存在-9f3a2b";
    match c
        .call(Request::Create {
            dir: missing.into(),
            profile: "shell".into(),
        })
        .unwrap()
    {
        Response::Error(_) => {}
        other => panic!("目录不存在时应当报错，实际 {other:?}"),
    }

    assert!(
        projects(&mut c).is_empty(),
        "建失败的目录不能进最近项目"
    );
}

#[test]
fn projects_is_empty_on_a_fresh_daemon() {
    let home = tempfile::tempdir().unwrap();
    let sock = home.path().join("daemon.sock");
    start_daemon(&sock);

    let mut c = Client::connect(&sock).unwrap();
    assert!(projects(&mut c).is_empty(), "全新守护进程的列表应为空");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --test projects_flow -- --test-threads=1`
Expected: 编译失败，`Request::Projects` / `Response::Projects` 不存在。

- [ ] **Step 3: 实现**

**3a. `src/proto.rs`** —— 在 `Request` 的 `Profiles,` 后面加一行：

```rust
    Profiles,
    Projects,
```

在 `Response` 的 `Profiles(Vec<String>),` 后面加一行：

```rust
    Profiles(Vec<String>),
    Projects(Vec<String>),
```

**3b. `src/session.rs:65`** —— 把 `recover` 开放给同 crate 的 daemon 复用（注释保持原样）：

```rust
pub(crate) fn recover<T>(r: std::sync::LockResult<T>) -> T {
```

**3c. `src/daemon.rs`** —— 顶部 use 加两行：

```rust
use std::sync::{Arc, Mutex};

use crate::projects::{store_path_for_socket, Store};
use crate::session::{recover, SessionManager};
```

（原来的 `use std::sync::Arc;` 和 `use crate::session::SessionManager;` 相应替换掉。）

`run_with_manager` 里，在 `let tick_mgr = mgr.clone();` **之前**建 store：

```rust
    // 存放位置跟着 socket 走，测试把 socket 放临时目录就自动隔离，
    // 不会去动真实的 ~/.dct/projects.json。
    let store = Arc::new(Mutex::new(Store::load(&store_path_for_socket(socket))));
```

把 `incoming()` 循环改成也把 store 交给连接线程：

```rust
    for conn in listener.incoming() {
        let conn = conn?;
        let m = mgr.clone();
        let s = store.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(conn, m, s) {
                eprintln!("连接处理失败: {e}");
            }
        });
    }
```

`serve` 加一个参数并往下传：

```rust
fn serve(stream: UnixStream, mgr: Arc<SessionManager>, store: Arc<Mutex<Store>>) -> Result<()> {
```

```rust
            Ok(req) => handle(req, &mgr, &store),
```

`handle` 加参数，并改写 `Create`、新增 `Projects`：

```rust
fn handle(req: Request, mgr: &Arc<SessionManager>, store: &Arc<Mutex<Store>>) -> Response {
    let r: anyhow::Result<Response> = match req {
        Request::List => Ok(Response::Sessions(mgr.list())),
        Request::Profiles => Ok(Response::Profiles(
            Profile::builtin_names()
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )),
        Request::Projects => Ok(Response::Projects(recover(store.lock()).list())),
        Request::Create { dir, profile } => {
            let dir = PathBuf::from(dir);
            let r = mgr.create(&dir, &profile).map(|id| Response::Created { id });
            // 只有建成功了才记账。建失败的目录进了「最近项目」，
            // 下次还会被选中、还会失败。
            if r.is_ok() {
                recover(store.lock()).touch(&dir);
            }
            r
        }
        Request::Input { id, text } => mgr.send_input(id, &text).map(|_| Response::Ok),
```

（`Input` 及其后的所有分支原样不动。）

- [ ] **Step 4: 跑测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -- --test-threads=1`
Expected: 全绿。若 `cargo clippy -- -D warnings` 报 `Store` 未使用之类的，说明某处接线漏了。

- [ ] **Step 5: 提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt && git add -A
git commit -m "feat: 协议加 Projects，建会话成功即记入最近项目"
```

---

