# dct 多电脑第 1 步：连通自己的电脑 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用户自己的几台电脑，用同一个 DataClue 账号登录后，经人眼核对 6 位数字组成「我的电脑」组。组里智能体可以 `dct peers` 查看别的电脑、`dct send` 给别的电脑上的会话留言；留言端到端加密，对方忙时排队。

**Architecture:**
- 新建纯逻辑 crate `crates/dct-mesh`，不碰网络和磁盘，负责：
  - 地址、电脑 id；
  - 组名单的规范形式和签名；
  - 6 位核对码；
  - 留言的签名 + 加密；
  - 中转令牌的格式与验签。
- 中转 `dct-srv` 按令牌认账号，只在同一账号的端点之间转发，并提供「同账号在线端点」查询。
- dct 守护进程第一次真正启动中转连接（`src/link.rs` 的 `Link`），并把来自电脑端点的信封交给新模块 `src/mesh/`。
- 命令行 `dct login | join | peers | send` 走守护进程的本机 socket。

**Tech Stack:** Rust 2021。纯 Rust 加密：
- `p256 0.13`（已有）
- `x25519-dalek 2`（feature `static_secrets`）
- `chacha20poly1305 0.10`
- `hkdf 0.12`
- `sha2 0.10`
- `base64 0.22`

中转：`axum 0.8` / `tokio`（已有）。客户端 HTTP：`ureq 2`（已有）。

**Spec:** `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`（第 0–2 段、「分三步做」第 1 步）；中转原设计 `docs/superpowers/specs/2026-08-23-dc-terminal-srv-design.md`。

## Global Constraints

- 所有新依赖必须是纯 Rust，不许有编译 C 的 crate（和 dct-brain 同一条规矩）。每个任务结束时 `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc` 必须通过（用户：「大家都用 windows」）。
- `crates/dct-mesh` 不碰网络、磁盘、环境变量、进程。随机数由调用方传进来。
- 中转**不解析 payload**，不落盘任何会话内容，连密文也不存（原设计决定一、第 0 段第 4 条）。`dct-srv` 里出现对 payload 的反序列化即为缺陷。
- **留言不带执行权**：送进会话的只是文字，外面包标记，格式固定为：
  - 第一行：`[来自 <电脑名>/<会话名> 的留言 #<id前4位>]`
  - 第二行起：留言原文
- 给用户看的话一律用中文，不出现栈追踪；命令行出错时退出码非 0。
- 凭据按用途分开：**中转绝不接收 LLM 的 api_key**。dct 拿 api_key 只去网关换中转令牌，换来的令牌只用来连中转。
- 提交信息用英文，**不加任何 Co-Authored-By / AI 署名**。
- 已有测试全部保持通过：`~/.cargo/bin/cargo test --workspace`，`cargo clippy --workspace --all-targets -- -D warnings`。`cargo fmt --check` 在 main 上本来就不过，新代码跟周围风格一致即可。

## 相对于 spec 的裁决（写计划时定的，执行时照此办）

1. **第 1 步电脑钥匙在所有平台都放文件（0600 / Windows ACL），Mac 也不用安全芯片。**
   - 原因一：守护进程在后台签名会撞上 -25308（`errSecInteractionNotAllowed`），这件事已经实测过。
   - 原因二：Windows 和 Mac 保持一致。
   - 代价：同账号的进程能冒用电脑钥匙。这和自动钥匙的已知缺口是同一级别。
