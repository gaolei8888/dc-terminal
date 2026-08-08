//! 九宫格视图：平铺所有会话的实时画面，只读。
//!
//! 上半截是布局数学，全是纯函数，跟终端、协议、会话都没关系，能独立测；
//! 下半截是按键和渲染，跟 `board.rs`/`pick.rs` 一样的 `handle_key` + `draw`。

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Paragraph};

use super::app::App;
use super::view::{is_plain_key, reply_key, Draft, Reply, View};
use super::widgets::Msg;
use super::widgets::{char_width, pad_to, screen_to_lines, status_label, status_style};
use super::{dim, session_action};
use crate::i18n::{text, Key, Lang};
use crate::proto::ScreenEntry;
use crate::pty::ScreenSpan;
use crate::session::{SessionInfo, SessionState};

pub const TILES_PER_PAGE: usize = 9;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// 当页格子数 → （行数，列数）。上限九格；空看板画一个空格子占位，
/// 免得渲染分支到处判零。
pub fn grid_shape(count: usize) -> (u16, u16) {
    match count {
        0 | 1 => (1, 1),
        2 => (1, 2),
        3 | 4 => (2, 2),
        5 | 6 => (2, 3),
        _ => (3, 3),
    }
}

pub fn page_of(focus: usize) -> usize {
    focus / TILES_PER_PAGE
}

pub fn page_count(total: usize) -> usize {
    if total == 0 {
        1
    } else {
        total.div_ceil(TILES_PER_PAGE)
    }
}

/// 焦点在格子间移动。左右在全体会话上一维回绕（越过页边自然翻页）；
/// 上下在当页的二维布局里走，向下越出最后一行收到最后一格。
pub fn move_focus(focus: usize, total: usize, dir: Dir) -> usize {
    if total == 0 {
        return 0;
    }
    // 焦点必须指向一个真实存在的会话。真会踩到的场景是「会话被停掉/清掉了，
    // 但 focus 还没重新收拢」——那时 `total - page_start` 会下溢，release 下
    // 不 panic，只是算出一个天文数字的页长，格子跟着乱。调用方（`run()` 每轮
    // 拉完会话列表之后）负责先把 focus 收进范围，这条断言是那份纪律的哨兵。
    debug_assert!(focus < total, "焦点 {focus} 越出会话总数 {total}");
    let page_start = page_of(focus) * TILES_PER_PAGE;
    let in_page = focus - page_start;
    let page_len = (total - page_start).min(TILES_PER_PAGE);
    let (_, cols) = grid_shape(page_len);
    let cols = cols as usize;
    match dir {
        Dir::Right => (focus + 1) % total,
        Dir::Left => (focus + total - 1) % total,
        Dir::Down => {
            let down = in_page + cols;
            page_start + down.min(page_len - 1)
        }
        Dir::Up => page_start + in_page.saturating_sub(cols),
    }
}

/// 下一个还在跑的会话，按 id 在 `sessions` 里的顺序，到尾回绕，跳过已停止
/// 的（停了的没画面可看，列表视图处理那种情况）。附加视图的 F3 靠它从
/// 当前会话直接跳到下一个能看的会话，不用先退回看板。当前会话是唯一
/// 在跑的（或者 `current` 压根不在 `sessions` 里）→ `None`，调用方原地不动。
pub fn next_running(sessions: &[SessionInfo], current: u32) -> Option<u32> {
    let cur = sessions.iter().position(|s| s.id == current)?;
    let n = sessions.len();
    (1..n)
        .map(|off| &sessions[(cur + off) % n])
        .find(|s| !matches!(s.state, SessionState::Stopped))
        .map(|s| s.id)
}

/// 按显示宽度裁一行。宽字符（CJK 占两列）跨过边界就整个丢掉——
/// 裁一半会把后面所有列推歪。宽度用 widgets 里的 `char_width`，跟
/// `truncate`/`pad_to` 是同一份定义：裁的地方和补空格的地方对「宽」的
/// 理解一旦分叉，列就对不上了。这里喂进来的是 agent 屏幕的任意内容，
/// 制表符、箭头、省略号都是常客，宽度必须按 Unicode 的正式宽度算。
pub fn crop_line(spans: &[ScreenSpan], max_cols: usize) -> Vec<ScreenSpan> {
    let mut out: Vec<ScreenSpan> = Vec::new();
    let mut used = 0usize;
    for sp in spans {
        if used >= max_cols {
            break;
        }
        let mut text = String::new();
        for ch in sp.text.chars() {
            let w = char_width(ch);
            if used + w > max_cols {
                break;
            }
            used += w;
            text.push(ch);
        }
        if !text.is_empty() {
            out.push(ScreenSpan {
                text,
                style: sp.style,
            });
        }
    }
    out
}

/// 小于这个尺寸就不画格子。九格里每格还要各扣掉两行边框，再小下去
/// 屏幕上只剩框线，用户看不出那是九宫格，也看不出出了什么事。
const MIN_COLS: u16 = 60;
const MIN_ROWS: u16 = 20;

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
///
/// 这里**没有**一条把按键逐个转发给 agent 的路径，这是设计约束：格子只读，
/// 想完整交互按 Enter 放大（见 `View::Grid` 的注释）。回复框是那条约束之外
/// 唯一的口子，它整句整句地发，不是按键转发。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let (focus, draft) = match &app.view {
        View::Grid { focus, reply } => (*focus, reply.clone()),
        _ => return Ok(()),
    };
    // 框开着的时候键盘**整个**归框。挡在这一层而不是在下面的 match 里逐个
    // 排除：漏掉任何一个动作键，用户打字打到那个字母就把会话停了，而
    // `s 停止` 撤不回来。这里一道关，比十一个 `if reply.is_none()` 可靠。
    if let Some(draft) = draft {
        return type_into_reply(app, focus, draft, key);
    }
    // 一次算好、整个函数用同一份：`grid_sessions()` 每次调用都重新遍历
    // 分组，中途再算一次就有了两份可能不同的真相。
    let visible = app.grid_sessions();
    let total = visible.len();
    match key.code {
        KeyCode::Up => move_grid_focus(app, focus, total, Dir::Up),
        KeyCode::Down => move_grid_focus(app, focus, total, Dir::Down),
        KeyCode::Left => move_grid_focus(app, focus, total, Dir::Left),
        // F3 = 「下一个」，跟会话视图里的 F3 是同一个动作，肌肉记忆只练一次
        KeyCode::Right | KeyCode::F(3) => move_grid_focus(app, focus, total, Dir::Right),
        // 换项目跟看板同一批键、同一个语义（共用 `jump_project`/`goto_project`，
        // 不各抄一份）。九宫格没有组头可停，所以「换项目」= 焦点跳到那个项目的
        // 第一个格子，同时看板那边的光标也跟着走——两个模式共用一个光标。
        //
        // 数字键在列表上有可见的号码（组头前面印着 `1`…`9`），九宫格里没有
        // 地方放它；键仍然绑着，是为了跟列表一致，见任务报告里记的这处取舍。
        KeyCode::Tab => {
            super::jump_project(app, 1);
            focus_first_of_current_group(app);
        }
        KeyCode::BackTab => {
            super::jump_project(app, -1);
            focus_first_of_current_group(app);
        }
        KeyCode::Char(c @ '1'..='9') if is_plain_key(&key) => {
            super::goto_project(app, c as usize - '1' as usize);
            focus_first_of_current_group(app);
        }
        // `Tab` 走得到一个空项目，就得走得掉它：`x` 只拿得掉「pinned 且没有
        // 会话」的组（守卫在 `unpin_current` 里），那种组在九宫格里没有格子，
        // 少了这个键用户只能先 `g` 回列表才拿得掉。
        //
        // 拿掉之后**必须**重新对齐光标：光标此刻正停在被删掉的那个组头上，
        // `refresh_rows` 的锚点跟着一起没了，只能退回第 0 行——也就是一个跟
        // 用户毫无关系的项目。而九宫格里看得见的指针是 `▶`，它还在原来那一格
        // 上。不对齐的话，`x` 之后底栏写着 A、`▶` 在 C，`n` 会开进 A。
        //
        // 这里也走无条件的那一支：光标原来站的组刚被删掉，它现在的位置是个
        // 兜底值，没有资格再跟焦点争「当前项目」是谁。
        KeyCode::Char('x') if is_plain_key(&key) => {
            super::unpin_current(app);
            point_cursor_at_focus(app);
        }
        // `i` 开回复框。收件人在这一刻钉死成会话 id，之后焦点再怎么动都
        // 不改——见 `Draft::id`。
        KeyCode::Char('i') if is_plain_key(&key) => {
            app.view = match visible.get(focus).map(|s| s.id) {
                Some(id) => View::Grid {
                    focus,
                    reply: Some(Draft {
                        id,
                        text: String::new(),
                    }),
                },
                // 焦点下面没有会话，只可能是九宫格一个格子都没有——那时候
                // 屏幕正中已经写着「还没有会话，按 n 新建」了。底栏再说一遍
                // 同一件事（还是另一种措辞）只会让人以为是两回事；这里说的
                // 是**另一个**事实：这一下按键没有作用对象。跟看板上同名的
                // 情形用同一条词条（`board::handle_key`）。
                None => {
                    app.message = text(Key::NoSessionSelected, app.lang).into();
                    return Ok(());
                }
            }
        }
        KeyCode::Char('g') if is_plain_key(&key) => super::toggle_view_mode(app),
        // 底栏只有一行，装不下的键都在这扇门后面（底栏尾巴上那条 `? …`）。
        // 回复框开着时走不到这里——上面那道关把键盘整个交给了框。
        KeyCode::Char('?') if is_plain_key(&key) => super::keys::open(app),
        // 九宫格是看板的另一种画法，不是另一个世界：开会话、换项目、
        // 管密钥、退出这几个键跟列表里一模一样（共用同一份实现，见
        // mod.rs 里这几个函数的注释）。用户不该因为切了个视图就得先退
        // 回去才能新建会话。
        KeyCode::Char('q') if is_plain_key(&key) => app.quit = true,
        KeyCode::Char('n') | KeyCode::Char('N') if is_plain_key(&key) => {
            super::open_new_session(app, key.code)
        }
        KeyCode::Char('p') if is_plain_key(&key) => super::open_project_picker(app),
        KeyCode::Char('c') if is_plain_key(&key) => super::open_secrets(app),
        // `l` = language。设置页跟 `c 密钥` 挨着：两个都是「配置」类入口，
        // 而且跟 a/g 一样，两个视图共用同一个键。
        KeyCode::Char('l') if is_plain_key(&key) => super::open_settings(app),
        KeyCode::Enter => {
            if let Some(id) = visible.get(focus).map(|s| s.id) {
                // 放大也是一条离开九宫格的路：从会话里再退出来就到了列表，
                // 那时候光标同样得落在这个会话上（见 sync_board_cursor_from_grid）
                super::sync_board_cursor_from_grid(app);
                super::enter_session(app, id);
            }
        }
        // 跟看板同一套动作，作用在焦点格上——共用 `session_action`，
        // 不各抄一份（抄了将来只会改一半）。
        KeyCode::Char('s') | KeyCode::Char('u') | KeyCode::Char('d') if is_plain_key(&key) => {
            app.message = match visible.get(focus).map(|s| s.id) {
                Some(id) => session_action(app, key.code, id),
                // 同上：屏幕正中已经在说「这里什么都没有」，底栏说的是
                // 「这一下没有作用对象」——两个不同的事实，不是同一句话说两遍。
                None => text(Key::NoSessionSelected, app.lang).into(),
            };
        }
        _ => {}
    }
    Ok(())
}

