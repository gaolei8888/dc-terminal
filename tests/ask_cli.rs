//! `dct ask` 端到端（A2）：真的 `dct daemon` 子进程 + 真的 `dct ask` 二进制 + 本机回环上的假网关。
//! HOME 指向临时目录；守护进程的 PATH 清空（它的 dco 规矩刷新线程找不到 dco，不会去碰真 dco）；
//! 不碰真网关、不碰真的 ~/.dct。守护进程的 stdout/stderr 落在临时 HOME 里，隐私测试连它们一起扫。
#![cfg(unix)]
mod common;
use common::gateway::{delta, done, spawn_gw, Gw, Step};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

const SECRET_WORDS: &str = "绝密听写内容甲乙丙";
const REPLY: &str = "今天挺晴的。出门带伞。";

struct Env {
    home: tempfile::TempDir,
    daemon: Option<Child>,
    gw: Gw,
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn events(&self) -> Vec<Value> {
        self.stdout.lines().map(|l| serde_json::from_str(l).expect(l)).collect()
    }
    fn types(&self) -> Vec<String> {
        self.events().iter().map(|e| e["type"].as_str().unwrap().to_string()).collect()
    }
    fn last(&self) -> Value {
        self.events().pop().unwrap()
    }
}

fn dct_dir(home: &Path) -> PathBuf {
    home.join(".dct")
}

fn happy() -> Vec<Step> {
    vec![Step::Headers, delta("今天挺晴的。"), delta("出门带伞。"), done(), Step::EndChunks]
}

/// 像真的 Ollama 那样一直在吐（每 50ms 一句，共约 4 秒）。网关沉默的时候，到网关的连接要等到
/// 下一个字节或期限才会真关（A1 的已知限制：ureq 2.x 的阻塞读没法从别的线程打断）。
fn slow() -> Vec<Step> {
    let mut v = vec![Step::Headers, delta("第一句。")];
    for _ in 0..80 {
        v.push(Step::Sleep(50));
        v.push(delta("还有。"));
    }
    v
}

impl Env {
    /// `ask_cfg` 是 `[ask]` 那段（或空串）；daemon 先于 gateway 之外的一切就绪。
    fn start(ask_cfg: &str, scripts: Vec<Vec<Step>>) -> Env {
        let mut e = Env::without_daemon(ask_cfg, scripts);
        e.start_daemon();
        e
    }

    fn without_daemon(ask_cfg: &str, scripts: Vec<Vec<Step>>) -> Env {
        let home = tempfile::tempdir().unwrap();
        let gw = spawn_gw(scripts);
        let e = Env { home, daemon: None, gw };
        std::fs::create_dir_all(dct_dir(e.home.path()).join("profiles")).unwrap();
        let base = e.gw.url.trim_end_matches("/v1/chat/completions").to_string();
        std::fs::write(
            dct_dir(e.home.path()).join("profiles/gw.toml"),
            format!("name = \"gw\"\ncommand = [\"true\"]\n[api]\nbase_url = \"{base}\"\nwire = \"openai\"\n"),
        )
        .unwrap();
        let mut s = dct::secrets::SecretStore::load(&dct_dir(e.home.path()).join("secrets.toml"));
        s.set("gw", "test-key").unwrap();
        e.config(ask_cfg);
        e
    }

    fn config(&self, ask_cfg: &str) {
        std::fs::write(
            dct_dir(self.home.path()).join("config.toml"),
            format!("[llm]\nprovider = \"gw\"\nmodel = \"m\"\ntransport = \"http\"\n\n{ask_cfg}"),
        )
        .unwrap();
    }

    fn start_daemon(&mut self) {
        let h = self.home.path();
        let child = Command::new(env!("CARGO_BIN_EXE_dct"))
            .arg("daemon")
            .env("HOME", h)
            .env("USERPROFILE", h)
            .env("PATH", "/nonexistent")
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(h.join("daemon.out")).unwrap())
            .stderr(std::fs::File::create(h.join("daemon.err")).unwrap())
            .spawn()
            .unwrap();
        self.daemon = Some(child);
        let sock = dct_dir(h).join("daemon.sock");
        let t = Instant::now();
        while dct::client::Client::connect(&sock).ok().and_then(|mut c| c.protocol()).is_none() {
            assert!(t.elapsed() < Duration::from_secs(10), "守护进程没起来");
            sleep(Duration::from_millis(50));
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_dct"));
        c.args(args).env("HOME", self.home.path()).env("USERPROFILE", self.home.path());
        c
    }

