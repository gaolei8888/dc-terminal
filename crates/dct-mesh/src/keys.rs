//! 一台电脑的两把钥匙：P-256 用来签名单（跟 dct-brain 的票据签名同一条曲线，
//! 纯 Rust，没有 C），X25519 用来给 `seal` 模块的留言做端到端加密。种子由
//! 调用方给——这个 crate 不碰随机数生成器，见 lib.rs。
use p256::ecdsa::signature::{Signer as _, Verifier as _};
use p256::ecdsa::{Signature as EcSig, SigningKey, VerifyingKey};
use x25519_dalek::{PublicKey as KxPublicKey, StaticSecret};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// 签名种子不是一个合法的 P-256 标量（全零、或者不小于曲线阶）。
    BadSeed,
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            KeyError::BadSeed => "signing seed is not a valid key",
        })
    }
}

impl std::error::Error for KeyError {}

pub struct MachineKeys {
    sign: SigningKey,
    kx: StaticSecret,
}

impl MachineKeys {
    /// `sign_seed` 直接喂给 P-256（不合法就是 `KeyError::BadSeed`）；`kx_seed`
    /// 喂给 X25519——那条曲线在生成时就把种子夹紧（clamp）成合法标量，不会失败。
    pub fn from_seeds(sign_seed: [u8; 32], kx_seed: [u8; 32]) -> Result<MachineKeys, KeyError> {
        let sign = SigningKey::from_slice(&sign_seed).map_err(|_| KeyError::BadSeed)?;
        let kx = StaticSecret::from(kx_seed);
        Ok(MachineKeys { sign, kx })
    }

    /// SEC1 未压缩公钥，65 字节，`0x04` 开头——跟 dct-brain 的 `Signer::public_key` 同一种形状。
    pub fn sign_pub(&self) -> [u8; 65] {
        let point = self.sign.verifying_key().to_encoded_point(false);
        let mut out = [0u8; 65];
        out.copy_from_slice(point.as_bytes());
        out
    }

    pub fn kx_pub(&self) -> [u8; 32] {
        KxPublicKey::from(&self.kx).to_bytes()
    }

    /// 跟另一台电脑的 X25519 公钥做 Diffie-Hellman，给 `seal` 模块派生对称
    /// 密钥用。两边各自用「自己的私钥 + 对方的公钥」算，算出同一个共享点。
    pub fn diffie_hellman(&self, their_kx_pub: &[u8; 32]) -> [u8; 32] {
        let their = KxPublicKey::from(*their_kx_pub);
        self.kx.diffie_hellman(&their).to_bytes()
    }

    /// 对 `msg` 签名（内部先做 SHA-256），返回 64 字节 `r||s`。
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        let sig: EcSig = self.sign.sign(msg);
        let mut out = [0u8; 64];
        out.copy_from_slice(&sig.to_bytes());
        out
    }
}

/// 任何解不出来的公钥或验不过的签名都返回 `false`，不 panic、不区分原因——
/// 调用方只关心「这张名单/这条消息是不是真的」。
pub fn verify(sign_pub: &[u8; 65], msg: &[u8], sig: &[u8; 64]) -> bool {
    let Ok(vk) = VerifyingKey::from_sec1_bytes(sign_pub) else {
        return false;
    };
    let Ok(es) = EcSig::from_slice(sig) else {
        return false;
    };
    vk.verify(msg, &es).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(byte: u8) -> MachineKeys {
        MachineKeys::from_seeds([byte.max(1); 32], [byte.max(1); 32]).unwrap()
    }

    #[test]
    fn a_signature_verifies_under_its_own_public_key() {
        let k = keys(1);
        let sig = k.sign(b"hello");
        assert!(verify(&k.sign_pub(), b"hello", &sig));
    }

    #[test]
    fn a_signature_does_not_verify_under_a_different_key() {
        let a = keys(1);
        let b = keys(2);
        let sig = a.sign(b"hello");
        assert!(!verify(&b.sign_pub(), b"hello", &sig));
    }

    #[test]
    fn tampering_the_message_breaks_verification() {
        let k = keys(1);
        let sig = k.sign(b"hello");
        assert!(!verify(&k.sign_pub(), b"goodbye", &sig));
    }

    #[test]
    fn sign_pub_is_65_bytes_starting_with_0x04() {
        let k = keys(3);
        let pk = k.sign_pub();
        assert_eq!(pk.len(), 65);
        assert_eq!(pk[0], 0x04);
    }

    #[test]
    fn different_kx_seeds_give_different_kx_public_keys() {
        let a = keys(1);
        let b = keys(2);
        assert_ne!(a.kx_pub(), b.kx_pub());
    }

    #[test]
    fn an_all_zero_signing_seed_is_a_bad_seed() {
        assert!(matches!(
            MachineKeys::from_seeds([0u8; 32], [1u8; 32]),
            Err(KeyError::BadSeed)
        ));
    }

    #[test]
    fn garbage_public_keys_and_signatures_fail_closed_instead_of_panicking() {
        assert!(!verify(&[0u8; 65], b"hello", &[0u8; 64]));
    }

    #[test]
    fn diffie_hellman_is_symmetric_between_two_machines() {
        let a = keys(1);
        let b = keys(2);
        assert_eq!(a.diffie_hellman(&b.kx_pub()), b.diffie_hellman(&a.kx_pub()));
    }

    #[test]
    fn diffie_hellman_differs_for_a_different_peer() {
        let a = keys(1);
        let b = keys(2);
        let c = keys(3);
        assert_ne!(a.diffie_hellman(&b.kx_pub()), a.diffie_hellman(&c.kx_pub()));
    }
}
