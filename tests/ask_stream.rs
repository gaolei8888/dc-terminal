//! `dct ask` 的流式内核（A1）：真的 ureq 传输 + 本机回环上的假 OpenAI 兼容 SSE 网关。
//! 不碰真网关、不出本机。守护进程协议、配置、记忆是 A2，这里用 `commit` 回调和
//! 一个测试自带的小账本演示「取消的那轮不入记忆」。
#![cfg(unix)]
use dct::ask::{run_turn, ClientGone, ErrorCode, Gate, Terminal, Turn, TurnDone, POLL};
use dct::llm::creds::Credential;
use dct::llm::stream::{stream_body, StreamError, StreamOpen, UreqOpen};
use serde_json::json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

// ---- 假网关 ----

enum Step {
    Headers,
    /// 一个 SSE 事件（一个 chunked 块）。
    Sse(String),
    Sleep(u64),
    /// 空的 chunked 终止块。
    EndChunks,
    /// 整个响应一次写完（非 200 用）。
    Status(u16),
}

#[derive(Default)]
struct Conn {
    body: String,
    authed: bool,
    /// 假网关第一次看到对面关了连接的时刻。
    closed_at: Option<Instant>,
    /// 每个 `Sse` 块写出去的时刻。
    sent_at: Vec<Instant>,
    /// 写失败了（对面已关）之后脚本就停了。
    write_failed: bool,
}

struct Gw {
    url: String,
    conns: Arc<Mutex<Vec<Arc<Mutex<Conn>>>>>,
}

impl Gw {
    fn conn(&self, i: usize) -> Arc<Mutex<Conn>> {
        // 先等连接出现，免得断言跑在 accept 前面。
        let t = Instant::now();
        loop {
            if let Some(c) = self.conns.lock().unwrap().get(i).cloned() {
                return c;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "第 {i} 个连接一直没来");
            sleep(Duration::from_millis(10));
        }
    }

    fn wait_closed(&self, i: usize, within: Duration) -> Instant {
        let t = Instant::now();
        loop {
            if let Some(at) = self.conn(i).lock().unwrap().closed_at {
                return at;
            }
            assert!(t.elapsed() < within, "假网关 {within:?} 内没看到第 {i} 个连接被关");
            sleep(Duration::from_millis(5));
        }
    }
}

fn spawn_gw(scripts: Vec<Vec<Step>>) -> Gw {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/chat/completions", l.local_addr().unwrap());
    let conns: Arc<Mutex<Vec<Arc<Mutex<Conn>>>>> = Arc::default();
    let seen = conns.clone();
    std::thread::spawn(move || {
        for script in scripts {
            let Ok((mut s, _)) = l.accept() else { return };
            let conn: Arc<Mutex<Conn>> = Arc::default();
            seen.lock().unwrap().push(conn.clone());
            std::thread::spawn(move || serve(&mut s, script, &conn));
        }
    });
    Gw { url, conns }
}

fn serve(s: &mut TcpStream, script: Vec<Step>, conn: &Arc<Mutex<Conn>>) {
    // 读请求：头 + Content-Length 的正文。
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head_end, len) = loop {
        let n = s.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).to_string();
        if let Some(i) = text.find("\r\n\r\n") {
            let len = text[..i]
                .lines()
                .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                .unwrap_or(0);
            if buf.len() >= i + 4 + len {
                let head = text[..i].to_lowercase();
                conn.lock().unwrap().authed = head.contains("authorization: bearer test-key");
                break (i + 4, len);
            }
        }
    };
    conn.lock().unwrap().body = String::from_utf8_lossy(&buf[head_end..head_end + len]).into();

    // 看着对面：读到 EOF / 错误 = 对面关了。
    let mut watch = s.try_clone().unwrap();
    let c2 = conn.clone();
    std::thread::spawn(move || {
        let mut b = [0u8; 64];
        while watch.read(&mut b).map(|n| n > 0).unwrap_or(false) {}
        c2.lock().unwrap().closed_at.get_or_insert_with(Instant::now);
    });

    for step in script {
        let r = match step {
            Step::Headers => s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Step::Sse(t) => {
                let r = s.write_all(format!("{:x}\r\n{t}\r\n", t.len()).as_bytes());
                conn.lock().unwrap().sent_at.push(Instant::now());
                r
            }
            Step::Sleep(ms) => {
                sleep(Duration::from_millis(ms));
                Ok(())
            }
            Step::EndChunks => s.write_all(b"0\r\n\r\n"),
            Step::Status(code) => s.write_all(
                format!("HTTP/1.1 {code} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").as_bytes(),
            ),
        };
        if r.and_then(|_| s.flush()).is_err() {
            conn.lock().unwrap().write_failed = true;
            return;
        }
    }
}

