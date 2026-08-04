// Task 6 实现

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use std::path::PathBuf;
use std::time::Duration;

use crate::client::Client;
use crate::profile::ProfileStatus;
use crate::proto::{socket_path, ProfileEntry, Request, Response, SecretPrompt};
use crate::pty::ScreenSpan;
use crate::session::SessionInfo;
use crate::verify::VerifyOutcome;

mod widgets;
use widgets::{pad_to, screen_to_lines, short_path, truncate};
pub use widgets::{status_color, status_label, Msg};

mod app;
use app::App;

mod view;
use view::{
    back_one_level, escape_hint, expand_path, filter_projects, idle_help, is_ctrl_q,
    message_after_transition, DeleteKeyAction, SecretPhase, View,
};
pub use view::{
    clean_secret, decide_delete_key, digit_index, pick_action, quick_start_target, secret_rows,
    verify_message, verify_outcome_applies_to, PickAction,
};

/// 弱化文字（说明栏、提示、不可用项）统一用这个灰。不能用
/// `Color::DarkGray`：它是 ANSI 亮黑（8 号色），Solarized Dark 等主题
/// 把 8 号色设成和背景同色，整段文字直接隐形——选 agent 菜单里
/// 所有不可用项和说明栏就这样消失过，只剩一个悬空的 ▶。
/// `Indexed` 走 256 色表的固定灰，不经过终端主题的 16 色映射，
/// 深浅背景下都可见。
const DIM: Color = Color::Indexed(245);

/// 还原终端：退出 raw mode、关掉括号粘贴、离开 alternate screen。
///
/// 抽成自由函数是因为有两个调用方——`TerminalGuard::drop` 和信号线程。
/// 两份各自维护的清理代码迟早会漂移，而漂移的后果是用户拿到一个半还原的终端。
///
/// 两步都 `let _ =` 吞错：`Drop` 里不能 panic，而且这里能做的补救本来就只有
/// 「尽量多还原一点」。
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
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

