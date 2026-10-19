//! 守护进程里的 ask 服务：校验 → 查配置 → 占在途标记 → 带着记忆跑一轮 → 收尾。
//!
//! 它只认三样东西：一个可注入的流式传输（`Target`）、一条客户端连接（`ClientGone`）、
//! 一个出口（`emit`，一个 NDJSON 事件加退出码）。**拿不到会话、`Request::Input`、dco、屏幕**，
//! 回复在类型上只有 `emit` 一个去处（设计 §2.3、§5）。本文件不写盘、不打印；
//! `ask.log` 只由 `log.rs` 写元数据。

use super::input::{validate, AskRequest};
use super::log::{self, Meta};
use super::memory::{Memory, Turn as Remembered};
use super::{prompt, refusal, run_turn, ClientGone, ErrorCode, Gate, Terminal, Turn, POLL};
use crate::config::{Config, LlmConfig, Transport};
use crate::llm::creds::Borrowed;
use crate::llm::resolve::{http_endpoint, resolve_http};
use crate::llm::stream::{stream_body, StreamOpen, UreqOpen};
use crate::llm::{Backend, LlmError, Prompt, Usage};
use crate::profile::{Profile, Wire};
use crate::secrets::SecretStore;
use serde_json::{json, Value};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 话要发去的地方：已经查过配置、选好凭据的流式传输和模型名。
pub struct Target {
    pub opener: Arc<dyn StreamOpen>,
    pub model: String,
}

/// `[llm]` 指向哪台主机，且 ask 能不能用它：只认 http 传输（cli 传输会拉起第三方命令，不得悄悄
/// 把听写发给它）、openai 线格式、主机在 `[ask].allowed_hosts` 里。不查 `[ask].enabled`、不查凭据——
/// `dct ask --enable` 要在同意之前告诉用户话会发去哪。原因词是固定的。
pub fn destination<'a>(
    cfg: &'a Config,
    lookup: &dyn Fn(&str) -> Option<Profile>,
) -> Result<(&'a LlmConfig, String), &'static str> {
    let llm = cfg.llm.as_ref().ok_or("llm_missing")?;
    if llm.transport != Transport::Http {
        return Err("llm_not_http");
    }
    let (host, wire) = http_endpoint(llm, lookup).map_err(|_| "llm_unusable")?;
    if wire != Wire::Openai {
        return Err("llm_unusable");
    }
    if !cfg.ask.allows(&host) {
        return Err("host_not_allowed");
    }
    Ok((llm, host))
}

impl Target {
    /// 配置 → 目的地。`[ask]` 没开最先说（没同意之前不谈别的）；到这里为止没有任何网络请求。
    pub fn from_config(
        cfg: &Config,
        lookup: &dyn Fn(&str) -> Option<Profile>,
        secrets: &SecretStore,
        oauth: &dyn Fn(&str) -> Option<Borrowed>,
    ) -> Result<Target, &'static str> {
        if !cfg.ask.enabled {
            return Err("ask_off");
        }
        let (llm, _) = destination(cfg, lookup)?;
        let t = resolve_http(llm, lookup, secrets, oauth).map_err(|_| "llm_unusable")?;
        Ok(Target { opener: Arc::new(UreqOpen::new(t.url, t.cred)), model: t.model })
    }
}

/// `emit(事件, 退出码)`：退出码只有终止事件带。返回 Err = 对面写不进去了。
pub type Emit<'a> = &'a mut dyn FnMut(Value, Option<i32>) -> io::Result<()>;

pub struct Service {
    gate: Gate,
    memory: Mutex<Memory>,
    log: Option<PathBuf>,
}

fn refuse(emit: Emit, code: ErrorCode, reason: &str) {
    let _ = emit(refusal(code, reason), Some(code.exit_code()));
}

impl Service {
    /// `log` = `ask.log` 的路径；`None`（测试）不记。
    pub fn new(log: Option<PathBuf>) -> Service {
        Service { gate: Gate::default(), memory: Mutex::new(Memory::default()), log }
    }

    pub fn gate(&self) -> &Gate {
        &self.gate
    }

    fn memory(&self) -> std::sync::MutexGuard<'_, Memory> {
        self.memory.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `dct ask --forget`。
    pub fn forget(&self, conversation_id: &str) {
        self.memory().forget(conversation_id);
    }