fn delta(text: &str) -> Step {
    let v = json!({"choices":[{"delta":{"content": text}}]});
    Step::Sse(format!("data: {v}\n\n"))
}

fn done() -> Step {
    Step::Sse("data: [DONE]\n\n".into())
}

// ---- 跑一轮的脚手架 ----

fn opener(url: &str) -> Arc<dyn StreamOpen> {
    Arc::new(UreqOpen::new(url.to_string(), Credential::Key("test-key".into())))
}

fn turn(url: &str, user: &str, deadline_ms: u64) -> Turn {
    let msgs = [json!({"role":"system","content":"固定系统提示"}), json!({"role":"user","content": user})];
    Turn {
        opener: opener(url),
        body: stream_body("m", &msgs, 200),
        started: Instant::now(),
        deadline: Duration::from_millis(deadline_ms),
        poll: POLL,
    }
}

/// 一个永远「还在」的客户端。
struct Alive;
impl ClientGone for Alive {
    fn gone(&mut self) -> bool {
        false
    }
}

#[derive(Default)]
struct Out {
    sentences: Vec<(usize, String, Instant)>,
    commits: Vec<(Vec<String>, bool)>,
}

fn go(t: Turn, gate: &Gate, conv: &str, peer: &mut dyn ClientGone, out: &mut Out) -> Terminal {
    let ticket = gate.begin(conv).unwrap();
    let (sent, commits) = (&mut out.sentences, &mut out.commits);
    run_turn(
        t,
        ticket,
        peer,
        &mut |i, s| {
            sent.push((i, s.to_string(), Instant::now()));
            Ok(())
        },
        |d: &TurnDone| commits.push((d.sentences.to_vec(), d.truncated)),
    )
}

fn texts(o: &Out) -> Vec<&str> {
    o.sentences.iter().map(|(_, s, _)| s.as_str()).collect()
}

fn happy_script() -> Vec<Step> {
    vec![Step::Headers, delta("今天"), delta("挺晴的。出门"), delta("带伞。"), done(), Step::EndChunks]
}

// ---- 正常路径 ----

#[test]
fn sentences_stream_out_as_they_complete_and_the_turn_ends_with_end() {
    let gw = spawn_gw(vec![vec![Step::Headers, delta("第一句。"), Step::Sleep(500), delta("第二句"), delta("没标点"), done(), Step::EndChunks]]);
    let mut out = Out::default();
    let r = go(turn(&gw.url, "你好", 5000), &Gate::default(), "c1", &mut Alive, &mut out);

    assert_eq!(texts(&out), ["第一句。", "第二句没标点"]);
    assert_eq!(out.sentences.iter().map(|s| s.0).collect::<Vec<_>>(), [0, 1]);
    let Terminal::End { sentences: 2, truncated: false, first_ms, total_ms } = r else { panic!("{r:?}") };
    assert!(first_ms < 400 && total_ms >= 500, "first={first_ms} total={total_ms}");
    // 真流：第一句在网关写出第二块**之前**就已经交出来了。
    assert!(out.sentences[0].2 < gw.conn(0).lock().unwrap().sent_at[1], "第一句等到了后面的数据才出来");
    assert_eq!(out.commits, [(vec!["第一句。".to_string(), "第二句没标点".to_string()], false)]);
}

#[test]
fn the_request_is_a_streaming_chat_with_the_bearer_and_no_tools() {
    let gw = spawn_gw(vec![happy_script()]);
    go(turn(&gw.url, "听写的话", 5000), &Gate::default(), "c1", &mut Alive, &mut Out::default());
    let c = gw.conn(0);
    let c = c.lock().unwrap();
    assert!(c.authed);
    let b: serde_json::Value = serde_json::from_str(&c.body).unwrap();
    assert_eq!(b["stream"], true);
    assert_eq!(b["messages"][1]["content"], "听写的话");
    for k in ["tools", "functions", "tool_choice"] {
        assert!(b.get(k).is_none());
    }
}

