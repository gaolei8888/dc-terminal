//! 一条留言：先用发件方的签名钥匙签一份规范字节（`dct-msg-v1`），把
//! `{"m": <消息>, "sig": <签名>}` 序列化成 JSON，再用收件方的 X25519 公钥
//! 加密——中转看不到 `kind`/`body`/会话名这些内容，但**路由本身不保密**：
//! 中转要知道这条 `Sealed` 是发给哪个 endpoint 的才能转发，`open` 也要调用
//! 方在信封上另外附一个 `envelope_from`（见下）——所以中转看得到、也必须看
//! 得到收发双方是谁，密文只藏内容，不藏地址。
//!
//! 加密用的是「每条消息一把新临时钥匙」的 ECIES：调用方给一个 `eph_seed`
//! （这个 crate 不生成随机数，见 lib.rs）。真正喂给 X25519 的临时私钥不是
//! `eph_seed` 本身，而是 `SHA-256("dct-seal-eph-v1" ‖ eph_seed ‖ plain)`——
//! 把加密前的明文也拌进去，这样即使调用方不小心对两条**不同**的消息传了同
//! 一个种子，算出来的临时私钥也不同（明文不同，摘要就不同）。但这不是纵容
//! 调用方省事：`eph_seed` 仍然必须每次调用都用新鲜的随机数——两条内容完全
//! 相同的消息如果还用同一个种子，会算出一模一样的密文，等于告诉中转「这是
//! 同一条消息又发了一遍」，这是这份哈希绑定防不住的。
//!
//! 临时私钥跟收件方的公钥做 Diffie-Hellman，再用 HKDF-SHA256 把共享点拉成
//! 一把 ChaCha20-Poly1305 密钥。nonce 固定用全 0——这是安全的，因为派生出来
//! 的对称密钥已经绑死了这一条消息的明文，不会有「同一把密钥、两条不同明文
//! 共用同一个 nonce」这种 ChaCha20-Poly1305 真正怕的情况。
use crate::canon::field;
use crate::id;
use crate::keys::{self, MachineKeys};
use crate::roster::Roster;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey as KxPublicKey, StaticSecret};

pub const MSG_VERSION_TAG: &str = "dct-msg-v1";

/// 消息种类，JSON 里用全小写（`Msg` -> `"msg"`，`StatusRequest` ->
/// `"statusrequest"`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Msg,
    Receipt,
    StatusRequest,
    Status,
}

/// `from`/`to` 是收发双方的 endpoint，`from_session`/`to_session` 是各自机器
/// 上具体是哪个终端会话（比如 `dc-terminal`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub kind: Kind,
    pub from: String,
    pub from_session: String,
    pub to: String,
    pub to_session: String,
    pub body: String,
    pub sent_at: u64,
}

/// 内层（加密前）的 JSON 形状：消息本体 + 发件方对它的签名。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignedMessage {
    m: Message,
    sig: String,
}

/// 密封后的留言：`v` 是格式版本，`eph` 是这条消息专用的一次性 X25519 公钥
/// （标准 base64），`ct` 是密文（标准 base64，带 Poly1305 认证标签）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    pub v: u32,
    pub eph: String,
    pub ct: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    /// 密文解不开——钥匙不对、被中转改过，`eph`/`ct` 根本不是合法的
    /// base64/密文，`v` 不是这份代码认识的版本，或者 `eph` 是一个会让
    /// Diffie-Hellman 算出全零共享密钥的恶意/退化公钥。
    Decrypt,
    /// 解密出来的明文不是 `SignedMessage` 期望的 JSON 形状。
    BadJson,
    /// `roster` 里没有 `m.from` 这个 endpoint（或者有，但它的 `endpoint` 跟
    /// 自己的 `sign_pub` 对不上，不能当成真的绑定）。
    UnknownSender,
    /// 用发件方在名单里的公钥验签，验不过。
    BadSignature,
    /// 中转传来的信封发件人跟签过名的 `m.from` 不一致——中转换了发件人。
    FromMismatch,
    /// `m.to` 不是我自己的 endpoint。
    NotForMe,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OpenError::Decrypt => "message could not be decrypted",
            OpenError::BadJson => "decrypted payload is not a valid message",
            OpenError::UnknownSender => "sender is not a bound member of the roster",
            OpenError::BadSignature => "message signature does not verify",
            OpenError::FromMismatch => "envelope sender does not match the signed sender",
            OpenError::NotForMe => "message is not addressed to me",
        })
    }
}

