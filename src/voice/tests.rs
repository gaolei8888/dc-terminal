use super::*;
use std::cell::RefCell;
use std::rc::Rc;

fn sess(id: u32, tag: &str, state: SessionState, last: u64) -> SessionInfo {
    SessionInfo { id, profile: "claude".into(), dir: "/d".into(), state, activity: String::new(), is_agent: true, tag: tag.into(), last_active_ms: last }
}

fn utt(seq: u64, text: &str, conf: f64) -> Utterance {
    Utterance { seq, text: text.into(), confidence: conf, trigger: ALLOWED_TRIGGER.into() }
}

// ---------- 纠错 ----------

#[test]
fn alias_replaces_whole_word_only() {
    let cfg = Config::default();
    assert_eq!(fix_aliases("张瑜，帮我看看", &cfg), "章鱼，帮我看看");
    assert_eq!(fix_aliases("给张宇说你好", &cfg), "给章鱼说你好");
    assert_eq!(fix_aliases("请张瑜帮我", &cfg), "请章鱼帮我");
    assert_eq!(fix_aliases("张瑜", &cfg), "章鱼");
}

#[test]
fn alias_does_not_replace_substrings() {
    let cfg = Config::default();
    assert_eq!(fix_aliases("我在练张瑜伽", &cfg), "我在练张瑜伽");
    assert_eq!(fix_aliases("大张瑜", &cfg), "大张瑜");
    let mut c = Config { alias: BTreeMap::new() };
    c.alias.insert("octo".into(), vec!["ok to".into()]);
    assert_eq!(fix_aliases("ok to, hi", &c), "octo, hi");
    assert_eq!(fix_aliases("book to read", &c), "book to read");
    assert_eq!(fix_aliases("ok tomorrow", &c), "ok tomorrow");
}

#[test]
fn text_without_alias_is_untouched() {
    assert_eq!(fix_aliases("你好，请帮我看一下现在的目录", &Config::default()), "你好，请帮我看一下现在的目录");
    assert_eq!(fix_aliases("anything", &Config { alias: BTreeMap::new() }), "anything");
}

#[test]
fn config_parses_and_rejects_garbage() {
    let c = parse_config("[alias]\n章鱼 = [\"a\", \"b\"]\n").unwrap();
    assert_eq!(c.alias["章鱼"], vec!["a", "b"]);
    assert!(parse_config("[alias\n").is_err());
    assert!(parse_config("[alias]\n章鱼 = 3\n").is_err());
    assert!(parse_config("").unwrap().alias.is_empty());
}

// ---------- 过闸 ----------

#[test]
fn bare_confirmation_words_are_detected() {
    for t in ["好", "好的", "好的。", "嗯", "嗯嗯", "嗯嗯嗯", "是", "确认", "同意", "可以", "行", "yes", "Yes!", "OK", "ok.", "y", " 好 ", "好，", "确认。"] {
        assert!(is_bare_confirmation(t), "{t}");
    }
}

#[test]
fn sentences_containing_confirmation_are_not_bare() {
    for t in ["好的，继续做", "可以帮我看看目录吗", "是不是这样", "yes please run the tests", "嗯，把它删了", "", "。", "好好学习"] {
        assert!(!is_bare_confirmation(t), "{t}");
    }
}

#[test]
fn cancel_phrases_are_whole_sentence() {
    for t in ["取消", "取消。", "算了", "不要发", "别发", "别发了", "Cancel"] {
        assert!(is_cancel(t), "{t}");
    }
    for t in ["取消订单这个功能怎么做", "算了算了不要紧", "你好"] {
        assert!(!is_cancel(t), "{t}");
    }
}

#[test]
fn only_asking_state_counts_as_waiting_for_approval() {
    assert!(waiting_for_approval(&sess(1, "a", SessionState::Asking, 0)));
    for st in [SessionState::Working, SessionState::Idle, SessionState::Failed, SessionState::Unknown] {
        assert!(!waiting_for_approval(&sess(1, "a", st, 0)), "{st:?}");
    }
}

// ---------- 路由 ----------

