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


## Controller ruling (overrides the brief)
`wire::Payload::JoinPending` is `JoinPending { member: Member, sig: String }` (not `code_hint`): the existing member replies with its own Member, self-signed with the dct-join-v1 canonical form (same as JoinRequest). Provide `wire::sign_member(m: &Member, keys: &MachineKeys) -> String` and `wire::verify_member(m: &Member, sig: &str) -> bool` and use them for both JoinRequest and JoinPending. Pin the tag `{"t":"join_pending",...}` in the wire tag test.
