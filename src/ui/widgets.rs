use ratatui::prelude::*;

use crate::pty::{ScreenColor, ScreenSpan, ScreenStyle};
use crate::session::SessionState;

use super::DIM;

pub fn status_label(s: SessionState) -> &'static str {
    match s {
        SessionState::Working => "干活中",
        SessionState::Asking => "等你回答",
        SessionState::Idle => "空闲",
        SessionState::Stopped => "已停止",
        SessionState::Unknown => "—",
    }
}

pub fn status_color(s: SessionState) -> Color {
    match s {
        SessionState::Working => Color::Cyan,
        SessionState::Asking => Color::Yellow,
        SessionState::Idle => Color::Green,
        SessionState::Stopped => DIM,
        SessionState::Unknown => DIM,
    }
}

/// 底部状态栏要显示的一句话。`error` 决定它是灰字还是红字——
/// 出错和成功用同一种颜色，用户分不出刚才那步到底成没成。
pub struct Msg {
    pub text: String,
    pub error: bool,
}

impl Msg {
    pub fn err(text: String) -> Msg {
        Msg { text, error: true }
    }
}

impl From<&str> for Msg {
    fn from(s: &str) -> Msg {
        Msg {
            text: s.to_string(),
            error: false,
        }
    }
}

impl From<String> for Msg {
    fn from(text: String) -> Msg {
        Msg { text, error: false }
    }
}

