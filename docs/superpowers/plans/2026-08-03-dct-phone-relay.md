# dct 手机中继实施计划（ask_human + Telegram + dc_llm）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 打通「agent 问你 → 手机收到一句话 → 你口述回复 → agent 拿到答案继续」这条链路。做完之后人可以离开电脑。

**Architecture:** agent 通过 MCP 调 `ask_human`，请求沿已有的 Unix socket 到守护进程；守护进程把会话置为 `Asking`，问题经 `dc_llm` 压成一句话加编号选项，由 Telegram 适配器发出；回复回来经 `dc_llm` 归类，作为 `ask_human` 的返回值送回 agent。

**Tech Stack:** 复用地基计划的 Rust 栈。新增 HTTP 客户端（`ureq`，阻塞式，与「不引入 async 运行时」一致）。

**前置：** `docs/superpowers/plans/2026-08-01-dct-core.md` 已完成（守护进程、会话、worktree、检查点、TUI）。

## 与 spec 的一处偏离

spec 写的 Bridge 是「绑 `127.0.0.1` 的本地 HTTP 服务 + token 校验」。本计划**不建 HTTP 服务**，改用已有的 Unix socket：

`dct mcp --session <id>` 作为 MCP stdio 服务被 agent 拉起，`ask_human` 经 Unix socket 转发给守护进程。

理由：去掉整个 HTTP 服务器和 token 分发，且没有任何网络监听器，权限靠文件系统，比 `127.0.0.1` + token 更强。代价是只覆盖支持 MCP 的 agent；纯 PTY 的通用 CLI 仍退回屏幕正则，与原设计一致。

## Global Constraints

- Rust ≥ 1.80，edition 2021，单 crate，二进制 `dct`
- **不引入 async 运行时**。阻塞 IO + 线程
- 用户可见文案与发往手机的消息用中文
- **出站消息必须经得起被念出来**：一句话、自足、无文件路径、无 diff、无代码块、不引用"上面第 N 项"
- **选项必须同时编号和带标签**，因为口述回复是"就第二个吧"而不是 `2`
- **永远不替用户回答**，不设超时自动选默认项
- Telegram bot **只接受用户本人 chat id 的消息**
- `cargo fmt --check` 与全量测试必须通过
- 每个任务结束必须提交

---

### Task 1: socket 权限收紧与配置文件

**Files:**
- Modify: `src/daemon.rs`（`run()` 里创建目录后设权限）
- Create: `src/config.rs`
- Modify: `src/lib.rs`（加 `pub mod config;`）

**Interfaces:**
- Consumes: 无
- Produces: `config::Config { pub telegram_token: Option<String>, pub telegram_chat_id: Option<i64>, pub llm_base_url: String, pub llm_model: String }`；`Config::load() -> Result<Config>`；`config::config_path() -> PathBuf`

**说明：** 现在 `~/.dct/` 按默认 umask 建成 `0755`，同机器任何用户都能连 socket 操控会话。必须 `0700`。配置放 `~/.dct/config.toml`，缺失时返回默认值（`llm_base_url` 默认 `http://127.0.0.1:8700/v1`，`llm_model` 默认 `qwen3-vl`），Telegram 字段缺失表示未配置。

- [ ] **Step 1: 写失败的测试**

`src/config.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let c = Config::from_toml(
            r#"
            telegram_token = "123:abc"
            telegram_chat_id = 456
            llm_base_url = "http://x/v1"
            llm_model = "m"
            "#,
        )
        .unwrap();
        assert_eq!(c.telegram_token.as_deref(), Some("123:abc"));
        assert_eq!(c.telegram_chat_id, Some(456));
        assert_eq!(c.llm_base_url, "http://x/v1");
    }

    #[test]
    fn empty_config_uses_defaults() {
        let c = Config::from_toml("").unwrap();
        assert!(c.telegram_token.is_none());
        assert!(c.telegram_chat_id.is_none());
        assert_eq!(c.llm_base_url, "http://127.0.0.1:8700/v1");
        assert!(!c.llm_model.is_empty());
    }
}
```

