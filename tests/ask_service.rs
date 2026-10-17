//! 守护进程里的 ask 服务（A2）：真的 ureq 传输 + 本机回环上的假网关，不碰真网关。
//! 校验、配置检查、全局 1 个在途、记忆、故障表、隐私、让路。守护进程协议和 CLI 在 `ask_cli.rs`。
#![cfg(unix)]
mod common;
use common::gateway::{delta, done, spawn_gw, Gw, Step};
use dct::ask::input::AskRequest;
use dct::ask::service::{Service, Target, Yielding};
use dct::ask::ClientGone;
use dct::llm::creds::Credential;
use dct::llm::stream::UreqOpen;
use dct::llm::{Backend, LlmError, Prompt};
use serde_json::{json, Value};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

fn target(url: &str) -> Result<Target, &'static str> {
    Ok(Target { opener: Arc::new(UreqOpen::new(url.to_string(), Credential::Key("test-key".into()))), model: "m".into() })
}

fn req(text: &str, conv: &str) -> AskRequest {
    AskRequest { v: Some(1), text: text.into(), source: "voice".into(), conversation_id: conv.into(), persona: None, timeout_ms: Some(5000) }
}

struct Alive;
impl ClientGone for Alive {
    fn gone(&mut self) -> bool {
        false
    }
}

