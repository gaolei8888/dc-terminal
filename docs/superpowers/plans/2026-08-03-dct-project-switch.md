# dct 看板内切换项目实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 看板上按 `p` 弹出项目选择器，选中后新建的会话落到那个目录，不必退出 dct 重开。

**Architecture:** 三层各自独立可测——`projects.rs` 只管「最近项目」的持久化（纯数据 + 文件 IO）；`daemon.rs` 在 `Create` 成功后记一笔，并通过新增的 `Request::Projects` 把列表交给界面；`ui.rs` 只管交互，选中结果存成界面自己的 `current_dir`。

**Tech Stack:** 沿用现有依赖，**一个新依赖都不加**（`serde` / `serde_json` / `ratatui` / `crossterm` 都已在 `Cargo.toml` 里）。

**Spec:** `docs/superpowers/specs/2026-08-03-dct-project-switch-design.md`

## Global Constraints

- Rust ≥ 1.80，edition 2021，单 crate，二进制 `dct`
- **不引入 async 运行时**，也不加新依赖。阻塞 IO + 线程
- 用户可见文案一律中文
- 不出现 git / 终端黑话，错误说人话
- 锁一律经 `session::recover()` 处理 poison，不用裸 `.lock().unwrap()`
- `cargo fmt --check` 与全量测试必须通过
- 跑 cargo 前先 `export PATH="$HOME/.cargo/bin:$PATH"`（rustup 装在 `~/.cargo`，shell 配置没改）
- 测试统一 `--test-threads=1`（仓库既有约定）
- 每个任务结束必须提交

## 文件结构

| 文件 | 职责 | 任务 |
|---|---|---|
| `src/projects.rs`（新建） | 最近项目列表的读写与排序。不认识会话，也不认识界面 | 1 |
| `src/lib.rs`（改） | 加 `pub mod projects;` | 1 |
| `src/proto.rs`（改） | 加 `Request::Projects` / `Response::Projects` | 2 |
| `src/daemon.rs`（改） | 持有 `Arc<Mutex<Store>>`，`Create` 成功后 `touch`，响应 `Projects` | 2 |
| `src/session.rs`（改） | `recover` 改成 `pub(crate)`，供 daemon 复用 | 2 |
| `tests/projects_flow.rs`（新建） | 端到端：建会话 → 列表顺序正确 | 2 |
| `src/ui.rs`（改） | 纯函数 `expand_path` / `filter_projects` / `move_sel_n` | 3 |
| `src/ui.rs`（改） | 底部状态栏：当前项目 + 错误红字 | 4 |
| `src/ui.rs`（改） | `View::PickProject` 与 `p` 键交互 | 5 |

---

### Task 1: `projects.rs` —— 最近项目的持久化

**Files:**
- Create: `src/projects.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: 无
- Produces:
  - `projects::Store`
  - `Store::load(path: &Path) -> Store`
  - `Store::list(&self) -> Vec<String>`
  - `Store::touch(&mut self, dir: &Path)`
  - `projects::store_path_for_socket(socket: &Path) -> PathBuf`

**说明：** 这是一份便利性缓存。文件缺失、JSON 损坏、磁盘写不进去——**一律不报错**，
最坏退化成空列表。绝不能因为它让守护进程起不来。

- [ ] **Step 1: 写失败的测试**

新建 `src/projects.rs`，先只写测试模块（实现下一步补）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// canonicalize 会把 macOS 上 `/var/...` 的临时目录解成 `/private/var/...`，
    /// 所以断言里的期望值必须做同样的归一，否则测试在 macOS 上必失败。
    fn canon(p: &std::path::Path) -> String {
        std::fs::canonicalize(p).unwrap().display().to_string()
    }

    #[test]
    fn touch_moves_existing_entry_to_front() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();

        let mut s = Store::load(&tmp.path().join("projects.json"));
        s.touch(&a);
        s.touch(&b);
        s.touch(&a);

        assert_eq!(s.list(), vec![canon(&a), canon(&b)], "重复项要去重并提到最前");
    }

    #[test]
    fn touch_caps_at_twenty() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Store::load(&tmp.path().join("projects.json"));
        for i in 0..25 {
            let d = tmp.path().join(format!("p{i}"));
            std::fs::create_dir(&d).unwrap();
            s.touch(&d);
        }
        let list = s.list();
        assert_eq!(list.len(), 20, "上限 20 条");
        assert_eq!(list[0], canon(&tmp.path().join("p24")), "最新的在最前");
        assert!(
            !list.contains(&canon(&tmp.path().join("p0"))),
            "最旧的应当被挤掉"
        );
    }

    #[test]
    fn corrupt_json_degrades_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("projects.json");
        std::fs::write(&f, "{ 这不是 JSON").unwrap();
        let s = Store::load(&f);
        assert!(s.list().is_empty(), "损坏的文件必须当空列表，不能 panic");
    }

    #[test]
    fn missing_file_degrades_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::load(&tmp.path().join("没有这个文件.json"));
        assert!(s.list().is_empty());
    }

    #[test]
    fn touch_survives_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("projects.json");
        let d = tmp.path().join("proj");
        std::fs::create_dir(&d).unwrap();

        let mut s = Store::load(&f);
        s.touch(&d);
        drop(s);

        let s2 = Store::load(&f);
        assert_eq!(s2.list(), vec![canon(&d)], "touch 必须落盘，重新 load 读得回");
    }

    #[test]
    fn touch_keeps_unresolvable_path_as_is() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("已经删掉了");
        let mut s = Store::load(&tmp.path().join("projects.json"));
        s.touch(&gone);
        assert_eq!(
            s.list(),
            vec![gone.display().to_string()],
            "canonicalize 失败时存原样，不能丢掉这一条"
        );
    }

    #[test]
    fn store_path_sits_next_to_socket() {
        let p = store_path_for_socket(std::path::Path::new("/home/x/.dct/daemon.sock"));
        assert_eq!(p, std::path::PathBuf::from("/home/x/.dct/projects.json"));
    }
}
```

