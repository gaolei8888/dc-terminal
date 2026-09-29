//! `dct login` / `dct join` / `dct peers` / `dct send`。
//!
//! 这里只是把话说给人听：真正的事（换令牌、问别的电脑、签名单）都在守护
//! 进程里做——它握着中转连接、钥匙和密钥仓，命令行这边一样也不碰。
//!
//! 跟守护进程说话的那一下（`call`）和问用户的那一下（`ask`）都是参数，
//! 于是整套对话不起守护进程、不碰终端也能测。
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::client::Client;
use crate::i18n::{msg, text, Key, Lang};
use crate::proto::{
    ErrorCode, MeshProblem, MeshView, PeerView, PendingJoin, Request, Response, SendOutcome,
};

/// `dct join` 最多等多久有人点同意。跟对面那条请求的有效期一样长。
const JOIN_WAIT: Duration = crate::mesh::JOIN_TTL;
/// 等同意时多久问一次守护进程。
const JOIN_POLL: Duration = Duration::from_secs(1);
/// 守护进程替我们打网络的那几条请求（换令牌、问别的电脑）等多久。
const SLOW_CALL: Duration = Duration::from_secs(40);

type Call<'a> = &'a mut dyn FnMut(Request) -> Result<Response>;
/// 把一句提示给用户看、读回他敲的一行。读不到（没有终端、EOF）是 `None`。
type Ask<'a> = &'a mut dyn FnMut(&str) -> Option<String>;

/// `dct join` 的参数。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct JoinOpts {
    /// 顺手改名；空 = 不改。
    pub name: String,
    /// 不问了，直接认定这一台（脚本用）。
    pub confirm: Option<String>,
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
        let slow = matches!(
            req,
            Request::MeshLogin
                | Request::MeshJoin { .. }
                | Request::MeshPeers
                | Request::MeshSend { .. }
        );
        if slow {
            client.call_within(req, SLOW_CALL)
        } else {
            client.call(req)
        }
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
    match cmd {
        "login" => login(&mut call, &mut out, &mut err, lang),
        "join" => match parse_join(rest) {
            Some(opts) => join(
                &mut call,
                &mut ask,
                &mut out,
                &mut err,
                lang,
                &opts,
                JOIN_WAIT,
                &|| std::thread::sleep(JOIN_POLL),
            ),
            None => {
                let _ = writeln!(err, "{}", msg::mesh_join_usage(lang));
                2
            }
        },
        "peers" => peers(&mut call, &mut ask, &mut out, &mut err, lang, rest),
        "send" => {
            // 在 dct 的会话里跑的，守护进程给子进程设过这个变量（`session.rs`）。
            let from = std::env::var(crate::session::SESSION_ID_ENV)
                .ok()
                .and_then(|v| v.trim().parse().ok());
            send(&mut call, &mut out, &mut err, lang, rest, from)
        }
        _ => 2,
    }
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

/// `--name X` / `--name=X`、`--confirm X` / `--confirm=X`，都可以不给。
/// `None` = 用法不对。
fn parse_join(args: &[String]) -> Option<JoinOpts> {
    let mut opts = JoinOpts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix("--name=") {
            opts.name = v.to_string();
        } else if let Some(v) = a.strip_prefix("--confirm=") {
            opts.confirm = Some(v.to_string());
        } else if a == "--name" {
            opts.name = it.next()?.clone();
        } else if a == "--confirm" {
            opts.confirm = Some(it.next()?.clone());
        } else {
            return None;
        }
    }
    Some(opts)
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
    let _ = writeln!(out, "{}", msg::mesh_logged_in(lang, &after.name));
    0
}

/// 已经跟别的电脑同组了吗（名单上不止我自己）。
fn grouped(v: &MeshView) -> bool {
    v.members.iter().any(|m| !m.is_me)
}