/// 测试里可以随时「关连接」的客户端。
#[derive(Clone, Default)]
struct Hangup(Arc<AtomicBool>);
impl ClientGone for Hangup {
    fn gone(&mut self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

type Events = Vec<(Value, Option<i32>)>;

fn ask(svc: &Service, r: &AskRequest, resolve: &dyn Fn() -> Result<Target, &'static str>, peer: &mut dyn ClientGone) -> Events {
    let mut ev = Events::new();
    svc.ask(r, resolve, peer, &mut |v, exit| {
        ev.push((v, exit));
        Ok(())
    });
    ev
}

fn ask_at(svc: &Service, r: &AskRequest, url: &str) -> Events {
    ask(svc, r, &|| target(url), &mut Alive)
}

fn types(ev: &Events) -> Vec<&str> {
    ev.iter().map(|(v, _)| v["type"].as_str().unwrap()).collect()
}

fn sentences(ev: &Events) -> Vec<&str> {
    ev.iter().filter(|(v, _)| v["type"] == "sentence").map(|(v, _)| v["text"].as_str().unwrap()).collect()
}

/// 每次调用恰好一个终止事件，且是最后一个；返回它和退出码。
fn terminal(ev: &Events) -> (&Value, i32) {
    let ends: Vec<_> = ev.iter().filter(|(v, _)| matches!(v["type"].as_str(), Some("end" | "error"))).collect();
    assert_eq!(ends.len(), 1, "要恰好一个终止事件：{ev:?}");
    assert!(std::ptr::eq(&ev.last().unwrap().0, &ends[0].0), "终止事件要排最后：{ev:?}");
    (&ends[0].0, ends[0].1.expect("终止事件带退出码"))
}

fn happy() -> Vec<Step> {
    vec![Step::Headers, delta("今天挺晴的。"), delta("出门带伞。"), done(), Step::EndChunks]
}

fn body(gw: &Gw, i: usize) -> Value {
    serde_json::from_str(&gw.conn(i).lock().unwrap().body).unwrap()
}

fn contents(b: &Value) -> Vec<String> {
    b["messages"].as_array().unwrap().iter().map(|m| m["content"].as_str().unwrap().to_string()).collect()
}

fn connections(gw: &Gw) -> usize {
    sleep(Duration::from_millis(100));
    gw.conns.lock().unwrap().len()
}

// ---- 正常路径 ----

#[test]
fn a_turn_emits_start_sentences_end_and_exit_0() {
    let gw = spawn_gw(vec![happy()]);
    let ev = ask_at(&Service::new(None), &req("今天天气", "c1"), &gw.url);
    assert_eq!(types(&ev), ["start", "sentence", "sentence", "end"]);
    assert_eq!(ev[0].0, json!({"v":1,"type":"start","conversation_id":"c1","turn":1}));
    assert_eq!(sentences(&ev), ["今天挺晴的。", "出门带伞。"]);
    assert_eq!(ev[1].0["i"], 0);
    let (end, exit) = terminal(&ev);
    assert_eq!((end["sentences"].as_u64(), end["truncated"].as_bool(), exit), (Some(2), Some(false), 0));
    // 只有终止事件带退出码。
    assert!(ev[..3].iter().all(|(_, e)| e.is_none()));
}

#[test]
fn the_request_has_the_fixed_prompt_the_user_text_and_no_thinking_or_tool_fields() {
    let gw = spawn_gw(vec![happy()]);
    let mut r = req("今天天气", "c1");
    r.persona = Some("爱笑的章鱼".into());
    ask_at(&Service::new(None), &r, &gw.url);
    let b = body(&gw, 0);
    let mut keys: Vec<&str> = b.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["max_tokens", "messages", "model", "stream"], "没有 tools / think / reasoning 之类字段");
    assert_eq!((b["stream"].as_bool(), b["max_tokens"].as_u64()), (Some(true), Some(200)));
    let m = b["messages"].as_array().unwrap();
    assert_eq!((m[0]["role"].as_str(), m[1]["role"].as_str(), m.len()), (Some("system"), Some("user"), 2));
    assert!(m[0]["content"].as_str().unwrap().contains("爱笑的章鱼") && m[0]["content"].as_str().unwrap().contains("不能操作电脑"));
    assert_eq!(m[1]["content"], "今天天气");
}

// ---- 记忆 ----

#[test]
fn the_next_turn_carries_the_history_and_forget_clears_it() {
    let gw = spawn_gw(vec![happy(), happy(), happy()]);
    let svc = Service::new(None);
    ask_at(&svc, &req("第一句话", "c1"), &gw.url);
    let ev = ask_at(&svc, &req("第二句话", "c1"), &gw.url);
    assert_eq!(ev[0].0["turn"], 2);
    let c = contents(&body(&gw, 1));
    assert_eq!(c[1..], ["第一句话", "今天挺晴的。出门带伞。", "第二句话"]);
    // 别的对话看不到。
    ask_at(&svc, &req("别人的话", "c2"), &gw.url);
    assert_eq!(contents(&body(&gw, 2)).len(), 2);
    // 忘掉之后从头开始。
    svc.forget("c1");
    let gw = spawn_gw(vec![happy()]);
    let ev = ask_at(&svc, &req("第三句话", "c1"), &gw.url);
    assert_eq!(ev[0].0["turn"], 1);
    assert_eq!(contents(&body(&gw, 0)).len(), 2);
}

#[test]
fn a_failed_turn_is_not_remembered_so_a_retry_does_not_repeat_the_user() {
    let gw = spawn_gw(vec![vec![Step::Status(500)], happy()]);
    let svc = Service::new(None);
    let ev = ask_at(&svc, &req("重试的话", "c1"), &gw.url);
    assert_eq!(terminal(&ev).1, 17);
    ask_at(&svc, &req("重试的话", "c1"), &gw.url);
    assert_eq!(contents(&body(&gw, 1)).len(), 2, "失败那轮不该入记忆");
}

#[test]
fn a_turn_cut_short_by_the_deadline_remembers_only_what_was_said_marked_truncated() {
    let gw = spawn_gw(vec![vec![Step::Headers, delta("说出口的。"), delta("没说完"), Step::Sleep(5000)], happy()]);
    let svc = Service::new(None);
    let mut r = req("长问题", "c1");
    r.timeout_ms = Some(1000);
    let ev = ask_at(&svc, &r, &gw.url);
    let (end, exit) = terminal(&ev);
    assert_eq!((end["type"].as_str(), end["truncated"].as_bool(), exit), (Some("end"), Some(true), 0), "{ev:?}");
    assert_eq!(sentences(&ev), ["说出口的。"]);
    ask_at(&svc, &req("接着问", "c1"), &gw.url);
    let c = contents(&body(&gw, 1));
    assert_eq!(c[1..], ["长问题", "说出口的。…", "接着问"], "只记说出口的，带截断标记，半句话不在里面");
}

#[test]
fn persona_is_per_request_and_never_remembered() {
    let gw = spawn_gw(vec![happy(), happy()]);
    let svc = Service::new(None);
    let mut r = req("一", "c1");
    r.persona = Some("人设甲".into());
    ask_at(&svc, &r, &gw.url);
    ask_at(&svc, &req("二", "c1"), &gw.url);
    let b = body(&gw, 1);
    assert!(!b.to_string().contains("人设甲"), "dct 不存人设：{b}");
}

// ---- 取消：关连接 = 整轮丢弃，立刻重发不 busy ----

#[test]
fn hanging_up_mid_turn_drops_the_whole_turn_and_an_immediate_resend_is_never_busy() {
    // 旧连接让假网关故意拖着不收尾：重发不能靠旧线程恰好先退出才通过。
    let gw = spawn_gw(vec![vec![Step::Headers, delta("旧的第一句。"), Step::Sleep(4000)], happy()]);
    let svc = Arc::new(Service::new(None));
    let hang = Hangup::default();
    let (s2, h2, url) = (svc.clone(), hang.clone(), gw.url.clone());
    let old = std::thread::spawn(move || ask(&s2, &req("被取消的话", "c1"), &|| target(&url), &mut h2.clone()));
    // 等第一句出来再挂断。
    let t = Instant::now();
    while gw.conns.lock().unwrap().is_empty() || gw.conn(0).lock().unwrap().sent_at.is_empty() {
        assert!(t.elapsed() < Duration::from_secs(3));
        sleep(Duration::from_millis(10));
    }
    sleep(Duration::from_millis(100));
    hang.0.store(true, Ordering::SeqCst);
    let old_ev = old.join().unwrap();
    assert!(old_ev.iter().all(|(v, _)| v["type"] != "end" && v["type"] != "error"), "对面已经走了，不再发终止事件：{old_ev:?}");

    // 立刻重发同一个对话。
    let ev = ask_at(&svc, &req("重发的话", "c1"), &gw.url);
    assert_eq!(types(&ev).last(), Some(&"end"), "{ev:?}");
    let c = contents(&body(&gw, 1));
    assert_eq!(c[1..], ["重发的话"], "被取消的那轮（话和半截回复）不在记忆里：{c:?}");

    // 不同的对话也不会被卡。
    let gw2 = spawn_gw(vec![happy()]);
    hang.0.store(false, Ordering::SeqCst);
    let ev = ask_at(&svc, &req("别的对话", "c2"), &gw2.url);
    assert_eq!(terminal(&ev).1, 0);
}

#[test]
fn a_new_ask_for_the_same_conversation_supersedes_the_old_one_which_hears_cancelled_exit_18() {
    let gw = spawn_gw(vec![vec![Step::Headers, delta("旧的。"), Step::Sleep(1500)], happy()]);
    let svc = Arc::new(Service::new(None));
    let (s2, url) = (svc.clone(), gw.url.clone());
    let old = std::thread::spawn(move || ask(&s2, &req("旧话", "c1"), &|| target(&url), &mut Alive));
    let t = Instant::now();
    while gw.conns.lock().unwrap().is_empty() || gw.conn(0).lock().unwrap().sent_at.is_empty() {
        assert!(t.elapsed() < Duration::from_secs(3));
        sleep(Duration::from_millis(10));
    }
    let new = ask_at(&svc, &req("新话", "c1"), &gw.url);
    assert_eq!(terminal(&new).1, 0);
    let old = old.join().unwrap();
    let (e, exit) = terminal(&old);
    assert_eq!((e["code"].as_str(), exit), (Some("cancelled"), 18), "{old:?}");
    // 被取代的旧一轮不入记忆：下一轮只带新话这一轮。
    let gw2 = spawn_gw(vec![happy()]);
    ask_at(&svc, &req("再问", "c1"), &gw2.url);
    assert_eq!(contents(&body(&gw2, 0))[1..], ["新话", "今天挺晴的。出门带伞。", "再问"]);
}

#[test]
fn a_second_conversation_while_one_is_in_flight_is_busy_without_touching_the_gateway() {
    let gw = spawn_gw(vec![vec![Step::Headers, delta("在说。"), Step::Sleep(2500)]]);
    let svc = Arc::new(Service::new(None));
    let (s2, url) = (svc.clone(), gw.url.clone());
    let first = std::thread::spawn(move || ask(&s2, &req("甲", "a"), &|| target(&url), &mut Alive));
    let t = Instant::now();
    while gw.conns.lock().unwrap().is_empty() {
        assert!(t.elapsed() < Duration::from_secs(3));
        sleep(Duration::from_millis(10));
    }
    sleep(Duration::from_millis(300));
    let other = spawn_gw(vec![happy()]);
    let ev = ask_at(&svc, &req("乙", "b"), &other.url);
    assert_eq!(types(&ev), ["error"], "没有 start：根本没受理");
    let (e, exit) = terminal(&ev);
    assert_eq!((e["code"].as_str(), exit, e["retryable"].as_bool()), (Some("busy"), 14, Some(true)));
    assert_eq!(connections(&other), 0, "忙的时候不去碰网关");
    first.join().unwrap();
}

// ---- 配置 / 校验：没发出任何网络请求 ----

#[test]
fn not_enabled_comes_back_with_a_reason_and_no_gateway_traffic() {
    let gw = spawn_gw(vec![happy()]);
    for reason in ["ask_off", "llm_missing", "llm_not_http", "llm_unusable", "host_not_allowed"] {
        let ev = ask(&Service::new(None), &req("话", "c1"), &|| Err(reason), &mut Alive);
        assert_eq!(types(&ev), ["error"]);
        let (e, exit) = terminal(&ev);
        assert_eq!((e["code"].as_str(), e["reason"].as_str(), exit, e["retryable"].as_bool()), (Some("not_enabled"), Some(reason), 11, Some(false)));
    }
    assert_eq!(connections(&gw), 0);
}

#[test]
fn bad_input_is_refused_with_exit_2_before_anything_else() {
    let gw = spawn_gw(vec![happy()]);
    let svc = Service::new(None);
    let mut bad = vec![req("", "c1"), req("字", "bad id"), req(&"字".repeat(501), "c1")];
    bad.push(AskRequest { source: "mic".into(), ..req("话", "c1") });
    bad.push(AskRequest { v: Some(2), ..req("话", "c1") });
    let reasons = ["empty_input", "bad_conversation_id", "too_long", "bad_source", "bad_version"];
    for (r, reason) in bad.iter().zip(reasons) {
        // 即便配置也是关的，输入错误先说（不泄露配置状态给不合法的调用）。
        let ev = ask(&svc, r, &|| Err("ask_off"), &mut Alive);
        let (e, exit) = terminal(&ev);
        assert_eq!((types(&ev).len(), e["code"].as_str(), e["reason"].as_str(), exit, e["retryable"].as_bool()), (1, Some("bad_request"), Some(reason), 2, Some(false)));
    }
    assert_eq!(connections(&gw), 0);
}

// ---- 故障表：每种落在一个码上 ----

#[test]
fn every_failure_lands_on_one_code_with_exactly_one_terminal_event() {
    let cases: Vec<(Vec<Step>, &str, i32)> = vec![
        (vec![Step::Status(401)], "auth", 13),
        (vec![Step::Status(403)], "auth", 13),
        (vec![Step::Status(429)], "busy", 14),
        (vec![Step::Status(503)], "busy", 14),
        (vec![Step::Status(500)], "bad_reply", 17),
        (vec![Step::Headers, Step::Sse("data: {oops\n\n".into()), Step::EndChunks], "bad_reply", 17),
        (vec![Step::Headers, done(), Step::EndChunks], "empty", 16),
        (vec![Step::Headers, Step::Sse(format!("data: {}\n\n", json!({"choices":[{"delta":{"reasoning_content":"只在想"}}]}))), done(), Step::EndChunks], "empty", 16),
    ];
    for (script, code, exit) in cases {
        let gw = spawn_gw(vec![script]);
        let ev = ask_at(&Service::new(None), &req("话", "c1"), &gw.url);
        let (e, x) = terminal(&ev);
        assert_eq!((e["type"].as_str(), e["code"].as_str(), x), (Some("error"), Some(code), exit), "{ev:?}");
        assert_eq!(types(&ev)[0], "start", "网关阶段的失败前面有 start");
    }
    // 联不上。
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/x", l.local_addr().unwrap());
    drop(l);
    let ev = ask_at(&Service::new(None), &req("话", "c1"), &url);
    assert_eq!((terminal(&ev).0["code"].as_str(), terminal(&ev).1), (Some("unreachable"), 12));
    // 连上了但到期没有一句：慢，不是联不上。
    let gw = spawn_gw(vec![vec![Step::Headers, Step::Sleep(5000)]]);
    let mut r = req("话", "c1");
    r.timeout_ms = Some(1000);
    let ev = ask_at(&Service::new(None), &r, &gw.url);
    assert_eq!((terminal(&ev).0["code"].as_str(), terminal(&ev).1), (Some("slow"), 15));
}

// ---- 隐私 ----

#[test]
fn nothing_but_sentence_events_carries_text_and_the_log_is_metadata_only() {
    let d = tempfile::tempdir().unwrap();
    let log = d.path().join("ask.log");
    let gw = spawn_gw(vec![happy(), vec![Step::Status(401)]]);
    let svc = Service::new(Some(log.clone()));
    let mut r = req("绝密听写内容", "c-unique-conv-id");
    r.persona = Some("绝密人设".into());
    let ev = ask_at(&svc, &r, &gw.url);
    let bad = ask_at(&svc, &r, &gw.url);
    for (v, _) in ev.iter().chain(bad.iter()).filter(|(v, _)| v["type"] != "sentence") {
        let s = v.to_string();
        // `start` 里有 dco 自己给的对话 id（设计 §6.1），除此之外没有任何文字。
        assert!(!s.contains("绝密") && !s.contains("挺晴"), "{s}");
    }
    let text = std::fs::read_to_string(&log).unwrap();
    assert_eq!(text.lines().count(), 2, "每次 ask 一行：{text}");
    for bad in ["绝密", "挺晴", "出门", "unique-conv-id", "c-unique"] {
        assert!(!text.contains(bad), "ask.log 里不该有 {bad}：{text}");
    }
    let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!((first["outcome"].as_str(), first["source"].as_str(), first["chars_in"].as_u64()), (Some("end"), Some("voice"), Some(6)));
    assert_eq!(first["chars_out"].as_u64(), Some(("今天挺晴的。出门带伞。").chars().count() as u64));
    let second: Value = serde_json::from_str(text.lines().nth(1).unwrap()).unwrap();
    assert_eq!(second["outcome"], "auth");
}

#[test]
fn a_cancelled_turn_is_logged_as_cancelled_with_no_text() {
    let d = tempfile::tempdir().unwrap();
    let log = d.path().join("ask.log");
    let gw = spawn_gw(vec![vec![Step::Headers, delta("说了一句。"), Step::Sleep(3000)]]);
    let svc = Service::new(Some(log.clone()));
    let hang = Hangup::default();
    let h2 = hang.clone();
    std::thread::spawn(move || {
        sleep(Duration::from_millis(600));
        h2.0.store(true, Ordering::SeqCst);
    });
    ask(&svc, &req("会被取消", "c1"), &|| target(&gw.url), &mut hang.clone());
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("\"cancelled\"") && !text.contains("说了一句") && !text.contains("会被取消"), "{text}");
}