在 `src/lib.rs` 的模块列表里按字母序加一行（在 `pub mod profile;` 之前）：

```rust
pub mod projects;
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test projects -- --test-threads=1`
Expected: 编译失败，`Store` / `store_path_for_socket` 未定义。

- [ ] **Step 3: 实现**

把下面的代码写在 `src/projects.rs` 的测试模块**之前**：

```rust
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 列表上限。20 条足够覆盖手头在做的项目，再多列表本身就难挑了。
const MAX: usize = 20;

/// 磁盘格式。包一层对象而不是直接存数组，是为了将来加字段时老文件仍能读。
#[derive(Default, Serialize, Deserialize)]
struct Disk {
    #[serde(default)]
    recent: Vec<String>,
}

/// 最近开过会话的项目目录，最近使用的在最前。
pub struct Store {
    path: PathBuf,
    recent: Vec<String>,
}

/// 存放位置跟着 socket 走，而不是直接拼 `$HOME`。生产环境 socket 在
/// `~/.dct/daemon.sock`，推出来就是 `~/.dct/projects.json`，与直接拼 `$HOME` 同一个
/// 文件；而集成测试把 socket 建在临时目录里，于是自动拿到一份隔离的 store，
/// 不会去动你真实的那份。
pub fn store_path_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("projects.json"),
        None => PathBuf::from("projects.json"),
    }
}

impl Store {
    /// 文件不存在、JSON 语法错、字段类型不对——一律当空列表。
    /// 这是便利性缓存，不值得为它让守护进程起不来。
    pub fn load(path: &Path) -> Store {
        let recent = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Disk>(&s).ok())
            .map(|d| d.recent)
            .unwrap_or_default();
        Store {
            path: path.to_path_buf(),
            recent,
        }
    }

    pub fn list(&self) -> Vec<String> {
        self.recent.clone()
    }

    /// 记一笔：去重、提到最前、截断、落盘。
    pub fn touch(&mut self, dir: &Path) {
        // 归一成绝对路径，免得 `.` 和 `/abs/path` 在列表里各占一行。
        // 归一失败（目录刚被删）就存原样——丢掉这一条比存个粗糙的路径更糟。
        let key = std::fs::canonicalize(dir)
            .unwrap_or_else(|_| dir.to_path_buf())
            .display()
            .to_string();
        self.recent.retain(|p| p != &key);
        self.recent.insert(0, key);
        self.recent.truncate(MAX);
        self.save();
    }

    /// 落盘失败一律忽略：丢的是便利性，不是数据。内存里的列表照常可用。
    fn save(&self) {
        let Some(parent) = self.path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let Ok(json) = serde_json::to_string(&Disk {
            recent: self.recent.clone(),
        }) else {
            return;
        };
        // 原子写：先写同目录的临时文件再 rename。直接覆写的话，写到一半断电
        // 会留下半截 JSON，下次 load 解析失败就把整个列表丢了。
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_err() {
            return;
        }
        let _ = std::fs::rename(&tmp, &self.path);
    }
}
```

`tempfile` 已在 `[dev-dependencies]` 里，不需要改 `Cargo.toml`。

- [ ] **Step 4: 跑测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test projects -- --test-threads=1 && cargo test -- --test-threads=1`
Expected: 新增 7 个测试全绿，原有测试不受影响。

- [ ] **Step 5: 提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt && git add -A
git commit -m "feat: 最近项目列表的持久化"
```

---

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

### Task 3: 界面用的三个纯函数

**Files:**
- Modify: `src/ui.rs`

**Interfaces:**
- Consumes: 无
- Produces:
  - `ui::expand_path(input: &str, base: &Path) -> PathBuf`
  - `ui::filter_projects(all: &[String], filter: &str) -> Vec<String>`
  - `ui::move_sel_n(st: &mut ListState, len: usize, delta: i32)`（现有 `move_sel` 改为委托给它）

**说明：** 选择器里真正有逻辑的部分全在这三个纯函数里，先单独做出来并测好，
Task 5 的交互代码就只剩接线。

- [ ] **Step 1: 写失败的测试**

在 `src/ui.rs` 的 `mod tests` 里追加（放在 `fn buffer_text` 之前）：