2. **第 1 步不做 MCP 工具，只做命令行。** dct 目前没有 MCP 服务（查过：`src/` 里没有 mcp）。会话里的 Claude / Codex 本来就能跑命令行，`dct peers` / `dct send` 直接能用。MCP 放到第 2 步。
3. **组里任何一台成员电脑都能批准新电脑加入**，第 1 步不设单一群主。名单的签名者必须是上一版名单里的成员。「群主」这个概念留给第 3 步（群）。
4. **第 1 步没有离线留言**：对方不在线就直接告诉发件人「对方不在线」。outbox 和重发是第 2 步的事，spec 已经这么分。
5. 电脑之间的信封用端点 id 前缀区分：电脑一律以 `c-` 开头。手机那条路（`proto` 请求）不变。

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `crates/dct-mesh/src/lib.rs` | 模块出口 |
| `crates/dct-mesh/src/canon.rs` | 长度前缀编码（跟 dct-brain 同一种） |
| `crates/dct-mesh/src/id.rs` | 电脑 id、地址 `电脑名/会话名` 解析 |
| `crates/dct-mesh/src/keys.rs` | 电脑钥匙（P-256 签名 + X25519 加密）的纯内存表示 |
| `crates/dct-mesh/src/sas.rs` | 6 位核对码 |
| `crates/dct-mesh/src/roster.rs` | 组名单、签名、验证、版本递增 |
| `crates/dct-mesh/src/seal.rs` | 留言签名 + 加密 / 解密 + 验签 |
| `crates/dct-mesh/src/wire.rs` | 电脑之间 payload 的 JSON 形状 |
| `crates/dct-mesh/src/relay_token.rs` | 中转令牌格式、签发（测试 / 开发用）、验签 |
| `crates/dct-link/src/lib.rs` | `LINK_VERSION` → 4；新增 `PATH_PEERS` 和 `PeersResponse` |
| `crates/dct-srv/src/lib.rs` | 令牌验证、账号隔离、`/link/peers`、`token` 子命令 |
| `crates/dct-srv/src/main.rs` | `--relay-keys`：有它才允许监听公网地址 |
| `src/link.rs` | 泛化处理函数：电脑信封交给 mesh，手机信封照旧 |
| `src/mesh/mod.rs` | 守护进程里的 mesh 状态（名单、待批准加入、排队的留言） |
| `src/mesh/store.rs` | `~/.dct/mesh/` 下的文件：钥匙、名单、电脑名 |
| `src/mesh/login.rs` | 用 api_key 去网关换中转令牌 |
| `src/mesh/deliver.rs` | 把留言送进会话：包标记、忙时排队、回执 |
| `src/mesh/cli.rs` | `dct login | join | peers | send` |
| `src/proto.rs` | 新请求：`MeshStatus`、`MeshLogin`、`MeshJoin`、`MeshApprove`、`MeshRemove`、`MeshPeers`、`MeshSend` |
| `src/daemon.rs` | 启动中转连接线程和 mesh 投递线程；`handle` 接新请求 |
| `src/pty.rs` / `src/session.rs` | 给会话进程设 `DCT_SESSION_ID` 环境变量 |
| `src/ui/board.rs`、`src/ui/mod.rs`、`src/i18n.rs` | 「我的电脑」一栏、加入确认行、留言计数 |
| `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md` | 附录：网关签中转令牌的接口契约 |

---

### Task 1: `dct-mesh` crate —— id、核对码、钥匙、名单

**Files:**
- Create: `crates/dct-mesh/Cargo.toml`
- Create: `crates/dct-mesh/src/lib.rs`
- Create: `crates/dct-mesh/src/canon.rs`
- Create: `crates/dct-mesh/src/id.rs`
- Create: `crates/dct-mesh/src/keys.rs`
- Create: `crates/dct-mesh/src/sas.rs`
- Create: `crates/dct-mesh/src/roster.rs`
- Modify: 根 `Cargo.toml`（workspace members 加 `crates/dct-mesh`；`[dependencies]` 加 `dct-mesh = { path = "crates/dct-mesh" }`）