    fn run(&self, args: &[&str], stdin: &str) -> Out {
        let mut child = self.cmd(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        let o = child.wait_with_output().unwrap();
        Out { code: o.status.code(), stdout: String::from_utf8_lossy(&o.stdout).into(), stderr: String::from_utf8_lossy(&o.stderr).into() }
    }

    fn ask(&self, conv: &str, text: &str) -> Out {
        self.run(&["ask"], &req(conv, text))
    }

    fn connections(&self) -> usize {
        sleep(Duration::from_millis(150));
        self.gw.conns.lock().unwrap().len()
    }

    fn body(&self, i: usize) -> Value {
        serde_json::from_str(&self.gw.conn(i).lock().unwrap().body).unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        if let Some(d) = &mut self.daemon {
            let _ = d.kill();
            let _ = d.wait();
        }
    }
}

fn req(conv: &str, text: &str) -> String {
    format!("{}\n", serde_json::json!({"v":1,"text":text,"source":"voice","conversation_id":conv,"timeout_ms":8000}))
}

const ON: &str = "[ask]\nenabled = true\nallowed_hosts = [\"127.0.0.1\"]\n";

fn contents(b: &Value) -> Vec<String> {
    b["messages"].as_array().unwrap().iter().map(|m| m["content"].as_str().unwrap().to_string()).collect()
}

/// 递归读 `dir` 下所有文件，返回包含 `needle` 的那些。
fn files_containing(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            hits.extend(files_containing(&p, needle));
        } else if std::fs::read(&p).is_ok_and(|b| String::from_utf8_lossy(&b).contains(needle)) {
            hits.push(p);
        }
    }
    hits
}

// ---- 正常路径与隐私 ----

#[test]
fn a_turn_streams_ndjson_events_and_exits_0_with_nothing_on_stderr() {
    let e = Env::start(ON, vec![happy()]);
    let o = e.ask("c-7f3a", SECRET_WORDS);
    assert_eq!(o.code, Some(0), "{}\n{}", o.stdout, o.stderr);
    assert_eq!(o.types(), ["start", "sentence", "sentence", "end"]);
    let ev = o.events();
    assert_eq!((ev[0]["conversation_id"].as_str(), ev[0]["turn"].as_u64()), (Some("c-7f3a"), Some(1)));
    assert_eq!(ev[1]["text"], "今天挺晴的。");
    assert_eq!((o.last()["sentences"].as_u64(), o.last()["truncated"].as_bool()), (Some(2), Some(false)));
    assert_eq!(o.stderr, "", "ask 路径不往 stderr 写任何东西");
    // 网关收到的是流式请求 + 带钥匙 + 话在 user 消息里。
    let c = e.gw.conn(0);
    assert!(c.lock().unwrap().authed);
    assert_eq!(contents(&e.body(0)).last().map(String::as_str), Some(SECRET_WORDS));
}

#[test]
fn the_words_and_the_replies_exist_nowhere_but_stdout_and_the_gateway_not_in_logs_files_or_daemon_output() {
    let e = Env::start(ON, vec![happy(), happy()]);
    let o1 = e.ask("c-priv", SECRET_WORDS);
    let o2 = e.ask("c-priv", "第二句绝密话");
    for o in [&o1, &o2] {
        assert_eq!(o.code, Some(0));
        assert_eq!(o.stderr, "");
    }
    // 给 `forget` 一个机会也留痕：它自己不该写文字。
    assert_eq!(e.run(&["ask", "--forget", "c-priv"], "").code, Some(0));
    let h = e.home.path();
    for needle in [SECRET_WORDS, "第二句绝密话", "今天挺晴的", "出门带伞", "c-priv"] {
        let hits = files_containing(h, needle);
        // 对话 id 是 dco 自己的，但 ask.log 里只有哈希：连它也不该出现。
        assert!(hits.is_empty(), "{needle:?} 出现在了磁盘上：{hits:?}");
    }
    // 元数据日志有，且是 0600。
    let log = std::fs::read_to_string(dct_dir(h).join("ask.log")).unwrap();
    let lines: Vec<Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|l| l["outcome"] == "end" && l["source"] == "voice" && l["conv"].as_str().unwrap().len() == 4));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(dct_dir(h).join("ask.log")).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn memory_carries_over_between_calls_and_forget_clears_it() {
    let e = Env::start(ON, vec![happy(), happy(), happy()]);
    e.ask("c1", "第一问");
    let o = e.ask("c1", "第二问");
    assert_eq!(o.events()[0]["turn"], 2);
    assert_eq!(contents(&e.body(1))[1..], ["第一问", REPLY, "第二问"]);
    assert_eq!(e.run(&["ask", "--forget", "c1"], "").code, Some(0));
    e.ask("c1", "第三问");
    assert_eq!(contents(&e.body(2)).len(), 2, "忘掉之后没有历史");
}

