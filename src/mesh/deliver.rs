//! 留言：`dct peers` 的详情、`dct send`，以及收件这一侧怎么把留言送进会话。
//!
//! **留言不带执行权。** 送进会话的只是一段文字，外面包着一行标记（`marker`），
//! 让那边的智能体知道这是另一台电脑上的智能体说的，不是用户本人的指令。
//! 敲字走的是手机那一路已经在用的 `bridge::SessionWriter::type_into`，不另开
//! 一条敲键的路。
//!
//! 只送进**智能体会话**：命令行会话里敲一段字再按回车，就是替别人执行了一条
//! 命令。
//!
//! 线上的形状（`Receipt`、`StatusBody`）都在密封留言的正文里，中转看不见。
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use dct_mesh::roster::Member;
use dct_mesh::seal::{self, Kind, Message};
use dct_mesh::wire::{self, Payload};
use serde::{Deserialize, Serialize};

use super::net::Net;
use super::{Mesh, QueuedMsg};
use crate::link::LinkError;
use crate::proto::{MeshProblem, PeerView, SendOutcome, SessionBrief};
use crate::session::{SessionInfo, SessionState};

/// 留言正文洗过之后最多多少个字符。更长的该改成派活（第二步）。
pub const MAX_BODY_CHARS: usize = 8000;
/// 一个会话最多排多少条。满了就回 `Refused`，不挤掉旧的。
pub const QUEUE_CAP: usize = 50;
/// 发一条留言等回执多久。收件那边当场敲进去（或者排上队）就回，不等智能体。
pub const SEND_ASK_TIMEOUT: Duration = Duration::from_secs(10);
/// `dct peers` 问每台电脑的状态等多久。
pub const STATUS_ASK_TIMEOUT: Duration = Duration::from_secs(3);
/// 投递线程多久看一次排队。
pub const TICK: Duration = Duration::from_secs(1);
/// 不在 dct 会话里跑 `dct send`（`DCT_SESSION_ID` 没有）时，标记里写的发件方。
pub const FROM_TERMINAL: &str = "终端";
/// 标记里发件会话名最多几个字。名字是对方给的，不能让它撑满一屏。
const MAX_LABEL_CHARS: usize = 64;

/// 送进会话的那段文字：第一行是标记，第二行起是留言原文。
pub fn marker(from_machine: &str, from_session: &str, id: &str, body: &str) -> String {
    let short: String = id.chars().take(4).collect();
    format!("[来自 {from_machine}/{from_session} 的留言 #{short}]\n{body}")
}

/// 这个状态下能不能插话：只有 `Idle`。`Asking` 是智能体在等用户拍板，
/// 这时候塞一句别人的话进去，会被当成用户的回答。
pub fn ready(state: SessionState) -> bool {
    state == SessionState::Idle
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooLong;

/// 去掉控制字符和转义序列（`session::sanitize`），逐行洗，换行留着；再卡长度。
pub fn clean_body(text: &str) -> Result<String, TooLong> {
    let lines: Vec<String> = text.split('\n').map(crate::session::sanitize).collect();
    let body = lines.join("\n").trim_end().to_string();
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(TooLong);
    }
    Ok(body)
}

/// 标记里的发件会话名：对方给的，洗干净、截短，空了就是「终端」。
fn clean_label(s: &str) -> String {
    let s: String = crate::session::sanitize(s)
        .chars()
        .take(MAX_LABEL_CHARS)
        .collect();
    let s = s.trim();
    if s.is_empty() {
        FROM_TERMINAL.to_string()
    } else {
        s.to_string()
    }
}

/// 给人看的会话状态。
pub fn state_label(s: SessionState) -> &'static str {
    match s {
        SessionState::Working => "忙",
        SessionState::Idle => "闲",
        SessionState::Asking => "等你回答",
        SessionState::Stopped => "已停止",
        SessionState::Failed => "出错了",
        SessionState::Unknown => "不清楚",
    }
}

/// 看板上组头的名字：项目目录的最后一段（同 `ui::view` 里组名的算法）。
pub fn project_name(dir: &str) -> String {
    Path::new(dir)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.to_string())
}

/// 按地址里的会话名找会话。
///
/// - `#12` 直接是会话编号；
/// - 否则跟看板上的名字比：会话那一行的名字（`ui::widgets::session_label`，
///   起过名就是名字，没起过是 profile），或者它所在的项目名（组头）。
///
/// 找不到是 `Err(空)`；不止一个是 `Err(候选)`，候选写成 `#编号 名字（项目）`。
pub fn resolve<'a>(
    sessions: &'a [SessionInfo],
    want: &str,
) -> Result<&'a SessionInfo, Vec<String>> {
    let want = want.trim();
    if let Some(n) = want.strip_prefix('#') {
        let id: Option<u32> = n.parse().ok();
        return sessions
            .iter()
            .find(|s| Some(s.id) == id)
            .ok_or_else(Vec::new);
    }
    let hits: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| crate::ui::widgets::session_label(s) == want || project_name(&s.dir) == want)
        .collect();
    match hits.as_slice() {
        [one] => Ok(one),
        many => Err(many
            .iter()
            .map(|s| {
                format!(
                    "#{} {}（{}）",
                    s.id,
                    crate::ui::widgets::session_label(s),
                    project_name(&s.dir)
                )
            })
            .collect()),
    }
}

/// `电脑名/会话名`。电脑名里不许有 `/`（`mesh::valid_name`），所以按第一个
/// `/` 切；会话名里有 `/` 也不要紧。
pub fn split_address(to: &str) -> Option<(&str, &str)> {
    let (m, s) = to.trim().split_once('/')?;
    let (m, s) = (m.trim(), s.trim());
    (!m.is_empty() && !s.is_empty()).then_some((m, s))
}

/// 收件方回给发件方的回执，在密封留言的正文里（中转看不见）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "r", rename_all = "snake_case")]
pub enum Receipt {
    Delivered,
    Queued,
    NoSuchSession { candidates: Vec<String> },
    SessionStopped,
    Refused,
}

impl From<Receipt> for SendOutcome {
    fn from(r: Receipt) -> SendOutcome {
        match r {
            Receipt::Delivered => SendOutcome::Delivered,
            Receipt::Queued => SendOutcome::Queued,
            Receipt::NoSuchSession { candidates } => SendOutcome::NoSuchSession(candidates),
            Receipt::SessionStopped => SendOutcome::SessionStopped,
            Receipt::Refused => SendOutcome::Refused,
        }
    }
}

