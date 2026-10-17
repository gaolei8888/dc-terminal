//! `dct ask`：dco 拉起的短命薄客户端（设计 §2）。
//!
//! - `dct ask`：stdin 一行 JSON（话**只走 stdin，不走 argv**），stdout 一行一个 NDJSON 事件
//!   （`start` / `sentence` / `end` / `error`），退出码见 `ErrorCode::exit_code`。
//!   不拉起守护进程：连不上就是 `daemon_down`（退出码 10）。**这个路径不往 stderr 写任何东西。**
//! - `dct ask --forget <对话 id>`：忘掉一个对话的记忆。
//! - `dct ask --enable`：开 `[ask]`。要本人在终端里看完文案、输入 y（没有 `--yes`——同意不能被
//!   别的程序代点）。dco 看到 `not_enabled` / `reason = ask_off` 时该做的是请用户自己运行它。
//!
//! 取消 = 关连接：dco 杀掉本进程，内核关 socket，守护进程发现后停生成（`service`）。stdout 被
//! 对面关了（写失败）也一样直接退出——退出就关了 socket。

use super::input::{parse_line, valid_conversation_id, MAX_STDIN};
use super::service::destination;
use super::{refusal, ErrorCode};
use crate::config::Config;
use crate::i18n::{msg, AskEnableNote, Lang};
use crate::proto::{socket_path, Request, Response};
use crate::sys::ipc::Stream;
use serde_json::Value;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::Path;
use std::time::Duration;

/// 守护进程自己有期限（`timeout_ms`，最多 30 秒）；这里多等一点，只为守护进程卡死时不跟着挂住。
const GRACE: Duration = Duration::from_secs(5);

pub fn run(args: &[String], lang: Lang) -> i32 {
    match args {
        [] => ask(),
        [f] if f == "--enable" => enable(lang),
        [f, id] if f == "--forget" => forget(id),
        // 不回显参数：位置参数里要是有人放了听写的话，也不该被打到任何地方。
        _ => fail(ErrorCode::BadRequest, "usage"),
    }
}

fn out(v: &Value) -> io::Result<()> {
    let mut o = io::stdout().lock();
    writeln!(o, "{v}")?;
    o.flush()
}

/// 打一个 `error` 事件，返回对应的退出码。
fn fail(code: ErrorCode, reason: &str) -> i32 {
    let _ = out(&refusal(code, reason));
    code.exit_code()
}

fn connect(wait: Duration) -> io::Result<(BufReader<Stream>, Stream)> {
    let s = Stream::connect(socket_path())?;
    s.set_read_timeout(Some(wait))?;
    Ok((BufReader::new(s.try_clone()?), s))
}

fn send(w: &mut Stream, req: &Request) -> io::Result<()> {
    writeln!(w, "{}", serde_json::to_string(req)?)?;
    w.flush()
}

fn ask() -> i32 {
    let mut line = String::new();
    let read = io::stdin().lock().take(MAX_STDIN + 1).read_line(&mut line);
    let parsed = match read {
        Ok(n) if n as u64 > MAX_STDIN => Err("too_long"),
        Ok(_) => parse_line(&line),
        Err(_) => Err("not_json"),
    };
    let (req, input) = match parsed {
        Ok(p) => p,
        Err(reason) => return fail(ErrorCode::BadRequest, reason),
    };
    let Ok((mut reader, mut w)) = connect(input.timeout + GRACE) else {
        return fail(ErrorCode::DaemonDown, "socket");
    };
    if send(&mut w, &Request::Ask(req)).is_err() {
        return fail(ErrorCode::DaemonDown, "socket");
    }
    let mut l = String::new();
    loop {
        l.clear();
        match reader.read_line(&mut l) {
            Ok(0) => return fail(ErrorCode::DaemonDown, "closed"),
            Ok(_) => match serde_json::from_str::<Response>(&l) {
                Ok(Response::AskEvent { event, exit }) => {
                    // 对面不读了（stdout 关了）：直接退出，socket 随之关闭，守护进程取消这一轮。
                    if out(&event).is_err() {
                        return ErrorCode::Cancelled.exit_code();
                    }
                    if let Some(code) = exit {
                        return code;
                    }
                }
                // 不认识 `Ask` 的旧守护进程回的是别的响应：要 `dct restart`。
                _ => return fail(ErrorCode::DaemonDown, "protocol"),
            },
            Err(_) => return fail(ErrorCode::Slow, "daemon_silent"),
        }
    }
}

