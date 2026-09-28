//! 组名单：谁在「我的电脑」这个组里，各自的公钥是什么。名单是签过名的、有
//! 版本号的对象，不是一条消息——新版本必须由上一版里的成员签，版本号必须
//! 往前走，这样中转（不受信任）转发的名单伪造不出来，也回放不了旧版本。
//!
//! 规范形式（dct-roster-v1）：依次写 `field("dct-roster-v1")`、
//! `field(group)`、`field(version 十进制)`、`field(成员数 十进制)`；成员按
//! `endpoint` 字典序排，每人依次写 `field(name)`、`field(endpoint)`、
//! `field(sign_pub)`、`field(kx_pub)`、`field(added_at 十进制)`。签名是
//! P-256 ECDSA（SHA-256）对这串字节签的，64 字节 `r||s`，标准 base64。
use crate::canon::field;
use crate::keys::{self, MachineKeys};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};

pub const ROSTER_VERSION_TAG: &str = "dct-roster-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub endpoint: String,
    /// SEC1 未压缩 P-256 公钥（65 字节），标准 base64。
    pub sign_pub: String,
    /// X25519 公钥（32 字节），标准 base64。
    pub kx_pub: String,
    pub added_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roster {
    pub group: String,
    pub version: u64,
    pub members: Vec<Member>,
}

impl Roster {
    pub fn member(&self, endpoint: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.endpoint == endpoint)
    }

    pub fn by_name(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRoster {
    pub roster: Roster,
    /// 签名者的 endpoint。
    pub signer: String,
    /// 64 字节 `r||s`，标准 base64。
    pub sig: String,
}

/// 名单的规范字节：members 先按 endpoint 排序，再逐个写定长前缀字段。
pub fn bytes(r: &Roster) -> Vec<u8> {
    let mut members: Vec<&Member> = r.members.iter().collect();
    members.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));

    let mut out = Vec::new();
    field(&mut out, ROSTER_VERSION_TAG);
    field(&mut out, &r.group);
    field(&mut out, &r.version.to_string());
    field(&mut out, &members.len().to_string());
    for m in members {
        field(&mut out, &m.name);
        field(&mut out, &m.endpoint);
        field(&mut out, &m.sign_pub);
        field(&mut out, &m.kx_pub);
        field(&mut out, &m.added_at.to_string());
    }
    out
}

/// 用 `keys` 对 `r` 签名，`signer` 是签名者在（新）名单里的那条成员记录——
/// 只取它的 `endpoint`，`accept` 拿这个 endpoint 去上一版名单里找对应的公钥验签。
pub fn sign(r: Roster, signer: &Member, keys: &MachineKeys) -> SignedRoster {
    let msg = bytes(&r);
    let sig = keys.sign(&msg);
    SignedRoster {
        roster: r,
        signer: signer.endpoint.clone(),
        sig: STANDARD.encode(sig),
    }
}

/// 一个组的第一份名单：只有 `me` 一个成员，version 1，自签。
pub fn genesis(me: Member, group: String, keys: &MachineKeys) -> SignedRoster {
    let roster = Roster {
        group,
        version: 1,
        members: vec![me.clone()],
    };
    sign(roster, &me, keys)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosterError {
    /// `current` 是 `None`，但 incoming 不是一份合法的创世名单
    /// （version != 1，或者不止一个成员，或者签名者不是那唯一成员）。
    NotGenesis,
    /// `group` 跟 `current` 的不一样。
    GroupMismatch,
    /// version 没有严格大于 `current.version`（防回放旧名单）。
    VersionNotNewer,
    /// 签名者不在 `current` 名单里。
    UnknownSigner,
    /// 签名验不过，或者签名 base64 解不出来。
    BadSignature,
    /// 成员里有重复的 name 或 endpoint。
    Duplicate,
    /// 有成员的公钥解不出来（sign_pub 不是 65 字节 0x04 开头，或 kx_pub 不是 32 字节）。
    BadKey,
    /// 签名者在自己签的这一版里把自己从名单上删掉了——这类改动必须由别的在
    /// 任成员签，不能自己签自己退出。
    SignerRemoved,
}

impl std::fmt::Display for RosterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RosterError::NotGenesis => {
                "first roster must be version 1, self-signed, with only that one member"
            }
            RosterError::GroupMismatch => "roster is for a different group",
            RosterError::VersionNotNewer => "roster version is not newer than the current one",
            RosterError::UnknownSigner => "signer is not a member of the current roster",
            RosterError::BadSignature => "roster signature does not verify",
            RosterError::Duplicate => "roster has a duplicate name or endpoint",
            RosterError::BadKey => "a member's public key cannot be decoded",
            RosterError::SignerRemoved => "the signer cannot remove itself in the version it signs",
        })
    }
}

impl std::error::Error for RosterError {}

fn decode_sign_pub(b64: &str) -> Option<[u8; 65]> {
    let raw = STANDARD.decode(b64).ok()?;
    if raw.len() != 65 || raw[0] != 0x04 {
        return None;
    }
    let mut out = [0u8; 65];
    out.copy_from_slice(&raw);
    Some(out)
}

