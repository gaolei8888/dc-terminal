### Task 5: 协议与守护进程

**Files:**
- Create: `src/proto.rs`
- Create: `src/daemon.rs`
- Create: `src/client.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `session::{SessionManager, SessionInfo}`、`git::FileStat`
- Produces: `proto::Request`、`proto::Response`、`proto::socket_path() -> PathBuf`；`daemon::run(&Path) -> Result<()>`；`client::Client::connect(&Path) -> Result<Client>`、`Client::call(&mut self, Request) -> Result<Response>`

**说明：** 按行分隔的 JSON，一行一条。socket 在 `~/.dct/daemon.sock`。守护进程一个连接一个线程，共享 `Arc<Mutex<SessionManager>>`，另有一个后台线程每 200ms 调 `tick()`。

- [ ] **Step 1: 写失败的集成测试**

`tests/daemon_roundtrip.rs`：

```rust
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use dct::client::Client;
use dct::proto::{Request, Response};

#[test]
fn daemon_serves_create_list_and_stop() {
    let dir = tempfile::tempdir().unwrap();
    let sock: PathBuf = dir.path().join("d.sock");

    let s = sock.clone();
    std::thread::spawn(move || {
        dct::daemon::run(&s).unwrap();
    });

    // 等 socket 出现
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        sleep(Duration::from_millis(50));
    }

    let workdir = tempfile::tempdir().unwrap();
    let mut c = Client::connect(&sock).unwrap();

    let resp = c
        .call(Request::Create {
            dir: workdir.path().display().to_string(),
            profile: "shell".into(),
        })
        .unwrap();
    let id = match resp {
        Response::Created { id } => id,
        other => panic!("预期 Created，实际 {other:?}"),
    };

    match c.call(Request::List).unwrap() {
        Response::Sessions(v) => {
            assert_eq!(v.len(), 1);
            assert_eq!(v[0].id, id);
            assert_eq!(v[0].profile, "shell");
        }
        other => panic!("预期 Sessions，实际 {other:?}"),
    }

    assert!(matches!(c.call(Request::Stop { id }).unwrap(), Response::Ok));
}

#[test]
fn unknown_session_returns_error_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let sock: PathBuf = dir.path().join("e.sock");
    let s = sock.clone();
    std::thread::spawn(move || {
        dct::daemon::run(&s).unwrap();
    });
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        sleep(Duration::from_millis(50));
    }

    let mut c = Client::connect(&sock).unwrap();
    match c.call(Request::Stop { id: 999 }).unwrap() {
        Response::Error(msg) => assert!(msg.contains("没有这个会话")),
        other => panic!("预期 Error，实际 {other:?}"),
    }
}
```

因为集成测试要引 crate，需要把 `src/main.rs` 拆成 `src/lib.rs` + `src/main.rs`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --test daemon_roundtrip`
Expected: 编译失败，找不到 crate `dct` 的 `client` / `proto` / `daemon`。

- [ ] **Step 3: 拆出 lib.rs**

`src/lib.rs`：

```rust
pub mod client;
pub mod daemon;
pub mod git;
pub mod profile;
pub mod proto;
pub mod pty;
pub mod session;
pub mod ui;
```

`src/main.rs` 暂时改成：

```rust
fn main() -> anyhow::Result<()> {
    println!("dct");
    Ok(())
}
```

`src/ui.rs` 先建空文件（Task 6 填内容）：

```rust
// Task 6 实现
```

- [ ] **Step 4: 实现协议**

`src/proto.rs`：

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::git::FileStat;
use crate::session::SessionInfo;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    List,
    Create { dir: String, profile: String },
    Input { id: u32, text: String },
    Screen { id: u32 },
    Stop { id: u32 },
    Undo { id: u32 },
    Diff { id: u32 },
    Profiles,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Sessions(Vec<SessionInfo>),
    Created { id: u32 },
    Screen(String),
    Diff(Vec<FileStat>),
    Profiles(Vec<String>),
    Ok,
    Error(String),
}

pub fn socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".dct").join("daemon.sock")
}
```

- [ ] **Step 5: 实现守护进程**

`src/daemon.rs`：

```rust
use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::profile::Profile;
use crate::proto::{Request, Response};
use crate::session::SessionManager;

pub fn run(socket: &Path) -> Result<()> {
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;

    let mgr = Arc::new(Mutex::new(SessionManager::new()));

    let tick_mgr = mgr.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        tick_mgr.lock().unwrap().tick();
    });

    for conn in listener.incoming() {
        let conn = conn?;
        let m = mgr.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(conn, m) {
                eprintln!("连接处理失败: {e}");
            }
        });
    }
    Ok(())
}

fn serve(stream: UnixStream, mgr: Arc<Mutex<SessionManager>>) -> Result<()> {
    let mut out = stream.try_clone()?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle(req, &mgr),
            Err(e) => Response::Error(format!("请求解析失败: {e}")),
        };
        writeln!(out, "{}", serde_json::to_string(&resp)?)?;
        out.flush()?;
    }
    Ok(())
}

fn handle(req: Request, mgr: &Arc<Mutex<SessionManager>>) -> Response {
    let mut m = mgr.lock().unwrap();
    let r: anyhow::Result<Response> = match req {
        Request::List => Ok(Response::Sessions(m.list())),
        Request::Profiles => Ok(Response::Profiles(
            Profile::builtin_names().iter().map(|s| s.to_string()).collect(),
        )),
        Request::Create { dir, profile } => {
            m.create(&PathBuf::from(dir), &profile).map(|id| Response::Created { id })
        }
        Request::Input { id, text } => m.send_input(id, &text).map(|_| Response::Ok),
        Request::Screen { id } => m.screen(id).map(Response::Screen),
        Request::Stop { id } => m.stop(id).map(|_| Response::Ok),
        Request::Undo { id } => m.undo(id).map(|_| Response::Ok),
        Request::Diff { id } => m.diff(id).map(Response::Diff),
    };
    r.unwrap_or_else(|e| Response::Error(e.to_string()))
}
```

- [ ] **Step 6: 实现客户端**

`src/client.rs`：

```rust
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::proto::{Request, Response};

pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    pub fn connect(socket: &Path) -> Result<Client> {
        let stream = UnixStream::connect(socket)
            .with_context(|| format!("连不上守护进程: {}", socket.display()))?;
        Ok(Client { reader: BufReader::new(stream.try_clone()?), writer: stream })
    }

    pub fn call(&mut self, req: Request) -> Result<Response> {
        writeln!(self.writer, "{}", serde_json::to_string(&req)?)?;
        self.writer.flush()?;
        let mut line = String::new();
        self.reader.read_line(&mut line).context("守护进程没有回应")?;
        Ok(serde_json::from_str(&line)?)
    }
}
```

- [ ] **Step 7: 跑测试确认通过**

Run: `cargo test --test daemon_roundtrip -- --test-threads=1`
Expected: 2 个测试 PASS。

再跑全量：`cargo test -- --test-threads=1`，全部 PASS。

- [ ] **Step 8: 提交**

```bash
git add src/ tests/
git commit -m "feat: 守护进程、Unix socket 协议与客户端"
```

---

