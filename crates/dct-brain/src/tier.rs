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
    /// 不能撤回的删除和授权：跟 `Money` 一样每次都要用户本人签，任何规则都不能
    /// 放行（用户 2026-10-01 定）。只是新增的一档，旧的批准记录照旧有效。
    #[serde(rename = "critical")]
    Critical,
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
            Tier::Critical => "critical",
            Tier::Money => "money",
        }
    }

    /// dcv 里用户写的规则能不能放行这一档。只有比 `Critical` 低的才能。
    pub fn rule_can_waive(self) -> bool {
        self < Tier::Critical
    }

    /// `None` 表示不要票（只读）。物理动作全自动是用户定的（设计第 4 节）。
    pub fn required_signer(self) -> Option<SignerRole> {
        match self {
            Tier::Read => None,
            Tier::SelfOnly | Tier::Physical => Some(SignerRole::Auto),
            Tier::Content | Tier::Critical | Tier::Money => Some(SignerRole::User),
        }
    }
}

// 词表照抄 dco 设计第 2 节，只许增加，不许删。中文按子串比（「立即发布」），
// 英文按整词比（「Posts」标签页不是「post」）。
const CONTENT_ZH: &[&str] = &["发布", "发表", "发送", "分享", "转发", "上传", "保存", "提交", "确认", "完成"];
const CONTENT_EN: &[&str] = &["post", "publish", "send", "share", "repost", "upload", "save", "submit", "confirm", "done"];
const MONEY_ZH: &[&str] = &["支付", "购买", "下单", "充值", "付款", "付费", "结算", "订阅"];
const MONEY_EN: &[&str] = &["pay", "buy", "order", "checkout", "purchase", "payment", "subscribe"];
// 发布流程的最后一步常常只写这些。单看字说不清是不是对外，**提议**时按对外算，
// 用户批准时可以改低。运行时的兜底不看这张表，否则又回到「太严」。
const AMBIGUOUS_ZH: &[&str] = &["下一步", "继续", "确定", "好的"];
const AMBIGUOUS_EN: &[&str] = &["next", "continue", "ok", "okay", "yes", "allow"];
// 否定：「不保存」「Don't save」不是保存。中文只认明确的否定词开头（不是裸「不」），
// 免得「不限量购买」「保存不了」这类词被当成否定；英文只认第一个词，免得
// 「Save for later」被当成否定。无论怎么判否定，含钱词的子句永远不会被否定压低
// （见 `is_money_clause` / `effectively_negated`）。
const NEGATION_ZH: &[&str] = &[
    "不要", "不用", "不保存", "不发", "不分享", "不上传", "不转发", "不提交", "不确认", "不完成", "不了", "不再", "不需要", "不同意", "不允许", "取消", "暂不", "以后再说", "稍后", "放弃", "别",
];
const NEGATION_EN: &[&str] = &["don't", "dont", "not", "cancel", "no", "skip", "later", "discard"];

// 2026-10-01 盲测（dc-octo 的 410 条标签）之后补的几类，同样只许增加。英文可以
// 是短语（「delete account」），按整词连着比。
// 不能撤回的删除和授权：`Critical`，任何规则都不能放行。「注销」单独不算——
// Windows 上它是「退出登录」。
const CRITICAL_ZH: &[&str] = &[
    "删除账号", "删除帐号", "删除账户", "注销账号", "注销帐号", "注销账户", "抹掉", "抹除", "清空废纸篓", "恢复出厂", "格式化",
    "授权", "信任此", "信任这", "信任该", "允许",
];
const CRITICAL_EN: &[&str] = &[
    "delete account", "delete my account", "erase", "empty trash", "factory reset", "format disk", "format drive",
    "format card", "wipe", "authorize", "authorise", "trust", "allow", "grant",
];
// 删除、安装、对外联络、登录：`Content`。
const MORE_CONTENT_ZH: &[&str] = &[
    "删除", "清空", "卸载", "移除", "重置", "安装", "立即下载", "仍要打开", "启用宏", "回复", "邀请", "投币", "登录",
];
const MORE_CONTENT_EN: &[&str] = &[
    "delete", "remove", "uninstall", "reset", "install", "run anyway", "open anyway", "enable macros", "reply",
    "invite", "forward", "sign in", "log in", "login",
];
// 丢掉东西的「不」：不保存、Don't save、放弃、Discard。按删除算（`Content`），
// 不让否定把它压低。
const DISCARD_ZH: &[&str] = &["不保存", "放弃"];
const DISCARD_EN: &[&str] = &["don't save", "dont save", "do not save", "discard"];
const MORE_MONEY_ZH: &[&str] = &["转账", "还款", "开通", "打赏"];
const MORE_MONEY_EN: &[&str] = &["top up", "place bid", "reload balance", "reload card", "reload wallet"];
/// 标签里有这些记号、又有数字，就是一个价钱：`Money`。
const CURRENCY_MARKS: &[&str] = &["¥", "￥", "$", "€", "£", "💎", "金币", "钻石"];

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
}

