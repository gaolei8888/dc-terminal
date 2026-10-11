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
    /// 和 `popups` 一一对应：写进 dcv 的弹窗正文（确认时按它核对 dcv 里的那一份）。
    pub popup_bodies: Vec<String>,
    /// 没写进去的弹窗草稿：(名字, dcv 的拒收原因)。不重试，不影响流程草稿。
    pub popup_failures: Vec<(String, String)>,
}

/// dcv 的正文指纹（dc-vault procedure.rs `normalize_body` + `body_sha_of`：`\r\n` 折成 `\n`、
/// 去掉首尾换行，再算 sha256，写成 `sha256:<hex>`）。流程、弹窗规则同一种算法。
pub fn body_sha256(body: &str) -> String {
    let normalized = body.replace("\r\n", "\n");
    dct_brain::canon::sha256_id(normalized.trim_matches('\n').as_bytes())
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
    let mut popup_bodies = Vec::new();
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
            Ok(_) => {
                popups.push(format!("{}:{}", d.vault, d.name));
                popup_bodies.push(d.body.clone());
            }
            Err(e) => popup_failures.push((d.name.clone(), e.to_string())),
        }
    }
    Ok(Written {
        procedure: format!("{}:{}", p.vault, name),
        popups,
        rounds,
        final_body: body,
        popup_bodies,
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

/// dcv 正文里的一步（`N. 动作: 内容`）翻成人话。标记行（3 个空格开头）见 `body_lines`。
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
        // 没有人话翻译的动作：整行照抄（动作词也露出来），不能只给内容把动作藏起来。
        _ => format!("（原文）{action}: {arg}"),
    }
}

/// 正文的一行：步骤（`N. 动作: 内容`）翻成人话；别的行（完成标志、缩进的、看不懂的）原样
/// 显示出来，一行都不跳过——正文里有什么，卡片上就有什么。
enum Line {
    Step(u32, String),
    Other(String),
}

fn body_lines(body: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(m) = line.strip_prefix("   ") {
            let m = m.trim();
            let shown = match m
                .strip_prefix("完成标志:")
                .map(str::trim)
                .and_then(|v| v.strip_prefix("verify_text"))
            {
                Some(t) => format!("（做完后确认看得到「{}」）", unquote(t)),
                _ => format!("（原文）{m}"),
            };
            out.push(Line::Other(shown));
            continue;
        }
        let step = line.split_once(". ").and_then(|(num, rest)| {
            let n = num.trim().parse::<u32>().ok()?;
            let (action, arg) = rest.split_once(':').unwrap_or((rest, ""));
            Some(Line::Step(n, plain_step(n, action.trim(), arg)))
        });
        out.push(step.unwrap_or_else(|| Line::Other(format!("（原文）{}", line.trim()))));
    }
    out
}

/// 看不见或会改显示顺序的字符（同 campaign 的那张表），再加上换行以外的控制字符（含制表符）：
/// 这种字能让卡片上看到的和 dcv 里存的不是一回事。
fn hidden_char(s: &str) -> Option<char> {
    s.chars()
        .find(|&c| c != '\n' && (c == '\t' || crate::campaign::is_hidden_or_control(c)))
}