/// `Status` 的正文。状态按枚举名发，到了发件那边再翻成人话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusBody {
    pub os: String,
    pub sessions: Vec<StatusSession>,
    pub tentacles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSession {
    pub name: String,
    pub state: SessionState,
    pub dir: String,
}

/// 收件这一侧要的、守护进程里的东西：会话清单、往会话里敲字、触手。
pub trait Inbox: Send + Sync {
    fn sessions(&self) -> Vec<SessionInfo>;
    /// 敲进去**并按回车**。跟手机那一路是同一个方法（`SessionWriter::type_into`）。
    fn type_into(&self, id: u32, text: &str) -> Result<(), String>;
    fn tentacles(&self) -> Vec<String>;
}

/// 守护进程里真的那一个。
pub struct LocalInbox {
    pub mgr: Arc<crate::session::SessionManager>,
    /// `~/.dco/endpoint.json`。找不到家目录就是 `None`。
    pub dco: Option<PathBuf>,
}

impl LocalInbox {
    pub fn new(mgr: Arc<crate::session::SessionManager>) -> LocalInbox {
        LocalInbox {
            mgr,
            dco: crate::sys::home().map(|h| h.join(".dco").join("endpoint.json")),
        }
    }
}

impl Inbox for LocalInbox {
    fn sessions(&self) -> Vec<SessionInfo> {
        self.mgr.list()
    }

    fn type_into(&self, id: u32, text: &str) -> Result<(), String> {
        crate::bridge::SessionWriter::type_into(&*self.mgr, id, text)
    }

    fn tentacles(&self) -> Vec<String> {
        self.dco.as_deref().map(read_tentacles).unwrap_or_default()
    }
}

/// 从 dco 的 `endpoint.json` 里读 `capabilities`：字符串照用，对象取它的
/// `name`（没有就 `kind`）。读不到、格式不对都是空，不报错——dco 没装是常态。
pub fn read_tentacles(path: &Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let Some(caps) = v.get("capabilities").and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    caps.iter()
        .filter_map(|c| match c {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(o) => o
                .get("name")
                .or_else(|| o.get("kind"))
                .and_then(|n| n.as_str())
                .map(str::to_string),
            _ => None,
        })
        .map(|s| clean_label(&s))
        .collect()
}

fn lock(m: &Mutex<Mesh>) -> std::sync::MutexGuard<'_, Mesh> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Mesh {
    pub fn with_inbox(mut self, inbox: Arc<dyn Inbox>) -> Mesh {
        self.inbox = Some(inbox);
        self
    }

    /// 一条验过的留言（`on_envelope` 已经做完名单、签名、时间窗、去重）：
    /// 找到会话，空着就敲进去，忙就排队。
    pub(super) fn receive(&mut self, m: &Message) -> Receipt {
        let r = self.receive_inner(m);
        self.journal.mesh(&format!(
            "msg from={} id={} to={} result={}",
            m.from,
            m.id,
            clean_label(&m.to_session),
            match &r {
                Receipt::Delivered => "delivered",
                Receipt::Queued => "queued",
                Receipt::NoSuchSession { .. } => "no_such_session",
                Receipt::SessionStopped => "session_stopped",
                Receipt::Refused => "refused",
            }
        ));
        r
    }

    fn receive_inner(&mut self, m: &Message) -> Receipt {
        let Some(inbox) = self.inbox.clone() else {
            return Receipt::Refused;
        };
        let Ok(body) = clean_body(&m.body) else {
            return Receipt::Refused;
        };
        let sessions = inbox.sessions();
        let s = match resolve(&sessions, &m.to_session) {
            Ok(s) => s,
            Err(candidates) => return Receipt::NoSuchSession { candidates },
        };
        if matches!(s.state, SessionState::Stopped | SessionState::Failed) {
            return Receipt::SessionStopped;
        }
        if !s.is_agent {
            return Receipt::Refused;
        }
        let from = self
            .roster
            .as_ref()
            .and_then(|r| r.roster.member(&m.from))
            .map(|x| x.name.clone())
            .unwrap_or_else(|| m.from.clone());
        let text = marker(&from, &clean_label(&m.from_session), &m.id, &body);
        let queued = self.queues.get(&s.id).map_or(0, |q| q.len());
        // 前面还有排着的就排在它们后面，哪怕会话此刻空着：先来的先送。
        if queued == 0 && ready(s.state) {
            return match inbox.type_into(s.id, &text) {
                Ok(()) => Receipt::Delivered,
                Err(_) => Receipt::Refused,
            };
        }
        if queued >= QUEUE_CAP {
            return Receipt::Refused;
        }
        self.queues.entry(s.id).or_default().push_back(QueuedMsg {
            msg: m.clone(),
            text,
            received_at: Instant::now(),
        });
        Receipt::Queued
    }

    /// 投递线程每秒调一次：每个排着队的会话，空着就送**一条**（送完它就在忙，
    /// 下一条等它再空下来）；会话没了或者停了，它的队整个丢掉，记一行。
    pub fn deliver_queued(&mut self) {
        if self.queues.is_empty() {
            return;
        }
        let Some(inbox) = self.inbox.clone() else {
            return;
        };
        let sessions = inbox.sessions();
        let mut ids: Vec<u32> = self.queues.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let state = sessions.iter().find(|s| s.id == id).map(|s| s.state);
            match state {
                None | Some(SessionState::Stopped) => {
                    let n = self.queues.remove(&id).map(|q| q.len()).unwrap_or(0);
                    self.journal
                        .mesh(&format!("queue_dropped session={id} count={n}"));
                }
                Some(st) if ready(st) => {
                    let Some(q) = self.queues.get_mut(&id) else {
                        continue;
                    };
                    let Some(next) = q.pop_front() else {
                        continue;
                    };
                    let how = match inbox.type_into(id, &next.text) {
                        Ok(()) => "delivered",
                        Err(_) => "type_failed",
                    };
                    self.journal.mesh(&format!(
                        "queued_msg from={} id={} session={id} result={how}",
                        next.msg.from, next.msg.id
                    ));
                }
                Some(_) => {}
            }
        }
        self.queues.retain(|_, q| !q.is_empty());
    }

    /// 这台电脑此刻的样子，回 `StatusRequest` 用，`dct peers` 里自己那一行也用。
    pub(super) fn local_status(&self) -> StatusBody {
        let (sessions, tentacles) = match &self.inbox {
            Some(i) => (i.sessions(), i.tentacles()),
            None => (Vec::new(), Vec::new()),
        };
        StatusBody {
            os: std::env::consts::OS.to_string(),
            sessions: sessions
                .iter()
                .map(|s| StatusSession {
                    name: crate::ui::widgets::session_label(s).to_string(),
                    state: s.state,
                    dir: s.dir.clone(),
                })
                .collect(),
            tentacles,
        }
    }

    /// 发件会话在标记里叫什么：看板上的名字；不在会话里是「终端」。
    fn session_name_of(&self, id: Option<u32>) -> String {
        let Some(id) = id else {
            return FROM_TERMINAL.to_string();
        };
        self.inbox
            .as_ref()
            .and_then(|i| {
                i.sessions()
                    .into_iter()
                    .find(|s| s.id == id)
                    .map(|s| crate::ui::widgets::session_label(&s).to_string())
            })
            .unwrap_or_else(|| format!("#{id}"))
    }

    /// 一条新消息，id 是新的随机数。**重发也要走这里拿新 id**：收件那边按
    /// `(from, id)` 去重，同一个 id 再来一次会被悄悄丢掉。
    fn new_message(
        &self,
        kind: Kind,
        to: &str,
        from_session: &str,
        to_session: &str,
        body: &str,
    ) -> Message {
        let r = (self.rand)();
        let id: String = r[..8].iter().map(|b| format!("{b:02x}")).collect();
        Message {
            id,
            kind,
            from: self.me.endpoint.clone(),
            from_session: from_session.to_string(),
            to: to.to_string(),
            to_session: to_session.to_string(),
            body: body.to_string(),
            sent_at: (self.clock)(),
        }
    }

    fn seal_for(&self, to: &Member, m: &Message) -> Option<Vec<u8>> {
        let kx: [u8; 32] = STANDARD.decode(&to.kx_pub).ok()?.try_into().ok()?;
        let sealed = seal::seal(m, &self.keys, &kx, (self.rand)());
        Some(wire::encode(&Payload::Sealed(sealed)))
    }

    /// 打开 `ask` 回来的答复：必须是 `from` 签的、`kind` 对、id 就是我发的那条。
    fn open_answer(&self, bytes: &[u8], from: &str, kind: Kind, id: &str) -> Option<Message> {
        let Ok(Payload::Sealed(s)) = wire::decode(bytes) else {
            return None;
        };
        let roster = self.roster.as_ref()?;
        let m = seal::open(&s, &self.keys, &roster.roster, from).ok()?;
        (m.kind == kind && m.id == id && m.from == from).then_some(m)
    }
}

