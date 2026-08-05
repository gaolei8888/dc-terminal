//! 九宫格视图：平铺所有会话的实时画面，只读。
//!
//! 上半截是布局数学，全是纯函数，跟终端、协议、会话都没关系，能独立测；
//! 下半截是按键和渲染，跟 `board.rs`/`pick.rs` 一样的 `handle_key` + `draw`。

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Paragraph};

use super::app::App;
use super::view::{is_plain_key, View};
use super::widgets::{char_width, screen_to_lines, status_label, status_style};
use super::{dim, session_action};
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
/// 这里**没有**一条把按键转发给 agent 的路径，这是设计约束：格子只读，
/// 想打字按 Enter 放大（见 `View::Grid` 的注释）。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Grid { focus } = app.view else {
        return Ok(());
    };
    let total = app.sessions.len();
    match key.code {
        KeyCode::Up => {
            app.view = View::Grid {
                focus: move_focus(focus, total, Dir::Up),
            }
        }
        KeyCode::Down => {
            app.view = View::Grid {
                focus: move_focus(focus, total, Dir::Down),
            }
        }
        KeyCode::Left => {
            app.view = View::Grid {
                focus: move_focus(focus, total, Dir::Left),
            }
        }
        // F3 = 「下一个」，跟会话视图里的 F3 是同一个动作，肌肉记忆只练一次
        KeyCode::Right | KeyCode::F(3) => {
            app.view = View::Grid {
                focus: move_focus(focus, total, Dir::Right),
            }
        }
        // 回列表前把列表光标对到焦点格上，理由见 sync_board_cursor_from_grid
        KeyCode::Char('g') if is_plain_key(&key) => {
            super::sync_board_cursor_from_grid(app);
            app.view = View::Board;
        }
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
        KeyCode::Enter => {
            if let Some(id) = app.sessions.get(focus).map(|s| s.id) {
                // 会话标题要显示项目名
                app.need_sessions = true;
                // 放大也是一条离开九宫格的路：从会话里再退出来就到了列表，
                // 那时候光标同样得落在这个会话上（见 sync_board_cursor_from_grid）
                super::sync_board_cursor_from_grid(app);
                app.view = View::Attached(id);
            }
        }
        // 跟看板同一套动作，作用在焦点格上——共用 `session_action`，
        // 不各抄一份（抄了将来只会改一半）。
        KeyCode::Char('s') | KeyCode::Char('u') | KeyCode::Char('d') if is_plain_key(&key) => {
            app.message = match app.sessions.get(focus).map(|s| s.id) {
                Some(id) => session_action(app, key.code, id),
                None => "还没有会话".into(),
            };
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let View::Grid { focus } = app.view else {
        return;
    };
    draw_grid(
        f,
        area,
        &app.sessions,
        &app.grid_screens,
        focus,
        app.connected,
    );
}

/// 画九宫格。格子的顺序 = 当页会话的顺序；画面按 id 跟 `screens` 配对，
/// 一时没配上的格子只画标题和空白——下一轮 300ms 就有了，比画错内容强。
///
/// 跟 `App` 解耦（只吃它真正用得上的那几样）是为了能在测试里直接喂 fixture，
/// 不必为了断言一句「窗口太小」去拼一个完整的 `App`。
fn draw_grid(
    f: &mut Frame,
    area: Rect,
    sessions: &[SessionInfo],
    screens: &[ScreenEntry],
    focus: usize,
    connected: bool,
) {
    if area.width < MIN_COLS || area.height < MIN_ROWS {
        // 说人话说清下一步做什么：这是用户自己能修好的事
        f.render_widget(
            Paragraph::new("窗口太小，放大终端窗口后再看九宫格").centered(),
            centered_line(area),
        );
        return;
    }
    if sessions.is_empty() {
        // `n` 在九宫格里跟在列表里是同一个键，直接说怎么开，别让用户
        // 先绕回列表
        f.render_widget(
            Paragraph::new("还没有会话，按 n 开一个").centered(),
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
        // 标题就是状态指示器：状态词用 status_style 上色，跟列表同一套颜色
        // （已停止是灰的），扫一眼九个格子就知道谁在干活、谁停了。
        let title = Line::from(vec![
            Span::raw(format!(" {} {} ", info.id, info.profile)),
            Span::styled(
                format!("{} ", status_label(info.state)),
                status_style(info.state),
            ),
        ]);
        // 断连时整屏格子一律红框：九个静止的画面看上去跟活的一模一样，
        // 不给个视觉提示，用户会以为 agent 都不动了（列表和会话视图断连时
        // 也是转红框，三处一致）。焦点格用青色；其余用 DIM 而不是 DarkGray——
        // 后者是 ANSI 亮黑，有些主题把它设成背景同色，整圈边框会隐形
        // （见 mod.rs 里 DIM 的注释）。
        //
        // 断连时焦点格是红色加粗，不是青色：颜色已经被「数据过期」这件事
        // 占用了，焦点只能换一个维度来标。全都染成同一种红的话，用户就找不到
        // 自己按方向键移到哪儿了。
        let border = if !connected {
            let red = Style::default().fg(Color::Red);
            if focused {
                red.add_modifier(Modifier::BOLD)
            } else {
                red
            }
        } else if focused {
            Style::default().fg(Color::Cyan)
        } else {
            dim()
        };
        let block = Block::bordered().title(title).border_style(border);
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
            f.render_widget(Paragraph::new(screen_to_lines(&cropped)), inner);
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
        }
    }

    /// 焦点从 0 一路走到 2 再走回来，视图始终留在九宫格里。
    #[test]
    fn arrows_move_the_focus_and_stay_in_the_grid() {
        let (mut app, _dir) = App::test_app();
        app.sessions = (1..=3).map(|i| session(i, SessionState::Idle)).collect();
        app.view = View::Grid { focus: 0 };

        handle_key(&mut app, key(KeyCode::Right)).unwrap();
        assert!(matches!(app.view, View::Grid { focus: 1 }));
        handle_key(&mut app, key(KeyCode::Down)).unwrap();
        assert!(matches!(app.view, View::Grid { focus: 2 }));
        handle_key(&mut app, key(KeyCode::Left)).unwrap();
        assert!(matches!(app.view, View::Grid { focus: 1 }));
        handle_key(&mut app, key(KeyCode::Up)).unwrap();
        assert!(matches!(app.view, View::Grid { focus: 0 }));
    }

    #[test]
    fn f3_moves_to_the_next_tile_like_the_right_arrow() {
        // 跟会话视图里的 F3 是同一个动作，两处语义一致
        let (mut app, _dir) = App::test_app();
        app.sessions = (1..=3).map(|i| session(i, SessionState::Idle)).collect();
        app.view = View::Grid { focus: 2 };
        handle_key(&mut app, key(KeyCode::F(3))).unwrap();
        assert!(matches!(app.view, View::Grid { focus: 0 }), "到头回绕");
    }

    #[test]
    fn enter_zooms_into_the_focused_session() {
        // 格子只读，交互全靠放大——这条路径断了，九宫格就没法用了
        let (mut app, _dir) = App::test_app();
        app.sessions = (1..=3).map(|i| session(i, SessionState::Idle)).collect();
        app.view = View::Grid { focus: 2 };
        handle_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(matches!(app.view, View::Attached(3)));
        assert!(app.need_sessions, "会话标题要显示项目名，得重拉一次列表");
    }

    #[test]
    fn g_goes_back_to_the_list() {
        let (mut app, _dir) = App::test_app();
        app.sessions = vec![session(1, SessionState::Idle)];
        app.view = View::Grid { focus: 0 };
        handle_key(&mut app, key(KeyCode::Char('g'))).unwrap();
        assert!(matches!(app.view, View::Board));
    }

    /// `g` 回列表要把光标带到焦点格上。反方向（列表 → 九宫格）由 `board.rs`
    /// 的 `g_enters_the_grid_focused_on_the_selected_session` 盯着。
    #[test]
    fn g_moves_the_list_cursor_to_the_focused_tile() {
        let (mut app, _dir) = App::test_app();
        app.sessions = (1..=6).map(|i| session(i, SessionState::Idle)).collect();
        app.list_state.select(Some(0));
        app.view = View::Grid { focus: 4 };
        handle_key(&mut app, key(KeyCode::Char('g'))).unwrap();
        assert!(matches!(app.view, View::Board));
        assert_eq!(
            app.list_state.selected(),
            Some(4),
            "从第 5 格回列表，光标必须停在第 5 行——\
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
        app.sessions = (1..=6).map(|i| session(i, SessionState::Idle)).collect();
        app.list_state.select(Some(0));
        app.view = View::Grid { focus: 4 };
        super::super::sync_board_cursor_from_grid(&mut app);
        assert_eq!(app.list_state.selected(), Some(4));

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
        app.sessions = (1..=6).map(|i| session(i, SessionState::Idle)).collect();
        app.list_state.select(Some(0));
        app.view = View::Grid { focus: 3 };
        handle_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(matches!(app.view, View::Attached(4)));
        assert_eq!(app.list_state.selected(), Some(3));
    }

    #[test]
    fn typing_in_a_tile_does_nothing_at_all() {
        // 格子里任何按键都不会送进 agent（设计约束，见 View::Grid 的注释）。
        // 这里能验证的是「什么都没发生」：视图没变，也没冒出一句消息。
        // 挑的都是九宫格没有绑定的键——绑了的那几个（n/N/p/c/q/s/u/d）
        // 做的是看板上同名键的那件事，不是「打字」。
        let (mut app, _dir) = App::test_app();
        app.sessions = vec![session(1, SessionState::Idle)];
        app.view = View::Grid { focus: 0 };
        for c in ['x', '中', 'z'] {
            handle_key(&mut app, key(KeyCode::Char(c))).unwrap();
            assert!(matches!(app.view, View::Grid { focus: 0 }));
            assert_eq!(app.message.text, "");
        }
    }

    #[test]
    fn q_quits_from_the_grid_just_like_from_the_list() {
        let (mut app, _dir) = App::test_app();
        app.sessions = vec![session(1, SessionState::Idle)];
        app.view = View::Grid { focus: 0 };
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
            on_board.sessions = vec![session(1, SessionState::Idle)];
            on_board.list_state.select(Some(0));
            on_board.view = View::Board;
            super::super::board::handle_key(&mut on_board, key(KeyCode::Char(c))).unwrap();

            let (mut on_grid, _d2) = App::test_app();
            on_grid.sessions = vec![session(1, SessionState::Idle)];
            on_grid.view = View::Grid { focus: 0 };
            handle_key(&mut on_grid, key(KeyCode::Char(c))).unwrap();

            assert_eq!(
                on_grid.message.text, on_board.message.text,
                "「{c}」在两个视图里给的反馈必须一样"
            );
            assert!(!on_grid.message.text.is_empty(), "失败了要说话：{c}");
            assert!(
                matches!(on_grid.view, View::Grid { focus: 0 }),
                "拿不到数据就留在原地，不能把用户甩到别的屏幕上：{c}"
            );
        }
    }

    #[test]
    fn actions_on_an_empty_board_say_so_instead_of_panicking() {
        // 会话全没了还按 s：不能拿 sessions[focus] 直接索引
        let (mut app, _dir) = App::test_app();
        app.view = View::Grid { focus: 0 };
        handle_key(&mut app, key(KeyCode::Char('s'))).unwrap();
        assert_eq!(app.message.text, "还没有会话");
        handle_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(matches!(app.view, View::Grid { .. }), "空看板放大不了");
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
        term.draw(|f| draw_grid(f, f.area(), &sessions, &screens, 0, true))
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
        term.draw(|f| draw_grid(f, f.area(), &sessions, &[], 1, true))
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
        term.draw(|f| draw_grid(f, f.area(), &sessions, &[], 1, false))
            .unwrap();

        let buf = term.backend().buffer();
        // 只看框线本身：标题里的状态词是另一套颜色（干活中就是青的），
        // 它跟连没连上没关系。
        let is_border = |c: &ratatui::buffer::Cell| "─│┌┐└┘".contains(c.symbol());
        let borders = cells_with(buf, is_border);
        assert!(!borders.is_empty(), "格子总该有边框");
        let red_borders = cells_with(buf, |c| is_border(c) && c.style().fg == Some(Color::Red));
        assert_eq!(
            red_borders.len(),
            borders.len(),
            "断连时每一格的边框都得是红的，不能有格子还留着「一切正常」的颜色"
        );
        // 焦点格（右半屏）靠加粗区分：颜色已经被「数据过期」占用了
        let bold_xs = cells_with(buf, |c| {
            c.style().fg == Some(Color::Red) && c.style().add_modifier.contains(Modifier::BOLD)
        });
        assert!(!bold_xs.is_empty(), "断连时也要看得出焦点在哪一格");
        assert!(
            bold_xs.iter().all(|(x, _)| *x >= 40),
            "只有焦点格该加粗，实际加粗的列：{bold_xs:?}"
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
        term.draw(|f| draw_grid(f, f.area(), &sessions, &[entry(1, "x")], 0, true))
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
        term.draw(|f| draw_grid(f, f.area(), &many, &[], 0, true))
            .unwrap();
        assert!(squashed(&term).contains("1/2"), "多页要画页码");

        // 翻到第二页：页码跟着走
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw_grid(f, f.area(), &many, &[], 9, true))
            .unwrap();
        assert!(squashed(&term).contains("2/2"));

        // 单页画 1/1 是噪音
        let few: Vec<SessionInfo> = (1..=3).map(|i| session(i, SessionState::Idle)).collect();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw_grid(f, f.area(), &few, &[], 0, true))
            .unwrap();
        assert!(!squashed(&term).contains("1/1"), "单页不画页码");
    }

    #[test]
    fn a_tiny_terminal_gets_a_sentence_instead_of_a_mangled_grid() {
        use ratatui::backend::TestBackend;

        let sessions: Vec<SessionInfo> = (1..=9).map(|i| session(i, SessionState::Idle)).collect();
        let mut term = Terminal::new(TestBackend::new(40, 12)).unwrap();
        term.draw(|f| draw_grid(f, f.area(), &sessions, &[], 0, true))
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
        term.draw(|f| draw_grid(f, f.area(), &[], &[], 0, true))
            .unwrap();
        let c = squashed(&term);
        assert!(
            c.contains("还没有会话"),
            "空看板要说话，不能只画一个空框：{c}"
        );
        assert!(c.contains("按n开一个"), "n 在这一屏就能按，直接说：{c}");
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
        term.draw(|f| draw_grid(f, f.area(), &many, &[], 0, true))
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
        term.draw(|f| draw_grid(f, f.area(), &sessions, &[entry(99, "别人的画面")], 0, true))
            .unwrap();
        let c = squashed(&term);
        assert!(c.contains("干活中"), "标题照画：{c}");
        assert!(
            !c.contains("别人的画面"),
            "id 对不上的画面绝不能画进来：{c}"
        );
    }
}
