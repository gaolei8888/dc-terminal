//! 设置页：目前只有语言一项。看板按 `l` 进。
//!
//! 跟 `secret.rs` 的密钥页分开是两码事——那边管「哪个 agent 用哪把密钥」，
//! 这里管界面本身怎么显示。

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem};

use crate::i18n::{text, Key, Lang};
use crate::settings::{save_lang, settings_path_for_socket};

use super::app::App;
use super::view::View;
use super::widgets::Msg;
use super::{dim, move_sel_n};

/// **这个函数里永远不要 `continue`。** 理由同 `board.rs`：循环末尾还有一段
/// 清理陈旧 `message` 的逻辑，跳过它会让一句普通反馈盖掉屏幕上唯一的出路。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Settings { mut state } = app.view.clone() else {
        return Ok(());
    };
    match key.code {
        KeyCode::Esc => app.view = View::Board,
        KeyCode::Down | KeyCode::Up => {
            let d = if key.code == KeyCode::Down { 1 } else { -1 };
            move_sel_n(&mut state, Lang::all().len(), d);
            app.view = View::Settings { state };
        }
        KeyCode::Enter => {
            let chosen = state.selected().and_then(|i| Lang::all().get(i)).copied();
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
            app.view = View::Board;
        }
        _ => app.view = View::Settings { state },
    }
    Ok(())
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let View::Settings { state } = &app.view else {
        return;
    };
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
                    .borders(Borders::ALL)
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
    use ratatui::widgets::ListState;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn on_settings(app: &mut App, selected: usize) {
        let mut st = ListState::default();
        st.select(Some(selected));
        app.view = View::Settings { state: st };
    }

    /// 选中一种语言按 Enter：界面语言当场就变，并且落盘——下次开 dct 还是它。
    #[test]
    fn choosing_a_language_applies_it_and_writes_it_to_disk() {
        let (mut app, dir) = App::test_app();
        app.lang = Lang::Zh;
        let en_index = Lang::all().iter().position(|l| *l == Lang::En).unwrap();
        on_settings(&mut app, en_index);

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

    /// 语言列表用各自的语言写，光标能走遍每一行。
    #[test]
    fn every_language_is_listed_in_its_own_language() {
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
}