fn peer_view(name: String, online: bool, status: Option<StatusBody>) -> PeerView {
    let s = status.unwrap_or(StatusBody {
        os: String::new(),
        sessions: Vec::new(),
        tentacles: Vec::new(),
    });
    PeerView {
        name,
        online,
        os: s.os,
        sessions: s
            .sessions
            .into_iter()
            .map(|x| SessionBrief {
                name: clean_label(&x.name),
                state: state_label(x.state).to_string(),
                dir: crate::session::sanitize(&x.dir),
            })
            .collect(),
        tentacles: s.tentacles.iter().map(|t| clean_label(t)).collect(),
    }
}

/// `dct peers` 的详情：名单上每台电脑，按名单顺序，自己也在里面。
///
/// 在线的每台并排问一句加密的 `StatusRequest`，最多等 3 秒；没回话的也算
/// 不在线，只有名字。**问的时候不攥着锁。**
pub fn peers(mesh: &Mutex<Mesh>, net: &dyn Net) -> Result<Vec<PeerView>, MeshProblem> {
    let online: HashSet<String> = net.peers().unwrap_or_default().into_iter().collect();
    let (members, mine, asks) = {
        let m = lock(mesh);
        let roster = m.roster.as_ref().ok_or(MeshProblem::NotLoggedIn)?;
        let members: Vec<Member> = roster.roster.members.clone();
        let mut asks = Vec::new();
        for x in &members {
            if x.endpoint == m.me.endpoint || !online.contains(&x.endpoint) {
                continue;
            }
            let msg = m.new_message(Kind::StatusRequest, &x.endpoint, FROM_TERMINAL, "", "");
            if let Some(p) = m.seal_for(x, &msg) {
                asks.push((x.endpoint.clone(), msg.id, p));
            }
        }
        (members, m.local_status(), asks)
    };
    let replies: Vec<(String, String, Vec<u8>)> = std::thread::scope(|s| {
        let hs: Vec<_> = asks
            .into_iter()
            .map(|(ep, id, p)| {
                s.spawn(move || (ep.clone(), id, net.ask(&ep, p, STATUS_ASK_TIMEOUT)))
            })
            .collect();
        hs.into_iter()
            .filter_map(|h| h.join().ok())
            .filter_map(|(ep, id, r)| r.ok().map(|b| (ep, id, b)))
            .collect()
    });
    let m = lock(mesh);
    let mut mine = Some(mine);
    Ok(members
        .into_iter()
        .map(|x| {
            if x.endpoint == m.me.endpoint {
                return peer_view(x.name, true, mine.take());
            }
            let status = replies
                .iter()
                .find(|(ep, _, _)| *ep == x.endpoint)
                .and_then(|(ep, id, b)| m.open_answer(b, ep, Kind::Status, id))
                .and_then(|a| serde_json::from_str::<StatusBody>(&a.body).ok());
            peer_view(x.name, status.is_some(), status)
        })
        .collect())
}

