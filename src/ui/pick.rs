use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use crate::i18n::{msg, text, Key};
use crate::profile::ProfileStatus;
use crate::proto::{Request, Response, SecretPrompt};

use super::app::App;
use super::view::{
    digit_index, expand_path, pick_action, Pane, PickAction, ProjectPicker, SecretPhase, View,
};
use super::widgets::{pad_to, short_path, truncate, Msg};
use super::{dim, move_sel_n};

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match app.view.clone() {
        View::PickProfile { .. } => handle_pick_profile(app, key),
        View::PickProject(_) => handle_pick_project(app, key),
        _ => Ok(()),
    }
}

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
fn handle_pick_profile(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::PickProfile {
        entries,
        mut state,
        warning,
    } = app.view.clone()
    else {
        return Ok(());
    };
    if key.code == KeyCode::Esc {
        app.view = super::home_view(app);
    } else {
        // ↑↓ 只挪光标、不选定，所以放在算「选中第几项」之前：
        // 挪完直接落到 chosen = None，不会误触发下面的路由。
        let chosen: Option<usize> = match key.code {
            KeyCode::Down | KeyCode::Up => {
                let d = if key.code == KeyCode::Down { 1 } else { -1 };
                move_sel_n(&mut state, entries.len(), d);
                None
            }
            KeyCode::Enter => state.selected(),
            KeyCode::Char(c) => digit_index(c).filter(|i| *i < entries.len()),
            _ => None,
        };
        // 四条分支的落点：pick_action 只是个纯函数分类器，真正
        // 建会话/开安装窗口这些带副作用的活儿在这里做。
        app.view = match chosen.map(|i| (i, pick_action(&entries[i], app.lang))) {
            None => View::PickProfile {
                entries,
                state,
                warning,
            },
            Some((_, PickAction::Start(name))) => {
                // 选完直接进会话。用户选中的意图就是「我要用这个
                // agent 干活」，先弹回看板再让他找一遍自己刚建的
                // 会话是白让人做第二次选择。建失败才回选择器。
                let dir = app.current_dir.display().to_string();
                match app.client().and_then(|c| {
                    c.call(Request::Create {
                        dir,
                        profile: name,
                        // 选择器里选的就是用户真的要用的 agent——
                        // 与「帮你装 CLI」那条 remember=false 的路径区分开。
                        remember: true,
                    })
                }) {
                    Ok(Response::Created { id }) => {
                        app.need_sessions = true; // 会话标题要显示项目名
                        View::Attached(id)
                    }
                    Ok(Response::Error(ref e)) => {
                        app.message = Msg::err(crate::i18n::msg::error(app.lang, e));
                        View::PickProfile {
                            entries,
                            state,
                            warning,
                        }
                    }
                    _ => {
                        app.message = Msg::err(text(Key::CreateFailed, app.lang).into());
                        View::PickProfile {
                            entries,
                            state,
                            warning,
                        }
                    }
                }
            }
            Some((i, PickAction::AskSecret(_))) => {
                // AskSecret(usize) 里那个下标只是占位——pick_action
                // 只拿得到一个 &ProfileEntry，不知道它在列表里排第几
                // （见 PickAction 的注释）。真下标是这里的 i，
                // 从 entries[i] 取出来的正是被选中的这一行。
                let e = &entries[i];
                View::EnterSecret {
                    profile: e.name.clone(),
                    label: e.label.clone(),
                    // NeedsSecret 状态却没带 SecretPrompt 是数据不一致
                    // （daemon 那边的 bug），兜底成空提示而不是 panic——
                    // 用户最多看到少一行说明，不该因为这个直接崩溃。
                    prompt: e.secret.clone().unwrap_or(SecretPrompt {
                        hint: String::new(),
                        url: None,
                    }),
                    buf: String::new(),
                    phase: SecretPhase::Typing,
                    // 从选择器进来的意图是「开工」，存完直接建会话，
                    // 不回这里。
                    return_to_settings: false,
                }
            }
            Some((_, PickAction::Install { profile, command })) => {
                // 用命令行会话跑安装命令，让用户看着它装，而不是
                // 干等一句「装不了」。remember: false —— 这不是
                // 用户选的 agent，记了下次按 n 会掉进命令行。
                let dir = app.current_dir.display().to_string();
                match app.client().and_then(|c| {
                    c.call(Request::Create {
                        dir,
                        profile: "shell".into(),
                        remember: false,
                    })
                }) {
                    Ok(Response::Created { id }) => {
                        let line = format!("{}\n", command.join(" "));
                        let _ = app
                            .client()
                            .and_then(|c| c.call(Request::Input { id, text: line }));
                        app.message = msg::installing(app.lang, &profile).into();
                        app.need_sessions = true;
                        View::Attached(id)
                    }
                    _ => {
                        app.message = Msg::err(text(Key::CannotOpenInstallWindow, app.lang).into());
                        View::PickProfile {
                            entries,
                            state,
                            warning,
                        }
                    }
                }
            }
            Some((_, PickAction::Blocked(msg))) => {
                app.message = Msg::err(msg);
                View::PickProfile {
                    entries,
                    state,
                    warning,
                }
            }
        };
    }
    Ok(())
}