```rust
    #[test]
    fn expand_path_handles_tilde_and_relative() {
        let base = std::path::Path::new("/base");
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());

        assert_eq!(expand_path("/abs/x", base), std::path::PathBuf::from("/abs/x"));
        assert_eq!(expand_path("~/x", base), home.join("x"));
        assert_eq!(expand_path("~", base), home);
        assert_eq!(expand_path("rel/x", base), std::path::PathBuf::from("/base/rel/x"));
        // 用户粘贴路径常带尾随空格
        assert_eq!(expand_path("  /abs/x  ", base), std::path::PathBuf::from("/abs/x"));
        // `~foo` 不是家目录展开，是个叫 ~foo 的相对路径
        assert_eq!(expand_path("~foo", base), std::path::PathBuf::from("/base/~foo"));
    }

    #[test]
    fn filter_projects_is_case_insensitive_substring() {
        let all = vec![
            "/Users/lei/work/dc/dc-terminal".to_string(),
            "/Users/lei/work/dc/dc_workbench".to_string(),
            "/Users/lei/tmp/scratch".to_string(),
        ];

        assert_eq!(filter_projects(&all, "").len(), 3, "空过滤词返回全部");
        // 三条里只有两条含 work（scratch 那条不含）。需要为 2 而不是 3——
        // 大写的 WORK 匹配到小写的 work，正是这条断言要证的事。
        assert_eq!(filter_projects(&all, "WORK").len(), 2, "不区分大小写");
        assert_eq!(
            filter_projects(&all, "dc-term"),
            vec!["/Users/lei/work/dc/dc-terminal".to_string()],
            "匹配的是完整路径的任意位置"
        );
        assert_eq!(filter_projects(&all, "scratch").len(), 1);
        assert!(filter_projects(&all, "没有这个").is_empty());
    }

    #[test]
    fn move_sel_n_clamps_at_both_ends() {
        let mut st = ListState::default();
        st.select(Some(0));

        move_sel_n(&mut st, 3, -1);
        assert_eq!(st.selected(), Some(0), "顶端再往上不动");

        move_sel_n(&mut st, 3, 1);
        move_sel_n(&mut st, 3, 1);
        move_sel_n(&mut st, 3, 1);
        assert_eq!(st.selected(), Some(2), "底端再往下不动");

        // 空列表不能 panic，也不能选中不存在的行
        let mut empty = ListState::default();
        move_sel_n(&mut empty, 0, 1);
        assert_eq!(empty.selected(), None);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib ui -- --test-threads=1`
Expected: 编译失败，`expand_path` / `filter_projects` / `move_sel_n` 未定义。

- [ ] **Step 3: 实现**

`src/ui.rs` 顶部的 `use std::path::PathBuf;` 改成：

```rust
use std::path::{Path, PathBuf};
```

把现有的 `move_sel`（`src/ui.rs:362`）**整个替换**成下面两个函数：

```rust
/// 光标移动的通用版本：只认列表长度，不认列表里装的是什么。
/// 项目选择器和会话看板共用它。
fn move_sel_n(st: &mut ListState, len: usize, delta: i32) {
    if len == 0 {
        st.select(None);
        return;
    }
    let cur = st.selected().unwrap_or(0) as i32;
    let next = (cur + delta).clamp(0, len as i32 - 1);
    st.select(Some(next as usize));
}

fn move_sel(st: &mut ListState, sessions: &[SessionInfo], delta: i32) {
    move_sel_n(st, sessions.len(), delta);
}
```

在 `short_path`（`src/ui.rs:351`）后面加这两个：

```rust
/// 把用户敲进来的路径变成绝对路径：`~` 展开成家目录，相对路径按 `base` 解析。
/// 只做字符串层面的展开，**不做存在性校验**——调用方自己决定不存在时怎么办。
fn expand_path(input: &str, base: &Path) -> PathBuf {
    // 粘贴进来的路径经常带尾随空格
    let t = input.trim();
    let home = || PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));

    if t == "~" {
        return home();
    }
    // 只认 `~/`：`~foo` 是别人的家目录（我们不支持），当普通相对路径处理
    if let Some(rest) = t.strip_prefix("~/") {
        return home().join(rest);
    }
    let p = Path::new(t);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// 不区分大小写的子串过滤。匹配**完整路径**而不只是目录名，
/// 这样 `work` 和 `dc-term` 都能用来找同一个项目。
fn filter_projects(all: &[String], filter: &str) -> Vec<String> {
    if filter.is_empty() {
        return all.to_vec();
    }
    let f = filter.to_lowercase();
    all.iter()
        .filter(|p| p.to_lowercase().contains(&f))
        .cloned()
        .collect()
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -- --test-threads=1`
Expected: 全绿。

此时 `expand_path` / `filter_projects` 还没有调用点，`cargo build` 会报 `dead_code` 警告——
**这是预期的**，Task 5 接线后消失。不要为了消警告加 `#[allow(dead_code)]`。