`tests/socket_perms.rs`：

```rust
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

#[test]
fn socket_dir_is_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let sock: PathBuf = dir.path().join("sub").join("daemon.sock");
    let s = sock.clone();
    std::thread::spawn(move || {
        let _ = dct::daemon::run(&s);
    });
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        sleep(Duration::from_millis(50));
    }
    let mode = std::fs::metadata(sock.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700, "socket 目录必须只有属主可访问，实际 {mode:o}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --test socket_perms && cargo test config`
Expected: `socket_perms` 断言失败（实际 0o755），`config` 编译失败（`Config` 未定义）。

- [ ] **Step 3: 实现**

`src/config.rs`：`Config` 用 `serde::Deserialize` + `#[serde(default)]`，`llm_base_url` / `llm_model` 用 `#[serde(default = "...")]` 提供默认值。`from_toml(&str) -> Result<Config>` 走 `toml::from_str`。`load()` 读 `config_path()`，文件不存在时等价于 `from_toml("")`。`config_path()` 是 `$HOME/.dct/config.toml`。

`src/daemon.rs` 的 `run()`：`create_dir_all(parent)` 之后加
`std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;`（需 `use std::os::unix::fs::PermissionsExt;`）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --test socket_perms && cargo test config && cargo test -- --test-threads=1`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
cargo fmt && git add -A
git commit -m "feat: socket 目录收紧到 0700，加配置文件"
```

---

### Task 2: 问题队列与 Asking 状态

**Files:**
- Modify: `src/session.rs`
- Modify: `src/proto.rs`
- Modify: `src/daemon.rs`

**Interfaces:**
- Consumes: `session::SessionManager`、`session::SessionState`
- Produces: `session::Question { pub id: u64, pub session_id: u32, pub text: String, pub options: Vec<String> }`；`SessionManager::ask(&self, session_id: u32, text: String, options: Vec<String>) -> Result<u64>`（登记问题、把会话置 `Asking`、返回问题 id）；`SessionManager::pending_questions(&self) -> Vec<Question>`；`SessionManager::answer(&self, question_id: u64, answer: String) -> Result<()>`；`SessionManager::take_answer(&self, question_id: u64) -> Option<String>`（取走答案，取到即清除）；`proto::Request::{Ask{session_id,text,options}, PollAnswer{question_id}}`；`proto::Response::{Asked{question_id}, Answer(Option<String>)}`

**说明：** `ask()` 不阻塞——MCP 侧轮询 `PollAnswer`。这样守护进程不需要为每个等待中的问题挂住一个连接线程。答案到达前 `PollAnswer` 返回 `Answer(None)`。会话在 `answer()` 之后回到 `Working`。

`tick()` 必须继续跳过 `Asking` 状态的会话（地基计划已实现，本任务不能破坏）。

- [ ] **Step 1: 写失败的测试**

`src/session.rs` 测试模块追加（沿用该模块已有的 `init_repo`、`fake_agent` 辅助函数）：