/// `dct send`：给 `to`（`电脑名/会话名`）留一句话，等对方回执。
///
/// 中转回 `Busy`（对方的收件箱满了，这一封没收下）时换一个**新 id** 再试
/// 一次。别的失败不重试：没等到回执不等于没送到，再发一次可能敲两遍。
pub fn send(
    mesh: &Mutex<Mesh>,
    net: &dyn Net,
    to: &str,
    text: &str,
    from_session: Option<u32>,
) -> Result<SendOutcome, MeshProblem> {
    let body = clean_body(text).map_err(|_| MeshProblem::TooLong)?;
    let (machine, session) =
        split_address(to).ok_or_else(|| MeshProblem::BadAddress(to.to_string()))?;
    let (target, from_name, is_me) = {
        let m = lock(mesh);
        let roster = m.roster.as_ref().ok_or(MeshProblem::NotLoggedIn)?;
        let Some(t) = roster
            .roster
            .by_name(machine)
            .or_else(|| roster.roster.member(machine))
            .cloned()
        else {
            return Ok(SendOutcome::NoSuchMachine);
        };
        let is_me = t.endpoint == m.me.endpoint;
        (t, m.session_name_of(from_session), is_me)
    };
    // 发给这台电脑自己的会话：不过中转，走同一个收件口。
    if is_me {
        let mut m = lock(mesh);
        let msg = m.new_message(Kind::Msg, &target.endpoint, &from_name, session, &body);
        return Ok(m.receive(&msg).into());
    }
    for attempt in 0..2 {
        let (id, payload) = {
            let m = lock(mesh);
            let msg = m.new_message(Kind::Msg, &target.endpoint, &from_name, session, &body);
            let Some(p) = m.seal_for(&target, &msg) else {
                return Ok(SendOutcome::Refused);
            };
            (msg.id, p)
        };
        match net.ask(&target.endpoint, payload, SEND_ASK_TIMEOUT) {
            Ok(reply) => {
                let m = lock(mesh);
                let r = m
                    .open_answer(&reply, &target.endpoint, Kind::Receipt, &id)
                    .and_then(|a| serde_json::from_str::<Receipt>(&a.body).ok());
                return Ok(r.map(Into::into).unwrap_or(SendOutcome::NoAnswer));
            }
            Err(LinkError::Relay(dct_link::LinkError::Offline)) => return Ok(SendOutcome::Offline),
            Err(LinkError::Relay(dct_link::LinkError::Busy)) if attempt == 0 => {
                lock(mesh)
                    .journal
                    .mesh(&format!("send_busy_retry to={} id={id}", target.endpoint));
                continue;
            }
            Err(LinkError::Relay(dct_link::LinkError::NoAnswer)) | Err(LinkError::Unreachable) => {
                return Ok(SendOutcome::NoAnswer)
            }
            Err(_) => return Ok(SendOutcome::Refused),
        }
    }
    Ok(SendOutcome::Refused)
}

/// 投递线程的一拍。
pub fn tick(mesh: &Mutex<Mesh>) {
    lock(mesh).deliver_queued();
}

#[cfg(test)]
pub mod testing {
    use super::*;

    /// 内存里的会话清单。`type_into` 记下敲了什么、给谁，并且像真的一样把
    /// 那个会话推进 `Working`。
    #[derive(Default)]
    pub struct FakeInbox {
        pub sessions: Mutex<Vec<SessionInfo>>,
        pub typed: Mutex<Vec<(u32, String)>>,
        pub tentacles: Vec<String>,
    }

    impl FakeInbox {
        pub fn new() -> Arc<FakeInbox> {
            Arc::new(FakeInbox::default())
        }

        pub fn add(&self, id: u32, tag: &str, dir: &str, state: SessionState, is_agent: bool) {
            self.sessions.lock().unwrap().push(SessionInfo {
                id,
                profile: "claude".into(),
                dir: dir.into(),
                state,
                activity: String::new(),
                is_agent,
                tag: tag.into(),
            });
        }

        pub fn set_state(&self, id: u32, st: SessionState) {
            for s in self.sessions.lock().unwrap().iter_mut() {
                if s.id == id {
                    s.state = st;
                }
            }
        }

        pub fn remove(&self, id: u32) {
            self.sessions.lock().unwrap().retain(|s| s.id != id);
        }

        pub fn typed(&self) -> Vec<(u32, String)> {
            self.typed.lock().unwrap().clone()
        }
    }

    impl Inbox for FakeInbox {
        fn sessions(&self) -> Vec<SessionInfo> {
            self.sessions.lock().unwrap().clone()
        }

        fn type_into(&self, id: u32, text: &str) -> Result<(), String> {
            if !self.sessions.lock().unwrap().iter().any(|s| s.id == id) {
                return Err("gone".into());
            }
            self.typed.lock().unwrap().push((id, text.to_string()));
            self.set_state(id, SessionState::Working);
            Ok(())
        }

        fn tentacles(&self) -> Vec<String> {
            self.tentacles.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeInbox;
    use super::*;
    use crate::mesh::net::testing::{FakeHub, FakeNet};
    use dct_link::{EndpointId, Envelope};
    use dct_mesh::roster::{self, SignedRoster};
    use dct_mesh::{id, MachineKeys};
    use std::sync::atomic::{AtomicU8, Ordering};
    use SessionState::*;

    fn keys(b: u8) -> MachineKeys {
        MachineKeys::from_seeds([b; 32], [b.wrapping_add(100); 32]).unwrap()
    }

    fn member(k: &MachineKeys, name: &str) -> Member {
        Member {
            name: name.into(),
            endpoint: id::endpoint_for(&k.sign_pub()),
            sign_pub: STANDARD.encode(k.sign_pub()),
            kx_pub: STANDARD.encode(k.kx_pub()),
            added_at: 1,
        }
    }

    /// A、B、C 三台一个组（A 签的 v2）。
    fn roster3() -> SignedRoster {
        let (ka, kb, kc) = (keys(1), keys(2), keys(3));
        let ma = member(&ka, "A");
        let g = roster::genesis(ma.clone(), "mine".into(), &ka);
        let mut r = g.roster.clone();
        r.version = 2;
        r.members.push(member(&kb, "B"));
        r.members.push(member(&kc, "C"));
        roster::sign(r, &ma, &ka)
    }

    /// 随机数按调用次数递增：第 n 次调用得到 `[n; 32]`。每次 `send` 先取一次
    /// 给 id、再取一次给密封，所以第一条留言的 id 是 `0101…`，第二条 `0303…`。
    fn counting_rand() -> impl Fn() -> [u8; 32] + Send + 'static {
        let n = AtomicU8::new(0);
        move || [n.fetch_add(1, Ordering::SeqCst) + 1; 32]
    }

    struct Node {
        mesh: Arc<Mutex<Mesh>>,
        net: FakeNet,
        ep: String,
        inbox: Arc<FakeInbox>,
    }

    fn node(hub: &Arc<FakeHub>, seed: u8, name: &str, r: &SignedRoster) -> Node {
        let inbox = FakeInbox::new();
        let mesh = Mesh::new(keys(seed), name.into(), Some(r.clone()))
            .with_rand(counting_rand())
            .with_inbox(inbox.clone());
        let mesh = Arc::new(Mutex::new(mesh));
        hub.register(mesh.clone());
        let ep = mesh.lock().unwrap().endpoint().to_string();
        Node {
            net: hub.net_for(&ep),
            mesh,
            ep,
            inbox,
        }
    }