#[test]
fn think_text_and_reasoning_fields_never_come_out_or_reach_memory() {
    let reasoning = Step::Sse(format!("data: {}\n\n", json!({"choices":[{"delta":{"reasoning_content":"内心戏。"}}]})));
    let gw = spawn_gw(vec![vec![Step::Headers, reasoning, delta("<think>藏。</think>说出口的。"), done(), Step::EndChunks]]);
    let mut out = Out::default();
    go(turn(&gw.url, "x", 5000), &Gate::default(), "c1", &mut Alive, &mut out);
    assert_eq!(texts(&out), ["说出口的。"]);
    assert_eq!(out.commits[0].0, ["说出口的。"]);
}

// ---- 故障表：每种落在一个码上，恰好一个终止结果，失败的轮不入记忆 ----

fn error_of(r: Terminal) -> ErrorCode {
    match r {
        Terminal::Error { code, .. } => code,
        other => panic!("要 Error，拿到 {other:?}"),
    }
}

#[test]
fn http_statuses_map_to_auth_busy_and_bad_reply() {
    for (status, want) in [(401, ErrorCode::Auth), (403, ErrorCode::Auth), (429, ErrorCode::Busy), (503, ErrorCode::Busy), (500, ErrorCode::BadReply)] {
        let gw = spawn_gw(vec![vec![Step::Status(status)]]);
        let mut out = Out::default();
        let r = go(turn(&gw.url, "x", 5000), &Gate::default(), "c1", &mut Alive, &mut out);
        assert_eq!(error_of(r), want, "HTTP {status}");
        assert!(out.sentences.is_empty() && out.commits.is_empty());
    }
}

#[test]
fn a_refused_connection_is_unreachable() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/x", l.local_addr().unwrap());
    drop(l);
    let r = go(turn(&url, "x", 5000), &Gate::default(), "c1", &mut Alive, &mut Out::default());
    assert_eq!(error_of(r), ErrorCode::Unreachable);
}

#[test]
fn a_gateway_that_accepts_but_never_answers_is_slow_not_unreachable() {
    let gw = spawn_gw(vec![vec![Step::Sleep(5000)]]);
    let t0 = Instant::now();
    let mut out = Out::default();
    let r = go(turn(&gw.url, "x", 600), &Gate::default(), "c1", &mut Alive, &mut out);
    assert_eq!(error_of(r), ErrorCode::Slow);
    assert!(t0.elapsed() < Duration::from_millis(1500));
    assert!(out.commits.is_empty());
    // 期限到了，到网关的连接也要关（ureq 的整体期限让阻塞的读返回，reader 被丢掉）。
    gw.wait_closed(0, Duration::from_millis(1500));
    // 直接问传输层：同一个判据。
    let gw = spawn_gw(vec![vec![Step::Sleep(3000)]]);
    let e = UreqOpen::new(gw.url.clone(), Credential::Key("test-key".into())).open(&json!({}), Duration::from_millis(300)).err();
    assert_eq!(e, Some(StreamError::Slow));
}

#[test]
fn silence_after_the_headers_before_any_sentence_is_slow() {
    let gw = spawn_gw(vec![vec![Step::Headers, Step::Sleep(5000)]]);
    let r = go(turn(&gw.url, "x", 500), &Gate::default(), "c1", &mut Alive, &mut Out::default());
    assert_eq!(error_of(r), ErrorCode::Slow);
    gw.wait_closed(0, Duration::from_millis(1500));
}

