### Task 3: 按意图定档，按钮文字只往高了调

**Files:**
- Modify: `crates/dct-brain/src/tier.rs`

**Interfaces:**
- Consumes: Task 1 的 `Tier`。
- Produces（dct 批准流程时给提议，dco 执行前做兜底）：
  - `tier_for_step(action: &str, arg: &str) -> Tier`：**提议**这一步的档位，用户批准时可以改。
  - `tier_for_label(label: &str) -> Tier`：按钮文字**至少**意味着哪一档；说明不了什么时返回 `Tier::Read`（不往上调）。
  - `floor_for_element(label: &str, confirmed: bool) -> Tier`：没确认过的元素至少按 `Content`。
  - `raise(approved: Tier, label: &str, confirmed: bool) -> Tier` = `approved.max(floor_for_element(label, confirmed))`，**永远不会比 `approved` 低**。

设计依据（第 4 节「按意图定档」）：只看按钮字两头都错。「Next / Continue / OK / 下一步」这类字说不清是不是对外，提议时按对外算，交给用户在批准时改；「不保存 / Don't save / Not now」是否定，不算保存；没确认过的元素一律按最高档。

- [ ] **Step 1: 写失败的测试**

在 `tier.rs` 的 `mod tests` 里追加：

```rust
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
        for arg in ["Don't save", "Don’t save", "不保存", "取消发布", "Not now", "Cancel", "暂不上传", "Discard post"] {
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
        assert_eq!(tier_for_label("Don't save"), Tier::Read);
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
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p dct-brain tier`
Expected: 编译失败，`tier_for_step` 等没有定义。

- [ ] **Step 3: 实现**

在 `tier.rs` 的 `impl Tier` 下面加：

```rust
// 词表照抄 dco 设计第 2 节，只许增加，不许删。中文按子串比（「立即发布」），
// 英文按整词比（「Posts」标签页不是「post」）。
const CONTENT_ZH: &[&str] = &["发布", "发表", "发送", "分享", "转发", "上传", "保存", "提交", "确认", "完成"];
const CONTENT_EN: &[&str] = &["post", "publish", "send", "share", "repost", "upload", "save", "submit", "confirm", "done"];
const MONEY_ZH: &[&str] = &["支付", "购买", "下单", "充值", "付款"];
const MONEY_EN: &[&str] = &["pay", "buy", "order", "checkout", "purchase"];
// 发布流程的最后一步常常只写这些。单看字说不清是不是对外，**提议**时按对外算，
// 用户批准时可以改低。运行时的兜底不看这张表，否则又回到「太严」。
const AMBIGUOUS_ZH: &[&str] = &["下一步", "继续", "确定", "好的"];
const AMBIGUOUS_EN: &[&str] = &["next", "continue", "ok", "okay", "yes", "allow"];
// 否定：「不保存」「Don't save」不是保存。中文只认开头，免得「保存不了」被当成否定；
// 英文只认第一个词，免得「Save for later」被当成否定。
const NEGATION_ZH: &[&str] = &["不", "取消", "暂不", "以后再说", "稍后", "放弃", "别"];
const NEGATION_EN: &[&str] = &["don't", "dont", "not", "cancel", "no", "skip", "later", "discard"];

fn words(label: &str) -> Vec<String> {
    label
        .replace('’', "'")
        .to_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

fn hit(label: &str, zh: &[&str], en: &[&str]) -> bool {
    zh.iter().any(|z| label.contains(z)) || words(label).iter().any(|w| en.contains(&w.as_str()))
}

fn negated(label: &str) -> bool {
    let t = label.trim_start();
    NEGATION_ZH.iter().any(|n| t.starts_with(n))
        || words(t).first().is_some_and(|w| NEGATION_EN.contains(&w.as_str()))
}

/// 按钮上的字**至少**意味着哪一档。说明不了什么（空、否定、普通词）就是 `Read`，
/// 也就是不往上调。
pub fn tier_for_label(label: &str) -> Tier {
    let label = label.trim();
    if label.is_empty() || negated(label) {
        return Tier::Read;
    }
    if hit(label, MONEY_ZH, MONEY_EN) {
        Tier::Money
    } else if hit(label, CONTENT_ZH, CONTENT_EN) {
        Tier::Content
    } else {
        Tier::Read
    }
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
        "open_app" | "navigate_by_intent" | "scroll" | "back" | "type_param" | "set_checked" => {
            Tier::SelfOnly
        }
        // 选文件就是把本机文件交出去（dcv 会话提醒的）。
        "pick_file" => Tier::Content,
        "tap_by_intent" | "confirm_dialog" => {
            let a = arg.trim();
            if negated(a) {
                return Tier::SelfOnly;
            }
            let default = if action == "confirm_dialog" || hit(a, AMBIGUOUS_ZH, AMBIGUOUS_EN) {
                Tier::Content
            } else {
                Tier::SelfOnly
            };
            tier_for_label(a).max(default)
        }
        // dcv 写入时就拒绝词表外的动作；万一漏进来，按对外算。
        _ => Tier::Content,
    }
}
```

- [ ] **Step 4: 跑测试，确认通过**

Run: `cargo test -p dct-brain`
Expected: 全部 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/dct-brain/src/tier.rs
git commit -m "feat(brain): propose tiers from each step's intent; button text only raises"
```

---