**Interfaces:**
- Produces（`dct_mesh::...`）：
  - `canon::field(out: &mut Vec<u8>, s: &str)`
  - `id::endpoint_for(sign_pub: &[u8; 65]) -> String`（`"c-"` + SHA-256 前 10 字节的小写十六进制，共 22 字符）
  - `id::Address { machine: String, session: String }`
  - `id::Address::parse(s: &str) -> Result<Address, AddrError>`
  - `keys::MachineKeys::from_seeds(sign_seed: [u8; 32], kx_seed: [u8; 32]) -> Result<MachineKeys, KeyError>`
  - `MachineKeys::sign_pub(&self) -> [u8; 65]`
  - `MachineKeys::kx_pub(&self) -> [u8; 32]`
  - `MachineKeys::sign(&self, msg: &[u8]) -> [u8; 64]`
  - `keys::verify(sign_pub: &[u8; 65], msg: &[u8], sig: &[u8; 64]) -> bool`
  - `sas::code(a: &Member, b: &Member) -> String`（6 位数字，不足补 0）
  - `roster::Member { name: String, endpoint: String, sign_pub: String /*b64*/, kx_pub: String /*b64*/, added_at: u64 }`
  - `roster::Roster { group: String, version: u64, members: Vec<Member> }`
  - `roster::SignedRoster { roster: Roster, signer: String, sig: String /*b64*/ }`
  - `roster::bytes(r: &Roster) -> Vec<u8>`
  - `roster::sign(r: Roster, signer: &Member, keys: &MachineKeys) -> SignedRoster`
  - `roster::genesis(me: Member, group: String, keys: &MachineKeys) -> SignedRoster`
  - `roster::accept(current: Option<&SignedRoster>, incoming: &SignedRoster) -> Result<(), RosterError>`
  - `Roster::member(&self, endpoint: &str) -> Option<&Member>`
  - `Roster::by_name(&self, name: &str) -> Option<&Member>`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "dct-mesh"
version = "0.1.0"
edition = "2021"
license = "MIT"

# 纯逻辑：不碰网络、磁盘、环境变量；随机数由调用方给。全部纯 Rust，没有 C。
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
base64 = "0.22"
p256 = { version = "0.13", default-features = false, features = ["ecdsa", "std"] }
x25519-dalek = { version = "2", features = ["static_secrets"] }
chacha20poly1305 = { version = "0.10", default-features = false, features = ["alloc"] }
hkdf = "0.12"
```

- [ ] **Step 2: 写失败的测试（sas.rs、id.rs、roster.rs 里的 `#[cfg(test)]`）**

```rust
// sas.rs
#[test]
fn code_matches_the_fixed_vector_and_ignores_argument_order() {
    let a = Member { name: "A".into(), endpoint: "c-aaaa".into(),
        sign_pub: B64.encode([&[4u8][..], &[1u8; 64][..]].concat()),
        kx_pub: B64.encode([2u8; 32]), added_at: 0 };
    let b = Member { name: "B".into(), endpoint: "c-bbbb".into(),
        sign_pub: B64.encode([&[4u8][..], &[3u8; 64][..]].concat()),
        kx_pub: B64.encode([5u8; 32]), added_at: 0 };
    assert_eq!(code(&a, &b), "280288"); // 控制端用独立 Python 实现算出
    assert_eq!(code(&b, &a), "280288");
}
#[test]
fn a_swapped_key_changes_the_code() {
    // 服务器换了一把加密公钥 → 两边数字对不上
}

// id.rs
#[test] fn endpoint_is_c_dash_plus_20_hex_and_a_valid_relay_id() { /* EndpointId 字符集 [A-Za-z0-9_.:-]，长度 22 */ }
#[test] fn address_parses_machine_slash_session_and_rejects_bad_ones() {
    // "公司Windows/dc-terminal" ok；"/x"、"x/"、"x"、"a/b/c" 都是 Err
}

// roster.rs
#[test] fn genesis_is_accepted_when_there_is_no_current_roster() {}
#[test] fn a_later_version_signed_by_an_existing_member_is_accepted() {}
#[test] fn a_roster_signed_by_a_non_member_is_refused() {}
#[test] fn same_or_lower_version_is_refused() {}              // 防回放旧名单
#[test] fn a_different_group_id_is_refused() {}
#[test] fn duplicate_names_or_endpoints_are_refused() {}
#[test] fn a_tampered_member_list_fails_the_signature() {}
#[test] fn the_signer_may_not_be_removed_in_the_same_version_it_signs() {} // 签名者必须在上一版名单里
```