fn to_color(c: ScreenColor) -> Option<Color> {
    match c {
        ScreenColor::Default => None,
        ScreenColor::Idx(i) => Some(Color::Indexed(i)),
        ScreenColor::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

fn to_style(s: &ScreenStyle) -> Style {
    let mut st = Style::default();
    if let Some(c) = to_color(s.fg) {
        st = st.fg(c);
    }
    if let Some(c) = to_color(s.bg) {
        st = st.bg(c);
    }
    let mut m = Modifier::empty();
    if s.bold {
        m |= Modifier::BOLD;
    }
    if s.italic {
        m |= Modifier::ITALIC;
    }
    if s.underline {
        m |= Modifier::UNDERLINED;
    }
    if s.inverse {
        m |= Modifier::REVERSED;
    }
    st.add_modifier(m)
}

/// agent 屏幕的样式化内容转成 ratatui 的行。丢掉样式的话 Claude Code
/// 那种靠颜色区分的输出会退化成一片单色，基本没法看。
pub(crate) fn screen_to_lines(screen: &[Vec<ScreenSpan>]) -> Vec<Line<'static>> {
    screen
        .iter()
        .map(|row| {
            Line::from(
                row.iter()
                    .map(|sp| Span::styled(sp.text.clone(), to_style(&sp.style)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// 一个字符在等宽终端里占几列。CJK/全角字符占两列，其余占一列。
///
/// 这里必须用 Unicode 的正式宽度表，不能拿「码位大于 U+1100 就算两列」
/// 糊弄：制表符（`─ │ ╭ ╰`，U+2500 段）、省略号 `…`、箭头这些码位都在
/// U+1100 之上，实际只占一列。agent 画的边框全是这类字符，按两列算等于
/// 把一行的宽度算成两倍，裁到一半就断了。
///
/// `truncate`/`pad_to`/九宫格的 `crop_line` 共用这一份定义——裁的地方和
/// 补空格的地方对「宽」的理解不能分叉，否则列会漂。
pub(crate) fn char_width(ch: char) -> usize {
    // 控制字符没有宽度（`width()` 返回 None），当零列算：它们本来就不占格子。
    unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)
}

fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// 按显示宽度截断，超出的用 … 收尾。看板一行放不下就裁，不能让它换行把表格冲乱。
pub(crate) fn truncate(s: &str, max: usize) -> String {
    let mut w = 0;
    let mut out = String::new();
    for ch in s.chars() {
        let cw = char_width(ch);
        if w + cw > max {
            out.push('…');
            return out;
        }
        w += cw;
        out.push(ch);
    }
    out
}

/// 按显示宽度右补空格，对齐到 `width` 列。不能用 `format!("{:<N}")`——
/// 那是按字符数补的，中文字符占两列却只算一个字符，中英文标签混排时
/// 后面的列就会跟着漂移（`命令行` 3 个字符 6 列，`Claude` 6 个字符也是
/// 6 列，`{:<14}` 会让前者多出 3 格空白）。agent 选择器这一屏中英文
/// 标签常年混着出现（`Claude`/`Codex` 和 `命令行`），不是边角情况。
pub(crate) fn pad_to(s: &str, width: usize) -> String {
    let mut out = s.to_string();
    out.push_str(&" ".repeat(width.saturating_sub(display_width(s))));
    out
}

/// 把 $HOME 缩成 ~，界面上路径太长会被裁掉。
pub(crate) fn short_path(p: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && p.starts_with(&h) => format!("~{}", &p[h.len()..]),
        _ => p.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_to_aligns_cjk_and_ascii_labels_to_the_same_display_width() {
        // 回归测试，对应审查发现「中英混排的列对不齐」：`format!("{:<N}")`
        // 按字符数补空格，中文字符占两列却只算一个字符，`命令行`（3 字符）
        // 和 `Claude`（6 字符）补到同样的字符数时，显示宽度会差 3 列。
        // agent 选择器里这两种标签常年混排，不是边角情况。
        let cjk = pad_to("命令行", 14);
        let ascii = pad_to("Claude", 14);
        assert_eq!(
            display_width(&cjk),
            display_width(&ascii),
            "CJK 标签和 ASCII 标签补齐后显示宽度必须相等：{cjk:?} vs {ascii:?}"
        );
        assert_eq!(display_width(&cjk), 14);
        assert_eq!(display_width(&ascii), 14);
    }

    #[test]
    fn pad_to_never_shrinks_a_string_already_at_or_over_width() {
        // saturating_sub 保底：显示宽度已经达到/超过目标时不能倒扣出负数
        // 长度导致 panic（`" ".repeat()` 拿到下溢的 usize 会直接崩）。
        assert_eq!(pad_to("一二三四五六七", 10), "一二三四五六七");
        assert_eq!(pad_to("abc", 2), "abc");
    }

    #[test]
    fn box_drawing_and_ellipsis_are_one_column_wide() {
        // 回归测试：早年的 char_width 是「码位 > U+1100 就算两列」，制表符
        // （agent 画的提示框边框）和省略号被算成双宽，一行的宽度算成两倍，
        // 九宫格的 crop_line 会把边框裁掉一半。
        for ch in ['─', '│', '╭', '╰', '…', '→', '▶'] {
            assert_eq!(char_width(ch), 1, "{ch:?} 在终端里只占一列");
        }
        // CJK 仍然是两列，这才是这个函数存在的理由
        assert_eq!(char_width('干'), 2);
        assert_eq!(char_width('ａ'), 2, "全角字母也是两列");
    }

    #[test]
    fn status_labels_are_chinese() {
        assert_eq!(status_label(SessionState::Working), "干活中");
        assert_eq!(status_label(SessionState::Asking), "等你回答");
        assert_eq!(status_label(SessionState::Idle), "空闲");
        assert_eq!(status_label(SessionState::Stopped), "已停止");
    }

    #[test]
    fn unknown_state_shows_a_dash() {
        assert_eq!(status_label(SessionState::Unknown), "—");
    }

    #[test]
    fn asking_and_working_use_different_colors() {
        assert_ne!(
            status_color(SessionState::Asking),
            status_color(SessionState::Working)
        );
    }

    #[test]
    fn msg_from_str_is_not_an_error() {
        let m: Msg = "完成".into();
        assert!(!m.error);
        assert_eq!(m.text, "完成");
        assert!(Msg::err("炸了".into()).error);
    }
}
