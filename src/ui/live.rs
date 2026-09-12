//! 直播观众链接——老师这一侧的面板：上架/下架会话、看链接和二维码、停播。
//!
//! 看板/附着视图按 `L` 进（`ui::mod` 的按键分派里，先于各自的
//! `handle_key` 拦一道）。状态住在 `App::live`，不是这个视图自己——
//! 理由跟 `App::web` 一模一样：顶栏那行「正在直播」常驻提示要在**任何**
//! 视图下都读得到这份状态，塞进 `View::Live` 只有停在这一屏时才读得到。
//!
//! **这一屏最重要的东西不是它自己，是 `live_banner()` 拼出来的那一行**：
//! 它被画进全局底栏（`ui::mod::draw` 里，跟 `attach::scroll_hint` 抢
//! 同一个槽位，但优先级更高、而且不会被 `message` 盖掉）——这个功能最
//! 危险的失败模式不是链接泄露，是老师忘了自己在播、切去处理一件私事。
//! 常驻、不能折叠、不能关，就是为了防这件事。

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{ListState, Paragraph, Wrap};

use crate::i18n::{msg, text, Key, Lang};
use crate::proto::{LiveInfo, LiveReadiness, Request, Response};
use crate::session::SessionInfo;

use super::app::App;
use super::view::{is_plain_key, View};
use super::widgets::{session_label, Msg};
use super::{accent, danger, dim, move_sel_n};

/// 在不在播——只看 `id` 是不是空串，见 `LiveState::info()` 那份约定
/// （没有房间时回一个 `id: String::new()` 的 `LiveInfo`，不是 `Option`，
/// 这样 `LiveInfo` 才能原样经过协议线又不必包一层 `Option`）。
pub(crate) fn is_live(info: &LiveInfo) -> bool {
    !info.id.is_empty()
}

/// **屏幕上必须一直说着「你在播」。**
///
/// 这个功能最危险的失败模式不是链接泄露，是老师忘了自己在播、切去处理一件
/// 私事。所以这一行常驻、不折叠、不可关，而且带着实时人数——「7 人在看」
/// 比任何提示语都有效。
///
/// `readiness` 不是 `Ready` 的时候要如实说：中转还没认得这场直播的时候，
/// 这句话绝不能看着像「一切正常」——那正是老师会把一条当时打不开的链接
/// 发给全班的那一刻。
pub(crate) fn live_banner(info: &LiveInfo, lang: Lang) -> String {
    let base = msg::live_on_air(lang, info.staged.len(), info.viewers);
    match &info.readiness {
        LiveReadiness::Ready => base,
        LiveReadiness::Pending => format!("{base} · {}", text(Key::LiveConnectingToRelay, lang)),
        // 本地化就在这里发生——`reason` 是一个码，不是句子（见
        // `LiveReadiness` 的文档注释）。
        LiveReadiness::Failed(why) => format!("{base} · {}", msg::live_start_failed(lang, why)),
    }
}

/// 这一行常驻提示该用什么颜色画：`Ready` 是安心的强调色，`Pending` 只是
/// 平常字（还没到出错的地步，但也不该看着跟 `Ready` 一样安心），
/// `Failed` 是红——它字面上就是一句错误。
pub(crate) fn banner_style(info: &LiveInfo) -> Style {
    match &info.readiness {
        LiveReadiness::Ready => accent(),
        LiveReadiness::Pending => dim(),
        LiveReadiness::Failed(_) => danger(),
    }
}

/// 屏幕上要显示的链接：**token 打点，一个字符都不许露**。
///
/// 跟 `web::address_for_display` 不一样的地方是这里不止剪掉 `#t=…`，
/// 还补一截看得出「这里本来有一段」的打点尾巴——教室里学生会问「链接呢」，
/// 老师念的是这一行，念出来的必须是「打开屏幕上那个码」而不是一串能被
/// 抄下来的字符。
pub(crate) fn link_line(info: &LiveInfo) -> String {
    let base = match info.url.find('#') {
        Some(i) => &info.url[..i],
        None => info.url.as_str(),
    };
    format!("{base}#t={}", "·".repeat(8))
}

