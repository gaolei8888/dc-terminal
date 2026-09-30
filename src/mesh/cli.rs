//! `dct login` / `dct join` / `dct peers` / `dct send`。
//!
//! 这里只是把话说给人听：真正的事（换令牌、问别的电脑、签名单）都在守护
//! 进程里做——它握着中转连接、钥匙和密钥仓，命令行这边一样也不碰。
//!
//! 跟守护进程说话的那一下（`call`）和问用户的那一下（`ask`）都是参数，
//! 于是整套对话不起守护进程、不碰终端也能测。
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use crate::client::Client;
use crate::i18n::{msg, text, Key, Lang};
use crate::proto::{
    ErrorCode, MeshProblem, MeshView, PeerView, PendingJoin, Request, Response, SendOutcome,
};

/// 守护进程替我们打网络的那几条请求（换令牌、问别的电脑、广播名单）等多久。
/// 必须比守护进程那边最坏的情况（`mesh::worst_case`）长出一截，见 `wait_for`。
const MESH_CALL_WAIT: Duration = Duration::from_secs(90);

/// 这一条请求命令行等多久：多电脑的请求等 `MESH_CALL_WAIT`，别的（握手）
/// 按本机答一句的 `READ_TIMEOUT`。
///
/// 等不够的代价不是「慢一点」：守护进程可能在命令行放弃之后才办成——批准
/// 已经签进名单、发给了每一台，屏幕上却说失败，用户会再批一次。
fn wait_for(req: &Request) -> Duration {
    match crate::mesh::worst_case(req) {
        Some(_) => MESH_CALL_WAIT,
        None => crate::client::READ_TIMEOUT,
    }
}

type Call<'a> = &'a mut dyn FnMut(Request) -> Result<Response>;
/// 把一句提示给用户看、读回他敲的一行。读不到（没有终端、EOF）是 `None`。
type Ask<'a> = &'a mut dyn FnMut(&str) -> Option<String>;

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
    let mut client = client;
    let mut call = |req: Request| {
        let wait = wait_for(&req);
        client.call_within(req, wait)
    };
    let mut ask = |prompt: &str| {
        print!("{prompt}");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim().to_string()),
        }
    };
    dispatch(cmd, rest, &mut call, &mut ask, &mut out, &mut err, lang)
}

