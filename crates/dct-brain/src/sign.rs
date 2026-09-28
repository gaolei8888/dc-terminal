//! 签名和验签。算法是 ECDSA P-256 + SHA-256（「es256」）：Mac 安全芯片、手机 passkey
//! 签出来的都是它，纯 Rust 实现，没有 C。
//!
//! 验签只回答「这张票是不是配对过的钥匙签的、档位够不够、设备和时间对不对」。
//! **一次性编号不在这里查**：通过之后由调用方（dco）记住 nonce，直到票过期。
//!
//! **防重放认 nonce（或票的 digest），绝不能认签名字节或签过的 JSON 的哈希**：
//! P-256 的 `s` 取 `s` 或 `n-s` 对同一条消息都是合法签名（CryptoKit / WebAuthn
//! 都不保证只出低 S），同一张票能有不止一个合法签名字节串。按签名字节去重，
//! 一次已批准的发布或付款就可能被重放第二次。
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
    /// 同一把公钥在 `trusted` 里配对了不止一次（哪怕角色不同）：按 key_id 认不出该给
    /// 哪个角色，不猜、不按列表顺序挑，直接拒。
    AmbiguousKey,
    WrongRole { need: SignerRole },
    BadSignature,
    NotInBatch,
    WrongDevice,
    TooEarly,
    Expired,
    /// 批准记录对应的流程步骤已经变了（Task 6）。
    StepsChanged,
    /// 批准记录是给另一条流程的。
    WrongProcedure,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            VerifyError::Unsupported => "票的版本或算法不认识",
            VerifyError::UnknownKey => "签名的钥匙没配对过",
            VerifyError::AmbiguousKey => "同一把钥匙配对了不止一次，不认",
            VerifyError::WrongRole { .. } => "这一档要用户本人签名",
            VerifyError::BadSignature => "签名不对",
            VerifyError::NotInBatch => "这张票不在签过的那一批里",
            VerifyError::WrongDevice => "这张票不是给这台设备的",
            VerifyError::TooEarly => "还没到可以执行的时间",
            VerifyError::Expired => "票已经过期",
            VerifyError::StepsChanged => "流程的步骤改过了，要重新批准",
            VerifyError::WrongProcedure => "这份批准记录是给另一条流程的",
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
    let mut matching = trusted.iter().filter(|k| key_id(&k.public_key) == sig.key_id);
    let key = matching.next().ok_or(VerifyError::UnknownKey)?;
    // 同一把钥匙配对了不止一次（哪怕角色不同）：不按列表顺序挑一个，直接拒。
    if matching.next().is_some() {
        return Err(VerifyError::AmbiguousKey);
    }
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

/// 通过之后，重放要靠调用方按 `ticket.nonce`（或 `ticket.digest()`）记账，**不能**
/// 按 `signature.sig` 或签过的 JSON 的哈希去重——同一票的合法签名字节不止一种
/// （`s` 和 `n-s` 都对得上），按签名字节记账挡不住重放。
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

/// 测试用：对任意消息签一个 `Signature`，好在 crate 外面验证 `Signer` 的实现。
#[cfg(any(test, feature = "soft-signer"))]
pub fn make_signature_for_tests(s: &dyn Signer, msg: &[u8]) -> Signature {
    make_signature(s, msg, "").expect("sign")
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

    #[test]
    fn a_batch_signed_by_the_automatic_key_still_needs_the_users_key_for_outward_tickets() {
        let (auto, _, trusted) = keys();
        let batch = sign_batch(vec![ticket(Tier::SelfOnly, 'a'), ticket(Tier::Content, 'b')], &auto, "").unwrap();
        assert_eq!(
            verify(&batch[1], &trusted, "mac-lei", 150),
            Err(VerifyError::WrongRole { need: SignerRole::User })
        );
    }

    #[test]
    fn an_empty_batch_list_never_contains_the_ticket() {
        let (auto, _, trusted) = keys();
        let mut st = sign_one(ticket(Tier::SelfOnly, 'a'), &auto, "").unwrap();
        st.batch = Some(vec![]);
        assert_eq!(verify(&st, &trusted, "mac-lei", 150), Err(VerifyError::NotInBatch));
    }

    #[test]
    fn an_unknown_algorithm_is_unsupported() {
        let (auto, _, trusted) = keys();
        let mut st = sign_one(ticket(Tier::SelfOnly, 'a'), &auto, "").unwrap();
        st.signature.alg = "rs256".into();
        assert_eq!(verify(&st, &trusted, "mac-lei", 150), Err(VerifyError::Unsupported));
    }

    #[test]
    fn the_same_key_paired_under_two_roles_is_refused() {
        let auto = SoftSigner::from_seed(SignerRole::Auto, 1);
        let trusted = vec![
            TrustedKey { role: SignerRole::Auto, public_key: auto.public_key() },
            TrustedKey { role: SignerRole::User, public_key: auto.public_key() },
        ];
        let st = sign_one(ticket(Tier::SelfOnly, 'a'), &auto, "").unwrap();
        assert_eq!(verify(&st, &trusted, "mac-lei", 150), Err(VerifyError::AmbiguousKey));
    }

    #[test]
    fn high_s_twin_of_a_valid_signature_still_verifies() {
        // p256/ecdsa 不保证只出低 S：s 和 n-s 对同一条消息都是合法签名。verify 两个都要认，
        // 重放要靠调用方按 nonce/digest 挡，不能指望「签名字节唯一」。
        let (auto, _, trusted) = keys();
        let st = sign_one(ticket(Tier::SelfOnly, 'a'), &auto, "").unwrap();
        let raw = STANDARD.decode(&st.signature.sig).unwrap();
        let es = EcSig::from_slice(&raw).unwrap();
        let twin = EcSig::from_scalars(es.r(), -es.s()).expect("negated s is still a valid scalar");
        assert_ne!(twin.to_bytes(), es.to_bytes(), "twin must actually be the other root, not the same signature");

        let mut twinned = st.clone();
        twinned.signature.sig = STANDARD.encode(twin.to_bytes());
        assert_eq!(verify(&twinned, &trusted, "mac-lei", 150), Ok(()));
    }
}
