use std::path::{Path, PathBuf};

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
        no_git,
    } = app.view.clone()
    else {
        return Ok(());
    };
    // 五个重建点（下面每一条不动手的分支都要把这一屏原样搭回来）都得带上
    // `no_git`，所以先收成一个闭包，省得抄五遍还漏一份。
    let same = |entries, state, no_git| View::PickProfile {
        entries,
        state,
        warning: warning.clone(),
        no_git,
    };
    if key.code == KeyCode::Esc {
        app.view = super::home_view(app);
    } else if key.code == KeyCode::Char('g') && no_git {
        // `g`：就地建一个 git 仓库。
        //
        // **只在 `no_git` 为真时才认这个键**——它是屏幕上写着的那句
        // 「按 g 初始化」的另一半，那句话不写的时候这个键也不该有反应
        // （这个项目的规矩是屏幕和键盘必须对得上，两个方向都算）。
        //
        // `is_repo` 走 `rev-parse --is-inside-work-tree`，父目录是仓库时
        // 它也为真——所以走到这里意味着**往上一级也没有仓库**，`git init`
        // 不可能建出一个嵌套在别人工作区里的仓库来。
        let dir = app.current_dir();
        match crate::git::init(&dir) {
            Ok(()) => {
                app.message = text(Key::GitRepoCreated, app.lang).into();
                // 提示和红边框跟着消失：这一屏九项 agent 现在真的能用了。
                app.view = same(entries, state, false);
            }
            Err(e) => {
                app.message = Msg::err(msg::git_init_failed(app.lang, &e.to_string()));
                app.view = same(entries, state, no_git);
            }
        }
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
            None => same(entries, state, no_git),
            Some((_, PickAction::Start(name))) => {
                // 选完直接进会话。用户选中的意图就是「我要用这个
                // agent 干活」，先弹回看板再让他找一遍自己刚建的
                // 会话是白让人做第二次选择。建失败才回选择器。
                let dir = app.current_dir().display().to_string();
                // 选择器里选的就是用户真的要用的 agent——与「帮你装 CLI」
                // 那条 remember=false 的路径区分开。走 `create_session`
                // 而不是自己发请求：底栏那句 `n 新建 <agent>` 的缓存由它
                // 统一跟上（见它的文档）。
                match super::create_session(app, &dir, &name, true) {
                    Ok(Response::Created { id }) => {
                        app.need_sessions = true; // 会话标题要显示项目名
                        View::Attached(id)
                    }
                    Ok(Response::Error(ref e)) => {
                        app.message = Msg::err(crate::i18n::msg::error(app.lang, e));
                        same(entries, state, no_git)
                    }
                    _ => {
                        app.message = Msg::err(text(Key::CreateFailed, app.lang).into());
                        same(entries, state, no_git)
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
                let dir = app.current_dir().display().to_string();
                match super::create_session(app, &dir, "shell", false) {
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
                        same(entries, state, no_git)
                    }
                }
            }
            Some((_, PickAction::Blocked(msg))) => {
                app.message = Msg::err(msg);
                same(entries, state, no_git)
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

    // ——新建项目态：收的是一个**目录名**，不是路径——
    //
    // 排在手输路径态前面纯粹是因为两者互斥（同时只可能有一个是 Some），
    // 顺序不影响结果；分开写是因为两者收的东西和校验规则都不一样，
    // 见 `ProjectPicker::naming` 的文档注释。
    if let Some(mut buf) = p.naming.clone() {
        match key.code {
            KeyCode::Esc => p.naming = None,
            KeyCode::Enter => match new_project_path(&p.cwd, &buf) {
                Ok(dir) => {
                    // 先建目录再 `git init`：建不出来就没有仓库可初始化，
                    // 反过来做会在失败时留下一个半成品。
                    match std::fs::create_dir(&dir) {
                        Ok(()) => {
                            // **`git init` 失败不挡路。** 目录已经建好了，
                            // 项目本身成立；没有仓库这件事在下一屏有整块
                            // 红边框和 `g` 那条出路在说（见 `handle_pick_profile`），
                            // 在这里把用户按回输入框只会让他重打一遍名字，
                            // 而重打解决不了「机器上没装 git」。
                            if let Err(e) = crate::git::init(&dir) {
                                app.message =
                                    Msg::err(msg::git_init_failed(app.lang, &e.to_string()));
                            }
                            super::pin_project(app, dir);
                            return Ok(());
                        }
                        Err(e) => {
                            app.message =
                                Msg::err(msg::new_project_failed(app.lang, &e.to_string()));
                        }
                    }
                }
                // 三种毛病三句话，而且**都留在输入态里**：改个名字就能继续，
                // 把人踢回列表等于让他刚打的名字白打。
                Err(NameProblem::Empty) => {
                    app.message = Msg::err(text(Key::NewProjectNoName, app.lang).into());
                }
                Err(NameProblem::Separator) => {
                    app.message = Msg::err(text(Key::NewProjectBadName, app.lang).into());
                }
                Err(NameProblem::Exists) => {
                    app.message = Msg::err(msg::new_project_exists(app.lang, buf.trim()));
                }
            },
            KeyCode::Backspace => {
                buf.pop();
                p.naming = Some(buf);
            }
            KeyCode::Char(c) => {
                buf.push(c);
                p.naming = Some(buf);
            }
            _ => {}
        }
        app.view = View::PickProject(p);
        return Ok(());
    }

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
                        super::pin_project(app, dir);
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
                // +2 是末尾那两行「手输路径…」「新建项目…」，它们不参与
                // 过滤，永远在。
                Pane::Recent => {
                    let n = p.shown_recent().len() + 2;
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
                        // 过了最后一条数据之后是两个动作行：先「手输路径…」，
                        // 再「新建项目…」。顺序跟画出来的一致，靠的是同一个
                        // `shown_recent().len()` 做基准。
                        None => {
                            if i == shown.len() {
                                p.typing_path = Some(String::new());
                            } else {
                                p.naming = Some(String::new());
                            }
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
                    super::pin_project(app, dir);
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
/// 新建项目时，名字有什么毛病。**分开三种**：三句话不一样，而一句
/// 「名字不合法」等于让用户自己猜是哪儿不对。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NameProblem {
    /// 什么都没打
    Empty,
    /// 名字里带路径分隔符或者 `..`
    Separator,
    /// 这个名字已经有了
    Exists,
}

/// 把「在 `parent` 里新建一个叫 `name` 的项目」算成一个路径。
pub(crate) fn new_project_path(parent: &Path, name: &str) -> Result<PathBuf, NameProblem> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NameProblem::Empty);
    }
    // **分隔符和 `..` 一律挡住。** 用户看着的是「在这个目录里新建」，
    // 而 `../x` 会把目录建到别处去——屏幕和实际发生的事对不上，是这个
    // 项目里最不能接受的一类 bug。要建到别处，那是「手输路径」那条路。
    // Windows 的 `\` 也算：那台机器上它就是分隔符。
    if name.contains('/') || name.contains('\\') || name.split('.').all(|p| p.is_empty()) {
        return Err(NameProblem::Separator);
    }
    let dir = parent.join(name);
    // `exists` 而不是 `is_dir`：同名的**文件**也占着这个名字，
    // `create_dir` 到那儿一样会失败，而那时候的报错没法给用户下一步。
    if dir.exists() {
        return Err(NameProblem::Exists);
    }
    Ok(dir)
}

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
        no_git,
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
    // 标题里带上当前项目。这一屏（`n`/`N` 是浮层，`p` 之后是整屏换掉）铺满了
    // 九个 agent 的名字和说明，而按下 Enter 的后果是「在某个目录里开一个会话」
    // ——「哪个目录」原本全屏只有底栏中段那一小块字写着。`p` 选完项目直接弹到
    // 这儿（见 `ui::pin_project`）之后更要紧：用户刚做完「去哪个项目」这个决定，
    // 屏幕整个换了一屏，标题上认一眼比回头去底栏找便宜得多。
    //
    // 只写组名（最后一段目录名），不写路径：标题是一行，路径动辄几十列，
    // 挤掉的正是后面那半句 warning——而 warning 是「为什么这一项用不了」的
    // 唯一出处。同名项目分不清这件事由底栏中段兜着（`project_label` 会往前
    // 贴父目录）。
    let here = app.current_group().map(|g| truncate(&g.name, 20));
    let base = match &here {
        Some(p) => format!("{} · {p}", text(Key::PickAgentTitle, app.lang)),
        None => text(Key::PickAgentTitle, app.lang).to_string(),
    };
    // 两句提示可以同时有，接在同一行上：`no_git` 说的是「列表没问题，但在
    // 这个目录里都用不了」，`warning` 说的是「列表本身有问题」。哪句在前
    // 不是随手排的——`no_git` 后面跟着一个**能按的键**（`g`），而 warning
    // 是一句读完就完的说明；标题放不下的时候被切掉的是后半句，切掉说明
    // 比切掉出路好。
    let title = [
        Some(base),
        no_git.then(|| text(Key::NotAGitRepoHint, app.lang).to_string()),
        warning.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" —— ");
    let border = if warning.is_some() || *no_git {
        Style::default().fg(Color::Red)
    } else {
        border_style
    };
    let mut s = state.clone();
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::TOP | Borders::BOTTOM)
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

    // 新建态跟手输态一样占满整层：这时候屏幕上只有一件事在发生。
    // 标题里带上目录——见 `msg::new_project_in` 的注释。
    if let Some(buf) = &p.naming {
        f.render_widget(
            Paragraph::new(format!("{buf}▌")).block(
                Block::default()
                    .borders(Borders::TOP | Borders::BOTTOM)
                    .border_style(border_style)
                    .title(msg::new_project_in(
                        lang,
                        &short_path(&p.cwd.display().to_string()),
                    )),
            ),
            area,
        );
        return;
    }

    // 手输态占满整层，不分栏：这时候屏幕上只有一件事在发生。
    if let Some(buf) = &p.typing_path {
        f.render_widget(
            Paragraph::new(format!("{buf}▌")).block(
                Block::default()
                    .borders(Borders::TOP | Borders::BOTTOM)
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
    // 两个动作行不参与过滤，永远在最后：先「手输路径…」，再「新建项目…」。
    // 顺序跟 `handle_pick_project` 里那个 `i == shown.len()` 的判断绑在一起，
    // 改这里就要改那里。
    items.push(ListItem::new(Line::from(Span::styled(
        text(Key::ManualPath, lang),
        Style::default().fg(Color::Cyan),
    ))));
    items.push(ListItem::new(Line::from(Span::styled(
        text(Key::NewProject, lang),
        Style::default().fg(Color::Cyan),
    ))));
    f.render_stateful_widget(
        List::new(items)
            .block(pane_block(
                text(Key::RecentProjects, lang).to_string(),
                p.focus == Pane::Recent,
                border_style,
            ))
            // **只有有焦点的那一栏画光标。** 两栏都画的话，屏幕上同时
            // 有两个长得一模一样的 `▶`，而按 Tab 之后唯一变的东西是边框
            // 颜色——那一档差别在不少主题下几乎看不出来。用户于是既看不出
            // 焦点在哪，也看不出 Tab 有没有生效，方向键动的那一栏跟他盯着
            // 的那个光标对不上。空字符串留两格，是为了让列表本身的缩进
            // 在两种状态下一样宽，不然切焦点时整栏文字会横跳一下。
            .highlight_symbol(if p.focus == Pane::Recent {
                "▶ "
            } else {
                "  "
            }),
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
                let mut spans = vec![
                    Span::raw(truncate(&r.name, 30)),
                    // git 仓库的标记压暗：它是辅助信息，不该比目录名还抢眼
                    Span::styled(mark, dim()),
                ];
                // POSIX 目录名里只有 `/` 和 NUL 不合法——转义序列这类看不见
                // 的字节完全合法，而 `truncate` 已经把它们从上面那个 span
                // 的显示里滤掉了。不在这里补一句，这种名字选中前跟一个正常
                // 目录长得一模一样，用户没有任何办法在选之前发现不对劲。
                // **打开目录仍然用 `row.name` 原始值**（见 `handle_pick_project`
                // 里的 `p.cwd.join(&row.name)`）——这一句只负责让异常在选择
                // 的那一刻被看见，绝不能反过来去动打开逻辑。
                if r.name.chars().any(|c| c.is_control()) {
                    spans.push(Span::styled(
                        format!(" {}", text(Key::HiddenCharsInName, lang)),
                        dim(),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        f.render_stateful_widget(
            List::new(items)
                .block(browse_block)
                // 同左栏：焦点不在这儿就不画光标，见那边的注释。
                .highlight_symbol(if p.focus == Pane::Browse {
                    "▶ "
                } else {
                    "  "
                }),
            cols[1],
            &mut p.browse_state.clone(),
        );
    }
}

/// 有焦点那一栏的边框加亮。两栏并排时，用户必须一眼看出打字会落在哪一边——
/// 看不出来的话，他打的字会在他以为的另一栏里过滤，而那一栏毫无反应。
fn pane_block(title: String, focused: bool, base: Style) -> Block<'static> {
    Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
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
            no_git: false,
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

    /// 换完项目，光标就得落在那个项目的组上——「当前项目 = 光标在哪个组」，
    /// 所以不挪光标等于什么都没发生：底栏还写着旧项目，`n` 也还是开在旧项目里。
    /// 等下一轮 `need_sessions` 才重算的话，中间那一帧屏幕上就是上一个项目。
    #[test]
    fn confirming_a_project_moves_the_cursor_into_its_group_at_once() {
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
            is_agent: true,
            tag: String::new(),
        };
        app.set_sessions(vec![mk(1, &a), mk(2, &b)]);
        app.list_state.select(Some(0));
        assert!(app.current_dir().ends_with("a"), "前提：光标在 a 上");

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

        assert!(app.current_dir().ends_with("b"), "项目切过去了");
        assert_eq!(
            app.selected_session().map(|s| s.id),
            None,
            "光标停在 b 的组头上，不是随便某个会话上"
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
        let (app, guard) = App::test_app();
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

    /// 新建输入态整层只画一件事，而且标题必须**把目录说出来**——「叫什么
    /// 名字」少了「建在哪儿」，用户没法确认自己是不是先把浏览栏挪对了。
    #[test]
    fn the_naming_prompt_names_the_directory_it_will_build_in() {
        let (_t, _g, mut app, root) = tree(&["outer"]);
        open_picker(&mut app, vec![], root.join("outer"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        for c in "abc".chars() {
            press(&mut app, KeyCode::Char(c));
        }

        let mut term = ratatui::Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let screen = buffer_text(term.backend().buffer());

        assert!(
            screen.contains("outer"),
            "标题没说建在哪个目录里：\n{screen}"
        );
        assert!(screen.contains("abc"), "打进去的名字没上屏：\n{screen}");
    }

    /// 「新建项目…」那一行永远在左栏末尾，**过滤也删不掉它**——它是个
    /// 动作，不是一条数据。过滤词把它滤没了的话，用户越是找不到项目
    /// （正是最该新建的时候），这个入口越是不见。
    #[test]
    fn the_new_project_row_is_always_there() {
        let (_t, _g, mut app, root) = tree(&["a"]);
        open_picker(&mut app, vec![root.join("a").display().to_string()], root);
        // 打一个跟谁都不匹配的过滤词
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Char('z'));

        let mut term = ratatui::Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let screen = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();

        let label: String = text(Key::NewProject, app.lang)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            screen.contains(&label),
            "过滤之后新建入口不见了：\n{screen}"
        );
    }

    /// 选中那一行按 Enter，进的是**打名字**的输入态，不是直接建目录：
    /// 建之前得让用户看清楚建在哪儿、叫什么。
    #[test]
    fn enter_on_the_new_project_row_asks_for_a_name() {
        let (_t, _g, mut app, root) = tree(&["a"]);
        open_picker(&mut app, vec![root.join("a").display().to_string()], root);
        // 最近栏：0 = a，1 = 手输路径…，2 = 新建项目…
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);

        assert_eq!(picker(&app).naming, Some(String::new()), "没进新建输入态");
    }

    /// 打完名字回车：目录建在**浏览栏当前那个目录**里，顺手 `git init`，
    /// 然后直接进选 agent 那一屏——新建一个项目的全部意义就是马上开工。
    #[test]
    fn naming_creates_the_folder_and_goes_on_to_pick_an_agent() {
        let (_t, _g, mut app, root) = tree(&[]);
        open_picker(&mut app, vec![], root.clone());
        // 没有最近项目，所以左栏只有两行：0 = 手输路径…，1 = 新建项目…
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        for c in "my-thing".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);

        let made = root.join("my-thing");
        assert!(made.is_dir(), "目录没建出来");
        assert!(made.join(".git").exists(), "没有 git init");
        // 建完就当它是被选中的项目往下走。测试里没有守护进程，所以
        // `open_new_session` 拉不到 agent 列表、停在看板上——跟「在浏览栏
        // 选中一个已有目录」那条路的落点完全一样（见
        // `choosing_a_folder_pins_it`），这里钉的是**项目切过去了**。
        assert!(
            app.current_dir().ends_with("my-thing"),
            "新建完没把它当成当前项目：{}",
            app.current_dir().display()
        );
        assert!(matches!(app.view, View::Board), "建完没往下走");
    }

    /// 名字带 `..` 的时候：**不建，也不退出输入态**——用户改个名字就能继续，
    /// 而屏幕上那句话要说清楚为什么。
    #[test]
    fn a_name_that_escapes_keeps_you_in_the_prompt_with_a_reason() {
        // **浏览位置要挑在 tempdir 里面一层。** 直接停在 tempdir 根上的话，
        // 「跑到外面去了没有」这句断言问的是**系统临时目录**里有没有那个
        // 名字——那是所有测试、所有历史运行共用的一块地方，别的东西在那儿
        // 留下同名目录，这条测试就会毫无道理地红（这不是假想：写这条测试的
        // RED 阶段自己就在那儿留过一个）。停在里面一层，父目录也归 tempdir
        // 管，跑完自动清掉，断言问的才是这次运行真正发生的事。
        let (_t, _g, mut app, root) = tree(&["outer"]);
        let outer = root.join("outer");
        open_picker(&mut app, vec![], outer.clone());
        // 没有最近项目时，「新建项目…」是第二行
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        for c in "../escaped".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);

        assert!(picker(&app).naming.is_some(), "被踢出了输入态，名字白打了");
        assert!(app.message.error, "没给出错的话");
        assert!(!root.join("escaped").exists(), "目录建到浏览位置外面去了");
    }

    /// 空名字不建目录——`parent.join("")` 就是 `parent` 自己，不挡住的话
    /// 用户多按一次 Enter 就把**当前浏览的那个目录**当成新项目选走了。
    #[test]
    fn an_empty_name_is_refused() {
        let (_t, _g, _app, root) = tree(&[]);
        assert_eq!(new_project_path(&root, ""), Err(NameProblem::Empty));
        assert_eq!(new_project_path(&root, "   "), Err(NameProblem::Empty));
    }

    /// **名字里不许有分隔符或 `..`。** 这不是洁癖：`../../x` 会在用户以为
    /// 「在这个目录里新建」的时候，把目录建到浏览位置之外去——他看着的那
    /// 一屏和实际发生的事对不上。要建到别处有「手输路径」那条路。
    #[test]
    fn a_name_that_escapes_the_browsed_directory_is_refused() {
        let (_t, _g, _app, root) = tree(&[]);
        for bad in ["../x", "a/b", "..", "/etc"] {
            assert_eq!(
                new_project_path(&root, bad),
                Err(NameProblem::Separator),
                "{bad} 该被挡住"
            );
        }
    }

    /// 名字撞车不覆盖、不静默接受：已经有的目录该用「选」的，不是「新建」的。
    #[test]
    fn a_name_that_already_exists_is_refused() {
        let (_t, _g, _app, root) = tree(&["taken"]);
        assert_eq!(new_project_path(&root, "taken"), Err(NameProblem::Exists));
    }

    /// 好名字算出来的就是浏览目录底下那一个。
    #[test]
    fn a_good_name_lands_inside_the_browsed_directory() {
        let (_t, _g, _app, root) = tree(&[]);
        assert_eq!(
            new_project_path(&root, "my-thing"),
            Ok(root.join("my-thing"))
        );
        // 两头的空格是手滑，不是名字的一部分
        assert_eq!(
            new_project_path(&root, " my-thing "),
            Ok(root.join("my-thing"))
        );
    }

    /// **屏幕上只能有一个光标。**
    ///
    /// 两栏各画各的 `▶`、只靠边框颜色区分焦点，是这一屏最要命的一个洞：
    /// 用户按 Tab 之后屏幕上什么明显的东西都没变（边框那一档颜色差在很多
    /// 主题下几乎看不出来），而另一栏那个 `▶` 看上去跟真光标一模一样——
    /// 于是「Tab 没反应」「方向键动的是另一栏」这两句抱怨其实是同一个 bug。
    /// 焦点在哪，`▶` 就只能在哪。
    #[test]
    fn only_the_focused_pane_shows_a_cursor() {
        let (_t, _g, mut app, root) = tree(&["a", "b"]);
        open_picker(&mut app, vec![root.join("a").display().to_string()], root);

        let mut term = ratatui::Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let screen = buffer_text(term.backend().buffer());

        assert_eq!(
            screen.matches('\u{25b6}').count(),
            1,
            "焦点在最近栏，屏幕上却有不止一个光标：\n{screen}"
        );
    }

    /// 光标跟着焦点走。按一下 Tab，`▶` 必须**从左栏挪到右栏**——数目对
    /// 不代表位置对，上面那条只钉住「只有一个」，这条钉住「在正确的一边」。
    #[test]
    fn the_cursor_moves_to_the_other_pane_on_tab() {
        let (_t, _g, mut app, root) = tree(&["a", "b"]);
        open_picker(&mut app, vec![root.join("a").display().to_string()], root);
        press(&mut app, KeyCode::Tab);

        let mut term = ratatui::Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let buf = term.backend().buffer().clone();

        // 两栏是 42% / 58% 分的（见 `draw_pick_project` 的 `Layout`），
        // 不是对半——按 42% 那条线分，才是「在哪一栏」的正确判据。
        let half = buf.area.width * 42 / 100;
        let mut left = 0;
        let mut right = 0;
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if buf.cell((x, y)).map(|c| c.symbol()) == Some("\u{25b6}") {
                    if x < half {
                        left += 1;
                    } else {
                        right += 1;
                    }
                }
            }
        }
        assert_eq!((left, right), (0, 1), "Tab 之后光标没挪到右栏");
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
        // 比末段而不是整条路径：`current_dir()` 给的是分组键（已归一化），
        // macOS 上 `/var/...` 会变成 `/private/var/...`。要问的是「选中的是
        // 不是 proj 这个目录」，归一化不改变这个答案。
        assert!(
            app.current_dir().ends_with("proj"),
            "选定的是高亮那个目录，实际 {}",
            app.current_dir().display()
        );
        assert!(matches!(app.view, View::Board), "选完就回家");
        // 原来这里断言底栏出现一句「已切到 X」。`p` 降格成「把项目摆上看板」
        // 之后那句话没了：换项目是 `Tab`，而摆上看板这件事屏幕自己看得见——
        // 多出来一个组、光标落进去。断言改成断言那两件看得见的事。
        assert!(
            app.groups.iter().any(|g| g.name == "proj"),
            "选中的项目要作为一个组出现在看板上"
        );
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("proj".to_string()),
            "光标要落进那个组"
        );
    }

    /// 选中一个名字里带看不见字符的目录之后，真正打开的必须是**磁盘上那个
    /// 真实目录**，逐字节对得上——`truncate` 只清洗显示，`p.cwd.join` 用的
    /// 还是 `row.name` 原始值。这是这一节的安全底线：任何把清洗后的名字
    /// 拿去拼路径的改法都会在这里露出来（`canon` 需要目录真的存在才能
    /// 归一化，拼错了会直接找不到这个组）。
    /// 名字里带控制字符的目录**在 Windows 上造不出来**：NTFS 不允许
    /// 0x00-0x1F 出现在文件名里，`create_dir` 直接报「文件名语法不正确」。
    /// 而被测的判据正是 `char::is_control()`——也就是说这个局面在那个平台上
    /// 根本不会出现，不是「没覆盖到」。
    #[test]
    #[cfg(unix)]
    fn enter_opens_the_real_directory_even_when_its_name_hides_something_invisible() {
        let weird = "weird\x1bname";
        let (_t, _g, mut app, root) = tree(&[weird]);
        open_picker(&mut app, vec![], root.clone());
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);

        assert_eq!(
            app.current_group().map(|g| g.dir.clone()),
            Some(crate::ui::view::canon(&root.join(weird))),
            "打开的必须是原始名字对应的真实目录，不是清洗过的名字"
        );
    }

    /// 浏览栏画目录名时，含看不见字符的目录要挂一个压暗提示；正常目录
    /// **不能**挂——后一半同样重要，否则每一行都挂着它，提示就失去了意义。
    /// 名字里带控制字符的目录**在 Windows 上造不出来**：NTFS 不允许
    /// 0x00-0x1F 出现在文件名里，`create_dir` 直接报「文件名语法不正确」。
    /// 而被测的判据正是 `char::is_control()`——也就是说这个局面在那个平台上
    /// 根本不会出现，不是「没覆盖到」。
    #[test]
    #[cfg(unix)]
    fn draw_marks_directories_whose_name_hides_something_invisible() {
        let (_t, _g, mut app, root) = tree(&["normal", "weird\x1bname"]);
        open_picker(&mut app, vec![], root);
        press(&mut app, KeyCode::Tab);

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        // 只挤掉半角空格，留着换行：宽字符（这里是中文提示）在 ratatui 的
        // 单元格网格里，续格填的是一个字面空格而不是空串，逐格拼起来就会在
        // 每个汉字中间插一格——`draw_renders_all_profile_statuses_and_the_warning_border`
        // 那条测试也踩过同一件事，处理方式是连换行一起挤掉；这里要按行分开断言
        // 「正常目录不挂/异常目录要挂」，换行不能丢。
        let rendered: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| *c != ' ')
            .collect();
        let marker = text(Key::HiddenCharsInName, app.lang);

        let normal_line = rendered
            .lines()
            .find(|l| l.contains("normal"))
            .expect("normal 目录要画出来");
        assert!(
            !normal_line.contains(marker),
            "正常目录不该挂这个提示：{normal_line}"
        );

        // 显示名已经被 truncate 洗掉了看不见的字节，找那一行只能靠洗过的名字。
        let weird_line = rendered
            .lines()
            .find(|l| l.contains("weirdname"))
            .expect("藏着看不见字符的目录也要画出来（显示是清洗过的名字）");
        assert!(
            weird_line.contains(marker),
            "藏着看不见字符的目录要挂这个提示：{weird_line}"
        );
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

    /// 手输路径态按 Esc 退的是**一层**——回两栏那一屏，不是一步关掉整个
    /// 选择器，也不能顺手清掉已经打好的过滤词。
    ///
    /// 这条以前测的是 `view::back_one_level`（Ctrl+Q 那条全局退路）。Ctrl+Q
    /// 没了之后退路只剩这一个 Esc 分支，测试跟着搬到真正的按键处理上。
    #[test]
    fn escape_leaves_the_typing_state_before_leaving_the_picker() {
        let (_t, _g, mut app, root) = tree(&["a"]);
        open_picker(&mut app, vec!["/w/a".into()], root);
        let mut p = picker(&app);
        p.filter = "a".into();
        p.typing_path = Some("/tmp/b".into());
        app.view = View::PickProject(p);

        press(&mut app, KeyCode::Esc);

        let p = picker(&app);
        assert_eq!(p.typing_path, None, "应当退出手输态");
        assert_eq!(p.filter, "a", "退一层不该顺手清掉过滤词");
        assert_eq!(p.recent, vec!["/w/a".to_string()], "项目列表不该丢");
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

    /// 选 agent 这一屏要写着「在哪个项目里开」。按下 Enter 的后果是在某个
    /// 目录里起一个会话，而 `p` 之后是直接弹到这儿的——用户刚换完项目，
    /// 屏幕整个变了样，标题得替他确认一次。
    #[test]
    fn the_agent_picker_says_which_project_it_will_open_in() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![sess_in(1, "/Users/lei/work/dc/ai-mania")]);
        app.view = View::PickProfile {
            entries: vec![crate::proto::ProfileEntry {
                name: "claude".into(),
                label: "Claude".into(),
                note: String::new(),
                status: ProfileStatus::Ready,
                secret: None,
                install: None,
                has_secret: false,
            }],
            state: ratatui::widgets::ListState::default(),
            warning: None,
            no_git: false,
        };
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains("ai-mania"),
            "标题要写明会开在哪个项目里：{content}"
        );
    }

    fn one_ready_entry() -> Vec<crate::proto::ProfileEntry> {
        vec![crate::proto::ProfileEntry {
            name: "claude".into(),
            label: "Claude".into(),
            note: String::new(),
            status: ProfileStatus::Ready,
            secret: None,
            install: None,
            has_secret: false,
        }]
    }

    /// 不是 git 仓库的项目里，这一屏九项 agent 一个都开不起来（拒绝在
    /// `session.rs` 建会话那一步）。原来用户只能按下 Enter 才知道，然后
    /// 收一句红字、全屏没有一个键能让他往前走。标题上得先说，并且说出
    /// 那条出路。
    #[test]
    fn a_non_git_project_says_so_before_the_user_presses_enter() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let (mut app, _d) = App::test_app();
        app.view = View::PickProfile {
            entries: one_ready_entry(),
            state: ratatui::widgets::ListState::default(),
            warning: None,
            no_git: true,
        };
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();

        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let hint: String = text(Key::NotAGitRepoHint, app.lang)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains(&hint),
            "标题要先说这儿不是 git 仓库：{content}"
        );
    }

    /// `g` 就地把它变成 git 仓库，然后那句提示自己消失——用户不用退出去
    /// 再进来一次才发现现在能用了。
    #[test]
    fn g_creates_the_git_project_right_there() {
        let (mut app, d) = App::test_app();
        assert!(
            !crate::git::is_repo(&app.current_dir()),
            "前提：当前项目还不是 git 仓库（{}）",
            d.path().display()
        );
        app.view = View::PickProfile {
            entries: one_ready_entry(),
            state: ratatui::widgets::ListState::default(),
            warning: None,
            no_git: true,
        };

        handle_key(&mut app, KeyEvent::from(KeyCode::Char('g'))).unwrap();

        assert!(
            crate::git::is_repo(&app.current_dir()),
            "g 要真的建出仓库来"
        );
        assert!(
            matches!(app.view, View::PickProfile { no_git: false, .. }),
            "建完提示要跟着消失，不然用户以为还没成"
        );
        assert!(!app.message.error, "成功了不该报红");
    }

    /// **是** git 仓库的时候 `g` 不认——屏幕上那句「按 g 初始化」不写了，
    /// 键也就不该有反应。反过来的写法（键永远认、屏幕有时候写）会让用户在
    /// 一个正常项目里手滑按到 `g`，然后 dct 对着他的仓库跑一遍 `git init`。
    #[test]
    fn g_does_nothing_when_the_project_already_is_a_repo() {
        let (mut app, d) = App::test_app();
        app.view = View::PickProfile {
            entries: one_ready_entry(),
            state: ratatui::widgets::ListState::default(),
            warning: None,
            no_git: false,
        };

        handle_key(&mut app, KeyEvent::from(KeyCode::Char('g'))).unwrap();

        assert!(
            !d.path().join(".git").exists(),
            "no_git 为假时 g 不该动手——它读的是这个标志，不是文件系统"
        );
    }

    fn sess_in(id: u32, dir: &str) -> crate::session::SessionInfo {
        crate::session::SessionInfo {
            id,
            profile: "claude".into(),
            dir: dir.into(),
            state: crate::session::SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }
    }

    /// `p` 选定一个项目之后，它必须以一个组的形式出现在看板上，并且光标落进去——
    /// 否则用户按完 `p` 什么都没发生，`n` 也去不了那儿。
    #[test]
    fn confirming_a_project_puts_it_on_the_board_and_moves_the_cursor_there() {
        let (mut app, d) = App::test_app();
        let target = d.path().join("newproj");
        std::fs::create_dir(&target).unwrap();
        app.set_sessions(vec![]);

        super::super::pin_project(&mut app, target.clone());

        assert!(
            app.pinned.iter().any(|p| p.ends_with("newproj")),
            "要进 pinned"
        );
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("newproj".to_string()),
            "光标要落在新组上"
        );
    }

    /// 选完项目紧接着要问「用哪个 agent」——用户按 `p` 是为了去那儿干活，
    /// 不是为了看一眼看板。
    ///
    /// 单测里没有守护进程，`Profiles` 一定失败，所以这里断言的是那次失败
    /// 留下的痕迹（同 `grid` 里那条「四个键在两个视图里一模一样」的测法）：
    /// 底栏那句「列不出 agent」证明这一步真的去问过。**顺带钉住失败时的
    /// 落点**：不能把用户留在他刚刚确认过的项目选择器上——那一屏看起来像
    /// 「这一下没生效」，而项目其实已经切好了。
    #[test]
    fn confirming_a_project_goes_on_to_ask_which_agent() {
        let (mut app, d) = App::test_app();
        let target = d.path().join("newproj");
        std::fs::create_dir(&target).unwrap();
        app.set_sessions(vec![]);
        app.view = View::PickProject(ProjectPicker::new(Vec::new(), d.path().to_path_buf()));

        super::super::pin_project(&mut app, target.clone());

        assert_eq!(
            app.message.text,
            text(Key::CannotListAgents, app.lang),
            "要去拉一次 agent 列表"
        );
        assert!(
            !matches!(app.view, View::PickProject(_)),
            "拉不到列表也不能把用户留在项目选择器上"
        );
    }

    /// `x` 只能拿掉空组。有会话的组必须拒绝——「顺便停掉所有会话」是个
    /// 用户没要求过的复合动作，而 `s` 已经能一个一个停。
    #[test]
    fn removing_a_group_that_still_has_sessions_is_refused() {
        let (mut app, _d) = App::test_app();
        app.pinned = vec!["/w/a".to_string()];
        app.set_sessions(vec![sess_in(1, "/w/a")]);
        app.list_state.select(Some(0));

        let removed = super::super::unpin_current(&mut app);

        assert!(!removed, "拒绝了就得如实报告——调用方要靠它决定动不动光标");
        assert_eq!(app.groups.len(), 1, "组还在");
        assert!(app.message.error, "要给一句红字提示");
    }

    /// `x` 落在一个空组上就真的把它拿掉——本地 `pinned` 和看板上的组
    /// 必须一起消失，不能只改一半让下一次重算把它又变回来。
    #[test]
    fn removing_an_empty_group_takes_it_off_the_board() {
        let (mut app, d) = App::test_app();
        let gone = d.path().join("空项目");
        std::fs::create_dir(&gone).unwrap();
        app.pinned = vec![gone.display().to_string()];
        app.set_sessions(vec![sess_in(1, "/w/a")]);
        let gi = app
            .groups
            .iter()
            .position(|g| g.name == "空项目")
            .expect("前提：空组在看板上");
        super::super::goto_project(&mut app, gi);

        let removed = super::super::unpin_current(&mut app);

        assert!(removed, "真拿掉了就得如实报告");
        assert!(
            !app.groups.iter().any(|g| g.name == "空项目"),
            "组要从看板上消失"
        );
        assert!(app.pinned.is_empty(), "本地 pinned 也要一起清掉");
        assert!(!app.message.error, "拿掉空组不是错误");
    }

    /// **`pinned` 里存的拼写和分组键（canon 之后的）可能不是同一个字符串。**
    /// macOS 上 `/tmp/x` 归一化成 `/private/tmp/x`；按字面比对去删的话，
    /// `x` 会看起来「按了没反应」——组消失一帧，下一次重算又原样回来。
    /// 符号链接：Windows 上建它要开发者模式或管理员权限，摆不出这个现场。
    #[test]
    #[cfg(unix)]
    fn removing_a_group_matches_pinned_by_canonical_path_not_by_spelling() {
        let (mut app, d) = App::test_app();
        let real = d.path().join("链接目标");
        std::fs::create_dir(&real).unwrap();
        let link = d.path().join("软链");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        // 用户当初 pin 的是软链那条拼写，分组键却是 canon 之后的真实路径
        app.pinned = vec![link.display().to_string()];
        app.set_sessions(vec![]);
        app.list_state.select(Some(0));

        let removed = super::super::unpin_current(&mut app);

        assert!(removed, "真拿掉了就得如实报告");
        assert!(app.pinned.is_empty(), "按归一化后的路径比对才删得掉");
        assert!(app.groups.is_empty(), "组也要跟着消失");
    }

    /// 开机兜底：一个组都没有时把启动目录摆上去。没有它，全新安装的第一屏
    /// 是一个连光标都落不下去的空盒子，`n` 也没有目标。
    #[test]
    fn a_board_with_nothing_on_it_gets_the_start_dir() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![]);
        assert!(app.groups.is_empty(), "前提：全新守护进程，什么都没有");

        super::super::seed_start_project(&mut app);

        assert_eq!(app.groups.len(), 1, "看板上永远至少有一个组");
        assert!(app.current_group().is_some(), "光标有地方落");
        assert_eq!(app.current_dir(), super::super::view::canon(&app.start_dir));
    }

    /// 已经有组了就不补——启动目录跟用户手头这些项目未必有关系，
    /// 硬塞一行进去只是噪音。
    #[test]
    fn seeding_leaves_a_board_that_already_has_a_group_alone() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![sess_in(1, "/w/a")]);

        super::super::seed_start_project(&mut app);

        assert_eq!(app.groups.len(), 1, "不该多出启动目录那一行");
        assert!(app.pinned.is_empty());
    }

    /// **后台事件不许换掉当前项目——哪怕那个组是「有会话」才在看板上的。**
    ///
    /// 成员规则是「有**在跑**的会话 ∪ pinned」，所以一个没 pin 的组，最后一个
    /// 会话自己跑完停掉的那一刻就整个没了。要是光标正站在它里面，
    /// `find_anchor` 既找不到会话行、也找不到同 dir 的组头，只能退回第 0 行——
    /// **当前项目在用户没碰键盘的时候变了**，接着的 `n`/`x` 作用在别的项目上。
    /// 那正是整条分支要消灭的缺陷，也正好违反 spec §三的「组不塌陷」。
    ///
    /// 支点是「光标落在哪个组上就 pin 哪个组」。这条测试走的就是那条路：
    /// 挪光标 → `pin_cursor_group`（主循环每轮都调）→ 后台 `set_sessions`。
    #[test]
    fn a_background_stop_never_moves_the_cursor_out_of_its_project() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![sess_in(1, "/w/a"), sess_in(2, "/w/b")]);
        assert!(app.pinned.is_empty(), "前提：两个组都不是 pin 上来的");
        // 行：[组头 a, 1, 组头 b, 2]——用户挪到 b 的会话上
        app.list_state.select(Some(3));
        super::super::pin_cursor_group(&mut app);

        // 后台那一轮 List：b 的唯一一个会话自己跑完停了。没有任何按键。
        let mut done = sess_in(2, "/w/b");
        done.state = crate::session::SessionState::Stopped;
        app.set_sessions(vec![sess_in(1, "/w/a"), done]);

        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("b".to_string()),
            "当前项目不许在用户没按键的时候变；n/x 会跟着跑到别的项目上去"
        );
        assert_eq!(app.groups.len(), 2, "组不塌陷：b 还在看板上");
    }

    /// 反过来的一半：**用户从没去过**的组，最后一个会话停掉时照旧下看板——
    /// 这正是 `x` 能真的拿掉东西所依赖的那条成员规则。少了这一条，上面那条
    /// 测试用「所有组都不会消失」也能通过，而 `x` 会退回成一个死键。
    #[test]
    fn a_project_the_cursor_never_visited_still_leaves_when_it_goes_quiet() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![sess_in(1, "/w/a"), sess_in(2, "/w/b")]);
        // 光标留在 a 上，从没去过 b
        app.list_state.select(Some(1));
        super::super::pin_cursor_group(&mut app);

        let mut done = sess_in(2, "/w/b");
        done.state = crate::session::SessionState::Stopped;
        app.set_sessions(vec![sess_in(1, "/w/a"), done]);

        assert_eq!(
            app.groups
                .iter()
                .map(|g| g.name.clone())
                .collect::<Vec<_>>(),
            vec!["a".to_string()],
            "没去过的项目安静下来就该下看板"
        );
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("a".into())
        );
    }

    /// **守护进程报回来的 `pinned` 不许把光标脚下那个组冲掉。**
    ///
    /// `pin_cursor_group` 在 `PinProject` 失败时也只记在本地（这是对的），
    /// 但下一轮把守护进程那份整个盖上来、紧跟着 `refresh_rows`，那条本地的 pin
    /// 就没了——光标脚下那个组要是只靠 pin 留着，这一下它从看板上消失，光标
    /// 掉回第 0 行。当前项目在用户没按键的时候变了，而起因只是一次 IPC 往返
    /// （超时会丢连接、下一次调用透明重连，于是后面的请求全成功，没有任何
    /// 错误浮上来）；另一个 dct 窗口按 `x` 也是同一条路。
    #[test]
    fn a_projects_sync_never_unpins_the_group_under_the_cursor() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![sess_in(1, "/w/a"), sess_in(2, "/w/b")]);
        app.list_state.select(Some(3));
        super::super::pin_cursor_group(&mut app);
        // b 的会话停了：现在它留在看板上**只**靠那条 pin
        let mut done = sess_in(2, "/w/b");
        done.state = crate::session::SessionState::Stopped;
        app.set_sessions(vec![sess_in(1, "/w/a"), done]);
        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("b".to_string()),
            "前提：光标还在 b 上"
        );

        // 守护进程报回来的那份里没有 b（请求掉了，或者另一个窗口 x 掉了）
        super::super::adopt_pinned(&mut app, vec!["/w/a".to_string()]);

        assert_eq!(
            app.current_group().map(|g| g.name.clone()),
            Some("b".to_string()),
            "组不塌陷不能押在一次 IPC 往返成功上"
        );
        assert_eq!(app.groups.len(), 2);
    }

    /// 但别的项目照旧同步：另一个 dct 窗口 `p` 上来的要出现、`x` 掉的要消失。
    /// 只保光标脚下那一个。
    #[test]
    fn a_projects_sync_still_adopts_every_other_change() {
        let (mut app, _d) = App::test_app();
        app.pinned = vec!["/w/gone".to_string()];
        app.set_sessions(vec![sess_in(1, "/w/a")]);
        app.list_state.select(Some(1));
        super::super::pin_cursor_group(&mut app);

        // 另一个窗口：x 掉了 /w/gone，p 上来了 /w/new
        super::super::adopt_pinned(&mut app, vec!["/w/a".to_string(), "/w/new".to_string()]);

        let names: Vec<String> = app.groups.iter().map(|g| g.name.clone()).collect();
        assert!(
            !names.contains(&"gone".to_string()),
            "x 掉的要消失：{names:?}"
        );
        assert!(
            names.contains(&"new".to_string()),
            "p 上来的要出现：{names:?}"
        );
    }

    /// 同一个组里上下走一百下，只该 pin 一次——判据是这个组自己的 `pinned`
    /// 标志，不是「光标动了没有」。每个方向键一次守护进程往返的话，看板在
    /// 长列表里会一顿一顿。
    #[test]
    fn pinning_the_cursor_group_is_once_per_project_not_once_per_keypress() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![sess_in(1, "/w/a"), sess_in(2, "/w/a")]);

        for row in [0usize, 1, 2, 1, 0, 2] {
            app.list_state.select(Some(row));
            super::super::pin_cursor_group(&mut app);
        }

        assert_eq!(
            app.pinned.len(),
            1,
            "同一个项目只 pin 一次：{:?}",
            app.pinned
        );
    }

    /// **只剩已停止会话的项目，`x` 拿得掉。**
    ///
    /// 已停止的会话没有进程，拿掉这个组毁不掉任何还活着的东西。按「有没有
    /// 会话」拒绝会拒出一个死局：这个项目永远下不了看板，底栏连 `x` 都不写，
    /// 而那句「先停掉才能移除」说的正是用户刚做过的事——唯一的出路是去
    /// 另一个终端敲 `dct prune`，而 TUI 里根本没有这个键。
    ///
    /// 断言到「这一行真的没了」，不只是「函数返回了 true」：`x` 只做 unpin，
    /// 而组还有第二个来源（有会话）——只改拒绝判据的话，这一行会原样留在
    /// 屏幕上，按下去什么都不发生。
    #[test]
    fn x_removes_a_project_that_only_holds_stopped_sessions() {
        let (mut app, _d) = App::test_app();
        let mut done = sess_in(9, "/w/z");
        done.state = crate::session::SessionState::Stopped;
        app.pinned = vec!["/w/z".to_string()];
        app.set_sessions(vec![sess_in(1, "/w/a"), done]);
        // 行：[组头 a, 1, 组头 z, 9]——光标停到 z 上
        app.list_state.select(Some(2));
        assert_eq!(app.current_dir(), std::path::PathBuf::from("/w/z"));

        assert!(super::super::unpin_current(&mut app), "拿得掉");

        assert!(
            !app.groups.iter().any(|g| g.name == "z"),
            "那一行必须真的从看板上没了，而不是取消 pin 之后原样留着"
        );
        assert!(app.message.text.is_empty(), "成功不该报错");
    }

    /// 反过来：还有**在跑**的会话就照旧拒绝，并且说那句话——这时候
    /// 「先停掉才能移除」是真能照做的建议。
    #[test]
    fn x_still_refuses_a_project_with_a_running_session() {
        let (mut app, _d) = App::test_app();
        app.pinned = vec!["/w/z".to_string()];
        app.set_sessions(vec![sess_in(1, "/w/a"), sess_in(9, "/w/z")]);
        app.list_state.select(Some(2));
        assert_eq!(app.current_dir(), std::path::PathBuf::from("/w/z"));

        assert!(!super::super::unpin_current(&mut app), "拿不掉");

        assert!(app.groups.iter().any(|g| g.name == "z"), "组还在");
        assert!(
            app.message.error && !app.message.text.is_empty(),
            "要红字说一句"
        );
    }

    /// 底栏和 `?` 浮层写不写 `x 移除`，跟它拿不拿得掉必须逐条对上——
    /// 屏幕上写着做不到的操作比不写更糟，而一个做得到却不写的键，
    /// 用户永远找不到。
    #[test]
    fn the_bar_offers_x_on_a_project_that_only_holds_stopped_sessions() {
        let (mut app, _d) = App::test_app();
        let mut done = sess_in(9, "/w/z");
        done.state = crate::session::SessionState::Stopped;
        app.pinned = vec!["/w/z".to_string()];
        app.set_sessions(vec![sess_in(1, "/w/a"), done]);
        app.list_state.select(Some(2));

        assert!(
            super::super::help_ctx_for(&app, &View::Board).can_remove,
            "拿得掉就得写出来"
        );

        // 换成一个在跑的会话：同一个位置，答案必须翻过来
        app.set_sessions(vec![sess_in(1, "/w/a"), sess_in(9, "/w/z")]);
        assert!(
            !super::super::help_ctx_for(&app, &View::Board).can_remove,
            "拿不掉就不许写"
        );
    }

    /// **开机补位是后台路径，不许换用户正看着的那一屏。**
    ///
    /// 它挂在第一次 `List` **成功**之后。守护进程慢上几轮的话，用户完全
    /// 可能已经按 `N` 开了选择器、按 `l` 进了设置页——这时候把他甩回看板，
    /// 是一次他没有按过任何键的视图切换。今天走不到只是因为第一轮几乎
    /// 总是一次就成，那是运气不是设计。
    #[test]
    fn seeding_never_yanks_the_user_out_of_a_screen_they_opened() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![]);
        app.view = View::Settings {
            state: ListState::default(),
            sub: None,
        };

        super::super::seed_start_project(&mut app);

        assert!(
            matches!(app.view, View::Settings { .. }),
            "用户开的设置页必须还在"
        );
        assert_eq!(app.groups.len(), 1, "补位本身照做");
    }

    /// 反过来：本来就在看板上时照常重算落点——那时候「回家」是恒等式，
    /// 唯一的作用是让刚摆上去的组在九宫格里也有个合理的焦点。
    #[test]
    fn seeding_still_lands_home_when_the_user_is_already_on_the_board() {
        let (mut app, _d) = App::test_app();
        app.set_sessions(vec![]);
        app.view_mode = crate::ui::ViewMode::Grid;
        app.view = View::grid(7);

        super::super::seed_start_project(&mut app);

        assert!(
            matches!(app.view, View::Grid { focus: 0, .. }),
            "还在九宫格里，焦点收拢到一个真实存在的格子"
        );
    }
}