/// **这个函数里永远不要 `continue`。** 理由同上。
fn handle_pick_project(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::PickProject(mut p) = app.view.clone() else {
        return Ok(());
    };

    // ——手输路径态：可见字符全进输入框，不再当过滤用——
    if let Some(mut buf) = p.typing_path.clone() {
        match key.code {
            KeyCode::Esc => p.typing_path = None,
            KeyCode::Enter => {
                if buf.trim().is_empty() {
                    // expand_path("", base) 会解析成 base 自己，is_dir() 照样为真——
                    // 空输入不挡住的话，用户在这一步犹豫多按一次 Enter，
                    // 就会被无声切回启动目录。
                    app.message = Msg::err(text(Key::NoPathTyped, app.lang).into());
                } else {
                    let dir = expand_path(&buf, &app.start_dir);
                    if dir.is_dir() {
                        super::switch_project(app, dir);
                        return Ok(());
                    }
                    // 不是 git 仓库这件事不在这里判——留给 create()
                    app.message =
                        Msg::err(msg::not_a_directory(app.lang, &dir.display().to_string()));
                }
            }
            KeyCode::Backspace => {
                buf.pop();
                p.typing_path = Some(buf);
            }
            KeyCode::Char(c) => {
                buf.push(c);
                p.typing_path = Some(buf);
            }
            _ => {}
        }
        app.view = View::PickProject(p);
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => {
            app.view = super::home_view(app);
            return Ok(());
        }
        KeyCode::Tab | KeyCode::BackTab => {
            p.focus = match p.focus {
                Pane::Recent => Pane::Browse,
                Pane::Browse => Pane::Recent,
            };
            // 过滤词是对着上一栏打的，换了焦点就不成立了
            p.filter.clear();
        }
        KeyCode::Down | KeyCode::Up => {
            let d = if key.code == KeyCode::Down { 1 } else { -1 };
            match p.focus {
                // +1 是末行那个「手输路径…」，它不参与过滤，永远在
                Pane::Recent => {
                    let n = p.shown_recent().len() + 1;
                    move_sel_n(&mut p.recent_state, n, d);
                }
                Pane::Browse => {
                    let n = p.shown_entries().len();
                    move_sel_n(&mut p.browse_state, n, d);
                }
            }
        }
        // `→` 是「往里走」：在浏览栏进子目录，在最近栏把浏览器切到那个项目
        // 所在的位置——用户想从一个熟悉的项目附近开始找，这是最短的一步。
        KeyCode::Right => match p.focus {
            Pane::Browse => {
                let shown = p.shown_entries();
                if let Some(row) = p.browse_state.selected().and_then(|i| shown.get(i)) {
                    let next = p.cwd.join(&row.name);
                    p.browse_to(next);
                }
            }
            Pane::Recent => {
                let shown = p.shown_recent();
                if let Some(dir) = p.recent_state.selected().and_then(|i| shown.get(i)) {
                    p.browse_to(PathBuf::from(dir));
                    p.focus = Pane::Browse;
                }
            }
        },
        KeyCode::Left => {
            if p.focus == Pane::Browse {
                // 已经在根目录时 parent() 是 None——原地不动，不 panic
                if let Some(parent) = p.cwd.parent().map(|x| x.to_path_buf()) {
                    p.browse_to(parent);
                }
            }
        }
        KeyCode::Enter => {
            let chosen: Option<PathBuf> = match p.focus {
                Pane::Recent => {
                    let shown = p.shown_recent();
                    let i = p.recent_state.selected().unwrap_or(0);
                    match shown.get(i) {
                        Some(dir) => Some(PathBuf::from(dir)),
                        // 选中的是末行「手输路径…」
                        None => {
                            p.typing_path = Some(String::new());
                            None
                        }
                    }
                }
                // 浏览栏的 Enter 是**选定**，不是进入。走到一个目录上多半是
                // 因为它就是要找的项目；想再往下钻有 `→`，那是个方向键，
                // 语义天然就是「往里走」。
                Pane::Browse => {
                    let shown = p.shown_entries();
                    p.browse_state
                        .selected()
                        .and_then(|i| shown.get(i))
                        .map(|row| p.cwd.join(&row.name))
                }
            };
            if let Some(dir) = chosen {
                if dir.is_dir() {
                    super::switch_project(app, dir);
                    return Ok(());
                }
                // 列表里那条不删——可能只是外置盘没挂
                app.message = Msg::err(msg::cannot_find_anymore(
                    app.lang,
                    &short_path(&dir.display().to_string()),
                ));
            }
        }
        KeyCode::Backspace => {
            p.filter.pop();
            reset_cursor(&mut p);
        }
        KeyCode::Char(c) => {
            p.filter.push(c);
            reset_cursor(&mut p);
        }
        _ => {}
    }
    app.view = View::PickProject(p);
    Ok(())
}

