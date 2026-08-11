//! 设置页：设置项列表，看板按 `l` 进。今天两项——语言、手机通知，选中
//! 「手机通知」进的是 `View::Phone`（见 `ui/phone.rs`）。
//!
//! 跟 `secret.rs` 的密钥页分开是两码事——那边管「哪个 agent 用哪把密钥」，
//! 这里管界面本身怎么显示。
//!
//! **这一页原来就是语言列表**，`ListState` 的下标直接映射 `Lang::all()`。
//! 加了第二项之后光标含义分成了两层：`View::Settings.state` 走顶层
//! `SettingsItem::all()`，选中「语言」才会开出 `lang` 那个子列表、下标
//! 才改映射 `Lang::all()`。层级语义全记在 `View::Settings.lang` 是
//! `Some` 还是 `None` 上，见它自己的字段注释。

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::i18n::{text, Key, Lang};
use crate::settings::{save_lang, settings_path_for_socket};

use super::app::App;
use super::view::{SettingsItem, View};
use super::widgets::Msg;
use super::{dim, move_sel_n};

/// **这个函数里永远不要 `continue`。** 理由同 `board.rs`：循环末尾还有一段
/// 清理陈旧 `message` 的逻辑，跳过它会让一句普通反馈盖掉屏幕上唯一的出路。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Settings { state, lang } = app.view.clone() else {
        return Ok(());
    };

    if let Some(ls) = lang {
        handle_language_list(app, key, state, ls);
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => app.view = super::home_view(app),
        KeyCode::Down | KeyCode::Up => {
            let mut state = state;
            let d = if key.code == KeyCode::Down { 1 } else { -1 };
            move_sel_n(&mut state, SettingsItem::all().len(), d);
            app.view = View::Settings { state, lang: None };
        }
        KeyCode::Enter => match state.selected().and_then(SettingsItem::at) {
            Some(SettingsItem::Language) => {
                // 进语言子列表：光标预选到当前语言。这条意图以前长在
                // `open_settings`（一开设置页就见得到「现在是哪个」）——
                // 顶层列表现在装的是设置项，语言下标对它没有意义了，
                // 意图本身没丢，只是挪到了这一步，真正该问「现在是哪个
                // 语言」的地方。
                let mut ls = ListState::default();
                ls.select(Lang::all().iter().position(|l| *l == app.lang));
                app.view = View::Settings {
                    state,
                    lang: Some(ls),
                };
            }
            Some(SettingsItem::Phone) => {
                // 现查一次现在是什么状态——`Request::PhoneStatus` 在守护
                // 进程那侧是纯内存读，不打网络，这次同步调用不会卡界面。
                // 拿不到就从 `Off` 起步：这是第一次打开这一页，没有「原来」
                // 可留（同 `fetch_phone_status` 文档注释里的约定）。
                let status = super::fetch_phone_status(
                    app,
                    crate::proto::PhoneStatus {
                        state: crate::proto::PhoneState::Off,
                        bot: None,
                        owner: None,
                    },
                );
                app.view = View::Phone {
                    status,
                    entry: None,
                };
            }
            // 下标越界（比如列表变短后光标停在旧位置）：什么都不做，
            // 比默默选中第一项更诚实——见 `SettingsItem::at` 的文档。
            None => app.view = View::Settings { state, lang: None },
        },
        _ => app.view = View::Settings { state, lang: None },
    }
    Ok(())
}