pub fn run(client: Client, default_dir: PathBuf) -> Result<()> {
    // 必须在 enable_raw_mode 之前装：装早了无害（还没进 raw mode 时
    // restore_terminal() 没有副作用，多发一次 LeaveAlternateScreen 也无害），
    // 装晚了就有一个「已经进 raw mode 但信号还没被接管」的真空窗口。
    // 跟 TerminalGuard 提前构造是同一个理由。
    spawn_signal_restore();
    enable_raw_mode()?;
    // 必须在 EnterAlternateScreen / Terminal::new 之前构造：这样即便它们俩失败，
    // raw mode 也还是能被 Drop 恢复。
    let _guard = TerminalGuard;
    let mut stdout = std::io::stdout();
    // 开括号粘贴：不开的话粘贴的文字会一个字符一个事件地进来，
    // 粘一段话就是几百次往返，慢到没法用。
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App::new(client, default_dir);

    loop {
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
                        app.view = match verify_message(outcome) {
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
                                    app.message = format!("已保存 {label} 的密钥").into();
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
                                        Ok(Response::Error(e)) => View::EnterSecret {
                                            profile,
                                            label,
                                            prompt,
                                            buf,
                                            phase: SecretPhase::Failed(e),
                                            return_to_settings,
                                        },
                                        _ => View::EnterSecret {
                                            profile,
                                            label,
                                            prompt,
                                            buf,
                                            phase: SecretPhase::Failed(
                                                "开不了会话，再试一次".into(),
                                            ),
                                            return_to_settings,
                                        },
                                    }
                                }
                                Ok(Response::Error(e)) => View::EnterSecret {
                                    profile,
                                    label,
                                    prompt,
                                    buf,
                                    phase: SecretPhase::Failed(e),
                                    return_to_settings,
                                },
                                _ => View::EnterSecret {
                                    profile,
                                    label,
                                    prompt,
                                    buf,
                                    phase: SecretPhase::Failed("密钥没存上，再试一次".into()),
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
                    app.sessions = v;
                    app.connected = true;
                }
                _ => app.connected = false,
            }
            app.need_sessions = false;
        }
        if app.list_state.selected().is_none() && !app.sessions.is_empty() {
            app.list_state.select(Some(0));
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
                Ok(Response::Screen { lines, cursor }) => {
                    app.screen = lines;
                    app.screen_cursor = cursor;
                    app.connected = true;
                }
                _ => app.connected = false,
            }
        }

        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &app.view,
                    sessions: &app.sessions,
                    st: &mut app.list_state,
                    screen: &app.screen,
                    cursor: app.screen_cursor,
                    message: &app.message,
                    connected: app.connected,
                    current: &app.current_dir.display().to_string(),
                },
            )
        })?;

        // 会话里要跟手：刷新慢了，你敲的字要等下一轮才显示，每次按键都像卡了一下。
        // 看板不需要这么勤快，150ms 足够，也省得每轮都去锁一遍所有会话。
        let tick = if attached { 16 } else { 150 };
        if !event::poll(Duration::from_millis(tick))? {
            continue;
        }
        let ev = event::read()?;
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
                        app.message = Msg::err("守护进程连不上，粘贴的内容没发出去".into());
                    }
                }
                // 手输路径态：粘贴直接进输入框。从别处拷一条路径粘进来一步到位，
                // 这是不做目录浏览器的底气。trim 掉换行——从终端或文件管理器
                // 拷路径经常带一个尾随换行，不去掉会拼出一个不存在的目录。
                View::PickProject {
                    typing_path: Some(buf),
                    ..
                } => buf.push_str(text.trim()),
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
                View::Board => handle_board(&mut app, key)?,
                View::PickProfile { .. } => handle_pick_profile(&mut app, key)?,
                View::PickProject { .. } => handle_pick_project(&mut app, key)?,
                View::Attached(_) => handle_attached(&mut app, key)?,
                View::EnterSecret { .. } => handle_enter_secret(&mut app, key)?,
                View::Secrets { .. } => handle_secrets(&mut app, key)?,
            }
        }

        // 退出必须在这里落地，不能拖到循环末尾的收尾代码之后：quit 只在
        // view 还是 Board 的两条路上置位（Ctrl+Q 在顶层 back_one_level 返回
        // None、或者看板上按 q），今天确实是走到下面 needs_*_refetch /
        // message_after_transition 也不会有副作用——但那是因为"只有 Board
        // 会置 quit"这条事实，不是这段代码本身保证的。往后随便一个新退出点
        // 从别的 view 置位 quit，就会在退出前多打一次 Request::Profiles、
        // 多改一次 app.message。在这里 break 直接还原了原来 `break Ok(())`
        // 的位置——退出这件事不依赖任何视图不变的假设。
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
            app.view = match app.client().and_then(|c| c.call(Request::Profiles)) {
                Ok(Response::Profiles { entries, warning }) => {
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
                Ok(Response::Error(e)) => View::PickProfile {
                    entries: Vec::new(),
                    state: ListState::default(),
                    warning: Some(e),
                },
                _ => View::PickProfile {
                    entries: Vec::new(),
                    state: ListState::default(),
                    warning: Some("拿不到 agent 列表".into()),
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
            app.view = match app.client().and_then(|c| c.call(Request::Profiles)) {
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
                Ok(Response::Error(e)) => {
                    app.message = Msg::err(e);
                    View::Board
                }
                _ => {
                    app.message = Msg::err("拿不到密钥列表".into());
                    View::Board
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

/// **这个函数里永远不要 `continue`。** 它是从主循环的 `match` 里抽出来的，
/// 循环末尾还有一段清理陈旧 `message` 的逻辑；早年这些代码还在循环体里时，
/// 一个 `continue` 跳过了它，一句普通的「已切到 X」盖掉了屏幕上唯一告诉
/// 用户怎么退出的行（`e0ba1ec`）。现在它是函数，`return` 是安全的，
/// 但如果哪天又被内联回循环里，这条约束就会重新生效。
fn handle_board(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Down => move_sel(&mut app.list_state, &app.sessions, 1),
        KeyCode::Up => move_sel(&mut app.list_state, &app.sessions, -1),
        KeyCode::Char('n') | KeyCode::Char('N') => {
            // entries 带的是完整信息（label/note/status/密钥提示/安装提示），
            // 渲染时把置灰项和原因画出来、四种状态各自路由到哪，见
            // pick_action 和下面 View::PickProfile 的按键分支。n 和 N
            // 都要这份列表——n 拿它判断上次那个 agent 现在还在不在
            // Ready，N 拿它渲染选择器——所以只拉一次，不分两条路各拉各的。
            match app.client().and_then(|c| c.call(Request::Profiles)) {
                Ok(Response::Profiles { entries, warning }) => {
                    // 把「拉完列表但没能直开」的三种落点（选择器为空、
                    // 建会话失败两种）收在一处，省得同一段 ListState
                    // 初始化抄三遍——那种抄法迟早有一份漏了空表守卫。
                    let picker = |entries: Vec<ProfileEntry>, warning: Option<String>| {
                        let mut state = ListState::default();
                        // daemon 目前总是至少返回九个内置 profile，这里
                        // 空表分支基本走不到；但选中一个不存在的下标，
                        // 按 Enter 就是 entries[0] 越界 panic——这种最坏
                        // 结果不该只靠"实践中到不了"兜底，一行守卫不值钱。
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
                    let last = if key.code == KeyCode::Char('n') {
                        match app.client().and_then(|c| c.call(Request::LastProfile)) {
                            Ok(Response::LastProfile(l)) => l,
                            _ => None,
                        }
                    } else {
                        None
                    };
                    match quick_start_target(last.as_deref(), &entries) {
                        Some(name) => {
                            // 同 View::PickProfile 里 PickAction::Start 那支：
                            // 「n」等价于「已经替用户选好了上次那个」，
                            // 建完直接进会话，不用再让他确认一遍。
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
                                Ok(Response::Error(e)) => {
                                    app.message = Msg::err(e);
                                    app.view = picker(entries, warning);
                                }
                                _ => {
                                    app.message = Msg::err("创建失败".into());
                                    app.view = picker(entries, warning);
                                }
                            }
                        }
                        None => app.view = picker(entries, warning),
                    }
                }
                // 列表都拿不到，直开和选择器都没法走，只能告诉用户
                // 这次干瞪眼——留在 Board 上，视图没变，走到循环
                // 末尾 message_after_transition 会把这条消息原样
                // 留住（同其他分支，不用 continue 抢跑跳过收尾）。
                Ok(Response::Error(e)) => app.message = Msg::err(e),
                _ => app.message = Msg::err("拿不到 agent 列表".into()),
            }
        }
        KeyCode::Char('p') => {
            // 拿不到列表就不进选择器：进去看见一片空白，用户会以为
            // 自己从来没开过项目。
            match app.client().and_then(|c| c.call(Request::Projects)) {
                Ok(Response::Projects(mut all)) => {
                    // 全新守护进程列表是空的，补上启动目录，
                    // 保证第一次用也不会看到空列表。
                    let start = app.start_dir.display().to_string();
                    if !all.contains(&start) {
                        all.push(start);
                    }
                    let mut state = ListState::default();
                    state.select(Some(0));
                    app.view = View::PickProject {
                        all,
                        filter: String::new(),
                        state,
                        typing_path: None,
                    };
                }
                Ok(Response::Error(e)) => app.message = Msg::err(e),
                _ => app.message = Msg::err("拿不到项目列表".into()),
            }
        }
        KeyCode::Char('c') => {
            // 拿不到列表就不进设置页：留在看板上给一句错误，总比
            // 弹进一个既没数据、又没地方显示错误的空白页强
            // （`View::Secrets` 没有 `warning` 字段，见其字段注释）。
            match app.client().and_then(|c| c.call(Request::Profiles)) {
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
                Ok(Response::Error(e)) => app.message = Msg::err(e),
                _ => app.message = Msg::err("拿不到密钥列表".into()),
            }
        }
        KeyCode::Enter => {
            if let Some(id) = selected(&app.sessions, &app.list_state).map(|s| s.id) {
                app.view = View::Attached(id);
                app.need_sessions = true; // 会话标题要显示项目名
            }
        }
        KeyCode::Char('u') => {
            app.message = act(app, |id| Request::Undo { id });
        }
        KeyCode::Char('s') => {
            app.message = act(app, |id| Request::Stop { id });
        }
        KeyCode::Char('d') => {
            if let Some(id) = selected(&app.sessions, &app.list_state).map(|s| s.id) {
                app.message = match app.client().and_then(|c| c.call(Request::Diff { id })) {
                    Ok(Response::Diff(v)) if v.is_empty() => "没有改动".into(),
                    Ok(Response::Diff(v)) => v
                        .iter()
                        .map(|f| format!("{} +{} -{}", f.path, f.added, f.removed))
                        .collect::<Vec<_>>()
                        .join("  ")
                        .into(),
                    Ok(Response::Error(e)) => Msg::err(e),
                    _ => Msg::err("请求失败".into()),
                };
            }
        }
        _ => {}
    }
    Ok(())
}

/// **这个函数里永远不要 `continue`。** 见 `handle_board` 的文档注释。
fn handle_pick_profile(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::PickProfile {
        entries,
        mut state,
        warning,
    } = app.view.clone()
    else {
        return Ok(());
    };
    if key.code == KeyCode::Esc {
        app.view = View::Board;
    } else {
        // ↑↓ 只挪光标、不选定，所以放在算「选中第几项」之前：
        // 挪完直接落到 chosen = None，不会误触发下面的路由。
        let chosen: Option<usize> = match key.code {
            KeyCode::Down | KeyCode::Up => {
                let d = if key.code == KeyCode::Down { 1 } else { -1 };
                move_sel_n(&mut state, entries.len(), d);
                None
            }
            KeyCode::Enter => state.selected(),
            KeyCode::Char(c) => digit_index(c).filter(|i| *i < entries.len()),
            _ => None,
        };
        // 四条分支的落点：pick_action 只是个纯函数分类器，真正
        // 建会话/开安装窗口这些带副作用的活儿在这里做。
        app.view = match chosen.map(|i| (i, pick_action(&entries[i]))) {
            None => View::PickProfile {
                entries,
                state,
                warning,
            },
            Some((_, PickAction::Start(name))) => {
                // 选完直接进会话。用户选中的意图就是「我要用这个
                // agent 干活」，先弹回看板再让他找一遍自己刚建的
                // 会话是白让人做第二次选择。建失败才回选择器。
                let dir = app.current_dir.display().to_string();
                match app.client().and_then(|c| {
                    c.call(Request::Create {
                        dir,
                        profile: name,
                        // 选择器里选的就是用户真的要用的 agent——
                        // 与「帮你装 CLI」那条 remember=false 的路径区分开。
                        remember: true,
                    })
                }) {
                    Ok(Response::Created { id }) => {
                        app.need_sessions = true; // 会话标题要显示项目名
                        View::Attached(id)
                    }
                    Ok(Response::Error(e)) => {
                        app.message = Msg::err(e);
                        View::PickProfile {
                            entries,
                            state,
                            warning,
                        }
                    }
                    _ => {
                        app.message = Msg::err("创建失败".into());
                        View::PickProfile {
                            entries,
                            state,
                            warning,
                        }
                    }
                }
            }
            Some((i, PickAction::AskSecret(_))) => {
                // AskSecret(usize) 里那个下标只是占位——pick_action
                // 只拿得到一个 &ProfileEntry，不知道它在列表里排第几
                // （见 PickAction 的注释）。真下标是这里的 i，
                // 从 entries[i] 取出来的正是被选中的这一行。
                let e = &entries[i];
                View::EnterSecret {
                    profile: e.name.clone(),
                    label: e.label.clone(),
                    // NeedsSecret 状态却没带 SecretPrompt 是数据不一致
                    // （daemon 那边的 bug），兜底成空提示而不是 panic——
                    // 用户最多看到少一行说明，不该因为这个直接崩溃。
                    prompt: e.secret.clone().unwrap_or(SecretPrompt {
                        hint: String::new(),
                        url: None,
                    }),
                    buf: String::new(),
                    phase: SecretPhase::Typing,
                    // 从选择器进来的意图是「开工」，存完直接建会话，
                    // 不回这里。
                    return_to_settings: false,
                }
            }
            Some((_, PickAction::Install { profile, command })) => {
                // 用命令行会话跑安装命令，让用户看着它装，而不是
                // 干等一句「装不了」。remember: false —— 这不是
                // 用户选的 agent，记了下次按 n 会掉进命令行。
                let dir = app.current_dir.display().to_string();
                match app.client().and_then(|c| {
                    c.call(Request::Create {
                        dir,
                        profile: "shell".into(),
                        remember: false,
                    })
                }) {
                    Ok(Response::Created { id }) => {
                        let line = format!("{}\n", command.join(" "));
                        let _ = app
                            .client()
                            .and_then(|c| c.call(Request::Input { id, text: line }));
                        app.message =
                            format!("正在安装 {profile}，装完按 Ctrl+Q 回看板再按 N").into();
                        app.need_sessions = true;
                        View::Attached(id)
                    }
                    _ => {
                        app.message = Msg::err("开不了安装窗口".into());
                        View::PickProfile {
                            entries,
                            state,
                            warning,
                        }
                    }
                }
            }
            Some((_, PickAction::Blocked(msg))) => {
                app.message = Msg::err(msg);
                View::PickProfile {
                    entries,
                    state,
                    warning,
                }
            }
        };
    }
    Ok(())
}

/// **这个函数里永远不要 `continue`。** 见 `handle_board` 的文档注释。
fn handle_pick_project(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::PickProject {
        all,
        mut filter,
        mut state,
        typing_path,
    } = app.view.clone()
    else {
        return Ok(());
    };
    match typing_path {
        // ——手输路径态：可见字符全进输入框，不再当过滤用——
        Some(mut buf) => match key.code {
            KeyCode::Esc => {
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: None,
                }
            }
            KeyCode::Enter => {
                if buf.trim().is_empty() {
                    // expand_path("", base) 会解析成 base 自己（非绝对路径走
                    // base.join("")），is_dir() 照样为真——空输入不挡住的话，
                    // 用户在这一步犹豫多按一次 Enter，就会被无声切回启动目录。
                    app.message = Msg::err("还没输入路径".into());
                    app.view = View::PickProject {
                        all,
                        filter,
                        state,
                        typing_path: Some(buf),
                    };
                } else {
                    let p = expand_path(&buf, &app.start_dir);
                    if p.is_dir() {
                        // 「当前项目」已经在底部边框标题里，这里说的是刚发生的动作
                        app.message =
                            format!("已切到 {}", short_path(&p.display().to_string())).into();
                        app.current_dir = p;
                        app.view = View::Board;
                    } else {
                        // 不是 git 仓库这件事不在这里判——留给 create()
                        app.message = Msg::err(format!("{} 不是一个目录", p.display()));
                        app.view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: Some(buf),
                        };
                    }
                }
            }
            KeyCode::Backspace => {
                buf.pop();
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: Some(buf),
                };
            }
            KeyCode::Char(c) => {
                buf.push(c);
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: Some(buf),
                };
            }
            _ => {
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: Some(buf),
                }
            }
        },
        // ——列表态——
        None => match key.code {
            KeyCode::Esc => app.view = View::Board,
            KeyCode::Down | KeyCode::Up => {
                let delta = if key.code == KeyCode::Down { 1 } else { -1 };
                // +1 是末行那个「手输路径…」，它不参与过滤，永远在
                let n = filter_projects(&all, &filter).len() + 1;
                move_sel_n(&mut state, n, delta);
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: None,
                };
            }
            KeyCode::Enter => {
                let shown = filter_projects(&all, &filter);
                let i = state.selected().unwrap_or(0);
                if i >= shown.len() {
                    // 选中的是末行「手输路径…」
                    app.view = View::PickProject {
                        all,
                        filter,
                        state,
                        typing_path: Some(String::new()),
                    };
                } else {
                    let p = PathBuf::from(&shown[i]);
                    if p.is_dir() {
                        app.message = format!("已切到 {}", short_path(&shown[i])).into();
                        app.current_dir = p;
                        app.view = View::Board;
                    } else {
                        // 列表里那条不删——可能只是外置盘没挂
                        app.message = Msg::err(format!("{} 现在找不到了", short_path(&shown[i])));
                        app.view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: None,
                        };
                    }
                }
            }
            KeyCode::Backspace => {
                filter.pop();
                state.select(Some(0));
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: None,
                };
            }
            KeyCode::Char(c) => {
                filter.push(c);
                // 过滤变了就回到第一项，否则光标可能停在已被过滤掉的行号上
                state.select(Some(0));
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: None,
                };
            }
            _ => {
                app.view = View::PickProject {
                    all,
                    filter,
                    state,
                    typing_path: None,
                }
            }
        },
    }
    Ok(())
}