/// 过滤变了就把光标收回第一行，否则它会停在一个已经被过滤掉的行号上。
fn reset_cursor(p: &mut ProjectPicker) {
    match p.focus {
        Pane::Recent => p.recent_state.select(Some(0)),
        Pane::Browse => p.browse_state.select(if p.shown_entries().is_empty() {
            None
        } else {
            Some(0)
        }),
    }
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    match &app.view {
        View::PickProfile { .. } => draw_pick_profile(f, area, app),
        View::PickProject(_) => draw_pick_project(f, area, app),
        _ => {}
    }
}

fn draw_pick_profile(f: &mut Frame, area: Rect, app: &mut App) {
    let View::PickProfile {
        entries,
        state,
        warning,
    } = &app.view
    else {
        return;
    };
    // 断连时用红色边框给出明确的视觉提示：界面上的数据是上一次成功请求
    // 留下的陈旧快照，不代表守护进程现在的真实状态。
    let border_style = if app.connected {
        Style::default()
    } else {
        Style::default().fg(Color::Red)
    };
    let items: Vec<ListItem> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let num = if i < 9 {
                format!("{}. ", i + 1)
            } else {
                "   ".to_string()
            };
            let reason = match &e.status {
                ProfileStatus::Ready => String::new(),
                ProfileStatus::NeedsSecret => text(Key::ReasonNeedsSecret, app.lang).into(),
                ProfileStatus::NeedsDependency { label } => {
                    msg::reason_needs_dependency(app.lang, label)
                }
                ProfileStatus::NotInstalled { .. } => {
                    text(Key::ReasonNotInstalled, app.lang).into()
                }
            };
            // 不可用的整行压暗，不只是把原因压暗——用户是先看名字再看原因的，
            // 名字亮着会让他先以为能用
            let base = if matches!(e.status, ProfileStatus::Ready) {
                Style::default()
            } else {
                dim()
            };
            ListItem::new(Line::from(vec![
                Span::styled(num, base),
                Span::styled(pad_to(&truncate(&e.label, 14), 14), base),
                Span::styled(pad_to(&truncate(&e.note, 26), 26), base.patch(dim())),
                Span::styled(reason, base.patch(dim())),
            ]))
        })
        .collect();

    // warning 这里直接原样显示，不做字符串加工——分类翻译成人话是
    // secrets.rs（load_error）/ profile.rs（load_dir）的责任，
    // 到这里的时候应该已经是完整的中文句子。唯一保留的例外是
    // profile.rs::describe_toml_error 里「expected ...」那半句可能
    // 是英文：那是用户自己写的 profile TOML 解析报错，行号已经是
    // 中文「第 N 行」，用户本来就在手改 TOML 文件，英文的语法期望
    // 提示比吞掉更有用（详见该函数的注释）。
    let title = match warning {
        Some(w) => format!("{} —— {w}", text(Key::PickAgentTitle, app.lang)),
        None => text(Key::PickAgentTitle, app.lang).to_string(),
    };
    let border = if warning.is_some() {
        Style::default().fg(Color::Red)
    } else {
        border_style
    };
    let mut s = state.clone();
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border)
                    .title(title),
            )
            .highlight_symbol("▶ "),
        area,
        &mut s,
    );
}

