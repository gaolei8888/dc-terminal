//! 停下的规矩：登录 / 验证码 / 账号警告画面、禁点按钮、步数 / 时间 / 没变化上限。
//! 全是代码，探索循环在问任何模型之前先过这里。词表命中一律「宁可多停」。
use crate::hands::El;
use crate::platform::Platform;
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
];
/// 通用禁点词；平台自己的在 `Platform::never_tap`。
const NEVER: &[&str] = &[
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
    // brief 的测试要求「Delete」也拦，但 dct-brain 把单独的「删除」算 Content（不到 Critical），
    // 所以不可撤回的删除在这里补进禁点词。
    "delete",
    "删除",
];

/// 和 dct-brain 的档位同一套规整（全角、零宽、弯引号、汉字间空格、小写），不另写一份。
fn fold(s: &str) -> String {
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

/// 这个元素能不能进选项：禁点词（通用 + 平台）、tier >= Critical 一律不行。
pub fn tappable(label: &str, p: &Platform) -> bool {
    let f = fold(label);
    let banned = NEVER
        .iter()
        .copied()
        .map(str::to_owned)
        .chain(p.never_tap.iter().map(|w| fold(w)));
    if banned.filter(|w| !w.is_empty()).any(|w| f.contains(&w)) {
        return false;
    }
    tier_for_label(label) < Tier::Critical
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
    use crate::hands::{El, Frac};
    use crate::platform::Table;

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
}