```rust
    #[test]
    fn ask_sets_asking_and_answer_clears_it() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake").unwrap();

        let qid = m
            .ask(id, "用哪个方案".into(), vec!["先跑通".into(), "先重构".into()])
            .unwrap();

        let st = m.list().iter().find(|s| s.id == id).unwrap().state;
        assert_eq!(st, SessionState::Asking);

        let pending = m.pending_questions();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, qid);
        assert_eq!(pending[0].options.len(), 2);

        assert!(m.take_answer(qid).is_none(), "还没回答时不该有答案");

        m.answer(qid, "先跑通".into()).unwrap();
        assert_eq!(m.take_answer(qid).as_deref(), Some("先跑通"));
        assert!(m.take_answer(qid).is_none(), "答案只能取走一次");

        let st = m.list().iter().find(|s| s.id == id).unwrap().state;
        assert_eq!(st, SessionState::Working);
    }

    #[test]
    fn tick_does_not_override_asking() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake").unwrap();
        m.send_input(id, "READY").unwrap();
        m.send_input(id, "").unwrap();
        m.ask(id, "问题".into(), vec![]).unwrap();

        for _ in 0..5 {
            m.tick();
        }
        let st = m.list().iter().find(|s| s.id == id).unwrap().state;
        assert_eq!(st, SessionState::Asking, "Asking 不能被 idle 正则覆盖");
    }

    #[test]
    fn answer_to_unknown_question_errors() {
        let m = SessionManager::new();
        let err = m.answer(999, "x".into()).unwrap_err().to_string();
        assert!(err.contains("没有这个问题"), "实际: {err}");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test session -- --test-threads=1`
Expected: 编译失败，`ask`/`pending_questions`/`answer`/`take_answer` 未定义。

- [ ] **Step 3: 实现**

`SessionManager` 增加两个字段：`next_question_id: AtomicU64`、`questions: Mutex<HashMap<u64, (Question, Option<String>)>>`（问题 + 已到达的答案）。所有锁沿用地基计划里的 `recover()` 处理 poison。

`ask()`：分配 id，插入 questions，把会话状态置 `Asking`。会话不存在时报中文错误。
`answer()`：找不到问题 id 时报"没有这个问题: {id}"；找到则写入答案，并把对应会话置回 `Working`。
`take_answer()`：取走并清空该问题的答案槽。
`pending_questions()`：返回尚未有答案的问题列表。

`proto.rs` 增加对应 `Request`/`Response` 变体，`daemon.rs` 的 `handle()` 接上。`Question` 需要 `Serialize`/`Deserialize`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
cargo fmt && git add -A
git commit -m "feat: 问题队列与 Asking 状态，协议加 Ask/PollAnswer"
```

---

### Task 3: `dct mcp` —— ask_human 的 MCP stdio 服务

**Files:**
- Create: `src/mcp.rs`
- Modify: `src/lib.rs`、`src/main.rs`
- Create: `tests/mcp_stdio.rs`

**Interfaces:**
- Consumes: `client::Client`、`proto::{Request, Response}`
- Produces: `mcp::serve(session_id: u32, socket: &Path, input: impl BufRead, output: impl Write) -> Result<()>`

**说明：** MCP 是 JSON-RPC 2.0 over stdio，一行一条消息。本任务只实现三个方法：`initialize`、`tools/list`、`tools/call`。只暴露一个工具 `ask_human`，入参 `{ question: string, options?: string[] }`。

`tools/call` 的处理：发 `Request::Ask` 拿到 question_id，然后每 500ms 发一次 `Request::PollAnswer` 直到拿到答案，把答案作为工具结果返回。**不设超时**——永远等，这是"永远不替用户回答"的直接体现。

`serve()` 的 IO 参数化是为了能在测试里用内存缓冲驱动，不必真起子进程。

`main.rs` 加一条分支：`dct mcp --session <id>`，用真实 stdin/stdout 调 `serve()`。

- [ ] **Step 1: 写失败的测试**

`tests/mcp_stdio.rs`：

```rust
use std::io::Cursor;
use std::path::PathBuf;
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
            break;
        }
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn lists_ask_human_tool() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("d.sock");
    start_daemon(&sock);

    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        "\n"
    );
    let mut out = Vec::new();
    dct::mcp::serve(1, &sock, Cursor::new(input), &mut out).unwrap();

    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("ask_human"), "tools/list 必须暴露 ask_human: {text}");
    assert!(text.contains("\"id\":1"), "initialize 必须有响应");
}

