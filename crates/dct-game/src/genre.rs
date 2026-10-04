//! 快速鉴定「这是哪一类游戏」：只用本机能得到的东西（有没有合格棋盘、屏幕上的字）。
//! 线索是游戏**类别**的通用词，不含任何具体游戏；放在下面的表里，以后可以搬去 dcv。
//! 不确定就说看不出，绝不乱猜。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Genre {
    Match3,
    HiddenObject,
    Unknown,
}

/// 命中任一个就判寻物。
const HIDDEN_STRONG: &[&str] = &["collector's edition", "hidden object", "inventory", "journal", "物品栏", "寻物", "收藏版"];
/// 至少命中两个**不同**的才判寻物（消除游戏的主菜单也有 play / options / extras）。
const HIDDEN_WEAK: &[&str] = &["hint", "map", "extras", "find", "skip", "提示", "地图", "跳过"];

/// 去前后空白、转小写、把全角字母数字转成半角。中间的空白不动（「hidden   object」不是线索词）。
fn normalise(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

pub fn classify(texts: &[String], board_ok: Option<bool>) -> Genre {
    if board_ok == Some(true) {
        return Genre::Match3;
    }
    let norm: Vec<String> = texts.iter().map(|t| normalise(t)).filter(|t| !t.is_empty()).collect();
    let hits = |table: &[&str]| table.iter().filter(|cue| norm.iter().any(|t| t.contains(**cue))).count();
    if hits(HIDDEN_STRONG) >= 1 || hits(HIDDEN_WEAK) >= 2 {
        Genre::HiddenObject
    } else {
        Genre::Unknown
    }
}

pub fn sentence(g: Genre) -> &'static str {
    match g {
        Genre::Match3 => "这是方格消除类游戏，我会玩。",
        Genre::HiddenObject => "这看起来是寻物类游戏，我现在还不会玩。",
        Genre::Unknown => "我看不出这是哪一类游戏。",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_readable_board_is_match3_even_with_hidden_object_words_on_screen() {
        assert_eq!(classify(&t(&["Hint", "Inventory", "Journal"]), Some(true)), Genre::Match3);
    }

    #[test]
    fn one_strong_cue_is_enough() {
        assert_eq!(classify(&t(&["Enemy in Reflection", "COLLECTOR'S EDITION", "PLAY"]), Some(false)), Genre::HiddenObject);
        assert_eq!(classify(&t(&["物品栏"]), None), Genre::HiddenObject);
    }

    #[test]
    fn one_weak_cue_alone_is_not_enough_but_two_different_ones_are() {
        // 方格消除游戏的主菜单也有 PLAY / OPTIONS / EXTRAS 这类字
        assert_eq!(classify(&t(&["PLAY", "OPTIONS", "EXTRAS"]), Some(false)), Genre::Unknown);
        assert_eq!(classify(&t(&["Hint"]), Some(false)), Genre::Unknown);
        assert_eq!(classify(&t(&["Hint", "Map"]), Some(false)), Genre::HiddenObject);
        // 同一个词出现多次只算一个
        assert_eq!(classify(&t(&["Hint", "hint", "HINT"]), Some(false)), Genre::Unknown);
    }

    #[test]
    fn matching_ignores_case_whitespace_and_full_width_letters() {
        assert_eq!(classify(&t(&["  Ｉｎｖｅｎｔｏｒｙ "]), None), Genre::HiddenObject);
        assert_eq!(classify(&t(&["Hidden   Object"]), None), Genre::Unknown, "中间多空格不是线索词，不乱猜");
        assert_eq!(classify(&t(&["HIDDEN OBJECT"]), None), Genre::HiddenObject);
    }

    #[test]
    fn nothing_readable_and_no_board_is_unknown_not_hidden_object() {
        assert_eq!(classify(&[], Some(false)), Genre::Unknown);
        assert_eq!(classify(&[], None), Genre::Unknown);
        assert_eq!(classify(&t(&["", "  "]), Some(false)), Genre::Unknown);
    }

    #[test]
    fn the_three_sentences_are_fixed_plain_chinese() {
        assert_eq!(sentence(Genre::Match3), "这是方格消除类游戏，我会玩。");
        assert_eq!(sentence(Genre::HiddenObject), "这看起来是寻物类游戏，我现在还不会玩。");
        assert_eq!(sentence(Genre::Unknown), "我看不出这是哪一类游戏。");
    }
}