fn draw_pick_project(f: &mut Frame, area: Rect, app: &mut App) {
    let View::PickProject(p) = &app.view else {
        return;
    };
    let lang = app.lang;
    let border_style = if app.connected {
        Style::default()
    } else {
        Style::default().fg(Color::Red)
    };

    // 手输态占满整层，不分栏：这时候屏幕上只有一件事在发生。
    if let Some(buf) = &p.typing_path {
        f.render_widget(
            Paragraph::new(format!("{buf}▌")).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(text(Key::TypePathTitle, lang)),
            ),
            area,
        );
        return;
    }

    // 左边窄一点：最近项目只有名字和路径，而右边要放得下目录名加 git 标记。
    let cols = Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(area)
        .to_vec();

    // ——左：最近——
    let shown = p.shown_recent();
    let mut items: Vec<ListItem> = shown
        .iter()
        .map(|path| {
            let short = short_path(path);
            let name = std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| short.clone());
            ListItem::new(Line::from(vec![
                Span::raw(pad_to(&truncate(&name, 18), 18)),
                Span::styled(truncate(&short, 22), dim()),
            ]))
        })
        .collect();
    // 兜底入口不参与过滤，永远在最后一行
    items.push(ListItem::new(Line::from(Span::styled(
        text(Key::ManualPath, lang),
        Style::default().fg(Color::Cyan),
    ))));
    f.render_stateful_widget(
        List::new(items)
            .block(pane_block(
                text(Key::RecentProjects, lang).to_string(),
                p.focus == Pane::Recent,
                border_style,
            ))
            .highlight_symbol("▶ "),
        cols[0],
        &mut p.recent_state.clone(),
    );

    // ——右：浏览——
    let rows = p.shown_entries();
    let browse_block = pane_block(
        short_path(&p.cwd.display().to_string()),
        p.focus == Pane::Browse,
        border_style,
    );
    if rows.is_empty() {
        // 空目录和读不了的目录落在同一句话上：对用户来说这两种情况
        // 能做的事完全一样（← 回上一级，或者去别处找）。
        f.render_widget(
            Paragraph::new(text(Key::NoSubfolders, lang))
                .centered()
                .block(browse_block),
            cols[1],
        );
    } else {
        let items: Vec<ListItem> = rows
            .iter()
            .map(|r| {
                let mark = if r.is_git { " ●" } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::raw(truncate(&r.name, 30)),
                    // git 仓库的标记压暗：它是辅助信息，不该比目录名还抢眼
                    Span::styled(mark, dim()),
                ]))
            })
            .collect();
        f.render_stateful_widget(
            List::new(items).block(browse_block).highlight_symbol("▶ "),
            cols[1],
            &mut p.browse_state.clone(),
        );
    }
}

