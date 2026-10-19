//! `dct ask` 的内核：一轮流式问答怎么跑、怎么取消、怎么收尾。
//!
//! 本模块**拿不到会话列表、`Request::Input`、dco 连接、屏幕内容**——它只认一个
//! 可注入的流式传输和一条客户端连接，回复出口只有 `on_sentence` 回调。回复在
//! 类型上没有地方被当命令用（设计 §2.3、§5）。
//!
//! 一轮怎么跑在这里（`run_turn`）；一轮之外的东西各有各的文件：`input` 校验、
//! `memory` 对话记忆（只在内存）、`prompt` 固定提示、`service` 把这些接起来
//! （守护进程里的 ask 服务）、`log` 只写元数据的 `ask.log`、`cli` 是 `dct ask`。
//! 记忆用 `run_turn` 的 `commit` 回调表达：只在这一轮没被取消时、在门锁里调一次。
//!
//! 结构性约束（`tests/ask_service.rs` 钉着）：整个 `src/ask/` 不碰会话、`Request::Input`、
//! 守护进程；只有 `log.rs` 写盘、只有 `cli.rs` 往 stdout 打字。其余文件不打印、不写任何东西。

pub mod cli;
pub mod early;
pub mod input;
pub mod log;
pub mod memory;
pub mod prompt;
pub mod service;
pub mod split;

use crate::llm::stream::{parse_frame, Frame, SseParser, StreamError, StreamOpen};
use serde_json::{json, Value};
use split::SentenceSplitter;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// 轮询取消（客户端 EOF、被取代）和期限的周期。
pub const POLL: Duration = Duration::from_millis(250);

/// 设计 §6.2 的错误码。退出码和线上的字符串都只在这里定义一次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    BadRequest,
    DaemonDown,
    NotEnabled,
    Unreachable,
    Auth,
    Busy,
    Slow,
    Empty,
    BadReply,
    Cancelled,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::DaemonDown => "daemon_down",
            ErrorCode::NotEnabled => "not_enabled",
            ErrorCode::Unreachable => "unreachable",
            ErrorCode::Auth => "auth",
            ErrorCode::Busy => "busy",
            ErrorCode::Slow => "slow",
            ErrorCode::Empty => "empty",
            ErrorCode::BadReply => "bad_reply",
            ErrorCode::Cancelled => "cancelled",
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::BadRequest => 2,
            ErrorCode::DaemonDown => 10,
            ErrorCode::NotEnabled => 11,
            ErrorCode::Unreachable => 12,
            ErrorCode::Auth => 13,
            ErrorCode::Busy => 14,
            ErrorCode::Slow => 15,
            ErrorCode::Empty => 16,
            ErrorCode::BadReply => 17,
            ErrorCode::Cancelled => 18,
        }
    }

    /// 重试有没有意义：钥匙失效和请求本身有问题，重试没用。
    pub fn retryable(self) -> bool {
        !matches!(self, ErrorCode::Auth | ErrorCode::BadRequest | ErrorCode::NotEnabled)
    }
}

impl From<StreamError> for ErrorCode {
    fn from(e: StreamError) -> Self {
        match e {
            StreamError::Unreachable => ErrorCode::Unreachable,
            StreamError::Auth => ErrorCode::Auth,
            StreamError::Busy => ErrorCode::Busy,
            StreamError::Slow => ErrorCode::Slow,
            StreamError::BadReply => ErrorCode::BadReply,
        }
    }
}

/// 一轮的结果。**恰好一个**（设计 §6.1：`end` 或 `error`；对面已经关了就没有事件）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminal {
    /// `hdr_ms` / `delta_ms`：分段计时（请求→响应头、响应头→第一个有字的增量），起点同 `first_ms`。
    End { sentences: usize, truncated: bool, first_ms: u64, hdr_ms: u64, delta_ms: u64, total_ms: u64 },
    Error { code: ErrorCode, after_ms: u64 },
    /// 客户端关了连接：没人在读，不发任何事件。
    Gone,
}

impl Terminal {
    /// `dct ask` 的退出码（设计 §6.2）。`Gone` 没有人在读，也没有人看退出码。
    pub fn exit_code(&self) -> i32 {
        match self {
            Terminal::End { .. } => 0,
            Terminal::Error { code, .. } => code.exit_code(),
            Terminal::Gone => ErrorCode::Cancelled.exit_code(),
        }
    }