fn decode_kx_pub(b64: &str) -> Option<[u8; 32]> {
    let raw = STANDARD.decode(b64).ok()?;
    if raw.len() != 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&raw);
    Some(out)
}

fn decode_sig(b64: &str) -> Option<[u8; 64]> {
    let raw = STANDARD.decode(b64).ok()?;
    if raw.len() != 64 {
        return None;
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&raw);
    Some(out)
}

fn has_duplicates(members: &[Member]) -> bool {
    for i in 0..members.len() {
        for j in (i + 1)..members.len() {
            if members[i].name == members[j].name || members[i].endpoint == members[j].endpoint {
                return true;
            }
        }
    }
    false
}

fn all_keys_decode(members: &[Member]) -> bool {
    members
        .iter()
        .all(|m| decode_sign_pub(&m.sign_pub).is_some() && decode_kx_pub(&m.kx_pub).is_some())
}

/// 校验一份新收到的（签过名的）名单能不能取代 `current`。规则见模块顶部：
/// 没有 `current` 时只接受创世名单；有 `current` 时要求 group 相同、版本
/// 严格变新、签名者是 `current` 里的成员且签名用它在 `current` 里的公钥验
/// 得过、新名单没有重复项、所有公钥都能解码、签名者没有把自己从这一版里删掉。
pub fn accept(current: Option<&SignedRoster>, incoming: &SignedRoster) -> Result<(), RosterError> {
    let msg = bytes(&incoming.roster);
    let sig = decode_sig(&incoming.sig).ok_or(RosterError::BadSignature)?;

    let signer_sign_pub = match current {
        None => {
            if incoming.roster.version != 1 || incoming.roster.members.len() != 1 {
                return Err(RosterError::NotGenesis);
            }
            let only = &incoming.roster.members[0];
            if only.endpoint != incoming.signer {
                return Err(RosterError::NotGenesis);
            }
            decode_sign_pub(&only.sign_pub).ok_or(RosterError::BadKey)?
        }
        Some(cur) => {
            if incoming.roster.group != cur.roster.group {
                return Err(RosterError::GroupMismatch);
            }
            if incoming.roster.version <= cur.roster.version {
                return Err(RosterError::VersionNotNewer);
            }
            let signer_member = cur
                .roster
                .member(&incoming.signer)
                .ok_or(RosterError::UnknownSigner)?;
            decode_sign_pub(&signer_member.sign_pub).ok_or(RosterError::BadKey)?
        }
    };

    if !keys::verify(&signer_sign_pub, &msg, &sig) {
        return Err(RosterError::BadSignature);
    }

    if has_duplicates(&incoming.roster.members) {
        return Err(RosterError::Duplicate);
    }

    if !all_keys_decode(&incoming.roster.members) {
        return Err(RosterError::BadKey);
    }

    if incoming.roster.member(&incoming.signer).is_none() {
        return Err(RosterError::SignerRemoved);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member_from(keys: &MachineKeys, name: &str) -> Member {
        Member {
            name: name.into(),
            endpoint: crate::id::endpoint_for(&keys.sign_pub()),
            sign_pub: STANDARD.encode(keys.sign_pub()),
            kx_pub: STANDARD.encode(keys.kx_pub()),
            added_at: 0,
        }
    }

    fn keys_for(byte: u8) -> MachineKeys {
        MachineKeys::from_seeds([byte.max(1); 32], [byte.max(1); 32]).unwrap()
    }

    #[test]
    fn genesis_is_accepted_when_there_is_no_current_roster() {
        let ka = keys_for(1);
        let a = member_from(&ka, "A");
        let g = genesis(a, "mine-a".into(), &ka);
        assert_eq!(accept(None, &g), Ok(()));
    }

    #[test]
    fn a_later_version_signed_by_an_existing_member_is_accepted() {
        let ka = keys_for(1);
        let kb = keys_for(2);
        let a = member_from(&ka, "A");
        let b = member_from(&kb, "B");
        let g = genesis(a.clone(), "mine-a".into(), &ka);

        let v2 = Roster {
            group: "mine-a".into(),
            version: 2,
            members: vec![a.clone(), b],
        };
        let signed_v2 = sign(v2, &a, &ka);
        assert_eq!(accept(Some(&g), &signed_v2), Ok(()));
    }

    #[test]
    fn a_roster_signed_by_a_non_member_is_refused() {
        let ka = keys_for(1);
        let kb = keys_for(2);
        let kc = keys_for(3);
        let a = member_from(&ka, "A");
        let b = member_from(&kb, "B");
        let c = member_from(&kc, "C");
        let g = genesis(a.clone(), "mine-a".into(), &ka);

        let v2 = Roster {
            group: "mine-a".into(),
            version: 2,
            members: vec![a, b],
        };
        // 签名者 c 从来没在 current 名单里出现过。
        let signed_v2 = sign(v2, &c, &kc);
        assert_eq!(
            accept(Some(&g), &signed_v2),
            Err(RosterError::UnknownSigner)
        );
    }

    #[test]
    fn same_or_lower_version_is_refused() {
        let ka = keys_for(1);
        let a = member_from(&ka, "A");
        let g = genesis(a.clone(), "mine-a".into(), &ka);

        let same_version = Roster {
            group: "mine-a".into(),
            version: 1,
            members: vec![a.clone()],
        };
        let replay = sign(same_version, &a, &ka);
        assert_eq!(accept(Some(&g), &replay), Err(RosterError::VersionNotNewer));

        let lower_version = Roster {
            group: "mine-a".into(),
            version: 0,
            members: vec![a.clone()],
        };
        let lower = sign(lower_version, &a, &ka);
        assert_eq!(accept(Some(&g), &lower), Err(RosterError::VersionNotNewer));
    }

    #[test]
    fn a_different_group_id_is_refused() {
        let ka = keys_for(1);
        let a = member_from(&ka, "A");
        let g = genesis(a.clone(), "mine-a".into(), &ka);

        let other_group = Roster {
            group: "mine-b".into(),
            version: 2,
            members: vec![a.clone()],
        };
        let signed = sign(other_group, &a, &ka);
        assert_eq!(accept(Some(&g), &signed), Err(RosterError::GroupMismatch));
    }

    #[test]
    fn duplicate_names_or_endpoints_are_refused() {
        let ka = keys_for(1);
        let a = member_from(&ka, "A");
        let g = genesis(a.clone(), "mine-a".into(), &ka);

        let mut dup_endpoint = a.clone();
        dup_endpoint.name = "A-2".into();
        let v2 = Roster {
            group: "mine-a".into(),
            version: 2,
            members: vec![a.clone(), dup_endpoint],
        };
        let signed = sign(v2, &a, &ka);
        assert_eq!(accept(Some(&g), &signed), Err(RosterError::Duplicate));
    }

    #[test]
    fn a_tampered_member_list_fails_the_signature() {
        let ka = keys_for(1);
        let kb = keys_for(2);
        let a = member_from(&ka, "A");
        let b = member_from(&kb, "B");
        let g = genesis(a.clone(), "mine-a".into(), &ka);

        let v2 = Roster {
            group: "mine-a".into(),
            version: 2,
            members: vec![a.clone(), b.clone()],
        };
        let mut signed = sign(v2, &a, &ka);
        // 签完之后偷偷改成员列表：签名不会跟着变。
        signed.roster.members[1].name = "Mallory".into();
        assert_eq!(accept(Some(&g), &signed), Err(RosterError::BadSignature));
    }

    #[test]
    fn the_signer_may_not_be_removed_in_the_same_version_it_signs() {
        let ka = keys_for(1);
        let kb = keys_for(2);
        let a = member_from(&ka, "A");
        let b = member_from(&kb, "B");
        let g0 = genesis(a.clone(), "mine-a".into(), &ka);

        let v2 = Roster {
            group: "mine-a".into(),
            version: 2,
            members: vec![a.clone(), b.clone()],
        };
        let g1 = sign(v2, &a, &ka);
        assert_eq!(accept(Some(&g0), &g1), Ok(()));

        // A 还在 current（v1 之后的 v2）里，用它的钥匙签一份 v3，但那份 v3 把
        // 它自己从成员列表里删了。即便签名对 current 来说完全合法，也要拒绝：
        // 退出必须由别的在任成员签，不能自己签自己退出。
        let v3_without_a = Roster {
            group: "mine-a".into(),
            version: 3,
            members: vec![b],
        };
        let signed_v3 = sign(v3_without_a, &a, &ka);
        assert_eq!(
            accept(Some(&g1), &signed_v3),
            Err(RosterError::SignerRemoved)
        );
    }

    #[test]
    fn bytes_are_stable_regardless_of_member_order_in_the_struct() {
        let ka = keys_for(1);
        let kb = keys_for(2);
        let a = member_from(&ka, "A");
        let b = member_from(&kb, "B");

        let forward = Roster {
            group: "g".into(),
            version: 1,
            members: vec![a.clone(), b.clone()],
        };
        let backward = Roster {
            group: "g".into(),
            version: 1,
            members: vec![b, a],
        };
        assert_eq!(bytes(&forward), bytes(&backward));
    }

    #[test]
    fn member_and_by_name_look_up_the_right_entry() {
        let ka = keys_for(1);
        let kb = keys_for(2);
        let a = member_from(&ka, "A");
        let b = member_from(&kb, "B");
        let r = Roster {
            group: "g".into(),
            version: 1,
            members: vec![a.clone(), b.clone()],
        };
        assert_eq!(r.member(&a.endpoint), Some(&a));
        assert_eq!(r.by_name("B"), Some(&b));
        assert_eq!(r.by_name("nobody"), None);
        assert_eq!(r.member("c-doesnotexist"), None);
    }
}