/// 当前项目里能上架的会话——直播只认**当前项目**这一批，不是全体会话。
///
/// **这是有意的取舍，不是漏做。** 教室场景下老师直播的是手头这一个项目，
/// 把别的项目的会话也混进同一张清单只会让它变得没法用——项目一多，
/// 找到自己要的那几路全靠肉眼在几十行里翻。代价也写在这里：**要上架
/// 别的项目的会话，得先把光标切到那个项目再进这一页**（`L` 键读的是
/// `App::current_group()`，也就是看板/九宫格光标当下停在哪个项目上）；
/// 这一页本身不提供跨项目选择的入口。
fn current_project_sessions(app: &App) -> Vec<SessionInfo> {
    app.current_group()
        .map(|g| g.sessions.clone())
        .unwrap_or_default()
}

/// 手写的 base64（标准字母表，带 `=` 补位）。只为 `write_osc52_clipboard`
/// 一个用途，不为它专门引入一个 crate——OSC 52 的负载就是原始字节的
/// base64，没有别的花样。
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(b2 & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// 把带 token 的完整链接写进系统剪贴板，走 OSC 52——终端自己转发给系统
/// 剪贴板，dct 不用知道剪贴板在哪儿，也不用为这一个键多引入一个依赖。
///
/// **这不是往屏幕上打印这段文字**：OSC 52 是终端看不见的一段转义序列，
/// 跟真的往 alternate screen 里画字是两回事——token 因此不会因为这一个
/// 键而破了「绝不上屏」那条规矩。副作用没法单测，跟 `ui::mod` 里那几处
/// `execute!` 写光标形状是同一个道理。
///
/// **OSC 52 是单向的，没有回执。** dct 把这段转义序列写给终端之后，
/// 终端收没收、系统剪贴板真的变没变，这个进程永远不知道——老终端、
/// 某些 ssh/tmux 中转会原样吞掉它，既不报错也不生效。所以调用方
/// （`handle_key` 里的 `c` 分支）**不能把这次调用当成"复制成功了"**，
/// 只能说"已经发给终端了"，并且必须同时给一条不靠剪贴板的退路——
/// 面板上那块二维码，见 `Key::LiveLinkCopied` 的措辞。说了"已复制"而
/// 剪贴板其实没变，比不提供复制更糟：老师会把剪贴板里的旧内容当成
/// 链接发给全班。
fn write_osc52_clipboard(text: &str) {
    let payload = base64_encode(text.as_bytes());
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::style::Print(format!("\x1b]52;c;{payload}\x07"))
    );
}

/// 看板/附着视图按 `L` 进这一页。**先问一次 `LiveStatus`**，不是直接切
/// 视图再等下一帧：这一屏从第一帧起就要说清楚「到底在不在播」，拿一份
/// 好几秒前的旧状态开场，跟一开场就说错话是一回事（同
/// `settings_view::open_web` 的取舍）。拿不到就当作没在播，宁可少说。
pub(crate) fn open(app: &mut App) {
    app.live = match app.client().and_then(|c| c.call(Request::LiveStatus)) {
        Ok(Response::Live(info)) => info,
        _ => app.live.clone(),
    };
    let mut state = ListState::default();
    if !current_project_sessions(app).is_empty() {
        state.select(Some(0));
    }
    app.view = View::Live { state };
}

