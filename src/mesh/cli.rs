//! `dct login` / `dct join` / `dct peers`。
//!
//! 这里只是把话说给人听：真正的事（换令牌、问别的电脑、签名单）都在守护
//! 进程里做——它握着中转连接、钥匙和密钥仓，命令行这边一样也不碰。
//!
//! 跟守护进程说话的那一下（`call`）是参数，于是整套对话不起守护进程也能测。
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::client::Client;
use crate::i18n::{msg, text, Key, Lang};
use crate::proto::{ErrorCode, MeshProblem, MeshView, Request, Response};

/// `dct join` 最多等多久有人点同意。跟对面那条请求的有效期一样长。
const JOIN_WAIT: Duration = crate::mesh::JOIN_TTL;
/// 等同意时多久问一次守护进程。
const JOIN_POLL: Duration = Duration::from_secs(1);
/// 守护进程替我们打网络的那几条请求（换令牌、问别的电脑）等多久。
const SLOW_CALL: Duration = Duration::from_secs(40);

type Call<'a> = &'a mut dyn FnMut(Request) -> Result<Response>;

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
        let slow = matches!(req, Request::MeshLogin | Request::MeshJoin { .. });
        if slow {
            client.call_within(req, SLOW_CALL)
        } else {
            client.call(req)
        }
    };
    match cmd {
        "login" => login(&mut call, &mut out, &mut err, lang),
        "join" => match parse_join(rest) {
            Some(name) => join(
                &mut call,
                &mut out,
                &mut err,
                lang,
                &name,
                JOIN_WAIT,
                &|| std::thread::sleep(JOIN_POLL),
            ),
            None => {
                let _ = writeln!(err, "{}", msg::mesh_join_usage(lang));
                2
            }
        },
        "peers" => peers(&mut call, &mut out, &mut err, lang, rest),
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

/// `--name 公司Windows` / `--name=公司Windows`，或者什么都不给。`None` = 用法不对。
fn parse_join(args: &[String]) -> Option<String> {
    match args {
        [] => Some(String::new()),
        [flag, name] if flag == "--name" => Some(name.clone()),
        [one] => one.strip_prefix("--name=").map(str::to_string),
        _ => None,
    }
}

/// 问一次守护进程，要的是 `MeshView`。别的答复都当失败，原因印到 `err`。
fn view_of(call: Call, req: Request, err: &mut dyn Write, lang: Lang) -> Option<MeshView> {
    match call(req) {
        Ok(Response::Mesh(v)) => Some(v),
        Ok(Response::Error(e)) => {
            let _ = writeln!(err, "{}", msg::error(lang, &e));
            None
        }
        Ok(other) => {
            let _ = writeln!(
                err,
                "{}",
                msg::error(lang, &ErrorCode::Internal(format!("{other:?}")))
            );
            None
        }
        Err(e) => {
            let code = match e.downcast::<crate::proto::CodedError>() {
                Ok(c) => c.0,
                Err(e) => ErrorCode::Internal(e.to_string()),
            };
            let _ = writeln!(err, "{}", msg::error(lang, &code));
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

pub(crate) fn join(
    call: Call,
    out: &mut dyn Write,
    err: &mut dyn Write,
    lang: Lang,
    name: &str,
    wait: Duration,
    pause: &dyn Fn(),
) -> i32 {
    let Some(now) = view_of(call, Request::MeshStatus, err, lang) else {
        return 1;
    };
    if !now.logged_in {
        let _ = writeln!(
            err,
            "{}",
            msg::error(lang, &ErrorCode::Mesh(MeshProblem::NotLoggedIn))
        );
        return 1;
    }
    let req = Request::MeshJoin {
        name: name.to_string(),
    };
    let Some(asked) = view_of(call, req, err, lang) else {
        return 1;
    };
    match asked.joining.as_slice() {
        [one] => {
            let _ = writeln!(out, "{}", msg::mesh_compare_codes(lang, Some(&one.code)));
        }
        many => {
            let _ = writeln!(out, "{}", msg::mesh_compare_codes(lang, None));
            for j in many {
                let _ = writeln!(out, "{}", msg::mesh_code_line(lang, &j.name, &j.code));
            }
        }
    }
    let _ = writeln!(out, "{}", msg::mesh_waiting_for_approval(lang));
    let _ = out.flush();

    let deadline = Instant::now() + wait;
    loop {
        pause();
        if let Some(v) = view_of(call, Request::MeshStatus, err, lang) {
            if grouped(&v) {
                let _ = writeln!(out, "{}", msg::mesh_joined_group(lang));
                return 0;
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
                let _ = writeln!(
                    err,
                    "{}",
                    msg::error(lang, &ErrorCode::Mesh(MeshProblem::NotLoggedIn))
                );
                return 1;
            }
            let _ = writeln!(out, "{}", msg::mesh_members_header(lang));
            for m in &v.members {
                let _ = writeln!(
                    out,
                    "{}",
                    msg::mesh_member_line(lang, &m.name, m.online, m.is_me)
                );
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
            let req = Request::MeshApprove {
                endpoint: who.to_string(),
                yes,
            };
            let Some(v) = view_of(call, req, err, lang) else {
                return 1;
            };
            if yes {
                let name = v
                    .members
                    .iter()
                    .find(|m| m.endpoint == *who || m.name == *who)
                    .map(|m| m.name.as_str())
                    .unwrap_or(who);
                let _ = writeln!(out, "{}", msg::mesh_member_joined(lang, name));
            } else {
                let _ = writeln!(out, "{}", msg::mesh_member_refused(lang, who));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{MemberView, PendingJoin};
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

    #[test]
    fn join_prints_one_code_per_computer_and_waits_for_the_roster() {
        let mut asked = view(true, &[("B", true)]);
        asked.joining = vec![
            PendingJoin {
                name: "家里Mac".into(),
                endpoint: "c-a".into(),
                code: "111111".into(),
            },
            PendingJoin {
                name: "办公室".into(),
                endpoint: "c-c".into(),
                code: "222222".into(),
            },
        ];
        let sc = Script::new(vec![
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(asked),
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(view(true, &[("B", true), ("家里Mac", false)])),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = join(
            &mut sc.call(),
            &mut out,
            &mut err,
            Lang::Zh,
            "公司Windows",
            Duration::from_secs(60),
            &|| {},
        );
        assert_eq!(code, 0, "{}", s(&err));
        let o = s(&out);
        assert!(o.contains("  家里Mac：111111"), "{o}");
        assert!(o.contains("  办公室：222222"), "{o}");
        assert!(o.trim_end().ends_with("已加入"), "{o}");
        assert!(sc.seen.borrow()[1].contains("公司Windows"));
    }

    #[test]
    fn join_with_one_computer_puts_the_code_in_the_sentence() {
        let mut asked = view(true, &[("B", true)]);
        asked.joining = vec![PendingJoin {
            name: "A".into(),
            endpoint: "c-a".into(),
            code: "654321".into(),
        }];
        let sc = Script::new(vec![
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(asked),
            Response::Mesh(view(true, &[("B", true), ("A", false)])),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        join(
            &mut sc.call(),
            &mut out,
            &mut err,
            Lang::Zh,
            "",
            Duration::from_secs(60),
            &|| {},
        );
        assert!(s(&out).contains(
            "请在你已有的任意一台电脑上看一眼：那边会显示一个 6 位数，跟这里的 654321 一样就点同意"
        ));
    }

    #[test]
    fn join_gives_up_after_the_wait() {
        let mut asked = view(true, &[("B", true)]);
        asked.joining = vec![PendingJoin {
            name: "A".into(),
            endpoint: "c-a".into(),
            code: "1".into(),
        }];
        let sc = Script::new(vec![
            Response::Mesh(view(true, &[("B", true)])),
            Response::Mesh(asked),
            Response::Mesh(view(true, &[("B", true)])),
        ]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = join(
            &mut sc.call(),
            &mut out,
            &mut err,
            Lang::Zh,
            "",
            Duration::ZERO,
            &|| {},
        );
        assert_eq!(code, 1);
        assert!(s(&err).contains("10 分钟里没等到同意"));
    }

    #[test]
    fn join_before_login_says_to_log_in() {
        let sc = Script::new(vec![Response::Mesh(view(false, &[]))]);
        let (mut out, mut err) = (vec![], vec![]);
        let code = join(
            &mut sc.call(),
            &mut out,
            &mut err,
            Lang::Zh,
            "",
            Duration::ZERO,
            &|| {},
        );
        assert_eq!(code, 1);
        assert_eq!(s(&err).trim(), "还没登录多电脑，先运行 dct login");
    }

    #[test]
    fn join_arguments() {
        let a = |v: &[&str]| parse_join(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&[]), Some(String::new()));
        assert_eq!(a(&["--name", "公司Windows"]), Some("公司Windows".into()));
        assert_eq!(a(&["--name=家里"]), Some("家里".into()));
        assert_eq!(a(&["--nope"]), None);
        assert_eq!(a(&["--name"]), None);
    }

    #[test]
    fn peers_lists_members_and_asks_about_pending_joins() {
        let mut v = view(true, &[("B", true), ("A", false)]);
        v.pending = vec![PendingJoin {
            name: "公司Windows".into(),
            endpoint: "c-w".into(),
            code: "123456".into(),
        }];
        let sc = Script::new(vec![Response::Mesh(v)]);
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(peers(&mut sc.call(), &mut out, &mut err, Lang::Zh, &[]), 0);
        let o = s(&out);
        assert!(o.contains("  B  (这台)"), "{o}");
        assert!(o.contains(
            "一台叫 公司Windows 的电脑想加入「我的电脑」。它屏幕上的数字是 123456 吗？(y/n)"
        ));
        assert!(o.contains("dct peers approve 公司Windows"), "{o}");
    }

    #[test]
    fn peers_approve_refuse_and_remove_say_what_happened() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (mut out, mut err) = (vec![], vec![]);

        let sc = Script::new(vec![Response::Mesh(view(
            true,
            &[("B", true), ("C", false)],
        ))]);
        assert_eq!(
            peers(
                &mut sc.call(),
                &mut out,
                &mut err,
                Lang::Zh,
                &args(&["approve", "C"])
            ),
            0
        );
        assert!(sc.seen.borrow()[0].contains("yes: true"));
        assert!(s(&out).contains("C 已加入"));

        let sc = Script::new(vec![Response::Mesh(view(true, &[("B", true)]))]);
        out.clear();
        peers(
            &mut sc.call(),
            &mut out,
            &mut err,
            Lang::Zh,
            &args(&["approve", "C", "--no"]),
        );
        assert!(sc.seen.borrow()[0].contains("yes: false"));
        assert!(s(&out).contains("已拒绝 C"));

        let sc = Script::new(vec![Response::Mesh(view(true, &[("B", true)]))]);
        out.clear();
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
}
