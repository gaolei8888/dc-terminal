### Task 5: 签名、批量签、验签

**Files:**
- Create: `crates/dct-brain/src/sign.rs`
- Modify: `crates/dct-brain/src/lib.rs`（加 `pub mod sign;`）

**Interfaces:**
- Consumes: `canon`、`ticket::{Ticket, TICKET_VERSION}`、`tier::{Tier, SignerRole}`。
- Produces:
  - `trait Signer { fn role(&self) -> SignerRole; fn public_key(&self) -> [u8; 65]; fn sign(&self, msg: &[u8], reason: &str) -> Result<[u8; 64], SignError>; }`：对 `msg` 签名（内部先做 SHA-256），返回 64 字节 `r||s`；`public_key` 是 SEC1 未压缩格式（65 字节，`0x04` 开头）。CryptoKit 的 `x963Representation` / `rawRepresentation` 正好是这两种格式。
  - `enum SignError { Unavailable, Cancelled, Other(String) }`（实现 `Display` 和 `std::error::Error`）。
  - `key_id(&[u8; 65]) -> String`（公钥的 `sha256:` 指纹）。
  - `struct Signature { key_id, alg /* "es256" */, sig /* base64 标准编码的 r||s */ }`。
  - `struct SignedTicket { ticket: Ticket, signature: Signature, batch: Option<Vec<String>> }`。
  - `struct TrustedKey { role: SignerRole, public_key: [u8; 65] }`。
  - `enum VerifyError { Unsupported, UnknownKey, WrongRole { need: SignerRole }, BadSignature, NotInBatch, WrongDevice, TooEarly, Expired, StepsChanged }`（`StepsChanged` 给 Task 6 用）。
  - `batch_bytes(&[String]) -> Vec<u8>`、`sign_one(Ticket, &dyn Signer, &str) -> Result<SignedTicket, SignError>`、`sign_batch(Vec<Ticket>, &dyn Signer, &str) -> Result<Vec<SignedTicket>, SignError>`、`verify_sig(&Signature, &[u8], &[TrustedKey]) -> Result<SignerRole, VerifyError>`、`verify(&SignedTicket, &[TrustedKey], device: &str, now: u64) -> Result<(), VerifyError>`。
  - `soft::SoftSigner`（只在 `test` 或 `soft-signer` 特性下有）：`SoftSigner::from_seed(role, seed: u8)`、`.trusted() -> TrustedKey`。
- **一次性编号不在这里查**：`verify` 通过后，由调用方（dco）记住 `nonce`，直到票过期。

**批量签（设计第 3 节）：** 签的是 `field("dct-batch-v1") + field(个数) + 每个 field(票的 digest)`；每张票都带着整个 `batch` 列表，验的时候先确认自己的 digest 在列表里，再对列表验签。

**谁能签什么：** 票的档位 `required_signer()` 是 `None` 或 `Auto` 时，用户钥匙和自动钥匙都行；是 `User` 时，只认用户钥匙。

- [ ] **Step 1: 写失败的测试**