/// 空格/`r` 共用的落地：把新的上架名单发给守护进程，按回答更新
/// `App::live`。**上架不认识的会话 id 会被整体拒绝**（`daemon.rs` 的
/// `validated_staging`），这条错误必须显示出来，不能悄悄吞掉——否则老师会
/// 以为自己按的勾生效了，而实际上整条名单都没改。
///
/// # 开场和改上架走的是两条不同的请求
///
/// `new_link = false`（勾选框）：已经在播的时候发 `LiveRestage`——**链接
/// 一个字都不变**。走 `LiveStart` 的话每按一次空格就换一把新 token，已经
/// 发给全班的链接当场作废、200 个学生一起掉线，而老师屏幕上没有任何提示。
///
/// `new_link = true`（`r` 键）：发 `LiveStart`，真的换一条新链接、旧的
/// 作废——那才是那个键该有的唯一语义。
///
/// 没在播的时候（第一次勾选）无论哪种都只能是 `LiveStart`：还没有房间
/// 可改。
fn apply_staged(app: &mut App, staged: Vec<(u32, String)>, new_link: bool) {
    let req = staging_request(&app.live, staged, new_link);
    match app.client().and_then(|c| c.call(req)) {
        Ok(Response::Live(info)) => app.live = info,
        Ok(Response::Error(e)) => {
            let reason = msg::error(app.lang, &e);
            app.message = Msg::err(msg::live_start_rejected(app.lang, &reason));
        }
        _ => app.message = Msg::err(text(Key::RequestFailed, app.lang).into()),
    }
}

/// 该发哪一条请求。**纯函数，好测**——这个判断本身就是 I1 那个 bug 的
/// 全部内容（勾一下复选框把全班的链接作废了），它不该只活在一段要真守护
/// 进程才跑得到的代码里。
fn staging_request(live: &LiveInfo, staged: Vec<(u32, String)>, new_link: bool) -> Request {
    let ids = staged.iter().map(|(id, _)| *id).collect();
    let names = staged.iter().map(|(_, n)| n.clone()).collect();
    if new_link || !is_live(live) {
        Request::LiveStart { ids, names }
    } else {
        Request::LiveRestage { ids, names }
    }
}

/// 空格键：把光标这一行加进/踢出上架名单。
fn toggle_selected(app: &mut App, state: &ListState) {
    let sessions = current_project_sessions(app);
    let Some(i) = state.selected() else {
        return;
    };
    let Some(s) = sessions.get(i) else {
        return;
    };
    let id = s.id;
    let name = session_label(s).to_string();
    let mut staged = app.live.staged.clone();
    match staged.iter().position(|(sid, _)| *sid == id) {
        Some(pos) => {
            staged.remove(pos);
        }
        None => staged.push((id, name)),
    }
    // **改勾选绝不换链接。** 已经在播就走 `LiveRestage`，见 `apply_staged`
    // 上那段注释：老师加一路的时候不该把全班的链接作废掉。
    apply_staged(app, staged, false);
}

/// `r` 换链接：原样重发一次当前的上架名单，但走的是 `LiveStart`。
/// **`LiveStart` 每次都会起一个新房间、发一把新 token**（`LiveState::start`
/// 的约定，见 `starting_again_replaces_the_previous_room`），所以重发就是
/// 「换一条新链接、旧的立刻失效」——这是 `r` 唯一该有的语义，也是这一支跟
/// 勾选框那一支（`LiveRestage`）唯一的区别。
fn regenerate_link(app: &mut App) {
    let staged = app.live.staged.clone();
    apply_staged(app, staged, true);
}

/// `s` 停播。
fn stop_live(app: &mut App) {
    match app.client().and_then(|c| c.call(Request::LiveStop)) {
        Ok(Response::Live(info)) => {
            app.live = info;
            app.message = text(Key::LiveStoppedMessage, app.lang).into();
        }
        _ => app.message = Msg::err(text(Key::RequestFailed, app.lang).into()),
    }
}

