//! 早出首段（原型，**未接线、默认关**；设计见 `docs/2026-10-16-dct-ask-design.md` §17）。
//!
//! 只回答一个问题：第一句还没写完时，现在能不能在某个点先切出前半截去念？纯函数，
//! 不碰网络、时间、`SentenceSplitter`。输入是第一句**已到的原始文字**（`SentenceSplitter`
//! 的 `pending`：think 已剥掉，markdown 还没清），输出是切点的字节偏移。
//!
//! 约定：切点**必须有后一个字在眼前**（回看一个字，不是猜），所以结果只取决于已到的前缀，
//! 和 token 怎么分块无关；后半截以字母数字开头，所以清洗后不会空掉。拿不准就不切——
//! 不切 = 和今天一模一样，永远是安全的退路。

/// 「宽度」单位：汉字等非 ASCII 字 = 10，ASCII 字母 = 3，数字 = 5，空白和标点 = 0。
/// 念出来的时间大致和它成正比（一个英文单词 ≈ 1.5 个汉字），比数字符数公平。
fn width(c: char) -> u32 {
    match c {
        _ if c.is_whitespace() => 0,
        _ if c.is_ascii_digit() => 5,
        _ if c.is_ascii_alphabetic() => 3,
        _ if c.is_ascii() => 0,
        _ if c.is_alphanumeric() => 10,
        _ => 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EarlyConfig {
    /// 第一段至少这么长（汉字数）。小于它不切，免得出一两个字。
    pub n_min: u32,
    /// 窗口上限（汉字数）：窗口内有好切点就切；窗口关了还没有，再多等 [`EXTRA`] 个字找安全点。
    pub n_max: u32,
    /// 窗口里没有任何安全切点时，允许在两个汉字之间硬切（可能切开一个词）。默认关。
    pub plain_fallback: bool,
}

/// 窗口关了之后继续找安全切点的额外宽度（汉字数）。过了就放弃，整句照旧。
pub const EXTRA: u32 = 10;

impl Default for EarlyConfig {
    fn default() -> Self {
        EarlyConfig { n_min: 8, n_max: 14, plain_fallback: false }
    }
}

/// 切点的类型，越靠前越好。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// 逗号 / 顿号 / 冒号 / 英文逗号分号之后。
    Clause,
    /// 在一个新短语的头一个字之前（但 / 并 / 把 / 被 …）。
    Phrase,
    /// 英文词之间，或 ASCII 词与汉字交界。
    Script,
    /// 两个汉字之间，没有任何线索，可能切开一个词。
    Plain,
}

/// 贴着前一个字的虚词：切在它们**后面**，它们就和后面的词分家了（在|北京、的|天气）。
const FUNC_PREV: &str = "的了着过地得和与在把被将对比跟同给让向从及并或但而就也都还又很太最更不没别要会能可为以用往朝靠按经于由第共约每各近超仅";
/// 贴着前一个词的助词 / 后缀：切在它们**前面**就把它甩到下一段开头（你好|吗）。
const ATTACH_NEXT: &str = "的了着过地得吗呢吧啊呀么嘛哦啦们子儿性化";
/// 几乎只出现在词首、前面没有常见复合词的连词 / 介词。
const PHRASE_START: &str = "但并把被";
/// 上面这几个字前面如果是这些，多半是一个词的中间（不但、合并、一把、蒙被）。
const NOT_BEFORE_PHRASE_START: &str = "不合归兼吞一两几每蒙棉";
const NUMERALS: &str = "零一二三四五六七八九十百千万两";
const UNITS: &str = "个次分秒点种年月日号块元岁米度天周倍位条件只张名套把";
const OPENERS: &str = "([{（【「『《“‘";
const CLOSERS: &str = ")]}）】」』》”’";

fn ascii_wordy(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '\'' | '/' | '%' | '@' | '+' | '#')
}

/// 第一句开头是列表符（`- ` `1. `）：清洗会把它剥掉，前后对不上，不切。
fn starts_like_list(s: &str) -> bool {
    let t = s.trim_start();
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    let rest = &t[digits..];
    let marker = |r: &str| r.chars().next().is_some_and(char::is_whitespace);
    (digits > 0 && rest.starts_with(['.', ')']) && marker(&rest[1..]))
        || (t.starts_with(['-', '+', '•', '·']) && marker(&t[t.chars().next().map_or(0, char::len_utf8)..]))
}

