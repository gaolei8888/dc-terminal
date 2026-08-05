use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem};

use super::app::App;
use super::view::{is_plain_key, View};
use super::widgets::{short_path, status_label, status_style, truncate};
use super::{
    dim, move_sel, open_new_session, open_project_picker, open_secrets, selected, session_action,
};

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Char('q') if is_plain_key(&key) => app.quit = true,
        KeyCode::Down => move_sel(&mut app.list_state, &app.sessions, 1),
        KeyCode::Up => move_sel(&mut app.list_state, &app.sessions, -1),
        KeyCode::Char('n') | KeyCode::Char('N') if is_plain_key(&key) => {
            open_new_session(app, key.code)
        }
        KeyCode::Char('p') if is_plain_key(&key) => open_project_picker(app),
        KeyCode::Char('c') if is_plain_key(&key) => open_secrets(app),
        KeyCode::Enter => {
            if let Some(id) = selected(&app.sessions, &app.list_state).map(|s| s.id) {
                app.view = View::Attached(id);
                app.need_sessions = true; // 会话标题要显示项目名
            }
        }
        KeyCode::Char('g') if is_plain_key(&key) => {
            // 进九宫格时焦点落在列表当前选中的那一行：两个视图对「当前是
            // 哪个会话」的认知必须一致，不然按完 g 焦点跳到别处，用户会
            // 以为自己按错了键。
            app.view = View::Grid {
                focus: app.list_state.selected().unwrap_or(0),
            };
        }
        // 三个动作跟九宫格共用 session_action，区别只在「当前会话」是
        // 选中行还是焦点格
        KeyCode::Char('u') | KeyCode::Char('s') | KeyCode::Char('d') if is_plain_key(&key) => {
            app.message = match selected(&app.sessions, &app.list_state).map(|s| s.id) {
                Some(id) => session_action(app, key.code, id),
                None => "没有选中会话".into(),
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
        "dct 会话看板".to_string()
    } else {
        "dct 会话看板（连接已断开，数据可能已过期）".to_string()
    };
    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .map(|s| {
            ListItem::new(Line::from(vec![
                Span::raw(format!("{:>3}  ", s.id)),
                Span::styled(format!("{:<8}", status_label(s.state)), status_style(s.state)),
                Span::raw(format!("{:<10}", s.profile)),
                Span::styled(
                    format!("{:<22}", truncate(&short_path(&s.dir), 22)),
                    dim(),
                ),
                Span::raw(truncate(&s.activity, 60)),
            ]))
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
        app.list_state.select(Some(2));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(matches!(app.view, View::Grid { focus: 2 }));
    }
}
