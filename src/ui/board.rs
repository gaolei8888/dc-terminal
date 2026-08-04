use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::proto::{ProfileEntry, Request, Response};

use super::app::App;
use super::view::{quick_start_target, secret_rows, View};
use super::widgets::{short_path, status_color, status_label, truncate, Msg};
use super::{act, move_sel, selected, DIM};

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Down => move_sel(&mut app.list_state, &app.sessions, 1),
        KeyCode::Up => move_sel(&mut app.list_state, &app.sessions, -1),
        KeyCode::Char('n') | KeyCode::Char('N') => {
            // entries 带的是完整信息（label/note/status/密钥提示/安装提示），
            // 渲染时把置灰项和原因画出来、四种状态各自路由到哪，见
            // pick_action 和下面 View::PickProfile 的按键分支。n 和 N
            // 都要这份列表——n 拿它判断上次那个 agent 现在还在不在
            // Ready，N 拿它渲染选择器——所以只拉一次，不分两条路各拉各的。
            match app.client().and_then(|c| c.call(Request::Profiles)) {
                Ok(Response::Profiles { entries, warning }) => {
                    // 把「拉完列表但没能直开」的三种落点（选择器为空、
                    // 建会话失败两种）收在一处，省得同一段 ListState
                    // 初始化抄三遍——那种抄法迟早有一份漏了空表守卫。
                    let picker = |entries: Vec<ProfileEntry>, warning: Option<String>| {
                        let mut state = ListState::default();
                        // daemon 目前总是至少返回九个内置 profile，这里
                        // 空表分支基本走不到；但选中一个不存在的下标，
                        // 按 Enter 就是 entries[0] 越界 panic——这种最坏
                        // 结果不该只靠"实践中到不了"兜底，一行守卫不值钱。
                        if !entries.is_empty() {
                            state.select(Some(0));
                        }
                        View::PickProfile {
                            entries,
                            state,
                            warning,
                        }
                    };
                    // 大写 N 一定要看一眼选择器，不查上次用的是谁；
                    // 小写 n 才去问 daemon 上次记的是哪个 agent。
                    let last = if key.code == KeyCode::Char('n') {
                        match app.client().and_then(|c| c.call(Request::LastProfile)) {
                            Ok(Response::LastProfile(l)) => l,
                            _ => None,
                        }
                    } else {
                        None
                    };
                    match quick_start_target(last.as_deref(), &entries) {
                        Some(name) => {
                            // 同 View::PickProfile 里 PickAction::Start 那支：
                            // 「n」等价于「已经替用户选好了上次那个」，
                            // 建完直接进会话，不用再让他确认一遍。
                            let dir = app.current_dir.display().to_string();
                            match app.client().and_then(|c| {
                                c.call(Request::Create {
                                    dir,
                                    profile: name,
                                    remember: true,
                                })
                            }) {
                                Ok(Response::Created { id }) => {
                                    app.need_sessions = true; // 会话标题要显示项目名
                                    app.view = View::Attached(id);
                                }
                                Ok(Response::Error(e)) => {
                                    app.message = Msg::err(e);
                                    app.view = picker(entries, warning);
                                }
                                _ => {
                                    app.message = Msg::err("创建失败".into());
                                    app.view = picker(entries, warning);
                                }
                            }
                        }
                        None => app.view = picker(entries, warning),
                    }
                }
                // 列表都拿不到，直开和选择器都没法走，只能告诉用户
                // 这次干瞪眼——留在 Board 上，视图没变，走到循环
                // 末尾 message_after_transition 会把这条消息原样
                // 留住（同其他分支，不用 continue 抢跑跳过收尾）。
                Ok(Response::Error(e)) => app.message = Msg::err(e),
                _ => app.message = Msg::err("拿不到 agent 列表".into()),
            }
        }
        KeyCode::Char('p') => {
            // 拿不到列表就不进选择器：进去看见一片空白，用户会以为
            // 自己从来没开过项目。
            match app.client().and_then(|c| c.call(Request::Projects)) {
                Ok(Response::Projects(mut all)) => {
                    // 全新守护进程列表是空的，补上启动目录，
                    // 保证第一次用也不会看到空列表。
                    let start = app.start_dir.display().to_string();
                    if !all.contains(&start) {
                        all.push(start);
                    }
                    let mut state = ListState::default();
                    state.select(Some(0));
                    app.view = View::PickProject {
                        all,
                        filter: String::new(),
                        state,
                        typing_path: None,
                    };
                }
                Ok(Response::Error(e)) => app.message = Msg::err(e),
                _ => app.message = Msg::err("拿不到项目列表".into()),
            }
        }
        KeyCode::Char('c') => {
            // 拿不到列表就不进设置页：留在看板上给一句错误，总比
            // 弹进一个既没数据、又没地方显示错误的空白页强
            // （`View::Secrets` 没有 `warning` 字段，见其字段注释）。
            match app.client().and_then(|c| c.call(Request::Profiles)) {
                Ok(Response::Profiles { entries, .. }) => {
                    let mut state = ListState::default();
                    if !secret_rows(&entries).is_empty() {
                        state.select(Some(0));
                    }
                    app.view = View::Secrets {
                        entries,
                        state,
                        pending_delete: None,
                    };
                }
                Ok(Response::Error(e)) => app.message = Msg::err(e),
                _ => app.message = Msg::err("拿不到密钥列表".into()),
            }
        }
        KeyCode::Enter => {
            if let Some(id) = selected(&app.sessions, &app.list_state).map(|s| s.id) {
                app.view = View::Attached(id);
                app.need_sessions = true; // 会话标题要显示项目名
            }
        }
        KeyCode::Char('u') => {
            app.message = act(app, |id| Request::Undo { id });
        }
        KeyCode::Char('s') => {
            app.message = act(app, |id| Request::Stop { id });
        }
        KeyCode::Char('d') => {
            if let Some(id) = selected(&app.sessions, &app.list_state).map(|s| s.id) {
                app.message = match app.client().and_then(|c| c.call(Request::Diff { id })) {
                    Ok(Response::Diff(v)) if v.is_empty() => "没有改动".into(),
                    Ok(Response::Diff(v)) => v
                        .iter()
                        .map(|f| format!("{} +{} -{}", f.path, f.added, f.removed))
                        .collect::<Vec<_>>()
                        .join("  ")
                        .into(),
                    Ok(Response::Error(e)) => Msg::err(e),
                    _ => Msg::err("请求失败".into()),
                };
            }
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
                Span::styled(
                    format!("{:<8}", status_label(s.state)),
                    Style::default().fg(status_color(s.state)),
                ),
                Span::raw(format!("{:<10}", s.profile)),
                Span::styled(
                    format!("{:<22}", truncate(&short_path(&s.dir), 22)),
                    Style::default().fg(DIM),
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
