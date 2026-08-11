//! 手机通知设置页。
//!
//! **这一页存在的全部理由是那一行状态。** 填令牌是容易的那一半——难的
//! 是配对本身是异步的：守护进程验过令牌之后转去后台长轮询，等用户在
//! Telegram 上给 bot 发第一条消息。不给这件事一个去处，用户填完令牌就会
//! 对着一片空白发呆，不知道自己是不是做对了、接下来该干什么。
//!
//! 四种状态，每一种都要带下一步——「已连上」是唯一不需要下一步的终点，
//! 其余三种都必须给出路（`every_state_tells_the_user_what_to_do_next`）。
//!
//! 令牌是密钥。`status_line`/`next_step` 两个函数**故意不读**
//! `PhoneState::Broken` 的 `message` 字段——这是一条纵深防御，见
//! `proto::PhoneState::Broken` 的文档注释和
//! `the_token_never_appears_in_any_status_text` 这条测试。`reason` 字段
//! 不受这条限制：它是个封闭的三值枚举，不可能夹带令牌，`next_step` 靠它
//! 分岔出三句不同的下一步（fix round 1 的根因修复——以前只有一句写死的
//! 话，令牌失效、被拉黑、网络不通说的是同一句，其中两种听那句话去做完全
//! 没用）。

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::client::Client;
use crate::i18n::{msg, text, Key, Lang};
use crate::proto::{socket_path, PhoneBrokenReason, PhoneState, PhoneStatus, Request, Response};

use super::app::App;
use super::dim;
use super::view::{phone_key_has_effect, settings_state_on_phone, PhoneEntry, SecretPhase, View};

/// 那一行状态。**四种取值都要给非空文案**——见模块头注释。
pub(crate) fn status_line(status: &PhoneStatus, lang: Lang) -> String {
    match &status.state {
        PhoneState::Off => text(Key::PhoneOff, lang).to_string(),
        PhoneState::WaitingForPairing => {
            let bot = status.bot.as_deref().unwrap_or("?");
            msg::phone_waiting(lang, bot)
        }
        PhoneState::Paired => match &status.owner {
            Some(owner) => msg::phone_paired(lang, owner),
            None => text(Key::PhonePairedNoOwner, lang).to_string(),
        },
        // **故意不读 `message` 字段**——见模块头注释和
        // `the_token_never_appears_in_any_status_text`。真正的诊断详情
        // 画在 `draw()` 里单独的一行，不经过这个函数。三种原因共用同一句
        // headline，区分交给 `next_step`。
        PhoneState::Broken { .. } => text(Key::PhoneBrokenHeadline, lang).to_string(),
    }
}

/// 下一步该干什么。「已连上」是唯一的终点，返回 `None`；其余三种都必须
/// 给出路——一个不告诉用户下一步该干什么的错误，按房规就是没写完。
pub(crate) fn next_step(status: &PhoneStatus, lang: Lang) -> Option<String> {
    match &status.state {
        PhoneState::Off => Some(text(Key::PhoneOffNextStep, lang).to_string()),
        PhoneState::WaitingForPairing => Some(text(Key::PhoneWaitingNextStep, lang).to_string()),
        PhoneState::Paired => None,
        // 读 `reason`（安全：封闭枚举，不可能夹带令牌），**不读 `message`**
        // ——三个原因、三句不同的下一步，见模块头注释。
        PhoneState::Broken { reason, .. } => Some(
            match reason {
                PhoneBrokenReason::BadToken => text(Key::PhoneNextStepBadToken, lang),
                PhoneBrokenReason::BotBlocked => text(Key::PhoneNextStepBlocked, lang),
                PhoneBrokenReason::Unreachable => text(Key::PhoneNextStepUnreachable, lang),
            }
            .to_string(),
        ),
    }
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Phone { status, entry } = app.view.clone() else {
        return Ok(());
    };
    match entry {
        Some(e) => handle_entry(app, key, status, e),
        None => handle_status(app, key, status),
    }
    Ok(())
}