#[test]
fn the_protocol_version_is_25() {
    let e = Env::start(ON, vec![]);
    let mut c = dct::client::Client::connect(&dct_dir(e.home.path()).join("daemon.sock")).unwrap();
    assert_eq!(c.protocol(), Some(25));
}

// ---- opt-in ----

#[test]
fn ask_is_off_by_default_and_the_gateway_is_never_touched() {
    let e = Env::start("", vec![happy()]);
    let o = e.ask("c1", SECRET_WORDS);
    assert_eq!(o.code, Some(11));
    assert_eq!(o.types(), ["error"]);
    assert_eq!((o.last()["code"].as_str(), o.last()["reason"].as_str()), (Some("not_enabled"), Some("ask_off")));
    // 写了 enabled = false 同样。
    e.config("[ask]\nenabled = false\nallowed_hosts = [\"127.0.0.1\"]\n");
    assert_eq!(e.ask("c1", "x").last()["reason"], "ask_off");
    assert_eq!(e.connections(), 0);
}

#[test]
fn turning_ask_on_or_off_in_the_config_takes_effect_without_a_restart() {
    let e = Env::start("", vec![happy()]);
    assert_eq!(e.ask("c1", "x").code, Some(11));
    e.config(ON);
    assert_eq!(e.ask("c1", "x").code, Some(0));
    e.config("");
    assert_eq!(e.ask("c1", "x").code, Some(11));
}

#[test]
fn a_host_outside_the_allow_list_and_a_cli_transport_llm_are_not_enabled_with_no_traffic() {
    let e = Env::start("[ask]\nenabled = true\nallowed_hosts = [\"dc-llm.tzspace.cn\"]\n", vec![happy()]);
    let o = e.ask("c1", "x");
    assert_eq!((o.code, o.last()["reason"].as_str()), (Some(11), Some("host_not_allowed")));
    std::fs::write(dct_dir(e.home.path()).join("config.toml"), format!("[llm]\nprovider = \"claude\"\n\n{ON}")).unwrap();
    assert_eq!(e.ask("c1", "x").last()["reason"], "llm_not_http");
    std::fs::write(dct_dir(e.home.path()).join("config.toml"), ON).unwrap();
    assert_eq!(e.ask("c1", "x").last()["reason"], "llm_missing");
    assert_eq!(e.connections(), 0);
}

// ---- 输入 ----

#[test]
fn bad_input_is_exit_2_one_error_line_and_no_gateway_traffic() {
    let e = Env::start(ON, vec![happy()]);
    let long = format!("{}\n", serde_json::json!({"text":"字".repeat(501),"source":"voice","conversation_id":"c1"}));
    let cases: Vec<(String, &str)> = vec![
        ("not json\n".into(), "not_json"),
        ("".into(), "not_json"),
        (req("c1", "   "), "empty_input"),
        (req("bad id", "话"), "bad_conversation_id"),
        (long, "too_long"),
        ("x".repeat(20_000) + "\n", "too_long"),
        (format!("{}\n", serde_json::json!({"text":"话","source":"mic","conversation_id":"c1"})), "bad_source"),
    ];
    for (stdin, reason) in cases {
        let o = e.run(&["ask"], &stdin);
        assert_eq!(o.code, Some(2), "{reason}: {}", o.stdout);
        assert_eq!(o.stdout.lines().count(), 1);
        assert_eq!((o.last()["code"].as_str(), o.last()["reason"].as_str()), (Some("bad_request"), Some(reason)));
        assert_eq!(o.stderr, "");
    }
    assert_eq!(e.connections(), 0);
}

#[test]
fn words_on_the_command_line_are_refused_and_never_echoed() {
    let e = Env::start(ON, vec![happy()]);
    for args in [vec!["ask", SECRET_WORDS], vec!["ask", "--text", SECRET_WORDS], vec!["ask", "--forget"], vec!["ask", "--forget", "a", "b"]] {
        let o = e.run(&args, &req("c1", "x"));
        assert_eq!((o.code, o.last()["reason"].as_str()), (Some(2), Some("usage")), "{args:?}");
        assert!(!o.stdout.contains("绝密") && o.stderr.is_empty(), "{o:?}", o = (o.stdout, o.stderr));
    }
    assert_eq!(e.run(&["ask", "--forget", "bad id"], "").last()["reason"], "bad_conversation_id");
    assert_eq!(e.connections(), 0);
}

