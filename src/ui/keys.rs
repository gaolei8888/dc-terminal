//! 「全部按键」浮层：底栏那一行放不下的键都在这里。
//!
//! 为什么要有它：底栏只有一行（多一行就少一行内容区，九宫格在 80×24 下会
//! 直接跌破 `grid.rs` 的 `MIN_ROWS`），而能按的键有十来个。放不下的必须丢，
//! 但丢掉的键仍然真的管用——「屏幕上没写却真管用的键」正是这个仓库反复
//! 警惕的东西。底栏尾巴上那条 `? …` 是门，这一屏是门后。

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::app::App;
use super::dim;
use super::view::View;
use super::widgets::{display_width, help_spans, item_width, wrap_items};
use crate::i18n::{help_items, text, HelpItem, Key, Lang};

/// 浮层里的一组键。分组是为了能扫读：一屏十几个键平铺下来，用户只能一条条
/// 读过去；分成「走动 / 会话 / 设置」三组，他能直接跳到自己要找的那一类。
struct Group {
    title: Key,
    items: Vec<HelpItem>,
}

/// 三组键。按 `?` 之前那一屏决定几处措辞：从九宫格进来的，`Enter` 是放大、
/// `g` 回列表、还多一条 `i 回一句`；从列表进来的反过来。
///
/// 写成「跟着来路走」而不是列一张固定表，是因为这一屏的作用就是**替用户
/// 回答「我现在能按什么」**。列一张两个视图混在一起的表，等于把他刚躲开的
/// 那个问题又还给他。
fn groups(from: &View, lang: Lang) -> Vec<Group> {
    let in_grid = matches!(from, View::Grid { .. });
    let mut move_keys: Vec<HelpItem> = help_items(
        &[
            if in_grid {
                ("", Key::MoveArrows)
            } else {
                ("↑↓", Key::Select)
            },
            if in_grid {
                ("Enter", Key::Zoom)
            } else {
                ("Enter", Key::Open)
            },
            if in_grid {
                ("g", Key::List)
            } else {
                ("g", Key::Grid)
            },
        ],
        lang,
    );
    if in_grid {
        move_keys.extend(help_items(&[("i", Key::ReplyOnce)], lang));
    } else {
        // `Tab` 只在列表上绑着（`board::handle_key`），九宫格没有它——
        // 这一屏的作用是回答「我现在能按什么」，列一个这个视图按不动的键
        // 就是在骗人。列表这边它反倒是**日常换项目的主路径**，底栏那三个
        // 位子有时轮不到它，浮层里绝不能也没有。
        move_keys.extend(help_items(&[("Tab", Key::SwitchProject)], lang));
    }
    vec![
        Group {
            title: Key::KeysGroupMove,
            items: move_keys,
        },
        Group {
            title: Key::KeysGroupSession,
            items: help_items(
                &[
                    ("n", Key::New),
                    ("N", Key::SwitchAgent),
                    ("s", Key::Stop),
                    ("u", Key::Undo),
                    ("d", Key::Diff),
                ],
                lang,
            ),
        },
        Group {
            title: Key::KeysGroupConfig,
            // `p` 写的是「加项目」不是「换项目」：换项目是 `Tab`（零弹窗、
            // 一个键），`p` 只剩「把一个看板上还没有的项目摆上来」这一件事。
            // 照着旧措辞按 `p` 的人会以为能一步换过去，弹出来的却是选择器。
            //
            // `x 移除` 只在列表上绑着，而且只对空组管用（`unpin_current`）。
            // 它在底栏里只有光标停在那种组上时才写，浮层这边是常驻的一览表
            // ——不列的话，这个键就成了「屏幕上从没写过却真管用」的那种。
            items: {
                let mut v = help_items(&[("p", Key::AddProject)], lang);
                if !in_grid {
                    v.extend(help_items(&[("x", Key::RemoveProject)], lang));
                }
                v.extend(help_items(
                    &[
                        ("c", Key::Secrets),
                        ("l", Key::SettingsTitle),
                        ("q", Key::Quit),
                    ],
                    lang,
                ));
                v
            },
        },
    ]
}

/// 打开浮层。记住**开门之前那一屏**，不是 `home_view()` 算出来的家——
/// 从九宫格按 `?` 再关掉，必须回到刚才那个焦点格上。
pub(crate) fn open(app: &mut App) {
    app.view = View::Keys {
        from: Box::new(app.view.clone()),
    };
}