    fn hex_id(n: u8) -> String {
        format!("{n:02x}").repeat(8)
    }

    // —— 纯函数 ——

    #[test]
    fn the_marker_is_the_fixed_two_line_format() {
        assert_eq!(
            marker(
                "公司Windows",
                "dc-terminal",
                "a1b2c3d4e5",
                "跑一下 Windows 测试"
            ),
            "[来自 公司Windows/dc-terminal 的留言 #a1b2]\n跑一下 Windows 测试"
        );
        assert_eq!(
            marker("家里Mac", "终端", "ab", "第一行\n第二行"),
            "[来自 家里Mac/终端 的留言 #ab]\n第一行\n第二行"
        );
    }

    #[test]
    fn only_idle_is_ready() {
        let table = [
            (Idle, true),
            (Working, false),
            (Asking, false),
            (Stopped, false),
            (Failed, false),
            (Unknown, false),
        ];
        for (s, want) in table {
            assert_eq!(ready(s), want, "{s:?}");
        }
    }

    #[test]
    fn a_body_is_cleaned_of_escapes_and_control_bytes_and_keeps_its_lines() {
        assert_eq!(
            clean_body("\x1b[31m红\x1b[0m字\x07\r\n第二行\x1b[2J\n\x1b]0;标题\x07").unwrap(),
            "红字\n第二行\n]0;标题"
        );
        // 退格吃掉前一个字，跟终端里一样。
        assert_eq!(clean_body("ab\x7fc").unwrap(), "ac");
    }

    #[test]
    fn a_body_over_8000_chars_is_refused() {
        assert_eq!(
            clean_body(&"字".repeat(8000)).unwrap().chars().count(),
            8000
        );
        assert_eq!(clean_body(&"字".repeat(8001)), Err(TooLong));
        // 按洗过之后算：转义序列不占名额。
        let padded = format!("{}\x1b[0m\x1b[0m", "a".repeat(8000));
        assert!(clean_body(&padded).is_ok());
    }

    fn info(id: u32, tag: &str, dir: &str) -> SessionInfo {
        SessionInfo {
            id,
            profile: "claude".into(),
            dir: dir.into(),
            state: Idle,
            activity: String::new(),
            is_agent: true,
            tag: tag.into(),
        }
    }

    #[test]
    fn a_session_is_found_by_number_board_name_or_project() {
        let s = vec![
            info(3, "修登录白屏", "/w/dc-terminal"),
            info(5, "", "/w/dc_classroom"),
        ];
        assert_eq!(resolve(&s, "#5").unwrap().id, 5);
        assert_eq!(resolve(&s, "修登录白屏").unwrap().id, 3);
        assert_eq!(resolve(&s, "dc-terminal").unwrap().id, 3);
        // 没起名的会话，看板上写的是 profile。
        assert_eq!(resolve(&s, "claude").unwrap().id, 5);
        assert_eq!(resolve(&s, "#9").map(|x| x.id), Err(vec![]));
        assert_eq!(resolve(&s, "#x").map(|x| x.id), Err(vec![]));
        assert_eq!(resolve(&s, "没有").map(|x| x.id), Err(vec![]));
    }

    #[test]
    fn two_sessions_with_the_same_name_are_ambiguous_and_listed() {
        let s = vec![
            info(3, "", "/w/dc-terminal"),
            info(4, "写文档", "/w/dc-terminal"),
            info(6, "", "/w/other"),
        ];
        assert_eq!(
            resolve(&s, "dc-terminal").map(|x| x.id),
            Err(vec![
                "#3 claude（dc-terminal）".to_string(),
                "#4 写文档（dc-terminal）".to_string()
            ])
        );
        assert_eq!(
            resolve(&s, "claude").unwrap_err().len(),
            2,
            "3 号和 6 号在看板上都叫 claude"
        );
    }

    #[test]
    fn addresses_split_at_the_first_slash() {
        assert_eq!(
            split_address("公司Windows/dc-terminal"),
            Some(("公司Windows", "dc-terminal"))
        );
        assert_eq!(split_address(" A / #3 "), Some(("A", "#3")));
        assert_eq!(split_address("A/a/b"), Some(("A", "a/b")));
        assert_eq!(split_address("A"), None);
        assert_eq!(split_address("A/"), None);
        assert_eq!(split_address("/s"), None);
    }

    #[test]
    fn tentacles_come_from_the_dco_endpoint_file_or_are_empty() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("endpoint.json");
        assert!(read_tentacles(&p).is_empty(), "没有文件");
        std::fs::write(&p, "{not json").unwrap();
        assert!(read_tentacles(&p).is_empty(), "坏文件");
        std::fs::write(&p, r#"{"device":"mac"}"#).unwrap();
        assert!(read_tentacles(&p).is_empty(), "没有 capabilities");
        std::fs::write(
            &p,
            r#"{"capabilities":["iPhone 镜像",{"name":"摄像头"},{"kind":"gpu"},7]}"#,
        )
        .unwrap();
        assert_eq!(read_tentacles(&p), ["iPhone 镜像", "摄像头", "gpu"]);
    }

    // —— 收件：排队的规矩 ——

    /// B 上一个会话 #7（`dc-terminal`），状态 `state`。直接喂 `receive`。
    fn receiver(state: SessionState, is_agent: bool) -> (Mesh, Arc<FakeInbox>, SignedRoster) {
        let r = roster3();
        let inbox = FakeInbox::new();
        inbox.add(7, "", "/w/dc-terminal", state, is_agent);
        let b = Mesh::new(keys(2), "B".into(), Some(r.clone())).with_inbox(inbox.clone());
        (b, inbox, r)
    }

    fn msg_from_a(r: &SignedRoster, id: &str, to_session: &str, body: &str) -> Message {
        Message {
            id: id.into(),
            kind: Kind::Msg,
            from: r.roster.by_name("A").unwrap().endpoint.clone(),
            from_session: "写文档".into(),
            to: r.roster.by_name("B").unwrap().endpoint.clone(),
            to_session: to_session.into(),
            body: body.into(),
            sent_at: 0,
        }
    }

