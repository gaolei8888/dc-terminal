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
use ratatui::style::Modifier;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::i18n::{msg, text, Key, Lang};
use crate::proto::{PhoneState, PhoneStatus, Request, Response, WebInfo};

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
        // 局域网手机端的开关。**放在打字分支之后**（上面那个 `phone_buf`
        // 的早退）——令牌里也可能有 `w`，先看开关的话用户打到一半会莫名
        // 其妙地把口子打开，见 `w_while_typing_a_token_is_just_a_letter`。
        KeyCode::Char('w') if is_plain_key(&key) => {
            app.web = send_web_request(app, toggle_request(&app.web));
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

/// `w` 那一支用的：发一条局域网开关请求，拿回新状态。
///
/// **失败就原样留着旧状态**，跟 `send_phone_request` 同一个取舍：守护进程
/// 一时连不上不该让界面上那一节凭空翻转成另一个样子——屏幕上写着「开着」
/// 而实际没开，比一个按了没反应的键坏得多。连不上这件事本身有整条边框
/// 变红在说（`app.connected`）。
fn send_web_request(app: &mut App, req: Request) -> WebInfo {
    match app.client().and_then(|c| c.call(req)) {
        Ok(Response::Web(info)) => info,
        _ => app.web.clone(),
    }
}

/// 按 `w` 该发哪一条请求：关着就开，开着就关。
///
/// **看 `on`，不看 `url`。** 「开着但算不出局域网地址」那一格 `url` 是
/// `None`，照 `url` 判的话按 `w` 会再发一次 `WebEnable`，而 `web_enable`
/// 看见已经在跑就原样返回——于是这个开关在最需要它的那一格里变成一个
/// 按了没反应的键。
pub(crate) fn toggle_request(info: &WebInfo) -> Request {
    if info.on {
        Request::WebDisable
    } else {
        Request::WebEnable
    }
}

/// 局域网手机端的状态行。三种取值：关着、开着且有地址、开着但算不出地址。
///
/// **「开着但算不出地址」不能借用「开着」那一句**：那一句后面跟着一个
/// 地址和一块码，而这一种情况两样都没有——照着说会让用户去找一个屏幕上
/// 根本不存在的东西（同 `status_line` 里 `WaitingForPairing` 没有 bot 名字
/// 那一格的道理）。
pub(crate) fn web_status_line(info: &WebInfo, lang: Lang) -> String {
    if !info.on {
        return text(Key::WebOffLine, lang).to_string();
    }
    if info.address_unknown || info.url.is_none() {
        return text(Key::WebAddressUnknownLine, lang).to_string();
    }
    text(Key::WebOnLine, lang).to_string()
}

/// 下一步。**三种状态一个都不能少**——包括「开着」。这跟上面那一节
/// 不一样：Telegram 那边 `Paired` 是终点，而这里「开着」不是，它下面还有
/// 一个码要扫、一个开关要关，用户不问一声是不知道怎么关的。
pub(crate) fn web_next_step(info: &WebInfo, lang: Lang) -> Option<String> {
    let key = if !info.on {
        Key::WebNextStepOff
    } else if info.address_unknown || info.url.is_none() {
        Key::WebNextStepAddressUnknown
    } else {
        Key::WebNextStepOn
    };
    Some(text(key, lang).to_string())
}

/// 屏幕上要显示的地址：**把 `#t=…` 那一截剪掉**。
///
/// 二维码里必须带令牌（那是给摄像头的，也是这个功能唯一的入口），但写成
/// 字的那一行只能到端口为止：屏幕会被拍照、被投影、被录进屏幕录像，而
/// 一个看过这行字的人就能往你的终端里敲字。同 `status_line` 那条
/// 「令牌绝不上屏」的规矩，`the_token_never_appears_in_the_address_on_screen`
/// 钉的就是这一条。
pub(crate) fn address_for_display(url: &str) -> &str {
    match url.find('#') {
        Some(i) => &url[..i],
        None => url,
    }
}

/// 把地址画成一片二维码行。**宽度不够就返回 `None`**。
///
/// 画一块缺了边的码不是"尽力而为"，是让用户对着一个永远扫不出来的东西
/// 试半天——那时候该退回文字（`Key::WebQrTooNarrow` 那一句两条出路都写了）。
/// 编码的是**带令牌的完整地址**：手机扫到的必须是能直接打开的链接。
pub(crate) fn qr_lines(url: &str, width: u16) -> Option<Vec<Line<'static>>> {
    let art = crate::qr::render(url)?;
    if art.cols > width as usize {
        return None;
    }
    Some(
        art.rows
            .iter()
            .map(|row| {
                Line::from(
                    row.iter()
                        .map(|cell| {
                            // `▀` 的前景是上半格、背景是下半格——一个字符格
                            // 装上下两个模块，码才不会被拉成长方形（见 `qr.rs`）。
                            Span::styled("\u{2580}", Style::default().fg(cell.top).bg(cell.bottom))
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect(),
    )
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

    // —— 局域网手机端 ——
    //
    // 跟上面那一节挨着，但**中间必须有一条空行加一个标题**：两件事同名
    // 「手机」，一个是 Telegram 推消息、一个是同一个 WiFi 下打开的网页，
    // 挨着放而不分开写，用户会以为下面那块码是拿去配 Telegram 的。
    //
    // 正在打字/正在验证的时候不画：那两个临时态占着整屏，而这一节在那时候
    // 只会把人的注意力从"手里正在填的令牌"上引开。
    if app.phone_verify_rx.is_none() && app.phone_buf.is_none() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            text(Key::WebSection, app.lang),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(web_status_line(&app.web, app.lang)));
        if let Some(step) = web_next_step(&app.web, app.lang) {
            lines.push(Line::from(Span::styled(step, super::dim())));
        }
        if let Some(url) = &app.web.url {
            lines.push(Line::from(""));
            // 屏幕上写到端口为止，令牌只进码里（`address_for_display`）。
            lines.push(Line::from(address_for_display(url).to_string()));
            lines.push(Line::from(""));
            match qr_lines(url, area.width) {
                Some(qr) => lines.extend(qr),
                // 画不下就换成话，不留半块码：见 `qr_lines`。
                None => lines.push(Line::from(Span::styled(
                    text(Key::WebQrTooNarrow, app.lang),
                    super::dim(),
                ))),
            }
        }
    }

    f.render_widget(
        Paragraph::new(lines)
            // **必须折行。** 这一页上有两句话是「少一个字就没法用」的：
            // 窄窗口那句要说完两条出路（拉宽窗口／照着地址手输），
            // Telegram 那句要说完去哪儿粘贴令牌。不折行的话它们在窄窗口下
            // 被从中间切掉，留在屏幕上的是半句没法执行的指令。
            //
            // 二维码那几行不受影响：`qr_lines` 已经保证画出来的码不宽于
            // 这块区域，宽度没超就没有什么可折的。
            .wrap(Wrap { trim: false })
            .block(
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

    /// 这一小节跟上面那一节是一个道理：**每一种状态都要带下一步**。
    /// 「开着」在这里**不是**终点——它下面还有一个码要扫、一个开关要关，
    /// 所以三种状态一个都不能少（这跟 Telegram 那节的 `Paired` 不一样）。
    #[test]
    fn every_web_state_tells_the_user_what_to_do_next() {
        for info in [
            WebInfo {
                on: false,
                url: None,
                address_unknown: false,
            },
            WebInfo {
                on: true,
                url: Some("http://192.168.1.5:53412/#t=abc".into()),
                address_unknown: false,
            },
            WebInfo {
                on: true,
                url: None,
                address_unknown: true,
            },
        ] {
            assert!(
                !web_status_line(&info, Lang::Zh).is_empty(),
                "{info:?} 没有状态文案"
            );
            assert!(
                web_next_step(&info, Lang::Zh).is_some(),
                "{info:?} 没有给出下一步"
            );
        }
    }

    /// `w` 是个开关：关着就开，开着就关。**用 `on` 判，不用 `url`
    /// 判**——「开着但算不出地址」那一格 `url` 是 `None`，照 `url` 判的话
    /// 那时候按 `w` 会再开一次（`web_enable` 看见已经在跑会原样返回），
    /// 于是这个开关在最需要它的那一格里变成一个按了没反应的键。
    #[test]
    fn w_turns_it_on_when_off_and_off_when_on() {
        let off = WebInfo {
            on: false,
            url: None,
            address_unknown: false,
        };
        assert!(matches!(toggle_request(&off), Request::WebEnable));

        let on = WebInfo {
            on: true,
            url: Some("http://1.2.3.4:9/#t=x".into()),
            address_unknown: false,
        };
        assert!(matches!(toggle_request(&on), Request::WebDisable));

        // 开着但没地址：还是「关掉」，见上面那段注释。
        let blind = WebInfo {
            on: true,
            url: None,
            address_unknown: true,
        };
        assert!(matches!(toggle_request(&blind), Request::WebDisable));
    }

    /// **令牌绝不上屏。** 二维码里带着它（那是给摄像头的），但写成字的
    /// 那一行只能到端口为止——屏幕会被拍照、会被投影、会被录进屏。
    #[test]
    fn the_token_never_appears_in_the_address_on_screen() {
        let shown = address_for_display("http://192.168.1.5:53412/#t=deadbeefcafe");
        assert_eq!(shown, "http://192.168.1.5:53412/");
        assert!(!shown.contains("deadbeefcafe"), "令牌跑到屏幕上了：{shown}");
    }

    /// 没有 fragment 的地址原样返回——剪的是令牌，不是地址本身。
    #[test]
    fn an_address_without_a_token_is_left_alone() {
        assert_eq!(
            address_for_display("http://192.168.1.5:53412/"),
            "http://192.168.1.5:53412/"
        );
    }

    /// 宽度不够时**不画半块码**：一块画不全的码扫不出来，而用户会对着它
    /// 试半天。那时候该退回文字（`WebQrTooNarrow` 那一句）。
    #[test]
    fn a_narrow_window_gets_words_instead_of_a_broken_code() {
        assert!(qr_lines("http://192.168.1.5:53412/#t=abc", 10).is_none());
    }

    /// 宽度够就真的画得出来，而且**画的是带令牌的那个地址**——手机扫到的
    /// 必须是能直接用的链接，不是屏幕上那截剪过的。
    #[test]
    fn a_wide_enough_window_draws_the_code() {
        let lines = qr_lines("http://192.168.1.5:53412/#t=abc", 80).expect("80 列放得下这个码");
        assert!(!lines.is_empty());
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

    /// **正在打令牌的时候，`w` 就是个字母。** 这一页上 `w` 是局域网那一节的
    /// 开关，但令牌里也可能有 `w`——按键分发要是先看开关再看输入框，用户
    /// 打到一半会莫名其妙地把局域网口子打开，而他正在打的那个字丢了。
    #[test]
    fn w_while_typing_a_token_is_just_a_letter() {
        let (mut app, _dir) = App::test_app();
        app.view = phone_view(PhoneState::Off);
        app.phone_buf = Some(String::new());

        handle_key(&mut app, key(KeyCode::Char('w'))).unwrap();

        assert_eq!(app.phone_buf, Some("w".into()), "w 该落进令牌输入框");
        assert!(!app.web.on, "打字的时候不该把局域网口子打开");
    }

    /// 把这一页画出来，取屏幕上的文字。
    ///
    /// **空白全部滤掉**（同 `attach.rs::screen_text`）：宽字符在 `TestBackend`
    /// 里占两格，第二格是空的，逐格拼出来的中文会变成「局 域 网」——照原样
    /// 比对的话，一条本来该通过的断言会因为渲染细节而失败。
    fn screen_of(app: &mut App, width: u16, height: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        term.draw(|f| draw(f, f.area(), app)).unwrap();
        let buf = term.backend().buffer().clone();
        let a = buf.area;
        (0..a.height)
            .flat_map(|y| (0..a.width).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    /// 断言用的：跟 `screen_of` 一样把空白滤掉再比。
    fn squeeze(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// 开着的时候，这一页得让人能用上它：**局域网那一节要露面**，地址要在
    /// 屏幕上（否则扫不了码的人没有第二条路）。
    #[test]
    fn the_lan_section_shows_the_address_when_it_is_on() {
        let (mut app, _dir) = App::test_app();
        app.view = phone_view(PhoneState::Off);
        app.web = WebInfo {
            on: true,
            url: Some("http://192.168.1.5:53412/#t=deadbeefcafe".into()),
            address_unknown: false,
        };

        let screen = screen_of(&mut app, 80, 40);

        assert!(
            screen.contains(&squeeze(text(Key::WebSection, app.lang))),
            "局域网那一节没露面：\n{screen}"
        );
        assert!(
            screen.contains("192.168.1.5:53412"),
            "地址没上屏，扫不了码的人就没路了：\n{screen}"
        );
    }

    /// **令牌不许出现在屏幕上任何地方。** 二维码里有它（那是给摄像头的），
    /// 但一个看过这块屏的人不该能把它抄下来——屏幕会被拍照、投影、录屏。
    #[test]
    fn the_token_is_nowhere_on_the_screen() {
        let (mut app, _dir) = App::test_app();
        app.view = phone_view(PhoneState::Off);
        app.web = WebInfo {
            on: true,
            url: Some("http://192.168.1.5:53412/#t=deadbeefcafe".into()),
            address_unknown: false,
        };

        let screen = screen_of(&mut app, 80, 40);

        assert!(
            !screen.contains("deadbeefcafe"),
            "令牌被写到屏幕上了：\n{screen}"
        );
    }

    /// 窗口太窄画不下码的时候，**不许留一块画不全的码**：得换成那句给出路
    /// 的话（拉宽窗口，或者照着地址手输）。
    #[test]
    fn a_narrow_page_says_so_instead_of_drawing_half_a_code() {
        let (mut app, _dir) = App::test_app();
        app.view = phone_view(PhoneState::Off);
        app.web = WebInfo {
            on: true,
            url: Some("http://192.168.1.5:53412/#t=deadbeefcafe".into()),
            address_unknown: false,
        };

        let screen = screen_of(&mut app, 30, 20);

        assert!(
            screen.contains(&squeeze(text(Key::WebQrTooNarrow, app.lang))),
            "窄窗口下没给出路：\n{screen}"
        );
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
