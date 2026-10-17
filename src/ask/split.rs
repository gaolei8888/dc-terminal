//! 断句：喂 token 增量，吐一句一句的话。纯函数，不碰网络和时间。
//!
//! 每句**一凑齐就出**——首句延迟是 dco 最在意的指标，所以不为了「标最后一句」
//! 或者吸收后面的引号去多等一个 token（见设计 §3.3）。代价：句末符后面紧跟的
//! 收引号 / 重复标点会成为下一「句」，它们不含字，清洗后为空，被丢掉。
//!
//! 思考内容（内联 `<think>…</think>`）在这里剥掉，**不外露、不入记忆**；
//! `reasoning` 之类的独立字段在 SSE 解析那一层就不读（`llm::stream`）。

use regex::Regex;
use std::sync::OnceLock;

/// dco `speak` 的上限，字符数。
pub const MAX_SENTENCE_CHARS: usize = 120;

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

/// 句末符。ASCII `.` 不在里面，单独判（`3.5`、`1. 买菜`）。
fn is_terminator(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '!' | '?' | '…' | '；' | '\n')
}

/// 软切点：一句超长又没有句末符时，优先在这里切。
fn is_soft_break(c: char) -> bool {
    matches!(c, '，' | '、' | ',')
}

#[derive(Default)]
pub struct SentenceSplitter {
    /// 还没分类的原始文本尾巴（可能是半个 `<think>` 标签）。
    raw: String,
    in_think: bool,
    /// 正在凑的一句。
    pending: String,
    /// `pending` 末尾是一个还没定论的 ASCII `.`：下一个字符是空白才算句末。
    dot_pending: bool,
}

impl SentenceSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂一段增量，返回这次凑齐的句子（已清洗、非空）。
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.raw.push_str(delta);
        let mut out = Vec::new();
        for c in self.take_visible() {
            self.feed(c, &mut out);
        }
        out
    }

    /// **自然结束**时调用：剩下的尾巴作为最后一句。超时 / 断流造成的结束不要调，
    /// 直接丢掉 splitter——不发半句话。
    pub fn finish(mut self) -> Option<String> {
        // 自然结束时，不完整的 `<think>` 前缀（比如末尾一个 `<`）就是普通文字。
        let rest = if self.in_think { String::new() } else { std::mem::take(&mut self.raw) };
        self.pending.push_str(&rest);
        clean(&self.pending)
    }

    /// 从 `raw` 里取出能确定是「可见文字」的字符，剥掉 think 段。
    fn take_visible(&mut self) -> Vec<char> {
        let mut vis = String::new();
        loop {
            let tag = if self.in_think { CLOSE } else { OPEN };
            if let Some(i) = self.raw.find(tag) {
                if !self.in_think {
                    vis.push_str(&self.raw[..i]);
                }
                self.raw.drain(..i + tag.len());
                self.in_think = !self.in_think;
                continue;
            }
            // 没找到完整标签：末尾若是标签的前缀就留着等下一段，其余可以定论。
            let len = self.raw.len();
            let keep = (1..tag.len())
                .rev()
                .find(|&n| n <= len && self.raw.is_char_boundary(len - n) && tag.starts_with(&self.raw[len - n..]))
                .unwrap_or(0);
            let cut = self.raw.len() - keep;
            if !self.in_think {
                vis.push_str(&self.raw[..cut]);
            }
            self.raw.drain(..cut);
            return vis.chars().collect();
        }
    }

    fn feed(&mut self, c: char, out: &mut Vec<String>) {
        if self.dot_pending {
            self.dot_pending = false;
            if c.is_whitespace() {
                self.emit(out);
            }
        }
        self.pending.push(c);
        if is_terminator(c) {
            self.emit(out);
        } else if c == '.' && !self.pending.trim_end_matches('.').ends_with(|d: char| d.is_ascii_digit()) {
            self.dot_pending = true;
        } else if self.pending.chars().count() >= MAX_SENTENCE_CHARS {
            self.cut_long(out);
        }
    }

    fn emit(&mut self, out: &mut Vec<String>) {
        let s = std::mem::take(&mut self.pending);
        out.extend(clean(&s));
    }

    /// 超长且没有句末符：在已有的最后一个软切点切，没有就硬切。
    fn cut_long(&mut self, out: &mut Vec<String>) {
        let at = self
            .pending
            .char_indices()
            .filter(|&(_, c)| is_soft_break(c))
            .map(|(i, c)| i + c.len_utf8())
            .next_back()
            .unwrap_or(self.pending.len());
        let rest = self.pending.split_off(at);
        self.emit(out);
        self.pending = rest;
    }
}

fn ansi() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // CSI（`ESC [ … 终结字节`）和 OSC（`ESC ] … BEL 或 ESC \`）。
    R.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)").unwrap())
}

fn list_prefix() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\s*(?:[-+•·]|\d+[.)])\s+").unwrap())
}