- [ ] **Step 5: 提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt && git add -A
git commit -m "feat: 路径展开、项目过滤与通用光标移动"
```

---

### Task 4: 底部状态栏 —— 当前项目、错误红字、按视图给提示、逆转键改 F2

**Files:**
- Modify: `src/ui.rs`

**Interfaces:**
- Consumes: `ui::short_path`（已有）
- Produces:
  - `ui::Msg { pub text: String, pub error: bool }`，带 `Msg::err(String) -> Msg`、`impl From<&str> for Msg`、`impl From<String> for Msg`
  - `draw()` 签名新增两个参数：`message: &Msg`（原为 `&str`）、`current: &str`

**说明：** 这个任务收三件互相纠缠的界面债，都落在同一段底部栏代码上，分开做会改两遍。

**（1）错误看不出是错误。** 现在所有提示——包括守护进程返回的错误——都用同一种灰字。
Task 5 的选择器要报「这不是一个目录」，必须一眼能看出是错误。顺带把 `Response::Error`
也标红，与已有的「断连时边框变红」是同一套语言。

**（2）会话视图显示的是看板的按键表。** 底部栏现在不分视图，进了会话仍然写着
`n 新建  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动  q 退出`——而这些键在会话视图里
**全部会被转发给 agent**。用户照着按 `n`，字母 n 落进 Claude Code 的输入框。这不是提示
缺失，是提示在骗人。改成提示跟着视图走。

**（3）逆转键与标题栏不一致，且偷了 Esc。** 实测当前行为：

| 位置 | 现状 |
|---|---|
| `src/ui.rs:232` | 会话视图截走 **`Esc`** 回看板 |
| `src/ui.rs:417,419` | 标题栏写「**Ctrl+B** 返回看板」——按了没反应 |
| `src/ui.rs:569` | 测试注释写「返回看板改用 Ctrl+B」，并断言 Esc 会转发给 agent |

`ff1e37d` 改了文案和测试注释，没改按键处理。结果是 Esc 被吞（Claude Code 里按 Esc
取消不掉任何东西），而标题栏宣传的键什么也不做。

**裁定：逆转键改成 `F2`。** `Esc` 和 `Ctrl+B` 一律还给 agent——Esc 是 agent 的取消键，
`Ctrl+B` 是 Claude Code 的「转后台」。F2 没有任何 CLI agent 在用，不需要双击透传这种
隐形状态，对非程序员也更直白。

这个任务做完，界面可见变化：底部多了「当前项目：…」、会话视图的提示换成 F2 那句、
标题栏改说 F2。`p` 键在 Task 5 才有。

- [ ] **Step 1: 写失败的测试**

在 `src/ui.rs` 的 `mod tests` 里追加：

```rust
    #[test]
    fn msg_from_str_is_not_an_error() {
        let m: Msg = "完成".into();
        assert!(!m.error);
        assert_eq!(m.text, "完成");
        assert!(Msg::err("炸了".into()).error);
    }

    #[test]
    fn bottom_bar_shows_current_project() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        term.draw(|f| {
            draw(
                f,
                &View::Board,
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/Users/lei/work/dc/dc-terminal",
            )
        })
        .unwrap();

        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains("dc-terminal"),
            "底部必须显示当前项目，实际（已去空白）: {content}"
        );
    }

    #[test]
    fn error_message_is_red() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        term.draw(|f| {
            draw(
                f,
                &View::Board,
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::err("不是一个目录".into()),
                true,
                "/tmp",
            )
        })
        .unwrap();

        let buf = term.backend().buffer();
        let area = buf.area;
        let red = (0..area.height).any(|y| {
            (0..area.width).any(|x| {
                buf.cell((x, y))
                    .map(|c| c.style().fg == Some(Color::Red) && c.symbol() != " ")
                    .unwrap_or(false)
            })
        });
        assert!(red, "错误提示必须用红字，否则跟成功提示长得一样");
    }

    #[test]
    fn f2_is_not_forwarded_but_esc_is() {
        // F2 是逆转键，dct 自己吃掉；Esc 必须还给 agent——
        // Claude Code 靠 Esc 取消/清空/关弹窗。
        assert_eq!(key_to_input(&key(KeyCode::F(2))), None);
        assert_eq!(key_to_input(&key(KeyCode::Esc)).as_deref(), Some("\u{1b}"));
        // Ctrl+B 是 Claude Code 的「转后台」，也必须透传
        let ctrl_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_eq!(key_to_input(&ctrl_b).as_deref(), Some("\u{2}"));
    }

    #[test]
    fn bottom_bar_help_follows_the_view() {
        use ratatui::backend::TestBackend;

        let sessions = vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/tmp/a".into(),
            state: SessionState::Working,
            activity: String::new(),
        }];
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut st = ListState::default();

        let text_of = |term: &Terminal<TestBackend>| -> String {
            buffer_text(term.backend().buffer())
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        };

        // 会话视图：绝不能显示看板的按键表——那些键在这里全被转给 agent
        term.draw(|f| {
            draw(
                f,
                &View::Attached(1),
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp/a",
            )
        })
        .unwrap();
        let c = text_of(&term);
        assert!(c.contains("F2回看板"), "会话视图要给出逆转键提示：{c}");
        assert!(c.contains("新建会话"), "还要说清新建会话怎么走：{c}");
        assert!(!c.contains("u回滚"), "会话视图不能显示看板按键表：{c}");

        // 看板视图：仍然显示看板的按键表
        term.draw(|f| {
            draw(
                f,
                &View::Board,
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp/a",
            )
        })
        .unwrap();
        let c = text_of(&term);
        assert!(c.contains("u回滚"), "看板要显示自己的按键表：{c}");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib ui -- --test-threads=1`
Expected: 编译失败，`Msg` 未定义、`draw` 参数个数不对；`bottom_bar_help_follows_the_view` 断言失败
（现在的底部栏不分视图，会话视图里照样显示看板按键表）；`f2_is_not_forwarded_but_esc_is` 通过
（`key_to_input` 本来就这么写的——真正的缺陷在 `View::Attached` 分支截了 `Esc`，见 Step 3g）。

- [ ] **Step 3: 实现**

**3a.** 在 `src/ui.rs` 的 `enum View` 定义**之前**加：

```rust
/// 底部状态栏要显示的一句话。`error` 决定它是灰字还是红字——
/// 出错和成功用同一种颜色，用户分不出刚才那步到底成没成。
pub struct Msg {
    pub text: String,
    pub error: bool,
}

impl Msg {
    pub fn err(text: String) -> Msg {
        Msg { text, error: true }
    }
}

impl From<&str> for Msg {
    fn from(s: &str) -> Msg {
        Msg {
            text: s.to_string(),
            error: false,
        }
    }
}

