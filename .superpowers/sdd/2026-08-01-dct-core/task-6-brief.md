### Task 6: TUI 看板

**Files:**
- Modify: `src/ui.rs`

**Interfaces:**
- Consumes: `client::Client`、`proto::{Request, Response}`、`session::SessionState`
- Produces: `ui::run(client: Client, default_dir: PathBuf) -> Result<()>`；`ui::status_label(SessionState) -> &'static str`；`ui::status_color(SessionState) -> ratatui::style::Color`

**按键：** `n` 新建会话（在默认目录，profile 选择用弹窗列表）、`↑/↓` 选会话、`Enter` 进入/退出该会话的屏幕视图、`u` 回滚、`s` 停止、`d` 看 diff、`q` 退出。进入屏幕视图后打字直接送进 agent，`Esc` 回看板。

- [ ] **Step 1: 写失败的测试**

`src/ui.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;

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
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test ui`
Expected: 编译失败，`status_label` 未定义。

- [ ] **Step 3: 实现看板**

`src/ui.rs`：

```rust
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use std::path::PathBuf;
use std::time::Duration;

use crate::client::Client;
use crate::proto::{Request, Response};
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

#[derive(Clone)]
enum View {
    Board,
    Attached(u32),
    PickProfile(Vec<String>),
}

pub fn run(mut client: Client, default_dir: PathBuf) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut view = View::Board;
    let mut list_state = ListState::default();
    let mut sessions: Vec<SessionInfo> = Vec::new();
    let mut message = String::new();
    let mut screen = String::new();

    let res = loop {
        if let Ok(Response::Sessions(v)) = client.call(Request::List) {
            sessions = v;
        }
        if list_state.selected().is_none() && !sessions.is_empty() {
            list_state.select(Some(0));
        }
        if let View::Attached(id) = &view {
            let id = *id;
            if let Ok(Response::Screen(s)) = client.call(Request::Screen { id }) {
                screen = s;
            }
        }

        term.draw(|f| draw(f, &view, &sessions, &mut list_state, &screen, &message))?;

        if !event::poll(Duration::from_millis(150))? {
            continue;
        }
        let Event::Key(key) = event::read()? else { continue };
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
                    }
                }
                KeyCode::Char('u') => {
                    message = act(&mut client, &sessions, &list_state, |id| Request::Undo { id });
                }
                KeyCode::Char('s') => {
                    message = act(&mut client, &sessions, &list_state, |id| Request::Stop { id });
                }
                KeyCode::Char('d') => {
                    if let Some(s) = selected(&sessions, &list_state) {
                        message = match client.call(Request::Diff { id: s.id }) {
                            Ok(Response::Diff(v)) if v.is_empty() => "没有改动".into(),
                            Ok(Response::Diff(v)) => v
                                .iter()
                                .map(|f| format!("{} +{} -{}", f.path, f.added, f.removed))
                                .collect::<Vec<_>>()
                                .join("  "),
                            Ok(Response::Error(e)) => e,
                            _ => "请求失败".into(),
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
                            dir: default_dir.display().to_string(),
                            profile,
                        }) {
                            Ok(Response::Created { id }) => format!("已开会话 {id}"),
                            Ok(Response::Error(e)) => e,
                            _ => "创建失败".into(),
                        };
                        view = View::Board;
                    }
                }
                _ => {}
            },
            View::Attached(id) => {
                match key.code {
                    KeyCode::Esc => view = View::Board,
                    KeyCode::Enter => {
                        let _ = client.call(Request::Input { id, text: String::new() });
                    }
                    KeyCode::Char(c) => {
                        let _ = client.call(Request::Input { id, text: c.to_string() });
                    }
                    _ => {}
                }
            }
        }
    };

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    res
}

fn selected<'a>(sessions: &'a [SessionInfo], st: &ListState) -> Option<&'a SessionInfo> {
    st.selected().and_then(|i| sessions.get(i))
}

fn move_sel(st: &mut ListState, sessions: &[SessionInfo], delta: i32) {
    if sessions.is_empty() {
        return;
    }
    let cur = st.selected().unwrap_or(0) as i32;
    let next = (cur + delta).clamp(0, sessions.len() as i32 - 1);
    st.select(Some(next as usize));
}

fn act(
    client: &mut Client,
    sessions: &[SessionInfo],
    st: &ListState,
    make: impl Fn(u32) -> Request,
) -> String {
    match selected(sessions, st) {
        None => "没有选中会话".into(),
        Some(s) => match client.call(make(s.id)) {
            Ok(Response::Ok) => "完成".into(),
            Ok(Response::Error(e)) => e,
            _ => "请求失败".into(),
        },
    }
}

fn draw(
    f: &mut Frame,
    view: &View,
    sessions: &[SessionInfo],
    st: &mut ListState,
    screen: &str,
    message: &str,
) {
    let chunks = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(f.area());

    match view {
        View::Attached(id) => {
            let title = format!("会话 {id} —— Esc 返回看板");
            f.render_widget(
                Paragraph::new(screen).block(Block::default().borders(Borders::ALL).title(title)),
                chunks[0],
            );
        }
        View::PickProfile(profiles) => {
            let text: Vec<Line> = profiles
                .iter()
                .enumerate()
                .map(|(i, p)| Line::from(format!("{}. {}", i + 1, p)))
                .collect();
            f.render_widget(
                Paragraph::new(text)
                    .block(Block::default().borders(Borders::ALL).title("选 agent（按数字，Esc 取消）")),
                chunks[0],
            );
        }
        View::Board => {
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
                        Span::raw(s.dir.clone()),
                    ]))
                })
                .collect();
            f.render_stateful_widget(
                List::new(items)
                    .block(Block::default().borders(Borders::ALL).title("dct 会话看板"))
                    .highlight_symbol("▶ "),
                chunks[0],
                st,
            );
        }
    }

    let help = if message.is_empty() {
        "n 新建  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动  q 退出".to_string()
    } else {
        message.to_string()
    };
    f.render_widget(
        Paragraph::new(help).block(Block::default().borders(Borders::ALL)),
        chunks[1],
    );
}
```

注意 `View` 必须 `#[derive(Clone)]` 并且 `match view.clone()`。如果写成 `match &view`，在分支里给 `view` 赋新值会被借用检查器拒掉（E0506）——`View::PickProfile(profiles)` 这个分支既用了绑定又要改 `view`。`Vec<String>` 只有两三个元素，克隆的代价可以忽略。

`Request::Input` 按字符发送，Task 4 的 `send_input` 已经处理好了：只有空字符串（回车）才打检查点，逐字符输入不会产生提交。这里不需要改 `session.rs`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 全部 PASS（含改过的 session 测试）。

- [ ] **Step 5: 提交**

```bash
git add src/
git commit -m "feat: TUI 看板与会话屏幕视图"
```

---