fn two() -> Vec<SessionInfo> {
    vec![sess(1, "dc-octo", SessionState::Idle, 100), sess(2, "Claude Main", SessionState::Idle, 500)]
}

#[test]
fn default_route_is_most_recently_active() {
    match route("你好", &two()) {
        Route::To { id, text, .. } => assert_eq!((id, text.as_str()), (2, "你好")),
        r => panic!("{r:?}"),
    }
    // 顺序反过来也一样：看的是 last_active，不是位置
    let mut v = two();
    v.reverse();
    assert!(matches!(route("你好", &v), Route::To { id: 2, .. }));
    let mut v = two();
    v[0].last_active_ms = 900;
    assert!(matches!(route("你好", &v), Route::To { id: 1, .. }));
}

#[test]
fn explicit_names_with_each_prefix() {
    for (said, want) in [("给dc-octo说，帮我看下", "帮我看下"), ("对 dc octo 说：帮我看下", "帮我看下"), ("告诉dc-octo 帮我看下", "帮我看下"), ("告诉DCOCTO，帮我看下", "帮我看下"), ("给DC-Octo说帮我看下", "帮我看下")] {
        match route(said, &two()) {
            Route::To { id, text, name } => assert_eq!((id, text.as_str(), name.as_str()), (1, want, "dc-octo"), "{said}"),
            r => panic!("{said}: {r:?}"),
        }
    }
}

#[test]
fn explicit_name_ignores_case_spaces_and_hyphens() {
    assert!(matches!(route("给claude main说你好", &two()), Route::To { id: 2, .. }));
    assert!(matches!(route("给 CLAUDE-MAIN 说你好", &two()), Route::To { id: 2, .. }));
    assert!(matches!(route("给claudemain说你好", &two()), Route::To { id: 2, .. }));
}

#[test]
fn explicit_by_number() {
    assert!(matches!(route("给#1说你好", &two()), Route::To { id: 1, .. }));
    assert!(matches!(route("告诉#2 你好", &two()), Route::To { id: 2, .. }));
    // #1 不能吃掉 #12 的开头
    let mut v = two();
    v.push(sess(12, "x", SessionState::Idle, 0));
    match route("给#12说你好", &v) {
        Route::To { id, text, .. } => assert_eq!((id, text.as_str()), (12, "你好")),
        r => panic!("{r:?}"),
    }
}

#[test]
fn unknown_name_falls_back_to_default_and_keeps_text() {
    match route("告诉我现在几点", &two()) {
        Route::To { id, text, .. } => assert_eq!((id, text.as_str()), (2, "告诉我现在几点")),
        r => panic!("{r:?}"),
    }
    assert!(matches!(route("对不起我说错了", &two()), Route::To { id: 2, .. }));
}

#[test]
fn several_matches_are_ambiguous() {
    let v = vec![sess(1, "claude", SessionState::Idle, 1), sess(2, "claude", SessionState::Idle, 2)];
    assert_eq!(route("给claude说你好", &v), Route::Ambiguous);
    // 编号能分开
    assert!(matches!(route("给#1说你好", &v), Route::To { id: 1, .. }));
}

#[test]
fn no_session_means_no_route() {
    assert_eq!(route("你好", &[]), Route::NoSession);
    let stopped = vec![sess(1, "a", SessionState::Stopped, 9)];
    assert_eq!(route("你好", &stopped), Route::NoSession);
}

#[test]
fn shell_sessions_never_receive_voice() {
    let mut shell = sess(3, "sh", SessionState::Idle, 9999);
    shell.is_agent = false;
    let v = vec![sess(1, "a", SessionState::Idle, 1), shell.clone()];
    assert!(matches!(route("你好", &v), Route::To { id: 1, .. }));
    assert_eq!(route("你好", &[shell]), Route::NoSession);
}

#[test]
fn hear_reply_is_parsed() {
    let v = json!({"utterances":[{"seq":3,"text":"你好","confidence":0.9,"language":"zh","source":"voice","trigger":"octopus_click"},{"seq":4,"text":"没置信度"},{"nope":1}]});
    let u = parse_hear(&v);
    let mut no_trigger = utt(4, "没置信度", 0.0);
    no_trigger.trigger = String::new();
    assert_eq!(u, vec![utt(3, "你好", 0.9), no_trigger]);
    assert!(parse_hear(&json!({})).is_empty());
}