// ---- 守护进程不在 / 太旧 ----

#[test]
fn without_a_daemon_it_is_daemon_down_at_once_and_does_not_start_one() {
    let e = Env::without_daemon(ON, vec![]);
    let t = Instant::now();
    let o = e.ask("c1", "x");
    assert_eq!((o.code, o.last()["code"].as_str(), o.last()["retryable"].as_bool()), (Some(10), Some("daemon_down"), Some(true)));
    assert!(t.elapsed() < Duration::from_secs(2), "要立刻返回");
    assert!(!dct_dir(e.home.path()).join("daemon.sock").exists(), "不拉起守护进程");
    assert_eq!(e.run(&["ask", "--forget", "c1"], "").code, Some(10));
}

#[test]
fn an_older_daemon_that_does_not_know_ask_is_daemon_down_with_reason_protocol() {
    let e = Env::without_daemon(ON, vec![]);
    let l = std::os::unix::net::UnixListener::bind(dct_dir(e.home.path()).join("daemon.sock")).unwrap();
    std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut line = String::new();
        BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
        let mut s = s;
        // 旧守护进程认不出 Ask：回的是 BadRequest。
        let resp = dct::proto::Response::Error(dct::proto::ErrorCode::BadRequest("unknown variant".into()));
        writeln!(s, "{}", serde_json::to_string(&resp).unwrap()).unwrap();
    });
    let o = e.ask("c1", "x");
    assert_eq!((o.code, o.last()["reason"].as_str()), (Some(10), Some("protocol")), "{}", o.stdout);
}

// ---- 取消：杀进程 / 关 stdout = 关连接 ----