impl From<String> for Msg {
    fn from(text: String) -> Msg {
        Msg { text, error: false }
    }
}
```

**3c.** `message` 的全部赋值点共 8 处，逐处照下表改。行号是改动前的 `src/ui.rs`，
**从下往上改**，免得前面的编辑把后面的行号顶跑。

| 行 | 改前 | 改后 |
|---|---|---|
| 499 | `} else if message.is_empty() {` | 见 3e，整段替换 |
| 239 | `message = "守护进程连不上，刚才那次输入没发出去".into();` | `message = Msg::err("守护进程连不上，刚才那次输入没发出去".into());` |
| 215 | 见下方 A | |
| 195 | 见下方 B | |
| 184 / 189 | `message = act(...)` | 不改（`act` 的返回类型换了，赋值处不用动） |
| 154 | `message = "守护进程连不上，粘贴的内容没发出去".into();` | `message = Msg::err("守护进程连不上，粘贴的内容没发出去".into());` |
| 78 | `let mut message = String::new();` | `let mut message: Msg = "".into();` |

**A（215 起，`Request::Create` 的 match）：**

```rust
                        message = match client.call(Request::Create {
                            dir: current_dir.display().to_string(),
                            profile,
                        }) {
                            Ok(Response::Created { id }) => format!("已开会话 {id}").into(),
                            Ok(Response::Error(e)) => Msg::err(e),
                            _ => Msg::err("创建失败".into()),
                        };
```

**B（195 起，`Request::Diff` 的 match）：**

```rust
                        message = match client.call(Request::Diff { id: s.id }) {
                            Ok(Response::Diff(v)) if v.is_empty() => "没有改动".into(),
                            Ok(Response::Diff(v)) => v
                                .iter()
                                .map(|f| format!("{} +{} -{}", f.path, f.added, f.removed))
                                .collect::<Vec<_>>()
                                .join("  ")
                                .into(),
                            Ok(Response::Error(e)) => Msg::err(e),
                            _ => Msg::err("请求失败".into()),
                        };
```

**C（371 起，`act()`）：** 返回类型 `-> String` 改成 `-> Msg`，三个分支：

```rust
        Ok(Response::Ok) => "完成".into(),
        Ok(Response::Error(e)) => Msg::err(e),
        _ => Msg::err("请求失败".into()),
```

判断标准只有一条：**这句话是不是在报错**。是就 `Msg::err(...)`，不是就 `.into()`。

**3d.** `draw()` 签名末尾加一个参数，并把 `message` 的类型换掉：

```rust
fn draw(
    f: &mut Frame,
    view: &View,
    sessions: &[SessionInfo],
    st: &mut ListState,
    screen: &[Vec<ScreenSpan>],
    cursor: (u16, u16),
    message: &Msg,
    connected: bool,
    current: &str,
) {
```

**3e.** `draw()` 末尾那段底部栏整个替换成：

```rust
    // 提示必须跟着视图走。底部栏原来不分视图，进了会话仍写着看板的按键表，
    // 而那些键在会话视图里全部被转发给 agent——用户照着按 n，字母 n 会落进
    // Claude Code 的输入框。显示做不到的操作比不显示更糟。
    let idle_help = match view {
        View::Attached(_) => "F2 回看板（回看板后按 n 新建会话）　其余按键都发给 agent",
        View::PickProfile(_) => "按数字选 agent，Esc 取消",
        View::Board => "n 新建  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动  q 退出",
    };

    let (help, style) = if !connected {
        (
            "守护进程连不上，界面数据可能已过期".to_string(),
            Style::default().fg(Color::Red),
        )
    } else if message.text.is_empty() {
        (idle_help.to_string(), Style::default())
    } else if message.error {
        (message.text.clone(), Style::default().fg(Color::Red))
    } else {
        (message.text.clone(), Style::default())
    };
    // 当前项目放在边框标题里，框内只留一行字。中文是双宽字符，
    // 「当前项目：~/work/dc/dc-terminal」加上看板按键表在 80 列终端里放不下同一行，
    // 挤在一起会被 Paragraph 直接截断——标题行本来就空着，正好用它。
    f.render_widget(
        Paragraph::new(help).style(style).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("当前项目：{}", short_path(current))),
        ),
        chunks[1],
    );