#[test]
fn outgoing_has_prefix_and_no_control_chars() {
    assert_eq!(outgoing("看目录"), "（语音输入，可能有识别错误）看目录");
    assert_eq!(outgoing("a\x1bb\rc\nd"), "（语音输入，可能有识别错误）abcd");
}

// ---------- 假的外部世界 ----------

#[derive(Default)]
struct World {
    now: u64,
    /// 每次 hear 调用的剧本：到了 (at_ms) 就把这些话交出去。
    script: Vec<(u64, Utterance)>,
    hear_calls: Vec<(u64, u64)>,
    sessions: Vec<SessionInfo>,
    /// 在这个时刻之后，把会话 id 的状态改成 Asking。
    flip_to_asking: Option<(u64, u32)>,
    sent: Vec<(u64, u32, String)>,
    said: Vec<String>,
    errors: Vec<String>,
    log: Vec<Value>,
    hear_error: Option<HearError>,
    /// 第一次 hear（取启动前积压）交出的话。
    backlog: Vec<Utterance>,
    first: bool,
}

type W = Rc<RefCell<World>>;

struct FHearer(W);
impl Hearer for FHearer {
    fn hear(&mut self, since: u64, wait: u64) -> Result<Vec<Utterance>, HearError> {
        let mut w = self.0.borrow_mut();
        w.hear_calls.push((since, wait));
        if let Some(e) = w.hear_error.clone() {
            return Err(e);
        }
        if !w.first {
            w.first = true;
            return Ok(std::mem::take(&mut w.backlog));
        }
        // 到点的先交出去；没有的话，等到最早的一句（只要在 wait 之内），否则白等 wait。
        let now = w.now;
        let due = |at: u64, u: &Utterance, t: u64| at <= t && u.seq > since;
        let mut t = now;
        if !w.script.iter().any(|(at, u)| due(*at, u, now)) {
            match w.script.iter().filter(|(at, u)| u.seq > since && *at <= now + wait).map(|(at, _)| *at).min() {
                Some(at) => t = at,
                None => {
                    w.now += wait;
                    return Ok(vec![]);
                }
            }
        }
        w.now = t;
        let mut out = vec![];
        let mut rest = vec![];
        for (at, u) in std::mem::take(&mut w.script) {
            if due(at, &u, t) {
                out.push(u);
            } else {
                rest.push((at, u));
            }
        }
        w.script = rest;
        Ok(out)
    }
}
struct FDaemon(W);
impl Daemon for FDaemon {
    fn sessions(&mut self) -> Result<Vec<SessionInfo>, String> {
        let mut w = self.0.borrow_mut();
        if let Some((at, id)) = w.flip_to_asking {
            if w.now >= at {
                for s in w.sessions.iter_mut().filter(|s| s.id == id) {
                    s.state = SessionState::Asking;
                }
            }
        }
        Ok(w.sessions.clone())
    }
    fn type_into(&mut self, id: u32, text: &str) -> Result<(), String> {
        let mut w = self.0.borrow_mut();
        let now = w.now;
        w.sent.push((now, id, text.to_string()));
        Ok(())
    }
}
struct FClock(W);
impl Clock for FClock {
    fn now_ms(&self) -> u64 {
        self.0.borrow().now
    }
    fn sleep_ms(&mut self, ms: u64) {
        self.0.borrow_mut().now += ms;
    }
}
struct FOut(W);
impl Out for FOut {
    fn say(&mut self, l: &str) {
        self.0.borrow_mut().said.push(l.into());
    }
    fn error(&mut self, l: &str) {
        self.0.borrow_mut().errors.push(l.into());
    }
}
struct FLog(W);
impl Log for FLog {
    fn append(&mut self, r: &Value) {
        self.0.borrow_mut().log.push(r.clone());
    }
}

