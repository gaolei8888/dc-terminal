use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, HighlightSpacing, List, ListItem, Paragraph};

use super::app::App;
use super::view::is_plain_key;
use super::widgets::{
    display_width, pad_to, project_label, session_label, status_label, status_style, truncate,
};
use super::view::{ProjectGroup, Row};
use crate::session::{SessionInfo, SessionState};
use super::{
    accent, danger, dim, open_new_session, open_project_picker, open_secrets, session_action,
};
use crate::i18n::{msg, text, Key};

/// `▶ ` 那个高亮记号占的列数。
const SYMBOL_COLS: usize = 2;

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Char('q') if is_plain_key(&key) => app.quit = true,
        KeyCode::Down => super::move_row(app, 1),
        KeyCode::Up => super::move_row(app, -1),
        // 一步换项目。这是日常换项目的**主路径**——`p` 只在要去一个
        // 看板上还没有的项目时才用。
        KeyCode::Tab => super::jump_project(app, 1),
        KeyCode::BackTab => super::jump_project(app, -1),
        KeyCode::Char(c @ '1'..='9') if is_plain_key(&key) => {
            super::goto_project(app, c as usize - '1' as usize)
        }
        // 折叠/展开当前组。看板上左右键原来没有用途，九宫格那边是移动焦点，
        // 两个视图各自的方向语义不冲突。
        KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => super::toggle_collapse(app),
        KeyCode::Char('n') | KeyCode::Char('N') if is_plain_key(&key) => {
            open_new_session(app, key.code)
        }
        KeyCode::Char('p') if is_plain_key(&key) => open_project_picker(app),
        // 列表这边成不成功都不用再动光标：这里只有**一个**指针（光标本身），
        // 没有第二个要跟它对齐。成功时组没了，`refresh_rows` 的锚点回落是
        // 列表自己的事；被拒绝时什么都没变，光标本来就该原地不动。
        // 九宫格那边不一样，它还有个看得见的 `▶`——见 `grid::handle_key`。
        KeyCode::Char('x') if is_plain_key(&key) => {
            let _ = super::unpin_current(app);
        }
        KeyCode::Char('c') if is_plain_key(&key) => open_secrets(app),
        // 加电脑：要一个 6 位邀请码（不用 `i`：九宫格里 `i` 是开回复框）。
        KeyCode::Char('a') if is_plain_key(&key) => super::computers::start_invite(app),
        // `l` = language。设置页跟 `c 密钥` 挨着：两个都是「配置」类入口，
        // 而且跟 g 一样，两个视图共用同一个键。
        KeyCode::Char('l') if is_plain_key(&key) => super::open_settings(app),
        KeyCode::Enter => {
            if let Some(id) = app.selected_session().map(|s| s.id) {
                super::enter_session(app, id);
            }
        }
        // 切模式并记住。焦点/光标的对齐在 toggle_view_mode 里统一做——
        // 两个方向各写一份的话，迟早只改对一半。
        KeyCode::Char('g') if is_plain_key(&key) => super::toggle_view_mode(app),
        // 底栏只有一行，装不下的键都在这扇门后面（底栏尾巴上那条 `? …`）
        KeyCode::Char('?') if is_plain_key(&key) => super::keys::open(app),
        // 三个动作跟九宫格共用 session_action，区别只在「当前会话」是
        // 选中行还是焦点格
        KeyCode::Char('u') | KeyCode::Char('s') | KeyCode::Char('d') if is_plain_key(&key) => {
            app.message = match app.selected_session().map(|s| s.id) {
                Some(id) => session_action(app, key.code, id),
                None => text(Key::NoSessionSelected, app.lang).into(),
            };
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    // 断连时用红色边框给出明确的视觉提示：界面上的数据是上一次成功请求
    // 留下的陈旧快照，不代表守护进程现在的真实状态。
    let border_style = if app.connected {
        Style::default()
    } else {
        danger()
    };
    // 当前项目：整组左侧一条竖色条。不靠光标行——光标只标「哪一行」，
    // 项目要的是「哪一片」，隔着屏幕就得认得出来。
    //
    // 算在标题之前：标题上那块牌子写的就是这个组。
    let current = app
        .list_state
        .selected()
        .and_then(|i| super::view::group_of(&app.rows, i));

    // 标题：`dct 会话看板` + 当前项目的**完整路径**反白成一块牌子，断连那
    // 半句接在牌子后面。
    //
    // 三处说同一件事，各答一个问题，都不多余：标题答「我在哪个项目」（这里
    // 宽度有余，路径写得全，而组头那 18 列写不全），组头那块牌子答「屏幕上
    // 哪几行是我的」（一条 1 列宽的竖条答不了，这才是这次改动的起因），底栏
    // 答「按 `n` 会开在哪」。
    //
    // 牌子在前、断连在后：断连是全屏唯一一处红字，它挤长了只该把标题右边
    // 那条横线吃掉，不该把「我在哪」顶出屏幕——恰恰是断连这一屏最想知道
    // 自己在哪。所以不再用 `msg::title_with` 把两句拼成一个 `String`。
    let base = text(Key::BoardTitle, app.lang);
    let mut title = vec![Span::raw(format!("{base} "))];
    // 这一行左半边（标题 + 牌子 + 断连）实际吃掉了多少列——版本号是否有
    // 命放在右边，得拿这个数去跟屏幕宽度比，不能只看牌子自己的预算。
    let mut used = display_width(base) + 1;
    if let Some(gi) = current {
        let g = &app.groups[gi];
        // 预算：整行减掉标题、断连那半句、和右边至少留的几列横线。量不下
        // 就少贴几段父目录（`project_label` 自己会退到只写名字），而不是
        // 让它去挤断连那句话。
        let taken = display_width(base)
            + 1
            + if app.connected {
                0
            } else {
                display_width(text(Key::Disconnected, app.lang)) + 4
            };
        let room = (area.width as usize).saturating_sub(taken + 6);
        let chip = format!(
            " {} ",
            project_label(&g.name, &g.parent, room.saturating_sub(2))
        );
        used += display_width(&chip);
        title.push(Span::styled(
            chip,
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
        ));
    }
    if !app.connected {
        let warn = format!("（{}）", text(Key::Disconnected, app.lang));
        used += display_width(&warn);
        title.push(Span::styled(warn, Style::default().fg(Color::Red)));
    }
    let title = Line::from(title);

    // 版本号：这一行上最不值钱的一段。学生装完在下载页被告知来对版本号，
    // 但眼睛第一眼看的是看板，不是要另外翻到的设置页——所以补一份，跟
    // 设置页共用同一个 `msg::dct_version`，两处不会因为各写一份文案而
    // 吵起来（也不会有人在这里手写版本号）。
    //
    // 位置钉在整行最右端（`title_top` 独立于左边那个 `title`，靠
    // `right_aligned` 贴边框右角），不是接在断连那半句后面——那半句已经
    // 是全屏唯一一处红字，版本号没资格再往它旁边挤。
    //
    // 但它得给别人让路：宽度不够时，横线先被压缩、牌子先退化、断连那句
    // 红字最后才可能被牺牲——版本号必须在这一切发生之前就自己消失，不
    // 能靠 `Block` 自动裁剪硬挤掉别的东西（两个 title 的宽度加起来超过
    // 屏幕宽度时谁盖住谁是未定义的观感，不能拿去赌）。所以这里手动算宽度、
    // 手动决定要不要贴这个 title，而不是无条件塞给 `title_top`。
    let version = msg::dct_version(app.lang, env!("CARGO_PKG_VERSION"));
    let version_width = display_width(&version);
    // 版本号和左边内容之间至少留一格空白，版本号右边（贴着边框角）至少
    // 留 2 列可见的横线——不然它会看着像是被边框切掉了一半，而不是特意
    // 贴边显示。
    const MIN_GAP_BEFORE_VERSION: usize = 1;
    const MIN_RULE_AFTER_VERSION: usize = 2;
    let show_version = (area.width as usize)
        >= used + MIN_GAP_BEFORE_VERSION + version_width + MIN_RULE_AFTER_VERSION;

    // **看板是唯一不用 `widgets::header` 的一屏**，标题仍然嵌在那条横线里。
    //
    // 理由是它是所有浮层的**背景**：选项目的浮层居中盖在它身上，看板往下
    // 挪一行，浮层上方能露出来的看板内容就少一行——`the_project_picker_is_
    // an_overlay_not_a_takeover` 抓到的正是这个（第一条会话行整个滑到浮层
    // 标题底下）。表头那两行在别处不花钱（原来上下边框也是两行），只有这里
    // 花在了「背景还剩多少看得见」上。
    //
    // 底下那条边框还是去掉了：少一条线，还给列表一行。
    let mut block = Block::default()
        .borders(Borders::TOP)
        .border_style(border_style)
        .title(title);
    if show_version {
        block = block.title_top(Line::from(Span::styled(version, dim())).right_aligned());
    }
    // 多电脑那两块：标题下面的邀请码，底部一段「我的电脑」。都是现成的
    // 数据（`App::mesh`），这里不发请求。
    let inner = block.inner(area);
    let width = inner.width as usize;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let prompt = super::computers::invite_lines(&app.mesh, app.lang, width, now);
    let section = super::computers::section_lines(&app.mesh, app.lang, width);
    let (prompt_area, list_area, section_area) = split(inner, prompt.len(), section.len());
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(prompt), prompt_area);
    if app.rows.is_empty() {
        f.render_widget(
            Paragraph::new(text(Key::NoSessionsChooseProject, app.lang)).centered(),
            list_area,
        );
    } else {
        // 行内容的宽度：整块列表减掉 `▶ ` 那两列。`HighlightSpacing::Always`
        // 让每一行都先空出这两列——右对齐的行尾标记要拿这个数算位置，不能
        // 让它随「有没有选中行」变来变去。
        let width = (list_area.width as usize).saturating_sub(SYMBOL_COLS);
        let items = rows_for(app, width, current, now_ms());
        // 实色档下选中行整行铺上底栏同一档颜色（见 `paint_selected`），
        // 那时候不再需要 List 自己的高亮；横线档 / `NO_COLOR` 没有实色可铺，
        // 退回加粗，`▶` 两种情况下都在。
        let highlight = if super::bar_style(app.bar).is_some() {
            Style::default()
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        f.render_stateful_widget(
            List::new(items)
                .highlight_symbol("▶ ")
                .highlight_spacing(HighlightSpacing::Always)
                .highlight_style(highlight),
            list_area,
            &mut app.list_state,
        );
    }
    f.render_widget(Paragraph::new(section), section_area);
}