```

底部栏框内仍是一行文字，`Layout::vertical` 的 `Constraint::Length(3)` 不用动。

**3f.** `run()` 里调用 `draw` 的地方补上新参数。`run()` 的 `default_dir` 现在既是当前项目、
又是相对路径的解析基准，先改名并留出可变性（Task 5 会真正改它）：

```rust
pub fn run(mut client: Client, default_dir: PathBuf) -> Result<()> {
```

函数体开头（`enable_raw_mode()?;` 之前）加：

```rust
    // start_dir 是 dct 启动时的目录，只用来解析用户敲进来的相对路径，永不改变。
    // current_dir 是「新会话开在哪」，Task 5 的选择器会改它。
    let start_dir = default_dir.clone();
    let mut current_dir = default_dir;
```

`term.draw(...)` 的闭包改成：

```rust
        term.draw(|f| {
            draw(
                f,
                &view,
                &sessions,
                &mut list_state,
                &screen,
                screen_cursor,
                &message,
                connected,
                &current_dir.display().to_string(),
            )
        })?;
```

`Request::Create` 那处的 `dir` 改用 `current_dir`：

```rust
                        message = match client.call(Request::Create {
                            dir: current_dir.display().to_string(),
                            profile,
                        }) {
```

`start_dir` 此时还没有调用点，会有 `unused_variable` 警告——Task 5 接线后消失。

**3g. 逆转键改成 F2，`Esc` 还给 agent。** 找到 `View::Attached(id)` 那个分支
（`src/ui.rs:228-242`），把开头的注释与条件整个换掉：

```rust
            View::Attached(id) => {
                // F2 是唯一被 dct 吃掉的键，其余一律 key_to_input 翻译成终端字节
                // 送进去。Esc 必须还给 agent——Claude Code 靠它取消/清空/关弹窗；
                // Ctrl+B 也必须还回去，那是 Claude Code 的「转后台」。
                // 逆转键挑 F2 是因为没有 CLI agent 在用它，不必搞双击透传。
                if key.code == KeyCode::F(2) {
                    view = View::Board;
                    need_sessions = true;
                } else if let Some(text) = key_to_input(&key) {
```

后面的函数体（发送失败的错误提示那几行）保持原样，只是那句赋值按 3c 的规则改成
`Msg::err(...)`。

**3h. 标题栏改说 F2。** `draw()` 的 `View::Attached` 分支里（`src/ui.rs:417,419`）
两处字面量把 `Ctrl+B` 换成 `F2`：

```rust
            let title = if connected {
                format!("会话 {id} · {project} —— F2 返回看板")
            } else {
                format!("会话 {id} · {project}（连接已断开，画面可能过期）—— F2 返回看板")
            };
```

**3i. 修掉那条会误导人的测试注释。** `mod tests` 里 `esc_is_forwarded_to_the_agent`
的注释写着「返回看板改用 Ctrl+B」，是错的（`ff1e37d` 只改了文案没改代码）。改成：

```rust
    #[test]
    fn esc_is_forwarded_to_the_agent() {
        // agent 靠 Esc 做取消/清空/关弹窗，抢走它会让 agent 的交互失灵。
        // 返回看板用 F2。
        assert_eq!(key_to_input(&key(KeyCode::Esc)).as_deref(), Some("\u{1b}"));
    }
```

**3g.** `mod tests` 里已有的 `draw_does_not_panic_for_all_views` 和
`disconnected_state_shows_warning_in_bottom_bar` 每个 `draw(...)` 调用都要补参数：
`""` / `"完成"` 这类实参改成 `&Msg::from("")` / `&Msg::from("完成")`，末尾加 `"/tmp/proj"`。

- [ ] **Step 4: 跑测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -- --test-threads=1`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt && git add -A
git commit -m "feat: 底部显示当前项目，错误提示改红字"
```

---

### Task 5: `p` 键与项目选择器

**Files:**
- Modify: `src/ui.rs`

**Interfaces:**
- Consumes: `expand_path`、`filter_projects`、`move_sel_n`（Task 3）；`Msg`（Task 4）；
  `proto::Request::Projects`、`proto::Response::Projects`（Task 2）
- Produces: 无（这是最后一个任务）

**说明：** 交互规则见 spec 的「界面」段。四条容易做错的：

1. `p` **只在看板视图生效**。会话视图里所有按键都转发给 agent，抢走 `p` 会让 agent 里打不出这个字母
2. 末行「手输路径…」**不参与过滤**，永远在。否则打了没匹配的字，连兜底入口都消失
3. 手输状态下**可见字符全进输入框**，不再当过滤用
4. 只校验 `is_dir()`。**是不是 git 仓库不在这里判**——那条规则留在 `SessionManager::create()`，两处各判一次迟早漂移

- [ ] **Step 1: 写失败的测试**

在 `src/ui.rs` 的 `mod tests` 里追加：

```rust
    #[test]
    fn draw_does_not_panic_for_project_picker() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        st.select(Some(0));
        let all = vec![
            "/Users/lei/work/dc/dc-terminal".to_string(),
            "/Users/lei/work/dc/dc_workbench".to_string(),
        ];

        // 列表态
        term.draw(|f| {
            draw(
                f,
                &View::PickProject {
                    all: all.clone(),
                    filter: String::new(),
                    state: st.clone(),
                    typing_path: None,
                },
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp",
            )
        })
        .unwrap();

        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(content.contains("dc-terminal"), "列表要显示项目：{content}");
        assert!(content.contains("手输路径"), "末行兜底入口必须在：{content}");

        // 过滤到无匹配：只剩兜底那一行，不能 panic
        term.draw(|f| {
            draw(
                f,
                &View::PickProject {
                    all: all.clone(),
                    filter: "没有这个".to_string(),
                    state: st.clone(),
                    typing_path: None,
                },
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp",
            )
        })
        .unwrap();
        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(content.contains("手输路径"), "无匹配时兜底入口仍要在：{content}");

        // 手输态
        term.draw(|f| {
            draw(
                f,
                &View::PickProject {
                    all: all.clone(),
                    filter: String::new(),
                    state: st.clone(),
                    typing_path: Some("~/work/x".to_string()),
                },
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp",
            )
        })
        .unwrap();
        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(content.contains("~/work/x"), "手输态要回显已输入的路径：{content}");

        // 空列表（全新守护进程）也不能 panic
        term.draw(|f| {
            draw(
                f,
                &View::PickProject {
                    all: Vec::new(),
                    filter: String::new(),
                    state: ListState::default(),
                    typing_path: None,
                },
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp",
            )
        })
        .unwrap();
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib ui -- --test-threads=1`
Expected: 编译失败，`View::PickProject` 不存在。

- [ ] **Step 3: 实现**

**3a.** `enum View` 加一个变体：

```rust
#[derive(Clone)]
enum View {
    Board,
    Attached(u32),
    PickProfile(Vec<String>),
    PickProject {
        /// 守护进程返回的完整列表，过滤不改动它
        all: Vec<String>,
        /// 用户打的字
        filter: String,
        state: ListState,
        /// Some 表示正处在「手输路径」的输入态
        typing_path: Option<String>,
    },
}
```

**3b.** `View::Board` 的按键 match 里，在 `KeyCode::Char('n')` 分支后面加：

```rust
                KeyCode::Char('p') => {
                    // 拿不到列表就不进选择器：进去看见一片空白，用户会以为
                    // 自己从来没开过项目。
                    match client.call(Request::Projects) {
                        Ok(Response::Projects(mut all)) => {
                            // 全新守护进程列表是空的，补上启动目录，
                            // 保证第一次用也不会看到空列表。
                            let start = start_dir.display().to_string();
                            if !all.contains(&start) {
                                all.push(start);
                            }
                            let mut state = ListState::default();
                            state.select(Some(0));
                            view = View::PickProject {
                                all,
                                filter: String::new(),
                                state,
                                typing_path: None,
                            };
                        }
                        Ok(Response::Error(e)) => message = Msg::err(e),
                        _ => message = Msg::err("拿不到项目列表".into()),
                    }
                }
```

**3c.** 在 `View::PickProfile(profiles) => match key.code { ... }` 分支**之后**加整个新分支：

```rust
            View::PickProject {
                all,
                mut filter,
                mut state,
                typing_path,
            } => match typing_path {
                // ——手输路径态：可见字符全进输入框，不再当过滤用——
                Some(mut buf) => match key.code {
                    KeyCode::Esc => {
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: None,
                        }
                    }
                    KeyCode::Enter => {
                        let p = expand_path(&buf, &start_dir);
                        if p.is_dir() {
                            // 「当前项目」已经在底部边框标题里，这里说的是刚发生的动作
                            message =
                                format!("已切到 {}", short_path(&p.display().to_string())).into();
                            current_dir = p;
                            view = View::Board;
                        } else {
                            // 不是 git 仓库这件事不在这里判——留给 create()
                            message = Msg::err(format!("{} 不是一个目录", p.display()));
                            view = View::PickProject {
                                all,
                                filter,
                                state,
                                typing_path: Some(buf),
                            };
                        }
                    }
                    KeyCode::Backspace => {
                        buf.pop();
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: Some(buf),
                        };
                    }
                    KeyCode::Char(c) => {
                        buf.push(c);
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: Some(buf),
                        };
                    }
                    _ => {
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: Some(buf),
                        }
                    }
                },
                // ——列表态——
                None => match key.code {
                    KeyCode::Esc => view = View::Board,
                    KeyCode::Down | KeyCode::Up => {
                        let delta = if key.code == KeyCode::Down { 1 } else { -1 };
                        // +1 是末行那个「手输路径…」，它不参与过滤，永远在
                        let n = filter_projects(&all, &filter).len() + 1;
                        move_sel_n(&mut state, n, delta);
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: None,
                        };
                    }
                    KeyCode::Enter => {
                        let shown = filter_projects(&all, &filter);
                        let i = state.selected().unwrap_or(0);
                        if i >= shown.len() {
                            // 选中的是末行「手输路径…」
                            view = View::PickProject {
                                all,
                                filter,
                                state,
                                typing_path: Some(String::new()),
                            };
                        } else {
                            let p = PathBuf::from(&shown[i]);
                            if p.is_dir() {
                                message = format!("已切到 {}", short_path(&shown[i])).into();
                                current_dir = p;
                                view = View::Board;
                            } else {
                                // 列表里那条不删——可能只是外置盘没挂
                                message =
                                    Msg::err(format!("{} 现在找不到了", short_path(&shown[i])));
                                view = View::PickProject {
                                    all,
                                    filter,
                                    state,
                                    typing_path: None,
                                };
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        filter.pop();
                        state.select(Some(0));
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: None,
                        };
                    }
                    KeyCode::Char(c) => {
                        filter.push(c);
                        // 过滤变了就回到第一项，否则光标可能停在已被过滤掉的行号上
                        state.select(Some(0));
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: None,
                        };
                    }
                    _ => {
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: None,
                        }
                    }
                },
            },