#[test]
fn ask_human_blocks_until_answered() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("d.sock");
    start_daemon(&sock);

    let workdir = tempfile::tempdir().unwrap();
    let mut c = Client::connect(&sock).unwrap();
    let sid = match c
        .call(Request::Create {
            dir: workdir.path().display().to_string(),
            profile: "shell".into(),
        })
        .unwrap()
    {
        Response::Created { id } => id,
        other => panic!("预期 Created，实际 {other:?}"),
    };

    // 另一个线程稍后回答
    let sock2 = sock.clone();
    std::thread::spawn(move || {
        let mut c = Client::connect(&sock2).unwrap();
        for _ in 0..100 {
            if let Ok(Response::Sessions(_)) = c.call(Request::List) {
                // 等问题登记上来
            }
            if let Ok(Response::Questions(qs)) = c.call(Request::PendingQuestions) {
                if let Some(q) = qs.first() {
                    let _ = c.call(Request::Answer {
                        question_id: q.id,
                        answer: "第二个".into(),
                    });
                    return;
                }
            }
            sleep(Duration::from_millis(50));
        }
    });

    let input = format!(
        "{}\n",
        serde_json::json!({
            "jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"ask_human","arguments":{
                "question":"用哪个方案","options":["先跑通","先重构"]}}
        })
    );
    let mut out = Vec::new();
    dct::mcp::serve(sid, &sock, Cursor::new(input), &mut out).unwrap();

    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("第二个"), "工具结果必须带回答案: {text}");
}
```

注意这个测试用到 `Request::PendingQuestions` 和 `Request::Answer` 两个变体、以及 `Response::Questions`——Task 2 里如果没加，这里补上（同样要在 `daemon.rs` 的 `handle()` 里接好）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --test mcp_stdio -- --test-threads=1`
Expected: 编译失败，`dct::mcp` 不存在。

- [ ] **Step 3: 实现**

`src/mcp.rs`：逐行读 JSON-RPC 请求，按 `method` 分派。

- `initialize` → 返回 `{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"dct","version":"0.1.0"}}`
- `tools/list` → 返回一个工具：name `ask_human`，description 用中文说明"向用户提问并等待回答"，inputSchema 声明 `question`（必填 string）和 `options`（可选 string 数组）
- `tools/call` 且 name 是 `ask_human` → `Request::Ask` → 轮询 `PollAnswer`（500ms 间隔，无超时）→ 返回 `{"content":[{"type":"text","text":"<答案>"}]}`
- 其他 method → 返回 JSON-RPC error `-32601 method not found`

输入结束（EOF）时正常返回 `Ok(())`。

