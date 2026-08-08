use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, ListState, Paragraph};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use crate::client::Client;
use crate::proto::{ProfileEntry, Request, Response};
use crate::session::SessionInfo;
use crate::theme::Theme;

mod widgets;
use widgets::short_path;
pub use widgets::{status_label, status_style, Msg};

mod app;
use app::App;

mod attach;
mod board;
mod grid;
mod keys;
mod pick;
mod secret;
mod settings_view;

mod view;
use view::SecretPhase;
use view::{
    back_one_level, escape_hint, idle_help, is_ctrl_q, message_after_transition,
    session_ended_notice, Scope, View,
};
pub use view::{
    clean_secret, decide_delete_key, digit_index, pick_action, quick_start_target, secret_rows,
    verify_message, verify_outcome_applies_to, PickAction, ViewMode,
};

/// 启动时探测出来的终端背景。`run()` 设一次，之后只读。
///
/// 用全局而不是给 `DrawInput` 之类的渲染入参加字段：主题是进程级配置，
/// 启动后不变，塞进每帧的入参是把一个常量伪装成状态；而渲染函数散在
/// `board`/`grid`/`pick`/`secret` 四个模块里，加一个必填字段就是几十处
/// 纯噪音的改动（测试里的构造点尤其多）。
static THEME: OnceLock<Theme> = OnceLock::new();

/// 探测终端背景并记下来。`run()` 在 `enable_raw_mode()` 之后、
/// `EnterAlternateScreen` 之前调，只调一次。
pub fn init_theme() {
    let _ = THEME.set(crate::theme::detect());
}

/// 弱化文字（说明栏、提示、不可用项、九宫格里没聚焦的格子）统一用这个样式。
///
/// 不能用 `Color::DarkGray`：它是 ANSI 亮黑（8 号色），Solarized Dark 等主题
/// 把 8 号色设成和背景同色，整段文字直接隐形——选 agent 菜单里所有不可用项和
/// 说明栏就这样消失过，只剩一个悬空的 ▶。
///
/// 也不能写死一个 256 色的灰：那治好了深色背景，却在浅色背景上同样接近隐形。
/// 一个写死的灰不可能同时适配深浅两种底色，所以跟着探测出来的背景走
/// （`Dark` 用偏亮的灰、`Light` 用偏暗的灰、探不出来就用终端自己的 DIM
/// 属性，见 `theme::Theme::dim`）。
///
/// 没探测过就按 `Unknown` 算——那是三种取值里最保守的一个（只挂 DIM 修饰符，
/// 不钉任何颜色），所以测试和任何绕过 `run()` 的路径都能正常渲染。
pub fn dim() -> Style {
    THEME.get().copied().unwrap_or(Theme::Unknown).dim()
}

/// 还原终端：退出 raw mode、关掉括号粘贴、离开 alternate screen。
///
/// 抽成自由函数是因为有两个调用方——`TerminalGuard::drop` 和信号线程。
/// 两份各自维护的清理代码迟早会漂移，而漂移的后果是用户拿到一个半还原的终端。
///
/// 两步都 `let _ =` 吞错：`Drop` 里不能 panic，而且这里能做的补救本来就只有
/// 「尽量多还原一点」。
fn restore_terminal() {
    let _ = disable_raw_mode();
    // 无条件关鼠标捕获，不管这次运行有没有真的开过：没开过时多发一次关闭
    // 序列是无害的，而漏关会让用户的终端从此点哪儿都冒出 SGR 乱码——
    // 比翻不了历史严重得多，所以这里不像捕获本身那样只在会话里才动作。
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
}

/// 这一帧要不要开关鼠标捕获。`None` = 上一帧和这一帧「在不在会话里」
/// 没变，什么都不用做。
///
/// 抽成纯函数是因为副作用（`execute!` 往 stdout 写转义序列）没法单测，
/// 判断「变没变」这件事能测——而且判断错了后果不轻：漏开会让滚轮/点击
/// 走终端自己的选中逻辑而不是这套协议，漏关会让用户退回看板之后连
/// 拖选文字复制都做不了。
fn mouse_capture_transition(was_attached: bool, is_attached: bool) -> Option<bool> {
    if was_attached == is_attached {
        None
    } else {
        Some(is_attached)
    }
}

/// 兜底恢复终端状态。ratatui 的 `Terminal` 不会在 `Drop` 里自动退出 raw
/// mode / alternate screen；`run()` 的主循环里到处都是 `?`，一旦某次
/// `client.call`/`term.draw` 出错就会直接从函数返回，跳过写在循环末尾的清理代码，
/// 把用户的终端卡在 raw mode（回显、行缓冲全关）。这个 guard 保证不管是提前
/// `return`/`?`、正常 `break`，还是 panic 展开，`Drop` 都会跑一次。
///
/// 它盖不住的只剩信号——那条交给 `spawn_signal_restore`。
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// 让 SIGTERM / SIGINT / SIGHUP 也能还原终端。
///
/// 为什么不是信号 handler：handler 里能调的函数必须 async-signal-safe，而
/// crossterm 的 `disable_raw_mode()` 内部要锁一把全局 Mutex 去取原始 termios——
/// 信号打断的正好是持锁的主线程时就死锁。`sigwait` 在普通线程上下文里返回，
/// 之后跑的是普通代码，这个约束整个消失，也才谈得上跟 `TerminalGuard` 共用
/// 同一个 `restore_terminal()`。
///
/// 为什么不是「置个标志位让主循环自己退」：主循环卡在 `client.call` 上
/// （守护进程死了、socket 不回）时永远轮不到下一个 tick，而那正是用户会去
/// 别的窗口 kill 的场景——恰好是最需要它工作的时候不工作。
///
/// 屏蔽掩码会被子进程继承（`execve` 之后仍保留），但这里不用担心：TUI 进程
/// 在 `run()` 里不 fork 任何东西，PTY 全在守护进程里（`src/pty.rs`），而守护
/// 进程在 `src/main.rs:60` 就已经拉起，早于 `src/main.rs:72` 的 `ui::run`。
///
/// raw mode 下 Ctrl+C 不产生 SIGINT（termios 关了 ISIG），所以屏蔽 SIGINT
/// 不影响 Ctrl+C 透传给 agent；这条只对外部 `kill -INT` 生效。
fn spawn_signal_restore() {
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGHUP);
        // 主线程先屏蔽，之后 spawn 出来的线程继承这份掩码，
        // 于是这三个信号只会被下面的 sigwait 取走，不会走默认处置直接杀进程。
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }

    std::thread::spawn(move || {
        let mut signo: libc::c_int = 0;
        if unsafe { libc::sigwait(&set, &mut signo) } != 0 {
            return;
        }
        restore_terminal();
        // 不能用 `exit`：它会跑 atexit 和静态析构，而主线程此刻还在跑自己的事，
        // 两边可能同时清理终端或撞上同一把锁。终端已经在上一行还原好了，立刻走人。
        // 退出码 128 + signo 是 shell 惯例，SIGTERM 就是 143，脚本还能判断死因。
        unsafe { libc::_exit(128 + signo) };
    });
}