`crates/dct-brain/src/sign.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::soft::SoftSigner;
    use super::*;
    use crate::ticket::{params_sha256, Subject, TICKET_VERSION};
    use std::collections::BTreeMap;

    fn ticket(tier: Tier, nonce_byte: char) -> Ticket {
        Ticket {
            v: TICKET_VERSION,
            nonce: nonce_byte.to_string().repeat(32),
            device: "mac-lei".into(),
            tier,
            subject: Subject::Procedure {
                name: "social:edit-bio".into(),
                steps_sha256: "sha256:s".into(),
                body_sha256: "sha256:b".into(),
            },
            params_sha256: params_sha256(&BTreeMap::new()),
            earliest: 100,
            expires: 200,
        }
    }

    fn keys() -> (SoftSigner, SoftSigner, Vec<TrustedKey>) {
        let auto = SoftSigner::from_seed(SignerRole::Auto, 1);
        let user = SoftSigner::from_seed(SignerRole::User, 2);
        let trusted = vec![auto.trusted(), user.trusted()];
        (auto, user, trusted)
    }

    #[test]
    fn a_self_ticket_signed_by_the_automatic_key_verifies() {
        let (auto, _, trusted) = keys();
        let st = sign_one(ticket(Tier::SelfOnly, 'a'), &auto, "").unwrap();
        assert_eq!(verify(&st, &trusted, "mac-lei", 150), Ok(()));
    }

    #[test]
    fn an_outward_ticket_needs_the_users_key() {
        let (auto, user, trusted) = keys();
        let by_auto = sign_one(ticket(Tier::Content, 'a'), &auto, "").unwrap();
        assert_eq!(
            verify(&by_auto, &trusted, "mac-lei", 150),
            Err(VerifyError::WrongRole { need: SignerRole::User })
        );
        let by_user = sign_one(ticket(Tier::Content, 'a'), &user, "发到 TikTok").unwrap();
        assert_eq!(verify(&by_user, &trusted, "mac-lei", 150), Ok(()));
        // 用户钥匙什么档都能签。
        let self_by_user = sign_one(ticket(Tier::SelfOnly, 'b'), &user, "").unwrap();
        assert_eq!(verify(&self_by_user, &trusted, "mac-lei", 150), Ok(()));
    }

    #[test]
    fn any_tampering_breaks_the_signature() {
        let (_, user, trusted) = keys();
        let mut st = sign_one(ticket(Tier::Content, 'a'), &user, "").unwrap();
        st.ticket.tier = Tier::SelfOnly;
        assert_eq!(verify(&st, &trusted, "mac-lei", 150), Err(VerifyError::BadSignature));
    }

    #[test]
    fn keys_not_paired_are_refused() {
        let (_, user, _) = keys();
        let stranger = SoftSigner::from_seed(SignerRole::User, 9);
        let st = sign_one(ticket(Tier::Content, 'a'), &stranger, "").unwrap();
        assert_eq!(verify(&st, &[user.trusted()], "mac-lei", 150), Err(VerifyError::UnknownKey));
    }

    #[test]
    fn device_and_time_window_are_enforced() {
        let (auto, _, trusted) = keys();
        let st = sign_one(ticket(Tier::SelfOnly, 'a'), &auto, "").unwrap();
        assert_eq!(verify(&st, &trusted, "phone", 150), Err(VerifyError::WrongDevice));
        assert_eq!(verify(&st, &trusted, "mac-lei", 99), Err(VerifyError::TooEarly));
        assert_eq!(verify(&st, &trusted, "mac-lei", 100), Ok(()));
        assert_eq!(verify(&st, &trusted, "mac-lei", 200), Err(VerifyError::Expired));
    }

    #[test]
    fn one_signature_covers_a_whole_batch() {
        let (_, user, trusted) = keys();
        let batch = sign_batch(vec![ticket(Tier::Content, 'a'), ticket(Tier::Content, 'b')], &user, "5 个平台").unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].signature, batch[1].signature);
        for st in &batch {
            assert_eq!(verify(st, &trusted, "mac-lei", 150), Ok(()));
        }
        // 一张不在这批里的票，拿这批的签名冒充不行。
        let mut forged = batch[0].clone();
        forged.ticket = ticket(Tier::Money, 'c');
        assert_eq!(verify(&forged, &trusted, "mac-lei", 150), Err(VerifyError::NotInBatch));
    }

    #[test]
    fn batch_bytes_are_stable() {
        let d = vec![
            "sha256:2215e2abe4c81f86b8c35d7be809bd6559d6a90eff5c758974fc5a1308c612c1".to_string(),
            "sha256:4ee473f5dececb43e8b583cf200002bfc42f8290e8701914907c033f63d14490".to_string(),
        ];
        assert_eq!(
            crate::canon::sha256_id(&batch_bytes(&d)),
            "sha256:5a95c7bf18b94dbc8d93bfc0c86018ed7a84f30565df37aee83a3f0748256c35"
        );
    }

    #[test]
    fn signed_tickets_round_trip_through_json() {
        let (auto, _, trusted) = keys();
        let st = sign_one(ticket(Tier::SelfOnly, 'a'), &auto, "").unwrap();
        let back: SignedTicket = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        assert_eq!(verify(&back, &trusted, "mac-lei", 150), Ok(()));
        assert!(!serde_json::to_string(&st).unwrap().contains("batch"));
    }
}
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p dct-brain sign`
Expected: 编译失败。

- [ ] **Step 3: 实现**

在 `sign.rs` 顶部加：