#[test]
fn a_reply_with_only_thinking_or_nothing_visible_is_empty() {
    let thinking = Step::Sse(format!("data: {}\n\n", json!({"choices":[{"delta":{"reasoning":"只有思考"}}]})));
    for body in [vec![Step::Headers, thinking, done(), Step::EndChunks], vec![Step::Headers, delta("**"), delta("  "), done(), Step::EndChunks], vec![Step::Headers, delta("<think>全是思考。</think>"), done(), Step::EndChunks]] {
        let gw = spawn_gw(vec![body]);
        let mut out = Out::default();
        let r = go(turn(&gw.url, "x", 5000), &Gate::default(), "c1", &mut Alive, &mut out);
        assert_eq!(error_of(r), ErrorCode::Empty);
        assert!(out.sentences.is_empty() && out.commits.is_empty());
    }
}

#[test]
fn unreadable_streams_are_bad_reply() {
    let scripts = vec![
        vec![Step::Headers, Step::Sse("data: {不是json\n\n".into()), Step::EndChunks],
        vec![Step::Headers, Step::Sse("data: {\"error\":{\"message\":\"boom\"}}\n\n".into()), Step::EndChunks],
        vec![Step::Headers, Step::Sse(format!("data: {}\n\n", "x".repeat(70_000))), Step::EndChunks],
        // 没有任何内容就断了，也没有 [DONE]。
        vec![Step::Headers, Step::EndChunks],
    ];
    for s in scripts {
        let gw = spawn_gw(vec![s]);
        let mut out = Out::default();
        let r = go(turn(&gw.url, "x", 5000), &Gate::default(), "c1", &mut Alive, &mut out);
        assert_eq!(error_of(r), ErrorCode::BadReply);
        assert!(out.commits.is_empty());
    }
}

#[test]
fn a_stream_cut_after_some_sentences_ends_truncated_and_drops_the_half_sentence() {
    // 没有 [DONE]：流自己停了。没写完的「半句」不发，入记忆的是已说出的。
    let gw = spawn_gw(vec![vec![Step::Headers, delta("说完的一句。没说完"), Step::EndChunks]]);
    let mut out = Out::default();
    let r = go(turn(&gw.url, "x", 5000), &Gate::default(), "c1", &mut Alive, &mut out);
    assert!(matches!(r, Terminal::End { sentences: 1, truncated: true, .. }), "{r:?}");
    assert_eq!(texts(&out), ["说完的一句。"]);
    assert_eq!(out.commits, [(vec!["说完的一句。".to_string()], true)]);
}

#[test]
fn the_deadline_after_some_sentences_ends_truncated_and_closes_the_connection() {
    let gw = spawn_gw(vec![vec![Step::Headers, delta("先说一句。再说"), Step::Sleep(5000)]]);
    let mut out = Out::default();
    let r = go(turn(&gw.url, "x", 700), &Gate::default(), "c1", &mut Alive, &mut out);
    assert!(matches!(r, Terminal::End { sentences: 1, truncated: true, .. }), "{r:?}");
    assert_eq!(texts(&out), ["先说一句。"]);
    assert_eq!(out.commits, [(vec!["先说一句。".to_string()], true)]);
    gw.wait_closed(0, Duration::from_millis(1500));
}

// ---- 取消：关连接 = 取消 ----

/// 客户端：一对 socket，`daemon` 那头交给 run_turn 当 peer，`client` 那头由测试关。
fn pair() -> (UnixStream, UnixStream) {
    UnixStream::pair().unwrap()
}

#[test]
fn the_unix_peer_check_sees_eof_but_not_data_or_silence() {
    let (mut daemon, mut client) = pair();
    assert!(!daemon.gone());
    client.write_all(b"x").unwrap(); // 有数据 ≠ 关了
    assert!(!daemon.gone());
    let mut b = [0u8; 1];
    daemon.read_exact(&mut b).unwrap();
    drop(client);
    assert!(daemon.gone());
}