fn world(sessions: Vec<SessionInfo>, script: Vec<(u64, Utterance)>) -> W {
    Rc::new(RefCell::new(World { now: 10_000, script, sessions, ..Default::default() }))
}

fn go(w: &W, hears: usize) -> i32 {
    let (mut h, mut d, mut c, mut o, mut l) = (FHearer(w.clone()), FDaemon(w.clone()), FClock(w.clone()), FOut(w.clone()), FLog(w.clone()));
    let cfg = Config::default();
    Relay::new(&mut h, &mut d, &mut c, &mut o, &mut l, &cfg).run(Some(hears))
}

fn actions(w: &W) -> Vec<String> {
    w.borrow().log.iter().map(|r| r["action"].as_str().unwrap().to_string()).collect()
}

fn idle_one() -> Vec<SessionInfo> {
    vec![sess(1, "dc-octo", SessionState::Idle, 100)]
}

// ---------- 闸 ----------

#[test]
fn happy_path_sends_after_window_with_prefix() {
    let w = world(idle_one(), vec![(10_000, utt(1, "你好，请帮我看一下现在的目录", 0.9))]);
    assert_eq!(go(&w, 3), 0);
    let b = w.borrow();
    assert_eq!(b.sent.len(), 1);
    assert_eq!((b.sent[0].1, b.sent[0].2.as_str()), (1, "（语音输入，可能有识别错误）你好，请帮我看一下现在的目录"));
    assert!(b.said.iter().any(|s| s.contains("听到：「你好，请帮我看一下现在的目录」 → 发给：dc-octo")));
    assert_eq!(b.log.last().unwrap()["action"], "sent");
    assert_eq!(b.log.last().unwrap()["target"], "dc-octo");
}

#[test]
fn low_confidence_is_blocked_at_the_edge() {
    for (c, sent) in [(0.49, 0), (0.5, 1)] {
        let w = world(idle_one(), vec![(10_000, utt(1, "帮我看目录", c))]);
        go(&w, 3);
        assert_eq!(w.borrow().sent.len(), sent, "confidence {c}");
        if sent == 0 {
            assert_eq!(actions(&w), ["low_confidence"]);
            assert!(w.borrow().said.iter().any(|s| s == "没听清，再说一遍？"));
        }
    }
}

#[test]
fn bare_confirmations_are_never_sent() {
    for t in ["好", "好的。", "嗯嗯", "同意", "确认", "OK", "yes"] {
        let w = world(idle_one(), vec![(10_000, utt(1, t, 0.95))]);
        go(&w, 3);
        assert!(w.borrow().sent.is_empty(), "{t}");
        assert_eq!(actions(&w), ["blocked_confirm"], "{t}");
        assert!(w.borrow().said.iter().any(|s| s.contains("键盘上确认")));
    }
}

#[test]
fn confirmation_hidden_behind_a_routing_prefix_is_still_blocked() {
    let w = world(idle_one(), vec![(10_000, utt(1, "给dc-octo说好", 0.95))]);
    go(&w, 3);
    assert!(w.borrow().sent.is_empty());
    assert_eq!(actions(&w), ["blocked_confirm"]);
}

#[test]
fn sentence_that_starts_with_confirmation_is_sent() {
    let w = world(idle_one(), vec![(10_000, utt(1, "好的，继续做", 0.95))]);
    go(&w, 3);
    assert_eq!(w.borrow().sent.len(), 1);
}

#[test]
fn session_waiting_for_approval_gets_nothing() {
    let w = world(vec![sess(1, "dc-octo", SessionState::Asking, 100)], vec![(10_000, utt(1, "把它删掉", 0.95))]);
    go(&w, 3);
    assert!(w.borrow().sent.is_empty());
    assert_eq!(actions(&w), ["blocked_prompt"]);
    assert!(w.borrow().said.iter().any(|s| s.contains("键盘上确认")));
}