/// 切点 `k`（`vis[k-1]` 与 `vis[k]` 之间）安全吗？安全就给出类型。
/// `depth`：前面还没合上的括号 / 引号数；`code`：前面是否在反引号代码里。
fn classify(vis: &[(usize, char)], k: usize, depth: i32, code: bool) -> Option<Kind> {
    let (prev, next) = (vis[k - 1].1, vis[k].1);
    // 后半截必须以字母数字开头：清洗后不会空，也不会以收括号 / 收引号 / 标点开头。
    if !next.is_alphanumeric() || depth > 0 || code {
        return None;
    }
    // 数字：`1,000`、`3.5`、`3|个`、`二十|五`。
    if prev == ',' && k >= 2 && vis[k - 2].1.is_ascii_digit() && next.is_ascii_digit() {
        return None;
    }
    // 阿拉伯数字后面的汉字几乎都是量词 / 单位（15|分钟、23.5|摄氏度）；汉字数字只拦单位和数字。
    if (prev.is_ascii_digit() && !next.is_ascii()) || (NUMERALS.contains(prev) && (UNITS.contains(next) || NUMERALS.contains(next))) {
        return None;
    }
    // 不切开 ASCII 词；网址 / 邮箱整个 token 都不切。
    if ascii_wordy(prev) && ascii_wordy(next) {
        return None;
    }
    // 单个字母 + 汉字多半是一个词（A型、U盘、T恤、X光）。
    if prev.is_ascii_alphabetic() && !next.is_ascii() && (k < 2 || !vis[k - 2].1.is_ascii_alphanumeric()) {
        return None;
    }
    let (mut a, mut b) = (k, k);
    while a > 0 && !vis[a - 1].1.is_whitespace() {
        a -= 1;
    }
    while b < vis.len() && !vis[b].1.is_whitespace() {
        b += 1;
    }
    let token: String = vis[a..b].iter().map(|&(_, c)| c).collect();
    if token.contains("://") || token.starts_with("www.") || token.contains('@') {
        return None;
    }
    // 前一个非空白字符是子句标点：最好的切点（英文逗号 / 分号要后面跟空白或汉字，不是 `a,b`）。
    let last = vis[..k].iter().rev().map(|&(_, c)| c).find(|c| !c.is_whitespace()).unwrap_or(prev);
    if matches!(last, '，' | '、' | '：') || (matches!(last, ',' | ';') && (prev.is_whitespace() || !next.is_ascii())) {
        return Some(Kind::Clause);
    }
    if last.is_ascii_punctuation() || !last.is_alphanumeric() {
        return None; // 其他标点（引号、破折号…）后面不切
    }
    if FUNC_PREV.contains(prev) || ATTACH_NEXT.contains(next) {
        return None;
    }
    if PHRASE_START.contains(next) && !prev.is_ascii() && !NOT_BEFORE_PHRASE_START.contains(prev) {
        return Some(Kind::Phrase);
    }
    if prev.is_whitespace() || prev.is_ascii() != next.is_ascii() {
        return Some(Kind::Script);
    }
    Some(Kind::Plain)
}