/// **这个函数里永远不要 `continue`。** 理由同 `web.rs`/`board.rs`：循环
/// 末尾还有一段清理陈旧 `message` 的逻辑，跳过它会让一句普通反馈盖掉
/// 屏幕上唯一的出路。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Live { mut state } = app.view.clone() else {
        return Ok(());
    };
    match key.code {
        KeyCode::Esc => {
            app.view = super::home_view(app);
            return Ok(());
        }
        KeyCode::Down if is_plain_key(&key) => {
            let len = current_project_sessions(app).len();
            move_sel_n(&mut state, len, 1);
        }
        KeyCode::Up if is_plain_key(&key) => {
            let len = current_project_sessions(app).len();
            move_sel_n(&mut state, len, -1);
        }
        KeyCode::Char(' ') if is_plain_key(&key) => toggle_selected(app, &state),
        // `c`/`r`/`s` 只在真的在播的时候才有意义——没有链接可复制/换，
        // 没有播可停。**没有前提也不判**：屏幕上按 Esc 之外的键什么都
        // 不发生，比按下去报一句看不懂的错要老实。
        KeyCode::Char('c') if is_plain_key(&key) && is_live(&app.live) => {
            write_osc52_clipboard(&app.live.url);
            app.message = text(Key::LiveLinkCopied, app.lang).into();
        }
        KeyCode::Char('r') if is_plain_key(&key) && is_live(&app.live) => regenerate_link(app),
        KeyCode::Char('s') if is_plain_key(&key) && is_live(&app.live) => stop_live(app),
        _ => {}
    }
    app.view = View::Live { state };
    Ok(())
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let View::Live { state } = app.view.clone() else {
        return;
    };
    let rule = if app.connected { dim() } else { danger() };
    let body = super::widgets::header(f, area, text(Key::LiveSection, app.lang), rule);

    let live = app.live.clone();
    let lang = app.lang;
    let mut lines: Vec<Line> = Vec::new();

    if is_live(&live) {
        lines.push(Line::from(Span::styled(
            live_banner(&live, lang),
            banner_style(&live),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(link_line(&live), accent())));
        lines.push(Line::from(""));
        // 画的是**带 token 的完整地址**：手机扫到的必须是能直接打开的
        // 链接，屏幕上写的字（上面那一行）才是打点过的那份。
        match super::web::qr_lines(&live.url, body.width) {
            Some(qr) => lines.extend(qr),
            // 宽度不够就换成话，不留半块码——同 `web::qr_lines` 的约定。
            None => lines.push(Line::from(Span::styled(
                text(Key::WebQrTooNarrow, lang),
                dim(),
            ))),
        }
    } else {
        lines.push(Line::from(text(Key::LiveOffLine, lang)));
    }

    lines.push(Line::from(""));

    let sessions = current_project_sessions(app);
    if sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            text(Key::LiveNoSessionsToStage, lang),
            dim(),
        )));
    } else {
        for (i, s) in sessions.iter().enumerate() {
            let staged = live.staged.iter().any(|(id, _)| *id == s.id);
            let cursor = if state.selected() == Some(i) {
                "▶ "
            } else {
                "  "
            };
            let checkbox = if staged { "[x] " } else { "[ ] " };
            let row = format!("{cursor}{checkbox}{:>3}  {}", s.id, session_label(s));
            let style = if staged { accent() } else { Style::default() };
            lines.push(Line::from(Span::styled(row, style)));
        }
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::LiveFailure;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn info(staged: Vec<(u32, String)>, viewers: u32, readiness: LiveReadiness) -> LiveInfo {
        LiveInfo {
            id: "abc".into(),
            token: "t".repeat(64),
            url: "https://x/live/abc#t=…".into(),
            staged,
            viewers,
            readiness,
        }
    }

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

    /// **屏幕上必须一直说着「你在播」。** 出自 task-7-brief：两路上架、
    /// 7 个人在看的时候，这一行得同时说出「正在直播」、路数、人数。
    #[test]
    fn the_top_bar_always_says_you_are_live() {
        let line = live_banner(
            &info(
                vec![(3, "前端".into()), (5, "后端".into())],
                7,
                LiveReadiness::Ready,
            ),
            Lang::Zh,
        );
        assert!(line.contains("正在直播"));
        assert!(line.contains('2'), "没说在播几路：{line}");
        assert!(line.contains('7'), "没说几个人在看：{line}");
    }

    /// token 不许出现在屏幕上——老师会录屏，会投影。
    #[test]
    fn the_token_never_reaches_the_screen() {
        let mut i = info(vec![], 0, LiveReadiness::Ready);
        i.token = "s".repeat(64);
        i.url = format!("https://x/live/abc#t={}", "s".repeat(64));
        let shown = link_line(&i);
        assert!(
            !shown.contains(&"s".repeat(8)),
            "链接行上带着 token：{shown}"
        );
        assert!(shown.contains("#t=") && shown.contains('·'), "得留个打点的尾巴：{shown}");
    }

    /// `readiness` 不是 `Ready` 的时候必须如实说——**绝不能看着像一切正常**，
    /// 那正是老师会把一条当时打不开的链接发给全班的那一刻。
    #[test]
    fn pending_readiness_says_so_instead_of_looking_fine() {
        let line = live_banner(&info(vec![], 0, LiveReadiness::Pending), Lang::Zh);
        assert!(line.contains("正在直播"), "{line}");
        assert!(
            line.contains("中转") || line.contains("连接"),
            "没说还在连中转：{line}"
        );
    }

    /// 同上，`Failed` 那一档要把原因说出来——**由界面这一侧组句**，
    /// 守护进程给的只是一个码。
    #[test]
    fn failed_readiness_carries_the_reason() {
        let line = live_banner(
            &info(vec![], 0, LiveReadiness::Failed(LiveFailure::Unreachable)),
            Lang::Zh,
        );
        assert!(line.contains("中转"), "{line}");
        assert!(line.contains("失败"), "{line}");
    }

    /// **英文界面上不许冒出中文。**
    ///
    /// 早先 `LiveReadiness::Failed` 带的是一句守护进程拼好的中文，于是
    /// `Lang::En` 下这一行会是 "Failed to go live: 连不上中转，稍后会自动
    /// 重试"。仓库里那条「英文文案里不许有汉字」的守卫钉的是 `Key` 那张
    /// 表，一句带参数拼出来的话从它旁边绕了过去——这条测试补的正是那道缝。
    #[test]
    fn the_english_banner_never_says_anything_in_chinese() {
        use crate::i18n::has_han;
        for why in [LiveFailure::Unreachable, LiveFailure::Refused(413)] {
            let line = live_banner(&info(vec![], 0, LiveReadiness::Failed(why.clone())), Lang::En);
            assert!(!has_han(&line), "英文界面上说了中文：{line}");
        }
        // `Pending` 那一档走的是 `Key` 表，顺手一起钉住。
        let pending = live_banner(&info(vec![], 0, LiveReadiness::Pending), Lang::En);
        assert!(!has_han(&pending), "英文界面上说了中文：{pending}");
    }

    /// 没有 fragment 的地址原样返回——剪的是 token，不是地址本身。
    #[test]
    fn a_url_without_a_fragment_is_left_alone_but_still_gets_a_dotted_tail() {
        let mut i = info(vec![], 0, LiveReadiness::Ready);
        i.url = "https://x/live/abc".into();
        assert_eq!(link_line(&i), "https://x/live/abc#t=········");
    }

    /// **令牌不许出现在屏幕上任何地方**——同 `web.rs` 的
    /// `the_token_is_nowhere_on_the_screen`。
    #[test]
    fn the_token_is_nowhere_on_the_screen() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Live {
            state: ListState::default(),
        };
        app.live = LiveInfo {
            id: "abc".into(),
            token: "deadbeefcafe".repeat(4),
            url: format!("https://x/live/abc#t={}", "deadbeefcafe".repeat(4)),
            staged: vec![],
            viewers: 3,
            readiness: LiveReadiness::Ready,
        };

        let screen = screen_of(&mut app, 80, 40);

        assert!(
            !screen.contains("deadbeefcafe"),
            "令牌被写到屏幕上了：\n{screen}"
        );
    }

    /// 窗口太窄画不下码的时候，不许留一块画不全的码。
    #[test]
    fn a_narrow_window_says_so_instead_of_drawing_half_a_code() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Live {
            state: ListState::default(),
        };
        app.live = LiveInfo {
            id: "abc".into(),
            token: "t".repeat(64),
            url: "https://x/live/abc#t=deadbeefcafe".into(),
            staged: vec![],
            viewers: 0,
            readiness: LiveReadiness::Ready,
        };

        let screen = screen_of(&mut app, 20, 20);
        let squeeze = |s: &str| -> String { s.chars().filter(|c| !c.is_whitespace()).collect() };

        assert!(
            screen.contains(&squeeze(text(Key::WebQrTooNarrow, app.lang))),
            "窄窗口下没给出路：\n{screen}"
        );
    }

    /// Esc 退回家视图——列表模式下就是看板。
    #[test]
    fn escape_goes_home() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Live {
            state: ListState::default(),
        };

        handle_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert!(matches!(app.view, View::Board));
    }

    /// 不在播的时候，`c`/`r`/`s` 什么都不该发生——没有链接可复制/换/停。
    #[test]
    fn c_r_s_do_nothing_when_not_live() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Live {
            state: ListState::default(),
        };

        for code in [KeyCode::Char('c'), KeyCode::Char('r'), KeyCode::Char('s')] {
            handle_key(&mut app, key(code)).unwrap();
        }

        assert!(app.live.id.is_empty(), "没在播的时候不该凭空冒出一个房间");
    }

    /// **已经在播的时候改勾选，走的必须是 `LiveRestage`。** 走 `LiveStart`
    /// 的话每按一次空格就换一把新 token：老师上架「后端」发了链接，想再
    /// 加一路「前端」按一下空格，200 个学生同时掉线，而他屏幕上什么提示
    /// 都没有。
    #[test]
    fn toggling_a_checkbox_while_live_restages_instead_of_reissuing_the_link() {
        let live = info(vec![(1, "后端".into())], 0, LiveReadiness::Ready);
        let req = staging_request(&live, vec![(1, "后端".into()), (2, "前端".into())], false);
        assert!(
            matches!(req, Request::LiveRestage { .. }),
            "改勾选发的不是 LiveRestage，全班的链接会当场作废：{req:?}"
        );
    }

    /// 还没在播的时候只能是 `LiveStart`——没有房间可改。
    #[test]
    fn the_first_checkbox_starts_a_new_broadcast() {
        let off = LiveInfo {
            id: String::new(),
            token: String::new(),
            url: String::new(),
            staged: vec![],
            viewers: 0,
            readiness: LiveReadiness::Pending,
        };
        let req = staging_request(&off, vec![(1, "前端".into())], false);
        assert!(matches!(req, Request::LiveStart { .. }), "{req:?}");
    }

    /// `r` 键是真的换一条链接、旧的作废——**这是它唯一该有的语义**，所以
    /// 哪怕正在播也走 `LiveStart`。
    #[test]
    fn the_new_link_key_really_does_reissue_the_link() {
        let live = info(vec![(1, "后端".into())], 0, LiveReadiness::Ready);
        let req = staging_request(&live, vec![(1, "后端".into())], true);
        assert!(matches!(req, Request::LiveStart { .. }), "{req:?}");
    }

    /// `is_live` 只看 `id` 是不是空串。
    #[test]
    fn is_live_is_purely_about_the_id() {
        let off = LiveInfo {
            id: String::new(),
            token: String::new(),
            url: String::new(),
            staged: vec![],
            viewers: 0,
            readiness: LiveReadiness::Pending,
        };
        assert!(!is_live(&off));
        assert!(is_live(&info(vec![], 0, LiveReadiness::Ready)));
    }
}
