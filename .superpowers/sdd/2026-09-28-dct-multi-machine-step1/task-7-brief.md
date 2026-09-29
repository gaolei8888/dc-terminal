### Task 7: 留言：`dct peers` 详情、`dct send`、投递进会话、忙时排队

**Files:**
- Create: `src/mesh/deliver.rs`
- Modify: `src/pty.rs` 或 `src/session.rs`：创建会话时加环境变量 `DCT_SESSION_ID=<id>`
- Modify: `src/proto.rs`：
  - 新请求：`MeshPeers`、`MeshSend { to: String, text: String, from_session: Option<u32> }`
  - 新响应：`Response::MeshPeers(Vec<PeerView>)`、`Response::MeshSent(SendOutcome)`
- Modify: `src/daemon.rs`：起一条 mesh 投递线程，每 1 秒检查一次排队
- Modify: `src/mesh/mod.rs`、`src/mesh/cli.rs`

**Interfaces:**
- Produces:
  - `PeerView`：
    - `name`、`online`、`os: String`
    - `sessions: Vec<SessionBrief>`，其中 `SessionBrief { name: String, state: String /*忙/闲/等你回答/已停止*/, dir: String }`
    - `tentacles: Vec<String>`
  - `SendOutcome { Delivered, Queued, Offline, NoSuchMachine, NoSuchSession, SessionStopped, Refused }`
  - `mesh::deliver::marker(from_machine: &str, from_session: &str, id: &str, body: &str) -> String`：结果正好是 Global Constraints 里写的两行格式
  - `mesh::deliver::ready(state: SessionState) -> bool`：只有 `Idle` 返回 true；`Asking`（在等用户拍板）不插话

**行为：**
- **会话名**：
  - 用看板上显示的那个名字：先看 `SessionInfo.tag`，没有就用项目目录名；实施者看 `src/ui/board.rs` 是怎么显示的，用同一个函数；
  - 地址里的会话名也可以写成 `#<id>`；
  - 同名的会话不止一个时，返回 `NoSuchSession` 并列出候选。
- **`dct peers`**：
  - 对每个在线成员发加密的 `StatusRequest`，并发，3 秒超时；
  - 对方回 `Status`，内容是系统（`std::env::consts::OS`）、会话列表，以及从 `~/.dco/endpoint.json` 里读出的 `capabilities`。**读不到就是空，不报错**；
  - 离线的成员只显示名字和「离线」。
- **`dct send <电脑名/会话名> "<内容>"`**：
  - `from_session` 取环境变量 `DCT_SESSION_ID`，不在 dct 会话里运行就是 `None`，显示为「终端」；
  - 用 `ask` 发出去，等对方同步回来的 Receipt；
  - 打印：
    - 已送达：「已送到 公司Windows/dc-terminal」
    - 已排队：「对方正忙，已排队，忙完就送进去」
    - 离线：「公司Windows 现在不在线，没送出去（离线留言下一步才做）」
  - 退出码：送达和排队为 0，其余为 1。
- **收件方**：
  - 目标会话 `ready` 就马上把 `marker(...)` 加一个回车敲进去。敲的方式和 `bridge.rs` 的 `deliver_to` 一样：通过会话的 writer 或者 `Request::Input`，实施者照那里的做法复用，**不要另写一个敲键路径**；
  - 否则放进 `queues`，最多 50 条，满了回 `Refused`；
  - 投递线程每秒检查一次，会话变成 `Idle` 就按顺序送一条（一次只送一条，送完等它再次 Idle）；
  - 会话已停止或失败，回 `SessionStopped`；
  - 排队中的留言在会话被关掉时丢弃，并写一行 journal。
- **留言正文**：先去掉控制字符和转义序列（复用 `session.rs` 的 `sanitize`），长度上限 8000 字符，超过就拒绝，并提示「太长了，请缩短或者改成派活（下一步）」。

- [ ] **Step 1: 失败的测试**
  - `marker` 的精确输出；
  - `ready` 的真值表；
  - 排队的规则：先进先出，上限 50，会话停止时回 `SessionStopped`，一次只送一条；
  - 带转义序列的正文被清干净；
  - 会话名同名时报歧义；
  - `DCT_SESSION_ID` 确实传进了子进程（照 `pty.rs` 里 `spawn_passes_env_to_the_child` 的写法）；
  - 端到端，用两个内存 `Mesh` 加假 `Net`：A send → B 收到 → B 会话空闲 → 敲进去的文本和 `marker` 一致；B 会话忙时返回 Queued，转成空闲后才送进去。
- [ ] **Step 2–4: 实现、通过**，外加 Windows 检查
- [ ] **Step 5: Commit** —— `git commit -m "feat(mesh): dct peers and dct send, delivered into an idle session with a marker"`

---


## Controller ruling
Use `mesh::Net` / `FakeNet` from Task 5 for the multi-machine tests.
