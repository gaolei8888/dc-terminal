//! 把摸索得到的草稿交给 dcv。总是 `--source import:agent`；dcv 拒收就把原因交给修草稿的人（最多 3 轮）。
//! 这里只加草稿，绝不批准：agent 写的东西永远不给自己放行。
use crate::procedures::{AddKind, AddReq, Dcv};
use dct_explore::draft::{PopupDraft, ProcedureDraft};
use dct_explore::hands::Hands;

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
    /// dcv 最后收下的正文（可能是修过的）：确认卡片按这份显示。
    pub final_body: String,
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
        final_body: body,
        popup_failures,
    })
}

/// 给用户看的确认卡片：`text` 是终端里的人话，`json` 给 --json 用。
pub struct Card {
    pub text: String,
    pub json: serde_json::Value,
}

/// 去掉成对的「」。
fn unquote(s: &str) -> &str {
    let t = s.trim();
    t.strip_prefix('「')
        .and_then(|x| x.strip_suffix('」'))
        .unwrap_or(t)
}

/// dcv 正文里的一步（`N. 动作: 内容`）翻成人话。标记行（3 个空格开头）跳过。
fn plain_step(n: u32, action: &str, arg: &str) -> String {
    let arg = arg.trim();
    match action {
        "open_url" => format!("打开 {arg}"),
        "verify_text" if n == 2 => format!("确认已经登录（看得到「{}」）", unquote(arg)),
        "verify_text" => format!("确认看得到「{}」", unquote(arg)),
        "tap_by_intent" => format!("点「{arg}」"),
        "type_param" => {
            let name = arg.trim_matches(|c| c == '{' || c == '}');
            let is_text = matches!(name, "text" | "title" | "description" | "caption")
                || name.ends_with("_text");
            if is_text {
                "填入要发的文字".to_string()
            } else {
                format!("填入「{name}」这一项")
            }
        }
        "pick_file" => format!("选文件（{}）", arg.trim_matches(|c| c == '{' || c == '}')),
        "scroll" if arg.contains("up") || arg.contains('上') => "往上滚".to_string(),
        "scroll" => "往下滚".to_string(),
        "wait" => "等一下".to_string(),
        "back" => "返回".to_string(),
        // 词表以外的不该出现（dcv 已验过）；退而求其次照抄内容，不露动作词。
        _ => arg.to_string(),
    }
}

fn plain_steps(body: &str) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for line in body.lines() {
        if line.starts_with("   ") {
            continue;
        }
        let Some((num, rest)) = line.split_once(". ") else {
            continue;
        };
        let Ok(n) = num.trim().parse::<u32>() else {
            continue;
        };
        let (action, arg) = rest.split_once(':').unwrap_or((rest, ""));
        out.push((n, plain_step(n, action.trim(), arg)));
    }
    out
}

fn popup_line(d: &PopupDraft) -> String {
    let field = |k: &str| {
        d.body
            .lines()
            .find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string()))
            .unwrap_or_default()
    };
    format!("看到「{}」就点「{}」", field("看到:"), field("点:"))
}

pub fn render_card(
    w: &Written,
    p: &ProcedureDraft,
    pops: &[PopupDraft],
    learned: &[String],
) -> Card {
    let steps = plain_steps(&w.final_body);
    let what = p.description.split('（').next().unwrap_or(&p.description);
    let name = w
        .procedure
        .split_once(':')
        .map_or(w.procedure.as_str(), |x| x.1);
    let mut t = format!("学会了：{}（{}，草稿）\n", what, name);
    for (n, s) in &steps {
        t += &format!("  {n}. {s}\n");
    }
    if !pops.is_empty() {
        t += &format!("另外记下 {} 条弹窗规则：\n", pops.len());
        for d in pops {
            t += &format!("  {}\n", popup_line(d));
        }
    }
    if !learned.is_empty() {
        t += &format!(
            "摸索时学会了 {} 个按钮（确认后重放直接用）\n",
            learned.len()
        );
    }
    if !p.maybe_one_off.is_empty() {
        let ns: Vec<String> = p.maybe_one_off.iter().map(|n| n.to_string()).collect();
        t += &format!(
            "⚠ 第 {} 步可能只偶尔出现（弹窗里的按钮），下次没弹出来会卡住，到时会再补。\n",
            ns.join("、")
        );
    }
    for n in &p.notes {
        t += &format!("⚠ {n}\n");
    }
    for (name, why) in &w.popup_failures {
        t += &format!("⚠ 弹窗规则「{name}」没能记下：{why}\n");
    }
    t += "确认后以后就照这个自动发（需要按一下指纹）。确认吗？[y/N]";
    let json = serde_json::json!({
        "procedure": w.procedure,
        "steps": steps.iter().map(|(n, s)| serde_json::json!({"n": n, "text": s})).collect::<Vec<_>>(),
        "popups": w.popups,
        "popup_rules": pops.iter().map(popup_line).collect::<Vec<_>>(),
        "learned": learned,
        "maybe_one_off": p.maybe_one_off,
        "notes": p.notes,
    });
    Card { text: t, json }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    Yes,
    No,
}

