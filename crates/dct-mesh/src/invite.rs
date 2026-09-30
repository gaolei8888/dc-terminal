//! 邀请码（dct-invite-v1）的纯计算部分：生成 6 位码、把码和两台电脑的记录
//! 喂给 SPAKE2、从共享密钥派生两把确认钥匙、算和验两边的确认值。
//!
//! **不碰网络、不碰时间、不自己拿随机数**：码和 SPAKE2 用的随机数都由调用方
//! 给（`Code::random` 的 `rand`、`Handshake::*` 的 `seed`）。状态机（码什么时候
//! 作废、谁能发 `InviteFinish`）在 `dct` 的 `mesh::invite` 里。
//!
//! 规范（字节精确，跟 `docs/superpowers/specs/2026-09-30-dct-invite-code-design.md`
//! 第 2 段一致）：
//!
//! - 口令 = `field("dct-invite-v1")` ‖ `field(码的 6 个 ASCII 数字)`；
//! - 身份 `idA = rec(A)`、`idB = rec(B)`，`rec(m)` = `field(name)` ‖
//!   `field(endpoint)` ‖ `field(sign_pub)` ‖ `field(kx_pub)`（`added_at` 不算）；
//! - SPAKE2：`spake2` 0.4.0，`Ed25519Group`，A（邀请方）`start_a`，B（加入方）
//!   `start_b`；线上的消息是库给的 33 字节（1 字节角色 + 32 字节点）；
//! - `kA`/`kB` = HKDF-SHA256(ikm = K, salt = 空, info = "dct-invite-v1 confirm A"/"… B")；
//! - `T` = `field("dct-invite-v1")` ‖ `field(group)` ‖ `field_bytes(rec(A))` ‖
//!   `field_bytes(rec(B))` ‖ `field_bytes(msgA)` ‖ `field_bytes(msgB)`；
//! - `cA = HMAC-SHA256(kA, T)`，`cB = HMAC-SHA256(kB, T)`；验的时候常数时间比较
//!   （`hmac` 的 `verify_slice`）。
//!
//! `spake2` 0.4 用自己的 M/N 常数，**跟 RFC 9382 不互通**；这里的固定向量只防
//! 我们自己的回归。
use std::fmt;

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha256, Sha512};
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::canon::{field, field_bytes};
use crate::roster::Member;

pub const INVITE_VERSION_TAG: &str = "dct-invite-v1";
const CONFIRM_INFO_A: &[u8] = b"dct-invite-v1 confirm A";
const CONFIRM_INFO_B: &[u8] = b"dct-invite-v1 confirm B";
const RNG_TAG: &str = "dct-invite-v1 rng";

/// 码有几位。
pub const CODE_DIGITS: usize = 6;
/// SPAKE2 一条消息的字节数（角色 1 + Ed25519 点 32）。
pub const MSG_LEN: usize = 33;
/// 确认值的字节数（HMAC-SHA256）。
pub const TAG_LEN: usize = 32;

/// 一百万的最大整数倍、且不超过 2^32：落在它下面的 u32 取模才均匀。
const SAMPLE_LIMIT: u32 = 4_294_000_000;

/// 6 位十进制数字，前导 0 照留（`012345` 就是 `012345`）。
///
/// `Debug` 不打出数字：码是一次性的口令，不该进任何日志。
#[derive(Clone, PartialEq, Eq)]
pub struct Code(String);

impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Code(******)")
    }
}

impl Code {
    /// 人敲进来的码。空格、`-`（含全角的）、制表符不算；全角数字当半角。
    /// 去掉这些之后必须正好是 6 位数字，否则 `None`。
    pub fn parse(input: &str) -> Option<Code> {
        let mut digits = String::with_capacity(CODE_DIGITS);
        for c in input.chars() {
            match c {
                ' ' | '-' | '\t' | '\u{3000}' | '\u{FF0D}' => {}
                '0'..='9' => digits.push(c),
                '\u{FF10}'..='\u{FF19}' => {
                    digits.push(char::from(b'0' + (c as u32 - 0xFF10) as u8))
                }
                _ => return None,
            }
            if digits.len() > CODE_DIGITS {
                return None;
            }
        }
        (digits.len() == CODE_DIGITS).then_some(Code(digits))
    }

