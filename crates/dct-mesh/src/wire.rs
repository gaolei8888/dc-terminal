//! 中转（relay）上跑的消息线上形状：一台电脑发给中转的每一条 JSON 都是一个
//! `Payload`，`t` 字段说明它是密封留言、加入请求、整份名单，还是「你的加入
//! 申请还在等别人批」。
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
    JoinPending { member: Member, sig: String },
}

/// 新电脑请求加入组：`member` 是它自己的名单条目（还没被任何人签认），
/// `sig` 是它用自己的钥匙对 `member` 的 `dct-join-v1` 规范字节签的名——证明
/// 它真的掌握 `member.sign_pub` 对应的私钥。ECDSA 签名可延展（见
/// `keys::MachineKeys::sign` 的文档），`sig` 不能当 id 或去重键用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinRequest {
    pub member: Member,
    pub sig: String,
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
        };
        assert!(serde_json::to_string(&pending)
            .unwrap()
            .contains("\"t\":\"join_pending\""));
    }

    #[test]
    fn payloads_round_trip_through_encode_and_decode() {
        let k = keys_for(1);
        let m = member_from(&k, "laptop");
        let p = Payload::Join(JoinRequest {
            member: m,
            sig: "sig".into(),
        });
        let encoded = encode(&p);
        assert_eq!(decode(&encoded).unwrap(), p);
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
