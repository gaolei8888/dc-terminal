//! 停下的规矩：登录 / 验证码 / 账号警告画面、禁点按钮、步数 / 时间 / 没变化上限。
//! 全是代码，探索循环在问任何模型之前先过这里。词表命中一律「宁可多停」。
use crate::app_profile::AppProfile;
use crate::hands::El;
use dct_brain::tier::{tier_for_label, Tier};

/// 带上命中的那行字，给用户看。
#[derive(Debug, Clone, PartialEq)]
pub enum Halt {
    Login(String),
    Captcha(String),
    Warning(String),
}

const LOGIN: &[&str] = &[
    "sign in",
    "log in",
    "login",
    "登录",
    "扫码",
    "phone, email, or username",
    "forgot password",
    "忘记密码",
    "create account",
    "二维码",
    "qr code",
    "scan the code",
];
const CAPTCHA: &[&str] = &[
    "captcha",
    "verify you are human",
    "not a robot",
    "验证码",
    "安全验证",
    "滑块",
    "drag the slider",
    "enter the code",
    "短信",
    "two-factor",
    "2fa",
    "confirm it's you",
    "确认是你本人",
    "手机号验证",
    "手机验证",
    "phone verification",
    "verification code",
    "security check",
    "are you human",
    // normalize 已把弯引号变直，所以只写直引号一种
    "verify you're human",
];
const WARNING: &[&str] = &[
    "unusual activity",
    "异常",
    "locked",
    "suspended",
    "restricted",
    "操作频繁",
    "too many requests",
    "内容被限制",
    "violat",
    "account disabled",
    "账号被封",
];
/// 不看档案的通用禁点词：不可撤回的删除。
/// brief 的测试要求「Delete」也拦，但 dct-brain 把单独的「删除」算 Content（不到 Critical），
/// 所以这里补进禁点词。关注、订阅、推广一类跟具体应用有关的词不在代码里：
/// 它们是每个档案自己的数据（`AppProfile::never_tap`）。
const NEVER_ANYWHERE: &[&str] = &["delete", "删除"];

/// 和 dct-brain 的档位同一套规整（全角、零宽、弯引号、汉字间空格、小写），不另写一份。
pub(crate) fn fold(s: &str) -> String {
    dct_brain::tier::normalize(s)
}

fn first_hit(elements: &[El], words: &[&str]) -> Option<String> {
    elements.iter().find_map(|e| {
        let f = fold(&e.text);
        words.iter().any(|w| f.contains(w)).then(|| e.text.clone())
    })
}

/// 整行包含词表里任一项就停：登录 → 验证码 → 账号警告。宁可多停。
pub fn screen_halt(elements: &[El]) -> Option<Halt> {
    first_hit(elements, LOGIN)
        .map(Halt::Login)
        .or_else(|| first_hit(elements, CAPTCHA).map(Halt::Captcha))
        .or_else(|| first_hit(elements, WARNING).map(Halt::Warning))
}

/// `label` 里有 `words` 中的任一项（两边都先规整；空词不算）。
fn hits_never(label: &str, words: &[String]) -> bool {
    let f = fold(label);
    words
        .iter()
        .map(|w| fold(w))
        .any(|w| !w.is_empty() && f.contains(&w))
}

/// 这个元素能不能进选项：档案自己的禁点词、通用禁点词、tier >= Critical 一律不行。
pub fn tappable(label: &str, p: &AppProfile) -> bool {
    !hits_never(label, &p.never_tap) && tappable_with(label, &[])
}

/// 通用那一半，再加调用方给的禁点词（不认识档案的一侧——比如模型回复的读取——只拿到词，不拿档案）。
fn tappable_with(label: &str, never: &[String]) -> bool {
    let f = fold(label);
    if NEVER_ANYWHERE.iter().any(|w| f.contains(w)) || hits_never(label, never) {
        return false;
    }
    tier_for_label(label) < Tier::Critical
}

/// 图标名字最多几个字：按钮名都很短，长的多半是一句话或别的东西。
pub const MAX_NAME_CHARS: usize = 40;

