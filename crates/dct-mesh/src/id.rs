//! 一台电脑在中转上的身份，和「机器/会话」这种可读地址的解析。
use sha2::{Digest, Sha256};

/// 每台电脑的中转端点 id：`"c-"` + 这台电脑 P-256 签名公钥 SHA-256 的前 10
/// 字节，转成小写十六进制。稳定（同一把钥匙永远算出同一个 id），碰撞概率
/// 可忽略（80 位），而且落在 `dct-link::EndpointId` 的字符集
/// `[A-Za-z0-9_.:-]`、长度 <= 64 之内——`"c-"` 加 20 个十六进制字符正好 22。
pub fn endpoint_for(sign_pub: &[u8; 65]) -> String {
    let hex = format!("{:x}", Sha256::digest(sign_pub));
    format!("c-{}", &hex[..20])
}

/// 「机器/会话」这种给人看的地址，比如 `公司Windows/dc-terminal`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub machine: String,
    pub session: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrError {
    /// 没有 `/`，或者不止一个。
    WrongPartCount,
    /// 机器名或会话名是空串。
    EmptyPart,
}

impl std::fmt::Display for AddrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AddrError::WrongPartCount => "address needs exactly one '/'",
            AddrError::EmptyPart => "machine and session names cannot be empty",
        })
    }
}

impl std::error::Error for AddrError {}

impl Address {
    pub fn parse(s: &str) -> Result<Address, AddrError> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() != 2 {
            return Err(AddrError::WrongPartCount);
        }
        let (machine, session) = (parts[0], parts[1]);
        if machine.is_empty() || session.is_empty() {
            return Err(AddrError::EmptyPart);
        }
        Ok(Address {
            machine: machine.to_string(),
            session: session.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_is_c_dash_plus_20_hex_and_a_valid_relay_id() {
        let pk = [4u8; 65];
        let id = endpoint_for(&pk);
        assert_eq!(id.len(), 22);
        assert!(id.starts_with("c-"));
        assert!(id[2..]
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // dct-link::EndpointId 的字符集是 [A-Za-z0-9_.:-]，长度上限 64。
        assert!(id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-')));
        assert!(id.chars().count() <= 64);
    }

    #[test]
    fn endpoint_is_stable_for_the_same_key_and_differs_for_different_keys() {
        let a = [4u8; 65];
        let mut b = [4u8; 65];
        b[64] = 5;
        assert_eq!(endpoint_for(&a), endpoint_for(&a));
        assert_ne!(endpoint_for(&a), endpoint_for(&b));
    }

    #[test]
    fn address_parses_machine_slash_session_and_rejects_bad_ones() {
        let ok = Address::parse("公司Windows/dc-terminal").unwrap();
        assert_eq!(ok.machine, "公司Windows");
        assert_eq!(ok.session, "dc-terminal");

        assert_eq!(Address::parse("/x"), Err(AddrError::EmptyPart));
        assert_eq!(Address::parse("x/"), Err(AddrError::EmptyPart));
        assert_eq!(Address::parse("x"), Err(AddrError::WrongPartCount));
        assert_eq!(Address::parse("a/b/c"), Err(AddrError::WrongPartCount));
    }
}
