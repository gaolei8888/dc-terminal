//! 看板上的「我的电脑」：底部那一段电脑名单、顶部那一行加入确认、会话行
//! 末尾的「✉ N」。
//!
//! **绘制路径上不发任何请求。** `MeshStatus` 在守护进程那边要问一趟中转
//! （谁在线），`MeshApprove` 要把新名单发给组里每台电脑——两条都可能卡
//! 好几秒，所以都丢给后台线程，主循环每轮 `poll` 一次收结果（同
//! `pair_start_rx` 的做法）。画的时候只读 `MeshPanel` 里现成的那一份。
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;

use super::app::App;
use super::view::View;
use super::widgets::{display_width, truncate, Msg};
use super::{dim, theme_now};
use crate::i18n::{msg, Lang};
use crate::proto::{MemberView, MeshView, PendingJoin, Request, Response};

/// 多久问一次 `MeshStatus`。
pub(crate) const POLL_EVERY: Duration = Duration::from_secs(5);
/// 批准要把新名单发给组里每台电脑，比本机答一句慢得多（同 `dct peers
/// approve` 用的慢超时）。
const ANSWER_TIMEOUT: Duration = Duration::from_secs(40);
/// 「我的电脑」最多列几台，多的只报个数。
pub(crate) const MAX_SHOWN: usize = 5;

type Reply = Result<Response, String>;

/// 看板上多电脑那一块的全部状态。
#[derive(Default)]
pub(crate) struct MeshPanel {
    /// 最近一次拿到的现状。`None` = 还没问到过（守护进程太老、刚启动），
    /// 这时候那一段整个不画，不猜。
    pub view: Option<MeshView>,
    status_rx: Option<Receiver<Reply>>,
    last_fetch: Option<Instant>,
    answer_rx: Option<Receiver<Reply>>,
    /// 正在回答的那一条（端点、数字、同意与否）。回答飞着的时候确认行不画、
    /// y/n 不再接——免得连按两下，第二下落到下一条请求上。
    answering: Option<(PendingJoin, bool)>,
}

impl MeshPanel {
    /// 此刻确认行上问的是哪一条：最早来的那一条（`pending` 按到达顺序排）。
    pub(crate) fn asking(&self) -> Option<&PendingJoin> {
        if self.answering.is_some() {
            return None;
        }
        self.view.as_ref().and_then(|v| v.pending.first())
    }

    /// 这个会话这次运行期间收到过几条留言。
    pub(crate) fn messages_for(&self, session: u32) -> u32 {
        self.view
            .as_ref()
            .and_then(|v| v.messages.get(&session).copied())
            .unwrap_or(0)
    }
}

/// 在后台线程里发一条请求，结果从返回的通道里取。
fn spawn_call(socket: PathBuf, req: Request, timeout: Duration) -> Receiver<Reply> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = crate::client::Client::connect(&socket)
            .and_then(|mut c| c.call_within(req, timeout))
            .map_err(|e| e.to_string());
        let _ = tx.send(r);
    });
    rx
}

