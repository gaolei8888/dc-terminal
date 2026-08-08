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
use super::view::{HelpCtx, View};
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
///
/// **同一个理由，`ctx` 也必须进来。** 「不许宣传一个按不动的键」这条规矩
/// 不分屏：以前 `s`/`u`/`d` 靠底栏按 `HelpCtx` 过滤，浮层跟着沾光；底栏
/// 收成三个位子之后，这三个键唯一的落点就是这一屏，它再无条件列出来，
/// 那条规矩就整个没人执行了——对着一个命令行会话写 `u 回滚`，按下去
/// 拿到的是 `NotAnAgentSession`。
fn groups(from: &View, ctx: HelpCtx, lang: Lang) -> Vec<Group> {
    let in_grid = matches!(from, View::Grid { .. });
    // 方向键这一条不设前提，跟下面那些键不同。理由不是「看板上总有一个组」——
    // 那句话是假的：`App::new` 的 `pinned` 是空的，开机补启动目录那一下
    // （`seed_start_project`）要等守护进程第一次 `List` 回来才发生，中间那几十
    // 毫秒看板真的一行都没有。写在这里的理由是**代价**：其余的键按不动时会
    // 弹一句错（`u` 拿到 `NotAnAgentSession`）或者悄悄什么都不发生，用户会
    // 以为自己按错了；空列表上按方向键则是所有人对列表的既有预期，没有任何
    // 需要解释的后果。而且这一条是这一组的**锚**——把它也去掉，那半秒里
    // 「走动」组会是个空标题。
    let mut move_keys: Vec<HelpItem> = help_items(
        &[if in_grid {
            ("", Key::MoveArrows)
        } else {
            ("↑↓", Key::Select)
        }],
        lang,
    );
    // 折叠只有看板绑着（`board::handle_key` 的 Left/Right/Space）；九宫格
    // 那边左右键是移动焦点，写上去就是教人按错。
    //
    // 键名列写 `Space` 不写 `空格`：这一列是**键盘上那个键叫什么**，跟界面
    // 语言无关，整个仓库都是 `Tab`/`Enter`/`Esc`/`Ctrl+Q`/`F3` 这样原样写的。
    // 写成中文的话英文用户会看到 `←→/空格 fold`——而 i18n 那条「英文里不许
    // 有汉字」的守卫只扫 `text()`，看不见键名列。现在下面
    // `no_key_column_is_ever_written_in_chinese` 把这一列也扫上了。
    if !in_grid {
        move_keys.extend(help_items(&[("←→/Space", Key::ToggleCollapse)], lang));
    }
    // `Enter` 没有作用对象时不写：列表停在组头上、九宫格一个活着的会话
    // 都没有，按下去都是无声无息。
    if ctx.selected.is_some() {
        move_keys.extend(help_items(
            &[if in_grid {
                ("Enter", Key::Zoom)
            } else {
                ("Enter", Key::Open)
            }],
            lang,
        ));
    }
    move_keys.extend(help_items(
        &[if in_grid {
            ("g", Key::List)
        } else {
            ("g", Key::Grid)
        }],
        lang,
    ));
    if in_grid && ctx.selected.is_some() {
        move_keys.extend(help_items(&[("i", Key::ReplyOnce)], lang));
    }
    // `Tab` 两个视图都绑着（`board::handle_key` / `grid::handle_key`），
    // 所以两屏都写。它是**日常换项目的主路径**，底栏那三个位子经常轮不到它
    // （九宫格里尤其：`Enter`/`i`/`n` 先占满），浮层里绝不能也没有。
    // 只有一个项目时它原地打转（见 `HelpCtx::can_switch_project`），那种时候
    // 两屏同样都不写——这一屏的作用是回答「我现在能按什么」，列一个按不动的
    // 键就是在骗人。
    // 数字键跟 `Tab` 是同一个动作的两种走法（都落到 `goto_project`/
    // `jump_project`），所以前提也是同一个：只有一个项目时 `1` 就是原地不动、
    // `2`…`9` 越界什么都不发生。两个视图都绑着，两屏都写。
    if ctx.can_switch_project {
        move_keys.extend(help_items(
            &[("Tab", Key::SwitchProject), ("1…9", Key::GotoProject)],
            lang,
        ));
    }

    // 作用在选中会话上的三个键，跟底栏当年那份判断逐条对上：停不掉的不写
    // `s`，没有检查点的不写 `u`/`d`。
    let mut session_keys = help_items(&[("n", Key::New), ("N", Key::SwitchAgent)], lang);
    if ctx.can_stop() {
        session_keys.extend(help_items(&[("s", Key::Stop)], lang));
    }
    if ctx.can_checkpoint() {
        session_keys.extend(help_items(&[("u", Key::Undo), ("d", Key::Diff)], lang));
    }

    vec![
        Group {
            title: Key::KeysGroupMove,
            items: move_keys,
        },
        Group {
            title: Key::KeysGroupSession,
            items: session_keys,
        },
        Group {
            title: Key::KeysGroupConfig,
            // `p` 写的是「加项目」不是「换项目」：换项目是 `Tab`（零弹窗、
            // 一个键），`p` 只剩「把一个看板上还没有的项目摆上来」这一件事。
            // 照着旧措辞按 `p` 的人会以为能一步换过去，弹出来的却是选择器。
            //
            // `x 移除` 现在两个视图都绑着，但只对「pinned 且空」的组管用
            // （`unpin_current` 的两条守卫）——不满足就不写，同上。
            items: {
                let mut v = help_items(&[("p", Key::AddProject)], lang);
                if ctx.can_remove {
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
    // 按**开门之前那一屏**算上下文，不是按 `app.view`（那是 `View::Keys`
    // 自己）。从九宫格按 `?`，问的得是那一格的状态。
    let groups = groups(from, super::help_ctx_for(app, from), app.lang);

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

    /// 「什么都能按」那一档：选中一个正在跑的 agent 会话、看板上有第二个
    /// 项目、光标所在的组可以移除。浮层现在按上下文过滤，所以「某个键在不在
    /// 这一屏上」的断言必须先把它能按的前提摆出来。
    fn everything_available() -> HelpCtx {
        HelpCtx {
            selected: Some(crate::ui::view::SelectedSession {
                is_agent: true,
                state: crate::session::SessionState::Idle,
            }),
            can_remove: true,
            can_switch_project: true,
        }
    }

    fn listed(from: &View, ctx: HelpCtx) -> String {
        groups(from, ctx, Lang::Zh)
            .iter()
            .map(|g| help_text(&g.items))
            .collect::<Vec<_>>()
            .join("  ")
    }

    /// 底栏丢掉的键必须**全部**在这一屏上。这条是整个设计的支点：少一个，
    /// 那个键就成了「屏幕上没写却真管用」的键，而这正是要消灭的东西。
    #[test]
    fn every_key_the_bar_drops_is_in_here() {
        let listed = listed(&View::Board, everything_available());
        for k in [
            "n 新建",
            "N 换 agent",
            "s 停止",
            "u 回滚",
            "d 改动",
            "p 加项目",
            "x 移除",
            "Tab 换项目",
            "1…9 直达项目",
            "←→/Space 折叠",
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
        let grid = listed(&View::grid(0), everything_available());
        assert!(grid.contains("Enter 放大"), "{grid}");
        assert!(grid.contains("g 列表"), "{grid}");
        assert!(grid.contains("i 回一句"), "{grid}");
        // 换项目那两个键两个视图都绑着，所以两屏都得写——底栏里它们经常
        // 被三条动作的上限挤掉，这一屏是它们唯一的落点。
        assert!(grid.contains("Tab 换项目"), "{grid}");
        assert!(grid.contains("1…9 直达项目"), "{grid}");
        assert!(grid.contains("x 移除"), "{grid}");
        // 但折叠只有看板绑着：九宫格的左右键是移动焦点
        assert!(
            !grid.contains("折叠"),
            "九宫格没绑折叠，写了就是教人按错：{grid}"
        );

        let board = listed(&View::Board, everything_available());
        assert!(board.contains("Enter 进会话"), "{board}");
        assert!(board.contains("g 九宫格"), "{board}");
        assert!(board.contains("←→/Space 折叠"), "{board}");
        assert!(
            !board.contains("i 回一句"),
            "列表里没有回复框，写了就是教人按错：{board}"
        );
    }

    /// **浮层同样不许宣传一个按不动的键。**
    ///
    /// 底栏收成三个位子之后，`s`/`u`/`d` 唯一的落点就是这一屏——它再无条件
    /// 列出来的话，「屏幕上写着做不到的操作」这条规矩就整个没人执行了。
    /// 三条各自的前提跟守护进程侧逐条对上：命令行会话没有检查点
    /// （`checkpoint_base` 直接返回 `NotAnAgentSession`），已经停掉的会话
    /// 再停一次只会得到一句错误。
    #[test]
    fn the_overlay_filters_the_keys_that_act_on_a_session() {
        use crate::session::SessionState;
        use crate::ui::view::SelectedSession;

        let shell = HelpCtx {
            selected: Some(SelectedSession {
                is_agent: false,
                state: SessionState::Idle,
            }),
            ..everything_available()
        };
        let s = listed(&View::Board, shell);
        assert!(!s.contains("u 回滚"), "命令行会话回滚不了：{s}");
        assert!(!s.contains("d 改动"), "命令行会话没有改动可看：{s}");
        assert!(s.contains("s 停止"), "但停得掉：{s}");

        let stopped = HelpCtx {
            selected: Some(SelectedSession {
                is_agent: true,
                state: SessionState::Stopped,
            }),
            ..everything_available()
        };
        let s = listed(&View::Board, stopped);
        assert!(!s.contains("s 停止"), "已经停了：{s}");
        assert!(s.contains("d 改动"), "停了的会话检查点还在：{s}");

        // 光标停在组头上：三个键一个都没有作用对象，`Enter` 也一样
        let header = HelpCtx {
            selected: None,
            ..everything_available()
        };
        let s = listed(&View::Board, header);
        for k in ["s 停止", "u 回滚", "d 改动", "Enter"] {
            assert!(!s.contains(k), "组头行上「{k}」按不动：{s}");
        }
        // 但这一屏不该因此变成空的——去别处的键照旧都在
        assert!(s.contains("n 新建") && s.contains("p 加项目"), "{s}");
    }

    /// `Tab` 和 `x` 在浮层里也按同一条规矩：只有一个项目时 `Tab` 原地打转，
    /// 组里还有会话时 `x` 会被拒绝。**两个视图同一条规矩**——它们现在两边
    /// 都绑着，一边写一边不写就是又分了岔。
    #[test]
    fn the_overlay_gates_tab_and_x_on_the_board_state() {
        let alone = HelpCtx {
            can_switch_project: false,
            can_remove: false,
            ..everything_available()
        };
        for from in [View::Board, View::grid(0)] {
            let s = listed(&from, alone);
            assert!(!s.contains("Tab"), "只有一个项目，Tab 什么都不做：{s}");
            assert!(
                !s.contains("1…9"),
                "数字键跟 Tab 同一个动作，前提也一样：{s}"
            );
            assert!(!s.contains("x 移除"), "非空组拿不掉：{s}");
        }
    }

    /// **键名列里不许出现汉字。**
    ///
    /// 按键表的每一条有两半：说明那一列走 `text()`，`i18n` 的
    /// `no_english_entry_contains_han_characters` 管得着；键名那一列是写死的
    /// 字面量，那条守卫**完全看不见**。于是 `("←→/空格", ToggleCollapse)`
    /// 这种错能一路走到英文界面上，显示成 `←→/空格 fold`，而所有测试都是绿的。
    ///
    /// 键名列写的是**键盘上那个键叫什么**，跟界面语言无关——所以顺带把「两种
    /// 语言下键名列必须一模一样」也钉住：哪天有人给某个键名加了译文，这条会红。
    /// 单独修掉一处不够，下一处照样是隐形的，所以补的是守卫不是补丁。
    #[test]
    fn no_key_column_is_ever_written_in_chinese() {
        use crate::i18n::has_han;
        // `View` 没有 `Debug`，失败信息里用「列表 / 九宫格」指回来。
        for (name, from) in [("列表", View::Board), ("九宫格", View::grid(0))] {
            for ctx in [
                everything_available(),
                HelpCtx {
                    selected: None,
                    can_remove: false,
                    can_switch_project: false,
                },
            ] {
                let en: Vec<String> = groups(&from, ctx, Lang::En)
                    .iter()
                    .flat_map(|g| g.items.iter().map(|it| it.key.to_string()))
                    .collect();
                let zh: Vec<String> = groups(&from, ctx, Lang::Zh)
                    .iter()
                    .flat_map(|g| g.items.iter().map(|it| it.key.to_string()))
                    .collect();
                for k in &en {
                    assert!(!has_han(k), "键名列里写了汉字：{k:?}（{name}）");
                }
                assert_eq!(en, zh, "键名列跟着语言变了（{name}）");
            }
        }
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
        // `s`/`u`/`d` 现在按上下文过滤（见 `the_overlay_filters_...`），
        // 所以要断言它们画得出来，得先真的有一个能停、能回滚的会话。
        app.set_sessions(vec![crate::session::SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/w/proj".into(),
            state: crate::session::SessionState::Working,
            activity: String::new(),
            is_agent: true,
        }]);
        app.list_state.select(Some(1));
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