`main.rs` 加分支：`dct mcp --session <id>`，解析 `--session` 后的数字，用 `std::io::stdin().lock()` 和 `std::io::stdout().lock()` 调 `serve()`。参数缺失或不是数字时打印中文错误并 `exit(2)`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
cargo fmt && git add -A
git commit -m "feat: dct mcp —— ask_human 的 MCP stdio 服务"
```

---

### Task 4: dc_llm 压缩与归类

**Files:**
- Create: `src/llm.rs`
- Modify: `src/lib.rs`、`Cargo.toml`（加 `ureq = "2"`）

**Interfaces:**
- Consumes: `config::Config`
- Produces: `llm::Llm::new(base_url: String, model: String) -> Llm`；`Llm::compress(&self, question: &str, options: &[String]) -> Result<String>`（压成一句话，附编号带标签的选项）；`Llm::classify(&self, reply: &str, options: &[String]) -> Result<Option<usize>>`（把口述回复归类到某个选项，归不出来返回 `None`）

**说明：** 走 OpenAI 兼容的 `/chat/completions`。**两个函数都必须能降级**：`dc_llm` 连不上时 `compress` 返回原问题加编号选项（不压缩），`classify` 退回纯数字匹配。降级不能报错中断流程。

- [ ] **Step 1: 写失败的测试**

`src/llm.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Vec<String> {
        vec!["先跑通".into(), "先重构".into(), "先补测试".into()]
    }

    #[test]
    fn compress_degrades_without_llm() {
        // 指向一个不可能有服务的地址，必须降级而不是报错
        let l = Llm::new("http://127.0.0.1:1/v1".into(), "m".into());
        let msg = l.compress("这是一个很长的问题", &opts()).unwrap();
        assert!(msg.contains("这是一个很长的问题"));
        assert!(msg.contains("1") && msg.contains("先跑通"), "选项必须编号且带标签: {msg}");
        assert!(!msg.contains("```"), "出站消息不能有代码块");
    }

    #[test]
    fn classify_degrades_to_digit_match() {
        let l = Llm::new("http://127.0.0.1:1/v1".into(), "m".into());
        assert_eq!(l.classify("2", &opts()).unwrap(), Some(1));
        assert_eq!(l.classify("第 3 个", &opts()).unwrap(), Some(2));
        assert_eq!(l.classify("完全不相关的话", &opts()).unwrap(), None);
    }

    #[test]
    fn classify_without_options_returns_none() {
        let l = Llm::new("http://127.0.0.1:1/v1".into(), "m".into());
        assert_eq!(l.classify("随便说点什么", &[]).unwrap(), None);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test llm`
Expected: 编译失败，`Llm` 未定义。

- [ ] **Step 3: 实现**

`compress` 的系统提示必须要求：一句话、自足、不含文件路径/diff/代码块、不引用"上面第 N 项"。拿到模型输出后**仍要做一次机械校验**——含 ``` 或换行超过 2 行就丢弃模型输出改用降级版本。选项一律由代码拼接成 `1. 先跑通 / 2. 先重构` 的形式，不交给模型生成，避免模型改写选项文字。

`classify` 的系统提示要求只输出一个数字或 `none`。解析失败时退回数字匹配：从回复里抽第一个 1..=n 的数字。

HTTP 超时设 10 秒，任何错误都走降级路径。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test llm && cargo test -- --test-threads=1`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
cargo fmt && git add -A
git commit -m "feat: dc_llm 压缩与归类，带降级路径"
```

---

### Task 5: Telegram 适配器与端到端接线

**Files:**
- Create: `src/relay.rs`
- Modify: `src/daemon.rs`（起中继线程）、`src/session.rs`（会话启动时写 `.mcp.json`）、`src/lib.rs`
- Create: `tests/relay_contract.rs`

**Interfaces:**
- Consumes: `config::Config`、`llm::Llm`、`session::Question`
- Produces: `relay::Channel`（trait：`send(&self, text: &str) -> Result<i64>`、`poll(&self) -> Result<Vec<String>>`）；`relay::Telegram`（实现 `Channel`）；`relay::run(mgr, config)`（中继线程主循环）

**说明：** 中继线程每秒做两件事——把新的 `pending_questions` 经 `compress` 发出去；把 `poll()` 收到的回复经 `classify` 变成答案交给 `SessionManager::answer()`。

**Telegram 只接受配置里那个 chat id 的消息**，其他一律丢弃。

会话启动时在 worktree 里写 `.mcp.json`，声明 `dct mcp --session <id>` 这个 MCP 服务，agent 拉起时才看得到 `ask_human`。

- [ ] **Step 1: 写失败的测试**

`tests/relay_contract.rs`：用一个内存假通道验证消息契约，不碰网络。

```rust
use std::sync::{Arc, Mutex};

use dct::relay::Channel;

#[derive(Default, Clone)]
struct FakeChannel {
    sent: Arc<Mutex<Vec<String>>>,
    inbox: Arc<Mutex<Vec<String>>>,
}

impl Channel for FakeChannel {
    fn send(&self, text: &str) -> anyhow::Result<i64> {
        self.sent.lock().unwrap().push(text.to_string());
        Ok(1)
    }
    fn poll(&self) -> anyhow::Result<Vec<String>> {
        Ok(std::mem::take(&mut *self.inbox.lock().unwrap()))
    }
}

#[test]
fn outbound_message_is_voice_safe() {
    let ch = FakeChannel::default();
    let msg = dct::relay::format_question(
        "要不要把 src/main.rs 里的错误处理改掉",
        &["改".to_string(), "不改".to_string()],
    );
    ch.send(&msg).unwrap();

    let sent = ch.sent.lock().unwrap();
    let m = &sent[0];
    assert!(!m.contains("```"), "不能有代码块: {m}");
    assert!(m.lines().count() <= 3, "念得出来的长度，最多 3 行: {m}");
    assert!(m.contains("1") && m.contains("改"), "选项要编号带标签: {m}");
}

