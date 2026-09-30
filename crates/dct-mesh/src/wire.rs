//! 中转（relay）上跑的消息线上形状：一台电脑发给中转的每一条 JSON 都是一个
//! `Payload`，`t` 字段说明它是密封留言、加入请求、整份名单、「你的加入申请
//! 收到了，这是我的随机数」，还是加入方揭晓自己的随机数。
//!
//! 加入的三步（先承诺、再揭晓，见 `sas` 模块头）：
//!
//! 1. 加入方 → 邀请方：`Join`（自签的成员记录 + 承诺，`ask`）；
//! 2. 邀请方 → 加入方：`JoinPending`（自签的成员记录 + 新鲜随机数，`ask` 的答复）；
//! 3. 加入方 → 邀请方：`JoinReveal`（自己的随机数，`send`）。
//!
//! 两边都要到第 3 步之后才有数字可亮。
use crate::canon::field;
use crate::id;
use crate::keys::{self, MachineKeys};
use crate::roster::{Member, SignedRoster};
use crate::seal::Sealed;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};

pub const JOIN_VERSION_TAG: &str = "dct-join-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Payload {
    Sealed(Sealed),
    Join(JoinRequest),
    Roster(SignedRoster),
    /// 邀请方的答复：自签的成员记录，和它为这一次加入新出的随机数
    /// （32 字节，标准 base64）。每收到一次 `Join` 就换一个。
    JoinPending {
        member: Member,
        sig: String,
        nonce: String,
    },
    /// 加入方揭晓 `Join` 里承诺过的随机数（32 字节，标准 base64）。发件
    /// 端点由中转认证，邀请方按它找到那一条请求。
    JoinReveal {
        nonce: String,
    },

    // —— 邀请码（dct-invite-v1，见 `invite` 模块头）。全走 `ask`：B 问，A 当场答。

    /// B → 每台同账号在线电脑：你手上有没有一个还有效的邀请码？不带任何东西，
    /// 也不消耗码。
    InviteProbe,
    /// A 的答复：有。`member`/`sig` 是 A 自签的成员记录（`sign_member`），
    /// `group` 是 A 的组 id——确认值 `T` 里要用，B 这时还没有 A 的名单。
    InviteOpen {
        member: Member,
        sig: String,
        group: String,
    },
    /// A 的答复：没有（没发过、过期了、用掉了、正有别人在用）。
    NoInvite,
    /// B → A：B 自签的成员记录，和 B 的 SPAKE2 消息（33 字节，标准 base64）。
    InviteJoin {
        member: Member,
        sig: String,
        spake: String,
    },
    /// A 的答复：A 的 SPAKE2 消息（33 字节）和确认值 `cA`（32 字节），都是标准 base64。
    InviteKey {
        spake: String,
        confirm: String,
    },
    /// B → A：确认值 `cB`（32 字节，标准 base64）。
    InviteFinish {
        confirm: String,
    },
    /// A 的答复：把 B 签进去的新名单。
    InviteDone {
        roster: SignedRoster,
    },
    /// A 的答复：不行。**不说为什么**——码错、过期、已被用掉、签名不对，
    /// 一律这一句（同 `Mesh::on_envelope` 的「验证失败都是沉默」）。
    InviteFailed,
}

/// 新电脑请求加入组：`member` 是它自己的名单条目（还没被任何人签认），
/// `sig` 是它用自己的钥匙对 `member` 的 `dct-join-v1` 规范字节签的名——证明
/// 它真的掌握 `member.sign_pub` 对应的私钥。ECDSA 签名可延展（见
/// `keys::MachineKeys::sign` 的文档），`sig` 不能当 id 或去重键用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinRequest {
    pub member: Member,
    pub sig: String,
    /// `sas::commit(member, 我的随机数)`，32 字节，标准 base64。随机数本身
    /// 等拿到邀请方的随机数之后才在 `JoinReveal` 里给。`sig` 不覆盖它：换掉
    /// 承诺的人拿不出能打开它的随机数，换了也只是让这一次加入对不上。
    pub commit: String,
}

