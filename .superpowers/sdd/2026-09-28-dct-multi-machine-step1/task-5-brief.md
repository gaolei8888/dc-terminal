### Task 5: 电脑钥匙和名单落盘；守护进程启动中转连接

**Files:**
- Create: `src/mesh/store.rs`
- Modify: `src/mesh/mod.rs`
- Modify: `src/link.rs`
- Modify: `src/daemon.rs`

**Interfaces:**
- Consumes:
  - `dct_mesh::{keys::MachineKeys, roster::*, wire::*, seal::*}`
  - `link::{Link, LinkConfig}`
  - `secrets::SecretStore`
- Produces:
  - `mesh::store::Store::at(dir: PathBuf) -> Store`
  - `Store::default_dir() -> PathBuf`，即 `~/.dct/mesh`，跟 `KeyStore::default_dir` 同一个父目录
  - `Store::load_or_create_keys(&self, rand: &dyn Fn() -> [u8; 32]) -> Result<MachineKeys>`
    - 写出 `sign.key` 和 `kx.key`：base64，用 `crate::sys::fs::create_private` 创建（0600 / ACL）
    - 目录用 `restrict_dir_to_owner`
  - `Store::name(&self) -> Result<String>`：默认主机名；`Store::set_name(&self, n: &str)`
  - `Store::roster(&self) -> Result<Option<SignedRoster>>`
  - `Store::save_roster(&self, r: &SignedRoster)`：写临时文件再 rename
  - `mesh::Mesh`，守护进程里的共享状态，`Arc<Mutex<..>>`。字段：
    - keys、me: Member、roster
    - `pending_joins: Vec<(JoinRequest, String /*code*/, Instant)>`
    - `queues: HashMap<u32 /*session id*/, VecDeque<QueuedMsg>>`
  - `Mesh::on_envelope(&mut self, env: &Envelope) -> Option<Vec<u8>>`：电脑信封的唯一入口，返回值是要回给发件方的 payload。
  - `link.rs`：新增 `pub type Handler = Arc<dyn Fn(&Envelope) -> Option<Vec<u8>> + Send + Sync>;` 和 `Link::with_handler(cfg, handler)`。原来的 `Link::new(cfg, dispatch)` 改成它的包装：解码 proto `Request` → dispatch → 编码 `Response`，行为不变，已有测试照旧通过。
  - `link::peers(cfg: &LinkConfig, agent: &ureq::Agent) -> Result<Vec<EndpointId>, LinkError>`
  - `link::send(cfg, agent, env: Envelope) -> Result<(), LinkError>`
  - `link::ask(cfg, agent, env: Envelope) -> Result<Envelope, LinkError>`

**守护进程：**
- 启动时，secrets 里有 `__relay__` 就起一条独立线程跑 `Link::with_handler`：
  - endpoint = `id::endpoint_for(sign_pub)`，base = 中转地址；
  - **不在 200ms 的 tick 线程里做任何网络 IO**。
- 中转地址：配置项 `~/.dct/config.toml` 的 `[mesh] relay = "https://…"`，默认值写成常量 `DEFAULT_RELAY = "https://dataclue.cn/dct-relay"`。上线前这个地址还连不上，连接线程按 `backoff_start` / `backoff_max` 退避，不报错刷屏。
- 令牌剩不到 1 天：由连接线程调 `fetch_token` 续期，失败就记一条警告，然后继续用旧令牌。
- handler 分流：
  - `from` 以 `c-` 开头 → `mesh.on_envelope`；
  - 否则走原来的 proto dispatch。

  手机那条路这一步不接，但保留。
- `on_envelope` 的处理：
  - `Payload::Roster`：`roster::accept(current, incoming)` 通过才保存；
  - `Payload::Sealed`：`seal::open` 之后按 kind 处理：
    - `Msg`：交给投递，见 Task 7。这一步先放进 `queues` 并返回 `Receipt`；
    - `StatusRequest`：返回加密的 `Status`，内容见 Task 7；
    - 其他：忽略。
  - `Payload::Join`：见 Task 6。
- 任何验证失败：直接丢弃，只写一行 journal（`crate::journal`），不回任何东西，免得给伪造者当探针。

- [ ] **Step 1: 失败的测试**
  - store：
    - 钥匙生成一次后再 load 还是同一把；
    - 文件权限 0600（`#[cfg(unix)]`）；
    - 名单写入是原子的（临时文件加 rename）。
  - link：
    - `with_handler` 收到信封后，回复的 `seq` 和 `to` 对得上；
    - `Link::new` 的已有测试不动。
  - mesh：
    - 签名不对的名单被丢弃，旧名单保留；
    - 不在名单里的电脑发来的 `Sealed` 被丢弃，并且不回复；
    - `c-` 开头的信封走 mesh，其他的走 proto。
- [ ] **Step 2–4: 实现、通过**，外加 Windows 检查
- [ ] **Step 5: Commit** —— `git commit -m "feat(mesh): machine keys on disk and the daemon's first real relay connection"`

---


## Controller ruling (adds to the brief)
Define `mesh::Net` trait in src/mesh/net.rs: `fn peers(&self) -> Result<Vec<String>, LinkError>; fn send(&self, to: &str, payload: Vec<u8>) -> Result<(), LinkError>; fn ask(&self, to: &str, payload: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, LinkError>;` with `LinkNet` (wraps LinkConfig + ureq agent) and, under cfg(test) or a `pub mod testing`, `FakeNet`: an in-memory hub where several `Mesh` instances register by endpoint, `send`/`ask` call the target's `on_envelope` synchronously, `peers` lists other registered endpoints. Tasks 6 and 7 test with FakeNet.