    /// 从随机字节里拒绝采样出一个均匀的 000000–999999。`rand` 每次给 32
    /// 字节，按 8 个大端 u32 依次试；不小于 `SAMPLE_LIMIT` 的扔掉。8 个全扔
    /// 掉（概率约 10⁻²⁴）就再要一次。
    pub fn random(rand: &mut dyn FnMut() -> [u8; 32]) -> Code {
        loop {
            let b = rand();
            for chunk in b.chunks_exact(4) {
                let x = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if x < SAMPLE_LIMIT {
                    return Code(format!("{:06}", x % 1_000_000));
                }
            }
        }
    }

    /// 6 个数字，没有空格。`dct join` 后面敲的就是这个。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 给人看的：`482 913`。
    pub fn spaced(&self) -> String {
        format!("{} {}", &self.0[..3], &self.0[3..])
    }
}

/// 一台电脑在邀请里的身份：名字、端点、两把公钥，逐项 `field`。
pub fn rec(m: &Member) -> Vec<u8> {
    let mut out = Vec::new();
    field(&mut out, &m.name);
    field(&mut out, &m.endpoint);
    field(&mut out, &m.sign_pub);
    field(&mut out, &m.kx_pub);
    out
}

/// SPAKE2 的口令字节。
pub fn password(code: &Code) -> Vec<u8> {
    let mut out = Vec::new();
    field(&mut out, INVITE_VERSION_TAG);
    field(&mut out, code.as_str());
    out
}

