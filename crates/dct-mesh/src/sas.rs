//! 短认证字符串（SAS）：新电脑（加入方）和一台已有的电脑（邀请方）各自算出
//! 同一个 6 位数，用户读一眼两块屏幕，对得上才批。
//!
//! **为什么要先承诺、再揭晓。** 6 位数只有一百万种。如果数字只由两台电脑的
//! 公钥算出来，中转看得见所有公钥（`Join`、`JoinPending`、名单都是明文），
//! 它可以一把一把地生成假钥匙，直到某一把跟邀请方算出来的数字恰好等于新电脑
//! 屏幕上的那个——一台笔记本几秒钟的事，还能对着长期不变的钥匙提前算好一张
//! 表。所以数字里还要混进**两边各出的一次性随机数**，而且谁都不能看了对方的
//! 再挑自己的（同 ZRTP、蓝牙数字比较的做法）：
//!
//! 1. 加入方先只交一个承诺 `commit(自己的成员记录, n加入)`（`Join` 里）；
//! 2. 邀请方回自己的成员记录和一个新鲜的 `n邀请`（`JoinPending` 里）；
//! 3. 加入方这时才揭晓 `n加入`（`JoinReveal`），邀请方验它跟承诺对得上。
//!
//! 数字 = `code(邀请方, 加入方, n邀请, n加入)`。加入方在拿到 `n邀请` 之前就
//! 被承诺锁死了，邀请方的 `n邀请` 又是在收到承诺之后才出的——中转每冒充一次
//! 只有一百万分之一的机会，而且每猜一次都要让真的电脑亮一次数字。
//!
//! **电脑名也算在里面**：用户核对的是「叫 公司Windows 的那台，数字是
//! 123456」，名字被换掉，数字也就对不上。
//!
//! 规范（dct-sas-v2，字节精确，跟 `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`
//! 第 1 段「6 位数怎么算」一致）：
//!
//! - 一台电脑的记录 `rec(m)` = `field(name)`、`field(endpoint)`、
//!   `field(sign_pub)`、`field(kx_pub)` 首尾相接（公钥都是标准 base64；
//!   `added_at` 不算）；随机数 32 字节，写成标准 base64 再 `field`。
//! - 承诺 = SHA-256(`field("dct-sas-commit-v2")` ‖ `rec(加入方)` ‖
//!   `field(b64(n加入))`)，32 字节。
//! - 数字 = SHA-256(`field("dct-sas-v2")` ‖ `rec(邀请方)` ‖ `rec(加入方)` ‖
//!   `field(b64(n邀请))` ‖ `field(b64(n加入))`) 的前 4 字节大端 u32
//!   `% 1_000_000`，补零到 6 位。
use crate::canon::field;
use crate::roster::Member;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};

pub const SAS_VERSION_TAG: &str = "dct-sas-v2";
pub const COMMIT_VERSION_TAG: &str = "dct-sas-commit-v2";

/// 一次性随机数的字节数。
pub const NONCE_LEN: usize = 32;

pub type Nonce = [u8; NONCE_LEN];

fn record(out: &mut Vec<u8>, m: &Member) {
    field(out, &m.name);
    field(out, &m.endpoint);
    field(out, &m.sign_pub);
    field(out, &m.kx_pub);
}

fn nonce_field(out: &mut Vec<u8>, n: &Nonce) {
    field(out, &STANDARD.encode(n));
}

/// 加入方对「我是谁 + 我的随机数」的承诺。放进 `Join`，随机数先不给。
pub fn commit(joiner: &Member, joiner_nonce: &Nonce) -> [u8; 32] {
    let mut out = Vec::new();
    field(&mut out, COMMIT_VERSION_TAG);
    record(&mut out, joiner);
    nonce_field(&mut out, joiner_nonce);
    Sha256::digest(&out).into()
}

/// 揭晓的随机数跟先前的承诺对不对得上（成员记录也得是承诺时的那一份）。
pub fn opens(commitment: &[u8; 32], joiner: &Member, joiner_nonce: &Nonce) -> bool {
    commit(joiner, joiner_nonce) == *commitment
}

