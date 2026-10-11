//! 轨迹 → dcv 流程正文（和弹窗规则草稿）。纯代码照抄，不经模型。
//! 正文格式照 dc-vault 的解析器：`N. 动作: 内容`，完成标志行缩进 3 个空格。
use crate::gate::fold;
use crate::platform::Platform;
use crate::trace::{Trace, TraceStep};

#[derive(Debug, Clone, PartialEq)]
pub struct ProcedureDraft {
    pub name: String, // promote-<platform>-<post|video>
    pub vault: String,
    pub description: String,
    pub params: Vec<String>,
    pub platform: String,
    pub learned_from: String, // run:<id>
    pub body: String,         // dcv 正文
    /// 可能只偶尔出现的步骤号（确认卡片上标出）
    pub maybe_one_off: Vec<u32>,
    /// 给确认卡片的提示（比如关弹窗的点击因为弹窗文字不合格而没能写成规则）
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PopupDraft {
    pub name: String,
    pub vault: String,
    pub description: String,
    pub learned_from: String,
    pub body: String,
}

#[derive(Debug, PartialEq)]
pub enum DraftError {
    LoginGateNotSeen(String),
    NoPublish,
    Empty,
    /// 某一步的内容为空或含换行，写出来 dcv 会拒收（步骤号从 1 数，含自动插入的两步）。
    BadStepArg(String),
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// 稳定的 64 位 FNV-1a，取 8 位十六进制；不同 Rust 版本结果一样（不能用 DefaultHasher）。
fn short_hash(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for part in parts {
        for b in part.bytes().chain(std::iter::once(0u8)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    format!("{:08x}", h >> 32)
}

fn is_dismiss(arg: &str, dismiss: &[String]) -> bool {
    let a = fold(arg);
    dismiss.iter().any(|d| fold(d) == a)
}

fn marker_ok(s: &str) -> bool {
    !s.contains('{') && !s.contains('}') && !s.contains('\n')
}

pub fn draft(
    t: &Trace,
    p: &Platform,
    dismiss: &[String],
    file_params: &[String],
) -> Result<(ProcedureDraft, Vec<PopupDraft>), DraftError> {
    let mut steps: Vec<&TraceStep> = t.steps.iter().filter(|s| s.ok).collect();
    if steps.is_empty() {
        return Err(DraftError::Empty);
    }
    // 登录闸：必须是第一屏上真看到过的字。
    let gate = match p.login_gate.as_deref().map(str::trim) {
        Some(g) if !g.is_empty() && marker_ok(g) => g,
        other => {
            return Err(DraftError::LoginGateNotSeen(
                other.unwrap_or("").to_string(),
            ))
        }
    };
    let fg = fold(gate);
    if !t.first_lines.iter().any(|l| fold(l).contains(&fg)) {
        return Err(DraftError::LoginGateNotSeen(gate.to_string()));
    }
    if !steps.iter().any(|s| s.publish) {
        return Err(DraftError::NoPublish);
    }
    // 开头的 open_url 由平台表的入口网址代替（域名写死）。
    if steps[0].action == "open_url" {
        steps.remove(0);
    }

    // 关弹窗的点击 → 弹窗规则草稿，不进步骤。
    let mut notes = Vec::new();
    let mut pops: Vec<PopupDraft> = Vec::new();
    let learned_from = format!("run:{}", t.run_id);
    let mut kept: Vec<&TraceStep> = Vec::new();
    for s in steps {
        if s.action == "tap_by_intent" && !s.vanished.is_empty() && is_dismiss(&s.arg, dismiss) {
            let seen = s
                .vanished
                .iter()
                .map(|v| v.trim())
                .max_by_key(|v| v.chars().count())
                .unwrap_or("");
            let n = seen.chars().count();
            if (4..=200).contains(&n) && !seen.contains('\n') {
                // 名字 = 平台-弹窗文字的 ASCII 前缀-短哈希：中文弹窗前缀为空也不会撞名；
                // 哈希同时算了点的字，名字相同就说明内容相同。
                let sl: String = slug(seen).chars().take(24).collect();
                let sl = sl.trim_matches('-');
                let hash = short_hash(&[seen, &s.arg]);
                let name = if sl.is_empty() {
                    format!("{}-popup-{hash}", p.name)
                } else {
                    format!("{}-{sl}-{hash}", p.name)
                };
                if !pops.iter().any(|x| x.name == name) {
                    pops.push(PopupDraft {
                        name,
                        vault: p.vault.clone(),
                        description: format!(
                            "{} 上的弹窗「{}」，点 {} 关掉",
                            p.name,
                            seen.replace('"', ""),
                            s.arg.replace('"', "")
                        ),
                        learned_from: learned_from.clone(),
                        body: format!("应用: {}\n看到: {}\n点: {}", p.browser, seen, s.arg),
                    });
                }
            } else {
                notes.push(format!(
                    "点「{}」关掉的弹窗文字不合格（要 4 到 200 个字、一行），没能写成弹窗规则，也没进步骤",
                    s.arg
                ));
            }
            continue;
        }
        kept.push(s);
    }

    // 同一个参数的 type_param 只留最后一次，连同它前面那一下填字的点击。
    let mut drop = vec![false; kept.len()];
    for (i, s) in kept.iter().enumerate() {
        if s.action != "type_param" {
            continue;
        }
        let later = kept[i + 1..]
            .iter()
            .any(|o| o.action == "type_param" && o.arg == s.arg);
        if later {
            drop[i] = true;
            if i > 0 && kept[i - 1].action == "tap_by_intent" {
                drop[i - 1] = true;
            }
        }
    }
    let kept: Vec<&TraceStep> = kept
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !drop[*i])
        .map(|(_, s)| s)
        .collect();

    // 拼正文：1 open_url，2 verify_text，然后抄。
    let mut lines = vec![
        format!("1. open_url: {}", p.entry_url),
        format!("2. verify_text: 「{gate}」"),
    ];
    let mut n: u32 = 2;
    let mut maybe_one_off = Vec::new();
    let mut after_publish = false;
    for s in kept {
        n += 1;
        if s.action != "back" && (s.arg.trim().is_empty() || s.arg.contains('\n')) {
            return Err(DraftError::BadStepArg(format!(
                "第 {n} 步 {} 的内容为空或含换行",
                s.action
            )));
        }
        lines.push(format!("{n}. {}: {}", s.action, s.arg));
        if s.publish {
            after_publish = true;
            if let Some(done) = p.publish_done.as_deref().map(str::trim) {
                if !done.is_empty()
                    && marker_ok(done)
                    && !p.unverified.iter().any(|u| u == "publish_done")
                {
                    lines.push(format!("   完成标志: verify_text 「{done}」"));
                }
            }
        } else if s.action == "tap_by_intent"
            && (after_publish || !s.vanished.is_empty())
            && !is_dismiss(&s.arg, dismiss)
        {
            maybe_one_off.push(n);
        }
    }

    let video = !file_params.is_empty();
    let kind = if video { "video" } else { "post" };
    let what = if video {
        "一个视频"
    } else {
        "一条帖子"
    };
    Ok((
        ProcedureDraft {
            name: format!("promote-{}-{kind}", p.name),
            vault: p.vault.clone(),
            description: format!("在 {} 上发{what}（dct 摸索学会，run:{}）", p.name, t.run_id),
            params: t.params.clone(),
            platform: p.name.clone(),
            learned_from,
            body: lines.join("\n"),
            maybe_one_off,
            notes,
        },
        pops,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ladder::Who;
    use crate::platform::Table;
    use crate::trace::{Trace, TraceStep};

    fn s(action: &str, arg: &str, publish: bool, vanished: &[&str]) -> TraceStep {
        TraceStep {
            action: action.into(),
            arg: arg.into(),
            by: Who::Jeff,
            obs_before: None,
            obs_after: None,
            scene_before: None,
            scene_after: None,
            ok: true,
            publish,
            vanished: vanished.iter().map(|v| v.to_string()).collect(),
            learned_keys: vec![],
            from_procedure: false,
        }
    }
    fn dismiss() -> Vec<String> {
        vec!["Cancel".into(), "Not now".into()]
    }
    fn x_trace(steps: Vec<TraceStep>) -> Trace {
        Trace {
            run_id: "x-20261011-1a2b".into(),
            platform: "x".into(),
            params: vec!["post_text".into()],
            first_lines: vec!["What's happening?".into()],
            steps,
        }
    }

    #[test]
    fn x_post_becomes_a_five_step_procedure_with_the_login_gate_second() {
        let t = Table::seed();
        let tr = x_trace(vec![
            s("open_url", "https://x.com/compose/post", false, &[]),
            s("tap_by_intent", "What's happening?", false, &[]),
            s("type_param", "post_text", false, &[]),
            s("tap_by_intent", "Post", true, &[]),
        ]);
        let (p, pops) = draft(&tr, t.get("x").unwrap(), &dismiss(), &[]).unwrap();
        assert_eq!(p.name, "promote-x-post");
        assert_eq!(p.learned_from, "run:x-20261011-1a2b");
        assert_eq!(p.body, "1. open_url: https://x.com/compose/post\n2. verify_text: 「What's happening?」\n3. tap_by_intent: What's happening?\n4. type_param: post_text\n5. tap_by_intent: Post");
        assert!(pops.is_empty());
    }

    #[test]
    fn a_dismiss_tap_becomes_a_popup_draft_not_a_step() {
        let t = Table::seed();
        let mut tr = x_trace(vec![
            s(
                "open_url",
                "https://www.tiktok.com/tiktokstudio/upload",
                false,
                &[],
            ),
            s(
                "tap_by_intent",
                "Cancel",
                false,
                &["Turn on automatic content checks?"],
            ),
            s("tap_by_intent", "Post", true, &[]),
        ]);
        tr.platform = "tiktok".into();
        tr.first_lines = vec!["Select video".into()];
        let (p, pops) = draft(&tr, t.get("tiktok").unwrap(), &dismiss(), &[]).unwrap();
        assert!(!p.body.contains("Cancel"));
        assert_eq!(pops.len(), 1);
        assert_eq!(
            pops[0].body,
            "应用: Google Chrome\n看到: Turn on automatic content checks?\n点: Cancel"
        );
    }

    #[test]
    fn a_non_dismiss_popup_step_is_flagged_for_the_user() {
        let t = Table::seed();
        let mut tr = x_trace(vec![
            s(
                "open_url",
                "https://www.tiktok.com/tiktokstudio/upload",
                false,
                &[],
            ),
            s("tap_by_intent", "Got it", false, &["New: content checks"]),
            s("tap_by_intent", "Post", true, &[]),
            s("tap_by_intent", "Post now", false, &["Continue to post?"]),
        ]);
        tr.platform = "tiktok".into();
        tr.first_lines = vec!["Select video".into()];
        let (p, _) = draft(&tr, t.get("tiktok").unwrap(), &dismiss(), &[]).unwrap();
        assert!(
            p.body.contains("tap_by_intent: Got it") && p.body.contains("tap_by_intent: Post now")
        );
        assert_eq!(p.maybe_one_off, vec![3, 5]);
    }

    #[test]
    fn only_the_last_fill_of_a_param_is_kept() {
        let t = Table::seed();
        let tr = x_trace(vec![
            s("open_url", "https://x.com/compose/post", false, &[]),
            s("tap_by_intent", "What's happening?", false, &[]),
            s("type_param", "post_text", false, &[]),
            s("pick_file", "image", false, &[]),
            s("tap_by_intent", "What's happening?", false, &[]),
            s("type_param", "post_text", false, &[]),
            s("tap_by_intent", "Post", true, &[]),
        ]);
        let (p, _) = draft(&tr, t.get("x").unwrap(), &dismiss(), &["image".into()]).unwrap();
        assert_eq!(p.body.matches("type_param: post_text").count(), 1);
        assert!(p.body.find("pick_file").unwrap() < p.body.find("type_param").unwrap());
        assert_eq!(p.name, "promote-x-video");
    }

    #[test]
    fn no_login_gate_seen_means_no_draft() {
        let t = Table::seed();
        let mut tr = x_trace(vec![
            s("open_url", "https://x.com/compose/post", false, &[]),
            s("tap_by_intent", "Post", true, &[]),
        ]);
        tr.first_lines = vec!["Home".into()];
        assert!(matches!(
            draft(&tr, t.get("x").unwrap(), &dismiss(), &[]),
            Err(DraftError::LoginGateNotSeen(_))
        ));
    }

    #[test]
    fn unverified_publish_done_is_not_used_as_a_marker() {
        let t = Table::seed();
        let tr = x_trace(vec![
            s("open_url", "https://x.com/compose/post", false, &[]),
            s("tap_by_intent", "Post", true, &[]),
        ]);
        let (p, _) = draft(&tr, t.get("x").unwrap(), &dismiss(), &[]).unwrap();
        assert!(!p.body.contains("完成标志"), "x 的 publish_done 还没核实");
    }

    fn x_basic() -> Vec<TraceStep> {
        vec![
            s("open_url", "https://x.com/compose/post", false, &[]),
            s("tap_by_intent", "Post", true, &[]),
        ]
    }

    #[test]
    fn verified_publish_done_becomes_a_three_space_marker_line() {
        let t = Table::seed();
        let mut p = t.get("x").unwrap().clone();
        p.unverified.retain(|u| u != "publish_done");
        assert_eq!(p.publish_done.as_deref(), Some("Your post was sent"));
        let (d, _) = draft(&x_trace(x_basic()), &p, &dismiss(), &[]).unwrap();
        assert!(
            d.body.ends_with(
                "3. tap_by_intent: Post\n   完成标志: verify_text 「Your post was sent」"
            ),
            "{}",
            d.body
        );
    }

    #[test]
    fn a_marker_with_braces_is_omitted() {
        let t = Table::seed();
        let mut p = t.get("x").unwrap().clone();
        p.unverified.clear();
        p.publish_done = Some("Sent {post_text}".into());
        let (d, _) = draft(&x_trace(x_basic()), &p, &dismiss(), &[]).unwrap();
        assert!(!d.body.contains("完成标志"));
    }

    #[test]
    fn no_publish_step_and_empty_trace_are_errors() {
        let t = Table::seed();
        let tr = x_trace(vec![s(
            "open_url",
            "https://x.com/compose/post",
            false,
            &[],
        )]);
        assert_eq!(
            draft(&tr, t.get("x").unwrap(), &dismiss(), &[]).unwrap_err(),
            DraftError::NoPublish
        );
        let tr = x_trace(vec![]);
        assert_eq!(
            draft(&tr, t.get("x").unwrap(), &dismiss(), &[]).unwrap_err(),
            DraftError::Empty
        );
    }

    #[test]
    fn short_popup_text_goes_to_notes_without_rule_or_step() {
        let t = Table::seed();
        let tr = x_trace(vec![
            s("open_url", "https://x.com/compose/post", false, &[]),
            s("tap_by_intent", "Cancel", false, &["Hi"]),
            s("tap_by_intent", "Post", true, &[]),
        ]);
        let (d, pops) = draft(&tr, t.get("x").unwrap(), &dismiss(), &[]).unwrap();
        assert!(pops.is_empty());
        assert!(!d.body.contains("Cancel"));
        assert_eq!(d.notes.len(), 1);
    }

    fn popup_trace(texts: &[&str]) -> Trace {
        let mut steps = vec![s("open_url", "https://x.com/compose/post", false, &[])];
        for t in texts {
            steps.push(s("tap_by_intent", "Cancel", false, &[t]));
        }
        steps.push(s("tap_by_intent", "Post", true, &[]));
        x_trace(steps)
    }

    fn assert_valid_slug(n: &str) {
        assert!(
            !n.is_empty()
                && n.len() <= 80
                && !n.starts_with('-')
                && !n.ends_with('-')
                && n.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "bad name {n}"
        );
    }

    #[test]
    fn two_distinct_chinese_popups_get_two_valid_distinct_names() {
        let t = Table::seed();
        let tr = popup_trace(&["开启自动内容检查吗", "是否继续发布内容"]);
        let (d, pops) = draft(&tr, t.get("x").unwrap(), &dismiss(), &[]).unwrap();
        assert_eq!(pops.len(), 2);
        assert_ne!(pops[0].name, pops[1].name);
        pops.iter().for_each(|p| assert_valid_slug(&p.name));
        assert!(d.notes.is_empty());
    }

    #[test]
    fn two_english_popups_sharing_a_24_char_prefix_both_survive() {
        let t = Table::seed();
        let tr = popup_trace(&[
            "Turn on automatic content checks for videos?",
            "Turn on automatic content checks for photos?",
        ]);
        let (_, pops) = draft(&tr, t.get("x").unwrap(), &dismiss(), &[]).unwrap();
        assert_eq!(pops.len(), 2);
        assert_ne!(pops[0].name, pops[1].name);
        pops.iter().for_each(|p| assert_valid_slug(&p.name));
    }

    #[test]
    fn the_same_popup_seen_twice_is_one_draft_with_a_stable_name() {
        let t = Table::seed();
        let tr = popup_trace(&["开启自动内容检查吗", "开启自动内容检查吗"]);
        let (_, pops) = draft(&tr, t.get("x").unwrap(), &dismiss(), &[]).unwrap();
        assert_eq!(pops.len(), 1);
        let (_, again) = draft(&tr, t.get("x").unwrap(), &dismiss(), &[]).unwrap();
        assert_eq!(pops[0].name, again[0].name);
    }

    #[test]
    fn an_empty_or_multiline_step_arg_is_an_error() {
        let t = Table::seed();
        for bad in ["", "a\nb"] {
            let tr = x_trace(vec![
                s("open_url", "https://x.com/compose/post", false, &[]),
                s("tap_by_intent", bad, false, &[]),
                s("tap_by_intent", "Post", true, &[]),
            ]);
            assert!(
                matches!(
                    draft(&tr, t.get("x").unwrap(), &dismiss(), &[]),
                    Err(DraftError::BadStepArg(_))
                ),
                "{bad:?}"
            );
        }
    }
}
