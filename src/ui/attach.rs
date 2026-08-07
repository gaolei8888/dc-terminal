use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::i18n::Lang;
use crate::proto::{MouseForward, MouseForwardKind, Request};
use crate::session::{ScrollBy, ScrollState};

use super::app::App;
use super::key_to_input;
use super::view::View;
use super::widgets::{screen_to_lines, short_path, Msg};

/// 滚轮一格滚几行。3 是终端惯例，改了会跟用户在别处（浏览器、编辑器）的
/// 肌肉记忆打架。
const WHEEL_ROWS: i32 = 3;

/// 一次滚轮/翻页该做什么。纯函数，好测。
pub(crate) enum ScrollAction {
    /// 转发给 agent，dct 自己不处理。
    Forward,
    /// dct 自己滚这么多行；正数往上（看历史），负数往下。
    Scroll(i32),
    /// 什么都不做——没有历史可滚，滚了也白滚。
    Ignore,
}

/// 谁拿这一格滚轮。
///
/// 判据是「agent 有没有开鼠标上报」，不是「它在不在备用屏」——实测下来
/// Claude Code 备用屏 + 全套鼠标，codex 内联 + 完全不要鼠标，两个真实
/// agent 在这两个维度上正好相反。按鼠标分流恰好把两边都送到握着内容的
/// 那一方：Claude Code 自己管视口，codex 的历史在 dct 缓冲里。
pub(crate) fn wheel_action(st: &ScrollState, up: bool) -> ScrollAction {
    if st.agent_owns {
        return ScrollAction::Forward;
    }
    if st.max == 0 {
        return ScrollAction::Ignore;
    }
    ScrollAction::Scroll(if up { WHEEL_ROWS } else { -WHEEL_ROWS })
}

/// 翻页键归谁。`None` = 不归 dct 管，让它落到普通按键路径送给 agent。
///
/// `End` 用 `Scroll(-i32::MAX)` 而不是单独一个 `Bottom` 分支：`ScrollBy::Rows`
/// 在守护进程侧是 `saturating_sub` 之后再钳到 `[0, max]`，结果跟
/// `ScrollBy::Bottom` 完全一样，多一个分支只是多一处要测的东西。
pub(crate) fn key_scroll(st: &ScrollState, key: &KeyEvent, page: u16) -> Option<ScrollAction> {
    if st.agent_owns {
        return None;
    }
    // 一屏减 2 行：留两行重叠，翻页之后还能看到上一屏的尾巴，不然读长输出时
    // 每翻一页都要重新找位置。窗口太小时至少滚 1 行，不能算出 0 或负数。
    let step = i32::from(page).saturating_sub(2).max(1);
    match key.code {
        // 没有历史可翻时 PageUp/PageDown 也该放行给 agent，不能在这儿吃掉
        // 变成死键——跟 `wheel_action` 的 `max == 0` 判据是同一条路由规则，
        // 两个入口对同一个问题必须给同一个答案。一个刚建的、还没吐出一屏
        // 内容的 inline agent 正是这种情况：PageUp 对它来说是普通的编辑键。
        KeyCode::PageUp | KeyCode::PageDown if st.max == 0 => None,
        KeyCode::PageUp => Some(ScrollAction::Scroll(step)),
        KeyCode::PageDown => Some(ScrollAction::Scroll(-step)),
        KeyCode::End if st.offset > 0 => Some(ScrollAction::Scroll(-i32::MAX)),
        _ => None,
    }
}