- [ ] **Step 3: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test -p dct-mesh`
Expected: 编译失败或测试失败

- [ ] **Step 4: 实现**

`canon.rs`，编码和 dct-brain 的 `canon::field` 一样：

```rust
pub fn field(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(s.as_bytes());
}
```

`sas.rs`：

```rust
/// 两台电脑各自算出同一个 6 位数；服务器伪造任何一把公钥，两边就对不上。
/// 规范（dct-sas-v1）：按 endpoint 字典序排两台；依次写
/// field("dct-sas-v1")，再对每台写 field(endpoint)、field(sign_pub)、field(kx_pub)
/// （公钥都是 STANDARD base64）；SHA-256；取前 4 字节大端 u32 % 1_000_000，补零到 6 位。
pub fn code(a: &Member, b: &Member) -> String {
    let (x, y) = if a.endpoint <= b.endpoint { (a, b) } else { (b, a) };
    let mut out = Vec::new();
    field(&mut out, "dct-sas-v1");
    for m in [x, y] {
        field(&mut out, &m.endpoint);
        field(&mut out, &m.sign_pub);
        field(&mut out, &m.kx_pub);
    }
    let d = Sha256::digest(&out);
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}
```

`roster.rs` 的规范形式（dct-roster-v1）：
- 按顺序写 `field("dct-roster-v1")`、`field(group)`、`field(version 十进制)`、`field(成员数)`；
- 成员按 endpoint 字典序排，每人依次写 `field(name)`、`field(endpoint)`、`field(sign_pub)`、`field(kx_pub)`、`field(added_at 十进制)`；
- 签名是 P-256 ECDSA（SHA-256）对上述字节签名，结果为 64 字节 r||s，用 STANDARD base64 编码。

`accept` 的规则：
- `current` 为 `None`：`incoming.roster.version == 1`，只有一个成员，签名者就是这个成员，签名有效。
- `current` 为 `Some`：
  - `group` 相同；
  - `version > current.version`；
  - 签名者在 `current` 名单里，签名用签名者在 `current` 里的公钥验；
  - 签名有效；
  - 名单里没有重复的 name 或 endpoint；
  - 所有公钥都能解码：sign 为 65 字节且以 0x04 开头，kx 为 32 字节。

`keys.rs`：`SigningKey::from_bytes(&sign_seed.into())`（种子不合法就返回 `KeyError::BadSeed`）、`x25519_dalek::StaticSecret::from(kx_seed)`；`sign` 用 `p256::ecdsa::signature::Signer`，输出 `Signature::to_bytes()`；`verify` 解析 SEC1 公钥，任何错误都返回 false。

- [ ] **Step 5: 跑测试确认通过**，再加 Windows 检查

Run:
- `~/.cargo/bin/cargo test -p dct-mesh`
- `~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/dct-mesh
git commit -m "feat(mesh): machine ids, 6-digit compare code and signed group rosters"
```

---

### Task 2: `dct-mesh` —— 留言的签名加密、线上形状、中转令牌

**Files:**
- Create: `crates/dct-mesh/src/seal.rs`
- Create: `crates/dct-mesh/src/wire.rs`
- Create: `crates/dct-mesh/src/relay_token.rs`
- Modify: `crates/dct-mesh/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 的 `MachineKeys`、`keys::verify`、`Roster`、`canon::field`
- Produces:
  - `seal::Message { id: String, kind: Kind, from: String /*endpoint*/, from_session: String, to: String, to_session: String, body: String, sent_at: u64 }`
  - `seal::Kind { Msg, Receipt, StatusRequest, Status }`（serde 小写）
  - `seal::bytes(m: &Message) -> Vec<u8>`（规范 dct-msg-v1）
  - `seal::seal(m: &Message, me: &MachineKeys, to_kx_pub: &[u8; 32], eph_seed: [u8; 32]) -> Sealed`
  - `seal::Sealed { v: u32, eph: String, ct: String }`
  - `seal::open(s: &Sealed, me: &MachineKeys, roster: &Roster, envelope_from: &str) -> Result<Message, OpenError>`
  - `OpenError { Decrypt, BadJson, UnknownSender, BadSignature, FromMismatch, NotForMe }`
  - `wire::Payload`，serde 用 `#[serde(tag = "t", rename_all = "snake_case")]`：
    - `Sealed(Sealed)`
    - `Join(JoinRequest)`
    - `Roster(SignedRoster)`
    - `JoinPending { code_hint: String }`
  - `wire::JoinRequest { member: Member, sig: String }`，由新电脑自己签，规范 dct-join-v1
  - `wire::encode(p: &Payload) -> Vec<u8>`
  - `wire::decode(b: &[u8]) -> Result<Payload, serde_json::Error>`
  - `relay_token::Claims { account: String, endpoint: String, exp: u64 }`
  - `relay_token::bytes(c: &Claims) -> Vec<u8>`（规范 dct-relay-token-v1）
  - `relay_token::issue(c: &Claims, issuer: &p256::ecdsa::SigningKey) -> String`
  - `relay_token::verify(token: &str, issuers: &[[u8; 65]], now: u64) -> Result<Claims, TokenError>`
  - `TokenError { Malformed, BadSignature, Expired }`