```rust
//! 签名和验签。算法是 ECDSA P-256 + SHA-256（「es256」）：Mac 安全芯片、手机 passkey
//! 签出来的都是它，纯 Rust 实现，没有 C。
//!
//! 验签只回答「这张票是不是配对过的钥匙签的、档位够不够、设备和时间对不对」。
//! **一次性编号不在这里查**：通过之后由调用方（dco）记住 nonce，直到票过期。
use crate::canon::{field, sha256_id};
use crate::ticket::{Ticket, TICKET_VERSION};
use crate::tier::{SignerRole, Tier};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use p256::ecdsa::{signature::Verifier as _, Signature as EcSig, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const ALG: &str = "es256";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    /// 这台机器没有安全芯片 / 钥匙还没建。
    Unavailable,
    /// 用户取消了 Touch ID，或者没通过。
    Cancelled,
    Other(String),
}

impl std::fmt::Display for SignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignError::Unavailable => write!(f, "这台电脑上用不了安全芯片签名"),
            SignError::Cancelled => write!(f, "没有通过指纹确认"),
            SignError::Other(m) => write!(f, "签名失败：{m}"),
        }
    }
}

impl std::error::Error for SignError {}

pub trait Signer {
    fn role(&self) -> SignerRole;
    /// SEC1 未压缩公钥，65 字节，`0x04` 开头。
    fn public_key(&self) -> [u8; 65];
    /// 对 `msg` 签名（内部先做 SHA-256），返回 64 字节 `r||s`。
    /// `reason` 是给用户看的那句人话，Touch ID 弹窗里显示它；自动钥匙忽略。
    fn sign(&self, msg: &[u8], reason: &str) -> Result<[u8; 64], SignError>;
}

pub fn key_id(public_key: &[u8; 65]) -> String {
    sha256_id(public_key)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub key_id: String,
    pub alg: String,
    /// base64（标准编码）的 64 字节 `r||s`。
    pub sig: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTicket {
    pub ticket: Ticket,
    pub signature: Signature,
    /// 批量签时这一批所有票的 digest；单张签时没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedKey {
    pub role: SignerRole,
    pub public_key: [u8; 65],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    Unsupported,
    UnknownKey,
    WrongRole { need: SignerRole },
    BadSignature,
    NotInBatch,
    WrongDevice,
    TooEarly,
    Expired,
    /// 批准记录对应的流程步骤已经变了（Task 6）。
    StepsChanged,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            VerifyError::Unsupported => "票的版本或算法不认识",
            VerifyError::UnknownKey => "签名的钥匙没配对过",
            VerifyError::WrongRole { .. } => "这一档要用户本人签名",
            VerifyError::BadSignature => "签名不对",
            VerifyError::NotInBatch => "这张票不在签过的那一批里",
            VerifyError::WrongDevice => "这张票不是给这台设备的",
            VerifyError::TooEarly => "还没到可以执行的时间",
            VerifyError::Expired => "票已经过期",
            VerifyError::StepsChanged => "流程的步骤改过了，要重新批准",
        };
        f.write_str(s)
    }
}

impl std::error::Error for VerifyError {}

pub fn batch_bytes(digests: &[String]) -> Vec<u8> {
    let mut o = Vec::new();
    field(&mut o, "dct-batch-v1");
    field(&mut o, &digests.len().to_string());
    for d in digests {
        field(&mut o, d);
    }
    o
}

pub(crate) fn make_signature(s: &dyn Signer, msg: &[u8], reason: &str) -> Result<Signature, SignError> {
    let raw = s.sign(msg, reason)?;
    Ok(Signature {
        key_id: key_id(&s.public_key()),
        alg: ALG.into(),
        sig: STANDARD.encode(raw),
    })
}

pub fn sign_one(ticket: Ticket, s: &dyn Signer, reason: &str) -> Result<SignedTicket, SignError> {
    let signature = make_signature(s, &ticket.canonical_bytes(), reason)?;
    Ok(SignedTicket { ticket, signature, batch: None })
}

/// 一次签一批：同一条视频发 5 个平台，只按 1 次指纹（设计第 3 节）。
pub fn sign_batch(tickets: Vec<Ticket>, s: &dyn Signer, reason: &str) -> Result<Vec<SignedTicket>, SignError> {
    let digests: Vec<String> = tickets.iter().map(Ticket::digest).collect();
    let signature = make_signature(s, &batch_bytes(&digests), reason)?;
    Ok(tickets
        .into_iter()
        .map(|ticket| SignedTicket {
            ticket,
            signature: signature.clone(),
            batch: Some(digests.clone()),
        })
        .collect())
}

/// 验 `msg` 上的签名，返回签名钥匙的角色。
pub fn verify_sig(sig: &Signature, msg: &[u8], trusted: &[TrustedKey]) -> Result<SignerRole, VerifyError> {
    if sig.alg != ALG {
        return Err(VerifyError::Unsupported);
    }
    let key = trusted
        .iter()
        .find(|k| key_id(&k.public_key) == sig.key_id)
        .ok_or(VerifyError::UnknownKey)?;
    let raw = STANDARD.decode(&sig.sig).map_err(|_| VerifyError::BadSignature)?;
    let es = EcSig::from_slice(&raw).map_err(|_| VerifyError::BadSignature)?;
    let vk = VerifyingKey::from_sec1_bytes(&key.public_key).map_err(|_| VerifyError::BadSignature)?;
    vk.verify(msg, &es).map_err(|_| VerifyError::BadSignature)?;
    Ok(key.role)
}

fn role_may_sign(tier: Tier, role: SignerRole) -> bool {
    match tier.required_signer() {
        None | Some(SignerRole::Auto) => true,
        Some(SignerRole::User) => role == SignerRole::User,
    }
}

pub fn verify(st: &SignedTicket, trusted: &[TrustedKey], device: &str, now: u64) -> Result<(), VerifyError> {
    let t = &st.ticket;
    if t.v != TICKET_VERSION {
        return Err(VerifyError::Unsupported);
    }
    if t.device != device {
        return Err(VerifyError::WrongDevice);
    }
    if now < t.earliest {
        return Err(VerifyError::TooEarly);
    }
    if now >= t.expires {
        return Err(VerifyError::Expired);
    }
    let msg = match &st.batch {
        None => t.canonical_bytes(),
        Some(list) => {
            if !list.contains(&t.digest()) {
                return Err(VerifyError::NotInBatch);
            }
            batch_bytes(list)
        }
    };
    let role = verify_sig(&st.signature, &msg, trusted)?;
    if !role_may_sign(t.tier, role) {
        return Err(VerifyError::WrongRole { need: SignerRole::User });
    }
    Ok(())
}

/// 软件钥匙：只给测试用，冒充安全芯片。正式构建里没有这个模块。
#[cfg(any(test, feature = "soft-signer"))]
pub mod soft {
    use super::*;
    use p256::ecdsa::{signature::Signer as _, SigningKey};

    pub struct SoftSigner {
        role: SignerRole,
        key: SigningKey,
    }

    impl SoftSigner {
        /// 固定的私钥，测试结果可复现。不同 `seed` 得到不同的钥匙。
        pub fn from_seed(role: SignerRole, seed: u8) -> Self {
            let mut b = [0x11u8; 32];
            b[31] = seed.max(1);
            SoftSigner {
                role,
                key: SigningKey::from_slice(&b).expect("valid scalar"),
            }
        }

        pub fn trusted(&self) -> TrustedKey {
            TrustedKey {
                role: self.role,
                public_key: self.public_key(),
            }
        }
    }

    impl Signer for SoftSigner {
        fn role(&self) -> SignerRole {
            self.role
        }

        fn public_key(&self) -> [u8; 65] {
            let p = self.key.verifying_key().to_encoded_point(false);
            let mut a = [0u8; 65];
            a.copy_from_slice(p.as_bytes());
            a
        }

        fn sign(&self, msg: &[u8], _reason: &str) -> Result<[u8; 64], SignError> {
            let s: EcSig = self.key.sign(msg);
            let mut a = [0u8; 64];
            a.copy_from_slice(&s.to_bytes());
            Ok(a)
        }
    }
}
```

`lib.rs` 加 `pub mod sign;`。如果 `p256 0.13` 的某个方法名跟上面不一样（例如 `from_slice`、`to_encoded_point`），以 docs.rs 上 `p256 0.13` / `ecdsa 0.16` 的文档为准改调用，**不要换库、不要加带 C 的依赖**。

- [ ] **Step 4: 跑测试，确认通过；确认没有引入 C**

Run: `cargo test -p dct-brain && cargo tree -p dct-brain -e normal | grep -iE "\bcc v|openssl|ring v|-sys v" ; echo "exit=$?"`
Expected: 测试全部 PASS；`cargo tree` 那一段什么都没打印（`exit=1`），也就是依赖树里没有 C。

- [ ] **Step 5: Commit**

```bash
git add crates/dct-brain Cargo.lock
git commit -m "feat(brain): sign and verify tickets with P-256, one signature for a batch"
```

---