fn forget(id: &str) -> i32 {
    if !valid_conversation_id(id) {
        return fail(ErrorCode::BadRequest, "bad_conversation_id");
    }
    let Ok((mut reader, mut w)) = connect(GRACE) else {
        return fail(ErrorCode::DaemonDown, "socket");
    };
    if send(&mut w, &Request::AskForget { conversation_id: id.to_string() }).is_err() {
        return fail(ErrorCode::DaemonDown, "socket");
    }
    let mut l = String::new();
    match reader.read_line(&mut l) {
        Ok(n) if n > 0 && matches!(serde_json::from_str(&l), Ok(Response::Ok)) => 0,
        _ => fail(ErrorCode::DaemonDown, "protocol"),
    }
}

fn enable(lang: Lang) -> i32 {
    let socket = socket_path();
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    enable_with(
        &crate::config::config_path_for_socket(&socket),
        &crate::profile::profiles_dir_for_socket(&socket),
        lang,
        interactive,
        &mut io::stdin().lock(),
        &mut io::stdout(),
    )
}

/// `--enable` 的全部流程，输入输出可注入。退出码：0 = 开了或本来就开着；1 = 没开（没同意 /
/// 已有 `[ask]` 不去动 / 写不进）；2 = 不在终端里；11 = 用不了（`[llm]` 没配好等）。
pub fn enable_with(
    config_path: &Path,
    profiles_dir: &Path,
    lang: Lang,
    interactive: bool,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> i32 {
    let say = |out: &mut dyn Write, note| {
        let _ = writeln!(out, "{}", msg::ask_enable_note(lang, note, config_path));
    };
    if !interactive {
        say(out, AskEnableNote::NeedTerminal);
        return 2;
    }
    let cfg = Config::load(config_path);
    let (custom, _) = crate::profile::all_profiles(profiles_dir);
    let host = match destination(&cfg, &crate::profile::lookup_in(&custom)) {
        Ok((_, host)) => host,
        Err(reason) => {
            let _ = writeln!(out, "{}", msg::ask_cannot(lang, reason));
            return 11;
        }
    };
    if cfg.ask.enabled {
        say(out, AskEnableNote::AlreadyOn);
        return 0;
    }
    let _ = write!(out, "{}", msg::ask_consent(lang, &host));
    let _ = out.flush();
    let mut answer = String::new();
    if input.read_line(&mut answer).is_err() || !answer.trim().eq_ignore_ascii_case("y") {
        say(out, AskEnableNote::Declined);
        return 1;
    }
    match crate::llm_optin::enable_ask(config_path, &host) {
        Ok(true) => {
            say(out, AskEnableNote::Enabled);
            0
        }
        Ok(false) => {
            say(out, AskEnableNote::SectionExists);
            1
        }
        Err(_) => {
            say(out, AskEnableNote::WriteFailed);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份能走通的 `[llm]`：自定义 profile 指向本机一个假主机，不碰任何真网关。
    fn home(llm_extra: &str) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let (cfg, profiles) = (d.path().join("config.toml"), d.path().join("profiles"));
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(
            profiles.join("gw.toml"),
            "name = \"gw\"\ncommand = [\"true\"]\n[api]\nbase_url = \"https://gw.example.com\"\nwire = \"openai\"\n",
        )
        .unwrap();
        std::fs::write(&cfg, format!("[llm]\nprovider = \"gw\"\nmodel = \"m\"\ntransport = \"http\"\n{llm_extra}")).unwrap();
        (d, cfg, profiles)
    }

    fn run_enable(cfg: &Path, profiles: &Path, interactive: bool, typed: &str) -> (i32, String) {
        let mut out = Vec::new();
        let code = enable_with(cfg, profiles, Lang::Zh, interactive, &mut typed.as_bytes(), &mut out);
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn a_host_outside_the_allow_list_is_refused_before_any_consent() {
        let (_d, cfg, profiles) = home("");
        // 默认名单只有 dc-llm.tzspace.cn，gw.example.com 不在：不问、不写。
        let (code, text) = run_enable(&cfg, &profiles, true, "y\n");
        assert!(code == 11 && text.contains("allowed_hosts") && !text.contains("输入 y"), "{code} {text}");
        assert!(!std::fs::read_to_string(&cfg).unwrap().contains("[ask]"));
    }

    #[test]
    fn consent_names_the_destination_and_only_y_turns_it_on() {
        let (_d, cfg, profiles) = home("");
        // 让这台主机在默认名单里：base_url 指向默认主机。
        std::fs::write(
            profiles.join("gw.toml"),
            format!("name = \"gw\"\ncommand = [\"true\"]\n[api]\nbase_url = \"https://{}\"\nwire = \"openai\"\n", crate::config::ASK_DEFAULT_HOST),
        )
        .unwrap();
        for no in ["", "\n", "n\n", "yes\n", "好\n"] {
            let (code, text) = run_enable(&cfg, &profiles, true, no);
            assert_eq!(code, 1, "{no:?}: {text}");
            assert!(text.contains(crate::config::ASK_DEFAULT_HOST) && text.contains("不写盘"), "同意文案要说去哪、不写盘：{text}");
            assert!(!Config::load(&cfg).ask.enabled, "{no:?} 不该开");
        }
        let (code, _) = run_enable(&cfg, &profiles, true, "Y\n");
        assert_eq!(code, 0);
        assert!(Config::load(&cfg).ask.enabled);
        // 再来一次：本来就开着，不再问。
        let (code, text) = run_enable(&cfg, &profiles, true, "");
        assert_eq!((code, text.contains("不写盘")), (0, false));
    }

    #[test]
    fn without_a_terminal_it_refuses_and_writes_nothing() {
        let (_d, cfg, profiles) = home("");
        let before = std::fs::read_to_string(&cfg).unwrap();
        let (code, text) = run_enable(&cfg, &profiles, false, "y\n");
        assert_eq!(code, 2, "{text}");
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), before, "非终端不能代点同意");
    }

    #[test]
    fn a_broken_llm_setup_says_why_and_never_asks() {
        let d = tempfile::tempdir().unwrap();
        let (cfg, profiles) = (d.path().join("config.toml"), d.path().join("profiles"));
        for (toml, word) in [
            ("", "还没配 [llm]"),
            ("[llm]\nprovider = \"claude\"\n", "命令行传输"),
            ("[llm]\nprovider = \"nope\"\ntransport = \"http\"\n", "没配好"),
        ] {
            std::fs::write(&cfg, toml).unwrap();
            let (code, text) = run_enable(&cfg, &profiles, true, "y\n");
            assert!(code == 11 && text.contains(word) && !text.contains("输入 y"), "{toml:?}: {code} {text}");
        }
    }

    #[test]
    fn an_existing_ask_section_is_not_touched() {
        let (_d, cfg, profiles) = home("");
        std::fs::write(
            profiles.join("gw.toml"),
            format!("name = \"gw\"\ncommand = [\"true\"]\n[api]\nbase_url = \"https://{}\"\nwire = \"openai\"\n", crate::config::ASK_DEFAULT_HOST),
        )
        .unwrap();
        let mut text = std::fs::read_to_string(&cfg).unwrap();
        text.push_str("[ask]\nenabled = false\n");
        std::fs::write(&cfg, &text).unwrap();
        let (code, said) = run_enable(&cfg, &profiles, true, "y\n");
        assert_eq!(code, 1, "{said}");
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), text);
    }
}