/// 草稿里标出的「可能只偶尔出现」的步号是按原来的正文算的；修过的正文按同一行内容
/// （`动作: 内容`）重新找步号，有一个找不到或对上不止一行 → None（不标步号）。
fn one_off_in_final(p: &ProcedureDraft, final_body: &str) -> Option<Vec<u32>> {
    if final_body == p.body {
        return Some(p.maybe_one_off.clone());
    }
    let steps = |body: &str| -> Vec<(u32, String)> {
        body.lines()
            .filter(|l| !l.starts_with("   "))
            .filter_map(|l| {
                let (num, rest) = l.split_once(". ")?;
                Some((num.trim().parse::<u32>().ok()?, rest.trim().to_string()))
            })
            .collect()
    };
    let (old, new) = (steps(&p.body), steps(final_body));
    p.maybe_one_off
        .iter()
        .map(|n| {
            let (_, content) = old.iter().find(|(m, _)| m == n)?;
            match new.iter().filter(|(_, c)| c == content).collect::<Vec<_>>()[..] {
                [(m, _)] => Some(*m),
                _ => None,
            }
        })
        .collect()
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
) -> Result<Card, String> {
    for (what, text) in std::iter::once(("流程正文", w.final_body.as_str()))
        .chain(pops.iter().map(|d| ("弹窗规则", d.body.as_str())))
    {
        if let Some(c) = hidden_char(text) {
            return Err(format!(
                "{what}里有看不见的字符或控制字符（U+{:04X}），卡片上看到的可能不是 dcv 里存的",
                c as u32
            ));
        }
    }
    let lines = body_lines(&w.final_body);
    let steps: Vec<(u32, &str)> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Step(n, s) => Some((*n, s.as_str())),
            Line::Other(_) => None,
        })
        .collect();
    let one_off = one_off_in_final(p, &w.final_body);
    let what = p.description.split('（').next().unwrap_or(&p.description);
    let name = w
        .procedure
        .split_once(':')
        .map_or(w.procedure.as_str(), |x| x.1);
    let mut t = format!("学会了：{}（{}，草稿）\n", what, name);
    for l in &lines {
        match l {
            Line::Step(n, s) => t += &format!("  {n}. {s}\n"),
            Line::Other(s) => t += &format!("       {s}\n"),
        }
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
    match &one_off {
        Some(v) if !v.is_empty() => {
            let ns: Vec<String> = v.iter().map(|n| n.to_string()).collect();
            t += &format!(
                "⚠ 第 {} 步可能只偶尔出现（弹窗里的按钮），下次没弹出来会卡住，到时会再补。\n",
                ns.join("、")
            );
        }
        Some(_) => {}
        None => {
            t += "⚠ 正文被修过，原来标出的「可能只偶尔出现」的步骤在修过的正文里对不上了，这次没有标步号。\n";
        }
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
        "maybe_one_off": one_off.clone().unwrap_or_default(),
        "body_lines": lines.iter().map(|l| match l {
            Line::Step(n, s) => format!("{n}. {s}"),
            Line::Other(s) => s.clone(),
        }).collect::<Vec<_>>(),
        "notes": p.notes,
    });
    if let Some(c) = hidden_char(&t) {
        return Err(format!(
            "卡片里有看不见的字符或控制字符（U+{:04X}），不给你确认",
            c as u32
        ));
    }
    Ok(Card { text: t, json })
}