/// Yes：先批准流程（`approve_proc`，会弹 Touch ID），成功后才批准弹窗草稿、确认学到的按钮；
/// 流程这一步失败就整个返回 Err，什么都不生效。
/// No：只把学到的按钮标成拒绝，草稿留在 dcv 里不动。
pub fn apply_confirmation(
    v: Verdict,
    dcv: &dyn Dcv,
    approve_proc: &mut dyn FnMut(&str) -> anyhow::Result<()>,
    hands: &mut dyn Hands,
    w: &Written,
    learned: &[String],
) -> anyhow::Result<()> {
    match v {
        Verdict::No => {
            if !learned.is_empty() {
                hands
                    .confirm_learned(&[], learned)
                    .map_err(|e| anyhow::anyhow!("{}", e.message))?;
            }
        }
        Verdict::Yes => {
            approve_proc(&w.procedure)?;
            for name in &w.popups {
                let shown = dcv.show(name)?;
                let sha = shown["body_sha256"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("dcv 回复缺 body_sha256"))?;
                dcv.approve(name, sha)?;
            }
            if !learned.is_empty() {
                hands
                    .confirm_learned(learned, &[])
                    .map_err(|e| anyhow::anyhow!("{}", e.message))?;
            }
        }
    }
    Ok(())
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
            body: "应用: x\n看到: Turn on notifications\n点: Not now".into(),
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

    // ---- D13: confirmation card and verdict ----

    fn written() -> Written {
        Written {
            procedure: "social-global:promote-x-post".into(),
            popups: vec!["social-global:tiktok-turn-on-automatic-con".into()],
            rounds: 1,
            popup_failures: vec![],
            final_body: draft_with_one_off().body,
        }
    }

    fn draft_with_one_off() -> dct_explore::draft::ProcedureDraft {
        let mut d = draft();
        d.body = "1. open_url: https://x.com/compose/post\n2. verify_text: 「What's happening?」\n3. tap_by_intent: What's happening?\n4. type_param: post_text\n5. tap_by_intent: Post".into();
        d.maybe_one_off = vec![3, 5];
        d.notes = vec!["有一次点击没能写成弹窗规则".into()];
        d
    }

    fn card_popup() -> dct_explore::draft::PopupDraft {
        dct_explore::draft::PopupDraft {
            name: "tiktok-turn-on-automatic-con".into(),
            vault: "social-global".into(),
            description: "弹窗".into(),
            learned_from: "run:x-20261011-1a2b".into(),
            body: "应用: tiktok\n看到: Turn on automatic content checks?\n点: Cancel".into(),
        }
    }

    struct FakeDcvWithShow {
        approved: RefCell<Vec<String>>,
    }
    impl FakeDcvWithShow {
        fn new() -> Self {
            Self {
                approved: RefCell::new(vec![]),
            }
        }
        fn approved_popups(&self) -> Vec<String> {
            self.approved.borrow().clone()
        }
    }
    impl crate::procedures::Dcv for FakeDcvWithShow {
        fn show(&self, _: &str) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({"kind": "popup", "body_sha256": "sha256:aa"}))
        }
        fn approve(&self, n: &str, s: &str) -> anyhow::Result<String> {
            assert_eq!(s, "sha256:aa");
            self.approved.borrow_mut().push(n.into());
            Ok(s.into())
        }
        fn add(&self, _: &crate::procedures::AddReq) -> anyhow::Result<serde_json::Value> {
            unreachable!()
        }
        fn vocab(&self) -> anyhow::Result<serde_json::Value> {
            unreachable!()
        }
        fn popups(&self, _: &str) -> anyhow::Result<Vec<serde_json::Value>> {
            unreachable!()
        }
    }

    #[test]
    fn the_card_lists_steps_in_plain_words_and_flags_one_off_steps() {
        let c = render_card(
            &written(),
            &draft_with_one_off(),
            &[card_popup()],
            &["k1".into()],
        );
        assert!(c.text.contains("1. 打开 https://x.com/compose/post"));
        assert!(c
            .text
            .contains("2. 确认已经登录（看得到「What's happening?」）"));
        assert!(c.text.contains("3. 点「What's happening?」"));
        assert!(c.text.contains("填入要发的文字"));
        assert!(c
            .text
            .contains("看到「Turn on automatic content checks?」就点「Cancel」"));
        for w in [
            "type_param",
            "tap_by_intent",
            "open_url",
            "verify_text",
            "pick_file",
        ] {
            assert!(!c.text.contains(w), "给人看的不出现动作词 {w}");
        }
        assert!(c.text.contains("可能只偶尔出现"));
        assert!(c.text.contains("第 3、5 步"));
        assert!(c.text.contains("有一次点击没能写成弹窗规则"));
        assert!(c.text.contains("需要按一下指纹"));
        assert_eq!(c.json["procedure"], "social-global:promote-x-post");
        assert_eq!(c.json["steps"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn other_actions_are_plain_words_too() {
        let mut d = draft_with_one_off();
        d.maybe_one_off = vec![];
        d.notes = vec![];
        d.body = "1. open_url: https://x.com\n2. pick_file: video\n3. scroll: down\n4. scroll: up\n5. wait: 1\n6. back: \n7. verify_text: 「Done」\n8. type_param: title".into();
        let mut w = written();
        w.final_body = d.body.clone();
        let c = render_card(&w, &d, &[], &[]);
        for want in [
            "选文件（video）",
            "往下滚",
            "往上滚",
            "等一下",
            "返回",
            "3. ",
            "7. 确认看得到「Done」",
            "8. 填入",
        ] {
            assert!(c.text.contains(want), "缺 {want}: {}", c.text);
        }
        for w in ["pick_file", "scroll", "wait:", "back", "type_param"] {
            assert!(!c.text.contains(w), "动作词 {w}: {}", c.text);
        }
    }

    #[test]
    fn yes_approves_procedure_popups_and_learned_buttons() {
        let mut approved = vec![];
        let mut h = dct_explore::testing::FakeHands::default();
        let d = FakeDcvWithShow::new();
        apply_confirmation(
            Verdict::Yes,
            &d,
            &mut |n| {
                approved.push(n.to_string());
                Ok(())
            },
            &mut h,
            &written(),
            &["k1".into()],
        )
        .unwrap();
        assert_eq!(approved, vec!["social-global:promote-x-post"]);
        assert_eq!(
            d.approved_popups(),
            vec!["social-global:tiktok-turn-on-automatic-con"]
        );
        assert!(h.calls.iter().any(|c| c == "confirm_learned accept=[k1]"));
    }

    #[test]
    fn no_rejects_learned_buttons_and_approves_nothing() {
        let mut approved: Vec<String> = vec![];
        let mut h = dct_explore::testing::FakeHands::default();
        let d = FakeDcvWithShow::new();
        apply_confirmation(
            Verdict::No,
            &d,
            &mut |n| {
                approved.push(n.into());
                Ok(())
            },
            &mut h,
            &written(),
            &["k1".into()],
        )
        .unwrap();
        assert!(approved.is_empty());
        assert!(d.approved_popups().is_empty());
        assert!(h.calls.iter().any(|c| c == "confirm_learned reject=[k1]"));
    }

    #[test]
    fn a_failed_touch_id_leaves_popups_and_learned_unconfirmed() {
        let mut h = dct_explore::testing::FakeHands::default();
        let d = FakeDcvWithShow::new();
        let r = apply_confirmation(
            Verdict::Yes,
            &d,
            &mut |_| anyhow::bail!("用户取消了指纹"),
            &mut h,
            &written(),
            &["k1".into()],
        );
        assert!(r.is_err());
        assert!(d.approved_popups().is_empty());
        assert!(h.calls.is_empty(), "流程没批准，学到的按钮也不确认");
    }

    #[test]
    fn the_card_shows_the_body_the_fixer_produced() {
        struct Swap;
        impl Fixer for Swap {
            fn fix(&self, b: &str, _: &str) -> Option<String> {
                Some(b.replace("Poost", "Post"))
            }
        }
        let d = FakeDcv::default();
        d.reject.borrow_mut().push("第 2 步不对".into());
        let mut p = draft();
        p.body = "1. open_url: https://x.com/compose/post\n2. tap_by_intent: Poost".into();
        let w = write_drafts(&d, &Swap, &p, &[]).unwrap();
        let c = render_card(&w, &p, &[], &[]);
        assert!(c.text.contains("点「Post」"), "{}", c.text);
        assert!(!c.text.contains("Poost"), "{}", c.text);
        assert_eq!(c.json["steps"][1]["text"], "点「Post」");
    }

    #[test]
    fn the_card_header_shows_the_renamed_procedure() {
        let d = FakeDcv {
            exists: vec!["social-global:promote-x-post".into()],
            ..Default::default()
        };
        let p = draft();
        let w = write_drafts(&d, &NoFix, &p, &[]).unwrap();
        let c = render_card(&w, &p, &[], &[]);
        assert!(c.text.contains("promote-x-post-1a2b，草稿"), "{}", c.text);
        assert_eq!(c.json["procedure"], "social-global:promote-x-post-1a2b");
    }

    #[test]
    fn notes_and_popup_failures_appear_on_the_card() {
        let mut w = written();
        w.popup_failures = vec![("tiktok-x".into(), "弹窗不收".into())];
        let c = render_card(&w, &draft_with_one_off(), &[], &[]);
        assert!(c.text.contains("有一次点击没能写成弹窗规则"));
        assert!(c.text.contains("tiktok-x") && c.text.contains("弹窗不收"));
    }

    #[test]
    fn only_real_text_params_become_the_text_to_post() {
        let mut d = draft_with_one_off();
        d.maybe_one_off = vec![];
        d.body = "1. open_url: https://x.com\n2. type_param: context\n3. type_param: caption\n4. type_param: post_text".into();
        let mut w = written();
        w.final_body = d.body.clone();
        let c = render_card(&w, &d, &[], &[]);
        assert!(c.text.contains("2. 填入「context」这一项"), "{}", c.text);
        assert!(c.text.contains("3. 填入要发的文字"));
        assert!(c.text.contains("4. 填入要发的文字"));
    }
}