```

**3d.** `draw()` 的 `match view` 里，在 `View::PickProfile(...)` 分支之后加渲染：

```rust
        View::PickProject {
            all,
            filter,
            state,
            typing_path,
        } => {
            if let Some(buf) = typing_path {
                f.render_widget(
                    Paragraph::new(format!("{buf}▌")).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(border_style)
                            .title("输入项目路径（Enter 确认，Esc 返回列表）"),
                    ),
                    chunks[0],
                );
            } else {
                let shown = filter_projects(all, filter);
                let mut items: Vec<ListItem> = shown
                    .iter()
                    .map(|p| {
                        let short = short_path(p);
                        let name = std::path::Path::new(p)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| short.clone());
                        ListItem::new(Line::from(vec![
                            Span::raw(format!("{:<20}", truncate(&name, 20))),
                            Span::styled(
                                truncate(&short, 50),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ]))
                    })
                    .collect();
                // 兜底入口不参与过滤，永远在最后一行
                items.push(ListItem::new(Line::from(Span::styled(
                    "手输路径…",
                    Style::default().fg(Color::Cyan),
                ))));

                let title = if filter.is_empty() {
                    "选项目（↑↓ 选，Enter 确认，直接打字过滤，Esc 取消）".to_string()
                } else {
                    format!("选项目（过滤：{filter}）")
                };
                // state 是 View 里那份的副本，draw 只读不写，所以这里克隆一份给
                // render_stateful_widget 用，不去动 `st`（那是看板的光标）。
                let mut s = state.clone();
                f.render_stateful_widget(
                    List::new(items)
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(border_style)
                                .title(title),
                        )
                        .highlight_symbol("▶ "),
                    chunks[0],
                    &mut s,
                );
            }
        }