// ---- 结构性 ----

#[test]
fn ask_code_has_no_session_or_daemon_access_and_only_two_files_touch_disk_or_stdout() {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut files: Vec<_> = std::fs::read_dir(format!("{root}/src/ask")).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "rs")).collect();
    files.push(format!("{root}/src/llm/stream.rs").into());
    assert!(files.len() >= 9, "{files:?}");
    for f in files {
        let name = f.file_name().unwrap().to_str().unwrap().to_string();
        let src = std::fs::read_to_string(&f).unwrap();
        let prod: String =
            src.split("#[cfg(test)]").next().unwrap().lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        for bad in ["Request::Input", "SessionManager", "crate::session", "crate::daemon", "crate::client", "Client::connect"] {
            assert!(!prod.contains(bad), "{name} 里出现了 {bad}");
        }
        let io = ["std::fs", "File::", "OpenOptions", "println!", "eprintln!", "print!", "dbg!", "write_all", "stdout()", "stderr()"];
        let allowed = matches!(name.as_str(), "log.rs" | "cli.rs");
        if !allowed {
            for bad in io {
                assert!(!prod.contains(bad), "{name} 不该碰 {bad}（只有 log.rs 写盘、cli.rs 打字）");
            }
        }
    }
}

// ---- 给 ask 让路 ----

