//! 一条留言：先用发件方的签名钥匙签一份规范字节（`dct-msg-v1`），把
//! `{"m": <消息>, "sig": <签名>}` 序列化成 JSON，再用收件方的 X25519 公钥
//! 加密——中转只能看见密文和一个临时公钥，看不到消息内容、收发双方是谁。
//!
//! 加密用的是「每条消息一把新临时钥匙」的 ECIES：调用方给一个 `eph_seed`
//! （这个 crate 不生成随机数，见 lib.rs），派生出一次性的 X25519 密钥对，跟
//! 收件方的公钥做 Diffie-Hellman，再用 HKDF-SHA256 把共享点拉成一把
//! ChaCha20-Poly1305 密钥。nonce 固定用全 0——这是安全的，因为每条消息的
//! 临时密钥都是新算出来的，同一把密钥只会被用来加密这一条消息、只用这一个
//! nonce，ChaCha20-Poly1305 真正要防的「同一把密钥、不同消息复用 nonce」的
//! 场景根本不会发生。
use crate::canon::field;
use crate::id;
use crate::keys::{self, MachineKeys};
use crate::roster::Roster;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
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
    /// 密文解不开——钥匙不对、被中转改过，或者 `eph`/`ct` 根本不是合法的
    /// base64/密文。
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

/// 签名并加密一条消息，只有掌握 `to_kx_pub` 对应私钥的那台电脑能打开。
pub fn seal(m: &Message, me: &MachineKeys, to_kx_pub: &[u8; 32], eph_seed: [u8; 32]) -> Sealed {
    let sig = me.sign(&bytes(m));
    let inner = SignedMessage {
        m: m.clone(),
        sig: STANDARD.encode(sig),
    };
    let plain = serde_json::to_vec(&inner).expect("SignedMessage always serializes");

    let eph = StaticSecret::from(eph_seed);
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
pub fn open(
    s: &Sealed,
    me: &MachineKeys,
    roster: &Roster,
    envelope_from: &str,
) -> Result<Message, OpenError> {
    let eph_raw = STANDARD.decode(&s.eph).map_err(|_| OpenError::Decrypt)?;
    let eph_pub: [u8; 32] = eph_raw.try_into().map_err(|_| OpenError::Decrypt)?;
    let ct = STANDARD.decode(&s.ct).map_err(|_| OpenError::Decrypt)?;

    let shared = me.diffie_hellman(&eph_pub);
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
    fn a_sender_whose_roster_entry_fails_endpoint_binding_is_refused() {
        let alice = keys_for(1);
        let bob = keys_for(2);
        let mut a = member_from(&alice, "alice");
        // 名单里的记录被人改了：endpoint 不再是从 alice 的 sign_pub 算出来的。
        let real_from = a.endpoint.clone();
        a.endpoint = "c-0000000000000000000f".into();
        let roster = roster_of(vec![a, member_from(&bob, "bob")]);

        let m = sample_message(&real_from, &id::endpoint_for(&bob.sign_pub()));
        let sealed = seal(&m, &alice, &bob.kx_pub(), [7u8; 32]);

        assert_eq!(
            open(&sealed, &bob, &roster, &real_from),
            Err(OpenError::UnknownSender)
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
