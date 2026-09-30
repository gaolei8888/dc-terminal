//! `dct login` / `dct invite` / `dct join` / `dct peers` / `dct send`。
//!
//! 这里只是把话说给人听：真正的事（换令牌、问别的电脑、签名单）都在守护
//! 进程里做——它握着中转连接、钥匙和密钥仓，命令行这边一样也不碰。
//!
//! 跟守护进程说话的那一下（`call`）是参数，于是整套对话不起守护进程、
//! 不碰终端也能测。
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use crate::client::Client;
use crate::i18n::{msg, text, Key, Lang};
use crate::proto::{
    ErrorCode, MeshProblem, MeshView, PeerView, Request, Response, SendOutcome,
};

/// 守护进程替我们打网络的那几条请求（换令牌、问别的电脑、广播名单）等多久。
/// 必须比守护进程那边最坏的情况（`mesh::worst_case`）长出一截，见 `wait_for`。
const MESH_CALL_WAIT: Duration = Duration::from_secs(90);

/// `dct invite` 等结果时多久问一次守护进程（倒计时也按这个跳）。
const INVITE_POLL: Duration = Duration::from_secs(1);

/// 这一条请求命令行等多久：多电脑的请求等 `MESH_CALL_WAIT`，别的（握手）
/// 按本机答一句的 `READ_TIMEOUT`。
///
/// 等不够的代价不是「慢一点」：守护进程可能在命令行放弃之后才办成——新
/// 电脑已经进了组，屏幕上却说失败，用户会拿一个已经作废的码再试一次。
fn wait_for(req: &Request) -> Duration {
    match crate::mesh::worst_case(req) {
        Some(_) => MESH_CALL_WAIT,
        None => crate::client::READ_TIMEOUT,
    }
}

type Call<'a> = &'a mut dyn FnMut(Request) -> Result<Response>;

/// `dct join` 的参数。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct JoinOpts {
    /// 老电脑上显示的邀请码，照用户敲的原样（空格、`-` 由守护进程那边去掉）。
    pub code: String,
    /// 用这个名字进组；空 = 用现在的名字。
    pub name: String,
}

pub fn run(args: &[String]) -> i32 {
    let sock = crate::proto::socket_path();
    let lang = lang_for(&sock);
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => return 2,
    };
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    // `dct peers` 只是问一句，守护进程没在跑就如实说——同 `dct ps`，问
    // 「有什么」不该把「没有」变成「有」。登录、加入是祈使句，要它在跑。
    let client = if cmd == "peers" && rest.is_empty() {
        match Client::connect(&sock) {
            Ok(c) => c,
            Err(_) => {
                let _ = writeln!(out, "{}", text(Key::NoDaemonRunning, lang));
                return 0;
            }
        }
    } else {
        match connect_or_start(&sock) {
            Some(c) => c,
            None => {
                let _ = writeln!(err, "{}", msg::error(lang, &ErrorCode::DaemonNotResponding));
                return 1;
            }
        }
    };
    // `dct invite` 被 Ctrl-C：把码收回，不留一个没人看着的码挂 10 分钟。
    // **必须装在连上守护进程之后**：Unix 上这一下会屏蔽 SIGINT/SIGTERM，
    // 屏蔽掩码会被子进程继承——要是在 `connect_or_start` 拉起守护进程之前
    // 装，那个守护进程就再也收不到 `dct stop` 的 SIGTERM。
    if cmd == "invite" {
        crate::sys::signal::restore_terminal_when_killed(cancel_invite_on_exit);
    }
    let mut client = client;
    let mut call = |req: Request| {
        let wait = wait_for(&req);
        client.call_within(req, wait)
    };
    dispatch(cmd, rest, &mut call, &mut out, &mut err, lang)
}

/// 连上守护进程之后的全部：先握手，再按子命令办。
fn dispatch(
    cmd: &str,
    rest: &[String],
    call: Call,
    out: &mut dyn Write,
    err: &mut dyn Write,
    lang: Lang,
) -> i32 {
    if !daemon_is_current(call, err, lang) {
        return 1;
    }
    match cmd {
        "login" => login(call, out, err, lang),
        "invite" if rest.is_empty() => invite(
            call,
            out,
            err,
            lang,
            std::io::stdout().is_terminal(),
            &unix_now,
            &|| std::thread::sleep(INVITE_POLL),
        ),
        "invite" => {
            let _ = writeln!(err, "{}", msg::mesh_invite_usage(lang));
            2
        }
        "join" => match parse_join(rest) {
            Some(opts) => join(call, out, err, lang, &opts),
            None => {
                let _ = writeln!(err, "{}", msg::mesh_join_usage(lang));
                2
            }
        },
        "peers" => peers(call, out, err, lang, rest),
        "send" => {
            // 在 dct 的会话里跑的，守护进程给子进程设过这个变量（`session.rs`）。
            let from = std::env::var(crate::session::SESSION_ID_ENV)
                .ok()
                .and_then(|v| v.trim().parse().ok());
            send(call, out, err, lang, rest, from)
        }
        _ => 2,
    }
}

/// 跟界面启动时同一个握手（`main::run_ui` 里的 `daemon_status`）。守护进程
/// 一活好几天，「新命令碰上旧守护进程」是常态：它不认得 `Mesh*` 请求，直接
/// 送过去换回来的是一句 serde 的原话。这里先问清楚，旧了就说人话、退 1。
///
/// **不替用户重启**：重启会断掉正在跑的会话，那得他自己决定（`dct restart`）。
fn daemon_is_current(call: Call, err: &mut dyn Write, lang: Lang) -> bool {
    let protocol = match call(Request::Hello) {
        Ok(Response::Hello { protocol }) => Some(protocol),
        _ => None,
    };
    if crate::proto::daemon_status(protocol) == crate::proto::DaemonStatus::Same {
        return true;
    }
    let _ = writeln!(err, "{}", msg::mesh_stale_daemon(lang));
    false
}

