//! 局域网手机端设置页。设置页选中「局域网手机端」进。
//!
//! **跟手机通知（`ui::phone`）是两件事，各占一行设置项。** 两者都叫「手机」，
//! 曾经挤在同一页上：一个是 Telegram 推消息（要 BotFather 的令牌），一个是
//! 同一个 WiFi 下手机浏览器打开的网页（扫码就行）。挤在一起的代价是真的
//! 发生过——用户进那一页按 Enter，想开的是网页，弹出来的却是「粘贴 BotFather
//! 的令牌」。一页只讲一件事，Enter 就不会落错地方。
//!
//! 状态放在 `App::web` 而不是 `View::Web` 里，理由见 `App::web` 的文档注释。

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Wrap};

use crate::i18n::{text, Key, Lang};
use crate::proto::{Request, Response, WebInfo};

use super::app::App;
use super::view::{is_plain_key, View};
use super::{accent, danger, dim};

/// 开关那一支用的：发一条请求，拿回新状态。
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

/// 按下开关该发哪一条请求：关着就开，开着就关。
///
/// **看 `on`，不看 `url`。** 「开着但算不出局域网地址」那一格 `url` 是
/// `None`，照 `url` 判的话会再发一次 `WebEnable`，而 `web_enable` 看见
/// 已经在跑就原样返回——于是这个开关在最需要它的那一格里变成一个按了
/// 没反应的键。
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