/// **这个函数里永远不要 `continue`。** 见 `handle_board` 的文档注释。
fn handle_attached(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Attached(id) = app.view.clone() else {
        return Ok(());
    };
    // F2 是唯一被 dct 吃掉的键，其余一律 key_to_input 翻译成终端字节
    // 送进去——方向键、退格、Tab、Ctrl 组合都要能用，否则在 Claude Code
    // 里连打错字都退不了格。Esc 必须还给 agent——Claude Code 靠它
    // 取消/清空/关弹窗（底部那句 "Esc to cancel"）；Ctrl+B 也必须还回去，
    // 那是 Claude Code 的「转后台」。逆转键挑 F2 是因为没有 CLI agent
    // 在用它，不必搞双击透传那种隐形状态。
    if key.code == KeyCode::F(2) {
        app.view = View::Board;
        app.need_sessions = true;
    } else if let Some(text) = key_to_input(&key) {
        // 发送失败时不能静默吞掉——用户打字没反应会分不清是卡顿还是断连。
        // “连不上”这个视觉状态统一交给循环顶部的 List/Screen 探测去判定。
        if app
            .client()
            .and_then(|c| c.call(Request::Input { id, text }))
            .is_err()
        {
            app.message = Msg::err("守护进程连不上，刚才那次输入没发出去".into());
        }
    }
    Ok(())
}