    /// NDJSON 终止事件；`Gone` 没有。
    pub fn event(&self) -> Option<Value> {
        match self {
            Terminal::End { sentences, truncated, first_ms, hdr_ms, delta_ms, total_ms } => Some(json!({
                "v": 1, "type": "end", "sentences": sentences, "truncated": truncated,
                "first_ms": first_ms, "hdr_ms": hdr_ms, "delta_ms": delta_ms, "total_ms": total_ms,
            })),
            Terminal::Error { code, after_ms } => Some(json!({
                "v": 1, "type": "error", "code": code.as_str(),
                "stage": "before_first_sentence", "retryable": code.retryable(), "after_ms": after_ms,
            })),
            Terminal::Gone => None,
        }
    }
}

/// 还没受理就被拒（输入不合法、没开、忙）：只有一个 `error` 事件，没有 `start`；
/// `reason` 是固定的原因词，不含任何输入文字。服务端和 CLI 共用。
pub fn refusal(code: ErrorCode, reason: &str) -> Value {
    let mut ev = Terminal::Error { code, after_ms: 0 }.event().unwrap_or_default();
    ev["reason"] = json!(reason);
    ev
}

/// 交给 `commit` 的：这一轮实际说出去的句子。超时 / 断流截断的那一轮，是已说出的那几句。
#[derive(Debug)]
pub struct TurnDone<'a> {
    pub sentences: &'a [String],
    pub truncated: bool,
}

// ---- 在途标记：全局 1 个，同对话新请求取代旧请求 ----

#[derive(Default)]
struct Slot {
    next_id: u64,
    /// (票号, 对话 id, 取消标志)
    cur: Option<(u64, String, Arc<AtomicBool>)>,
}

#[derive(Clone, Default)]
pub struct Gate(Arc<Mutex<Slot>>);

fn lock(m: &Mutex<Slot>) -> MutexGuard<'_, Slot> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 全局已经有一个别的对话在途。
#[derive(Debug, PartialEq, Eq)]
pub struct Busy;

impl Gate {
    /// 占在途标记。同一个对话已有在途的：**取代**它（旧的被置取消）；别的对话在途：`Busy`。
    pub fn begin(&self, conversation_id: &str) -> Result<Ticket, Busy> {
        let mut g = lock(&self.0);
        if let Some((_, conv, flag)) = &g.cur {
            if conv != conversation_id {
                return Err(Busy);
            }
            flag.store(true, Ordering::SeqCst);
        }
        g.next_id += 1;
        let id = g.next_id;
        let cancelled = Arc::new(AtomicBool::new(false));
        g.cur = Some((id, conversation_id.to_string(), cancelled.clone()));
        Ok(Ticket { gate: self.0.clone(), id, cancelled })
    }

    /// 有没有一轮在途。别的用同一网关的功能（出错解释）靠它给 ask 让路（`service::Yielding`）。
    pub fn busy(&self) -> bool {
        lock(&self.0).cur.is_some()
    }
}

/// 一轮在途的凭证。**标记归发现取消的这条线程管，不归 HTTP 工作线程**：取消时
/// 在同一把锁里置标志并清标记，之后不管后台线程什么时候退出，下一个请求都不会
/// 撞上 `Busy`。丢掉凭证也会清标记（出错路径）。
pub struct Ticket {
    gate: Arc<Mutex<Slot>>,
    id: u64,
    cancelled: Arc<AtomicBool>,
}

impl Ticket {
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// 同步取消：置标志 + 清标记，一个临界区。
    pub fn cancel(&self) {
        let mut g = lock(&self.gate);
        self.cancelled.store(true, Ordering::SeqCst);
        self.release(&mut g);
    }

    /// 正常收尾：**没被取消**才调 `commit`，并且与清标记在同一把锁下，所以取消和
    /// 完成恰好一个赢，晚到的取消改不了已经提交的记忆。返回 false = 取消先到。
    pub fn complete(self, commit: impl FnOnce()) -> bool {
        let mut g = lock(&self.gate);
        if self.cancelled() {
            return false;
        }
        commit();
        self.release(&mut g);
        true
    }

