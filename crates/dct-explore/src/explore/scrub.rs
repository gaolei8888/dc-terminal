//! 参数值换回 `{名字}`。画面上的每个元素先分两类（裁决 R14）：
//!
//! - **值元素**：整段就是参数值的一部分（且这个参数这次已经打过字），或者里面含着一整段参数值
//!   （值本身也要够长）。它整段显示成 `{名字}`，不参加任何规矩（停下、禁点、档位、发布词），
//!   也不能按它自己的字去点。
//!   「一部分」按词对齐：英文要盖住值里至少 2 个完整的词（两头可以是被文字识别切开的半个词，
//!   但「Log in」不算「Blog in Paris」的一部分），中文按字算、至少 4 个字；只有一个词的元素
//!   只有像网址 / 标签（带 `/ . @ #`、去掉标点后 ≥ 12 个字母）才算，所以界面上单独一个标题词
//!   （「Verification」）永远是界面元素。
//! - **界面元素**：规矩照原文判断；显示时只换掉整段值、值的整行，以及和值连着相同的**长**片段
//!   （≥ 4 个词，或 ≥ 8 个中文字）——两个词的巧合（「What's happening」）不换。
//!
//! 不是元素的字（dco 的话、图标名字、目标）按界面元素的规则换。
use crate::gate::fold;
use crate::hands::{El, Params};
use crate::trace::redact;
use std::cell::RefCell;
use std::collections::BTreeSet;

/// 界面元素里连着这么多个值的词才换。
const LONG_WORDS: usize = 4;
/// 界面元素里连着这么多个值的中文字才换。
const LONG_CJK: usize = 8;
/// 值元素：中文至少这么多字。
const VALUE_CJK: usize = 4;
/// 值元素：没有空格的英文长串至少这么长（网址、标签）。
const VALUE_ASCII: usize = 12;