#[test]
fn approval_prompt_appearing_during_cancel_window_still_blocks() {
    let w = world(idle_one(), vec![(10_000, utt(1, "帮我看目录", 0.95))]);
    w.borrow_mut().flip_to_asking = Some((11_000, 1));
    go(&w, 3);
    assert!(w.borrow().sent.is_empty());
    assert_eq!(actions(&w), ["blocked_prompt"]);
    assert_eq!(w.borrow().log[0]["reason"], "waiting_for_approval_at_send");
}

#[test]
fn explicit_target_waiting_is_blocked_even_if_default_is_free() {
    let v = vec![sess(1, "a", SessionState::Asking, 1), sess(2, "b", SessionState::Idle, 9)];
    let w = world(v, vec![(10_000, utt(1, "给a说你好", 0.95))]);
    go(&w, 3);
    assert!(w.borrow().sent.is_empty());
    assert_eq!(actions(&w), ["blocked_prompt"]);
}

#[test]
fn no_session_and_ambiguous_are_reported() {
    let w = world(vec![], vec![(10_000, utt(1, "你好", 0.9))]);
    go(&w, 3);
    assert_eq!(actions(&w), ["no_session"]);
    assert!(w.borrow().said.iter().any(|s| s == "现在没有开着的会话"));
    let v = vec![sess(1, "c", SessionState::Idle, 1), sess(2, "c", SessionState::Idle, 2)];
    let w = world(v, vec![(10_000, utt(1, "给c说你好", 0.9))]);
    go(&w, 3);
    assert_eq!(actions(&w), ["ambiguous"]);
    assert!(w.borrow().said.iter().any(|s| s == "有几个叫这个名字的会话，说编号"));
    assert!(w.borrow().sent.is_empty());
}

#[test]
fn alias_is_fixed_before_routing_and_both_texts_are_logged() {
    let v = vec![sess(1, "章鱼", SessionState::Idle, 100), sess(2, "other", SessionState::Idle, 900)];
    let w = world(v, vec![(10_000, utt(1, "给张瑜说看一下目录", 0.9))]);
    go(&w, 3);
    let b = w.borrow();
    assert_eq!(b.sent[0].1, 1);
    assert_eq!(b.log[0]["text_raw"], "给张瑜说看一下目录");
    assert_eq!(b.log[0]["text_fixed"], "给章鱼说看一下目录");
}

#[test]
fn rate_limit_allows_one_sentence_per_two_seconds() {
    // 第二句在第一句送出的同一时刻就到了（取消窗口里来的别的话）：不送
    let w = world(idle_one(), vec![(10_000, utt(1, "第一句", 0.9)), (10_500, utt(2, "第二句", 0.9))]);
    go(&w, 4);
    let b = w.borrow();
    assert_eq!(b.sent.len(), 1, "{:?}", b.sent);
    assert!(b.sent[0].2.contains("第一句"));
    assert_eq!(b.log.iter().filter(|r| r["reason"] == "rate_limited").count(), 1);
}

#[test]
fn a_later_sentence_after_the_gap_is_sent() {
    let w = world(idle_one(), vec![(10_000, utt(1, "第一句", 0.9)), (16_000, utt(2, "第二句", 0.9))]);
    go(&w, 6);
    assert_eq!(w.borrow().sent.len(), 2);
}

// ---------- 预告与取消 ----------

#[test]
fn cancel_inside_window_drops_the_sentence_and_itself() {
    let w = world(idle_one(), vec![(10_000, utt(1, "把测试全删了", 0.9)), (11_000, utt(2, "取消", 0.9))]);
    go(&w, 4);
    let b = w.borrow();
    assert!(b.sent.is_empty(), "{:?}", b.sent);
    assert_eq!(actions(&w).as_slice(), ["cancelled"]);
    assert!(b.said.iter().any(|s| s == "已取消"));
}

#[test]
fn other_speech_during_window_does_not_cancel_and_is_not_lost() {
    let w = world(idle_one(), vec![(10_000, utt(1, "第一句话", 0.9)), (11_000, utt(2, "再来一句", 0.9))]);
    go(&w, 4);
    let b = w.borrow();
    assert!(b.sent[0].2.contains("第一句话"));
    // 第二句被处理了（这里因限速不送，但有记录，没有悄悄消失）
    assert_eq!(b.log.len(), 2);
}

