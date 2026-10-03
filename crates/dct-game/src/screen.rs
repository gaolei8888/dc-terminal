//! 认画面：一次 `see`（OCR）读到的元素 → 这是什么画面、该点哪个。纯函数，不碰屏幕。
//! 分类的顺序和白名单见设计 docs/superpowers/specs/2026-10-03-dct-match3-next-level-design.md。

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub id: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// 生命用完了。
    LivesOut,
    /// 通关结算。
    Won,
    /// 有个整条文字正好是“关闭”这一类词的按钮：点它。
    Dismiss(Element),
    /// 画面上有价格，又没有安全的关闭按钮。
    Money,
    /// 广告（右上角的叉叉没有字，dco 不让点）。
    Ad,
    /// 有个整条文字正好是“再来一次”的按钮。
    Retry(Element),
    /// 恰好一个整条文字正好是“开始”的按钮。
    PlayButton(Element),
    Unknown,
}

const DISMISS: &[&str] = &[
    "close", "no thanks", "not now", "later", "maybe later", "skip", "cancel", "got it", "tap to continue", "next", "关闭", "以后再说", "暂不", "跳过",
    "取消", "知道了", "下一步",
];
const RETRY: &[&str] = &["try again", "retry", "再试一次", "重试"];
const PLAY: &[&str] = &["play", "start", "开始", "开始游戏"];
const MONEY_MARKS: &[&str] = &["¥", "￥", "$", "€", "£", "💎", "金币", "钻石", "购买", "支付", "充值"];
const MONEY_WORDS: &[&str] = &["buy", "purchase", "pay", "checkout", "gold", "gem", "gems", "bar", "bars"];
const WON_PHRASES: &[&str] = &["level complete", "level completed", "level cleared", "通关"];

/// 小写、合并空白、去掉两头的标点和符号（OCR 常把叉叉、箭头读成一个字符粘在旁边）。
fn norm(s: &str) -> String {
    let lower = s.to_lowercase();
    let joined = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.trim_matches(|c: char| !c.is_alphanumeric() && !is_cjk(c)).to_string()
}

fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

fn exact<'a>(els: &'a [Element], list: &[&str]) -> Vec<&'a Element> {
    els.iter().filter(|e| list.contains(&norm(&e.text).as_str())).collect()
}

fn has_money(els: &[Element]) -> bool {
    els.iter().any(|e| {
        let t = e.text.to_lowercase();
        MONEY_MARKS.iter().any(|m| t.contains(m)) || words(&t).iter().any(|w| MONEY_WORDS.contains(&w.as_str()))
    })
}

fn has_ad(els: &[Element]) -> bool {
    els.iter().any(|e| words(&e.text).iter().any(|w| w == "ad" || w == "ads" || w == "advert" || w == "advertisement"))
}

fn lives_out(all: &str) -> bool {
    let w = words(all);
    let lives = w.iter().any(|x| x == "lives" || x == "life");
    let out = ["no more", "out of", "ask friends", "get more", "0 lives"].iter().any(|p| all.contains(p));
    (lives && out) || (all.contains("生命") && ["用完", "不足", "没有了"].iter().any(|p| all.contains(p)))
}

