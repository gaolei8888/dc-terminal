//! 界面语言：语言的解析与词条表。
//!
//! 这个模块**不认识界面，也不认识守护进程**——它只回答「这条文案在这种语言里
//! 怎么说」。守护进程那边永远不组句（它连用户选了什么语言都不知道），只报
//! 错误码，组句一律发生在界面进程，所以切语言立刻生效、不用重启 daemon。

use serde::{Deserialize, Serialize};

/// 界面语言。第一阶段两种；将来加 `Ja` 那天，编译器会把每一条没翻的都点名——
/// 这是选枚举而不是配置文件的唯一理由，也正是本项目要的。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Lang {
    En,
    Zh,
}

impl Lang {
    /// 语言自己的名字。误切到看不懂的语言时，用户还得能认出自己那一行切回来——
    /// 所以这里绝不能是「英语 / 中文」这种用当前语言写的译名。
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Zh => "中文",
        }
    }

    /// 存进 settings.json 的稳定短码。跟枚举顺序无关，加语言不会让老文件失效。
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Zh => "zh",
        }
    }

    pub fn from_code(s: &str) -> Option<Lang> {
        match s {
            "en" => Some(Lang::En),
            "zh" => Some(Lang::Zh),
            _ => None,
        }
    }

    pub fn all() -> &'static [Lang] {
        &[Lang::En, Lang::Zh]
    }
}

/// 最终用哪种语言。优先级：`DCT_LANG` > 用户存过的设置 > 系统 locale > `En`。
///
/// `env` 是闭包不是直接读 `std::env`：环境变量是进程全局状态，测试里改它会互相
/// 打架。生产传 `|k| std::env::var(k).ok()`，测试传一张假表。
pub fn resolve(saved: Option<Lang>, env: &dyn Fn(&str) -> Option<String>) -> Lang {
    // DCT_LANG 压过一切：它是「这一次就用这个」的逃生口，值不认识就当没设，
    // 继续往下走，而不是硬摔成 En——用户打错一个字母不该丢掉他存过的选择。
    if let Some(l) = env("DCT_LANG").as_deref().and_then(Lang::from_code) {
        return l;
    }
    if let Some(l) = saved {
        return l;
    }
    // 系统 locale 只认主码：`zh_CN.UTF-8` → `zh`。认不出就是 En。
    for k in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let Some(v) = env(k) else { continue };
        if v.is_empty() {
            continue;
        }
        let primary = v.split(['_', '.', '@']).next().unwrap_or("");
        if let Some(l) = Lang::from_code(primary) {
            return l;
        }
    }
    Lang::En
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fake_env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    #[test]
    fn english_is_the_default_when_nothing_says_otherwise() {
        assert_eq!(resolve(None, &fake_env(&[])), Lang::En);
    }

    #[test]
    fn a_saved_choice_beats_the_system_locale() {
        // 用户在设置里明确选过，就不该被系统 locale 推翻
        let env = fake_env(&[("LANG", "en_US.UTF-8")]);
        assert_eq!(resolve(Some(Lang::Zh), &env), Lang::Zh);
    }

    #[test]
    fn dct_lang_beats_even_a_saved_choice() {
        let env = fake_env(&[("DCT_LANG", "en")]);
        assert_eq!(resolve(Some(Lang::Zh), &env), Lang::En);
    }

    /// 打错的 `DCT_LANG` 不能把用户存过的选择也一起丢掉——那是「我想临时换一下」
    /// 变成「我的设置没了」。
    #[test]
    fn an_unknown_dct_lang_falls_through_instead_of_resetting() {
        let env = fake_env(&[("DCT_LANG", "klingon")]);
        assert_eq!(resolve(Some(Lang::Zh), &env), Lang::Zh);
    }

    #[test]
    fn the_system_locale_is_read_down_to_its_primary_code() {
        assert_eq!(
            resolve(None, &fake_env(&[("LANG", "zh_CN.UTF-8")])),
            Lang::Zh
        );
        assert_eq!(
            resolve(None, &fake_env(&[("LC_ALL", "zh_TW.UTF-8")])),
            Lang::Zh
        );
        assert_eq!(
            resolve(None, &fake_env(&[("LANG", "ja_JP.UTF-8")])),
            Lang::En
        );
    }

    /// `LC_ALL` 压过 `LC_MESSAGES` 压过 `LANG`，这是 POSIX 的规矩，不是我们定的。
    #[test]
    fn locale_variables_are_checked_in_posix_order() {
        let env = fake_env(&[("LC_ALL", "zh_CN.UTF-8"), ("LANG", "en_US.UTF-8")]);
        assert_eq!(resolve(None, &env), Lang::Zh);
    }

    /// 空字符串等于没设。真实环境里 `LANG=` 很常见（尤其在 cron 和容器里），
    /// 当成「设了一个认不出的值」会让它挡住后面本来有效的变量。
    #[test]
    fn an_empty_locale_variable_is_skipped_not_honored() {
        let env = fake_env(&[("LC_ALL", ""), ("LANG", "zh_CN.UTF-8")]);
        assert_eq!(resolve(None, &env), Lang::Zh);
    }

    #[test]
    fn codes_round_trip_and_unknown_ones_are_rejected() {
        for l in Lang::all() {
            assert_eq!(Lang::from_code(l.code()), Some(*l));
        }
        assert_eq!(Lang::from_code("klingon"), None);
    }

    /// 语言名必须用它自己的语言写：用户误切到看不懂的语言之后，
    /// 唯一能自救的线索就是在列表里认出自己那一行。
    #[test]
    fn each_language_is_named_in_its_own_language() {
        assert_eq!(Lang::En.native_name(), "English");
        assert_eq!(Lang::Zh.native_name(), "中文");
    }
}