/// 焦点挪一格，**并且把列表光标一起挪过去**。
///
/// 九宫格现在铺的是所有项目的会话（分组之前它只画一个作用域），所以左右
/// 一走就可能跨进另一个项目。而「当前项目」唯一的答案处是列表光标
/// （`App::current_group`）——不一起挪的话，底栏的项目名、`n` 把新会话开在
/// 哪个目录、`x` 拿掉哪个组，全都还指着上一个项目，而屏幕上焦点分明已经在
/// 别人家的格子里了。这正是这一版要消灭的「屏幕和状态各说各的」。
///
/// **走 `point_cursor_at_focus` 而不是 `sync_board_cursor_from_grid`。**
/// 后者带着一条「焦点可能是陈旧的、别拿它改写光标」的守卫，那条守卫是为
/// 「用户没有碰过焦点」的出口准备的。方向键恰恰相反：按下它就是用户在说
/// 「我现在指的是这一格」，没有任何情况该让这个动作只挪一半。让它也过一遍
/// 那条守卫的话，同一个方向键会因为一个屏幕上看不见的状态而有两种含义，
/// 而屏幕上没有任何东西能告诉用户现在是哪一种。
fn move_grid_focus(app: &mut App, focus: usize, total: usize, dir: Dir) {
    app.view = View::grid(move_focus(focus, total, dir));
    point_cursor_at_focus(app);
}

/// 把列表光标指到**此刻焦点那一格**的会话上，无条件。
///
/// 跟 `sync_board_cursor_from_grid` 的差别只有那条「焦点是不是陈旧的」守卫。
/// 这里的两个调用点都属于「焦点就是此刻唯一有意义的指针」，守卫在这儿只会
/// 添乱：方向键是用户显式的指点动作；`x` 则是把光标原来站的那个组整个删掉了，
/// 光标剩下的落点是 `refresh_rows` 的兜底第 0 行，跟用户毫无关系。
///
/// 焦点一时对不上任何会话（刚被停掉/清掉，还没收拢）就什么都不做——
/// 乱指一个比不动更糟，同 `point_cursor_at_session`。
fn point_cursor_at_focus(app: &mut App) {
    let View::Grid { focus, .. } = app.view else {
        return;
    };
    if let Some(id) = app.grid_sessions().get(focus).map(|s| s.id) {
        super::point_cursor_at_session(app, id);
    }
}

/// 把焦点挪到当前组的第一个活会话上。
///
/// 找不到（这个组的会话全停了、或者压根是个空组）就**不动**：空组在九宫格里
/// 一个格子都没有，硬挪只会把焦点指到别人家的格子上去。
///
/// 焦点不动不等于这一下按键没反应——`jump_project`/`goto_project` 已经把
/// 列表光标挪到了新项目的组头上，底栏那一段项目名读的就是它，用户看得见
/// 自己换到了哪儿，接着按 `n` 开在那儿、按 `x` 把它拿掉。
fn focus_first_of_current_group(app: &mut App) {
    // `first_live` 是「这个项目在九宫格里有没有格子」唯一的判断处——
    // `sync_board_cursor_from_grid` 问的是同一个问题，两边必须逐字一致，
    // 理由见 `ProjectGroup::first_live` 自己的文档。
    let Some(first) = app.current_group().and_then(|g| g.first_live()) else {
        return;
    };
    if let Some(i) = app.grid_sessions().iter().position(|s| s.id == first) {
        app.view = View::grid(i);
    }
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let (focus, draft) = match &app.view {
        View::Grid { focus, reply } => (*focus, reply.clone()),
        _ => return,
    };
    let visible = app.grid_sessions();
    draw_grid(
        f,
        area,
        &visible,
        &app.grid_screens,
        focus,
        Chrome {
            connected: app.connected,
            lang: app.lang,
        },
        !app.sessions.is_empty(),
    );
    if let Some(draft) = draft {
        let who = visible
            .iter()
            .find(|s| s.id == draft.id)
            .map(|s| format!("{} {}", s.id, s.profile))
            // 收件人在打字途中被停掉了。仍然照实写出 id——用户得看得见
            // 自己正在对谁说话，哪怕那个会话刚没了。
            .unwrap_or_else(|| draft.id.to_string());
        draw_reply(f, area, &draft.text, &who, app.lang);
    }
}

/// 画九宫格。格子的顺序 = 当页会话的顺序；画面按 id 跟 `screens` 配对，
/// 一时没配上的格子只画标题和空白——下一轮 300ms 就有了，比画错内容强。
///
/// 跟 `App` 解耦（只吃它真正用得上的那几样）是为了能在测试里直接喂 fixture，
/// 不必为了断言一句「窗口太小」去拼一个完整的 `App`。
/// 画格子时那几样「跟会话数据无关、只影响怎么呈现」的东西。打包成一个结构体
/// 而不是继续往参数表上加：它们总是一起传、一起来自 `App`，而八个位置参数
/// 里传错顺序（两个 bool、两个枚举）编译器是拦不住的。
#[derive(Clone, Copy)]
pub(crate) struct Chrome {
    pub connected: bool,
    pub lang: Lang,
}

fn draw_grid(
    f: &mut Frame,
    area: Rect,
    sessions: &[SessionInfo],
    screens: &[ScreenEntry],
    focus: usize,
    chrome: Chrome,
    // 作用域里有会话，只是全停了——空状态那句话要说得不一样
    has_stopped: bool,
) {
    let Chrome { connected, lang } = chrome;
    if area.width < MIN_COLS || area.height < MIN_ROWS {
        // 说人话说清下一步做什么：这是用户自己能修好的事
        f.render_widget(
            Paragraph::new(text(Key::WindowTooSmall, lang)).centered(),
            centered_line(area),
        );
        return;
    }
    if sessions.is_empty() {
        // 「一个会话都没有」和「有会话但都停了」是两回事。后者说成前者
        // 会让用户以为自己的会话丢了——它们其实好端端在列表里，还能
        // 回滚、看改动。这一句要说清它们在哪。
        if has_stopped {
            f.render_widget(
                Paragraph::new(text(Key::AllSessionsStopped, lang)).centered(),
                centered_line(area),
            );
            return;
        }
        // `n` 在九宫格里跟在列表里是同一个键，直接说怎么开，别让用户
        // 先绕回列表。分组之后九宫格画的是**所有**项目的会话，所以
        // 「一个都没有」就是字面意思，不再需要「别的项目里可能还有」那半句。
        // 「一个项目组都没有」也不可能走到这儿——开机时 `seed_start_project`
        // 会把启动目录摆上去。
        f.render_widget(
            Paragraph::new(text(Key::NoSessionsRunningPressN, lang)).centered(),
            centered_line(area),
        );
        return;
    }

    let total = sessions.len();
    let page = page_of(focus);
    let start = (page * TILES_PER_PAGE).min(total);
    let page_sessions = &sessions[start..(start + TILES_PER_PAGE).min(total)];
    let (rows, cols) = grid_shape(page_sessions.len());

    // 多页时先从底部切一行出来放页码。不切、直接把页码画在 area 右下角的话，
    // 它会盖在最底下那一排格子的边框上，看起来像边框破了个洞。
    let pages = page_count(total);
    let (tiles_area, footer) = if pages > 1 {
        let parts = Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
            .split(area)
            .to_vec();
        (parts[0], Some(parts[1]))
    } else {
        (area, None)
    };

    let row_areas = Layout::vertical(vec![Constraint::Ratio(1, rows as u32); rows as usize])
        .split(tiles_area)
        .to_vec();
    let tile_areas: Vec<Rect> = row_areas
        .iter()
        .flat_map(|r| {
            Layout::horizontal(vec![Constraint::Ratio(1, cols as u32); cols as usize])
                .split(*r)
                .to_vec()
        })
        .collect();

    for (i, info) in page_sessions.iter().enumerate() {
        let tile = tile_areas[i];
        let focused = start + i == focus;
        // 「出问题了」比「你选中了它」优先：断连是整屏过期，失败是这一格
        // 出事，两种都用红。焦点在红态下不换颜色，换的是边框字符和色块——
        // 颜色这一个维度已经被占用了，再抢就两件事都说不清。
        let alarmed = !connected || matches!(info.state, SessionState::Failed);
        let focus_color = if alarmed { Color::Red } else { Color::Cyan };

        // 标题就是状态指示器：状态词用 status_style 上色，跟列表同一套颜色
        // （已停止是灰的），扫一眼九个格子就知道谁在干活、谁停了。
        let mut title = vec![
            // 跟列表的 `highlight_symbol` 同一个符号：两个模式看起来才是
            // 同一件事。
            Span::raw(if focused { "▶" } else { " " }),
            Span::raw(format!("{} {} ", info.id, info.profile)),
            Span::styled(
                format!("{} ", status_label(info.state, lang)),
                status_style(info.state),
            ),
        ];
        // 九个格子长得都一样，不点名项目就分不出谁是谁。九宫格现在画的是
        // 所有项目的会话，所以这一条**无条件**加——不加就没有任何地方能
        // 告诉用户这一格属于谁。
        let project = std::path::Path::new(&info.dir)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| info.dir.clone());
        title.push(Span::styled(format!("{project} "), dim()));
        // 焦点格的标题反色成一条**铺满整格宽度**的实心色块。
        //
        // 只把标题文字反色不够：文字才占十来列，格子宽三四十列，剩下的还是
        // 一条细边框，扫视时那一小块跟别的格子的标题混在一起。补齐到整行之后
        // 焦点格顶上是一条完整的横条，隔着屏幕都认得出来，用户不需要先知道
        // 「有选中这回事」才找得到它。
        //
        // 补空格而不是靠别的办法：Block 的 title 只画文字那几列，宽度得自己
        // 撑开。`pad_to` 按显示宽度补，CJK 占两列这件事它已经处理了——用
        // `len()` 的话中文标题会补出双倍长度，把右上角的框线顶掉。
        //
        // 反色时状态词交出自己的颜色：底色已经占满整条，青底上的红字比黑字
        // 更难读，而状态本来就写着字（「干活中」/「出错」），不靠颜色也读得出。
        if focused {
            let block = Style::default().bg(focus_color).fg(Color::Black);
            let text: String = title.iter().map(|s| s.content.as_ref()).collect();
            // 左右两列是边框，标题能占的就是中间这些列
            let width = tile.width.saturating_sub(2) as usize;
            title = vec![Span::styled(pad_to(&text, width), block)];
        }
        let title = Line::from(title);
        // 边框的**颜色**说状态，边框的**字符**说焦点。两件事各占一个维度，
        // 才能同时说清「这一格出事了」和「你现在站在这一格」。
        //
        // 颜色：断连时整屏格子一律红框——九个静止的画面看上去跟活的一模一样，
        // 不给个视觉提示，用户会以为 agent 都不动了（列表和会话视图断连时
        // 也是转红框，三处一致）。单格失败也红，跟那一屏红区分得开：那是
        // 整屏都红，这是九个里的某一个红。其余用 DIM 而不是 DarkGray——后者
        // 是 ANSI 亮黑，有些主题把它设成背景同色，整圈边框会隐形（见 mod.rs
        // 里 DIM 的注释）。
        //
        // 字符：焦点格用 Thick（`┏━┓`）。这里原来用的是 `Modifier::BOLD`，
        // 理由写的是「笔画粗细不受主题影响」——方向没错，手段是坏的：BOLD
        // 加在框线字符上，绝大多数终端字体没有对应的加粗字形，修饰被直接
        // 忽略。于是焦点实际只剩「青 vs 暗」一个颜色维度，浅色主题下几乎
        // 看不出来，红态下更是完全没有标记。Thick 换的是字符本身，终端支
        // 不支持 BOLD、用户配的什么配色，都不影响。
        let border = if alarmed {
            Style::default().fg(Color::Red)
        } else if focused {
            Style::default().fg(Color::Cyan)
        } else {
            dim()
        };
        let block = Block::bordered()
            .border_type(if focused {
                BorderType::Thick
            } else {
                BorderType::Plain
            })
            .title(title)
            .border_style(border);
        let inner = block.inner(tile);
        f.render_widget(block, tile);

        if let Some(entry) = screens.iter().find(|e| e.id == info.id) {
            // 取底部 N 行：agent 的输入框和最新输出都在屏幕底部
            let skip = entry.lines.len().saturating_sub(inner.height as usize);
            let cropped: Vec<Vec<ScreenSpan>> = entry.lines[skip..]
                .iter()
                .map(|l| crop_line(l, inner.width as usize))
                .collect();
            // 不画光标：只读的格子画光标只会误导用户在这里打字
            //
            // 非焦点格的画面整体压暗。前两个维度（颜色、框线字符）都已经用
            // 满了，对比度是第三个——让另外八格退到背景层，焦点格不用再加
            // 任何装饰就自己跳出来了。
            //
            // 代价说清楚：那八格的 agent 输出确实变灰了，而「一眼扫全部」正是
            // 九宫格存在的理由。但压暗不是隐藏——字还在，标题上的状态词也还是
            // 原色（压暗只作用于画面，不碰标题），而「找不到焦点在哪一格」比
            // 「另外八格淡一点」更痛。
            let lines = screen_to_lines(&cropped);
            f.render_widget(
                Paragraph::new(if focused { lines } else { recede(lines) }),
                inner,
            );
        }
    }

    // 页码贴在自己那一行的右端，只有多页才画——单页画一个 1/1 纯属噪音
    if let Some(footer) = footer {
        f.render_widget(
            Paragraph::new(format!("{}/{} ", page + 1, pages)).right_aligned(),
            footer,
        );
    }
}