#[test]
fn poll_returns_and_drains() {
    let ch = FakeChannel::default();
    ch.inbox.lock().unwrap().push("第二个".into());
    assert_eq!(ch.poll().unwrap(), vec!["第二个".to_string()]);
    assert!(ch.poll().unwrap().is_empty(), "poll 必须取走消息");
}
```

`src/relay.rs` 末尾加 Telegram 的 chat id 过滤测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_foreign_chat_id() {
        let updates = r#"{"ok":true,"result":[
            {"update_id":1,"message":{"chat":{"id":111},"text":"我的"}},
            {"update_id":2,"message":{"chat":{"id":999},"text":"别人的"}}
        ]}"#;
        let msgs = Telegram::extract_messages(updates, 111).unwrap();
        assert_eq!(msgs, vec!["我的".to_string()], "只能收自己 chat id 的消息");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --test relay_contract && cargo test relay`
Expected: 编译失败，`dct::relay` 不存在。

- [ ] **Step 3: 实现**

`format_question(question, options) -> String`：拼成"一句话 + 换行 + `1. 标签 / 2. 标签`"。这是纯函数，不调 LLM，供测试和降级共用。

`Telegram`：`send` 打 `https://api.telegram.org/bot<token>/sendMessage`；`poll` 打 `getUpdates` 带 `offset`（记住上次 `update_id + 1`）。`extract_messages(json, chat_id)` 是纯函数，便于测试。

`relay::run`：循环里发新问题、收回复、归类、`answer()`。已发过的问题 id 记在集合里避免重发。配置里没有 Telegram token 时这个线程直接不启动（并在守护进程日志里说明）。

`session.rs`：`create()` 在 agent 会话的 worktree 里写 `.mcp.json`：
```json
{"mcpServers":{"dct":{"command":"<current_exe>","args":["mcp","--session","<id>"]}}}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 全绿。

- [ ] **Step 5: 手动端到端验证（需要真人）**

1. 在 `~/.dct/config.toml` 填 Telegram token 和自己的 chat id
2. `dct`，开一个 claude 会话
3. 让 agent 问一个有选项的问题（比如直接要求它"用 ask_human 问我该选哪个方案"）
4. 手机上应当收到一条**一句话 + 编号选项**的消息
5. 用语音口述"就第二个吧"回复
6. 桌面上会话应当从「等你回答」变回「干活中」，agent 拿到答案继续

第 4 步如果消息很长或带路径，说明 `compress` 的提示词要调；第 6 步如果归类错了，说明 `classify` 要调。

- [ ] **Step 6: 提交**

```bash
cargo fmt && git add -A
git commit -m "feat: Telegram 中继与 .mcp.json 接线"
```

---

## 完成标准

```bash
cargo test -- --test-threads=1   # 全绿
cargo clippy -- -D warnings      # 无警告
cargo fmt --check                # 格式干净
```

加上 Task 5 Step 5 的六条手动验证通过。

## 下一份计划

手机端命令（`/new` `/list` `/diff` `/undo` `/stop`）、飞书 / 企业微信 / SMS 三个通道与主备切换、Codex profile、守护进程重启后列出遗留 worktree。