/// 比词之前先把字摆正：全角变半角、去掉零宽字符、弯引号变直、去掉末尾的
/// 省略号、去掉两个汉字之间的空格（OCR 常把「支付」认成「支 付」）、把粘在
/// 一起的英文按大小写拆开（「SendNow」）、最后转小写。
fn normalize(label: &str) -> String {
    let mut chars: Vec<char> = label
        .chars()
        .filter_map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0),
            '\u{3000}' => Some(' '),
            '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' => None,
            '\u{2018}' | '\u{2019}' => Some('\''),
            c => Some(c),
        })
        .collect();
    loop {
        while chars.last().is_some_and(|c| c.is_whitespace()) {
            chars.pop();
        }
        if chars.last() == Some(&'…') {
            chars.pop();
        } else if chars.ends_with(&['.', '.', '.']) {
            chars.truncate(chars.len() - 3);
        } else {
            break;
        }
    }
    let cjk_beside = |i: usize| {
        let prev = chars[..i].iter().rev().find(|c| !c.is_whitespace());
        let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
        prev.is_some_and(|&c| is_cjk(c)) && next.is_some_and(|&c| is_cjk(c))
    };
    let mut out = String::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        if c.is_whitespace() && cjk_beside(i) {
            continue;
        }
        if i > 0 && c.is_ascii_uppercase() && chars[i - 1].is_ascii_lowercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out.trim().to_lowercase()
}