pub fn run(
    client: Client,
    default_dir: PathBuf,
    lang: crate::i18n::Lang,
    socket: PathBuf,
    view_mode: ViewMode,
) -> Result<()> {
    // 必须在 enable_raw_mode 之前装：装早了无害（还没进 raw mode 时
    // restore_terminal() 没有副作用，多发一次 LeaveAlternateScreen 也无害），
    // 装晚了就有一个「已经进 raw mode 但信号还没被接管」的真空窗口。
    // 跟 TerminalGuard 提前构造是同一个理由。
    spawn_signal_restore();
    enable_raw_mode()?;
    // 必须在 EnterAlternateScreen / Terminal::new 之前构造：这样即便它们俩失败，
    // raw mode 也还是能被 Drop 恢复。
    let _guard = TerminalGuard;
    // 探测终端背景，位置被两头夹死：
    // - 必须在 enable_raw_mode() 之后：OSC 11 的回复是终端塞进 stdin 的一串
    //   字节，非 raw 模式下会被行缓冲（它不带换行，读不出来）并且被回显到
    //   屏幕上（用户会看见乱码）。
    // - 必须在 EnterAlternateScreen 之前：万一有字节漏到屏幕上，此刻还在主屏、
    //   还没开始画界面，脏字符会被随后的 alternate screen 切换盖掉；反过来就是
    //   把乱码糊在已经画好的界面上。
    // 在 TerminalGuard 之后是为了万一探测里有什么 panic，raw mode 仍能恢复。
    init_theme();
    let mut stdout = std::io::stdout();
    // 开括号粘贴：不开的话粘贴的文字会一个字符一个事件地进来，
    // 粘一段话就是几百次往返，慢到没法用。
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App::new(client, default_dir, lang, socket, view_mode);
    // 鼠标捕获只在会话里开：看板不需要滚，而开着捕获会让终端原生的选中
    // 复制失效——把这个代价限制在真正需要它的地方。见下面 `term.draw`
    // 之前那段每帧检查一次的逻辑，以及它为什么不挂在某一个「进入会话」的
    // 分支上（进 `View::Attached` 的路不止一条：`enter_session`、密钥验证
    // 通过后直接建会话、九宫格里……挂哪个分支都会漏另一条）。
    let mut mouse_captured = false;

    // 有标签是因为下面排空鼠标事件那段需要从一个嵌套的 `while` 里跳回
    // 这个循环的顶部，而不是跳回 `while` 自己——普通的无标签 `continue`
    // 已经覆盖了这个函数里所有别的 `continue`，不用因为加了一个标签
    // 就把它们全部改成 `continue 'main`。
    'main: loop {
        // 收后台验证的结果，必须在 term.draw 之前——通过了要直接把视图
        // 切成新开的会话，不然用户看见的这一帧还是「正在验证…」，多闪一下。
        if let Some(rx) = &app.verify_rx {
            if let Ok((sent_profile, sent_buf, outcome)) = rx.try_recv() {
                // 不管接下来用不用得上这个结果，先把 Receiver 收掉：
                // 它已经出结果了，没有第二次可读。
                app.verify_rx = None;
                if let View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    return_to_settings,
                    ..
                } = app.view.clone()
                {
                    // 这条结果只有在「发起验证时的 (profile, buf)」跟「此刻
                    // 屏幕上这一份 (profile, buf)」完全一致时才有落点——见
                    // 上面声明 `verify_rx` 时的注释和
                    // `verify_outcome_applies_to` 的文档注释。用户可能在这次
                    // 网络探测跑着的时候已经 Ctrl+Q/Esc 退出去，甚至绕回来
                    // 在另一个 agent 身上重新填了密钥；这时候视图仍然是
                    // `EnterSecret`，光看"是不是这个变体"分不出是不是同一个
                    // 请求，必须把 profile 和 buf 都比对上。不满足就直接
                    // 扔掉，不切视图——套在一个不相干的 profile/密钥上
                    // 比什么都不做更危险（见 CRITICAL 1 的复现步骤）。
                    if verify_outcome_applies_to(&sent_profile, &sent_buf, &profile, &buf) {
                        app.view = match verify_message(outcome, app.lang) {
                            Some(m) => View::EnterSecret {
                                profile,
                                label,
                                prompt,
                                buf,
                                phase: SecretPhase::Failed(m),
                                return_to_settings,
                            },
                            // 通过：先存盘。存密钥必须先于「开会话」/「回设置页」两条
                            // 后续路径都成立的前提——回设置页要读一份刷新过的 has_secret
                            // 才能显示「已配」，开会话是从磁盘上已经存好的密钥里现读
                            // 一份给新会话用的（见 daemon.rs），顺序反了新会话拿到的
                            // 还是空密钥。
                            None => match app.client().and_then(|c| {
                                c.call(Request::SetSecret {
                                    profile: profile.clone(),
                                    value: buf.clone(),
                                })
                            }) {
                                Ok(Response::Ok) if return_to_settings => {
                                    // 从设置页进来的是「改配置」，不是「开工」——
                                    // 存完直接回设置页，不建会话。这里**不能**甩一个
                                    // 空壳指望循环收尾那段通用重拉逻辑去补：那段逻辑
                                    // 挂在按键处理之后，而这整段 verify_rx 分支跑在
                                    // 循环顶部、不受「这一轮有没有按键」摆布——如果
                                    // 用户这时候没再按键，`event::poll` 超时会直接
                                    // `continue` 到下一轮循环顶部，跳过收尾，空壳会
                                    // 一直空着，直到用户偶然按下一个键才被补上（手测
                                    // 时真的复现了：改完密钥，界面卡在一屏空列表，
                                    // 直到按了 Ctrl+Q 再按 c 才刷出来）。直接现查一遍，
                                    // 光标顺手定在刚改的这一行上。
                                    //
                                    // 改完给一句确认：这一行本身会从「未配」翻成
                                    // 「已配」，但删除那条路径有「已删除 X 的密钥」
                                    // 的消息条打底，改密钥这条路径原来什么都不说，
                                    // 是同一对镜像操作里唯一没反馈的一半——补齐。
                                    app.message =
                                        crate::i18n::msg::secret_saved(app.lang, &label).into();
                                    refetch_secrets(&mut app, Some(&profile))
                                }
                                Ok(Response::Ok) => {
                                    let dir = app.current_dir.display().to_string();
                                    match app.client().and_then(|c| {
                                        c.call(Request::Create {
                                            dir,
                                            profile: profile.clone(),
                                            remember: true,
                                        })
                                    }) {
                                        Ok(Response::Created { id }) => {
                                            app.need_sessions = true; // 会话标题要显示项目名
                                            View::Attached(id)
                                        }
                                        Ok(Response::Error(ref e)) => View::EnterSecret {
                                            profile,
                                            label,
                                            prompt,
                                            buf,
                                            phase: SecretPhase::Failed(crate::i18n::msg::error(
                                                app.lang, e,
                                            )),
                                            return_to_settings,
                                        },
                                        _ => View::EnterSecret {
                                            profile,
                                            label,
                                            prompt,
                                            buf,
                                            phase: SecretPhase::Failed(
                                                crate::i18n::text(
                                                    crate::i18n::Key::SessionOpenFailed,
                                                    app.lang,
                                                )
                                                .into(),
                                            ),
                                            return_to_settings,
                                        },
                                    }
                                }
                                Ok(Response::Error(ref e)) => View::EnterSecret {
                                    profile,
                                    label,
                                    prompt,
                                    buf,
                                    phase: SecretPhase::Failed(crate::i18n::msg::error(
                                        app.lang, e,
                                    )),
                                    return_to_settings,
                                },
                                _ => View::EnterSecret {
                                    profile,
                                    label,
                                    prompt,
                                    buf,
                                    phase: SecretPhase::Failed(
                                        crate::i18n::text(
                                            crate::i18n::Key::SecretNotSaved,
                                            app.lang,
                                        )
                                        .into(),
                                    ),
                                    return_to_settings,
                                },
                            },
                        };
                    }
                    // else：profile 或 buf 对不上——这条结果对应的是一个用户
                    // 已经离开的请求，扔了，不切视图。
                }
                // else：视图现在压根就不是 EnterSecret 了（比如用户 Esc/Ctrl+Q
                // 提前离开，切到了看板/选择器/设置页）。同样没有落点，扔了。
            }
        }

        let attached = matches!(app.view, View::Attached(_));
        if app.need_sessions || !attached {
            match app.client().and_then(|c| c.call(Request::List)) {
                Ok(Response::Sessions(v)) => {
                    app.set_sessions(v);
                    app.connected = true;
                }
                _ => app.connected = false,
            }
            app.need_sessions = false;
        }
        if app.list_state.selected().is_none() && !app.visible.is_empty() {
            app.list_state.select(Some(0));
        }
        // 会话可能在两轮之间消失（自己退了、被 s 停掉清了），焦点必须跟着
        // 收回来。不收的话 grid::move_focus 会拿到一个越界的下标——它的
        // debug_assert 就是为这条路径设的，而 release 下越界会算出一个荒唐
        // 的页长，格子全乱。收在这里（拉完列表、画之前）是唯一能保证
        // 渲染和按键看到的是同一个合法焦点的地方。
        if let View::Grid { focus, .. } = app.view {
            let last = app.visible.len().saturating_sub(1);
            if focus > last {
                app.view = View::grid(last);
            }
        }
        if let View::Grid { focus, .. } = app.view {
            let page = grid::page_of(focus);
            let start = page * grid::TILES_PER_PAGE;
            let ids: Vec<u32> = app
                .grid_visible
                .iter()
                .skip(start)
                .take(grid::TILES_PER_PAGE)
                .map(|s| s.id)
                .collect();
            // 300ms 一轮就够：格子是扫一眼的东西，不是打字的地方（附加视图
            // 的 16ms 是为了跟手，这里没有手要跟）。只有「翻了页（或刚进来）」
            // 才插队立刻取一次——那时候手里这批画面画的是别的会话，等满
            // 300ms 就是让新的一页空白着晾用户小半秒。这个条件取完就自己
            // 消掉，绕过节流最多一次。
            let page_changed = app.grid_page != Some(page);
            let due = page_changed
                || app
                    .grid_last_fetch
                    .is_none_or(|t| t.elapsed() >= Duration::from_millis(300));
            if due {
                match app.client().and_then(|c| c.call(Request::Screens { ids })) {
                    Ok(Response::Screens { screens }) => {
                        app.grid_screens = screens;
                        app.connected = true;
                    }
                    // 老守护进程不认识 Screens。列表视图还能用，退回去并
                    // 说清怎么修——别让用户对着一屏空格子猜。（`dct restart`
                    // 还不存在，所以只能说退出再启动。）
                    //
                    // 敢把 Error 一律诊断成「守护进程是旧版本」而不看里面写了
                    // 什么，靠的是一条事实：daemon 侧 `Screens` 那条分支
                    // （`daemon.rs` 的 `handle`）返回的永远是 `Ok`，`mgr.screens()`
                    // 不会失败——所以能走到这里的 Error 只可能是 `serve` 的
                    // 请求解析失败，而新客户端发的请求老守护进程解析不了，就是
                    // 版本对不上。**哪天 `screens()` 变成可能失败的，这句诊断就
                    // 成了假话**，那时必须改成把 Error 里的原文说给用户听。
                    Ok(Response::Error(_)) => {
                        app.message = Msg::err(
                            crate::i18n::text(crate::i18n::Key::DaemonTooOld, app.lang).into(),
                        );
                        app.view = home_view(&app);
                        app.need_sessions = true;
                    }
                    _ => app.connected = false,
                }
                app.grid_page = Some(page);
                app.grid_last_fetch = Some(std::time::Instant::now());
            }
        } else {
            // 离开九宫格就把「手里这批画面是哪一页的」忘掉，这样下次进来
            // `page_changed` 一定成立，第一帧插队立刻取一次，不用干等 300ms。
            // 这一句在 `grid_screens` 空不空之外：第一次取画面就失败的时候，
            // 画面是空的而 `grid_page` 已经被写上了，若跟着 `is_empty` 一起
            // 跳过重置，300ms 内退出再进来就是对着一屏空白熬满节流。
            app.grid_page = None;
            // 画面也扔掉。留着的话，下次再按 g 进来的第一帧画的是上一次的
            // 旧画面（可能是几分钟前的，甚至是已经没了的会话）。收在这里
            // 而不是在每个「离开九宫格」的按键分支里各清一次：出口有 g、
            // Ctrl+Q、Enter 放大、n/p/c 弹出的那几个视图……漏一个就是一帧
            // 残影，而这一条判断覆盖了全部。
            app.grid_screens.clear();
        }
        if let View::Attached(id) = &app.view {
            let id = *id;
            // 把 agent 画面区的真实大小告诉它。不做的话它永远按初始宽度排版，
            // 窗口再宽也只用左边一块。减 2 是边框。
            let area = term.size()?;
            let rows = area.height.saturating_sub(2 + 3);
            let cols = area.width.saturating_sub(2);
            if app.sent_size != Some((id, rows, cols))
                && rows > 0
                && cols > 0
                && app
                    .client()
                    .and_then(|c| c.call(Request::Resize { id, rows, cols }))
                    .is_ok()
            {
                app.sent_size = Some((id, rows, cols));
            }
            match app.client().and_then(|c| c.call(Request::Screen { id })) {
                Ok(Response::Screen {
                    lines,
                    cursor,
                    state,
                    scroll,
                }) => {
                    app.screen = lines;
                    app.screen_cursor = cursor;
                    // 按键和滚轮怎么分流、底栏那句提示写什么，都看这一份——
                    // 每 16ms 跟着 `Screen` 一起刷新，滞后最多一帧，够用了。
                    app.scroll = scroll;
                    app.connected = true;
                    // agent 自己退出之后不能把用户留在这里：那是一张纯空白页
                    // （agent 在 alternate screen 里画，退出时恢复的主屏从来
                    // 没被写过），底栏还写着「其余按键都发给 agent」，而他敲的
                    // 每个键都掉进一个死掉的 pty 里无声消失。
                    if let Some(notice) = session_ended_notice(id, state, app.lang) {
                        app.view = home_view(&app);
                        // 回看板得重新拉一次 List：贴在会话里这一路都没拉，
                        // 手里的 sessions 是进会话之前那份，缺的正是「这个
                        // 会话已经没了」这条更新。
                        app.need_sessions = true;
                        // 会话正常结束不是错误，用普通提示，不是红字
                        app.message = notice.into();
                        // 下一个会话的尺寸要重新协商：sent_size 记的是刚退出
                        // 的这个 id，留着会让新会话第一帧按错的宽度排版。
                        app.sent_size = None;
                    } else if state == crate::session::SessionState::Failed {
                        // 已经拿到这个会话**这一次**失败的解释了：不再问、
                        // 也不再碰 app.message——见 `App::explained_failure`
                        // 上的注释，这是不让附加视图 16ms 一帧焊死消息栏的
                        // 关键。只有还没拿到答案时才问一次。
                        let already_have = app
                            .explained_failure
                            .as_ref()
                            .is_some_and(|(cached_id, _)| *cached_id == id);
                        if !already_have {
                            // 出错解释是异步算出来的（daemon 侧丢给了后台
                            // 线程，不在 tick() 里等模型），这里问到才显示；
                            // 没问到就什么都不做——今天原本就没有这条提示，
                            // 界面必须长得一模一样，不能因为这个功能露出一个
                            // 新的空白/报错态。
                            if let Ok(Response::Explanation(Some(text))) = app
                                .client()
                                .and_then(|c| c.call(Request::Explanation { id }))
                            {
                                app.message =
                                    Msg::err(crate::i18n::msg::session_failure_explained(
                                        app.lang, id, &text,
                                    ));
                                app.explained_failure = Some((id, text));
                            }
                        }
                    } else if app
                        .explained_failure
                        .as_ref()
                        .is_some_and(|(cached_id, _)| *cached_id == id)
                    {
                        // 这个会话不再是 Failed 了（恢复了）：把缓存忘掉。
                        // 不忘的话，它下次再坏，`already_have` 会一直是
                        // true，新的一次失败永远问不出新的解释。
                        app.explained_failure = None;
                    }
                }
                _ => app.connected = false,
            }
        }

        // 检查一次「在不在会话里」有没有变，而不是在每个能进/出 `View::Attached`
        // 的分支各开关一次——那样的分支太多、太容易漏（上面 `mouse_captured`
        // 声明处的注释列了几条）。放在 `term.draw` 之前是因为这一轮循环里
        // 所有会改 `app.view` 的代码（`verify_rx` 收尾、`Screen` 探测发现
        // 会话已结束……）到这里都已经跑完，此刻的 `app.view` 就是即将画出来
        // 的那一帧。
        let is_attached = matches!(app.view, View::Attached(_));
        if let Some(enable) = mouse_capture_transition(mouse_captured, is_attached) {
            let _ = if enable {
                execute!(std::io::stdout(), EnableMouseCapture)
            } else {
                execute!(std::io::stdout(), DisableMouseCapture)
            };
            mouse_captured = enable;
        }

        term.draw(|f| draw(f, &mut app))?;

        // 会话里要跟手：刷新慢了，你敲的字要等下一轮才显示，每次按键都像卡了一下。
        // 看板不需要这么勤快，150ms 足够，也省得每轮都去锁一遍所有会话。
        let tick = if attached { 16 } else { 150 };
        if !event::poll(Duration::from_millis(tick))? {
            continue;
        }
        let mut ev = event::read()?;
        // 鼠标事件在这里先摘出来单独处理，而且可能不止吃掉这一个：见下面
        // 的注释。这个 `while` 结束之后，`ev` 保证不再是 `Event::Mouse`，
        // 后面的 Paste/Key 分支照旧用它。
        //
        // 循环体里对 `handle_mouse` 的调用不违反房规（见 `attach::handle_key`
        // 头上「永远不要 continue」那条）——它压根不 continue，只是被这个
        // `while` 循环调用；`continue 'main` 才是真正跳过循环末尾清理
        // `message` 那段的地方，而 `handle_mouse` 内部**不许**碰
        // `app.message`，那条约束写在它自己的文档注释上，不受这里怎么
        // 调用它影响。
        while let Event::Mouse(m) = ev {
            let acted = attach::handle_mouse(&mut app, m);
            if acted {
                // 真的送出了一次请求（滚动了、转发了点击/松开）：状态可能
                // 变了，照旧走一次完整的循环体去重新取一遍 Screen、重绘。
                continue 'main;
            }
            // 没做事的绝大多数是纯移动——`EnableMouseCapture` 打开的
            // `?1003h` 是任意移动追踪，跟 agent 有没有订阅无关，鼠标扫一下
            // 80 列宽的窗口就是几十个这种事件，`handle_mouse` 早就把它们
            // 原地丢掉了（见它的文档）。问题是外层 `continue` 会把「取一次
            // Screen、画一帧」全套重放一遍——用一个被丢弃的小事件换来一次
            // 昂贵的守护进程往返和终端重绘，跟当初「移动事件不转发是为了
            // 省流量」的初衷正好背道而驰。这里原地看一眼有没有紧跟着到达
            // 的下一个事件：有就继续在这个 `while` 里处理掉，没有就老实
            // 结束这一轮——不需要刷新时最多等到下一次自然的 16ms/150ms
            // tick，不会更旧，只是不为每一个被丢弃的移动事件单独刷一次。
            if !event::poll(Duration::from_millis(0))? {
                continue 'main;
            }
            ev = event::read()?;
        }
        // 粘贴整段一次发完，不能拆成一个个字符
        if let Event::Paste(text) = ev {
            match &mut app.view {
                View::Attached(id) => {
                    let id = *id;
                    // 这里不走 `app.client()`：它需要 `&mut self`（整个 App），
                    // 跟上面 `&mut app.view` 这个字段级借用同时活着会撞借用检查——
                    // 直接查字段，`None` 归到跟真实断线一样的失败路径。
                    let failed = match app.client.as_mut() {
                        Some(c) => !text.is_empty() && c.call(Request::Input { id, text }).is_err(),
                        None => !text.is_empty(),
                    };
                    if failed {
                        app.message = Msg::err(
                            crate::i18n::text(crate::i18n::Key::PasteNotSent, app.lang).into(),
                        );
                    }
                }
                // 手输路径态：粘贴直接进输入框。从别处拷一条路径粘进来一步到位，
                // 这是不做目录浏览器的底气。trim 掉换行——从终端或文件管理器
                // 拷路径经常带一个尾随换行，不去掉会拼出一个不存在的目录。
                View::PickProject(p) if p.typing_path.is_some() => {
                    if let Some(buf) = p.typing_path.as_mut() {
                        buf.push_str(text.trim());
                    }
                }
                // 密钥十有八九是粘进来的，不是敲的——用户拿到手的字符串通常带
                // 引号、Bearer 前缀、尾随换行，clean_secret 统一洗一遍。
                // Verifying 期间不接：那次验证已经把当时的 buf 发出去了，
                // 这时候再改只会让用户误以为下一次回车用的是新值。
                View::EnterSecret { buf, phase, .. }
                    if !matches!(phase, SecretPhase::Verifying) =>
                {
                    buf.push_str(&clean_secret(&text));
                }
                _ => {}
            }
            continue;
        }
        let Event::Key(key) = ev else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // 处理这次按键前拍个快照，处理完之后用来判断 message 该不该清——
        // 见 message_after_transition 的注释。
        let view_kind_before = std::mem::discriminant(&app.view);
        let message_text_before = app.message.text.clone();
        let message_error_before = app.message.error;

        // Ctrl+Q 在所有视图里都是「退一层，一直按就退到头」。
        //
        // 加这个键是因为真实事故：用户不知道有 F2，在会话里怎么按都出不去，
        // 只能去别的窗口 kill 进程。`Q = quit` 是非程序员唯一猜得到的组合，
        // 而 Claude Code 不占用它——代价只是从 agent 手里拿走 0x11。
        //
        // 拦截**必须**留在 `match view.clone()` 之前，别挪进去：`PickProject`
        // 的打字过滤和手输路径都靠 `Char(c)` 累加，而 Ctrl+Q 在 crossterm 里
        // 就是 `Char('q')` 带 CONTROL——挪进去就会往过滤框里塞一个 q。
        if is_ctrl_q(&key) {
            // 从九宫格退回列表时，列表光标要落在刚才那个焦点格上（见
            // `sync_board_cursor_from_grid`）。必须在 `back_one_level` 之前调，
            // 那之后 `app.view` 已经不是 Grid 了。
            sync_board_cursor_from_grid(&mut app);
            match back_one_level(app.view.clone()) {
                None => app.quit = true,
                Some(next) => {
                    // 回看板要重新拉一次会话列表，否则看板显示的是进会话之前的旧快照
                    app.need_sessions = matches!(next, View::Board);
                    app.view = next;
                }
            }
        } else {
            // 必须 clone：分支里要给 view 赋值，match &view 会被借用检查器拒掉
            match app.view.clone() {
                View::Board => board::handle_key(&mut app, key)?,
                View::PickProfile { .. } => pick::handle_key(&mut app, key)?,
                View::PickProject(_) => pick::handle_key(&mut app, key)?,
                View::Attached(_) => attach::handle_key(&mut app, key)?,
                View::Grid { .. } => grid::handle_key(&mut app, key)?,
                View::Keys { .. } => keys::handle_key(&mut app, key)?,
                View::Settings { .. } => settings_view::handle_key(&mut app, key)?,
                View::EnterSecret { .. } => secret::handle_key(&mut app, key)?,
                View::Secrets { .. } => secret::handle_key(&mut app, key)?,
            }
        }

        // 退出必须在这里落地，不能拖到循环末尾的收尾代码之后。现在有三条路
        // 会置 quit：Ctrl+Q 在顶层（`back_one_level` 返回 None）、看板上按 q、
        // 九宫格里按 q。走到下面的 needs_*_refetch / message_after_transition
        // 也不会有副作用，但那是这三条路各自的巧合，不是那段代码保证的——
        // 而且退出点还会再增加（九宫格那条就是后加的）。在这里 break 直接
        // 还原了原来 `break Ok(())` 的位置：退出这件事不依赖任何关于「谁能
        // 置 quit」的假设，往后新加的退出点也不会在退出前多打一次
        // Request::Profiles、多改一次 app.message。
        if app.quit {
            break;
        }

        // 好几条路都能把 view 换成一个空的 PickProfile——Ctrl+Q 走
        // back_one_level（它是纯函数，拿不到 daemon 连接，只能给个
        // entries: vec![] 的空壳，约定见它的文档注释），EnterSecret 自己的
        // Esc 分支也直接手搭了同一个空壳。两条路都得补，所以放在这里统一
        // 收口，而不是在每个「退回选择器」的地方各查一次——漏一个分支就是
        // 一屏空白，用户会以为自己一个 agent 都没装。
        let needs_profile_refetch =
            matches!(&app.view, View::PickProfile { entries, .. } if entries.is_empty());
        if needs_profile_refetch {
            app.view = match app
                .client()
                .and_then(|c| c.call(Request::Profiles { lang }))
            {
                Ok(Response::Profiles { entries, warnings }) => {
                    let warning = join_warnings(&warnings, lang);
                    let mut state = ListState::default();
                    if !entries.is_empty() {
                        state.select(Some(0));
                    }
                    View::PickProfile {
                        entries,
                        state,
                        warning,
                    }
                }
                Ok(Response::Error(ref e)) => View::PickProfile {
                    entries: Vec::new(),
                    state: ListState::default(),
                    warning: Some(crate::i18n::msg::error(lang, e)),
                },
                _ => View::PickProfile {
                    entries: Vec::new(),
                    state: ListState::default(),
                    warning: Some(
                        crate::i18n::text(crate::i18n::Key::CannotListAgents, app.lang).into(),
                    ),
                },
            };
        }

        // 同样的空壳套路用在 Secrets 上：EnterSecret 的 Esc/Ctrl+Q 从设置页
        // 那条分支进来时、以及验证成功后回设置页时，都是先甩一个空壳占位，
        // 这里补一次 Profiles 把数据填上。`Secrets` 没有 `warning` 字段
        // （跟 `PickProfile` 不一样，见它的字段注释——密钥页的错误反馈走的
        // 是 `message`），拉取失败就直接退回看板并把原因放进 `message`，
        // 总比让用户卡在一屏永远拉不出数据的空列表上强。
        let needs_secrets_refetch =
            matches!(&app.view, View::Secrets { entries, .. } if entries.is_empty());
        if needs_secrets_refetch {
            app.view = match app
                .client()
                .and_then(|c| c.call(Request::Profiles { lang }))
            {
                Ok(Response::Profiles { entries, .. }) => {
                    let mut state = ListState::default();
                    if !secret_rows(&entries).is_empty() {
                        state.select(Some(0));
                    }
                    View::Secrets {
                        entries,
                        state,
                        pending_delete: None,
                    }
                }
                Ok(Response::Error(ref e)) => {
                    app.message = Msg::err(crate::i18n::msg::error(app.lang, e));
                    home_view(&app)
                }
                _ => {
                    app.message = Msg::err(
                        crate::i18n::text(crate::i18n::Key::CannotListSecrets, app.lang).into(),
                    );
                    home_view(&app)
                }
            };
        }

        // 视图变了就把上一屏的残留消息清掉，好让「按视图给提示」的 idle_help
        // 露出来；除非这条消息本身就是这次切换的操作结果（见函数注释）。
        //
        // CRITICAL：这段清理必须原样留在循环末尾，不能挪进任何按键分支——
        // e0ba1ec 就是在这里翻的车：一句普通的「已切到 X」盖掉了屏幕上
        // 唯一告诉用户怎么退出的行。退出本身在上面已经 `break` 掉了，走不到
        // 这里；这段清理只服务于还要继续循环的那些迭代。
        let view_changed = std::mem::discriminant(&app.view) != view_kind_before;
        let message_changed =
            app.message.text != message_text_before || app.message.error != message_error_before;
        app.message = message_after_transition(view_changed, message_changed, app.message);
    }

    Ok(())
}