    fn release(&self, g: &mut Slot) {
        if g.cur.as_ref().is_some_and(|(id, ..)| *id == self.id) {
            g.cur = None;
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut g = lock(&self.gate);
        self.release(&mut g);
    }
}

// ---- 客户端连接是否还在 ----

/// 客户端（`dct ask` 进程 / dco）是不是已经关了连接。靠写失败发现太晚——句子之间
/// 可能隔一秒以上——所以流式期间要主动问。
pub trait ClientGone {
    fn gone(&mut self) -> bool;
}

/// 非阻塞地 peek 一个字节：读到 0 = 对面关了；没数据 = 还在；有数据（客户端在
/// 请求之后不该再发东西）也算还在。
#[cfg(unix)]
impl ClientGone for crate::sys::ipc::Stream {
    fn gone(&mut self) -> bool {
        use std::os::fd::AsRawFd;
        let mut b = 0u8;
        let n = unsafe {
            libc::recv(self.as_raw_fd(), (&mut b as *mut u8).cast(), 1, libc::MSG_PEEK | libc::MSG_DONTWAIT)
        };
        match n {
            0 => true,
            n if n > 0 => false,
            _ => !matches!(
                io::Error::last_os_error().kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ),
        }
    }
}

/// **Windows 上这一条做不到，如实写在这里：** uds_windows 的 socket 没有 peek，
/// 这里恒返回 false，对面关了只能靠写失败发现（`on_sentence` 返回 Err）。所以在 Windows 上，
/// 「关连接 = 取消」在句子之间和出第一句之前**要等到下一次写才生效**，比设计 §6.5 弱：
/// 取消后同一对话的重发在旧一轮还没写失败之前会被当成「取代」（正常处理），
/// 但网关那一路会多生成一会儿。dco 目前只有 Mac 版，所以没有为它补一条没法验证的
/// Windows 路径；要补的话得有 Windows 机器能测。
#[cfg(not(unix))]
impl ClientGone for crate::sys::ipc::Stream {
    fn gone(&mut self) -> bool {
        false
    }
}

// ---- 一轮 ----

pub struct Turn {
    pub opener: Arc<dyn StreamOpen>,
    /// 完整的请求体（`stream_body` 造的）。
    pub body: Value,
    /// 从守护进程收到请求起算的总期限。
    pub started: Instant,
    pub deadline: Duration,
    pub poll: Duration,
}

enum Msg {
    /// 响应头到了（`open` 返回的那一刻，在工作线程上取的时刻）。
    Opened(Instant),
    Bytes(Vec<u8>),
    /// 干净的 EOF。
    Eof,
    Failed(StreamError),
    /// 读到一半出错；`true` = 读超时。
    ReadErr(bool),
}

/// 工作线程：开连接、阻塞读、转发字节。每次读返回都看 `abandon`，看到就返回——
/// 丢掉 reader 即关到网关的连接。
fn pump(opener: Arc<dyn StreamOpen>, body: Value, left: Duration, abandon: Arc<AtomicBool>, tx: mpsc::Sender<Msg>) {
    let mut r = match opener.open(&body, left) {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.send(Msg::Failed(e));
            return;
        }
    };
    if tx.send(Msg::Opened(Instant::now())).is_err() {
        return;
    }
    let mut buf = [0u8; 4096];
    while !abandon.load(Ordering::SeqCst) {
        let m = match r.read(&mut buf) {
            Ok(0) => Msg::Eof,
            Ok(n) => Msg::Bytes(buf[..n].to_vec()),
            Err(e) => Msg::ReadErr(matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)),
        };
        let last = !matches!(m, Msg::Bytes(_));
        if abandon.load(Ordering::SeqCst) || tx.send(m).is_err() || last {
            return;
        }
    }
}