/// 此刻的 Unix 毫秒，量「空闲多久」用。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 行首那几列固定宽度：项目色条 1、组序号 ` 1 ` 3、折叠记号 `▾` 1。
/// 记号后面那一格空隙算在项目列里（见 `Cols::proj`）。
const LEAD_COLS: usize = 1 + 3 + 1;
/// 会话号 `{id:>3}` 加两格空隙。
const ID_COLS: usize = 5;
/// 状态前那个圆点加一格空隙。
const DOT_COLS: usize = 2;
/// 列与列之间的空隙。
const GAP: usize = 2;
/// 项目名那一列内容的上限（宽屏 / 100 列以下）。超过的裁短——名字裁短了
/// 还认得出是哪个项目，红字被挤下屏幕就什么都没有了（见 `rows_for` 头上那段）。
const PROJ_MAX: usize = 16;
const PROJ_MAX_NARROW: usize = 12;
/// 会话名那一列的上限，同上。
const NAME_MAX: usize = 16;
const NAME_MAX_NARROW: usize = 10;
/// 窄于这个宽度就用上面那几个 `_NARROW`：80 列上每一格都是从 activity
/// 那边抠出来的，而 activity 是这一屏上唯一说「它此刻在干什么」的那一列。
const NARROW: usize = 100;
/// 智能体名的上限：内置 profile 最长的 `qwen-api` 正好 8 列。
const AGENT_MAX: usize = 8;
/// activity 那一列最多画多宽：70 列内容，裁了的话再加那一个 `…`。宽屏上
/// 一整行 200 列的终端输出读不完，也不是这一列要说的事。
const ACTIVITY_MAX: usize = 70 + 1;

/// 看板这一屏的列宽。**一帧只算一次**，组头、合成的单会话行、组里的会话行
/// 都按这一份排——列对不齐，向来是因为每种行各算各的。
///
/// 宽度跟着屏幕上真有的内容走（最长的项目名、最长的状态词……），各有上限：
/// 手上项目都叫 `web`/`api` 的人不该替一个想象中的长名字留 16 列空白，
/// 而 80 列上每一格都是从 activity 那一列抠出来的。
struct Cols {
    /// 项目名那一列：左边一格（牌子的左垫）、名字、右边一格（牌子的右垫）、
    /// 再一格空隙。**没戴牌子的名字也从左边第二格起写**，所以戴不戴牌子，
    /// 名字的第一个字都落在同一列上。
    proj: usize,
    /// 状态词，不含前面的圆点。
    status: usize,
    /// 智能体名。
    agent: usize,
    /// 会话名。0 = 这一屏没有一个会话起了名字，这一列连空隙一起省掉。
    name: usize,
}

impl Cols {
    /// 一行会话在 activity 之前占掉的列数。
    fn prefix(&self) -> usize {
        LEAD_COLS
            + self.proj
            + ID_COLS
            + DOT_COLS
            + self.status
            + GAP
            + self.agent
            + GAP
            + if self.name == 0 { 0 } else { self.name + GAP }
    }
}

/// 裁到**不超过** `w` 列。`truncate` 真裁了的时候会多补一个 `…`（max + 1 列），
/// 这里把那一格先让出来，调用方不用再记着「传 15 才是 16」。
fn clip(s: &str, w: usize) -> String {
    if w == 0 {
        String::new()
    } else if display_width(s) <= w {
        truncate(s, w)
    } else {
        truncate(s, w - 1)
    }
}

/// 会话的状态记号和状态词的颜色：一档强调色给干活中，灰给空闲，红只给出错。
///
/// 不直接用 `status_style`：那是九宫格标题条的配色，空闲在那儿是绿的。看板
/// 一屏二十几行，绿的空闲和青的干活中挨在一起，扫一眼分不出谁在动——这里要
/// 的是「在动的亮、停着的暗」。等你回答照旧用它自己那档暖色，出错照旧红。
fn state_mark(state: SessionState) -> (&'static str, Style) {
    match state {
        SessionState::Working => ("●", accent()),
        SessionState::Asking => ("◆", status_style(SessionState::Asking)),
        SessionState::Idle => ("○", dim()),
        SessionState::Failed => ("✕", danger()),
        SessionState::Stopped => ("·", dim()),
        // 状态词本身就是「—」（不知道），前面再点一个记号是在装作知道点什么。
        SessionState::Unknown => (" ", dim()),
    }
}

/// 状态那一格的字：空闲的带上空了多久。
fn state_text(s: &SessionInfo, lang: crate::i18n::Lang, now_ms: u64) -> String {
    if s.state == SessionState::Idle {
        let secs = (s.last_active_ms > 0).then(|| now_ms.saturating_sub(s.last_active_ms) / 1000);
        msg::idle_for(lang, secs)
    } else {
        status_label(s.state, lang).to_string()
    }
}

/// 会话那一格的名字。没起名字时留空——智能体那一列已经写着 profile 了，
/// `session_label` 的回落在这里只会把同一个词印两遍。
fn session_name(s: &SessionInfo) -> &str {
    if s.tag.is_empty() {
        ""
    } else {
        session_label(s)
    }
}

/// 一行正在拼的字，顺手记着已经用掉多少列。
struct Cells {
    spans: Vec<Span<'static>>,
    w: usize,
}

impl Cells {
    fn push(&mut self, text: String, style: Style) {
        self.w += display_width(&text);
        self.spans.push(Span::styled(text, style));
    }
}

/// 每个组在项目那一列上写什么：通常只是名字；**两个组同名时**才带上父目录，
/// 用 `project_label` 从近到远贴几段——两个 `web` 并排，光看名字认不出哪个
/// 是哪个，而不重名的时候那串路径什么都没说，只占地方。
fn group_labels(groups: &[ProjectGroup], cap: usize) -> Vec<String> {
    groups
        .iter()
        .map(|g| {
            if groups.iter().filter(|o| o.name == g.name).count() > 1 {
                project_label(&g.name, &g.parent, cap)
            } else {
                g.name.clone()
            }
        })
        .collect()
}

