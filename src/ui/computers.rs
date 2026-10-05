//! 看板上的「我的电脑」：顶上的邀请码（按 `a` 要来的）、底部那一段电脑
//! 名单、会话行末尾的「✉ N」。
//!
//! **绘制路径上不发任何请求。** `MeshStatus` 在守护进程那边要问一趟中转
//! （谁在线），可能卡好几秒；`MeshInvite` 虽然只在本机，也照样丢给后台
//! 线程，主循环每轮 `poll` 一次收结果（同 `pair_start_rx` 的做法）。画的
//! 时候只读 `MeshPanel` 里现成的那一份。
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use ratatui::prelude::*;

use super::app::App;
use super::view::View;
use super::widgets::{display_width, truncate, Msg};
use super::{dim, theme_now};
use crate::i18n::{msg, Lang};
use crate::proto::{InviteOutcome, MemberView, MeshView, Request, Response};

/// 多久问一次 `MeshStatus`。
pub(crate) const POLL_EVERY: Duration = Duration::from_secs(5);
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
    /// 按了 `a`、`MeshInvite` 还在飞。飞着的时候再按不再发第二条。
    invite_rx: Option<Receiver<Reply>>,
    /// 这个界面自己要来的最近一个码（`InviteView::id`）。只有它的结果才在
    /// 底栏说一句——别处（`dct invite`）要的码，那边自己会说。
    created: Option<u64>,
    /// 已经说过的那个结果（`InviteNote::id`），同一句不说两遍。
    announced: Option<u64>,
}

impl MeshPanel {
    /// 换上一份新的现状。
    pub(crate) fn set_view(&mut self, v: MeshView) {
        self.view = Some(v);
    }

    /// 这个会话这次运行期间收到过几条留言。
    pub(crate) fn messages_for(&self, session: u32) -> u32 {
        self.view
            .as_ref()
            .and_then(|v| v.messages.get(&session).copied())
            .unwrap_or(0)
    }
}

/// 看板上按 `a`：要一个新码（旧的由守护进程作废）。上一条还在飞就不再发。
pub(crate) fn start_invite(app: &mut App) {
    if app.mesh.invite_rx.is_some() {
        return;
    }
    app.mesh.invite_rx = Some(spawn_call(
        app.socket.clone(),
        Request::MeshInvite,
        crate::client::READ_TIMEOUT * 2,
    ));
}

