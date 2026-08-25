//! 手机网页要用到的全部文案，由守护进程发过去。
//!
//! **网页里一个字的用户可见文案都不许写死。** 这不是洁癖，是这个仓库既有的
//! 规矩在新地方的落点：`proto.rs` 里那条「`ProfileEntry.label` 是 `String`
//! 不是 `LocalizedText`」说的就是——**决定用户看到什么字的地方只有一个，
//! 就是守护进程**。网页里再抄一份的话：
//!
//! - `l` 键切了界面语言，手机上不跟着变（它抄的那份不知道这回事）
//! - `i18n.rs` 那两条守卫（两种语言都组得出话、英文里不许有汉字）查不到它
//! - 同一个状态词在电脑上和手机上写得不一样，而没有任何东西会报错
//!
//! 所以网页拿到的是一张表，`t("idle")` 这样取。表里有哪些键由 [`NEEDED`] 定，
//! 而 `page_asks_only_for_strings_that_exist` 那条测试会把网页里真的取过的键
//! 跟这张表对一遍——网页多问一个键，测试就挂。

use crate::i18n::{text, Key, Lang};

/// 手机网页需要的键。左边是网页里写的名字，右边是 `i18n` 的 `Key`。
///
/// **加一行之前先问一句：这句话在电脑上已经有了吗。** 有就复用同一个 `Key`，
/// 别新起一个——两个 `Key` 说同一件事，迟早有一天两边的措辞会不一样，
/// 而那种不一致没有任何测试抓得到。
pub const NEEDED: &[(&str, Key)] = &[
    ("title", Key::BoardTitle),
    ("empty", Key::NoSessionsHere),
    ("offline", Key::PhoneOffline),
    ("working", Key::StatusWorking),
    ("asking", Key::StatusAsking),
    ("idle", Key::StatusIdle),
    ("stopped", Key::StatusStopped),
    ("failed", Key::StatusFailed),
    ("unknown", Key::StatusUnknown),
];

/// 一门语言的整张表，JSON。
pub fn bundle(lang: Lang) -> String {
    let map: std::collections::BTreeMap<&str, &str> = NEEDED
        .iter()
        .map(|(name, key)| (*name, text(*key, lang)))
        .collect();
    // 这张表是我们自己拼的，只有 `&str`，序列化不可能失败——真失败了也
    // 只能给一张空表，而空表在网页上的样子是「所有文字都不见了」，
    // 比 panic 掉整个守护进程强。
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_needed_string_exists_in_both_languages() {
        for lang in Lang::all() {
            for (name, key) in NEEDED {
                let s = text(*key, *lang);
                assert!(!s.trim().is_empty(), "{name} 在 {lang:?} 下是空的");
            }
        }
    }

    #[test]
    fn the_bundle_is_json_with_every_name_in_it() {
        for lang in Lang::all() {
            let json: std::collections::BTreeMap<String, String> =
                serde_json::from_str(&bundle(*lang)).unwrap();
            for (name, _) in NEEDED {
                assert!(json.contains_key(*name), "{lang:?} 的表里少了 {name}");
            }
        }
    }

    /// 中文和英文必须真的不一样——至少在那些**不是符号**的条目上。
    ///
    /// 这条钉的是「表是按语言取的」这件事本身：一个把 `lang` 参数丢掉的实现
    /// （比如写死 `Lang::En`）会让手机端永远是英文，而上面两条测试照样全绿。
    #[test]
    fn the_two_languages_are_actually_different() {
        let zh: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&bundle(Lang::Zh)).unwrap();
        let en: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&bundle(Lang::En)).unwrap();

        // `unknown` 两边都是「—」，那是故意的（见 `StatusUnknown`），不算数。
        let differing = NEEDED
            .iter()
            .filter(|(name, _)| *name != "unknown")
            .filter(|(name, _)| zh[*name] != en[*name])
            .count();
        assert_eq!(
            differing,
            NEEDED.len() - 1,
            "有条目两种语言写得一模一样，多半是漏翻了：\n{zh:?}\n{en:?}"
        );
    }
}
