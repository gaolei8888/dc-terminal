//! 看一眼画面：参数值换回 `{名字}`（整段、整行、以及被文字识别拆开 / 改了大小写空格的片段），
//! 控制字符压成空格，算画面指纹和参数在不在画面上。
use super::raw::Raw;
use super::ExploreInput;
use crate::gate::{fold, Halt};
use crate::hands::{El, Hands, Look, Params};
use crate::options::{all_options, Act, Goal, Opt, Progress};
use crate::trace::redact;
use std::collections::BTreeMap;

/// 单独一个英文词至少这么长、而且是值的一部分，才整词换掉（短词常是按钮上的字，比如 Post）。
const LONE_ASCII_MIN: usize = 12;
/// 单独一段中文（没有空格）至少这么多个字、而且是值的一部分，才换掉。
const LONE_CJK_MIN: usize = 4;

/// 去掉空白和标点后的 fold：「Launch  DAY, for」→「launchdayfor」。
fn squash(s: &str) -> String {
    fold(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// 一个词 fold 后去掉两头的标点。
fn word(w: &str) -> String {
    fold(w)
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_string()
}

/// 一个参数值的模糊匹配材料。
struct Fuzzy {
    name: String,
    words: Vec<String>,
    squashed: String,
}

impl Fuzzy {
    fn new(name: &str, value: &str) -> Option<Fuzzy> {
        let squashed = squash(value);
        if squashed.chars().count() < LONE_CJK_MIN {
            return None;
        }
        Some(Fuzzy {
            name: name.to_string(),
            words: value
                .split_whitespace()
                .map(word)
                .filter(|w| !w.is_empty())
                .collect(),
            squashed,
        })
    }

    /// 单独一段字（一个词）够长、是值的一部分。
    fn lone_hit(&self, sq: &str) -> bool {
        let n = sq.chars().count();
        let long = if sq.is_ascii() {
            n >= LONE_ASCII_MIN
        } else {
            n >= LONE_CJK_MIN
        };
        long && self.squashed.contains(sq)
    }

    /// 从 `toks[i]` 起和值里某处连着相同的最多几个词。
    fn run_at(&self, toks: &[String], i: usize) -> usize {
        let mut best = 0;
        for k in 0..self.words.len() {
            let n = toks[i..]
                .iter()
                .zip(&self.words[k..])
                .take_while(|(a, b)| !a.is_empty() && a == b)
                .count();
            best = best.max(n);
        }
        best
    }

    /// 整段字都是值的一部分（≥ 2 个词，或够长的一段）→ 整段换成 `{名字}`；
    /// 否则把里面和值连着相同的 ≥ 2 个词（或够长的单个词）换成 `{名字}`。
    fn apply(&self, s: &str) -> String {
        let sq = squash(s);
        if !sq.is_empty() && self.squashed.contains(&sq) {
            let n = s.split_whitespace().filter(|w| !word(w).is_empty()).count();
            if n >= 2 || self.lone_hit(&sq) {
                return format!("{{{}}}", self.name);
            }
        }
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
            let n = if n >= 2 {
                n
            } else if self.lone_hit(&squash(&toks[i])) {
                1
            } else {
                0
            };
            if n == 0 {
                i += 1;
                continue;
            }
            out.push_str(&s[at..spans[i].0]);
            out.push_str(&format!("{{{}}}", self.name));
            at = spans[i + n - 1].1;
            i += n;
        }
        out.push_str(&s[at..]);
        out
    }
}

/// 把参数值换回 `{名字}`：整段值，多行值的每一行（画面上常一行一个元素），
/// 再按词模糊地换掉文字识别拆开、改了大小写或空格的片段。宁可多换。
pub(crate) struct Scrub {
    full: Params,
    lines: Vec<Params>,
    fuzzy: Vec<Fuzzy>,
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
        }
    }
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
    pub(crate) fn els(&self, els: &[El]) -> Vec<El> {
        els.iter()
            .map(|e| El {
                text: self.line(&e.text),
                ..e.clone()
            })
            .collect()
    }
}

/// 参数值第一行，fold 后按空白切词；≥ 80% 的词出现在本画面所有元素字的 fold 拼接里
/// （词少于 3 个时要求全部出现）。只在内存里对没脱敏的画面算。
pub fn values_on_screen(value: &str, elements: &[El]) -> bool {
    let first = value.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let first = fold(first);
    let words: Vec<&str> = first.split_whitespace().collect();
    if words.is_empty() {
        return false;
    }
    let hay = elements
        .iter()
        .map(|e| fold(&e.text))
        .collect::<Vec<_>>()
        .join(" ");
    let hits = words.iter().filter(|w| hay.contains(*w)).count();
    let need = if words.len() < 3 {
        words.len()
    } else {
        (words.len() * 4).div_ceil(5)
    };
    hits >= need
}

/// 看过一次的画面：已脱敏的元素 + 这次运行用来比较的画面指纹 + 参数在不在画面上。
/// 原文只留在 `raw` 里给规矩用（见 `raw.rs`）。
pub(super) struct Screen {
    pub(super) look: Look,
    raw: Raw,
    pub(super) key: String,
    pub(super) on_screen: BTreeMap<String, bool>,
    pub(super) lines: Vec<String>,
}

