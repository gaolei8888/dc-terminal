//! 看一眼画面：元素分成值元素和界面元素（见 `scrub.rs`），脱敏、控制字符压成空格，
//! 算画面指纹和参数在不在画面上；选项按原文过规矩（见 `raw.rs`）。
use super::raw::Raw;
pub(crate) use super::scrub::Scrub;
use super::ExploreInput;
use crate::gate::{fold, Halt};
use crate::hands::{El, Hands, Look};
use crate::options::{all_options, Act, Goal, Opt, Progress};
use std::collections::{BTreeMap, HashMap};

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
    let raw_els = Raw::new(&raw.elements, scrub);
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

    /// 选项：显示用脱敏后的字，能不能点按原文判断（值元素、禁点词、档位）；意图名里带参数占位符的
    /// 点击不进选项（那等于把要发的内容当按钮名交给 dco、写进流程）。还没填好的文字参数要是这一屏
    /// 没有填字选项（发帖框里还剩半截帖子，是值元素），就用 `fills` 里上一次起作用的那个。
    pub(super) fn options(
        &self,
        goal: &Goal,
        prog: &Progress,
        scrub: &Scrub,
        on: &dyn Fn(&str) -> bool,
        fills: &HashMap<String, Opt>,
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
        let files_done = goal.file_params.iter().all(|p| prog.attached.contains(p));
        let has_fill = |all: &[Opt], p: &str| {
            all.iter()
                .any(|o| matches!(&o.act, Act::Fill { param, .. } if param == p))
        };
        for p in goal.text_params.iter().filter(|p| !prog.filled.contains(p)) {
            if let Some(f) = fills.get(p).filter(|_| files_done && !has_fill(&all, p)) {
                let at = all
                    .iter()
                    .position(|o| {
                        matches!(
                            o.act,
                            Act::Scroll { .. } | Act::Wait | Act::Done | Act::Stuck
                        )
                    })
                    .unwrap_or(all.len());
                if !all.iter().any(|o| o.key == f.key) {
                    all.insert(at, f.clone());
                }
            }
        }
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
    #[test]
    fn values_on_screen_wants_most_words_of_the_first_line() {
        let v = "one two three four five\nsecond line";
        assert!(values_on_screen(v, &[el("ONE two three four")]));
        assert!(!values_on_screen(v, &[el("one two three")]));
        assert!(values_on_screen("hi there", &[el("hi"), el("there")]));
        assert!(!values_on_screen("hi there", &[el("hi")]));
        assert!(!values_on_screen("", &[el("x")]));
    }
}
