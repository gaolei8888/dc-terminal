//! 手机通知设置页。设置页选中「Phone」进。
//!
//! **这一页存在的全部理由是那一行状态。** 配对是异步的——守护进程一直
//! 轮询，直到用户在 Telegram 里给 bot 发第一条消息——没有这一页，用户
//! 填完令牌就是在对着一屏「什么都没发生」发呆。四种状态，`Paired` 之外
//! 每一种都要带下一步（见下面 `status_line`/`next_step` 头上的注释）。
//!
//! 「正在打字」和「验证中」两个临时态存在 `App`（`phone_buf`/
//! `phone_verify_rx`）而不是 `View::Phone` 里，理由见 `View::Phone` 的
//! 文档注释：`View` 要整体 `Clone`，装着后台线程结果的 `Receiver` 进不去。

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::i18n::{msg, text, Key, Lang};
use crate::proto::{PhoneState, PhoneStatus, Request, Response};

use super::app::App;
use super::view::{clean_secret, is_plain_key, View};

/// 状态行：**四种取值，每一种都要给用户看得懂的一句话**。
///
/// `PhoneState::Broken` 的内容**从不**被读取——那个字符串按约定应该是
/// 「已经成文的人话」，但这个函数不信任那份约定：万一某处写值的代码手滑
/// 塞进了原始错误甚至令牌本身，这里也绝不会把它带上屏幕（见
/// `PhoneState::Broken` 的文档注释和 `the_token_never_appears_in_any_status_text`）。
pub(crate) fn status_line(status: &PhoneStatus, lang: Lang) -> String {
    match &status.state {
        PhoneState::Off => text(Key::PhoneOffLine, lang).to_string(),
        // **必须点名 bot**，否则「去发条消息」是一句没法执行的话
        // （见 `waiting_names_the_bot`）。没有名字**会**在生产里真的发生：
        // 守护进程重启时，只要密钥仓里已经有令牌就直接进这个状态
        // （`daemon::initial_phone_status`），但 bot 用户名要等 Task 5 的
        // bridge 线程真的跑起来、重新打一次 `getMe` 才补得上。这不是
        // `Off`——令牌还在，配对多半也还在——所以绝不能借用 `PhoneOffLine`
        // （那曾经是这里的写法，被判为一个可复现的生产缺陷：重启完打开
        // 这一页会显示「手机通知还没打开」，而它根本没关）。这里给一句
        // 诚实的「正在重新接上」，不点名、也不假装点名。
        PhoneState::WaitingForPairing => match &status.bot {
            Some(bot) => msg::phone_waiting_for_pairing(lang, bot),
            None => text(Key::PhoneReconnectingLine, lang).to_string(),
        },
        PhoneState::Paired => match &status.owner {
            Some(owner) => msg::phone_paired(lang, owner),
            None => text(Key::PhonePairedLine, lang).to_string(),
        },
        PhoneState::Broken(_) => text(Key::PhoneBrokenLine, lang).to_string(),
    }
}

/// 下一步。**`Paired` 是唯一不需要下一步的**——它是终点，其余三种都必须
/// 给出路，一个不告诉用户下一步该干什么的错误按房规就是没写完。
pub(crate) fn next_step(status: &PhoneStatus, lang: Lang) -> Option<String> {
    match &status.state {
        PhoneState::Off => Some(text(Key::PhoneNextStepOff, lang).to_string()),
        // 没有 bot 名字时不能叫用户「去给它发条消息」——那正是
        // `waiting_names_the_bot` 要防的事，这里给的是一个不点名也站得住
        // 的下一步：等一下。
        PhoneState::WaitingForPairing => Some(
            match &status.bot {
                Some(_) => text(Key::PhoneNextStepWaiting, lang),
                None => text(Key::PhoneNextStepReconnecting, lang),
            }
            .to_string(),
        ),
        PhoneState::Paired => None,
        PhoneState::Broken(_) => Some(text(Key::PhoneNextStepBroken, lang).to_string()),
    }
}

