use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem};

use super::app::App;
use super::view::is_plain_key;
use super::widgets::{
    display_width, pad_to, project_label, session_label, status_label, status_style, truncate,
};
use super::{
    accent, danger, dim, open_new_session, open_project_picker, open_secrets, session_action,
};
use crate::i18n::{msg, text, Key};

/// 组头上，最后一格（「还没有会话 …」/ agent 统计）之前的所有东西占几列。
///
/// `2` 是 `List` 为 `highlight_symbol("▶ ")` 在**每一行**预留的宽度——不是
/// 只给选中那一行，这是最容易在算宽度时漏掉的一段。其余依次是：项目色条 1、
/// 序号 3、折叠箭头 2、项目名 18、父目录 18。
const HEADER_PREFIX_COLS: usize = 2 + 1 + 3 + 2 + 18 + 18;

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

    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| {
            let (gi, bar) = match row {
                super::view::Row::Header(g) | super::view::Row::Session(g, _) => {
                    (*g, if Some(*g) == current { "┃" } else { " " })
                }
            };
            let g = &app.groups[gi];
            let mut spans = vec![Span::styled(bar, accent())];
            match row {
                super::view::Row::Header(_) => {
                    // 序号只给前九个组：`1`…`9` 直达。第十个起靠 Tab，
                    // 印一个按不动的号码等于在屏幕上说谎。
                    let num = if gi < 9 {
                        format!(" {} ", gi + 1)
                    } else {
                        "   ".to_string()
                    };
                    spans.push(Span::styled(num, dim()));
                    spans.push(Span::raw(if g.collapsed { "▸ " } else { "▾ " }));
                    // 目录被删了：名字标灰并点出来。会话本身还活着（进程的 cwd
                    // 已经打开），组照常留在看板上——让它消失才是真的找不回来了。
                    let gone = !g.dir.exists();
                    // **先截再补**，跟下面那行父目录一样。`pad_to` 只补不截，
                    // 光靠它的话一个长目录名（`我的自媒体电商代运营项目` 是
                    // 24 列，而中文项目名长成这样再普通不过）会把整行右推，
                    // 80 列上先被吃掉的正是行尾那个红色的「N 个出错」——
                    // 一个坏掉的项目于是看起来一切正常，而这是 dct 最贵的
                    // 失败模式。名字裁短了还看得出是哪个项目，出错的红字被
                    // 裁掉了就什么都没有了。
                    //
                    // 上限取 17 不是 18：`truncate` 裁完会补一个 `…`，17 列
                    // 的内容加那一个字符正好 18，列宽分毫不动。名字比父目录
                    // 那一列（16）宽一格是有意的——认项目靠的是名字。
                    //
                    // 当前项目的名字**反白成一块牌子**，不是加粗了事：屏幕上
                    // 每个项目名都是加粗的，加粗认不出哪个是自己那个。反白是
                    // 这一屏里唯一一块底色反过来的地方，不需要先知道「左边那
                    // 条竖线是什么意思」就跳得出来——而不知道这件事的人正是
                    // 找不到自己在哪的那个人。用 `REVERSED` 而不是挑一个具名
                    // 色的理由跟底栏那块牌子一样（见 `ui/mod.rs` 里那段）：
                    // 六档底栏配色里任何写死的前景色都会在某一档上糊掉。
                    //
                    // 列宽分毫不动：牌子前后各垫一个空格，所以名字的预算从
                    // 17 收到 15（15 列内容 + `truncate` 可能补的那一个 `…`
                    // + 两个空格 = 18）。这一格右边是 agent 统计和行尾那个
                    // 红色的「N 个出错」——它被挤掉是 dct 最贵的失败模式，
                    // 牌子绝不能从别人的列里借宽度。
                    //
                    // 目录没了的组不贴牌子：那一行整行标灰是在说「这东西现在
                    // 是坏的」，反白会把它重新变成屏幕上最显眼的东西。
                    if Some(gi) == current && !gone {
                        // 牌子**只包住名字**，右边补的空白照旧是普通底色：
                        // 反白连着补到 18 列的话，短名字后面会拖出一条长
                        // 尾巴，看着像进度条而不像一块牌子。补白单独一段，
                        // 两段加起来仍然是 18 列。
                        let chip = format!(" {} ", truncate(&g.name, 15));
                        let w = display_width(&chip);
                        spans.push(Span::styled(
                            chip,
                            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
                        ));
                        spans.push(Span::raw(" ".repeat(18usize.saturating_sub(w))));
                    } else {
                        spans.push(Span::styled(
                            pad_to(&truncate(&g.name, 17), 18),
                            if gone {
                                dim()
                            } else {
                                Style::default().add_modifier(Modifier::BOLD)
                            },
                        ));
                    }
                    spans.push(Span::styled(pad_to(&truncate(&g.parent, 16), 18), dim()));
                    if gone {
                        spans.push(Span::styled(text(Key::ProjectDirGone, app.lang), dim()));
                    } else if g.sessions.is_empty() {
                        // 空项目的组头上补一句「上次用的是谁」。
                        //
                        // 「哪个项目用哪个 agent」本来只有底栏那一条 `n 新建
                        // <agent>` 答得出，而底栏在 80 列上会把 agent 名让掉
                        // （`bar_keys` 里那条「让的是半句说明、不是一个键」），
                        // 于是一个还没有会话的项目在整个屏幕上无处可查——而
                        // 这正是这次改造被提出来时的那句话。
                        //
                        // 接在这一格后面而不是另开一列：这一格本来就只有
                        // 「还没有会话」这半句，右边全是空白。从没记过 agent
                        // 的项目照旧只写原来那句，不编一个名字出来。
                        //
                        // 宽度：这一格拿到的是**整行减去前缀**，而前缀里有一段
                        // 容易漏掉的——`List` 给 `highlight_symbol("▶ ")` 在
                        // **每一行**都预留两列，不只是选中那行。算漏它的话
                        // 80 列上这句会正好被右边框切掉尾巴（最长的内置 agent
                        // 名是 8 列：`opencode`/`deepseek`/`qwen-api`）。
                        // 自建 profile 的名字要多长有多长，所以除了把文案收短，
                        // 还得真按剩下的宽度裁一次。
                        //
                        // 传给 `truncate` 的是 `room - 1`，不是 `room`：它**真的
                        // 裁了**的时候返回的是 `max + 1` 列——那个 `…` 是在长度
                        // 判断之后才追加的（`widgets::truncate`，同 `:134` 那行
                        // 父目录为什么写 16 填 18）。照 `room` 传的话，省略号
                        // 自己正好落在边框那一列上被 ratatui 剪掉，屏幕上留下一个
                        // 齐齐整整、看不出被截过的名字——正是这段代码要防的那件事，
                        // 只是换了个更隐蔽的样子。
                        let hint = text(Key::NoSessionsHere, app.lang);
                        let line = match &g.last_profile {
                            Some(p) => {
                                format!("{hint} · {} {p}", text(Key::LastUsedAgent, app.lang))
                            }
                            None => hint.to_string(),
                        };
                        // 列表这个块只画上下边框（`Borders::TOP | Borders::BOTTOM`），
                        // 左右不再吃掉列——复制文字不该带上边框字符——所以
                        // 这里不用再像改动前那样先减 2 补偿左右边框。
                        let room = (area.width as usize).saturating_sub(HEADER_PREFIX_COLS);
                        spans.push(Span::styled(truncate(&line, room.saturating_sub(1)), dim()));
                    } else {
                        let agents: Vec<String> = g
                            .agent_counts()
                            .into_iter()
                            .map(|(name, n)| format!("{name}×{n}"))
                            .collect();
                        spans.push(Span::raw(pad_to(&agents.join(" "), 22)));
                        let failed = g.failed();
                        if failed > 0 {
                            spans.push(Span::styled(msg::failed_count(app.lang, failed), danger()));
                        }
                    }
                }
                super::view::Row::Session(_, si) => {
                    let s = &g.sessions[*si];
                    spans.push(Span::raw(format!("  {:>3}  ", s.id)));
                    spans.push(Span::styled(
                        pad_to(status_label(s.state, app.lang), 8),
                        status_style(s.state),
                    ));
                    // 名字比原来的 profile 那一格宽（10 → 16）：profile 名最长
                    // 8 列，名字是 12 个汉字。多出来的 6 列从 activity 那边收，
                    // 整行总宽不变。传 15 给 truncate 而不是 16 —— 它真裁了的
                    // 时候返回的是 max + 1 列（那个 `…` 是长度判断之后才追加的），
                    // 照 16 传的话省略号会把列宽顶宽一格。
                    spans.push(Span::raw(pad_to(&truncate(session_label(s), 15), 16)));
                    // 会话行不重复项目名——组头已经说了，宽度还给 activity，
                    // 它是屏幕上最先被截断的信息。
                    spans.push(Span::raw(truncate(&s.activity, 70)));
                }
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

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
    f.render_stateful_widget(
        List::new(items).block(block).highlight_symbol("▶ "),
        area,
        &mut app.list_state,
    );
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

    /// **杀变异①**：`session_label(s)` 被换成 `s.profile`。
    ///
    /// 组头本来就会用 `s.profile` 报一句「这个项目里有几个 claude」
    /// （`agent_counts()`，跟 tag 完全无关），所以单看「屏幕上有没有出现
    /// `claude`」测不出这个变异——不管名字列画的是 tag 还是 profile，
    /// `claude` 反正都会因为组头而出现一次。真正能分开两者的是**出现了
    /// 几次**：名字列如果也被换成 profile，`claude` 就会在组头之外再多冒
    /// 一次。
    #[test]
    fn the_session_row_shows_the_name_not_the_profile() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        let mut s = sess(1, &proj);
        s.tag = "改登录页文案".into(); // 6 个汉字 = 12 列，落在 15 列预算内，不会被截
        app.set_sessions(vec![s]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(c.contains("改登录页文案"), "会话行必须画出名字：{c}");
        assert_eq!(
            c.matches("claude").count(),
            1,
            "claude 只该在组头的 agent 统计里出现一次；如果名字列被换回 s.profile，\
             这里会变成 2：{c}"
        );
    }

    /// **杀变异③**：`pad_to(…, 16)` 被删掉。
    ///
    /// 名字比 15 列窄时 `truncate` 根本不会触发省略号，输出原样就是 tag
    /// 本身——这时候把这一列撑到 16 列全靠 `pad_to`。`screen_text` 会把
    /// 空白过滤掉，补没补空格从它的输出里看不出来；这里改用 `row_with` 保留
    /// 原始列位置，直接量「名字」到「activity」之间隔了几列。
    #[test]
    fn a_short_name_is_padded_out_to_the_full_sixteen_columns() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        let mut s = sess(1, &proj);
        s.tag = "NAME".into(); // 4 列，纯 ASCII——不会触发截断，量起来没有歧义
        s.activity = "ACTV_TAIL".into();
        app.set_sessions(vec![s]);

        // 200 列：给 activity 留足空间，不让这一行的宽度上限在这条测试里
        // 掺和进来——这条测的是 `pad_to` 补没补，不是截断裁没裁。
        let mut term = Terminal::new(TestBackend::new(200, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let row = row_with(&term, "ACTV_TAIL");
        assert_eq!(
            cols_between(&row, "NAME", "ACTV_TAIL"),
            16,
            "`NAME` 到 `ACTV_TAIL` 应该正好隔 16 列（`pad_to` 补齐的宽度）；\
             删掉 `pad_to` 的话这里会变成 4（tag 自己的宽度，一点没补）：{row:?}"
        );
    }

    /// **杀变异②**：`truncate(…, 15)` 被删掉。
    ///
    /// tag 给到 20 列（全 ASCII，量起来没有 CJK 的歧义）。真按 15 列截的话，
    /// 触发点正好撞在预算撑满的那一刻（15 个单列字符），输出是「15 个字符 +
    /// 一个省略号」= 16 列，`pad_to` 这时候是空操作。删掉 `truncate` 之后
    /// 整条 tag（20 列）原样画出来，`pad_to` 的目标 16 又补不了负数，名字列
    /// 直接涨到 20 列——`LONGNAME` 记号到 `ACTV_TAIL` 记号之间的列数就是能不能
    /// 抓住这个变异的地方。
    #[test]
    fn an_oversized_name_is_truncated_before_it_can_eat_the_activity_budget() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "proj");
        let mut s = sess(1, &proj);
        s.tag = format!("LONGNAME{}", "X".repeat(12)); // 20 列
        s.activity = "ACTV_TAIL".into();
        app.set_sessions(vec![s]);

        let mut term = Terminal::new(TestBackend::new(200, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let row = row_with(&term, "ACTV_TAIL");
        assert_eq!(
            cols_between(&row, "LONGNAME", "ACTV_TAIL"),
            16,
            "`LONGNAME…` 到 `ACTV_TAIL` 应该正好隔 16 列；删掉 `truncate` 的话\
             整条 20 列的 tag 会原样画出来，这里会变成 20：{row:?}"
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
        let mut s = sess(1, &proj);
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

    /// **牌子只包住名字，不许把补白一起反白。** 反白连着补到 18 列的话，
    /// 短名字后面会拖出一条长尾巴，看着像进度条。牌子两端各一个空格，
    /// 所以反白的宽度恰好是「名字 + 2」。
    #[test]
    fn the_chip_hugs_the_name_instead_of_filling_the_column() {
        let (mut app, dir) = App::test_app();
        let mine = real_dir(&dir, "dct");
        app.set_sessions(vec![sess(1, &mine)]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        // 组头那一行里反白的格子：只有 ` dct `（标题那块牌子在另一行）
        let buf = term.backend().buffer();
        let row = (0..buf.area.height)
            .find(|&y| {
                (0..buf.area.width).any(|x| buf.cell((x, y)).map(|c| c.symbol()) == Some("▾"))
            })
            .expect("没有组头行");
        let on_row: String = (0..buf.area.width)
            .filter_map(|x| buf.cell((x, row)))
            .filter(|c| c.style().add_modifier.contains(Modifier::REVERSED))
            .map(|c| c.symbol().to_string())
            .collect();
        assert_eq!(on_row, " dct ", "牌子该是名字加左右各一个空格：{on_row:?}");
    }

    /// **组头上的牌子不许把名字那一列撑宽。** 牌子的两个空格是从名字自己的
    /// 18 列预算里出的（截断上限 17 → 15），不是从右边邻居那里借的——右边
    /// 是 agent 统计和行尾那句红字，后者被挤掉是 dct 最贵的失败模式。
    #[test]
    fn the_chip_does_not_widen_the_name_column() {
        let (mut app, dir) = App::test_app();
        // 两个组：一个戴牌子（光标所在），一个没戴。两行的父目录列必须
        // 落在同一列上，否则就是牌子把列撑宽了。
        let mine = real_dir(&dir, "mine-project-here");
        let other = real_dir(&dir, "other-project-x");
        app.set_sessions(vec![sess(1, &mine), sess(2, &other)]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        // 用组序号当记号，不用项目名：完整路径也写在**标题**那一行（标题上
        // 那块牌子），拿名字找行会先撞上标题。
        let chipped = row_with(&term, "1 ▾");
        let plain = row_with(&term, "2 ▾");
        // `▾` 到 agent 统计之间的列数：名字列 + 父目录列，两行必须完全一样
        assert_eq!(
            cols_between(&chipped, "▾", "claude×1"),
            cols_between(&plain, "▾", "claude×1"),
            "戴牌子的那一行把名字列撑宽了：\n{chipped}\n{plain}"
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
    /// 两个项目都在屏幕上，各自的会话待在自己的组头底下，谁属于谁一眼可见。
    #[test]
    fn the_board_groups_every_session_under_its_own_project() {
        let (mut app, dir) = App::test_app();
        // 组头上的 agent 统计只在目录真的还在时才画（不在就换成
        // 「目录不在了」），所以这两个项目必须是磁盘上真实存在的目录。
        let one = real_dir(&dir, "dc-terminal");
        let two = real_dir(&dir, "dc_desktop");
        app.set_sessions(vec![sess(1, &one), sess(2, &two)]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(c.contains("dc-terminal"), "两个项目都要有组头：{c}");
        assert!(c.contains("dc_desktop"), "别的项目不再被藏起来：{c}");
        // 组头上写着这个项目里有几个什么 agent
        assert!(c.contains("claude×1"), "组头要数出这个项目里的 agent：{c}");
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

    /// **还没有会话的项目，组头上要写出它上次用的是哪个 agent。**
    ///
    /// 「哪个项目用哪个 agent」在别处只有底栏那一条 `n 新建 <agent>` 答得出，
    /// 而底栏在 80 列上放不下时会把 agent 名让掉——于是一个空项目在整个屏幕
    /// 上没有一处答得出这个问题，而那正是这次改造被提出来时的那句话。
    ///
    /// **两种语言都在 80 列上验一遍**：中文双宽字少、英文单宽词长，谁先撞到
    /// 右边界不是想当然的。
    ///
    /// **用最长的那个 agent 名，不是最短的。** 内置 profile 里最长的是 8 列
    /// （`opencode`/`deepseek`/`qwen-api`，`profiles/` 里数出来的），而
    /// `claude` 只有 6 列——拿 `claude` 测出来的「放得下」在真实的 8 列名字
    /// 上会被右边框静默切掉两列。这个 fixture 把最长的那个也过一遍。
    #[test]
    fn an_empty_project_names_the_agent_it_last_used() {
        // 键名列宽度无关，测的是最短和最长两档都完整
        for agent in ["claude", "opencode"] {
            for (lang, want) in [
                (crate::i18n::Lang::Zh, format!("上次用{agent}")),
                (crate::i18n::Lang::En, format!("lastused{agent}")),
            ] {
                let (mut app, dir) = App::test_app();
                app.lang = lang;
                let proj = real_dir(&dir, "我的项目");
                app.pinned = vec![proj.clone()];
                app.profiles.insert(
                    super::super::view::canon(std::path::Path::new(&proj))
                        .display()
                        .to_string(),
                    agent.into(),
                );
                app.set_sessions(vec![]);
                assert!(app.groups[0].sessions.is_empty(), "前提：一个会话都没有");

                let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
                term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

                let c = screen_text(&term);
                assert!(c.contains(&want), "{lang:?}/80 列下少了「{want}」：{c}");
            }
        }
    }

    /// 自建 profile 的名字要多长有多长。放不下时要看得见一个 `…`——被这一行
    /// 的右边缘无声吃掉的话，用户根本不知道后面还有字。
    ///
    /// **断言必须钉到那个省略号出现在哪一个字之后**，不能只问「这一屏上有没有
    /// `…`」：父目录那一列自己就会裁出一个（临时目录的路径很长），于是
    /// `contains('…')` 无论截断代码在不在都是真的。这条测试第一版就是那么写的，
    /// 删掉整段截断照样绿——评审抓到了，这里记下来。
    ///
    /// 这一条同时从**两个**方向钉住 `HEADER_PREFIX_COLS` 和那次
    /// `saturating_sub(1)`：
    ///
    /// - 截断整个删掉 → 这一格铺到 36 列被 `List` 按区域宽度剪断，屏幕上
    ///   没有省略号；
    /// - 常量小了（`room` 变大）→ 裁得更靠后，`truncate` 返回 `max+1` 列，
    ///   省略号越过这一行的宽度被剪掉，屏幕上同样没有它；
    /// - 常量大了 → 裁得更靠前，省略号在别的字后面。
    ///
    /// 三种都对不上下面那个字面量。
    #[test]
    fn an_over_long_agent_name_is_cut_visibly_not_by_the_border() {
        let (mut app, dir) = App::test_app();
        app.lang = crate::i18n::Lang::En;
        let proj = real_dir(&dir, "我的项目");
        app.pinned = vec![proj.clone()];
        app.profiles.insert(
            super::super::view::canon(std::path::Path::new(&proj))
                .display()
                .to_string(),
            "an-absurdly-long-locally-defined-agent".into(),
        );
        app.set_sessions(vec![]);

        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        // 80 列：这个块只画上下边框，不再吃列，前缀 44，这一格 36 列。
        // `no sessions · last used ` 占 24，`truncate` 拿到 35，于是名字
        // 露出 11 个字符再补一个 `…`——一共 36 列，一列不多一列不少。
        let c = screen_text(&term);
        assert!(
            c.contains("lastusedan-absurdly…"),
            "省略号必须自己也画得出来，而不是被截断悄悄吃掉：{c}"
        );
    }

    /// 内置 profile 名字最长 8 列——`an_empty_project_names_the_agent_it_last_used`
    /// 里那个 `opencode` 是照着 `profiles/` 挑的，不是随手写的。哪天加进来一个
    /// 更长的，这条会红，提醒去看那个宽度还够不够。
    #[test]
    fn no_builtin_agent_name_is_wider_than_the_header_budget_allows() {
        let longest = crate::profile::Profile::builtin_names()
            .into_iter()
            .max_by_key(|n| super::super::widgets::display_width(n))
            .unwrap();
        assert_eq!(
            super::super::widgets::display_width(longest),
            8,
            "内置 agent 名最长 8 列（{longest}）；变了就去复核组头那一格的宽度"
        );
    }

    /// 从没记过 agent 的项目照旧只写原来那句——不编一个名字出来，
    /// 也不留一个「上次用 」的空尾巴。
    #[test]
    fn an_empty_project_with_no_recorded_agent_says_only_what_it_knows() {
        let (mut app, dir) = App::test_app();
        let proj = real_dir(&dir, "崭新项目");
        app.pinned = vec![proj];
        app.set_sessions(vec![]);

        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(c.contains("还没有会话"), "原来那句还在：{c}");
        assert!(!c.contains("上次用"), "没有记录就别起头说一半：{c}");
    }

    /// 「当前项目 = 光标所在的组」，所以进一个别的组的会话既不需要改写
    /// 什么，也没有什么可以报告的——静默改变当前项目正是上一版被判为
    /// 「混乱」的原因，而这一版结构上就不会发生。
    #[test]
    fn entering_a_session_never_announces_a_project_change() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/b")]);
        // 行：[组头 a, 会话 1, 组头 b, 会话 2]
        app.list_state.select(Some(3));
        assert_eq!(
            app.current_dir(),
            PathBuf::from("/w/b"),
            "前提：光标在 b 上"
        );

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();

        assert!(matches!(app.view, View::Attached(2)), "进的是会话 2");
        assert!(
            !app.message.text.contains("已切到"),
            "没有任何项目被改写，不该报告：{}",
            app.message.text
        );
    }

    /// 光标停在组头上时按 Enter 没有会话可进，不能 panic、也不能进错一个。
    #[test]
    fn enter_on_a_group_header_does_nothing() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/a")]);
        app.list_state.select(Some(0));

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();

        assert!(matches!(app.view, View::Board), "组头上没有会话可进");
    }

    /// Tab 是日常换项目的主路径：一步跳到下一个组头，到头回绕。
    #[test]
    fn tab_jumps_to_the_next_project_and_wraps() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/b")]);
        app.list_state.select(Some(0));

        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.current_dir(), PathBuf::from("/w/b"));
        assert_eq!(app.list_state.selected(), Some(2), "落在组头上");

        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.current_dir(), PathBuf::from("/w/a"), "到头回绕");
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
}
