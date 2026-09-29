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