/// **这个函数里永远不要 `continue`。** 理由同 `settings_view.rs`/`board.rs`：
/// 循环末尾还有一段清理陈旧 `message` 的逻辑，跳过它会让一句普通反馈盖掉
/// 屏幕上唯一的出路。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Phone { status } = app.view.clone() else {
        return Ok(());
    };

    if app.phone_verify_rx.is_some() {
        // 验证在后台线程跑，敲字符/回车都改不了那次正在飞的请求，只留 Esc：
        // 想退就现在退，且必须现在就扔掉 receiver——不然迟到的结果会套在
        // 一个用户已经不认得的视图上（同 `secret.rs` 的 `SecretPhase::Verifying`）。
        if key.code == KeyCode::Esc {
            app.phone_verify_rx = None;
            app.phone_buf = None;
        }
        return Ok(());
    }

    if let Some(buf) = app.phone_buf.clone() {
        return handle_typing(app, key, buf);
    }

    match key.code {
        KeyCode::Esc => {
            app.view = View::Settings {
                state: ratatui::widgets::ListState::default(),
                sub: None,
            }
        }
        KeyCode::Enter
            if matches!(status.state, PhoneState::Off | PhoneState::Broken(_))
                && is_plain_key(&key) =>
        {
            app.phone_buf = Some(String::new());
        }
        // 换一台手机：忘掉当前主人，退回等配对，令牌不动。
        KeyCode::Char('r') if matches!(status.state, PhoneState::Paired) && is_plain_key(&key) => {
            app.view = View::Phone {
                status: send_phone_request(app, Request::PhoneUnpair, status),
            };
        }
        // 整个关掉：只要不是已经 Off，就有东西可关。
        KeyCode::Char('x') if !matches!(status.state, PhoneState::Off) && is_plain_key(&key) => {
            app.view = View::Phone {
                status: send_phone_request(app, Request::PhoneDisable, status),
            };
        }
        _ => {}
    }
    Ok(())
}

/// `handle_key` 里的两个同步请求（`PhoneUnpair`/`PhoneDisable`）共用的
/// 一小段：都不碰网络，只是本地读写状态槽/密钥仓，堵在按键循环里没问题
/// ——跟 `PhoneSetToken` 不一样，那条要打真的 Telegram 网络，必须走
/// 后台线程（见 `submit_token`）。
fn send_phone_request(app: &mut App, req: Request, fallback: PhoneStatus) -> PhoneStatus {
    match app.client().and_then(|c| c.call(req)) {
        Ok(Response::Phone(status)) => status,
        _ => fallback,
    }
}

/// **这个函数里永远不要 `continue`。** 理由同 `handle_key`。
fn handle_typing(app: &mut App, key: KeyEvent, mut buf: String) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.phone_buf = None,
        KeyCode::Enter => submit_token(app, buf),
        KeyCode::Backspace => {
            buf.pop();
            app.phone_buf = Some(buf);
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            buf.push(c);
            app.phone_buf = Some(clean_secret(&buf));
        }
        _ => app.phone_buf = Some(buf),
    }
    Ok(())
}

