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
/// 确认行上换了一条（或者刚出现）之后，多久内 y/n 不算数：一次刷新恰好在
/// 按键前把 A 换成了 B，这一下不能落到用户没看过的 B 上；平时当「新建会话」
/// 按的 `n` 也不能拒掉一条刚冒出来、还没看清的请求。
pub(crate) const SETTLE: Duration = Duration::from_millis(500);
/// 「我的电脑」最多列几台，多的只报个数。
pub(crate) const MAX_SHOWN: usize = 5;

type Reply = Result<Response, String>;

/// 看板上多电脑那一块的全部状态。
#[derive(Default)]
pub(crate) struct MeshPanel {
    /// 最近一次拿到的现状。`None` = 还没问到过（守护进程太老、刚启动），
    /// 这时候那一段整个不画，不猜。**换它走 `set_view`**：那里记着确认行
    /// 上那一条是什么时候出现的（`SETTLE`）。
    pub view: Option<MeshView>,
    /// 确认行上此刻那一条（端点、数字），以及它从什么时候开始挂在那儿。
    shown: Option<(String, String)>,
    shown_since: Option<Instant>,
    status_rx: Option<Receiver<Reply>>,
    last_fetch: Option<Instant>,
    answer_rx: Option<Receiver<Reply>>,
    /// 正在回答的那一条（端点、数字、同意与否）。回答飞着的时候确认行不画、
    /// y/n 不再接——免得连按两下，第二下落到下一条请求上。
    answering: Option<(PendingJoin, bool)>,
}

impl MeshPanel {
    /// 换上一份新的现状。确认行上那一条的身份（端点、数字）变了，就从
    /// `now` 重新计时。
    pub(crate) fn set_view(&mut self, v: MeshView, now: Instant) {
        let id = v
            .pending
            .first()
            .map(|p| (p.endpoint.clone(), p.code.clone()));
        if id != self.shown {
            self.shown = id;
            self.shown_since = Some(now);
        }
        self.view = Some(v);
    }

