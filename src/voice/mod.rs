//! `dct voice`：把用户说的话（dco 听写出来的文字）当成用户亲手打的字，送进他最近对话的那个会话。
//! 设计：docs/superpowers/specs/2026-10-04-dct-voice-relay-design.md。
//!
//! 这里分两层：上面是纯函数和循环（`Hearer` / `Daemon` / `Clock` / `Out` / `Log` 都是 trait，测试用假的），
//! 下面 `unix` 里接真的 dco、守护进程的 socket、文件。
//!
//! 守住的原则：**语音永远不算授权**。等批准的会话不送、单独的确认词不送、没听清不送、说太快不送，
//! 决定要送之后还有 2 秒的取消窗口，窗口结束时会话状态要再看一次。
use crate::session::{SessionInfo, SessionState};
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};

/// 送进会话的话前面固定加的一句，让 agent 知道要宽容。
pub const PREFIX: &str = "（语音输入，可能有识别错误）";
/// 低于这个置信度不送。
pub const MIN_CONFIDENCE: f64 = 0.5;
/// 预告之后等多久才真的送（期间说「取消」可以不发）。
pub const CANCEL_WINDOW_MS: u64 = 2000;
/// 两次送出之间至少隔多久。
pub const MIN_GAP_MS: u64 = 2000;
/// 每轮向 dco 要话最多等多久。
pub const HEAR_WAIT_MS: u64 = 20_000;

const USAGE: &str = "用法：dct voice（前台一直听，Ctrl-C 退出）";

#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    pub seq: u64,
    pub text: String,
    pub confidence: f64,
    /// dco 说这句话是怎么触发的。目前只认用户亲手点章鱼（`octopus_click`）；缺失当作不认。
    pub trigger: String,
}

/// 目前唯一允许把话送进会话的触发方式：用户本人点了章鱼。
/// 以后的唤醒词等不在这里，要单独决定，不能默认放行（人声可以被电视或别人冒充）。
pub const ALLOWED_TRIGGER: &str = "octopus_click";

#[derive(Debug, Clone, PartialEq)]
pub struct HearError {
    pub code: String,
    pub message: String,
}

pub trait Hearer {
    fn hear(&mut self, since_seq: u64, wait_ms: u64) -> Result<Vec<Utterance>, HearError>;
}

pub trait Daemon {
    fn sessions(&mut self) -> Result<Vec<SessionInfo>, String>;
    /// 敲进去并按回车。
    fn type_into(&mut self, id: u32, text: &str) -> Result<(), String>;
}

pub trait Clock {
    fn now_ms(&self) -> u64;
    fn sleep_ms(&mut self, ms: u64);
}

pub trait Out {
    fn say(&mut self, line: &str);
    fn error(&mut self, line: &str);
    /// 尽力在 dco 的说明条上显示同样的字；显示不了就算了。
    fn banner(&mut self, _text: &str) {}
}

pub trait Log {
    fn append(&mut self, rec: &Value);
}

// ---------- 配置与纠错 ----------

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// 写对的词 -> 识别器常写成的别的样子。
    pub alias: BTreeMap<String, Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        let mut alias = BTreeMap::new();
        alias.insert("章鱼".to_string(), ["张瑜", "张宇", "章玉", "张羽"].iter().map(|s| s.to_string()).collect());
        Config { alias }
    }
}

