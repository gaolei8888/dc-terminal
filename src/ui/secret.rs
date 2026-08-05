use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::client::Client;
use crate::proto::{socket_path, Request, Response, SecretPrompt};
use crate::verify::VerifyOutcome;

use super::app::App;
use super::view::{
    decide_delete_key, is_plain_key, secret_rows, DeleteKeyAction, SecretPhase, View,
};
use super::widgets::{pad_to, truncate, Msg};
use super::{dim, move_sel_n, open_url, refetch_secrets};

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match app.view.clone() {
        View::EnterSecret { .. } => handle_enter_secret(app, key),
        View::Secrets { .. } => handle_secrets(app, key),
        _ => Ok(()),
    }
}

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
fn handle_enter_secret(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::EnterSecret {
        profile,
        label,
        prompt,
        mut buf,
        phase,
        return_to_settings,
    } = app.view.clone()
    else {
        return Ok(());
    };
    match phase.clone() {
        SecretPhase::Verifying => {
            // 验证在后台线程跑，buf 已经发出去了，这期间敲字符/回车
            // 都改不了那次正在飞的请求，只会让用户误以为在做别的事。
            // 只留 Esc：想退就现在退，且必须现在就扔掉 verify_rx——
            // 不然迟到的结果会套在一个用户已经不认得的视图上。
            if key.code == KeyCode::Esc {
                app.verify_rx = None;
                app.view = if return_to_settings {
                    View::Secrets {
                        entries: Vec::new(),
                        state: ListState::default(),
                        pending_delete: None,
                    }
                } else {
                    View::PickProfile {
                        entries: Vec::new(),
                        state: ListState::default(),
                        warning: None,
                    }
                };
            } else {
                app.view = View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    phase: SecretPhase::Verifying,
                    return_to_settings,
                };
            }
        }
        SecretPhase::Typing | SecretPhase::Failed(_) => match key.code {
            KeyCode::Esc => {
                app.view = if return_to_settings {
                    View::Secrets {
                        entries: Vec::new(),
                        state: ListState::default(),
                        pending_delete: None,
                    }
                } else {
                    View::PickProfile {
                        entries: Vec::new(),
                        state: ListState::default(),
                        warning: None,
                    }
                };
            }
            KeyCode::Enter => {
                let (tx, rx) = std::sync::mpsc::channel();
                // 后台验证线程要自己开一条到守护进程的连接——主循环这条 client
                // 正忙着画界面。`socket_path()` 是纯函数（只读 $HOME），比把
                // Client 内部私有的 socket 字段掏出来更省事。
                let sock = socket_path();
                let p = profile.clone();
                let v = buf.clone();
                // 结果送回来时要能比对"这还是不是当初发起这次验证的
                // 那个请求"（见 `verify_outcome_applies_to`），所以
                // 在 `p`/`v` 被移进 `Request::VerifySecret` 之前先
                // 各留一份拷贝，跟结果一起送回主循环。
                let stamped_profile = p.clone();
                let stamped_buf = v.clone();
                std::thread::spawn(move || {
                    // 另开一条连接：主循环那条还要继续画界面
                    let outcome = Client::connect(&sock)
                        .and_then(|mut c| {
                            c.call(Request::VerifySecret {
                                profile: p,
                                value: v,
                            })
                        })
                        .map(|r| match r {
                            Response::Verify(o) => o,
                            _ => VerifyOutcome::Unreachable,
                        })
                        .unwrap_or(VerifyOutcome::Unreachable);
                    let _ = tx.send((stamped_profile, stamped_buf, outcome));
                });
                app.verify_rx = Some(rx);
                app.view = View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    phase: SecretPhase::Verifying,
                    return_to_settings,
                };
            }
            KeyCode::Backspace => {
                buf.pop();
                app.view = View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    phase: SecretPhase::Typing,
                    return_to_settings,
                };
            }
            // Ctrl+O 不用 o：o 得留给密钥输入本身
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // MINOR 8（最终整分支 code review）：`open` 只在 macOS
                // 上存在，Linux 桌面环境一般是 `xdg-open`；两边都
                // 打不开的话必须告诉用户，不能让「Ctrl+O 打开申领
                // 页面」这行提示看着能按、按下去却悄无声息——用户
                // 会以为是自己按错了键。
                if let Some(url) = &prompt.url {
                    if !open_url(url) {
                        app.message =
                            Msg::err(crate::i18n::msg::cannot_open_browser(app.lang, url));
                    }
                }
                app.view = View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    phase,
                    return_to_settings,
                };
            }
            KeyCode::Char(c) => {
                buf.push(c);
                app.view = View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    phase: SecretPhase::Typing,
                    return_to_settings,
                };
            }
            _ => {
                app.view = View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    phase,
                    return_to_settings,
                };
            }
        },
    }
    Ok(())
}

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
fn handle_secrets(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Secrets {
        entries,
        mut state,
        pending_delete,
    } = app.view.clone()
    else {
        return Ok(());
    };
    match key.code {
        KeyCode::Esc => app.view = super::home_view(app),
        KeyCode::Down | KeyCode::Up => {
            let d = if key.code == KeyCode::Down { 1 } else { -1 };
            move_sel_n(&mut state, secret_rows(&entries).len(), d);
            // 光标一动就撤销武装状态：武装的是「这一行」，挪开之后
            // 再按第二次 d，落地的必须是新选中行的第一次按键，不能让
            // 上一行攒的「再按一次就删」悄悄延续到新行头上（见 Finding 1）。
            // 顺带清掉「再按一次删除 X」那句消息——行内提示已经跟着
            // 光标挪走了，底部消息栏要是还留着旧行的名字，用户会
            // 搞不清这次挪动到底有没有把武装状态带走。
            if pending_delete.is_some() {
                app.message = "".into();
            }
            app.view = View::Secrets {
                entries,
                state,
                pending_delete: None,
            };
        }
        KeyCode::Enter => {
            let rows = secret_rows(&entries);
            // find 而不是直接 entries[i]：rows 是 entries 过滤掉不需要密钥
            // 的行之后的结果，下标不对应；按名字在 entries 里找回
            // 完整的那一条，才拿得到 label/secret 提示。
            let target = state
                .selected()
                .and_then(|i| rows.get(i))
                .and_then(|(name, _)| entries.iter().find(|e| &e.name == name));
            app.view = match target {
                Some(e) => View::EnterSecret {
                    profile: e.name.clone(),
                    label: e.label.clone(),
                    // 这一页只列了 secret.is_some() 的行（见 secret_rows），
                    // 所以这里的 unwrap_or 只是跟 AskSecret 那条路径的兜底
                    // 手法保持一致，实际不会被这个默认值命中。
                    prompt: e.secret.clone().unwrap_or(SecretPrompt {
                        hint: String::new(),
                        url: None,
                    }),
                    buf: String::new(),
                    phase: SecretPhase::Typing,
                    // 从设置页进来，改完要回设置页
                    return_to_settings: true,
                },
                // Enter 也是「其他键」，没找到目标（没有选中行）时
                // 留在原地也要把武装状态清掉。
                None => View::Secrets {
                    entries,
                    state,
                    pending_delete: None,
                },
            };
        }
        KeyCode::Char('d') if is_plain_key(&key) => {
            let rows = secret_rows(&entries);
            let target = state.selected().and_then(|i| rows.get(i)).cloned();
            // 判断这半是纯函数（见 decide_delete_key 的文档注释，
            // 它是这个任务的单测入口）；发不发 DeleteSecret 请求
            // 这半必须留在这里，因为它要碰 daemon 连接。
            app.view = match decide_delete_key(target, &pending_delete) {
                // 没配过的密钥没什么可删的——照样发一次 DeleteSecret
                // 只会得到一句空洞的「已删除」，用户会怀疑自己是不是
                // 删错了别的东西。
                DeleteKeyAction::NotConfigured => {
                    app.message =
                        crate::i18n::text(crate::i18n::Key::NothingToDelete, app.lang).into();
                    View::Secrets {
                        entries,
                        state,
                        pending_delete: None,
                    }
                }
                // 第二次按 d：武装记的名字正是当前选中行，才真删。
                DeleteKeyAction::Confirm(name) => {
                    match app.client().and_then(|c| {
                        c.call(Request::DeleteSecret {
                            profile: name.clone(),
                        })
                    }) {
                        Ok(Response::Ok) => {
                            app.message = crate::i18n::msg::secret_deleted(
                                app.lang,
                                &entries
                                    .iter()
                                    .find(|e| e.name == name)
                                    .map(|e| e.label.clone())
                                    .unwrap_or(name.clone()),
                            )
                            .into();
                            refetch_secrets(app, Some(&name))
                        }
                        Ok(Response::Error(ref e)) => {
                            app.message = Msg::err(crate::i18n::msg::error(app.lang, e));
                            View::Secrets {
                                entries,
                                state,
                                pending_delete: None,
                            }
                        }
                        _ => {
                            app.message = Msg::err(
                                crate::i18n::text(crate::i18n::Key::SecretNotDeleted, app.lang)
                                    .into(),
                            );
                            View::Secrets {
                                entries,
                                state,
                                pending_delete: None,
                            }
                        }
                    }
                }
                // 第一次按 d：武装，不发任何请求。行内会画出「再按
                // d 删除」（见 draw() 里 pending_delete 那一支）；
                // 消息栏再重复一遍是双保险，行内提示万一没看到，
                // 底栏还有一句。
                DeleteKeyAction::Arm(name) => {
                    app.message = crate::i18n::msg::confirm_delete_secret(
                        app.lang,
                        &entries
                            .iter()
                            .find(|e| e.name == name)
                            .map(|e| e.label.clone())
                            .unwrap_or_else(|| name.clone()),
                    )
                    .into();
                    View::Secrets {
                        entries,
                        state,
                        pending_delete: Some(name),
                    }
                }
                DeleteKeyAction::NoSelection => View::Secrets {
                    entries,
                    state,
                    pending_delete: None,
                },
            };
        }
        // 任何其他键都取消武装——这是 Finding 1 要求的「反应性按键
        // 不该踩中确认」的核心：只有原地再按一次 d 才算确认，别的
        // 任何输入都当作取消，而不是悄悄忽略武装状态继续挂着。
        _ => {
            // 同 ↑↓ 分支：武装期间挂着的「再按一次删除 X」提示要
            // 跟着武装状态一起清掉，不然取消之后底部还留着一句
            // 半真半假的话。
            if pending_delete.is_some() {
                app.message = "".into();
            }
            app.view = View::Secrets {
                entries,
                state,
                pending_delete: None,
            }
        }
    }
    Ok(())
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    match &app.view {
        View::EnterSecret { .. } => draw_enter_secret(f, area, app),
        View::Secrets { .. } => draw_secrets(f, area, app),
        _ => {}
    }
}

