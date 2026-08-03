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
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::client::Client;
use crate::proto::{Request, Response};
use crate::pty::{ScreenColor, ScreenSpan, ScreenStyle};
use crate::session::{SessionInfo, SessionState};

pub fn status_label(s: SessionState) -> &'static str {
    match s {
        SessionState::Working => "干活中",
        SessionState::Asking => "等你回答",
        SessionState::Idle => "空闲",
        SessionState::Stopped => "已停止",
    }
}

pub fn status_color(s: SessionState) -> Color {
    match s {
        SessionState::Working => Color::Cyan,
        SessionState::Asking => Color::Yellow,
        SessionState::Idle => Color::Green,
        SessionState::Stopped => Color::DarkGray,
    }
}

/// 底部状态栏要显示的一句话。`error` 决定它是灰字还是红字——
/// 出错和成功用同一种颜色，用户分不出刚才那步到底成没成。
pub struct Msg {
    pub text: String,
    pub error: bool,
}

impl Msg {
    pub fn err(text: String) -> Msg {
        Msg { text, error: true }
    }
}

impl From<&str> for Msg {
    fn from(s: &str) -> Msg {
        Msg {
            text: s.to_string(),
            error: false,
        }
    }
}

impl From<String> for Msg {
    fn from(text: String) -> Msg {
        Msg { text, error: false }
    }
}

#[derive(Clone)]
enum View {
    Board,
    Attached(u32),
    PickProfile(Vec<String>),
}

/// 兜底恢复终端状态。ratatui 的 `Terminal` 不会在 `Drop` 里自动退出 raw
/// mode / alternate screen；`run()` 的主循环里到处都是 `?`，一旦某次
/// `client.call`/`term.draw` 出错就会直接从函数返回，跳过写在循环末尾的清理代码，
/// 把用户的终端卡在 raw mode（回显、行缓冲全关）。这个 guard 保证不管是提前
/// `return`/`?`、正常 `break`，还是 panic 展开，`Drop` 都会跑一次——`Drop` 里不能
/// panic，所以两步清理都用 `let _ =` 吞掉错误。
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            std::io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
    }
}