/// 去掉空白和标点后的 fold：「Launch  DAY, for」→「launchdayfor」。
fn squash(s: &str) -> String {
    fold(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// 中日文字：一个字就是一个单位（不靠空格分词）。
fn cjk(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFF66..=0xFF9F | 0x20000..=0x3FFFF)
}

/// 去掉空白和标点后的字，外加每个（按空白切的）词在里面的起止位置：用来做按词对齐的比较，
/// 文字识别多出 / 少了空格也对得上。
struct Sq {
    chars: Vec<char>,
    words: Vec<(usize, usize)>,
}

impl Sq {
    fn new(s: &str) -> Sq {
        let (mut chars, mut words) = (Vec::new(), Vec::new());
        for w in fold(s).split_whitespace() {
            let a = chars.len();
            chars.extend(w.chars().filter(|c| c.is_alphanumeric()));
            if chars.len() > a {
                words.push((a, chars.len()));
            }
        }
        Sq { chars, words }
    }
    /// 第 i 个字前面是不是词的边界（两头、词的起止、挨着中文字）。
    fn boundary(&self, i: usize) -> bool {
        i == 0
            || i == self.chars.len()
            || self.words.iter().any(|&(a, b)| a == i || b == i)
            || cjk(self.chars[i - 1])
            || cjk(self.chars[i])
    }
    /// `needle` 在这里出现的每个起点。
    fn find<'a>(&'a self, needle: &'a [char]) -> impl Iterator<Item = usize> + 'a {
        let n = needle.len();
        (0..(self.chars.len() + 1).saturating_sub(n))
            .filter(move |&p| n > 0 && self.chars[p..p + n] == *needle)
    }
    /// `[p, end)` 完整盖住的、不全是中文的词有几个。
    fn whole_words(&self, p: usize, end: usize) -> usize {
        self.words
            .iter()
            .filter(|&&(a, b)| a >= p && b <= end && self.chars[a..b].iter().any(|c| !cjk(*c)))
            .count()
    }
}

/// 像网址 / 标签 / 账号的一整串：带 `/ @ #`，或者两个字母数字中间夹着 `.`（域名），去掉标点后
/// ≥ 12 个字母。末尾的句号不算（「Login-required.」是标题，不是网址）。
fn link_like(raw: &str, sq: &Sq) -> bool {
    let t: Vec<char> = raw.trim().chars().collect();
    let inner_dot = t
        .windows(3)
        .any(|w| w[1] == '.' && w[0].is_alphanumeric() && w[2].is_alphanumeric());
    raw.split_whitespace().count() == 1
        && (t.iter().any(|c| matches!(c, '/' | '@' | '#')) || inner_dot)
        && sq.chars.len() >= VALUE_ASCII
}

/// 一个词 fold 后去掉两头的标点。
fn word(w: &str) -> String {
    fold(w)
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_string()
}

/// 单个字的 fold：全角变半角、小写。
fn nc(c: char) -> char {
    let c = match c {
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        c => c,
    };
    c.to_lowercase().next().unwrap_or(c)
}

/// 值本身够不够长（「含着一整段值」才算数）：≥ 2 个词，或 ≥ 4 个中文字，或一整段 ≥ 12 个字母的长串。
fn substantial(raw: &str, sq: &str) -> bool {
    let words = raw
        .split_whitespace()
        .filter(|w| !word(w).is_empty())
        .count();
    let cjk = sq.chars().filter(|c| !c.is_ascii()).count();
    words >= 2 || cjk >= VALUE_CJK || (sq.is_ascii() && sq.len() >= VALUE_ASCII)
}

/// 一个参数值的模糊匹配材料。
struct Fuzzy {
    name: String,
    words: Vec<String>,
    chars: Vec<char>,
    sq: Sq,
    /// 值本身够长，「含着一整段值」才算数（太短的值会撞上普通界面字）。
    whole_ok: bool,
}

impl Fuzzy {
    fn new(name: &str, value: &str) -> Option<Fuzzy> {
        let squashed = squash(value);
        if squashed.chars().count() < VALUE_CJK {
            return None;
        }
        Some(Fuzzy {
            name: name.to_string(),
            words: value
                .split_whitespace()
                .map(word)
                .filter(|w| !w.is_empty())
                .collect(),
            chars: value
                .chars()
                .map(nc)
                .filter(|c| c.is_alphanumeric())
                .collect(),
            whole_ok: substantial(value, &squashed),
            sq: Sq::new(value),
        })
    }

    /// 这段元素原文是不是值的一部分（按词对齐，见文件开头）。
    fn has_piece(&self, raw: &str, e: &Sq) -> bool {
        let n = e.chars.len();
        let cjk_n = e.chars.iter().filter(|c| cjk(**c)).count();
        // 网址一类的长串在值里常常是一个「词」的中间一段（去掉了 https://），不要求对齐。
        let link = link_like(raw, e);
        // 英文：元素自己至少 2 个词，盖住值里至少 2 个完整的词，元素里每个词的分界都落在值的词边界上
        // （「Login」不是「log in」拼起来的一个词）。
        self.sq.find(&e.chars).any(|p| {
            link || cjk_n >= VALUE_CJK
                || (e.words.len() >= 2
                    && self.sq.whole_words(p, p + n) >= 2
                    && e.words[..e.words.len() - 1]
                        .iter()
                        .all(|&(_, b)| self.sq.boundary(p + b)))
        })
    }
    /// 这段元素原文里含着一整段值（起止都落在元素的词边界上）。
    fn inside(&self, e: &Sq) -> bool {
        let n = self.sq.chars.len();
        self.whole_ok
            && e.find(&self.sq.chars)
                .any(|p| e.boundary(p) && e.boundary(p + n))
    }

    /// 从 `toks[i]` 起和值里某处连着相同的最多几个词。
    fn run_at(&self, toks: &[String], i: usize) -> usize {
        (0..self.words.len())
            .map(|k| {
                toks[i..]
                    .iter()
                    .zip(&self.words[k..])
                    .take_while(|(a, b)| !a.is_empty() && a == b)
                    .count()
            })
            .max()
            .unwrap_or(0)
    }

    /// 一个词里和值连着相同的中文（字母数字）字数，从第 i 个起。
    fn chars_at(&self, t: &[char], i: usize) -> usize {
        (0..self.chars.len())
            .map(|k| {
                t[i..]
                    .iter()
                    .zip(&self.chars[k..])
                    .take_while(|(a, b)| a == b)
                    .count()
            })
            .max()
            .unwrap_or(0)
    }

    /// 一个没有空格的非英文词：把和值连着相同的 ≥ 8 个字换掉。
    fn apply_cjk(&self, w: &str) -> String {
        let pos: Vec<(usize, usize, char)> = w
            .char_indices()
            .filter(|(_, c)| c.is_alphanumeric())
            .map(|(i, c)| (i, i + c.len_utf8(), nc(c)))
            .collect();
        let t: Vec<char> = pos.iter().map(|p| p.2).collect();
        let mut out = String::new();
        let (mut at, mut i) = (0, 0);
        while i < t.len() {
            let n = self.chars_at(&t, i);
            if n >= LONG_CJK {
                out.push_str(&w[at..pos[i].0]);
                out.push_str(&format!("{{{}}}", self.name));
                at = pos[i + n - 1].1;
                i += n;
            } else {
                i += 1;
            }
        }
        out.push_str(&w[at..]);
        out
    }

    /// 界面字：和值连着相同的 ≥ 4 个词换成 `{名字}`；非英文的词里 ≥ 8 个连着相同的字也换。
    fn apply(&self, s: &str) -> String {
        let spans: Vec<(usize, usize)> = s
            .split_whitespace()
            .map(|w| {
                let start = w.as_ptr() as usize - s.as_ptr() as usize;
                (start, start + w.len())
            })
            .collect();
        let toks: Vec<String> = spans.iter().map(|&(a, b)| word(&s[a..b])).collect();
        let mut out = String::with_capacity(s.len());
        let (mut at, mut i) = (0, 0);
        while i < toks.len() {
            let n = self.run_at(&toks, i);
            let (a, b) = spans[i];
            if n >= LONG_WORDS {
                out.push_str(&s[at..a]);
                out.push_str(&format!("{{{}}}", self.name));
                at = spans[i + n - 1].1;
                i += n;
            } else if !s[a..b].is_ascii() {
                out.push_str(&s[at..a]);
                out.push_str(&self.apply_cjk(&s[a..b]));
                at = b;
                i += 1;
            } else {
                i += 1;
            }
        }
        out.push_str(&s[at..]);
        out
    }
}

pub(crate) struct Scrub {
    full: Params,
    lines: Vec<Params>,
    fuzzy: Vec<Fuzzy>,
    /// 这次已经打过字的参数：只有它们的片段才可能出现在画面上（发帖框里的提示字和帖子开头一样时
    /// 不会被当成帖子）。
    typed: RefCell<BTreeSet<String>>,
}

impl Scrub {
    pub(crate) fn new(params: &Params) -> Scrub {
        let mut lines = Vec::new();
        for (k, v) in params {
            for l in v.lines().map(str::trim) {
                if l != v.trim() && l.chars().count() >= 4 {
                    lines.push([(k.clone(), l.to_string())].into_iter().collect());
                }
            }
        }
        Scrub {
            full: params.clone(),
            lines,
            fuzzy: params
                .iter()
                .filter_map(|(k, v)| Fuzzy::new(k, v))
                .collect(),
            typed: RefCell::new(BTreeSet::new()),
        }
    }
    /// 要往画面上打这个参数了（在 `type_param` 之前调；重放 / 修补时是流程里已经走过的 `type_param`）。
    pub(crate) fn mark_typed(&self, name: &str) {
        self.typed.borrow_mut().insert(name.to_string());
    }
    /// 这段画面原文是不是「值元素」，是的话回参数名。
    pub(crate) fn value_of(&self, raw: &str) -> Option<&str> {
        let e = Sq::new(raw);
        if e.chars.is_empty() {
            return None;
        }
        let typed = self.typed.borrow();
        self.fuzzy
            .iter()
            .find(|f| f.inside(&e) || (typed.contains(&f.name) && f.has_piece(raw, &e)))
            .map(|f| f.name.as_str())
    }
    /// 界面字的脱敏：整段值、值的整行、和值连着相同的长片段。
    pub(crate) fn text(&self, s: &str) -> String {
        let mut out = redact(s, &self.full);
        for m in &self.lines {
            out = redact(&out, m);
        }
        for f in &self.fuzzy {
            out = f.apply(&out);
        }
        out
    }
    /// 一行给人和模型看的字：先脱敏（多行值还能整段对上），再把换行和其他控制字符压成空格，
    /// 这样画面上的字没法在 state 里伪造出一行「[e9] …」。
    pub(crate) fn line(&self, s: &str) -> String {
        self.text(s)
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    }
    /// 这段字里有参数占位符（`{post_text}` 之类）：说明它带着要发的内容，不能当意图名 / 图标名交给 dco。
    pub(crate) fn has_placeholder(&self, s: &str) -> bool {
        self.full.keys().any(|k| s.contains(&format!("{{{k}}}")))
    }
    /// 给外面看的元素：值元素整段换成 `{名字}`，界面元素按 `line`。
    pub(crate) fn els(&self, els: &[El]) -> Vec<El> {
        els.iter()
            .map(|e| El {
                text: match self.value_of(&e.text) {
                    Some(name) => format!("{{{name}}}"),
                    None => self.line(&e.text),
                },
                ..e.clone()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrub(v: &str) -> Scrub {
        Scrub::new(
            &[("post_text".to_string(), v.to_string())]
                .into_iter()
                .collect(),
        )
    }

    #[test]
    fn whole_values_and_single_lines_are_replaced() {
        let s = scrub("first line here\nsecond");
        assert_eq!(s.text("first line here\nsecond"), "{post_text}");
        assert_eq!(s.text("[e2] first line here"), "[e2] {post_text}");
    }

    #[test]
    fn ui_text_loses_only_long_runs_of_the_value() {
        let s = scrub("Launch day for our tiny robot kit");
        assert_eq!(s.text("Lei · launch day for our tiny"), "Lei · {post_text}");
        assert_eq!(s.text("Reply to tiny robot now"), "Reply to tiny robot now");
        let s = scrub("What's happening at booth 12, come by");
        assert_eq!(s.text("What's happening?"), "What's happening?");
        let s = scrub("今天发布我们的小机器人套件");
        assert_eq!(s.text("看看今天发布我们的小机器人"), "看看{post_text}");
        assert_eq!(s.text("今天发布"), "今天发布");
    }

    #[test]
    fn fragments_count_as_the_value_only_once_it_was_typed() {
        let s = scrub("Launch day for our tiny robot kit");
        assert_eq!(s.value_of("Launch day for our"), None);
        assert_eq!(
            s.value_of("LAUNCH DAY FOR OUR TINY ROBOT KIT"),
            Some("post_text")
        );
        s.mark_typed("post_text");
        assert_eq!(s.value_of("Launch day for our"), Some("post_text"));
        assert_eq!(s.value_of("tiny  robot kit!"), Some("post_text"));
        assert_eq!(s.value_of("kit"), None, "一个短词不算");
        assert_eq!(s.value_of("Post"), None);
        let c = scrub("今天发布我们的小机器人套件");
        c.mark_typed("post_text");
        assert_eq!(c.value_of("今天发布我们"), Some("post_text"));
        assert_eq!(c.value_of("发布"), None);
    }

    #[test]
    fn pieces_are_matched_on_word_boundaries() {
        let s = scrub("Blog in Paris this weekend");
        s.mark_typed("post_text");
        assert_eq!(s.value_of("Log in"), None, "log 不是 blog 的一部分");
        assert_eq!(s.value_of("in Paris"), Some("post_text"));
        let s = scrub("Complete the verification at our booth");
        s.mark_typed("post_text");
        assert_eq!(s.value_of("Verification"), None, "单独一个标题词是界面字");
        let s = scrub("Launch day for our tiny robot kit");
        s.mark_typed("post_text");
        assert_eq!(
            s.value_of("y for our"),
            Some("post_text"),
            "切开的半个词也算"
        );
        assert_eq!(
            s.value_of("Launch dayfor our"),
            Some("post_text"),
            "少了空格"
        );
        assert_eq!(s.value_of("ay f"), None, "盖不住两个完整的词");
        let s = scrub("See https://example.com/launch-day today");
        s.mark_typed("post_text");
        assert_eq!(s.value_of("example.com/launch-day"), Some("post_text"));
        assert_eq!(s.value_of("Login-required"), None);
        assert_eq!(s.value_of("Login-required."), None, "末尾句号不算网址");
        assert_eq!(s.value_of("example.com"), None, "不到 12 个字母");
        let s = scrub("Please log in to see the demo");
        s.mark_typed("post_text");
        assert_eq!(s.value_of("Login"), None, "一个词不是两个词拼起来的");
        assert_eq!(s.value_of("log in"), Some("post_text"));
        let s = scrub("Follow @leining_tech_official for more");
        s.mark_typed("post_text");
        assert_eq!(s.value_of("@leining_tech_official"), Some("post_text"));
    }

    #[test]
    fn a_whole_value_counts_only_on_word_boundaries() {
        let s = scrub("is here");
        assert_eq!(s.value_of("It is here now"), Some("post_text"));
        assert_eq!(s.value_of("This here"), None, "this 里的 is 不算");
        let c = scrub("我们的小机器人");
        assert_eq!(c.value_of("看看我们的小机器人吧"), Some("post_text"));
    }

    #[test]
    fn a_short_value_does_not_swallow_ui_text() {
        let s = scrub("test");
        assert_eq!(s.value_of("Latest posts"), None);
    }

    #[test]
    fn a_line_has_no_control_characters_after_redaction() {
        let s = scrub("two\nlines here");
        assert_eq!(s.line("two\nlines here"), "{post_text}");
        assert_eq!(s.line("Drafts\n[e9] 按钮 Post\r"), "Drafts [e9] 按钮 Post ");
    }
}