/// 两块屏幕上的 6 位数。参数有方向：先邀请方、后加入方，随机数同序。
pub fn code(
    inviter: &Member,
    joiner: &Member,
    inviter_nonce: &Nonce,
    joiner_nonce: &Nonce,
) -> String {
    let mut out = Vec::new();
    field(&mut out, SAS_VERSION_TAG);
    record(&mut out, inviter);
    record(&mut out, joiner);
    nonce_field(&mut out, inviter_nonce);
    nonce_field(&mut out, joiner_nonce);
    let digest = Sha256::digest(&out);
    let n = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    format!("{n:06}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member_a() -> Member {
        Member {
            name: "A".into(),
            endpoint: "c-aaaa".into(),
            sign_pub: STANDARD.encode([&[4u8][..], &[1u8; 64][..]].concat()),
            kx_pub: STANDARD.encode([2u8; 32]),
            added_at: 0,
        }
    }

    fn member_b() -> Member {
        Member {
            name: "B".into(),
            endpoint: "c-bbbb".into(),
            sign_pub: STANDARD.encode([&[4u8][..], &[3u8; 64][..]].concat()),
            kx_pub: STANDARD.encode([5u8; 32]),
            added_at: 0,
        }
    }

    /// 中转冒充 B 用的那台：它自己的钥匙，名字照抄 B 的。
    fn member_x() -> Member {
        Member {
            name: "B".into(),
            endpoint: "c-xxxx".into(),
            sign_pub: STANDARD.encode([&[4u8][..], &[9u8; 64][..]].concat()),
            kx_pub: STANDARD.encode([9u8; 32]),
            added_at: 0,
        }
    }

    const NA: Nonce = [0x11; 32];
    const NB: Nonce = [0x22; 32];

    /// 已知答案向量：控制端用独立的 Python 实现（hashlib + base64，照模块
    /// 头的规范逐字节拼）算出，跟这里的 Rust 实现对拍。规范一改，这条就红
    /// ——改规范的人得有意地一起改这两个值和设计文档里那一段。
    #[test]
    fn commit_and_code_match_the_fixed_vectors() {
        assert_eq!(
            commit(&member_b(), &NB),
            [
                0x1b, 0x37, 0x73, 0x0e, 0x64, 0x0d, 0x4c, 0x4e, 0xe0, 0xd2, 0xb1, 0x91, 0x54, 0x92,
                0xa5, 0xc8, 0x6a, 0xa2, 0xc6, 0x7a, 0x66, 0x52, 0x5e, 0xb3, 0xbc, 0x50, 0xe3, 0x69,
                0x83, 0x43, 0x4c, 0x50
            ]
        );
        assert_eq!(code(&member_a(), &member_b(), &NA, &NB), "204106");
    }

    #[test]
    fn the_roles_are_not_interchangeable() {
        let (a, b) = (member_a(), member_b());
        assert_ne!(code(&b, &a, &NA, &NB), code(&a, &b, &NA, &NB));
        assert_ne!(code(&a, &b, &NB, &NA), code(&a, &b, &NA, &NB));
    }

    #[test]
    fn every_input_changes_the_code() {
        let (a, b) = (member_a(), member_b());
        let base = code(&a, &b, &NA, &NB);
        let mut m = b.clone();
        m.kx_pub = STANDARD.encode([9u8; 32]);
        assert_ne!(code(&a, &m, &NA, &NB), base, "换加密公钥");
        let mut m = b.clone();
        m.sign_pub = STANDARD.encode([&[4u8][..], &[7u8; 64][..]].concat());
        assert_ne!(code(&a, &m, &NA, &NB), base, "换签名公钥");
        let mut m = b.clone();
        m.name = "公司Windows".into();
        assert_ne!(code(&a, &m, &NA, &NB), base, "换名字");
        let mut m = a.clone();
        m.name = "家里Mac".into();
        assert_ne!(code(&m, &b, &NA, &NB), base, "换邀请方的名字");
        assert_ne!(code(&a, &b, &[0x12; 32], &NB), base, "换邀请方的随机数");
        assert_ne!(code(&a, &b, &NA, &[0x23; 32]), base, "换加入方的随机数");
    }

    #[test]
    fn a_reveal_opens_only_its_own_commitment() {
        let b = member_b();
        let c = commit(&b, &NB);
        assert!(opens(&c, &b, &NB));
        assert!(!opens(&c, &b, &[0x23; 32]), "换随机数");
        let mut renamed = b.clone();
        renamed.name = "别的名字".into();
        assert!(!opens(&c, &renamed, &NB), "换成员记录");
        assert!(!opens(&c, &member_x(), &NB), "换成别的电脑");
    }

    /// 审查的 C1 攻击，换到现在的规则下：
    ///
    /// 中转把 A 以前的 `JoinPending`（带着旧的 `n邀请`）回放给 B，B 揭晓
    /// 自己的随机数、屏幕上亮出 `target`。中转再以假电脑 X（名字也叫 B）
    /// 向 A 请求加入。它要让 A 亮出同一个数字——
    ///
    /// - 只看数字的话，拿到 A 新出的 `n邀请` 之后去磨随机数是磨得出来的：
    ///   下面第 `GROUND` 个随机数就正好对上（控制端用 Python 离线磨出来的，
    ///   这里只验结果，不在测试里磨）。
    /// - 但 X 的随机数在它拿到 `n邀请` **之前**就已经承诺了。磨出来的那个
    ///   打不开承诺，A 不认；承诺里那个随机数算出来的数字对不上。
    #[test]
    fn a_relay_that_grinds_after_seeing_the_inviter_nonce_cannot_open_its_commitment() {
        let (a, b, x) = (member_a(), member_b(), member_x());
        let old_na = NA;
        let target = code(&a, &b, &old_na, &NB);

        // 中转先交承诺（这时它还不知道 A 会出什么随机数）。
        let committed: Nonce = [0x44; 32];
        let cx = commit(&x, &committed);
        // A 收到承诺之后才出新的随机数。
        let fresh_na: Nonce = [0x33; 32];

        const GROUND: u64 = 1_181_180;
        let mut ground = [0u8; 32];
        ground[24..].copy_from_slice(&GROUND.to_be_bytes());
        assert_eq!(
            code(&a, &x, &fresh_na, &ground),
            target,
            "只看数字，事后磨是磨得出来的"
        );
        assert!(!opens(&cx, &x, &ground), "磨出来的随机数打不开先前的承诺");
        assert_ne!(
            code(&a, &x, &fresh_na, &committed),
            target,
            "承诺过的那个随机数对不上"
        );
    }

    /// 先承诺、后拿到邀请方的随机数：中转挑什么都只是一次瞎猜。一千次都
    /// 用同一个承诺去碰一千个新鲜的邀请方随机数，对上的次数应该是零（期望
    /// 0.001 次）。对照：旧规则（数字只看公钥）下，同样一千把假钥匙里只要
    /// 有一把对上就永远对上，而且可以离线磨。
    #[test]
    fn with_the_commitment_fixed_each_attempt_is_a_one_in_a_million_guess() {
        let (a, b, x) = (member_a(), member_b(), member_x());
        let target = code(&a, &b, &NA, &NB);
        let committed: Nonce = [0x44; 32];
        let hits = (0u32..1000)
            .filter(|i| {
                let mut na = [0x55u8; 32];
                na[..4].copy_from_slice(&i.to_be_bytes());
                code(&a, &x, &na, &committed) == target
            })
            .count();
        assert_eq!(hits, 0);
    }

    /// 回放一份旧的 `JoinPending`：B 这一次出的是新随机数，数字跟上一次
    /// A 屏幕上亮过的那个不一样——旧的核对结果挪不到新的一次上。
    #[test]
    fn a_replayed_inviter_answer_gives_a_different_code_next_time() {
        let (a, b) = (member_a(), member_b());
        let first = code(&a, &b, &NA, &NB);
        let second = code(&a, &b, &NA, &[0x66; 32]);
        assert_ne!(first, second);
    }

    #[test]
    fn the_code_is_always_six_digits_even_when_the_number_is_small() {
        let (a, b) = (member_a(), member_b());
        let mut found = false;
        for i in 0u8..=255 {
            let c = code(&a, &b, &[i; 32], &NB);
            assert_eq!(c.len(), 6, "code must always be exactly 6 chars: {c}");
            assert!(c.chars().all(|ch| ch.is_ascii_digit()));
            if c.starts_with('0') {
                found = true;
            }
        }
        assert!(found, "expected at least one nonce to need zero-padding");
    }
}