/// 顺序就是优先级：生命用完 → 通关 → 安全的关闭 → 价格 → 广告 → 再来一次 → 开始 → 不认识。
/// 关闭排在价格前面：一个带价格的失败弹窗里有正好是 “No thanks” 的按钮，点它是安全的。
pub fn classify(els: &[Element]) -> Screen {
    let all = els.iter().map(|e| e.text.to_lowercase()).collect::<Vec<_>>().join(" \n ");
    if lives_out(&all) {
        return Screen::LivesOut;
    }
    if WON_PHRASES.iter().any(|p| all.contains(p)) {
        return Screen::Won;
    }
    if let Some(e) = exact(els, DISMISS).first() {
        return Screen::Dismiss((*e).clone());
    }
    if has_money(els) {
        return Screen::Money;
    }
    if has_ad(els) {
        return Screen::Ad;
    }
    if let Some(e) = exact(els, RETRY).first() {
        return Screen::Retry((*e).clone());
    }
    let plays = exact(els, PLAY);
    if plays.len() == 1 {
        return Screen::PlayButton(plays[0].clone());
    }
    Screen::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn els(texts: &[&str]) -> Vec<Element> {
        texts.iter().enumerate().map(|(i, t)| Element { id: format!("e{}", i + 1), text: t.to_string() }).collect()
    }

    fn id(s: &Screen) -> &str {
        match s {
            Screen::Dismiss(e) | Screen::Retry(e) | Screen::PlayButton(e) => &e.id,
            _ => "",
        }
    }

    /// 2026-10-03 真机上通关后的画面，OCR 原样。
    #[test]
    fn the_real_daily_stamps_screen_is_a_play_button() {
        let s = classify(&els(&["Daily Stamps", "Complete tasks to get stamps ana", "unlock reward chests", "Play"]));
        assert_eq!((id(&s), matches!(s, Screen::PlayButton(_))), ("e4", true));
    }

    #[test]
    fn a_price_without_a_safe_close_is_money() {
        assert_eq!(classify(&els(&["Out of moves", "Continue for 💎 5"])), Screen::Money);
        assert_eq!(classify(&els(&["Buy now", "Play"])), Screen::Money);
        assert_eq!(classify(&els(&["Play now for 💎 5"])), Screen::Money);
    }

    #[test]
    fn every_money_word_counts() {
        for t in ["Buy now", "Purchase", "Pay", "Checkout", "Gold", "Gem", "Gems", "Bar", "Bars", "购买道具", "5 金币", "充值", "支付"] {
            assert_eq!(classify(&els(&[t])), Screen::Money, "{t}");
        }
        // 整词才算：Payment 不是 pay 这个词，但 Pay 在句子里就是
        assert_eq!(classify(&els(&["Barbara"])), Screen::Unknown);
    }

    #[test]
    fn a_price_with_an_exact_no_thanks_is_dismissed_not_stopped() {
        let s = classify(&els(&["Out of moves", "Continue for 💎 5", "No thanks"]));
        assert_eq!((id(&s), matches!(s, Screen::Dismiss(_))), ("e3", true));
    }

    #[test]
    fn an_ad_is_an_ad_unless_there_is_a_safe_close() {
        assert_eq!(classify(&els(&["Watch an ad for a sweet treat", "Watch ad"])), Screen::Ad);
        assert!(matches!(classify(&els(&["Watch ad", "Not now"])), Screen::Dismiss(_)));
    }

    #[test]
    fn two_exact_play_buttons_are_unknown_but_a_garbled_one_is_ignored() {
        assert_eq!(classify(&els(&["Level 1712", "Select boosters:", "Play", "Play"])), Screen::Unknown);
        let s = classify(&els(&["Level 1712", "Select boosters:", "Play", "B Play"]));
        assert_eq!((id(&s), matches!(s, Screen::PlayButton(_))), ("e3", true));
    }

    #[test]
    fn lives_out_beats_everything_below_it() {
        assert_eq!(classify(&els(&["No more lives", "Ask friends", "Play"])), Screen::LivesOut);
        assert_eq!(classify(&els(&["Out of lives", "Close"])), Screen::LivesOut);
        assert_eq!(classify(&els(&["生命用完了", "关闭"])), Screen::LivesOut);
    }

    #[test]
    fn a_won_screen_stops_even_when_it_offers_next() {
        assert_eq!(classify(&els(&["Level Complete!", "Next"])), Screen::Won);
    }

    #[test]
    fn retry_is_found_by_its_whole_text() {
        let s = classify(&els(&["Out of moves", "Try again"]));
        assert_eq!((id(&s), matches!(s, Screen::Retry(_))), ("e2", true));
        assert_eq!(classify(&els(&["Try again later maybe"])), Screen::Unknown);
    }

    #[test]
    fn matching_ignores_case_spaces_and_stray_symbols() {
        assert!(matches!(classify(&els(&["  NOT   NOW "])), Screen::Dismiss(_)));
        assert!(matches!(classify(&els(&["Close ×"])), Screen::Dismiss(_)));
        assert!(matches!(classify(&els(&["关闭"])), Screen::Dismiss(_)));
    }

    #[test]
    fn close_beats_play_and_save_my_progress_is_never_chosen() {
        assert!(matches!(classify(&els(&["Play", "Close"])), Screen::Dismiss(_)));
        let s = classify(&els(&["Play", "Save My Progress"]));
        assert_eq!((id(&s), matches!(s, Screen::PlayButton(_))), ("e1", true));
        assert_eq!(classify(&els(&["Save My Progress"])), Screen::Unknown);
    }

    #[test]
    fn claim_and_collect_are_not_on_the_list() {
        for t in ["Claim", "Collect", "Continue", "OK", "Collect your daily treat!"] {
            assert_eq!(classify(&els(&[t])), Screen::Unknown, "{t}");
        }
    }

    #[test]
    fn nothing_to_read_is_unknown() {
        assert_eq!(classify(&[]), Screen::Unknown);
        assert_eq!(classify(&els(&["38", "90", "4"])), Screen::Unknown);
    }
}