/// 语言子列表的按键——**这就是改结构之前的整页逻辑**，原样搬过来，只是
/// 从「唯一一层」变成了「深入之后的这一层」。
fn handle_language_list(app: &mut App, key: KeyEvent, top: ListState, mut ls: ListState) {
    match key.code {
        // 退一层回顶层设置项列表，不是回看板——Ctrl+Q 见
        // `view::back_one_level` 里对应的那一条，两边必须一致，不然
        // 同一屏里两个逃生键说的不是同一件事。
        KeyCode::Esc => {
            app.view = View::Settings {
                state: top,
                lang: None,
            }
        }
        KeyCode::Down | KeyCode::Up => {
            let d = if key.code == KeyCode::Down { 1 } else { -1 };
            move_sel_n(&mut ls, Lang::all().len(), d);
            app.view = View::Settings {
                state: top,
                lang: Some(ls),
            };
        }
        KeyCode::Enter => {
            let chosen = ls.selected().and_then(|i| Lang::all().get(i)).copied();
            if let Some(l) = chosen {
                app.lang = l;
                // 立刻写盘。不写的话用户下次开 dct 发现语言变回去了，
                // 而他明明记得自己选过——这正是 `save_lang` 返回 `Result`
                // 而不是像「最近项目」那样吞掉错误的理由。
                let path = settings_path_for_socket(&app.socket);
                match save_lang(&path, l) {
                    // 语言已经切了，这句反馈用的就是新语言——用户按下 Enter
                    // 之后第一眼看到的就是切换生效的证据。
                    Ok(()) => app.message = text(Key::SettingsTitle, l).into(),
                    Err(e) => app.message = Msg::err(format!("{e}")),
                }
            }
            app.view = super::home_view(app);
        }
        _ => {
            app.view = View::Settings {
                state: top,
                lang: Some(ls),
            }
        }
    }
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let View::Settings { state, lang } = &app.view else {
        return;
    };

    let border_style = if app.connected {
        Style::default()
    } else {
        Style::default().fg(Color::Red)
    };

    if let Some(ls) = lang {
        // 语言子列表：画法跟改结构之前的整页一模一样——各语言用自己的
        // 语言写，当前项打勾。误切到看不懂的语言之后，这个符号是跨语言
        // 都认得的线索，这条道理没有因为多了一层而改变。
        let items: Vec<ListItem> = Lang::all()
            .iter()
            .map(|l| {
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
        let mut s = ls.clone();
        f.render_stateful_widget(
            List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(
                            "{} · {}",
                            text(Key::SettingsTitle, app.lang),
                            text(Key::Language, app.lang)
                        ))
                        .border_style(border_style),
                )
                .highlight_symbol("▶ "),
            area,
            &mut s,
        );
        return;
    }

    let items: Vec<ListItem> = SettingsItem::all()
        .iter()
        .map(|item| {
            let label = match item {
                SettingsItem::Language => text(Key::Language, app.lang),
                SettingsItem::Phone => text(Key::Phone, app.lang),
            };
            ListItem::new(Line::from(Span::raw(label)))
        })
        .collect();

    let mut s = state.clone();
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(text(Key::SettingsTitle, app.lang))
                    .border_style(border_style),
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

    /// 光标停在顶层设置项列表的第 `selected` 项上。
    fn on_settings(app: &mut App, selected: usize) {
        let mut st = ListState::default();
        st.select(Some(selected));
        app.view = View::Settings {
            state: st,
            lang: None,
        };
    }

    /// 光标已经深入语言子列表，停在第 `selected` 种语言上——测「选中语言
    /// 按 Enter 怎么样」不需要先测「顶层怎么钻进去」，两件事分开测，一个
    /// 出错时另一个不会跟着一起红。
    fn on_language_list(app: &mut App, selected: usize) {
        let mut ls = ListState::default();
        ls.select(Some(selected));
        app.view = View::Settings {
            state: ListState::default(),
            lang: Some(ls),
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

    /// 选中一种语言按 Enter：界面语言当场就变，并且落盘——下次开 dct 还是它。
    /// 这是从改结构之前原样搬过来的回归测试，只是入口从「顶层」换成了
    /// 「已经在语言子列表里」。
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

    /// Esc 不改任何东西。设置页最怕的就是「路过一下就把配置改了」。
    #[test]
    fn escaping_out_of_settings_changes_nothing() {
        let (mut app, _dir) = App::test_app();
        app.lang = Lang::Zh;
        on_settings(&mut app, 0);

        handle_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert_eq!(app.lang, Lang::Zh, "Esc 不该改语言");
        assert!(matches!(app.view, View::Board));
        assert!(
            crate::settings::load_lang(&settings_path_for_socket(&app.socket)).is_none(),
            "Esc 不该写盘"
        );
    }

    /// 深入语言子列表之后 Esc 退一层回顶层设置项列表，不是直接回看板——
    /// 这是这次重构本身引入的新台阶，得有测试钉住它，不然下一次改动会把
    /// 它和顶层的 Esc 语义弄混，两个逃生键说的就不再是同一件事。
    #[test]
    fn escaping_the_language_list_goes_back_to_the_settings_list_not_the_board() {
        let (mut app, _dir) = App::test_app();
        on_language_list(&mut app, 0);

        handle_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert!(
            matches!(app.view, View::Settings { lang: None, .. }),
            "Esc 从语言子列表出来应该退回顶层设置项列表"
        );
    }

    /// 顶层选中「语言」按 Enter 要能钻进语言子列表，并且光标预选在当前
    /// 语言上——这份「进来第一眼看到现在是哪个」的意图以前长在
    /// `open_settings` 里，这次重构把它挪到了这一步，得有测试接住，不然
    /// 下一次改动会以为「预选当前项」只是巧合。
    #[test]
    fn entering_the_language_item_opens_the_list_preselected_on_the_current_language() {
        let (mut app, _dir) = App::test_app();
        app.lang = Lang::Zh;
        let top = SettingsItem::all()
            .iter()
            .position(|i| *i == SettingsItem::Language)
            .unwrap();
        on_settings(&mut app, top);

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        match &app.view {
            View::Settings { lang: Some(ls), .. } => {
                let zh_index = Lang::all().iter().position(|l| *l == Lang::Zh).unwrap();
                assert_eq!(ls.selected(), Some(zh_index), "要预选在当前语言上");
            }
            _ => panic!("选中「语言」按 Enter 应该钻进语言子列表"),
        }
    }

    /// 顶层选中「手机通知」按 Enter：这一屏不再是空操作（那是 Task 3 留的
    /// 占位，见它自己的报告里「Placeholder left for Task 4」一节）——现在
    /// 要真的钻进 `View::Phone`。`App::test_app()` 连不上真实守护进程，
    /// `fetch_phone_status` 拿不到答案时退化成 `Off`，这条测试断言的正是
    /// 这个诚实的兜底，不是「什么都可能发生」。
    #[test]
    fn entering_the_phone_item_opens_the_phone_view() {
        let (mut app, _dir) = App::test_app();
        let phone = SettingsItem::all()
            .iter()
            .position(|i| *i == SettingsItem::Phone)
            .unwrap();
        on_settings(&mut app, phone);

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        match &app.view {
            View::Phone { status, entry } => {
                assert_eq!(
                    status.state,
                    crate::proto::PhoneState::Off,
                    "连不上时退化成 Off"
                );
                assert!(entry.is_none(), "刚打开这一页不该带着一个正在填的输入框");
            }
            _ => panic!("选中「手机通知」按 Enter 应该打开 View::Phone"),
        }
    }

    /// **不能靠巧合过关。** 上一条测试连不上真守护进程，`fetch_phone_status`
    /// 断线时的兜底恰好也是 `Off`——如果这一支被悄悄改成直接写死
    /// `PhoneStatus { state: Off, .. }`、压根不去问守护进程，上一条测试
    /// 照样通过。起一个真守护进程，提前塞一个令牌进它的 `secrets.toml`
    /// （`daemon.rs` 见到它会报 `WaitingForPairing`），确认这里拿到的是
    /// 这个真答案，才能证明这一支真的调用了 `fetch_phone_status`，不是
    /// 现编了一个巧合相等的默认值。
    #[test]
    fn entering_the_phone_item_reaches_the_real_daemon_not_a_hardcoded_default() {
        use crate::client::Client;
        use crate::secrets::{secrets_path_for_socket, SecretStore, PHONE_TOKEN_KEY};

        let home = tempfile::tempdir().unwrap();
        let sock = home.path().join("daemon.sock");
        // 必须在起 daemon 之前把令牌写好，理由同 `ui::mod::tests::
        // fetch_phone_status_reaches_the_real_daemon_when_connected`。
        let mut disk = SecretStore::load(&secrets_path_for_socket(&sock));
        disk.set(PHONE_TOKEN_KEY, "pre-seeded-token").unwrap();

        super::super::start_daemon_at(&sock);

        let work = tempfile::tempdir().unwrap();
        let mut app = App::new(
            Client::connect(&sock).unwrap(),
            work.path().to_path_buf(),
            crate::i18n::Lang::Zh,
            sock.clone(),
            super::super::view::ViewMode::List,
        );
        let phone = SettingsItem::all()
            .iter()
            .position(|i| *i == SettingsItem::Phone)
            .unwrap();
        on_settings(&mut app, phone);

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        match &app.view {
            View::Phone { status, .. } => assert_eq!(
                status.state,
                crate::proto::PhoneState::WaitingForPairing,
                "该拿到守护进程的真答案"
            ),
            _ => panic!("选中「手机通知」按 Enter 应该打开 View::Phone"),
        }
    }

    /// 方向键要能从「语言」走到「手机通知」。`move_sel_n` 的长度参数如果
    /// 悄悄改回 `Lang::all().len()`，今天两份长度数值上都是 2，看不出
    /// 差别——这条测试不问长度参数从哪儿来，只问「按下去到没到」，是这份
    /// 数值巧合之外唯一还能钉住这条移动的测试（见任务报告的变异测试表）。
    #[test]
    fn pressing_down_from_language_selects_phone() {
        let (mut app, _dir) = App::test_app();
        on_settings(&mut app, 0);

        handle_key(&mut app, key(KeyCode::Down)).unwrap();

        match &app.view {
            View::Settings {
                state, lang: None, ..
            } => {
                assert_eq!(
                    state.selected().and_then(SettingsItem::at),
                    Some(SettingsItem::Phone),
                    "↓ 应该能从语言走到手机通知"
                );
            }
            _ => panic!("方向键应该还在顶层设置项列表里"),
        }
    }

    /// 这两个长度一相等，`handle_key` 里 move_sel_n 的长度参数写错也测不出来
    /// （见 task-3 报告的变异 #2）。哪天不相等了，这条会红——那时候补一条
    /// 「↓ 能走到最后一个设置项」的测试，把长度来源真正钉住。
    #[test]
    fn the_length_coincidence_that_hides_a_wrong_move_sel_n_source() {
        assert_eq!(
            SettingsItem::all().len(),
            Lang::all().len(),
            "长度不再巧合相等了：去补一条方向键能走到最后一项设置的测试，\
             把 move_sel_n 的长度来源钉死，不能再靠这份巧合掩护"
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

    /// 顶层列表把两个设置项都列出来，各自用界面语言写——手机通知今天还是
    /// 占位，但它已经是一个用户看得见、选得中的行，标签不能漏。
    #[test]
    fn the_top_level_list_shows_both_settings_items() {
        let (mut app, _dir) = App::test_app();
        on_settings(&mut app, 0);
        let mut term = Terminal::new(ratatui::backend::TestBackend::new(60, 10)).unwrap();
        term.draw(|f| draw(f, f.area(), &mut app)).unwrap();
        let buf = term.backend().buffer();
        let a = buf.area;
        let c: String = (0..a.height)
            .flat_map(|y| (0..a.width).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            c.contains(
                &text(Key::Language, app.lang)
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>()
            ),
            "顶层列表要列出「语言」：{c}"
        );
        assert!(
            c.contains(
                &text(Key::Phone, app.lang)
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>()
            ),
            "顶层列表要列出「手机通知」：{c}"
        );
    }
}