#[test]
fn closing_the_client_between_sentences_is_found_by_polling_and_closes_the_gateway_connection() {
    // 网关一边吐句子一边等；句子之间的空档里没有写，只能靠 EOF 轮询发现。
    let mut script = vec![Step::Headers, delta("第一句。")];
    for _ in 0..40 {
        script.push(Step::Sleep(100));
        script.push(delta("还有。"));
    }
    let gw = spawn_gw(vec![script]);
    let (mut daemon, client) = pair();
    let closed = Arc::new(Mutex::new(None));
    let c2 = closed.clone();
    std::thread::spawn(move || {
        sleep(Duration::from_millis(500));
        *c2.lock().unwrap() = Some(Instant::now());
        drop(client);
    });
    let mut out = Out::default();
    let r = go(turn(&gw.url, "x", 20_000), &Gate::default(), "c1", &mut daemon, &mut out);
    let returned = Instant::now();
    assert_eq!(r, Terminal::Gone);
    assert_eq!(r.event(), None, "对面不读了，不发终止事件");
    let closed_at = closed.lock().unwrap().unwrap();
    assert!(returned.duration_since(closed_at) < POLL + Duration::from_millis(250), "一个轮询周期内要发现");
    // 取消的那轮不入记忆。
    assert!(out.commits.is_empty());
    // 到网关的连接被关；之后 run_turn 不再往外交任何东西。
    let gw_closed = gw.wait_closed(0, Duration::from_millis(1500));
    assert!(gw_closed.duration_since(closed_at) < Duration::from_millis(900));
    let n = out.sentences.len();
    sleep(Duration::from_millis(500));
    assert_eq!(out.sentences.len(), n);
}

#[test]
fn cancelling_during_prefill_is_prompt_for_the_caller_and_closes_when_the_first_bytes_land() {
    // 响应头来了，首个增量要 1.5 秒（预填）。这期间工作线程阻塞在读上，ureq 2.x
    // 没法从别的线程打断它；run_turn 本身一个轮询周期内返回，连接在首个字节到达时关。
    let gw = spawn_gw(vec![vec![Step::Headers, Step::Sleep(1500), delta("迟到的一句。"), Step::Sleep(100), delta("更迟。"), Step::Sleep(3000)]]);
    let (mut daemon, client) = pair();
    let t0 = Instant::now();
    std::thread::spawn(move || {
        sleep(Duration::from_millis(300));
        drop(client);
    });
    let mut out = Out::default();
    let r = go(turn(&gw.url, "x", 20_000), &Gate::default(), "c1", &mut daemon, &mut out);
    assert_eq!(r, Terminal::Gone);
    assert!(t0.elapsed() < Duration::from_millis(300) + POLL + Duration::from_millis(250));
    assert!(out.sentences.is_empty() && out.commits.is_empty());
    let first_bytes = Instant::now();
    let closed = gw.wait_closed(0, Duration::from_millis(3000));
    // 首个增量到达后（≈ t0+1.5s）关；网关随后的写会失败，后面的句子到不了。
    assert!(closed.duration_since(t0) < Duration::from_millis(2300), "{:?}", closed.duration_since(t0));
    let _ = first_bytes;
    assert!(out.sentences.is_empty());
}

#[test]
fn after_a_cancel_the_same_conversation_can_resend_at_once_and_is_never_busy() {
    // 旧连接故意晚关：网关在第一句之后沉默，工作线程阻塞在读上，还活着。
    let gw = spawn_gw(vec![
        vec![Step::Headers, delta("旧的第一句。"), Step::Sleep(3000), delta("旧的尾巴。")],
        happy_script(),
    ]);
    let gate = Gate::default();
    let (mut daemon, client) = pair();
    let client = Mutex::new(Some(client));
    let ticket = gate.begin("c1").unwrap();
    let mut got = vec![];
    let r = run_turn(
        turn(&gw.url, "旧话", 20_000),
        ticket,
        &mut daemon,
        &mut |_, s| {
            got.push(s.to_string());
            client.lock().unwrap().take(); // 第 1 句一到就关客户端
            Ok(())
        },
        |_| panic!("取消的轮次不该提交"),
    );
    assert_eq!(r, Terminal::Gone);

    // **立刻**重发：不 sleep，不等后台线程。此刻假网关还没看到旧连接被关，
    // 证明放行靠的是同步清标记，不是旧线程恰好先退出。
    assert!(gw.conn(0).lock().unwrap().closed_at.is_none(), "旧连接应该还开着（这条测试要的就是这个前提）");
    let mut out = Out::default();
    let r = go(turn(&gw.url, "新话", 5000), &gate, "c1", &mut Alive, &mut out);
    assert!(matches!(r, Terminal::End { truncated: false, .. }), "重发撞上了 {r:?}");
    assert_eq!(texts(&out), ["今天挺晴的。", "出门带伞。"]);
    // 记忆：只有新轮次提交，旧的那句和它的半截回复不在里面。
    assert_eq!(out.commits.len(), 1);
    assert!(!out.commits[0].0.concat().contains("旧"));
    assert!({ let b = gw.conn(1).lock().unwrap().body.clone(); b.contains("新话") && !b.contains("旧") });
}

