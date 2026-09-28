//! 流程批准记录：用户确认一条流程时，同时确认每一步的档位（设计第 4 节）。
//! 流程文件本身不写档位（dcv 定的），档位随这份记录保存；记录带用户签名，
//! 本机文件被人改过就不再认。
use crate::canon::field;
use crate::sign::{make_signature, verify, verify_sig, SignError, SignedTicket, Signature, Signer, TrustedKey, VerifyError};
use crate::tier::{SignerRole, Tier};
use crate::ticket::Subject;
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

/// dco 拿到一张流程票要跑之前，把票和批准记录一起验：票本身要过 `sign::verify`
/// （签名、设备、时间窗口、签名角色够不够这一档）；批准记录要是给票里写的同一条
/// 流程、同一份步骤签的；而且**票的档位不能比批准记录算出来的档位低**——否则
/// 拿一张自动钥匙签的自用档票，去跑一条批准记录写明要用户签的对外流程。
///
/// 不改任何 canonical form 或 golden 值：这只是把已有的 `verify` 和 `verify_approval`
/// 接起来，多加一条档位比较。
pub fn verify_procedure_ticket(
    st: &SignedTicket,
    sa: &SignedApproval,
    trusted: &[TrustedKey],
    device: &str,
    now: u64,
) -> Result<(), VerifyError> {
    verify(st, trusted, device, now)?;
    let (name, steps_sha256) = match &st.ticket.subject {
        Subject::Procedure { name, steps_sha256, .. } => (name.as_str(), steps_sha256.as_str()),
        // 批准记录只管流程；一张单动作票没有对应的批准记录可比。
        Subject::Action { .. } => return Err(VerifyError::WrongProcedure),
    };
    verify_approval(sa, trusted, name, steps_sha256)?;
    if st.ticket.tier < sa.approval.run_tier() {
        return Err(VerifyError::TierBelowApproval);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::soft::SoftSigner;
    use crate::sign::sign_one;
    use crate::ticket::{params_sha256, Ticket, TICKET_VERSION};
    use std::collections::BTreeMap;

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

    fn ticket_for(tier: Tier) -> Ticket {
        Ticket {
            v: TICKET_VERSION,
            nonce: "0123456789abcdef0123456789abcdef".into(),
            device: "mac-lei".into(),
            tier,
            subject: Subject::Procedure {
                name: "social:edit-bio".into(),
                steps_sha256: STEPS.into(),
                body_sha256: format!("sha256:{}", "ab".repeat(32)),
            },
            params_sha256: params_sha256(&BTreeMap::new()),
            earliest: 100,
            expires: 200,
        }
    }

    #[test]
    fn a_valid_ticket_and_approval_pair_verifies() {
        let user = SoftSigner::from_seed(SignerRole::User, 2);
        let sa = sign_approval(approval(), &user, "批准").unwrap(); // run_tier() == Content
        let st = sign_one(ticket_for(Tier::Content), &user, "发到 TikTok").unwrap();
        assert_eq!(verify_procedure_ticket(&st, &sa, &[user.trusted()], "mac-lei", 150), Ok(()));
    }

    #[test]
    fn a_self_tier_ticket_cannot_ride_a_content_approval() {
        let user = SoftSigner::from_seed(SignerRole::User, 2);
        let auto = SoftSigner::from_seed(SignerRole::Auto, 1);
        let trusted = vec![user.trusted(), auto.trusted()];
        let sa = sign_approval(approval(), &user, "批准").unwrap(); // run_tier() == Content
        let st = sign_one(ticket_for(Tier::SelfOnly), &auto, "").unwrap();
        assert_eq!(
            verify_procedure_ticket(&st, &sa, &trusted, "mac-lei", 150),
            Err(VerifyError::TierBelowApproval)
        );
    }

    #[test]
    fn ticket_for_a_different_procedure_or_steps_is_refused() {
        let user = SoftSigner::from_seed(SignerRole::User, 2);
        let sa = sign_approval(approval(), &user, "批准").unwrap();

        let mut wrong_name = ticket_for(Tier::Content);
        if let Subject::Procedure { name, .. } = &mut wrong_name.subject {
            *name = "social:edit-bio-copy".into();
        }
        let st = sign_one(wrong_name, &user, "").unwrap();
        assert_eq!(
            verify_procedure_ticket(&st, &sa, &[user.trusted()], "mac-lei", 150),
            Err(VerifyError::WrongProcedure)
        );

        let mut wrong_steps = ticket_for(Tier::Content);
        if let Subject::Procedure { steps_sha256, .. } = &mut wrong_steps.subject {
            *steps_sha256 = "sha256:other".into();
        }
        let st2 = sign_one(wrong_steps, &user, "").unwrap();
        assert_eq!(
            verify_procedure_ticket(&st2, &sa, &[user.trusted()], "mac-lei", 150),
            Err(VerifyError::StepsChanged)
        );
    }

    #[test]
    fn a_tampered_approval_fails_ticket_verification() {
        let user = SoftSigner::from_seed(SignerRole::User, 2);
        let mut sa = sign_approval(approval(), &user, "批准").unwrap();
        sa.approval.step_tiers[3] = Tier::SelfOnly;
        let st = sign_one(ticket_for(Tier::Content), &user, "").unwrap();
        assert_eq!(
            verify_procedure_ticket(&st, &sa, &[user.trusted()], "mac-lei", 150),
            Err(VerifyError::BadSignature)
        );
    }
}