impl std::error::Error for OpenError {}

/// `dct-msg-v1` 规范字节：依次写 `field("dct-msg-v1")`、`id`、`kind`、
/// `from`、`from_session`、`to`、`to_session`、`body`、`sent_at`。`kind` 走
/// serde 的小写编码，跟 `Kind` 的 `#[serde(rename_all = "lowercase")]` 是
/// 同一份真相——不在这里另抄一份映射表，免得两边改一处漏一处却测不出来。
pub fn bytes(m: &Message) -> Vec<u8> {
    let mut out = Vec::new();
    field(&mut out, MSG_VERSION_TAG);
    field(&mut out, &m.id);
    field(&mut out, &kind_str(m.kind));
    field(&mut out, &m.from);
    field(&mut out, &m.from_session);
    field(&mut out, &m.to);
    field(&mut out, &m.to_session);
    field(&mut out, &m.body);
    field(&mut out, &m.sent_at.to_string());
    out
}

fn kind_str(k: Kind) -> String {
    let quoted = serde_json::to_string(&k).expect("Kind always serializes");
    quoted.trim_matches('"').to_string()
}

const SEAL_INFO: &[u8] = b"dct-seal-v1";
const EPH_INFO: &[u8] = b"dct-seal-eph-v1";

fn derive_key(eph_pub: &[u8; 32], to_kx_pub: &[u8; 32], shared: &[u8; 32]) -> [u8; 32] {
    let mut salt = Vec::with_capacity(64);
    salt.extend_from_slice(eph_pub);
    salt.extend_from_slice(to_kx_pub);
    let hk = Hkdf::<Sha256>::new(Some(&salt), shared);
    let mut key = [0u8; 32];
    hk.expand(SEAL_INFO, &mut key)
        .expect("32 bytes is a valid SHA-256 HKDF output length");
    key
}

/// 临时 X25519 私钥不是 `eph_seed` 本身，而是把它跟（加密前的）明文一起做
/// 一次 SHA-256：`SHA-256("dct-seal-eph-v1" || eph_seed || plain)`。见模块
/// 顶部的注释——这样同一个种子配不同的消息也不会撞出同一把临时钥匙，但
/// `eph_seed` 本身仍然必须是调用方每次给的新鲜随机数。
fn ephemeral_secret(eph_seed: [u8; 32], plain: &[u8]) -> StaticSecret {
    let mut hasher = Sha256::new();
    hasher.update(EPH_INFO);
    hasher.update(eph_seed);
    hasher.update(plain);
    let digest = hasher.finalize();
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&digest);
    StaticSecret::from(seed)
}

/// 签名并加密一条消息，只有掌握 `to_kx_pub` 对应私钥的那台电脑能打开。
pub fn seal(m: &Message, me: &MachineKeys, to_kx_pub: &[u8; 32], eph_seed: [u8; 32]) -> Sealed {
    let sig = me.sign(&bytes(m));
    let inner = SignedMessage {
        m: m.clone(),
        sig: STANDARD.encode(sig),
    };
    let plain = serde_json::to_vec(&inner).expect("SignedMessage always serializes");

    let eph = ephemeral_secret(eph_seed, &plain);
    let eph_pub = KxPublicKey::from(&eph);
    let their_pub = KxPublicKey::from(*to_kx_pub);
    let shared = eph.diffie_hellman(&their_pub);

    let key_bytes = derive_key(eph_pub.as_bytes(), to_kx_pub, shared.as_bytes());
    let key = Key::from_slice(&key_bytes);
    let cipher = ChaCha20Poly1305::new(key);
    // 全 0 nonce 在这里是安全的：`eph_seed` 每条消息都不同，派生出来的对称
    // 密钥也就每条消息都不同——同一把密钥只会被用来加密恰好一条消息、恰好
    // 用这一个 nonce。ChaCha20-Poly1305 真正危险的是「同一把密钥」配「不同
    // 消息」却复用了 nonce，而这里密钥本身就是一次性的，这种情况不会发生。
    let nonce = Nonce::from_slice(&[0u8; 12]);
    let ct = cipher
        .encrypt(
            nonce,
            Payload {
                msg: &plain,
                aad: SEAL_INFO,
            },
        )
        .expect("encryption with a freshly derived key/nonce cannot fail");

    Sealed {
        v: 1,
        eph: STANDARD.encode(eph_pub.as_bytes()),
        ct: STANDARD.encode(ct),
    }
}

