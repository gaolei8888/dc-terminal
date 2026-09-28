use serde::{Deserialize, Serialize};

/// 一个动作能影响多大范围。**顺序就是「谁更严」**：取 `max` 就是往高了调，
/// 所有兜底都只能这么调，永远不往低了调。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tier {
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "self")]
    SelfOnly,
    #[serde(rename = "physical")]
    Physical,
    #[serde(rename = "content")]
    Content,
    #[serde(rename = "money")]
    Money,
}

/// 这一档的票该由谁来签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignerRole {
    /// 自动钥匙：不打扰用户。
    #[serde(rename = "auto")]
    Auto,
    /// 用户本人：Touch ID 或手机 passkey。
    #[serde(rename = "user")]
    User,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Read => "read",
            Tier::SelfOnly => "self",
            Tier::Physical => "physical",
            Tier::Content => "content",
            Tier::Money => "money",
        }
    }

    /// `None` 表示不要票（只读）。物理动作全自动是用户定的（设计第 4 节）。
    pub fn required_signer(self) -> Option<SignerRole> {
        match self {
            Tier::Read => None,
            Tier::SelfOnly | Tier::Physical => Some(SignerRole::Auto),
            Tier::Content | Tier::Money => Some(SignerRole::User),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stricter_tiers_sort_higher() {
        assert!(Tier::Read < Tier::SelfOnly);
        assert!(Tier::SelfOnly < Tier::Physical);
        assert!(Tier::Physical < Tier::Content);
        assert!(Tier::Content < Tier::Money);
    }

    #[test]
    fn names_match_dcv_and_dco() {
        for (t, n) in [
            (Tier::Read, "read"),
            (Tier::SelfOnly, "self"),
            (Tier::Physical, "physical"),
            (Tier::Content, "content"),
            (Tier::Money, "money"),
        ] {
            assert_eq!(t.name(), n);
            assert_eq!(serde_json::to_string(&t).unwrap(), format!("\"{n}\""));
            assert_eq!(serde_json::from_str::<Tier>(&format!("\"{n}\"")).unwrap(), t);
        }
    }

    #[test]
    fn who_signs_each_tier() {
        assert_eq!(Tier::Read.required_signer(), None);
        assert_eq!(Tier::SelfOnly.required_signer(), Some(SignerRole::Auto));
        // 物理动作全自动：用户定的「便利性第一位」。
        assert_eq!(Tier::Physical.required_signer(), Some(SignerRole::Auto));
        assert_eq!(Tier::Content.required_signer(), Some(SignerRole::User));
        assert_eq!(Tier::Money.required_signer(), Some(SignerRole::User));
    }
}