/// 这一屏什么都不做，只有「关掉」。
///
/// 关的键给了四个（Esc / `?` / `q` / Enter）而不是「按任意键」：用户是带着
/// 「我要按哪个键」的问题进来的，看完直接按那个键是最自然的动作——如果任意键
/// 都关窗，他按下的 `s` 会先被吃掉一次，得再按一遍才生效，而 `s` 停止不可撤销，
/// 那一下「好像没反应」很容易变成按两次。所以：只认关窗键，其余一律不理。
///
/// `q` 在这里是关窗不是退出 dct：左段写的就是「Esc 返回」，这一屏没有任何
/// 地方承诺过 `q` 会退出。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Keys { from } = app.view.clone() else {
        return Ok(());
    };
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::Char('q') => {
            app.view = *from;
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &App) {
    let View::Keys { from } = &app.view else {
        return;
    };
    let groups = groups(from, app.lang);

    // 先在「最多能有多宽」里折行，再按折出来的**实际**宽高裁浮层。
    //
    // 两步而不是直接按上限画：宽终端上这几组键只要五十来列，铺满 4/5 屏
    // 就是一个左边一行字、右边一大片空白的框——用户会去找那片空白里是不是
    // 还有东西。框贴着内容走，眼睛才知道到哪儿为止。
    let max_w = (area.width.saturating_mul(4) / 5).max(20);
    let inner_w = max_w.saturating_sub(4).max(16) as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut widest = 0usize;
    for (i, g) in groups.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        let title = text(g.title, app.lang);
        widest = widest.max(display_width(title));
        lines.push(Line::from(Span::styled(title.to_string(), dim())));
        for row in wrap_items(&g.items, inner_w.saturating_sub(2)) {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(help_spans(&row));
            // 缩进 2 + 各条自身宽度 + 条与条之间的两格分隔
            let row_w = 2
                + row.iter().map(|it| item_width(it)).sum::<usize>()
                + 2 * row.len().saturating_sub(1);
            widest = widest.max(row_w);
            lines.push(Line::from(spans));
        }
    }

    // 标题也算宽度：「全部按键」比内容窄得多，但英文下不一定。
    widest = widest.max(display_width(text(Key::AllKeys, app.lang)));
    let want_w = (widest as u16 + 4).min(max_w);
    let popup = popup_area(area, want_w, lines.len() as u16 + 2);
    f.render_widget(ratatui::widgets::Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(text(Key::AllKeys, app.lang)),
        ),
        popup,
    );
}