- [ ] **Step 1: 写失败的测试**

```rust
// seal.rs
#[test] fn sealed_message_opens_for_the_recipient_and_round_trips() {}
#[test] fn a_different_recipient_cannot_open_it() {}                // OpenError::Decrypt
#[test] fn a_sender_not_in_the_roster_is_refused() {}               // UnknownSender
#[test] fn envelope_from_must_match_the_signed_from() {}            // FromMismatch：中转换了发件人
#[test] fn a_message_addressed_to_another_machine_is_refused() {}   // NotForMe
#[test] fn flipping_one_ciphertext_byte_fails() {}
#[test] fn two_seals_of_the_same_message_differ_with_different_eph_seeds() {}

// wire.rs
#[test] fn payload_tags_are_pinned() {
    // Join → {"t":"join",...}；Sealed → {"t":"sealed",...}；Roster → {"t":"roster",...}
}
#[test] fn a_join_request_signature_covers_every_member_field() {}

// relay_token.rs
#[test] fn issued_token_verifies_and_returns_claims() {}
#[test] fn expired_token_is_refused() {}
#[test] fn token_from_an_unknown_issuer_is_refused() {}
#[test] fn changing_the_endpoint_inside_the_token_breaks_it() {}
#[test] fn garbage_is_malformed_not_a_panic() {}
```

- [ ] **Step 2: 跑测试确认失败** —— `~/.cargo/bin/cargo test -p dct-mesh`

- [ ] **Step 3: 实现**

`seal`：
1. `sig = me.sign(&bytes(m))`；
2. `plain = serde_json::to_vec(&{"m": m, "sig": b64(sig)})`；
3. 加密：
   - `eph = StaticSecret::from(eph_seed)`，`eph_pub = PublicKey::from(&eph)`；
   - `shared = eph.diffie_hellman(&to_kx_pub)`；
   - `key = Hkdf::<Sha256>::new(Some(&[eph_pub, to_kx_pub].concat()), shared.as_bytes()).expand(b"dct-seal-v1", &mut [0u8; 32])`；
   - `ct = ChaCha20Poly1305::new(key).encrypt(&[0u8; 12].into(), Payload { msg: &plain, aad: b"dct-seal-v1" })`。

   每条留言都有自己的临时密钥，所以固定用全 0 nonce 是安全的。**这一条要写进注释。**

`open`：
1. 用 `me` 的 kx 私钥和 `eph` 反推同一把 key，解密；
2. 解析 JSON；
3. 查 `roster.member(m.from)`；
4. 用它的 `sign_pub` 验签；
5. 检查 `m.from == envelope_from`；
6. 检查 `m.to == 我的 endpoint`。

`dct-msg-v1` 规范：依次写 `field("dct-msg-v1")`、`id`、`kind`、`from`、`from_session`、`to`、`to_session`、`body`、`sent_at` 各一个 field。

`dct-join-v1` 规范：依次写 `field("dct-join-v1")`、`name`、`endpoint`、`sign_pub`、`kx_pub`、`added_at`，签名用新电脑自己的钥匙。

`relay_token`：
- `token = base64url_nopad(JSON {"account", "endpoint", "exp", "sig"})`，其中 `sig` 是对 `dct-relay-token-v1` 规范字节的 P-256 签名，STANDARD base64 编码的 r||s；
- 规范字节：依次写 `field("dct-relay-token-v1")`、`field(account)`、`field(endpoint)`、`field(exp)`；
- `verify` 依次试每个 issuer 公钥，任意一把验过就算通过；再检查 `exp > now`。

