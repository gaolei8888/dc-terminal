use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::proto::Request;

use super::app::App;
use super::key_to_input;
use super::view::View;
use super::widgets::{screen_to_lines, short_path, Msg};

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Attached(id) = app.view.clone() else {
        return Ok(());
    };
    // F2 是唯一被 dct 吃掉的键，其余一律 key_to_input 翻译成终端字节
    // 送进去——方向键、退格、Tab、Ctrl 组合都要能用，否则在 Claude Code
    // 里连打错字都退不了格。Esc 必须还给 agent——Claude Code 靠它
    // 取消/清空/关弹窗（底部那句 "Esc to cancel"）；Ctrl+B 也必须还回去，
    // 那是 Claude Code 的「转后台」。逆转键挑 F2 是因为没有 CLI agent
    // 在用它，不必搞双击透传那种隐形状态。
    if key.code == KeyCode::F(2) {
        app.view = super::home_view(app);
        app.need_sessions = true;
    } else if key.code == KeyCode::F(3) {
        // F3 = 直接切到下一个在跑的会话，不用先退回看板。选 F3 沿用
        // F2 的理由：没有 CLI agent 用 F 功能键，偷它不踩任何人。
        // 在 `visible` 里找下一个，不是全量：F3 该在你眼下这批会话里轮转。
        // 进会话时当前项目已经跟着切过去了（见 `enter_session`），所以正在
        // 附加的这个必定在 `visible` 里，轮转起点不会落空。
        match super::grid::next_running(&app.visible, id) {
            Some(next) => super::enter_session(app, next),
            None => {
                app.message =
                    crate::i18n::text(crate::i18n::Key::NoOtherRunningSession, app.lang).into()
            }
        }
    } else if let Some(text) = key_to_input(&key) {
        // 发送失败时不能静默吞掉——用户打字没反应会分不清是卡顿还是断连。
        // “连不上”这个视觉状态统一交给循环顶部的 List/Screen 探测去判定。
        if app
            .client()
            .and_then(|c| c.call(Request::Input { id, text }))
            .is_err()
        {
            app.message =
                Msg::err(crate::i18n::text(crate::i18n::Key::InputNotSent, app.lang).into());
        }
    }
    Ok(())
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let View::Attached(id) = &app.view else {
        return;
    };
    let id = *id;
    // 断连时用红色边框给出明确的视觉提示：界面上的数据是上一次成功请求
    // 留下的陈旧快照，不代表守护进程现在的真实状态。
    let border_style = if app.connected {
        Style::default()
    } else {
        Style::default().fg(Color::Red)
    };
    // 标题显示用户当初指定的项目目录，不是内部的 worktree 路径——
    // 给用户看 .git/dct-worktrees/s2 只会让他不知道自己在哪。
    let project = app
        .sessions
        .iter()
        .find(|s| s.id == id)
        .map(|s| short_path(&s.dir))
        .unwrap_or_default();
    let title = if app.connected {
        crate::i18n::msg::session_title(app.lang, id, &project)
    } else {
        crate::i18n::msg::session_title_disconnected(app.lang, id, &project)
    };
    f.render_widget(
        Paragraph::new(screen_to_lines(&app.screen)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        ),
        area,
    );
    // 把 agent 屏幕里的光标位置映射到真实终端上。没有这一步用户
    // 看到的只是一张死截图，不知道自己打的字会落在哪。+1 是边框。
    let (row, col) = app.screen_cursor;
    let x = area.x + 1 + col;
    let y = area.y + 1 + row;
    if x < area.x + area.width.saturating_sub(1) && y < area.y + area.height.saturating_sub(1) {
        f.set_cursor_position((x, y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionInfo, SessionState};
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
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

    #[test]
    fn f3_jumps_straight_to_the_next_running_session() {
        // 不用先退回看板：从会话 1 按 F3 直接落在下一个在跑的会话上。
        let (mut app, _dir) = App::test_app();
        app.sessions = vec![
            session(1, SessionState::Working),
            session(2, SessionState::Stopped),
            session(3, SessionState::Idle),
        ];
        // `session()` 的 dir 是 /tmp/a：对上当前项目，走真实的过滤路径
        app.current_dir = std::path::PathBuf::from("/tmp/a");
        app.refresh_visible();
        app.view = View::Attached(1);
        handle_key(&mut app, key(KeyCode::F(3))).unwrap();
        assert!(
            matches!(app.view, View::Attached(3)),
            "跳过停掉的 2，落在 3 上"
        );
        assert!(app.need_sessions, "会话标题要显示新会话的项目名");
    }

    #[test]
    fn f3_says_so_when_this_is_the_only_running_session() {
        // 唯一在跑的会话按 F3：不能跳回自己，也不能悄无声息什么都不做。
        let (mut app, _dir) = App::test_app();
        app.sessions = vec![session(1, SessionState::Working)];
        app.current_dir = std::path::PathBuf::from("/tmp/a");
        app.refresh_visible();
        app.view = View::Attached(1);
        handle_key(&mut app, key(KeyCode::F(3))).unwrap();
        assert!(matches!(app.view, View::Attached(1)), "不能跳回自己");
        assert!(!app.message.text.is_empty(), "得说一句，不能默不作声");
    }
}