/// 居中，但**不铺满**：背后的看板/九宫格要还看得见，用户才知道自己只是叠了
/// 一层（跟项目选择器同一种呈现）。放不下就退化成整块区域——那时候「浮」
/// 已经没有意义了。
fn popup_area(area: Rect, want_w: u16, want_h: u16) -> Rect {
    let w = want_w.clamp(20, area.width).min(area.width);
    let h = want_h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::help_text;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn screen(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        let a = buf.area;
        (0..a.height)
            .flat_map(|y| (0..a.width).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    /// 底栏丢掉的键必须**全部**在这一屏上。这条是整个设计的支点：少一个，
    /// 那个键就成了「屏幕上没写却真管用」的键，而这正是要消灭的东西。
    #[test]
    fn every_key_the_bar_drops_is_in_here() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Board;
        let listed = groups(&View::Board, Lang::Zh)
            .iter()
            .map(|g| help_text(&g.items))
            .collect::<Vec<_>>()
            .join("  ");
        for k in [
            "n 新建",
            "N 换 agent",
            "s 停止",
            "u 回滚",
            "d 改动",
            "p 加项目",
            "x 移除",
            "Tab 换项目",
            "c 密钥",
            "l 设置",
            "q 退出",
            "g 九宫格",
        ] {
            assert!(listed.contains(k), "浮层里少了「{k}」：{listed}");
        }
    }

    /// 措辞跟着来路走：九宫格里 `Enter` 是放大、`g` 回列表，还多一条 `i`。
    /// 列一张两个视图混在一起的表，等于把「我现在能按什么」这个问题又还给用户。
    #[test]
    fn the_wording_follows_where_you_came_from() {
        let grid = groups(&View::grid(0), Lang::Zh)
            .iter()
            .map(|g| help_text(&g.items))
            .collect::<Vec<_>>()
            .join("  ");
        assert!(grid.contains("Enter 放大"), "{grid}");
        assert!(grid.contains("g 列表"), "{grid}");
        assert!(grid.contains("i 回一句"), "{grid}");

        let board = groups(&View::Board, Lang::Zh)
            .iter()
            .map(|g| help_text(&g.items))
            .collect::<Vec<_>>()
            .join("  ");
        assert!(board.contains("Enter 进会话"), "{board}");
        assert!(board.contains("g 九宫格"), "{board}");
        assert!(
            !board.contains("i 回一句"),
            "列表里没有回复框，写了就是教人按错：{board}"
        );
    }

    /// 浮层不是全屏接管：背后那一屏要还看得见。
    #[test]
    fn the_overlay_leaves_the_board_visible_behind_it() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![crate::session::SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/w/proj".into(),
            state: crate::session::SessionState::Idle,
            activity: "背后的看板".into(),
            is_agent: true,
        }]);
        open(&mut app);

        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| super::super::draw(f, &mut app)).unwrap();
        let c = screen(&term);
        assert!(c.contains("全部按键"), "浮层自己要画出来：{c}");
        assert!(c.contains("背后的看板"), "背后的看板必须还看得见：{c}");
    }

    /// 80×24 是最常见的终端下限：这一屏必须在那里也画得全，否则「找回被丢掉
    /// 的键」这条路在最需要它的尺寸上断掉。
    #[test]
    fn the_overlay_fits_in_eighty_by_twenty_four() {
        let (mut app, _dir) = App::test_app();
        open(&mut app);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| super::super::draw(f, &mut app)).unwrap();
        let c = screen(&term);
        for k in ["n新建", "N换agent", "s停止", "u回滚", "d改动", "l设置"] {
            assert!(c.contains(k), "80×24 下「{k}」没画出来：{c}");
        }
    }

    /// 宽终端上浮层要贴着内容走，不能按屏幕比例铺开。左边一行字、右边一大片
    /// 空白的框，会让用户去找那片空白里是不是还有东西没显示。
    #[test]
    fn the_overlay_hugs_its_content_on_a_wide_terminal() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Board;
        open(&mut app);
        let mut term = Terminal::new(TestBackend::new(200, 30)).unwrap();
        term.draw(|f| super::super::draw(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        let top = (0..buf.area.height)
            .find(|y| {
                (0..buf.area.width)
                    .filter_map(|x| buf.cell((x, *y)))
                    .any(|c| c.symbol() == "全")
            })
            .expect("浮层顶边总该在屏幕上");
        let frame_w = (0..buf.area.width)
            .filter(|x| {
                buf.cell((*x, top)).is_some_and(|c| {
                    matches!(c.symbol(), "┌" | "─" | "┐" | "全" | "部" | "按" | "键")
                })
            })
            .count();
        assert!(
            frame_w < 80,
            "200 列下浮层宽了 {frame_w} 列——该贴着内容，不是按屏幕比例铺开"
        );
    }

    /// 关掉之后回到**开门之前那一屏**，不是回看板：从九宫格按 `?` 再退出来
    /// 落回列表，等于用户按一下问号顺手换了个视图。
    #[test]
    fn closing_goes_back_to_where_it_was_opened_from() {
        let (mut app, _dir) = App::test_app();
        app.view = View::grid(2);
        open(&mut app);
        assert!(matches!(app.view, View::Keys { .. }));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE),
        )
        .unwrap();
        assert!(matches!(app.view, View::Grid { focus: 2, .. }));
    }

    /// `q` 在这一屏是关窗，不是退出 dct——左段写的就是「Esc 返回」，
    /// 这里没有任何地方承诺过 `q` 会退出。
    #[test]
    fn q_closes_the_overlay_instead_of_quitting() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Board;
        open(&mut app);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('q'), crossterm::event::KeyModifiers::NONE),
        )
        .unwrap();
        assert!(!app.quit, "在按键表里按 q 不该退出整个 dct");
        assert!(matches!(app.view, View::Board));
    }

    /// 别的键一律不理。任意键都关窗的话，用户看完直接按 `s`，那一下会先被
    /// 吃掉——而 `s 停止` 不可撤销，「好像没反应」很容易变成按两次。
    #[test]
    fn other_keys_do_nothing_here() {
        let (mut app, _dir) = App::test_app();
        app.view = View::Board;
        open(&mut app);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('s'), crossterm::event::KeyModifiers::NONE),
        )
        .unwrap();
        assert!(matches!(app.view, View::Keys { .. }), "s 不该关窗");
    }
}