fn words(label: &str) -> Vec<String> {
    label
        .replace('\u{2019}', "'")
        .to_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// 中文按子串比；英文按整词比，短语就按连着的几个整词比。
fn hit(label: &str, zh: &[&str], en: &[&str]) -> bool {
    if zh.iter().any(|z| label.contains(z)) {
        return true;
    }
    let joined = format!(" {} ", words(label).join(" "));
    en.iter().any(|e| joined.contains(&format!(" {e} ")))
}

fn is_price(clause: &str) -> bool {
    CURRENCY_MARKS.iter().any(|m| clause.contains(m)) && clause.chars().any(|c| c.is_ascii_digit())
}

fn is_money_clause(clause: &str) -> bool {
    hit(clause, MONEY_ZH, MONEY_EN) || hit(clause, MORE_MONEY_ZH, MORE_MONEY_EN) || is_price(clause)
}

fn is_discard(clause: &str) -> bool {
    hit(clause, DISCARD_ZH, DISCARD_EN)
}

fn negated(label: &str) -> bool {
    let t = label.trim_start();
    NEGATION_ZH.iter().any(|n| t.starts_with(n))
        || words(t).first().is_some_and(|w| NEGATION_EN.contains(&w.as_str()))
}

/// A clause that names money can never be hidden by a negation word — see
/// design ruling: 「不限量购买」/"No-fee checkout" must still read as `Money`.
/// 丢掉东西的说法（「不保存」「Don't save」）同理：它不是拒绝，是删除。
fn effectively_negated(clause: &str) -> bool {
    !is_money_clause(clause) && !is_discard(clause) && negated(clause)
}

fn split_clauses(label: &str) -> Vec<&str> {
    label
        .split(['，', ',', ';', '；', '、', '。', '.', '!', '！', '?', '？'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect()
}

fn score_clause(clause: &str) -> Tier {
    if is_money_clause(clause) {
        Tier::Money
    } else if hit(clause, CRITICAL_ZH, CRITICAL_EN) {
        Tier::Critical
    } else if hit(clause, CONTENT_ZH, CONTENT_EN) || hit(clause, MORE_CONTENT_ZH, MORE_CONTENT_EN) || is_discard(clause) {
        Tier::Content
    } else {
        Tier::Read
    }
}

/// 按钮上的字**至少**意味着哪一档。分句判断否定，避免「放弃草稿，直接发布」这类
/// 多意图的标签被前半句的否定完全压低。每句开头的否定说不清本意，只有非否定句
/// 才能撑起来。
pub fn tier_for_label(label: &str) -> Tier {
    let label = normalize(label);
    let label = label.as_str();
    if label.is_empty() {
        return Tier::Read;
    }

    let clauses = split_clauses(label);
    if clauses.is_empty() {
        return Tier::Read;
    }

    clauses
        .iter()
        .filter_map(|clause| {
            if effectively_negated(clause) {
                None
            } else {
                Some(score_clause(clause))
            }
        })
        .max()
        .unwrap_or(Tier::Read)
}

/// 没确认过的元素（模型第一次认出来的图标、按钮）至少按对外算（设计第 4 节）。
pub fn floor_for_element(label: &str, confirmed: bool) -> Tier {
    let by_text = tier_for_label(label);
    if confirmed {
        by_text
    } else {
        by_text.max(Tier::Content)
    }
}

/// 兜底：只往高了调，永远不比 `approved` 低。
pub fn raise(approved: Tier, label: &str, confirmed: bool) -> Tier {
    approved.max(floor_for_element(label, confirmed))
}

/// 批准流程时给每一步**提议**的档位；用户确认或改过之后才生效。
pub fn tier_for_step(action: &str, arg: &str) -> Tier {
    match action {
        "verify_text" | "wait" | "read_value" => Tier::Read,
        // open_url 只是在这次运行的浏览器窗口里打开一个网址，不往外送任何东西。
        "open_app" | "open_url" | "navigate_by_intent" | "scroll" | "back" | "type_param"
        | "set_checked" => Tier::SelfOnly,
        // 选文件就是把本机文件交出去（dcv 会话提醒的）。
        "pick_file" => Tier::Content,
        "tap_by_intent" | "confirm_dialog" => {
            let a = normalize(arg);
            let a = a.as_str();
            let clauses = split_clauses(a);

            // Only return SelfOnly if EVERY clause is negated (a money clause is
            // never "effectively" negated, so its presence rules this out).
            if !clauses.is_empty() && clauses.iter().all(|c| effectively_negated(c)) {
                return Tier::SelfOnly;
            }

            // Compute label tier using per-clause logic.
            let label_tier = tier_for_label(a);

            // Determine default: ambiguous words and confirm_dialog apply to non-negated clauses only.
            let default = if action == "confirm_dialog" {
                Tier::Content
            } else {
                // For tap_by_intent, check if any non-negated clause has ambiguous words.
                let has_ambiguous_in_nonnegated = clauses
                    .iter()
                    .any(|c| !effectively_negated(c) && hit(c, AMBIGUOUS_ZH, AMBIGUOUS_EN));
                if has_ambiguous_in_nonnegated {
                    Tier::Content
                } else {
                    Tier::SelfOnly
                }
            };

            label_tier.max(default)
        }
        // dcv 写入时就拒绝词表外的动作；万一漏进来，按对外算。
        _ => Tier::Content,
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
        assert!(Tier::Content < Tier::Critical);
        assert!(Tier::Critical < Tier::Money);
    }

    #[test]
    fn only_tiers_below_critical_can_be_waived_by_a_rule() {
        for t in [Tier::Read, Tier::SelfOnly, Tier::Physical, Tier::Content] {
            assert!(t.rule_can_waive(), "{t:?}");
        }
        for t in [Tier::Critical, Tier::Money] {
            assert!(!t.rule_can_waive(), "{t:?}");
        }
    }

    #[test]
    fn names_match_dcv_and_dco() {
        for (t, n) in [
            (Tier::Read, "read"),
            (Tier::SelfOnly, "self"),
            (Tier::Physical, "physical"),
            (Tier::Content, "content"),
            (Tier::Critical, "critical"),
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
        assert_eq!(Tier::Critical.required_signer(), Some(SignerRole::User));
        assert_eq!(Tier::Money.required_signer(), Some(SignerRole::User));
    }

    #[test]
    fn reading_steps_are_read() {
        for a in ["verify_text", "wait", "read_value"] {
            assert_eq!(tier_for_step(a, "粉丝"), Tier::Read, "{a}");
        }
    }

    #[test]
    fn moving_around_and_filling_in_only_affect_yourself() {
        for (a, arg) in [
            ("open_app", "TikTok"),
            ("open_url", "https://studio.youtube.com"),
            ("navigate_by_intent", "我的 → 编辑资料 → 简介"),
            ("scroll", "down"),
            ("back", ""),
            ("type_param", "bio"),
            ("set_checked", "同时发到 Story = off"),
            ("tap_by_intent", "编辑资料"),
            // 「Posts」标签页不是「post」：英文按整词比。
            ("tap_by_intent", "Posts"),
        ] {
            assert_eq!(tier_for_step(a, arg), Tier::SelfOnly, "{a} {arg}");
        }
    }

    #[test]
    fn outward_words_and_handing_over_files_are_content() {
        for (a, arg) in [
            ("tap_by_intent", "保存"),
            ("tap_by_intent", "立即发布"),
            ("tap_by_intent", "Post"),
            ("tap_by_intent", "Save"),
            ("pick_file", "photo"),
        ] {
            assert_eq!(tier_for_step(a, arg), Tier::Content, "{a} {arg}");
        }
    }

    #[test]
    fn ambiguous_last_buttons_are_proposed_as_content() {
        // 发布流程的最后一步可能只写着这些——提议时宁可严，用户批准时可以改低。
        for arg in ["Next", "Continue", "OK", "下一步", "继续", "确定"] {
            assert_eq!(tier_for_step("tap_by_intent", arg), Tier::Content, "{arg}");
        }
        assert_eq!(tier_for_step("confirm_dialog", "OK"), Tier::Content);
    }

    #[test]
    fn negations_are_not_the_thing_they_negate() {
        for arg in ["取消发布", "Not now", "Cancel", "暂不上传", "不允许", "Don't Allow", "Keep subscription", "Skip"] {
            assert_eq!(tier_for_step("tap_by_intent", arg), Tier::SelfOnly, "{arg}");
        }
        assert_eq!(tier_for_step("confirm_dialog", "Cancel"), Tier::SelfOnly);
    }

    #[test]
    fn money_words_are_money() {
        assert_eq!(tier_for_step("tap_by_intent", "立即支付"), Tier::Money);
        assert_eq!(tier_for_step("tap_by_intent", "Checkout"), Tier::Money);
    }

    #[test]
    fn unknown_actions_are_treated_as_outward() {
        assert_eq!(tier_for_step("delete_everything", ""), Tier::Content);
    }

    #[test]
    fn label_floor_only_speaks_when_the_text_says_something() {
        assert_eq!(tier_for_label("Save"), Tier::Content);
        assert_eq!(tier_for_label("Pay now"), Tier::Money);
        assert_eq!(tier_for_label(""), Tier::Read);
        assert_eq!(tier_for_label("Posts"), Tier::Read);
        assert_eq!(tier_for_label("Cancel"), Tier::Read);
        // 运行时的兜底不管「Next」：那是批准流程时的事，运行时再拦会重回「太严」。
        assert_eq!(tier_for_label("Next"), Tier::Read);
    }

    #[test]
    fn raising_never_lowers() {
        assert_eq!(raise(Tier::SelfOnly, "Post", true), Tier::Content);
        assert_eq!(raise(Tier::Content, "Cancel", true), Tier::Content);
        assert_eq!(raise(Tier::Money, "", true), Tier::Money);
        assert_eq!(raise(Tier::SelfOnly, "编辑资料", true), Tier::SelfOnly);
    }

    #[test]
    fn unconfirmed_elements_count_as_outward_at_least() {
        assert_eq!(raise(Tier::SelfOnly, "Next", false), Tier::Content);
        assert_eq!(raise(Tier::SelfOnly, "", false), Tier::Content);
        assert_eq!(raise(Tier::SelfOnly, "支付", false), Tier::Money);
        assert_eq!(floor_for_element("编辑资料", true), Tier::Read);
    }

    #[test]
    fn multi_clause_labels_judge_negation_per_clause() {
        // Negated first clause doesn't suppress content in later clause.
        assert_eq!(tier_for_step("tap_by_intent", "放弃草稿，直接发布"), Tier::Content);
        assert_eq!(tier_for_step("tap_by_intent", "No, post anyway"), Tier::Content);
        assert_eq!(tier_for_label("放弃草稿，直接发布"), Tier::Content);
        assert_eq!(tier_for_label("No, post anyway"), Tier::Content);
        assert_eq!(raise(Tier::SelfOnly, "No, post anyway", true), Tier::Content);
        // All clauses negated still counts as negation.
        assert_eq!(tier_for_step("tap_by_intent", "Not now, maybe later"), Tier::SelfOnly);
    }

    #[test]
    fn throwing_work_away_is_not_a_decline() {
        // 「不保存」「Don't save」「放弃」会丢掉东西：按删除算，不按否定压低。
        for arg in ["Don't save", "Don\u{2019}t save", "Do not save", "不保存", "放弃修改", "Discard post", "Discard"] {
            assert_eq!(tier_for_label(arg), Tier::Content, "{arg}");
            assert_eq!(tier_for_step("tap_by_intent", arg), Tier::Content, "{arg}");
        }
    }

    #[test]
    fn deleting_installing_and_reaching_out_are_content() {
        for arg in [
            "删除", "删除照片", "清空", "卸载", "移除", "重置", "Delete", "Remove", "Uninstall", "Reset",
            "安装", "立即下载", "仍要打开", "启用宏", "Install", "Run anyway", "Open anyway", "Enable Macros",
            "回复", "邀请好友", "投币", "Reply", "Invite", "Forward",
            "登录", "Sign in", "Log in",
        ] {
            assert_eq!(tier_for_label(arg), Tier::Content, "{arg}");
            assert_eq!(tier_for_step("tap_by_intent", arg), Tier::Content, "{arg}");
        }
    }

    #[test]
    fn irreversible_deletes_and_grants_are_critical() {
        for arg in [
            "删除账号", "注销账号", "抹掉", "抹掉所有内容和设置", "清空废纸篓", "恢复出厂设置", "格式化",
            "Delete account", "Delete Account", "Erase", "Erase All Content and Settings", "Empty Trash",
            "Factory reset", "Format disk", "Wipe",
            "授权", "授权登录", "同意授权", "信任此设备", "允许", "允许访问", "Authorize", "Trust", "Trust this device",
            "Allow", "Allow access", "Grant access",
        ] {
            assert_eq!(tier_for_label(arg), Tier::Critical, "{arg}");
            assert_eq!(tier_for_step("tap_by_intent", arg), Tier::Critical, "{arg}");
        }
        // 拒绝授权就是拒绝：照常压低。
        for arg in ["不允许", "Don't Allow", "Not now"] {
            assert_eq!(tier_for_label(arg), Tier::Read, "{arg}");
        }
    }

    #[test]
    fn more_money_words_and_bare_prices_are_money() {
        for arg in [
            "转账", "立即还款", "开通会员", "打赏", "Top up", "Place bid", "Reload balance",
            "$99.99", "¥ 68", "￥6", "€5", "£3.50", "💎 500", "500 金币", "100钻石", "US$ 4.99",
        ] {
            assert_eq!(tier_for_label(arg), Tier::Money, "{arg}");
            assert_eq!(tier_for_step("tap_by_intent", arg), Tier::Money, "{arg}");
        }
        // 只有币种记号、没有数字的不算价钱。
        assert_eq!(tier_for_label("$"), Tier::Read);
        assert_eq!(tier_for_label("金币商城"), Tier::Read);
    }

    #[test]
    fn spacing_width_case_and_zero_width_do_not_hide_words() {
        for (arg, want) in [
            ("立即支 付", Tier::Money),
            ("发 送", Tier::Content),
            ("SendNow", Tier::Content),
            ("PayNow", Tier::Money),
            ("ＰＡＹ", Tier::Money),
            ("ｄｅｌｅｔｅ ａｃｃｏｕｎｔ", Tier::Critical),
            ("删\u{200B}除账号", Tier::Critical),
            ("DELETE ACCOUNT", Tier::Critical),
            ("删除账号…", Tier::Critical),
            ("Delete account...", Tier::Critical),
        ] {
            assert_eq!(tier_for_label(arg), want, "{arg:?}");
        }
        // 整词照旧：「Posts」标签页、「Payments」历史不因为去空格变成别的词。
        assert_eq!(tier_for_label("Posts"), Tier::Read);
        assert_eq!(tier_for_label("Settings"), Tier::Read);
    }

    #[test]
    fn money_clauses_are_never_suppressed_by_negation() {
        // 「不限量购买」: bare 「不」 is not one of the explicit negation words,
        // and even if it were, a money clause can never be negated away.
        assert_eq!(tier_for_label("不限量购买"), Tier::Money);
        assert_eq!(tier_for_step("tap_by_intent", "不限量购买"), Tier::Money);

        // Hyphenated English negation token ("no") still splits out, but the
        // clause names money so it stays Money regardless.
        assert_eq!(tier_for_label("No-fee checkout"), Tier::Money);
        for arg in ["付费解锁", "去结算", "Subscribe", "Payment"] {
            assert_eq!(tier_for_label(arg), Tier::Money, "{arg}");
        }
        for arg in ["不分享", "不上传", "不转发", "不提交", "不确认"] {
            assert_eq!(tier_for_step("tap_by_intent", arg), Tier::SelfOnly, "{arg}");
        }
        assert_eq!(tier_for_step("tap_by_intent", "No-fee checkout"), Tier::Money);

        // An explicit negation word paired with a money word: money wins.
        assert_eq!(tier_for_label("不要付款"), Tier::Money);
        assert_eq!(tier_for_step("tap_by_intent", "不要付款"), Tier::Money);
    }
}
