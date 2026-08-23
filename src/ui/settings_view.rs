//! 设置页：一张「设置项」列表。看板按 `l` 进。
//!
//! **加进第二项之前这一页是纯语言列表**，`ListState` 的下标直接映射
//! `Lang::all()`；现在映射 [`SettingsItem`]，选中「语言」那一项才进语言
//! 子列表（今天的语言选择逻辑原样搬到了这一层，行为不变）。
//!
//! 跟 `secret.rs` 的密钥页分开是两码事——那边管「哪个 agent 用哪把密钥」，
//! 这里管界面本身怎么显示。

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::i18n::{text, Key, Lang};
use crate::proto::{Request, Response};
use crate::settings::{save_bar_theme, save_lang, settings_path_for_socket};

use super::app::App;
use super::view::{SubList, View};
use super::widgets::Msg;
use super::{dim, move_sel_n, BarTheme};

/// 设置页的条目。**加进第二项之前这一页是纯语言列表**，`ListState` 的下标
/// 直接映射 `Lang::all()`；现在映射这个枚举，选中语言那一项才进语言列表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsItem {
    Language,
    Theme,
    Phone,
}

impl SettingsItem {
    pub(crate) fn all() -> &'static [SettingsItem] {
        &[
            SettingsItem::Language,
            SettingsItem::Theme,
            SettingsItem::Phone,
        ]
    }

    /// 越界返回 `None` 而不是兜底成第一项：`ListState` 的选中项可能停在
    /// 一个已经不存在的位置，那时候什么都不做，比默默把用户带进语言页好。
    pub(crate) fn at(i: usize) -> Option<SettingsItem> {
        SettingsItem::all().get(i).copied()
    }

    fn label(self, lang: Lang) -> &'static str {
        match self {
            SettingsItem::Language => text(Key::Language, lang),
            SettingsItem::Theme => text(Key::BarTheme, lang),
            SettingsItem::Phone => text(Key::Phone, lang),
        }
    }
}

/// **这个函数里永远不要 `continue`。** 理由同 `board.rs`：循环末尾还有一段
/// 清理陈旧 `message` 的逻辑，跳过它会让一句普通反馈盖掉屏幕上唯一的出路。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Settings { state, sub } = app.view.clone() else {
        return Ok(());
    };

    match sub {
        Some(SubList::Language(ls)) => handle_language_key(app, key, state, ls),
        Some(SubList::Theme(ts)) => handle_theme_key(app, key, state, ts),
        None => handle_top_key(app, key, state),
    }
}

/// 顶层设置项列表：`Language` 进语言子列表，`Phone` 进手机通知页
/// （`View::Phone`，见 `ui::phone`）。
fn handle_top_key(app: &mut App, key: KeyEvent, mut state: ListState) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.view = super::home_view(app),
        KeyCode::Down | KeyCode::Up => {
            let d = if key.code == KeyCode::Down { 1 } else { -1 };
            move_sel_n(&mut state, SettingsItem::all().len(), d);
            app.view = View::Settings { state, sub: None };
        }
        KeyCode::Enter => match state.selected().and_then(SettingsItem::at) {
            Some(SettingsItem::Language) => {
                let mut lang_state = ListState::default();
                lang_state.select(Some(
                    Lang::all().iter().position(|l| *l == app.lang).unwrap_or(0),
                ));
                app.view = View::Settings {
                    state,
                    sub: Some(SubList::Language(lang_state)),
                };
            }
            Some(SettingsItem::Theme) => {
                let mut ts = ListState::default();
                ts.select(Some(
                    BarTheme::all()
                        .iter()
                        .position(|t| *t == app.bar)
                        .unwrap_or(0),
                ));
                app.view = View::Settings {
                    state,
                    sub: Some(SubList::Theme(ts)),
                };
            }
            Some(SettingsItem::Phone) => open_phone(app, state),
            None => {
                app.view = View::Settings { state, sub: None };
            }
        },
        _ => app.view = View::Settings { state, sub: None },
    }
    Ok(())
}