    /// 处理一次 ask，阻塞到这一轮结束。顺序固定：校验（`bad_request`）→ 配置（`not_enabled`，
    /// 此前没有任何网络请求）→ 在途标记（`busy`）→ 受理（`start`）→ 一轮。每次调用恰好
    /// 一个终止事件；对面已经关了连接就一个都不发。
    pub fn ask(&self, req: &AskRequest, resolve: &dyn Fn() -> Result<Target, &'static str>, peer: &mut dyn ClientGone, emit: Emit) {
        let started = Instant::now();
        let input = match validate(req) {
            Ok(i) => i,
            Err(reason) => return refuse(emit, ErrorCode::BadRequest, reason),
        };
        let note = |chars_out: usize, (first_ms, hdr_ms, delta_ms): (u64, u64, u64), outcome: &str| {
            if let Some(path) = &self.log {
                let m = Meta {
                    source: input.source.as_str(),
                    conversation_id: &input.conversation_id,
                    chars_in: input.text.chars().count(),
                    chars_out,
                    first_ms,
                    hdr_ms,
                    delta_ms,
                    total_ms: started.elapsed().as_millis() as u64,
                    outcome,
                };
                log::append(path, &m);
            }
        };
        let conv = input.conversation_id.as_str();
        let target = match resolve() {
            Ok(t) => t,
            Err(reason) => {
                note(0, (0, 0, 0), ErrorCode::NotEnabled.as_str());
                return refuse(emit, ErrorCode::NotEnabled, reason);
            }
        };
        let ticket = match self.gate.begin(conv) {
            Ok(t) => t,
            Err(_) => {
                note(0, (0, 0, 0), ErrorCode::Busy.as_str());
                return refuse(emit, ErrorCode::Busy, "in_flight");
            }
        };
        // 占到标记之后再读历史：同对话的旧一轮此刻已被取代，不会再写进来。
        let history = self.memory().history(conv, Instant::now());
        let start = json!({"v": 1, "type": "start", "conversation_id": conv, "turn": history.len() + 1});
        if emit(start, None).is_err() {
            return note(0, (0, 0, 0), "cancelled"); // 票随之丢掉，标记清除
        }
        let body = stream_body(&target.model, &prompt::messages(input.source, input.persona.as_deref(), &history, &input.text), prompt::MAX_TOKENS);
        let turn = Turn { opener: target.opener, body, started, deadline: input.timeout, poll: POLL };
        let mut chars_out = 0;
        let term = run_turn(
            turn,
            ticket,
            peer,
            &mut |i, s| {
                chars_out += s.chars().count();
                emit(json!({"v": 1, "type": "sentence", "i": i, "text": s}), None)
            },
            |d| {
                let turn = Remembered { user: input.text.clone(), reply: prompt::join_sentences(d.sentences), truncated: d.truncated };
                self.memory().commit(conv, turn, Instant::now());
            },
        );
        match &term {
            Terminal::End { truncated, first_ms, hdr_ms, delta_ms, .. } => note(chars_out, (*first_ms, *hdr_ms, *delta_ms), if *truncated { "truncated" } else { "end" }),
            Terminal::Error { code, .. } => note(chars_out, (0, 0, 0), code.as_str()),
            Terminal::Gone => note(chars_out, (0, 0, 0), "cancelled"),
        }
        if let Some(ev) = term.event() {
            let _ = emit(ev, Some(term.exit_code()));
        }
    }
}

/// 给 ask 让路（设计 §6.4「ask 优先于 dct 其他用同一网关的功能」）。包住守护进程里别的 LLM
/// 用户（出错解释、会话起名）：ask 在途时它们**先等**，等到 ask 结束才去碰网关；最多等
/// `wait`，期满不抢、直接返回 `Timeout`——那些功能本来就有不依赖 LLM 的退路。
/// 只管守护进程里的调用；`dct answer` / `dct do` / `dct game` 是别的进程，从这里管不到。
/// 也只是「不在 ask 在途时开始」，不会去打断已经发出去的那一次。
pub struct Yielding {
    inner: Arc<dyn Backend>,
    gate: Gate,
    wait: Duration,
}

impl Yielding {
    pub fn new(inner: Arc<dyn Backend>, gate: Gate) -> Yielding {
        Yielding { inner, gate, wait: Duration::from_secs(20) }
    }

    pub fn with_wait(mut self, wait: Duration) -> Yielding {
        self.wait = wait;
        self
    }

    fn pause(&self) -> Result<(), LlmError> {
        let t = Instant::now();
        while self.gate.busy() {
            if t.elapsed() >= self.wait {
                return Err(LlmError::Timeout);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }
}

impl Backend for Yielding {
    fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
        self.pause()?;
        self.inner.complete(p)
    }

    fn complete_counted(&self, p: &Prompt) -> Result<(String, Option<Usage>), LlmError> {
        self.pause()?;
        self.inner.complete_counted(p)
    }
}