/// 底栏那一句滚动提示。`None` = 不显示，让位给平时的按键表。
pub(crate) fn scroll_hint(st: &ScrollState, lang: Lang) -> Option<String> {
    if st.offset > 0 && st.new_lines > 0 {
        return Some(crate::i18n::msg::scroll_new_lines_below(lang, st.new_lines));
    }
    if st.offset > 0 {
        return Some(crate::i18n::msg::scrolled_up(lang, st.offset));
    }
    // agent 自己占着画面又不收鼠标：谁都滚不了。装死的话用户会以为
    // 滚轮坏了，一直试。
    if !st.agent_owns && st.alt_screen {
        return Some(crate::i18n::msg::agent_owns_the_screen(lang));
    }
    None
}

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
    } else if let Some(action) = key_scroll(
        &app.scroll,
        &key,
        // `app.screen` 是上一次 `Screen` 响应的行数，而那个行数正是 dct
        // 最后一次通过 `Request::Resize` 告诉 agent 的高度——`screen_spans()`
        // 按 vt100 的配置尺寸返回，永远是这个数（见 `pty.rs`）。用它当
        // 「一屏多高」不需要另外在 `handle_key` 里穿一份终端尺寸进来。
        app.screen.len() as u16,
    ) {
        // PageUp/PageDown/End 在「dct 自己攥着历史」时归 dct 管，直接在这里
        // 消费掉——不落进 `key_to_input`：那条路会把它们编码成转义序列发给
        // agent，而 agent 早就把这些键的处理权交出去了（`agent_owns` 判据
        // 见 `wheel_action` 的文档），送过去只会石沉大海，翻页变成死键。
        if let ScrollAction::Scroll(n) = action {
            // 失败就静默：跟下面 `Input` 分支不同，这不是用户主动敲字符
            // 没反应会分不清是卡顿还是断连——滚动失败一次，下一帧的
            // `Screen` 探测自然会把 `connected` 标成假，断连提示走那条路。
            let _ = app.client().and_then(|c| {
                c.call(Request::Scroll {
                    id,
                    by: ScrollBy::Rows(n),
                })
            });
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
    // 会话内容区左上角在真实终端上的坐标，鼠标事件换算列/行要用它。
    // 必须在这里记，不能让 `handle_mouse` 自己硬算边框宽度——布局改了
    // 硬算的数就错了，而且错得很安静（见 `App::screen_origin` 的文档）。
    app.screen_origin = Some((area.x + 1, area.y + 1));
    // 把 agent 屏幕里的光标位置映射到真实终端上。没有这一步用户
    // 看到的只是一张死截图，不知道自己打的字会落在哪。+1 是边框。
    let (row, col) = app.screen_cursor;
    let x = area.x + 1 + col;
    let y = area.y + 1 + row;
    if x < area.x + area.width.saturating_sub(1) && y < area.y + area.height.saturating_sub(1) {
        f.set_cursor_position((x, y));
    }
}

/// **这个函数里不许改 `app.message`。** 主循环在调用它之后可能直接
/// `continue` 回循环顶部，跳过了循环末尾清理陈旧消息的那一段——在这里
/// 设一条消息，它会一直挂在屏幕上直到下一次按键。同理也不用 `?` 把
/// `app.client()` 的错误往上抛：鼠标事件一秒钟能来几十个，某一次因为
/// 断线送失败不该让整个界面跟着退出——下一帧的 `Screen` 探测自然会把
/// `connected` 标成假，断连提示走那条既有的路。
///
/// 返回这次事件有没有真的送出一次请求（滚动，或者转发点击/松开）。调用方
/// （`run()`）靠这个判断值不值得为它触发一次完整的「取 Screen、重绘」——
/// 纯移动这类被就地丢弃的事件占了鼠标事件的大多数，不该背上这个代价
/// （见 `run()` 里排空鼠标事件那段的注释）。没有任何一条路径会失败到需要
/// 报错的地步，所以是 `bool` 不是 `Result<()>`——把「这里不会出错」这件事
/// 写进类型里，比只写在注释里更可信，调用点也就不再需要一个不会触发的 `?`。
pub(crate) fn handle_mouse(app: &mut App, m: MouseEvent) -> bool {
    let View::Attached(id) = app.view else {
        return false;
    };
    let (up, forwardable) = match m.kind {
        MouseEventKind::ScrollUp => (true, Some(MouseForwardKind::WheelUp)),
        MouseEventKind::ScrollDown => (false, Some(MouseForwardKind::WheelDown)),
        MouseEventKind::Down(b) => (false, Some(MouseForwardKind::Press(button_code(b)))),
        MouseEventKind::Up(b) => (false, Some(MouseForwardKind::Release(button_code(b)))),
        // 纯移动（以及拖拽）不转发：Claude Code 开了 ?1003h，每动一下就是
        // 一个事件，全部经 socket 转发过去量很大，换来的只是悬停高亮。
        // 这是有意的部分实现，不是遗漏。
        _ => (false, None),
    };
    let Some(kind) = forwardable else {
        return false;
    };

    let is_wheel = matches!(
        kind,
        MouseForwardKind::WheelUp | MouseForwardKind::WheelDown
    );
    if is_wheel {
        match wheel_action(&app.scroll, up) {
            ScrollAction::Ignore => return false,
            ScrollAction::Scroll(n) => {
                let _ = app.client().and_then(|c| {
                    c.call(Request::Scroll {
                        id,
                        by: ScrollBy::Rows(n),
                    })
                });
                return true;
            }
            ScrollAction::Forward => {}
        }
    } else if !app.scroll.agent_owns {
        // 不收鼠标的 agent 收到点击/松开事件只会看到一串乱码。
        return false;
    }

    // 终端坐标减掉内容区左上角，换算成 agent 画面里的坐标。任何一边越界
    // （点在了边框、底栏上）就直接丢——那些地方压根不是 agent 的画面。
    let Some((col, row)) = app
        .screen_origin
        .and_then(|(c0, r0)| Some((m.column.checked_sub(c0)?, m.row.checked_sub(r0)?)))
    else {
        return false;
    };

    let _ = app.client().and_then(|c| {
        c.call(Request::Mouse {
            id,
            event: MouseForward {
                col,
                row,
                kind,
                shift: m.modifiers.contains(KeyModifiers::SHIFT),
                alt: m.modifiers.contains(KeyModifiers::ALT),
                ctrl: m.modifiers.contains(KeyModifiers::CONTROL),
            },
        })
    });
    true
}

fn button_code(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionInfo, SessionState};

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
            is_agent: true,
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

    /// `handle_mouse` 靠 `screen_origin` 把终端坐标换算成 agent 画面里的
    /// 坐标，自己绝不硬算边框宽度——这条测试钉住 `draw` 真的记对了那个
    /// 坐标。少了它，`draw` 里 `app.screen_origin = Some((area.x, area.y))`
    /// （漏掉 `+1` 的边框偏移）这种改动全量测试照样 542/0 全绿，因为没有
    /// 别的测试断言过这个字段的值——鼠标点哪儿都会偏一格，而且偏得悄无
    /// 声息，正是 `screen_origin` 这个字段本来要防的那类 bug。
    #[test]
    fn draw_records_the_bordered_content_corner_as_the_screen_origin() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.sessions = vec![session(1, SessionState::Working)];
        app.view = View::Attached(1);
        // 不用整块屏幕：故意给一个不是 (0, 0) 的偏移，这样如果 `draw` 悄悄
        // 写成了直接抄 `area.x`/`area.y`（漏掉 `+1` 的边框），或者写死了
        // 一个常数，这条测试都能抓出来，而不是恰好在 (0, 0) 时侥幸对上。
        let area = Rect {
            x: 3,
            y: 2,
            width: 40,
            height: 12,
        };

        term.draw(|f| draw(f, area, &mut app)).unwrap();

        assert_eq!(
            app.screen_origin,
            Some((area.x + 1, area.y + 1)),
            "内容区左上角要算上边框（+1），不能是 area 自己的坐标"
        );
    }

    fn own(agent_owns: bool, max: usize, offset: usize, new_lines: usize) -> ScrollState {
        ScrollState {
            agent_owns,
            alt_screen: false,
            max,
            offset,
            new_lines,
        }
    }

    #[test]
    fn an_agent_that_wants_the_mouse_gets_the_wheel() {
        assert!(matches!(
            wheel_action(&own(true, 0, 0, 0), true),
            ScrollAction::Forward
        ));
    }

    #[test]
    fn otherwise_dct_scrolls_three_rows_per_notch() {
        assert!(matches!(
            wheel_action(&own(false, 500, 0, 0), true),
            ScrollAction::Scroll(3)
        ));
        assert!(matches!(
            wheel_action(&own(false, 500, 10, 0), false),
            ScrollAction::Scroll(-3)
        ));
    }

    #[test]
    fn there_is_nothing_to_scroll_when_there_is_no_history() {
        assert!(matches!(
            wheel_action(&own(false, 0, 0, 0), true),
            ScrollAction::Ignore
        ));
    }

    #[test]
    fn page_keys_belong_to_the_agent_when_it_owns_the_viewport() {
        // None 表示「不归我管」，让它落到普通按键路径送给 agent
        assert!(key_scroll(&own(true, 0, 0, 0), &key(KeyCode::PageUp), 24).is_none());
        assert!(key_scroll(&own(true, 0, 0, 0), &key(KeyCode::End), 24).is_none());
    }

    /// 跟 `wheel_action` 的 `there_is_nothing_to_scroll_when_there_is_no_history`
    /// 是同一条路由规则的两个入口，必须给出同一个答案：没有历史时 dct 不该
    /// 吃掉 PageUp/PageDown。不这样的话，一个刚建的、还没吐出一屏内容的
    /// inline agent 上按 PageUp 会被 dct 悄悄吞掉、什么反应都没有——而滚轮
    /// 在同样的状态下（`wheel_action` 的 `max == 0` 分支）老老实实转发。
    #[test]
    fn page_keys_are_not_scroll_keys_when_there_is_no_history() {
        assert!(key_scroll(&own(false, 0, 0, 0), &key(KeyCode::PageUp), 24).is_none());
        assert!(key_scroll(&own(false, 0, 0, 0), &key(KeyCode::PageDown), 24).is_none());
    }

    #[test]
    fn page_keys_scroll_a_screen_minus_two() {
        let up = key_scroll(&own(false, 500, 0, 0), &key(KeyCode::PageUp), 24).unwrap();
        assert!(matches!(up, ScrollAction::Scroll(22)));
        let down = key_scroll(&own(false, 500, 30, 0), &key(KeyCode::PageDown), 24).unwrap();
        assert!(matches!(down, ScrollAction::Scroll(-22)));
    }

    #[test]
    fn a_tiny_window_still_scrolls_at_least_one_row() {
        let up = key_scroll(&own(false, 500, 0, 0), &key(KeyCode::PageUp), 2).unwrap();
        assert!(matches!(up, ScrollAction::Scroll(1)), "别算出 0 行或负数");
    }

    #[test]
    fn ordinary_keys_are_not_scroll_keys() {
        assert!(key_scroll(&own(false, 500, 10, 0), &key(KeyCode::Char('a')), 24).is_none());
    }

    #[test]
    fn end_with_nothing_scrolled_is_not_a_scroll_key() {
        // offset == 0：已经在底部，没什么可回的。让它落到普通按键路径——
        // 对大多数 agent 来说 End 本来就是「跳到行尾」的编辑键，dct 不该
        // 在什么都没滚的时候把这个键吃掉。
        assert!(key_scroll(&own(false, 500, 0, 0), &key(KeyCode::End), 24).is_none());
    }

    #[test]
    fn end_jumps_all_the_way_down() {
        let a = key_scroll(&own(false, 500, 40, 0), &key(KeyCode::End), 24).unwrap();
        assert!(matches!(a, ScrollAction::Scroll(n) if n == -i32::MAX));
    }

    #[test]
    fn the_hint_says_how_much_is_waiting_below() {
        let st = own(false, 500, 40, 12);
        let h = scroll_hint(&st, Lang::Zh).unwrap();
        assert!(h.contains("12"), "得说清有多少新东西: {h}");
        // 用户正翻着历史、新内容还在堆积，是最想立刻跳回去看最新输出的
        // 时候——光说「有新东西」不说怎么回去，等于只交代了一半。
        assert!(h.contains("End"), "得说清怎么回去: {h}");
        let en = scroll_hint(&st, Lang::En).unwrap();
        assert!(
            !en.is_empty() && en.contains("12") && en.contains("End"),
            "英文版也要说清同一个数字，以及怎么回去: {en}"
        );
    }

    #[test]
    fn the_hint_says_how_to_get_back_when_nothing_is_new() {
        let st = own(false, 500, 40, 0);
        let h = scroll_hint(&st, Lang::Zh).unwrap();
        assert!(h.contains("40"), "得说清翻了多远: {h}");
        assert!(h.contains("End"), "得说清怎么回去: {h}");
        let en = scroll_hint(&st, Lang::En).unwrap();
        assert!(
            en.contains("40") && en.contains("End"),
            "英文版也要说清: {en}"
        );
    }

    #[test]
    fn an_alt_screen_agent_that_ignores_the_mouse_gets_an_explanation() {
        let mut st = own(false, 0, 0, 0);
        st.alt_screen = true;
        let h = scroll_hint(&st, Lang::Zh).expect("这种情况谁都滚不了，必须说一声");
        assert!(!h.contains("End"), "都滚不了了就别提回底部: {h}");
        assert!(
            !h.contains("备用屏") && !h.contains("scrollback"),
            "不能有黑话: {h}"
        );
        let en = scroll_hint(&st, Lang::En).expect("英文版同样得说一声");
        assert!(!en.contains("End"));
        let en_lower = en.to_lowercase();
        assert!(
            !en_lower.contains("scrollback")
                && !en_lower.contains("alt screen")
                && !en_lower.contains("alternate screen")
                && !en_lower.contains("buffer"),
            "英文版也不能有黑话: {en}"
        );
    }

    #[test]
    fn a_fresh_session_with_no_history_says_nothing() {
        assert!(scroll_hint(&own(false, 0, 0, 0), Lang::Zh).is_none());
        assert!(scroll_hint(&own(true, 0, 0, 0), Lang::Zh).is_none());
    }

    /// `handle_key` 端到端地钉住路由：`key_scroll` 说「归我管」的时候，
    /// dct 得真的把它吃掉，不能落进 `key_to_input` 转发给 agent。
    #[test]
    fn a_page_key_is_consumed_silently_when_dct_owns_scrolling() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Attached(1);
        app.scroll = own(false, 500, 0, 0);
        handle_key(&mut app, key(KeyCode::PageUp)).unwrap();
        assert!(
            app.message.text.is_empty(),
            "滚动请求失败要静默，不能每次翻页都报错: {}",
            app.message.text
        );
    }

    /// 反过来：agent 攥着视口时，`key_scroll` 返回 `None`，PageUp 必须落到
    /// 普通按键路径。测试 App 是断连的，这条路径失败了会把原因写进
    /// `app.message`——用这条侧面证明按键确实被转发过去了，不是被
    /// dct 自己悄悄吞掉。
    #[test]
    fn a_page_key_falls_through_to_the_agent_when_it_owns_the_viewport() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Attached(1);
        app.scroll = ScrollState {
            agent_owns: true,
            ..Default::default()
        };
        handle_key(&mut app, key(KeyCode::PageUp)).unwrap();
        assert!(
            !app.message.text.is_empty(),
            "agent 攥着视口时 PageUp 该走普通按键路径，断连了就该跟别的按键一样报错"
        );
    }

    fn mouse_ev(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn button_code_maps_left_middle_right_to_zero_one_two() {
        assert_eq!(button_code(MouseButton::Left), 0);
        assert_eq!(button_code(MouseButton::Middle), 1);
        assert_eq!(button_code(MouseButton::Right), 2);
    }

    #[test]
    fn handle_mouse_does_nothing_off_the_attached_view() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Board;
        assert!(!handle_mouse(
            &mut app,
            mouse_ev(MouseEventKind::ScrollUp, 5, 5)
        ));
        assert!(app.message.text.is_empty());
    }

    /// client 是 `None`（`App::test_app` 就是断连的），下面每种可转发的
    /// 事件都会在真正发请求那一步失败——这条测试盯的是那次失败不能像
    /// `handle_key` 里的 `Input` 分支那样反手把错误焊进 `app.message`。
    /// 这是 e0ba1ec 那类 bug 的另一种化身：主循环在 `handle_mouse` 后面
    /// 可能直接跳回循环顶部，跳过了收尾清理，这里设的消息会一直挂到下
    /// 一次按键。
    ///
    /// `agent_owns = true` 是故意的：只有这样滚轮和点击/松开才会真的走到
    /// 发请求那一步（而不是被 `wheel_action`/「不收鼠标的 agent」提前
    /// 挡下），这条测试才对得上「调用失败」这四个字，而不是「压根没调用」。
    /// 顺带验证了 `handle_mouse` 的返回值：真发了请求的返回 `true`，
    /// 纯移动这种从不发请求的返回 `false`。
    #[test]
    fn handle_mouse_never_touches_the_message_even_when_the_call_fails() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Attached(1);
        app.scroll.agent_owns = true;
        app.screen_origin = Some((1, 1));
        for (kind, expect_acted) in [
            (MouseEventKind::ScrollUp, true),
            (MouseEventKind::ScrollDown, true),
            (MouseEventKind::Down(MouseButton::Left), true),
            (MouseEventKind::Up(MouseButton::Left), true),
            (MouseEventKind::Moved, false),
        ] {
            let acted = handle_mouse(&mut app, mouse_ev(kind, 5, 5));
            assert_eq!(acted, expect_acted, "事件是 {kind:?}");
            assert!(
                app.message.text.is_empty(),
                "handle_mouse 不许改 message，事件是 {kind:?}"
            );
        }
    }

    #[test]
    fn a_click_with_no_known_screen_origin_is_dropped_not_guessed() {
        // 还没画过一帧时 `screen_origin` 是 `None`。猜一个边框宽度会在布局
        // 改了之后悄悄点歪；不猜、直接丢，比错误的坐标安全。
        let (mut app, _dir) = App::test_app();
        app.view = View::Attached(1);
        app.scroll.agent_owns = true; // 让点击走到坐标换算那一步，不要提前 return
        app.screen_origin = None;
        assert!(!handle_mouse(
            &mut app,
            mouse_ev(MouseEventKind::Down(MouseButton::Left), 5, 5)
        ));
        assert!(app.message.text.is_empty());
    }
}