/// 拿一次当前的手机通知状态，进 `View::Phone`。拿不到就留在设置页给一句
/// 错误——同 `mod.rs::open_secrets` 的道理：总比弹进一个既没数据、又没地方
/// 显示错误的空白页强。
fn open_phone(app: &mut App, state: ListState) {
    match app.client().and_then(|c| c.call(Request::PhoneStatus)) {
        Ok(Response::Phone(status)) => app.view = View::Phone { status },
        Ok(Response::Error(ref e)) => {
            app.message = Msg::err(crate::i18n::msg::error(app.lang, e));
            app.view = View::Settings { state, sub: None };
        }
        _ => {
            app.message = Msg::err(text(Key::RequestFailed, app.lang).into());
            app.view = View::Settings { state, sub: None };
        }
    }
}

/// 语言子列表：跟改结构之前的顶层逻辑完全一样，只是现在挂在「语言」这一项
/// 底下，`Esc` 退回设置项列表而不是直接回看板。
fn handle_language_key(
    app: &mut App,
    key: KeyEvent,
    state: ListState,
    mut lang_state: ListState,
) -> Result<()> {
    match key.code {
        // 回设置项列表，不是回看板——用户此刻只退了一层。
        KeyCode::Esc => {
            app.view = View::Settings { state, sub: None };
        }
        KeyCode::Down | KeyCode::Up => {
            let d = if key.code == KeyCode::Down { 1 } else { -1 };
            move_sel_n(&mut lang_state, Lang::all().len(), d);
            app.view = View::Settings {
                state,
                sub: Some(SubList::Language(lang_state)),
            };
        }
        KeyCode::Enter => {
            let chosen = lang_state
                .selected()
                .and_then(|i| Lang::all().get(i))
                .copied();
            if let Some(lang) = chosen {
                app.lang = lang;
                // 立刻写盘。不写的话用户下次开 dct 发现语言变回去了，
                // 而他明明记得自己选过——这正是 `save_lang` 返回 `Result`
                // 而不是像「最近项目」那样吞掉错误的理由。
                let path = settings_path_for_socket(&app.socket);
                match save_lang(&path, lang) {
                    // 语言已经切了，这句反馈用的就是新语言——用户按下 Enter
                    // 之后第一眼看到的就是切换生效的证据。
                    Ok(()) => app.message = text(Key::SettingsTitle, lang).into(),
                    Err(e) => app.message = Msg::err(format!("{e}")),
                }
            }
            app.view = super::home_view(app);
        }
        _ => {
            app.view = View::Settings {
                state,
                sub: Some(SubList::Language(lang_state)),
            }
        }
    }
    Ok(())
}

/// 配色子列表。跟语言那一层同构，只有两处不同：选中当场生效（`set_bar_theme`
/// 之后下一帧就是新配色），以及**存盘失败不回滚内存里的选择**——用户已经
/// 看到新配色了，再悄悄变回去比一句错误提示更让人摸不着头脑。
fn handle_theme_key(
    app: &mut App,
    key: KeyEvent,
    state: ListState,
    mut ts: ListState,
) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.view = View::Settings { state, sub: None },
        KeyCode::Down | KeyCode::Up => {
            let d = if key.code == KeyCode::Down { 1 } else { -1 };
            move_sel_n(&mut ts, BarTheme::all().len(), d);
            app.view = View::Settings {
                state,
                sub: Some(SubList::Theme(ts)),
            };
        }
        KeyCode::Enter => {
            let chosen = ts.selected().and_then(|i| BarTheme::all().get(i)).copied();
            if let Some(t) = chosen {
                app.bar = t;
                let path = settings_path_for_socket(&app.socket);
                match save_bar_theme(&path, t) {
                    Ok(()) => app.message = text(Key::BarTheme, app.lang).into(),
                    Err(e) => app.message = Msg::err(format!("{e}")),
                }
            }
            app.view = super::home_view(app);
        }
        _ => {
            app.view = View::Settings {
                state,
                sub: Some(SubList::Theme(ts)),
            }
        }
    }
    Ok(())
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let View::Settings { state, sub } = app.view.clone() else {
        return;
    };
    match sub {
        Some(SubList::Language(ls)) => draw_language_list(f, area, app, &ls),
        Some(SubList::Theme(ts)) => draw_theme_list(f, area, app, &ts),
        None => draw_settings_items(f, area, app, &state),
    }
}

