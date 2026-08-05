use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem};

use super::app::App;
use super::view::{is_plain_key, Scope};
use super::widgets::{pad_to, short_path, status_label, status_style, truncate};
use super::{
    dim, move_sel, open_new_session, open_project_picker, open_secrets, selected, session_action,
};
use crate::i18n::{msg, text, Key};

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Char('q') if is_plain_key(&key) => app.quit = true,
        KeyCode::Down => move_sel(&mut app.list_state, &app.visible, 1),
        KeyCode::Up => move_sel(&mut app.list_state, &app.visible, -1),
        // 作用域跟着用户走，不跟着视图走：九宫格上的 `a` 是同一个键、
        // 改的是同一个 `app.scope`，切视图不会让会话数变化。
        KeyCode::Char('a') if is_plain_key(&key) => super::toggle_scope(app),
        KeyCode::Char('n') | KeyCode::Char('N') if is_plain_key(&key) => {
            open_new_session(app, key.code)
        }
        KeyCode::Char('p') if is_plain_key(&key) => open_project_picker(app),
        KeyCode::Char('c') if is_plain_key(&key) => open_secrets(app),
        // `l` = language。设置页跟 `c 密钥` 挨着：两个都是「配置」类入口，
        // 而且跟 a/g 一样，两个视图共用同一个键。
        KeyCode::Char('l') if is_plain_key(&key) => super::open_settings(app),
        KeyCode::Enter => {
            if let Some(id) = selected(&app.visible, &app.list_state).map(|s| s.id) {
                super::enter_session(app, id);
            }
        }
        // 切模式并记住。焦点/光标的对齐在 toggle_view_mode 里统一做——
        // 两个方向各写一份的话，迟早只改对一半。
        KeyCode::Char('g') if is_plain_key(&key) => super::toggle_view_mode(app),
        // 三个动作跟九宫格共用 session_action，区别只在「当前会话」是
        // 选中行还是焦点格
        KeyCode::Char('u') | KeyCode::Char('s') | KeyCode::Char('d') if is_plain_key(&key) => {
            app.message = match selected(&app.visible, &app.list_state).map(|s| s.id) {
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
    // 作用域进标题：屏幕上少了一半会话时，用户必须一眼看出是过滤掉了
    // 而不是会话没了。
    let scoped = match app.scope {
        Scope::CurrentProject => Key::BoardTitle,
        Scope::AllProjects => Key::BoardTitleAllProjects,
    };
    let title = if app.connected {
        text(scoped, app.lang).to_string()
    } else {
        msg::title_with(app.lang, scoped, text(Key::Disconnected, app.lang))
    };
    // 只看当前项目时不画路径列：底栏已经写着当前项目，每一行再重复一遍
    // 是把 22 列花在同一句话上。腾出来的宽度给 activity——它是现在屏幕上
    // 最先被截断的信息。
    let show_dir = app.scope == Scope::AllProjects;
    let activity_cols = if show_dir { 60 } else { 82 };
    let items: Vec<ListItem> = app
        .visible
        .iter()
        .map(|s| {
            let mut spans = vec![
                Span::raw(format!("{:>3}  ", s.id)),
                Span::styled(
                    pad_to(status_label(s.state, app.lang), 8),
                    status_style(s.state),
                ),
                Span::raw(pad_to(&s.profile, 10)),
            ];
            if show_dir {
                spans.push(Span::styled(
                    pad_to(&truncate(&short_path(&s.dir), 22), 22),
                    dim(),
                ));
            }
            spans.push(Span::raw(truncate(&s.activity, activity_cols)));
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
        }
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
    /// dc_desktop 的会话。看板从来没有按项目过滤过。
    #[test]
    fn the_board_never_shows_another_projects_sessions() {
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/w/dc-terminal");
        app.sessions = vec![sess(1, "/w/dc-terminal"), sess(2, "/w/dc_desktop")];
        app.refresh_visible();

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(
            !c.contains("dc_desktop"),
            "别的项目的会话绝不能出现在看板上：{c}"
        );
    }

    /// 切到「全部项目」之后，别的项目的会话要回来，而且每行要标出它属于谁
    /// ——不标的话用户没法分辨屏幕上这些会话分别在哪。
    #[test]
    fn the_all_projects_view_brings_them_back_with_their_paths() {
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/w/dc-terminal");
        app.sessions = vec![sess(1, "/w/dc-terminal"), sess(2, "/w/dc_desktop")];
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        )
        .unwrap();

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let c = screen_text(&term);
        assert!(c.contains("dc_desktop"), "全部项目视图要列出别的项目：{c}");
        assert!(c.contains("全部项目"), "标题要说明当前处在哪个作用域：{c}");
    }

    /// 从「全部项目」进了别的项目的会话，按 F2 回看板时它不在作用域里——
    /// 会话看起来像是消失了。所以进会话时把当前项目切成它的目录：
    /// 「你在哪个会话里，当前项目就是哪个」。
    ///
    /// 必须给一句消息说明：静默改变当前项目正是这一版被判为「混乱」的原因。
    #[test]
    fn entering_a_session_from_another_project_switches_to_it() {
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/w/a");
        app.sessions = vec![sess(1, "/w/a"), sess(2, "/w/b")];
        app.scope = Scope::AllProjects;
        app.refresh_visible();
        app.list_state.select(Some(1));

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();

        assert!(matches!(app.view, View::Attached(2)), "进的是会话 2");
        assert_eq!(app.current_dir, PathBuf::from("/w/b"), "当前项目跟着会话走");
        assert!(
            app.message.text.contains("已切到"),
            "换了项目必须说一声，不能悄悄改：{}",
            app.message.text
        );
    }

    /// 反过来：进的是当前项目自己的会话，就不该冒出一句「已切到」——
    /// 什么都没变还报告一次，会让用户以为刚才误触了什么。
    #[test]
    fn entering_a_session_in_the_current_project_says_nothing() {
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/w/a");
        app.sessions = vec![sess(1, "/w/a")];
        app.refresh_visible();
        app.list_state.select(Some(0));

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();

        assert!(matches!(app.view, View::Attached(1)));
        assert!(
            !app.message.text.contains("已切到"),
            "项目没变就别报告：{}",
            app.message.text
        );
    }

    #[test]
    fn g_enters_the_grid_focused_on_the_selected_session() {
        // 两个视图对「当前是哪个会话」的认知必须一致：列表选中第三行，
        // 按 g 之后焦点就该落在第三格，不能弹回第一格。
        let (mut app, _dir) = App::test_app();
        app.sessions = (1..=4)
            .map(|id| SessionInfo {
                id,
                profile: "claude".into(),
                dir: "/tmp/a".into(),
                state: SessionState::Idle,
                activity: String::new(),
            })
            .collect();
        app.current_dir = PathBuf::from("/tmp/a");
        app.refresh_visible();
        app.list_state.select(Some(2));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(matches!(app.view, View::Grid { focus: 2 }));
        assert_eq!(
            app.view_mode,
            crate::ui::ViewMode::Grid,
            "g 切的是**模式**，不是打开一个附属页面——下次回家也该落在九宫格"
        );
    }
}