/// dcv 里现在的这一份和卡片上给用户看的是不是同一版（按 dcv 的正文指纹比）。
fn check_shown(dcv: &dyn Dcv, name: &str, want: &str, what: &str) -> anyhow::Result<()> {
    let shown = dcv.show(name)?;
    let got = shown["body_sha256"].as_str().unwrap_or("");
    if got != want {
        anyhow::bail!(
            "dcv 里的{what} {name} 和卡片上给你看的不一样（卡片是 {want}，dcv 里是 {got}），这次什么都没批准；请重新跑一次再看"
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    Yes,
    No,
}

/// Yes：先核对 dcv 里的流程和弹窗草稿就是卡片上给用户看的那一版（正文指纹）、里面没有看不见的
/// 字符，再批准流程（`approve_proc`，带着那个指纹，会弹 Touch ID），成功后才批准弹窗草稿、
/// 确认学到的按钮；任何一步失败就整个返回 Err，什么都不生效。
/// No：只把学到的按钮标成拒绝，草稿留在 dcv 里不动。
pub fn apply_confirmation(
    v: Verdict,
    dcv: &dyn Dcv,
    approve_proc: &mut dyn FnMut(&str, &str) -> anyhow::Result<()>,
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
            // 先全部核对（不弹指纹）：看不见的字符、dcv 里的流程和弹窗是不是卡片上那一版。
            let bodies = std::iter::once(&w.final_body).chain(&w.popup_bodies);
            if let Some(c) = bodies.filter_map(|b| hidden_char(b)).next() {
                anyhow::bail!(
                    "草稿里有看不见的字符或控制字符（U+{:04X}），不批准",
                    c as u32
                );
            }
            if w.popup_bodies.len() != w.popups.len() {
                anyhow::bail!("弹窗规则和它们的正文对不上，不批准");
            }
            let want = body_sha256(&w.final_body);
            check_shown(dcv, &w.procedure, &want, "流程")?;
            let pops: Vec<(&String, String)> = w
                .popups
                .iter()
                .zip(&w.popup_bodies)
                .map(|(n, b)| (n, body_sha256(b)))
                .collect();
            for (name, sha) in &pops {
                check_shown(dcv, name, sha, "弹窗规则")?;
            }
            // Touch ID 绑在这个指纹上（approve_proc 再让 dcv 在锁里核一遍）。
            approve_proc(&w.procedure, &want)?;
            for (name, sha) in &pops {
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
            popup_bodies: vec![card_popup().body],
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
        fn show(&self, n: &str) -> anyhow::Result<serde_json::Value> {
            let body = if n.ends_with(":promote-x-post") {
                written().final_body
            } else {
                card_popup().body
            };
            Ok(serde_json::json!({"body_sha256": body_sha256(&body)}))
        }
        fn approve(&self, n: &str, s: &str) -> anyhow::Result<String> {
            assert_eq!(s, body_sha256(&card_popup().body));
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
        )
        .unwrap();
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
        let c = render_card(&w, &d, &[], &[]).unwrap();
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
            &mut |n, _| {
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
            &mut |n, _| {
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
            &mut |_, _| anyhow::bail!("用户取消了指纹"),
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
        let c = render_card(&w, &p, &[], &[]).unwrap();
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
        let c = render_card(&w, &p, &[], &[]).unwrap();
        assert!(c.text.contains("promote-x-post-1a2b，草稿"), "{}", c.text);
        assert_eq!(c.json["procedure"], "social-global:promote-x-post-1a2b");
    }

    #[test]
    fn notes_and_popup_failures_appear_on_the_card() {
        let mut w = written();
        w.popup_failures = vec![("tiktok-x".into(), "弹窗不收".into())];
        let c = render_card(&w, &draft_with_one_off(), &[], &[]).unwrap();
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
        let c = render_card(&w, &d, &[], &[]).unwrap();
        assert!(c.text.contains("2. 填入「context」这一项"), "{}", c.text);
        assert!(c.text.contains("3. 填入要发的文字"));
        assert!(c.text.contains("4. 填入要发的文字"));
    }

    // ---- 确认只批准卡片上给用户看的那一版；卡片上不许有看不见的字 ----

    /// dcv 里每个名字对应的正文（show 回 dcv 算的指纹）。
    struct BodiesDcv {
        bodies: Vec<(String, String)>,
        approved: RefCell<Vec<(String, String)>>,
    }
    impl crate::procedures::Dcv for BodiesDcv {
        fn show(&self, n: &str) -> anyhow::Result<serde_json::Value> {
            let (_, b) = self
                .bodies
                .iter()
                .find(|(k, _)| k == n)
                .ok_or_else(|| anyhow::anyhow!("没有 {n}"))?;
            Ok(serde_json::json!({"body": b, "body_sha256": body_sha256(b)}))
        }
        fn approve(&self, n: &str, s: &str) -> anyhow::Result<String> {
            self.approved.borrow_mut().push((n.into(), s.into()));
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
    fn bodies(proc_body: &str, popup_body: &str) -> BodiesDcv {
        BodiesDcv {
            bodies: vec![
                ("social-global:promote-x-post".into(), proc_body.into()),
                (
                    "social-global:tiktok-turn-on-automatic-con".into(),
                    popup_body.into(),
                ),
            ],
            approved: RefCell::new(vec![]),
        }
    }

    #[test]
    fn the_body_fingerprint_is_dcvs() {
        // sha256("abc")；dcv 先把 \r\n 折成 \n、去掉首尾换行再算。
        let abc = "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(body_sha256("abc"), abc);
        assert_eq!(body_sha256("\n\nabc\r\n\n"), abc);
        assert_ne!(body_sha256(" abc"), abc);
    }

    #[test]
    fn touch_id_is_bound_to_the_body_the_card_showed() {
        let w = written();
        let d = bodies(&w.final_body, &card_popup().body);
        let mut seen: Vec<(String, String)> = vec![];
        let mut h = dct_explore::testing::FakeHands::default();
        apply_confirmation(
            Verdict::Yes,
            &d,
            &mut |n, sha| {
                seen.push((n.into(), sha.into()));
                Ok(())
            },
            &mut h,
            &w,
            &[],
        )
        .unwrap();
        assert_eq!(
            seen,
            vec![(w.procedure.clone(), body_sha256(&w.final_body))]
        );
        assert_eq!(
            d.approved.borrow().as_slice(),
            [(w.popups[0].clone(), body_sha256(&card_popup().body))]
        );
    }

    #[test]
    fn a_procedure_changed_in_dcv_after_the_card_is_not_approved() {
        let w = written();
        let d = bodies(
            &w.final_body.replace("Post", "Delete account"),
            &card_popup().body,
        );
        let mut called = false;
        let mut h = dct_explore::testing::FakeHands::default();
        let r = apply_confirmation(
            Verdict::Yes,
            &d,
            &mut |_, _| {
                called = true;
                Ok(())
            },
            &mut h,
            &w,
            &["k1".into()],
        );
        let e = r.unwrap_err().to_string();
        assert!(e.contains("不一样"), "{e}");
        assert!(!called, "对不上就不弹指纹");
        assert!(d.approved.borrow().is_empty());
        assert!(h.calls.is_empty());
    }

    #[test]
    fn a_popup_changed_in_dcv_after_the_card_is_not_approved() {
        let w = written();
        let d = bodies(&w.final_body, &card_popup().body.replace("Cancel", "Allow"));
        let mut called = false;
        let mut h = dct_explore::testing::FakeHands::default();
        let r = apply_confirmation(
            Verdict::Yes,
            &d,
            &mut |_, _| {
                called = true;
                Ok(())
            },
            &mut h,
            &w,
            &[],
        );
        assert!(r.is_err());
        assert!(!called, "弹窗也先核对，再弹指纹");
        assert!(d.approved.borrow().is_empty());
    }

    #[test]
    fn a_body_with_hidden_or_control_characters_is_neither_shown_nor_approved() {
        for bad in ["\u{202E}", "\u{1b}[2K", "\r", "\t", "\u{200B}", "\u{2028}"] {
            let mut w = written();
            w.final_body = format!("{}\n6. tap_by_intent: Po{bad}st", w.final_body);
            assert!(
                render_card(&w, &draft_with_one_off(), &[], &[]).is_err(),
                "{bad:?}"
            );
            let d = bodies(&w.final_body, &card_popup().body);
            let mut called = false;
            let mut h = dct_explore::testing::FakeHands::default();
            let r = apply_confirmation(
                Verdict::Yes,
                &d,
                &mut |_, _| {
                    called = true;
                    Ok(())
                },
                &mut h,
                &w,
                &[],
            );
            assert!(r.is_err() && !called, "{bad:?}");
        }
        let mut pop = card_popup();
        pop.body = pop.body.replace("Cancel", "Can\u{202E}cel");
        assert!(render_card(&written(), &draft_with_one_off(), &[pop], &[]).is_err());
    }

    #[test]
    fn every_line_of_the_body_is_on_the_card_even_indented_or_unknown_ones() {
        let mut d = draft_with_one_off();
        d.maybe_one_off = vec![];
        d.notes = vec![];
        d.body = "1. open_url: https://x.com\n2. tap_by_intent: Post\n   完成标志: verify_text 「Your post was sent」\n   3. tap_by_intent: Buy coins\nsomething else\n4. confirm_dialog: OK".into();
        let mut w = written();
        w.final_body = d.body.clone();
        let c = render_card(&w, &d, &[], &[]).unwrap();
        assert!(
            c.text.contains("看得到「Your post was sent」"),
            "{}",
            c.text
        );
        assert!(c.text.contains("3. tap_by_intent: Buy coins"), "{}", c.text);
        assert!(c.text.contains("something else"), "{}", c.text);
        assert!(
            c.text.contains("4. （原文）confirm_dialog: OK"),
            "{}",
            c.text
        );
    }

    #[test]
    fn one_off_step_numbers_follow_the_fixed_body() {
        // 修过的正文在前面多了一步：原来的第 3 步变成第 4 步。
        let mut w = written();
        w.final_body = "1. open_url: https://x.com/compose/post\n2. verify_text: 「What's happening?」\n3. wait: 2\n4. tap_by_intent: What's happening?\n5. type_param: post_text\n6. tap_by_intent: Post".into();
        w.rounds = 2;
        let c = render_card(&w, &draft_with_one_off(), &[], &[]).unwrap();
        assert!(c.text.contains("第 4、6 步"), "{}", c.text);
        assert_eq!(c.json["maybe_one_off"], serde_json::json!([4, 6]));

        // 对不上（修过的正文里那一步没了）：不标步号，说一句。
        w.final_body = "1. open_url: https://x.com/compose/post\n2. verify_text: 「What's happening?」\n3. type_param: post_text".into();
        let c = render_card(&w, &draft_with_one_off(), &[], &[]).unwrap();
        assert!(!c.text.contains("第 3、5 步"), "{}", c.text);
        assert!(c.text.contains("修过"), "{}", c.text);
        assert_eq!(c.json["maybe_one_off"], serde_json::json!([]));
    }
}