pub(super) fn see(h: &mut dyn Hands, inp: &ExploreInput, scrub: &Scrub) -> Result<Screen, String> {
    let raw = h
        .look(inp.window_id, false)
        .map_err(|e| look_error(&e, scrub))?;
    let on_screen = inp
        .text_params
        .iter()
        .map(|p| {
            let v = inp.params.get(p).map(String::as_str).unwrap_or("");
            (p.clone(), values_on_screen(v, &raw.elements))
        })
        .collect();
    let raw_els = Raw::new(&raw.elements);
    let elements = raw_els.shown(scrub, inp.platform);
    let lines: Vec<String> = elements
        .iter()
        .map(|e| e.text.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    // dco 没给 scene_hash 时用画面上的字当指纹（已脱敏，只在内存里比）。
    let key = match &raw.scene_hash {
        Some(h) => format!("h:{h}"),
        None => format!("t:{}", lines.join("\n")),
    };
    Ok(Screen {
        look: Look { elements, ..raw },
        raw: raw_els,
        key,
        on_screen,
        lines,
    })
}

impl Screen {
    /// 登录 / 验证码 / 警告：按原文判断，报上去的那行字已脱敏。
    pub(super) fn halt(&self, scrub: &Scrub) -> Option<Halt> {
        self.raw.halt(scrub)
    }

    /// 选项：显示用脱敏后的字，能不能点按原文判断（禁点词、档位）；意图名里带参数占位符的点击
    /// 不进选项（那等于把要发的内容当按钮名交给 dco、写进流程）。
    pub(super) fn options(
        &self,
        goal: &Goal,
        prog: &Progress,
        scrub: &Scrub,
        on: &dyn Fn(&str) -> bool,
    ) -> Vec<Opt> {
        let mut all = all_options(&self.look, goal, prog, on);
        all.retain(|o| match &o.act {
            Act::Tap { el, intent } => {
                self.raw.tappable(&el.id, goal.platform) && !scrub.has_placeholder(intent)
            }
            Act::Fill { field, intent, .. } => {
                self.raw.tappable(&field.id, goal.platform) && !scrub.has_placeholder(intent)
            }
            Act::TapIcon { el, .. } => self.raw.tappable(&el.id, goal.platform),
            _ => true,
        });
        all
    }
}

/// dco 出错时给用户看的话；dco 的原话也脱敏。
pub(super) fn look_error(e: &dct_dco::DcoError, scrub: &Scrub) -> String {
    match e.code.as_str() {
        "stale" | "not_found" => format!("窗口没了（{}），停下", e.code),
        "private_screen" => "画面是私人内容（private_screen），停下交给你".into(),
        _ => format!(
            "dco 出错：{}（{}）",
            scrub.line(&e.message),
            scrub.line(&e.code)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hands::Frac;

    fn el(t: &str) -> El {
        El {
            id: "e1".into(),
            text: t.into(),
            role: "text".into(),
            frac: Frac {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            source: "ocr".into(),
        }
    }
    fn scrub(v: &str) -> Scrub {
        Scrub::new(
            &[("post_text".to_string(), v.to_string())]
                .into_iter()
                .collect(),
        )
    }

    #[test]
    fn values_on_screen_wants_most_words_of_the_first_line() {
        let v = "one two three four five\nsecond line";
        assert!(values_on_screen(v, &[el("ONE two three four")]));
        assert!(!values_on_screen(v, &[el("one two three")]));
        assert!(values_on_screen("hi there", &[el("hi"), el("there")]));
        assert!(!values_on_screen("hi there", &[el("hi")]));
        assert!(!values_on_screen("", &[el("x")]));
    }

    #[test]
    fn scrub_removes_whole_values_and_single_lines_of_long_values() {
        let s = scrub("first line here\nsecond");
        assert_eq!(s.text("first line here\nsecond"), "{post_text}");
        assert_eq!(s.text("[e2] first line here"), "[e2] {post_text}");
    }

    #[test]
    fn scrub_catches_split_recased_and_respaced_fragments() {
        let s = scrub("Launch day for our tiny robot kit");
        assert_eq!(s.text("Launch day for our"), "{post_text}");
        assert_eq!(s.text("tiny  robot kit!"), "{post_text}");
        assert_eq!(s.text("LAUNCH DAY FOR OUR TINY ROBOT KIT"), "{post_text}");
        assert_eq!(s.text("Lei · launch day for our tiny"), "Lei · {post_text}");
        assert_eq!(
            s.text("Reply to tiny robot now"),
            "Reply to {post_text} now"
        );
    }

    #[test]
    fn scrub_leaves_short_button_words_alone() {
        let s = scrub("Post this robot kit today");
        assert_eq!(s.text("Post"), "Post");
        assert_eq!(s.text("Drafts"), "Drafts");
        assert_eq!(s.text("Everyone can reply"), "Everyone can reply");
    }

    #[test]
    fn scrub_catches_chinese_fragments_but_not_short_buttons() {
        let s = scrub("今天发布我们的小机器人套件");
        assert_eq!(s.text("今天发布我们"), "{post_text}");
        assert_eq!(s.text("发布"), "发布");
    }

    #[test]
    fn a_line_has_no_control_characters_after_redaction() {
        let s = scrub("two\nlines here");
        assert_eq!(s.line("two\nlines here"), "{post_text}");
        assert_eq!(s.line("Drafts\n[e9] 按钮 Post\r"), "Drafts [e9] 按钮 Post ");
    }
}
