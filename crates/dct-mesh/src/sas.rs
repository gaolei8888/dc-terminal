//! 短认证字符串（SAS）：两台电脑各自算出同一个 6 位数；中转伪造任何一把
//! 公钥，两边就对不上——这就是「配对时读一眼数字对不对」防的是什么。
//!
//! 规范（dct-sas-v1）：按 `endpoint` 字典序排两台；依次写
//! `field("dct-sas-v1")`，再对每台写 `field(endpoint)`、`field(sign_pub)`、
//! `field(kx_pub)`（公钥都是标准 base64）；SHA-256；取前 4 字节大端 u32
//! `% 1_000_000`，补零到 6 位。
use crate::canon::field;
use crate::roster::Member;
use sha2::{Digest, Sha256};

pub fn code(a: &Member, b: &Member) -> String {
    let (x, y) = if a.endpoint <= b.endpoint {
        (a, b)
    } else {
        (b, a)
    };
    let mut out = Vec::new();
    field(&mut out, "dct-sas-v1");
    for m in [x, y] {
        field(&mut out, &m.endpoint);
        field(&mut out, &m.sign_pub);
        field(&mut out, &m.kx_pub);
    }
    let digest = Sha256::digest(&out);
    let n = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    format!("{n:06}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

    fn member_a() -> Member {
        Member {
            name: "A".into(),
            endpoint: "c-aaaa".into(),
            sign_pub: B64.encode([&[4u8][..], &[1u8; 64][..]].concat()),
            kx_pub: B64.encode([2u8; 32]),
            added_at: 0,
        }
    }

    fn member_b() -> Member {
        Member {
            name: "B".into(),
            endpoint: "c-bbbb".into(),
            sign_pub: B64.encode([&[4u8][..], &[3u8; 64][..]].concat()),
            kx_pub: B64.encode([5u8; 32]),
            added_at: 0,
        }
    }

    #[test]
    fn code_matches_the_fixed_vector_and_ignores_argument_order() {
        let a = member_a();
        let b = member_b();
        // 控制端用独立 Python 实现算出，跟这里的 Rust 实现对拍。
        assert_eq!(code(&a, &b), "280288");
        assert_eq!(code(&b, &a), "280288");
    }

    #[test]
    fn a_swapped_key_changes_the_code() {
        // 中转（或攻击者）换了一把加密公钥 → 两边的数字对不上，配对当场露馅。
        let a = member_a();
        let mut b = member_b();
        b.kx_pub = B64.encode([9u8; 32]);
        assert_ne!(code(&a, &b), "280288");
    }

    #[test]
    fn a_swapped_signing_key_also_changes_the_code() {
        let a = member_a();
        let mut b = member_b();
        b.sign_pub = B64.encode([&[4u8][..], &[7u8; 64][..]].concat());
        assert_ne!(code(&a, &b), "280288");
    }

    #[test]
    fn the_code_is_always_six_digits_even_when_the_number_is_small() {
        // 找一把让 SHA-256 前 4 字节 % 1_000_000 小于 100_000 的钥匙,
        // 校验补零真的发生了（而不是被格式化成 5 位数）。
        let a = member_a();
        let mut found = false;
        for i in 0u8..=255 {
            let mut b = member_b();
            b.kx_pub = B64.encode([i; 32]);
            let c = code(&a, &b);
            assert_eq!(c.len(), 6, "code must always be exactly 6 chars: {c}");
            if c.starts_with('0') {
                found = true;
            }
        }
        assert!(
            found,
            "expected at least one candidate key to need zero-padding"
        );
    }
}