/// 名字里能有的字：字母、数字（含中文）、空格和几个常见标点。控制字符、换行、
/// 看不见的格式字符、改显示顺序的字符都不是字母数字，一律不收。
fn name_char_ok(c: char) -> bool {
    c.is_alphanumeric() || c == ' ' || "-_'’.&+/()!?#@,:·、".contains(c)
}

/// 名字的样子：不空、不太长、只有正常的字，而且不是通用禁点词 / 关键档位。
/// 大模型的回答在读进来时就先过这一道：调用方把档案的禁点词当一串字给进来，
/// 这里不认识档案本身；`check_icon_name` 在这之上再看发布词。
pub fn icon_name_shape(name: &str, never: &[String]) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("名字是空的".into());
    }
    if n.chars().count() > MAX_NAME_CHARS {
        return Err(format!("名字超过 {MAX_NAME_CHARS} 个字"));
    }
    if !n.chars().all(name_char_ok) {
        return Err("名字里有控制字符或看不见的字符".into());
    }
    if !tappable_with(n, never) {
        return Err("名字是不能点的按钮（禁点词、删除、付费一类）".into());
    }
    Ok(())
}

/// 图标名字查完的结果。
#[derive(Debug, Clone, PartialEq)]
pub enum IconName {
    Ok,
    /// 名字是平台的发布词（或含发布词）：dco 会按这个字去点，等于点发布——要先问用户。
    Publish,
    Refused(String),
}

/// `w` 在 `hay` 里作为整词出现（两边不是 ASCII 字母数字；中文没有词边界，出现就算）。
fn contains_word(hay: &str, w: &str) -> bool {
    let edge = |c: Option<char>| !c.is_some_and(|c| c.is_ascii_alphanumeric());
    hay.match_indices(w)
        .any(|(i, _)| edge(hay[..i].chars().next_back()) && edge(hay[i + w.len()..].chars().next()))
}