pub fn run(mut client: Client, default_dir: PathBuf) -> Result<()> {
    // start_dir 是 dct 启动时的目录，只用来解析用户敲进来的相对路径，永不改变。
    // current_dir 是「新会话开在哪」，Task 5 的选择器会改它。
    let start_dir = default_dir.clone();
    let mut current_dir = default_dir;

    enable_raw_mode()?;
    // 必须在 EnterAlternateScreen / Terminal::new 之前构造：这样即便它们俩失败，
    // raw mode 也还是能被 Drop 恢复。
    let _guard = TerminalGuard;
    let mut stdout = std::io::stdout();
    // 开括号粘贴：不开的话粘贴的文字会一个字符一个事件地进来，
    // 粘一段话就是几百次往返，慢到没法用。
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut view = View::Board;
    let mut list_state = ListState::default();
    let mut sessions: Vec<SessionInfo> = Vec::new();
    let mut message: Msg = "".into();
    let mut screen: Vec<Vec<ScreenSpan>> = Vec::new();
    let mut screen_cursor = (0u16, 0u16);
    // 上次告诉 agent 的画面尺寸，变了才发 Resize，避免每帧一次多余请求
    let mut sent_size: Option<(u32, u16, u16)> = None;
    // 连不上守护进程 / 请求失败时置 false，看板上要能看出数据是陈旧的，
    // 不能让用户以为界面上的“干活中”还代表当前真实状态。每次循环开头的
    // List（以及 Attached 视图下的 Screen）调用是唯一的真相来源——它总在
    // 当次的 term.draw 之前重新算一遍，所以不需要（也不应该）预置初值。
    let mut connected = true;

    // 进了会话就不用再每轮拉 List：它是给看板用的，而且服务端要逐个锁会话、
    // 取每个会话的最后一行，纯属浪费。只在看板上、或刚从会话里退出来时拉一次。
    let mut need_sessions = true;

    let res = loop {
        let attached = matches!(view, View::Attached(_));
        if need_sessions || !attached {
            match client.call(Request::List) {
                Ok(Response::Sessions(v)) => {
                    sessions = v;
                    connected = true;
                }
                _ => connected = false,
            }
            need_sessions = false;
        }
        if list_state.selected().is_none() && !sessions.is_empty() {
            list_state.select(Some(0));
        }
        if let View::Attached(id) = &view {
            let id = *id;
            // 把 agent 画面区的真实大小告诉它。不做的话它永远按初始宽度排版，
            // 窗口再宽也只用左边一块。减 2 是边框。
            let area = term.size()?;
            let rows = area.height.saturating_sub(2 + 3);
            let cols = area.width.saturating_sub(2);
            if sent_size != Some((id, rows, cols)) && rows > 0 && cols > 0 {
                if client.call(Request::Resize { id, rows, cols }).is_ok() {
                    sent_size = Some((id, rows, cols));
                }
            }
            match client.call(Request::Screen { id }) {
                Ok(Response::Screen { lines, cursor }) => {
                    screen = lines;
                    screen_cursor = cursor;
                    connected = true;
                }
                _ => connected = false,
            }
        }

        term.draw(|f| {
            draw(
                f,
                &view,
                &sessions,
                &mut list_state,
                &screen,
                screen_cursor,
                &message,
                connected,
                &current_dir.display().to_string(),
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
            if let View::Attached(id) = view {
                if !text.is_empty() && client.call(Request::Input { id, text }).is_err() {
                    message = Msg::err("守护进程连不上，粘贴的内容没发出去".into());
                }
            }
            continue;
        }
        let Event::Key(key) = ev else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // 必须 clone：分支里要给 view 赋值，match &view 会被借用检查器拒掉
        match view.clone() {
            View::Board => match key.code {
                KeyCode::Char('q') => break Ok(()),
                KeyCode::Down => move_sel(&mut list_state, &sessions, 1),
                KeyCode::Up => move_sel(&mut list_state, &sessions, -1),
                KeyCode::Char('n') => {
                    if let Ok(Response::Profiles(p)) = client.call(Request::Profiles) {
                        view = View::PickProfile(p);
                    }
                }
                KeyCode::Enter => {
                    if let Some(s) = selected(&sessions, &list_state) {
                        view = View::Attached(s.id);
                        need_sessions = true; // 会话标题要显示项目名
                    }
                }
                KeyCode::Char('u') => {
                    message = act(&mut client, &sessions, &list_state, |id| Request::Undo {
                        id,
                    });
                }
                KeyCode::Char('s') => {
                    message = act(&mut client, &sessions, &list_state, |id| Request::Stop {
                        id,
                    });
                }
                KeyCode::Char('d') => {
                    if let Some(s) = selected(&sessions, &list_state) {
                        message = match client.call(Request::Diff { id: s.id }) {
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
            },
            View::PickProfile(profiles) => match key.code {
                KeyCode::Esc => view = View::Board,
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    let idx = c.to_digit(10).unwrap() as usize;
                    if idx >= 1 && idx <= profiles.len() {
                        let profile = profiles[idx - 1].clone();
                        message = match client.call(Request::Create {
                            dir: current_dir.display().to_string(),
                            profile,
                        }) {
                            Ok(Response::Created { id }) => format!("已开会话 {id}").into(),
                            Ok(Response::Error(e)) => Msg::err(e),
                            _ => Msg::err("创建失败".into()),
                        };
                        view = View::Board;
                    }
                }
                _ => {}
            },
            View::Attached(id) => {
                // F2 是唯一被 dct 吃掉的键，其余一律 key_to_input 翻译成终端字节
                // 送进去——方向键、退格、Tab、Ctrl 组合都要能用，否则在 Claude Code
                // 里连打错字都退不了格。Esc 必须还给 agent——Claude Code 靠它
                // 取消/清空/关弹窗（底部那句 "Esc to cancel"）；Ctrl+B 也必须还回去，
                // 那是 Claude Code 的「转后台」。逆转键挑 F2 是因为没有 CLI agent
                // 在用它，不必搞双击透传那种隐形状态。
                if key.code == KeyCode::F(2) {
                    view = View::Board;
                    need_sessions = true;
                } else if let Some(text) = key_to_input(&key) {
                    // 发送失败时不能静默吞掉——用户打字没反应会分不清是卡顿还是断连。
                    // “连不上”这个视觉状态统一交给循环顶部的 List/Screen 探测去判定。
                    if client.call(Request::Input { id, text }).is_err() {
                        message = Msg::err("守护进程连不上，刚才那次输入没发出去".into());
                    }
                }
            }
        }
    };

    res
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

fn to_color(c: ScreenColor) -> Option<Color> {
    match c {
        ScreenColor::Default => None,
        ScreenColor::Idx(i) => Some(Color::Indexed(i)),
        ScreenColor::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

fn to_style(s: &ScreenStyle) -> Style {
    let mut st = Style::default();
    if let Some(c) = to_color(s.fg) {
        st = st.fg(c);
    }
    if let Some(c) = to_color(s.bg) {
        st = st.bg(c);
    }
    let mut m = Modifier::empty();
    if s.bold {
        m |= Modifier::BOLD;
    }
    if s.italic {
        m |= Modifier::ITALIC;
    }
    if s.underline {
        m |= Modifier::UNDERLINED;
    }
    if s.inverse {
        m |= Modifier::REVERSED;
    }
    st.add_modifier(m)
}

/// agent 屏幕的样式化内容转成 ratatui 的行。丢掉样式的话 Claude Code
/// 那种靠颜色区分的输出会退化成一片单色，基本没法看。
fn screen_to_lines(screen: &[Vec<ScreenSpan>]) -> Vec<Line<'static>> {
    screen
        .iter()
        .map(|row| {
            Line::from(
                row.iter()
                    .map(|sp| Span::styled(sp.text.clone(), to_style(&sp.style)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// 按显示宽度截断，超出的用 … 收尾。看板一行放不下就裁，不能让它换行把表格冲乱。
fn truncate(s: &str, max: usize) -> String {
    let mut w = 0;
    let mut out = String::new();
    for ch in s.chars() {
        let cw = if (ch as u32) > 0x1100 { 2 } else { 1 };
        if w + cw > max {
            out.push('…');
            return out;
        }
        w += cw;
        out.push(ch);
    }
    out
}

/// 把 $HOME 缩成 ~，界面上路径太长会被裁掉。
fn short_path(p: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && p.starts_with(&h) => format!("~{}", &p[h.len()..]),
        _ => p.to_string(),
    }
}

/// 把用户敲进来的路径变成绝对路径：`~` 展开成家目录，相对路径按 `base` 解析。
/// 只做字符串层面的展开，**不做存在性校验**——调用方自己决定不存在时怎么办。
fn expand_path(input: &str, base: &Path) -> PathBuf {
    // 粘贴进来的路径经常带尾随空格
    let t = input.trim();
    let home = || PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));

    if t == "~" {
        return home();
    }
    // 只认 `~/`：`~foo` 是别人的家目录（我们不支持），当普通相对路径处理
    if let Some(rest) = t.strip_prefix("~/") {
        return home().join(rest);
    }
    let p = Path::new(t);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// 不区分大小写的子串过滤。匹配**完整路径**而不只是目录名，
/// 这样 `work` 和 `dc-term` 都能用来找同一个项目。
fn filter_projects(all: &[String], filter: &str) -> Vec<String> {
    if filter.is_empty() {
        return all.to_vec();
    }
    let f = filter.to_lowercase();
    all.iter()
        .filter(|p| p.to_lowercase().contains(&f))
        .cloned()
        .collect()
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

fn act(
    client: &mut Client,
    sessions: &[SessionInfo],
    st: &ListState,
    make: impl Fn(u32) -> Request,
) -> Msg {
    match selected(sessions, st) {
        None => "没有选中会话".into(),
        Some(s) => match client.call(make(s.id)) {
            Ok(Response::Ok) => "完成".into(),
            Ok(Response::Error(e)) => Msg::err(e),
            _ => Msg::err("请求失败".into()),
        },
    }
}

fn draw(
    f: &mut Frame,
    view: &View,
    sessions: &[SessionInfo],
    st: &mut ListState,
    screen: &[Vec<ScreenSpan>],
    cursor: (u16, u16),
    message: &Msg,
    connected: bool,
    current: &str,
) {
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
        View::PickProfile(profiles) => {
            let text: Vec<Line> = profiles
                .iter()
                .enumerate()
                .map(|(i, p)| Line::from(format!("{}. {}", i + 1, p)))
                .collect();
            f.render_widget(
                Paragraph::new(text).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style)
                        .title("选 agent（按数字，Esc 取消）"),
                ),
                chunks[0],
            );
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
                            Style::default().fg(Color::DarkGray),
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
    }

    // 提示必须跟着视图走。底部栏原来不分视图，进了会话仍写着看板的按键表，
    // 而那些键在会话视图里全部被转发给 agent——用户照着按 n，字母 n 会落进
    // Claude Code 的输入框。显示做不到的操作比不显示更糟。
    let idle_help = match view {
        View::Attached(_) => "F2 回看板（回看板后按 n 新建会话）　其余按键都发给 agent",
        View::PickProfile(_) => "按数字选 agent，Esc 取消",
        View::Board => "n 新建  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动  q 退出",
    };

    let (help, style) = if !connected {
        (
            "守护进程连不上，界面数据可能已过期".to_string(),
            Style::default().fg(Color::Red),
        )
    } else if message.text.is_empty() {
        (idle_help.to_string(), Style::default())
    } else if message.error {
        (message.text.clone(), Style::default().fg(Color::Red))
    } else {
        (message.text.clone(), Style::default())
    };
    // 当前项目放在边框标题里，框内只留一行字。中文是双宽字符，
    // 「当前项目：~/work/dc/dc-terminal」加上看板按键表在 80 列终端里放不下同一行，
    // 挤在一起会被 Paragraph 直接截断——标题行本来就空着，正好用它。
    f.render_widget(
        Paragraph::new(help).style(style).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("当前项目：{}", short_path(current))),
        ),
        chunks[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
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

    #[test]
    fn status_labels_are_chinese() {
        assert_eq!(status_label(SessionState::Working), "干活中");
        assert_eq!(status_label(SessionState::Asking), "等你回答");
        assert_eq!(status_label(SessionState::Idle), "空闲");
        assert_eq!(status_label(SessionState::Stopped), "已停止");
    }

    #[test]
    fn asking_and_working_use_different_colors() {
        assert_ne!(
            status_color(SessionState::Asking),
            status_color(SessionState::Working)
        );
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
                &View::Board,
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp/proj",
            )
        })
        .unwrap();
        // 看板视图，带提示消息
        term.draw(|f| {
            draw(
                f,
                &View::Board,
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from("完成"),
                true,
                "/tmp/proj",
            )
        })
        .unwrap();
        // 看板为空列表也不能 panic
        term.draw(|f| {
            draw(
                f,
                &View::Board,
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp/proj",
            )
        })
        .unwrap();
        // 断连状态：底部提示和边框都要切到断连样式，也不能 panic
        term.draw(|f| {
            draw(
                f,
                &View::Board,
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                false,
                "/tmp/proj",
            )
        })
        .unwrap();
        // profile 选择弹窗
        let profiles = vec!["claude".to_string(), "shell".to_string()];
        term.draw(|f| {
            draw(
                f,
                &View::PickProfile(profiles.clone()),
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp/proj",
            )
        })
        .unwrap();
        // 已进入会话的屏幕视图
        term.draw(|f| {
            draw(
                f,
                &View::Attached(1),
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp/proj",
            )
        })
        .unwrap();
        // 已进入会话但断连了
        term.draw(|f| {
            draw(
                f,
                &View::Attached(1),
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                false,
                "/tmp/proj",
            )
        })
        .unwrap();
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
                &View::Board,
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from("完成"),
                false,
                "/tmp/proj",
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
    fn expand_path_handles_tilde_and_relative() {
        let base = std::path::Path::new("/base");
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());

        assert_eq!(
            expand_path("/abs/x", base),
            std::path::PathBuf::from("/abs/x")
        );
        assert_eq!(expand_path("~/x", base), home.join("x"));
        assert_eq!(expand_path("~", base), home);
        assert_eq!(
            expand_path("rel/x", base),
            std::path::PathBuf::from("/base/rel/x")
        );
        // 用户粘贴路径常带尾随空格
        assert_eq!(
            expand_path("  /abs/x  ", base),
            std::path::PathBuf::from("/abs/x")
        );
        // `~foo` 不是家目录展开，是个叫 ~foo 的相对路径
        assert_eq!(
            expand_path("~foo", base),
            std::path::PathBuf::from("/base/~foo")
        );
    }

    #[test]
    fn filter_projects_is_case_insensitive_substring() {
        let all = vec![
            "/Users/lei/work/dc/dc-terminal".to_string(),
            "/Users/lei/work/dc/dc_workbench".to_string(),
            "/Users/lei/tmp/scratch".to_string(),
        ];

        assert_eq!(filter_projects(&all, "").len(), 3, "空过滤词返回全部");
        assert_eq!(filter_projects(&all, "WORK").len(), 2, "不区分大小写");
        assert_eq!(
            filter_projects(&all, "dc-term"),
            vec!["/Users/lei/work/dc/dc-terminal".to_string()],
            "匹配的是完整路径的任意位置"
        );
        assert_eq!(filter_projects(&all, "scratch").len(), 1);
        assert!(filter_projects(&all, "没有这个").is_empty());
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

    #[test]
    fn msg_from_str_is_not_an_error() {
        let m: Msg = "完成".into();
        assert!(!m.error);
        assert_eq!(m.text, "完成");
        assert!(Msg::err("炸了".into()).error);
    }

    #[test]
    fn bottom_bar_shows_current_project() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        term.draw(|f| {
            draw(
                f,
                &View::Board,
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/Users/lei/work/dc/dc-terminal",
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
                &View::Board,
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::err("不是一个目录".into()),
                true,
                "/tmp",
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
                &View::Attached(1),
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp/a",
            )
        })
        .unwrap();
        let c = text_of(&term);
        assert!(c.contains("F2回看板"), "会话视图要给出逆转键提示：{c}");
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
                &View::Board,
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp/a",
            )
        })
        .unwrap();
        let c = text_of(&term);
        assert!(c.contains("u回滚"), "看板要显示自己的按键表：{c}");
    }
}