/// 用系统默认方式打开一个网址，成功了返回 `true`。
///
/// `open` 只在 macOS 上存在；Linux 桌面环境的等价物一般是 `xdg-open`。
/// 两个都试一遍失败了才认输——用户按下 Ctrl+O 是在等申领页面弹出来，
/// 悄无声息什么都不做，他分不清是自己按错了键还是这台机器就是打不开
/// 浏览器（调用方在拿到 `false` 时要把这句话说出来，见按键处理里的注释）。
fn open_url(url: &str) -> bool {
    ["open", "xdg-open"]
        .iter()
        .any(|cmd| std::process::Command::new(cmd).arg(url).spawn().is_ok())
}

/// 把一次按键翻译成要送进 agent 的字节。返回 `None` 表示这个键不转发。
///
/// 空串是与 `session::send_input` 约定的"回车"信号——只有它会触发检查点，
/// 逐字符输入不会产生提交。所以回车必须返回 `Some(String::new())` 而不是 "\r"。
pub fn key_to_input(key: &KeyEvent) -> Option<String> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let s = match key.code {
        KeyCode::Enter => String::new(),
        KeyCode::Char(c) if ctrl => {
            // Ctrl+Q 是 dct 自己的逃生键，绝不透传——见 is_ctrl_q 的注释
            if c.eq_ignore_ascii_case(&'q') {
                return None;
            }
            // Ctrl+A..Ctrl+Z -> 0x01..0x1a，其余 Ctrl 组合不转发
            let lower = c.to_ascii_lowercase();
            if lower.is_ascii_lowercase() {
                char::from(lower as u8 - b'a' + 1).to_string()
            } else {
                return None;
            }
        }
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Backspace => "\x7f".into(),
        KeyCode::Tab => "\t".into(),
        KeyCode::BackTab => "\x1b[Z".into(),
        KeyCode::Up => "\x1b[A".into(),
        KeyCode::Down => "\x1b[B".into(),
        KeyCode::Right => "\x1b[C".into(),
        KeyCode::Left => "\x1b[D".into(),
        KeyCode::Home => "\x1b[H".into(),
        KeyCode::End => "\x1b[F".into(),
        KeyCode::PageUp => "\x1b[5~".into(),
        KeyCode::PageDown => "\x1b[6~".into(),
        KeyCode::Delete => "\x1b[3~".into(),
        KeyCode::Insert => "\x1b[2~".into(),
        // Esc 必须转发：agent 拿它做取消、清空、关弹窗
        KeyCode::Esc => "\x1b".into(),
        _ => return None,
    };
    Some(s)
}

fn selected<'a>(sessions: &'a [SessionInfo], st: &ListState) -> Option<&'a SessionInfo> {
    st.selected().and_then(|i| sessions.get(i))
}

/// 底栏画按键表要知道的「现在能干什么」。
///
/// 「当前是哪个会话」在两个视图里问法不同：列表问光标选中了哪一行
/// （`visible` + `list_state`），九宫格问焦点在哪一格（`grid_visible` +
/// `focus`）——跟 `session_action` 的两个调用点是同一条分岔，理由也一样。
/// 收在这里一次，就不会有「底栏按列表算、按键按格子算」这种两边不一致。
fn help_ctx(app: &App) -> view::HelpCtx {
    // 九宫格不画已停止的会话，所以「屏幕上有没有会话」在两个视图里
    // 数的是两个集合——列表看 `visible`，格子看 `grid_visible`。
    let (all, cur) = match &app.view {
        View::Grid { focus, .. } => (&app.grid_visible, app.grid_visible.get(*focus)),
        _ => (&app.visible, selected(&app.visible, &app.list_state)),
    };
    view::HelpCtx {
        has_sessions: !all.is_empty(),
        selected: cur.map(|s| view::SelectedSession {
            is_agent: s.is_agent,
            state: s.state,
        }),
    }
}

/// 切到另一个项目：告诉用户、改 `current_dir`、重算作用域、回看板。
///
/// 抽成一个函数是因为选择器有两条确认路径（列表选中、手输路径），
/// 而这四步必须整套发生。分开写的话，漏掉重算的那条路会让屏幕停在
/// 上一个项目的会话上、底栏却已经写着新项目——用户看到的就是
/// 「同一个 session 变成了不同的项目」。
pub(crate) fn switch_project(app: &mut App, dir: std::path::PathBuf) {
    // 「当前项目」已经在底部边框标题里，这里说的是刚发生的动作
    app.message =
        crate::i18n::msg::switched_to(app.lang, &short_path(&dir.display().to_string())).into();
    app.current_dir = dir;
    app.refresh_visible();
    app.view = home_view(app);
}