/// 跑一轮。每句**立刻**交给 `on_sentence(i, 文字)`（返回 Err = 对面写不进去了，当取消）；
/// `commit` 只在这一轮没被取消、且有句子说出去时调一次。返回时在途标记已清。
pub fn run_turn(
    turn: Turn,
    ticket: Ticket,
    peer: &mut dyn ClientGone,
    on_sentence: &mut dyn FnMut(usize, &str) -> io::Result<()>,
    commit: impl FnOnce(&TurnDone),
) -> Terminal {
    let Turn { opener, body, started, deadline, poll } = turn;
    let end_at = started + deadline;
    let abandon = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let left = end_at.saturating_duration_since(Instant::now());
    {
        let abandon = abandon.clone();
        std::thread::spawn(move || pump(opener, body, left, abandon, tx));
    }

    let ms = |t: Instant| t.duration_since(started).as_millis() as u64;
    let mut sent: Vec<String> = Vec::new();
    let mut first: Option<u64> = None;
    // 分段计时：响应头到达、第一个有字的增量到达的时刻。
    let (mut opened, mut first_delta): (Option<Instant>, Option<Instant>) = (None, None);
    let (mut sse, mut split) = (SseParser::default(), SentenceSplitter::new());

    // 收尾：只有这里和下面的 `Gone` 返回。`natural` = 流以 `[DONE]` 结束。
    macro_rules! conclude {
        ($natural:expr, $fail:expr) => {{
            abandon.store(true, Ordering::SeqCst);
            let after_ms = ms(Instant::now());
            if sent.is_empty() {
                let code = if $natural { ErrorCode::Empty } else { $fail };
                return Terminal::Error { code, after_ms };
            }
            let truncated = !$natural;
            let done = ticket.complete(|| commit(&TurnDone { sentences: &sent, truncated }));
            return if done {
                let hdr_ms = opened.map_or(0, ms);
                let delta_ms = opened.zip(first_delta).map_or(0, |(o, d)| d.saturating_duration_since(o).as_millis() as u64);
                Terminal::End { sentences: sent.len(), truncated, first_ms: first.unwrap_or(0), hdr_ms, delta_ms, total_ms: after_ms }
            } else {
                Terminal::Error { code: ErrorCode::Cancelled, after_ms }
            };
        }};
    }
    macro_rules! gone {
        () => {{
            ticket.cancel();
            abandon.store(true, Ordering::SeqCst);
            return Terminal::Gone;
        }};
    }
    macro_rules! say {
        ($s:expr) => {{
            if on_sentence(sent.len(), &$s).is_err() {
                gone!();
            }
            first.get_or_insert_with(|| ms(Instant::now()));
            sent.push($s);
        }};
    }

    loop {
        if peer.gone() {
            gone!();
        }
        if ticket.cancelled() {
            abandon.store(true, Ordering::SeqCst);
            return Terminal::Error { code: ErrorCode::Cancelled, after_ms: ms(Instant::now()) };
        }
        let now = Instant::now();
        if now >= end_at {
            conclude!(false, ErrorCode::Slow);
        }
        let msg = match rx.recv_timeout(poll.min(end_at - now)) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => conclude!(false, ErrorCode::BadReply),
        };
        match msg {
            Msg::Opened(t) => opened = Some(t),
            Msg::Failed(e) => conclude!(false, e.into()),
            Msg::ReadErr(timed_out) => {
                conclude!(false, if timed_out { ErrorCode::Slow } else { ErrorCode::BadReply })
            }
            // 没有 `[DONE]` 就断了：流不完整，尾巴不发。
            Msg::Eof => conclude!(false, ErrorCode::BadReply),
            Msg::Bytes(b) => {
                let Ok(payloads) = sse.push(&b) else { conclude!(false, ErrorCode::BadReply) };
                for p in payloads {
                    match parse_frame(&p) {
                        Err(_) => conclude!(false, ErrorCode::BadReply),
                        Ok(Frame::Delta(t)) => {
                            if !t.is_empty() {
                                first_delta.get_or_insert_with(Instant::now);
                            }
                            for s in split.push(&t) {
                                say!(s);
                            }
                        }
                        Ok(Frame::Done) => {
                            if let Some(tail) = std::mem::take(&mut split).finish() {
                                say!(tail);
                            }
                            conclude!(true, ErrorCode::BadReply);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_map_to_the_exit_codes_and_wire_names_of_the_design() {
        let table = [
            (ErrorCode::BadRequest, "bad_request", 2),
            (ErrorCode::DaemonDown, "daemon_down", 10),
            (ErrorCode::NotEnabled, "not_enabled", 11),
            (ErrorCode::Unreachable, "unreachable", 12),
            (ErrorCode::Auth, "auth", 13),
            (ErrorCode::Busy, "busy", 14),
            (ErrorCode::Slow, "slow", 15),
            (ErrorCode::Empty, "empty", 16),
            (ErrorCode::BadReply, "bad_reply", 17),
            (ErrorCode::Cancelled, "cancelled", 18),
        ];
        for (c, name, exit) in table {
            assert_eq!((c.as_str(), c.exit_code()), (name, exit));
        }
        // 钥匙失效重试没用；慢和忙可以再来。
        assert!(!ErrorCode::Auth.retryable() && ErrorCode::Slow.retryable() && ErrorCode::Busy.retryable());
    }

    #[test]
    fn terminal_events_have_the_documented_shape() {
        let end = Terminal::End { sentences: 2, truncated: false, first_ms: 1840, hdr_ms: 600, delta_ms: 900, total_ms: 3320 };
        assert_eq!(
            end.event().unwrap(),
            json!({"v":1,"type":"end","sentences":2,"truncated":false,"first_ms":1840,"hdr_ms":600,"delta_ms":900,"total_ms":3320})
        );
        let err = Terminal::Error { code: ErrorCode::Slow, after_ms: 8000 };
        assert_eq!(err.event().unwrap(), json!({"v":1,"type":"error","code":"slow","stage":"before_first_sentence","retryable":true,"after_ms":8000}));
        assert_eq!(Terminal::Gone.event(), None);
    }
}