/// 连上守护进程之后的全部：先握手，再按子命令办。
fn dispatch(
    cmd: &str,
    rest: &[String],
    call: Call,
    ask: Ask,
    out: &mut dyn Write,
    err: &mut dyn Write,
    lang: Lang,
) -> i32 {
    if !daemon_is_current(call, err, lang) {
        return 1;
    }
    match cmd {
        "login" => login(call, out, err, lang),
        "join" => match parse_join(rest) {
            Some(opts) => join(call, out, err, lang, &opts),
            None => {
                let _ = writeln!(err, "{}", msg::mesh_join_usage(lang));
                2
            }
        },
        "peers" => peers(call, ask, out, err, lang, rest),
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

/// 按名字或端点在一张列表里找一条。找不到、不止一条都是 `Err`。
fn pick<'a>(list: &'a [PendingJoin], who: &str) -> Result<&'a PendingJoin, usize> {
    let hits: Vec<&PendingJoin> = list
        .iter()
        .filter(|p| p.endpoint == who || p.name == who)
        .collect();
    match hits.as_slice() {
        [p] => Ok(p),
        _ => Err(hits.len()),
    }
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
    ask: Ask,
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
            for p in &v.pending {
                // 两台同名就只能按端点批，提示里直接给端点。
                let dup = v.pending.iter().filter(|q| q.name == p.name).count() > 1;
                let target = if dup { &p.endpoint } else { &p.name };
                let _ = writeln!(
                    out,
                    "{}",
                    msg::mesh_join_prompt(lang, &c(&p.name), &c(&p.code))
                );
                let _ = writeln!(out, "{}", msg::mesh_approve_hint(lang, &c(target)));
            }
            0
        }
        ["approve", who] | ["approve", who, "--no"] => {
            let yes = args.len() == 2;
            // 先看一眼挂着的是哪一条，把名字和数字再给用户看一遍；送去批的
            // 是**这次给他看的**端点和数字，守护进程两样都对上才批。
            let Some(v) = view_of(call, Request::MeshStatus, err, lang) else {
                return 1;
            };
            let p = match pick(&v.pending, who) {
                Ok(p) => p.clone(),
                Err(0) => {
                    say_error(
                        err,
                        lang,
                        &ErrorCode::Mesh(MeshProblem::NoSuchRequest(who.to_string())),
                    );
                    return 1;
                }
                Err(_) => {
                    say_error(
                        err,
                        lang,
                        &ErrorCode::Mesh(MeshProblem::Ambiguous(who.to_string())),
                    );
                    return 1;
                }
            };
            if yes {
                let q = format!("{} ", msg::mesh_join_prompt(lang, &c(&p.name), &c(&p.code)));
                let said_yes = ask(&q).is_some_and(|a| a.trim().eq_ignore_ascii_case("y"));
                if !said_yes {
                    let _ = writeln!(out, "{}", msg::mesh_not_approved(lang));
                    return 1;
                }
            }
            let req = Request::MeshApprove {
                endpoint: p.endpoint.clone(),
                code: p.code.clone(),
                yes,
            };
            if view_of(call, req, err, lang).is_none() {
                return 1;
            }
            if yes {
                let _ = writeln!(out, "{}", msg::mesh_member_joined(lang, &c(&p.name)));
            } else {
                let _ = writeln!(out, "{}", msg::mesh_member_refused(lang, &c(&p.name)));
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
            pending: vec![],
            joining: vec![],
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
            &["join"],
            &["peers"],
            &["peers", "approve", "B"],
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
                &mut no_ask,
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
            Request::MeshApprove {
                endpoint: "c-x".into(),
                code: "123456".into(),
                yes: true,
            },
            Request::MeshConfirmInviter {
                endpoint: "c-x".into(),
            },
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
        v.pending = vec![pj(evil, "c-w", "123456")];
        let sc = Script::new(vec![
            Response::Mesh(v),
            Response::Error(ErrorCode::DaemonNotResponding),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(
            peers(
                &mut sc.call(),
                &mut no_ask,
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
            &mut no_ask,
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
            &mut no_ask,
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
            &mut no_ask,
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

    fn pj(name: &str, ep: &str, code: &str) -> PendingJoin {
        PendingJoin {
            name: name.into(),
            endpoint: ep.into(),
            code: code.into(),
        }
    }

    fn no_ask(_: &str) -> Option<String> {
        panic!("不该问用户")
    }

    #[test]
    fn peers_lists_members_and_asks_about_pending_joins() {
        let mut v = view(true, &[("B", true), ("A", false)]);
        v.pending = vec![pj("公司Windows", "c-w", "123456")];
        // 详情问不成（旧守护进程、出错）：退回名单那几行。
        let sc = Script::new(vec![
            Response::Mesh(v),
            Response::Error(ErrorCode::DaemonNotResponding),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(
            peers(
                &mut sc.call(),
                &mut no_ask,
                &mut out,
                &mut err,
                Lang::Zh,
                &[]
            ),
            0
        );
        let o = s(&out);
        assert!(o.contains("  B  (这台)"), "{o}");
        assert!(o.contains(
            "一台叫 公司Windows 的电脑想加入「我的电脑」。它屏幕上的数字是 123456 吗？(y/n)"
        ));
        assert!(o.contains("dct peers approve 公司Windows"), "{o}");
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// `approve <名字>`：再给用户看一遍名字和数字，答 y 才送；送的是这次
    /// 给他看的那条的端点和数字。
    #[test]
    fn peers_approve_reshows_the_code_and_sends_endpoint_and_code() {
        let mut v = view(true, &[("B", true)]);
        v.pending = vec![pj("C", "c-c", "123456")];
        let sc = Script::new(vec![
            Response::Mesh(v.clone()),
            Response::Mesh(view(true, &[("B", true), ("C", false)])),
        ]);
        let shown = RefCell::new(String::new());
        let mut ask = |p: &str| {
            *shown.borrow_mut() = p.to_string();
            Some("y".to_string())
        };
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(
            peers(
                &mut sc.call(),
                &mut ask,
                &mut out,
                &mut err,
                Lang::Zh,
                &args(&["approve", "C"])
            ),
            0
        );
        assert!(shown.borrow().contains("一台叫 C 的电脑") && shown.borrow().contains("123456"));
        assert_eq!(
            sc.seen.borrow()[1],
            r#"MeshApprove { endpoint: "c-c", code: "123456", yes: true }"#
        );
        assert!(s(&out).contains("C 已加入"));

        // 答 n：什么都不送。
        let sc = Script::new(vec![Response::Mesh(v.clone())]);
        let mut ask = |_: &str| Some("n".to_string());
        out.clear();
        assert_eq!(
            peers(
                &mut sc.call(),
                &mut ask,
                &mut out,
                &mut err,
                Lang::Zh,
                &args(&["approve", "C"])
            ),
            1
        );
        assert_eq!(sc.seen.borrow().len(), 1);
        assert!(s(&out).contains("没有批准"));

        // --no：不问，直接拒。
        let sc = Script::new(vec![
            Response::Mesh(v.clone()),
            Response::Mesh(view(true, &[("B", true)])),
        ]);
        out.clear();
        peers(
            &mut sc.call(),
            &mut no_ask,
            &mut out,
            &mut err,
            Lang::Zh,
            &args(&["approve", "c-c", "--no"]),
        );
        assert!(sc.seen.borrow()[1].contains("yes: false"));
        assert!(s(&out).contains("已拒绝 C"));
    }

    #[test]
    fn peers_approve_with_two_of_the_same_name_asks_for_the_endpoint() {
        let mut v = view(true, &[("B", true)]);
        v.pending = vec![pj("C", "c-c", "1"), pj("C", "c-x", "2")];
        let sc = Script::new(vec![Response::Mesh(v)]);
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(
            peers(
                &mut sc.call(),
                &mut no_ask,
                &mut out,
                &mut err,
                Lang::Zh,
                &args(&["approve", "C"])
            ),
            1
        );
        assert!(s(&err).contains("不止一台叫 C"));
        assert_eq!(sc.seen.borrow().len(), 1);
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
            &mut no_ask,
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
            &mut no_ask,
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
                &mut no_ask,
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
                &mut no_ask,
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
        let code = dispatch("join", &[], &mut sc.call(), &mut no_ask, &mut out, &mut err, Lang::Zh);
        assert_eq!(code, 2);
        assert!(s(&err).contains("dct join <邀请码>"), "{}", s(&err));
    }
}