/// 在 `roster` 里找 `endpoint` 对应的签名公钥，同时校验这条名单记录的
/// `endpoint` 真的是从它自己的 `sign_pub` 算出来的——`roster` 这里不保证是
/// 已经过 `roster::accept` 校验过的名单，所以这条绑定必须在这里自己查一遍，
/// 不能假设调用方已经查过。任何解不出来的情况都当成「没有这个发件人」。
fn bound_sender_key(roster: &Roster, endpoint: &str) -> Option<[u8; 65]> {
    let member = roster.member(endpoint)?;
    let raw = STANDARD.decode(&member.sign_pub).ok()?;
    let sign_pub: [u8; 65] = raw.try_into().ok()?;
    if id::endpoint_for(&sign_pub) != endpoint {
        return None;
    }
    Some(sign_pub)
}

/// 打开一条密封留言。`envelope_from` 是中转在这条消息外面附的「这是谁发的」
/// 元数据（不是加密内容的一部分），必须跟解密后签过名的 `m.from` 一致。
///
/// **这个函数不做重放保护。** 它只管「这条留言是不是真的、没被改过、是发
/// 给我的」，不管「我是不是已经处理过这一条了」——两次 `open` 同一个
/// `Sealed` 会两次成功返回同一个 `Message`。按 `(m.from, m.id)` 去重、再给
/// `m.sent_at` 卡一个时间窗口，这是调用方（存留言的那一层）的活。
pub fn open(
    s: &Sealed,
    me: &MachineKeys,
    roster: &Roster,
    envelope_from: &str,
) -> Result<Message, OpenError> {
    if s.v != 1 {
        return Err(OpenError::Decrypt);
    }

    let eph_raw = STANDARD.decode(&s.eph).map_err(|_| OpenError::Decrypt)?;
    let eph_pub: [u8; 32] = eph_raw.try_into().map_err(|_| OpenError::Decrypt)?;
    let ct = STANDARD.decode(&s.ct).map_err(|_| OpenError::Decrypt)?;

    let shared = me.diffie_hellman(&eph_pub);
    if shared == [0u8; 32] {
        // `eph` 是攻击者能自由构造的（它就是密文旁边那个"临时公钥"字段）。
        // 用一个退化/低阶点（比如全零）当公钥，可以让 Diffie-Hellman 不管
        // 我的私钥是什么都算出全零共享密钥——不拒绝的话，攻击者就拿到了一
        // 把跟我的私钥完全脱钩、自己就能算出来的"共享密钥"。
        return Err(OpenError::Decrypt);
    }
    let my_kx_pub = me.kx_pub();
    let key_bytes = derive_key(&eph_pub, &my_kx_pub, &shared);
    let key = Key::from_slice(&key_bytes);
    let cipher = ChaCha20Poly1305::new(key);
    let nonce = Nonce::from_slice(&[0u8; 12]);
    let plain = cipher
        .decrypt(
            nonce,
            Payload {
                msg: &ct,
                aad: SEAL_INFO,
            },
        )
        .map_err(|_| OpenError::Decrypt)?;

    let signed: SignedMessage = serde_json::from_slice(&plain).map_err(|_| OpenError::BadJson)?;

    let sig_raw = STANDARD
        .decode(&signed.sig)
        .map_err(|_| OpenError::BadSignature)?;
    let sig: [u8; 64] = sig_raw.try_into().map_err(|_| OpenError::BadSignature)?;

    let sender_key = bound_sender_key(roster, &signed.m.from).ok_or(OpenError::UnknownSender)?;
    if !keys::verify(&sender_key, &bytes(&signed.m), &sig) {
        return Err(OpenError::BadSignature);
    }

    if signed.m.from != envelope_from {
        return Err(OpenError::FromMismatch);
    }

    let my_endpoint = id::endpoint_for(&me.sign_pub());
    if signed.m.to != my_endpoint {
        return Err(OpenError::NotForMe);
    }

    Ok(signed.m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::Member;

    fn keys_for(byte: u8) -> MachineKeys {
        MachineKeys::from_seeds([byte.max(1); 32], [byte.max(1); 32]).unwrap()
    }

    fn member_from(k: &MachineKeys, name: &str) -> Member {
        Member {
            name: name.into(),
            endpoint: id::endpoint_for(&k.sign_pub()),
            sign_pub: STANDARD.encode(k.sign_pub()),
            kx_pub: STANDARD.encode(k.kx_pub()),
            added_at: 0,
        }
    }

    fn roster_of(members: Vec<Member>) -> Roster {
        Roster {
            group: "mine".into(),
            version: 1,
            members,
        }
    }

    fn sample_message(from: &str, to: &str) -> Message {
        Message {
            id: "msg-1".into(),
            kind: Kind::Msg,
            from: from.into(),
            from_session: "laptop/dc-terminal".into(),
            to: to.into(),
            to_session: "desktop/dc-terminal".into(),
            body: "hello".into(),
            sent_at: 1234,
        }
    }

    #[test]
    fn sealed_message_opens_for_the_recipient_and_round_trips() {
        let alice = keys_for(1);
        let bob = keys_for(2);
        let a = member_from(&alice, "alice");
        let roster = roster_of(vec![a.clone(), member_from(&bob, "bob")]);

        let m = sample_message(&a.endpoint, &id::endpoint_for(&bob.sign_pub()));
        let sealed = seal(&m, &alice, &bob.kx_pub(), [7u8; 32]);

        let opened = open(&sealed, &bob, &roster, &a.endpoint).expect("should open");
        assert_eq!(opened, m);
    }

    #[test]
    fn a_different_recipient_cannot_open_it() {
        let alice = keys_for(1);
        let bob = keys_for(2);
        let carol = keys_for(3);
        let a = member_from(&alice, "alice");
        let roster = roster_of(vec![a.clone(), member_from(&bob, "bob")]);

        let m = sample_message(&a.endpoint, &id::endpoint_for(&bob.sign_pub()));
        let sealed = seal(&m, &alice, &bob.kx_pub(), [7u8; 32]);

        assert_eq!(
            open(&sealed, &carol, &roster, &a.endpoint),
            Err(OpenError::Decrypt)
        );
    }

    #[test]
    fn a_sender_not_in_the_roster_is_refused() {
        let alice = keys_for(1);
        let bob = keys_for(2);
        let a = member_from(&alice, "alice");
        // bob 的名单里没有 alice。
        let roster = roster_of(vec![member_from(&bob, "bob")]);

        let m = sample_message(&a.endpoint, &id::endpoint_for(&bob.sign_pub()));
        let sealed = seal(&m, &alice, &bob.kx_pub(), [7u8; 32]);

        assert_eq!(
            open(&sealed, &bob, &roster, &a.endpoint),
            Err(OpenError::UnknownSender)
        );
    }

    #[test]
    fn a_roster_entry_whose_endpoint_does_not_match_its_own_key_is_refused() {
        // `roster.member(endpoint)` looks entries up *by* `endpoint`, so a
        // record whose `endpoint` field doesn't match its own key can only
        // be found by looking it up under the (wrong) `endpoint` it claims —
        // which is exactly what happens here. This is the scenario the
        // binding check inside `bound_sender_key` exists to catch: without
        // it, whoever controls `mallory`'s key could sign as `x` and have it
        // accepted, because the roster entry *claims* `x` even though it was
        // never bound to `x`'s real key.
        let alice = keys_for(1);
        let bob = keys_for(2);
        let mallory = keys_for(3);
        let x = id::endpoint_for(&alice.sign_pub());

        let forged = Member {
            name: "alice".into(),
            endpoint: x.clone(),
            sign_pub: STANDARD.encode(mallory.sign_pub()),
            kx_pub: STANDARD.encode(mallory.kx_pub()),
            added_at: 0,
        };
        let roster = roster_of(vec![forged, member_from(&bob, "bob")]);

        // Mallory crafts and signs (with her own real key) a message that
        // claims to be from `x`.
        let m = sample_message(&x, &id::endpoint_for(&bob.sign_pub()));
        let sealed = seal(&m, &mallory, &bob.kx_pub(), [7u8; 32]);

        assert_eq!(
            open(&sealed, &bob, &roster, &x),
            Err(OpenError::UnknownSender)
        );
    }

    #[test]
    fn a_message_forged_by_another_member_claiming_to_be_someone_else_is_refused() {
        // Alice is a genuine, correctly bound roster member. Mallory (also a
        // genuine roster member, under her own key) crafts a message whose
        // `from` field claims to be Alice, and signs it with her own key
        // instead. The roster lookup succeeds (Alice's real entry), so this
        // can only be caught by verifying the signature against Alice's key
        // — which must fail, because it was never made by Alice.
        let alice = keys_for(1);
        let bob = keys_for(2);
        let mallory = keys_for(3);
        let a = member_from(&alice, "alice");
        let roster = roster_of(vec![
            a.clone(),
            member_from(&bob, "bob"),
            member_from(&mallory, "mallory"),
        ]);

        let m = sample_message(&a.endpoint, &id::endpoint_for(&bob.sign_pub()));
        let sealed = seal(&m, &mallory, &bob.kx_pub(), [7u8; 32]);

        assert_eq!(
            open(&sealed, &bob, &roster, &a.endpoint),
            Err(OpenError::BadSignature)
        );
    }

    #[test]
    fn an_unsupported_sealed_version_is_rejected() {
        let alice = keys_for(1);
        let bob = keys_for(2);
        let a = member_from(&alice, "alice");
        let roster = roster_of(vec![a.clone(), member_from(&bob, "bob")]);

        let m = sample_message(&a.endpoint, &id::endpoint_for(&bob.sign_pub()));
        let mut sealed = seal(&m, &alice, &bob.kx_pub(), [7u8; 32]);
        sealed.v = 2;

        assert_eq!(
            open(&sealed, &bob, &roster, &a.endpoint),
            Err(OpenError::Decrypt)
        );
    }

    #[test]
    fn an_all_zero_shared_secret_is_rejected() {
        let alice = keys_for(1);
        let bob = keys_for(2);
        let roster = roster_of(vec![member_from(&alice, "alice"), member_from(&bob, "bob")]);

        // `[0u8; 32]` is a well-known low-order X25519 public key: the DH
        // result is all-zero regardless of the recipient's private key.
        let forged = Sealed {
            v: 1,
            eph: STANDARD.encode([0u8; 32]),
            ct: STANDARD.encode([0u8; 16]),
        };

        assert_eq!(
            open(&forged, &bob, &roster, &id::endpoint_for(&alice.sign_pub())),
            Err(OpenError::Decrypt)
        );
    }

    #[test]
    fn an_all_zero_shared_secret_is_rejected_even_when_the_ciphertext_would_otherwise_open() {
        // The test above uses garbage ciphertext, so it can't tell the
        // explicit all-zero-shared-secret check apart from the AEAD tag
        // just happening to fail anyway. This test crafts ciphertext that
        // genuinely *would* decrypt and verify successfully under the key
        // the all-zero shared secret derives to, so it only passes if the
        // explicit check is the thing doing the rejecting.
        let alice = keys_for(1);
        let bob = keys_for(2);
        let a = member_from(&alice, "alice");
        let roster = roster_of(vec![a.clone(), member_from(&bob, "bob")]);

        let m = sample_message(&a.endpoint, &id::endpoint_for(&bob.sign_pub()));
        let sig = alice.sign(&bytes(&m));
        let inner = SignedMessage {
            m: m.clone(),
            sig: STANDARD.encode(sig),
        };
        let plain = serde_json::to_vec(&inner).unwrap();

        // Same key derivation `open` would use for this `eph`/`to_kx_pub`
        // pair *if* it didn't reject an all-zero shared secret first.
        let eph_pub = [0u8; 32];
        let key_bytes = derive_key(&eph_pub, &bob.kx_pub(), &[0u8; 32]);
        let key = Key::from_slice(&key_bytes);
        let cipher = ChaCha20Poly1305::new(key);
        let nonce = Nonce::from_slice(&[0u8; 12]);
        let ct = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: &plain,
                    aad: SEAL_INFO,
                },
            )
            .unwrap();

        let forged = Sealed {
            v: 1,
            eph: STANDARD.encode(eph_pub),
            ct: STANDARD.encode(ct),
        };

        assert_eq!(
            open(&forged, &bob, &roster, &a.endpoint),
            Err(OpenError::Decrypt)
        );
    }

    #[test]
    fn envelope_from_must_match_the_signed_from() {
        let alice = keys_for(1);
        let bob = keys_for(2);
        let mallory = keys_for(3);
        let a = member_from(&alice, "alice");
        let roster = roster_of(vec![
            a.clone(),
            member_from(&bob, "bob"),
            member_from(&mallory, "mallory"),
        ]);

        let m = sample_message(&a.endpoint, &id::endpoint_for(&bob.sign_pub()));
        let sealed = seal(&m, &alice, &bob.kx_pub(), [7u8; 32]);

        // 中转把信封上的发件人换成了 mallory，但密文里签的还是 alice。
        let mallory_endpoint = id::endpoint_for(&mallory.sign_pub());
        assert_eq!(
            open(&sealed, &bob, &roster, &mallory_endpoint),
            Err(OpenError::FromMismatch)
        );
    }

    #[test]
    fn a_message_addressed_to_another_machine_is_refused() {
        let alice = keys_for(1);
        let bob = keys_for(2);
        let carol = keys_for(3);
        let a = member_from(&alice, "alice");
        let roster = roster_of(vec![a.clone(), member_from(&bob, "bob")]);

        // 消息本来是发给 carol 的，但被密封给了 bob 的公钥。
        let m = sample_message(&a.endpoint, &id::endpoint_for(&carol.sign_pub()));
        let sealed = seal(&m, &alice, &bob.kx_pub(), [7u8; 32]);

        assert_eq!(
            open(&sealed, &bob, &roster, &a.endpoint),
            Err(OpenError::NotForMe)
        );
    }

    #[test]
    fn flipping_one_ciphertext_byte_fails() {
        let alice = keys_for(1);
        let bob = keys_for(2);
        let a = member_from(&alice, "alice");
        let roster = roster_of(vec![a.clone(), member_from(&bob, "bob")]);

        let m = sample_message(&a.endpoint, &id::endpoint_for(&bob.sign_pub()));
        let mut sealed = seal(&m, &alice, &bob.kx_pub(), [7u8; 32]);

        let mut ct = STANDARD.decode(&sealed.ct).unwrap();
        ct[0] ^= 0x01;
        sealed.ct = STANDARD.encode(ct);

        assert_eq!(
            open(&sealed, &bob, &roster, &a.endpoint),
            Err(OpenError::Decrypt)
        );
    }

    #[test]
    fn two_seals_of_the_same_message_differ_with_different_eph_seeds() {
        let alice = keys_for(1);
        let bob = keys_for(2);
        let a = member_from(&alice, "alice");

        let m = sample_message(&a.endpoint, &id::endpoint_for(&bob.sign_pub()));
        let s1 = seal(&m, &alice, &bob.kx_pub(), [7u8; 32]);
        let s2 = seal(&m, &alice, &bob.kx_pub(), [8u8; 32]);

        assert_ne!(s1.eph, s2.eph);
        assert_ne!(s1.ct, s2.ct);
    }

    #[test]
    fn same_eph_seed_but_a_different_message_gives_a_different_ephemeral_key() {
        // The ephemeral private key is derived from `eph_seed` *and* the
        // plaintext (see the module doc), specifically so that a caller
        // accidentally reusing a seed across two different messages still
        // gets a fresh key each time.
        let alice = keys_for(1);
        let bob = keys_for(2);
        let a = member_from(&alice, "alice");

        let m1 = sample_message(&a.endpoint, &id::endpoint_for(&bob.sign_pub()));
        let mut m2 = m1.clone();
        m2.body = "a different message entirely".into();

        let s1 = seal(&m1, &alice, &bob.kx_pub(), [7u8; 32]);
        let s2 = seal(&m2, &alice, &bob.kx_pub(), [7u8; 32]);

        assert_ne!(s1.eph, s2.eph);
    }

    #[test]
    fn seal_matches_a_known_answer_vector() {
        // Pins the full crypto pipeline (ephemeral key derivation, HKDF,
        // ChaCha20-Poly1305) for a fixed set of inputs, so a change to any
        // step's constants/ordering shows up as a diff here instead of only
        // as "round-trips still work" (which a compatible-on-both-sides bug
        // would never catch).
        let alice = MachineKeys::from_seeds([1u8; 32], [2u8; 32]).unwrap();
        let bob = MachineKeys::from_seeds([3u8; 32], [4u8; 32]).unwrap();
        let m = Message {
            id: "kat-1".into(),
            kind: Kind::Msg,
            from: id::endpoint_for(&alice.sign_pub()),
            from_session: "laptop/dc-terminal".into(),
            to: id::endpoint_for(&bob.sign_pub()),
            to_session: "desktop/dc-terminal".into(),
            body: "known answer".into(),
            sent_at: 1_700_000_000,
        };
        let sealed = seal(&m, &alice, &bob.kx_pub(), [9u8; 32]);

        assert_eq!(sealed.eph, "2o+txP0fptgL9khgRgMoteHSoimDytrPGNjUSL8ytSg=");
        assert_eq!(
            sealed.ct,
            "Z5juSC8J3a51ggq2BqI/eKrUgZQ2nPLBksI+IqIf4jNaGgvRu4U/cfvHhax6xdXpYXF3ybTqQOqgx30j\
             /ofubdwJDLw49t8eqcqnD4JSNleXerU3ndG4mcPoswwekYxd5esEwI0FfKjW0hHag/zrMnPDE4VxzWWs\
             c1HCsEp5MZsPAdydxfVDoSV0prgGjtfMHGhFpkXJ6GJo0+kJS2Y4kGgIse35WMWE3yLBG9vruld8dKwN\
             O5dGDKwt9fdHDleCe7Kj4PCw2TMv8VGve4VpQU4Cjuy+effrivxxaAhwsnxHblnrYuWK7JzalbB1dNc0\
             eaRTaUlisi09OdTPIy93NeyTjXY8//RZxaqAoybC6pNfnB9mSnb8mIQJHp+KMA+5DBqZ2d3bVxEQxgPR\
             ov+3IOPjSVA/gEsvSHLGEl3VNz0/fA=="
        );
    }

    #[test]
    fn kind_serializes_to_lowercase_json_strings() {
        assert_eq!(serde_json::to_string(&Kind::Msg).unwrap(), "\"msg\"");
        assert_eq!(
            serde_json::to_string(&Kind::Receipt).unwrap(),
            "\"receipt\""
        );
        assert_eq!(
            serde_json::to_string(&Kind::StatusRequest).unwrap(),
            "\"statusrequest\""
        );
        assert_eq!(serde_json::to_string(&Kind::Status).unwrap(), "\"status\"");
    }

    #[test]
    fn message_bytes_are_length_prefixed_fields_in_order() {
        let m = sample_message("c-from", "c-to");
        let out = bytes(&m);
        let mut expected = Vec::new();
        field(&mut expected, MSG_VERSION_TAG);
        field(&mut expected, "msg-1");
        field(&mut expected, "msg");
        field(&mut expected, "c-from");
        field(&mut expected, "laptop/dc-terminal");
        field(&mut expected, "c-to");
        field(&mut expected, "desktop/dc-terminal");
        field(&mut expected, "hello");
        field(&mut expected, "1234");
        assert_eq!(out, expected);
    }
}