/// 回复框收到一个键。判断全在 `reply_key`（纯函数，好测），这里只做 I/O
/// 和视图切换。
fn type_into_reply(app: &mut App, focus: usize, draft: Draft, key: KeyEvent) -> Result<()> {
    match reply_key(&draft.text, &key) {
        Reply::Typing(text) => {
            app.view = View::Grid {
                focus,
                reply: Some(Draft { id: draft.id, text }),
            }
        }
        Reply::Cancel => app.view = View::grid(focus),
        Reply::Send(body) => {
            app.message = send_reply(app, draft.id, &body);
            app.view = View::grid(focus);
        }
        // `\x03` = Ctrl+C，跟 `key_to_input` 给附加视图算出来的是同一个字节
        Reply::Interrupt => {
            app.message = match send_input(app, draft.id, "\u{3}") {
                Ok(()) => text(Key::ActionDone, app.lang).into(),
                Err(m) => m,
            };
            app.view = View::grid(focus);
        }
    }
    Ok(())
}

/// 把一句话交给 agent：先送文字，**再单独送一个空 `Input`**。
///
/// 空 `Input` 在守护进程侧就是「按回车」，而且回车那一步还会打检查点
/// （见 `session.rs::send_input`）。所以这两步不能反、也不能合并成一次：
/// 合并了就没有「发这句话之前」那个还原点，用户按 `u` 回滚不回来——而
/// 「回滚」正是这个工具敢让非程序员放手用 agent 的底气。
///
/// 文字没送出去就不按回车：半句话加一个回车，等于把残缺的指令交给了 agent。
fn send_reply(app: &mut App, id: u32, body: &str) -> Msg {
    let sent = if body.is_empty() {
        send_input(app, id, "")
    } else {
        send_input(app, id, body).and_then(|()| send_input(app, id, ""))
    };
    match sent {
        Ok(()) => text(Key::ActionDone, app.lang).into(),
        Err(m) => m,
    }
}

/// 一次 `Request::Input`。失败原样交给调用方——发不出去必须说出来，
/// 悄悄吞掉的话用户以为自己回过话了，其实 agent 还在那儿等着。
fn send_input(app: &mut App, id: u32, body: &str) -> std::result::Result<(), Msg> {
    let req = crate::proto::Request::Input {
        id,
        text: body.to_string(),
    };
    match app.client().and_then(|c| c.call(req)) {
        Ok(crate::proto::Response::Ok) => Ok(()),
        Ok(crate::proto::Response::Error(ref e)) => {
            Err(Msg::err(crate::i18n::msg::error(app.lang, e)))
        }
        _ => Err(Msg::err(text(Key::RequestFailed, app.lang).into())),
    }
}

/// 回复框：**盖**在九宫格最下面那一行上。
///
/// 是盖上去的，不是从上面切一行下来。切的话内容区就少一行，而 80×24 下
/// 九宫格的内容区正好等于 `MIN_ROWS`——少一行整个视图就换成一句「窗口太小」，
/// 框一开格子全没了。盖在最后一行只压掉最下面那排格子的下边框，代价最小。
fn draw_reply(f: &mut Frame, area: Rect, draft: &str, who: &str, lang: Lang) {
    let row = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(1),
        width: area.width,
        height: 1,
    };
    f.render_widget(ratatui::widgets::Clear, row);

    // 收件人写在最前面，而且是「会话号 + agent 名字」。发错人撤不回来，
    // 所以这不是装饰——用户打字时眼睛就在这一行上，收件人必须在他视线里。
    let to = Span::styled(
        format!("→ {who}："),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    // 空框时给一句话，把「直接回车 = 同意」说出来。这是最高频的用法，
    // 不写的话用户会以为必须先打点什么才能回。
    let body = if draft.is_empty() {
        vec![
            Span::styled("▌", Style::default().fg(Color::Cyan)),
            Span::styled(format!("  {}", text(Key::EmptyReplyIsEnter, lang)), dim()),
        ]
    } else {
        vec![
            Span::raw(draft.to_string()),
            Span::styled("▌", Style::default().fg(Color::Cyan)),
        ]
    };
    let mut spans = vec![to];
    spans.extend(body);
    f.render_widget(Paragraph::new(Line::from(spans)), row);
}