    #[test]
    fn an_idle_session_gets_the_marker_typed_in_at_once() {
        let (mut b, inbox, r) = receiver(Idle, true);
        let m = msg_from_a(&r, "a1b2c3", "dc-terminal", "跑一下测试");
        assert_eq!(b.receive(&m), Receipt::Delivered);
        assert_eq!(
            inbox.typed(),
            [(7, "[来自 A/写文档 的留言 #a1b2]\n跑一下测试".to_string())]
        );
        assert!(b.queues.is_empty());
    }

    #[test]
    fn a_busy_session_queues_first_in_first_out_one_at_a_time() {
        let (mut b, inbox, r) = receiver(Working, true);
        for i in 1..=3 {
            let m = msg_from_a(&r, &format!("id{i}"), "#7", &format!("第{i}条"));
            assert_eq!(b.receive(&m), Receipt::Queued);
        }
        assert!(inbox.typed().is_empty(), "忙的时候一个字都不敲");

        // 还在忙：一拍过去什么都不送。
        b.deliver_queued();
        assert!(inbox.typed().is_empty());

        // 空下来：只送最早那一条；送完它就在忙，下一拍不送。
        inbox.set_state(7, Idle);
        b.deliver_queued();
        b.deliver_queued();
        let typed: Vec<String> = inbox.typed().into_iter().map(|(_, t)| t).collect();
        assert_eq!(typed, [marker("A", "写文档", "id1", "第1条")]);

        // 又空下来：第二条。
        inbox.set_state(7, Idle);
        b.deliver_queued();
        inbox.set_state(7, Idle);
        b.deliver_queued();
        let typed: Vec<String> = inbox.typed().into_iter().map(|(_, t)| t).collect();
        assert_eq!(
            typed,
            [
                marker("A", "写文档", "id1", "第1条"),
                marker("A", "写文档", "id2", "第2条"),
                marker("A", "写文档", "id3", "第3条"),
            ]
        );
        assert!(b.queues.is_empty(), "送完的队列不留空壳");
    }

    /// 前面还有排着的，会话此刻空着也排到后面：先来的先送。
    #[test]
    fn a_new_message_waits_behind_queued_ones_even_when_idle() {
        let (mut b, inbox, r) = receiver(Working, true);
        assert_eq!(
            b.receive(&msg_from_a(&r, "old1", "#7", "先")),
            Receipt::Queued
        );
        inbox.set_state(7, Idle);
        assert_eq!(
            b.receive(&msg_from_a(&r, "new1", "#7", "后")),
            Receipt::Queued
        );
        assert!(inbox.typed().is_empty());
        b.deliver_queued();
        assert_eq!(inbox.typed()[0].1, marker("A", "写文档", "old1", "先"));
    }

    #[test]
    fn the_queue_holds_at_most_50_and_refuses_the_next() {
        let (mut b, _inbox, r) = receiver(Working, true);
        for i in 0..QUEUE_CAP {
            assert_eq!(
                b.receive(&msg_from_a(&r, &format!("q{i}"), "#7", "x")),
                Receipt::Queued
            );
        }
        assert_eq!(
            b.receive(&msg_from_a(&r, "q50", "#7", "x")),
            Receipt::Refused
        );
        assert_eq!(b.queues[&7].len(), 50);
        assert_eq!(b.queues[&7][0].msg.id, "q0", "满了拒新的，不挤旧的");
    }

    #[test]
    fn asking_is_not_ready_and_queues() {
        let (mut b, inbox, r) = receiver(Asking, true);
        assert_eq!(b.receive(&msg_from_a(&r, "a", "#7", "x")), Receipt::Queued);
        assert!(inbox.typed().is_empty(), "在等用户拍板时不插话");
    }

    #[test]
    fn a_stopped_or_failed_session_says_so() {
        for st in [Stopped, Failed] {
            let (mut b, inbox, r) = receiver(st, true);
            assert_eq!(
                b.receive(&msg_from_a(&r, "a", "#7", "x")),
                Receipt::SessionStopped
            );
            assert!(inbox.typed().is_empty() && b.queues.is_empty(), "{st:?}");
        }
    }

    /// 命令行会话里敲字再回车就是替人执行命令：不送。
    #[test]
    fn a_shell_session_never_gets_a_message() {
        for st in [Idle, Working, Unknown] {
            let (mut b, inbox, r) = receiver(st, false);
            assert_eq!(
                b.receive(&msg_from_a(&r, "a", "#7", "ls")),
                Receipt::Refused
            );
            assert!(inbox.typed().is_empty() && b.queues.is_empty(), "{st:?}");
        }
    }

    #[test]
    fn a_receiver_refuses_an_over_long_body_and_cleans_a_dirty_one() {
        let (mut b, inbox, r) = receiver(Idle, true);
        let long = msg_from_a(&r, "l", "#7", &"x".repeat(MAX_BODY_CHARS + 1));
        assert_eq!(b.receive(&long), Receipt::Refused);
        let mut dirty = msg_from_a(&r, "d", "#7", "\x1b[31m红\x1b[0m\r字");
        dirty.from_session = "\x1b[2J坏\n名".into();
        assert_eq!(b.receive(&dirty), Receipt::Delivered);
        assert_eq!(inbox.typed()[0].1, "[来自 A/坏名 的留言 #d]\n红字");
    }

    #[test]
    fn an_ambiguous_or_missing_session_is_no_such_session() {
        let (mut b, inbox, r) = receiver(Idle, true);
        inbox.add(8, "", "/w/dc-terminal", Idle, true);
        assert_eq!(
            b.receive(&msg_from_a(&r, "a", "dc-terminal", "x")),
            Receipt::NoSuchSession {
                candidates: vec![
                    "#7 claude（dc-terminal）".into(),
                    "#8 claude（dc-terminal）".into()
                ]
            }
        );
        assert_eq!(
            b.receive(&msg_from_a(&r, "b", "nope", "x")),
            Receipt::NoSuchSession { candidates: vec![] }
        );
        assert!(inbox.typed().is_empty());
    }

    /// 会话关掉（从清单上没了，或者停了）：它排着的留言整个丢掉，记一行。
    #[test]
    fn a_queue_is_dropped_with_a_journal_line_when_its_session_goes_away() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("sessions.log");
        let j = crate::journal::Journal::new();
        j.set_path(path.clone());
        let (b, inbox, r) = receiver(Working, true);
        let mut b = b.with_journal(Arc::new(j));
        inbox.add(9, "", "/w/other", Working, true);
        b.receive(&msg_from_a(&r, "a", "#7", "x"));
        b.receive(&msg_from_a(&r, "b", "#7", "y"));
        b.receive(&msg_from_a(&r, "c", "#9", "z"));