    /// 确认行上那一条已经挂够 `SETTLE` 了吗。`view` 被绕过 `set_view` 换掉
    /// 的话身份对不上，一律当没挂够——宁可这一下不算数。
    fn settled(&self, p: &PendingJoin, now: Instant) -> bool {
        self.shown.as_ref() == Some(&(p.endpoint.clone(), p.code.clone()))
            && self
                .shown_since
                .is_some_and(|t| now.saturating_duration_since(t) >= SETTLE)
    }

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
/// 只在看板和九宫格上问（九宫格上有人想加入时要提醒一句）——别的视图不画
/// 这一块。
pub(crate) fn poll(app: &mut App, now: Instant) {
    if let Some(rx) = &app.mesh.answer_rx {
        if let Ok(r) = rx.try_recv() {
            app.mesh.answer_rx = None;
            let answered = app.mesh.answering.take();
            // 回答之前发出去的那次状态查询，回来的是批准之前的样子，扔掉。
            app.mesh.status_rx = None;
            match (r, answered) {
                (Ok(Response::Mesh(v)), Some((asked, yes))) => {
                    say(
                        app,
                        if yes {
                            msg::mesh_member_joined(app.lang, &clean(&asked.name))
                        } else {
                            msg::mesh_member_refused(app.lang, &clean(&asked.name))
                        }
                        .into(),
                    );
                    app.mesh.set_view(v, now);
                    app.mesh.last_fetch = Some(now);
                }
                (Ok(Response::Error(e)), _) => {
                    say(app, Msg::err(msg::error(app.lang, &e)));
                    app.mesh.last_fetch = None;
                }
                _ => {
                    say(
                        app,
                        Msg::err(msg::error(
                            app.lang,
                            &crate::proto::ErrorCode::DaemonNotResponding,
                        )),
                    );
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
                    app.mesh.set_view(v, now);
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
        && matches!(app.view, View::Board | View::Grid { .. })
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

/// 回答晚到的那句话：人已经在会话里了就不说——会话视图的底栏是给 agent
/// 那一屏用的，一句「已加入」盖上去没人需要。
fn say(app: &mut App, m: Msg) {
    if !matches!(app.view, View::Attached(_)) {
        app.message = m;
    }
}

/// 看板上按了 y / n：确认行在的时候由它接管（返回 `true`）。**只从
/// `board::handle_key` 调**——会话视图里 y/n 归 agent，九宫格里也不接
/// （那边 `n` 是新建会话，只提醒一句「回看板确认」）。
pub(crate) fn handle_key(app: &mut App, key: &KeyEvent) -> bool {
    handle_key_at(app, key, Instant::now())
}

pub(crate) fn handle_key_at(app: &mut App, key: &KeyEvent, now: Instant) -> bool {
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
    // 刚换上来的一条还没挂够：这一下不算它的（y 什么都不做，n 照旧是新建
    // 会话）。
    if !app.mesh.settled(&p, now) {
        return false;
    }
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

/// 别的电脑给的名字：跟命令行、标记行同一份洗法（`deliver::clean_name`）。
fn clean(s: &str) -> String {
    crate::mesh::deliver::clean_name(s)
}

/// 九宫格顶上那几行：有电脑在等批准时提醒一句回看板（y/n 只在看板上接）。
/// 整句折行，不截。
pub(crate) fn grid_notice_lines(panel: &MeshPanel, lang: Lang, width: usize) -> Vec<Line<'static>> {
    if panel.asking().is_none() {
        return Vec::new();
    }
    let style = theme_now().asking().add_modifier(Modifier::BOLD);
    wrap(&msg::mesh_grid_notice(lang), width)
        .into_iter()
        .map(|l| Line::from(Span::styled(l, style)))
        .collect()
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

    /// 早于 `SETTLE`：确认行已经挂够了。
    pub(crate) fn long_ago() -> Instant {
        let now = Instant::now();
        now.checked_sub(SETTLE * 4).unwrap_or(now)
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
        app.mesh.set_view(status, long_ago());
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
        app.mesh.set_view(two_waiting(), long_ago());
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

    fn grid_rows(app: &mut App, w: u16, h: u16) -> Vec<String> {
        use ratatui::backend::TestBackend;
        let mut term = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::grid::draw(f, f.area(), app))
            .unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| {
                (0..w)
                    .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                    .collect::<String>()
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect()
            })
            .collect()
    }

    /// I1：九宫格里有电脑在等：顶上一行暖色提醒，整句不截（80 列折成两行）；
    /// 没人等就不画。
    #[test]
    fn the_grid_tells_you_to_go_to_the_board_when_a_computer_waits() {
        let (mut app, _d) = App::test_app();
        app.view = View::grid(0);
        app.mesh
            .set_view(mesh_view(true, &[("家里Mac", true, true)], &[]), long_ago());
        let r = grid_rows(&mut app, 80, 20);
        assert!(!r.concat().contains("有电脑想加入"), "{r:?}");

        app.mesh.set_view(two_waiting(), long_ago());
        let r = grid_rows(&mut app, 80, 20);
        let want: String = msg::mesh_grid_notice(Lang::Zh)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert_eq!(r[0..2].concat(), want, "{r:?}");
        assert!(r[0].starts_with("有电脑想加入"));
        // 窄、矮都不 panic
        for (w, h) in [(30, 10), (10, 3), (1, 1), (0, 0)] {
            grid_rows(&mut app, w, h);
        }
    }

    fn grid_app_with_sessions() -> (App, tempfile::TempDir) {
        let (mut app, dir) = App::test_app();
        app.connected = true;
        app.set_sessions(
            (1..=4)
                .map(|i| crate::session::SessionInfo {
                    id: i,
                    profile: "claude".into(),
                    dir: "/tmp/a".into(),
                    state: crate::session::SessionState::Idle,
                    activity: String::new(),
                    is_agent: true,
                    tag: format!("tile{i}"),
                })
                .collect(),
        );
        app.view = View::grid(0);
        (app, dir)
    }

    fn whole_screen(app: &mut App, w: u16, h: u16) -> String {
        use ratatui::backend::TestBackend;
        let mut term = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    /// 80×24 整屏：九宫格的内容区正好是 `MIN_ROWS`。提醒要在，格子也要在，
    /// 不能换成「窗口太小」——提醒是盖上去的，不是切掉两行。
    #[test]
    fn the_grid_notice_keeps_the_tiles_on_an_80_by_24_terminal() {
        let (mut app, _d) = grid_app_with_sessions();
        app.mesh.set_view(two_waiting(), long_ago());
        let c = whole_screen(&mut app, 80, 24);
        let notice: String = msg::mesh_grid_notice(Lang::Zh)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(c.contains(&notice), "{c}");
        assert!(!c.contains("窗口太小"), "{c}");
        assert!(
            c.contains("tile2") && c.contains("tile4"),
            "格子该还在：{c}"
        );
        // 九宫格自己拿到的内容区正好是 `MIN_ROWS`（底栏多占两行时 80×24 就是
        // 这样）：让不起，只能盖。
        for h in [20, 21] {
            let r = grid_rows(&mut app, 80, h).concat();
            assert!(r.contains(&notice), "{h}: {r}");
            assert!(!r.contains("窗口太小"), "{h}: {r}");
            // 盖住的是第一排格子的标题行；第二排整个都在。
            assert!(r.contains("tile3") && r.contains("tile4"), "{h}: {r}");
        }
        // 高得多的窗口里让得起：提醒在上面，格子一个不少
        let c = whole_screen(&mut app, 80, 40);
        assert!(c.contains(&notice) && c.contains("tile1"), "{c}");
        assert!(!c.contains("窗口太小"), "{c}");
    }

    /// 回复框开着时不提醒：那时按 g 是往框里打字，「按 g 切到看板」是错话。
    #[test]
    fn the_grid_notice_is_not_shown_while_the_reply_box_is_open() {
        let (mut app, _d) = grid_app_with_sessions();
        app.mesh.set_view(two_waiting(), long_ago());
        crate::ui::grid::handle_key(&mut app, key('i')).unwrap();
        assert!(matches!(app.view, View::Grid { reply: Some(_), .. }));
        let c = whole_screen(&mut app, 80, 24);
        assert!(!c.contains("有电脑想加入"), "{c}");
    }

    /// I1：九宫格里也每 5 秒问一次（不然根本不知道有人在等）。
    #[test]
    fn status_is_asked_in_the_grid_too() {
        let (mut app, _d, got) = board_app(mesh_view(true, &[], &[]));
        app.view = View::grid(0);
        app.mesh.view = None;
        poll(&mut app, Instant::now());
        let deadline = Instant::now() + Duration::from_secs(5);
        while statuses(&got) < 1 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// I1：九宫格里 y/n 不接（`n` 在那边是新建会话）。
    #[test]
    fn y_and_n_in_the_grid_send_no_approval() {
        let (mut app, _d, got) = board_app(two_waiting());
        app.view = View::grid(0);
        crate::ui::dispatch_key(&mut app, key('y')).unwrap();
        crate::ui::dispatch_key(&mut app, key('n')).unwrap();
        assert!(app.mesh.answering.is_none());
        std::thread::sleep(Duration::from_millis(200));
        poll(&mut app, Instant::now());
        assert!(approvals(&got).is_empty(), "{:?}", approvals(&got));
    }

    /// M1：确认行上那一条刚出现、或者刚被换成另一条，`SETTLE` 之内 y/n 不算数。
    #[test]
    fn a_request_that_just_appeared_or_changed_is_not_answered_yet() {
        let (mut app, _d, got) = board_app(mesh_view(true, &[], &[]));
        let t0 = Instant::now();
        let a = mesh_view(true, &[], &[("甲", "c-a", "111111")]);
        let b = mesh_view(true, &[], &[("乙", "c-b", "222222")]);

        // 刚出现
        app.mesh.set_view(a.clone(), t0);
        assert!(!handle_key_at(&mut app, &key('y'), t0 + SETTLE / 5));
        assert!(!handle_key_at(&mut app, &key('n'), t0 + SETTLE / 5));
        // 同一条再刷新一次不重新计时
        app.mesh.set_view(a.clone(), t0 + SETTLE / 2);
        // A 在按键前一刻被换成了 B
        let t1 = t0 + SETTLE * 3;
        app.mesh.set_view(b.clone(), t1);
        assert!(!handle_key_at(&mut app, &key('y'), t1 + SETTLE / 5));
        assert!(approvals(&got).is_empty());
        // B 挂够了才算
        assert!(handle_key_at(&mut app, &key('y'), t1 + SETTLE));
        wait_until(&mut app, |a| a.mesh.answering.is_none());
        assert_eq!(
            approvals(&got),
            [("c-b".to_string(), "222222".to_string(), true)]
        );

        // 绕过 `set_view` 换掉的现状：身份对不上，一律不算数。
        let (mut app, _d3, got3) = board_app(mesh_view(true, &[], &[]));
        app.mesh.set_view(a.clone(), t0);
        app.mesh.view = Some(b.clone());
        assert!(!handle_key_at(&mut app, &key('y'), t0 + SETTLE * 10));
        assert!(approvals(&got3).is_empty());

        // 同一条被刷新（身份没变）：从第一次出现算起。
        let (mut app, _d2, got) = board_app(mesh_view(true, &[], &[]));
        app.mesh.set_view(a.clone(), t0);
        app.mesh.set_view(a, t0 + SETTLE / 2);
        assert!(handle_key_at(&mut app, &key('n'), t0 + SETTLE));
        wait_until(&mut app, |a| a.mesh.answering.is_none());
        assert_eq!(approvals(&got).len(), 1);
    }

    /// M4：别的电脑给的名字里有双向控制符、零宽字符：画之前去掉。
    #[test]
    fn format_characters_in_a_remote_name_never_reach_the_screen() {
        let name = "\u{202E}公司\u{200B}Win\u{2066}";
        let panel = {
            let mut p = MeshPanel::default();
            p.set_view(
                mesh_view(true, &[(name, true, false)], &[(name, "c-x", "123456")]),
                long_ago(),
            );
            p
        };
        let text: String = prompt_lines(&panel, Lang::Zh, 80)
            .into_iter()
            .chain(section_lines(&panel, Lang::Zh, 80))
            .flat_map(|l| l.spans.into_iter().map(|s| s.content.into_owned()))
            .collect();
        assert!(text.contains("公司Win"), "{text:?}");
        assert!(
            !text.chars().any(crate::mesh::deliver::is_format_char),
            "{text:?}"
        );
    }

    /// M6：回答晚到、人已经进了会话：不往会话的底栏上说话；现状照样换上。
    #[test]
    fn a_late_answer_does_not_talk_over_the_session_view() {
        let (mut app, _d, got) = board_app(two_waiting());
        crate::ui::dispatch_key(&mut app, key('y')).unwrap();
        app.view = View::Attached(1);
        app.message = "会话里的话".into();
        let deadline = Instant::now() + Duration::from_secs(5);
        while approvals(&got).is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        wait_until(&mut app, |a| a.mesh.answering.is_none());
        assert_eq!(app.message.text, "会话里的话");
        assert_eq!(app.mesh.view.as_ref().unwrap().pending.len(), 1);
    }

    #[test]
    fn wrapping_counts_wide_characters_as_two_columns() {
        assert_eq!(wrap("一二三abc", 4), ["一二", "三ab", "c"]);
        assert_eq!(wrap("abc", 0), Vec::<String>::new());
        assert_eq!(wrap("一", 1), ["一"], "一个字都放不下也不能死循环");
    }
}