#[test]
fn nothing_is_sent_before_the_window_ends() {
    let w = world(idle_one(), vec![(10_000, utt(1, "看目录", 0.9))]);
    go(&w, 3);
    let b = w.borrow();
    assert_eq!(b.sent.len(), 1);
    assert!(b.sent[0].0 >= 12_000, "sent at {}", b.sent[0].0);
}

#[test]
fn cancel_right_at_window_end_is_too_late_but_just_before_is_in_time() {
    let w = world(idle_one(), vec![(10_000, utt(1, "看目录", 0.9)), (11_990, utt(2, "算了", 0.9))]);
    go(&w, 4);
    assert!(w.borrow().sent.is_empty());
    let w = world(idle_one(), vec![(10_000, utt(1, "看目录", 0.9)), (12_100, utt(2, "算了", 0.9))]);
    go(&w, 4);
    assert_eq!(w.borrow().sent.len(), 1);
}

#[test]
fn stray_cancel_with_nothing_pending_is_not_sent_to_a_session() {
    let w = world(idle_one(), vec![(10_000, utt(1, "取消", 0.9))]);
    go(&w, 3);
    assert!(w.borrow().sent.is_empty());
}

#[test]
fn low_confidence_cancel_still_cancels() {
    let w = world(idle_one(), vec![(10_000, utt(1, "看目录", 0.9)), (11_000, utt(2, "取消", 0.2))]);
    go(&w, 4);
    assert!(w.borrow().sent.is_empty());
}

// ---------- 回路 ----------

#[test]
fn unavailable_dco_prints_a_plain_sentence_and_exits_nonzero() {
    let w = world(idle_one(), vec![]);
    w.borrow_mut().hear_error = Some(HearError { code: "dco_down".into(), message: "x".into() });
    assert_eq!(go(&w, 3), 1);
    assert_eq!(w.borrow().errors, ["dco 没在运行，先打开 dco 再试。"]);
    assert_eq!(w.borrow().hear_calls.len(), 1, "不能死循环");
    let w = world(idle_one(), vec![]);
    w.borrow_mut().hear_error = Some(HearError { code: "voice_off".into(), message: "语音没开".into() });
    assert_eq!(go(&w, 3), 1);
    assert!(w.borrow().errors[0].contains("语音没开"));
}

#[test]
fn error_in_the_middle_of_listening_also_exits_nonzero() {
    struct Flaky(usize);
    impl Hearer for Flaky {
        fn hear(&mut self, _: u64, _: u64) -> Result<Vec<Utterance>, HearError> {
            self.0 += 1;
            if self.0 >= 3 { Err(HearError { code: "dco_timeout".into(), message: "超时".into() }) } else { Ok(vec![]) }
        }
    }
    let w = world(idle_one(), vec![]);
    let (mut h, mut d, mut c, mut o, mut l) = (Flaky(0), FDaemon(w.clone()), FClock(w.clone()), FOut(w.clone()), FLog(w.clone()));
    let cfg = Config::default();
    assert_eq!(Relay::new(&mut h, &mut d, &mut c, &mut o, &mut l, &cfg).run(Some(10)), 1);
    assert_eq!(w.borrow().errors.len(), 1);
}

#[test]
fn since_seq_advances_and_a_replayed_sentence_is_not_handled_twice() {
    struct Replay(W, Vec<u64>);
    impl Hearer for Replay {
        fn hear(&mut self, since: u64, _: u64) -> Result<Vec<Utterance>, HearError> {
            self.1.push(since);
            // 一个不守规矩的 dco：不管 since 是多少都把同一句再交一遍
            let mut w = self.0.borrow_mut();
            w.now += 5000;
            Ok(vec![utt(7, "看目录", 0.9)])
        }
    }
    let w = world(idle_one(), vec![]);
    let (mut h, mut d, mut c, mut o, mut l) = (Replay(w.clone(), vec![]), FDaemon(w.clone()), FClock(w.clone()), FOut(w.clone()), FLog(w.clone()));
    let cfg = Config::default();
    Relay::new(&mut h, &mut d, &mut c, &mut o, &mut l, &cfg).run(Some(5));
    // 第一次是启动前的积压（丢掉），之后同一个 seq 不会再被处理
    assert_eq!(w.borrow().sent.len(), 0);
    assert_eq!(h.1[0], 0);
    assert!(h.1[1..].iter().all(|s| *s == 7), "{:?}", h.1);
}