/// 把一屏文字推到背景层：给每个 span 叠上 `DIM`。
///
/// 用 DIM 而不是把前景色统一改成灰：agent 的输出自带颜色（diff 的红绿、
/// 语法高亮、报错的红），统一改色等于把这些信息一起抹掉；DIM 是在原色上
/// 降亮度，颜色关系还在。
///
/// DIM 加在**文字**上跟 BOLD 加在框线字符上是两回事——后者绝大多数字体
/// 没有对应字形所以被忽略（见 `draw_grid` 里边框那段），普通文字的 DIM
/// 终端普遍支持。
fn recede(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            Line::from(
                line.spans
                    .into_iter()
                    .map(|s| {
                        let style = s.style.add_modifier(Modifier::DIM);
                        Span::styled(s.content, style)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// 整块区域里垂直居中的那一行。一句话的提示贴在最上面像是画残了，
/// 居中才像是「这一屏就是想告诉你这句话」。
fn centered_line(area: Rect) -> Rect {
    Rect {
        x: area.x,
        y: area.y + area.height / 2,
        width: area.width,
        height: 1.min(area.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::{ScreenSpan, ScreenStyle};
    use crate::session::SessionState;

    #[test]
    fn shape_scales_with_session_count() {
        assert_eq!(grid_shape(1), (1, 1));
        assert_eq!(grid_shape(2), (1, 2));
        assert_eq!(grid_shape(3), (2, 2));
        assert_eq!(grid_shape(4), (2, 2));
        assert_eq!(grid_shape(5), (2, 3));
        assert_eq!(grid_shape(6), (2, 3));
        assert_eq!(grid_shape(7), (3, 3));
        assert_eq!(grid_shape(9), (3, 3));
        // 超过 9 的调用方先按页切好再问形状，这里按满页算
        assert_eq!(grid_shape(0), (1, 1), "空看板画一个空格子占位");
    }

    #[test]
    fn paging_math() {
        assert_eq!(page_of(0), 0);
        assert_eq!(page_of(8), 0);
        assert_eq!(page_of(9), 1);
        assert_eq!(page_count(0), 1);
        assert_eq!(page_count(9), 1);
        assert_eq!(page_count(10), 2);
    }

    #[test]
    fn focus_moves_in_two_dimensions_and_wraps_pages() {
        // 5 个会话 → 2×3 布局，index 0..=4
        assert_eq!(move_focus(0, 5, Dir::Right), 1);
        assert_eq!(
            move_focus(2, 5, Dir::Down),
            4,
            "2 的正下方越出最后一行，收到最后一格"
        );
        assert_eq!(move_focus(0, 5, Dir::Down), 3);
        assert_eq!(move_focus(4, 5, Dir::Right), 0, "尾格右移回绕到头");
        assert_eq!(move_focus(0, 5, Dir::Left), 4, "头格左移回绕到尾");
        // 10 个会话：第 8 格（第一页尾）右移进第二页
        assert_eq!(move_focus(8, 10, Dir::Right), 9);
        assert_eq!(move_focus(9, 10, Dir::Right), 0);
    }

    fn sp(text: &str) -> ScreenSpan {
        ScreenSpan {
            text: text.into(),
            style: ScreenStyle::default(),
        }
    }

    #[test]
    fn next_running_wraps_and_skips_stopped() {
        let sessions = vec![
            session(1, SessionState::Working),
            session(2, SessionState::Stopped),
            session(3, SessionState::Idle),
        ];
        assert_eq!(next_running(&sessions, 1), Some(3), "2 停了要跳过");
        assert_eq!(next_running(&sessions, 3), Some(1), "到尾回绕");
        let only = vec![session(1, SessionState::Working)];
        assert_eq!(
            next_running(&only, 1),
            None,
            "没有别的会话就别跳，跳回自己是噪音"
        );
    }

    #[test]
    fn focus_moves_up_within_the_page() {
        // 5 个会话 → 2×3：3 在第二行第一列，上移回到 0
        assert_eq!(move_focus(3, 5, Dir::Up), 0);
        assert_eq!(move_focus(4, 5, Dir::Up), 1);
        // 第一行往上收到本页第一格、不回绕（回绕的是左右，见 move_focus
        // 的注释）——跟 Down 越出末行收到最后一格是对称的。
        assert_eq!(move_focus(1, 5, Dir::Up), 0);
        assert_eq!(move_focus(0, 5, Dir::Up), 0);
        // 第二页的格子上移仍留在第二页：页内坐标是 in_page，不是全局下标
        assert_eq!(move_focus(12, 14, Dir::Up), 9);
    }

    #[test]
    fn crop_cuts_at_display_width_without_splitting_wide_chars() {
        // "干活中" 每个字占 2 列。上限 5 列 → 只装得下 2 个字（4 列），
        // 第 3 个字会跨过边界，整个丢掉。
        let out = crop_line(&[sp("干活中")], 5);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "干活");

        // 跨 span 累计：第一个 span 占 3 列，剩 2 列只够 "b" 一个
        let out = crop_line(&[sp("abc"), sp("bcd")], 5);
        assert_eq!(out[1].text, "bc");

        // 不超限的原样保留
        let out = crop_line(&[sp("ok")], 80);
        assert_eq!(out[0].text, "ok");

        // 正好装满：一列不多一列不少，不该被误裁掉最后一个字
        let out = crop_line(&[sp("干活")], 4);
        assert_eq!(out[0].text, "干活");

        // 零宽的格子（边框吃光了内部宽度）什么都画不下，也不能 panic
        assert!(crop_line(&[sp("abc")], 0).is_empty());
    }

    #[test]
    fn box_drawing_lines_crop_at_their_real_width() {
        // 回归测试：Claude Code 这类 agent 的输入框是一整行制表符画出来的。
        // 早年的宽度表把 U+1100 以上的字符一律当双宽，38 列的横线被算成 76 列，
        // 只画得出 19 个——屏幕上那条框线短了一半，`│` 开头的行还会少一列内容。
        let rule: String = "─".repeat(38);
        let out = crop_line(&[sp(&rule)], 38);
        assert_eq!(
            out[0].text.chars().count(),
            38,
            "38 列的格子要装得下 38 个制表符"
        );

        // `│` 前缀的一行：边框加内容合起来正好占满，一个字符都不该丢
        let out = crop_line(&[sp("│"), sp("hello")], 6);
        assert_eq!(out[0].text, "│");
        assert_eq!(out[1].text, "hello");
    }

    // ———— 视图：按键 ————

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn session(id: u32, state: SessionState) -> SessionInfo {
        SessionInfo {
            id,
            profile: "claude".into(),
            dir: "/tmp/a".into(),
            state,
            activity: String::new(),
            is_agent: true,
        }
    }

    fn session_in(id: u32, dir: &str) -> SessionInfo {
        SessionInfo {
            id,
            profile: "claude".into(),
            dir: dir.into(),
            state: SessionState::Idle,
            activity: String::new(),
            is_agent: true,
        }
    }

    fn grid_text(app: &mut App) -> String {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        term.draw(|f| draw(f, f.area(), app)).unwrap();
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

    /// **焦点必须一眼看得出来。** 只靠边框换色不够：一条 1 格宽的线在浅色
    /// 主题上跟灰线几乎一样，而且用户得先知道「有选中这回事」才会去找它。
    /// 用跟列表同一个 `▶` 符号，两个模式看起来才是一件事。
    #[test]
    fn the_focused_tile_is_marked_with_the_same_arrow_the_list_uses() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=3).map(|i| session(i, SessionState::Idle)).collect());
        app.view = View::grid(1);

        let c = grid_text(&mut app);
        assert_eq!(c.matches('▶').count(), 1, "有且只有一个格子带标记：{c}");
        // 标记要在焦点那一格的标题上：格子 2 是 `▶2claude`
        assert!(c.contains("▶2claude"), "标记要落在焦点格上：{c}");
    }

    /// 「一个会话都没有」和「有会话但都停了」是两回事。后者说成前者会让
    /// 用户以为自己的会话丢了——它们其实好端端在列表里，还能回滚、看改动。
    #[test]
    fn a_grid_of_only_stopped_sessions_says_where_they_went() {
        let (mut app, _dir) = App::test_app();
        let mut s1 = session(1, SessionState::Stopped);
        s1.dir = "/tmp/a".into();
        app.set_sessions(vec![s1]);
        app.view = View::grid(0);

        let c = grid_text(&mut app);
        assert!(c.contains("都停了"), "要说清是「停了」不是「没有」：{c}");
        assert!(c.contains("g"), "要指路：按 g 回列表能看到它们：{c}");
    }

    /// 出错的格子边框转红。九个格子长得都一样，光靠标题里那两个字太容易
    /// 漏——用户扫一眼就该看见是哪一格出事了。
    #[test]
    fn a_failed_tile_gets_a_red_border() {
        let (mut app, _dir) = App::test_app();
        let mut s = session(1, SessionState::Failed);
        s.dir = "/tmp/a".into();
        app.set_sessions(vec![s]);
        app.view = View::grid(0);

        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let buf = term.backend().buffer();
        let a = buf.area;
        let red = (0..a.height).any(|y| {
            (0..a.width).any(|x| {
                buf.cell((x, y))
                    .map(|c| c.style().fg == Some(Color::Red) && c.symbol() != " ")
                    .unwrap_or(false)
            })
        });
        assert!(red, "出错的格子必须有红色，扫一眼就看得见");
    }

    /// 九宫格跟列表看的是同一批会话——列表现在按项目分组列出全部，
    /// 格子也就该把全部都画出来。每一格标题**无条件**带项目名：九个格子
    /// 长得都一样，不点名就分不出谁是谁。
    #[test]
    fn every_tile_names_its_own_project() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![
            session_in(1, "/w/dc-terminal"),
            session_in(2, "/w/dc_desktop"),
        ]);
        app.view = View::grid(0);

        let c = grid_text(&mut app);
        assert!(c.contains("1claude"), "两个项目的格子都要在：{c}");
        assert!(c.contains("2claude"), "别的项目不再被藏起来：{c}");
        assert!(c.contains("dc-terminal"), "格子标题要带项目名：{c}");
        assert!(c.contains("dc_desktop"), "格子标题要带项目名：{c}");
    }

    /// **同一个项目的格子必须连排。** 格子上没有组头（二维布局里没地方放），
    /// 「谁跟谁是一伙的」全靠挨着——一旦按 id 全局排序，两个项目的格子就会
    /// 交错着铺满九宫格，用户只能一格一格读项目名。顺序由 `grid_sessions()`
    /// 给（组序 + 组内 id 序），这条盯着它别被改回去。
    ///
    /// 连排还顺带管住了翻页：一个项目只有在它自己跨过第 9 格时才会被页边
    /// 切开，而不是因为别的项目插了队。
    #[test]
    fn tiles_are_ordered_by_project_then_id() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![
            session_in(9, "/w/b"),
            session_in(2, "/w/a"),
            session_in(5, "/w/a"),
        ]);
        assert_eq!(
            app.grid_sessions().iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![2, 5, 9],
            "同一项目的格子必须连排，不能按 id 全局排序把它们打散"
        );
    }

    /// `Tab` 在九宫格里跟在列表里是同一件事：换到下一个项目。焦点落到那个
    /// 项目的第一个格子上，列表光标（「当前项目」唯一的答案处）也一起走。
    #[test]
    fn tab_moves_the_focus_to_the_next_project() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![session_in(1, "/w/a"), session_in(2, "/w/b")]);
        app.view = View::grid(0);

        handle_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("b".to_string())
        );
        assert!(
            matches!(app.view, View::Grid { focus: 1, .. }),
            "焦点要落到 b 的第一个格子上"
        );

        handle_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("a".to_string()),
            "到头回绕，跟列表一样"
        );
        assert!(matches!(app.view, View::Grid { focus: 0, .. }));
    }

    #[test]
    fn shift_tab_goes_back_to_the_previous_project() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![session_in(1, "/w/a"), session_in(2, "/w/b")]);
        app.view = View::grid(0);

        handle_key(&mut app, key(KeyCode::BackTab)).unwrap();
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("b".to_string()),
            "往回一格从头绕到尾"
        );
        assert!(matches!(app.view, View::Grid { focus: 1, .. }));
    }

    /// 数字键直达第 N 个项目，跟列表上一模一样（列表的组头上印着这个号码）。
    /// 越界什么都不做——按了 `9` 而只有两个项目时，不动比跳到最后一个好懂。
    #[test]
    fn a_digit_jumps_straight_to_that_project() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![
            session_in(1, "/w/a"),
            session_in(2, "/w/b"),
            session_in(3, "/w/c"),
        ]);
        app.view = View::grid(0);

        handle_key(&mut app, key(KeyCode::Char('3'))).unwrap();
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("c".to_string())
        );
        assert!(matches!(app.view, View::Grid { focus: 2, .. }));

        handle_key(&mut app, key(KeyCode::Char('9'))).unwrap();
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("c".to_string()),
            "越界不动"
        );
        assert!(matches!(app.view, View::Grid { focus: 2, .. }));
    }

    /// `Tab` 走到一个**空**项目（`p` 摆上来但还没开会话）：九宫格里它一个
    /// 格子都没有，焦点无处可去，只能留在原地——硬挪就会指到别人家的格子上。
    ///
    /// 但这一下**不是没反应**：底栏的项目名读的是列表光标，`jump_project`
    /// 已经把它挪过去了，用户看得见自己换到了哪个项目，接着按 `n` 就开在那儿、
    /// 按 `x` 就把它拿掉。
    #[test]
    fn tab_onto_an_empty_project_switches_project_without_moving_the_focus() {
        let (mut app, _dir) = App::test_app();
        app.pinned = vec!["/w/z".into()];
        app.set_sessions(vec![session_in(1, "/w/a")]);
        app.view = View::grid(0);

        handle_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("z".to_string()),
            "项目确实换过去了，按键不能是无声的"
        );
        assert!(
            matches!(app.view, View::Grid { focus: 0, .. }),
            "空项目没有格子，焦点只能留在原地"
        );
        assert_eq!(
            app.current_dir(),
            std::path::PathBuf::from("/w/z"),
            "接着按 n 要开在刚换过去的那个项目里"
        );
    }

    /// **换到一个空项目之后按 `g`，落点必须还是那个空项目。**
    ///
    /// 这是上一条的下半程，而且是真正会咬人的那一半：空项目上焦点是**故意**
    /// 留旧的（它指着上一个项目的格子），而 `g` 会走
    /// `sync_board_cursor_from_grid` 拿焦点去改写光标——不设防的话，用户
    /// `p` 摆上 z、`Tab` 过去、底栏明明写着 z，一按 `g` 就回到了 a，接着
    /// 按 `n` 会把新会话开进 a。`Enter` 放大走的是同一条同步，同一个洞。
    #[test]
    fn tab_onto_an_empty_project_then_g_still_lands_on_that_project() {
        let (mut app, _dir) = App::test_app();
        app.pinned = vec!["/w/z".into()];
        app.set_sessions(vec![session_in(1, "/w/a")]);
        app.view_mode = crate::ui::ViewMode::Grid;
        app.view = View::grid(0);

        handle_key(&mut app, key(KeyCode::Tab)).unwrap();
        handle_key(&mut app, key(KeyCode::Char('g'))).unwrap();

        assert!(matches!(app.view, View::Board));
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("z".to_string()),
            "焦点是故意留旧的，`g` 不能拿它把刚换过去的项目换回来"
        );
        assert_eq!(app.current_dir(), std::path::PathBuf::from("/w/z"));
    }

    /// **会话全停了的项目不是「焦点陈旧」，别把它们混成一件事。**
    ///
    /// 一个所有会话都已停止的项目，在九宫格里同样一个格子都没有。但用户在它
    /// 上面按方向键时，焦点是**他自己挪的**——那是一次显式的指点动作，光标
    /// 必须跟着走。要是拿「这个组没有活格子」当作「焦点是陈旧的」的判据，
    /// 这里就会被一起挡掉：用户从全停项目进九宫格、方向键挪到别的项目的活
    /// 格子上、再回列表，光标却还停在那个**已停止**的会话上——而 `s`/`u`
    /// 都不可撤销，正是 `sync_board_cursor_from_grid` 开篇警告的那种事故。
    #[test]
    fn an_all_stopped_project_does_not_freeze_the_cursor_where_it_stands() {
        let (mut app, _dir) = App::test_app();
        let mut stopped = session_in(9, "/w/zz");
        stopped.state = SessionState::Stopped;
        app.set_sessions(vec![session_in(2, "/w/b"), session_in(3, "/w/b"), stopped]);
        // 行是 [组头 b, 2, 3, 组头 zz, 9]：光标停在 zz 的那个已停止会话上
        app.list_state.select(Some(4));
        assert_eq!(
            app.selected_session().map(|s| s.id),
            Some(9),
            "前提：光标在 zz 的已停止会话上"
        );
        app.view_mode = crate::ui::ViewMode::Grid;
        // 进九宫格：zz 在这儿没有格子，焦点只能回落到第 0 格（b 的会话 2）
        app.view = super::super::home_view(&app);
        assert!(matches!(app.view, View::Grid { focus: 0, .. }));

        // 用户自己把焦点挪到 b 的第二个活格子上——这是显式的指点动作
        handle_key(&mut app, key(KeyCode::Right)).unwrap();
        handle_key(&mut app, key(KeyCode::Char('g'))).unwrap();

        assert!(matches!(app.view, View::Board));
        assert_eq!(
            app.selected_session().map(|s| s.id),
            Some(3),
            "光标必须落在用户指着的那个活会话上，而不是留在已停止的 9 上——\
             接下来的 s/u 都不可撤销"
        );
    }

    /// **空项目一旦拿到会话，陈旧的焦点也不会自己回正。**
    ///
    /// `Tab` 到空项目 z 之后焦点是故意留旧的（指着 a 的格子）。这时候后台
    /// 那一轮 `List` 轮询把 z 的新会话捎了回来（另一个 dct 窗口开的，或者
    /// 刚从 `n` 回来），z 于是有了活会话——但**没有任何东西会把焦点挪进 z**。
    /// 拿「这个组有没有活格子」当判据的话，守卫会在这一刻打开，而焦点仍然是
    /// 旧的，`g` 照样把用户送回 a。
    #[test]
    fn a_stale_focus_stays_stale_after_the_empty_project_gains_a_session() {
        let (mut app, _dir) = App::test_app();
        app.pinned = vec!["/w/z".into()];
        app.set_sessions(vec![session_in(1, "/w/a")]);
        app.view_mode = crate::ui::ViewMode::Grid;
        app.view = View::grid(0);

        handle_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("z".to_string())
        );

        // 后台轮询：z 有会话了。光标（锚在 z 的组头上）不动，焦点也没人挪。
        app.set_sessions(vec![session_in(1, "/w/a"), session_in(5, "/w/z")]);
        assert!(
            matches!(app.view, View::Grid { focus: 0, .. }),
            "没有任何东西会把焦点挪进 z"
        );

        handle_key(&mut app, key(KeyCode::Char('g'))).unwrap();
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("z".to_string()),
            "焦点还是旧的（指着 a 的格子），z 有没有会话都不该让它改写光标"
        );
    }

    /// 同一个洞的另一条出口：`Enter` 放大也调 `sync_board_cursor_from_grid`。
    /// 空项目上按 `Enter` 会放大那个**陈旧焦点**指着的会话（用户看得见 `▶`
    /// 在那儿，这一步不算意外），但它不该顺手把当前项目也改回去——
    /// 从会话里退出来时，用户应当回到自己刚换过去的那个项目。
    #[test]
    fn zooming_from_an_empty_project_does_not_change_the_project_back() {
        let (mut app, _dir) = App::test_app();
        app.pinned = vec!["/w/z".into()];
        app.set_sessions(vec![session_in(1, "/w/a")]);
        app.view = View::grid(0);

        handle_key(&mut app, key(KeyCode::Tab)).unwrap();
        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        assert!(matches!(app.view, View::Attached(1)));
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("z".to_string())
        );
    }

    /// `x` 在九宫格里也能把一个空项目拿掉：`Tab` 走得到它，就得走得掉它，
    /// 否则用户只能先按 `g` 回列表——而那正是「九宫格是看板的另一种画法，
    /// 不是另一个世界」这条约束要消灭的东西。
    ///
    /// 拿掉之后光标必须**跟着 `▶` 走**：被删的那个组头就是光标站的地方，
    /// `refresh_rows` 的锚点跟着一起没了，只能退回第 0 行——那是个跟用户毫无
    /// 关系的项目，而屏幕上看得见的指针 `▶` 还在别处。
    ///
    /// **三个组是必须的**：只有两个组时「退回第 0 行」碰巧就是正确答案，
    /// 这条断言会因为巧合而通过，什么都没测到。这里让焦点停在**最后**一个
    /// 项目 `z` 上，删掉中间的空组 `m`，第 0 行是 `a`——错了就看得见。
    #[test]
    fn x_removes_an_empty_project_and_leaves_the_cursor_where_the_focus_is() {
        let (mut app, _dir) = App::test_app();
        app.pinned = vec!["/w/m".into()];
        app.set_sessions(vec![session_in(1, "/w/a"), session_in(2, "/w/z")]);
        // 组序是 a / m / z，焦点落在 z 的格子上
        app.view = View::grid(1);
        app.list_state.select(Some(3));
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("z".to_string()),
            "前提：一开始光标和焦点都在 z 上"
        );

        // 走到空组 m（`Tab` 从 z 绕回 a，再一下到 m），再把它拿掉
        handle_key(&mut app, key(KeyCode::Char('2'))).unwrap();
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("m".to_string())
        );
        handle_key(&mut app, key(KeyCode::Char('x'))).unwrap();

        assert_eq!(app.groups.len(), 2, "空项目该被拿掉");
        assert!(app.groups.iter().all(|g| g.name != "m"));
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("z".to_string()),
            "▶ 还在 z 的格子上，光标就不能退回第 0 行的 a——那会让 n 开进 a"
        );
    }

    /// **方向键跨过项目边界时，「当前项目」得跟着走。**
    ///
    /// 九宫格现在铺的是所有项目的会话，左右一走就可能跨进另一个项目。不同步
    /// 的话，底栏还写着上一个项目名、`n` 会把新会话开进上一个项目里，而用户
    /// 眼睛盯着的分明是另一格——「屏幕和状态各说各的」正是这一版要消灭的东西。
    #[test]
    fn moving_the_focus_across_a_project_boundary_switches_the_current_project() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![session_in(1, "/w/a"), session_in(2, "/w/b")]);
        app.view = View::grid(0);
        // 光标停在 a 的会话行上（进九宫格时两边本来就是对齐的）
        app.list_state.select(Some(1));

        handle_key(&mut app, key(KeyCode::Right)).unwrap();
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("b".to_string()),
            "焦点已经在 b 的格子上了，当前项目就必须是 b"
        );
        assert_eq!(app.current_dir(), std::path::PathBuf::from("/w/b"));
    }

    /// 换完项目再按 `g` 回列表，落点必须还是那个项目。两个视图共用一个光标，
    /// `Tab` 只挪动其中一个就等于把它们劈开了。
    #[test]
    fn tab_then_g_lands_on_the_same_project() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![session_in(1, "/w/a"), session_in(2, "/w/b")]);
        app.view_mode = crate::ui::ViewMode::Grid;
        app.view = View::grid(0);

        handle_key(&mut app, key(KeyCode::Tab)).unwrap();
        handle_key(&mut app, key(KeyCode::Char('g'))).unwrap();
        assert!(matches!(app.view, View::Board));
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("b".to_string()),
            "九宫格里换了项目，回列表不能又回到原来那个"
        );
    }

    /// 一个会话都没有时，直接说怎么开一个。分组之后九宫格画的是所有项目的
    /// 会话，「这里没有但别处可能有」这种半句话不再存在。
    #[test]
    fn an_empty_grid_says_how_to_start_a_session() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(Vec::new());
        app.view = View::grid(0);

        let c = grid_text(&mut app);
        assert!(c.contains("还没有会话"), "空状态要说人话：{c}");
        assert!(c.contains("按n新建"), "并指出下一步怎么做：{c}");
    }

    /// 焦点从 0 一路走到 2 再走回来，视图始终留在九宫格里。
    #[test]
    fn arrows_move_the_focus_and_stay_in_the_grid() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=3).map(|i| session(i, SessionState::Idle)).collect());
        app.view = View::grid(0);

        handle_key(&mut app, key(KeyCode::Right)).unwrap();
        assert!(matches!(app.view, View::Grid { focus: 1, .. }));
        handle_key(&mut app, key(KeyCode::Down)).unwrap();
        assert!(matches!(app.view, View::Grid { focus: 2, .. }));
        handle_key(&mut app, key(KeyCode::Left)).unwrap();
        assert!(matches!(app.view, View::Grid { focus: 1, .. }));
        handle_key(&mut app, key(KeyCode::Up)).unwrap();
        assert!(matches!(app.view, View::Grid { focus: 0, .. }));
    }

    #[test]
    fn f3_moves_to_the_next_tile_like_the_right_arrow() {
        // 跟会话视图里的 F3 是同一个动作，两处语义一致
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=3).map(|i| session(i, SessionState::Idle)).collect());
        app.view = View::grid(2);
        handle_key(&mut app, key(KeyCode::F(3))).unwrap();
        assert!(matches!(app.view, View::Grid { focus: 0, .. }), "到头回绕");
    }

    #[test]
    fn enter_zooms_into_the_focused_session() {
        // 格子只读，交互全靠放大——这条路径断了，九宫格就没法用了
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=3).map(|i| session(i, SessionState::Idle)).collect());
        app.view = View::grid(2);
        handle_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(matches!(app.view, View::Attached(3)));
        assert!(app.need_sessions, "会话标题要显示项目名，得重拉一次列表");
    }

    #[test]
    fn g_switches_back_to_list_mode_and_remembers_it() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![session(1, SessionState::Idle)]);
        app.view_mode = crate::ui::ViewMode::Grid;
        app.view = View::grid(0);
        handle_key(&mut app, key(KeyCode::Char('g'))).unwrap();
        assert!(matches!(app.view, View::Board));
        assert_eq!(app.view_mode, crate::ui::ViewMode::List);
        // 记住选择：不落盘的话「记住」只在本次进程里成立
        assert_eq!(
            crate::settings::load_view_mode(&crate::settings::settings_path_for_socket(
                &app.socket
            )),
            Some(crate::ui::ViewMode::List),
            "切模式必须落盘"
        );
    }

    /// `g` 回列表要把光标带到焦点格上。反方向（列表 → 九宫格）由 `board.rs`
    /// 的 `g_enters_the_grid_focused_on_the_selected_session` 盯着。
    #[test]
    fn g_moves_the_list_cursor_to_the_focused_tile() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=6).map(|i| session(i, SessionState::Idle)).collect());
        app.list_state.select(Some(0));
        app.view_mode = crate::ui::ViewMode::Grid;
        app.view = View::grid(4);
        handle_key(&mut app, key(KeyCode::Char('g'))).unwrap();
        assert!(matches!(app.view, View::Board));
        assert_eq!(
            app.list_state.selected(),
            Some(5),
            "从第 5 格回列表，光标必须停在会话 5 那一行（组头占了第 0 行）——\
             不然接下来的 s/u 会停掉、回滚另一个会话"
        );
    }

    /// Ctrl+Q 那条出口的同步只能做在 `run()` 的按键循环里（`back_one_level`
    /// 是纯函数，拿不到 `list_state`），而循环要真终端才跑得起来、测不了。
    /// 能测的是它调的那个函数：在九宫格里对齐焦点、不在九宫格里一动不动
    /// （Ctrl+Q 是全局键，每次按都会经过它）。
    #[test]
    fn the_cursor_sync_follows_the_focus_and_leaves_other_views_alone() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=6).map(|i| session(i, SessionState::Idle)).collect());
        app.list_state.select(Some(0));
        app.view = View::grid(4);
        super::super::sync_board_cursor_from_grid(&mut app);
        // 行是 [组头, 1, 2, 3, 4, 5, 6]：第 5 格是会话 5，落在第 5 行
        assert_eq!(app.list_state.selected(), Some(5));

        // 从别的视图按 Ctrl+Q 时它也会被调到，那时候不该动光标
        app.view = View::Attached(1);
        app.list_state.select(Some(2));
        super::super::sync_board_cursor_from_grid(&mut app);
        assert_eq!(app.list_state.selected(), Some(2), "不在九宫格就别碰光标");
    }

    #[test]
    fn zooming_in_also_leaves_the_list_cursor_on_that_session() {
        // Enter 放大也是离开九宫格的一条路：从会话里 Ctrl+Q 出来就到列表，
        // 那时候光标得在刚才看的那个会话上。
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=6).map(|i| session(i, SessionState::Idle)).collect());
        app.list_state.select(Some(0));
        app.view = View::grid(3);
        handle_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(matches!(app.view, View::Attached(4)));
        assert_eq!(app.list_state.selected(), Some(4), "组头占了第 0 行");
    }

    #[test]
    fn typing_in_a_tile_does_nothing_at_all() {
        // 格子里任何按键都不会送进 agent（设计约束，见 View::Grid 的注释）。
        // 这里能验证的是「什么都没发生」：视图没变，也没冒出一句消息。
        // 挑的都是九宫格没有绑定的键——绑了的那几个（n/N/p/c/q/s/u/d/x/1-9）
        // 做的是看板上同名键的那件事，不是「打字」。`x` 从这张表里挪走了：
        // 它现在跟看板一样是「移除空项目」（见 `x_removes_an_empty_project_...`）。
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![session(1, SessionState::Idle)]);
        app.view = View::grid(0);
        for c in ['w', '中', 'z'] {
            handle_key(&mut app, key(KeyCode::Char(c))).unwrap();
            assert!(matches!(app.view, View::Grid { focus: 0, .. }));
            assert_eq!(app.message.text, "");
        }
    }

    #[test]
    fn q_quits_from_the_grid_just_like_from_the_list() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![session(1, SessionState::Idle)]);
        app.view = View::grid(0);
        handle_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        assert!(app.quit);
    }

    /// `n`/`N`/`p`/`c` 在九宫格里跟在列表里必须是同一件事（设计文档的按键表
    /// 里它们标着「不变」）。四个键实际要开的视图都得先问守护进程要数据，
    /// 而单测里没有守护进程——所以这里断言的是「两个视图的结果一模一样」：
    /// 同一句失败提示、都没有把用户甩到别的屏幕上。两边共用同一个函数
    /// （`open_new_session`/`open_project_picker`/`open_secrets`），拿到数据
    /// 之后开哪个视图这件事由「共用」本身保证，不会分叉。
    #[test]
    fn the_board_keys_behave_identically_in_the_grid() {
        for c in ['n', 'N', 'p', 'c'] {
            let (mut on_board, _d1) = App::test_app();
            on_board.set_sessions(vec![session(1, SessionState::Idle)]);
            on_board.list_state.select(Some(0));
            on_board.view = View::Board;
            super::super::board::handle_key(&mut on_board, key(KeyCode::Char(c))).unwrap();

            let (mut on_grid, _d2) = App::test_app();
            on_grid.set_sessions(vec![session(1, SessionState::Idle)]);
            on_grid.view = View::grid(0);
            handle_key(&mut on_grid, key(KeyCode::Char(c))).unwrap();

            assert_eq!(
                on_grid.message.text, on_board.message.text,
                "「{c}」在两个视图里给的反馈必须一样"
            );
            assert!(!on_grid.message.text.is_empty(), "失败了要说话：{c}");
            assert!(
                matches!(on_grid.view, View::Grid { focus: 0, .. }),
                "拿不到数据就留在原地，不能把用户甩到别的屏幕上：{c}"
            );
        }
    }

    /// 会话全没了还按 `s`：不能拿 `sessions[focus]` 直接索引。
    ///
    /// **底栏说的必须跟屏幕正中说的是两件事。** 空九宫格的正中已经写着
    /// 「还没有会话，按 n 新建」；底栏这一句说的是「这一下按键没有作用对象」，
    /// 跟看板上同名的情形共用同一条词条。两处都说「这里什么都没有」，只是
    /// 措辞不同的话，用户会以为屏幕在告诉他两件事。
    #[test]
    fn actions_on_an_empty_board_say_so_instead_of_panicking() {
        let (mut app, _dir) = App::test_app();
        app.view = View::grid(0);
        handle_key(&mut app, key(KeyCode::Char('s'))).unwrap();
        // 「底栏和空屏说的不是同一句话」由
        // `the_empty_screen_and_the_bar_do_not_say_the_same_thing_twice` 单独盯
        assert_eq!(app.message.text, "没有选中会话");
        handle_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(matches!(app.view, View::Grid { .. }), "空看板放大不了");
    }

    /// 空九宫格上按 `s`，屏幕上同时有两句话：正中一句「这里什么都没有」、
    /// 底栏一句「这一下没有作用对象」。它们必须是**两个事实**，不能是同一件
    /// 事的两种措辞——同屏出现时用户会以为那是两件事，然后去找第二件。
    #[test]
    fn the_empty_screen_and_the_bar_do_not_say_the_same_thing_twice() {
        let (mut app, _dir) = App::test_app();
        app.view = View::grid(0);
        handle_key(&mut app, key(KeyCode::Char('s'))).unwrap();

        let centre = grid_text(&mut app);
        assert!(
            centre.contains("还没有会话"),
            "正中说的是「这里空的」：{centre}"
        );
        assert!(centre.contains("按n新建"), "并指出下一步：{centre}");
        assert_eq!(
            app.message.text, "没有选中会话",
            "底栏说的是另一件事：这一下按键没有作用对象"
        );
        // 反向守卫：底栏那句话不许再变回一句「这里什么都没有」
        assert!(
            !app.message.text.contains("按 n"),
            "底栏不该再重复一遍「按 n 新建」：{}",
            app.message.text
        );
    }

    // ———— 视图：渲染 ————

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        let area = buf.area;
        let mut s = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    s.push_str(cell.symbol());
                }
            }
            s.push('\n');
        }
        s
    }

    /// 去掉空白再比：ratatui 给宽字符后面那个 cell 塞的是空格，逐 cell 拼
    /// 出来的文本每个汉字后面都夹一个空格（同 mod.rs 里既有的做法）。
    fn squashed(term: &Terminal<ratatui::backend::TestBackend>) -> String {
        buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    /// 满足条件的格子坐标。断言颜色时逐 cell 找比按行列硬算稳。
    fn cells_with(
        buf: &ratatui::buffer::Buffer,
        pred: impl Fn(&ratatui::buffer::Cell) -> bool,
    ) -> Vec<(u16, u16)> {
        let mut out = Vec::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if buf.cell((x, y)).map(&pred).unwrap_or(false) {
                    out.push((x, y));
                }
            }
        }
        out
    }

    fn entry(id: u32, text: &str) -> ScreenEntry {
        ScreenEntry {
            id,
            lines: vec![vec![sp(text)]],
        }
    }

    #[test]
    fn tiles_show_the_session_status_in_the_title() {
        use ratatui::backend::TestBackend;

        // 标题就是状态指示器：扫一眼就要知道谁在干活、谁停了。
        let sessions = vec![
            session(1, SessionState::Working),
            session(2, SessionState::Stopped),
        ];
        let screens = vec![entry(1, "hello-from-one"), entry(2, "hello-from-two")];
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &sessions,
                &screens,
                0,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();

        let c = squashed(&term);
        assert!(c.contains("干活中"), "干活中的会话要标出来：{c}");
        assert!(c.contains("已停止"), "停掉的会话格子留着、标题写明：{c}");
        assert!(c.contains("hello-from-one"), "格子里要有真实画面：{c}");
        assert!(
            c.contains("hello-from-two"),
            "停掉的会话画面冻在最后一帧：{c}"
        );
    }

    #[test]
    fn the_focused_tile_is_the_only_one_with_a_cyan_border() {
        use ratatui::backend::TestBackend;

        let sessions = vec![
            session(1, SessionState::Idle),
            session(2, SessionState::Idle),
        ];
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &sessions,
                &[],
                1,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();

        let buf = term.backend().buffer();
        // 焦点在第二格（右半屏），青色边框只该出现在右半边
        let cyan_xs: Vec<u16> = (0..buf.area.width)
            .filter(|x| {
                (0..buf.area.height).any(|y| {
                    buf.cell((*x, y))
                        .map(|c| c.style().fg == Some(Color::Cyan))
                        .unwrap_or(false)
                })
            })
            .collect();
        assert!(!cyan_xs.is_empty(), "焦点格必须看得出来");
        assert!(
            cyan_xs.iter().all(|x| *x >= 40),
            "只有焦点格该高亮，实际高亮的列：{cyan_xs:?}"
        );
    }

    /// 细框和粗框的字符集。焦点靠**字符本身**区分，所以测试也只能按字符
    /// 认——按颜色认就会把「红态下焦点没标记」这个 bug 一路放过去。
    const THIN: &str = "─│┌┐└┘";
    const THICK: &str = "━┃┏┓┗┛";

    /// **焦点必须一眼看得出来，而且不能只靠颜色。**
    ///
    /// 上一版焦点是「青色 + `Modifier::BOLD`」，注释还写着「笔画粗细不受
    /// 主题影响」——但 BOLD 加在框线字符上，绝大多数终端字体根本没有加粗的
    /// 框线字形，这个修饰会被直接忽略。于是焦点实际只剩「青 vs 暗」一个
    /// 维度，浅色主题下几乎看不出来。换 Thick 换的是字符本身，终端支不支持
    /// BOLD、用户配的什么配色，都不影响。
    #[test]
    fn the_focused_tile_is_the_only_one_with_a_thick_border() {
        use ratatui::backend::TestBackend;

        let sessions = vec![
            session(1, SessionState::Idle),
            session(2, SessionState::Idle),
        ];
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &sessions,
                &[],
                1,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();

        let buf = term.backend().buffer();
        let thick = cells_with(buf, |c| THICK.contains(c.symbol()));
        let thin = cells_with(buf, |c| THIN.contains(c.symbol()));
        assert!(!thick.is_empty(), "焦点格必须有粗框");
        assert!(!thin.is_empty(), "非焦点格必须还是细框");
        // 焦点在第二格（右半屏）
        assert!(
            thick.iter().all(|(x, _)| *x >= 40),
            "只有焦点格该用粗框，实际用粗框的列：{:?}",
            thick.iter().map(|(x, _)| *x).collect::<Vec<_>>()
        );
        assert!(
            thin.iter().all(|(x, _)| *x < 40),
            "非焦点格不该混进粗框区，实际细框的列：{:?}",
            thin.iter().map(|(x, _)| *x).collect::<Vec<_>>()
        );
    }

    /// 焦点格的标题整条反色（实心色块）。一条 1 格宽的边框线在浅色主题上
    /// 跟灰线几乎一样，用户得先知道「有选中这回事」才会去找它；实心色块
    /// 是屏幕上最扎眼的东西，不需要先学会看。
    #[test]
    fn the_focused_tiles_title_is_a_solid_block() {
        use ratatui::backend::TestBackend;

        let sessions = vec![
            session(1, SessionState::Idle),
            session(2, SessionState::Idle),
        ];
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &sessions,
                &[],
                1,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();

        let buf = term.backend().buffer();
        let block = cells_with(buf, |c| c.style().bg == Some(Color::Cyan));
        assert!(!block.is_empty(), "焦点格的标题该是一条青色实心块");
        assert!(
            block.iter().all(|(x, _)| *x >= 40),
            "色块只该出现在焦点格上，实际的列：{:?}",
            block.iter().map(|(x, _)| *x).collect::<Vec<_>>()
        );
        // 色块得在标题那一行上，不是飘在格子中间
        assert!(
            block.iter().all(|(_, y)| *y == 0),
            "色块该贴在标题行上，实际的行：{:?}",
            block.iter().map(|(_, y)| *y).collect::<Vec<_>>()
        );
        // 色块要**铺满整格宽度**，不是只包住标题那几个字。只反色文字的话
        // 色块才占十来列，格子宽三四十列，扫视时那一小块跟别的格子的标题
        // 混在一起——「不够明显」正是这么来的。
        //
        // 按**跨度**断言而不是格数：宽字符（「空闲」这种 CJK）在 ratatui 里
        // 只有首格带样式，后一格是个不带样式的空串占位。数格子的话每个中文
        // 都会少算一格，断言就变成了在数标题里有几个汉字。
        let left = block.iter().map(|(x, _)| *x).min().unwrap();
        let right = block.iter().map(|(x, _)| *x).max().unwrap();
        // 焦点格是右边那个：x 从 40 到 79，左右各一列边框
        assert_eq!(left, 41, "色块该从格子左边框内侧起头");
        assert_eq!(right, 78, "色块该一直铺到右边框内侧");
    }

    /// 非焦点格的画面要退到背景层。颜色和框线字符两个维度都已经用满了，
    /// 对比度是第三个——另外八格暗下去，焦点格不用再加装饰就自己跳出来。
    #[test]
    fn the_tiles_you_are_not_on_recede() {
        use ratatui::backend::TestBackend;

        let sessions = vec![
            session(1, SessionState::Idle),
            session(2, SessionState::Idle),
        ];
        let screens = vec![entry(1, "左边这格"), entry(2, "右边这格")];
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &sessions,
                &screens,
                1,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();

        let buf = term.backend().buffer();
        let dimmed = cells_with(buf, |c| {
            c.style().add_modifier.contains(Modifier::DIM) && c.symbol() != " "
        });
        assert!(!dimmed.is_empty(), "非焦点格的画面该压暗");
        // 焦点在右半屏，压暗的只该出现在左半屏的画面里
        assert!(
            dimmed.iter().all(|(x, _)| *x < 40),
            "焦点格的画面不该被压暗，实际压暗的列：{:?}",
            dimmed.iter().map(|(x, _)| *x).collect::<Vec<_>>()
        );
    }

    #[test]
    fn disconnected_tiles_turn_red_like_the_other_views() {
        use ratatui::backend::TestBackend;

        // 断连时九个静止的画面看上去跟活的一模一样。列表和会话视图都靠
        // 红边框说「这是过期快照」，格子不能例外。
        let sessions = vec![
            session(1, SessionState::Working),
            session(2, SessionState::Working),
        ];
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &sessions,
                &[],
                1,
                Chrome {
                    connected: false,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();

        let buf = term.backend().buffer();
        // 只看框线本身：标题里的状态词是另一套颜色（干活中就是青的），
        // 它跟连没连上没关系。粗细两套字符都要算进来——只数细框的话，
        // 焦点格（粗框）会整个溜出这条断言，「每一格都红」就成了空话。
        let is_border =
            |c: &ratatui::buffer::Cell| THIN.contains(c.symbol()) || THICK.contains(c.symbol());
        let borders = cells_with(buf, is_border);
        assert!(!borders.is_empty(), "格子总该有边框");
        let red_borders = cells_with(buf, |c| is_border(c) && c.style().fg == Some(Color::Red));
        assert_eq!(
            red_borders.len(),
            borders.len(),
            "断连时每一格的边框都得是红的，不能有格子还留着「一切正常」的颜色"
        );
        // 焦点格（右半屏）靠**框线字符**区分：颜色已经被「数据过期」占用了。
        // 这里原来断言的是 BOLD，而 BOLD 在框线字符上根本画不出来——
        // 测试过了，屏幕上却什么都没有。
        let thick = cells_with(buf, |c| THICK.contains(c.symbol()));
        assert!(!thick.is_empty(), "断连时也要看得出焦点在哪一格");
        assert!(
            thick.iter().all(|(x, _)| *x >= 40),
            "只有焦点格该用粗框，实际用粗框的列：{thick:?}"
        );
        // 标题色块跟着边框一起转红。一个格子上同时挂着青色块和红框，
        // 用户会当成两件不同的事，反而比不标还乱。
        assert!(
            cells_with(buf, |c| c.style().bg == Some(Color::Cyan)).is_empty(),
            "断连时焦点色块该是红的，不该还留着青色"
        );
        assert!(
            !cells_with(buf, |c| c.style().bg == Some(Color::Red)).is_empty(),
            "断连时焦点格照样要有实心色块"
        );
    }

    #[test]
    fn a_tile_never_draws_a_cursor() {
        use ratatui::backend::TestBackend;

        // 只读的格子画光标只会误导用户在这里打字
        // Frame 上的 cursor_position 是 pub(crate)，测不到；换个等价的问法：
        // 先把光标停在一个哨兵位置，画完之后它必须还在原地——挪动它的
        // 唯一途径是 `set_cursor_position`，而九宫格根本不该调它
        // （附加视图会调，那边才有人打字）。
        let sessions = vec![session(1, SessionState::Working)];
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.set_cursor_position((7, 7)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &sessions,
                &[entry(1, "x")],
                0,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();
        assert_eq!(
            term.get_cursor_position().unwrap(),
            ratatui::layout::Position { x: 7, y: 7 },
            "格子里不该有光标"
        );
    }

    #[test]
    fn page_number_shows_up_only_when_there_is_more_than_one_page() {
        use ratatui::backend::TestBackend;

        let many: Vec<SessionInfo> = (1..=12).map(|i| session(i, SessionState::Idle)).collect();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &many,
                &[],
                0,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();
        assert!(squashed(&term).contains("1/2"), "多页要画页码");

        // 翻到第二页：页码跟着走
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &many,
                &[],
                9,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();
        assert!(squashed(&term).contains("2/2"));

        // 单页画 1/1 是噪音
        let few: Vec<SessionInfo> = (1..=3).map(|i| session(i, SessionState::Idle)).collect();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &few,
                &[],
                0,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();
        assert!(!squashed(&term).contains("1/1"), "单页不画页码");
    }

    #[test]
    fn a_tiny_terminal_gets_a_sentence_instead_of_a_mangled_grid() {
        use ratatui::backend::TestBackend;

        let sessions: Vec<SessionInfo> = (1..=9).map(|i| session(i, SessionState::Idle)).collect();
        let mut term = Terminal::new(TestBackend::new(40, 12)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &sessions,
                &[],
                0,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();
        let c = squashed(&term);
        assert!(c.contains("窗口太小"), "画不下就直说：{c}");
        assert!(c.contains("放大终端窗口"), "还要说清下一步怎么办：{c}");
        assert!(!c.contains("干活"), "这时候不该再画格子：{c}");
    }

    #[test]
    fn an_empty_board_explains_how_to_get_a_session() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &[],
                &[],
                0,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();
        let c = squashed(&term);
        assert!(
            c.contains("还没有会话"),
            "空看板要说话，不能只画一个空框：{c}"
        );
        assert!(c.contains("按n新建"), "n 在这一屏就能按，直接说：{c}");
        assert!(
            !c.contains("回列表"),
            "别让用户先绕回列表——n 在这儿就管用：{c}"
        );
    }

    #[test]
    fn the_page_number_does_not_sit_on_a_tile_border() {
        use ratatui::backend::TestBackend;

        // 页码原来直接画在 area 右下角，正好压在最底下那排格子的边框上，
        // 看起来像边框破了个洞。现在先从底部切一行出来给它。
        let many: Vec<SessionInfo> = (1..=12).map(|i| session(i, SessionState::Idle)).collect();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &many,
                &[],
                0,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();

        let text = buffer_text(term.backend().buffer());
        let last_row = text.lines().last().unwrap();
        assert!(last_row.contains("1/2"), "页码就在这一行：{last_row:?}");
        assert!(
            !last_row.contains('─') && !last_row.contains('└') && !last_row.contains('│'),
            "页码那一行不该有任何格子边框：{last_row:?}"
        );
    }

    #[test]
    fn a_tile_without_a_screen_yet_still_draws_its_title() {
        use ratatui::backend::TestBackend;

        // 画面和会话列表是两路请求，慢一拍很正常：配不上的格子只画标题，
        // 不能因为找不到画面就整格消失。
        let sessions = vec![session(7, SessionState::Working)];
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw_grid(
                f,
                f.area(),
                &sessions,
                &[entry(99, "别人的画面")],
                0,
                Chrome {
                    connected: true,
                    lang: Lang::Zh,
                },
                false,
            )
        })
        .unwrap();
        let c = squashed(&term);
        assert!(c.contains("干活中"), "标题照画：{c}");
        assert!(
            !c.contains("别人的画面"),
            "id 对不上的画面绝不能画进来：{c}"
        );
    }

    /// 焦点格上按 `i` 开框，收件人钉成那一格的会话 id。
    #[test]
    fn i_opens_a_reply_box_addressed_to_the_focused_session() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=3).map(|i| session(i, SessionState::Idle)).collect());
        app.view = View::grid(1);

        handle_key(&mut app, key(KeyCode::Char('i'))).unwrap();
        match &app.view {
            View::Grid {
                focus,
                reply: Some(d),
            } => {
                assert_eq!(*focus, 1, "开框不该动焦点");
                assert_eq!(d.id, 2, "收件人该是焦点那一格的会话");
                assert!(d.text.is_empty(), "刚开的框该是空的");
            }
            _ => panic!("按 i 该开出回复框"),
        }
    }

    /// **框开着的时候动作键必须失效。** 这是整个功能里最贵的一个错：
    /// 用户打「so」的第一个字母就把会话停了，而停止不可撤销。
    /// `reply_key` 那边测的是判断，这里测的是「判断真的挡在了 handle_key 前面」
    /// ——两层都要，中间漏一层键就照样漏下去了。
    #[test]
    fn action_keys_do_not_fire_while_the_reply_box_is_open() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=3).map(|i| session(i, SessionState::Idle)).collect());
        app.view = View::grid(1);
        handle_key(&mut app, key(KeyCode::Char('i'))).unwrap();

        for c in ['s', 'u', 'd', 'q', 'g', 'n', 'p', 'a', 'c', 'l'] {
            handle_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        assert!(!app.quit, "`q` 在框里只是个字母，不该退出 dct");
        match &app.view {
            View::Grid {
                focus,
                reply: Some(d),
            } => {
                assert_eq!(*focus, 1, "焦点不该动");
                assert_eq!(d.text, "sudqgnpacl", "这些键该原样落进框里");
            }
            other => panic!(
                "该还停在开着框的九宫格里，实际换了视图（是九宫格：{}）",
                matches!(other, View::Grid { .. })
            ),
        }
    }

    /// 方向键在框里不动焦点——焦点一动，屏幕上「发给 X」那行就跟着变，
    /// 而用户正对着它打字。
    #[test]
    fn arrows_do_not_move_the_focus_while_the_box_is_open() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=3).map(|i| session(i, SessionState::Idle)).collect());
        app.view = View::grid(1);
        handle_key(&mut app, key(KeyCode::Char('i'))).unwrap();

        handle_key(&mut app, key(KeyCode::Right)).unwrap();
        handle_key(&mut app, key(KeyCode::Left)).unwrap();
        match &app.view {
            View::Grid {
                focus,
                reply: Some(d),
            } => {
                assert_eq!(*focus, 1);
                assert_eq!(d.id, 2, "收件人从头到尾是同一个");
            }
            _ => panic!("该还开着框"),
        }
    }

    /// Esc 关框、**不发**，而且不该顺手退出九宫格。
    #[test]
    fn esc_closes_the_box_and_leaves_the_grid_alone() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=3).map(|i| session(i, SessionState::Idle)).collect());
        app.view = View::grid(1);
        handle_key(&mut app, key(KeyCode::Char('i'))).unwrap();
        handle_key(&mut app, key(KeyCode::Char('x'))).unwrap();
        handle_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert!(
            matches!(
                app.view,
                View::Grid {
                    focus: 1,
                    reply: None
                }
            ),
            "Esc 该只关掉框，人还留在九宫格的同一格上"
        );
    }

    /// 关掉的框不许留着上次的半句话。留着的话，用户下次按 `i` 一回车，
    /// 上次没发的残句就跟着发出去了——而发出去撤不回来。
    #[test]
    fn reopening_the_box_starts_from_a_blank_draft() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=3).map(|i| session(i, SessionState::Idle)).collect());
        app.view = View::grid(1);

        handle_key(&mut app, key(KeyCode::Char('i'))).unwrap();
        for c in "别发这句".chars() {
            handle_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_key(&mut app, key(KeyCode::Esc)).unwrap();
        handle_key(&mut app, key(KeyCode::Char('i'))).unwrap();

        match &app.view {
            View::Grid { reply: Some(d), .. } => {
                assert!(
                    d.text.is_empty(),
                    "重开的框必须是空的，实际留着「{}」",
                    d.text
                )
            }
            _ => panic!("该开着框"),
        }
    }

    /// 一个会话都没有时按 `i` 不该开出一个没人收的框。
    #[test]
    fn i_on_an_empty_grid_says_so_instead_of_opening_a_box() {
        let (mut app, _dir) = App::test_app();
        app.view = View::grid(0);
        handle_key(&mut app, key(KeyCode::Char('i'))).unwrap();
        assert!(
            matches!(app.view, View::Grid { reply: None, .. }),
            "没有收件人就不该开框"
        );
        assert!(!app.message.text.is_empty(), "得说一句为什么没反应");
    }

    /// 框要画出来，而且**收件人必须在屏幕上**。发错 agent 撤不回来，
    /// 用户打字时眼睛就在这一行上，收件人不能只存在于内存里。
    #[test]
    fn the_reply_box_names_who_it_is_addressed_to() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions((1..=3).map(|i| session(i, SessionState::Idle)).collect());
        app.view = View::grid(1);
        handle_key(&mut app, key(KeyCode::Char('i'))).unwrap();
        for c in "继续".chars() {
            handle_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }

        let c = grid_text(&mut app);
        assert!(c.contains("2claude"), "回复行要写明发给谁：{c}");
        assert!(c.contains("继续"), "打的字要显示出来：{c}");
    }

    /// 空框时要把「直接回车 = 同意」写出来。这是最高频的用法，不写的话
    /// 用户会以为必须先打点什么才能回。
    #[test]
    fn an_empty_box_says_that_a_bare_enter_approves() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![session(1, SessionState::Idle)]);
        app.view = View::grid(0);
        handle_key(&mut app, key(KeyCode::Char('i'))).unwrap();

        let c = grid_text(&mut app);
        assert!(
            c.contains("直接回车表示同意"),
            "空框该说清回车是干什么的：{c}"
        );
    }

    /// 回复行是**盖**在最后一行上的，不是从上面切一行。切的话 80×24 下
    /// 内容区跌破 MIN_ROWS，框一开整个九宫格就换成「窗口太小」。
    #[test]
    fn opening_the_box_does_not_shrink_the_grid_away() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = App::test_app();
        app.connected = true;
        app.set_sessions((1..=4).map(|i| session(i, SessionState::Idle)).collect());
        app.view = View::grid(0);
        handle_key(&mut app, key(KeyCode::Char('i'))).unwrap();

        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let buf = term.backend().buffer();
        let screen: String = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect();
        assert!(!screen.contains("窗口太小"), "开框不该把格子挤没：{screen}");
        assert!(screen.contains("claude"), "格子该还在：{screen}");
    }
}