/// **这个函数里永远不要 `continue`。** 同 `settings_view.rs`/`secret.rs`
/// 的同名约束——循环末尾还有一段清理陈旧 `message` 的逻辑。
fn handle_status(app: &mut App, key: KeyEvent, status: PhoneStatus) {
    match key.code {
        KeyCode::Esc => {
            app.view = View::Settings {
                state: settings_state_on_phone(),
                lang: None,
            };
        }
        // 能不能按由 `phone_key_has_effect` 说了算——`view.rs::idle_help`
        // 决定写不写这个键用的是同一份判断，两处必须共用（见它的文档注释）。
        KeyCode::Enter if phone_key_has_effect(&status.state, key.code) => {
            app.view = View::Phone {
                status,
                entry: Some(PhoneEntry {
                    buf: String::new(),
                    phase: SecretPhase::Typing,
                }),
            };
        }
        KeyCode::Char('r') if phone_key_has_effect(&status.state, key.code) => {
            let resp = app.client().and_then(|c| c.call(Request::PhoneUnpair));
            app.view = View::Phone {
                status: apply_phone_response(status, resp),
                entry: None,
            };
        }
        KeyCode::Char('x') if phone_key_has_effect(&status.state, key.code) => {
            let resp = app.client().and_then(|c| c.call(Request::PhoneDisable));
            app.view = View::Phone {
                status: apply_phone_response(status, resp),
                entry: None,
            };
        }
        _ => {
            app.view = View::Phone {
                status,
                entry: None,
            };
        }
    }
}

/// `Request::PhoneUnpair`/`PhoneDisable` 的回答落地。拿不到新状态（断线、
/// 守护进程太旧）就留在原地——同这个仓库别的地方的约定，陈旧数据好过
/// 凭空捏造一个新状态。
fn apply_phone_response(fallback: PhoneStatus, resp: Result<Response>) -> PhoneStatus {
    match resp {
        Ok(Response::Phone(status)) => status,
        _ => fallback,
    }
}

