use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use crate::profile::ProfileStatus;
use crate::proto::{Request, Response, SecretPrompt};

use super::app::App;
use super::view::{
    digit_index, expand_path, filter_projects, pick_action, PickAction, SecretPhase, View,
};
use super::widgets::{pad_to, short_path, truncate, Msg};
use super::{move_sel_n, DIM};

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match app.view.clone() {
        View::PickProfile { .. } => handle_pick_profile(app, key),
        View::PickProject { .. } => handle_pick_project(app, key),
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
        app.view = View::Board;
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
        app.view = match chosen.map(|i| (i, pick_action(&entries[i]))) {
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
                    Ok(Response::Error(e)) => {
                        app.message = Msg::err(e);
                        View::PickProfile {
                            entries,
                            state,
                            warning,
                        }
                    }
                    _ => {
                        app.message = Msg::err("创建失败".into());
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
                        app.message =
                            format!("正在安装 {profile}，装完按 Ctrl+Q 回看板再按 N").into();
                        app.need_sessions = true;
                        View::Attached(id)
                    }
                    _ => {
                        app.message = Msg::err("开不了安装窗口".into());
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

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
fn handle_pick_project(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::PickProject {
        all,
        mut filter,
        mut state,
        typing_path,
    } = app.view.clone()
    else {
        return Ok(());
    };
    match typing_path {
        // ——手输路径态：可见字符全进输入框，不再当过滤用——
        Some(mut buf) => match key.code {
            KeyCode::Esc => {
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: None,
                }
            }
            KeyCode::Enter => {
                if buf.trim().is_empty() {
                    // expand_path("", base) 会解析成 base 自己（非绝对路径走
                    // base.join("")），is_dir() 照样为真——空输入不挡住的话，
                    // 用户在这一步犹豫多按一次 Enter，就会被无声切回启动目录。
                    app.message = Msg::err("还没输入路径".into());
                    app.view = View::PickProject {
                        all,
                        filter,
                        state,
                        typing_path: Some(buf),
                    };
                } else {
                    let p = expand_path(&buf, &app.start_dir);
                    if p.is_dir() {
                        // 「当前项目」已经在底部边框标题里，这里说的是刚发生的动作
                        app.message =
                            format!("已切到 {}", short_path(&p.display().to_string())).into();
                        app.current_dir = p;
                        app.view = View::Board;
                    } else {
                        // 不是 git 仓库这件事不在这里判——留给 create()
                        app.message = Msg::err(format!("{} 不是一个目录", p.display()));
                        app.view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: Some(buf),
                        };
                    }
                }
            }
            KeyCode::Backspace => {
                buf.pop();
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: Some(buf),
                };
            }
            KeyCode::Char(c) => {
                buf.push(c);
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: Some(buf),
                };
            }
            _ => {
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: Some(buf),
                }
            }
        },
        // ——列表态——
        None => match key.code {
            KeyCode::Esc => app.view = View::Board,
            KeyCode::Down | KeyCode::Up => {
                let delta = if key.code == KeyCode::Down { 1 } else { -1 };
                // +1 是末行那个「手输路径…」，它不参与过滤，永远在
                let n = filter_projects(&all, &filter).len() + 1;
                move_sel_n(&mut state, n, delta);
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: None,
                };
            }
            KeyCode::Enter => {
                let shown = filter_projects(&all, &filter);
                let i = state.selected().unwrap_or(0);
                if i >= shown.len() {
                    // 选中的是末行「手输路径…」
                    app.view = View::PickProject {
                        all,
                        filter,
                        state,
                        typing_path: Some(String::new()),
                    };
                } else {
                    let p = PathBuf::from(&shown[i]);
                    if p.is_dir() {
                        app.message = format!("已切到 {}", short_path(&shown[i])).into();
                        app.current_dir = p;
                        app.view = View::Board;
                    } else {
                        // 列表里那条不删——可能只是外置盘没挂
                        app.message = Msg::err(format!("{} 现在找不到了", short_path(&shown[i])));
                        app.view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: None,
                        };
                    }
                }
            }
            KeyCode::Backspace => {
                filter.pop();
                state.select(Some(0));
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: None,
                };
            }
            KeyCode::Char(c) => {
                filter.push(c);
                // 过滤变了就回到第一项，否则光标可能停在已被过滤掉的行号上
                state.select(Some(0));
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: None,
                };
            }
            _ => {
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: None,
                }
            }
        },
    }
    Ok(())
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    match &app.view {
        View::PickProfile { .. } => draw_pick_profile(f, area, app),
        View::PickProject { .. } => draw_pick_project(f, area, app),
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
                ProfileStatus::NeedsSecret => "（未填密钥）".into(),
                ProfileStatus::NeedsDependency { label } => {
                    format!("（需要先装 {label}）")
                }
                ProfileStatus::NotInstalled { .. } => "（未安装）".into(),
            };
            // 不可用的整行压暗，不只是把原因压暗——用户是先看名字再看原因的，
            // 名字亮着会让他先以为能用
            let base = if matches!(e.status, ProfileStatus::Ready) {
                Style::default()
            } else {
                Style::default().fg(DIM)
            };
            ListItem::new(Line::from(vec![
                Span::styled(num, base),
                Span::styled(pad_to(&truncate(&e.label, 14), 14), base),
                Span::styled(pad_to(&truncate(&e.note, 26), 26), base.fg(DIM)),
                Span::styled(reason, base.fg(DIM)),
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
        Some(w) => format!("选 agent —— {w}"),
        None => "选 agent".to_string(),
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
    let View::PickProject {
        all,
        filter,
        state,
        typing_path,
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
    if let Some(buf) = typing_path {
        f.render_widget(
            Paragraph::new(format!("{buf}▌")).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title("输入项目路径（Enter 确认，Esc 返回列表）"),
            ),
            area,
        );
    } else {
        let shown = filter_projects(all, filter);
        let mut items: Vec<ListItem> = shown
            .iter()
            .map(|p| {
                let short = short_path(p);
                let name = std::path::Path::new(p)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| short.clone());
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{:<20}", truncate(&name, 20))),
                    Span::styled(truncate(&short, 50), Style::default().fg(DIM)),
                ]))
            })
            .collect();
        // 兜底入口不参与过滤，永远在最后一行
        items.push(ListItem::new(Line::from(Span::styled(
            "手输路径…",
            Style::default().fg(Color::Cyan),
        ))));

        let title = if filter.is_empty() {
            "选项目（↑↓ 选，Enter 确认，直接打字过滤，Esc 取消）".to_string()
        } else {
            format!("选项目（过滤：{filter}）")
        };
        // state 是 View 里那份的副本，draw 只读不写，所以这里克隆一份给
        // render_stateful_widget 用，不去动看板的光标。
        let mut s = state.clone();
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
            &mut s,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        app.view = View::PickProject {
            all: all.clone(),
            filter: String::new(),
            state: st.clone(),
            typing_path: None,
        };
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
        app.view = View::PickProject {
            all: all.clone(),
            filter: "没有这个".to_string(),
            state: st.clone(),
            typing_path: None,
        };
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
        app.view = View::PickProject {
            all: all.clone(),
            filter: String::new(),
            state: st.clone(),
            typing_path: Some("~/work/x".to_string()),
        };
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
        app.view = View::PickProject {
            all: Vec::new(),
            filter: String::new(),
            state: ListState::default(),
            typing_path: None,
        };
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
    }
}