/// 跟 `ps`/`stop` 同一个语言来源。
fn lang_for(sock: &Path) -> Lang {
    let settings = crate::settings::settings_path_for_socket(sock);
    crate::i18n::resolve(crate::settings::load_lang(&settings), &|k| {
        std::env::var(k).ok()
    })
}

fn connect_or_start(sock: &Path) -> Option<Client> {
    if let Ok(c) = Client::connect(sock) {
        return Some(c);
    }
    let exe = std::env::current_exe().ok()?;
    crate::client::spawn_daemon(&exe, sock).ok()?;
    for _ in 0..50 {
        if let Ok(c) = Client::connect(sock) {
            return Some(c);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// `dct join <码> [--name X]`。码可以被空格拆成几段（`dct join 482 913`
/// 到这里是两个参数），拼回去；`--name X` / `--name=X` 可以放在哪儿都行。
/// `None` = 用法不对（没有码、不认识的 `--` 参数）。
fn parse_join(args: &[String]) -> Option<JoinOpts> {
    let mut opts = JoinOpts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix("--name=") {
            opts.name = v.to_string();
        } else if a == "--name" {
            opts.name = it.next()?.clone();
        } else if a.starts_with("--") {
            return None;
        } else {
            opts.code.push_str(a);
        }
    }
    (!opts.code.is_empty()).then_some(opts)
}

/// 别的电脑报来的东西（名字、系统、会话……）印到终端之前都过一遍：
/// 一个 `\x1b[8m` 就能把后面那行核对数字藏起来。守护进程给的现状已经洗过
/// （`group::view`），这里是第二道，跟界面用的是同一份洗法。
fn c(s: &str) -> String {
    crate::mesh::deliver::clean_name(s)
}

fn say_error(err: &mut dyn Write, lang: Lang, e: &ErrorCode) {
    let _ = writeln!(err, "{}", msg::error(lang, e));
}

/// 问一次守护进程，要的是 `MeshView`。别的答复都当失败，原因印到 `err`。
fn view_of(call: Call, req: Request, err: &mut dyn Write, lang: Lang) -> Option<MeshView> {
    match call(req) {
        Ok(Response::Mesh(v)) => Some(v),
        Ok(Response::Error(e)) => {
            say_error(err, lang, &e);
            None
        }
        Ok(other) => {
            say_error(err, lang, &ErrorCode::Internal(format!("{other:?}")));
            None
        }
        Err(e) => {
            let code = match e.downcast::<crate::proto::CodedError>() {
                Ok(c) => c.0,
                Err(e) => ErrorCode::Internal(e.to_string()),
            };
            say_error(err, lang, &code);
            None
        }
    }
}

pub(crate) fn login(call: Call, out: &mut dyn Write, err: &mut dyn Write, lang: Lang) -> i32 {
    let Some(before) = view_of(call, Request::MeshStatus, err, lang) else {
        return 1;
    };
    let Some(after) = view_of(call, Request::MeshLogin, err, lang) else {
        return 1;
    };
    if !before.in_group && after.in_group {
        let _ = writeln!(out, "{}", msg::mesh_first_computer(lang));
    }
    let _ = writeln!(out, "{}", msg::mesh_logged_in(lang, &c(&after.name)));
    0
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Ctrl-C 打断 `dct invite` 时跑（Unix 上在 `sigwait` 那条普通线程里，
/// Windows 上在控制台处理线程里——都能放心开 socket）。另开一条连接，
/// 跑完进程就退出。
fn cancel_invite_on_exit() {
    let sock = crate::proto::socket_path();
    if let Ok(mut c) = Client::connect(&sock) {
        let _ = c.call_within(Request::MeshInviteCancel, crate::client::READ_TIMEOUT);
    }
}

/// 老电脑上 `dct invite`：出一个码，印出来，一直等到它有结果（有人用它进了
/// 组、被试错作废、过期、被看板上新按的 `a` 换掉）。`tty` 为真时每秒刷一行
/// 倒计时。进组退 0，别的退 1。
#[allow(clippy::too_many_arguments)]
pub(crate) fn invite(
    call: Call,
    out: &mut dyn Write,
    err: &mut dyn Write,
    lang: Lang,
    tty: bool,
    now: &dyn Fn() -> u64,
    pause: &dyn Fn(),
) -> i32 {
    let v = match call(Request::MeshInvite) {
        Ok(Response::MeshInvite(v)) => v,
        Ok(Response::Error(e)) => {
            say_error(err, lang, &e);
            return 1;
        }
        Ok(other) => {
            say_error(err, lang, &ErrorCode::Internal(format!("{other:?}")));
            return 1;
        }
        Err(e) => {
            say_error(err, lang, &ErrorCode::Internal(e.to_string()));
            return 1;
        }
    };
    let spaced = format!("{} {}", &v.code[..3], &v.code[3..]);
    let _ = writeln!(out, "{}", msg::mesh_invite_code(lang, &spaced));
    let _ = writeln!(out, "{}", msg::mesh_invite_hint(lang, &v.code));
    let _ = out.flush();
    let mut ticked = false;
    // 倒计时那一行后面接的话要另起一行。
    let end = |out: &mut dyn Write, ticked: bool| {
        if ticked {
            let _ = writeln!(out);
        }
    };
    loop {
        pause();
        let Some(s) = view_of(call, Request::MeshStatus, err, lang) else {
            end(out, ticked);
            return 1;
        };
        if let Some(n) = s.invite_note.as_ref().filter(|n| n.id == v.id) {
            end(out, ticked);
            return match &n.outcome {
                crate::proto::InviteOutcome::Joined { name } => {
                    let _ = writeln!(out, "{}", msg::mesh_member_joined(lang, &c(name)));
                    0
                }
                crate::proto::InviteOutcome::Burned => {
                    let _ = writeln!(err, "{}", msg::mesh_invite_burned(lang));
                    1
                }
                crate::proto::InviteOutcome::Expired => {
                    let _ = writeln!(err, "{}", msg::mesh_invite_expired(lang));
                    1
                }
            };
        }
        match &s.invite {
            Some(cur) if cur.id == v.id => {}
            Some(_) => {
                end(out, ticked);
                let _ = writeln!(err, "{}", msg::mesh_invite_replaced(lang));
                return 1;
            }
            None => {
                end(out, ticked);
                let _ = writeln!(err, "{}", msg::mesh_invite_gone(lang));
                return 1;
            }
        }
        if tty {
            let left = v.expires_at.saturating_sub(now());
            let _ = write!(out, "\r{}", msg::mesh_invite_countdown(lang, left));
            let _ = out.flush();
            ticked = true;
        }
    }
}

/// 新电脑上：拿老电脑给的码加入。登录（没登录的话）、找发邀请的那台、
/// 核对，全在守护进程里一次办完；这里只把结果说给人听。
pub(crate) fn join(
    call: Call,
    out: &mut dyn Write,
    err: &mut dyn Write,
    lang: Lang,
    opts: &JoinOpts,
) -> i32 {
    let req = Request::MeshJoin {
        code: opts.code.clone(),
        name: opts.name.clone(),
    };
    let Some(v) = view_of(call, req, err, lang) else {
        return 1;
    };
    let names: Vec<String> = v.members.iter().map(|m| c(&m.name)).collect();
    let _ = writeln!(out, "{}", msg::mesh_joined_with(lang, &names));
    // 起的名字跟组里的撞了，老电脑给编了号：告诉他现在叫什么。
    if !opts.name.trim().is_empty() && v.name != opts.name.trim() {
        let _ = writeln!(out, "{}", msg::mesh_renamed_on_join(lang, &c(&v.name)));
    }
    0
}

pub(crate) fn peers(
    call: Call,
    out: &mut dyn Write,
    err: &mut dyn Write,
    lang: Lang,
    args: &[String],
) -> i32 {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] => {
            let Some(v) = view_of(call, Request::MeshStatus, err, lang) else {
                return 1;
            };
            if !v.logged_in {
                say_error(err, lang, &ErrorCode::Mesh(MeshProblem::NotLoggedIn));
                return 1;
            }
            let _ = writeln!(out, "{}", msg::mesh_members_header(lang));
            // 详情要问每台在线的电脑；问不成就退回名单上那几行，不整个失败。
            match call(Request::MeshPeers) {
                Ok(Response::MeshPeers(list)) => print_peers(out, lang, &v, &list),
                _ => {
                    for m in &v.members {
                        let _ = writeln!(
                            out,
                            "{}",
                            msg::mesh_member_line(lang, &c(&m.name), m.online, m.is_me)
                        );
                    }
                }
            }
            0
        }
        ["remove", who] => {
            let req = Request::MeshRemove {
                name: who.to_string(),
            };
            if view_of(call, req, err, lang).is_none() {
                return 1;
            }
            let _ = writeln!(out, "{}", msg::mesh_member_removed(lang, who));
            0
        }
        _ => {
            let _ = writeln!(err, "{}", msg::mesh_peers_usage(lang));
            2
        }
    }
}

/// `dct peers` 的详情：每台一行（在不在线、系统），下面是它开着的会话和触手。
fn print_peers(out: &mut dyn Write, lang: Lang, v: &MeshView, list: &[PeerView]) {
    for p in list {
        let is_me = v.members.iter().any(|m| m.is_me && m.name == p.name);
        let _ = writeln!(
            out,
            "{}",
            msg::mesh_peer_line(lang, &c(&p.name), p.online, is_me, &c(&p.os))
        );
        if !p.online {
            continue;
        }
        if p.sessions.is_empty() {
            let _ = writeln!(out, "{}", msg::mesh_no_sessions(lang));
        }
        for s in &p.sessions {
            let _ = writeln!(out, "    {}  {}  {}", c(&s.name), c(&s.state), c(&s.dir));
        }
        if !p.tentacles.is_empty() {
            let _ = writeln!(
                out,
                "{}",
                msg::mesh_tentacles_line(
                    lang,
                    &p.tentacles.iter().map(|t| c(t)).collect::<Vec<_>>()
                )
            );
        }
    }
}

/// `dct send <电脑名/会话名> "<内容>"`。送到、排上队退 0，别的退 1。
pub(crate) fn send(
    call: Call,
    out: &mut dyn Write,
    err: &mut dyn Write,
    lang: Lang,
    args: &[String],
    from_session: Option<u32>,
) -> i32 {
    let Some((to, words)) = args.split_first() else {
        let _ = writeln!(err, "{}", msg::mesh_send_usage(lang));
        return 2;
    };
    let text = words.join(" ");
    if text.trim().is_empty() {
        let _ = writeln!(err, "{}", msg::mesh_send_usage(lang));
        return 2;
    }
    let to = to.trim();
    let (machine, session) = crate::mesh::deliver::split_address(to).unwrap_or((to, ""));
    let req = Request::MeshSend {
        to: to.to_string(),
        text,
        from_session,
    };
    let outcome = match call(req) {
        Ok(Response::MeshSent(o)) => o,
        Ok(Response::Error(e)) => {
            say_error(err, lang, &e);
            return 1;
        }
        Ok(other) => {
            say_error(err, lang, &ErrorCode::Internal(format!("{other:?}")));
            return 1;
        }
        Err(e) => {
            let code = match e.downcast::<crate::proto::CodedError>() {
                Ok(c) => c.0,
                Err(e) => ErrorCode::Internal(e.to_string()),
            };
            say_error(err, lang, &code);
            return 1;
        }
    };
    match outcome {
        SendOutcome::Delivered => {
            let _ = writeln!(out, "{}", msg::mesh_sent(lang, to));
            0
        }
        SendOutcome::Queued => {
            let _ = writeln!(out, "{}", msg::mesh_queued(lang));
            0
        }
        SendOutcome::NoSuchMachine => {
            say_error(
                err,
                lang,
                &ErrorCode::Mesh(MeshProblem::NoSuchMachine(machine.to_string())),
            );
            1
        }
        other => {
            let _ = writeln!(
                err,
                "{}",
                msg::mesh_not_sent(lang, machine, session, &other)
            );
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::MemberView;
    use std::cell::RefCell;

    fn view(logged_in: bool, members: &[(&str, bool)]) -> MeshView {
        MeshView {
            logged_in,
            name: "B".into(),
            endpoint: "c-b".into(),
            in_group: !members.is_empty(),
            members: members
                .iter()
                .map(|(n, me)| MemberView {
                    name: n.to_string(),
                    endpoint: format!("c-{}", n.to_lowercase()),
                    online: true,
                    is_me: *me,
                })
                .collect(),
            messages: Default::default(),
            invite: None,
            invite_note: None,
        }
    }

    /// 按顺序回答预先排好的答复，记下收到的请求。
    struct Script {
        answers: RefCell<Vec<Response>>,
        seen: RefCell<Vec<String>>,
    }

    impl Script {
        fn new(answers: Vec<Response>) -> Script {
            Script {
                answers: RefCell::new(answers),
                seen: RefCell::new(vec![]),
            }
        }

        fn call(&self) -> impl FnMut(Request) -> Result<Response> + '_ {
            move |req| {
                self.seen.borrow_mut().push(format!("{req:?}"));
                let mut a = self.answers.borrow_mut();
                assert!(!a.is_empty(), "多问了一句：{req:?}");
                Ok(a.remove(0))
            }
        }
    }

    fn s(b: &[u8]) -> String {
        String::from_utf8(b.to_vec()).unwrap()
    }

    #[test]
    fn the_first_login_says_this_is_the_first_computer() {
        let sc = Script::new(vec![
            Response::Mesh(view(false, &[])),
            Response::Mesh(view(true, &[("B", true)])),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(login(&mut sc.call(), &mut out, &mut err, Lang::Zh), 0);
        assert!(
            s(&out).contains("这台电脑成了「我的电脑」组的第一台"),
            "{}",
            s(&out)
        );
        assert_eq!(sc.seen.borrow().as_slice(), ["MeshStatus", "MeshLogin"]);

        // 已经有组了：不再说「第一台」。
        let sc = Script::new(vec![
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(view(true, &[("B", true)])),
        ]);
        let mut out = vec![];
        assert_eq!(login(&mut sc.call(), &mut out, &mut err, Lang::Zh), 0);
        assert!(!s(&out).contains("第一台"));
    }

    /// 昨天的守护进程还在跑：它认得 `Hello`，报的是旧协议号。每一条多电脑
    /// 命令都要先握手，说人话、退 1——不能把 `MeshStatus` 送过去换回一句
    /// 「请求解析失败：unknown variant」。
    #[test]
    fn every_mesh_command_stops_at_a_stale_daemon_with_the_restart_hint() {
        let cases: &[&[&str]] = &[
            &["login"],
            &["invite"],
            &["join"],
            &["peers"],
            &["peers", "remove", "B"],
            &["send", "B/x", "hi"],
        ];
        for argv in cases {
            let (cmd, rest) = argv.split_first().unwrap();
            let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
            let sc = Script::new(vec![Response::Hello {
                protocol: crate::proto::PROTOCOL_VERSION - 1,
            }]);
            let (mut out, mut err) = (vec![], vec![]);
            let code = dispatch(
                cmd,
                &rest,
                &mut sc.call(),
                &mut out,
                &mut err,
                Lang::Zh,
            );
            assert_eq!(code, 1, "{argv:?}");
            assert_eq!(sc.seen.borrow().as_slice(), ["Hello"], "{argv:?}");
            let e = s(&err);
            assert!(e.contains("后台服务还是旧版本"), "{argv:?}: {e}");
            assert!(e.contains("dct restart"), "{argv:?}: {e}");
            assert!(!e.contains("unknown variant"), "{e}");
        }
    }

    /// 守护进程替命令行打中转、打网关的请求，命令行都要等得比守护进程最坏
    /// 情况更久：不然在断网、中转卡死的时候，命令行报「没响应」，守护进程
    /// 那边却已经把批准签好、发出去了。
    #[test]
    fn the_cli_outwaits_the_daemon_on_every_mesh_request() {
        let all = [
            Request::MeshStatus,
            Request::MeshLogin,
            Request::MeshJoin {
                code: "482913".into(),
                name: "n".into(),
            },
            Request::MeshInvite,
            Request::MeshInviteCancel,
            Request::MeshRemove { name: "n".into() },
            Request::MeshPeers,
            Request::MeshSend {
                to: "a/b".into(),
                text: "t".into(),
                from_session: None,
            },
        ];
        for req in &all {
            let worst = crate::mesh::worst_case(req).unwrap_or_else(|| panic!("{req:?}"));
            let wait = wait_for(req);
            assert!(
                wait >= worst + Duration::from_secs(10),
                "{req:?}: 命令行等 {wait:?}，守护进程最坏要 {worst:?}"
            );
        }
        assert_eq!(crate::mesh::worst_case(&Request::Hello), None);
        assert_eq!(wait_for(&Request::Hello), crate::client::READ_TIMEOUT);
    }

    /// I1：别的电脑报的名字（加入请求、名单、详情）印到终端之前洗干净：
    /// 一个 `\x1b[8m` 就能把后面那行数字藏起来。
    #[test]
    fn names_from_other_computers_are_cleaned_before_printing() {
        let evil = "W\x1b[8m\u{202e}in";
        let mut v = view(true, &[("B", true)]);
        v.members.push(crate::proto::MemberView {
            name: evil.into(),
            endpoint: "c-e".into(),
            online: false,
            is_me: false,
        });
        let sc = Script::new(vec![
            Response::Mesh(v),
            Response::Error(ErrorCode::DaemonNotResponding),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(
            peers(
                &mut sc.call(),
                &mut out,
                &mut err,
                Lang::Zh,
                &[]
            ),
            0
        );
        let o = s(&out);
        assert!(
            !o.chars()
                .any(|c| (c.is_control() && c != '\n') || crate::mesh::deliver::is_format_char(c)),
            "{o:?}"
        );
        assert!(o.contains("Win"), "{o}");

        // 详情那一路也一样。
        let mut v = view(true, &[("B", true)]);
        v.members[0].is_me = true;
        let list = vec![PeerView {
            name: evil.into(),
            online: true,
            os: "linux".into(),
            sessions: vec![],
            tentacles: vec![],
        }];
        let sc = Script::new(vec![Response::Mesh(v), Response::MeshPeers(list)]);
        let (mut out, mut err) = (vec![], vec![]);
        peers(
            &mut sc.call(),
            &mut out,
            &mut err,
            Lang::Zh,
            &[],
        );
        let o = s(&out);
        assert!(!o.contains('\x1b') && !o.contains('\u{202e}'), "{o:?}");

        // `dct join` 列组里有谁的那一行。
        let mut joined = view(true, &[("B", true)]);
        joined.members.push(crate::proto::MemberView {
            name: evil.into(),
            endpoint: "c-e".into(),
            online: true,
            is_me: false,
        });
        let sc = Script::new(vec![Response::Mesh(joined)]);
        let (mut out, mut err) = (vec![], vec![]);
        let o = JoinOpts {
            code: "482913".into(),
            name: String::new(),
        };
        assert_eq!(join(&mut sc.call(), &mut out, &mut err, Lang::Zh, &o), 0);
        let o = s(&out);
        assert!(!o.contains('\x1b') && !o.contains('\u{202e}'), "{o:?}");
    }

    /// 老到连 `Hello` 都不认得的守护进程：答不上来本身就是答案。
    #[test]
    fn a_daemon_that_cannot_answer_hello_counts_as_stale() {
        let mut call = |_: Request| -> Result<Response> {
            Err(anyhow::anyhow!("请求解析失败：unknown variant `Hello`"))
        };
        let (mut out, mut err) = (vec![], vec![]);
        let code = dispatch(
            "login",
            &[],
            &mut call,
            &mut out,
            &mut err,
            Lang::Zh,
        );
        assert_eq!(code, 1);
        assert!(s(&err).contains("dct restart"), "{}", s(&err));
    }

    #[test]
    fn a_current_daemon_passes_the_handshake() {
        let sc = Script::new(vec![
            Response::Hello {
                protocol: crate::proto::PROTOCOL_VERSION,
            },
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(view(true, &[("B", true)])),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = dispatch(
            "login",
            &[],
            &mut sc.call(),
            &mut out,
            &mut err,
            Lang::Zh,
        );
        assert_eq!(code, 0, "{}", s(&err));
        assert_eq!(
            sc.seen.borrow().as_slice(),
            ["Hello", "MeshStatus", "MeshLogin"]
        );
    }

    #[test]
    fn login_without_a_dc_account_exits_1_with_the_hint() {
        let sc = Script::new(vec![
            Response::Mesh(view(false, &[])),
            Response::Error(ErrorCode::Mesh(MeshProblem::NoDcAccount)),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(login(&mut sc.call(), &mut out, &mut err, Lang::Zh), 1);
        assert_eq!(
            s(&err).trim(),
            "先在 dct 里用 DC 配对账号（进 dct 按 c 选 DC）"
        );
    }

    /// 详情问不成（旧守护进程、出错）：退回名单那几行；不再有「谁在等批准」。
    #[test]
    fn peers_falls_back_to_the_member_lines() {
        let v = view(true, &[("B", true), ("A", false)]);
        let sc = Script::new(vec![
            Response::Mesh(v),
            Response::Error(ErrorCode::DaemonNotResponding),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(peers(&mut sc.call(), &mut out, &mut err, Lang::Zh, &[]), 0);
        let o = s(&out);
        assert!(o.contains("  B  (这台)"), "{o}");
        assert!(o.contains("  A  (在线)"), "{o}");
        assert!(!o.contains("approve"), "{o}");
    }

    /// `dct peers approve` 没有了：当成用法不对。
    #[test]
    fn peers_approve_is_gone() {
        let sc = Script::new(vec![]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = peers(&mut sc.call(), &mut out, &mut err, Lang::Zh, &args(&["approve", "B"]));
        assert_eq!(code, 2);
        assert_eq!(s(&err).trim(), "用法：dct peers | dct peers remove <电脑名>");
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// `dct peers` 的详情：每台一行带系统，在线的下面列会话和触手，不在线
    /// 的只有名字。
    #[test]
    fn peers_shows_sessions_and_tentacles_of_each_machine() {
        use crate::proto::SessionBrief;
        let v = view(
            true,
            &[("B", true), ("公司Windows", false), ("老笔记本", false)],
        );
        let list = vec![
            PeerView {
                name: "B".into(),
                online: true,
                os: "macos".into(),
                sessions: vec![],
                tentacles: vec![],
            },
            PeerView {
                name: "公司Windows".into(),
                online: true,
                os: "windows".into(),
                sessions: vec![SessionBrief {
                    name: "dc-terminal".into(),
                    state: "闲".into(),
                    dir: r"C:\w\dc-terminal".into(),
                }],
                tentacles: vec!["iPhone 镜像".into(), "摄像头".into()],
            },
            PeerView {
                name: "老笔记本".into(),
                online: false,
                os: String::new(),
                sessions: vec![],
                tentacles: vec![],
            },
        ];
        let sc = Script::new(vec![Response::Mesh(v), Response::MeshPeers(list)]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = peers(
            &mut sc.call(),
            &mut out,
            &mut err,
            Lang::Zh,
            &[],
        );
        assert_eq!(code, 0);
        assert_eq!(
            s(&out),
            "组里的电脑：\n  B  (这台)  macos\n    没有开着的会话\n  公司Windows  (在线)  windows\n    dc-terminal  闲  C:\\w\\dc-terminal\n    触手：iPhone 镜像、摄像头\n  老笔记本  (不在线)\n"
        );
        assert_eq!(sc.seen.borrow().as_slice(), ["MeshStatus", "MeshPeers"]);
    }

    fn send_with(
        outcome: Response,
        args: &[&str],
        from: Option<u32>,
    ) -> (i32, String, String, Vec<String>) {
        let sc = Script::new(vec![outcome]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = send(
            &mut sc.call(),
            &mut out,
            &mut err,
            Lang::Zh,
            &args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            from,
        );
        let seen = sc.seen.borrow().clone();
        (code, s(&out), s(&err), seen)
    }

    #[test]
    fn send_says_what_happened_and_exits_0_only_when_delivered_or_queued() {
        let to = "公司Windows/dc-terminal";
        let (code, out, _, seen) = send_with(
            Response::MeshSent(SendOutcome::Delivered),
            &[to, "跑一下测试"],
            Some(3),
        );
        assert_eq!((code, out.trim()), (0, "已送到 公司Windows/dc-terminal"));
        assert_eq!(
            seen,
            [r#"MeshSend { to: "公司Windows/dc-terminal", text_chars: 5, from_session: Some(3) }"#]
        );

        let (code, out, _, _) =
            send_with(Response::MeshSent(SendOutcome::Queued), &[to, "x"], None);
        assert_eq!((code, out.trim()), (0, "对方正忙，已排队，忙完就送进去"));

        let cases: Vec<(SendOutcome, &str)> = vec![
            (
                SendOutcome::Offline,
                "公司Windows 现在不在线，没送出去（离线留言下一步才做）",
            ),
            (SendOutcome::NoSuchMachine, "组里没有叫 公司Windows 的电脑"),
            (
                SendOutcome::NoSuchSession(vec![]),
                "公司Windows 上没有叫 dc-terminal 的会话。dct peers 能看到那边开着哪些会话",
            ),
            (
                SendOutcome::NoSuchSession(vec!["#3 claude（dc-terminal）".into(), "#4 写文档（dc-terminal）".into()]),
                "公司Windows 上叫 dc-terminal 的会话不止一个，改用编号指明，比如 公司Windows/#3：\n  #3 claude（dc-terminal）\n  #4 写文档（dc-terminal）",
            ),
            (
                SendOutcome::SessionStopped,
                "公司Windows/dc-terminal 这个会话已经停了，没送进去",
            ),
            (
                SendOutcome::Refused,
                "公司Windows 没收下这条留言（那边排队满了，或者那不是智能体会话）",
            ),
            (
                SendOutcome::NoAnswer,
                "公司Windows 没回话，不知道送到没有。可以用 dct peers 看看那边",
            ),
        ];
        for (o, want) in cases {
            let (code, out, err, _) = send_with(Response::MeshSent(o.clone()), &[to, "x"], None);
            assert_eq!(code, 1, "{o:?}");
            assert!(out.is_empty(), "{o:?}");
            assert_eq!(err.trim(), want, "{o:?}");
        }
    }

    #[test]
    fn send_too_long_and_usage() {
        let (code, _, err, _) = send_with(
            Response::Error(ErrorCode::Mesh(MeshProblem::TooLong)),
            &["A/b", "x"],
            None,
        );
        assert_eq!(
            (code, err.trim()),
            (1, "太长了，请缩短或者改成派活（下一步）")
        );

        // 没有内容、没有地址：不问守护进程，退 2。
        for args in [&[][..], &["A/b"][..], &["A/b", "  "][..]] {
            let sc = Script::new(vec![]);
            let (mut out, mut err) = (vec![], vec![]);
            let a: Vec<String> = args.iter().map(|x| x.to_string()).collect();
            assert_eq!(
                send(&mut sc.call(), &mut out, &mut err, Lang::Zh, &a, None),
                2
            );
            assert!(s(&err).contains("用法：dct send"));
        }

        // 没加引号的几个词拼成一句。
        let (_, _, _, seen) = send_with(
            Response::MeshSent(SendOutcome::Delivered),
            &["A/b", "跑", "一下"],
            None,
        );
        assert!(seen[0].contains("text_chars: 4"), "{seen:?}");
    }

    #[test]
    fn peers_remove_and_usage() {
        let (mut out, mut err) = (vec![], vec![]);
        let sc = Script::new(vec![Response::Mesh(view(true, &[("B", true)]))]);
        peers(
            &mut sc.call(),
            &mut out,
            &mut err,
            Lang::Zh,
            &args(&["remove", "C"]),
        );
        assert!(s(&out).contains("C 已移出"));

        let sc = Script::new(vec![Response::Error(ErrorCode::Mesh(
            MeshProblem::CannotRemoveSelf,
        ))]);
        err.clear();
        assert_eq!(
            peers(
                &mut sc.call(),
                &mut out,
                &mut err,
                Lang::Zh,
                &args(&["remove", "B"])
            ),
            1
        );
        assert!(s(&err).contains("不能移除这台电脑自己"));

        let sc = Script::new(vec![]);
        assert_eq!(
            peers(
                &mut sc.call(),
                &mut out,
                &mut err,
                Lang::Zh,
                &args(&["frob"])
            ),
            2
        );
    }

    #[test]
    fn join_arguments() {
        let a = |v: &[&str]| parse_join(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let j = |code: &str, name: &str| {
            Some(JoinOpts {
                code: code.into(),
                name: name.into(),
            })
        };
        assert_eq!(a(&["482913"]), j("482913", ""));
        assert_eq!(a(&["482", "913"]), j("482913", ""), "shell 把空格拆开了");
        assert_eq!(a(&["482-913"]), j("482-913", ""));
        assert_eq!(a(&["012345"]), j("012345", ""), "前导 0 原样");
        assert_eq!(a(&["482913", "--name", "公司电脑"]), j("482913", "公司电脑"));
        assert_eq!(a(&["--name=家里", "482913"]), j("482913", "家里"));
        assert_eq!(a(&[]), None, "没有码");
        assert_eq!(a(&["--name", "N"]), None, "只有名字没有码");
        assert_eq!(a(&["482913", "--confirm", "A"]), None, "--confirm 没有了");
        assert_eq!(a(&["482913", "--name"]), None);
    }

    /// 送给守护进程的就是用户敲的码和名字；进组之后报组里有谁。
    #[test]
    fn join_sends_the_code_and_says_who_is_in_the_group() {
        let mut v = view(true, &[("Mac", false), ("公司电脑", true)]);
        v.name = "公司电脑".into();
        let sc = Script::new(vec![Response::Mesh(v)]);
        let (mut out, mut err) = (vec![], vec![]);
        let o = JoinOpts {
            code: "482 913".into(),
            name: "公司电脑".into(),
        };
        assert_eq!(join(&mut sc.call(), &mut out, &mut err, Lang::Zh, &o), 0);
        assert_eq!(
            sc.seen.borrow().as_slice(),
            [r#"MeshJoin { code: "******", name: "公司电脑" }"#]
        );
        assert_eq!(s(&out).trim(), "已加入「我的电脑」，组里有：Mac、公司电脑");
    }

    #[test]
    fn join_tells_the_new_name_when_the_inviter_numbered_it() {
        let mut v = view(true, &[("Mac", false), ("Mac 2", true)]);
        v.name = "Mac 2".into();
        let sc = Script::new(vec![Response::Mesh(v)]);
        let (mut out, mut err) = (vec![], vec![]);
        let o = JoinOpts {
            code: "482913".into(),
            name: "Mac".into(),
        };
        assert_eq!(join(&mut sc.call(), &mut out, &mut err, Lang::Zh, &o), 0);
        assert!(s(&out).contains("组里已经有一台叫这个名字的了，这台电脑现在叫 Mac 2"), "{}", s(&out));
    }

    #[test]
    fn join_errors_are_said_in_words_and_exit_1() {
        for (p, want) in [
            (MeshProblem::WrongInviteCode, "邀请码不对或已作废，请在老电脑上按 a 重新生成"),
            (MeshProblem::BadInviteCode, "邀请码是 6 位数字，比如：dct join 482913"),
            (MeshProblem::NoDcAccount, "先在 dct 里用 DC 配对账号（进 dct 按 c 选 DC）"),
        ] {
            let sc = Script::new(vec![Response::Error(ErrorCode::Mesh(p))]);
            let (mut out, mut err) = (vec![], vec![]);
            let o = JoinOpts {
                code: "1".into(),
                name: String::new(),
            };
            assert_eq!(join(&mut sc.call(), &mut out, &mut err, Lang::Zh, &o), 1);
            assert_eq!(s(&err).trim(), want);
            assert!(out.is_empty());
        }
    }

    #[test]
    fn join_without_a_code_prints_the_usage() {
        let sc = Script::new(vec![Response::Hello {
            protocol: crate::proto::PROTOCOL_VERSION,
        }]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = dispatch("join", &[], &mut sc.call(), &mut out, &mut err, Lang::Zh);
        assert_eq!(code, 2);
        assert!(s(&err).contains("dct join <邀请码>"), "{}", s(&err));
    }

    fn invite_view(id: u64, code: &str) -> crate::proto::InviteView {
        crate::proto::InviteView {
            id,
            code: code.into(),
            expires_at: 1_000 + 600,
        }
    }

    /// 一份带着邀请码（或者结果）的现状。
    fn with_invite(
        invite: Option<crate::proto::InviteView>,
        note: Option<(u64, crate::proto::InviteOutcome)>,
    ) -> MeshView {
        let mut v = view(true, &[("Mac", true)]);
        v.invite = invite;
        v.invite_note = note.map(|(id, outcome)| crate::proto::InviteNote { id, outcome });
        v
    }

    fn run_invite(answers: Vec<Response>, tty: bool) -> (i32, String, String, Vec<String>) {
        let sc = Script::new(answers);
        let (mut out, mut err) = (vec![], vec![]);
        let code = invite(&mut sc.call(), &mut out, &mut err, Lang::Zh, tty, &|| 1_001, &|| {});
        let seen = sc.seen.borrow().clone();
        (code, s(&out), s(&err), seen)
    }

    /// 印出码（带空格）和怎么用，等到有人用它进组：报名字、退 0。
    #[test]
    fn invite_prints_the_code_and_waits_until_someone_joins() {
        use crate::proto::InviteOutcome::Joined;
        let (code, out, err, seen) = run_invite(
            vec![
                Response::MeshInvite(invite_view(3, "012345")),
                Response::Mesh(with_invite(Some(invite_view(3, "012345")), None)),
                Response::Mesh(with_invite(
                    None,
                    Some((
                        3,
                        Joined {
                            name: "公司电脑".into(),
                        },
                    )),
                )),
            ],
            false,
        );
        assert_eq!(code, 0, "{err}");
        assert_eq!(
            out,
            "邀请码 012 345 · 10 分钟内有效\n在新电脑上运行：dct join 012345（码只能用一次）\n公司电脑 已加入\n"
        );
        assert_eq!(seen, ["MeshInvite", "MeshStatus", "MeshStatus"]);
    }

    #[test]
    fn invite_says_when_the_code_was_burned_expired_replaced_or_withdrawn() {
        use crate::proto::InviteOutcome::{Burned, Expired};
        let cases: Vec<(MeshView, &str)> = vec![
            (
                with_invite(None, Some((3, Burned))),
                "有人用错码试过一次，码已作废。要加电脑就重新生成一个（看板上按 a，或运行 dct invite）",
            ),
            (
                with_invite(None, Some((3, Expired))),
                "邀请码过期了，没人用。要加电脑就重新生成一个（看板上按 a，或运行 dct invite）",
            ),
            (
                with_invite(Some(invite_view(4, "999999")), None),
                "这个邀请码已经换成新的了（看板上又按了 a？），以新的为准",
            ),
            (with_invite(None, None), "邀请码已经收回了"),
            // 上一个码的结果不算这一个的。
            (
                with_invite(None, Some((2, Burned))),
                "邀请码已经收回了",
            ),
        ];
        for (status, want) in cases {
            let (code, _, err, _) = run_invite(
                vec![
                    Response::MeshInvite(invite_view(3, "482913")),
                    Response::Mesh(status),
                ],
                false,
            );
            assert_eq!(code, 1);
            assert_eq!(err.trim(), want);
        }
    }

    /// 终端里每秒刷一行倒计时（`\r` 覆盖）；结果另起一行。管道里不刷。
    #[test]
    fn invite_counts_down_only_on_a_terminal() {
        use crate::proto::InviteOutcome::Joined;
        let answers = || {
            vec![
                Response::MeshInvite(invite_view(3, "482913")),
                Response::Mesh(with_invite(Some(invite_view(3, "482913")), None)),
                Response::Mesh(with_invite(None, Some((3, Joined { name: "B".into() })))),
            ]
        };
        let (_, out, _, _) = run_invite(answers(), true);
        assert!(out.contains("\r还剩 9:59"), "{out:?}");
        assert!(out.ends_with("\nB 已加入\n"), "{out:?}");
        let (_, out, _, _) = run_invite(answers(), false);
        assert!(!out.contains('\r'), "{out:?}");
    }

    #[test]
    fn invite_before_login_says_to_log_in() {
        let (code, out, err, _) = run_invite(
            vec![Response::Error(ErrorCode::Mesh(MeshProblem::NotLoggedIn))],
            false,
        );
        assert_eq!(code, 1);
        assert!(out.is_empty());
        assert_eq!(err.trim(), "还没登录多电脑，先运行 dct login");
    }

    #[test]
    fn invite_takes_no_arguments() {
        let sc = Script::new(vec![Response::Hello {
            protocol: crate::proto::PROTOCOL_VERSION,
        }]);
        let (mut out, mut err) = (vec![], vec![]);
        let rest = vec!["482913".to_string()];
        let code = dispatch("invite", &rest, &mut sc.call(), &mut out, &mut err, Lang::Zh);
        assert_eq!(code, 2);
        assert!(s(&err).contains("用法：dct invite"), "{}", s(&err));
    }
}