#[test]
fn after_a_cancel_a_different_conversation_is_not_blocked_either() {
    let gw = spawn_gw(vec![vec![Step::Headers, delta("第一句。"), Step::Sleep(3000)], happy_script()]);
    let gate = Gate::default();
    let (mut daemon, client) = pair();
    let client = Mutex::new(Some(client));
    let ticket = gate.begin("a").unwrap();
    let r = run_turn(turn(&gw.url, "x", 20_000), ticket, &mut daemon, &mut |_, _| { client.lock().unwrap().take(); Ok(()) }, |_| panic!());
    assert_eq!(r, Terminal::Gone);
    let r = go(turn(&gw.url, "y", 5000), &gate, "b", &mut Alive, &mut Out::default());
    assert!(matches!(r, Terminal::End { .. }), "{r:?}");
}

#[test]
fn a_new_ask_for_the_same_conversation_supersedes_the_old_one_with_cancelled() {
    let gw = spawn_gw(vec![vec![Step::Headers, delta("旧的。"), Step::Sleep(3000)]]);
    let gate = Gate::default();
    let g2 = gate.clone();
    let old = std::thread::spawn(move || {
        let mut out = Out::default();
        let r = go(turn(&gw.url, "旧", 20_000), &g2, "c1", &mut Alive, &mut out);
        (r, out.commits.len())
    });
    sleep(Duration::from_millis(400));
    // 同对话新请求：拿到票，旧的被取代。
    let new = gate.begin("c1").expect("同对话的新请求不该 Busy");
    let (r, commits) = old.join().unwrap();
    assert_eq!(error_of(r.clone()), ErrorCode::Cancelled);
    assert_eq!(r.event().unwrap()["code"], "cancelled");
    assert_eq!(commits, 0);
    // 旧的返回后，新的票仍然有效（旧票的 Drop 不能清掉新票的标记）。
    assert!(gate.begin("other").is_err(), "新票还在途，别的对话要 Busy");
    drop(new);
    assert!(gate.begin("other").is_ok());
}

#[test]
fn a_different_conversation_while_one_is_in_flight_is_busy() {
    let gate = Gate::default();
    let t = gate.begin("a").unwrap();
    assert!(gate.begin("b").is_err());
    drop(t); // 出错路径丢掉票也清标记
    assert!(gate.begin("b").is_ok());
}

#[test]
fn cancel_and_natural_end_race_has_exactly_one_winner() {
    // 先取消：complete 不提交；先完成：之后的取消改不了已提交的记忆。
    let gate = Gate::default();
    let t = gate.begin("c").unwrap();
    t.cancel();
    let mut committed = false;
    assert!(!t.complete(|| committed = true));
    assert!(!committed);
    assert!(gate.begin("other").is_ok(), "取消同步清了标记");

    let gate = Gate::default();
    let t = gate.begin("c").unwrap();
    let mut committed = false;
    assert!(t.complete(|| committed = true));
    assert!(committed);
}

// ---- 结构性 / 隐私：本模块不碰会话、不写盘、不打印 ----

#[test]
fn the_ask_core_has_no_session_input_disk_or_print_paths() {
    let root = env!("CARGO_MANIFEST_DIR");
    for f in ["src/ask/mod.rs", "src/ask/split.rs", "src/llm/stream.rs"] {
        let src = std::fs::read_to_string(format!("{root}/{f}")).unwrap();
        // 产品代码，去掉注释（注释里会解释「为什么不碰 Request::Input」）。
        let prod: String = src
            .split("#[cfg(test)]")
            .next()
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for bad in ["Request::Input", "SessionManager", "crate::session", "crate::daemon", "std::fs", "File::", "println!", "eprintln!", "dbg!", "write_all"] {
            assert!(!prod.contains(bad), "{f} 的产品代码里出现了 {bad}");
        }
    }
}