/// 32 字节（随机数、承诺）写成标准 base64。
pub fn encode32(b: &[u8; 32]) -> String {
    STANDARD.encode(b)
}

/// `encode32` 的反面。不是恰好 32 字节的标准 base64 就是 `None`。
pub fn decode32(s: &str) -> Option<[u8; 32]> {
    STANDARD.decode(s).ok()?.try_into().ok()
}

/// 任意字节（SPAKE2 消息、确认值）写成标准 base64。
pub fn encode_bytes(b: &[u8]) -> String {
    STANDARD.encode(b)
}

/// `encode_bytes` 的反面，而且必须正好 `len` 字节，否则 `None`。
pub fn decode_len(s: &str, len: usize) -> Option<Vec<u8>> {
    STANDARD.decode(s).ok().filter(|b| b.len() == len)
}

pub fn encode(p: &Payload) -> Vec<u8> {
    serde_json::to_vec(p).expect("Payload always serializes")
}

pub fn decode(b: &[u8]) -> Result<Payload, serde_json::Error> {
    serde_json::from_slice(b)
}

/// `dct-join-v1` 规范字节：依次写 `field("dct-join-v1")`、`name`、
/// `endpoint`、`sign_pub`、`kx_pub`、`added_at`。`JoinRequest` 和
/// `JoinPending` 都是「一台电脑对自己的 `Member` 记录自签名」，共用这一份
/// 编码和下面这两个函数。
fn member_bytes(m: &Member) -> Vec<u8> {
    let mut out = Vec::new();
    field(&mut out, JOIN_VERSION_TAG);
    field(&mut out, &m.name);
    field(&mut out, &m.endpoint);
    field(&mut out, &m.sign_pub);
    field(&mut out, &m.kx_pub);
    field(&mut out, &m.added_at.to_string());
    out
}

/// 用 `keys` 对 `m` 自签名，返回标准 base64 的 `r||s`。
pub fn sign_member(m: &Member, keys: &MachineKeys) -> String {
    STANDARD.encode(keys.sign(&member_bytes(m)))
}

