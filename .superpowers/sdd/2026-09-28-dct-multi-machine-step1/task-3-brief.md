### Task 3: 中转 —— 令牌鉴权、账号隔离、在线查询

**Files:**
- Modify: `crates/dct-link/src/lib.rs`
  - `LINK_VERSION` 改成 4，并在它的文档注释后追加一行「4 = 令牌要验、`/link/peers`」；
  - 新增 `pub const PATH_PEERS: &str = "/link/peers";` 和 `pub struct PeersResponse { pub online: Vec<EndpointId> }`，加钉线上形状的测试。
- Modify: `crates/dct-srv/Cargo.toml`（加 `dct-mesh = { path = "../dct-mesh" }`）
- Modify: `crates/dct-srv/src/lib.rs`
- Modify: `crates/dct-srv/src/main.rs`

**Interfaces:**
- Consumes: `dct_mesh::relay_token::{verify, issue, Claims}`
- Produces:
  - `Relay::with_issuers(cfg: Config, issuers: Vec<[u8; 65]>) -> Relay`
  - `Relay::new(cfg)` 保留，**意思是「不验令牌」，只给已有测试和本机开发用**
  - `Relay::peers(&self, auth: &AuthFrame) -> Result<Vec<EndpointId>, LinkError>`
  - `POST /link/peers`（请求体 `AuthFrame`，答复 `PeersResponse`）
  - CLI：
    - `dct-srv serve --relay-keys <file>`
    - `dct-srv token keygen --out <dir>`
    - `dct-srv token mint --key <file> --account <a> --endpoint <e> --ttl <secs>`

**行为：**
- `check()`：配置了 issuers 时，
  - 用 `relay_token::verify(auth.token, issuers, now)` 验令牌；
  - `claims.endpoint` 必须等于 `auth.endpoint`，否则 `LinkError::NotYours`；
  - 验不过返回 `LinkError::Unauthorized`。
- 每台设备记下它的 `account`（在 `poll` 时写进 `Device`）。
- `send` / `ask`：发件方和收件方的 `account` 不同 → `LinkError::NotYours`。收件方不在线仍然是 `Offline`。
- `peers`：返回与自己同账号、在线、不是自己的端点，按字典序排。
- `main.rs`：只有给了 `--relay-keys` 才允许绑非环回地址；没给时维持现状，`must_be_loopback`。`--with-link` 不带 `--relay-keys` 绑公网照样拒绝。
- keys 文件格式：每行一把 STANDARD base64 的 65 字节 SEC1 公钥，`#` 开头的是注释，空行忽略。
- `token keygen` 写出：
  - `<dir>/issuer.key`：32 字节标量的 base64，0600；
  - `<dir>/issuer.pub`：可以直接当 keys 文件用。
- `token mint` 打印一个令牌。**这两个子命令只给开发和本机端到端测试用**；正式令牌由网关签（Task 4 的契约）。
- **payload 仍然不解析。** 加一条守卫测试：扫描 `crates/dct-srv/src` 源码，确认没有出现 `dct_mesh::wire`、`seal::open`。

- [ ] **Step 1: 写失败的测试**（放在 `crates/dct-srv/src/lib.rs` 的 tests 里，沿用现有的 `cfg(50)`、`auth("b")` 等辅助函数；需要令牌时用 `dct_mesh::relay_token::issue` 现签一个）

```rust
#[tokio::test] async fn with_issuers_a_missing_or_forged_token_is_unauthorized() {}
#[tokio::test] async fn a_token_for_another_endpoint_is_not_yours() {}
#[tokio::test] async fn an_expired_token_is_unauthorized() {}
#[tokio::test] async fn envelopes_do_not_cross_accounts() {}          // A 账号发给 B 账号的在线端点 → NotYours
#[tokio::test] async fn peers_lists_only_same_account_online_others() {}
#[tokio::test] async fn relay_new_without_issuers_still_accepts_anything() {} // 已有测试的前提
#[test] fn public_bind_needs_relay_keys() {}                          // 解析 CLI：--with-link 无 --relay-keys 绑 0.0.0.0 → 拒绝
#[test] fn dct_srv_never_opens_mesh_payloads() {}                     // 源码守卫
```

- [ ] **Step 2: 确认失败** —— `~/.cargo/bin/cargo test -p dct-srv -p dct-link`
- [ ] **Step 3: 实现上面的行为**
- [ ] **Step 4: 确认通过**：`cargo test --workspace`，外加 clippy 和 Windows 检查
- [ ] **Step 5: Commit** —— `git commit -m "feat(srv): verify relay tokens, keep envelopes within an account, list online peers"`

---