fn draw_enter_secret(f: &mut Frame, area: Rect, app: &mut App) {
    let View::EnterSecret {
        label,
        prompt,
        buf,
        phase,
        return_to_settings,
        ..
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
    let mut lines: Vec<Line> = Vec::new();
    if !prompt.hint.is_empty() {
        lines.push(Line::from(Span::styled(prompt.hint.clone(), dim())));
        lines.push(Line::from(""));
    }
    // 显示成圆点：密钥不该以明文停在屏幕上，用户可能在录屏或在办公室
    lines.push(Line::from(format!("{}▌", "•".repeat(buf.chars().count()))));
    lines.push(Line::from(""));
    match phase {
        SecretPhase::Typing => {}
        SecretPhase::Verifying => lines.push(Line::from(Span::styled(
            crate::i18n::text(crate::i18n::Key::VerifyingShort, app.lang),
            Style::default().fg(Color::Cyan),
        ))),
        SecretPhase::Failed(m) => lines.push(Line::from(Span::styled(
            m.clone(),
            Style::default().fg(Color::Red),
        ))),
    }
    if prompt.url.is_some() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            crate::i18n::text(crate::i18n::Key::OpenSignupPage, app.lang),
            dim(),
        )));
    }
    // IMPORTANT 3（最终整分支 code review）：Task 13 把「回哪」这句话
    // 在 `escape_hint`/`idle_help` 上按 `return_to_settings` 分了岔，
    // 唯独漏了这个标题——它照旧硬编码「回列表」，跟低一行的底栏
    // 「Esc 回设置」当场自相矛盾，而标题字号更大，用户会先信错的
    // 那句。这里补上同样的分支，别让第三处文案再单独漂移。
    let title = if *return_to_settings {
        crate::i18n::msg::enter_secret_title(app.lang, label, true)
    } else {
        crate::i18n::msg::enter_secret_title(app.lang, label, false)
    };
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        ),
        area,
    );
}