/// **这个函数里永远不要 `continue`。** 见 `handle_board` 的文档注释。
fn handle_enter_secret(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::EnterSecret {
        profile,
        label,
        prompt,
        mut buf,
        phase,
        return_to_settings,
    } = app.view.clone()
    else {
        return Ok(());
    };
    match phase.clone() {
        SecretPhase::Verifying => {
            // 验证在后台线程跑，buf 已经发出去了，这期间敲字符/回车
            // 都改不了那次正在飞的请求，只会让用户误以为在做别的事。
            // 只留 Esc：想退就现在退，且必须现在就扔掉 verify_rx——
            // 不然迟到的结果会套在一个用户已经不认得的视图上。
            if key.code == KeyCode::Esc {
                app.verify_rx = None;
                app.view = if return_to_settings {
                    View::Secrets {
                        entries: Vec::new(),
                        state: ListState::default(),
                        pending_delete: None,
                    }
                } else {
                    View::PickProfile {
                        entries: Vec::new(),
                        state: ListState::default(),
                        warning: None,
                    }
                };
            } else {
                app.view = View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    phase: SecretPhase::Verifying,
                    return_to_settings,
                };
            }
        }
        SecretPhase::Typing | SecretPhase::Failed(_) => match key.code {
            KeyCode::Esc => {
                app.view = if return_to_settings {
                    View::Secrets {
                        entries: Vec::new(),
                        state: ListState::default(),
                        pending_delete: None,
                    }
                } else {
                    View::PickProfile {
                        entries: Vec::new(),
                        state: ListState::default(),
                        warning: None,
                    }
                };
            }
            KeyCode::Enter => {
                let (tx, rx) = std::sync::mpsc::channel();
                // 后台验证线程要自己开一条到守护进程的连接——主循环这条 client
                // 正忙着画界面。`socket_path()` 是纯函数（只读 $HOME），比把
                // Client 内部私有的 socket 字段掏出来更省事。
                let sock = socket_path();
                let p = profile.clone();
                let v = buf.clone();
                // 结果送回来时要能比对"这还是不是当初发起这次验证的
                // 那个请求"（见 `verify_outcome_applies_to`），所以
                // 在 `p`/`v` 被移进 `Request::VerifySecret` 之前先
                // 各留一份拷贝，跟结果一起送回主循环。
                let stamped_profile = p.clone();
                let stamped_buf = v.clone();
                std::thread::spawn(move || {
                    // 另开一条连接：主循环那条还要继续画界面
                    let outcome = Client::connect(&sock)
                        .and_then(|mut c| {
                            c.call(Request::VerifySecret {
                                profile: p,
                                value: v,
                            })
                        })
                        .map(|r| match r {
                            Response::Verify(o) => o,
                            _ => VerifyOutcome::Unreachable,
                        })
                        .unwrap_or(VerifyOutcome::Unreachable);
                    let _ = tx.send((stamped_profile, stamped_buf, outcome));
                });
                app.verify_rx = Some(rx);
                app.view = View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    phase: SecretPhase::Verifying,
                    return_to_settings,
                };
            }
            KeyCode::Backspace => {
                buf.pop();
                app.view = View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    phase: SecretPhase::Typing,
                    return_to_settings,
                };
            }
            // Ctrl+O 不用 o：o 得留给密钥输入本身
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // MINOR 8（最终整分支 code review）：`open` 只在 macOS
                // 上存在，Linux 桌面环境一般是 `xdg-open`；两边都
                // 打不开的话必须告诉用户，不能让「Ctrl+O 打开申领
                // 页面」这行提示看着能按、按下去却悄无声息——用户
                // 会以为是自己按错了键。
                if let Some(url) = &prompt.url {
                    if !open_url(url) {
                        app.message = Msg::err(format!("打不开浏览器，自己去访问 {url}"));
                    }
                }
                app.view = View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    phase,
                    return_to_settings,
                };
            }
            KeyCode::Char(c) => {
                buf.push(c);
                app.view = View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    phase: SecretPhase::Typing,
                    return_to_settings,
                };
            }
            _ => {
                app.view = View::EnterSecret {
                    profile,
                    label,
                    prompt,
                    buf,
                    phase,
                    return_to_settings,
                };
            }
        },
    }
    Ok(())
}