fn draw_settings_items(f: &mut Frame, area: Rect, app: &mut App, state: &ListState) {
    let items: Vec<ListItem> = SettingsItem::all()
        .iter()
        .map(|item| ListItem::new(Line::from(Span::raw(item.label(app.lang)))))
        .collect();

    let mut s = state.clone();
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::TOP | Borders::BOTTOM)
                    .title(text(Key::SettingsTitle, app.lang))
                    .border_style(if app.connected {
                        Style::default()
                    } else {
                        Style::default().fg(Color::Red)
                    }),
            )
            .highlight_symbol("▶ "),
        area,
        &mut s,
    );
}

fn draw_language_list(f: &mut Frame, area: Rect, app: &mut App, state: &ListState) {
    let items: Vec<ListItem> = Lang::all()
        .iter()
        .map(|l| {
            // 每种语言用它自己的语言写。当前这一项用 `✓` 标出来——
            // 误切到看不懂的语言之后，这个符号是跨语言都认得的线索。
            let mark = if *l == app.lang { "✓ " } else { "  " };
            let style = if *l == app.lang {
                Style::default()
            } else {
                dim()
            };
            ListItem::new(Line::from(vec![
                Span::styled(mark, style),
                Span::styled(l.native_name(), style),
            ]))
        })
        .collect();

    let mut s = state.clone();
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::TOP | Borders::BOTTOM)
                    .title(format!(
                        "{} · {}",
                        text(Key::SettingsTitle, app.lang),
                        text(Key::Language, app.lang)
                    ))
                    .border_style(if app.connected {
                        Style::default()
                    } else {
                        Style::default().fg(Color::Red)
                    }),
            )
            .highlight_symbol("▶ "),
        area,
        &mut s,
    );
}

/// 配色列表的那几行。**每一行用它自己那档配色画出来**——设置页和会话里的
/// F6 浮层共用这一份：两份各写一遍的话，往 `BarTheme` 里加一档就得改两处，
/// 而漏掉的那一处不会报错，只是少一行。
///
/// `current` 单独传进来而不是从 `App` 里取：浮层试穿的时候 `App::bar` 每按
/// 一下方向键就变一次，而 `✓` 要标的始终是**这一帧的当前档**，两边传的
/// 恰好都是它。
pub(crate) fn theme_items(lang: Lang, current: BarTheme) -> Vec<ListItem<'static>> {
    BarTheme::all()
        .iter()
        .map(|t| {
            // 当前这一档用 `✓` 标出来，跟语言列表同一个约定。
            let mark = if *t == current { "✓ " } else { "  " };
            let (sample, style) = theme_sample(*t, lang);
            ListItem::new(Line::from(vec![
                Span::raw(mark),
                Span::styled(sample, style.unwrap_or_else(dim)),
            ]))
        })
        .collect()
}

/// 一档配色的样品那一格：文字，以及画它该用的样式（`None` = `Lines` 那档，
/// 它没有底色，由调用方按「暗一点的普通文字」画）。
///
/// 单独拎出来只为一件事：**排版和量宽度必须读同一份格式串**。F6 浮层要按
/// 最宽的一行决定自己多宽，而它量的必须正是这里拼出来的那几个空格，不是
/// 另一处照着抄的一个数——抄件和正本分叉的时候没有任何东西会报错，屏幕上
/// 只是有一种语言被截掉半个字。
fn theme_sample(t: BarTheme, lang: Lang) -> (String, Option<Style>) {
    let name = t.label(lang);
    match t.style() {
        Some(style) => (format!("  {name}  "), Some(style)),
        None => (format!("  {name} ────  "), None),
    }
}

/// 这张列表最宽的一行占几列，含行首那两列（`✓ ` 或者两个空格）。
pub(crate) fn theme_items_width(lang: Lang) -> usize {
    BarTheme::all()
        .iter()
        .map(|t| 2 + crate::ui::widgets::display_width(&theme_sample(*t, lang).0))
        .max()
        .unwrap_or(0)
}