/// 新电脑上：问一遍已有的电脑，列出每台的数字，**让用户说出是哪一台**
/// （或者 `--confirm`），告诉守护进程只认那一台，然后等它的名单。
///
/// 不替用户挑：回话的电脑可能不止一台，其中哪台是中转塞进来的，只有看过
/// 两块屏幕的人知道。只有一台回话也一样要他说一声——那一台同样可能是塞
/// 进来的。
#[allow(clippy::too_many_arguments)]
pub(crate) fn join(
    call: Call,
    ask: Ask,
    out: &mut dyn Write,
    err: &mut dyn Write,
    lang: Lang,
    opts: &JoinOpts,
    wait: Duration,
    pause: &dyn Fn(),
) -> i32 {
    let Some(now) = view_of(call, Request::MeshStatus, err, lang) else {
        return 1;
    };
    if !now.logged_in {
        say_error(err, lang, &ErrorCode::Mesh(MeshProblem::NotLoggedIn));
        return 1;
    }
    // 邀请的有效期从守护进程问到那几台电脑的那一刻算起（`Mesh::set_invites`），
    // 等同意的期限也从这里算——不从认定之后才算，不然用户在提示那儿想一会儿，
    // 这边就会接着等一段那边早已不收的时间，最后只报一句笼统的超时。
    let deadline = Instant::now() + wait;
    let req = Request::MeshJoin {
        name: opts.name.clone(),
    };
    let Some(asked) = view_of(call, req, err, lang) else {
        return 1;
    };
    // 同名的不止一台时，名字认不出是哪台：把端点印在旁边，用户照着敲。
    let line = |j: &PendingJoin| {
        let dup = asked.joining.iter().filter(|x| x.name == j.name).count() > 1;
        if dup {
            msg::mesh_code_line_with_endpoint(lang, &j.name, &j.endpoint, &j.code)
        } else {
            msg::mesh_code_line(lang, &j.name, &j.code)
        }
    };
    match asked.joining.as_slice() {
        [one] => {
            let _ = writeln!(out, "{}", msg::mesh_compare_codes(lang, Some(&one.code)));
            let _ = writeln!(out, "{}", line(one));
        }
        many => {
            let _ = writeln!(out, "{}", msg::mesh_compare_codes(lang, None));
            for j in many {
                let _ = writeln!(out, "{}", line(j));
            }
        }
    }
    let _ = out.flush();

    let choice = match &opts.confirm {
        Some(c) => Some(c.clone()),
        None => ask(&msg::mesh_which_computer(lang)),
    };
    let choice = choice.map(|c| c.trim().to_string()).unwrap_or_default();
    if choice.is_empty() {
        let _ = writeln!(err, "{}", msg::mesh_join_cancelled(lang));
        return 1;
    }
    let inviter = match pick(&asked.joining, &choice) {
        Ok(p) => p.clone(),
        Err(0) => {
            let _ = writeln!(err, "{}", msg::mesh_not_a_responder(lang, &choice));
            return 1;
        }
        Err(_) => {
            let _ = writeln!(err, "{}", msg::mesh_ambiguous_responder(lang, &choice));
            return 1;
        }
    };
    let req = Request::MeshConfirmInviter {
        endpoint: inviter.endpoint.clone(),
    };
    let Some(v) = view_of(call, req, err, lang) else {
        return 1;
    };
    // 那边可能已经点过同意：认定的那一刻就进组了。
    if grouped(&v) {
        let _ = writeln!(out, "{}", msg::mesh_joined_group(lang));
        return 0;
    }
    let _ = writeln!(out, "{}", msg::mesh_waiting_for_approval(lang));
    let _ = out.flush();

    loop {
        pause();
        if let Some(v) = view_of(call, Request::MeshStatus, err, lang) {
            if grouped(&v) {
                let _ = writeln!(out, "{}", msg::mesh_joined_group(lang));
                return 0;
            }
            // 守护进程已经把这次邀请作废了（过了期限，或者别处又跑了一次
            // `dct join`）：再等也收不到，现在就说。
            if v.joining.is_empty() {
                let _ = writeln!(err, "{}", msg::mesh_join_timed_out(lang));
                return 1;
            }
        }
        if Instant::now() >= deadline {
            let _ = writeln!(err, "{}", msg::mesh_join_timed_out(lang));
            return 1;
        }
    }
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
                            msg::mesh_member_line(lang, &m.name, m.online, m.is_me)
                        );
                    }
                }
            }
            for p in &v.pending {
                // 两台同名就只能按端点批，提示里直接给端点。
                let dup = v.pending.iter().filter(|q| q.name == p.name).count() > 1;
                let target = if dup { &p.endpoint } else { &p.name };
                let _ = writeln!(out, "{}", msg::mesh_join_prompt(lang, &p.name, &p.code));
                let _ = writeln!(out, "{}", msg::mesh_approve_hint(lang, target));
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
                let q = format!("{} ", msg::mesh_join_prompt(lang, &p.name, &p.code));
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
                let _ = writeln!(out, "{}", msg::mesh_member_joined(lang, &p.name));
            } else {
                let _ = writeln!(out, "{}", msg::mesh_member_refused(lang, &p.name));
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
            msg::mesh_peer_line(lang, &p.name, p.online, is_me, &p.os)
        );
        if !p.online {
            continue;
        }
        if p.sessions.is_empty() {
            let _ = writeln!(out, "{}", msg::mesh_no_sessions(lang));
        }
        for s in &p.sessions {
            let _ = writeln!(out, "    {}  {}  {}", s.name, s.state, s.dir);
        }
        if !p.tentacles.is_empty() {
            let _ = writeln!(out, "{}", msg::mesh_tentacles_line(lang, &p.tentacles));
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

    fn opts(confirm: Option<&str>) -> JoinOpts {
        JoinOpts {
            name: String::new(),
            confirm: confirm.map(str::to_string),
        }
    }

    /// 两台回了话：逐台列数字，问是哪一台，用户说「家里Mac」，送去认定的
    /// 就是它的端点；然后等到名单。
    #[test]
    fn join_lists_codes_asks_which_computer_and_confirms_that_one() {
        let mut asked = view(true, &[("B", true)]);
        asked.joining = vec![
            pj("家里Mac", "c-a", "111111"),
            pj("办公室", "c-c", "222222"),
        ];
        let sc = Script::new(vec![
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(asked.clone()),
            Response::Mesh(asked.clone()),
            Response::Mesh(asked),
            Response::Mesh(view(true, &[("B", true), ("家里Mac", false)])),
        ]);
        let prompts = RefCell::new(Vec::<String>::new());
        let mut ask = |p: &str| {
            prompts.borrow_mut().push(p.to_string());
            Some("办公室".to_string())
        };
        let (mut out, mut err) = (vec![], vec![]);
        let o = JoinOpts {
            name: "公司Windows".into(),
            confirm: None,
        };
        let code = join(
            &mut sc.call(),
            &mut ask,
            &mut out,
            &mut err,
            Lang::Zh,
            &o,
            Duration::from_secs(60),
            &|| {},
        );
        assert_eq!(code, 0, "{}", s(&err));
        let o = s(&out);
        assert!(o.contains("  家里Mac：111111"), "{o}");
        assert!(o.contains("  办公室：222222"), "{o}");
        assert!(o.trim_end().ends_with("已加入"), "{o}");
        assert_eq!(prompts.borrow().len(), 1);
        assert!(prompts.borrow()[0].contains("哪一台电脑"));
        let seen = sc.seen.borrow();
        assert!(seen[1].contains("公司Windows"));
        assert_eq!(seen[2], r#"MeshConfirmInviter { endpoint: "c-c" }"#);
    }

    #[test]
    fn join_with_one_computer_puts_the_code_in_the_sentence_and_still_asks() {
        let mut asked = view(true, &[("B", true)]);
        asked.joining = vec![pj("A", "c-a", "654321")];
        let sc = Script::new(vec![
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(asked),
            // 那边已经点过同意：认定那一刻就进组了，不再等。
            Response::Mesh(view(true, &[("B", true), ("A", false)])),
        ]);
        let asked_user = RefCell::new(false);
        let mut ask = |_: &str| {
            *asked_user.borrow_mut() = true;
            Some("A".to_string())
        };
        let (mut out, mut err) = (vec![], vec![]);
        let code = join(
            &mut sc.call(),
            &mut ask,
            &mut out,
            &mut err,
            Lang::Zh,
            &opts(None),
            Duration::from_secs(60),
            &|| panic!("不该再等"),
        );
        assert_eq!(code, 0);
        assert!(*asked_user.borrow(), "只有一台也要用户说一声");
        assert!(s(&out).contains(
            "请在你已有的任意一台电脑上看一眼：那边会显示一个 6 位数，跟这里的 654321 一样就点同意"
        ));
    }

    /// `--confirm` 不问用户，直接认定。
    #[test]
    fn join_confirm_flag_skips_the_question() {
        let mut asked = view(true, &[("B", true)]);
        asked.joining = vec![pj("A", "c-a", "1"), pj("M", "c-m", "2")];
        let sc = Script::new(vec![
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(asked),
            Response::Mesh(view(true, &[("B", true), ("A", false)])),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = join(
            &mut sc.call(),
            &mut no_ask,
            &mut out,
            &mut err,
            Lang::Zh,
            &opts(Some("A")),
            Duration::from_secs(60),
            &|| {},
        );
        assert_eq!(code, 0);
        assert_eq!(
            sc.seen.borrow()[2],
            r#"MeshConfirmInviter { endpoint: "c-a" }"#
        );
    }

    /// 数字都对不上（直接回车）、说了一台没回过话的：不认定任何一台，退 1。
    #[test]
    fn join_without_a_matching_computer_confirms_nothing() {
        for (answer, want) in [
            (Some(""), "没有加入"),
            (None, "没有加入"),
            (Some("Z"), "没有叫 Z 的电脑回应过这次加入"),
        ] {
            let mut asked = view(true, &[("B", true)]);
            asked.joining = vec![pj("A", "c-a", "1")];
            let sc = Script::new(vec![
                Response::Mesh(view(true, &[("B", true)])),
                Response::Mesh(asked),
            ]);
            let mut ask = |_: &str| answer.map(str::to_string);
            let (mut out, mut err) = (vec![], vec![]);
            let code = join(
                &mut sc.call(),
                &mut ask,
                &mut out,
                &mut err,
                Lang::Zh,
                &opts(None),
                Duration::from_secs(60),
                &|| {},
            );
            assert_eq!(code, 1);
            assert!(s(&err).contains(want), "{answer:?}: {}", s(&err));
            assert_eq!(sc.seen.borrow().len(), 2, "没送认定");
        }
    }

    #[test]
    fn join_gives_up_after_the_wait() {
        let mut asked = view(true, &[("B", true)]);
        asked.joining = vec![pj("A", "c-a", "1")];
        let sc = Script::new(vec![
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(asked.clone()),
            Response::Mesh(asked.clone()),
            Response::Mesh(asked),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = join(
            &mut sc.call(),
            &mut no_ask,
            &mut out,
            &mut err,
            Lang::Zh,
            &opts(Some("A")),
            Duration::ZERO,
            &|| {},
        );
        assert_eq!(code, 1);
        assert_eq!(
            s(&err).trim(),
            "10 分钟里没等到同意，邀请已过期，请重新运行 dct join"
        );
    }

    /// 审查给的 m3：等同意的期限从问到那几台电脑时算起，跟邀请的有效期
    /// 同一个起点。用户在「哪一台」那儿想了比期限还久，认定之后只再看一眼
    /// 就该说过期，不该再接着等一整段。
    #[test]
    fn the_join_wait_starts_when_the_invites_were_made_not_after_confirming() {
        let mut asked = view(true, &[("B", true)]);
        asked.joining = vec![pj("A", "c-a", "1")];
        let sc = Script::new(vec![
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(asked.clone()),
            Response::Mesh(asked.clone()),
            // 只准再问这一次：从认定那一刻才起算的话，这里会接着问下去。
            Response::Mesh(asked),
        ]);
        let mut slow_user = |_: &str| {
            std::thread::sleep(Duration::from_millis(60));
            Some("A".to_string())
        };
        let (mut out, mut err) = (vec![], vec![]);
        let code = join(
            &mut sc.call(),
            &mut slow_user,
            &mut out,
            &mut err,
            Lang::Zh,
            &opts(None),
            Duration::from_millis(30),
            &|| {},
        );
        assert_eq!(code, 1);
        assert!(s(&err).contains("邀请已过期"), "{}", s(&err));
        assert_eq!(sc.seen.borrow().len(), 4);
    }

    /// 守护进程那边邀请已经作废了（`joining` 空了、也没进组）：马上说过期，
    /// 不等到期限。
    #[test]
    fn join_says_expired_as_soon_as_the_daemon_drops_the_invite() {
        let mut asked = view(true, &[("B", true)]);
        asked.joining = vec![pj("A", "c-a", "1")];
        let sc = Script::new(vec![
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(asked.clone()),
            Response::Mesh(asked),
            Response::Mesh(view(true, &[("B", true)])),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = join(
            &mut sc.call(),
            &mut no_ask,
            &mut out,
            &mut err,
            Lang::Zh,
            &opts(Some("A")),
            Duration::from_secs(600),
            &|| {},
        );
        assert_eq!(code, 1);
        assert!(
            s(&err).contains("邀请已过期，请重新运行 dct join"),
            "{}",
            s(&err)
        );
    }

    /// 审查给的 m2：两台同名都回了话。每行名字旁边印出端点，用户照着敲
    /// 端点就能认定；只敲名字的话，说清楚是回话的同名、该敲什么。
    #[test]
    fn join_with_two_responders_of_the_same_name_shows_their_endpoints() {
        let mut asked = view(true, &[("B", true)]);
        asked.joining = vec![
            pj("A", "c-a1", "111111"),
            pj("A", "c-a2", "222222"),
            pj("C", "c-c", "333333"),
        ];
        let sc = Script::new(vec![
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(asked.clone()),
            Response::Mesh(view(true, &[("B", true), ("A", false)])),
        ]);
        let mut ask = |_: &str| Some("c-a2".to_string());
        let (mut out, mut err) = (vec![], vec![]);
        let code = join(
            &mut sc.call(),
            &mut ask,
            &mut out,
            &mut err,
            Lang::Zh,
            &opts(None),
            Duration::from_secs(60),
            &|| {},
        );
        assert_eq!(code, 0, "{}", s(&err));
        let o = s(&out);
        assert!(o.contains("  A (c-a1)：111111"), "{o}");
        assert!(o.contains("  A (c-a2)：222222"), "{o}");
        assert!(o.contains("  C：333333"), "不同名的照旧只印名字：{o}");
        assert_eq!(
            sc.seen.borrow()[2],
            r#"MeshConfirmInviter { endpoint: "c-a2" }"#
        );

        // 只敲名字：不送认定，告诉他敲括号里的编号。
        let sc = Script::new(vec![
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(asked),
        ]);
        let mut ask = |_: &str| Some("A".to_string());
        let (mut out, mut err) = (vec![], vec![]);
        let code = join(
            &mut sc.call(),
            &mut ask,
            &mut out,
            &mut err,
            Lang::Zh,
            &opts(None),
            Duration::from_secs(60),
            &|| {},
        );
        assert_eq!(code, 1);
        assert_eq!(
            s(&err).trim(),
            "不止一台叫 A 的电脑回应了。重新运行 dct join，输入数字对得上的那台后面括号里的 c-… 编号"
        );
        assert_eq!(sc.seen.borrow().len(), 2);
    }

    #[test]
    fn join_before_login_says_to_log_in() {
        let sc = Script::new(vec![Response::Mesh(view(false, &[]))]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = join(
            &mut sc.call(),
            &mut no_ask,
            &mut out,
            &mut err,
            Lang::Zh,
            &opts(None),
            Duration::ZERO,
            &|| {},
        );
        assert_eq!(code, 1);
        assert_eq!(s(&err).trim(), "还没登录多电脑，先运行 dct login");
    }

    #[test]
    fn join_arguments() {
        let a = |v: &[&str]| parse_join(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&[]), Some(JoinOpts::default()));
        assert_eq!(a(&["--name", "公司Windows"]).unwrap().name, "公司Windows");
        assert_eq!(a(&["--name=家里"]).unwrap().name, "家里");
        let both = a(&["--name", "N", "--confirm", "家里Mac"]).unwrap();
        assert_eq!(both.name, "N");
        assert_eq!(both.confirm.as_deref(), Some("家里Mac"));
        assert_eq!(a(&["--confirm=A"]).unwrap().confirm.as_deref(), Some("A"));
        assert_eq!(a(&["--nope"]), None);
        assert_eq!(a(&["--name"]), None);
        assert_eq!(a(&["--confirm"]), None);
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
}