/// 发起验证：打真网络（Telegram 的 `getMe`），必须丢给后台线程，
/// 不能堵在按键循环里——同 `secret.rs` 里 `Enter` 那支的道理。
fn submit_token(app: &mut App, token: String) {
    let (tx, rx) = std::sync::mpsc::channel();
    let sock = crate::proto::socket_path();
    std::thread::spawn(move || {
        let outcome = crate::client::Client::connect(&sock)
            .and_then(|mut c| c.call(Request::PhoneSetToken { token }))
            .map(|r| match r {
                Response::Phone(status) => status,
                _ => PhoneStatus {
                    state: PhoneState::Broken(String::new()),
                    bot: None,
                    owner: None,
                },
            })
            .unwrap_or(PhoneStatus {
                state: PhoneState::Broken(String::new()),
                bot: None,
                owner: None,
            });
        let _ = tx.send(outcome);
    });
    app.phone_verify_rx = Some(rx);
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let View::Phone { status } = &app.view else {
        return;
    };
    let border_style = if app.connected {
        Style::default()
    } else {
        Style::default().fg(Color::Red)
    };
    let title = format!(
        "{} · {}",
        text(Key::SettingsTitle, app.lang),
        text(Key::Phone, app.lang)
    );

    let mut lines: Vec<Line> = Vec::new();
    if app.phone_verify_rx.is_some() {
        lines.push(Line::from(Span::styled(
            text(Key::VerifyingShort, app.lang),
            Style::default().fg(Color::Cyan),
        )));
    } else if let Some(buf) = &app.phone_buf {
        lines.push(Line::from(Span::styled(
            text(Key::PhonePasteToken, app.lang),
            super::dim(),
        )));
        lines.push(Line::from(""));
        // 显示成圆点：令牌不该以明文停在屏幕上，理由同 `secret.rs`。
        lines.push(Line::from(format!("{}▌", "•".repeat(buf.chars().count()))));
    } else {
        lines.push(Line::from(status_line(status, app.lang)));
        if let Some(step) = next_step(status, app.lang) {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(step, super::dim())));
        }
    }

    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::TOP | Borders::BOTTOM)
                .border_style(border_style)
                .title(title),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这一页存在的全部理由是那一行状态。四种取值，**每一种都要带下一步**——
    /// 一个不告诉用户下一步该干什么的错误，按房规就是没写完。
    #[test]
    fn every_state_tells_the_user_what_to_do_next() {
        for st in [
            PhoneState::Off,
            PhoneState::WaitingForPairing,
            PhoneState::Paired,
            PhoneState::Broken("token revoked".into()),
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
            PhoneState::Broken("token revoked".into()),
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

    /// 生产里真的会撞到的一格：守护进程重启，令牌还在（`WaitingForPairing`），
    /// 但 bot 用户名还没等到 bridge 重新查回来（`bot: None`）——这时候
    /// **既不能**说「手机通知还没打开」（令牌没丢，这不是 `Off`），
    /// **也不能**叫用户去给一个没有名字的 bot 发消息（`waiting_names_the_bot`
    /// 防的就是这个）。
    #[test]
    fn waiting_without_a_bot_name_is_neither_off_nor_a_dangling_instruction() {
        let st = PhoneStatus {
            state: PhoneState::WaitingForPairing,
            bot: None,
            owner: None,
        };
        let line = status_line(&st, Lang::Zh);
        assert_ne!(
            line,
            text(Key::PhoneOffLine, Lang::Zh),
            "令牌还在，不该说成关着的：{line}"
        );
        let step = next_step(&st, Lang::Zh).expect("这个状态仍然要给下一步");
        assert!(
            !step.contains('@') && !step.to_lowercase().contains("bot"),
            "没有名字就不能叫用户去找某个 bot：{step}"
        );
    }

    /// 令牌是密钥。**任何一处状态文案都不许把它带出来。**
    #[test]
    fn the_token_never_appears_in_any_status_text() {
        let st = PhoneStatus {
            state: PhoneState::Broken("123456:AAH-SECRET".into()),
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

    /// 两种语言都要覆盖到——`text()`/`msg::` 少翻一种语言，这条测试就会
    /// 拿到一句空字符串或者跟中文一模一样的英文。
    #[test]
    fn both_languages_produce_non_empty_status_lines() {
        for lang in Lang::all() {
            for st in [
                PhoneState::Off,
                PhoneState::WaitingForPairing,
                PhoneState::Paired,
                PhoneState::Broken("x".into()),
            ] {
                let s = status_line(
                    &PhoneStatus {
                        state: st,
                        bot: Some("bot".into()),
                        owner: Some("lei".into()),
                    },
                    *lang,
                );
                assert!(!s.is_empty());
            }
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn phone_view(state: PhoneState) -> View {
        View::Phone {
            status: PhoneStatus {
                state,
                bot: Some("my_dct_bot".into()),
                owner: None,
            },
        }
    }

    /// Esc 从手机页退出去，回到的是设置页——这一页唯一的来路就是设置页。
    #[test]
    fn escape_goes_back_to_settings() {
        let (mut app, _dir) = App::test_app();
        app.view = phone_view(PhoneState::Off);

        handle_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert!(matches!(app.view, View::Settings { .. }));
    }

    /// `Off` 状态下按 Enter 该开始打字，`phone_buf` 从 `None` 变成 `Some("")`。
    #[test]
    fn enter_on_off_starts_typing() {
        let (mut app, _dir) = App::test_app();
        app.view = phone_view(PhoneState::Off);

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        assert_eq!(app.phone_buf, Some(String::new()));
    }

    /// `WaitingForPairing` 下按 Enter 什么都不该发生——已经填过令牌了，
    /// 这个键在这个状态下没有意义。
    #[test]
    fn enter_on_waiting_does_nothing() {
        let (mut app, _dir) = App::test_app();
        app.view = phone_view(PhoneState::WaitingForPairing);

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        assert_eq!(app.phone_buf, None);
    }

    /// 打字态下敲字符要真的进 `phone_buf`，Esc 要能整个取消（回到「没在
    /// 打字」，不是清空成空字符串——那样再按 Esc 一次才会真的退出手机页，
    /// 用户会以为 Esc 没反应）。
    #[test]
    fn typing_accumulates_and_escape_cancels() {
        let (mut app, _dir) = App::test_app();
        app.view = phone_view(PhoneState::Off);
        app.phone_buf = Some(String::new());

        handle_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        handle_key(&mut app, key(KeyCode::Char('b'))).unwrap();
        assert_eq!(app.phone_buf.as_deref(), Some("ab"));

        handle_key(&mut app, key(KeyCode::Esc)).unwrap();
        assert_eq!(app.phone_buf, None, "Esc 要整个取消打字，不是清空成空串");
        assert!(
            matches!(app.view, View::Phone { .. }),
            "取消打字回到手机页本身，不是设置页——那是另一层 Esc"
        );
    }

    /// 验证中只认 Esc；别的键不该改动任何东西。
    #[test]
    fn verifying_ignores_everything_but_escape() {
        let (mut app, _dir) = App::test_app();
        app.view = phone_view(PhoneState::Off);
        let (_tx, rx) = std::sync::mpsc::channel();
        app.phone_verify_rx = Some(rx);
        app.phone_buf = Some("123456:tok".into());

        handle_key(&mut app, key(KeyCode::Char('x'))).unwrap();
        assert!(app.phone_verify_rx.is_some(), "验证中，别的键不该打断它");

        handle_key(&mut app, key(KeyCode::Esc)).unwrap();
        assert!(app.phone_verify_rx.is_none(), "Esc 要能取消正在飞的验证");
        assert!(app.phone_buf.is_none());
    }
}
