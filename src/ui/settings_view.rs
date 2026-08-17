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
use crate::settings::{save_lang, settings_path_for_socket};

use super::app::App;
use super::view::View;
use super::widgets::Msg;
use super::{dim, move_sel_n};

/// 设置页的条目。**加进第二项之前这一页是纯语言列表**，`ListState` 的下标
/// 直接映射 `Lang::all()`；现在映射这个枚举，选中语言那一项才进语言列表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsItem {
    Language,
    Phone,
}

impl SettingsItem {
    pub(crate) fn all() -> &'static [SettingsItem] {
        &[SettingsItem::Language, SettingsItem::Phone]
    }

    /// 越界返回 `None` 而不是兜底成第一项：`ListState` 的选中项可能停在
    /// 一个已经不存在的位置，那时候什么都不做，比默默把用户带进语言页好。
    pub(crate) fn at(i: usize) -> Option<SettingsItem> {
        SettingsItem::all().get(i).copied()
    }

    fn label(self, lang: Lang) -> &'static str {
        match self {
            SettingsItem::Language => text(Key::Language, lang),
            SettingsItem::Phone => text(Key::Phone, lang),
        }
    }
}

/// **这个函数里永远不要 `continue`。** 理由同 `board.rs`：循环末尾还有一段
/// 清理陈旧 `message` 的逻辑，跳过它会让一句普通反馈盖掉屏幕上唯一的出路。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Settings { state, lang } = app.view.clone() else {
        return Ok(());
    };

    if let Some(lang_state) = lang {
        return handle_language_key(app, key, state, lang_state);
    }
    handle_top_key(app, key, state)
}

/// 顶层设置项列表：`Language` 进语言子列表，`Phone` 眼下什么都不做——
/// 手机通知那一页是 Task 4 才建的 `View::Phone`，这里先按兵不动。
fn handle_top_key(app: &mut App, key: KeyEvent, mut state: ListState) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.view = super::home_view(app),
        KeyCode::Down | KeyCode::Up => {
            let d = if key.code == KeyCode::Down { 1 } else { -1 };
            move_sel_n(&mut state, SettingsItem::all().len(), d);
            app.view = View::Settings { state, lang: None };
        }
        KeyCode::Enter => match state.selected().and_then(SettingsItem::at) {
            Some(SettingsItem::Language) => {
                let mut lang_state = ListState::default();
                lang_state.select(Some(
                    Lang::all().iter().position(|l| *l == app.lang).unwrap_or(0),
                ));
                app.view = View::Settings {
                    state,
                    lang: Some(lang_state),
                };
            }
            // 手机通知页要等 Task 4 建好 `View::Phone` 才有地方去，这里先什么
            // 都不做——不是漏写，是这一项眼下还没有下一层。
            Some(SettingsItem::Phone) | None => {
                app.view = View::Settings { state, lang: None };
            }
        },
        _ => app.view = View::Settings { state, lang: None },
    }
    Ok(())
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
            app.view = View::Settings { state, lang: None };
        }
        KeyCode::Down | KeyCode::Up => {
            let d = if key.code == KeyCode::Down { 1 } else { -1 };
            move_sel_n(&mut lang_state, Lang::all().len(), d);
            app.view = View::Settings {
                state,
                lang: Some(lang_state),
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
                lang: Some(lang_state),
            }
        }
    }
    Ok(())
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let View::Settings { state, lang } = app.view.clone() else {
        return;
    };
    match lang {
        Some(lang_state) => draw_language_list(f, area, app, &lang_state),
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
            lang: None,
        };
    }

    /// 直接把光标摆进语言子列表——用来测子列表本身的行为，不用每次都先
    /// 走一遍「进设置页 → Enter「语言」」这条路。
    fn on_language_list(app: &mut App, selected: usize) {
        let mut lang_state = ListState::default();
        lang_state.select(Some(selected));
        app.view = View::Settings {
            state: ListState::default(),
            lang: Some(lang_state),
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

        let View::Settings { state, lang } = &app.view else {
            panic!("还应该在设置页");
        };
        assert!(lang.is_none(), "顶层列表移动不该顺手进子列表");
        assert_eq!(
            state.selected().and_then(SettingsItem::at),
            Some(SettingsItem::Phone)
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

        let View::Settings { lang, .. } = &app.view else {
            panic!("还应该在设置页");
        };
        let lang_state = lang.as_ref().expect("该进语言子列表了");
        assert_eq!(
            lang_state.selected(),
            Some(1),
            "光标要预先落在当前语言（Zh）上"
        );
        assert_eq!(app.lang, Lang::Zh, "只是打开子列表，还没真的选");
    }

    /// **手机通知眼下是空按钮。** `View::Phone` 是 Task 4 才建的，这里选中
    /// 「Phone」按 Enter 只能原地不动，不能 panic，也不能悄悄改语言。
    #[test]
    fn choosing_phone_does_nothing_yet() {
        let (mut app, _dir) = App::test_app();
        app.lang = Lang::Zh;
        on_settings_items(&mut app, 1);

        handle_key(&mut app, key(KeyCode::Enter)).unwrap();

        assert_eq!(app.lang, Lang::Zh, "选 Phone 不该改语言");
        assert!(
            matches!(app.view, View::Settings { .. }),
            "还应该停在设置页上"
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
        let View::Settings { lang, .. } = &app.view else {
            panic!("应该退回设置页，不是看板");
        };
        assert!(lang.is_none(), "该退回设置项列表，不是留在子列表里");
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