        inbox.remove(7);
        inbox.set_state(9, Stopped);
        b.deliver_queued();
        assert!(b.queues.is_empty());
        assert!(inbox.typed().is_empty());
        let log = std::fs::read_to_string(&path).unwrap();
        assert!(log.contains("queue_dropped session=7 count=2"), "{log}");
        assert!(log.contains("queue_dropped session=9 count=1"), "{log}");
    }

    /// 同一个信封来两次：去重在 `on_envelope` 里，第二次根本到不了敲字那一步。
    #[test]
    fn a_duplicate_message_is_typed_only_once() {
        let r = roster3();
        let a = Mesh::new(keys(1), "A".into(), Some(r.clone()));
        let (mut b, inbox, _) = receiver(Idle, true);
        let mut m = msg_from_a(&r, "dup1", "#7", "只敲一次");
        m.sent_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let sealed = seal::seal(&m, &a.keys, &b.keys.kx_pub(), [3; 32]);
        let env = Envelope {
            from: EndpointId::new(a.endpoint()).unwrap(),
            to: EndpointId::new(b.endpoint()).unwrap(),
            seq: 1,
            payload: wire::encode(&Payload::Sealed(sealed)),
            recipients: vec![],
        };
        assert!(b.on_envelope(&env).is_some());
        inbox.set_state(7, Idle);
        assert_eq!(b.on_envelope(&env), None, "第二次被去重丢掉，连回执都没有");
        assert_eq!(inbox.typed().len(), 1);
    }

    // —— 两台电脑，端到端 ——

    #[test]
    fn a_message_to_an_idle_session_is_typed_in_with_the_marker() {
        let hub = FakeHub::new();
        let r = roster3();
        let a = node(&hub, 1, "A", &r);
        let b = node(&hub, 2, "B", &r);
        a.inbox.add(1, "写文档", "/a/docs", Working, true);
        b.inbox.add(4, "", "/w/dc-terminal", Idle, true);

        let out = send(
            &a.mesh,
            &a.net,
            "B/dc-terminal",
            "跑一下 Windows 测试",
            Some(1),
        );
        assert_eq!(out, Ok(SendOutcome::Delivered));
        assert_eq!(
            b.inbox.typed(),
            [(4, marker("A", "写文档", &hex_id(1), "跑一下 Windows 测试"))]
        );
        assert_eq!(
            b.inbox.typed()[0].1,
            "[来自 A/写文档 的留言 #0101]\n跑一下 Windows 测试"
        );
        assert!(a.inbox.typed().is_empty());
    }

    #[test]
    fn a_message_to_a_busy_session_is_queued_and_typed_only_once_it_is_idle() {
        let hub = FakeHub::new();
        let r = roster3();
        let a = node(&hub, 1, "A", &r);
        let b = node(&hub, 2, "B", &r);
        b.inbox
            .add(4, "修登录白屏", "/w/dc-terminal", Working, true);

        // 不在会话里发：发件方写「终端」。
        let out = send(&a.mesh, &a.net, "B/修登录白屏", "有空看一下", None);
        assert_eq!(out, Ok(SendOutcome::Queued));
        assert!(b.inbox.typed().is_empty());

        tick(&b.mesh);
        assert!(b.inbox.typed().is_empty(), "还在忙");

        b.inbox.set_state(4, Idle);
        tick(&b.mesh);
        assert_eq!(
            b.inbox.typed(),
            [(4, marker("A", "终端", &hex_id(1), "有空看一下"))]
        );
    }

    #[test]
    fn sending_to_an_offline_or_unknown_machine_says_so() {
        let hub = FakeHub::new();
        let r = roster3();
        let a = node(&hub, 1, "A", &r);
        let b = node(&hub, 2, "B", &r);
        b.inbox.add(4, "", "/w/x", Idle, true);
        hub.set_online(&b.ep, false);
        assert_eq!(
            send(&a.mesh, &a.net, "B/#4", "hi", None),
            Ok(SendOutcome::Offline)
        );
        assert_eq!(
            send(&a.mesh, &a.net, "Z/#4", "hi", None),
            Ok(SendOutcome::NoSuchMachine)
        );
        assert_eq!(
            send(&a.mesh, &a.net, "B", "hi", None),
            Err(MeshProblem::BadAddress("B".into()))
        );
        assert_eq!(
            send(&a.mesh, &a.net, "B/#4", &"x".repeat(8001), None),
            Err(MeshProblem::TooLong)
        );
        assert!(b.inbox.typed().is_empty());
    }

    /// 发给这台电脑自己的会话：不过中转，照样包标记、照样排队规矩。
    #[test]
    fn a_message_to_this_computer_goes_straight_in() {
        let hub = FakeHub::new();
        let r = roster3();
        let a = node(&hub, 1, "A", &r);
        a.inbox.add(2, "", "/a/proj", Idle, true);
        hub.set_online(&a.ep, false);
        assert_eq!(
            send(&a.mesh, &a.net, "A/proj", "自己跟自己说", None),
            Ok(SendOutcome::Delivered)
        );
        assert_eq!(
            a.inbox.typed()[0].1,
            marker("A", "终端", &hex_id(1), "自己跟自己说")
        );
    }

    /// 回执必须是收件那台签的、对得上我发的 id。别的一律当没回。
    #[test]
    fn a_receipt_that_does_not_match_the_message_is_no_answer() {
        struct Liar {
            answer: Vec<u8>,
        }
        impl Net for Liar {
            fn peers(&self) -> Result<Vec<String>, LinkError> {
                Ok(vec![])
            }
            fn send(&self, _: &str, _: Vec<u8>) -> Result<(), LinkError> {
                Ok(())
            }
            fn ask(&self, _: &str, _: Vec<u8>, _: Duration) -> Result<Vec<u8>, LinkError> {
                Ok(self.answer.clone())
            }
        }
        let r = roster3();
        let a = Mutex::new(Mesh::new(keys(1), "A".into(), Some(r.clone())));
        let b = Mesh::new(keys(2), "B".into(), Some(r.clone()));
        let a_ep = a.lock().unwrap().endpoint().to_string();
        let a_kx = keys(1).kx_pub();
        // B 签的「已送达」，但 id 不是 A 这次发的。
        let wrong = Message {
            id: "not-yours".into(),
            kind: Kind::Receipt,
            from: b.endpoint().into(),
            from_session: String::new(),
            to: a_ep.clone(),
            to_session: String::new(),
            body: serde_json::to_string(&Receipt::Delivered).unwrap(),
            sent_at: 0,
        };
        let answer = wire::encode(&Payload::Sealed(seal::seal(
            &wrong, &b.keys, &a_kx, [4; 32],
        )));
        assert_eq!(
            send(&a, &Liar { answer }, "B/#1", "hi", None),
            Ok(SendOutcome::NoAnswer)
        );
        // C 签的、id 对得上也不行：问的是 B。
        let c = Mesh::new(keys(3), "C".into(), Some(r));
        let a2 =
            Mutex::new(Mesh::new(keys(1), "A".into(), c.roster.clone()).with_rand(counting_rand()));
        let mut forged = wrong.clone();
        forged.id = hex_id(1);
        forged.from = c.endpoint().into();
        let answer = wire::encode(&Payload::Sealed(seal::seal(
            &forged, &c.keys, &a_kx, [4; 32],
        )));
        assert_eq!(
            send(&a2, &Liar { answer }, "B/#1", "hi", None),
            Ok(SendOutcome::NoAnswer)
        );
    }