/// TOML 的裸键只能是 ASCII，而设计里的例子是 `章鱼 = [...]`：这里把行首非 ASCII 的裸键补上引号再解析。
fn quote_bare_keys(src: &str) -> String {
    src.lines()
        .map(|l| {
            let t = l.trim_start();
            match t.split_once('=') {
                Some((k, rest)) if !k.is_ascii() && !k.trim().starts_with(['"', '\'', '[', '#']) => format!("\"{}\" ={rest}", k.trim()),
                _ => l.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `~/.dct/voice.toml` 的内容。读不懂时给一句人话。
pub fn parse_config(src: &str) -> Result<Config, String> {
    let v: toml::Value = quote_bare_keys(src).parse().map_err(|e| format!("~/.dct/voice.toml 读不懂：{e}"))?;
    let mut alias = BTreeMap::new();
    if let Some(t) = v.get("alias") {
        let t = t.as_table().ok_or("~/.dct/voice.toml 里的 [alias] 要写成「词 = [\"别名\", ...]」")?;
        for (k, v) in t {
            let arr = v.as_array().ok_or_else(|| format!("~/.dct/voice.toml 里「{k}」后面要写成 [\"别名\", ...]"))?;
            alias.insert(k.clone(), arr.iter().filter_map(|a| a.as_str().map(String::from)).collect());
        }
    }
    Ok(Config { alias })
}

/// 别名紧挨着的汉字里，这些算「词的边界」（「给张瑜说」「张瑜你好」）；别的汉字紧挨着就是更长的词的一部分（「张瑜伽」）。
const BEFORE_OK: &str = "给对跟问叫让和同找请诉";
const AFTER_OK: &str = "说啊呀吧呢你帮请来去能可把的在是";

fn boundary_ok(c: Option<char>, allowed: &str) -> bool {
    match c {
        None => true,
        Some(c) if !c.is_alphanumeric() => true,
        Some(c) if c.is_ascii_alphanumeric() => false,
        Some(c) => allowed.contains(c),
    }
}

/// 只替换整词出现的别名。不是通用的同音字替换。
pub fn fix_aliases(text: &str, cfg: &Config) -> String {
    let mut pairs: Vec<(Vec<char>, &str)> = vec![];
    for (good, bad) in &cfg.alias {
        for b in bad {
            if !b.is_empty() {
                pairs.push((b.chars().collect(), good.as_str()));
            }
        }
    }
    pairs.sort_by_key(|(b, _)| std::cmp::Reverse(b.len()));
    let cs: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    'outer: while i < cs.len() {
        for (b, good) in &pairs {
            if cs[i..].starts_with(b) {
                let prev = i.checked_sub(1).map(|p| cs[p]);
                let next = cs.get(i + b.len()).copied();
                if boundary_ok(prev, BEFORE_OK) && boundary_ok(next, AFTER_OK) {
                    out.push_str(good);
                    i += b.len();
                    continue 'outer;
                }
            }
        }
        out.push(cs[i]);
        i += 1;
    }
    out
}

// ---------- 过闸 ----------

/// 只留字母数字：去掉所有标点和空白，英文转小写。
fn norm_phrase(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

const CONFIRM_WORDS: &[&str] = &["好", "好的", "好吧", "嗯", "嗯嗯", "是", "是的", "对", "对的", "确认", "确定", "同意", "批准", "可以", "行", "yes", "yeah", "ok", "okay", "y", "sure"];
const CONFIRM_REPEAT: &[char] = &['好', '嗯', '是', '行', '对'];

/// 整句只是一个确认词（「好」「嗯嗯」「OK」）。「好的，继续做」不算。
pub fn is_bare_confirmation(text: &str) -> bool {
    let n = norm_phrase(text);
    if n.is_empty() {
        return false;
    }
    if CONFIRM_WORDS.contains(&n.as_str()) {
        return true;
    }
    let mut it = n.chars();
    match it.next() {
        Some(first) if CONFIRM_REPEAT.contains(&first) => n.chars().all(|c| c == first),
        _ => false,
    }
}

const CANCEL_WORDS: &[&str] = &["取消", "算了", "不要发", "别发", "不发了", "不要发了", "别发了", "cancel"];

/// 整句是「取消」这一类话。
pub fn is_cancel(text: &str) -> bool {
    CANCEL_WORDS.contains(&norm_phrase(text).as_str())
}

/// 会话此刻在等用户批准。**语音不能回答这种提示。** 这是仓库里表示「agent 要用户处理」的那个状态
/// （`SessionState::Asking`）；别的状态（干活中、空闲、报错、不明）都不是批准提示。
pub fn waiting_for_approval(s: &SessionInfo) -> bool {
    s.state == SessionState::Asking
}

// ---------- 路由 ----------

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    To { id: u32, name: String, text: String },
    NoSession,
    Ambiguous,
}

pub fn display_name(s: &SessionInfo) -> String {
    if s.tag.is_empty() { s.profile.clone() } else { s.tag.clone() }
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace() && *c != '-').flat_map(|c| c.to_lowercase()).collect()
}

/// 从 `rest` 开头吃掉 `name`（忽略大小写、空格、连字符），返回吃到哪（字节）。
fn consume_name(rest: &str, name: &str) -> Option<usize> {
    let want: Vec<char> = squash(name).chars().collect();
    if want.is_empty() {
        return None;
    }
    let mut k = 0;
    let mut end = 0;
    for (i, c) in rest.char_indices() {
        if k == want.len() {
            break;
        }
        if c.is_whitespace() || c == '-' {
            continue;
        }
        if c.to_lowercase().next() != Some(want[k]) {
            return None;
        }
        k += 1;
        end = i + c.len_utf8();
    }
    (k == want.len()).then_some(end)
}

fn strip_seps(s: &str) -> &str {
    s.trim_start_matches(|c: char| c.is_whitespace() || "，,：:、。.".contains(c))
}

/// 话以「给X说」「对X说」「告诉X」开头：返回命中的会话编号和去掉开头后的话。
/// `Err(())` = 名字匹配到多个会话。
fn explicit_target(text: &str, agents: &[&SessionInfo]) -> Result<Option<(u32, String)>, ()> {
    let t = text.trim_start();
    for (prefix, needs_say) in [("给", true), ("对", true), ("告诉", false)] {
        let Some(rest) = t.strip_prefix(prefix) else { continue };
        let rest = rest.trim_start();
        let mut hits: Vec<(u32, String)> = vec![];
        for s in agents {
            for name in [display_name(s), format!("#{}", s.id)] {
                let Some(end) = consume_name(rest, &name) else { continue };
                let after = &rest[end..];
                let tail = if needs_say {
                    match after.trim_start().strip_prefix('说') {
                        Some(r) => r,
                        None => continue,
                    }
                } else {
                    if after.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) {
                        continue;
                    }
                    after
                };
                hits.push((s.id, strip_seps(tail).to_string()));
            }
        }
        hits.sort();
        hits.dedup_by_key(|h| h.0);
        match hits.len() {
            0 => {}
            1 => return Ok(hits.pop()),
            _ => return Err(()),
        }
    }
    Ok(None)
}

/// 决定这句话送给谁。只考虑 agent 会话（普通命令行会话会把话当命令执行，绝不送）。
pub fn route(text: &str, sessions: &[SessionInfo]) -> Route {
    let agents: Vec<&SessionInfo> = sessions.iter().filter(|s| s.is_agent).collect();
    match explicit_target(text, &agents) {
        Err(()) => return Route::Ambiguous,
        Ok(Some((id, rest))) => {
            let s = agents.iter().find(|s| s.id == id).expect("hit comes from agents");
            return Route::To { id, name: display_name(s), text: rest };
        }
        Ok(None) => {}
    }
    match agents.iter().filter(|s| s.state != SessionState::Stopped).max_by_key(|s| (s.last_active_ms, s.id)) {
        Some(s) => Route::To { id: s.id, name: display_name(s), text: text.trim().to_string() },
        None => Route::NoSession,
    }
}

/// 解析 dco `hear` 的回复。缺置信度的当 0（不送）。
pub fn parse_hear(v: &Value) -> Vec<Utterance> {
    v["utterances"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|u| {
                    Some(Utterance {
                        seq: u["seq"].as_u64()?,
                        text: u["text"].as_str()?.to_string(),
                        confidence: u["confidence"].as_f64().unwrap_or(0.0),
                        trigger: u["trigger"].as_str().unwrap_or("").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 送进会话的最终文字：固定前缀 + 原话，去掉控制字符（Esc、回车会干扰输入框）。
pub fn outgoing(text: &str) -> String {
    format!("{PREFIX}{}", text.chars().filter(|c| !c.is_control()).collect::<String>())
}

// ---------- 循环 ----------

pub struct Relay<'a> {
    pub hearer: &'a mut dyn Hearer,
    pub daemon: &'a mut dyn Daemon,
    pub clock: &'a mut dyn Clock,
    pub out: &'a mut dyn Out,
    pub log: &'a mut dyn Log,
    pub cfg: &'a Config,
    since_seq: u64,
    last_sent_ms: Option<u64>,
    pending: VecDeque<Utterance>,
}

fn hear_message(e: &HearError) -> String {
    match e.code.as_str() {
        "dco_too_old" => "dco 版本太旧，不会听写。先更新 dco 再试。".to_string(),
        "dco_down" => "dco 没在运行，先打开 dco 再试。".to_string(),
        _ => format!("听不到：{}。先确认 dco 已经打开，并且开了语音。", e.message),
    }
}

impl<'a> Relay<'a> {
    pub fn new(hearer: &'a mut dyn Hearer, daemon: &'a mut dyn Daemon, clock: &'a mut dyn Clock, out: &'a mut dyn Out, log: &'a mut dyn Log, cfg: &'a Config) -> Self {
        Relay { hearer, daemon, clock, out, log, cfg, since_seq: 0, last_sent_ms: None, pending: VecDeque::new() }
    }

    fn record(&mut self, raw: &str, fixed: &str, conf: f64, target: Option<&str>, action: &str, reason: &str) {
        let t = self.clock.now_ms();
        self.log.append(&json!({ "t": t, "text_raw": raw, "text_fixed": fixed, "confidence": conf, "target": target, "action": action, "reason": reason }));
    }

    /// 只取 seq 比已见过的更新的，并推进 `since_seq`。
    fn take_new(&mut self, got: Vec<Utterance>) -> Vec<Utterance> {
        let mut fresh: Vec<Utterance> = got.into_iter().filter(|u| u.seq > self.since_seq).collect();
        fresh.sort_by_key(|u| u.seq);
        if let Some(last) = fresh.last() {
            self.since_seq = last.seq;
        }
        fresh
    }

    /// 启动前已经说过的话不能补发：先取一次，丢掉，只记下编号。
    fn skip_backlog(&mut self) -> Result<(), HearError> {
        let got = self.hearer.hear(0, 0)?;
        let n = self.take_new(got).len();
        if n > 0 {
            self.out.say(&format!("跳过了启动前的 {n} 句话，只处理现在开始说的。"));
        }
        Ok(())
    }

    /// 跑到出错（返回退出码）；`max_hears` 给测试用，`None` 是一直听。
    pub fn run(&mut self, max_hears: Option<usize>) -> i32 {
        if let Err(e) = self.skip_backlog() {
            self.out.error(&hear_message(&e));
            return 1;
        }
        let mut hears = 0;
        loop {
            let u = match self.pending.pop_front() {
                Some(u) => u,
                None => {
                    if max_hears.is_some_and(|m| hears >= m) {
                        return 0;
                    }
                    hears += 1;
                    let got = match self.hearer.hear(self.since_seq, HEAR_WAIT_MS) {
                        Ok(g) => g,
                        Err(e) => {
                            self.out.error(&hear_message(&e));
                            return 1;
                        }
                    };
                    let fresh = self.take_new(got);
                    self.pending.extend(fresh);
                    continue;
                }
            };
            if let Err(e) = self.handle(u) {
                self.out.error(&hear_message(&e));
                return 1;
            }
        }
    }

    /// 一句话：纠错 -> 过闸 -> 路由 -> 预告 -> 等取消 -> 送出。
    pub fn handle(&mut self, u: Utterance) -> Result<(), HearError> {
        let raw = u.text.trim().to_string();
        let fixed = fix_aliases(&raw, self.cfg);
        let conf = u.confidence;
        if is_cancel(&fixed) {
            self.out.say("现在没有要取消的话。");
            self.record(&raw, &fixed, conf, None, "cancelled", "nothing_pending");
            return Ok(());
        }
        if u.trigger != ALLOWED_TRIGGER {
            self.out.say("这句话不是你点章鱼说的，我没有送进会话。");
            self.record(&raw, &fixed, conf, None, "blocked_trigger", &u.trigger);
            return Ok(());
        }
        if conf < MIN_CONFIDENCE {
            self.out.say("没听清，再说一遍？");
            self.record(&raw, &fixed, conf, None, "low_confidence", "below_threshold");
            return Ok(());
        }
        let confirm_msg = "这一步要你在键盘上确认（单独的「好」「是」「确认」这类话我不会送进会话）。";
        if is_bare_confirmation(&fixed) {
            self.out.say(confirm_msg);
            self.record(&raw, &fixed, conf, None, "blocked_confirm", "bare_confirmation");
            return Ok(());
        }
        let sessions = match self.daemon.sessions() {
            Ok(s) => s,
            Err(m) => {
                self.out.say(&format!("连不上 dct 的后台（{m}），这句没发。"));
                self.record(&raw, &fixed, conf, None, "no_session", "daemon_unreachable");
                return Ok(());
            }
        };
        let (id, name, body) = match route(&fixed, &sessions) {
            Route::NoSession => {
                self.out.say("现在没有开着的会话");
                self.record(&raw, &fixed, conf, None, "no_session", "no_agent_session");
                return Ok(());
            }
            Route::Ambiguous => {
                self.out.say("有几个叫这个名字的会话，说编号");
                self.record(&raw, &fixed, conf, None, "ambiguous", "name_matches_many");
                return Ok(());
            }
            Route::To { id, name, text } => (id, name, text),
        };
        if body.is_empty() || is_bare_confirmation(&body) {
            let (action, reason) = if body.is_empty() { ("low_confidence", "empty_after_route") } else { ("blocked_confirm", "bare_confirmation_after_route") };
            self.out.say(if body.is_empty() { "没听清要说什么，再说一遍？" } else { confirm_msg });
            self.record(&raw, &fixed, conf, Some(&name), action, reason);
            return Ok(());
        }
        let target = sessions.iter().find(|s| s.id == id);
        if target.is_none_or(|s| s.state == SessionState::Stopped) {
            self.out.say(&format!("{name} 已经停了，这句没发。"));
            self.record(&raw, &fixed, conf, Some(&name), "no_session", "target_stopped");
            return Ok(());
        }
        if target.is_some_and(waiting_for_approval) {
            self.block_prompt(&raw, &fixed, conf, &name, "waiting_for_approval");
            return Ok(());
        }
        if let Some(last) = self.last_sent_ms {
            if self.clock.now_ms().saturating_sub(last) < MIN_GAP_MS {
                self.out.say("说得太快了，这句先不发。");
                self.record(&raw, &fixed, conf, Some(&name), "blocked_confirm", "rate_limited");
                return Ok(());
            }
        }
        // 预告
        self.out.say(&format!("听到：「{body}」 → 发给：{name}（2 秒内说「取消」可以不发）"));
        self.out.banner(&body);
        if self.wait_cancel()? {
            self.out.say("已取消");
            self.record(&raw, &fixed, conf, Some(&name), "cancelled", "user_cancelled");
            return Ok(());
        }
        // 窗口里会话的状态可能变了：送之前再看一次。
        let sessions = match self.daemon.sessions() {
            Ok(s) => s,
            Err(m) => {
                self.out.say(&format!("连不上 dct 的后台（{m}），这句没发。"));
                self.record(&raw, &fixed, conf, Some(&name), "no_session", "daemon_unreachable");
                return Ok(());
            }
        };
        match sessions.iter().find(|s| s.id == id) {
            Some(s) if s.state != SessionState::Stopped => {
                if waiting_for_approval(s) {
                    self.block_prompt(&raw, &fixed, conf, &name, "waiting_for_approval_at_send");
                    return Ok(());
                }
            }
            _ => {
                self.out.say(&format!("{name} 已经停了，这句没发。"));
                self.record(&raw, &fixed, conf, Some(&name), "no_session", "target_gone_at_send");
                return Ok(());
            }
        }
        match self.daemon.type_into(id, &outgoing(&body)) {
            Ok(()) => {
                self.last_sent_ms = Some(self.clock.now_ms());
                self.out.say(&format!("已发给：{name}"));
                self.record(&raw, &fixed, conf, Some(&name), "sent", "");
            }
            Err(m) => {
                self.out.say(&format!("没送进去：{m}"));
                self.record(&raw, &fixed, conf, Some(&name), "no_session", "send_failed");
            }
        }
        Ok(())
    }

    fn block_prompt(&mut self, raw: &str, fixed: &str, conf: f64, name: &str, reason: &str) {
        self.out.say(&format!("这一步要你在键盘上确认：{name} 正在等你批准，语音不能代替。"));
        self.record(raw, fixed, conf, Some(name), "blocked_prompt", reason);
    }

    /// 等取消窗口走完；期间继续取话：整句是「取消」就丢掉这一句。别的话先排着，不丢也不影响。
    fn wait_cancel(&mut self) -> Result<bool, HearError> {
        let deadline = self.clock.now_ms() + CANCEL_WINDOW_MS;
        let mut cancelled = false;
        loop {
            let now = self.clock.now_ms();
            if now >= deadline || cancelled {
                break;
            }
            let got = self.hearer.hear(self.since_seq, deadline - now)?;
            let fresh = self.take_new(got);
            if fresh.is_empty() {
                self.clock.sleep_ms(50);
            }
            for u in fresh {
                if !cancelled && is_cancel(&fix_aliases(u.text.trim(), self.cfg)) {
                    cancelled = true;
                } else {
                    self.pending.push_back(u);
                }
            }
        }
        Ok(cancelled)
    }
}

// ---------- 命令行 ----------

pub fn run(args: &[String]) -> i32 {
    if !args.is_empty() {
        eprintln!("{USAGE}");
        return 2;
    }
    run_parsed()
}

#[cfg(not(unix))]
fn run_parsed() -> i32 {
    eprintln!("这一版只支持 Mac：听写要用 dco，而 dco 目前只有 Mac 版。");
    1
}

#[cfg(unix)]
fn run_parsed() -> i32 {
    unix::run_real()
}

#[cfg(unix)]
mod unix {
    use super::*;
    use crate::client::Client;
    use crate::proto::{Request, Response};
    use dct_game::dco::DcoClient;
    use dct_game::play::Dco;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::Duration;

    /// 要比 `HEAR_WAIT_MS` 长，否则 dco 还在等话就先读超时了。
    const DCO_TIMEOUT: Duration = Duration::from_secs(30);

    type Shared = Rc<RefCell<DcoClient>>;

    pub struct DcoHearer(pub Shared);

    impl Hearer for DcoHearer {
        fn hear(&mut self, since_seq: u64, wait_ms: u64) -> Result<Vec<Utterance>, HearError> {
            let body = self.0.borrow_mut().call("hear", json!({ "since_seq": since_seq, "wait_ms": wait_ms })).map_err(|e| HearError { code: e.code, message: e.message })?;
            Ok(parse_hear(&body))
        }
    }

    pub struct SocketDaemon(pub PathBuf);

    impl SocketDaemon {
        fn call(&self, req: Request) -> Result<Response, String> {
            let mut c = Client::connect(&self.0).map_err(|_| "后台没在运行".to_string())?;
            c.call(req).map_err(|e| e.to_string())
        }
    }

    impl Daemon for SocketDaemon {
        fn sessions(&mut self) -> Result<Vec<SessionInfo>, String> {
            match self.call(Request::List)? {
                Response::Sessions(v) => Ok(v),
                _ => Err("后台的回答读不懂".into()),
            }
        }

        fn type_into(&mut self, id: u32, text: &str) -> Result<(), String> {
            // 跟 `bridge::submit` 同一个两步约定：先写字，再单独送空串按回车。
            for t in [text, ""] {
                match self.call(Request::Input { id, text: t.to_string() })? {
                    Response::Ok => {}
                    Response::Error(e) => return Err(format!("{e:?}")),
                    _ => return Err("后台的回答读不懂".into()),
                }
            }
            Ok(())
        }
    }

    pub struct SystemClock;
    impl Clock for SystemClock {
        fn now_ms(&self) -> u64 {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
        }
        fn sleep_ms(&mut self, ms: u64) {
            std::thread::sleep(Duration::from_millis(ms));
        }
    }

    /// 终端 + dco 说明条（同一条 dco 连接，只在不听写的时候用）。
    pub struct Console(pub Shared);

    impl Out for Console {
        fn say(&mut self, line: &str) {
            println!("{line}");
        }
        fn error(&mut self, line: &str) {
            eprintln!("{line}");
        }
        fn banner(&mut self, text: &str) {
            if let Ok(mut d) = self.0.try_borrow_mut() {
                d.show_status_with("think", Some(text), None);
            }
        }
    }

    /// 一行一条 JSON，0600。只有文字。
    pub struct FileLog(pub PathBuf);

    impl FileLog {
        pub fn open(path: &Path) -> std::io::Result<FileLog> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path)?;
            Ok(FileLog(path.to_path_buf()))
        }
    }

    impl Log for FileLog {
        fn append(&mut self, rec: &Value) {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&self.0) {
                let _ = writeln!(f, "{rec}");
            }
        }
    }

    pub fn load_config(home: &Path) -> Result<Config, String> {
        match std::fs::read_to_string(home.join(".dct").join("voice.toml")) {
            Ok(s) => parse_config(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("读不了 ~/.dct/voice.toml：{e}")),
        }
    }

    pub fn run_real() -> i32 {
        let Some(home) = crate::sys::home() else {
            eprintln!("找不到家目录。");
            return 1;
        };
        let cfg = match load_config(&home) {
            Ok(c) => c,
            Err(m) => {
                eprintln!("{m}");
                return 1;
            }
        };
        let mut log = match FileLog::open(&home.join(".dct").join("voice.log")) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("记录文件开不了：{e}");
                return 1;
            }
        };
        let dco = match DcoClient::connect_with_timeout(&home.join(".dco"), DCO_TIMEOUT) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}", dct_game_error_text(&e));
                return 1;
            }
        };
        let shared = Rc::new(RefCell::new(dco));
        let mut hearer = DcoHearer(shared.clone());
        let mut daemon = SocketDaemon(crate::proto::socket_path());
        let mut clock = SystemClock;
        let mut out = Console(shared);
        println!("在听了：点章鱼说话，说完 2 秒后送进最近用的会话。Ctrl-C 退出。");
        Relay::new(&mut hearer, &mut daemon, &mut clock, &mut out, &mut log, &cfg).run(None)
    }

    fn dct_game_error_text(e: &dct_game::play::DcoError) -> String {
        crate::game::text::dco_error(e)
    }
}

#[cfg(test)]
mod tests;