- [ ] **Step 4: 跑测试确认通过**，加 Windows 检查

- [ ] **Step 5: Commit** —— `git commit -m "feat(mesh): signed and sealed messages, wire payloads, relay tokens"`

---

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

### Task 4: 网关签中转令牌的契约 + dct 这边换令牌

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`：末尾加「附录：网关签中转令牌（冻结的线上契约）」
- Create: `src/mesh/mod.rs`（这一步只放 `pub mod login;`，以后的任务再往里加）
- Create: `src/mesh/login.rs`
- Modify: `src/lib.rs`（加 `pub mod mesh;`）

**契约**（照抄进附录；网关由 dc_llm 那边实现，本任务不改 dc_llm 仓库）：

```
POST /admin/api/relay/token
  Authorization: Bearer <学生的 api_key>        ← 只发给网关，永不发给中转
  → {"endpoint": "c-<20 hex>"}
  ← 200 {"token": "<relay token>", "exp": <unix 秒>}
  ← 401 {"error": "invalid_api_key"}
  ← 404  功能开关 DC_RELAY_TOKENS_ENABLED 关着
  ← 400  endpoint 不合法（不是 c- 加 20 位小写十六进制）
令牌：dct-mesh::relay_token 的格式；account = 网关里这个账号的用户 id（十进制字符串）；
exp = 现在 + 7 天；签名钥匙是网关的一把 P-256，公钥交给中转的 --relay-keys 文件。
```

**Interfaces:**
- Produces:
  - `mesh::login::fetch_token(origin: &str, api_key: &str, endpoint: &str, send: &dyn Fn(&str, &str, &str) -> Result<(u16, String), String>) -> Result<(String, u64), String>`
    - `send` 的三个参数是 url、Bearer、JSON 请求体，返回状态码和响应体；测试里注入假的；
    - 失败时返回给用户看的中文，例如 401 → 「登录已失效，请先在 dct 里重新配对 DC 账号」，404 → 「服务器还没开放多电脑功能」。
  - `mesh::login::needs_renewal(exp: u64, now: u64) -> bool`：剩不到 1 天就返回 true。
  - `mesh::login::RELAY_TOKEN_KEY: &str = "__relay__"`：secrets.toml 里存令牌的键。令牌过期时间存在旁边的 `"__relay_exp__"`。

- [ ] **Step 1: 失败的测试**
  - 200 返回 (token, exp)
  - 401 / 404 / 400 / 网络错各自对应的中文
  - `needs_renewal` 的边界
  - api_key 只出现在 Bearer 头里，不出现在请求体
- [ ] **Step 2–4: 实现，测试通过**
- [ ] **Step 5: Commit** —— `git commit -m "feat(mesh): gateway contract for relay tokens and the dct-side exchange"`

控制端在这个任务之后要做一件事：把附录的契约发给 dc-llm 会话，请它实现。**发之前要用户同意。**

---

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

### Task 6: 登录、加入、批准、移除（命令行 + 协议）

**Files:**
- Create: `src/mesh/cli.rs`
- Modify: `src/proto.rs`
  - 新请求：`MeshStatus`、`MeshLogin`、`MeshJoin { name: String }`、`MeshApprove { endpoint: String, yes: bool }`、`MeshRemove { name: String }`
  - 新响应：`Response::Mesh(MeshView)`
  - `PROTOCOL_VERSION` 加一，并更新 `the_request_shape_is_pinned_to_the_protocol_version` 那一类守卫测试
- Modify: `src/daemon.rs`（`handle` 接新请求）
- Modify: `src/main.rs`：加 `Some("login")`、`Some("join")`、`Some("peers")`、`Some("send")`，都转到 `mesh::cli::run(&args)`
- Modify: `src/mesh/mod.rs`

**Interfaces:**
- Produces:
  - `proto::MeshView`：
    - `logged_in: bool`
    - `name: String`
    - `endpoint: String`
    - `in_group: bool`
    - `members: Vec<MemberView>`，其中 `MemberView { name, endpoint, online: bool, is_me: bool }`
    - `pending: Vec<PendingJoin>`，其中 `PendingJoin { name, endpoint, code }`
  - `mesh::cli::run(args: &[String]) -> i32`

**流程**（每一步都有对应的测试，用假的中转连接；`Link` 的 send/ask/peers 通过 `Mesh` 里一个可注入的 `Net` trait 走）：
1. `dct login`：
   - 读 secrets 里 `"dc"` 的 api_key。没有就提示：「先在 dct 里用 DC 配对账号（进 dct 按 c 选 DC）」，退出码 1。
   - 调 `fetch_token` 换令牌，存进 `__relay__`，通知守护进程开始连接。
   - 名单还不存在时：`roster::genesis(me, group = "mine-" + endpoint, keys)`，保存，打印「这台电脑成了「我的电脑」组的第一台」。
2. `dct join [--name 公司Windows]`，在新电脑上运行，前提是已经 login、还不在组里：
   - 可以顺手改名；
   - 向 `link::peers` 返回的每个在线端点发 `Payload::Join(JoinRequest)`；
   - 打印：「请在你已有的任意一台电脑上看一眼：那边会显示一个 6 位数，跟这里的 ______ 一样就点同意」。这个 6 位数是用 `sas::code(我, 对方)` 对每个在线端点各算一个；对方可能有好几台，就逐台列出「电脑名：数字」；
   - 等名单到来（最多 10 分钟）。收到的 `Payload::Roster` 包含我、而且 `roster::accept(None 或 当前)` 通过，就保存，打印「已加入」。

   注意：新电脑没有旧名单，它接受的第一份名单只要求**签名者就是给自己算过 6 位数的那台电脑**，并且名单里包含自己。
3. 已有的电脑收到 `Join`：
   - 验 `JoinRequest` 自签名；
   - 算出 6 位数；
   - 放进 `pending_joins`，TUI 和 `dct peers` 里都显示；
   - 回一个 `Payload::JoinPending`。
4. `dct peers approve <电脑名或 endpoint> [--no]`，或者在 TUI 里按 y：
   - `MeshApprove`：新名单 = 旧名单加上这台，`version + 1`，用本机钥匙签；
   - 保存，然后发给所有在线成员，加上新电脑；
   - 拒绝时只删掉 pending。
5. `dct peers remove <电脑名>`：
   - 新名单去掉这台，`version + 1`，签名并广播；
   - 不能移除自己，给出提示。

**给用户看的话**（全部走 i18n，在 `src/i18n.rs` 加 key 或 `msg` 函数）：
- 「一台叫 {name} 的电脑想加入「我的电脑」。它屏幕上的数字是 {code} 吗？(y/n)」
- 「{name} 已加入」
- 「{name} 已移出」
- 「还没登录多电脑，先运行 dct login」

- [ ] **Step 1: 失败的测试**
  - 用两个、三个内存里的 `Mesh` 加一个假 `Net`，跑完整的「A login → B login、join → A approve → B 已加入 → C join → B 批准 → A 也收到新名单」；
  - 中途伪造一个 Join：公钥被换了，6 位数就对不上；只要没人批准，它就进不了名单；
  - `remove` 之后，被移除的电脑再发 Sealed 会被丢弃；
  - `PROTOCOL_VERSION` 守卫。
- [ ] **Step 2–4: 实现、通过**，外加 Windows 检查
- [ ] **Step 5: Commit** —— `git commit -m "feat(mesh): dct login, join with a 6-digit compare, approve and remove"`

---

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

### Task 8: TUI「我的电脑」一栏、加入确认、留言计数

**Files:**
- Modify: `src/ui/board.rs`（`draw`）
- Modify: `src/ui/mod.rs`（按键 y/n 处理加入确认；每 5 秒请求一次 `MeshStatus`，不在绘制路径里做网络请求）
- Modify: `src/i18n.rs`

**行为：**
- 看板底部，在帮助行上面，加一段「我的电脑」：
  - 每台电脑一行：`● 家里Mac  本机` / `● 公司Windows  在线` / `○ 云服务器  离线`；
  - 没登录多电脑时只显示一行灰字：「多电脑未开启 · 运行 dct login」；
  - 最多显示 5 行，超出的显示「还有 N 台」。
- `pending` 不为空时，顶部显示一行黄字确认：「一台叫 {name} 的电脑想加入…数字是 {code} 吗？(y/n)」。y 或 n 发 `MeshApprove`。
  - 这一行出现时，y 和 n 只在看板视图里被它接管；**会话视图里不接管**，沿用「会话视图只吃 F 功能键」的规矩。
- 会话收到过留言，就在看板那一行末尾加「✉ N」：本次 dct 运行期间送进这个会话的条数，从 `MeshStatus` 带回。
- 不用 emoji 当图标的规矩：● ○ ✉ 是字符，不是彩色 emoji；按 `adaptive-color` 的设计选颜色。

- [ ] **Step 1: 失败的测试**：用 `ratatui::backend::TestBackend` 渲染，断言这些情况的输出：
  - 未登录那一行；
  - 三台电脑的行；
  - 超出 5 台时的「还有 N 台」；
  - 待确认那一行；
  - 在会话视图里按 y 不会发出 `MeshApprove`。
- [ ] **Step 2–4: 实现、通过**，外加 Windows 检查
- [ ] **Step 5: Commit** —— `git commit -m "feat(ui): my computers on the board, join prompt and message counts"`

---

### Task 9: 本机端到端和上线准备（上线本身要用户同意）

**Files:**
- Create: `tests/mesh_e2e.rs`
- Modify: `README.md`：写上多电脑的用法，还有 dct-srv `--relay-keys` 的部署方式

**Step 1: 本机端到端测试**，放在 `tests/mesh_e2e.rs`，属于 `#[ignore]` 的集成测试，手动 `cargo test --test mesh_e2e -- --ignored` 跑：
- 起一个 `dct-srv serve --with-link --relay-keys <tmp>/issuer.pub`，绑在 127.0.0.1 的随机端口；
- 两个独立的 `DCT_HOME` 临时目录，各起一个 daemon，用 `token mint` 签的令牌代替网关；
- 走完 login → join → approve → peers → send；
- 断言：B 的会话里出现 marker 文本；中转进程的日志里没有出现留言原文。