/// 新拿到的现状里有这个界面要来的那个码的结果，还没说过：说一句。
fn announce(app: &mut App) {
    let Some(n) = app.mesh.view.as_ref().and_then(|v| v.invite_note.clone()) else {
        return;
    };
    if app.mesh.created != Some(n.id) || app.mesh.announced == Some(n.id) {
        return;
    }
    app.mesh.announced = Some(n.id);
    let m = match &n.outcome {
        InviteOutcome::Joined { name } => msg::mesh_member_joined(app.lang, &clean(name)).into(),
        InviteOutcome::Burned => Msg::err(msg::mesh_invite_burned_board(app.lang)),
        InviteOutcome::Expired => Msg::err(msg::mesh_invite_expired_board(app.lang)),
    };
    say(app, m);
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
/// 只在看板和九宫格上问——别的视图不画这一块（九宫格上也要问：码的结果
/// 在底栏说，人可能正停在九宫格上）。
pub(crate) fn poll(app: &mut App, now: Instant) {
    if let Some(rx) = &app.mesh.invite_rx {
        match rx.try_recv() {
            Ok(r) => {
                app.mesh.invite_rx = None;
                match r {
                    Ok(Response::MeshInvite(v)) => {
                        app.mesh.created = Some(v.id);
                        if let Some(view) = app.mesh.view.as_mut() {
                            view.invite = Some(v);
                        }
                        // 要码之前发出去的那次状态查询，回来的是没有这个码的
                        // 样子，扔掉；下一轮就重新问，别等 5 秒。
                        app.mesh.status_rx = None;
                        app.mesh.last_fetch = None;
                    }
                    Ok(Response::Error(e)) => say(app, Msg::err(msg::error(app.lang, &e))),
                    _ => say(
                        app,
                        Msg::err(msg::error(
                            app.lang,
                            &crate::proto::ErrorCode::DaemonNotResponding,
                        )),
                    ),
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => app.mesh.invite_rx = None,
        }
    }
    if let Some(rx) = &app.mesh.status_rx {
        match rx.try_recv() {
            Ok(r) => {
                app.mesh.status_rx = None;
                // 问不到（断线、守护进程太老不认识这条）就留着手里那份。
                if let Ok(Response::Mesh(v)) = r {
                    app.mesh.set_view(v);
                    announce(app);
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
    {
        app.mesh.last_fetch = Some(now);
        app.mesh.status_rx = Some(spawn_call(
            app.socket.clone(),
            Request::MeshStatus,
            crate::client::READ_TIMEOUT * 2,
        ));
    }
}

/// 晚到的那句话：人已经在会话里了就不说——会话视图的底栏是给 agent
/// 那一屏用的，一句「已加入」盖上去没人需要。
fn say(app: &mut App, m: Msg) {
    if !matches!(app.view, View::Attached(_)) {
        app.message = m;
    }
}

/// 别的电脑给的名字：跟命令行、标记行同一份洗法（`deliver::clean_name`）。
fn clean(s: &str) -> String {
    crate::mesh::deliver::clean_name(s)
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

/// 顶部的邀请码：`邀请码 482 913 · 10 分钟内有效 · 还剩 9:41`（黄字，整句
/// 折行——码一位都不能被切掉），下面一行灰字说新电脑上敲什么。`now` 是
/// unix 秒；到点了就不画（现状最多晚 5 秒才知道它过期）。
pub(crate) fn invite_lines(panel: &MeshPanel, lang: Lang, width: usize, now: u64) -> Vec<Line<'static>> {
    let Some(v) = panel.view.as_ref().and_then(|v| v.invite.as_ref()) else {
        return Vec::new();
    };
    if now >= v.expires_at || v.code.len() != 6 {
        return Vec::new();
    }
    let spaced = format!("{} {}", &v.code[..3], &v.code[3..]);
    let line = format!(
        "{} · {}",
        msg::mesh_invite_code(lang, &spaced),
        msg::mesh_invite_countdown(lang, v.expires_at - now)
    );
    let style = theme_now().asking().add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line> = wrap(&line, width)
        .into_iter()
        .map(|l| Line::from(Span::styled(l, style)))
        .collect();
    if width > 1 {
        lines.push(Line::from(Span::styled(
            truncate(&msg::mesh_invite_hint(lang, &v.code), width.saturating_sub(1)),
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
    use crate::proto::{InviteNote, InviteView, MeshView};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::{Arc, Mutex};

    /// 一份现状：`members` 是（名字, 在线, 本机）。
    pub(crate) fn mesh_view(logged_in: bool, members: &[(&str, bool, bool)]) -> MeshView {
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
            messages: Default::default(),
            invite: None,
            invite_note: None,
        }
    }

    /// 假守护进程发的码在这一刻（unix 秒）过期。
    const INVITE_EXPIRES: u64 = 1_800_000_600;

    /// 假守护进程：每条连接一个线程（会话视图那条长连接不能把别的挡住），
    /// 记下收到的每一条请求。`MeshStatus` 回 `status`；`MeshInvite` 回一个
    /// 固定的码，`id` 是第几次要。
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
                            Request::MeshInvite => {
                                let n = got
                                    .lock()
                                    .unwrap()
                                    .iter()
                                    .filter(|r| matches!(r, Request::MeshInvite))
                                    .count() as u64;
                                Response::MeshInvite(InviteView {
                                    id: n + 1,
                                    code: "482913".into(),
                                    expires_at: INVITE_EXPIRES,
                                })
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

    fn count(got: &Arc<Mutex<Vec<Request>>>, f: fn(&Request) -> bool) -> usize {
        got.lock().unwrap().iter().filter(|r| f(r)).count()
    }

    fn invites(got: &Arc<Mutex<Vec<Request>>>) -> usize {
        count(got, |r| matches!(r, Request::MeshInvite))
    }

    fn statuses(got: &Arc<Mutex<Vec<Request>>>) -> usize {
        count(got, |r| matches!(r, Request::MeshStatus))
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

    fn board_app(status: MeshView) -> (App, tempfile::TempDir, Arc<Mutex<Vec<Request>>>) {
        let (mut app, dir) = App::test_app();
        let got = fake_daemon(&app.socket, status.clone());
        app.connected = true;
        app.view = View::Board;
        app.mesh.set_view(status);
        (app, dir, got)
    }

    fn text_of(lines: &[Line]) -> String {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn one_computer() -> MeshView {
        mesh_view(true, &[("家里Mac", true, true)])
    }

    /// 每 5 秒问一次，而且只在看板上问；一次在飞的时候不再发第二次。
    #[test]
    fn status_is_asked_every_five_seconds_and_only_on_the_board() {
        let status = one_computer();
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
        let (mut app, _d, got) = board_app(mesh_view(true, &[]));
        let (_tx, rx) = mpsc::channel::<Reply>();
        app.mesh.status_rx = Some(rx);
        poll(&mut app, Instant::now() + POLL_EVERY * 2);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(statuses(&got), 0);
    }

    /// 九宫格里也问：码的结果要在底栏说，人可能停在九宫格上。
    #[test]
    fn status_is_asked_in_the_grid_too() {
        let (mut app, _d, got) = board_app(mesh_view(true, &[]));
        app.view = View::grid(0);
        app.mesh.view = None;
        poll(&mut app, Instant::now());
        let deadline = Instant::now() + Duration::from_secs(5);
        while statuses(&got) < 1 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 别的电脑给的名字里有双向控制符、零宽字符：画之前去掉。
    #[test]
    fn format_characters_in_a_remote_name_never_reach_the_screen() {
        let name = "\u{202E}公司\u{200B}Win\u{2066}";
        let mut panel = MeshPanel::default();
        panel.set_view(mesh_view(true, &[(name, true, false)]));
        let text: String = section_lines(&panel, Lang::Zh, 80)
            .into_iter()
            .flat_map(|l| l.spans.into_iter().map(|s| s.content.into_owned()))
            .collect();
        assert!(text.contains("公司Win"), "{text:?}");
        assert!(
            !text.chars().any(crate::mesh::deliver::is_format_char),
            "{text:?}"
        );
    }

    #[test]
    fn wrapping_counts_wide_characters_as_two_columns() {
        assert_eq!(wrap("一二三abc", 4), ["一二", "三ab", "c"]);
        assert_eq!(wrap("abc", 0), Vec::<String>::new());
        assert_eq!(wrap("一", 1), ["一"], "一个字都放不下也不能死循环");
    }

    /// 看板上按 `a`：要一个码，拿到之后顶上画出来，带倒计时和新电脑上敲什么。
    #[test]
    fn a_on_the_board_gets_a_code_and_shows_it_with_a_countdown() {
        let (mut app, _d, got) = board_app(one_computer());
        crate::ui::dispatch_key(&mut app, key('a')).unwrap();
        wait_until(&mut app, |a| {
            a.mesh.view.as_ref().is_some_and(|v| v.invite.is_some())
        });
        assert_eq!(invites(&got), 1);
        assert_eq!(app.mesh.created, Some(1));
        let shown = text_of(&invite_lines(&app.mesh, Lang::Zh, 80, INVITE_EXPIRES - 599));
        assert_eq!(
            shown,
            "邀请码 482 913 · 10 分钟内有效 · 还剩 9:59\n在新电脑上运行：dct join 482913（码只能用一次）"
        );
    }

    /// 窄屏：码那一行整句折行，一位数字都不切掉。
    #[test]
    fn a_narrow_board_wraps_the_code_line_instead_of_cutting_it() {
        let mut v = one_computer();
        v.invite = Some(InviteView {
            id: 1,
            code: "482913".into(),
            expires_at: 1_600,
        });
        let mut panel = MeshPanel::default();
        panel.set_view(v);
        let lines = invite_lines(&panel, Lang::Zh, 20, 1_000);
        let all: String = lines[..lines.len() - 1]
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(all.contains("482 913"), "{all:?}");
        assert!(lines.len() > 2, "20 列放不下一行");
    }

    /// 连按两下 `a`：第一条还在飞，第二下不发；回来之后再按才换新码。
    #[test]
    fn pressing_a_twice_quickly_asks_once() {
        let (mut app, _d, got) = board_app(one_computer());
        start_invite(&mut app);
        start_invite(&mut app);
        wait_until(&mut app, |a| a.mesh.invite_rx.is_none());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(invites(&got), 1);
        start_invite(&mut app);
        wait_until(&mut app, |a| a.mesh.invite_rx.is_none());
        assert_eq!(invites(&got), 2);
        assert_eq!(app.mesh.created, Some(2), "以新码为准");
    }

    #[test]
    fn an_expired_code_is_not_drawn() {
        let mut v = one_computer();
        v.invite = Some(InviteView {
            id: 1,
            code: "012345".into(),
            expires_at: 1_000,
        });
        let mut panel = MeshPanel::default();
        panel.set_view(v);
        assert!(text_of(&invite_lines(&panel, Lang::Zh, 80, 999)).contains("012 345"));
        assert!(invite_lines(&panel, Lang::Zh, 80, 1_000).is_empty());
    }

    /// 这个界面要来的码有了结果：底栏说一句，只说一次；别处要来的码的结果不说。
    #[test]
    fn the_outcome_of_our_own_code_is_said_once() {
        use crate::proto::InviteOutcome::*;
        let cases = [
            (Joined { name: "公司电脑".into() }, "公司电脑 已加入"),
            (Burned, "有人用错码试过一次，码已作废，按 a 重新生成"),
            (Expired, "邀请码过期了，按 a 重新生成"),
        ];
        for (outcome, want) in cases {
            let mut v = one_computer();
            v.invite_note = Some(InviteNote { id: 7, outcome });
            let (mut app, _d, _got) = board_app(one_computer());
            app.mesh.created = Some(7);
            app.mesh.set_view(v.clone());
            announce(&mut app);
            assert_eq!(app.message.text, want);
            app.message = "别的".into();
            announce(&mut app);
            assert_eq!(app.message.text, "别的", "同一句不说两遍");

            app.mesh.created = Some(8);
            app.mesh.announced = None;
            announce(&mut app);
            assert_eq!(app.message.text, "别的", "不是这个界面要的码");
        }
    }

    /// 结果晚到、人已经进了会话：不往会话的底栏上说话。
    #[test]
    fn a_late_outcome_does_not_talk_over_the_session_view() {
        let mut v = one_computer();
        v.invite_note = Some(InviteNote {
            id: 1,
            outcome: crate::proto::InviteOutcome::Joined { name: "B".into() },
        });
        let (mut app, _d, _got) = board_app(v);
        app.mesh.created = Some(1);
        app.view = View::Attached(1);
        app.message = "会话里的话".into();
        announce(&mut app);
        assert_eq!(app.message.text, "会话里的话");
    }

    /// 会话视图里 `a` 归 agent：不要码。
    #[test]
    fn a_in_the_session_view_asks_for_no_code() {
        let (mut app, _d, got) = board_app(one_computer());
        app.set_sessions(vec![crate::session::SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/tmp/a".into(),
            state: crate::session::SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
            last_active_ms: 0,
        }]);
        app.client = Some(crate::client::Client::connect(&app.socket).unwrap());
        app.view = View::Attached(1);
        crate::ui::dispatch_key(&mut app, key('a')).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        poll(&mut app, Instant::now());
        assert_eq!(invites(&got), 0);
        assert!(app.mesh.invite_rx.is_none());
        assert!(
            got.lock()
                .unwrap()
                .iter()
                .any(|r| matches!(r, Request::Input { .. })),
            "a 该送进会话里"
        );
    }

    /// 没登录多电脑时按 `a`：守护进程说没登录，底栏照说。
    #[test]
    fn a_before_login_says_to_log_in() {
        let (mut app, _d, _got) = board_app(mesh_view(false, &[]));
        app.mesh.invite_rx = Some({
            let (tx, rx) = mpsc::channel();
            tx.send(Ok(Response::Error(crate::proto::ErrorCode::Mesh(
                crate::proto::MeshProblem::NotLoggedIn,
            ))))
            .unwrap();
            rx
        });
        poll(&mut app, Instant::now());
        assert_eq!(app.message.text, "还没登录多电脑，先运行 dct login");
        assert!(app.mesh.created.is_none());
    }
}
