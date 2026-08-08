use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem};

use super::app::App;
use super::view::is_plain_key;
use super::widgets::{pad_to, status_label, status_style, truncate};
use super::{dim, open_new_session, open_project_picker, open_secrets, session_action};
use crate::i18n::{msg, text, Key};

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
        Style::default().fg(Color::Red)
    };
    let title = if app.connected {
        text(Key::BoardTitle, app.lang).to_string()
    } else {
        msg::title_with(app.lang, Key::BoardTitle, text(Key::Disconnected, app.lang))
    };
    // 当前项目：整组左侧一条竖色条。不靠光标行——光标只标「哪一行」，
    // 项目要的是「哪一片」，隔着屏幕就得认得出来。
    let current = app
        .list_state
        .selected()
        .and_then(|i| super::view::group_of(&app.rows, i));

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
            let mut spans = vec![Span::styled(bar, Style::default().fg(Color::Cyan))];
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
                    spans.push(Span::styled(
                        pad_to(&g.name, 18),
                        if gone {
                            dim()
                        } else {
                            Style::default().add_modifier(Modifier::BOLD)
                        },
                    ));
                    spans.push(Span::styled(pad_to(&truncate(&g.parent, 16), 18), dim()));
                    if gone {
                        spans.push(Span::styled(text(Key::ProjectDirGone, app.lang), dim()));
                    } else if g.sessions.is_empty() {
                        spans.push(Span::styled(text(Key::NoSessionsHere, app.lang), dim()));
                    } else {
                        let agents: Vec<String> = g
                            .agent_counts()
                            .into_iter()
                            .map(|(name, n)| format!("{name}×{n}"))
                            .collect();
                        spans.push(Span::raw(pad_to(&agents.join(" "), 22)));
                        let failed = g.failed();
                        if failed > 0 {
                            spans.push(Span::styled(
                                msg::failed_count(app.lang, failed),
                                Style::default().fg(Color::Red),
                            ));
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
                    spans.push(Span::raw(pad_to(&s.profile, 10)));
                    // 会话行不重复项目名——组头已经说了，宽度还给 activity，
                    // 它是屏幕上最先被截断的信息。
                    spans.push(Span::raw(truncate(&s.activity, 76)));
                }
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(title),
            )
            .highlight_symbol("▶ "),
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
    #[test]
    fn a_group_whose_folder_is_gone_says_so_instead_of_vanishing() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/definitely-not-here")]);

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(c.contains("definitely-not-here"), "组还在看板上：{c}");
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