/// 主循环每轮调一次：收后台的结果；到点了、又没有一条在飞，就再问一次。
/// 只在看板上问——别的视图不画这一块。
pub(crate) fn poll(app: &mut App, now: Instant) {
    if let Some(rx) = &app.mesh.answer_rx {
        if let Ok(r) = rx.try_recv() {
            app.mesh.answer_rx = None;
            let answered = app.mesh.answering.take();
            // 回答之前发出去的那次状态查询，回来的是批准之前的样子，扔掉。
            app.mesh.status_rx = None;
            match (r, answered) {
                (Ok(Response::Mesh(v)), Some((asked, yes))) => {
                    app.message = if yes {
                        msg::mesh_member_joined(app.lang, &clean(&asked.name))
                    } else {
                        msg::mesh_member_refused(app.lang, &clean(&asked.name))
                    }
                    .into();
                    app.mesh.view = Some(v);
                    app.mesh.last_fetch = Some(now);
                }
                (Ok(Response::Error(e)), _) => {
                    app.message = Msg::err(msg::error(app.lang, &e));
                    app.mesh.last_fetch = None;
                }
                _ => {
                    app.message = Msg::err(msg::error(
                        app.lang,
                        &crate::proto::ErrorCode::DaemonNotResponding,
                    ));
                    app.mesh.last_fetch = None;
                }
            }
        }
    }
    if let Some(rx) = &app.mesh.status_rx {
        match rx.try_recv() {
            Ok(r) => {
                app.mesh.status_rx = None;
                // 问不到（断线、守护进程太老不认识这条）就留着手里那份。
                if let Ok(Response::Mesh(v)) = r {
                    app.mesh.view = Some(v);
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => app.mesh.status_rx = None,
        }
    }
    let due = app
        .mesh
        .last_fetch
        .is_none_or(|t| now.saturating_duration_since(t) >= POLL_EVERY);
    if due
        && app.connected
        && matches!(app.view, View::Board)
        && app.mesh.status_rx.is_none()
        && app.mesh.answering.is_none()
    {
        app.mesh.last_fetch = Some(now);
        app.mesh.status_rx = Some(spawn_call(
            app.socket.clone(),
            Request::MeshStatus,
            crate::client::READ_TIMEOUT * 2,
        ));
    }
}

/// 看板上按了 y / n：确认行在的时候由它接管（返回 `true`）。**只从
/// `board::handle_key` 调**——会话视图里 y/n 归 agent。
pub(crate) fn handle_key(app: &mut App, key: &KeyEvent) -> bool {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::META)
    {
        return false;
    }
    let yes = match key.code {
        KeyCode::Char('y') => true,
        KeyCode::Char('n') => false,
        _ => return false,
    };
    // 回答还在飞：吞掉，别让第二下 n 变成「新建会话」、也别落到下一条上。
    if app.mesh.answering.is_some() {
        return true;
    }
    let Some(p) = app.mesh.asking().cloned() else {
        return false;
    };
    // 发的就是屏幕上这一条的端点和数字，守护进程两样都对上才批。
    app.mesh.answer_rx = Some(spawn_call(
        app.socket.clone(),
        Request::MeshApprove {
            endpoint: p.endpoint.clone(),
            code: p.code.clone(),
            yes,
        },
        ANSWER_TIMEOUT,
    ));
    app.mesh.answering = Some((p, yes));
    true
}

/// 别的电脑给的名字：洗掉控制字符和转义序列，才能往终端上画。
fn clean(s: &str) -> String {
    crate::session::sanitize(s)
}

/// 按显示宽度折行（中文占两列）。`width` 为 0 时什么都不出。
fn wrap(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut line = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = super::widgets::char_width(ch);
        if w + cw > width && !line.is_empty() {
            out.push(std::mem::take(&mut line));
            w = 0;
        }
        line.push(ch);
        w += cw;
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// 顶部那几行：黄字的加入确认（整句折行，不截——数字和 (y/n) 一个都不能
/// 被切掉），后面跟「还有 N 条」；下面一行灰字提醒一次只加一台（放不下
/// 就截短，它只是提醒）。
pub(crate) fn prompt_lines(panel: &MeshPanel, lang: Lang, width: usize) -> Vec<Line<'static>> {
    let Some(p) = panel.asking() else {
        return Vec::new();
    };
    let mut ask = msg::mesh_join_prompt(lang, &clean(&p.name), &p.code);
    let more = panel.view.as_ref().map_or(0, |v| v.pending.len()) - 1;
    if more > 0 {
        ask = format!("{ask}  {}", msg::mesh_more_requests(lang, more));
    }
    let style = theme_now().asking().add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line> = wrap(&ask, width)
        .into_iter()
        .map(|l| Line::from(Span::styled(l, style)))
        .collect();
    if width > 1 {
        let hint = msg::mesh_cross_join_hint(lang);
        lines.push(Line::from(Span::styled(
            truncate(&hint, width.saturating_sub(1)),
            dim(),
        )));
    }
    lines
}

/// 名单怎么排：本机最前，然后在线的，然后离线的；同一档里照名单原来的顺序。
fn ordered(members: &[MemberView]) -> Vec<&MemberView> {
    let mut v: Vec<&MemberView> = members.iter().collect();
    v.sort_by_key(|m| (!m.is_me, !m.online));
    v
}

/// 底部「我的电脑」那一段。还没问到过就是空的。
pub(crate) fn section_lines(panel: &MeshPanel, lang: Lang, width: usize) -> Vec<Line<'static>> {
    let Some(v) = &panel.view else {
        return Vec::new();
    };
    if width == 0 {
        return Vec::new();
    }
    if !v.logged_in {
        return vec![Line::from(Span::styled(
            truncate(&msg::mesh_board_off(lang), width.saturating_sub(1)),
            dim(),
        ))];
    }
    let mut lines = vec![Line::from(Span::styled(
        truncate(&msg::mesh_board_header(lang), width.saturating_sub(1)),
        dim(),
    ))];
    let members = ordered(&v.members);
    for m in members.iter().take(MAX_SHOWN) {
        let state = msg::mesh_board_state(lang, m.online, m.is_me);
        let (dot, dot_style) = if m.online {
            ("●", theme_now().idle())
        } else {
            ("○", dim())
        };
        // 名字能用多宽：整行减去圆点、两处空白和后面那两个字。
        let room = width.saturating_sub(2 + 2 + display_width(&state) + 1);
        lines.push(Line::from(vec![
            Span::styled(format!("{dot} "), dot_style),
            Span::raw(truncate(&clean(&m.name), room.saturating_sub(1))),
            Span::raw("  "),
            Span::styled(state, dim()),
        ]));
    }
    if members.len() > MAX_SHOWN {
        lines.push(Line::from(Span::styled(
            truncate(
                &msg::mesh_board_more(lang, members.len() - MAX_SHOWN),
                width.saturating_sub(1),
            ),
            dim(),
        )));
    }
    lines
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::proto::MeshView;
    use crossterm::event::KeyEvent;
    use std::sync::{Arc, Mutex};

    /// 一份现状：`members` 是（名字, 在线, 本机），`pending` 是（名字, 端点, 数字）。
    pub(crate) fn mesh_view(
        logged_in: bool,
        members: &[(&str, bool, bool)],
        pending: &[(&str, &str, &str)],
    ) -> MeshView {
        MeshView {
            logged_in,
            name: "家里Mac".into(),
            endpoint: "c-home".into(),
            in_group: logged_in,
            members: members
                .iter()
                .enumerate()
                .map(|(i, (n, online, me))| MemberView {
                    name: n.to_string(),
                    endpoint: format!("c-{i}"),
                    online: *online,
                    is_me: *me,
                })
                .collect(),
            pending: pending
                .iter()
                .map(|(n, e, c)| PendingJoin {
                    name: n.to_string(),
                    endpoint: e.to_string(),
                    code: c.to_string(),
                })
                .collect(),
            joining: Vec::new(),
            messages: Default::default(),
        }
    }

    /// 假守护进程：每条连接一个线程（会话视图那条长连接不能把别的挡住），
    /// 记下收到的每一条请求。`MeshStatus` 回 `status`；`MeshApprove` 回一份
    /// 把那一条去掉之后的现状。
    fn fake_daemon(sock: &std::path::Path, status: MeshView) -> Arc<Mutex<Vec<Request>>> {
        use std::io::{BufRead, BufReader, Write};
        let listener = crate::sys::ipc::bind_private(sock).unwrap();
        let got: Arc<Mutex<Vec<Request>>> = Default::default();
        let got2 = got.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (got, status) = (got2.clone(), status.clone());
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut writer = stream;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        let Ok(req) = serde_json::from_str::<Request>(&line) else {
                            break;
                        };
                        let answer = match &req {
                            Request::MeshStatus => Response::Mesh(status.clone()),
                            Request::MeshApprove { endpoint, .. } => {
                                let mut v = status.clone();
                                v.pending.retain(|p| &p.endpoint != endpoint);
                                Response::Mesh(v)
                            }
                            _ => Response::Ok,
                        };
                        got.lock().unwrap().push(req);
                        let s = serde_json::to_string(&answer).unwrap();
                        if writeln!(writer, "{s}").is_err() {
                            break;
                        }
                    }
                });
            }
        });
        got
    }

    fn approvals(got: &Arc<Mutex<Vec<Request>>>) -> Vec<(String, String, bool)> {
        got.lock()
            .unwrap()
            .iter()
            .filter_map(|r| match r {
                Request::MeshApprove {
                    endpoint,
                    code,
                    yes,
                } => Some((endpoint.clone(), code.clone(), *yes)),
                _ => None,
            })
            .collect()
    }

    fn statuses(got: &Arc<Mutex<Vec<Request>>>) -> usize {
        got.lock()
            .unwrap()
            .iter()
            .filter(|r| matches!(r, Request::MeshStatus))
            .count()
    }

    /// 等到 `cond` 成立，最多 5 秒；每轮跑一次 `poll`（主循环就是这么收的）。
    fn wait_until(app: &mut App, mut cond: impl FnMut(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond(app) {
            assert!(Instant::now() < deadline, "等了 5 秒没等到");
            poll(app, Instant::now());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// 两台在等：最早来的那台在前。
    fn two_waiting() -> MeshView {
        mesh_view(
            true,
            &[("家里Mac", true, true)],
            &[
                ("新电脑", "c-first", "123456"),
                ("新电脑", "c-second", "654321"),
            ],
        )
    }

    fn board_app(status: MeshView) -> (App, tempfile::TempDir, Arc<Mutex<Vec<Request>>>) {
        let (mut app, dir) = App::test_app();
        let got = fake_daemon(&app.socket, status.clone());
        app.connected = true;
        app.view = View::Board;
        app.mesh.view = Some(status);
        (app, dir, got)
    }

    /// 看板上按 y：发的是屏幕上那一条（最早来的）的端点**和**数字，同意。
    /// 回来之后确认行换成下一条，底栏说一句「已加入」。
    #[test]
    fn y_on_the_board_approves_exactly_the_request_shown() {
        let (mut app, _d, got) = board_app(two_waiting());
        crate::ui::dispatch_key(&mut app, key('y')).unwrap();
        assert!(app.mesh.asking().is_none(), "回答飞着的时候确认行收起来");
        wait_until(&mut app, |a| a.mesh.answering.is_none());
        assert_eq!(
            approvals(&got),
            [("c-first".to_string(), "123456".to_string(), true)]
        );
        assert_eq!(app.message.text, "新电脑 已加入");
        assert_eq!(
            app.mesh.asking().map(|p| p.endpoint.as_str()),
            Some("c-second")
        );
    }

    /// 看板上按 n：拒绝的也是屏幕上那一条，**不批准**。
    #[test]
    fn n_on_the_board_refuses_the_request_shown_and_never_approves() {
        let (mut app, _d, got) = board_app(two_waiting());
        crate::ui::dispatch_key(&mut app, key('n')).unwrap();
        wait_until(&mut app, |a| a.mesh.answering.is_none());
        assert_eq!(
            approvals(&got),
            [("c-first".to_string(), "123456".to_string(), false)]
        );
        assert!(
            matches!(app.view, View::Board),
            "确认行在的时候 n 不是新建会话"
        );
        assert_eq!(app.message.text, "已拒绝 新电脑");
    }

    /// 回答还在飞的时候再按：吞掉，不发第二条（第二下不能落到下一条上）。
    #[test]
    fn a_second_press_while_answering_sends_nothing_more() {
        let (mut app, dir, got) = board_app(two_waiting());
        // 看板上有一个项目：漏过去的 n 会真的打开「新建会话」。
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        app.set_sessions(vec![crate::session::SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: proj.display().to_string(),
            state: crate::session::SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }]);
        app.list_state.select(Some(1));
        crate::ui::dispatch_key(&mut app, key('y')).unwrap();
        assert!(
            handle_key(&mut app, &key('n')),
            "回答飞着时 n 也归它，不落到「新建会话」"
        );
        crate::ui::dispatch_key(&mut app, key('y')).unwrap();
        crate::ui::dispatch_key(&mut app, key('n')).unwrap();
        wait_until(&mut app, |a| a.mesh.answering.is_none());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(approvals(&got).len(), 1);
        assert!(matches!(app.view, View::Board));
    }

    /// **会话视图里 y 归 agent**（「会话视图只吃 F 功能键」）：确认行挂着
    /// 也不发 `MeshApprove`。
    #[test]
    fn y_in_the_session_view_sends_no_approval() {
        let (mut app, _d, got) = board_app(two_waiting());
        app.set_sessions(vec![crate::session::SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/tmp/a".into(),
            state: crate::session::SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }]);
        app.client = Some(crate::client::Client::connect(&app.socket).unwrap());
        app.view = View::Attached(1);
        crate::ui::dispatch_key(&mut app, key('y')).unwrap();
        crate::ui::dispatch_key(&mut app, key('n')).unwrap();
        assert!(app.mesh.answering.is_none());
        std::thread::sleep(Duration::from_millis(200));
        poll(&mut app, Instant::now());
        assert!(approvals(&got).is_empty(), "{:?}", approvals(&got));
        assert!(
            got.lock()
                .unwrap()
                .iter()
                .any(|r| matches!(r, Request::Input { .. })),
            "y 该送进会话里"
        );
    }

    /// 没有等着的请求时 y/n 不归它：n 照旧是新建会话那一路。带 Ctrl 的也不归它。
    #[test]
    fn without_a_request_or_with_ctrl_the_keys_are_not_taken() {
        let (mut app, _d, got) = board_app(mesh_view(true, &[("家里Mac", true, true)], &[]));
        assert!(!handle_key(&mut app, &key('n')));
        assert!(!handle_key(&mut app, &key('y')));
        app.mesh.view = Some(two_waiting());
        assert!(!handle_key(
            &mut app,
            &KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL)
        ));
        assert!(approvals(&got).is_empty());
    }

    /// 每 5 秒问一次，而且只在看板上问；一次在飞的时候不再发第二次。
    #[test]
    fn status_is_asked_every_five_seconds_and_only_on_the_board() {
        let status = mesh_view(true, &[("家里Mac", true, true)], &[]);
        let (mut app, _d, got) = board_app(status.clone());
        app.mesh.view = None;
        let t0 = Instant::now();
        poll(&mut app, t0);
        poll(&mut app, t0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.mesh.view.is_none() {
            assert!(Instant::now() < deadline);
            poll(&mut app, t0 + Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(app.mesh.view, Some(status));
        assert_eq!(statuses(&got), 1);
        poll(&mut app, t0 + POLL_EVERY - Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(statuses(&got), 1, "不到 5 秒不问");

        app.view = View::Attached(1);
        poll(&mut app, t0 + POLL_EVERY * 3);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(statuses(&got), 1, "会话视图里不问");

        app.view = View::Board;
        poll(&mut app, t0 + POLL_EVERY * 3);
        let deadline = Instant::now() + Duration::from_secs(5);
        while statuses(&got) < 2 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 上一次还没回来（中转慢）：到点了也不再发第二次，不越堆越多。
    #[test]
    fn no_second_status_request_while_one_is_in_flight() {
        let (mut app, _d, got) = board_app(mesh_view(true, &[], &[]));
        let (_tx, rx) = mpsc::channel::<Reply>();
        app.mesh.status_rx = Some(rx);
        poll(&mut app, Instant::now() + POLL_EVERY * 2);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(statuses(&got), 0);
    }

    #[test]
    fn wrapping_counts_wide_characters_as_two_columns() {
        assert_eq!(wrap("一二三abc", 4), ["一二", "三ab", "c"]);
        assert_eq!(wrap("abc", 0), Vec::<String>::new());
        assert_eq!(wrap("一", 1), ["一"], "一个字都放不下也不能死循环");
    }
}