/// 两边确认值共同覆盖的那段字节 `T`。
pub fn transcript(group: &str, a: &Member, b: &Member, msg_a: &[u8], msg_b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    field(&mut out, INVITE_VERSION_TAG);
    field(&mut out, group);
    field_bytes(&mut out, &rec(a));
    field_bytes(&mut out, &rec(b));
    field_bytes(&mut out, msg_a);
    field_bytes(&mut out, msg_b);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteError {
    /// 对方的 SPAKE2 消息长度不对、角色不对，或者不是曲线上的点。
    BadMessage,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Inviter,
    Joiner,
}

/// 一边的 SPAKE2 进行到一半：自己的消息已经出了，等对方的。
pub struct Handshake {
    role: Role,
    spake: Spake2<Ed25519Group>,
    mine: Vec<u8>,
    a: Member,
    b: Member,
}

impl Handshake {
    /// 邀请方（A）。`a` 是自己，`b` 是来加入的那台自报的记录。
    pub fn inviter(code: &Code, a: &Member, b: &Member, seed: [u8; 32]) -> Handshake {
        let (spake, mine) = Spake2::<Ed25519Group>::start_a_with_rng(
            &Password::new(password(code)),
            &Identity::new(&rec(a)),
            &Identity::new(&rec(b)),
            SeedRng::new(seed),
        );
        Handshake {
            role: Role::Inviter,
            spake,
            mine,
            a: a.clone(),
            b: b.clone(),
        }
    }

    /// 加入方（B）。`a` 是 `InviteOpen` 里那台邀请方的记录，`b` 是自己。
    pub fn joiner(code: &Code, a: &Member, b: &Member, seed: [u8; 32]) -> Handshake {
        let (spake, mine) = Spake2::<Ed25519Group>::start_b_with_rng(
            &Password::new(password(code)),
            &Identity::new(&rec(a)),
            &Identity::new(&rec(b)),
            SeedRng::new(seed),
        );
        Handshake {
            role: Role::Joiner,
            spake,
            mine,
            a: a.clone(),
            b: b.clone(),
        }
    }

    /// 发给对方的那 33 字节。
    pub fn message(&self) -> &[u8] {
        &self.mine
    }

    /// 收到对方的消息：算出共享密钥、两把确认钥匙和 `T`。**这一步不说明码
    /// 对不对**——码不对照样算得出一个 K，只是两边的不一样；对不对要看确认值。
    pub fn finish(self, group: &str, theirs: &[u8]) -> Result<Confirmed, InviteError> {
        let k = self
            .spake
            .finish(theirs)
            .map_err(|_| InviteError::BadMessage)?;
        let (msg_a, msg_b) = match self.role {
            Role::Inviter => (self.mine.as_slice(), theirs),
            Role::Joiner => (theirs, self.mine.as_slice()),
        };
        let hk = Hkdf::<Sha256>::new(None, &k);
        let mut ka = [0u8; 32];
        let mut kb = [0u8; 32];
        hk.expand(CONFIRM_INFO_A, &mut ka)
            .expect("32 bytes is a valid HKDF-SHA256 length");
        hk.expand(CONFIRM_INFO_B, &mut kb)
            .expect("32 bytes is a valid HKDF-SHA256 length");
        Ok(Confirmed {
            ka,
            kb,
            transcript: transcript(group, &self.a, &self.b, msg_a, msg_b),
        })
    }
}

/// SPAKE2 做完之后两边手里的东西。不实现 `Debug`：里面是钥匙。
pub struct Confirmed {
    ka: [u8; 32],
    kb: [u8; 32],
    transcript: Vec<u8>,
}

impl Confirmed {
    fn mac(key: &[u8; 32]) -> Hmac<Sha256> {
        <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes any key length")
    }

    /// `cA`：邀请方在 `InviteKey` 里给的。
    pub fn inviter_tag(&self) -> [u8; TAG_LEN] {
        let mut m = Self::mac(&self.ka);
        m.update(&self.transcript);
        m.finalize().into_bytes().into()
    }

    /// `cB`：加入方在 `InviteFinish` 里给的。
    pub fn joiner_tag(&self) -> [u8; TAG_LEN] {
        let mut m = Self::mac(&self.kb);
        m.update(&self.transcript);
        m.finalize().into_bytes().into()
    }

    /// 加入方验 `cA`。常数时间。
    pub fn inviter_tag_ok(&self, tag: &[u8]) -> bool {
        let mut m = Self::mac(&self.ka);
        m.update(&self.transcript);
        m.verify_slice(tag).is_ok()
    }

    /// 邀请方验 `cB`。常数时间。
    pub fn joiner_tag_ok(&self, tag: &[u8]) -> bool {
        let mut m = Self::mac(&self.kb);
        m.update(&self.transcript);
        m.verify_slice(tag).is_ok()
    }
}

/// 把调用方给的 32 字节种子伸长成 SPAKE2 要的随机数流：
/// SHA-512(`field("dct-invite-v1 rng")` ‖ seed ‖ 计数器大端 8 字节)，一块 64 字节。
/// 种子来自系统随机数（`dct` 的 `os_rand`），所以这条流也是。
struct SeedRng {
    seed: [u8; 32],
    counter: u64,
    buf: [u8; 64],
    used: usize,
}

impl SeedRng {
    fn new(seed: [u8; 32]) -> SeedRng {
        SeedRng {
            seed,
            counter: 0,
            buf: [0; 64],
            used: 64,
        }
    }

    fn refill(&mut self) {
        let mut pre = Vec::new();
        field(&mut pre, RNG_TAG);
        let mut h = Sha512::new();
        h.update(&pre);
        h.update(self.seed);
        h.update(self.counter.to_be_bytes());
        self.buf.copy_from_slice(&h.finalize());
        self.counter += 1;
        self.used = 0;
    }
}

impl RngCore for SeedRng {
    fn next_u32(&mut self) -> u32 {
        rand_core::impls::next_u32_via_fill(self)
    }

    fn next_u64(&mut self) -> u64 {
        rand_core::impls::next_u64_via_fill(self)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for d in dest.iter_mut() {
            if self.used == self.buf.len() {
                self.refill();
            }
            *d = self.buf[self.used];
            self.used += 1;
        }
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for SeedRng {}
#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine as _};

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

    fn code(s: &str) -> Code {
        Code::parse(s).unwrap()
    }

    const SA: [u8; 32] = [0x11; 32];
    const SB: [u8; 32] = [0x22; 32];

    /// 两边各走一遍，返回（msgA, msgB, A 那边的结果, B 那边的结果）。
    fn run(
        code_a: &str,
        code_b: &str,
        a_sees_b: &Member,
        b_sees_a: &Member,
        group_a: &str,
        group_b: &str,
    ) -> (Vec<u8>, Vec<u8>, Confirmed, Confirmed) {
        let (a, b) = (member_a(), member_b());
        let ha = Handshake::inviter(&code(code_a), &a, a_sees_b, SA);
        let hb = Handshake::joiner(&code(code_b), b_sees_a, &b, SB);
        let (ma, mb) = (ha.message().to_vec(), hb.message().to_vec());
        let ca = ha.finish(group_a, &mb).unwrap();
        let cb = hb.finish(group_b, &ma).unwrap();
        (ma, mb, ca, cb)
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// 回归向量：这份实现自己算出来的（`spake2` 0.4 跟 RFC 9382 不互通，没有
    /// 外部向量可对）。规范、依赖版本、种子伸长的方式任何一样变了，这条就红
    /// ——改的人得有意地一起改这四个值。
    #[test]
    fn messages_and_tags_match_the_fixed_vectors() {
        let (ma, mb, ca, cb) = run(
            "482913",
            "482913",
            &member_b(),
            &member_a(),
            "mine-c-aaaa",
            "mine-c-aaaa",
        );
        assert_eq!(
            hex(&ma),
            "41b9ae5bb06aebcb471712289ea0b04434180af656316b01e0d288ee17d06a27e5"
        );
        assert_eq!(
            hex(&mb),
            "4295d789c2cda9b9d0c2375835455fe7b86018bc2425cc116f974293e13196045e"
        );
        assert_eq!(
            hex(&ca.inviter_tag()),
            "550ad8912b2c50f067a69352d17d96f13ae511ea21c6bf67fb82dcb17d9fbb75"
        );
        assert_eq!(
            hex(&cb.joiner_tag()),
            "183cd869eb83b867bff70c3df3fce018c2cd7434f1641b4004eabc98c4b1368f"
        );
        assert_eq!(ma.len(), MSG_LEN);
        assert_eq!(ma[0], b'A');
        assert_eq!(mb[0], b'B');
    }

    #[test]
    fn the_password_and_record_bytes_are_pinned() {
        assert_eq!(password(&code("482913")), b"13:dct-invite-v16:482913");
        let m = Member {
            name: "A".into(),
            endpoint: "c-aaaa".into(),
            sign_pub: "sp".into(),
            kx_pub: "kx".into(),
            added_at: 99,
        };
        assert_eq!(rec(&m), b"1:A6:c-aaaa2:sp2:kx", "added_at 不算");
    }

    #[test]
    fn the_same_code_confirms_both_ways() {
        let (_, _, ca, cb) = run("482913", "482913", &member_b(), &member_a(), "g", "g");
        assert!(cb.inviter_tag_ok(&ca.inviter_tag()), "B 验得过 A 的 cA");
        assert!(ca.joiner_tag_ok(&cb.joiner_tag()), "A 验得过 B 的 cB");
        assert_eq!(ca.inviter_tag(), cb.inviter_tag(), "两边算出同一个 cA");
        assert_ne!(ca.inviter_tag(), ca.joiner_tag(), "cA 和 cB 用的是两把钥匙");
    }

    #[test]
    fn a_code_one_digit_off_fails_both_confirmations() {
        let (_, _, ca, cb) = run("482913", "482914", &member_b(), &member_a(), "g", "g");
        assert!(!cb.inviter_tag_ok(&ca.inviter_tag()));
        assert!(!ca.joiner_tag_ok(&cb.joiner_tag()));
        let (_, _, ca, cb) = run("012345", "112345", &member_b(), &member_a(), "g", "g");
        assert!(!cb.inviter_tag_ok(&ca.inviter_tag()));
        assert!(!ca.joiner_tag_ok(&cb.joiner_tag()));
    }

    /// 中转在中间换掉任何一台记录里的任何一项（名字、端点、任一把公钥），
    /// 两边绑定的身份就不一样，确认值对不上。
    #[test]
    fn swapping_any_field_of_either_record_fails_the_confirmation() {
        type Mutate = fn(&mut Member);
        let mutants: Vec<(&str, Mutate)> = vec![
            ("name", |m| m.name = "公司电脑".into()),
            ("endpoint", |m| m.endpoint = "c-xxxx".into()),
            ("sign_pub", |m| {
                m.sign_pub = STANDARD.encode([&[4u8][..], &[9u8; 64][..]].concat())
            }),
            ("kx_pub", |m| m.kx_pub = STANDARD.encode([9u8; 32])),
        ];
        for (what, f) in &mutants {
            // B 看到的 A 被换了。
            let mut fake_a = member_a();
            f(&mut fake_a);
            let (_, _, ca, cb) = run("482913", "482913", &member_b(), &fake_a, "g", "g");
            assert!(!cb.inviter_tag_ok(&ca.inviter_tag()), "A 的 {what}");
            assert!(!ca.joiner_tag_ok(&cb.joiner_tag()), "A 的 {what}");
            // A 看到的 B 被换了。
            let mut fake_b = member_b();
            f(&mut fake_b);
            let (_, _, ca, cb) = run("482913", "482913", &fake_b, &member_a(), "g", "g");
            assert!(!cb.inviter_tag_ok(&ca.inviter_tag()), "B 的 {what}");
            assert!(!ca.joiner_tag_ok(&cb.joiner_tag()), "B 的 {what}");
        }
    }

    #[test]
    fn a_different_group_fails_the_confirmation() {
        let (_, _, ca, cb) = run(
            "482913",
            "482913",
            &member_b(),
            &member_a(),
            "mine-a",
            "mine-x",
        );
        assert!(!cb.inviter_tag_ok(&ca.inviter_tag()));
        assert!(!ca.joiner_tag_ok(&cb.joiner_tag()));
    }

    /// 反射：把 A 自己的消息原样送回给 A，或者把一个加入方的消息送给另一个
    /// 加入方——角色字节不对，直接拒。长度不对、不是曲线上的点，也拒。
    #[test]
    fn reflected_wrong_length_and_garbage_messages_are_refused() {
        let (a, b) = (member_a(), member_b());
        let ha = Handshake::inviter(&code("482913"), &a, &b, SA);
        let own = ha.message().to_vec();
        assert!(matches!(ha.finish("g", &own), Err(InviteError::BadMessage)));

        let hb = Handshake::joiner(&code("482913"), &a, &b, SB);
        let other_b = Handshake::joiner(&code("482913"), &a, &b, [0x33; 32]);
        assert!(matches!(
            hb.finish("g", other_b.message()),
            Err(InviteError::BadMessage)
        ));

        for bad in [vec![], vec![b'B'; 32], vec![b'B'; 34]] {
            let ha = Handshake::inviter(&code("482913"), &a, &b, SA);
            assert!(matches!(ha.finish("g", &bad), Err(InviteError::BadMessage)));
        }
        // 角色字节对、长度对，但这 32 字节解压不出曲线上的点（y = 0x0202…02
        // 没有对应的 x）。
        let mut not_a_point = vec![b'B'];
        not_a_point.extend_from_slice(&[0x02; 32]);
        let ha = Handshake::inviter(&code("482913"), &a, &b, SA);
        assert!(matches!(
            ha.finish("g", &not_a_point),
            Err(InviteError::BadMessage)
        ));
    }

    /// 验确认值只走 `hmac` 的 `verify_slice`（常数时间），不拿 `==` 比。
    #[test]
    fn tags_are_verified_in_constant_time() {
        let src = include_str!("invite.rs");
        let body = &src[..src.find("#[cfg(test)]").unwrap()];
        assert_eq!(body.matches(".verify_slice(tag)").count(), 2);
        assert!(!body.contains("== tag") && !body.contains("tag =="));
    }

    #[test]
    fn a_truncated_or_empty_tag_is_refused() {
        let (_, _, ca, cb) = run("482913", "482913", &member_b(), &member_a(), "g", "g");
        let tag = ca.inviter_tag();
        assert!(!cb.inviter_tag_ok(&tag[..31]));
        assert!(!cb.inviter_tag_ok(&[]));
        let mut flipped = tag;
        flipped[31] ^= 1;
        assert!(!cb.inviter_tag_ok(&flipped));
    }

    #[test]
    fn different_seeds_give_different_messages() {
        let (a, b) = (member_a(), member_b());
        let x = Handshake::inviter(&code("482913"), &a, &b, SA);
        let y = Handshake::inviter(&code("482913"), &a, &b, [0x12; 32]);
        assert_ne!(x.message(), y.message());
        let again = Handshake::inviter(&code("482913"), &a, &b, SA);
        assert_eq!(
            x.message(),
            again.message(),
            "同一个种子同一条消息（向量靠这个）"
        );
    }

    #[test]
    fn typed_codes_ignore_spaces_dashes_and_full_width_forms() {
        for ok in [
            "482913",
            "482 913",
            " 482913 ",
            "482-913",
            "48 29 13",
            "４８２９１３",
            "４８２　９１３",
            "482－913",
            "\t482913",
        ] {
            assert_eq!(
                Code::parse(ok).map(|c| c.as_str().to_string()),
                Some("482913".into()),
                "{ok:?}"
            );
        }
        assert_eq!(code("012345").as_str(), "012345", "前导 0 照留");
        assert_eq!(code("000000").as_str(), "000000");
        for bad in [
            "",
            "48291",
            "4829131",
            "48291a",
            "482_913",
            "482.913",
            "４８２９１",
            "①②③④⑤⑥",
        ] {
            assert_eq!(Code::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_code_prints_spaced_for_people_and_never_in_debug() {
        assert_eq!(code("482913").spaced(), "482 913");
        assert_eq!(code("012345").spaced(), "012 345");
        let d = format!("{:?}", code("482913"));
        assert!(!d.contains("482") && !d.contains("913"), "{d}");
    }

    /// 拒绝采样：落在 `SAMPLE_LIMIT` 及以上的 u32 必须扔掉（直接取模的话，
    /// 0–967295 会比别的多出现一点点——分布检查查不出这么小的偏差，这条查）。
    #[test]
    fn random_codes_skip_values_at_or_above_the_limit() {
        let mut block = [0u8; 32];
        block[..4].copy_from_slice(&u32::MAX.to_be_bytes());
        block[4..8].copy_from_slice(&SAMPLE_LIMIT.to_be_bytes());
        block[8..12].copy_from_slice(&482_913u32.to_be_bytes());
        assert_eq!(Code::random(&mut || block).as_str(), "482913");

        let mut edge = [0u8; 32];
        edge[..4].copy_from_slice(&(SAMPLE_LIMIT - 1).to_be_bytes());
        assert_eq!(Code::random(&mut || edge).as_str(), "999999");

        // 一整块都不能用：再要一块。
        let mut calls = 0;
        let mut next = || {
            calls += 1;
            if calls == 1 {
                [0xff; 32]
            } else {
                let mut b = [0u8; 32];
                b[..4].copy_from_slice(&7u32.to_be_bytes());
                b
            }
        };
        assert_eq!(Code::random(&mut next).as_str(), "000007");
    }

    /// 10⁶ 个码：只有数字、正好 6 位；首位和末位各自的分布粗略均匀（卡方，
    /// 9 个自由度，阈值 40 ≈ p 1e-5）。随机数用确定的 splitmix64，结果不会飘。
    #[test]
    fn a_million_random_codes_are_six_digits_and_roughly_uniform() {
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            let mut out = [0u8; 32];
            for chunk in out.chunks_exact_mut(8) {
                state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
                let mut z = state;
                z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                chunk.copy_from_slice(&(z ^ (z >> 31)).to_be_bytes());
            }
            out
        };
        const N: usize = 1_000_000;
        let (mut first, mut last) = ([0usize; 10], [0usize; 10]);
        for _ in 0..N {
            let c = Code::random(&mut next);
            let b = c.as_str().as_bytes();
            assert_eq!(b.len(), 6);
            assert!(b.iter().all(u8::is_ascii_digit));
            first[(b[0] - b'0') as usize] += 1;
            last[(b[5] - b'0') as usize] += 1;
        }
        let chi = |h: &[usize; 10]| {
            let e = N as f64 / 10.0;
            h.iter().map(|&o| (o as f64 - e).powi(2) / e).sum::<f64>()
        };
        assert!(chi(&first) < 40.0, "首位 {first:?}");
        assert!(chi(&last) < 40.0, "末位 {last:?}");
    }
}
