use ratatui::prelude::*;

use crate::i18n::HelpItem;
use crate::pty::{ScreenColor, ScreenSpan, ScreenStyle};
use crate::session::SessionState;

use super::dim;

pub fn status_label(s: SessionState, lang: crate::i18n::Lang) -> &'static str {
    use crate::i18n::{text, Key};
    text(
        match s {
            SessionState::Working => Key::StatusWorking,
            SessionState::Asking => Key::StatusAsking,
            SessionState::Idle => Key::StatusIdle,
            SessionState::Stopped => Key::StatusStopped,
            SessionState::Failed => Key::StatusFailed,
            SessionState::Unknown => Key::StatusUnknown,
        },
        lang,
    )
}

/// 状态在界面上的样式。返回 `Style` 而不是 `Color`：Stopped/Unknown 要用
/// `dim()`，而 `dim()` 在 `Theme::Unknown` 下表达的是 DIM 修饰符、不是某个
/// 颜色，`Color` 装不下。给 `dim()` 再开一个返回 `Color` 的孪生函数只能退回
/// 写死一个灰，等于在安全网上开个洞。
///
/// 干活中/等你回答/空闲仍用具名 ANSI 色：终端主题本来就保证这几个色在自己
/// 背景上可读，我们再去重映射等于跟用户自己的配色打架。
pub fn status_style(s: SessionState) -> Style {
    match s {
        SessionState::Working => Style::default().fg(Color::Cyan),
        SessionState::Asking => Style::default().fg(Color::Yellow),
        SessionState::Idle => Style::default().fg(Color::Green),
        // 出错了用红色：这是屏幕上唯一需要用户立刻做点什么的状态。
        SessionState::Failed => Style::default().fg(Color::Red),
        SessionState::Stopped => dim(),
        SessionState::Unknown => dim(),
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

pub(crate) fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// 按显示宽度截断，超出的用 … 收尾。看板一行放不下就裁，不能让它换行把表格冲乱。
///
/// **顺带把控制字符滤掉，这是渲染层唯一的一道防线。**`char_width` 对
/// 控制字符返回 0（见它自己的文档），如果不在这里主动丢弃，它们会带着
/// 零宽度混进 `out`：既不占用截断预算，又原样穿过这个函数，交给
/// `ratatui` 的 `Span::render_ref` 原样画出来——那条路不像
/// `Buffer::set_stringn`/`Paragraph` 那样过滤控制字符（细节见
/// `fix-1-brief.md`）。`\x1b[A` 这种转义序列一旦这样漏到终端上，就是
/// 每一帧都往看板里发一次真实的光标控制命令。
///
/// 选在这里补、不是在 `session_label` 补：这个函数是**看板列表项、
/// 九宫格标题、附着视图块标题**共用的唯一收窄口（`board.rs`/`grid.rs`/
/// `attach.rs` 的相关调用点都会经过这里），补在这一处就同时覆盖了四条
/// 渲染路径；`session_label` 只覆盖 tag 这一个字段，且它现在返回的是
/// 零拷贝的 `&str`，要在那边过滤就得先把签名改成拿所有权的 `String`，
/// 牵连所有调用点。
///
/// **只丢 `is_control()`，不做转义序列的整体识别**（不像守护进程侧
/// `session::sanitize` 那样把 `ESC '[' ... 终止字节` 整段吃掉）：这里
/// 要保的安全性质只有「控制字节不能原样落进终端」，`\x1b[A` 里的 `[`
/// 和 `A` 单独看是普通可打印字符，把 `ESC` 丢了之后，剩下的 `[A` 对
/// 终端来说就是两个字，不再是一条活的控制序列——守护进程侧要多做一步
/// 是因为它还要保证「记录下来的文本等于用户真正想打的话」，这里没有
/// 这层语义负担，不用照抄那一整套状态机。
///
/// **不会影响宽度/CJK 计算**：`char_width` 已经把控制字符算成 0 列，
/// 这里只是不再把它们 `push` 进 `out`，`w` 的累加值一分不差——跳过的
/// 字符本来就没有为 `w` 贡献过什么。
pub(crate) fn truncate(s: &str, max: usize) -> String {
    let mut w = 0;
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_control() {
            continue;
        }
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

/// 界面上代表一个会话的那一段文字：有名字就是名字，没有就退回 profile。
///
/// **四个视图共用这一个答案处。** 各判各的迟早分叉成「列表写着名字、
/// 格子写着 claude」，而这个功能存在的全部理由就是让同一个会话在哪儿
/// 看都是同一个东西。
pub(crate) fn session_label(s: &crate::session::SessionInfo) -> &str {
    if s.tag.is_empty() {
        &s.profile
    } else {
        &s.tag
    }
}

/// 把 $HOME 缩成 ~，界面上路径太长会被裁掉。
pub(crate) fn short_path(p: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && p.starts_with(&h) => format!("~{}", &p[h.len()..]),
        _ => p.to_string(),
    }
}

/// 底栏中段那块「我在哪个项目」的字，塞进 `cols` 列里。
///
/// 光写一个项目名是不够的：用户手上十来个项目里常有 `web` / `api` 这种
/// 重名的目录，光看名字认不出是哪一个。但中段的宽度是固定的（见
/// `mod.rs::PROJECT_COLS`），整条路径又几乎永远放不下——所以规则是
/// **名字优先，父目录按「从近到远」一段一段往前贴，贴不下就停**：
///
/// - `("dc-terminal", "~/work/dc")` 16 列 → `dc/dc-terminal`
/// - 同上 24 列 → `~/work/dc/dc-terminal`（整条都贴上了，补回 `~`）
/// - `("a", "/tmp")` 16 列 → `/tmp/a`
///
/// 「贴不下就停」而不是「跳过这一段接着试更短的」：路径中间挖掉一段之后
/// 读起来是**另一条真实存在的路径**，而用户没有任何线索知道那是拼出来的。
///
/// 名字自己就超宽时只截名字，一段父目录都不贴——这时候贴上去只会把名字
/// 挤得更短，而名字才是用来认项目的那部分。
///
/// 传进来的 `parent` 必须是 `short_path` 过的显示串（`ProjectGroup.parent`
/// 就是），**绝不能是 canon 过的路径**：macOS 上那会把用户敲的 `/tmp/x`
/// 显示成 `/private/tmp/x`。
pub(crate) fn project_label(name: &str, parent: &str, cols: usize) -> String {
    if display_width(name) > cols {
        return truncate(name, cols);
    }
    let comps: Vec<&str> = parent.split('/').filter(|s| !s.is_empty()).collect();
    let absolute = parent.starts_with('/');
    let mut best = name.to_string();
    for n in 1..=comps.len() {
        let mut cand = format!("{}/{}", comps[comps.len() - n..].join("/"), name);
        // 整条父目录都贴上了，绝对路径的那个开头 `/` 也得补回来——
        // 少了它 `/tmp/a` 会显示成 `tmp/a`，看着像个相对路径。
        if n == comps.len() && absolute {
            cand.insert(0, '/');
        }
        if display_width(&cand) > cols {
            break;
        }
        best = cand;
    }
    best
}

/// 把底栏那张按键表折成不超过 `width` 列的若干行。
///
/// 自己折而不是用 ratatui 的 `Wrap`：`Wrap` 在任何空白处断行，而按键表里
/// 「p 换项目」这种写法键名和说明之间就有一个空格，于是行尾会留下一个
/// 孤零零的 `p`，下一行开头是「换项目」——屏幕上看起来是两个键，其中一个
/// 还没有名字。这里只在**分隔符**处断：两个半角空格，或一个全角空格
/// （`　`，几条帮助文案用它当分隔）。
///
/// 单项本身超宽时独占一行，原样放出去（宁可让它被右端裁掉，也不能丢掉
/// 或者卡在死循环里）。
pub(crate) fn wrap_help(help: &str, width: usize) -> Vec<String> {
    let items: Vec<&str> = help
        .split("  ")
        .flat_map(|s| s.split('\u{3000}'))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let disp = |s: &str| s.chars().map(char_width).sum::<usize>();
    const SEP: &str = "  ";
    let sep_w = 2;

    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for it in items {
        let w = disp(it);
        if cur.is_empty() {
            cur.push_str(it);
            cur_w = w;
        } else if cur_w + sep_w + w <= width {
            cur.push_str(SEP);
            cur.push_str(it);
            cur_w += sep_w + w;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(it);
            cur_w = w;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// 一条提示占几列。跟 `HelpItem` 的 `Display` 必须一致——量的和画的
/// 不是同一串字符的话，算出来的宽度就是假的。
pub(crate) fn item_width(it: &HelpItem) -> usize {
    display_width(&it.to_string())
}

/// 底栏那一行：从前往后取放得下的若干条，**最后一条永远保留**。
///
/// 底栏只有一行（多一行就少一行内容区，九宫格在 80×24 下直接跌破
/// `grid.rs` 的 `MIN_ROWS`），所以放不下的必须丢。丢哪些由**调用方排的
/// 顺序**决定：`idle_help` 里越靠前的键越重要，尾巴上的 `? …` 是那扇
/// 「被丢掉的键都在里面」的门，它一旦被丢，用户就再也找不回那些键了——
/// 所以它是唯一一条不参与截断的。
///
/// 宽度小到连尾巴都放不下时仍然把尾巴还回去：让 `Paragraph` 去截，
/// 总比返回一个空行、屏幕上什么都不写强。
pub(crate) fn fit_help(items: &[HelpItem], width: usize) -> Vec<&HelpItem> {
    let Some((tail, head)) = items.split_last() else {
        return Vec::new();
    };
    const SEP_W: usize = 2;
    let mut out: Vec<&HelpItem> = Vec::new();
    // 给尾巴留的位置：它自己的宽度，外加它前面那个分隔符
    let budget = width.saturating_sub(item_width(tail) + SEP_W);
    let mut used = 0usize;
    for it in head {
        let w = item_width(it);
        let need = if out.is_empty() { w } else { SEP_W + w };
        if used + need > budget {
            break;
        }
        used += need;
        out.push(it);
    }
    out.push(tail);
    out
}

/// 把一排提示折成不超过 `width` 列的若干行（「全部按键」浮层用）。
///
/// 跟 `fit_help` 分开：那边只有一行、放不下就丢；这边是浮层，行数管够，
/// 一条都不能丢。
pub(crate) fn wrap_items(items: &[HelpItem], width: usize) -> Vec<Vec<&HelpItem>> {
    const SEP_W: usize = 2;
    let mut rows: Vec<Vec<&HelpItem>> = Vec::new();
    let mut cur: Vec<&HelpItem> = Vec::new();
    let mut used = 0usize;
    for it in items {
        let w = item_width(it);
        if !cur.is_empty() && used + SEP_W + w > width {
            rows.push(std::mem::take(&mut cur));
            used = 0;
        }
        used += if cur.is_empty() { w } else { SEP_W + w };
        cur.push(it);
    }
    if !cur.is_empty() {
        rows.push(cur);
    }
    rows
}

/// 一排提示画成带样式的一行：**键名加粗，说明不加粗**。
///
/// 加粗是这一行里唯一的层次——底栏是一串「字母 + 中文」交替的文字，
/// 不把字母挑出来的话，用户得逐个词去认哪个是能按的键。
pub(crate) fn help_spans(items: &[&HelpItem]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for it in items {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        if !it.key.is_empty() {
            spans.push(Span::styled(
                it.key.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(" "));
        }
        // 走 `label()` 而不是 `label`：动态标签（`n 新建 claude`）顶掉词条表
        // 那一条，量宽度的 `item_width` 走的也是它，两处必须是同一串字符。
        spans.push(Span::raw(it.label().to_string()));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{help_items, Key, Lang};

    fn items(pairs: &[(&'static str, Key)]) -> Vec<HelpItem> {
        help_items(pairs, Lang::Zh)
    }

    /// 中段那 16 列要用满：名字放得下就把父目录一段一段贴回去，
    /// 而不是让一个短名字后面拖着一大片空白。用户手上重名的目录很多
    /// （`web`/`api`/`docs`），光一个名字认不出是哪一个。
    #[test]
    fn project_label_spends_the_budget_on_parent_context() {
        assert_eq!(
            project_label("dc-terminal", "~/work/dc", 16),
            "dc/dc-terminal",
            "16 列只贴得下最近的一段"
        );
        assert_eq!(
            project_label("dc-terminal", "~/work/dc", 24),
            "~/work/dc/dc-terminal",
            "放得下就把整条贴回去"
        );
        // 绝对路径整条贴回去时，开头那个 `/` 得补上，否则看着像相对路径
        assert_eq!(project_label("a", "/tmp", 16), "/tmp/a");
        // 没有父目录（根目录下的项目）就只剩名字
        assert_eq!(project_label("a", "", 16), "a");
    }

    /// 名字自己就超宽时只截名字：贴父目录只会把用来认项目的那部分挤得更短。
    #[test]
    fn project_label_truncates_the_name_rather_than_the_parent() {
        // `truncate` 补的那个 `…` 会让结果比 max 多出一列（既有行为），
        // 中段留着 2 列间隔正好吃得下
        assert_eq!(
            project_label("a-very-long-project", "~/work", 10),
            "a-very-lon…"
        );
        // 中文项目名一个字两列，按显示宽度截，不能按字符数
        assert_eq!(project_label("一二三四五六", "~/w", 5), "一二…");
    }

    /// **核心回归测试**：控制字符/转义序列不能穿过 `truncate` 落进渲染
    /// 出的字符串——这是渲染层唯一的一道防线（细节见 `truncate` 自己的
    /// 文档、`fix-1-brief.md`、`fix-1-report.md` 的 Important 2）。
    /// 上箭头（`\x1b[A`）和退格（`\x7f`）是 fix-1-brief 按键表里
    /// 真实存在的转发字节，直接照抄。
    #[test]
    fn truncate_strips_control_bytes_before_they_can_reach_the_render_path() {
        assert_eq!(truncate("\x1b[Afix\x7f", 20), "[Afix");
        assert_eq!(truncate("\x1b\x01hi", 20), "hi");
    }

    /// 控制字符不占宽度预算（`char_width` 早就把它们算成 0 列），丢弃
    /// 它们不该让后面正常的字被多裁或者少裁一个——`w` 的累加值必须跟
    /// 「控制字符从没出现过」时完全一样。
    #[test]
    fn truncate_dropping_control_bytes_does_not_shift_the_width_budget() {
        assert_eq!(truncate("ab", 2), "ab");
        assert_eq!(
            truncate("a\x1bb", 2),
            "ab",
            "控制字符不该占用这 2 列里的任何一列"
        );
        assert_eq!(truncate("abc", 2), "ab…");
        assert_eq!(
            truncate("a\x1bbc", 2),
            "ab…",
            "丢弃控制字符之后，正常字符该不该被裁的判断不能变"
        );
    }

    /// 中文/CJK 宽度计算不受影响：控制字符和两列宽的字符混在一起时，
    /// 截断点仍然按显示宽度算，不是按字符数。
    #[test]
    fn truncate_control_byte_stripping_does_not_disturb_cjk_width_accounting() {
        // 4 列刚好放得下两个中文字，后面没有更多字符，不该被裁。
        assert_eq!(truncate("一\x1b二", 4), "一二");
        // 加上第三个字就超出 4 列，即使 4 列本身正好被前两个字占满，
        // 这一条钉的是「控制字符没有偷偷占掉本该属于第三个字的空间」。
        assert_eq!(truncate("一\x1b二三", 4), "一二…");
    }

    /// 路径中间不许挖空。`~/work/dc/x` 放不下时给出 `dc/x`，绝不能是
    /// `~/dc/x`——后者是**另一条真实存在的路径**，而用户没有任何线索
    /// 知道那是拼出来的。
    #[test]
    fn project_label_never_elides_the_middle_of_a_path() {
        let s = project_label("x", "~/work/dc", 8);
        assert_eq!(s, "dc/x");
        assert!(!s.starts_with("~/"), "开头那段贴不下就整段不贴：{s}");
    }

    /// 底栏只有一行，放不下的只能丢——但那扇能找回它们的门（尾巴上的
    /// `? …`）绝不能跟着一起被丢。丢了它，被截掉的键就真的没有任何入口了。
    #[test]
    fn fit_help_never_drops_the_tail() {
        let all = items(&[
            ("↑↓", Key::Select),
            ("Enter", Key::Open),
            ("n", Key::New),
            ("s", Key::Stop),
            ("d", Key::Diff),
            ("?", Key::MoreKeys),
        ]);
        for width in [0usize, 1, 3, 8, 20, 57] {
            let kept = fit_help(&all, width);
            assert_eq!(
                kept.last().copied(),
                all.last(),
                "{width} 列下尾巴被丢了：{kept:?}"
            );
        }
    }

    /// 截断从**尾巴前面**开始，而且是从后往前丢：越靠前的键越重要。
    #[test]
    fn fit_help_keeps_the_important_keys_first() {
        let all = items(&[
            ("↑↓", Key::Select),  // 7 列
            ("Enter", Key::Open), // 12 列
            ("n", Key::New),      // 6 列
            ("?", Key::MoreKeys), // 3 列
        ]);
        // 7 + 2 + 12 + 2 + 3 = 26 列：正好放得下前两条加尾巴，`n 新建` 放不下
        let kept = fit_help(&all, 26);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[0].key, "↑↓");
        assert_eq!(kept[1].key, "Enter");
        assert_eq!(kept[2].key, "?");
        // 宽一点就该多露一个出来
        assert_eq!(fit_help(&all, 34).len(), 4);
    }

    /// 无论多窄，底栏都只有一行——这是这次改动的全部意义。
    #[test]
    fn fit_help_always_fits_on_one_line() {
        let all = items(&[
            ("↑↓", Key::Select),
            ("Enter", Key::Open),
            ("n", Key::New),
            ("N", Key::SwitchAgent),
            ("p", Key::SwitchProject),
            // 这一条纯粹是**宽度素材**：这条测试算的是列数，不是哪个键真绑在
            // 看板上。原来放的是 `a 看全部项目`（中文 12 列 / 英文 14 列），
            // 那条词条随着分组一起删了，换成宽度最接近的 `进入文件夹`
            // （12 列 / 13 列），列数预算不变。
            ("e", Key::EnterFolder),
            ("?", Key::MoreKeys),
        ]);
        for width in [24usize, 40, 57, 80, 120] {
            let line: String = fit_help(&all, width)
                .iter()
                .map(|it| it.to_string())
                .collect::<Vec<_>>()
                .join("  ");
            assert!(
                display_width(&line) <= width,
                "{width} 列下折出了 {} 列：{line}",
                display_width(&line)
            );
        }
    }

    /// 键名加粗，说明不加粗。这一行里字母和中文交替出现，不给键名一点
    /// 重量的话，用户得逐个词去认哪个是能按的。
    #[test]
    fn help_spans_bold_only_the_key_name() {
        let all = items(&[("n", Key::New)]);
        let spans = help_spans(&all.iter().collect::<Vec<_>>());
        let bold: Vec<&str> = spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(bold, vec!["n"], "只有键名该是粗的");
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "n 新建", "加粗不能改变画出来的字");
    }

    /// 没有键名的那种提示（「其余按键都发给 agent」）不该在句首多一个空格。
    #[test]
    fn help_spans_of_a_keyless_item_start_with_the_label() {
        let all = items(&[("", Key::OtherKeysGoToAgent)]);
        let spans = help_spans(&all.iter().collect::<Vec<_>>());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, crate::i18n::text(Key::OtherKeysGoToAgent, Lang::Zh));
    }

    /// 浮层里一条都不能丢：底栏丢掉的键正是靠它才找得回来。
    #[test]
    fn wrap_items_keeps_every_item() {
        let all = items(&[
            ("n", Key::New),
            ("N", Key::SwitchAgent),
            ("s", Key::Stop),
            ("u", Key::Undo),
            ("d", Key::Diff),
        ]);
        let rows = wrap_items(&all, 16);
        assert!(rows.len() > 1, "16 列放不下，必须折行：{rows:?}");
        assert_eq!(rows.iter().map(|r| r.len()).sum::<usize>(), all.len());
    }

    /// 底栏按键表折行时，**一个键的名字和它的说明绝不能分家**。
    /// ratatui 自带的 `Wrap` 在任何空白处断行，于是「p 换项目」会被折成
    /// 行尾一个孤零零的 `p` 加下一行的「换项目」——屏幕上看起来像两个键，
    /// 而其中一个还没有名字。
    #[test]
    fn wrap_help_never_splits_a_key_from_its_label() {
        let help = "q 退出  n 新建  N 换 agent  p 换项目  a 看全部项目  c 密钥";
        let lines = wrap_help(help, 30);
        assert!(lines.len() > 1, "30 列放不下，必须折行：{lines:?}");
        for l in &lines {
            assert!(
                !l.trim_end().ends_with(" p")
                    && !l.trim_end().ends_with(" n")
                    && !l.trim_end().ends_with(" N"),
                "行尾留下了一个没有说明的孤零零的键：{l:?}"
            );
        }
        // 折行不能丢字：把各行拼回去，每个键都还在
        let joined = lines.join("");
        for k in ["q 退出", "n 新建", "N 换 agent", "p 换项目", "a 看全部项目"] {
            assert!(joined.contains(k), "折行把「{k}」弄丢了：{joined}");
        }
    }

    /// 单个键本身就比一行还宽时不能死循环，也不能把它整个丢掉。
    #[test]
    fn wrap_help_keeps_an_oversized_item_on_its_own_line() {
        let lines = wrap_help("a 一个特别特别特别长的说明文字", 8);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("特别特别"));
    }

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

    /// 出错是屏幕上唯一需要用户立刻做点什么的状态，必须是红的——
    /// 跟「已停止」的灰区分开：停了是他自己按的，出错不是。
    #[test]
    fn the_failed_state_is_red_and_named_in_both_languages() {
        use crate::i18n::Lang;
        assert_eq!(
            status_style(SessionState::Failed).fg,
            Some(Color::Red),
            "出错必须是红的"
        );
        assert_eq!(status_label(SessionState::Failed, Lang::Zh), "出错了");
        assert_eq!(status_label(SessionState::Failed, Lang::En), "error");
    }

    #[test]
    fn status_labels_are_translated() {
        use crate::i18n::Lang;
        assert_eq!(status_label(SessionState::Working, Lang::Zh), "干活中");
        assert_eq!(status_label(SessionState::Working, Lang::En), "working");
        assert_eq!(status_label(SessionState::Asking, Lang::Zh), "等你回答");
        assert_eq!(status_label(SessionState::Idle, Lang::Zh), "空闲");
        assert_eq!(status_label(SessionState::Stopped, Lang::Zh), "已停止");
    }

    #[test]
    fn unknown_state_shows_a_dash() {
        assert_eq!(
            status_label(SessionState::Unknown, crate::i18n::Lang::Zh),
            "—"
        );
    }

    #[test]
    fn asking_and_working_use_different_colors() {
        assert_ne!(
            status_style(SessionState::Asking),
            status_style(SessionState::Working)
        );
    }

    /// Stopped/Unknown 这两个「没在干活」的状态要走弱化样式，跟说明栏、
    /// 不可用项用的是同一套自适应灰，不能再自己钉一个写死的颜色。
    #[test]
    fn inactive_states_use_the_adaptive_dim_style() {
        assert_eq!(status_style(SessionState::Stopped), dim());
        assert_eq!(status_style(SessionState::Unknown), dim());
    }

    /// 测试进程里没人调过 `init_theme`，`dim()` 必须给出 `Unknown` 的样式，
    /// 而不是 panic 或者某个写死的灰。这条同时守着「探测没跑过也能正常渲染」
    /// 这个前提——所有渲染测试都靠它。
    #[test]
    fn dim_falls_back_to_unknown_before_detection() {
        assert_eq!(dim(), crate::theme::Theme::Unknown.dim());
    }

    #[test]
    fn msg_from_str_is_not_an_error() {
        let m: Msg = "完成".into();
        assert!(!m.error);
        assert_eq!(m.text, "完成");
        assert!(Msg::err("炸了".into()).error);
    }

    /// 「画哪一段」只有这一个答案处。散在四个视图里各判一次，迟早分叉成
    /// 「列表写着名字、格子写着 profile」。
    #[test]
    fn session_label_falls_back_to_the_profile_when_there_is_no_tag() {
        let mut s = crate::session::SessionInfo {
            id: 3,
            profile: "claude".into(),
            dir: "/w/a".into(),
            state: crate::session::SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        };
        assert_eq!(session_label(&s), "claude");
        s.tag = "修登录白屏".into();
        assert_eq!(session_label(&s), "修登录白屏");
    }
}