/// 校验 `sig` 是不是 `m` 自己的钥匙（`m.sign_pub`）对 `m` 签的名，并且
/// `m.endpoint` 真的是从 `m.sign_pub` 算出来的——单有签名不够：签名只证明
/// 「这份 `Member` 记录是这把 `sign_pub` 的主人认可的」，不证明它认领的
/// `endpoint` 就是自己的（一个合法成员完全可以自签一条 `endpoint` 写着别
/// 人地址的记录）。任何解不出来的 base64/公钥/签名都当作没验过，不 panic。
pub fn verify_member(m: &Member, sig: &str) -> bool {
    let Ok(raw_pub) = STANDARD.decode(&m.sign_pub) else {
        return false;
    };
    let Ok(sign_pub): Result<[u8; 65], _> = raw_pub.try_into() else {
        return false;
    };
    if id::endpoint_for(&sign_pub) != m.endpoint {
        return false;
    }
    let Ok(raw_sig) = STANDARD.decode(sig) else {
        return false;
    };
    let Ok(sig_arr): Result<[u8; 64], _> = raw_sig.try_into() else {
        return false;
    };
    keys::verify(&sign_pub, &member_bytes(m), &sig_arr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::Roster;

    fn keys_for(byte: u8) -> MachineKeys {
        MachineKeys::from_seeds([byte.max(1); 32], [byte.max(1); 32]).unwrap()
    }

    fn member_from(k: &MachineKeys, name: &str) -> Member {
        Member {
            name: name.into(),
            endpoint: id::endpoint_for(&k.sign_pub()),
            sign_pub: STANDARD.encode(k.sign_pub()),
            kx_pub: STANDARD.encode(k.kx_pub()),
            added_at: 42,
        }
    }

    #[test]
    fn payload_tags_are_pinned() {
        let k = keys_for(1);
        let m = member_from(&k, "laptop");

        let sealed = Payload::Sealed(Sealed {
            v: 1,
            eph: "eph".into(),
            ct: "ct".into(),
        });
        assert!(serde_json::to_string(&sealed)
            .unwrap()
            .contains("\"t\":\"sealed\""));

        let join = Payload::Join(JoinRequest {
            member: m.clone(),
            sig: "sig".into(),
            commit: "c".into(),
        });
        assert!(serde_json::to_string(&join)
            .unwrap()
            .contains("\"t\":\"join\""));

        let roster = Payload::Roster(SignedRoster {
            roster: Roster {
                group: "g".into(),
                version: 1,
                members: vec![m.clone()],
            },
            signer: m.endpoint.clone(),
            sig: "sig".into(),
        });
        assert!(serde_json::to_string(&roster)
            .unwrap()
            .contains("\"t\":\"roster\""));

        let pending = Payload::JoinPending {
            member: m,
            sig: "sig".into(),
            nonce: "n".into(),
        };
        assert!(serde_json::to_string(&pending)
            .unwrap()
            .contains("\"t\":\"join_pending\""));

        let reveal = Payload::JoinReveal { nonce: "n".into() };
        assert_eq!(
            serde_json::to_string(&reveal).unwrap(),
            r#"{"t":"join_reveal","nonce":"n"}"#
        );
    }

    /// 邀请码那 8 种的线上形状。改了就是跟别的版本的 dct 说不上话了。
    #[test]
    fn invite_payload_shapes_are_pinned() {
        let k = keys_for(1);
        let m = member_from(&k, "A");
        let r = SignedRoster {
            roster: Roster {
                group: "g".into(),
                version: 2,
                members: vec![m.clone()],
            },
            signer: m.endpoint.clone(),
            sig: "s".into(),
        };
        let cases = [
            (Payload::InviteProbe, r#"{"t":"invite_probe"}"#.to_string()),
            (Payload::NoInvite, r#"{"t":"no_invite"}"#.to_string()),
            (Payload::InviteFailed, r#"{"t":"invite_failed"}"#.to_string()),
            (
                Payload::InviteKey {
                    spake: "m".into(),
                    confirm: "c".into(),
                },
                r#"{"t":"invite_key","spake":"m","confirm":"c"}"#.to_string(),
            ),
            (
                Payload::InviteFinish { confirm: "c".into() },
                r#"{"t":"invite_finish","confirm":"c"}"#.to_string(),
            ),
        ];
        for (p, want) in &cases {
            assert_eq!(&serde_json::to_string(p).unwrap(), want);
            assert_eq!(&decode(want.as_bytes()).unwrap(), p);
        }
        for (p, tag) in [
            (
                Payload::InviteOpen {
                    member: m.clone(),
                    sig: "s".into(),
                    group: "g".into(),
                },
                r#""t":"invite_open""#,
            ),
            (
                Payload::InviteJoin {
                    member: m.clone(),
                    sig: "s".into(),
                    spake: "x".into(),
                },
                r#""t":"invite_join""#,
            ),
            (Payload::InviteDone { roster: r }, r#""t":"invite_done""#),
        ] {
            let json = serde_json::to_string(&p).unwrap();
            assert!(json.contains(tag), "{json}");
            assert_eq!(decode(json.as_bytes()).unwrap(), p);
        }
    }

    #[test]
    fn byte_fields_round_trip_only_at_their_exact_length() {
        let b = [7u8; 33];
        assert_eq!(decode_len(&encode_bytes(&b), 33), Some(b.to_vec()));
        assert_eq!(decode_len(&encode_bytes(&b), 32), None);
        assert_eq!(decode_len("not base64!!", 33), None);
        assert_eq!(decode_len("", 0), Some(vec![]));
    }

    #[test]
    fn payloads_round_trip_through_encode_and_decode() {
        let k = keys_for(1);
        let m = member_from(&k, "laptop");
        let p = Payload::Join(JoinRequest {
            member: m,
            sig: "sig".into(),
            commit: "c".into(),
        });
        let encoded = encode(&p);
        assert_eq!(decode(&encoded).unwrap(), p);
    }

    #[test]
    fn thirty_two_byte_values_round_trip_and_anything_else_is_refused() {
        let b = [7u8; 32];
        assert_eq!(decode32(&encode32(&b)), Some(b));
        assert_eq!(decode32(&STANDARD.encode([7u8; 31])), None);
        assert_eq!(decode32(&STANDARD.encode([7u8; 33])), None);
        assert_eq!(decode32("not base64!!"), None);
    }

    #[test]
    fn decode_rejects_garbage_without_panicking() {
        assert!(decode(b"not json").is_err());
        assert!(decode(b"{\"t\":\"nonsense\"}").is_err());
    }

    #[test]
    fn a_join_request_signature_covers_every_member_field() {
        let k = keys_for(1);
        let other = keys_for(2);
        let m = member_from(&k, "laptop");
        let sig = sign_member(&m, &k);
        assert!(verify_member(&m, &sig));

        let mut tampered = m.clone();
        tampered.name = "desktop".into();
        assert!(!verify_member(&tampered, &sig));

        let mut tampered = m.clone();
        tampered.added_at = 43;
        assert!(!verify_member(&tampered, &sig));

        let mut tampered = m.clone();
        tampered.endpoint = "c-0000000000000000000f".into();
        assert!(!verify_member(&tampered, &sig));

        // 换成另一把真实存在的 kx_pub：形状合法，但跟签名时用的字节不一样。
        let mut tampered = m.clone();
        tampered.kx_pub = STANDARD.encode(other.kx_pub());
        assert!(!verify_member(&tampered, &sig));

        // 换成另一把真实存在的 sign_pub：解码会成功（曲线上的合法点），但
        // 既不是签名用的那把私钥对应的公钥，签的字节也变了。
        let mut tampered = m;
        tampered.sign_pub = STANDARD.encode(other.sign_pub());
        assert!(!verify_member(&tampered, &sig));
    }

    #[test]
    fn verify_member_rejects_a_self_consistent_signature_whose_endpoint_is_someone_elses() {
        // `m` is genuinely, correctly signed by `k` over exactly these bytes
        // -- the signature check alone has nothing to object to. The only
        // thing wrong is that `m.endpoint` doesn't actually belong to `k`.
        let k = keys_for(1);
        let mut m = member_from(&k, "alice");
        m.endpoint = "c-notmykey0000000000".into();
        let sig = sign_member(&m, &k);
        assert!(!verify_member(&m, &sig));
    }

    #[test]
    fn member_bytes_matches_a_known_vector() {
        let m = Member {
            name: "alice".into(),
            endpoint: "c-known0000000000000f".into(),
            sign_pub: "signpubb64".into(),
            kx_pub: "kxpubb64".into(),
            added_at: 1_700_000_000,
        };
        let expected: &[u8] =
            b"11:dct-join-v15:alice21:c-known0000000000000f10:signpubb648:kxpubb6410:1700000000";
        assert_eq!(member_bytes(&m), expected);
    }

    #[test]
    fn a_signature_from_a_different_key_does_not_verify() {
        let k = keys_for(1);
        let other = keys_for(2);
        let m = member_from(&k, "laptop");
        let sig = sign_member(&m, &other);
        assert!(!verify_member(&m, &sig));
    }

    #[test]
    fn garbage_signatures_and_keys_fail_closed_instead_of_panicking() {
        let k = keys_for(1);
        let mut m = member_from(&k, "laptop");
        assert!(!verify_member(&m, "not base64!!"));

        m.sign_pub = "not base64!!".into();
        assert!(!verify_member(&m, &STANDARD.encode([0u8; 64])));
    }
}