/// 看板列表的每一行。
///
/// 三种行共用一套列（`Cols`）：
///
/// - `Row::Single`：一个会话的组，项目和会话合成一行——绝大多数项目就是这样。
/// - `Row::Header`：两个以上会话的组的标题，只写项目名；有出错的会话时在
///   状态那一列写红色的「N 个出错」。
/// - `Row::Session`：标题下面那几行，项目那一列空着，末尾一个 `├`/`└` 紧贴
///   会话号，把它们挂在标题下面。
///
/// **红字永远不能被挤下屏幕。** 会话静默失败是 dct 最贵的失败模式：组头的
/// 「N 个出错」写在状态那一列，前面只有长度封了顶的项目名；单会话行的
/// 「✕ 出错了」也在状态列。会被截的只有 activity，它在行尾的标记（`● 播`、
/// `✉ N`）之前先让位。
fn rows_for(
    app: &App,
    width: usize,
    current: Option<usize>,
    now_ms: u64,
) -> Vec<ListItem<'static>> {
    let lang = app.lang;
    let narrow = width < NARROW;
    let proj_cap = if narrow { PROJ_MAX_NARROW } else { PROJ_MAX };
    let name_cap = if narrow { NAME_MAX_NARROW } else { NAME_MAX };
    let labels = group_labels(&app.groups, proj_cap);
    let shown: Vec<&SessionInfo> = app
        .rows
        .iter()
        .filter_map(|r| r.session_index())
        .filter_map(|(g, s)| app.groups.get(g)?.sessions.get(s))
        .collect();
    let cols = Cols {
        // 内容 + 牌子左右各一格 + 一格空隙：当前项目的名字反白成一块牌子
        // （见下面），牌子不许从别的列借宽度。
        proj: app
            .rows
            .iter()
            .filter(|r| !matches!(r, Row::Session(..)))
            .map(|r| display_width(&labels[r.group()]).min(proj_cap))
            .max()
            .unwrap_or(0)
            .max(4)
            + 3,
        status: shown
            .iter()
            .map(|s| display_width(&state_text(s, lang, now_ms)))
            .max()
            .unwrap_or(0),
        agent: shown
            .iter()
            .map(|s| display_width(&s.profile).min(AGENT_MAX))
            .max()
            .unwrap_or(0),
        name: shown
            .iter()
            .map(|s| display_width(session_name(s)).min(name_cap))
            .max()
            .unwrap_or(0),
    };
    let selected = app.list_state.selected();

    app.rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let gi = row.group();
            let g = &app.groups[gi];
            let mut line = Cells {
                spans: Vec::new(),
                w: 0,
            };
            // 当前项目：整组左侧一条竖色条。光标只标「哪一行」，项目要的是
            // 「哪一片」，隔着屏幕就得认得出来。
            line.push(
                if Some(gi) == current { "┃" } else { " " }.into(),
                accent(),
            );
            // 序号只给前九个组：`1`…`9` 直达。第十个起靠 Tab，印一个按不动的
            // 号码等于在屏幕上说谎。组里的会话行不重复写。
            let num = match row {
                Row::Header(_) | Row::Single(_) if gi < 9 => format!(" {} ", gi + 1),
                _ => "   ".into(),
            };
            line.push(num, dim());
            let fold = match row {
                Row::Header(_) if g.collapsed => "▸",
                Row::Header(_) => "▾",
                Row::Single(_) | Row::Session(..) => " ",
            };
            line.push(fold.into(), dim());

            // 目录被删了：名字标灰并点出来。会话本身还活着（进程的 cwd 已经
            // 打开），组照常留在看板上——让它消失才是真的找不回来了。
            let gone = !g.dir.exists();
            match row {
                // 组里的会话行：项目那一格空着，末尾挂一个 `├`/`└` 紧贴会话号，
                // 把这几行跟上面那行标题连起来。挂在行首的话，它跟会话号之间
                // 隔着一整列空白，看着像飘在那儿，什么都没连上。
                Row::Session(_, si) => {
                    let branch = if *si + 1 == g.sessions.len() { "└" } else { "├" };
                    line.push(" ".repeat(cols.proj - 2), Style::default());
                    line.push(format!("{branch} "), dim());
                }
                // 当前项目的名字**反白成一块牌子**，不是加粗了事：屏幕上每个
                // 项目名都是加粗的，加粗认不出哪个是自己那个。用 `REVERSED`
                // 而不是挑一个具名色，理由跟底栏那块牌子一样（`ui/mod.rs`）：
                // 六档底栏配色里任何写死的前景色都会在某一档上糊掉。
                //
                // 牌子**只包住名字**，右边补的空白照旧是普通底色——反白连着
                // 补满一列的话，短名字后面拖出一条长尾巴，像进度条不像牌子。
                //
                // 目录没了的组不贴牌子：整行标灰是在说「这东西现在是坏的」，
                // 反白会把它重新变成屏幕上最显眼的东西。
                _ if Some(gi) == current && !gone => {
                    let chip = format!(" {} ", clip(&labels[gi], cols.proj - 3));
                    let w = display_width(&chip);
                    line.push(
                        chip,
                        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
                    );
                    line.push(" ".repeat(cols.proj.saturating_sub(w)), Style::default());
                }
                // **先截再补**：`pad_to` 只补不截，一个长目录名（`我的自媒体
                // 电商代运营项目` 是 24 列）会把整行右推，先被吃掉的正是红字。
                _ => line.push(
                    pad_to(&format!(" {}", clip(&labels[gi], cols.proj - 3)), cols.proj),
                    if gone {
                        dim()
                    } else {
                        Style::default().add_modifier(Modifier::BOLD)
                    },
                ),
            }

            // 行尾右对齐的那几个标记。
            let mut meta: Vec<(String, Style)> = Vec::new();
            match row.session_index() {
                None => {
                    // 组头：状态那一列写出错几个，跟会话行的「✕ 出错了」对齐。
                    line.push(" ".repeat(ID_COLS), Style::default());
                    if gone {
                        line.push(text(Key::ProjectDirGone, lang).into(), dim());
                    } else if g.failed() > 0 {
                        line.push(
                            format!("✕ {}", msg::failed_count(lang, g.failed())),
                            danger(),
                        );
                    }
                }
                Some((_, si)) => {
                    let s = &g.sessions[si];
                    let (dot, style) = state_mark(s.state);
                    line.push(format!("{:>3}  ", s.id), dim());
                    line.push(format!("{dot} "), style);
                    line.push(
                        pad_to(&state_text(s, lang, now_ms), cols.status + GAP),
                        style,
                    );
                    line.push(
                        pad_to(&clip(&s.profile, cols.agent), cols.agent + GAP),
                        dim(),
                    );
                    if cols.name > 0 {
                        line.push(
                            pad_to(&clip(session_name(s), cols.name), cols.name + GAP),
                            Style::default(),
                        );
                    }
                    debug_assert_eq!(line.w, cols.prefix());
                    // 上架标记：老师切到别的会话时，得一眼认出「还在播的是这几路」
                    // （只有直播面板上勾过的那几行，`App::live.staged`）。
                    if app.live.staged.iter().any(|(id, _)| *id == s.id) {
                        meta.push((text(Key::LiveOnAirMark, lang).into(), accent()));
                    }
                    // 「✉ N」：这次运行期间别的电脑送进这个会话几条留言。
                    let n = app.mesh.messages_for(s.id);
                    if n > 0 {
                        meta.push((msg::mesh_messages_mark(n), accent()));
                    }
                    if gone && matches!(row, Row::Single(_)) {
                        meta.push((text(Key::ProjectDirGone, lang).into(), dim()));
                    }
                    let meta_w: usize = meta.iter().map(|(m, _)| display_width(m) + 1).sum();
                    // 空闲的会话那一列留空：它停着，屏幕最后一行上剩下的只是
                    // 它自己的输入框和底栏，说明不了任何事。
                    let activity = if s.state == SessionState::Idle {
                        String::new()
                    } else {
                        clip(
                            &s.activity,
                            width.saturating_sub(line.w + meta_w).min(ACTIVITY_MAX),
                        )
                    };
                    let style = if s.state == SessionState::Working {
                        Style::default()
                    } else {
                        dim()
                    };
                    line.push(activity, style);
                }
            }
            if !meta.is_empty() {
                let meta_w: usize = meta.iter().map(|(m, _)| display_width(m) + 1).sum();
                line.push(
                    " ".repeat(width.saturating_sub(line.w + meta_w)),
                    Style::default(),
                );
                for (m, style) in meta {
                    line.push(format!(" {m}"), style);
                }
            }
            let item_style = if selected == Some(i) {
                paint_selected(&mut line.spans, app.bar)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(line.spans)).style(item_style)
        })
        .collect()
}