struct Counting(Arc<std::sync::atomic::AtomicUsize>);
impl Backend for Counting {
    fn complete(&self, _p: &Prompt) -> Result<String, LlmError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("解释".into())
    }
}

fn prompt() -> Prompt {
    Prompt { system: "s".into(), user: "u".into(), max_tokens: 8, image_png_base64: None, image_mime: None }
}

#[test]
fn other_gateway_users_wait_while_an_ask_is_in_flight_and_go_after() {
    let svc = Service::new(None);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let y = Arc::new(Yielding::new(Arc::new(Counting(calls.clone())), svc.gate().clone()));
    // 没有 ask 在途：直接过。
    assert_eq!(y.complete(&prompt()), Ok("解释".into()));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // ask 在途：别的调用不碰网关，等它结束。
    let ticket = svc.gate().begin("c1").unwrap();
    let y2 = y.clone();
    let waiter = std::thread::spawn(move || y2.complete(&prompt()));
    sleep(Duration::from_millis(400));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "ask 在途时不该放行");
    drop(ticket);
    assert_eq!(waiter.join().unwrap(), Ok("解释".into()));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn a_yielding_caller_gives_up_with_timeout_instead_of_waiting_forever() {
    let svc = Service::new(None);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let y = Yielding::new(Arc::new(Counting(calls.clone())), svc.gate().clone()).with_wait(Duration::from_millis(300));
    let _ticket = svc.gate().begin("c1").unwrap();
    let t = Instant::now();
    assert_eq!(y.complete(&prompt()), Err(LlmError::Timeout));
    assert!(t.elapsed() < Duration::from_secs(2));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "让路期满也不抢：宁可这次没有解释");
}