/// **这个函数里永远不要 `continue`。** 理由同 `phone.rs`/`board.rs`：
/// 循环末尾还有一段清理陈旧 `message` 的逻辑，跳过它会让一句普通反馈盖掉
/// 屏幕上唯一的出路。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if !matches!(app.view, View::Web) {
        return Ok(());
    }
    match key.code {
        KeyCode::Esc => {
            app.view = View::Settings {
                state: ratatui::widgets::ListState::default(),
                sub: None,
            }
        }
        // **开和关分成两个键，不是一个 Enter 来回切。** 照搬手机通知页的
        // 约定（Enter 开、`x` 关）：一个来回切的 Enter 在「已经开着」那一格
        // 是个会把人踢下线的键，而它恰好是最顺手、最容易误按的那个。
        KeyCode::Enter if !app.web.on && is_plain_key(&key) => {
            app.web = send_web_request(app, toggle_request(&app.web));
        }
        KeyCode::Char('x') if app.web.on && is_plain_key(&key) => {
            app.web = send_web_request(app, toggle_request(&app.web));
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    if !matches!(app.view, View::Web) {
        return;
    }
    let rule = if app.connected { dim() } else { danger() };
    let title = format!(
        "{} · {}",
        text(Key::SettingsTitle, app.lang),
        text(Key::WebSection, app.lang)
    );
    let body = super::widgets::header(f, area, &title, rule);

    let mut lines: Vec<Line> = vec![Line::from(web_status_line(&app.web, app.lang))];
    if let Some(step) = web_next_step(&app.web, app.lang) {
        lines.push(Line::from(Span::styled(step, dim())));
    }
    if !app.web.on {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            text(Key::WebFirewall, app.lang),
            dim(),
        )));
    }
    if let Some(url) = &app.web.url {
        lines.push(Line::from(""));
        // 屏幕上写到端口为止，令牌只进码里（`address_for_display`）。
        lines.push(Line::from(Span::styled(
            address_for_display(url).to_string(),
            accent(),
        )));
        lines.push(Line::from(""));
        match qr_lines(url, area.width) {
            Some(qr) => lines.extend(qr),
            // 画不下就换成话，不留半块码：见 `qr_lines`。
            None => lines.push(Line::from(Span::styled(
                text(Key::WebQrTooNarrow, app.lang),
                dim(),
            ))),
        }
    }

    f.render_widget(
        // 必须折行：窄窗口那句话要说完两条出路，被切一半就没法执行。
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        body,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn on(url: &str) -> WebInfo {
        WebInfo {
            on: true,
            url: Some(url.into()),
            address_unknown: false,
        }
    }

    /// 把这一页画出来，取屏幕上的文字。空白全部滤掉（同 `attach.rs::screen_text`）：
    /// 宽字符在 `TestBackend` 里占两格，逐格拼出来的中文会变成「局 域 网」。
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

    fn squeeze(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
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

    /// 开关就是个开关：关着就开，开着就关。**用 `on` 判，不用 `url`
    /// 判**——「开着但算不出地址」那一格 `url` 是 `None`，照 `url` 判的话
    /// 那时候按 `w` 会再开一次（`web_enable` 看见已经在跑会原样返回），
    /// 于是这个开关在最需要它的那一格里变成一个按了没反应的键。
    #[test]
    fn the_switch_opens_when_off_and_closes_when_on() {
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

    /// **令牌不许出现在屏幕上任何地方。** 二维码里有它（那是给摄像头的），
    /// 但一个看过这块屏的人不该能把它抄下来——屏幕会被拍照、投影、录屏。
    #[test]
    fn the_token_is_nowhere_on_the_screen() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Web;
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
        app.view = View::Web;
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

    /// **这一页不认 Telegram 那一套。** 拆成两页的全部理由就是这个：在
    /// 这儿按 Enter，开的必须是局域网口子，绝不能弹出「粘贴 BotFather 的
    /// 令牌」——那正是拆之前用户真的撞上的事。
    #[test]
    fn enter_here_never_asks_for_a_bot_token() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Web;

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        assert!(
            app.phone_buf.is_none(),
            "这一页的 Enter 把人带进了令牌输入框"
        );
    }

    /// 开着的时候 Enter 不再是开关：**它什么都不该做**。开关只有 `x`。
    /// 一个来回切的 Enter 在这一格会把手机上正开着的页面踢下线，而它恰好
    /// 是最顺手、最容易误按的键。
    #[test]
    fn enter_does_not_close_a_running_listener() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Web;
        app.web = on("http://192.168.1.5:53412/#t=abc");

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        assert!(app.web.on, "Enter 把开着的口子关掉了");
    }

    /// Esc 回设置页——这一页唯一的来路就是设置页。
    #[test]
    fn escape_goes_back_to_settings() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Web;

        handle_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert!(matches!(app.view, View::Settings { .. }));
    }

    /// 开着的时候地址要上屏（扫不了码的人得有第二条路），**但令牌不许上屏**。
    #[test]
    fn the_address_shows_but_the_token_never_does() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Web;
        app.web = on("http://192.168.1.5:53412/#t=deadbeefcafe");

        let screen = screen_of(&mut app, 80, 40);

        assert!(
            screen.contains("192.168.1.5:53412"),
            "地址没上屏：\n{screen}"
        );
        assert!(!screen.contains("deadbeefcafe"), "令牌上屏了：\n{screen}");
    }

    /// **打开之前就得说清楚系统会拦一下。**
    ///
    /// 第一次绑到所有网卡上时，Windows 和 macOS 都会弹一个授权框，而系统在
    /// 有人点它之前把那次调用按住。不知情的用户点了「取消」，之后只会看到
    /// 手机连不上，而屏幕上没有任何东西解释为什么——这正是「错误信息不给出
    /// 下一步就是没写完」那条房规要防的形状，只不过这一次要在**出错之前**说。
    #[test]
    fn the_firewall_prompt_is_explained_before_it_appears() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Web;
        app.web = WebInfo {
            on: false,
            url: None,
            address_unknown: false,
        };

        let screen = screen_of(&mut app, 80, 40);

        assert!(
            screen.contains(&squeeze(text(Key::WebFirewall, app.lang))),
            "还没打开的时候没讲防火墙那一下：
{screen}"
        );
    }

    /// 开着之后就不再提防火墙了：那时候他已经点过那个框，再说一遍是噪音，
    /// 而这一页最值钱的是二维码上面那几行。
    #[test]
    fn the_firewall_line_goes_away_once_it_is_on() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Web;
        app.web = WebInfo {
            on: true,
            url: Some("http://192.168.1.5:53412/#t=deadbeefcafe".into()),
            address_unknown: false,
        };

        let screen = screen_of(&mut app, 80, 40);

        assert!(
            !screen.contains(&squeeze(text(Key::WebFirewall, app.lang))),
            "开着之后还在讲防火墙：
{screen}"
        );
    }
}