/// 选中那一行整行铺上底栏同一档的实色，返回这一行的底色样式。
///
/// 只有一个 `▶` 的话，二十几行里找光标要先找那个小三角；整行铺色是一眼的事。
/// 色借底栏的：那几档是用户自己在设置页挑的，对比度有 `every_bar_theme_is_readable`
/// 守着——这里不另挑颜色，也就不会有第二份要守的色表。
///
/// 铺了底色之后，每一段原来的前景色都可能压不住它，所以一律退回底栏自己的
/// 前景色，只留加粗/反白这些修饰；**唯独红字换成 `bar_danger`**——它自带
/// 底色，十四档上都读得清，出错的那一行被选中时也还是红的。
///
/// 横线档 / `NO_COLOR` 没有实色可铺，原样不动（List 那边退回加粗）。
fn paint_selected(spans: &mut [Span<'static>], bar: super::BarTheme) -> Style {
    let Some(row) = super::bar_style(bar) else {
        return Style::default();
    };
    let red = danger();
    for span in spans.iter_mut() {
        span.style = if span.style == red {
            super::bar_danger(bar)
        } else {
            Style::default()
                .add_modifier(span.style.add_modifier)
                .remove_modifier(Modifier::DIM)
        };
    }
    row
}

/// 把标题下面那块分成三截：邀请码、列表、「我的电脑」。高度不够时先保
/// 邀请码（码一位都不能丢），再给列表留至少一行，剩下的才给底部那一段。
fn split(inner: Rect, prompt: usize, section: usize) -> (Rect, Rect, Rect) {
    let h = inner.height;
    let ph = (prompt.min(u16::MAX as usize) as u16).min(h);
    let rest = h - ph;
    let sh = (section.min(u16::MAX as usize) as u16).min(rest.saturating_sub(1));
    let lh = rest - sh;
    let at = |y: u16, height: u16| Rect {
        x: inner.x,
        y: inner.y + y,
        width: inner.width,
        height,
    };
    (at(0, ph), at(ph, lh), at(ph + lh, sh))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionInfo, SessionState};
    use crossterm::event::KeyModifiers;

    use super::super::view::View;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn sess(id: u32, dir: &str) -> SessionInfo {
        SessionInfo {
            id,
            profile: "claude".into(),
            dir: dir.into(),
            state: SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
            last_active_ms: 0,
        }
    }

    /// 一个干活中的会话：空闲的会话在看板上不写 activity，量 activity 列的
    /// 测试得用它。
    fn busy(id: u32, dir: &str) -> SessionInfo {
        SessionInfo {
            state: SessionState::Working,
            ..sess(id, dir)
        }
    }

    /// 磁盘上真实存在的一个项目目录。组头上的 agent 统计只在
    /// `dir.exists()` 时才画，所以断言统计的测试不能用凭空编的路径。
    fn real_dir(base: &tempfile::TempDir, name: &str) -> String {
        let p = base.path().join(name);
        std::fs::create_dir_all(&p).unwrap();
        p.display().to_string()
    }

    fn screen_text(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        let a = buf.area;
        (0..a.height)
            .flat_map(|y| (0..a.width).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    /// 找到屏幕上包含 `marker` 的那一行，**原样返回，不过滤空白**——跟
    /// `screen_text` 反着来：那边为了好比对内容才把空白洗掉，这里恰恰是要
    /// 保留 `pad_to` 补的空格和它们的列位置，不然量不出两个记号之间隔了
    /// 几列。给下面几条守 `board.rs:211` 列算术的测试用。
    fn row_with(term: &Terminal<TestBackend>, marker: &str) -> String {
        let buf = term.backend().buffer();
        let a = buf.area;
        (0..a.height)
            .map(|y| {
                (0..a.width)
                    .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                    .collect::<String>()
            })
            .find(|row| row.contains(marker))
            .unwrap_or_else(|| panic!("没有任何一行画出了记号 {marker:?}"))
    }

    /// 两个记号之间（含 `from_marker` 自己）隔了几**列**。必须用显示宽度量，
    /// 不能用字节数或字符数：`row.find` 给的是字节偏移，`…`/CJK 在 UTF-8
    /// 里是多字节，字节差和真实列差对不上——这个坑不是抄来的，是写这几条
    /// 测试时自己踩出来的。
    fn cols_between(row: &str, from_marker: &str, to_marker: &str) -> usize {
        let from = row.find(from_marker).unwrap();
        let to = row.find(to_marker).unwrap();
        unicode_width::UnicodeWidthStr::width(&row[from..to])
    }

    /// **核心回归测试，钉在渲染出的 buffer 这一层**（`fix-1-brief.md`
    /// 明确要的层级——见 `fix-1-report.md` 里对第一轮 review 的回应）。
    ///
    /// `session::tests` 里同名意图的测试断言的是 `SessionInfo.tag`，那是
    /// 渲染的**输入**，不是渲染的**结果**：`tag` 是 `#[serde(default)]`
    /// 的 JSON 字段，跨守护进程↔界面的 socket 走（见 `SessionInfo::tag`
    /// 自己的文档），一个连着**旧守护进程**的新界面——这是本仓库里
    /// 记录在案、会反复发生的组合——会原样拿到旧守护进程从来没洗过的
    /// `tag`。`session.rs` 那道 `sanitize` 过滤完全长在守护进程一侧，
    /// 界面这边没有第二道防线；这条测试才是真正验证「就算界面拿到一个
    /// 脏 tag，控制字符最终也画不到屏幕上」的地方。
    ///
    /// 看板列表项走的是 `Span::raw` → `List`/`ListItem` → `Line` →
    /// `Span::render_ref`（`board.rs:211`），跟 `Buffer::set_stringn`/
    /// `Paragraph` 不是同一条路，后两者会过滤控制字符，前者不会
    /// （细节见 `fix-1-brief.md`）。`screen_text` 只过滤空白字符，
    /// 控制字符如果真的穿透渲染，会原样出现在它的输出里。
    ///
    /// **界面侧的防线在 `widgets::truncate` 里**，不在这个测试文件——
    /// `board.rs:211` 那句 `truncate(session_label(s), 15)` 会先经过
    /// `truncate`，控制字符在那一层就被丢弃了（细节和为什么选那一层，
    /// 见 `truncate` 自己的文档、`fix-1-report.md` 的 Important 2）。
    ///
    /// **两条断言缺一不可**：只断言「控制字符不在」测不出「这一行到底
    /// 有没有画」——如果哪天改动让这个会话整行都不渲染了、或者
    /// `session_label`/`tag` 干脆不再画出来，控制字符自然也不在
    /// `screen_text` 里，这条测试会**假装通过**，其实什么都没验证到。
    /// 第二条断言钉住「标签洗完之后剩下的那部分文字确实上了屏幕」——
    /// `truncate("\x1b[Afix\x7f", 15)` 丢掉 `ESC` 和 `DEL` 之后剩下
    /// `"[Afix"`（`[` 和 `A` 是转义序列里的普通可打印字符，`truncate`
    /// 不做整条转义序列的识别，只逐字符丢控制字节，见它自己的文档）。
    /// 两条assert 合起来才是「标签画出来了，而且是干净的」。
    /// 会话列表里，上架的那几路要有「● 播」标记：老师切到别的会话时，
    /// 一眼能看出播的仍然是这几路。没上架的会话不该带这个标记。
    #[test]
    fn staged_sessions_are_marked_live_in_the_session_list() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        app.set_sessions(vec![sess(1, &proj), sess(2, &proj)]);
        app.live.staged = vec![(1, "claude".into())];

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        let mark = crate::i18n::text(crate::i18n::Key::LiveOnAirMark, app.lang)
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>();
        assert!(c.contains(&mark), "上架的会话没有播标记：{c}");
        assert_eq!(
            c.matches(&mark).count(),
            1,
            "只有一路上架，标记不该出现第二次：{c}"
        );
    }

    #[test]
    fn a_tag_with_control_bytes_never_reaches_the_rendered_buffer() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        let mut s = sess(1, &proj);
        // 上箭头的转义序列 + 一个字面的退格字节，模拟 fix-1-brief 里
        // 描述的、一个没洗过 tag 的旧守护进程会发过来的东西。
        s.tag = "\x1b[Afix\x7f".into();
        app.set_sessions(vec![s]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(
            !c.chars().any(|ch| ch.is_control()),
            "控制字符穿透渲染，落进了 buffer：{c:?}"
        );
        assert!(
            c.contains("[Afix"),
            "标签清洗剩下的可见部分应该照样画在屏幕上，不能连内容一起被吞掉：{c:?}"
        );
    }

    // 下面四条测的是 `board.rs:211` 那句 `pad_to(&truncate(session_label(s),
    // 15), 16)` 自己——把 profile 那 10 列压到 6 列腾给名字之后，看板列表
    // 对这套算术一条测试都没有（fix-3-brief.md，`final-review-report.md`
    // Finding 6）。四个变异各配一条：
    //   ① session_label(s) → s.profile         → the_session_row_shows_the_name_not_the_profile
    //   ② 删掉 truncate(…, 15)                  → an_oversized_name_is_truncated_before_it_can_eat_the_activity_budget
    //   ③ 删掉 pad_to(…, 16)                    → a_short_name_is_padded_out_to_the_full_sixteen_columns
    //   ④ activity 的 70 改回 76                → the_activity_column_still_truncates_at_seventy_not_seventy_six
    // 算术本身（fix-3-brief.md 的核对）：`truncate(s, 15)` 在真的截断时输出
    // 的宽度**不总是**正好 16——brief 里那句话只在「触发截断前累计宽度恰好
    // 撞满 15」时成立（比如 15 个纯 ASCII 字符打头的输入）；如果触发点提前
    // （例如宽字符让预算在 14 就跳空），输出可能只有 15 列。这不是产品代码的
    // bug：`pad_to(…, 16)` 兜的就是这个差——不管 `truncate` 吐出 15 还是 16，
    // `pad_to` 都会把它填满到 16，所以「这一列最终总是 16 列宽」这个不变量
    // 依然成立，brief 的结论（算术是对的、只需要盖住它）没有问题，只是它对
    // `truncate` 单独产出宽度的描述略有简化。下面四条测的正是这个组合后的
    // 不变量，不依赖 `truncate` 单独的输出宽度是 15 还是 16。

    /// **杀变异①**：`session_name(s)` 被换成 `s.profile`。
    ///
    /// 单会话的项目合成一行，屏幕上只有智能体那一列写着 `claude`——名字列
    /// 如果也被换成 profile，`claude` 就会多冒一次。原来组头上还有一串
    /// `claude×1`，那串统计已经拿掉了（每一行自己写着智能体），所以现在
    /// 正好是一次。
    #[test]
    fn the_session_row_shows_the_name_and_profile() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        let mut s = sess(1, &proj);
        s.tag = "改登录页文案".into(); // 6 个汉字 = 12 列，落在 12 列预算内，不会被截
        app.set_sessions(vec![s]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(c.contains("改登录页文案"), "会话行必须画出名字：{c}");
        assert_eq!(c.matches("claude").count(), 1, "智能体那一列写一次：{c}");
        assert!(!c.contains("claude×1"), "组头上的智能体统计已经拿掉了：{c}");
    }

    #[test]
    fn each_session_row_shows_its_own_agent() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        let mut first = sess(1, &proj);
        first.tag = "FIRST_TASK".into();
        let mut second = sess(2, &proj);
        second.profile = "codex".into();
        second.tag = "SECOND_TASK".into();
        app.set_sessions(vec![first, second]);
        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        assert!(row_with(&term, "FIRST_TASK").contains("claude"));
        assert!(row_with(&term, "SECOND_TASK").contains("codex"));
    }

    /// **杀变异③**：名字那一格的 `pad_to` 被删掉。
    ///
    /// 名字列的宽度是这一屏最长的那个名字（封顶 16）：两个项目，一个名字
    /// 12 列、一个 4 列，短的那个也得补到 12 列再加两格空隙，activity 才对得
    /// 上同一列。`screen_text` 会把空白过滤掉，这里改用 `row_with` 保留原始
    /// 列位置，直接量「名字」到「activity」之间隔了几列。
    #[test]
    fn a_short_name_is_padded_out_to_the_full_sixteen_columns() {
        let (mut app, dir) = App::test_app();
        let mut s = busy(1, &real_dir(&dir, "a"));
        s.tag = "NAME".into(); // 4 列，纯 ASCII——不会触发截断，量起来没有歧义
        s.activity = "ACTV_TAIL".into();
        let mut long = busy(2, &real_dir(&dir, "b"));
        long.tag = "TWELVE_WIDE_".into();
        long.activity = "LONG_TAIL".into();
        app.set_sessions(vec![s, long]);

        let mut term = Terminal::new(TestBackend::new(200, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let row = row_with(&term, "ACTV_TAIL");
        assert_eq!(
            cols_between(&row, "NAME", "ACTV_TAIL"),
            14,
            "`NAME` 到 `ACTV_TAIL` 应该正好隔 12 + 2 列（补到最长的那个名字）；\
             删掉 `pad_to` 的话这里会变成 4（tag 自己的宽度，一点没补）：{row:?}"
        );
        let other = row_with(&term, "LONG_TAIL");
        assert_eq!(
            row.find("ACTV_TAIL").map(|i| unicode_width::UnicodeWidthStr::width(&row[..i])),
            other.find("LONG_TAIL").map(|i| unicode_width::UnicodeWidthStr::width(&other[..i])),
            "两行的 activity 必须落在同一列：\n{row}\n{other}"
        );
    }

    /// **杀变异②**：名字那一格的 `clip` 被删掉。
    ///
    /// tag 给到 20 列（全 ASCII，量起来没有 CJK 的歧义），宽屏上名字列封顶
    /// 16：裁成「15 个字符 + 一个省略号」= 16 列，再加两格空隙。删掉裁剪的话
    /// 整条 tag 原样画出来，`LONGNAME` 到 `ACTV_TAIL` 之间就成了 22。
    #[test]
    fn an_oversized_name_is_truncated_before_it_can_eat_the_activity_budget() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        let mut s = busy(1, &proj);
        s.tag = format!("LONGNAME{}", "X".repeat(12)); // 20 列
        s.activity = "ACTV_TAIL".into();
        app.set_sessions(vec![s]);

        let mut term = Terminal::new(TestBackend::new(200, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let row = row_with(&term, "ACTV_TAIL");
        assert_eq!(
            cols_between(&row, "LONGNAME", "ACTV_TAIL"),
            18,
            "`LONGNAME…` 到 `ACTV_TAIL` 应该正好隔 16 + 2 列；删掉裁剪的话\
             整条 20 列的 tag 会原样画出来，这里会变成 22：{row:?}"
        );
    }

    /// **杀变异④**：activity 的截断预算从 70 被改回 76。
    ///
    /// 造一条 74 列长的 activity（70 个 `A` 紧跟着记号 `MARK`）：预算是 70
    /// 的话，第 71 列就会撞上截断，`MARK` 连一个字都露不出来；预算一旦变成
    /// 76，前 74 列全放得下，`MARK` 会整个冒出来。「`MARK` 在不在屏幕上」
    /// 直接就是「预算是不是 70」的答案，不用量列。
    #[test]
    fn the_activity_column_still_truncates_at_seventy_not_seventy_six() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        let mut s = busy(1, &proj);
        s.activity = format!("{}MARK", "A".repeat(70));
        app.set_sessions(vec![s]);

        // 200 列：activity 自己的 70 列预算不该被行宽提前掐断，
        // 这条测的是 `truncate` 里的数字，不是终端宽度。
        let mut term = Terminal::new(TestBackend::new(200, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(
            c.contains(&"A".repeat(70)),
            "70 个 A 应该都画出来了，不然下面「MARK 不在」的断言就是空的：{c}"
        );
        assert!(
            !c.contains("MARK"),
            "70 列预算下 MARK 连一个字都不该露出来；如果预算被改回 76，\
             MARK 会整个冒出来：{c}"
        );
    }

    /// 第 0 行（标题那一行）的文字，空白洗掉——`TestBackend` 把一个双宽
    /// 汉字画成「字 + 空格」两格，不洗的话 `contains("会话看板")` 永远不成立。
    fn title_text(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()))
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    /// 屏幕上所有**反白**格子的符号，按行优先拼起来（空白同样洗掉）。三处
    /// 项目标识（标题的牌子、组头的牌子、底栏的牌子——底栏不在这个 widget
    /// 里）都是靠 `REVERSED` 认的，用具名色断言会跟六档底栏配色打架。
    fn reversed_symbols(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        let a = buf.area;
        (0..a.height)
            .flat_map(|y| (0..a.width).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)))
            .filter(|c| c.style().add_modifier.contains(Modifier::REVERSED))
            .map(|c| c.symbol().to_string())
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    /// **当前项目在组头上反白成一块牌子，别的项目没有。**
    ///
    /// 改动前它只有左边一条 1 列宽的竖线，而名字本身跟别的项目名一样是
    /// 加粗的——「屏幕上哪几行是我的」于是全压在那一列上。
    #[test]
    fn the_current_project_wears_a_chip_on_its_group_row() {
        let (mut app, dir) = App::test_app();
        let mine = real_dir(&dir, "aaa-mine");
        let other = real_dir(&dir, "zzz-other");
        app.set_sessions(vec![sess(1, &mine), sess(2, &other)]);
        // 组是按名字排的，`set_sessions` 之后光标停在第一行——也就是 aaa-mine
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("aaa-mine".to_string()),
            "前提：光标停在 aaa-mine 那一组"
        );

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let rev = reversed_symbols(&term);
        assert!(rev.contains("aaa-mine"), "当前项目要反白：{rev}");
        assert!(
            !rev.contains("zzz-other"),
            "别的项目不许反白，否则牌子什么都没区分出来：{rev}"
        );
    }

    /// **牌子只包住名字，不许把补白一起反白。** 反白连着补满一列的话，
    /// 短名字后面会拖出一条长尾巴，看着像进度条。牌子两端各一个空格，
    /// 所以反白的宽度恰好是「名字 + 2」。
    #[test]
    fn the_chip_hugs_the_name_instead_of_filling_the_column() {
        let (mut app, dir) = App::test_app();
        let mine = real_dir(&dir, "dct");
        app.set_sessions(vec![sess(1, &mine)]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        // 当前项目那一行（行首竖色条 `┃`）里反白的格子：只有 ` dct `
        // （标题那块牌子在第 0 行）
        let buf = term.backend().buffer();
        let row = (1..buf.area.height)
            .find(|&y| {
                (0..buf.area.width).any(|x| buf.cell((x, y)).map(|c| c.symbol()) == Some("┃"))
            })
            .expect("没有当前项目那一行");
        let on_row: String = (0..buf.area.width)
            .filter_map(|x| buf.cell((x, row)))
            .filter(|c| c.style().add_modifier.contains(Modifier::REVERSED))
            .map(|c| c.symbol().to_string())
            .collect();
        assert_eq!(on_row, " dct ", "牌子该是名字加左右各一个空格：{on_row:?}");
    }

    /// **牌子不许把项目那一列撑宽。** 牌子的两个空格是从项目列自己的预算里
    /// 出的，不是从右边邻居那里借的——右边是状态列，组头的红字「N 个出错」
    /// 就写在那儿，它被挤掉是 dct 最贵的失败模式。
    #[test]
    fn the_chip_does_not_widen_the_name_column() {
        let (mut app, dir) = App::test_app();
        // 两个组：一个戴牌子（光标所在），一个没戴。两行的会话号必须落在
        // 同一列上，否则就是牌子把列撑宽了。
        let mine = real_dir(&dir, "mine-project-here");
        let other = real_dir(&dir, "other-project-x");
        app.set_sessions(vec![sess(41, &mine), sess(42, &other)]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        // 用会话号当记号（前后带空隙）：完整路径也写在**标题**那一行，拿
        // 项目名找行会先撞上标题。
        let chipped = row_with(&term, "  41  ");
        let plain = row_with(&term, "  42  ");
        assert!(chipped.contains('┃'), "前提：41 那一行是当前项目：{chipped}");
        let col = |row: &str, m: &str| {
            unicode_width::UnicodeWidthStr::width(&row[..row.find(m).unwrap()])
        };
        assert_eq!(
            col(&chipped, "41"),
            col(&plain, "42"),
            "戴牌子的那一行把项目列撑宽了：\n{chipped}\n{plain}"
        );
    }

    /// **标题上也写当前项目，写的是完整路径。** 组头那 18 列写不全路径，
    /// 而「我在哪个项目」值得写全一次；标题这一行宽度有余。
    #[test]
    fn the_title_carries_the_current_project() {
        let (mut app, dir) = App::test_app();
        let mine = real_dir(&dir, "dc-terminal");
        app.set_sessions(vec![sess(1, &mine)]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let title = title_text(&term);
        assert!(title.contains("dct会话看板"), "标题本身还在：{title}");
        assert!(title.contains("dc-terminal"), "标题上要写当前项目：{title}");
        let buf = term.backend().buffer();
        let on_title: String = (0..buf.area.width)
            .filter_map(|x| buf.cell((x, 0)))
            .filter(|c| c.style().add_modifier.contains(Modifier::REVERSED))
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            on_title.contains("dc-terminal"),
            "标题上那一段要反白成牌子，跟组头是同一个记号：{on_title}"
        );
    }

    /// **断连那半句永远接在牌子后面，而且两者都在场。** 断连是最想知道
    /// 自己在哪的那一屏：它挤长了只该吃掉标题右边那条横线。
    #[test]
    fn a_disconnected_title_keeps_both_the_project_and_the_warning() {
        let (mut app, dir) = App::test_app();
        let mine = real_dir(&dir, "dc-terminal");
        app.set_sessions(vec![sess(1, &mine)]);
        app.connected = false;

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let title = title_text(&term);
        let name_at = title
            .find("dc-terminal")
            .unwrap_or_else(|| panic!("少了项目名：{title}"));
        let warn_at = title
            .find("连接已断开")
            .unwrap_or_else(|| panic!("少了断连提示：{title}"));
        assert!(name_at < warn_at, "牌子要在断连那半句前面：{title}");
    }

    /// **宽屏下标题行右端写着运行版本号。** 断言用的是
    /// `env!("CARGO_PKG_VERSION")`，不是某个写死的字符串——版本号一发新版
    /// 就会变，字面量断言下一次发版就得红。
    #[test]
    fn the_title_shows_the_running_version_when_there_is_room() {
        let (mut app, dir) = App::test_app();
        let mine = real_dir(&dir, "dc-terminal");
        app.set_sessions(vec![sess(1, &mine)]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let title = title_text(&term);
        assert!(
            title.contains(env!("CARGO_PKG_VERSION")),
            "宽屏下标题行要写版本号：{title}"
        );
    }

    /// **窄屏下版本号是第一个让路的东西**，项目名和断连提示都还得在。
    ///
    /// 这条测试钉住优先级：版本号在这一行上最不值钱，横线可以被压没、
    /// 牌子可以退化、断连那句红字最后才可能被牺牲，但版本号必须先消失。
    /// 少了这条测试，下一个碰这段代码的人不会知道这个顺序是刻意的。
    #[test]
    fn a_narrow_title_drops_the_version_before_anything_else() {
        let (mut app, dir) = App::test_app();
        // 名字选得够短，保证 60 列上不会被牌子自己的退化逻辑先截断——
        // 这条测试要钉住的是版本号的优先级，不是牌子退化的行为，两者
        // 混在一起断言，牌子那边的正常截断会被误读成版本号抢了它的位置。
        let mine = real_dir(&dir, "app");
        app.set_sessions(vec![sess(1, &mine)]);
        app.connected = false;

        let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let title = title_text(&term);
        assert!(
            !title.contains(env!("CARGO_PKG_VERSION")),
            "60 列上版本号该已经被让掉了：{title}"
        );
        assert!(title.contains("app"), "项目名不许被版本号挤掉：{title}");
        assert!(
            title.contains("连接已断开"),
            "断连提示不许被版本号挤掉：{title}"
        );
    }

    /// **戴上牌子之后，80 列的中文项目名仍然挤不掉行尾那句红字。**
    ///
    /// `a_long_cjk_project_name_never_pushes_the_failure_count_off_screen`
    /// 守的是同一条列算术，这一条补的是「当前项目」这一支：牌子的两个空格
    /// 走的是名字自己的预算，走错了的话最先没的就是这句红字。
    #[test]
    fn a_chipped_cjk_name_still_leaves_room_for_the_failure_count() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "我的自媒体电商代运营项目");
        let mut bad = sess(2, &proj);
        bad.state = SessionState::Failed;
        app.set_sessions(vec![sess(1, &proj), bad]);

        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(c.contains("1个出错"), "牌子把红字挤下屏幕了：{c}");
        let rev = reversed_symbols(&term);
        assert!(
            rev.contains("我的自媒体"),
            "前提：这一行真的戴着牌子：{rev}"
        );
    }

    /// 目录没了的组不戴牌子：整行标灰是在说「这东西现在是坏的」，反白会
    /// 把它重新变成屏幕上最显眼的东西。
    #[test]
    fn a_group_whose_folder_is_gone_wears_no_chip() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/gone-for-good")]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let buf = term.backend().buffer();
        let on_rows: String = (1..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)))
            .filter(|c| c.style().add_modifier.contains(Modifier::REVERSED))
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            !on_rows.contains("gone-for-good"),
            "坏掉的组不该被反白强调：{on_rows}"
        );
    }

    /// 用户报的症状一：底栏写着「当前项目：dc-terminal」，看板却列着
    /// dc_desktop 的会话。答案不再是「把别的项目藏起来」，而是**分组**：
    /// 两个项目都在屏幕上，各自的会话待在自己那一行（或组头底下），谁属于谁
    /// 一眼可见。
    #[test]
    fn the_board_groups_every_session_under_its_own_project() {
        let (mut app, dir) = App::test_app();
        let one = real_dir(&dir, "dc-terminal");
        let two = real_dir(&dir, "dc_desktop");
        app.set_sessions(vec![sess(1, &one), sess(2, &two)]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(c.contains("dc-terminal"), "两个项目都要在：{c}");
        assert!(c.contains("dc_desktop"), "别的项目不再被藏起来：{c}");
        // 一个会话的项目合成一行：项目名和会话号在同一行上
        assert!(row_with(&term, "dc_desktop").contains("  2  "));
        assert!(!c.contains("claude×1"), "组头上不再数智能体：{c}");
    }

    /// 目录被删了要在组头上说出来，而不是让整个组从看板上消失——
    /// 会话本身还活着（进程的 cwd 早就打开了），消失才是真的找不回来。
    ///
    /// 名字挑一个放得进那 18 列的：名字列现在会截断（见
    /// `a_long_cjk_project_name_never_pushes_the_failure_count_off_screen`），
    /// 而这条问的是「这一行还在不在、有没有说清为什么灰」，不是截断规则。
    #[test]
    fn a_group_whose_folder_is_gone_says_so_instead_of_vanishing() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/gone-for-good")]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(c.contains("gone-for-good"), "组还在看板上：{c}");
        assert!(c.contains("目录不在了"), "并且说清为什么它是灰的：{c}");
    }

    /// 组头上的红字「N 个出错」。会话静默失败是 dct 最贵的失败模式——
    /// 组折起来的时候，这一行是屏幕上唯一还说得出「这个项目里出事了」的地方。
    #[test]
    fn a_group_header_calls_out_how_many_sessions_failed() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        let mut bad = sess(2, &proj);
        bad.state = SessionState::Failed;
        app.set_sessions(vec![sess(1, &proj), bad]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        assert!(
            screen_text(&term).contains("1个出错"),
            "组头必须点出出错的数量：{}",
            screen_text(&term)
        );
    }

    /// **长项目名不许把「N 个出错」挤下 80 列的屏幕。**
    ///
    /// 目标用户是中文用户，`我的自媒体电商代运营项目` 这样 24 列宽的目录名
    /// 是日常，不是边角。名字那一列原来只补不截（`pad_to`），于是整行被右推，
    /// 80 列上先被吃掉的正是行尾那句红字——而它是组折起来的时候，屏幕上
    /// 唯一还说得出「这个项目里出事了」的地方。名字裁短了还认得出是哪个项目，
    /// 红字被裁掉了就什么线索都不剩。
    #[test]
    fn a_long_cjk_project_name_never_pushes_the_failure_count_off_screen() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "我的自媒体电商代运营项目");
        let mut bad1 = sess(2, &proj);
        bad1.state = SessionState::Failed;
        let mut bad2 = sess(3, &proj);
        bad2.state = SessionState::Failed;
        app.set_sessions(vec![sess(1, &proj), bad1, bad2]);

        // 80 列是最常见的终端下限，也正是这一行最先崩的地方
        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(
            c.contains("2个出错"),
            "80 列下这句红字被长项目名挤掉了：{c}"
        );
        assert!(
            c.contains("我的自媒体电商"),
            "名字可以裁短，但要还认得出是哪个项目：{c}"
        );
    }

    #[test]
    fn saved_empty_projects_leave_a_start_hint_instead_of_rows() {
        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            let (mut app, _dir) = App::test_app();
            app.lang = lang;
            app.pinned = vec!["/w/unused-project".into()];
            app.set_sessions(vec![]);
            let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
            term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
            let c = screen_text(&term);
            assert!(!c.contains("unused-project"));
            let hint = text(Key::NoSessionsChooseProject, lang).replace(' ', "");
            assert!(c.contains(&hint), "空看板必须说明如何开始：{c}");
        }
    }

    /// 进会话不会报告「已切到」——光标没离开过它所在的组，项目根本没变，
    /// 也没有什么可以报告的。
    #[test]
    fn entering_a_session_never_announces_a_project_change() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/b")]);
        // 行：[a（会话 1）, b（会话 2）]，各合成一行
        app.list_state.select(Some(1));
        assert_eq!(
            app.current_dir(),
            PathBuf::from("/w/b"),
            "前提：光标在 b 上"
        );

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();

        assert!(matches!(app.view, View::Attached(2)), "合成行上的 Enter 进的是会话 2");
        assert!(
            !app.message.text.contains("已切到"),
            "没有任何项目被改写，不该报告：{}",
            app.message.text
        );
    }

    /// 光标停在组头上时按 Enter 没有会话可进，不能 panic、也不能进错一个。
    /// 组头只有两个以上会话的组才有（一个会话的组合成一行，Enter 进的就是它）。
    #[test]
    fn enter_on_a_group_header_does_nothing() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/a")]);
        app.list_state.select(Some(0));

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();

        assert!(matches!(app.view, View::Board), "组头上没有会话可进");
    }

    /// Tab 是日常换项目的主路径：一步跳到下一个组的头（组头或合成的那一行），
    /// 到头回绕。
    #[test]
    fn tab_jumps_to_the_next_project_and_wraps() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/a"), sess(3, "/w/a"), sess(2, "/w/b")]);
        app.list_state.select(Some(0));

        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.current_dir(), PathBuf::from("/w/b"));
        assert_eq!(app.list_state.selected(), Some(3), "落在 b 合成的那一行上");

        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.current_dir(), PathBuf::from("/w/a"), "到头回绕");
        assert_eq!(app.list_state.selected(), Some(0), "落在 a 的组头上");
    }

    /// 数字键直达前九个项目；越界什么都不做——按了 `7` 而只有两个项目时，
    /// 不动比跳到最后一个更好懂。
    #[test]
    fn digits_go_straight_to_a_project_and_ignore_the_ones_that_are_not_there() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/b")]);
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        )
        .unwrap();
        assert_eq!(app.current_dir(), PathBuf::from("/w/b"));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE),
        )
        .unwrap();
        assert_eq!(
            app.current_dir(),
            PathBuf::from("/w/b"),
            "第七个项目不存在，原地不动"
        );
    }

    /// 折叠：组里的会话行收起来，光标退回组头——不然它会停在一行已经
    /// 不存在的会话上。
    #[test]
    fn collapsing_a_group_hides_its_sessions_and_keeps_the_cursor_on_the_header() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/a")]);
        app.list_state.select(Some(2));

        handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)).unwrap();

        assert_eq!(app.rows.len(), 1, "折起来只剩组头");
        assert_eq!(app.list_state.selected(), Some(0));

        handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.rows.len(), 3, "再按一次展开");
    }

    #[test]
    fn g_enters_the_grid_focused_on_the_selected_session() {
        // 两个视图对「当前是哪个会话」的认知必须一致：列表选中第三个会话，
        // 按 g 之后焦点就该落在第三格，不能弹回第一格。
        let (mut app, _dir) = App::test_app();
        app.set_sessions(
            (1..=4)
                .map(|id| SessionInfo {
                    id,
                    profile: "claude".into(),
                    dir: "/tmp/a".into(),
                    state: SessionState::Idle,
                    activity: String::new(),
                    is_agent: true,
                    tag: String::new(),
                    last_active_ms: 0,
                })
                .collect(),
        );
        // 行：[组头, 1, 2, 3, 4]，第三个会话在第 3 行
        app.list_state.select(Some(3));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(matches!(app.view, View::Grid { focus: 2, .. }));
        assert_eq!(
            app.view_mode,
            crate::ui::ViewMode::Grid,
            "g 切的是**模式**，不是打开一个附属页面——下次回家也该落在九宫格"
        );
    }

    // —— 多电脑：「我的电脑」、加入确认、留言计数 ——

    use super::super::computers::tests::mesh_view;

    /// 每一行，空白洗掉（CJK 宽字符后面那一格是空白）。
    fn rows(term: &Terminal<TestBackend>) -> Vec<String> {
        let buf = term.backend().buffer();
        let a = buf.area;
        (0..a.height)
            .map(|y| {
                (0..a.width)
                    .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                    .collect::<String>()
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect()
            })
            .collect()
    }

    fn draw_mesh(view: Option<crate::proto::MeshView>, w: u16, h: u16) -> Terminal<TestBackend> {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        app.set_sessions(vec![sess(1, &proj)]);
        app.mesh.view = view;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        term
    }

    /// 没登录多电脑：底部只有一行灰字，不列名单、不画标题。
    #[test]
    fn not_logged_in_shows_a_single_gray_line_at_the_bottom() {
        let term = draw_mesh(Some(mesh_view(false, &[])), 80, 12);
        let r = rows(&term);
        assert_eq!(r[11], "多电脑未开启·运行dctlogin");
        assert!(!r[10].contains("我的电脑"), "{r:?}");
        assert_eq!(r.iter().filter(|l| l.contains("多电脑未开启")).count(), 1);
        let buf = term.backend().buffer();
        let first = (0..80)
            .find(|x| buf.cell((*x, 11)).is_some_and(|c| c.symbol() == "多"))
            .unwrap();
        assert_eq!(
            buf.cell((first, 11)).unwrap().style().add_modifier,
            crate::ui::dim().add_modifier,
            "是灰字"
        );
        assert_eq!(
            buf.cell((first, 11)).unwrap().fg,
            crate::ui::dim().fg.unwrap_or_default()
        );
    }

    /// 还没问到过（守护进程太老、刚启动）：那一段整个不画，不猜「没登录」。
    #[test]
    fn nothing_is_drawn_before_the_first_answer() {
        let r = rows(&draw_mesh(None, 80, 12));
        assert!(r[1].contains("proj"), "{r:?}");
        assert!(r[3..].iter().all(|l| l.is_empty()), "{r:?}");
    }

    /// 三台电脑：本机在最前，然后在线的，然后离线的；实心圆是在线。
    #[test]
    fn three_computers_are_listed_with_their_state() {
        let v = mesh_view(
            true,
            &[
                ("云服务器", false, false),
                ("公司Windows", true, false),
                ("家里Mac", true, true),
            ],
        );
        let r = rows(&draw_mesh(Some(v), 80, 12));
        assert_eq!(
            r[8..],
            [
                "我的电脑",
                "●家里Mac本机",
                "●公司Windows在线",
                "○云服务器离线"
            ],
            "{r:?}"
        );
    }

    /// 超过 5 台：列 5 台，剩下的说「还有 N 台」。
    #[test]
    fn more_than_five_computers_say_how_many_more() {
        let names = ["A1", "A2", "A3", "A4", "A5", "A6", "A7"];
        let members: Vec<(&str, bool, bool)> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (*n, true, i == 0))
            .collect();
        let r = rows(&draw_mesh(Some(mesh_view(true, &members)), 80, 14));
        assert_eq!(r[13], "还有2台", "{r:?}");
        assert_eq!(r[7], "我的电脑");
        assert_eq!(
            r[8..13],
            ["●A1本机", "●A2在线", "●A3在线", "●A4在线", "●A5在线"]
        );
        assert!(!r.iter().any(|l| l.contains("A6")));
    }

    /// 发着邀请码：标题下面一行黄字（码、有效期、倒计时），再一行灰字说
    /// 新电脑上敲什么；会话行被挤到下面。
    fn with_code() -> crate::proto::MeshView {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut v = mesh_view(
            true,
            &[
                ("家里Mac", true, true),
                ("一个特别特别长的电脑名字啊啊啊", false, false),
            ],
        );
        v.invite = Some(crate::proto::InviteView {
            id: 1,
            code: "482913".into(),
            expires_at: now + 300,
        });
        v
    }

    #[test]
    fn a_live_code_shows_under_the_title_with_the_hint() {
        let term = draw_mesh(Some(with_code()), 80, 12);
        let r = rows(&term);
        assert!(r[1].starts_with("邀请码482913·10分钟内有效·还剩"), "{r:?}");
        assert_eq!(r[2], "在新电脑上运行：dctjoin482913（码只能用一次）");
        let buf = term.backend().buffer();
        assert_eq!(
            buf.cell((0, 1)).unwrap().fg,
            crate::ui::theme_now().asking().fg.unwrap(),
            "码那一行是「等你拍板」那一档暖色"
        );
        assert!(r.iter().any(|l| l.contains("proj")));
    }

    /// 窄终端上折行，码一位都不能丢；矮到放不下时先保码；再窄再矮也不 panic。
    #[test]
    fn narrow_and_tiny_terminals_keep_the_code_and_never_panic() {
        let v = with_code();
        let c: String = rows(&draw_mesh(Some(v.clone()), 30, 20)).concat();
        assert!(c.contains("482913"), "{c}");
        let r = rows(&draw_mesh(Some(v.clone()), 80, 6));
        assert!(r[1].contains("482913"), "{r:?}");
        assert!(r[3].contains("proj"), "列表还剩一行：{r:?}");
        for (w, h) in [(80, 24), (30, 10), (30, 3), (10, 5), (1, 1), (2, 2), (0, 0)] {
            draw_mesh(Some(v.clone()), w, h);
            draw_mesh(Some(mesh_view(false, &[])), w, h);
        }
    }

    /// 会话收到过留言：那一行末尾「✉ N」。activity 再长也不能把它挤出屏幕。
    #[test]
    fn a_session_that_got_messages_shows_the_count_at_the_end_of_its_row() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        // 干活中：空闲的会话不写 activity，那样这条长 activity 就挤不到谁了
        let mut a = busy(1, &proj);
        a.activity = "x".repeat(200);
        app.set_sessions(vec![a, sess(2, &proj)]);
        let mut v = mesh_view(true, &[("家里Mac", true, true)]);
        v.messages = [(1, 3)].into_iter().collect();
        app.mesh.view = Some(v);
        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let row = row_with(&term, "✉");
        assert!(row.trim_end().ends_with("✉ 3"), "{row:?}");
        assert!(row.contains("  1  "), "是会话 #1 那一行：{row:?}");
        assert_eq!(rows(&term).iter().filter(|l| l.contains('✉')).count(), 1);
    }

    // —— 一组一行、列对齐、选中行 ——

    /// 原样的每一行（不洗空白），右边的空白去掉。贴进报告、量列都用它。
    fn raw_rows(term: &Terminal<TestBackend>) -> Vec<String> {
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0;
                while x < buf.area.width {
                    let sym = buf.cell((x, y)).map(|c| c.symbol().to_string()).unwrap_or_default();
                    // 双宽字符在 buffer 里占两格，第二格是空串：跳过它，不然
                    // 每个汉字后面都会多出一个空格。
                    x += unicode_width::UnicodeWidthStr::width(sym.as_str()).max(1) as u16;
                    line.push_str(&sym);
                }
                line.trim_end().to_string()
            })
            .collect()
    }

    /// **一个会话的项目只占一行，两个以上的才有标题行。** 22 个会话的看板
    /// 原来每个项目两行，一屏装不下；组头那一行除了项目名什么都没说。
    #[test]
    fn a_single_session_project_is_one_line_and_a_busier_one_keeps_its_title() {
        let (mut app, dir) = App::test_app();
        let one = real_dir(&dir, "aaa-one");
        let two = real_dir(&dir, "bbb-two");
        app.set_sessions(vec![sess(1, &one), sess(2, &two), sess(3, &two)]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let r = raw_rows(&term);

        // 第 0 行是标题；列表从第 1 行开始
        assert!(r[1].contains("aaa-one") && r[1].contains("  1  "), "aaa 合成一行：{r:#?}");
        assert!(r[2].contains("bbb-two") && !r[2].contains("  2  "), "bbb 的标题行：{r:#?}");
        // 树枝紧贴会话号，不是飘在行首
        assert!(r[3].contains("├   2  "), "{r:#?}");
        assert!(r[4].contains("└   3  "), "{r:#?}");
        assert!(r[5].is_empty(), "三个会话、两个项目，一共四行：{r:#?}");
        assert!(!r.iter().any(|l| l.contains('×')), "不再数智能体：{r:#?}");
    }

    /// **会话号、状态、智能体在三种行上落在同一列。**
    #[test]
    fn single_rows_and_grouped_rows_share_one_grid() {
        let (mut app, dir) = App::test_app();
        let one = real_dir(&dir, "a");
        let two = real_dir(&dir, "a-much-longer-name");
        let mut x = busy(7, &one);
        x.profile = "codex".into();
        app.set_sessions(vec![x, sess(12, &two), sess(301, &two)]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let r = raw_rows(&term);
        let col = |row: &str, m: &str| {
            let at = row.find(m).unwrap_or_else(|| panic!("{m:?} 不在 {row:?}"));
            unicode_width::UnicodeWidthStr::width(&row[..at])
        };
        // 会话号右对齐：个位、两位、三位数的最后一位在同一列
        assert_eq!(col(&r[1], "7 "), col(&r[3], "12 ") + 1, "{r:#?}");
        assert_eq!(col(&r[3], "12 "), col(&r[4], "301 ") + 1, "{r:#?}");
        // 状态圆点和智能体名
        assert_eq!(col(&r[1], "●"), col(&r[3], "○"), "{r:#?}");
        assert_eq!(col(&r[1], "codex"), col(&r[4], "claude"), "{r:#?}");
    }

    /// 父目录只在两个项目**同名**时才写：不重名的时候那串路径什么都没说。
    #[test]
    fn the_parent_dir_shows_only_when_two_projects_share_a_name() {
        let (mut app, dir) = App::test_app();
        let a = real_dir(&dir, "one/web");
        let b = real_dir(&dir, "two/web");
        let c = real_dir(&dir, "api");
        app.set_sessions(vec![sess(1, &a), sess(2, &b), sess(3, &c)]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let r = raw_rows(&term);
        assert!(r.iter().any(|l| l.contains("one/web")), "{r:#?}");
        assert!(r.iter().any(|l| l.contains("two/web")), "{r:#?}");
        let api = r.iter().find(|l| l.contains("  3  ")).unwrap();
        assert!(!api.contains("/api"), "不重名就只写名字：{api}");
    }

    /// 空闲的会话：状态写「空闲 N分钟」，activity 留空——它停着，屏幕最后一行
    /// 上剩下的只是它自己的底栏（这里故意给一句没洗过的，旧守护进程就会这么报）。
    #[test]
    fn an_idle_session_says_how_long_and_leaves_its_activity_blank() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        let mut idle = sess(1, &proj);
        idle.activity = "⏵⏵ bypass permissions on".into();
        idle.last_active_ms = now_ms() - (3 * 60 + 5) * 1000;
        let mut working = busy(2, &real_dir(&dir, "qqq"));
        working.activity = "Running gradle assembleDebug build".into();
        app.set_sessions(vec![idle, working]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let r = raw_rows(&term);
        let idle_row = r.iter().find(|l| l.contains("  1  ")).unwrap();
        assert!(idle_row.contains("○ 空闲 3分钟"), "{idle_row}");
        assert!(!idle_row.contains("bypass"), "空闲的不写 activity：{idle_row}");
        let busy_row = r.iter().find(|l| l.contains("  2  ")).unwrap();
        assert!(busy_row.contains("● 干活中"), "{busy_row}");
        assert!(busy_row.contains("Running gradle assembleDebug build"), "{busy_row}");
    }

    /// 80 列、activity 很长：截的是 activity，「N 个出错」和行尾的 `✉ N`
    /// 都还在，单会话行上的「✕ 出错了」也在。
    #[test]
    fn eighty_columns_cut_the_activity_never_the_red_or_the_marks() {
        let (mut app, dir) = App::test_app();
        let multi = real_dir(&dir, "我的自媒体电商代运营项目");
        let mut bad = busy(2, &multi);
        bad.state = SessionState::Failed;
        let mut long = busy(5, &real_dir(&dir, "dc-android"));
        long.tag = "平板视频部署与测试".into();
        long.activity = "general-purpose Running gradle assembleDebug build and more".repeat(3);
        let mut lone_bad = sess(9, &real_dir(&dir, "zzz"));
        lone_bad.state = SessionState::Failed;
        app.set_sessions(vec![sess(1, &multi), bad, long, lone_bad]);
        let mut v = super::super::computers::tests::mesh_view(true, &[("家里Mac", true, true)]);
        v.messages = [(5, 1)].into_iter().collect();
        app.mesh.view = Some(v);

        let mut term = Terminal::new(TestBackend::new(80, 14)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let r = raw_rows(&term);
        assert!(r.iter().any(|l| l.contains("✕ 1 个出错")), "{r:#?}");
        let mail = r.iter().find(|l| l.contains("  5  ")).unwrap();
        assert!(mail.ends_with("✉ 1"), "{mail:?}");
        assert!(mail.contains("general-pur"), "activity 还剩一截：{mail:?}");
        let lone = r.iter().find(|l| l.contains("  9  ")).unwrap();
        assert!(lone.contains("✕ 出错了"), "{lone:?}");
    }

    /// 选中那一行整行铺上底栏的颜色，一直铺到最右边；出错的红字在那一行上
    /// 换成自带底色的那块红，不会被底色吃掉。别的行不铺。
    #[test]
    fn the_selected_row_is_painted_across_the_full_width() {
        let (mut app, dir) = App::test_app();
        let mut bad = sess(1, &real_dir(&dir, "a"));
        bad.state = SessionState::Failed;
        app.set_sessions(vec![bad, sess(2, &real_dir(&dir, "b"))]);
        app.list_state.select(Some(0));
        let bar = super::super::bar_style(app.bar).expect("测试里实色档开着");

        let mut term = Terminal::new(TestBackend::new(100, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let buf = term.backend().buffer();
        assert_eq!(buf.cell((99, 1)).unwrap().style().bg, bar.bg, "选中行铺到最右边");
        assert_eq!(buf.cell((0, 1)).unwrap().style().bg, bar.bg, "▶ 那一格也在色条里");
        assert_ne!(buf.cell((99, 2)).unwrap().style().bg, bar.bg, "别的行不铺");
        let cross = (0..100u16)
            .find(|&x| buf.cell((x, 1)).unwrap().symbol() == "✕")
            .expect("出错的记号");
        assert_eq!(
            buf.cell((cross, 1)).unwrap().style().bg,
            super::super::bar_danger(app.bar).bg,
            "选中行上的红字换成自带底色的那块红"
        );
    }

    /// 合成行上的键：数字键落在它上面、Enter 进它的会话、折叠把它折成一行
    /// `▸` 组头、再展开回到合成行。
    #[test]
    fn digits_enter_and_collapse_all_work_on_a_single_row() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/b")]);
        app.list_state.select(Some(0));

        handle_key(&mut app, KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE)).unwrap();
        assert_eq!(app.selected_session().map(|s| s.id), Some(2), "落在 b 那一行上");

        handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.rows[1], Row::Header(1), "折起来是一行组头");
        assert_eq!(app.list_state.selected(), Some(1));
        assert!(app.selected_session().is_none());

        handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.rows[1], Row::Single(1), "展开回到合成行");

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert!(matches!(app.view, View::Attached(2)), "Enter 进的是它的会话");
    }

    /// 一屏真实的看板（2026-11-26 用户那一屏的形状：22 个会话、几乎每个项目
    /// 一个），120 和 80 列各画一遍：22 个会话一屏装得下，红字都在。
    /// `cargo test realistic_board -- --nocapture` 把两屏打出来看。
    #[test]
    fn a_realistic_board_fits_and_reads_at_120_and_80_columns() {
        let (mut app, dir) = App::test_app();
        let now = now_ms();
        let mut all = Vec::new();
        // (会话号, 项目, 智能体, 状态, 名字, activity, 空闲了几分钟)
        type Spec<'a> = (u32, &'a str, &'a str, SessionState, &'a str, &'a str, u64);
        let mut add = |(id, proj, profile, state, tag, act, idle_min): Spec| {
            let mut s = sess(id, &real_dir(&dir, proj));
            s.profile = profile.into();
            s.state = state;
            s.tag = tag.into();
            s.activity = act.into();
            s.last_active_ms = now - idle_min * 60_000;
            all.push(s);
        };
        use SessionState::{Failed, Idle, Working};
        add((
            47,
            "dc-android",
            "claude",
            Working,
            "平板视频部署与测试",
            "general-purpose Running gradle assembleDebug build",
            0,
        ));
        add((63, "dc-android", "codex", Idle, "阅读OpenAI手机文档", "", 12));
        add((7, "dc-device", "claude", Idle, "现在项目什么样了", "", 140));
        add((12, "dc-thesis", "claude", Working, "论文第三章改写", "1 shell", 0));
        add((3, "dc-terminal", "claude", Working, "看板列表美化", "Editing src/ui/board.rs", 0));
        add((21, "dc-terminal", "claude", Idle, "手机连接复查", "", 47));
        add((
            22,
            "dc-terminal",
            "codex",
            Failed,
            "发版检查",
            "API Error: Connection closed mid-response",
            0,
        ));
        add((5, "dc_desktop", "claude", Idle, "浏览器自动化", "", 3 * 24 * 60));
        add((8, "dc-vault", "kimi", Idle, "知识库导入器", "", 25));
        add((9, "dc-octo", "claude", Working, "设备触手", "Reading 14 files", 0));
        add((11, "dc_classroom", "glm", Idle, "课堂终端修复", "", 5));
        add((14, "dc_classeditor", "claude", Idle, "审批审计", "", 300));
        add((15, "dc_llm", "deepseek", Idle, "", "", 61));
        add((16, "dc-common", "claude", Idle, "共享契约", "", 2));
        add((17, "dc_exam", "qwen-api", Working, "监考实时页", "Running ./gradlew test", 0));
        add((18, "dc-local", "claude", Idle, "托盘菜单", "", 90));
        add((19, "dc-link-contract", "codex", Idle, "消息总线", "", 33));
        add((23, "dc_workspace", "shell", SessionState::Unknown, "", "$ docker ps", 0));
        add((24, "dc-video-mcp", "claude", Idle, "视频摘要", "", 18));
        add((25, "我的自媒体电商代运营项目", "claude", Working, "写小红书文案", "Writing 3 drafts", 0));
        add((26, "dc_gpu_worker", "claude", Idle, "头像生成", "", 400));
        add((27, "dc-device-sim", "claude", Idle, "故障注入", "", 9));
        app.set_sessions(all);
        app.live.staged = vec![(9, "claude".into())];
        let mut v = super::super::computers::tests::mesh_view(true, &[("家里Mac", true, true)]);
        v.messages = [(47, 1)].into_iter().collect();
        app.mesh.view = Some(v);
        app.list_state.select(Some(4));

        for w in [120u16, 80] {
            let mut term = Terminal::new(TestBackend::new(w, 32)).unwrap();
            term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
            let r = raw_rows(&term);
            println!("—— {w} 列 ——");
            for l in &r {
                println!("{l}");
            }
            for id in [47, 63, 7, 12, 3, 21, 22, 5, 27] {
                assert!(
                    r.iter().any(|l| l.contains(&format!(" {id}  "))),
                    "{w} 列：会话 {id} 不在屏幕上：{r:#?}"
                );
            }
            assert!(r.iter().any(|l| l.contains("✕ 1 个出错")), "{w} 列：{r:#?}");
            assert!(r.iter().any(|l| l.ends_with("✉ 1")), "{w} 列：{r:#?}");
            assert!(!r.iter().any(|l| l.contains("bypass") || l.contains('×')), "{r:#?}");
        }
    }
}