fn spawn_ask(e: &Env, conv: &str) -> Child {
    let mut c = e.cmd(&["ask"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(req(conv, SECRET_WORDS).as_bytes()).unwrap();
    c
}

fn read_until_sentence(r: &mut impl BufRead) {
    loop {
        let mut l = String::new();
        assert!(r.read_line(&mut l).unwrap() > 0, "还没等到句子流就结束了");
        if l.contains("\"sentence\"") {
            return;
        }
    }
}

#[test]
fn killing_the_client_cancels_at_the_gateway_and_an_immediate_resend_is_never_busy_and_forgets_the_turn() {
    let e = Env::start(ON, vec![slow(), happy()]);
    let mut c = spawn_ask(&e, "c1");
    read_until_sentence(&mut BufReader::new(c.stdout.as_mut().unwrap()));
    let killed = Instant::now();
    c.kill().unwrap();
    c.wait().unwrap();
    // 立刻重发同一对话：不等任何东西。
    let o = e.run(&["ask"], &req("c1", "重发的话"));
    assert_eq!(o.code, Some(0), "{}", o.stdout);
    assert_eq!(o.types().last().map(String::as_str), Some("end"));
    assert_eq!(contents(&e.body(1))[1..], ["重发的话"], "被取消那轮（话和半截回复）不入记忆");
    // 到网关的连接在一个读超时周期（250ms）附近被关掉，不等 4 秒的网关脚本走完。
    e.gw.wait_closed(0, Duration::from_millis(1500));
    assert!(killed.elapsed() < Duration::from_millis(3000));
}

#[test]
fn a_silent_gateway_does_not_hide_the_hangup_the_slot_frees_within_a_poll_period() {
    // 第一句之后网关沉默 3 秒：没有写可以失败，守护进程只能靠轮询 socket 的 EOF 发现对面走了。
    let silent = vec![Step::Headers, delta("第一句。"), Step::Sleep(3000), delta("迟到。"), done(), Step::EndChunks];
    let e = Env::start(ON, vec![silent, happy()]);
    let mut c = spawn_ask(&e, "a");
    read_until_sentence(&mut BufReader::new(c.stdout.as_mut().unwrap()));
    c.kill().unwrap();
    c.wait().unwrap();
    sleep(Duration::from_millis(600)); // 一个轮询周期（250ms）再加余量，远小于网关的 3 秒沉默
    let o = e.ask("b", "别的对话");
    assert_eq!(o.code, Some(0), "对面走了，别的对话不该再 busy：{}", o.stdout);
}

#[test]
fn closing_stdout_ends_the_client_and_cancels_the_turn() {
    let e = Env::start(ON, vec![slow(), happy()]);
    let mut c = spawn_ask(&e, "c1");
    let mut out = BufReader::new(c.stdout.take().unwrap());
    read_until_sentence(&mut out);
    drop(out); // dco 关 stdout
    // 下一次写（第二句）失败 → 客户端退出（18 = 被取消）→ socket 关 → 网关那边也关。
    let status = c.wait().unwrap();
    assert_eq!(status.code(), Some(18));
    e.gw.wait_closed(0, Duration::from_millis(2000));
    assert_eq!(e.ask("c1", "x").code, Some(0), "取消之后立刻能再问");
}

#[test]
fn a_second_conversation_while_one_is_in_flight_is_busy_exit_14() {
    let e = Env::start(ON, vec![slow()]);
    let mut c = spawn_ask(&e, "a");
    read_until_sentence(&mut BufReader::new(c.stdout.as_mut().unwrap()));
    let o = e.ask("b", "x");
    assert_eq!((o.code, o.last()["code"].as_str()), (Some(14), Some("busy")), "{}", o.stdout);
    assert_eq!(e.connections(), 1, "忙的时候不去碰网关");
    c.kill().unwrap();
    c.wait().unwrap();
}

// ---- 故障 ----

#[test]
fn gateway_failures_map_to_exit_codes_through_the_whole_stack() {
    for (script, code, exit) in [(vec![Step::Status(401)], "auth", 13), (vec![Step::Status(503)], "busy", 14), (vec![Step::Status(500)], "bad_reply", 17)] {
        let e = Env::start(ON, vec![script]);
        let o = e.ask("c1", "x");
        assert_eq!((o.code, o.last()["code"].as_str()), (Some(exit), Some(code)));
        assert_eq!(o.types(), ["start", "error"]);
    }
}

// ---- 给 ask 让路（守护进程里的其他网关用户：出错解释）----

/// 一个一起来就在屏幕上打出报错的会话：`error_pattern` 命中 → Failed → 守护进程去问模型解释。
fn failing_profile() -> dct::profile::Profile {
    dct::profile::Profile {
        name: "ask-test-failing".into(),
        command: vec!["sh".into(), "-c".into(), "echo BOOM_ERR; sleep 30".into()],
        is_agent: false,
        idle_pattern: None,
        busy_pattern: None,
        error_pattern: Some("BOOM_ERR".into()),
        env: Default::default(),
        secret: None,
        install: None,
        login: None,
        headless: None,
        api: None,
        label: Default::default(),
        note: Default::default(),
        resume_args: Default::default(),
        pairable: false,
        backend_only: false,
    }
}

#[test]
fn the_daemons_error_explanations_wait_for_an_ask_in_flight_and_go_after() {
    // 第 0 个连接是 ask 的流；第 1 个是出错解释（非流式，网关随便回个 500）。
    let e = Env::without_daemon(ON, vec![slow(), vec![Step::Status(500)]]);
    let mgr = std::sync::Arc::new(dct::session::SessionManager::new());
    mgr.register_profile(failing_profile());
    let sock = dct_dir(e.home.path()).join("daemon.sock");
    let s2 = sock.clone();
    std::thread::spawn(move || {
        let _ = dct::daemon::run_with_manager(&s2, mgr);
    });
    let t = Instant::now();
    while dct::client::Client::connect(&sock).ok().and_then(|mut c| c.protocol()).is_none() {
        assert!(t.elapsed() < Duration::from_secs(10), "守护进程没起来");
        sleep(Duration::from_millis(50));
    }
    let mut ask = spawn_ask(&e, "a");
    read_until_sentence(&mut BufReader::new(ask.stdout.as_mut().unwrap()));
    // ask 在途时起一个马上报错的会话。
    let mut c = dct::client::Client::connect(&sock).unwrap();
    let dir = e.home.path().to_string_lossy().to_string();
    let created = c.call(dct::proto::Request::Create { dir, profile: "ask-test-failing".into(), remember: false }).unwrap();
    assert!(matches!(created, dct::proto::Response::Created { .. }), "{created:?}");
    sleep(Duration::from_millis(1500)); // 足够它报错、被 tick 发现、去问解释
    assert_eq!(e.gw.conns.lock().unwrap().len(), 1, "ask 在途，出错解释不该去碰网关");
    // ask 一结束（对面走了），解释才去问。
    ask.kill().unwrap();
    ask.wait().unwrap();
    let t = Instant::now();
    while e.gw.conns.lock().unwrap().len() < 2 {
        assert!(t.elapsed() < Duration::from_secs(4), "ask 结束后出错解释一直没去问");
        sleep(Duration::from_millis(50));
    }
}