```

**3e.** 底部提示：Task 4 建的 `idle_help` match 要补一个 `PickProject` 分支，
`Board` 那句加上 `p 换项目`：

```rust
    let idle_help = match view {
        View::Attached(_) => "F2 回看板（回看板后按 n 新建会话）　其余按键都发给 agent",
        View::PickProfile(_) => "按数字选 agent，Esc 取消",
        View::PickProject { typing_path: Some(_), .. } => "输入路径后 Enter 确认，Esc 返回列表",
        View::PickProject { .. } => "↑↓ 选  Enter 确认  直接打字过滤  Esc 取消",
        View::Board => "n 新建  p 换项目  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动  q 退出",
    };
```

注意分支顺序：`typing_path: Some(_)` 必须排在通配的 `PickProject { .. }` 之前，
否则永远命中不到。

**3f.** 让粘贴在手输路径态里可用。现在主循环顶部的 `Event::Paste` 分支**只认会话视图**
（`src/ui.rs:151-158`），在选择器里粘贴会被整段吞掉——而「能粘贴路径」正是不做目录浏览器
的理由，必须补上。把那段整个替换成：

```rust
        if let Event::Paste(text) = ev {
            match &mut view {
                View::Attached(id) => {
                    if !text.is_empty() && client.call(Request::Input { id: *id, text }).is_err() {
                        message = Msg::err("守护进程连不上，粘贴的内容没发出去".into());
                    }
                }
                // 手输路径态：粘贴直接进输入框。从别处拷一条路径粘进来一步到位，
                // 这是不做目录浏览器的底气。trim 掉换行——从终端或文件管理器
                // 拷路径经常带一个尾随换行，不去掉会拼出一个不存在的目录。
                View::PickProject {
                    typing_path: Some(buf),
                    ..
                } => buf.push_str(text.trim()),
                _ => {}
            }
            continue;
        }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -- --test-threads=1 && cargo clippy -- -D warnings && cargo fmt --check`
Expected: 全绿，且 Task 3/4 留下的 `dead_code` / `unused_variable` 警告此时应当全部消失。

- [ ] **Step 5: 手动端到端验证（需要真人，在真终端里跑）**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release
cd ~/work/dc/dc-terminal && ./target/release/dct
```

逐条确认：

1. 底部显示 `当前项目：~/work/dc/dc-terminal`
2. 按 `n` 开一个 shell 会话 → 成功。会话里底部提示应当是 `F2 回看板…` 而**不是**看板按键表；
   按 `Esc` 和 `Ctrl+B` 都应当落进 agent（在 claude 会话里最容易验：Esc 能取消、Ctrl+B 能转后台）；
   按 `F2` 回看板
3. 按 `p` → 弹出列表，至少有 `dc-terminal` 一条，末行是「手输路径…」
4. 打 `work` → 列表被过滤；`Backspace` 删掉 → 恢复
5. 打 `没有这个` → 列表只剩「手输路径…」一行，**兜底入口没消失**
6. 选中「手输路径…」按 `Enter` → 变成输入框；打 `~/work/dc/dc_workbench` → `Enter`
7. 底部变成 `当前项目：~/work/dc/dc_workbench`
8. 按 `n` 新建 → 新会话的项目列显示 `dc_workbench`，**旧会话仍在看板上**（不过滤）
9. 再按 `p` → `dc_workbench` 现在排在 `dc-terminal 前面`
10. 按 `p`，选「手输路径…」，输入 `/tmp/根本不存在` → **红字**提示「不是一个目录」，且**不切换**
11. 还在手输态，用系统剪贴板拷一条真实路径，`Cmd+V` 粘贴 → 整条路径一次性进输入框（不是一个个字符），`Enter` 能切过去
12. `Enter` 进一个会话，在里面打 `p` → **字母 p 落进 agent**，没有弹出选择器
13. `q` 退出 → 终端状态正常（有回显、有换行）
14. 重开 `dct` → 底部当前项目回到启动目录（`current_dir` 不持久化），但按 `p` 列表里两个项目都还在

第 12 条最容易做错，务必单独确认。

- [ ] **Step 6: 提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt && git add -A
git commit -m "feat: p 键切换项目，选择器支持打字过滤与手输路径"
```

---

## 完成标准

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -- --test-threads=1   # 全绿
cargo clippy -- -D warnings      # 无警告
cargo fmt --check                # 格式干净
```

加上 Task 5 Step 5 的十四条手动验证通过。

## 下一份计划

做完转 `docs/superpowers/plans/2026-08-03-dct-phone-relay.md`（ask_human + Telegram + dc_llm）。
不再往项目选择器上加东西——目录浏览器、模糊匹配、置顶、扫描根目录都已在 spec 里明确否掉。