/// 有焦点那一栏的边框加亮。两栏并排时，用户必须一眼看出打字会落在哪一边——
/// 看不出来的话，他打的字会在他以为的另一栏里过滤，而那一栏毫无反应。
fn pane_block(title: String, focused: bool, base: Style) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(if focused {
            Style::default().fg(Color::Cyan)
        } else {
            base.patch(dim())
        })
        .title(title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::ListState;

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

    /// 覆盖选 agent 弹窗四种状态（`Ready`/`NeedsSecret`/`NeedsDependency`/
    /// `NotInstalled`）加一条 `warning` 同屏渲染的情况——这份 fixture 原来
    /// 挂在 `mod.rs` 的 `draw_does_not_panic_for_all_views` 上，那条测试在
    /// Task 5 把 `DrawInput` 换成 `App` 时被换成了空 `entries`，equivalent
    /// coverage 悄悄消失了：置灰整行的逻辑、三条原因文案
    /// （`（未填密钥）`/`（需要先装 X）`/`（未安装）`）、`pad_to`/`truncate`
    /// 对 label/note 的处理、以及 `warning` 触发的红色边框，全都没人再画
    /// 一遍。搬回来，并且不止断言不 panic——三条原因文案和 warning 文案
    /// 都要求真的出现在屏幕上，边框颜色也要求是红的。
    #[test]
    fn draw_renders_all_profile_statuses_and_the_warning_border() {
        use crate::profile::ProfileStatus;
        use crate::proto::{InstallPrompt, ProfileEntry};

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        let mut pick_state = ListState::default();
        pick_state.select(Some(0));
        let profile_entries = vec![
            ProfileEntry {
                name: "claude".into(),
                label: "Claude Code".into(),
                note: "官方 CLI".into(),
                status: ProfileStatus::Ready,
                secret: None,
                install: None,
                has_secret: false,
            },
            ProfileEntry {
                name: "kimi".into(),
                label: "Kimi".into(),
                note: "月之暗面".into(),
                status: ProfileStatus::NeedsSecret,
                secret: None,
                install: None,
                has_secret: false,
            },
            ProfileEntry {
                name: "glm".into(),
                label: "GLM".into(),
                note: "智谱".into(),
                status: ProfileStatus::NeedsDependency {
                    label: "Claude".into(),
                },
                secret: None,
                install: None,
                has_secret: false,
            },
            ProfileEntry {
                name: "codex".into(),
                label: "Codex".into(),
                note: "OpenAI".into(),
                status: ProfileStatus::NotInstalled {
                    command: "codex".into(),
                },
                secret: None,
                install: Some(InstallPrompt {
                    command: vec![
                        "npm".into(),
                        "i".into(),
                        "-g".into(),
                        "@openai/codex".into(),
                    ],
                    note: String::new(),
                }),
                has_secret: false,
            },
        ];
        app.view = View::PickProfile {
            entries: profile_entries,
            state: pick_state,
            warning: Some("secrets.toml 读不了".into()),
        };
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains("未填密钥"),
            "NeedsSecret 的原因文案要画出来：{content}"
        );
        assert!(
            content.contains("需要先装Claude"),
            "NeedsDependency 的原因文案要点名依赖：{content}"
        );
        assert!(
            content.contains("未安装"),
            "NotInstalled 的原因文案要画出来：{content}"
        );
        assert!(
            content.contains("secrets.toml读不了"),
            "warning 要显示在标题里：{content}"
        );

        let buf = term.backend().buffer();
        let area = buf.area;
        let red = (0..area.height).any(|y| {
            (0..area.width).any(|x| {
                buf.cell((x, y))
                    .map(|c| c.style().fg == Some(Color::Red) && c.symbol() != " ")
                    .unwrap_or(false)
            })
        });
        assert!(red, "有 warning 时边框要是红的");
    }

    /// 换完项目就得看到新项目的会话。等下一轮 `need_sessions` 才重算的话，
    /// 中间那一帧画的是上一个项目的会话、底栏却已经写着新项目——正是这一版
    /// 让用户觉得「同一个 session 变成了不同的项目」的那一幕。
    #[test]
    fn confirming_a_project_recomputes_the_visible_sessions_at_once() {
        use crate::session::{SessionInfo, SessionState};
        let (mut app, dir) = App::test_app();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();

        let mk = |id: u32, d: &std::path::Path| SessionInfo {
            id,
            profile: "claude".into(),
            dir: d.display().to_string(),
            state: SessionState::Idle,
            activity: String::new(),
        };
        app.sessions = vec![mk(1, &a), mk(2, &b)];
        app.current_dir = a.clone();
        app.refresh_visible();
        assert_eq!(app.visible.len(), 1, "前提：a 项目下只有会话 1");

        let mut st = ListState::default();
        st.select(Some(0));
        app.view = View::PickProject(ProjectPicker {
            filter: String::new(),
            typing_path: None,
            ..ProjectPicker::new(
                vec![b.display().to_string()],
                std::path::PathBuf::from("/tmp"),
            )
        });
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();

        assert_eq!(app.current_dir, b, "项目切过去了");
        assert_eq!(
            app.visible.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![2],
            "屏幕上的会话必须同一时刻跟着换，不能等下一轮"
        );
    }

    /// 造一棵能走的目录树。
    ///
    /// **两个 `TempDir` 都要交出去**：一个是这棵树本身，另一个是 `test_app`
    /// 给 socket 用的（切模式、存设置都会往它旁边写）。任何一个提前 drop，
    /// 对应的目录就在测试跑到一半时从磁盘上消失了——`list_dirs` 会读到空、
    /// `switch_project` 的 `is_dir()` 会变成 false，而失败信息完全不会
    /// 指向真正的原因。
    fn tree(
        dirs: &[&str],
    ) -> (
        tempfile::TempDir,
        tempfile::TempDir,
        App,
        std::path::PathBuf,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        for d in dirs {
            std::fs::create_dir_all(tmp.path().join(d)).unwrap();
        }
        let root = tmp.path().to_path_buf();
        let (mut app, guard) = App::test_app();
        app.current_dir = root.clone();
        (tmp, guard, app, root)
    }

    fn open_picker(app: &mut App, recent: Vec<String>, cwd: std::path::PathBuf) {
        app.view = View::PickProject(ProjectPicker::new(recent, cwd));
    }

    fn picker(app: &App) -> ProjectPicker {
        match &app.view {
            View::PickProject(p) => p.clone(),
            other => panic!("不在选项目里：{}", matches!(other, View::Board)),
        }
    }

    fn press(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
    }

    /// Tab 在两栏之间来回切。没有这一步，右边那栏就是个只能看不能用的装饰。
    #[test]
    fn tab_moves_the_focus_back_and_forth() {
        let (_t, _g, mut app, root) = tree(&["a"]);
        open_picker(&mut app, vec![], root);
        assert_eq!(picker(&app).focus, Pane::Recent, "开在最近那一栏");
        press(&mut app, KeyCode::Tab);
        assert_eq!(picker(&app).focus, Pane::Browse);
        press(&mut app, KeyCode::Tab);
        assert_eq!(picker(&app).focus, Pane::Recent);
    }

    /// `→` 进子目录，`←` 回上一级。这是「目录浏览器」这四个字的全部含义。
    #[test]
    fn right_descends_and_left_goes_up() {
        let (_t, _g, mut app, root) = tree(&["outer/inner"]);
        open_picker(&mut app, vec![], root.clone());
        press(&mut app, KeyCode::Tab);

        press(&mut app, KeyCode::Right);
        assert_eq!(picker(&app).cwd, root.join("outer"), "→ 要走进去");
        assert_eq!(
            picker(&app)
                .entries
                .iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>(),
            vec!["inner"],
            "列表要跟着换成新目录的内容"
        );

        press(&mut app, KeyCode::Left);
        assert_eq!(picker(&app).cwd, root, "← 要回上一级");
    }

    /// 一路 `←` 走到根目录不能 panic，也不能卡住不动就静默——原地停住即可。
    #[test]
    fn going_up_from_the_root_stays_put() {
        let (_t, _g, mut app, _root) = tree(&[]);
        open_picker(&mut app, vec![], std::path::PathBuf::from("/"));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Left);
        assert_eq!(picker(&app).cwd, std::path::PathBuf::from("/"));
    }

    /// **浏览栏的 Enter 是「选定」，不是「进入」。** 用户走到一个目录上，
    /// 多半是因为它就是他要的项目；想再往下钻有 `→`。
    #[test]
    fn enter_in_the_browser_picks_the_highlighted_folder() {
        let (_t, _g, mut app, root) = tree(&["proj/sub"]);
        open_picker(&mut app, vec![], root.clone());
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.current_dir, root.join("proj"), "选定的是高亮那个目录");
        assert!(matches!(app.view, View::Board), "选完就回家");
        assert!(app.message.text.contains("已切到"), "换了项目要说一声");
    }

    /// 最近栏的 `→` 把浏览器切到那个项目所在的位置——用户想从一个熟悉的
    /// 项目附近开始找，这是最短的一步。
    #[test]
    fn right_on_a_recent_project_opens_the_browser_there() {
        let (_t, _g, mut app, root) = tree(&["proj/sub"]);
        let proj = root.join("proj");
        open_picker(&mut app, vec![proj.display().to_string()], root);
        press(&mut app, KeyCode::Right);
        assert_eq!(picker(&app).cwd, proj);
        assert_eq!(picker(&app).focus, Pane::Browse, "焦点跟着过去");
    }

    /// 打字只过滤**当前焦点那一栏**。共用一个过滤词的话，用户在左边找项目，
    /// 右边的目录列表会跟着变空，而他并没有要求那件事。
    #[test]
    fn typing_filters_only_the_focused_pane() {
        let (_t, _g, mut app, root) = tree(&["alpha", "beta"]);
        open_picker(&mut app, vec!["/w/alpha".into(), "/w/beta".into()], root);

        press(&mut app, KeyCode::Char('a'));
        let p = picker(&app);
        assert_eq!(p.shown_recent().len(), 2, "「a」在两条路径里都有");
        assert_eq!(p.shown_entries().len(), 2, "浏览栏不该被左栏的过滤词影响");

        press(&mut app, KeyCode::Tab); // 切到浏览栏，过滤词清空
        press(&mut app, KeyCode::Char('b'));
        let p = picker(&app);
        assert_eq!(
            p.shown_entries()
                .iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>(),
            vec!["beta"],
            "现在过滤的是浏览栏"
        );
        assert_eq!(p.shown_recent().len(), 2, "左栏不受影响");
    }

    /// Esc 回到用户选的模式，不是永远的列表（复用 B 的 home_view）。
    #[test]
    fn escape_returns_to_the_chosen_mode() {
        let (_t, _g, mut app, root) = tree(&["a"]);
        app.view_mode = crate::ui::ViewMode::Grid;
        open_picker(&mut app, vec![], root);
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.view, View::Grid { .. }));
    }

    #[test]
    fn draw_does_not_panic_for_project_picker() {
        let mut st = ListState::default();
        st.select(Some(0));
        let all = vec![
            "/Users/lei/work/dc/dc-terminal".to_string(),
            "/Users/lei/work/dc/dc_workbench".to_string(),
        ];
        let (mut app, _dir) = App::test_app();

        // 列表态。每一段都新建一个 Terminal：ratatui 画中文这种宽字符时只写
        // 首格、第二格保留旧值，同一个 TestBackend 连画两帧再断言，上一帧的
        // 残字会拼进来，产生假阳性/假阴性（见 mod.rs 的
        // bottom_bar_help_follows_the_view 的注释）。这里每段内容长度、宽
        // 字符落点都不同，实测确实会踩上，所以都换新的 TestBackend，跟既有
        // 测试的写法保持一致。
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        app.view = View::PickProject(ProjectPicker {
            filter: String::new(),
            typing_path: None,
            ..ProjectPicker::new(all.clone(), std::path::PathBuf::from("/tmp"))
        });
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(content.contains("dc-terminal"), "列表要显示项目：{content}");
        assert!(
            content.contains("手输路径"),
            "末行兜底入口必须在：{content}"
        );

        // 过滤到无匹配：只剩兜底那一行，不能 panic
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        app.view = View::PickProject(ProjectPicker {
            filter: "没有这个".to_string(),
            typing_path: None,
            ..ProjectPicker::new(all.clone(), std::path::PathBuf::from("/tmp"))
        });
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains("手输路径"),
            "无匹配时兜底入口仍要在：{content}"
        );

        // 手输态
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        app.view = View::PickProject(ProjectPicker {
            filter: String::new(),
            typing_path: Some("~/work/x".to_string()),
            ..ProjectPicker::new(all.clone(), std::path::PathBuf::from("/tmp"))
        });
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains("~/work/x"),
            "手输态要回显已输入的路径：{content}"
        );

        // 空列表（全新守护进程）也不能 panic
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        app.view = View::PickProject(ProjectPicker {
            filter: String::new(),
            typing_path: None,
            ..ProjectPicker::new(Vec::new(), std::path::PathBuf::from("/tmp"))
        });
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
    }
}