**Step 2: 上线清单**（写进 README 的「部署」一节；**执行它属于上线，由控制端问过用户之后再做，子 agent 不执行**）：
1. 网关实现附录契约，打开 `DC_RELAY_TOKENS_ENABLED`，导出签名公钥；
2. dataclue.cn 上新增 `/dct-relay` 反向代理，TLS 用现有证书，参考记忆「dataclue-classroom-server-ops」；
3. 服务器上跑 `dct-srv serve --with-link --relay-keys /etc/dct-srv/gateway.pub --addr 127.0.0.1:<port>`，由反向代理对外。注意：这里 dct-srv 仍然只监听本机，公网流量由反向代理转进来；
4. 用两台真电脑（Mac 加 Windows）走一遍 login → join → send。

- [ ] **Step 3: Commit** —— `git commit -m "test(mesh): local end-to-end over a real relay; deployment notes"`

---

## 自检记录

- Spec 覆盖：
  - 第 0 段：Task 2 和 3 做加密与不解析，第 4 条由 Global Constraints 和裁决 4 保证；
  - 第 1 段：身份（Task 1、4、5），组（Task 1、6），加入与核对（Task 6），移除（Task 6）；
  - 第 2 段：地址与清单（Task 7），发（Task 7），收、标记、排队、回执（Task 7），离线（裁决 4，本步不做），权限（Global Constraints）；
  - 「看得到什么」的 TUI 部分：Task 8。「进入别的电脑的会话」是第 2 步。
- 相对 spec 的改动：见「相对于 spec 的裁决」1–5。
- 类型一致性：
  - `Member` / `SignedRoster` / `MachineKeys` / `Sealed` / `Payload` 在 Task 1–2 定义，Task 5–7 使用；
  - `Link::with_handler` 在 Task 5 定义，Task 6 和 7 通过 `Net` trait 使用；
  - `MeshView` / `PeerView` / `SendOutcome` 在 Task 6–7 定义，Task 8 使用。