/// 进一个会话。会话属于别的项目时，当前项目跟着切过去。
///
/// 「你在哪个会话里，当前项目就是哪个」——不这么做的话，从「全部项目」
/// 视图进了别的项目的会话，按 F2 回来时它已经被过滤掉了，看起来像是
/// 消失了；而且随手按 `n` 新建会话会开在一个你并没有在看的项目里。
///
/// 切了就必须说一声。静默改变当前项目正是这一版被判为「混乱」的原因。
pub(crate) fn enter_session(app: &mut App, id: u32) {
    // 在全量列表里找，不是 visible：从「全部项目」进来的那个会话，
    // 正是当前作用域看不见的那一个。
    if let Some(dir) = app
        .sessions
        .iter()
        .find(|s| s.id == id)
        .map(|s| std::path::PathBuf::from(&s.dir))
    {
        if !view::same_project(&dir, &app.current_dir) {
            app.message =
                crate::i18n::msg::switched_to(app.lang, &short_path(&dir.display().to_string()))
                    .into();
            app.current_dir = dir;
            app.refresh_visible();
        }
    }
    // 会话标题要显示项目名
    app.need_sessions = true;
    app.view = View::Attached(id);
    // 每次「进入」都当成一次全新的观察：`explained_failure` 缓存的是
    // 「上一次贴在这个会话里时看到的解释」，如果这个会话在用户离开、
    // 又回来之间恢复过、再坏过一次，缓存里那份还是上上次失败的旧话，
    // 不清掉的话 `run()` 主循环会以为「问过了」，永远不会去问新的那次。
    app.explained_failure = None;
    // CONTROLLER RULING：进入会话视图一律落在底部，不管上次离开时翻到
    // 哪儿了、也不管这一次算不算「换了个尺寸」。
    //
    // 原来落地在哪儿全看 `run()` 主循环里那条按 `sent_size` 判断要不要发
    // `Request::Resize` 的分支——`SessionManager::resize` 顺手把 scroll 归了
    // 零，于是「回不回底部」变成了一个跟尺寸缓存打不打得上的意外结果：
    // 切到另一个会话再回来，id 变了，`sent_size` 不匹配，触发 Resize，
    // 顺带回到底部；按 F2 回看板、什么都没点、又直接 Enter 回同一个会话，
    // `sent_size` 还对得上，Resize 不发，人就诡异地卡在几十行之前翻到的
    // 地方——同一个「回来看看」的意图，走两条路给两种结果。
    //
    // 离开了再回来，用户要看的是最新输出，不是他上次读到一半的历史——
    // 一种可预期的行为好过两种碰运气的。所以这里直接、明确地发一次
    // `Scroll::Bottom`，不再借 `sent_size`/`Resize` 的副作用捎带出来。
    // 失败就静默：跟 `handle_key` 里滚动请求失败的处理一样，这不是用户
    // 当下敲的一个键，没反应分不清是卡顿还是断连，下一帧的 `Screen`
    // 探测自然会把 `connected` 标成假。
    let _ = app.client().and_then(|c| {
        c.call(Request::Scroll {
            id,
            by: crate::session::ScrollBy::Bottom,
        })
    });
}

/// 把守护进程报回来的一串警告码组成一行人话。
///
/// 拼接（`；`）发生在这里而不是 daemon 侧：daemon 连用哪种语言都不知道，
/// 更不知道该用哪个分隔符——中文用顿号式的全角分号，英文该用 `; `。
fn join_warnings(
    warnings: &[crate::proto::WarningCode],
    lang: crate::i18n::Lang,
) -> Option<String> {
    if warnings.is_empty() {
        return None;
    }
    let sep = match lang {
        crate::i18n::Lang::Zh => "；",
        crate::i18n::Lang::En => "; ",
    };
    Some(
        warnings
            .iter()
            .map(|w| crate::i18n::msg::warning(lang, w))
            .collect::<Vec<_>>()
            .join(sep),
    )
}