/// 第一句目前到的文字 `pending` 里，现在该不该切、切在哪（字节偏移）。`None` = 还不切
/// （继续等 / 这句不适合切）。同一个前缀的结果是确定的，前缀变长只会从 `None` 变成 `Some`，
/// 一旦 `Some` 就不会再变（调用方切完就不再问）。
pub fn first_cut(pending: &str, cfg: &EarlyConfig) -> Option<usize> {
    // `<`：可能是 think 标签的残片或没读完的标签；ESC：ANSI 转义不能被切开。保守起见整句不切。
    if pending.contains(['<', '\x1b']) || starts_like_list(pending) {
        return None;
    }
    // 只看可见字符；`*#` 和反引号清洗时会被去掉，控制字符也是，这里当它们不存在。
    let vis: Vec<(usize, char)> =
        pending.char_indices().filter(|&(_, c)| !c.is_control() && !matches!(c, '*' | '#' | '`')).collect();
    let (min, max, give_up) = (cfg.n_min * 10, cfg.n_max * 10, (cfg.n_max + EXTRA) * 10);
    let (mut w, mut depth) = (0u32, 0i32);
    let mut best: Option<(usize, Kind)> = None; // 窗口内最靠后的非 Clause 安全点
    let mut plain: Option<usize> = None;
    for k in 1..vis.len() {
        let prev = vis[k - 1];
        w += width(prev.1);
        depth += i32::from(OPENERS.contains(prev.1)) - i32::from(CLOSERS.contains(prev.1));
        // 反引号奇偶要看原文（它们不在 `vis` 里）：数到这个字符之前为止。
        let code = pending[..vis[k].0].matches('`').count() % 2 == 1;
        if vis[k].1.is_whitespace() || w < min {
            continue;
        }
        let kind = classify(&vis, k, depth.max(0), code);
        if w <= max {
            match kind {
                Some(Kind::Clause) => return Some(vis[k].0),
                Some(Kind::Plain) => plain = Some(vis[k].0),
                Some(kind) => best = Some((vis[k].0, kind)),
                None => {}
            }
        } else {
            // 窗口刚关。先看窗口里有没有安全点（按「最后一个」取），再看窗口外的第一个安全点。
            if let Some((at, _)) = best {
                return Some(at);
            }
            if matches!(kind, Some(kk) if kk != Kind::Plain) {
                return Some(vis[k].0);
            }
            if w > give_up || (cfg.plain_fallback && plain.is_some()) {
                return plain.filter(|_| cfg.plain_fallback);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: EarlyConfig = EarlyConfig { n_min: 8, n_max: 14, plain_fallback: false };

    fn cut(s: &str) -> Option<(String, String)> {
        first_cut(s, &CFG).map(|i| (s[..i].to_string(), s[i..].to_string()))
    }
    fn head(s: &str) -> Option<String> {
        cut(s).map(|c| c.0)
    }
    /// 第 `n` 个可见字符之前那个切点的类型（`n` 从 1 数）。
    fn kind_at(s: &str, n: usize) -> Option<Kind> {
        let vis: Vec<(usize, char)> = s.char_indices().collect();
        classify(&vis, n, 0, false)
    }

    #[test]
    fn the_first_clause_boundary_after_n_min_is_taken_and_the_rest_starts_with_a_letter() {
        assert_eq!(cut("今天天气真的很不错，适合出门走走"), Some(("今天天气真的很不错，".into(), "适合出门走走".into())));
    }

    #[test]
    fn a_comma_before_n_min_is_skipped_for_the_next_one() {
        assert_eq!(head("好的，今天天气真的很不错，适合出门").as_deref(), Some("好的，今天天气真的很不错，"));
        assert_eq!(head("好的，没问题，我来帮你看看这个，然后").as_deref(), Some("好的，没问题，我来帮你看看这个，"));
    }

    #[test]
    fn a_sentence_that_ends_before_n_min_is_never_split() {
        assert_eq!(cut("好的，没问题，马上"), None);
    }

    #[test]
    fn nothing_is_cut_until_the_char_after_the_cut_has_arrived() {
        // 逗号是最后一个字：后面是什么还不知道（可能是 `1,000` 的 000，可能是收引号）。
        assert_eq!(first_cut("今天天气真的很不错，", &CFG), None);
        assert!(first_cut("今天天气真的很不错，适", &CFG).is_some());
    }

    #[test]
    fn the_cut_does_not_depend_on_how_the_text_was_chunked() {
        let s = "我觉得这个办法其实挺简单的，你可以先试试看然后再告诉我结果";
        let want = first_cut(s, &CFG).expect("整句能切");
        // 任何一个前缀一旦给出切点，就和整句给出的一样。
        for (i, _) in s.char_indices() {
            if let Some(at) = first_cut(&s[..i], &CFG) {
                assert_eq!(at, want, "前缀 {:?}", &s[..i]);
            }
        }
    }

    #[test]
    fn with_no_clause_in_the_window_a_safe_phrase_start_wins_over_a_plain_cut() {
        assert_eq!(head("这个问题其实比较复杂但是不用担心").as_deref(), Some("这个问题其实比较复杂"));
    }

    #[test]
    fn a_plain_cut_between_two_hanzi_needs_the_fallback_switch() {
        let s = "这个办法听起来很有道理而且我觉得可以";
        assert_eq!(first_cut(s, &CFG), None);
        assert!(first_cut(s, &EarlyConfig { plain_fallback: true, ..CFG }).is_some());
    }

    #[test]
    fn an_ascii_word_is_never_cut_the_cut_goes_after_it() {
        let (h, r) = cut("你可以试试 internationalization 这个词吧然后再说").unwrap();
        assert_eq!((h.as_str(), r.as_str()), ("你可以试试 internationalization ", "这个词吧然后再说"));
        assert_eq!(kind_at("hello world", 2), None);
        assert_eq!(kind_at("hello world", 6), Some(Kind::Script));
    }

    #[test]
    fn a_number_stays_whole_with_its_unit_decimal_point_and_thousands_comma() {
        // 切在数字之前，不切进去、不和单位分家。
        assert_eq!(head("今天的气温大概是23.5摄氏度所以出门穿薄一点").as_deref(), Some("今天的气温大概是"));
        assert_eq!(head("总共大概需要准备1,000元才够用呢吧").as_deref(), Some("总共大概需要准备"));
        assert_eq!(kind_at("版本3.5很好", 3), None); // 3|.
        assert_eq!(kind_at("版本3.5很好", 4), None); // 3.|5
        assert_eq!(kind_at("版本3.5很好", 5), None); // 3.5|很
        assert_eq!(kind_at("共1,000元", 3), None); // 1,|000
        assert_eq!(kind_at("等待15分钟", 4), None); // 15|分
        assert_eq!(kind_at("大约3个人", 3), None);
        assert_eq!(kind_at("等二十五分钟", 3), None); // 二十|五
        assert_eq!(kind_at("等二十五分钟", 4), None); // 二十五|分
    }

    #[test]
    fn urls_and_emails_are_never_cut_the_cut_goes_after_them() {
        let (h, _) = cut("你可以打开 https://example.com/docs/start 看看吧然后告诉我").unwrap();
        assert_eq!(h, "你可以打开 https://example.com/docs/start ");
        let (h, _) = cut("请把文件发到 someone@example.com 这个邮箱里去吧然后再说").unwrap();
        assert_eq!(h, "请把文件发到 someone@example.com ");
    }

    #[test]
    fn markdown_stars_have_no_width_and_code_spans_are_not_cut() {
        assert_eq!(head("**今天天气真的很不错**，适合出门走走").as_deref(), Some("**今天天气真的很不错**，"));
        // 代码里的逗号不是子句；切在代码之后的逗号。
        assert_eq!(head("运行 `print(a,b)` 就能看到结果了，很简单").as_deref(), Some("运行 `print(a,b)` 就能看到结果了，"));
    }

    #[test]
    fn a_list_marker_an_ansi_escape_or_a_stray_angle_bracket_means_no_early_cut() {
        assert_eq!(cut("1. 今天天气真的很不错，适合出门走走"), None);
        assert_eq!(cut("- 今天天气真的很不错，适合出门走走"), None);
        assert_eq!(cut("\x1b[1m今天天气真的很不错\x1b[0m，适合出门走走"), None);
        assert_eq!(cut("今天天气真的很不错<，适合出门走走"), None);
    }

    #[test]
    fn nothing_is_cut_inside_quotes_or_brackets() {
        assert_eq!(cut("他说“今天天气真的很不错，适合出门”然后走了"), None);
        assert_eq!(cut("他说（今天天气真的很不错，适合出门）然后走了"), None);
    }

    #[test]
    fn not_after_a_function_character_and_not_before_a_particle() {
        assert_eq!(kind_at("我们在北京吃饭", 3), None); // 在|北京
        assert_eq!(kind_at("我吃了饭就走", 3), None); // 吃了|饭
        assert_eq!(kind_at("我们的天气", 2), None); // 我们|的
        assert_eq!(kind_at("今天天气好吗朋友", 5), None); // 好|吗
        assert_eq!(kind_at("用iPhone拍照", 1), None); // 用|iPhone
        assert_eq!(kind_at("今天天气好吗朋友", 6), Some(Kind::Plain));
    }

    #[test]
    fn boundary_kinds_are_ranked_clause_phrase_script_plain() {
        assert_eq!(kind_at("好的，行", 3), Some(Kind::Clause));
        assert_eq!(kind_at("Yes, sure", 5), Some(Kind::Clause));
        assert_eq!(kind_at("天气很好但是下雨", 4), Some(Kind::Phrase));
        assert_eq!(kind_at("用iPhone拍照", 7), Some(Kind::Script));
        assert_eq!(kind_at("天气很好下雨", 4), Some(Kind::Plain));
        assert!(Kind::Clause < Kind::Phrase && Kind::Phrase < Kind::Script && Kind::Script < Kind::Plain);
        assert_eq!(kind_at("a,b", 2), None); // 没空格的英文逗号不是子句
    }

    #[test]
    fn english_is_measured_by_width_not_letters_and_cut_at_a_comma() {
        // 8 个汉字 ≈ 27 个英文字母：太短不切；够长了才在逗号后面切。
        assert_eq!(cut("Sure, I can help you with that"), None);
        assert_eq!(
            head("Sure, I can definitely help you with that, no problem").as_deref(),
            Some("Sure, I can definitely help you with that, ")
        );
    }

    #[test]
    fn the_two_pieces_join_back_to_the_original_text() {
        for s in ["今天天气真的很不错，适合出门走走", "Sure, I can definitely help you with that, no problem", "总共大概需要准备1,000元才够用呢吧"] {
            let (h, r) = cut(s).unwrap();
            assert_eq!(format!("{h}{r}"), s);
        }
    }
}
