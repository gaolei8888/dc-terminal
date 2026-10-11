//! 把摸索得到的草稿交给 dcv。总是 `--source import:agent`；dcv 拒收就把原因交给修草稿的人（最多 3 轮）。
//! 这里只加草稿，绝不批准：agent 写的东西永远不给自己放行。
use crate::procedures::{AddKind, AddReq, Dcv};
use dct_explore::draft::{PopupDraft, ProcedureDraft};

/// 修草稿的人（大模型）。None = 改不了 / 没配。
pub trait Fixer {
    fn fix(&self, body: &str, rejection: &str) -> Option<String>;
}

/// 拒收后最多写几次（第 1 次 + 2 次修改）。
const MAX_ROUNDS: u32 = 3;

#[derive(Debug)]
pub struct Written {
    /// 库:名字
    pub procedure: String,
    pub popups: Vec<String>,
    pub rounds: u32,
    /// 没写进去的弹窗草稿：(名字, dcv 的拒收原因)。不重试，不影响流程草稿。
    pub popup_failures: Vec<(String, String)>,
}

/// Err = (最后一版正文, 最后的拒收原因)。
pub fn write_drafts(
    dcv: &dyn Dcv,
    fixer: &dyn Fixer,
    p: &ProcedureDraft,
    pops: &[PopupDraft],
) -> Result<Written, (String, String)> {
    // 名字已有（不管是不是正式）：写成新草稿，指回原来的。
    let (name, draft_of) = if dcv.show(&format!("{}:{}", p.vault, p.name)).is_ok() {
        let tail: Vec<char> = p.learned_from.chars().rev().take(4).collect();
        let tail: String = tail.into_iter().rev().collect();
        (format!("{}-{}", p.name, tail), Some(p.name.as_str()))
    } else {
        (p.name.clone(), None)
    };
    let mut body = p.body.clone();
    let mut rounds = 0;
    loop {
        rounds += 1;
        let req = AddReq {
            kind: AddKind::Procedure,
            vault: &p.vault,
            name: &name,
            description: &p.description,
            params: &p.params,
            platform: Some(&p.platform),
            learned_from: &p.learned_from,
            draft_of,
            body: &body,
        };
        match dcv.add(&req) {
            Ok(_) => break,
            Err(e) => {
                let reason = e.to_string();
                if rounds >= MAX_ROUNDS {
                    return Err((body, reason));
                }
                match fixer.fix(&body, &reason) {
                    Some(fixed) => body = fixed,
                    None => return Err((body, reason)),
                }
            }
        }
    }
    let mut popups = Vec::new();
    let mut popup_failures = Vec::new();
    for d in pops {
        let req = AddReq {
            kind: AddKind::Popup,
            vault: &d.vault,
            name: &d.name,
            description: &d.description,
            params: &[],
            platform: None,
            learned_from: &d.learned_from,
            draft_of: None,
            body: &d.body,
        };
        match dcv.add(&req) {
            Ok(_) => popups.push(format!("{}:{}", d.vault, d.name)),
            Err(e) => popup_failures.push((d.name.clone(), e.to_string())),
        }
    }
    Ok(Written {
        procedure: format!("{}:{}", p.vault, name),
        popups,
        rounds,
        popup_failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeDcv {
        adds: RefCell<Vec<Vec<String>>>,
        reject: RefCell<Vec<String>>,
        exists: Vec<String>,
    }
    impl crate::procedures::Dcv for FakeDcv {
        fn show(&self, n: &str) -> anyhow::Result<serde_json::Value> {
            if self.exists.iter().any(|e| e == n) {
                Ok(serde_json::json!({"kind": "procedure"}))
            } else {
                anyhow::bail!("没有 {n}")
            }
        }
        fn approve(&self, _: &str, s: &str) -> anyhow::Result<String> {
            Ok(s.into())
        }
        fn add(&self, r: &crate::procedures::AddReq) -> anyhow::Result<serde_json::Value> {
            self.adds.borrow_mut().push(crate::procedures::add_args(r));
            if let Some(e) = self.reject.borrow_mut().pop() {
                anyhow::bail!(e)
            }
            Ok(serde_json::json!({"ok": true}))
        }
        fn vocab(&self) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
        fn popups(&self, _: &str) -> anyhow::Result<Vec<serde_json::Value>> {
            Ok(vec![])
        }
    }
    struct FixOnce;
    impl Fixer for FixOnce {
        fn fix(&self, b: &str, _: &str) -> Option<String> {
            Some(b.replace("bad", "good"))
        }
    }
    struct NoFix;
    impl Fixer for NoFix {
        fn fix(&self, _: &str, _: &str) -> Option<String> {
            None
        }
    }

    fn draft() -> dct_explore::draft::ProcedureDraft {
        dct_explore::draft::ProcedureDraft {
            name: "promote-x-post".into(),
            vault: "social-global".into(),
            description: "在 x 上发一条帖子（dct 摸索学会，run:x-20261011-1a2b）".into(),
            params: vec!["post_text".into()],
            platform: "x".into(),
            learned_from: "run:x-20261011-1a2b".into(),
            body: "1. open_url: https://x.com/compose/post\n2. bad".into(),
            maybe_one_off: vec![],
            notes: vec![],
        }
    }

    fn popup(name: &str) -> dct_explore::draft::PopupDraft {
        dct_explore::draft::PopupDraft {
            name: name.into(),
            vault: "social-global".into(),
            description: "弹窗".into(),
            learned_from: "run:x-20261011-1a2b".into(),
            body: "应用: Chrome\n看到: Turn on notifications\n点: Not now".into(),
        }
    }

    #[test]
    fn every_add_is_an_agent_draft() {
        let d = FakeDcv::default();
        write_drafts(&d, &NoFix, &draft(), &[]).unwrap();
        let a = &d.adds.borrow()[0];
        let src = a.iter().position(|x| x == "--source").unwrap();
        assert_eq!(a[src + 1], "import:agent");
        assert!(a.contains(&"run:x-20261011-1a2b".to_string()));
        assert!(a.contains(&"--json".to_string()));
    }

    #[test]
    fn a_rejection_is_fed_back_and_fixed_within_three_rounds() {
        let d = FakeDcv::default();
        d.reject.borrow_mut().push("第 2 步看不懂".into());
        let w = write_drafts(&d, &FixOnce, &draft(), &[]).unwrap();
        assert_eq!(w.rounds, 2);
        assert!(d.adds.borrow()[1].iter().any(|x| x.contains("good")));
    }

    #[test]
    fn three_rejections_give_the_last_body_and_reason_back() {
        let d = FakeDcv::default();
        for _ in 0..3 {
            d.reject.borrow_mut().push("还是不对".into());
        }
        let e = write_drafts(&d, &FixOnce, &draft(), &[]).unwrap_err();
        assert_eq!(d.adds.borrow().len(), 3);
        assert_eq!(e.1, "还是不对");
        assert!(e.0.contains("good"));
    }

    #[test]
    fn without_a_fixer_the_first_rejection_goes_to_the_user() {
        let d = FakeDcv::default();
        d.reject.borrow_mut().push("不收".into());
        assert!(write_drafts(&d, &NoFix, &draft(), &[]).is_err());
        assert_eq!(d.adds.borrow().len(), 1);
    }

    #[test]
    fn an_existing_name_gets_a_new_draft_that_points_at_it() {
        let d = FakeDcv {
            exists: vec!["social-global:promote-x-post".into()],
            ..Default::default()
        };
        let w = write_drafts(&d, &NoFix, &draft(), &[]).unwrap();
        assert_eq!(w.procedure, "social-global:promote-x-post-1a2b");
        let a = &d.adds.borrow()[0];
        assert_eq!(
            a[a.iter().position(|x| x == "--draft-of").unwrap() + 1],
            "promote-x-post"
        );
    }

    #[test]
    fn popups_are_written_each_once_as_agent_drafts() {
        let d = FakeDcv::default();
        let w = write_drafts(&d, &NoFix, &draft(), &[popup("a"), popup("b")]).unwrap();
        assert_eq!(
            w.popups,
            vec!["social-global:a".to_string(), "social-global:b".to_string()]
        );
        assert!(w.popup_failures.is_empty());
        let adds = d.adds.borrow();
        assert_eq!(adds.len(), 3);
        assert!(
            adds[1].contains(&"popup".to_string()) && adds[1].contains(&"import:agent".to_string())
        );
    }

    #[test]
    fn popup_failures_are_not_retried() {
        struct PopupRejecter(RefCell<u32>);
        impl crate::procedures::Dcv for PopupRejecter {
            fn show(&self, _: &str) -> anyhow::Result<serde_json::Value> {
                anyhow::bail!("没有")
            }
            fn approve(&self, _: &str, s: &str) -> anyhow::Result<String> {
                Ok(s.into())
            }
            fn add(&self, r: &crate::procedures::AddReq) -> anyhow::Result<serde_json::Value> {
                if matches!(r.kind, crate::procedures::AddKind::Popup) {
                    *self.0.borrow_mut() += 1;
                    anyhow::bail!("弹窗不收")
                }
                Ok(serde_json::json!({}))
            }
            fn vocab(&self) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
            fn popups(&self, _: &str) -> anyhow::Result<Vec<serde_json::Value>> {
                Ok(vec![])
            }
        }
        let d = PopupRejecter(RefCell::new(0));
        let w = write_drafts(&d, &FixOnce, &draft(), &[popup("a")]).unwrap();
        assert_eq!(*d.0.borrow(), 1);
        assert!(w.popups.is_empty());
        assert_eq!(
            w.popup_failures,
            vec![("a".to_string(), "弹窗不收".to_string())]
        );
    }
}