/// 清成「能念出来」的一句：去 ANSI、控制字符、markdown 标记；没有一个字（字母/数字/
/// 汉字）就返回 `None`——纯标点的「句子」不值得发。
fn clean(s: &str) -> Option<String> {
    let s = ansi().replace_all(s, "");
    let s: String = s.chars().filter(|c| !c.is_control() && !matches!(c, '*' | '#' | '`')).collect();
    let s = list_prefix().replace(s.trim_start_matches(|c: char| c.is_whitespace()), "");
    // 上一句句末符后面紧跟的收引号 / 重复标点，会落在这一句开头。
    let s = s.trim_start_matches(|c: char| "”’」』）)】》!！?？…。".contains(c)).trim();
    s.chars().any(char::is_alphanumeric).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(deltas: &[&str]) -> Vec<String> {
        let mut sp = SentenceSplitter::new();
        let mut out: Vec<String> = deltas.iter().flat_map(|d| sp.push(d)).collect();
        out.extend(sp.finish());
        out
    }

    #[test]
    fn splits_on_cjk_and_ascii_terminators_and_newlines() {
        assert_eq!(run(&["今天挺晴的。出门带伞！", "真的吗?好的\n行"]), ["今天挺晴的。", "出门带伞！", "真的吗?", "好的", "行"]);
    }

    #[test]
    fn a_sentence_comes_out_the_moment_it_is_complete() {
        let mut sp = SentenceSplitter::new();
        assert!(sp.push("今天").is_empty());
        assert_eq!(sp.push("晴。后"), ["今天晴。"]);
    }

    #[test]
    fn full_width_semicolon_ends_a_sentence_ascii_semicolon_does_not() {
        assert_eq!(run(&["甲；乙;丙"]), ["甲；", "乙;丙"]);
    }

    #[test]
    fn a_decimal_point_is_not_a_sentence_end_even_across_deltas() {
        assert_eq!(run(&["版本是3", ".", "5，很新。"]), ["版本是3.5，很新。"]);
    }

    #[test]
    fn an_ascii_period_ends_a_sentence_only_before_whitespace_or_at_the_end() {
        assert_eq!(run(&["It is ok. Then go", "."]), ["It is ok.", "Then go."]);
        assert_eq!(run(&["a.b is fine"]), ["a.b is fine"]);
        // 末尾的 `.` 在流结束时就是句末。
        assert_eq!(run(&["done."]), ["done."]);
    }

    #[test]
    fn ellipsis_ends_a_sentence() {
        assert_eq!(run(&["嗯…让我想想。"]), ["嗯…", "让我想想。"]);
    }

    #[test]
    fn short_sentences_are_not_merged() {
        assert_eq!(run(&["嗯。好。"]), ["嗯。", "好。"]);
    }

    #[test]
    fn punctuation_only_leftovers_are_dropped() {
        // 句末符后紧跟的收引号 / 重复标点单独成「句」，没有字，丢掉。
        assert_eq!(run(&["他说“好。”然后走了！！"]), ["他说“好。", "然后走了！"]);
    }

    #[test]
    fn an_overlong_sentence_is_cut_at_the_last_soft_break() {
        let a = "甲".repeat(70);
        let b = "乙".repeat(49); // 70 + 1 + 49 = 120：到上限时在逗号处切
        let out = run(&[&format!("{a}，{b}"), "丙。"]);
        assert_eq!(out, [format!("{a}，"), format!("{b}丙。")]);
    }

    #[test]
    fn an_overlong_sentence_without_punctuation_is_hard_cut() {
        let out = run(&[&"字".repeat(250)]);
        assert_eq!(out.iter().map(|s| s.chars().count()).collect::<Vec<_>>(), [120, 120, 10]);
        assert!(out.iter().all(|s| s.chars().count() <= MAX_SENTENCE_CHARS));
    }

    #[test]
    fn markdown_ansi_and_control_characters_are_cleaned() {
        assert_eq!(run(&["**好**的，`代码`。\n"]), ["好的，代码。"]);
        assert_eq!(run(&["\x1b[1;31m红\x1b[0m字\x07。"]), ["红字。"]);
        assert_eq!(run(&["\x1b]0;标题\x07正文。"]), ["正文。"]);
        assert_eq!(run(&["- 买菜。\n1. 做饭。\n# 标题\n"]), ["买菜。", "做饭。", "标题"]);
    }

    #[test]
    fn emoji_pass_through_and_a_lone_emoji_is_dropped() {
        assert_eq!(run(&["好呀😀。"]), ["好呀😀。"]);
        assert_eq!(run(&["😀。"]), Vec::<String>::new());
    }

    #[test]
    fn think_blocks_never_come_out_even_when_the_tags_are_split() {
        assert_eq!(run(&["<think>想一想。再想。</think>答案是四。"]), ["答案是四。"]);
        assert_eq!(run(&["<th", "ink>内心戏。</thi", "nk>说出口的。"]), ["说出口的。"]);
        assert_eq!(run(&["前。<think>藏", "起来", "</think>后。"]), ["前。", "后。"]);
    }

    #[test]
    fn an_unterminated_think_block_swallows_the_rest() {
        assert_eq!(run(&["开头。<think>想到一半。"]), ["开头。"]);
    }

    #[test]
    fn a_stray_angle_bracket_is_ordinary_text_at_a_natural_end() {
        assert_eq!(run(&["3 < 4 对吗<"]), ["3 < 4 对吗<"]);
    }

    #[test]
    fn the_natural_tail_is_the_last_sentence_but_a_dropped_splitter_says_nothing() {
        assert_eq!(run(&["第一句。没写完"]), ["第一句。", "没写完"]);
        let mut sp = SentenceSplitter::new();
        assert_eq!(sp.push("第一句。没写完"), ["第一句。"]);
        drop(sp); // 超时 / 断流：尾巴就这样丢掉，没有 API 能把它拿出来
    }

    #[test]
    fn empty_and_whitespace_input_give_nothing() {
        assert_eq!(run(&["", "  ", "\n\n"]), Vec::<String>::new());
    }
}