/// 大模型或用户给没字图标起的名字。dco 会按这个名字去点画面上的字，所以名字和画面上的字
/// 守同样的规矩：禁点词（通用 + 平台）、档位、长度和字符不合格的拒；含平台发布词的当发布
/// （先问用户；发布要用户自己点的平台交给用户）。
pub fn check_icon_name(name: &str, p: &AppProfile) -> IconName {
    if let Err(why) = icon_name_shape(name, &p.never_tap) {
        return IconName::Refused(why);
    }
    if !tappable(name, p) {
        return IconName::Refused("名字是这个平台不能点的按钮".into());
    }
    let f = fold(name.trim());
    let publish = p
        .publish_words
        .iter()
        .map(|w| fold(w))
        .any(|w| !w.is_empty() && contains_word(&f, &w));
    if publish {
        IconName::Publish
    } else {
        IconName::Ok
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub max_steps: u32,
    pub max_ms: u64,
    pub max_unchanged: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            max_steps: 40,
            max_ms: 600_000,
            max_unchanged: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Over {
    Steps,
    Time,
    Unchanged,
}

pub struct Meter {
    steps: u32,
    started_ms: u64,
    unchanged_in_a_row: u32,
}

impl Meter {
    pub fn new(now_ms: u64) -> Meter {
        Meter {
            steps: 0,
            started_ms: now_ms,
            unchanged_in_a_row: 0,
        }
    }

    /// 每执行完一个动作记一次；changed = 动作前后 scene_hash 不同。
    pub fn record(&mut self, changed: bool) {
        self.steps += 1;
        self.unchanged_in_a_row = if changed {
            0
        } else {
            self.unchanged_in_a_row + 1
        };
    }

    pub fn over(&self, b: &Budget, now_ms: u64) -> Option<Over> {
        if self.steps >= b.max_steps {
            Some(Over::Steps)
        } else if now_ms.saturating_sub(self.started_ms) > b.max_ms {
            Some(Over::Time)
        } else if self.unchanged_in_a_row >= b.max_unchanged {
            Some(Over::Unchanged)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_profile::Table;
    use crate::hands::{El, Frac};

    fn el(t: &str) -> El {
        El {
            id: "e1".into(),
            text: t.into(),
            role: "text".into(),
            frac: Frac {
                x: 0.5,
                y: 0.5,
                w: 0.1,
                h: 0.03,
            },
            source: "ocr".into(),
        }
    }

    #[test]
    fn login_pages_stop() {
        for t in [
            "Sign in to X",
            "Log in",
            "登录",
            "Phone, email, or username",
            "扫码登录",
            "二维码",
            "QR code",
            "Scan the code",
        ] {
            assert!(matches!(screen_halt(&[el(t)]), Some(Halt::Login(_))), "{t}");
        }
    }

    #[test]
    fn captcha_mid_run_is_a_stop() {
        for t in [
            "Verify you are human",
            "Prove you're not a robot",
            "验证码",
            "请完成安全验证",
            "Enter the code we sent",
            "Drag the slider",
            "手机验证",
            "Phone verification",
            "Verification code",
            "Security check",
            "Are you human?",
            "Verify you're human",
            "Verify you\u{2019}re human",
        ] {
            assert!(
                matches!(screen_halt(&[el("Post"), el(t)]), Some(Halt::Captcha(_))),
                "{t}"
            );
        }
    }

    #[test]
    fn account_warnings_stop() {
        for t in [
            "Your account has been locked",
            "unusual activity",
            "账号异常",
            "操作频繁",
            "Your account is restricted",
            "Account disabled",
            "账号被封",
        ] {
            assert!(
                matches!(screen_halt(&[el(t)]), Some(Halt::Warning(_))),
                "{t}"
            );
        }
    }

    #[test]
    fn a_normal_composer_does_not_stop() {
        assert_eq!(
            screen_halt(&[
                el("What's happening?"),
                el("Post"),
                el("Everyone can reply")
            ]),
            None
        );
    }

    #[test]
    fn halt_carries_the_matching_line() {
        assert_eq!(
            screen_halt(&[el("Post"), el("请完成安全验证")]),
            Some(Halt::Captcha("请完成安全验证".into()))
        );
    }

    #[test]
    fn follow_subscribe_boost_and_platform_never_tap_are_not_options() {
        let t = Table::seed();
        let x = t.get("x").unwrap();
        for l in [
            "Follow",
            "Subscribe",
            "Boost post",
            "Promote",
            "关注",
            "订阅",
            "推广",
            "Get verified",
        ] {
            assert!(!tappable(l, x), "{l}");
        }
        assert!(tappable("Post", x));
        assert!(tappable("Cancel", x));
        assert!(!tappable("Pay $8", x), "动钱的按 dct-brain 的档位拦");
        assert!(!tappable("Delete", x), "不可撤回的也拦");
    }

    /// 拆出去以前写死在 gate.rs 里的那批词。现在是每个档案自己的数据，
    /// 内置档案要逐行带着它们，行为才和以前一样。
    const FORMER_BUILTIN_NEVER: [&str; 12] = [
        "follow",
        "subscribe",
        "boost",
        "promote",
        "关注",
        "订阅",
        "推广",
        "付费",
        "get verified",
        "premium",
        "upgrade",
        "升级",
    ];

    #[test]
    fn every_builtin_profile_still_blocks_the_former_builtin_never_words() {
        let t = Table::seed();
        for p in &t.rows {
            for w in FORMER_BUILTIN_NEVER {
                assert!(!tappable(w, p), "{}：{w}", p.name);
                assert!(!tappable(&w.to_uppercase(), p), "{}：{w} 大写", p.name);
                assert!(
                    !tappable(&format!("  {w} now"), p),
                    "{}：{w} 夹在话里",
                    p.name
                );
                assert!(
                    matches!(check_icon_name(w, p), IconName::Refused(_)),
                    "{}：图标名 {w}",
                    p.name
                );
            }
        }
    }

    fn bare_profile() -> crate::app_profile::AppProfile {
        let t = Table::seed()
            .with_override(
                "[[platform]]\nname = \"bare\"\ndomain = \"b.example\"\nentry_url = \"https://b.example/\"\nbrowser = \"b\"\nvault = \"v\"\npublish_words = [\"Go\"]\nnever_tap = []\n",
            )
            .unwrap();
        t.get("bare").unwrap().clone()
    }

    #[test]
    fn a_profile_with_no_never_words_blocks_nothing_beyond_the_generic_rules() {
        let b = bare_profile();
        // 档案没写词，gate 自己就不拦：剩下被拦的只是 dct-brain 的档位认得的（动钱的）。
        for w in FORMER_BUILTIN_NEVER {
            assert_eq!(
                tappable(w, &b),
                tier_for_label(w) < Tier::Critical,
                "{w}：除了档位，没写禁点词的档案不该多拦"
            );
        }
        for w in ["follow", "boost", "promote", "关注", "推广", "get verified"] {
            assert!(tappable(w, &b), "{w}");
            assert_eq!(check_icon_name(w, &b), IconName::Ok, "{w}");
        }
        // 通用的两条仍在：不可撤回的删除、dct-brain 的档位。
        assert!(!tappable("Delete", &b));
        assert!(!tappable("删除", &b));
        assert!(!tappable("Pay $8", &b));
    }

    #[test]
    fn platform_never_tap_is_enforced_on_its_own() {
        let t = Table::seed()
            .with_override(
                "[[platform]]\nname = \"bsky\"\ndomain = \"bsky.app\"\nentry_url = \"https://bsky.app/\"\nbrowser = \"b\"\nvault = \"v\"\npublish_words = [\"Post\"]\nnever_tap = [\"Sponsor\"]\n",
            )
            .unwrap();
        let bsky = t.get("bsky").unwrap();
        let x = t.get("x").unwrap();
        assert!(!tappable("Sponsor this", bsky));
        assert!(!tappable("SPONSOR", bsky));
        assert!(tappable("Sponsor this", x));
    }

    #[test]
    fn default_budget_is_40_steps_10_minutes_3_unchanged() {
        let b = Budget::default();
        assert_eq!((b.max_steps, b.max_ms, b.max_unchanged), (40, 600_000, 3));
    }

    #[test]
    fn time_limit_boundary_is_strictly_after_600_000_ms() {
        let b = Budget::default();
        let m = Meter::new(1_000);
        assert!(
            m.over(&b, 601_000).is_none(),
            "exactly 10 minutes is still fine"
        );
        assert!(matches!(m.over(&b, 601_001), Some(Over::Time)));
    }

    #[test]
    fn limits_40_steps_10_minutes_3_unchanged() {
        let b = Budget {
            max_steps: 40,
            max_ms: 600_000,
            max_unchanged: 3,
        };
        let mut m = Meter::new(0);
        for _ in 0..39 {
            m.record(true);
        }
        assert!(m.over(&b, 1000).is_none());
        m.record(true);
        assert!(matches!(m.over(&b, 1000), Some(Over::Steps)));
        let mut m = Meter::new(0);
        assert!(matches!(m.over(&b, 600_001), Some(Over::Time)));
        m.record(false);
        m.record(false);
        assert!(m.over(&b, 1).is_none());
        m.record(false);
        assert!(matches!(m.over(&b, 1), Some(Over::Unchanged)));
        m.record(true);
        assert!(m.over(&b, 1).is_none(), "变了就重新数");
    }

    #[test]
    fn icon_names_follow_the_same_rules_as_screen_text() {
        let t = Table::seed();
        let x = t.get("x").unwrap();
        assert_eq!(check_icon_name("Add photo", x), IconName::Ok);
        assert_eq!(check_icon_name("Post", x), IconName::Publish);
        assert_eq!(check_icon_name(" post ", x), IconName::Publish);
        assert_eq!(check_icon_name("Post it", x), IconName::Publish);
        assert_eq!(check_icon_name("Repost", x), IconName::Ok, "整词才算");
        let xhs = t.get("xiaohongshu").unwrap();
        assert_eq!(check_icon_name("发布笔记", xhs), IconName::Publish);
        for bad in [
            "Follow",
            "Get verified",
            "推广",
            "Delete draft",
            "",
            "Add\u{200B}photo",
            "Add\u{202E}photo",
            "Add\tphoto",
            "Add\rphoto",
            &"x".repeat(41),
        ] {
            assert!(
                matches!(check_icon_name(bad, x), IconName::Refused(_)),
                "{bad:?}"
            );
        }
        assert!(icon_name_shape(&"字".repeat(40), &[]).is_ok());
        assert!(
            icon_name_shape("Delete", &[]).is_err(),
            "通用禁点词不看档案"
        );
        assert!(icon_name_shape("Boost", &[]).is_ok(), "没给词就不拦");
        assert!(icon_name_shape("Boost", &["boost".to_string()]).is_err());
    }
}
