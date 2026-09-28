//! 流程批准记录：用户确认一条流程时，同时确认每一步的档位（设计第 4 节）。
//! 流程文件本身不写档位（dcv 定的），档位随这份记录保存；记录带用户签名，
//! 本机文件被人改过就不再认。
use crate::canon::field;
use crate::sign::{make_signature, verify_sig, SignError, Signature, Signer, TrustedKey, VerifyError};
use crate::tier::{SignerRole, Tier};
use serde::{Deserialize, Serialize};

pub const APPROVAL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub v: u32,
    /// `库:名字`
    pub procedure: String,
    pub steps_sha256: String,
    pub body_sha256: String,
    /// 跟流程步骤一一对应，按 n 的顺序。
    pub step_tiers: Vec<Tier>,
    pub approved_at: u64,
}

impl Approval {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut o = Vec::new();
        field(&mut o, "dct-approval-v1");
        field(&mut o, &self.v.to_string());
        field(&mut o, &self.procedure);
        field(&mut o, &self.steps_sha256);
        field(&mut o, &self.body_sha256);
        field(&mut o, &self.step_tiers.len().to_string());
        for t in &self.step_tiers {
            field(&mut o, t.name());
        }
        field(&mut o, &self.approved_at.to_string());
        o
    }

    /// 整条流程的票按最严的那一步来签。
    pub fn run_tier(&self) -> Tier {
        self.step_tiers.iter().copied().max().unwrap_or(Tier::Read)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedApproval {
    pub approval: Approval,
    pub signature: Signature,
}

pub fn sign_approval(a: Approval, s: &dyn Signer, reason: &str) -> Result<SignedApproval, SignError> {
    if s.role() != SignerRole::User {
        return Err(SignError::Other("批准流程必须用用户本人的钥匙".into()));
    }
    let signature = make_signature(s, &a.canonical_bytes(), reason)?;
    Ok(SignedApproval { approval: a, signature })
}

pub fn verify_approval(
    sa: &SignedApproval,
    trusted: &[TrustedKey],
    procedure: &str,
    steps_sha256: &str,
) -> Result<(), VerifyError> {
    if sa.approval.v != APPROVAL_VERSION {
        return Err(VerifyError::Unsupported);
    }
    if sa.approval.procedure != procedure {
        return Err(VerifyError::WrongProcedure);
    }
    if sa.approval.steps_sha256 != steps_sha256 {
        return Err(VerifyError::StepsChanged);
    }
    let role = verify_sig(&sa.signature, &sa.approval.canonical_bytes(), trusted)?;
    if role != SignerRole::User {
        return Err(VerifyError::WrongRole { need: SignerRole::User });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::soft::SoftSigner;

    const STEPS: &str = "sha256:4d1077af6efac1184e410d23b0b7b120fd42f29d7dade3304ef2041ca10b565c";

    fn approval() -> Approval {
        Approval {
            v: APPROVAL_VERSION,
            procedure: "social:edit-bio".into(),
            steps_sha256: STEPS.into(),
            body_sha256: format!("sha256:{}", "ab".repeat(32)),
            step_tiers: vec![Tier::SelfOnly, Tier::SelfOnly, Tier::SelfOnly, Tier::Content],
            approved_at: 1_790_000_000,
        }
    }

    #[test]
    fn canonical_form_is_stable() {
        assert_eq!(
            crate::canon::sha256_id(&approval().canonical_bytes()),
            "sha256:4ba1f58585f9df3f798cc0178ea13e16f9c351aa0fc6adcc72a89ac17867432a"
        );
    }

    #[test]
    fn the_run_needs_the_strictest_steps_signer() {
        assert_eq!(approval().run_tier(), Tier::Content);
        let mut a = approval();
        a.step_tiers = vec![];
        assert_eq!(a.run_tier(), Tier::Read);
    }

    #[test]
    fn only_the_user_can_approve() {
        let auto = SoftSigner::from_seed(SignerRole::Auto, 1);
        assert!(sign_approval(approval(), &auto, "").is_err());
        let user = SoftSigner::from_seed(SignerRole::User, 2);
        let sa = sign_approval(approval(), &user, "批准").unwrap();
        assert_eq!(verify_approval(&sa, &[user.trusted()], "social:edit-bio", STEPS), Ok(()));
    }

    #[test]
    fn editing_the_record_or_the_steps_voids_it() {
        let user = SoftSigner::from_seed(SignerRole::User, 2);
        let sa = sign_approval(approval(), &user, "").unwrap();
        let mut lowered = sa.clone();
        lowered.approval.step_tiers[3] = Tier::SelfOnly;
        assert_eq!(verify_approval(&lowered, &[user.trusted()], "social:edit-bio", STEPS), Err(VerifyError::BadSignature));
        assert_eq!(verify_approval(&sa, &[user.trusted()], "social:edit-bio", "sha256:other"), Err(VerifyError::StepsChanged));
    }

    #[test]
    fn an_automatic_key_signature_is_not_an_approval() {
        // 就算有人绕过 sign_approval、直接用自动钥匙签了批准记录，验的时候也不认。
        let auto = SoftSigner::from_seed(SignerRole::Auto, 1);
        let sig = crate::sign::make_signature(&auto, &approval().canonical_bytes(), "").unwrap();
        let sa = SignedApproval { approval: approval(), signature: sig };
        assert_eq!(
            verify_approval(&sa, &[auto.trusted()], "social:edit-bio", STEPS),
            Err(VerifyError::WrongRole { need: SignerRole::User })
        );
    }

    #[test]
    fn approval_for_one_procedure_does_not_verify_for_another() {
        let user = SoftSigner::from_seed(SignerRole::User, 2);
        let sa = sign_approval(approval(), &user, "批准").unwrap();
        assert_eq!(verify_approval(&sa, &[user.trusted()], "social:edit-bio", STEPS), Ok(()));
        assert_eq!(
            verify_approval(&sa, &[user.trusted()], "social:edit-bio-copy", STEPS),
            Err(VerifyError::WrongProcedure)
        );
    }
}