fn draw_secrets(f: &mut Frame, area: Rect, app: &mut App) {
    let View::Secrets {
        entries,
        state,
        pending_delete,
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
    let rows = secret_rows(entries);
    let items: Vec<ListItem> = rows
        .iter()
        .map(|(name, configured)| {
            // 按名字回 entries 里找 label：rows 只是「名字 + 配没配」
            // 这两列的投影，界面上要给用户看的是人话名字，不是内部标识。
            let label = entries
                .iter()
                .find(|e| &e.name == name)
                .map(|e| e.label.clone())
                .unwrap_or_else(|| name.clone());
            // 武装了删除的那一行不显示「已配」——显示「再按 d 删除」，
            // 让用户在犯下第二次按键之前，眼睛里看到的就是明确的警告，
            // 而不是靠底部消息栏一句可能被扫过的小字（见 Finding 1）。
            if pending_delete.as_deref() == Some(name.as_str()) {
                ListItem::new(Line::from(vec![
                    Span::raw(pad_to(&truncate(&label, 14), 14)),
                    Span::styled(
                        crate::i18n::text(crate::i18n::Key::PressDAgainToDelete, app.lang),
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::raw(pad_to(&truncate(&label, 14), 14)),
                    // 三元式抬到 `Style` 这一层：`dim()` 返回的是 `Style`
                    // （`Unknown` 那支是 DIM 修饰符而不是颜色），塞不进 `.fg()`
                    Span::styled(
                        if *configured {
                            crate::i18n::text(crate::i18n::Key::SecretSet, app.lang)
                        } else {
                            crate::i18n::text(crate::i18n::Key::SecretUnset, app.lang)
                        },
                        if *configured {
                            Style::default().fg(Color::Green)
                        } else {
                            dim()
                        },
                    ),
                ]))
            }
        })
        .collect();
    let mut s = state.clone();
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(crate::i18n::text(crate::i18n::Key::SecretsTitle, app.lang)),
            )
            .highlight_symbol("▶ "),
        area,
        &mut s,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ProfileStatus;
    use crate::proto::ProfileEntry;
    use ratatui::backend::TestBackend;

    fn entry(name: &str, status: ProfileStatus) -> ProfileEntry {
        ProfileEntry {
            name: name.into(),
            label: name.into(),
            note: String::new(),
            has_secret: status != ProfileStatus::NeedsSecret,
            status,
            secret: None,
            install: None,
        }
    }

    /// 给一个 entry 挂上密钥提示——`secret_rows` 只列 `secret.is_some()` 的
    /// 行，光靠 `status` 不够，得真的声明了密钥这件事才会出现在密钥页上。
    /// `has_secret` 不在这里动，沿用 `entry()` 按 `status` 给的默认值——
    /// 两个测试用例恰好落在 `has_secret` 跟 `status` 一致的那一半（见
    /// `secret_rows` 的注释里 `NeedsDependency`/`NotInstalled` 那个反例）。
    fn with_secret(mut e: ProfileEntry) -> ProfileEntry {
        e.secret = Some(SecretPrompt {
            hint: String::new(),
            url: None,
        });
        e
    }

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

    /// 密钥比窄终端还宽的时候，圆点行不能把 ratatui 的 buffer 写出界——
    /// 真实场景：40 列的分屏终端 + 一个 100 字符的长 token。
    #[test]
    fn secret_view_dots_line_does_not_panic_when_wider_than_the_terminal() {
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.view = View::EnterSecret {
            profile: "kimi".into(),
            label: "Kimi".into(),
            prompt: SecretPrompt {
                hint: String::new(),
                url: None,
            },
            buf: "x".repeat(200),
            phase: SecretPhase::Typing,
            return_to_settings: false,
        };
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
    }

    /// MINOR 7（最终整分支 code review）：`draw_does_not_panic_for_all_views`
    /// 拿 `"sk-abc123"` 渲染过填密钥的三个阶段，但只断言了不 panic——真正
    /// 要守住的那一行（`"•".repeat(...)`）没人盯着。这条测试直接确认明文
    /// 不会出现在屏幕上，把这条这个分支上最要紧的安全属性变成一个真正的
    /// 回归测试，而不是"看代码觉得应该没问题"。
    #[test]
    fn secret_view_masks_the_key_on_screen() {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.view = View::EnterSecret {
            profile: "kimi".into(),
            label: "Kimi".into(),
            prompt: SecretPrompt {
                hint: String::new(),
                url: None,
            },
            buf: "sk-abc123".into(),
            phase: SecretPhase::Typing,
            return_to_settings: false,
        };
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        assert!(
            !buffer_text(term.backend().buffer()).contains("sk-abc123"),
            "密钥不能以明文出现在屏幕上"
        );
    }

    /// IMPORTANT 3（最终整分支 code review）：Task 13 把「Esc 回哪」这句话
    /// 按 `return_to_settings` 分了岔，但只改了 `escape_hint`/`idle_help`
    /// 两处，标题（画面里字号最大的那句话）被漏掉了，硬编码成「回列表」，
    /// 从设置页进来的这一屏会同时印着「回列表」（标题）和「回设置」
    /// （底栏）——两句自相矛盾。两种来源各画一遍，断言画面上只出现跟
    /// 这次来源匹配的那句话，另一句完全不出现，防止标题再单独漂移一次。
    #[test]
    fn secret_view_title_agrees_with_escape_hint_for_both_origins() {
        let render = |return_to_settings: bool| -> String {
            let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
            let (mut app, _dir) = App::test_app();
            app.view = View::EnterSecret {
                profile: "kimi".into(),
                label: "Kimi".into(),
                prompt: SecretPrompt {
                    hint: String::new(),
                    url: None,
                },
                buf: String::new(),
                phase: SecretPhase::Typing,
                return_to_settings,
            };
            term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
            buffer_text(term.backend().buffer())
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        };

        let from_picker = render(false);
        assert!(from_picker.contains("返回列表"), "{from_picker}");
        assert!(!from_picker.contains("返回设置"), "{from_picker}");

        let from_settings = render(true);
        assert!(from_settings.contains("返回设置"), "{from_settings}");
        assert!(!from_settings.contains("返回列表"), "{from_settings}");
    }

    #[test]
    fn secrets_view_renders_without_panicking_when_nothing_needs_a_key() {
        // 边界情况：所有 profile 都不需要密钥（或者用户碰巧只装了这类）。
        // 空列表不该让渲染 panic，也不该显示成一片空白无提示——至少标题
        // 「密钥设置」得画出来。
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        let entries = vec![entry("claude", ProfileStatus::Ready)];
        app.view = View::Secrets {
            entries,
            state: ListState::default(),
            pending_delete: None,
        };
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let c: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(c.contains(crate::i18n::text(crate::i18n::Key::SecretsTitle, app.lang)));
    }

    #[test]
    fn secrets_view_renders_configured_and_unconfigured_rows() {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        let mut state = ListState::default();
        state.select(Some(0));
        let entries = vec![
            with_secret(entry("kimi", ProfileStatus::Ready)),
            with_secret(entry("glm", ProfileStatus::NeedsSecret)),
        ];
        app.view = View::Secrets {
            entries,
            state,
            pending_delete: None,
        };
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let c: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            c.contains(crate::i18n::text(crate::i18n::Key::SecretSet, app.lang)),
            "配过的那行要显示已配：{c}"
        );
        assert!(
            c.contains(crate::i18n::text(crate::i18n::Key::SecretUnset, app.lang)),
            "没配的那行要显示未配：{c}"
        );
    }

    // ———— Finding 1（Task 13 code review）：删密钥的二次确认 ————
    //
    // `d` 在密钥页是真删除，物理键跟看板上「看 diff」那个无害的 `d` 完全
    // 一样，肌肉记忆会带过来。下面这条测试覆盖两段式确认在渲染上的落点：
    // 武装之后这一行该显示什么。

    #[test]
    fn secrets_view_renders_the_armed_delete_prompt_on_its_row() {
        // 武装之后这一行不该再显示「已配」，而要显示明确的「再按 d 删除」
        // 警告——这是 finding 里点名要求的「inline prompt on that row」。
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        let mut state = ListState::default();
        state.select(Some(0));
        let entries = vec![with_secret(entry("kimi", ProfileStatus::Ready))];
        app.view = View::Secrets {
            entries,
            state,
            pending_delete: Some("kimi".to_string()),
        };
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let c: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            c.contains("再按") && c.contains('d') && c.contains("删除"),
            "武装状态要在行内画出明确提示：{c}"
        );
        assert!(
            !c.contains(crate::i18n::text(crate::i18n::Key::SecretSet, app.lang)),
            "武装的这一行不该继续显示「已配」，会跟警告混在一起：{c}"
        );
    }
}