/// **这个函数里永远不要 `continue`。** 见 `handle_board` 的文档注释。
fn handle_secrets(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Secrets {
        entries,
        mut state,
        pending_delete,
    } = app.view.clone()
    else {
        return Ok(());
    };
    match key.code {
        KeyCode::Esc => app.view = View::Board,
        KeyCode::Down | KeyCode::Up => {
            let d = if key.code == KeyCode::Down { 1 } else { -1 };
            move_sel_n(&mut state, secret_rows(&entries).len(), d);
            // 光标一动就撤销武装状态：武装的是「这一行」，挪开之后
            // 再按第二次 d，落地的必须是新选中行的第一次按键，不能让
            // 上一行攒的「再按一次就删」悄悄延续到新行头上（见 Finding 1）。
            // 顺带清掉「再按一次删除 X」那句消息——行内提示已经跟着
            // 光标挪走了，底部消息栏要是还留着旧行的名字，用户会
            // 搞不清这次挪动到底有没有把武装状态带走。
            if pending_delete.is_some() {
                app.message = "".into();
            }
            app.view = View::Secrets {
                entries,
                state,
                pending_delete: None,
            };
        }
        KeyCode::Enter => {
            let rows = secret_rows(&entries);
            // find 而不是直接 entries[i]：rows 是 entries 过滤掉不需要密钥
            // 的行之后的结果，下标不对应；按名字在 entries 里找回
            // 完整的那一条，才拿得到 label/secret 提示。
            let target = state
                .selected()
                .and_then(|i| rows.get(i))
                .and_then(|(name, _)| entries.iter().find(|e| &e.name == name));
            app.view = match target {
                Some(e) => View::EnterSecret {
                    profile: e.name.clone(),
                    label: e.label.clone(),
                    // 这一页只列了 secret.is_some() 的行（见 secret_rows），
                    // 所以这里的 unwrap_or 只是跟 AskSecret 那条路径的兜底
                    // 手法保持一致，实际不会被这个默认值命中。
                    prompt: e.secret.clone().unwrap_or(SecretPrompt {
                        hint: String::new(),
                        url: None,
                    }),
                    buf: String::new(),
                    phase: SecretPhase::Typing,
                    // 从设置页进来，改完要回设置页
                    return_to_settings: true,
                },
                // Enter 也是「其他键」，没找到目标（没有选中行）时
                // 留在原地也要把武装状态清掉。
                None => View::Secrets {
                    entries,
                    state,
                    pending_delete: None,
                },
            };
        }
        KeyCode::Char('d') => {
            let rows = secret_rows(&entries);
            let target = state.selected().and_then(|i| rows.get(i)).cloned();
            // 判断这半是纯函数（见 decide_delete_key 的文档注释，
            // 它是这个任务的单测入口）；发不发 DeleteSecret 请求
            // 这半必须留在这里，因为它要碰 daemon 连接。
            app.view = match decide_delete_key(target, &pending_delete) {
                // 没配过的密钥没什么可删的——照样发一次 DeleteSecret
                // 只会得到一句空洞的「已删除」，用户会怀疑自己是不是
                // 删错了别的东西。
                DeleteKeyAction::NotConfigured => {
                    app.message = "这个还没配密钥，没什么可删的".into();
                    View::Secrets {
                        entries,
                        state,
                        pending_delete: None,
                    }
                }
                // 第二次按 d：武装记的名字正是当前选中行，才真删。
                DeleteKeyAction::Confirm(name) => {
                    match app.client().and_then(|c| {
                        c.call(Request::DeleteSecret {
                            profile: name.clone(),
                        })
                    }) {
                        Ok(Response::Ok) => {
                            app.message = format!(
                                "已删除 {} 的密钥",
                                entries
                                    .iter()
                                    .find(|e| e.name == name)
                                    .map(|e| e.label.clone())
                                    .unwrap_or(name.clone())
                            )
                            .into();
                            refetch_secrets(app, Some(&name))
                        }
                        Ok(Response::Error(e)) => {
                            app.message = Msg::err(e);
                            View::Secrets {
                                entries,
                                state,
                                pending_delete: None,
                            }
                        }
                        _ => {
                            app.message = Msg::err("密钥没删掉，再试一次".into());
                            View::Secrets {
                                entries,
                                state,
                                pending_delete: None,
                            }
                        }
                    }
                }
                // 第一次按 d：武装，不发任何请求。行内会画出「再按
                // d 删除」（见 draw() 里 pending_delete 那一支）；
                // 消息栏再重复一遍是双保险，行内提示万一没看到，
                // 底栏还有一句。
                DeleteKeyAction::Arm(name) => {
                    app.message = format!(
                        "再按一次 d 删除 {} 的密钥，按其他键取消",
                        entries
                            .iter()
                            .find(|e| e.name == name)
                            .map(|e| e.label.clone())
                            .unwrap_or_else(|| name.clone())
                    )
                    .into();
                    View::Secrets {
                        entries,
                        state,
                        pending_delete: Some(name),
                    }
                }
                DeleteKeyAction::NoSelection => View::Secrets {
                    entries,
                    state,
                    pending_delete: None,
                },
            };
        }
        // 任何其他键都取消武装——这是 Finding 1 要求的「反应性按键
        // 不该踩中确认」的核心：只有原地再按一次 d 才算确认，别的
        // 任何输入都当作取消，而不是悄悄忽略武装状态继续挂着。
        _ => {
            // 同 ↑↓ 分支：武装期间挂着的「再按一次删除 X」提示要
            // 跟着武装状态一起清掉，不然取消之后底部还留着一句
            // 半真半假的话。
            if pending_delete.is_some() {
                app.message = "".into();
            }
            app.view = View::Secrets {
                entries,
                state,
                pending_delete: None,
            }
        }
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

fn act(app: &mut App, make: impl Fn(u32) -> Request) -> Msg {
    let id = match selected(&app.sessions, &app.list_state) {
        None => return "没有选中会话".into(),
        Some(s) => s.id,
    };
    match app.client().and_then(|c| c.call(make(id))) {
        Ok(Response::Ok) => "完成".into(),
        Ok(Response::Error(e)) => Msg::err(e),
        _ => Msg::err("请求失败".into()),
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
    let result = app.client.as_mut().map(|c| c.call(Request::Profiles));
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

/// 左段固定占的列数：「Ctrl+Q 回看板」= 6 + 1 + 中文 3 字 × 2 = 13。
/// 三条文案里最长的就是它（「Ctrl+Q 回列表」同宽，「q 退出」更短）。
/// 写死而不是每帧算：左段宽度跟着文案跳动会让右段的消息忽宽忽窄。
const ESCAPE_HINT_COLS: u16 = 13;

/// 画一帧界面所需的全部输入。`draw()` 本身不产生任何状态，纯粹是把这些
/// 只读快照（加一个看板光标的可变借用）铺到屏幕上——打包成结构体只是为了
/// 让参数个数别再撞 clippy 的 `too_many_arguments`，不代表这些字段之间
/// 有什么共同的生命周期或所有权关系。
struct DrawInput<'a> {
    view: &'a View,
    sessions: &'a [SessionInfo],
    st: &'a mut ListState,
    screen: &'a [Vec<ScreenSpan>],
    cursor: (u16, u16),
    message: &'a Msg,
    connected: bool,
    current: &'a str,
}

fn draw(f: &mut Frame, ui: &mut DrawInput) {
    // 除 `st` 外都是引用/Copy 类型，读一份出来不算移动；`st` 是 `&mut`，
    // 得显式重借用，不然会撞上“不能从可变引用背后移走字段”。
    let view = ui.view;
    let sessions = ui.sessions;
    let st: &mut ListState = &mut *ui.st;
    let screen = ui.screen;
    let cursor = ui.cursor;
    let message = ui.message;
    let connected = ui.connected;
    let current = ui.current;
    let chunks = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(f.area());

    // 断连时用红色边框给出明确的视觉提示：界面上的数据是上一次成功请求
    // 留下的陈旧快照，不代表守护进程现在的真实状态。
    let border_style = if connected {
        Style::default()
    } else {
        Style::default().fg(Color::Red)
    };

    match view {
        View::Attached(id) => {
            // 标题显示用户当初指定的项目目录，不是内部的 worktree 路径——
            // 给用户看 .git/dct-worktrees/s2 只会让他不知道自己在哪。
            let project = sessions
                .iter()
                .find(|s| s.id == *id)
                .map(|s| short_path(&s.dir))
                .unwrap_or_default();
            let title = if connected {
                format!("会话 {id} · {project} —— F2 返回看板")
            } else {
                format!("会话 {id} · {project}（连接已断开，画面可能过期）—— F2 返回看板")
            };
            let area = chunks[0];
            f.render_widget(
                Paragraph::new(screen_to_lines(screen)).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style)
                        .title(title),
                ),
                area,
            );
            // 把 agent 屏幕里的光标位置映射到真实终端上。没有这一步用户
            // 看到的只是一张死截图，不知道自己打的字会落在哪。+1 是边框。
            let (row, col) = cursor;
            let x = area.x + 1 + col;
            let y = area.y + 1 + row;
            if x < area.x + area.width.saturating_sub(1)
                && y < area.y + area.height.saturating_sub(1)
            {
                f.set_cursor_position((x, y));
            }
        }
        View::PickProfile {
            entries,
            state,
            warning,
        } => {
            let items: Vec<ListItem> = entries
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let num = if i < 9 {
                        format!("{}. ", i + 1)
                    } else {
                        "   ".to_string()
                    };
                    let reason = match &e.status {
                        ProfileStatus::Ready => String::new(),
                        ProfileStatus::NeedsSecret => "（未填密钥）".into(),
                        ProfileStatus::NeedsDependency { label } => {
                            format!("（需要先装 {label}）")
                        }
                        ProfileStatus::NotInstalled { .. } => "（未安装）".into(),
                    };
                    // 不可用的整行压暗，不只是把原因压暗——用户是先看名字再看原因的，
                    // 名字亮着会让他先以为能用
                    let base = if matches!(e.status, ProfileStatus::Ready) {
                        Style::default()
                    } else {
                        Style::default().fg(DIM)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(num, base),
                        Span::styled(pad_to(&truncate(&e.label, 14), 14), base),
                        Span::styled(pad_to(&truncate(&e.note, 26), 26), base.fg(DIM)),
                        Span::styled(reason, base.fg(DIM)),
                    ]))
                })
                .collect();

            // warning 这里直接原样显示，不做字符串加工——分类翻译成人话是
            // secrets.rs（load_error）/ profile.rs（load_dir）的责任，
            // 到这里的时候应该已经是完整的中文句子。唯一保留的例外是
            // profile.rs::describe_toml_error 里「expected ...」那半句可能
            // 是英文：那是用户自己写的 profile TOML 解析报错，行号已经是
            // 中文「第 N 行」，用户本来就在手改 TOML 文件，英文的语法期望
            // 提示比吞掉更有用（详见该函数的注释）。
            let title = match warning {
                Some(w) => format!("选 agent —— {w}"),
                None => "选 agent".to_string(),
            };
            let border = if warning.is_some() {
                Style::default().fg(Color::Red)
            } else {
                border_style
            };
            let mut s = state.clone();
            f.render_stateful_widget(
                List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(border)
                            .title(title),
                    )
                    .highlight_symbol("▶ "),
                chunks[0],
                &mut s,
            );
        }
        View::EnterSecret {
            label,
            prompt,
            buf,
            phase,
            return_to_settings,
            ..
        } => {
            let mut lines: Vec<Line> = Vec::new();
            if !prompt.hint.is_empty() {
                lines.push(Line::from(Span::styled(
                    prompt.hint.clone(),
                    Style::default().fg(DIM),
                )));
                lines.push(Line::from(""));
            }
            // 显示成圆点：密钥不该以明文停在屏幕上，用户可能在录屏或在办公室
            lines.push(Line::from(format!("{}▌", "•".repeat(buf.chars().count()))));
            lines.push(Line::from(""));
            match phase {
                SecretPhase::Typing => {}
                SecretPhase::Verifying => lines.push(Line::from(Span::styled(
                    "正在验证…",
                    Style::default().fg(Color::Cyan),
                ))),
                SecretPhase::Failed(m) => lines.push(Line::from(Span::styled(
                    m.clone(),
                    Style::default().fg(Color::Red),
                ))),
            }
            if prompt.url.is_some() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Ctrl+O 打开申领页面",
                    Style::default().fg(DIM),
                )));
            }
            // IMPORTANT 3（最终整分支 code review）：Task 13 把「回哪」这句话
            // 在 `escape_hint`/`idle_help` 上按 `return_to_settings` 分了岔，
            // 唯独漏了这个标题——它照旧硬编码「回列表」，跟低一行的底栏
            // 「Esc 回设置」当场自相矛盾，而标题字号更大，用户会先信错的
            // 那句。这里补上同样的分支，别让第三处文案再单独漂移。
            let title = if *return_to_settings {
                format!("填 {label} 的密钥（Enter 确认，Esc 返回设置）")
            } else {
                format!("填 {label} 的密钥（Enter 确认，Esc 返回列表）")
            };
            f.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style)
                        .title(title),
                ),
                chunks[0],
            );
        }
        View::PickProject {
            all,
            filter,
            state,
            typing_path,
        } => {
            if let Some(buf) = typing_path {
                f.render_widget(
                    Paragraph::new(format!("{buf}▌")).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(border_style)
                            .title("输入项目路径（Enter 确认，Esc 返回列表）"),
                    ),
                    chunks[0],
                );
            } else {
                let shown = filter_projects(all, filter);
                let mut items: Vec<ListItem> = shown
                    .iter()
                    .map(|p| {
                        let short = short_path(p);
                        let name = std::path::Path::new(p)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| short.clone());
                        ListItem::new(Line::from(vec![
                            Span::raw(format!("{:<20}", truncate(&name, 20))),
                            Span::styled(truncate(&short, 50), Style::default().fg(DIM)),
                        ]))
                    })
                    .collect();
                // 兜底入口不参与过滤，永远在最后一行
                items.push(ListItem::new(Line::from(Span::styled(
                    "手输路径…",
                    Style::default().fg(Color::Cyan),
                ))));

                let title = if filter.is_empty() {
                    "选项目（↑↓ 选，Enter 确认，直接打字过滤，Esc 取消）".to_string()
                } else {
                    format!("选项目（过滤：{filter}）")
                };
                // state 是 View 里那份的副本，draw 只读不写，所以这里克隆一份给
                // render_stateful_widget 用，不去动 `st`（那是看板的光标）。
                let mut s = state.clone();
                f.render_stateful_widget(
                    List::new(items)
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(border_style)
                                .title(title),
                        )
                        .highlight_symbol("▶ "),
                    chunks[0],
                    &mut s,
                );
            }
        }
        View::Board => {
            let title = if connected {
                "dct 会话看板".to_string()
            } else {
                "dct 会话看板（连接已断开，数据可能已过期）".to_string()
            };
            let items: Vec<ListItem> = sessions
                .iter()
                .map(|s| {
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{:>3}  ", s.id)),
                        Span::styled(
                            format!("{:<8}", status_label(s.state)),
                            Style::default().fg(status_color(s.state)),
                        ),
                        Span::raw(format!("{:<10}", s.profile)),
                        Span::styled(
                            format!("{:<22}", truncate(&short_path(&s.dir), 22)),
                            Style::default().fg(DIM),
                        ),
                        Span::raw(truncate(&s.activity, 60)),
                    ]))
                })
                .collect();
            f.render_stateful_widget(
                List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(border_style)
                            .title(title),
                    )
                    .highlight_symbol("▶ "),
                chunks[0],
                st,
            );
        }
        View::Secrets {
            entries,
            state,
            pending_delete,
        } => {
            let rows = secret_rows(entries);
            let items: Vec<ListItem> = rows
                .iter()
                .map(|(name, configured)| {
                    // 按名字回 entries 里找 label：rows 只是「名字 + 配没配」
                    // 这两列的投影，界面上要给用户看的是人话名字，不是内部标识。
                    let label = entries
                        .iter()
                        .find(|e| &e.name == name)
                        .map(|e| e.label.clone())
                        .unwrap_or_else(|| name.clone());
                    // 武装了删除的那一行不显示「已配」——显示「再按 d 删除」，
                    // 让用户在犯下第二次按键之前，眼睛里看到的就是明确的警告，
                    // 而不是靠底部消息栏一句可能被扫过的小字（见 Finding 1）。
                    if pending_delete.as_deref() == Some(name.as_str()) {
                        ListItem::new(Line::from(vec![
                            Span::raw(pad_to(&truncate(&label, 14), 14)),
                            Span::styled(
                                "再按 d 删除，按其他键取消",
                                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                            ),
                        ]))
                    } else {
                        ListItem::new(Line::from(vec![
                            Span::raw(pad_to(&truncate(&label, 14), 14)),
                            Span::styled(
                                if *configured { "已配" } else { "未配" },
                                Style::default().fg(if *configured { Color::Green } else { DIM }),
                            ),
                        ]))
                    }
                })
                .collect();
            let mut s = state.clone();
            f.render_stateful_widget(
                List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(border_style)
                            .title("密钥设置"),
                    )
                    .highlight_symbol("▶ "),
                chunks[0],
                &mut s,
            );
        }
    }

    // 提示必须跟着视图走。底部栏原来不分视图，进了会话仍写着看板的按键表，
    // 而那些键在会话视图里全部被转发给 agent——用户照着按 n，字母 n 会落进
    // Claude Code 的输入框。显示做不到的操作比不显示更糟。
    //
    // 逃生键那一截已经挪进左段常驻，这里不再重复。
    let (help, style) = if !connected {
        (
            "守护进程连不上，界面数据可能已过期".to_string(),
            Style::default().fg(Color::Red),
        )
    } else if message.text.is_empty() {
        (idle_help(view).to_string(), Style::default())
    } else if message.error {
        (message.text.clone(), Style::default().fg(Color::Red))
    } else {
        (message.text.clone(), Style::default())
    };
    // 当前项目放在边框标题里，框内只留一行字。中文是双宽字符，
    // 「当前项目：~/work/dc/dc-terminal」加上按键表在 80 列终端里放不下同一行，
    // 挤在一起会被 Paragraph 直接截断——标题行本来就空着，正好用它。
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("当前项目：{}", short_path(current)));
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
        Paragraph::new(escape_hint(view)).style(Style::default().fg(Color::Cyan)),
        bar[0],
    );
    f.render_widget(
        Paragraph::new(truncate(&help, bar[1].width as usize)).style(style),
        bar[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::InstallPrompt;
    use crate::session::SessionState;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn entry(name: &str, status: ProfileStatus) -> ProfileEntry {
        ProfileEntry {
            name: name.into(),
            label: name.into(),
            note: String::new(),
            has_secret: status != ProfileStatus::NeedsSecret,
            status,
            secret: None,
            install: None,
        }
    }

    /// 给一个 entry 挂上密钥提示——`secret_rows` 只列 `secret.is_some()` 的
    /// 行，光靠 `status` 不够，得真的声明了密钥这件事才会出现在密钥页上。
    /// `has_secret` 不在这里动，沿用 `entry()` 按 `status` 给的默认值——
    /// 两个测试用例恰好落在 `has_secret` 跟 `status` 一致的那一半（见
    /// `secret_rows` 的注释里 `NeedsDependency`/`NotInstalled` 那个反例）。
    fn with_secret(mut e: ProfileEntry) -> ProfileEntry {
        e.secret = Some(SecretPrompt {
            hint: String::new(),
            url: None,
        });
        e
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
    /// 把三种 View（看板 / profile 选择弹窗 / 会话屏幕）实际渲染一遍，确认不 panic。
    /// 这不是端到端验证（没有真的起 daemon、走键盘事件循环），但能拦住
    /// “布局越界”“空列表 unwrap”这类会在真实交互里当场炸掉的问题。
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
            },
            SessionInfo {
                id: 2,
                profile: "shell".into(),
                dir: "/tmp/b".into(),
                state: SessionState::Asking,
                activity: "要用哪个方案？".into(),
            },
        ];

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        st.select(Some(0));

        // 看板视图，含空消息
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Board,
                    sessions: &sessions,
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp/proj",
                },
            )
        })
        .unwrap();
        // 看板视图，带提示消息
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Board,
                    sessions: &sessions,
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from("完成"),
                    connected: true,
                    current: "/tmp/proj",
                },
            )
        })
        .unwrap();
        // 看板为空列表也不能 panic
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Board,
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp/proj",
                },
            )
        })
        .unwrap();
        // 断连状态：底部提示和边框都要切到断连样式，也不能 panic
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Board,
                    sessions: &sessions,
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: false,
                    current: "/tmp/proj",
                },
            )
        })
        .unwrap();
        // profile 选择弹窗：混一点 Ready/未装/未填密钥/缺依赖，外加一条
        // warning，把置灰、原因文案、红色边框都过一遍，确认都不 panic。
        let mut pick_state = ListState::default();
        pick_state.select(Some(0));
        let profile_entries = vec![
            ProfileEntry {
                name: "claude".into(),
                label: "Claude Code".into(),
                note: "官方 CLI".into(),
                status: ProfileStatus::Ready,
                secret: None,
                install: None,
                has_secret: false,
            },
            ProfileEntry {
                name: "kimi".into(),
                label: "Kimi".into(),
                note: "月之暗面".into(),
                status: ProfileStatus::NeedsSecret,
                secret: None,
                install: None,
                has_secret: false,
            },
            ProfileEntry {
                name: "glm".into(),
                label: "GLM".into(),
                note: "智谱".into(),
                status: ProfileStatus::NeedsDependency {
                    label: "Claude".into(),
                },
                secret: None,
                install: None,
                has_secret: false,
            },
            ProfileEntry {
                name: "codex".into(),
                label: "Codex".into(),
                note: "OpenAI".into(),
                status: ProfileStatus::NotInstalled {
                    command: "codex".into(),
                },
                secret: None,
                install: Some(InstallPrompt {
                    command: vec![
                        "npm".into(),
                        "i".into(),
                        "-g".into(),
                        "@openai/codex".into(),
                    ],
                    note: String::new(),
                }),
                has_secret: false,
            },
        ];
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::PickProfile {
                        entries: profile_entries.clone(),
                        state: pick_state.clone(),
                        warning: Some("secrets.toml 读不了".into()),
                    },
                    sessions: &sessions,
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp/proj",
                },
            )
        })
        .unwrap();
        // 已进入会话的屏幕视图
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Attached(1),
                    sessions: &sessions,
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp/proj",
                },
            )
        })
        .unwrap();
        // 已进入会话但断连了
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Attached(1),
                    sessions: &sessions,
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: false,
                    current: "/tmp/proj",
                },
            )
        })
        .unwrap();
        // 填密钥视图，三个阶段各画一遍：打字中 / 验证中 / 失败
        for phase in [
            SecretPhase::Typing,
            SecretPhase::Verifying,
            SecretPhase::Failed("这个密钥用不了，可能是复制的时候少了一段".into()),
        ] {
            term.draw(|f| {
                draw(
                    f,
                    &mut DrawInput {
                        view: &View::EnterSecret {
                            profile: "kimi".into(),
                            label: "Kimi".into(),
                            prompt: SecretPrompt {
                                hint: "去 platform.moonshot.cn 生成一个".into(),
                                url: Some("https://platform.moonshot.cn".into()),
                            },
                            buf: "sk-abc123".into(),
                            phase,
                            return_to_settings: false,
                        },
                        sessions: &sessions,
                        st: &mut st,
                        screen: &[],
                        cursor: (0, 0),
                        message: &Msg::from(""),
                        connected: true,
                        current: "/tmp/proj",
                    },
                )
            })
            .unwrap();
        }
    }

    /// 密钥比窄终端还宽的时候，圆点行不能把 ratatui 的 buffer 写出界——
    /// 真实场景：40 列的分屏终端 + 一个 100 字符的长 token。
    #[test]
    fn secret_view_dots_line_does_not_panic_when_wider_than_the_terminal() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut st = ListState::default();
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::EnterSecret {
                        profile: "kimi".into(),
                        label: "Kimi".into(),
                        prompt: SecretPrompt {
                            hint: String::new(),
                            url: None,
                        },
                        buf: "x".repeat(200),
                        phase: SecretPhase::Typing,
                        return_to_settings: false,
                    },
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp",
                },
            )
        })
        .unwrap();
    }

    /// MINOR 7（最终整分支 code review）：`draw_does_not_panic_for_all_views`
    /// 拿 `"sk-abc123"` 渲染过填密钥的三个阶段，但只断言了不 panic——真正
    /// 要守住的那一行（`"•".repeat(...)`）没人盯着。这条测试直接确认明文
    /// 不会出现在屏幕上，把这条这个分支上最要紧的安全属性变成一个真正的
    /// 回归测试，而不是"看代码觉得应该没问题"。
    #[test]
    fn secret_view_masks_the_key_on_screen() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::EnterSecret {
                        profile: "kimi".into(),
                        label: "Kimi".into(),
                        prompt: SecretPrompt {
                            hint: String::new(),
                            url: None,
                        },
                        buf: "sk-abc123".into(),
                        phase: SecretPhase::Typing,
                        return_to_settings: false,
                    },
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp",
                },
            )
        })
        .unwrap();
        assert!(
            !buffer_text(term.backend().buffer()).contains("sk-abc123"),
            "密钥不能以明文出现在屏幕上"
        );
    }

    /// IMPORTANT 3（最终整分支 code review）：Task 13 把「Esc 回哪」这句话
    /// 按 `return_to_settings` 分了岔，但只改了 `escape_hint`/`idle_help`
    /// 两处，标题（画面里字号最大的那句话）被漏掉了，硬编码成「回列表」，
    /// 从设置页进来的这一屏会同时印着「回列表」（标题）和「回设置」
    /// （底栏）——两句自相矛盾。两种来源各画一遍，断言画面上只出现跟
    /// 这次来源匹配的那句话，另一句完全不出现，防止标题再单独漂移一次。
    #[test]
    fn secret_view_title_agrees_with_escape_hint_for_both_origins() {
        use ratatui::backend::TestBackend;

        let render = |return_to_settings: bool| -> String {
            let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
            let mut st = ListState::default();
            term.draw(|f| {
                draw(
                    f,
                    &mut DrawInput {
                        view: &View::EnterSecret {
                            profile: "kimi".into(),
                            label: "Kimi".into(),
                            prompt: SecretPrompt {
                                hint: String::new(),
                                url: None,
                            },
                            buf: String::new(),
                            phase: SecretPhase::Typing,
                            return_to_settings,
                        },
                        sessions: &[],
                        st: &mut st,
                        screen: &[],
                        cursor: (0, 0),
                        message: &Msg::from(""),
                        connected: true,
                        current: "/tmp",
                    },
                )
            })
            .unwrap();
            buffer_text(term.backend().buffer())
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        };

        let from_picker = render(false);
        assert!(from_picker.contains("返回列表"), "{from_picker}");
        assert!(!from_picker.contains("返回设置"), "{from_picker}");

        let from_settings = render(true);
        assert!(from_settings.contains("返回设置"), "{from_settings}");
        assert!(!from_settings.contains("返回列表"), "{from_settings}");
    }

    /// 断连时底部提示必须覆盖普通帮助文案 / 残留的 action 消息——否则用户会盯着
    /// 一句“完成”或按键提示看，误以为守护进程还活着。这里不渲染像素，只检查
    /// `draw()` 写进 buffer 的文字内容确实包含断连提示。
    #[test]
    fn disconnected_state_shows_warning_in_bottom_bar() {
        use ratatui::backend::TestBackend;

        let sessions: Vec<SessionInfo> = Vec::new();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();

        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Board,
                    sessions: &sessions,
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from("完成"),
                    connected: false,
                    current: "/tmp/proj",
                },
            )
        })
        .unwrap();
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

    #[test]
    fn escape_hint_survives_a_long_message() {
        use ratatui::backend::TestBackend;

        // 真实事故：在看板上按 p 换项目，「已切到 …」这条消息把整张按键表
        // 顶掉，其中就包括「q 退出」。用户从此没有任何地方能看到怎么退出。
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut st = ListState::default();
        let long = Msg::from(
            "已切到 ~/work/dc/dc-terminal，这条消息故意写得很长很长很长很长很长".to_string(),
        );
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Board,
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &long,
                    connected: true,
                    current: "/tmp",
                },
            )
        })
        .unwrap();
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
        let mut st = ListState::default();
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Attached(1),
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: false,
                    current: "/tmp",
                },
            )
        })
        .unwrap();
        let c = bar_text(&term);
        assert!(c.contains("Ctrl+Q回看板"), "断连时逃生提示必须还在：{c}");
        assert!(c.contains("连不上"), "断连提示本身也要显示：{c}");
    }

    #[test]
    fn bottom_bar_shows_current_project() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Board,
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/Users/lei/work/dc/dc-terminal",
                },
            )
        })
        .unwrap();

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
        let mut st = ListState::default();
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Board,
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::err("不是一个目录".into()),
                    connected: true,
                    current: "/tmp",
                },
            )
        })
        .unwrap();

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
    fn draw_does_not_panic_for_project_picker() {
        use ratatui::backend::TestBackend;

        let mut st = ListState::default();
        st.select(Some(0));
        let all = vec![
            "/Users/lei/work/dc/dc-terminal".to_string(),
            "/Users/lei/work/dc/dc_workbench".to_string(),
        ];

        // 列表态。每一段都新建一个 Terminal：ratatui 画中文这种宽字符时只写
        // 首格、第二格保留旧值，同一个 TestBackend 连画两帧再断言，上一帧的
        // 残字会拼进来，产生假阳性/假阴性（见 bottom_bar_help_follows_the_view
        // 的注释）。这里每段内容长度、宽字符落点都不同，实测确实会踩上，
        // 所以都换新的 TestBackend，跟既有测试的写法保持一致。
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::PickProject {
                        all: all.clone(),
                        filter: String::new(),
                        state: st.clone(),
                        typing_path: None,
                    },
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp",
                },
            )
        })
        .unwrap();

        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(content.contains("dc-terminal"), "列表要显示项目：{content}");
        assert!(
            content.contains("手输路径"),
            "末行兜底入口必须在：{content}"
        );

        // 过滤到无匹配：只剩兜底那一行，不能 panic
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::PickProject {
                        all: all.clone(),
                        filter: "没有这个".to_string(),
                        state: st.clone(),
                        typing_path: None,
                    },
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp",
                },
            )
        })
        .unwrap();
        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains("手输路径"),
            "无匹配时兜底入口仍要在：{content}"
        );

        // 手输态
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::PickProject {
                        all: all.clone(),
                        filter: String::new(),
                        state: st.clone(),
                        typing_path: Some("~/work/x".to_string()),
                    },
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp",
                },
            )
        })
        .unwrap();
        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains("~/work/x"),
            "手输态要回显已输入的路径：{content}"
        );

        // 空列表（全新守护进程）也不能 panic
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::PickProject {
                        all: Vec::new(),
                        filter: String::new(),
                        state: ListState::default(),
                        typing_path: None,
                    },
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp",
                },
            )
        })
        .unwrap();
    }

    #[test]
    fn bottom_bar_help_follows_the_view() {
        use ratatui::backend::TestBackend;

        let sessions = vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/tmp/a".into(),
            state: SessionState::Working,
            activity: String::new(),
        }];
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut st = ListState::default();

        let text_of = |term: &Terminal<TestBackend>| -> String {
            buffer_text(term.backend().buffer())
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        };

        // 会话视图：绝不能显示看板的按键表——那些键在这里全被转给 agent
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Attached(1),
                    sessions: &sessions,
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp/a",
                },
            )
        })
        .unwrap();
        let c = text_of(&term);
        assert!(c.contains("Ctrl+Q回看板"), "会话视图要给出逆转键提示：{c}");
        assert!(
            c.contains("F2同效"),
            "F2 是老用户的肌肉记忆，也要留在提示里：{c}"
        );
        assert!(c.contains("新建会话"), "还要说清新建会话怎么走：{c}");
        assert!(!c.contains("u回滚"), "会话视图不能显示看板按键表：{c}");

        // 看板视图：仍然显示看板的按键表。
        // 必须换一个全新的 TestBackend：ratatui 画宽字符（中文）时只写首个 cell，
        // 跳过被覆盖的第二个 cell，所以复用同一个 backend 时上一帧的残字会留在
        // 那些空位里，拼出「n新回建看…」这种把两帧混在一起的假文本。真实终端上
        // 宽字符本来就盖住两列，不存在这个问题——这纯粹是测试后端的假象。
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Board,
                    sessions: &sessions,
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp/a",
                },
            )
        })
        .unwrap();
        let c = text_of(&term);
        assert!(c.contains("u回滚"), "看板要显示自己的按键表：{c}");
    }

    #[test]
    fn secrets_view_renders_without_panicking_when_nothing_needs_a_key() {
        // 边界情况：所有 profile 都不需要密钥（或者用户碰巧只装了这类）。
        // 空列表不该让渲染 panic，也不该显示成一片空白无提示——至少标题
        // 「密钥设置」得画出来。
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        let text_of = |term: &Terminal<TestBackend>| -> String {
            buffer_text(term.backend().buffer())
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        };
        let entries = vec![entry("claude", ProfileStatus::Ready)];
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Secrets {
                        entries,
                        state: ListState::default(),
                        pending_delete: None,
                    },
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp/proj",
                },
            )
        })
        .unwrap();
        let c = text_of(&term);
        assert!(c.contains("密钥设置"));
    }

    #[test]
    fn secrets_view_renders_configured_and_unconfigured_rows() {
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        let text_of = |term: &Terminal<TestBackend>| -> String {
            buffer_text(term.backend().buffer())
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        };
        let mut state = ListState::default();
        state.select(Some(0));
        let entries = vec![
            with_secret(entry("kimi", ProfileStatus::Ready)),
            with_secret(entry("glm", ProfileStatus::NeedsSecret)),
        ];
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Secrets {
                        entries,
                        state,
                        pending_delete: None,
                    },
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp/proj",
                },
            )
        })
        .unwrap();
        let c = text_of(&term);
        assert!(c.contains("已配"), "配过的那行要显示已配：{c}");
        assert!(c.contains("未配"), "没配的那行要显示未配：{c}");
    }

    // ———— Finding 1（Task 13 code review）：删密钥的二次确认 ————
    //
    // `d` 在密钥页是真删除，物理键跟看板上「看 diff」那个无害的 `d` 完全
    // 一样，肌肉记忆会带过来。下面这组测试覆盖两段式确认的骨架：武装、
    // 确认、以及每一条取消路径——尤其是挪动光标必须让武装状态和选中行
    // 保持同步，不能分叉。

    #[test]
    fn secrets_view_renders_the_armed_delete_prompt_on_its_row() {
        // 武装之后这一行不该再显示「已配」，而要显示明确的「再按 d 删除」
        // 警告——这是 finding 里点名要求的「inline prompt on that row」。
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        let text_of = |term: &Terminal<TestBackend>| -> String {
            buffer_text(term.backend().buffer())
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        };
        let mut state = ListState::default();
        state.select(Some(0));
        let entries = vec![with_secret(entry("kimi", ProfileStatus::Ready))];
        term.draw(|f| {
            draw(
                f,
                &mut DrawInput {
                    view: &View::Secrets {
                        entries,
                        state,
                        pending_delete: Some("kimi".to_string()),
                    },
                    sessions: &[],
                    st: &mut st,
                    screen: &[],
                    cursor: (0, 0),
                    message: &Msg::from(""),
                    connected: true,
                    current: "/tmp/proj",
                },
            )
        })
        .unwrap();
        let c = text_of(&term);
        assert!(
            c.contains("再按") && c.contains('d') && c.contains("删除"),
            "武装状态要在行内画出明确提示：{c}"
        );
        assert!(
            !c.contains("已配"),
            "武装的这一行不该继续显示「已配」，会跟警告混在一起：{c}"
        );
    }
}