    /// 中转说对方收件箱满（`Busy`，这一封没收下）：换一个**新 id** 再发一次。
    /// 同一个 id 再发，要是第一封其实到了，第二封会被对方的去重悄悄吞掉。
    #[test]
    fn a_busy_relay_is_retried_once_with_a_fresh_id() {
        struct BusyOnce {
            inner: FakeNet,
            calls: Mutex<Vec<Vec<u8>>>,
        }
        impl Net for BusyOnce {
            fn peers(&self) -> Result<Vec<String>, LinkError> {
                self.inner.peers()
            }
            fn send(&self, to: &str, p: Vec<u8>) -> Result<(), LinkError> {
                self.inner.send(to, p)
            }
            fn ask(&self, to: &str, p: Vec<u8>, t: Duration) -> Result<Vec<u8>, LinkError> {
                let first = {
                    let mut c = self.calls.lock().unwrap();
                    c.push(p.clone());
                    c.len() == 1
                };
                if first {
                    return Err(LinkError::Relay(dct_link::LinkError::Busy));
                }
                self.inner.ask(to, p, t)
            }
        }
        let hub = FakeHub::new();
        let r = roster3();
        let a = node(&hub, 1, "A", &r);
        let b = node(&hub, 2, "B", &r);
        b.inbox.add(4, "", "/w/x", Idle, true);
        let net = BusyOnce {
            inner: hub.net_for(&a.ep),
            calls: Mutex::new(vec![]),
        };
        assert_eq!(
            send(&a.mesh, &net, "B/#4", "hi", None),
            Ok(SendOutcome::Delivered)
        );
        let calls = net.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        let ids: Vec<String> = calls
            .iter()
            .map(|p| {
                let Ok(Payload::Sealed(s)) = wire::decode(p) else {
                    panic!()
                };
                let bm = b.mesh.lock().unwrap();
                seal::open(&s, &bm.keys, &r.roster, &a.ep).unwrap().id
            })
            .collect();
        assert_ne!(ids[0], ids[1], "重发必须换 id");
        assert_eq!(b.inbox.typed().len(), 1);
        assert!(b.inbox.typed()[0].1.contains(&format!("#{}", &ids[1][..4])));
    }

    #[test]
    fn peers_shows_each_machine_with_its_sessions_and_tentacles() {
        let hub = FakeHub::new();
        let r = roster3();
        let a = node(&hub, 1, "A", &r);
        let b = {
            let inbox = Arc::new(FakeInbox {
                tentacles: vec!["iPhone 镜像".into(), "摄像头".into()],
                ..Default::default()
            });
            let mesh = Arc::new(Mutex::new(
                Mesh::new(keys(2), "B".into(), Some(r.clone())).with_inbox(inbox.clone()),
            ));
            hub.register(mesh.clone());
            inbox
        };
        // C 在名单上，但没连上中转。
        a.inbox.add(1, "写文档", "/a/docs", Working, true);
        b.add(4, "", "/w/dc-terminal", Idle, true);
        b.add(5, "修登录白屏", "/w/dc_classroom", Asking, true);

        let list = peers(&a.mesh, &a.net).unwrap();
        let brief = |n: &str, s: &str, d: &str| SessionBrief {
            name: n.into(),
            state: s.into(),
            dir: d.into(),
        };
        assert_eq!(
            list,
            [
                PeerView {
                    name: "A".into(),
                    online: true,
                    os: std::env::consts::OS.into(),
                    sessions: vec![brief("写文档", "忙", "/a/docs")],
                    tentacles: vec![],
                },
                PeerView {
                    name: "B".into(),
                    online: true,
                    os: std::env::consts::OS.into(),
                    sessions: vec![
                        brief("claude", "闲", "/w/dc-terminal"),
                        brief("修登录白屏", "等你回答", "/w/dc_classroom"),
                    ],
                    tentacles: vec!["iPhone 镜像".into(), "摄像头".into()],
                },
                PeerView {
                    name: "C".into(),
                    online: false,
                    os: String::new(),
                    sessions: vec![],
                    tentacles: vec![],
                },
            ]
        );
    }

    /// 在线、但回不出一份合法状态的：当不在线，只有名字。
    #[test]
    fn a_machine_that_does_not_answer_the_status_request_shows_as_offline() {
        let hub = FakeHub::new();
        let r = roster3();
        let a = node(&hub, 1, "A", &r);
        // B 登记在中转上，但它手上的名单里没有 A：A 的问话它验不过，不回。
        let lone = roster::genesis(member(&keys(2), "B"), "other".into(), &keys(2));
        let b = Arc::new(Mutex::new(Mesh::new(keys(2), "B".into(), Some(lone))));
        hub.register(b);
        let list = peers(&a.mesh, &a.net).unwrap();
        assert!(!list[1].online && list[1].os.is_empty());
    }

    #[test]
    fn not_logged_in_is_an_error() {
        let hub = FakeHub::new();
        let m = Arc::new(Mutex::new(Mesh::new(keys(1), "A".into(), None)));
        hub.register(m.clone());
        let net = hub.net_for(m.lock().unwrap().endpoint());
        assert_eq!(peers(&m, &net), Err(MeshProblem::NotLoggedIn));
        assert_eq!(
            send(&m, &net, "B/x", "hi", None),
            Err(MeshProblem::NotLoggedIn)
        );
    }
}