/// 配色列表。**每一行用它自己那档配色画出来**——配色这种东西描述不出来，
/// 「灰」「蓝」两个字说不清压在你终端上到底什么样，而这一行本身就是样品。
///
/// `Lines` 那一档没有底色可画，用一段横线当样品，跟它选中之后的样子对得上。
fn draw_theme_list(f: &mut Frame, area: Rect, app: &mut App, state: &ListState) {
    let items = theme_items(app.lang, app.bar);

    let mut s = state.clone();
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::TOP | Borders::BOTTOM)
                    .title(format!(
                        "{} · {}",
                        text(Key::SettingsTitle, app.lang),
                        text(Key::BarTheme, app.lang)
                    ))
                    .border_style(if app.connected {
                        Style::default()
                    } else {
                        Style::default().fg(Color::Red)
                    }),
            )
            .highlight_symbol("▶ "),
        area,
        &mut s,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// 光标停在设置项列表的第 `selected` 项上，还没进任何子列表。
    fn on_settings_items(app: &mut App, selected: usize) {
        let mut st = ListState::default();
        st.select(Some(selected));
        app.view = View::Settings {
            state: st,
            sub: None,
        };
    }

    /// 光标停在**指定的那一项**上。按下标写死的话，往列表中间插一项（「配色」
    /// 就是这么插进来的）会让一批测试悄悄改测别的东西——它们照样能过，
    /// 只是不再验原来那件事了。
    fn on_settings_item(app: &mut App, item: SettingsItem) {
        let i = SettingsItem::all()
            .iter()
            .position(|x| *x == item)
            .expect("这一项得在列表里");
        on_settings_items(app, i);
    }

    /// 顶层选中「配色」按 Enter：进配色子列表，光标落在当前那一档上，
    /// 而不是直接改配色——那是子列表自己的 Enter 才做的事。
    #[test]
    fn entering_colors_opens_the_sub_list_on_the_current_theme() {
        let (mut app, _dir) = App::test_app();
        app.bar = BarTheme::Green;
        on_settings_item(&mut app, SettingsItem::Theme);

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        let View::Settings { sub, .. } = &app.view else {
            panic!("还应该在设置页");
        };
        let Some(SubList::Theme(ts)) = sub else {
            panic!("该进配色子列表了，实际 {sub:?}");
        };
        assert_eq!(
            ts.selected().and_then(|i| BarTheme::all().get(i)).copied(),
            Some(BarTheme::Green),
            "光标该落在当前这一档上"
        );
        assert_eq!(app.bar, BarTheme::Green, "只是打开子列表，还没真的选");
    }

    /// 选中一档按 Enter：当场生效，而且必须落盘——不落盘的话用户下次开
    /// dct 发现配色变回去了，跟语言那条一个道理。
    #[test]
    fn choosing_a_color_applies_it_and_writes_it_to_disk() {
        let (mut app, _dir) = App::test_app();
        app.bar = BarTheme::Gray;
        let i = BarTheme::all()
            .iter()
            .position(|t| *t == BarTheme::Purple)
            .unwrap();
        let mut ts = ListState::default();
        ts.select(Some(i));
        app.view = View::Settings {
            state: ListState::default(),
            sub: Some(SubList::Theme(ts)),
        };

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        assert_eq!(app.bar, BarTheme::Purple, "配色要当场生效");
        assert_eq!(
            crate::settings::load_bar_theme(&settings_path_for_socket(&app.socket)),
            Some(BarTheme::Purple),
            "必须落盘，否则下次开又变回去"
        );
    }

    /// Esc 退出配色子列表不改任何东西，只退一层。
    #[test]
    fn escaping_out_of_colors_changes_nothing_and_goes_up_one_level() {
        let (mut app, _dir) = App::test_app();
        app.bar = BarTheme::Gray;
        let mut ts = ListState::default();
        ts.select(Some(1));
        app.view = View::Settings {
            state: ListState::default(),
            sub: Some(SubList::Theme(ts)),
        };

        handle_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert_eq!(app.bar, BarTheme::Gray, "Esc 不该改配色");
        let View::Settings { sub, .. } = &app.view else {
            panic!("应该退回设置页，不是看板");
        };
        assert!(sub.is_none(), "该退回设置项列表，不是留在子列表里");
    }

    /// 直接把光标摆进语言子列表——用来测子列表本身的行为，不用每次都先
    /// 走一遍「进设置页 → Enter「语言」」这条路。
    fn on_language_list(app: &mut App, selected: usize) {
        let mut lang_state = ListState::default();
        lang_state.select(Some(selected));
        app.view = View::Settings {
            state: ListState::default(),
            sub: Some(SubList::Language(lang_state)),
        };
    }

    /// 改结构之前，下标直接映射 Lang::all()。改完之后映射设置项。
    /// **这条是回归测试**：语言仍然切得动比手机通知能用更重要。
    #[test]
    fn the_first_item_is_language() {
        assert_eq!(SettingsItem::all()[0], SettingsItem::Language);
    }

    #[test]
    fn phone_is_a_settings_item_too() {
        assert!(SettingsItem::all().contains(&SettingsItem::Phone));
    }

    /// 下标越界不能 panic——`ListState` 的选中项在列表变短时会留在旧位置。
    #[test]
    fn an_out_of_range_index_selects_nothing() {
        assert_eq!(SettingsItem::at(99), None);
        assert_eq!(SettingsItem::at(0), Some(SettingsItem::Language));
    }

    /// 方向键要能从「语言」走到「Phone」——这条钉住 `move_sel_n` 的长度
    /// 参数用的是 `SettingsItem::all().len()`，不是旧的 `Lang::all().len()`。
    #[test]
    fn arrow_down_moves_from_language_to_phone() {
        let (mut app, _dir) = App::test_app();
        on_settings_items(&mut app, 0);

        handle_key(&mut app, key(KeyCode::Down)).unwrap();

        let View::Settings { state, sub } = &app.view else {
            panic!("还应该在设置页");
        };
        assert!(sub.is_none(), "顶层列表移动不该顺手进子列表");
        // 「配色」是后来插在语言和手机通知中间的，所以往下一格到的是它。
        assert_eq!(
            state.selected().and_then(SettingsItem::at),
            Some(SettingsItem::Theme)
        );
    }

    /// 顶层列表选中「语言」按 Enter：进语言子列表，光标落在当前语言上，
    /// 而不是直接改语言——那是子列表自己的 Enter 才做的事。
    #[test]
    fn entering_language_opens_the_sub_list_on_the_current_language() {
        let (mut app, _dir) = App::test_app();
        app.lang = Lang::Zh;
        on_settings_items(&mut app, 0);

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        let View::Settings { sub, .. } = &app.view else {
            panic!("还应该在设置页");
        };
        let Some(SubList::Language(lang_state)) = sub else {
            panic!("该进语言子列表了，实际 {sub:?}");
        };
        assert_eq!(
            lang_state.selected(),
            Some(1),
            "光标要预先落在当前语言（Zh）上"
        );
        assert_eq!(app.lang, Lang::Zh, "只是打开子列表，还没真的选");
    }

    /// 断开的 `App`（测试默认那种）拿不到手机状态：留在设置页给一句错误，
    /// 不能 panic，也不能悄悄改语言，也不能弹进一个没有数据的手机页。
    #[test]
    fn choosing_phone_without_a_daemon_stays_on_settings_with_an_error() {
        let (mut app, _dir) = App::test_app();
        app.lang = Lang::Zh;
        on_settings_item(&mut app, SettingsItem::Phone);

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        assert_eq!(app.lang, Lang::Zh, "选 Phone 不该改语言");
        assert!(
            matches!(app.view, View::Settings { .. }),
            "拿不到数据就该留在设置页上"
        );
        assert!(app.message.error, "要有一句红字告诉用户出了什么事");
    }

    /// **Ruling 2**：选中「Phone」按 Enter 要真的进 `View::Phone`，带着守护
    /// 进程刚给的那份状态；`Esc` 要能从手机页退回设置页（跟语言子列表退出
    /// 一样，一层一层退，不是一步退到底看板）。起一个真守护进程——断开的
    /// `App` 上 `Request::PhoneStatus` 直接失败，证明不了真正的转场。
    #[test]
    fn choosing_phone_enters_the_phone_page_and_escape_returns_to_settings() {
        use crate::client::Client;

        let home = tempfile::tempdir().unwrap();
        let sock = home.path().join("daemon.sock");
        let s = sock.clone();
        std::thread::spawn(move || {
            let _ = crate::daemon::run(&s);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !sock.exists() {
            assert!(std::time::Instant::now() < deadline, "daemon 没起来");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let mut app = App::new(
            Client::connect(&sock).unwrap(),
            home.path().to_path_buf(),
            Lang::Zh,
            sock,
            crate::ui::ViewMode::List,
        );
        on_settings_item(&mut app, SettingsItem::Phone);

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();
        let View::Phone { status } = &app.view else {
            panic!("选中 Phone 按 Enter 该进手机页，实际停在了别的视图上");
        };
        assert_eq!(
            status.state,
            crate::proto::PhoneState::Off,
            "刚起的守护进程还没配过令牌"
        );

        // Esc 在手机页由 `phone::handle_key` 接（同 `mod.rs` 的按键分发），
        // 不是这个模块自己的 `handle_key`——那个函数只认 `View::Settings`。
        crate::ui::phone::handle_key(&mut app, key(KeyCode::Esc)).unwrap();
        assert!(
            matches!(app.view, View::Settings { .. }),
            "Esc 要从手机页退回设置页，不是一步退到看板"
        );
    }

    /// 选中一种语言按 Enter：界面语言当场就变，并且落盘——下次开 dct 还是它。
    /// 这条是既有行为的回归测试，只是入口从顶层挪到了语言子列表。
    #[test]
    fn choosing_a_language_applies_it_and_writes_it_to_disk() {
        let (mut app, dir) = App::test_app();
        app.lang = Lang::Zh;
        let en_index = Lang::all().iter().position(|l| *l == Lang::En).unwrap();
        on_language_list(&mut app, en_index);

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        assert_eq!(app.lang, Lang::En, "界面语言要当场生效");
        let saved = crate::settings::load_lang(&settings_path_for_socket(&app.socket));
        assert_eq!(saved, Some(Lang::En), "必须落盘，否则下次开又变回去");
        drop(dir);
    }

    /// Esc 在语言子列表里只退一层，回设置项列表，不直接回看板。
    #[test]
    fn esc_in_the_language_list_returns_to_the_settings_item_list() {
        let (mut app, _dir) = App::test_app();
        app.lang = Lang::Zh;
        on_language_list(&mut app, 0);

        handle_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert_eq!(app.lang, Lang::Zh, "Esc 不该改语言");
        let View::Settings { sub, .. } = &app.view else {
            panic!("应该退回设置页，不是看板");
        };
        assert!(sub.is_none(), "该退回设置项列表，不是留在子列表里");
    }

    /// Esc 不改任何东西。设置页最怕的就是「路过一下就把配置改了」。
    #[test]
    fn escaping_out_of_settings_changes_nothing() {
        let (mut app, _dir) = App::test_app();
        app.lang = Lang::Zh;
        on_settings_items(&mut app, 0);

        handle_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert_eq!(app.lang, Lang::Zh, "Esc 不该改语言");
        assert!(matches!(app.view, View::Board));
        assert!(
            crate::settings::load_lang(&settings_path_for_socket(&app.socket)).is_none(),
            "Esc 不该写盘"
        );
    }

    /// 语言列表用各自的语言写，光标能走遍每一行。
    #[test]
    fn every_language_is_listed_in_its_own_language() {
        let (mut app, _dir) = App::test_app();
        on_language_list(&mut app, 0);
        let mut term = Terminal::new(ratatui::backend::TestBackend::new(60, 10)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let buf = term.backend().buffer();
        let a = buf.area;
        let c: String = (0..a.height)
            .flat_map(|y| (0..a.width).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect::<String>()
            // ratatui 画宽字符只写首格、第二格留空，不去掉空白的话
            // 「中文」在缓冲里是「中 文」（见 mod.rs 里同类测试的注释）
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        for l in Lang::all() {
            assert!(
                c.contains(l.native_name()),
                "{:?} 要用它自己的语言列出来：{c}",
                l
            );
        }
    }

    /// 设置页顶层列出的是设置项，不是语言名字——改结构之后画的应该是
    /// 「语言」「Phone」这两行，而不是曾经的「English」「简体中文」。
    #[test]
    fn the_top_level_page_lists_settings_items_not_languages() {
        let (mut app, _dir) = App::test_app();
        on_settings_items(&mut app, 0);
        let mut term = Terminal::new(ratatui::backend::TestBackend::new(60, 10)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let buf = term.backend().buffer();
        let a = buf.area;
        let c: String = (0..a.height)
            .flat_map(|y| (0..a.width).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect::<String>()
            // 宽字符（中文）在缓冲里第二格留空，不去掉空白的话「语言」会变成
            // 「语 言」，见 `every_language_is_listed_in_its_own_language` 的
            // 同一条注释。
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            c.contains(text(Key::Language, app.lang)),
            "顶层要写「语言」这个设置项：{c}"
        );
        assert!(
            c.contains(text(Key::Phone, app.lang)),
            "顶层要写「Phone」这个设置项：{c}"
        );
    }
}