/// **这个函数里永远不要 `continue`。** 同上。
fn handle_entry(app: &mut App, key: KeyEvent, status: PhoneStatus, entry: PhoneEntry) {
    let PhoneEntry { mut buf, phase } = entry;
    match phase {
        SecretPhase::Verifying => {
            // 验证在后台线程跑，buf 已经发出去了，这期间敲字符/回车都改不了
            // 那次正在飞的请求——只留 Esc，且必须现在就扔掉 phone_verify_rx，
            // 不然迟到的结果会套在一个用户已经不认得的视图上（同
            // `secret.rs::handle_enter_secret` 的 `Verifying` 分支）。
            if key.code == KeyCode::Esc {
                app.phone_verify_rx = None;
                app.view = View::Phone {
                    status,
                    entry: None,
                };
            } else {
                app.view = View::Phone {
                    status,
                    entry: Some(PhoneEntry {
                        buf,
                        phase: SecretPhase::Verifying,
                    }),
                };
            }
        }
        SecretPhase::Typing | SecretPhase::Failed(_) => match key.code {
            // 取消这次输入，回到状态页——不是回设置，用户可能只是手滑
            // 按错了 Enter，退两层找不到刚才在看的那行状态。
            KeyCode::Esc => {
                app.view = View::Phone {
                    status,
                    entry: None,
                };
            }
            KeyCode::Enter => {
                let (tx, rx) = std::sync::mpsc::channel();
                let sock = socket_path();
                let token = buf.clone();
                let lang = app.lang;
                let stamped_token = token.clone();
                std::thread::spawn(move || {
                    let outcome: std::result::Result<PhoneStatus, String> = Client::connect(&sock)
                        .and_then(|mut c| c.call(Request::PhoneSetToken { token, lang }))
                        .map_err(|_| {
                            crate::i18n::text(crate::i18n::Key::DaemonUnreachable, lang).to_string()
                        })
                        .and_then(|r| match r {
                            Response::Phone(status) => Ok(status),
                            Response::Error(ref e) => Err(crate::i18n::msg::error(lang, e)),
                            _ => Err(crate::i18n::text(crate::i18n::Key::RequestFailed, lang)
                                .to_string()),
                        });
                    let _ = tx.send((stamped_token, outcome));
                });
                app.phone_verify_rx = Some(rx);
                app.view = View::Phone {
                    status,
                    entry: Some(PhoneEntry {
                        buf,
                        phase: SecretPhase::Verifying,
                    }),
                };
            }
            KeyCode::Backspace => {
                buf.pop();
                app.view = View::Phone {
                    status,
                    entry: Some(PhoneEntry {
                        buf,
                        phase: SecretPhase::Typing,
                    }),
                };
            }
            KeyCode::Char(c) => {
                buf.push(c);
                app.view = View::Phone {
                    status,
                    entry: Some(PhoneEntry {
                        buf,
                        phase: SecretPhase::Typing,
                    }),
                };
            }
            _ => {
                app.view = View::Phone {
                    status,
                    entry: Some(PhoneEntry { buf, phase }),
                };
            }
        },
    }
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let View::Phone { status, entry } = &app.view else {
        return;
    };
    let border_style = if app.connected {
        Style::default()
    } else {
        Style::default().fg(Color::Red)
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(status_line(status, app.lang)));
    if let Some(step) = next_step(status, app.lang) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(step, dim())));
    }
    // 详情：`Broken` 里那句守护进程写好的人话，画在单独一行——`status_line`/
    // `next_step` 故意不读它（见模块头注释），但完全不展示的话这个字符串
    // 就白存了，用户也没法知道更具体的原因。
    if let PhoneState::Broken { message, .. } = &status.state {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            message.clone(),
            Style::default().fg(Color::Red),
        )));
    }

    if let Some(PhoneEntry { buf, phase }) = entry {
        lines.push(Line::from(""));
        // 显示成圆点：令牌不该以明文停在屏幕上，用户可能在录屏或在办公室。
        lines.push(Line::from(format!("{}▌", "•".repeat(buf.chars().count()))));
        match phase {
            SecretPhase::Typing => {}
            SecretPhase::Verifying => {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    text(Key::VerifyingShort, app.lang),
                    Style::default().fg(Color::Cyan),
                )));
            }
            SecretPhase::Failed(m) => {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    m.clone(),
                    Style::default().fg(Color::Red),
                )));
            }
        }
    }

    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(text(Key::Phone, app.lang)),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::super::view::SettingsItem;
    use super::*;

    /// 这一页存在的全部理由是那一行状态。四种取值，**每一种都要带下一步**——
    /// 一个不告诉用户下一步该干什么的错误，按房规就是没写完。
    #[test]
    fn every_state_tells_the_user_what_to_do_next() {
        for st in [
            PhoneState::Off,
            PhoneState::WaitingForPairing,
            PhoneState::Paired,
            PhoneState::Broken {
                reason: PhoneBrokenReason::BadToken,
                message: "token revoked".into(),
            },
        ] {
            let s = status_line(
                &PhoneStatus {
                    state: st.clone(),
                    bot: Some("my_bot".into()),
                    owner: None,
                },
                Lang::Zh,
            );
            assert!(!s.is_empty(), "{st:?} 没有状态文案");
        }
        // 「已连上」是唯一不需要下一步的：它就是终点。其余三种都必须给出路。
        for st in [
            PhoneState::Off,
            PhoneState::WaitingForPairing,
            PhoneState::Broken {
                reason: PhoneBrokenReason::BadToken,
                message: "token revoked".into(),
            },
        ] {
            let s = next_step(
                &PhoneStatus {
                    state: st.clone(),
                    bot: Some("my_bot".into()),
                    owner: None,
                },
                Lang::Zh,
            );
            assert!(s.is_some(), "{st:?} 没有给出下一步");
        }
        assert!(next_step(
            &PhoneStatus {
                state: PhoneState::Paired,
                bot: Some("my_bot".into()),
                owner: Some("lei".into())
            },
            Lang::Zh
        )
        .is_none());
    }

    /// 等配对时必须把 bot 名字说出来，否则「去给它发条消息」是句没法执行的话。
    #[test]
    fn waiting_names_the_bot() {
        let s = status_line(
            &PhoneStatus {
                state: PhoneState::WaitingForPairing,
                bot: Some("my_dct_bot".into()),
                owner: None,
            },
            Lang::Zh,
        );
        assert!(s.contains("my_dct_bot"), "等配对却没说是哪个 bot：{s}");
    }

    /// **fix round 2 的 Important 1。** 三个 `Broken` 原因说的是三句不同的
    /// 下一步——这是这一整轮修复的意义所在（`PhoneBrokenReason` 存在的
    /// 唯一理由）：`BadToken` 该重填，`BotBlocked` 该去解除拉黑，
    /// `Unreachable` 该等一等再试，把它们说成同一句话会让「离线但令牌
    /// 完好的用户被要求重填一份好端端的令牌」这个原始缺陷在测试全绿的
    /// 情况下悄悄回来——`every_state_tells_the_user_what_to_do_next` 只
    /// 断言 `next_step` 非空，三个原因全部折叠回同一句 `PhoneNextStepBadToken`
    /// 照样通过。这条测试直接比对三句话本身，两两不同（验证过：把
    /// `next_step` 的三个 `Broken` 分支都改成 `PhoneNextStepBadToken`，
    /// 这条测试红，`every_state_tells_the_user_what_to_do_next` 仍然绿）。
    #[test]
    fn the_three_broken_reasons_give_three_different_next_steps() {
        let step_for = |reason| {
            next_step(
                &PhoneStatus {
                    state: PhoneState::Broken {
                        reason,
                        message: "x".into(),
                    },
                    bot: None,
                    owner: None,
                },
                Lang::Zh,
            )
            .expect("Broken 必须给下一步")
        };
        let bad_token = step_for(PhoneBrokenReason::BadToken);
        let bot_blocked = step_for(PhoneBrokenReason::BotBlocked);
        let unreachable = step_for(PhoneBrokenReason::Unreachable);

        assert_ne!(bad_token, bot_blocked, "令牌失效和被拉黑不该说同一句下一步");
        assert_ne!(bad_token, unreachable, "令牌失效和连不上不该说同一句下一步");
        assert_ne!(bot_blocked, unreachable, "被拉黑和连不上不该说同一句下一步");
    }

    /// 令牌是密钥。**任何一处状态文案都不许把它带出来。**
    #[test]
    fn the_token_never_appears_in_any_status_text() {
        let st = PhoneStatus {
            state: PhoneState::Broken {
                reason: PhoneBrokenReason::BadToken,
                message: "123456:AAH-SECRET".into(),
            },
            bot: None,
            owner: None,
        };
        let s = format!(
            "{}{}",
            status_line(&st, Lang::Zh),
            next_step(&st, Lang::Zh).unwrap_or_default()
        );
        assert!(!s.contains("AAH-SECRET"), "令牌漏进了界面文案：{s}");
    }

    // ———— 补充测试：跟 status_line/next_step 同样重要,但不在 brief 的
    // Step 1 清单里——覆盖 handle_key 这一半 ————

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn off_status() -> PhoneStatus {
        PhoneStatus {
            state: PhoneState::Off,
            bot: None,
            owner: None,
        }
    }

    fn waiting_status() -> PhoneStatus {
        PhoneStatus {
            state: PhoneState::WaitingForPairing,
            bot: Some("my_dct_bot".into()),
            owner: None,
        }
    }

    /// `Off` 状态按 Enter 要能打开令牌输入框。
    #[test]
    fn enter_on_off_opens_the_token_entry() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Phone {
            status: off_status(),
            entry: None,
        };
        handle_key(&mut app, key(KeyCode::Enter)).unwrap();
        match &app.view {
            View::Phone {
                entry: Some(PhoneEntry { buf, phase }),
                ..
            } => {
                assert!(buf.is_empty());
                assert!(matches!(phase, SecretPhase::Typing));
            }
            _ => panic!("Enter 应该打开令牌输入框"),
        }
    }

    /// `Broken`（令牌坏了）按 Enter **也要**能打开输入框——这是修复它的
    /// 唯一路径。只测 `Off` 会漏掉这一半：`phone_key_has_effect` 是
    /// `Off | Broken(_)` 两个条件的析取，只覆盖 `Off` 分不出「漏了 Broken」
    /// 和「条件写对了」。
    #[test]
    fn enter_on_broken_also_opens_the_token_entry() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Phone {
            status: PhoneStatus {
                state: PhoneState::Broken {
                    reason: PhoneBrokenReason::BadToken,
                    message: "token revoked".into(),
                },
                bot: None,
                owner: None,
            },
            entry: None,
        };
        handle_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(
            matches!(app.view, View::Phone { entry: Some(_), .. }),
            "Broken 状态下 Enter 也该打开输入框——重新填令牌是修复它的路"
        );
    }

    /// `WaitingForPairing`（已经有令牌）按 Enter 不该做任何事——已经填过了。
    #[test]
    fn enter_while_waiting_does_not_reopen_the_entry() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Phone {
            status: waiting_status(),
            entry: None,
        };
        handle_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(
            matches!(app.view, View::Phone { entry: None, .. }),
            "已经有令牌时 Enter 不该打开输入框"
        );
    }

    /// 输入框里打字、退格都要落到 buf 上，并且停在 Typing 阶段。
    #[test]
    fn typing_and_backspace_edit_the_buffer() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Phone {
            status: off_status(),
            entry: Some(PhoneEntry {
                buf: String::new(),
                phase: SecretPhase::Typing,
            }),
        };
        handle_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        handle_key(&mut app, key(KeyCode::Char('b'))).unwrap();
        match &app.view {
            View::Phone {
                entry: Some(PhoneEntry { buf, .. }),
                ..
            } => assert_eq!(buf, "ab"),
            _ => panic!(),
        }
        handle_key(&mut app, key(KeyCode::Backspace)).unwrap();
        match &app.view {
            View::Phone {
                entry: Some(PhoneEntry { buf, .. }),
                ..
            } => assert_eq!(buf, "a"),
            _ => panic!(),
        }
    }

    /// Esc 在输入框里取消这次输入，退回状态页（不是设置页），而且要保留
    /// 之前的状态——不能因为取消了一次输入,连状态本身都被冲掉。
    #[test]
    fn esc_while_typing_cancels_back_to_the_status_page() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Phone {
            status: waiting_status(),
            entry: Some(PhoneEntry {
                buf: "partial".into(),
                phase: SecretPhase::Typing,
            }),
        };
        handle_key(&mut app, key(KeyCode::Esc)).unwrap();
        match &app.view {
            View::Phone {
                status,
                entry: None,
            } => {
                assert_eq!(status.state, PhoneState::WaitingForPairing);
            }
            _ => panic!("Esc 应该回到状态页，entry 清空"),
        }
    }

    /// Esc 在状态页（没有 entry）时回设置页，光标停在「手机通知」这一行。
    #[test]
    fn esc_on_the_status_page_goes_back_to_settings_on_the_phone_row() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Phone {
            status: off_status(),
            entry: None,
        };
        handle_key(&mut app, key(KeyCode::Esc)).unwrap();
        match &app.view {
            View::Settings { state, lang: None } => {
                assert_eq!(
                    state.selected().and_then(SettingsItem::at),
                    Some(SettingsItem::Phone)
                );
            }
            _ => panic!("Esc 应该回到设置页顶层列表"),
        }
    }

    /// Verifying 期间敲字符不能改动 buf——那次验证已经把当时的 buf 发出去
    /// 了，改了只会让用户误以为下一次回车用的是新值。
    #[test]
    fn typing_during_verifying_is_frozen() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Phone {
            status: off_status(),
            entry: Some(PhoneEntry {
                buf: "sent".into(),
                phase: SecretPhase::Verifying,
            }),
        };
        handle_key(&mut app, key(KeyCode::Char('x'))).unwrap();
        match &app.view {
            View::Phone {
                entry: Some(PhoneEntry { buf, phase }),
                ..
            } => {
                assert_eq!(buf, "sent", "Verifying 期间 buf 不能被改动");
                assert!(matches!(phase, SecretPhase::Verifying));
            }
            _ => panic!(),
        }
    }

    /// Esc 在 Verifying 期间要清掉 `phone_verify_rx`——不然迟到的结果会套在
    /// 一个用户已经不认得的视图上。
    #[test]
    fn esc_during_verifying_drops_the_pending_receiver() {
        let (mut app, _dir) = App::test_app();
        let (_tx, rx) = std::sync::mpsc::channel();
        app.phone_verify_rx = Some(rx);
        app.view = View::Phone {
            status: off_status(),
            entry: Some(PhoneEntry {
                buf: "sent".into(),
                phase: SecretPhase::Verifying,
            }),
        };
        handle_key(&mut app, key(KeyCode::Esc)).unwrap();
        assert!(app.phone_verify_rx.is_none());
        assert!(matches!(app.view, View::Phone { entry: None, .. }));
    }

    /// `r`/`x` 在 `Off` 状态下没有对象可作用，必须是空操作——不能凭空冒出
    /// 一次 `PhoneUnpair`/`PhoneDisable` 请求（`app` 没连守护进程,真打了
    /// 请求这条测试会因为 `Result::Err` 走进 fallback 分支,状态原地不动;
    /// 但更要紧的是这两个键在 `Off` 下压根不该被广告出来，见
    /// `view.rs::idle_help`）。
    #[test]
    fn r_and_x_on_off_are_no_ops() {
        for code in [KeyCode::Char('r'), KeyCode::Char('x')] {
            let (mut app, _dir) = App::test_app();
            app.view = View::Phone {
                status: off_status(),
                entry: None,
            };
            handle_key(&mut app, key(code)).unwrap();
            assert!(
                matches!(app.view, View::Phone { status, entry: None } if status.state == PhoneState::Off),
                "{code:?} 在 Off 状态下不该有任何效果"
            );
        }
    }

    /// 画一帧不能 panic，且令牌不能明文出现在屏幕上。
    #[test]
    fn draw_does_not_panic_and_masks_the_token() {
        let mut term = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.view = View::Phone {
            status: off_status(),
            entry: Some(PhoneEntry {
                buf: "123456:AAH-SECRET".into(),
                phase: SecretPhase::Typing,
            }),
        };
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect();
        assert!(
            !text.contains("AAH-SECRET"),
            "令牌不能以明文出现在屏幕上：{text}"
        );
    }

    /// `Broken` 的详情行画出来了（这是它唯一真正显示的地方，见模块头注释），
    /// 但同样不能泄露一个看着像令牌的字符串——这里特意造一个诊断消息本身
    /// 就很短的场景，只确认「文字确实上屏」，令牌安全性由上面那条守。
    #[test]
    fn draw_shows_the_broken_reason_as_a_detail_line() {
        let mut term = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.view = View::Phone {
            status: PhoneStatus {
                state: PhoneState::Broken {
                    reason: PhoneBrokenReason::BadToken,
                    message: "这个令牌用不了，重新输入一遍".into(),
                },
                bot: None,
                owner: None,
            },
            entry: None,
        };
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let buf = term.backend().buffer();
        // ratatui 画宽字符只写首格、第二格留空/留一个空白 —— 逐格拼起来的
        // 汉字之间会夹着这些空白，直接 `contains` 一段汉字子串会假失败，
        // 见 `settings_view.rs` 里同类测试上的注释。两边都去掉空白再比。
        let text: String = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let expect: String = "这个令牌用不了"
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(text.contains(&expect), "Broken 的详情要真的画出来：{text}");
    }
}