/// 浮层占的那块地方：各方向取「终端的一大半」和一个上限里的较小者，居中。
///
/// 设上限是因为在一块 200 列的屏幕上，一个铺满 80% 的对话框反而更难扫——
/// 眼睛要横跨整个屏幕才能把一行读完。
///
/// 终端小到放不下时**退化成全屏**，而不是显示一句「窗口太小」：选项目是
/// 用户此刻非做不可的事，挡住他没有意义（跟九宫格不同——那一屏本来就是
/// 「看一眼」，看不了就先别看）。
fn popup_area(area: Rect) -> Rect {
    // 再小就放不下两栏了：每栏各要两列边框，加上目录名和 git 标记。
    // 比这还小的终端，浮层已经没有「浮」的意义——全屏给他。
    const MIN_COLS: u16 = 40;
    const MIN_ROWS: u16 = 8;
    if area.width < MIN_COLS || area.height < MIN_ROWS {
        return area;
    }
    let w = (area.width.saturating_mul(4) / 5).clamp(MIN_COLS, 100);
    let h = (area.height.saturating_mul(3) / 4).clamp(MIN_ROWS, 24);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// `g` 键：在列表和九宫格之间切换，并且**记住**。
///
/// 两个模式共用这一个函数，切换和落盘绑在一起——分开写的话，某一边忘了
/// 落盘，用户就会遇到「有时记得、有时不记得」这种最难描述的 bug。
///
/// 落盘失败只在底栏说一句，不挡住切换本身：模式已经切了，屏幕上就是新的
/// 那个，硬要因为写不了文件把它切回去反而更费解。
pub(crate) fn toggle_view_mode(app: &mut App) {
    // 离开九宫格前先把列表光标对到焦点格上；`home_view` 负责反方向。
    // 两个视图对「当前是哪个会话」的认知必须一致，否则切一下模式，
    // 接下来的 `s`（停止）就毁在另一个会话上——这个键不可撤销。
    if matches!(app.view, View::Grid { .. }) {
        sync_board_cursor_from_grid(app);
    }
    app.view_mode = app.view_mode.toggled();
    app.view = home_view(app);
    let path = crate::settings::settings_path_for_socket(&app.socket);
    if let Err(e) = crate::settings::save_view_mode(&path, app.view_mode) {
        app.message = Msg::err(
            e.downcast_ref::<crate::proto::CodedError>()
                .map(|c| crate::i18n::msg::error(app.lang, &c.0))
                .unwrap_or_else(|| e.to_string()),
        );
    }
}

/// 用户选的那个模式对应的视图。**所有「回看板」的地方都必须走这里。**
///
/// 硬编码 `View::Board` 的每一处都是一个「明明在九宫格里，某次操作完却被
/// 甩回列表」的 bug——而且是偶发、复现不了的那种。
///
/// 顺带让两个模式对「当前是哪个会话」的认知保持一致：进九宫格时焦点落在
/// 列表光标那一行。反方向由 `sync_board_cursor_from_grid` 负责。
pub(crate) fn home_view(app: &App) -> View {
    match app.view_mode {
        ViewMode::List => View::Board,
        // 同样按会话 id 对——理由见 `sync_board_cursor_from_grid`。
        // 光标停在一个已停止的会话上时（九宫格里没有它）落回第一格：
        // 那是个「它在九宫格里不存在」的诚实答案，比对到旁边某个无关的
        // 会话上强。
        ViewMode::Grid => View::grid(
            app.list_state
                .selected()
                .and_then(|i| app.visible.get(i))
                .and_then(|s| app.grid_visible.iter().position(|g| g.id == s.id))
                .unwrap_or(0),
        ),
    }
}

/// `l` 键：打开设置页，光标预先落在当前语言上——用户进来第一眼要看到
/// 「现在是哪个」，而不是从头找。
pub(crate) fn open_settings(app: &mut App) {
    let mut state = ListState::default();
    state.select(Some(
        crate::i18n::Lang::all()
            .iter()
            .position(|l| *l == app.lang)
            .unwrap_or(0),
    ));
    app.view = View::Settings { state };
}

/// `a` 键：在「只看当前项目」和「全部项目」之间切换。看板和九宫格共用。
///
/// 切完立刻重算，不等下一轮 `need_sessions`——那要等到 300ms 之后，
/// 用户会以为这个键没反应，然后再按一次又切回去。
fn toggle_scope(app: &mut App) {
    app.scope = match app.scope {
        Scope::CurrentProject => Scope::AllProjects,
        Scope::AllProjects => Scope::CurrentProject,
    };
    app.refresh_visible();
}

/// 光标移动的通用版本：只认列表长度，不认列表里装的是什么。
/// 项目选择器和会话看板共用它。
fn move_sel_n(st: &mut ListState, len: usize, delta: i32) {
    if len == 0 {
        st.select(None);
        return;
    }
    let cur = st.selected().unwrap_or(0) as i32;
    let next = (cur + delta).clamp(0, len as i32 - 1);
    st.select(Some(next as usize));
}

fn move_sel(st: &mut ListState, sessions: &[SessionInfo], delta: i32) {
    move_sel_n(st, sessions.len(), delta);
}

/// 离开九宫格之前，把列表光标挪到当前焦点格上。
///
/// 两个视图对「当前是哪个会话」的认知必须一致——`board.rs` 的 `g` 分支
/// 已经做了列表→九宫格那一半，这是反过来的另一半。少了它，用户盯着第 5 格
/// 按 Ctrl+Q 回到列表，光标还停在第一行，下一个 `s`（停止）或 `u`（回滚）
/// 就毁在另一个会话上——这两个键都不可撤销，不能指望用户自己看出来。
///
/// 抽成函数是因为出口不止一个（`g`、Ctrl+Q、Enter 放大），而 Ctrl+Q 那条
/// 走的是 `back_one_level`——它是纯函数，手里根本没有 `list_state`。
/// 不在九宫格里就什么都不做，调用方不必先判视图。
pub(crate) fn sync_board_cursor_from_grid(app: &mut App) {
    if let View::Grid { focus, .. } = app.view {
        // **按会话 id 对，不按下标。** 九宫格不画已停止的会话，所以两个
        // 集合的下标根本对不上——第 2 格可能是列表的第 4 行。对错了的话，
        // 回列表后接下来的 `s`（停止）或 `u`（回滚）就毁在另一个会话上，
        // 而这两个键都不可撤销。
        if let Some(id) = app.grid_visible.get(focus).map(|s| s.id) {
            if let Some(i) = app.visible.iter().position(|s| s.id == id) {
                app.list_state.select(Some(i));
            }
        }
    }
}

/// `n`（开上次那个 agent）/ `N`（挑一个 agent）。
///
/// 看板和九宫格是同一块看板的两种画法，这四个「开东西」的键
/// （`n`/`N`/`p`/`c`）在两边必须一模一样，所以整段逻辑只留一份。
/// `code` 区分大小写 n：小写才去问 daemon 上次记的是哪个 agent。
pub(crate) fn open_new_session(app: &mut App, code: KeyCode) {
    // entries 带的是完整信息（label/note/status/密钥提示/安装提示），
    // 渲染时把置灰项和原因画出来、四种状态各自路由到哪，见
    // pick_action 和 View::PickProfile 的按键分支。n 和 N 都要这份
    // 列表——n 拿它判断上次那个 agent 现在还在不在 Ready，N 拿它渲染
    // 选择器——所以只拉一次，不分两条路各拉各的。
    let lang = app.lang;
    match app
        .client()
        .and_then(|c| c.call(Request::Profiles { lang }))
    {
        Ok(Response::Profiles { entries, warnings }) => {
            let warning = join_warnings(&warnings, app.lang);
            // 把「拉完列表但没能直开」的三种落点（选择器为空、建会话失败
            // 两种）收在一处，省得同一段 ListState 初始化抄三遍——那种
            // 抄法迟早有一份漏了空表守卫。
            let picker = |entries: Vec<ProfileEntry>, warning: Option<String>| {
                let mut state = ListState::default();
                // daemon 目前总是至少返回九个内置 profile，这里空表分支
                // 基本走不到；但选中一个不存在的下标，按 Enter 就是
                // entries[0] 越界 panic——这种最坏结果不该只靠"实践中
                // 到不了"兜底，一行守卫不值钱。
                if !entries.is_empty() {
                    state.select(Some(0));
                }
                View::PickProfile {
                    entries,
                    state,
                    warning,
                }
            };
            // 大写 N 一定要看一眼选择器，不查上次用的是谁；
            // 小写 n 才去问 daemon 上次记的是哪个 agent。
            let last = if code == KeyCode::Char('n') {
                let dir = app.current_dir.display().to_string();
                match app
                    .client()
                    .and_then(|c| c.call(Request::LastProfile { dir }))
                {
                    Ok(Response::LastProfile(l)) => l,
                    _ => None,
                }
            } else {
                None
            };
            match quick_start_target(last.as_deref(), &entries) {
                Some(name) => {
                    // 同 View::PickProfile 里 PickAction::Start 那支：
                    // 「n」等价于「已经替用户选好了上次那个」，建完直接
                    // 进会话，不用再让他确认一遍。
                    let dir = app.current_dir.display().to_string();
                    match app.client().and_then(|c| {
                        c.call(Request::Create {
                            dir,
                            profile: name,
                            remember: true,
                        })
                    }) {
                        Ok(Response::Created { id }) => {
                            app.need_sessions = true; // 会话标题要显示项目名
                            app.view = View::Attached(id);
                        }
                        Ok(Response::Error(ref e)) => {
                            app.message = Msg::err(crate::i18n::msg::error(app.lang, e));
                            app.view = picker(entries, warning);
                        }
                        _ => {
                            app.message = Msg::err(
                                crate::i18n::text(crate::i18n::Key::CreateFailed, app.lang).into(),
                            );
                            app.view = picker(entries, warning);
                        }
                    }
                }
                None => app.view = picker(entries, warning),
            }
        }
        // 列表都拿不到，直开和选择器都没法走，只能告诉用户这次干瞪眼——
        // 视图不变，走到循环末尾 message_after_transition 会把这条消息
        // 原样留住（同其他分支，不用 continue 抢跑跳过收尾）。
        Ok(Response::Error(ref e)) => app.message = Msg::err(crate::i18n::msg::error(app.lang, e)),
        _ => {
            app.message =
                Msg::err(crate::i18n::text(crate::i18n::Key::CannotListAgents, app.lang).into())
        }
    }
}

/// `p`：换项目。看板和九宫格共用，同 `open_new_session`。
pub(crate) fn open_project_picker(app: &mut App) {
    // 拿不到列表就不进选择器：进去看见一片空白，用户会以为
    // 自己从来没开过项目。
    match app.client().and_then(|c| c.call(Request::Projects)) {
        Ok(Response::Projects {
            recent: mut all, ..
        }) => {
            // 全新守护进程列表是空的，补上启动目录，
            // 保证第一次用也不会看到空列表。
            let start = app.start_dir.display().to_string();
            if !all.contains(&start) {
                all.push(start);
            }
            // 浏览器从当前项目所在的位置开起：用户按 p 多半是想换到
            // 「旁边那个」项目，从它的上一级开始找是最短的路。
            let from = app
                .current_dir
                .parent()
                .map(|x| x.to_path_buf())
                .unwrap_or_else(|| app.current_dir.clone());
            app.view = View::PickProject(crate::ui::view::ProjectPicker::new(all, from));
        }
        Ok(Response::Error(ref e)) => app.message = Msg::err(crate::i18n::msg::error(app.lang, e)),
        _ => {
            app.message =
                Msg::err(crate::i18n::text(crate::i18n::Key::CannotListProjects, app.lang).into())
        }
    }
}

/// `c`：密钥设置页。看板和九宫格共用，同 `open_new_session`。
pub(crate) fn open_secrets(app: &mut App) {
    // 拿不到列表就不进设置页：留在原地给一句错误，总比弹进一个既没数据、
    // 又没地方显示错误的空白页强（`View::Secrets` 没有 `warning` 字段，
    // 见其字段注释）。
    let lang = app.lang;
    match app
        .client()
        .and_then(|c| c.call(Request::Profiles { lang }))
    {
        Ok(Response::Profiles { entries, .. }) => {
            let mut state = ListState::default();
            if !secret_rows(&entries).is_empty() {
                state.select(Some(0));
            }
            app.view = View::Secrets {
                entries,
                state,
                pending_delete: None,
            };
        }
        Ok(Response::Error(ref e)) => app.message = Msg::err(crate::i18n::msg::error(app.lang, e)),
        _ => {
            app.message =
                Msg::err(crate::i18n::text(crate::i18n::Key::CannotListSecrets, app.lang).into())
        }
    }
}

/// 对某个会话做 `s`（停止）/ `u`（回滚）/ `d`（看改动），返回要显示的消息。
///
/// 看板和九宫格是同一套语义作用在不同的「当前会话」上（列表是选中行，
/// 九宫格是焦点格），所以发请求和拼消息这段只留一份：两边各抄一份的话，
/// 哪天改了 diff 的措辞或者错误分支，只会改到其中一半。
///
/// `code` 之外的按键返回空消息——调用方只在这三个键上调它，落到那条兜底
/// 说明分派写漏了；这时候不动 `message` 比编一句话给用户看更诚实。
pub(crate) fn session_action(app: &mut App, code: KeyCode, id: u32) -> Msg {
    let req = match code {
        KeyCode::Char('s') => Request::Stop { id },
        KeyCode::Char('u') => Request::Undo { id },
        KeyCode::Char('d') => Request::Diff { id },
        _ => return "".into(),
    };
    match app.client().and_then(|c| c.call(req)) {
        Ok(Response::Ok) => crate::i18n::text(crate::i18n::Key::ActionDone, app.lang).into(),
        Ok(Response::Diff(v)) if v.is_empty() => {
            crate::i18n::text(crate::i18n::Key::NoChanges, app.lang).into()
        }
        Ok(Response::Diff(v)) => v
            .iter()
            .map(|f| format!("{} +{} -{}", f.path, f.added, f.removed))
            .collect::<Vec<_>>()
            .join("  ")
            .into(),
        Ok(Response::Error(ref e)) => Msg::err(crate::i18n::msg::error(app.lang, e)),
        _ => Msg::err(crate::i18n::text(crate::i18n::Key::RequestFailed, app.lang).into()),
    }
}

/// 密钥页要展示的数据总在变——改完一条、删完一条都要照一份新的 `has_secret`
/// 才对得上。改/删/刚打开页面这三个调用点都要拉同一份数据，区别只在光标
/// 该落在哪：`focus` 给了 profile 名字就尽量把光标定在它原来那一行上
/// （删完/改完还盯着同一个 profile，比每次都弹回第一行顺手），不给就落在
/// 第一行（刚打开页面，没有"原来"）。
///
/// 拉取失败时退化成一个空 `entries` 的壳——同 `back_one_level` 对
/// `PickProfile`/`Secrets` 的约定：循环收尾那段通用重拉逻辑看到空壳会自己
/// 再补一次，这里不需要重复一份「失败了怎么办」的判断。
fn refetch_secrets(app: &mut App, focus: Option<&str>) -> View {
    // 直接查字段而不是走 `app.client()`：调用方往往还在同一个 `match` 里
    // 借着 `app` 的别的字段（比如 `entries`/`state` 已经从 `app.view` 解构
    // 出来），走一个吃 `&mut self` 的方法会跟这些借用打架。`None` 归到跟
    // 下面 `_ =>` 一样的失败落点——同真实断线共用一条路径，不新增分支。
    let lang = app.lang;
    let result = app
        .client
        .as_mut()
        .map(|c| c.call(Request::Profiles { lang }));
    match result {
        Some(Ok(Response::Profiles { entries, .. })) => {
            let rows = secret_rows(&entries);
            let mut state = ListState::default();
            if !rows.is_empty() {
                let idx = focus
                    .and_then(|name| rows.iter().position(|(n, _)| n == name))
                    .unwrap_or(0);
                state.select(Some(idx));
            }
            // 重拉之后不管改的还是删的都已经落定，武装状态没有意义可言了
            // ——不管刚才 pending_delete 是什么，新的一屏都从「没有武装」
            // 开始。
            View::Secrets {
                entries,
                state,
                pending_delete: None,
            }
        }
        _ => View::Secrets {
            entries: Vec::new(),
            state: ListState::default(),
            pending_delete: None,
        },
    }
}

/// 左段固定占的列数：最长的一条是会话视图的「Ctrl+Q（F2） 回看板」
/// = 6 + 全角括号 2 + "F2" 2 + 全角括号 2 + 空格 1 + 中文 3 字 × 2 = 19。
/// 其余各条都更短（「Ctrl+Q 回列表」13，「q 退出」7）。
/// 写死而不是每帧算：左段宽度跟着文案跳动会让右段的消息忽宽忽窄。
const ESCAPE_HINT_COLS: u16 = 19;

/// 画一帧界面。内容区（`chunks[0]`）按当前视图分派给各自模块的 `draw`；
/// 底部栏（`chunks[1]`：逃生键 + 消息/帮助文案）不分视图，统一在这里画。
fn draw(f: &mut Frame, app: &mut App) {
    // 提示必须跟着视图走。底部栏原来不分视图，进了会话仍写着看板的按键表，
    // 而那些键在会话视图里全部被转发给 agent——用户照着按 n，字母 n 会落进
    // Claude Code 的输入框。显示做不到的操作比不显示更糟。
    //
    // 逃生键那一截已经挪进左段常驻，这里不再重复。
    //
    // 算在布局之前：底栏要多高，取决于这段文字折成几行。
    //
    // 按键表和消息走两条路：按键表是一排结构化的条目（键名要加粗、放不下的
    // 按优先级丢，永远一行），消息是一句人话（一个字都不能丢，宁可折行）。
    let (bar, style) = if !app.connected {
        (
            BarContent::Text(crate::i18n::text(crate::i18n::Key::StaleData, app.lang).to_string()),
            Style::default().fg(Color::Red),
        )
    } else if app.message.text.is_empty() {
        // 会话视图里，滚动提示是持续状态（「翻到哪儿了」「下面有新内容」），
        // 按键表是「还能干什么」——两者抢的是同一行，而滚动提示更具体。
        // 只在 `message` 为空这一支里问它：`message` 优先在外层 if/else
        // 链上已经保证了（见函数头注释），这里不用重复判断。
        let scroll_hint = match &app.view {
            View::Attached(_) => attach::scroll_hint(&app.scroll, app.lang),
            _ => None,
        };
        match scroll_hint {
            Some(hint) => (BarContent::Text(hint), Style::default()),
            None => (
                BarContent::Keys(idle_help(&app.view, app.scope, app.lang, help_ctx(app))),
                Style::default(),
            ),
        }
    } else if app.message.error {
        (
            BarContent::Text(app.message.text.clone()),
            Style::default().fg(Color::Red),
        )
    } else {
        (BarContent::Text(app.message.text.clone()), Style::default())
    };

    // 右段可用宽度。跟下面切 `bar` 时算的是同一个数——不一致的话，按高度
    // 预留的行数就对不上真正折出来的行数，末尾几个键照样会被吃掉。
    let help_cols = f
        .area()
        .width
        .saturating_sub(2 + ESCAPE_HINT_COLS + 2) // 块的左右边框 + 左段及其间隔
        as usize;
    // 按键表永远一行：多一行底栏就多吃一行内容区，而九宫格在 80×24 下只差
    // 一行就跌破 `grid.rs` 的 `MIN_ROWS`，整屏换成一句「窗口太小」。放不下的
    // 键落进 `?` 浮层，不再挤占版面（见 `widgets::fit_help`）。
    let help_lines: Vec<Line> = match &bar {
        BarContent::Keys(items) => vec![Line::from(widgets::help_spans(&widgets::fit_help(
            items, help_cols,
        )))],
        BarContent::Text(t) => widgets::wrap_help(t, help_cols)
            .into_iter()
            .map(Line::from)
            .collect(),
    };

    // 底栏高度 = 上下边框 + 文案真正折出来的行数。
    //
    // 这里原来写死 4 行（两行文字），于是「按键表能不能放下」变成了一件
    // 靠人算、靠单测事后兜的事：往表里加一个键，末尾的 `d 改动` 就被右端
    // 悄悄截掉，而 u/s/d 里有两个是不可撤销的操作。屏幕上没写却真的管用的
    // 键，就是等着用户误按。按内容算高度之后，这一类 bug 在结构上不再可能
    // 发生——按键表恒为一行，只有长消息才会把底栏撑高。
    //
    // 上限三分之一屏：再窄的终端里还是会截，但那时候继续让位只会把内容区
    // 挤没。
    let bar_h = ((help_lines.len() as u16).max(1) + 2).min((f.area().height / 3).max(3));
    let chunks = Layout::vertical([Constraint::Min(3), Constraint::Length(bar_h)]).split(f.area());

    // 穷尽匹配而不是 if/else 链：少一个 View 变体的分支，if/else 链的兜底
    // `else` 会悄悄把新变体也归给 secret::draw，画出一片空白也照样编译通过；
    // match 会在加变体的那一刻直接编译报错，逼着调用点补上。跟 `run()` 里
    // 按键分发那个 `match app.view.clone()` 用的是同一个理由，这里同样
    // 必须 `.clone()`——各分支要把 `app` 借给 `board::draw`/`pick::draw`
    // 等函数，`match &app.view` 留着的借用会跟这些调用打架。
    match app.view.clone() {
        View::Board => board::draw(f, chunks[0], app),
        View::Attached(_) => attach::draw(f, chunks[0], app),
        View::Grid { .. } => grid::draw(f, chunks[0], app),
        View::PickProfile { .. } => pick::draw(f, chunks[0], app),
        // 选项目是**浮层**：先照常画用户的家视图，再在中间盖一层。
        // 背后留着看板是重点——用户要看得出自己只是叠了一层，Esc 一下
        // 就回去了。全屏接管正是上一版被判为「混乱」的一部分。
        View::PickProject(_) => {
            let home = home_view(app);
            let saved = std::mem::replace(&mut app.view, home);
            match app.view {
                View::Grid { .. } => grid::draw(f, chunks[0], app),
                _ => board::draw(f, chunks[0], app),
            }
            app.view = saved;
            let popup = popup_area(chunks[0]);
            f.render_widget(ratatui::widgets::Clear, popup);
            pick::draw(f, popup, app);
        }
        // 全部按键也是**浮层**，跟项目选择器同一种呈现：背后留着按 `?` 之前
        // 那一屏，用户看得出自己只是叠了一层。背后画的是 `from` 而不是
        // `home_view()`——从九宫格进来就该看见九宫格，连焦点格都不该动。
        View::Keys { from } => {
            let saved = std::mem::replace(&mut app.view, (*from).clone());
            match app.view {
                View::Grid { .. } => grid::draw(f, chunks[0], app),
                _ => board::draw(f, chunks[0], app),
            }
            app.view = saved;
            keys::draw(f, chunks[0], app);
        }
        View::EnterSecret { .. } | View::Secrets { .. } => secret::draw(f, chunks[0], app),
        View::Settings { .. } => settings_view::draw(f, chunks[0], app),
    }

    // 当前项目放在边框标题里，框内只留一行字。中文是双宽字符，
    // 「当前项目：~/work/dc/dc-terminal」加上按键表在 80 列终端里放不下同一行，
    // 挤在一起会被 Paragraph 直接截断——标题行本来就空着，正好用它。
    let block = Block::default().borders(Borders::ALL).title(format!(
        "{}：{}",
        crate::i18n::text(crate::i18n::Key::CurrentProject, app.lang),
        short_path(&app.current_dir.display().to_string())
    ));
    let inner = block.inner(chunks[1]);
    f.render_widget(block, chunks[1]);

    // 横向拆两段：左段是逃生键，永不让位；断连提示和消息只能吃掉右段。
    //
    // 拆之前的写法是一整行按优先级二选一，于是「已切到 X」这类完全正常的
    // 操作反馈会把整张按键表连同「q 退出」一起顶掉，而消息只在切视图时才清——
    // 用户不知道怎么切视图正是他卡住的原因，于是退出提示永久消失。
    // 拆成两段之后这件事在结构上不可能再发生。
    let bar = Layout::horizontal([
        Constraint::Length(ESCAPE_HINT_COLS + 2), // +2 是和右段之间的间隔
        Constraint::Min(0),
    ])
    .split(inner);
    f.render_widget(
        Paragraph::new(escape_hint(&app.view, app.lang)).style(Style::default().fg(Color::Cyan)),
        bar[0],
    );
    // 折行而不是截断：截断会把句尾那几个键悄悄抹掉，而用户没有任何线索
    // 知道自己少看了几个键。折行用 `wrap_help` 而不是 ratatui 的 `Wrap`，
    // 理由见那个函数——`Wrap` 会把「p 换项目」拆成行尾一个孤零零的 `p`。
    //
    // 用上面按 `help_cols` 折好的那一份，不在这里重折：底栏高度就是按它
    // 的行数留的，两处各折一次的话，宽度算法一旦分叉，留的行和画的行就对
    // 不上了。这条断言由 `the_bar_is_exactly_as_tall_as_the_help_it_holds` 盯着。
    debug_assert_eq!(
        help_cols, bar[1].width as usize,
        "预留底栏高度用的宽度必须跟真正画的宽度一致"
    );
    f.render_widget(Paragraph::new(help_lines).style(style), bar[1]);
}

/// 底栏右段这一帧要显示的东西。
///
/// 分成两支而不是一律当字符串：按键表要给键名加粗（加粗是 `Span` 一级的事，
/// 拼成字符串就切不回来了），而且放不下时按优先级丢；消息是一句人话，一个字
/// 都不能丢，宁可折行也不能截。
enum BarContent {
    Keys(Vec<crate::i18n::HelpItem>),
    Text(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;
    use app::App;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Important (a)/(b) 回归点，UI 这一侧：`explained_failure` 缓存必须在
    /// **每次进入**会话时清空——不然一个「恢复了、又坏了一次」的会话，会
    /// 一直顶着用户上一次贴在这里时看到的旧解释，`already_have` 永远为真，
    /// 新的一次失败问不出新答案。
    #[test]
    fn entering_a_session_forgets_any_previously_cached_explanation() {
        let (mut app, _dir) = App::test_app();
        app.sessions = vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: app.current_dir.display().to_string(),
            state: SessionState::Failed,
            activity: String::new(),
            is_agent: true,
        }];
        app.explained_failure = Some((1, "上一次贴在这里时看到的旧解释".into()));

        enter_session(&mut app, 1);

        assert_eq!(
            app.explained_failure, None,
            "进入会话必须当成一次全新的观察，不能继续顶着上一次的缓存"
        );
    }

    /// F7 回归测试：re-entering 落在底部必须是 `enter_session` 自己主动做的
    /// 一件事，不能是靠 `run()` 主循环里 `sent_size` 变了才顺带触发的
    /// `Resize` 副作用——不然「F3 直接切会话」（id 变了，`sent_size` 跟着
    /// 变）和「F2 回看板、原地 Enter 回同一个会话」（id 没变，`sent_size`
    /// 照样对得上，`Resize` 根本不会发）会给出两种不同的结果，同一个「回
    /// 来看看」的意图，一半时候把用户按第一种方式接回底部，另一半晾在
    /// 半空。
    ///
    /// 这条测试故意不走 `run()`、不发任何 `Resize`，只直接调
    /// `enter_session`：起一个真守护进程，攒出足够滚屏、真的往上翻一截，
    /// 确认 offset 确实大于零；然后单独调 `enter_session`（模拟"已经在看
    /// 这个会话，只是重新点了一下进来"，没有任何尺寸变化可以触发
    /// Resize），如果 offset 归零了，只能是这次改动新加的那次显式
    /// `Request::Scroll { by: Bottom }` 干的。
    #[test]
    fn entering_a_session_always_lands_at_the_bottom_even_without_a_resize() {
        use crate::client::Client;
        use crate::proto::{Request, Response};
        use crate::session::ScrollBy;
        use std::time::{Duration, Instant};

        let home = tempfile::tempdir().unwrap();
        let sock = home.path().join("daemon.sock");
        let s = sock.clone();
        std::thread::spawn(move || {
            let _ = crate::daemon::run(&s);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !sock.exists() {
            assert!(Instant::now() < deadline, "daemon 没起来");
            std::thread::sleep(Duration::from_millis(20));
        }

        let mut c = Client::connect(&sock).unwrap();
        let workdir = tempfile::tempdir().unwrap();
        let id = match c
            .call(Request::Create {
                dir: workdir.path().display().to_string(),
                profile: "shell".into(),
                remember: false,
            })
            .unwrap()
        {
            Response::Created { id } => id,
            other => panic!("预期 Created，实际 {other:?}"),
        };

        // 攒够滚屏内容：跟 `session.rs` 里 `scrolling_session` 用的是同一种
        // POSIX 循环，不挑具体 shell。
        c.call(Request::Input {
            id,
            text: "i=1; while [ $i -le 200 ]; do echo line-$i; i=$((i+1)); done\n".into(),
        })
        .unwrap();

        let screen = |c: &mut Client| -> (String, crate::session::ScrollState) {
            match c.call(Request::Screen { id }).unwrap() {
                Response::Screen { lines, scroll, .. } => {
                    let text = lines
                        .iter()
                        .flat_map(|l| l.iter())
                        .map(|s| s.text.as_str())
                        .collect::<String>();
                    (text, scroll)
                }
                other => panic!("预期 Screen，实际 {other:?}"),
            }
        };

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (text, scroll) = screen(&mut c);
            if text.contains("line-200") && scroll.max > 0 {
                break;
            }
            assert!(Instant::now() < deadline, "没等到滚屏内容攒够");
            std::thread::sleep(Duration::from_millis(50));
        }

        c.call(Request::Scroll {
            id,
            by: ScrollBy::Rows(20),
        })
        .unwrap();
        let (_, scroll) = screen(&mut c);
        assert!(scroll.offset > 0, "先确认真的往上翻了，不然下面测的是空话");

        // 模拟「已经在看这个会话，重新进来一次」——不经过 `run()`，直接调
        // `enter_session`：这条路径上没有任何 Resize 会发生。
        let mut app = App::new(
            Client::connect(&sock).unwrap(),
            workdir.path().to_path_buf(),
            crate::i18n::Lang::Zh,
            sock.clone(),
            ViewMode::List,
        );
        app.sessions = vec![SessionInfo {
            id,
            profile: "shell".into(),
            dir: workdir.path().display().to_string(),
            state: SessionState::Working,
            activity: String::new(),
            is_agent: false,
        }];
        app.current_dir = workdir.path().to_path_buf();
        app.view = View::Board;

        enter_session(&mut app, id);

        // `enter_session` 发的 Scroll 请求是异步落地的——给它一点时间，
        // 不是靠 sleep 赌运气，是轮询到超时才认输。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (_, scroll_after) = screen(&mut c);
            if scroll_after.offset == 0 {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "重新进入会话必须落在底部，不能停在离开前翻到的地方：offset={}",
                scroll_after.offset
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn other_ctrl_combos_still_reach_the_agent() {
        // 别误伤：Ctrl+C 是 Claude Code 的中断键，Ctrl+B 是它的「转后台」，
        // 两个都必须继续透传。
        assert_eq!(key_to_input(&ctrl('c')), Some("\u{3}".to_string()));
        assert_eq!(key_to_input(&ctrl('b')), Some("\u{2}".to_string()));
    }

    #[test]
    fn arrow_keys_are_forwarded_as_escape_sequences() {
        assert_eq!(key_to_input(&key(KeyCode::Up)).as_deref(), Some("\x1b[A"));
        assert_eq!(key_to_input(&key(KeyCode::Down)).as_deref(), Some("\x1b[B"));
        assert_eq!(
            key_to_input(&key(KeyCode::Right)).as_deref(),
            Some("\x1b[C")
        );
        assert_eq!(key_to_input(&key(KeyCode::Left)).as_deref(), Some("\x1b[D"));
    }

    #[test]
    fn editing_keys_are_forwarded() {
        assert_eq!(
            key_to_input(&key(KeyCode::Backspace)).as_deref(),
            Some("\x7f")
        );
        assert_eq!(key_to_input(&key(KeyCode::Tab)).as_deref(), Some("\t"));
        assert_eq!(
            key_to_input(&key(KeyCode::Delete)).as_deref(),
            Some("\x1b[3~")
        );
    }

    #[test]
    fn enter_sends_empty_string_so_checkpoint_fires() {
        // 空串是与 session::send_input 约定的回车信号，只有它会打检查点
        assert_eq!(key_to_input(&key(KeyCode::Enter)).as_deref(), Some(""));
    }

    #[test]
    fn ctrl_letters_become_control_bytes() {
        let c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_to_input(&c).as_deref(), Some("\u{3}"));
        let a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(key_to_input(&a).as_deref(), Some("\u{1}"));
    }

    #[test]
    fn plain_chars_pass_through() {
        assert_eq!(key_to_input(&key(KeyCode::Char('x'))).as_deref(), Some("x"));
        assert_eq!(
            key_to_input(&key(KeyCode::Char('中'))).as_deref(),
            Some("中")
        );
    }

    #[test]
    fn esc_is_forwarded_to_the_agent() {
        // agent 靠 Esc 做取消/清空/关弹窗，抢走它会让 agent 的交互失灵。
        // 返回看板用 F2。
        assert_eq!(key_to_input(&key(KeyCode::Esc)).as_deref(), Some("\u{1b}"));
    }

    /// `draw()` 是唯一没有靠 client/daemon 就能跑起来的部分——用 `TestBackend`
    /// 把几种 View（看板 / profile 选择弹窗 / 会话屏幕 / 填密钥）实际渲染
    /// 一遍，确认不 panic。这不是端到端验证（没有真的起 daemon、走键盘事件
    /// 循环），但能拦住“布局越界”“空列表 unwrap”这类会在真实交互里当场
    /// 炸掉的问题。这里只是把每种 View 都过一遍顶层 `draw()` 的分派，
    /// 用的多是空/最小 fixture；某个视图内容本身的渲染细节（置灰、原因
    /// 文案、红字警告、密钥打点、二次确认提示……）需要更讲究的 fixture，
    /// 那类测试跟着各自的模块走——目前 `pick.rs` 有 `PickProfile`/
    /// `PickProject` 的渲染细节测试，`secret.rs` 有 `EnterSecret`/
    /// `Secrets` 的；`board.rs`/`attach.rs` 还没有自己的渲染细节测试，
    /// 它们的内容渲染目前只被这条烟雾测试覆盖到"不 panic"这一层。
    #[test]
    fn draw_does_not_panic_for_all_views() {
        use ratatui::backend::TestBackend;

        let sessions = vec![
            SessionInfo {
                id: 1,
                profile: "claude".into(),
                dir: "/tmp/a".into(),
                state: SessionState::Working,
                activity: "正在读取 src/main.rs".into(),
                is_agent: true,
            },
            SessionInfo {
                id: 2,
                profile: "shell".into(),
                dir: "/tmp/b".into(),
                state: SessionState::Asking,
                activity: "要用哪个方案？".into(),
                is_agent: true,
            },
        ];

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.list_state.select(Some(0));
        app.current_dir = PathBuf::from("/tmp/proj");

        // 看板视图，含空消息
        app.view = View::Board;
        app.sessions = sessions.clone();
        app.message = Msg::from("");
        app.connected = true;
        term.draw(|f| draw(f, &mut app)).unwrap();
        // 看板视图，带提示消息
        app.message = Msg::from("完成");
        term.draw(|f| draw(f, &mut app)).unwrap();
        // 看板为空列表也不能 panic
        app.sessions = Vec::new();
        app.message = Msg::from("");
        term.draw(|f| draw(f, &mut app)).unwrap();
        // 断连状态：底部提示和边框都要切到断连样式，也不能 panic
        app.sessions = sessions.clone();
        app.connected = false;
        term.draw(|f| draw(f, &mut app)).unwrap();
        app.connected = true;

        // 九宫格：格子内容的渲染细节在 grid.rs 自己的测试里，这里只过一遍
        // 顶层分派（含底栏那截）
        app.view = View::grid(0);
        term.draw(|f| draw(f, &mut app)).unwrap();

        // profile 选择弹窗
        let mut pick_state = ListState::default();
        pick_state.select(Some(0));
        app.view = View::PickProfile {
            entries: Vec::new(),
            state: pick_state,
            warning: Some("secrets.toml 读不了".into()),
        };
        term.draw(|f| draw(f, &mut app)).unwrap();

        // 已进入会话的屏幕视图
        app.view = View::Attached(1);
        term.draw(|f| draw(f, &mut app)).unwrap();
        // 已进入会话但断连了
        app.connected = false;
        term.draw(|f| draw(f, &mut app)).unwrap();
        app.connected = true;

        // 填密钥视图，三个阶段各画一遍：打字中 / 验证中 / 失败
        for phase in [
            SecretPhase::Typing,
            SecretPhase::Verifying,
            SecretPhase::Failed("这个密钥用不了，可能是复制的时候少了一段".into()),
        ] {
            app.view = View::EnterSecret {
                profile: "kimi".into(),
                label: "Kimi".into(),
                prompt: crate::proto::SecretPrompt {
                    hint: "去 platform.moonshot.cn 生成一个".into(),
                    url: Some("https://platform.moonshot.cn".into()),
                },
                buf: "sk-abc123".into(),
                phase,
                return_to_settings: false,
            };
            term.draw(|f| draw(f, &mut app)).unwrap();
        }
    }

    /// 断连时底部提示必须覆盖普通帮助文案 / 残留的 action 消息——否则用户会盯着
    /// 一句“完成”或按键提示看，误以为守护进程还活着。这里不渲染像素，只检查
    /// `draw()` 写进 buffer 的文字内容确实包含断连提示。
    #[test]
    fn disconnected_state_shows_warning_in_bottom_bar() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/tmp/proj");
        app.view = View::Board;
        app.message = Msg::from("完成");
        app.connected = false;

        term.draw(|f| draw(f, &mut app)).unwrap();
        // ratatui 给宽字符（中文）后面那个 cell 塞的是 " "（`Cell::reset`），
        // 不是空串，所以逐 cell 拼出来的文本每个汉字后面都夹了一个空格
        // （"守 护 进 程..."）。去掉空白之后再做子串匹配，两边都做同样的
        // 归一化，不影响判断力。
        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains("守护进程连不上"),
            "断连时底部应显示明确提示，实际内容（已去空白）: {content}"
        );
        assert!(
            !content.contains("完成"),
            "断连提示必须盖过残留的旧 action 消息，实际内容（已去空白）: {content}"
        );
    }

    #[test]
    fn move_sel_n_clamps_at_both_ends() {
        let mut st = ListState::default();
        st.select(Some(0));

        move_sel_n(&mut st, 3, -1);
        assert_eq!(st.selected(), Some(0), "顶端再往上不动");

        move_sel_n(&mut st, 3, 1);
        move_sel_n(&mut st, 3, 1);
        move_sel_n(&mut st, 3, 1);
        assert_eq!(st.selected(), Some(2), "底端再往下不动");

        // 空列表不能 panic，也不能选中不存在的行
        let mut empty = ListState::default();
        move_sel_n(&mut empty, 0, 1);
        assert_eq!(empty.selected(), None);
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        let area = buf.area;
        let mut s = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    s.push_str(cell.symbol());
                }
            }
            s.push('\n');
        }
        s
    }

    /// 底栏左段的文字。宽字符在 TestBackend 里只占首个 cell，
    /// 所以统一滤掉空白再找子串，跟既有的 bottom_bar_help_follows_the_view 一致。
    fn bar_text(term: &Terminal<ratatui::backend::TestBackend>) -> String {
        buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    /// `ESCAPE_HINT_COLS` 是写死的，`escape_hint` 的文案却会跟着功能改。
    /// 两者一旦脱节，左段会把逃生键**静默截断**——而逃生键正是用户卡住时
    /// 唯一的出路，截断了不会报错、只会让人退不出来。所以这里穷举所有视图，
    /// 要求常量真的容得下最长的那一条。
    /// 一台开着一个 agent 会话、光标停在它上面的 dct。
    ///
    /// 按键表现在跟着「能不能按」走（`idle_help` 的 `HelpCtx`），所以断言
    /// 「某个键在不在屏幕上」之前必须先把那个键**能按**的前提摆出来——
    /// 空看板上没有 `s 停止` 不是 bug，是这次改动的目的。
    fn app_with_one_agent_session(view: View) -> (App, tempfile::TempDir) {
        let (mut app, dir) = App::test_app();
        app.connected = true;
        app.current_dir = std::path::PathBuf::from("/tmp/a");
        app.set_sessions(vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/tmp/a".into(),
            state: SessionState::Working,
            activity: String::new(),
            is_agent: true,
        }]);
        app.list_state.select(Some(0));
        app.view = view;
        (app, dir)
    }

    /// 80 列（最常见的下限）下右段只有 57 列，十来个键放不下。放不下的键
    /// 落进 `?` 浮层，**留在屏幕上的必须是最要紧的那几个**：看得见、进得去、
    /// 加上三个作用在选中会话上的动作。
    ///
    /// 这条盯的是排序：`idle_help` 里的顺序就是「谁先被丢」，把 `c 密钥`
    /// 排到 `s 停止` 前面不会有任何编译错误，只会让 80 列终端上的用户看不到
    /// 停止键。
    #[test]
    fn the_keys_that_survive_eighty_columns_are_the_ones_that_matter() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = app_with_one_agent_session(View::Board);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);

        for key in ["↑↓选择", "Enter进入", "n新建", "s停止", "u回滚", "d改动"] {
            assert!(c.contains(key), "按键表里的「{key}」被截掉了：{c}");
        }
    }

    /// 反过来的一半：空看板上那六个键一个都不该在，而 `n 新建` 必须在——
    /// 这时候它是屏幕上唯一有意义的动作。
    #[test]
    fn an_empty_board_only_offers_what_can_actually_be_done() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = App::test_app();
        app.connected = true;
        app.view = View::Board;
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);

        for key in ["↑↓选择", "Enter进入", "s停止", "u回滚", "d改动"] {
            assert!(!c.contains(key), "空看板上「{key}」按不动，不该写：{c}");
        }
        assert!(c.contains("n新建"), "空看板上唯一该按的键：{c}");
        assert!(c.contains("?…"), "门永远在：{c}");
    }

    /// 被截掉的键必须有个去处，而那扇门本身**绝不能**被截掉。
    /// 丢了它，`p/N/a/c/l` 就成了「屏幕上没写却真管用」的键。
    #[test]
    fn the_door_to_the_rest_of_the_keys_is_always_on_screen() {
        use ratatui::backend::TestBackend;

        for view in [View::Board, View::grid(0)] {
            for width in [40u16, 60, 80, 200] {
                let (mut app, _dir) = app_with_one_agent_session(view.clone());
                let mut term = Terminal::new(TestBackend::new(width, 24)).unwrap();
                term.draw(|f| draw(f, &mut app)).unwrap();
                let c = bar_text(&term);
                assert!(c.contains("?…"), "{width} 列下 `? …` 不见了：{c}");
            }
        }
    }

    /// 键名要加粗。这一行是「字母 + 中文」交替，不给字母一点重量的话，
    /// 用户得逐个词去认哪个是能按的键——这正是用户看着截图说「太啰嗦」的
    /// 那种啰嗦。
    #[test]
    fn the_key_letters_are_bold() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = app_with_one_agent_session(View::Board);
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        let top = bar_top(&term);
        let row = top + 1; // 顶边下面那一行就是按键表
        let bold: String = (0..buf.area.width)
            .filter_map(|x| buf.cell((x, row)))
            .filter(|c| c.style().add_modifier.contains(Modifier::BOLD))
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(bold.contains('n'), "`n` 该是粗的：{bold:?}");
        assert!(bold.contains('s'), "`s` 该是粗的：{bold:?}");
        assert!(
            !bold.contains('新'),
            "只有键名该加粗，说明不该跟着粗：{bold:?}"
        );
    }

    /// 底栏顶边所在的行号。顶边那一行写着「当前项目：」，找它就行。
    fn bar_top(term: &Terminal<ratatui::backend::TestBackend>) -> u16 {
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .find(|y| {
                (0..buf.area.width)
                    .filter_map(|x| buf.cell((x, *y)))
                    .any(|c| c.symbol() == "当")
            })
            .expect("底栏顶边总该在屏幕上")
    }

    /// 按键表**永远只占一行**（加上下边框共 3 行），多窄都一样。
    ///
    /// 这是用户那句「太啰嗦」的直接答复，也是一条结构约束：底栏每多一行，
    /// 内容区就少一行，而九宫格在 80×24 下只差一行就跌破 `grid.rs` 的
    /// `MIN_ROWS`，整屏换成一句「窗口太小」。折行的那一版把这件事变成了
    /// 「往表里加键之前先手算一遍宽度」，人肉记忆守不住。
    #[test]
    fn the_key_bar_is_always_one_line() {
        use ratatui::backend::TestBackend;

        for view in [View::Board, View::grid(0)] {
            for width in [40u16, 60, 80, 100, 160] {
                let (mut app, _dir) = App::test_app();
                app.view = view.clone();
                let mut term = Terminal::new(TestBackend::new(width, 24)).unwrap();
                term.draw(|f| draw(f, &mut app)).unwrap();
                assert_eq!(
                    24 - bar_top(&term),
                    3,
                    "{width} 列下按键表折行了，底栏不再是一行"
                );
            }
        }
    }

    /// 消息**不**跟着按键表一起被压成一行。
    ///
    /// 两者共用底栏这一格，但性质不同：按键表放不下的键在 `?` 浮层里找得
    /// 回来，而一句「XXX 不是会话号」被截掉就是真的没了。所以消息照旧走
    /// `wrap_help` 折行，底栏该长就长。
    #[test]
    fn a_long_message_still_wraps_instead_of_being_squeezed_into_one_line() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = App::test_app();
        app.connected = true;
        app.view = View::Board;
        app.message = format!("{}  {}", "很长的一句话".repeat(5), "另外半句".repeat(5)).into();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(
            24 - bar_top(&term) > 3,
            "长消息该把底栏撑高，而不是被压成一行截掉一半"
        );
    }

    /// 底栏不许把九宫格挤到画不出来。
    ///
    /// 底栏高度改成按内容算之后，按键表每多折一行，内容区就少一行。80×24
    /// 恰好卡在边界上：内容区 20 行 = `grid.rs` 的 `MIN_ROWS`，再少一行整个
    /// 九宫格就换成一句「窗口太小」。往九宫格那份按键表里加一个键就会触发，
    /// 而这在单测里不加一条断言是看不出来的——按键表的测试只数键，不看格子
    /// 还在不在。
    #[test]
    fn the_bottom_bar_never_squeezes_the_grid_off_the_screen() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = App::test_app();
        app.connected = true;
        app.current_dir = std::path::PathBuf::from("/tmp/a");
        app.set_sessions(vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/tmp/a".into(),
            state: SessionState::Working,
            activity: String::new(),
            is_agent: true,
        }]);
        app.view = View::grid(0);

        // 80×24 是最常见的终端下限，这一条必须在这个尺寸上成立
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let screen = bar_text(&term);
        assert!(
            !screen.contains("窗口太小"),
            "80×24 下九宫格必须画得出来，底栏不能把它挤没：{screen}"
        );
        assert!(screen.contains("claude"), "格子该在屏幕上：{screen}");
    }

    /// 终端够宽就该多露几个键出来。底栏恒为一行之后，宽度换来的不再是
    /// 「少折一行」，而是「少丢几个键」——不这么用的话，160 列终端跟 80 列
    /// 看到的是同样一小撮键，宽出来的一百列白白空着。
    #[test]
    fn a_wide_terminal_shows_more_keys() {
        use ratatui::backend::TestBackend;

        let mut counts = Vec::new();
        for width in [80u16, 160] {
            let (mut app, _dir) = App::test_app();
            app.view = View::Board;
            let mut term = Terminal::new(TestBackend::new(width, 24)).unwrap();
            term.draw(|f| draw(f, &mut app)).unwrap();
            counts.push(
                bar_text(&term).matches('…').count() + {
                    let c = bar_text(&term);
                    ["p换项目", "a看全部项目", "c密钥", "l设置", "N换agent"]
                        .iter()
                        .filter(|k| c.contains(**k))
                        .count()
                },
            );
        }
        assert!(
            counts[1] > counts[0],
            "160 列下该比 80 列多露几个键，实际 {counts:?}"
        );
    }

    /// 九宫格的按键表跟看板同一套规矩：留在屏幕上的是最要紧的那几个，
    /// 其中 `i 回一句` 是这个视图独有的能力，不写就找不到。
    #[test]
    fn the_grid_keeps_its_own_key_on_screen_at_eighty_columns() {
        use ratatui::backend::TestBackend;

        let (mut app, _dir) = app_with_one_agent_session(View::grid(0));
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);

        for key in ["q退出", "方向键选格子", "Enter放大", "i回一句", "n新建"] {
            assert!(c.contains(key), "九宫格按键表里的「{key}」被截掉了：{c}");
        }
    }

    /// 选项目是**浮层**不是全屏接管：画完之后，屏幕上必须**同时**看得到
    /// 浮层的内容和背后的看板。上一版全屏接管，用户按 p 的那一刻整个
    /// 界面消失，只剩一个几乎全空的框——这正是「完全是混乱的」的一部分。
    #[test]
    fn the_project_picker_is_an_overlay_not_a_takeover() {
        use ratatui::backend::TestBackend;
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/w/proj");
        app.sessions = vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/w/proj".into(),
            state: SessionState::Idle,
            activity: "背后的看板".into(),
            is_agent: true,
        }];
        app.refresh_visible();
        app.view = View::PickProject(crate::ui::view::ProjectPicker::new(
            vec!["/w/other".to_string()],
            PathBuf::from("/w"),
        ));

        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);

        assert!(c.contains("背后的看板"), "背后的看板必须还看得见：{c}");
        assert!(c.contains("最近"), "浮层自己也要画出来：{c}");
    }

    /// 终端小到放不下浮层时退化成全屏，而不是显示一句「窗口太小」：
    /// 选项目是用户此刻非做不可的事，挡住他没有意义。
    #[test]
    fn a_tiny_terminal_gets_the_picker_full_screen_rather_than_a_refusal() {
        let full = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 6,
        };
        assert_eq!(popup_area(full), full);
    }

    /// 宽屏上浮层不该铺满：一个横跨 200 列的对话框，眼睛要扫过整个屏幕
    /// 才能读完一行。
    #[test]
    fn a_wide_terminal_gets_a_bounded_centered_popup() {
        let full = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 60,
        };
        let p = popup_area(full);
        assert!(p.width <= 100, "宽度要有上限：{}", p.width);
        assert!(p.height <= 24, "高度要有上限：{}", p.height);
        assert!(p.x > 0 && p.y > 0, "要居中，不能贴边");
        assert_eq!(p.x + p.width / 2, full.width / 2, "水平居中");
    }

    #[test]
    fn escape_hint_cols_fits_every_view() {
        use unicode_width::UnicodeWidthStr;
        let views = [
            View::Board,
            View::Attached(1),
            View::grid(0),
            View::PickProfile {
                entries: Vec::new(),
                state: ListState::default(),
                warning: None,
            },
            View::PickProject(crate::ui::view::ProjectPicker {
                filter: String::new(),
                typing_path: None,
                ..crate::ui::view::ProjectPicker::new(Vec::new(), std::path::PathBuf::from("/tmp"))
            }),
            View::PickProject(crate::ui::view::ProjectPicker {
                filter: String::new(),
                typing_path: Some(String::new()),
                ..crate::ui::view::ProjectPicker::new(Vec::new(), std::path::PathBuf::from("/tmp"))
            }),
            View::Secrets {
                entries: Vec::new(),
                state: ListState::default(),
                pending_delete: None,
            },
            View::Settings {
                state: ListState::default(),
            },
            View::Keys {
                from: Box::new(View::Board),
            },
            // 填密钥有两条退路（回设置页 / 回选择器），两条文案都要量
            View::EnterSecret {
                profile: String::new(),
                label: String::new(),
                prompt: crate::proto::SecretPrompt {
                    hint: String::new(),
                    url: None,
                },
                buf: String::new(),
                phase: view::SecretPhase::Typing,
                return_to_settings: true,
            },
            View::EnterSecret {
                profile: String::new(),
                label: String::new(),
                prompt: crate::proto::SecretPrompt {
                    hint: String::new(),
                    url: None,
                },
                buf: String::new(),
                phase: view::SecretPhase::Typing,
                return_to_settings: false,
            },
        ];
        // 两种语言都要量。常量是写死的，而译文长度各不相同——只量中文的话，
        // 哪天某种语言的逃生键更长，就会在那种语言下被静默截断。
        for l in crate::i18n::Lang::all() {
            for v in &views {
                let hint = escape_hint(v, *l);
                assert!(
                    hint.width() <= ESCAPE_HINT_COLS as usize,
                    "{l:?} 下逃生键「{hint}」宽 {} 列，放不进 ESCAPE_HINT_COLS = {ESCAPE_HINT_COLS}",
                    hint.width()
                );
            }
        }
        // 常量不能比需要的更宽：多占的每一列都是从右段的消息里抢的
        let widest = crate::i18n::Lang::all()
            .iter()
            .flat_map(|l| views.iter().map(move |v| escape_hint(v, *l).width()))
            .max()
            .unwrap();
        assert_eq!(
            widest, ESCAPE_HINT_COLS as usize,
            "ESCAPE_HINT_COLS 应当正好等于最长文案的宽度"
        );
    }

    #[test]
    fn escape_hint_survives_a_long_message() {
        use ratatui::backend::TestBackend;

        // 真实事故：在看板上按 p 换项目，「已切到 …」这条消息把整张按键表
        // 顶掉，其中就包括「q 退出」。用户从此没有任何地方能看到怎么退出。
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/tmp");
        app.view = View::Board;
        app.message = Msg::from(
            "已切到 ~/work/dc/dc-terminal，这条消息故意写得很长很长很长很长很长".to_string(),
        );
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);
        assert!(
            c.contains("q退出"),
            "消息再长也不能把退出提示挤掉——这正是用户卡住的那一屏：{c}"
        );
    }

    #[test]
    fn escape_hint_survives_a_disconnect() {
        use ratatui::backend::TestBackend;

        // 出事的那一刻恰恰是最需要逃生提示的时候，断连提示不能把它顶掉。
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/tmp");
        app.view = View::Attached(1);
        app.connected = false;
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = bar_text(&term);
        assert!(
            c.contains("Ctrl+Q（F2）回看板"),
            "断连时逃生提示必须还在：{c}"
        );
        assert!(c.contains("连不上"), "断连提示本身也要显示：{c}");
    }

    #[test]
    fn bottom_bar_shows_current_project() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/Users/lei/work/dc/dc-terminal");
        app.view = View::Board;
        term.draw(|f| draw(f, &mut app)).unwrap();

        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains("dc-terminal"),
            "底部必须显示当前项目，实际（已去空白）: {content}"
        );
    }

    #[test]
    fn error_message_is_red() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let (mut app, _dir) = App::test_app();
        app.current_dir = PathBuf::from("/tmp");
        app.view = View::Board;
        app.message = Msg::err("不是一个目录".into());
        term.draw(|f| draw(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        let area = buf.area;
        let red = (0..area.height).any(|y| {
            (0..area.width).any(|x| {
                buf.cell((x, y))
                    .map(|c| c.style().fg == Some(Color::Red) && c.symbol() != " ")
                    .unwrap_or(false)
            })
        });
        assert!(red, "错误提示必须用红字，否则跟成功提示长得一样");
    }

    #[test]
    fn f2_is_not_forwarded_but_esc_is() {
        // F2 是逆转键，dct 自己吃掉；Esc 必须还给 agent——
        // Claude Code 靠 Esc 取消/清空/关弹窗。
        assert_eq!(key_to_input(&key(KeyCode::F(2))), None);
        assert_eq!(key_to_input(&key(KeyCode::Esc)).as_deref(), Some("\u{1b}"));
        // Ctrl+B 是 Claude Code 的「转后台」，也必须透传
        let ctrl_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_eq!(key_to_input(&ctrl_b).as_deref(), Some("\u{2}"));
    }

    #[test]
    fn f3_is_never_forwarded_to_the_agent() {
        // F3 在附加视图里被 dct 自己吃掉（跳到下一个在跑的会话），
        // 落进 key_to_input 的通配臂本来就返回 None——这条测试钉住这件事，
        // 免得以后有人改这个函数时不小心让它开始转发。
        assert_eq!(key_to_input(&key(KeyCode::F(3))), None);
    }

    #[test]
    fn bottom_bar_help_follows_the_view() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        // `u 回滚` 只在选中了一个 agent 会话时才写，所以这里必须真的有一个
        // ——空看板上没有那个键不是这条测试要抓的 bug（见
        // `an_empty_board_only_offers_what_can_actually_be_done`）。
        let (mut app, _dir) = app_with_one_agent_session(View::Board);

        let text_of = |term: &Terminal<TestBackend>| -> String {
            buffer_text(term.backend().buffer())
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        };

        // 会话视图：绝不能显示看板的按键表——那些键在这里全被转给 agent
        app.view = View::Attached(1);
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = text_of(&term);
        // F2 从右段的「F2 同效」挪进了左段的逃生键本身，两个键并列写在
        // 一处。断言跟着挪：老用户的肌肉记忆仍要在屏幕上找得到。
        assert!(
            c.contains("Ctrl+Q（F2）回看板"),
            "会话视图要给出逆转键提示，且两个键都要点名：{c}"
        );
        assert!(
            c.contains("F3下一个会话"),
            "F3 是九宫格快速跳转的入口，提示里丢了就没人知道：{c}"
        );
        assert!(c.contains("新建会话"), "还要说清新建会话怎么走：{c}");
        assert!(!c.contains("u回滚"), "会话视图不能显示看板按键表：{c}");

        // 看板视图：仍然显示看板的按键表。
        // 必须换一个全新的 TestBackend：ratatui 画宽字符（中文）时只写首个 cell，
        // 跳过被覆盖的第二个 cell，所以复用同一个 backend 时上一帧的残字会留在
        // 那些空位里，拼出「n新回建看…」这种把两帧混在一起的假文本。真实终端上
        // 宽字符本来就盖住两列，不存在这个问题——这纯粹是测试后端的假象。
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        app.view = View::Board;
        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = text_of(&term);
        assert!(c.contains("u回滚"), "看板要显示自己的按键表：{c}");
    }

    #[test]
    fn a_scroll_hint_takes_over_the_bottom_bar_when_there_is_history() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let (mut app, _dir) = app_with_one_agent_session(View::Attached(1));
        app.scroll = crate::session::ScrollState {
            agent_owns: false,
            alt_screen: false,
            max: 500,
            offset: 40,
            new_lines: 0,
        };

        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();

        // `App::test_app()` 默认 `Lang::Zh`，所以断言实际渲染出来的整句话
        // （`i18n::msg::scrolled_up` 的中文版，空白已被上面的 filter 去掉），
        // 不是随便抓两个数字——数字对了但拼错了别的字、或者 offset 算错但
        // 凑巧还是两位数，光查「有没有 4 和 0」都抓不出来。
        assert!(
            c.contains("已往上翻40行·按End回到底部"),
            "翻到哪儿了、怎么回去都要原样写在底栏：{c}"
        );
        assert!(
            !c.contains("F3下一个会话"),
            "有滚动提示可显示时，不该再挤按键表：{c}"
        );
    }

    /// 消息和滚动提示抢同一行时消息赢——消息是对用户刚才那个动作的回应，
    /// 滚动提示是持续状态，盖掉前者会让用户以为自己那步操作没反应。
    #[test]
    fn a_message_beats_the_scroll_hint() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let (mut app, _dir) = app_with_one_agent_session(View::Attached(1));
        app.scroll = crate::session::ScrollState {
            agent_owns: false,
            alt_screen: false,
            max: 500,
            offset: 40,
            new_lines: 0,
        };
        app.message = "已切到某个项目".into();

        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();

        assert!(c.contains("已切到某个项目"), "消息该赢，没显示出来：{c}");
        assert!(!c.contains("按End回到底部"), "滚动提示不该盖过消息：{c}");
    }

    /// F6 回归测试：英文滚动提示走的是 `BarContent::Text`，`wrap_help`
    /// 只在连续两个空格的地方才折行，而这句提示全是单空格，放不下就不是
    /// 折行，是被 `Paragraph`（没挂 `.wrap()`）直接从右边截断。原来的
    /// "↑ Scrolled up 40 line(s) · press End to jump back down" 有 54 列，
    /// 底栏右段宽度是「终端总宽 − 23」，55 列的终端只有 32 列可用——刚好
    /// 卡在「怎么回去」那半句中间，`End` 整个词被截没了：这个宽度算出的
    /// 切点（`old[:32]`）落在 "press " 之后、"End" 之前，用户会看到自己
    /// 正翻着历史，却读不到任何一个字告诉他怎么回去。缩短后的新版本
    /// （28 列）在同样的宽度下完整放得下。
    #[test]
    fn the_way_back_survives_a_narrow_terminal() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(55, 24)).unwrap();
        let (mut app, _dir) = app_with_one_agent_session(View::Attached(1));
        app.lang = crate::i18n::Lang::En;
        app.scroll = crate::session::ScrollState {
            agent_owns: false,
            alt_screen: false,
            max: 500,
            offset: 40,
            new_lines: 0,
        };

        term.draw(|f| draw(f, &mut app)).unwrap();
        let c = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();

        assert!(
            c.contains("End"),
            "55 列的终端上，用户翻到哪儿都不该看不到怎么回去：{c}"
        );
    }

    /// 副作用（`execute!` 往 stdout 写转义序列）没法单测，但「这一帧该不该
    /// 动一下捕获」这个判断能测——`run()` 每帧都靠它决定要不要发
    /// `EnableMouseCapture`/`DisableMouseCapture`，判断错了要么漏开
    /// （滚轮/点击走了终端自己的逻辑）要么漏关（退回看板还拖选不了文字）。
    #[test]
    fn mouse_capture_toggles_only_on_a_real_transition() {
        assert_eq!(mouse_capture_transition(false, false), None);
        assert_eq!(mouse_capture_transition(true, true), None);
        assert_eq!(mouse_capture_transition(false, true), Some(true));
        assert_eq!(mouse_capture_transition(true, false), Some(false));
    }
}