#[test]
fn since_seq_is_passed_forward_between_hears() {
    let w = world(idle_one(), vec![(10_000, utt(5, "看目录", 0.9)), (30_000, utt(9, "再看看", 0.9))]);
    go(&w, 6);
    let b = w.borrow();
    assert_eq!(b.hear_calls[0], (0, 0));
    let seqs: Vec<u64> = b.hear_calls.iter().map(|c| c.0).collect();
    assert!(seqs.windows(2).all(|p| p[0] <= p[1]), "{seqs:?}");
    assert!(seqs.contains(&5));
    assert_eq!(b.sent.len(), 2);
}

#[test]
fn backlog_from_before_start_is_dropped_not_replayed() {
    let w = world(idle_one(), vec![]);
    w.borrow_mut().backlog = vec![utt(1, "很久以前说的", 0.9), utt(2, "也是以前的", 0.9)];
    go(&w, 3);
    let b = w.borrow();
    assert!(b.sent.is_empty());
    assert!(b.said.iter().any(|s| s.contains("跳过了启动前的 2 句")));
    assert!(b.hear_calls.iter().skip(1).all(|c| c.0 == 2));
}

#[test]
fn log_lines_have_the_documented_fields_and_only_text() {
    let w = world(idle_one(), vec![(10_000, utt(1, "看目录", 0.9))]);
    go(&w, 3);
    let b = w.borrow();
    let r = b.log[0].as_object().unwrap();
    for k in ["t", "text_raw", "text_fixed", "confidence", "target", "action", "reason"] {
        assert!(r.contains_key(k), "{k}");
    }
    assert_eq!(r.len(), 7);
}

#[cfg(unix)]
#[test]
fn log_file_is_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("dct-voice-log-{}", std::process::id()));
    let p = dir.join("voice.log");
    let mut l = unix::FileLog::open(&p).unwrap();
    l.append(&json!({"a":1}));
    let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(std::fs::read_to_string(&p).unwrap().trim(), "{\"a\":1}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn args_are_refused() {
    assert_eq!(run(&["x".to_string()]), 2);
}


#[test]
fn only_a_click_on_the_octopus_may_send_speech_into_a_session() {
    let v = serde_json::json!({"utterances":[
        {"seq":1,"text":"你好","confidence":0.9,"trigger":"octopus_click"},
        {"seq":2,"text":"你好","confidence":0.9,"trigger":"wake_word"},
        {"seq":3,"text":"你好","confidence":0.9}]});
    let got = parse_hear(&v);
    assert_eq!(got.iter().map(|u| u.trigger.as_str()).collect::<Vec<_>>(), ["octopus_click", "wake_word", ""]);
    assert_eq!(got[0].trigger, ALLOWED_TRIGGER);
    assert_ne!(got[1].trigger, ALLOWED_TRIGGER);
    assert_ne!(got[2].trigger, ALLOWED_TRIGGER);
}

#[test]
fn speech_not_started_by_a_click_on_the_octopus_is_never_sent() {
    for trig in ["wake_word", "", "something_else"] {
        let mut u = utt(1, "帮我看目录", 0.95);
        u.trigger = trig.into();
        let w = world(idle_one(), vec![(10_000, u)]);
        go(&w, 3);
        assert!(w.borrow().sent.is_empty(), "trigger {trig:?}");
        assert_eq!(actions(&w), ["blocked_trigger"], "trigger {trig:?}");
    }
    // 对照：点章鱼触发的照常送出。
    let w = world(idle_one(), vec![(10_000, utt(1, "帮我看目录", 0.95))]);
    go(&w, 3);
    assert_eq!(w.borrow().sent.len(), 1);
}
