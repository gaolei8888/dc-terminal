//! 远程操作的被操作一方：收下组里别的电脑发来的 `Kind::RemoteCall`，排队、
//! 执行、答复。设计见 `docs/superpowers/specs/2026-11-19-dct-remote-control-design.md`
//! 第 1、3 节。
//!
//! 一条远程调用走三段：
//! 1. **收件线程、攥着 `Mesh` 锁**（`Mesh::take_remote_call`）：名单、签名、
//!    时间窗、信封去重在 `take_sealed` 里已经过了；这里看开关。关着当场答
//!    `RemoteControlOff`；开着把调用带出锁外。
//! 2. **收件线程、锁外**（`RemoteHost::submit`）：按 `seq` 去重、按发件电脑
//!    排进它自己的队列，立刻返回——收件线程（`link::Link::run`，一次处理一封）
//!    不能被一次几秒钟的 `Create` 堵住，后面的留言、状态查询都排在这条线上。
//! 3. **那台操作方电脑自己的工作线程**：执行（`Inbox::remote_call`，授权判定在
//!    那里面）、记下答复、封好经 `Net::reply` 答回那次 `ask`。
use std::collections::{HashMap, VecDeque};
use std::mem::discriminant;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dct_link::{Envelope, MAX_PAYLOAD};
use dct_mesh::seal::{Kind, Message};
use serde::Serialize;

use super::deliver::Inbox;
use super::net::Net;
use super::remote::{self, CallBody, Capability, ReplyBody};
use super::{Mesh, Step};
use crate::git::FileStat;
use crate::proto::{ErrorCode, Request, Response};

/// 多久之内有过「真在用」的请求，就算对方正在远程操作这台电脑（设计第 3 节）。
pub const PRESENCE_WINDOW: Duration = Duration::from_secs(10);

/// 每台操作方电脑记最近多少条 `(seq, 答复)`。重试是超时之后马上来的，
/// 几十条足够盖住；再旧的 `seq` 回「不可用」，不再执行。
pub const ANSWER_CACHE: usize = 64;

/// 工作线程闲多久就退出（下一条调用来了再起）。
const IDLE_EXIT: Duration = Duration::from_secs(30);

/// `Diff` 太长装不进一个信封时，截下来的列表最后补的那一行。
pub const TRUNCATED: &str = "（太长，已截断）";

/// 把一条答复 body 封成发给 `to_what` 发件人的 `RemoteReply` 信封 payload。
/// 封不出来（对方已经不在名单里）是 `None`。
pub type SealFn = Arc<dyn Fn(&Message, String) -> Option<Vec<u8>> + Send + Sync>;

/// 一条验过、开关也开着的远程调用。在 `Mesh` 锁里造好，锁外交给 `submit`。
pub struct Call {
    /// 对方那封信封的 `seq`。答复的信封带同一个 `seq`，中转靠它把答复配回
    /// 那次 `ask`——跟 `link::Link::reply_to` 同一条规矩。
    pub env_seq: u64,
    /// 解开的那条消息：回信要用它的 `id` 和会话（`Mesh::reply`）。
    pub msg: Message,
    /// 发件电脑在名单上的名字（「谁在操作」显示用）。
    pub from_name: String,
    /// 这台电脑自己的名字：`RemoteUnavailable { peer }` 里说「连不上谁」。
    pub me_name: String,
    pub body: CallBody,
}

/// 一封等着答复的信封。同一个 `seq` 可能有好几封（超时重试用新信封），
/// 每一封都得答——不知道对方还在等哪一封。
struct Waiter {
    env_seq: u64,
    msg: Message,
}

struct Job {
    seq: u64,
    req: Request,
    waiters: Vec<Waiter>,
    me_name: String,
}

/// 一台操作方电脑在这里的全部账。
#[derive(Default)]
struct Controller {
    /// 排着的调用。动手和管理类的排在看的前面（`accept`）。
    queue: VecDeque<Job>,
    /// 已经有答案、只差发出去的（重复的 `seq`、被顶掉的取画面……）。工作
    /// 线程先发这些：发信要打网络，不在收件线程上做。
    answers: VecDeque<(Waiter, String)>,
    /// 正在执行的那条的 `seq`，和它执行期间又来的同 `seq` 信封。
    running: Option<(u64, Vec<Waiter>)>,
    /// 最近执行完的 `(seq, 答复 body)`，最多 `ANSWER_CACHE` 条。
    done: VecDeque<(u64, String)>,
    /// 从 `done` 里挤出去过的最大 `seq`。不在 `done` 里、又不大于它的
    /// `seq`，要么执行过、答复已经忘了，要么旧得离谱——一律不再执行。
    ///
    /// 没用「执行到的最大 `seq`」：操作方的几条后台线程（画面、按键、
    /// 会话列表）共用一条 `seq`，并发发出，到达顺序可以跟编号顺序不一样；
    /// 按最大值判，晚到一步的按键会被当成「太旧」丢掉，正是反目标里的
    /// 「按键丢失」。只有真忘掉了的才回「太旧」。
    forgotten: Option<u64>,
    /// 这台电脑的工作线程在不在。
    has_worker: bool,
}

impl Controller {
    /// 收下一条调用：去重、排队。只动内存，不打网络。
    fn accept(&mut self, seq: u64, req: Request, w: Waiter, me_name: String) {
        if let Some((_, body)) = self.done.iter().find(|(s, _)| *s == seq) {
            // 执行过：原样回上次的答复，不再执行（设计第 1 节「不丢、不重」）。
            self.answers.push_back((w, body.clone()));
            return;
        }
        if let Some((s, ws)) = &mut self.running {
            if *s == seq {
                ws.push(w);
                return;
            }
        }
        if let Some(job) = self.queue.iter_mut().find(|j| j.seq == seq) {
            job.waiters.push(w);
            return;
        }
        if self.forgotten.is_some_and(|f| seq <= f) {
            self.answers.push_back((w, unavailable(seq, &me_name)));
            return;
        }
        let job = Job {
            seq,
            req,
            waiters: vec![w],
            me_name,
        };
        if is_view(&job.req) {
            // 积压时同一种「看」只留最新一条：旧画面没人要了。被顶掉的当场
            // 答「不可用」，对方的后台线程不用干等到超时，下一轮照取。
            // 只顶同一种（`Screens` 顶 `Screens`）：顶掉 `List` 会让对方把
            // 这台电脑标成离线。
            let kind = discriminant(&job.req);
            let mut kept = VecDeque::with_capacity(self.queue.len() + 1);
            for j in self.queue.drain(..) {
                if is_view(&j.req) && discriminant(&j.req) == kind {
                    for w in j.waiters {
                        self.answers.push_back((w, unavailable(j.seq, &j.me_name)));
                    }
                } else {
                    kept.push_back(j);
                }
            }
            self.queue = kept;
            self.queue.push_back(job);
        } else {
            // 按键排在所有「看」的前面、其它按键的后面：按键之间保序，
            // 只是不等画面。
            let at = self
                .queue
                .iter()
                .position(|j| is_view(&j.req))
                .unwrap_or(self.queue.len());
            self.queue.insert(at, job);
        }
    }

    fn remember(&mut self, seq: u64, body: String) {
        self.done.push_back((seq, body));
        while self.done.len() > ANSWER_CACHE {
            if let Some((s, _)) = self.done.pop_front() {
                self.forgotten = Some(self.forgotten.map_or(s, |f| f.max(s)));
            }
        }
    }
}

fn is_view(req: &Request) -> bool {
    remote::capability_of(req) == Some(Capability::View)
}

#[derive(Default)]
struct State {
    controllers: HashMap<String, Controller>,
    /// 按发件 endpoint：(名单上的名字, 最近一次「真在用」的时刻)。
    presence: HashMap<String, (String, Instant)>,
}

struct Shared {
    inbox: Arc<dyn Inbox>,
    net: Arc<dyn Net>,
    seal: SealFn,
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 封好、发出去。发不出去就算了：对方会超时、用同一个 `seq` 重试，
    /// 那时候答复已经在 `done` 里了。
    fn send(&self, w: &Waiter, body: &str) {
        if let Some(p) = (self.seal)(&w.msg, body.to_string()) {
            let _ = self.net.reply(&w.msg.from, w.env_seq, p);
        }
    }

    fn fits(&self, probe: &Message, body: &str) -> bool {
        (self.seal)(probe, body.to_string()).is_some_and(|p| p.len() <= MAX_PAYLOAD)
    }
}

/// 守护进程里那一个。放在 `Mesh` 上（`attach`），收件路径从那里拿。
pub struct RemoteHost {
    shared: Arc<Shared>,
}

impl RemoteHost {
    pub fn new(inbox: Arc<dyn Inbox>, net: Arc<dyn Net>, seal: SealFn) -> RemoteHost {
        RemoteHost {
            shared: Arc::new(Shared {
                inbox,
                net,
                seal,
                state: Mutex::new(State::default()),
                wake: Condvar::new(),
            }),
        }
    }

    /// 收下一条调用，立刻返回（不执行、不打网络）。
    pub fn submit(&self, call: Call) {
        let Call {
            env_seq,
            msg,
            from_name,
            me_name,
            body: CallBody { seq, req },
        } = call;
        let from = msg.from.clone();
        let spawn = {
            let mut st = self.shared.state();
            if remote::counts_as_presence(&req) {
                st.presence
                    .insert(from.clone(), (from_name, Instant::now()));
            }
            let c = st.controllers.entry(from.clone()).or_default();
            c.accept(seq, req, Waiter { env_seq, msg }, me_name);
            !std::mem::replace(&mut c.has_worker, true)
        };
        self.shared.wake.notify_all();
        if spawn {
            let shared = self.shared.clone();
            let who = from.clone();
            let started = std::thread::Builder::new()
                .name(format!("dct-remote-{from}"))
                .spawn(move || work(shared, who));
            if started.is_err() {
                // 线程起不来：标回「没有工作线程」，下一条调用再试着起。排着
                // 的那几条对方会超时、用同一个 `seq` 重试。
                if let Some(c) = self.shared.state().controllers.get_mut(&from) {
                    c.has_worker = false;
                }
            }
        }
    }

    /// 此刻（`now` 往前 `PRESENCE_WINDOW` 之内）真在用这台电脑的电脑名，排好序。
    pub fn viewers(&self, now: Instant) -> Vec<String> {
        let st = self.shared.state();
        let mut v: Vec<String> = st
            .presence
            .values()
            .filter(|(_, t)| now.saturating_duration_since(*t) < PRESENCE_WINDOW)
            .map(|(n, _)| n.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// 踢掉所有人：清在场、清队列（调用方同时把开关关上）。排着没执行的
    /// 当场答 `RemoteControlOff`，对方不用干等；正在执行的那一条拦不住，
    /// 照常答完。
    pub fn kick_all(&self) {
        {
            let mut st = self.shared.state();
            st.presence.clear();
            for c in st.controllers.values_mut() {
                for j in c.queue.drain(..) {
                    let body = encode(j.seq, &Response::Error(ErrorCode::RemoteControlOff));
                    for w in j.waiters {
                        c.answers.push_back((w, body.clone()));
                    }
                }
            }
        }
        self.shared.wake.notify_all();
    }
}

/// 把 `RemoteHost` 接到 `mesh` 上：答复用这台 `Mesh` 的钥匙封（只借一下锁），
/// 经 `net` 发出去。`Mesh` 里存的是它，它这边只拿 `Weak`，不成环。
pub fn attach(
    mesh: &Arc<Mutex<Mesh>>,
    inbox: Arc<dyn Inbox>,
    net: Arc<dyn Net>,
) -> Arc<RemoteHost> {
    let weak = Arc::downgrade(mesh);
    let seal: SealFn = Arc::new(move |to_what: &Message, body: String| {
        let mesh = weak.upgrade()?;
        let m = mesh.lock().unwrap_or_else(|e| e.into_inner());
        m.reply(to_what, Kind::RemoteReply, body)
    });
    let host = Arc::new(RemoteHost::new(inbox, net, seal));
    mesh.lock().unwrap_or_else(|e| e.into_inner()).remote = Some(host.clone());
    host
}

enum Next {
    Send(Waiter, String),
    Run(Job),
}

/// 一台操作方电脑的工作线程：先发现成的答复，再按队列一条一条执行。
fn work(shared: Arc<Shared>, from: String) {
    loop {
        let next = {
            let mut st = shared.state();
            loop {
                let c = st.controllers.entry(from.clone()).or_default();
                if let Some((w, body)) = c.answers.pop_front() {
                    break Next::Send(w, body);
                }
                if let Some(job) = c.queue.pop_front() {
                    c.running = Some((job.seq, Vec::new()));
                    break Next::Run(job);
                }
                let (g, t) = shared
                    .wake
                    .wait_timeout(st, IDLE_EXIT)
                    .unwrap_or_else(|e| e.into_inner());
                st = g;
                if t.timed_out() {
                    let c = st.controllers.entry(from.clone()).or_default();
                    if c.answers.is_empty() && c.queue.is_empty() {
                        c.has_worker = false;
                        return;
                    }
                }
            }
        };
        match next {
            Next::Send(w, body) => shared.send(&w, &body),
            Next::Run(Job {
                seq,
                req,
                waiters,
                me_name,
            }) => {
                // 执行半路 panic 不能把这条线带走：`has_worker` 还标着，线一死
                // 这台操作方电脑就再也没人服务了。
                let resp = catch_unwind(AssertUnwindSafe(|| shared.inbox.remote_call(&from, req)))
                    .unwrap_or_else(|_| {
                        Response::Error(ErrorCode::RemoteUnavailable {
                            peer: me_name.clone(),
                        })
                    });
                let probe = &waiters[0].msg;
                let body = fit(seq, resp, &me_name, |b| shared.fits(probe, b));
                let late = {
                    let mut st = shared.state();
                    let c = st.controllers.entry(from.clone()).or_default();
                    let late = c.running.take().map(|(_, ws)| ws).unwrap_or_default();
                    c.remember(seq, body.clone());
                    late
                };
                for w in waiters.iter().chain(late.iter()) {
                    shared.send(w, &body);
                }
            }
        }
    }
}

/// `ReplyBody` 的借用版：跟它序列化出来一模一样，不用为了编码克隆答复。
#[derive(Serialize)]
struct ReplyRef<'a> {
    seq: u64,
    resp: &'a Response,
}

fn encode(seq: u64, resp: &Response) -> String {
    serde_json::to_string(&ReplyRef { seq, resp }).unwrap_or_default()
}

fn unavailable(seq: u64, me_name: &str) -> String {
    encode(
        seq,
        &Response::Error(ErrorCode::RemoteUnavailable {
            peer: me_name.to_string(),
        }),
    )
}

/// 答复编成 body；封出来超过 `MAX_PAYLOAD`（中转收不下）就缩：
/// - `Diff`：留能装下的前几个文件，最后补一行 `TRUNCATED`；
/// - `Screens`：只回能装下的前几格，其余的不回——这个答复的形状没有
///   「这一格不可用」的位置，少了的格子对方当作这一轮没取到；
/// - 别的、或者缩到最小还装不下：答 `RemoteUnavailable`。不静默失败。
fn fit(seq: u64, resp: Response, me_name: &str, fits: impl Fn(&str) -> bool) -> String {
    let whole = encode(seq, &resp);
    if fits(&whole) {
        return whole;
    }
    let trimmed = match resp {
        Response::Diff(files) => {
            let cut = |k: usize| {
                let mut v: Vec<FileStat> = files[..k].to_vec();
                v.push(FileStat {
                    path: TRUNCATED.into(),
                    added: 0,
                    removed: 0,
                });
                encode(seq, &Response::Diff(v))
            };
            longest_prefix(files.len(), |k| fits(&cut(k))).map(cut)
        }
        Response::Screens { screens } => {
            let cut = |k: usize| {
                encode(
                    seq,
                    &Response::Screens {
                        screens: screens[..k].to_vec(),
                    },
                )
            };
            longest_prefix(screens.len(), |k| fits(&cut(k))).map(cut)
        }
        _ => None,
    };
    trimmed.unwrap_or_else(|| unavailable(seq, me_name))
}

/// `0..n` 里让 `ok` 成立的最大 `k`（`ok` 对前缀单调：`k` 装得下，更短的也
/// 装得下）；连 `0` 都不行就是 `None`。二分：每试一次要封一遍。
fn longest_prefix(n: usize, ok: impl Fn(usize) -> bool) -> Option<usize> {
    if !ok(0) {
        return None;
    }
    let (mut lo, mut hi) = (0, n); // ok(lo) 成立；答案在 [lo, hi)
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if ok(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    // n 本身一定不行（整份装不下才走到这里），所以答案 < n。
    Some(lo)
}

impl Mesh {
    /// 谁在远程操作这台电脑、踢人，都从这里拿（没接上就是 `None`）。
    pub fn remote_host(&self) -> Option<Arc<RemoteHost>> {
        self.remote.clone()
    }

    /// `take_sealed` 里名单、签名、时间窗、信封去重都过了之后，攥着锁做的
    /// 那一段：看开关、决定当场答还是带出锁外。
    pub(super) fn take_remote_call(&mut self, env: &Envelope, m: &Message) -> Step {
        // 没有接上执行的那一头，开关开着也等于关着。
        let on = self.remote.is_some() && self.store.as_ref().is_some_and(|s| s.remote_control());
        let body: CallBody = match serde_json::from_str(&m.body) {
            Ok(b) => b,
            Err(_) => {
                // 读得出 `seq`、读不出请求：多半是对方版本新、多了一种这边
                // 不认识的请求——那它也不在这边的白名单里。当场答，省得对方
                // 干等几轮超时。
                let seq = serde_json::from_str::<serde_json::Value>(&m.body)
                    .ok()
                    .and_then(|v| v.get("seq")?.as_u64());
                let Some(seq) = seq else {
                    return Step::Done(self.drop(env, "remote_call_unreadable"));
                };
                let code = if on {
                    ErrorCode::RemoteForbidden
                } else {
                    ErrorCode::RemoteControlOff
                };
                return Step::Done(self.answer_remote(m, seq, Response::Error(code)));
            }
        };
        if !on {
            // 开关关着：一条也不执行，当场答。
            return Step::Done(self.answer_remote(
                m,
                body.seq,
                Response::Error(ErrorCode::RemoteControlOff),
            ));
        }
        let from_name = self
            .roster
            .as_ref()
            .and_then(|r| r.roster.member(&m.from))
            .map(|x| x.name.clone())
            .unwrap_or_else(|| m.from.clone());
        Step::Remote(Call {
            env_seq: env.seq,
            msg: m.clone(),
            from_name,
            me_name: self.me.name.clone(),
            body,
        })
    }

    fn answer_remote(&self, m: &Message, seq: u64, resp: Response) -> Option<Vec<u8>> {
        let body = serde_json::to_string(&ReplyBody { seq, resp }).unwrap_or_default();
        self.reply(m, Kind::RemoteReply, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Journal;
    use crate::link::LinkError;
    use crate::mesh::deliver::testing::FakeInbox;
    use crate::mesh::net::testing::FakeHub;
    use crate::mesh::store::Store;
    use crate::proto::ScreenEntry;
    use crate::session::SessionState;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use dct_mesh::roster::{self, Member};
    use dct_mesh::seal;
    use dct_mesh::wire::{self, Payload};
    use dct_mesh::{id, MachineKeys};
    use std::sync::atomic::{AtomicBool, Ordering};

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

    fn now() -> u64 {
        super::super::unix_now()
    }

    /// 两台电脑 A（操作方）、B（被操作方）挂在一个 `FakeHub` 上。B 有一个
    /// 空闲的智能体会话 #7，开关按 `switch_on` 落盘，`RemoteHost` 接好。
    struct Two {
        a: Arc<Mutex<Mesh>>,
        b: Arc<Mutex<Mesh>>,
        a_ep: String,
        b_ep: String,
        hub: Arc<FakeHub>,
        inbox: Arc<FakeInbox>,
        host: Arc<RemoteHost>,
        log: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    fn two(switch_on: bool) -> Two {
        let (ka, kb) = (keys(1), keys(2));
        let (ma, mb) = (member(&ka, "A"), member(&kb, "B"));
        let g = roster::genesis(ma.clone(), "mine".into(), &ka);
        let mut r2 = g.roster.clone();
        r2.version = 2;
        r2.members.push(mb);
        let v2 = roster::sign(r2, &ma, &ka);

        let dir = tempfile::tempdir().unwrap();
        let st = Store::at(dir.path().join("mesh"));
        st.set_remote_control(switch_on).unwrap();
        let log = dir.path().join("sessions.log");
        let j = Journal::new();
        j.set_path(log.clone());

        let inbox = FakeInbox::new();
        inbox.add(7, "", "/w/proj", SessionState::Idle, true);
        let a = Mesh::new(ka, "A".into(), Some(v2.clone()));
        let b = Mesh::new(kb, "B".into(), Some(v2))
            .with_store(Store::at(dir.path().join("mesh")))
            .with_journal(Arc::new(j))
            .with_inbox(inbox.clone());
        let (a_ep, b_ep) = (a.endpoint().to_string(), b.endpoint().to_string());
        let (a, b) = (Arc::new(Mutex::new(a)), Arc::new(Mutex::new(b)));
        let hub = FakeHub::new();
        hub.register(a.clone());
        hub.register(b.clone());
        let host = attach(&b, inbox.clone(), Arc::new(hub.net_for(&b_ep)));
        Two {
            a,
            b,
            a_ep,
            b_ep,
            hub,
            inbox,
            host,
            log,
            _dir: dir,
        }
    }

    impl Two {
        /// A 封给 B 的一条消息（整个信封的 payload）。
        fn sealed(&self, kind: Kind, id: &str, to_session: &str, body: String) -> Vec<u8> {
            let a = self.a.lock().unwrap();
            let b_kx = self.b.lock().unwrap().keys.kx_pub();
            let m = Message {
                id: id.into(),
                kind,
                from: self.a_ep.clone(),
                from_session: String::new(),
                to: self.b_ep.clone(),
                to_session: to_session.into(),
                body,
                sent_at: now(),
            };
            wire::encode(&Payload::Sealed(seal::seal(
                &m,
                &a.keys,
                &b_kx,
                crate::mesh::os_rand(),
            )))
        }

        fn call(&self, seq: u64, req: Request, id: &str) -> Vec<u8> {
            let body = serde_json::to_string(&CallBody { seq, req }).unwrap();
            self.sealed(Kind::RemoteCall, id, "", body)
        }

        /// A 问 B，拿回解开的 `RemoteReply` body（原文）。
        fn ask(&self, payload: Vec<u8>, timeout: Duration) -> Result<String, LinkError> {
            let r = self
                .hub
                .net_for(&self.a_ep)
                .ask(&self.b_ep, payload, timeout)?;
            let Payload::Sealed(s) = wire::decode(&r).unwrap() else {
                panic!("回的该是密封的");
            };
            let a = self.a.lock().unwrap();
            let m =
                seal::open(&s, &a.keys, &a.roster.as_ref().unwrap().roster, &self.b_ep).unwrap();
            assert_eq!(m.kind, Kind::RemoteReply);
            Ok(m.body)
        }

        fn remote(&self, seq: u64, req: Request, id: &str) -> ReplyBody {
            let body = self
                .ask(self.call(seq, req, id), Duration::from_secs(5))
                .unwrap();
            serde_json::from_str(&body).unwrap()
        }
    }

    fn create() -> Request {
        Request::Create {
            dir: "/w/proj".into(),
            profile: "claude".into(),
            remember: false,
        }
    }

    #[test]
    fn a_remote_list_comes_back_when_the_switch_is_on() {
        let t = two(true);
        let r = t.remote(1, Request::List, "c1");
        assert_eq!(r.seq, 1);
        match r.resp {
            Response::Sessions(s) => assert_eq!(s.iter().map(|s| s.id).collect::<Vec<_>>(), [7]),
            other => panic!("该回会话列表：{other:?}"),
        }
        assert_eq!(t.inbox.remote_calls(), [r#""List""#]);
        assert_eq!(
            t.inbox.remote_calls.lock().unwrap()[0].0,
            t.a_ep,
            "执行时带着发件电脑"
        );
    }

    /// Review Focus 1：开关关着，一条也不执行，当场答 `RemoteControlOff`。
    #[test]
    fn switch_off_answers_remote_control_off_and_runs_nothing() {
        let t = two(false);
        // 当场答：B 的 `on_envelope` 直接回了，不经工作线程。
        let e = Envelope {
            from: dct_link::EndpointId::new(t.a_ep.clone()).unwrap(),
            to: dct_link::EndpointId::new(t.b_ep.clone()).unwrap(),
            seq: 9,
            payload: t.call(3, create(), "c1"),
            recipients: vec![],
        };
        assert!(crate::mesh::handle(&t.b, &e).is_some(), "关着也要当场答");
        let r = t.remote(4, Request::List, "c2");
        assert_eq!(r.seq, 4);
        assert!(
            matches!(r.resp, Response::Error(ErrorCode::RemoteControlOff)),
            "{:?}",
            r.resp
        );
        assert!(t.inbox.remote_calls().is_empty(), "一条都不该执行");
        assert!(t.host.viewers(Instant::now()).is_empty(), "关着不算在场");
    }

    /// Review Focus 4：超时重试用新信封、同一个 `seq`——只建一个会话，两次答复一样。
    #[test]
    fn the_same_seq_twice_runs_once_and_answers_the_same() {
        let t = two(true);
        let first = t
            .ask(t.call(5, create(), "c1"), Duration::from_secs(5))
            .unwrap();
        let again = t
            .ask(t.call(5, create(), "c2"), Duration::from_secs(5))
            .unwrap();
        assert_eq!(first, again);
        assert!(first.contains("Created"), "{first}");
        assert_eq!(t.inbox.remote_calls().len(), 1, "Create 只该执行一次");

        // 第一次还在执行时重试就到了：也只执行一次，两封都答同一个答复。
        let gate = Arc::new(AtomicBool::new(true));
        let g = gate.clone();
        *t.inbox.on_remote.lock().unwrap() = Some(Arc::new(move |_: &Request| {
            while g.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }));
        let t = Arc::new(t);
        let t1 = t.clone();
        let p1 = t.call(6, create(), "c3");
        let h1 = std::thread::spawn(move || t1.ask(p1, Duration::from_secs(5)).unwrap());
        wait_until(|| t.inbox.remote_calls().len() == 1 && t.host_running(6));
        let t2 = t.clone();
        let p2 = t.call(6, create(), "c4");
        let h2 = std::thread::spawn(move || t2.ask(p2, Duration::from_secs(5)).unwrap());
        std::thread::sleep(Duration::from_millis(100));
        gate.store(false, Ordering::SeqCst);
        let (a, b) = (h1.join().unwrap(), h2.join().unwrap());
        assert_eq!(a, b);
        assert_eq!(t.inbox.remote_calls().len(), 2, "seq 6 也只执行一次");
    }

    impl Two {
        /// 工作线程正在执行 `seq` 那一条。
        fn host_running(&self, seq: u64) -> bool {
            self.host
                .shared
                .state()
                .controllers
                .values()
                .any(|c| c.running.as_ref().is_some_and(|(s, _)| *s == seq))
        }
    }

    fn wait_until(f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < deadline, "等了 5 秒还没到");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Review Focus 2：远程调用执行要 3 秒，同时到的一条留言照样马上敲进去——
    /// 哪怕这台电脑只有一条收件线（`one_link_thread`）。
    #[test]
    fn a_slow_remote_call_does_not_hold_up_a_message() {
        let t = two(true);
        t.hub.one_link_thread();
        *t.inbox.on_remote.lock().unwrap() = Some(Arc::new(|_: &Request| {
            std::thread::sleep(Duration::from_secs(3));
        }));
        let t = Arc::new(t);
        let tr = t.clone();
        let p = t.call(1, Request::List, "c1");
        let slow = std::thread::spawn(move || tr.ask(p, Duration::from_secs(6)));
        wait_until(|| t.host_running(1));

        let started = Instant::now();
        let msg = t.sealed(Kind::Msg, "m1", "#7", "hi".into());
        t.hub
            .net_for(&t.a_ep)
            .ask(&t.b_ep, msg, Duration::from_secs(5))
            .expect("留言该当场回回执");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "留言等了 {:?}",
            started.elapsed()
        );
        assert_eq!(t.inbox.typed().len(), 1, "留言该已经敲进去了");
        assert!(t.inbox.remote_calls().is_empty(), "远程调用这时还没执行完");

        let body = slow.join().unwrap().expect("慢的那条做完也该答回去");
        assert!(body.contains("Sessions"), "{body}");
    }

    /// Review Focus 3：同一封信封原样再投——远程那张去重表挡掉，不进留言那张。
    #[test]
    fn a_replayed_envelope_is_dropped() {
        let t = two(true);
        let e = Envelope {
            from: dct_link::EndpointId::new(t.a_ep.clone()).unwrap(),
            to: dct_link::EndpointId::new(t.b_ep.clone()).unwrap(),
            seq: 1,
            payload: t.call(1, Request::List, "c1"),
            recipients: vec![],
        };
        assert_eq!(
            crate::mesh::handle(&t.b, &e),
            None,
            "开着：不当场答，交给工作线程"
        );
        wait_until(|| t.inbox.remote_calls().len() == 1);
        assert_eq!(crate::mesh::handle(&t.b, &e), None);
        let log = std::fs::read_to_string(&t.log).unwrap();
        assert!(log.contains("why=replay"), "{log}");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(t.inbox.remote_calls().len(), 1);
        let b = t.b.lock().unwrap();
        assert!(b.seen.set.is_empty(), "不该进留言的去重表");
        assert_eq!(b.remote_seen.set.len(), 1);
    }

    /// 从轮询里进来的远程答复（对方超时之后才到的那封）：丢掉、不回。
    #[test]
    fn a_late_remote_reply_is_dropped_without_answer() {
        let t = two(true);
        let body = serde_json::to_string(&ReplyBody {
            seq: 1,
            resp: Response::Ok,
        })
        .unwrap();
        let e = Envelope {
            from: dct_link::EndpointId::new(t.a_ep.clone()).unwrap(),
            to: dct_link::EndpointId::new(t.b_ep.clone()).unwrap(),
            seq: 1,
            payload: t.sealed(Kind::RemoteReply, "r1", "", body),
            recipients: vec![],
        };
        assert_eq!(crate::mesh::handle(&t.b, &e), None);
        assert!(t.inbox.remote_calls().is_empty());
    }

    // ---- 下面直接测 `RemoteHost`：不经网络，答复记在 `Rec` 里。----

    /// 记下每一条 `reply`：`(发给谁, 信封 seq, body)`。封信就是原样转成字节。
    #[derive(Default)]
    struct Rec {
        replies: Mutex<Vec<(String, u64, String)>>,
    }

    impl Net for Rec {
        fn peers(&self) -> Result<Vec<String>, LinkError> {
            Ok(vec![])
        }
        fn send(&self, _: &str, _: Vec<u8>) -> Result<(), LinkError> {
            Ok(())
        }
        fn ask(&self, _: &str, _: Vec<u8>, _: Duration) -> Result<Vec<u8>, LinkError> {
            Err(LinkError::Unreachable)
        }
        fn reply(&self, to: &str, seq: u64, payload: Vec<u8>) -> Result<(), LinkError> {
            self.replies.lock().unwrap().push((
                to.into(),
                seq,
                String::from_utf8(payload).unwrap(),
            ));
            Ok(())
        }
    }

    impl Rec {
        fn wait_for(&self, n: usize) -> Vec<(String, u64, String)> {
            wait_until(|| self.replies.lock().unwrap().len() >= n);
            self.replies.lock().unwrap().clone()
        }

        /// 信封 `env_seq` 收到的答复。
        fn answer(&self, env_seq: u64) -> ReplyBody {
            let r = self.replies.lock().unwrap();
            let (_, _, b) = r.iter().find(|(_, s, _)| *s == env_seq).expect("该答了");
            serde_json::from_str(b).unwrap()
        }
    }

    fn bare() -> (RemoteHost, Arc<FakeInbox>, Arc<Rec>) {
        let inbox = FakeInbox::new();
        let rec = Arc::new(Rec::default());
        let seal: SealFn = Arc::new(|_: &Message, body: String| Some(body.into_bytes()));
        (
            RemoteHost::new(inbox.clone(), rec.clone(), seal),
            inbox,
            rec,
        )
    }

    /// 发件电脑 `c-a`（叫 A）的一条调用；信封 seq 跟调用 seq 取一样，方便对。
    fn call(seq: u64, req: Request) -> Call {
        Call {
            env_seq: seq,
            msg: Message {
                id: format!("m{seq}"),
                kind: Kind::RemoteCall,
                from: "c-a".into(),
                from_session: String::new(),
                to: "c-b".into(),
                to_session: String::new(),
                body: String::new(),
                sent_at: 0,
            },
            from_name: "A".into(),
            me_name: "B".into(),
            body: CallBody { seq, req },
        }
    }

    /// 让工作线程卡在第一条（`Hello`）上，直到 `release` 被调。
    fn hold_first(inbox: &FakeInbox) -> (Arc<AtomicBool>, Arc<AtomicBool>) {
        let (held, release) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let (h, r) = (held.clone(), release.clone());
        *inbox.on_remote.lock().unwrap() = Some(Arc::new(move |req: &Request| {
            if matches!(req, Request::Hello) {
                h.store(true, Ordering::SeqCst);
                while !r.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }));
        (held, release)
    }

    #[test]
    fn keys_jump_ahead_of_queued_screens_and_old_screens_are_dropped() {
        let (host, inbox, rec) = bare();
        let (held, release) = hold_first(&inbox);
        host.submit(call(1, Request::Hello));
        wait_until(|| held.load(Ordering::SeqCst));
        host.submit(call(2, Request::Screens { ids: vec![1] }));
        host.submit(call(3, Request::List));
        host.submit(call(4, Request::Screens { ids: vec![2] }));
        host.submit(call(
            5,
            Request::Input {
                id: 7,
                text: "a".into(),
            },
        ));
        host.submit(call(
            6,
            Request::Key {
                id: 7,
                name: "Enter".into(),
            },
        ));
        release.store(true, Ordering::SeqCst);
        rec.wait_for(6);
        assert_eq!(
            inbox.remote_calls(),
            [
                r#""Hello""#.to_string(),
                r#"{"Input":{"id":7,"text":"a"}}"#.into(),
                r#"{"Key":{"id":7,"name":"Enter"}}"#.into(),
                r#""List""#.into(),
                r#"{"Screens":{"ids":[2]}}"#.into(),
            ],
            "按键先于画面、按键之间保序；旧的那张画面不取了，List 不被画面顶掉"
        );
        let dropped = rec.answer(2);
        assert_eq!(dropped.seq, 2);
        assert!(
            matches!(dropped.resp, Response::Error(ErrorCode::RemoteUnavailable { ref peer }) if peer == "B"),
            "{:?}",
            dropped.resp
        );
        assert!(matches!(rec.answer(4).resp, Response::Screens { .. }));
    }

    #[test]
    fn presence_counts_watching_and_typing_not_listing() {
        let (host, _inbox, rec) = bare();
        host.submit(call(1, Request::List));
        rec.wait_for(1);
        assert!(host.viewers(Instant::now()).is_empty(), "只取列表不算");
        host.submit(call(2, Request::Screen { id: 7 }));
        rec.wait_for(2);
        assert_eq!(host.viewers(Instant::now()), ["A"]);
        assert!(
            host.viewers(Instant::now() + Duration::from_secs(11))
                .is_empty(),
            "10 秒没动静就不算了"
        );
        host.submit(call(
            3,
            Request::Input {
                id: 7,
                text: "x".into(),
            },
        ));
        assert_eq!(host.viewers(Instant::now()), ["A"], "打字也算");
    }

    #[test]
    fn kick_all_clears_viewers() {
        let (host, inbox, rec) = bare();
        let (held, release) = hold_first(&inbox);
        host.submit(call(1, Request::Hello));
        wait_until(|| held.load(Ordering::SeqCst));
        host.submit(call(2, Request::Screen { id: 7 }));
        host.submit(call(
            3,
            Request::Input {
                id: 7,
                text: "x".into(),
            },
        ));
        assert_eq!(host.viewers(Instant::now()), ["A"]);
        host.kick_all();
        assert!(host.viewers(Instant::now()).is_empty());
        release.store(true, Ordering::SeqCst);
        rec.wait_for(3);
        assert_eq!(inbox.remote_calls(), [r#""Hello""#], "排着的不再执行");
        for s in [2, 3] {
            assert!(
                matches!(
                    rec.answer(s).resp,
                    Response::Error(ErrorCode::RemoteControlOff)
                ),
                "排着的当场答「关了」"
            );
        }
    }

    /// 几条后台线程共用一条 `seq`，到达顺序可以跟编号不一样：晚到的小号照常执行。
    /// 只有从缓存里挤出去过的旧号才回「不可用」。
    #[test]
    fn a_seq_out_of_order_still_runs_but_a_forgotten_one_does_not() {
        let (host, inbox, rec) = bare();
        host.submit(call(11, Request::List));
        host.submit(call(
            10,
            Request::Input {
                id: 7,
                text: "k".into(),
            },
        ));
        rec.wait_for(2);
        assert_eq!(inbox.remote_calls().len(), 2, "晚到的 10 号也该执行");

        // 用按键填：「看」在积压时会被同一种的新请求顶掉，不一定每条都执行。
        for s in 100..100 + ANSWER_CACHE as u64 + 1 {
            host.submit(call(
                s,
                Request::Input {
                    id: 7,
                    text: "k".into(),
                },
            ));
        }
        let n = 2 + ANSWER_CACHE + 1;
        rec.wait_for(n);
        let ran = inbox.remote_calls().len();
        // 10、11 早被挤出去了：重试不再执行。
        host.submit(call(
            10,
            Request::Input {
                id: 7,
                text: "k".into(),
            },
        ));
        let r = rec.wait_for(n + 1);
        assert_eq!(inbox.remote_calls().len(), ran, "挤出去的旧号不该再执行");
        let last: ReplyBody = serde_json::from_str(&r[n].2).unwrap();
        assert!(
            matches!(
                last.resp,
                Response::Error(ErrorCode::RemoteUnavailable { .. })
            ),
            "{:?} {:?}",
            last.seq,
            last.resp
        );
        // 还在缓存里的照样回缓存。
        host.submit(call(
            150,
            Request::Input {
                id: 7,
                text: "k".into(),
            },
        ));
        rec.wait_for(n + 2);
        assert_eq!(inbox.remote_calls().len(), ran);
    }

    /// 执行半路 panic：答「不可用」，这条线还活着，下一条照常执行。
    #[test]
    fn a_panicking_call_is_answered_and_the_worker_survives() {
        let (host, inbox, rec) = bare();
        *inbox.on_remote.lock().unwrap() = Some(Arc::new(|req: &Request| {
            if matches!(req, Request::Hello) {
                panic!("boom");
            }
        }));
        host.submit(call(1, Request::Hello));
        host.submit(call(2, Request::List));
        rec.wait_for(2);
        assert!(matches!(
            rec.answer(1).resp,
            Response::Error(ErrorCode::RemoteUnavailable { .. })
        ));
        assert!(matches!(rec.answer(2).resp, Response::Sessions(_)));
    }

    #[test]
    fn a_diff_too_big_for_one_envelope_is_cut_with_a_note() {
        let files: Vec<FileStat> = (0..1000)
            .map(|i| FileStat {
                path: format!("src/file_{i:04}.rs"),
                added: i,
                removed: 0,
            })
            .collect();
        let body = fit(7, Response::Diff(files), "B", |b| b.len() <= 4000);
        assert!(body.len() <= 4000);
        let r: ReplyBody = serde_json::from_str(&body).unwrap();
        assert_eq!(r.seq, 7);
        let Response::Diff(v) = r.resp else {
            panic!("还该是 Diff")
        };
        assert!(v.len() > 10 && v.len() < 1000, "{}", v.len());
        assert_eq!(v.last().unwrap().path, TRUNCATED);
        assert_eq!(v[0].path, "src/file_0000.rs", "留的是前几个");
        // 再多放一个就装不下了：截在最长处。
        assert!(v.len() - 1 < 1000);
    }

    #[test]
    fn screens_too_big_for_one_envelope_keep_the_ones_that_fit() {
        let screens: Vec<ScreenEntry> = (0..9)
            .map(|id| ScreenEntry {
                id,
                lines: vec![vec![crate::pty::ScreenSpan {
                    text: "x".repeat(500),
                    style: Default::default(),
                }]],
            })
            .collect();
        let whole = encode(
            1,
            &Response::Screens {
                screens: screens.clone(),
            },
        );
        let limit = whole.len() / 2;
        let body = fit(1, Response::Screens { screens }, "B", |b| b.len() <= limit);
        assert!(body.len() <= limit);
        let r: ReplyBody = serde_json::from_str(&body).unwrap();
        let Response::Screens { screens } = r.resp else {
            panic!("还该是 Screens")
        };
        let ids: Vec<u32> = screens.iter().map(|s| s.id).collect();
        assert_eq!(ids, (0..ids.len() as u32).collect::<Vec<_>>(), "留前几格");
        assert!((3..9).contains(&ids.len()), "{ids:?}");
    }

    #[test]
    fn an_answer_that_cannot_shrink_is_reported_unavailable_not_dropped() {
        let body = fit(3, Response::Explanation(Some("x".repeat(100))), "B", |b| {
            b.len() <= 60
        });
        let r: ReplyBody = serde_json::from_str(&body).unwrap();
        assert!(matches!(
            r.resp,
            Response::Error(ErrorCode::RemoteUnavailable { ref peer }) if peer == "B"
        ));
    }

    // ---- 操作方：`mesh::call_remote`（A 经中转调 B） ----

    /// 包在 A 的 `FakeNet` 外面：记下每次 `ask` 发出去的 payload；
    /// `lose_first` 设了，第一次 `ask` 照常投给 B（B 照常执行、答复），但
    /// 答复「丢在半路」——A 这边看到的是超时。
    struct Lossy {
        inner: crate::mesh::net::testing::FakeNet,
        lose_first: AtomicBool,
        asked: Mutex<Vec<Vec<u8>>>,
    }

    impl Net for Lossy {
        fn peers(&self) -> Result<Vec<String>, LinkError> {
            self.inner.peers()
        }
        fn send(&self, to: &str, payload: Vec<u8>) -> Result<(), LinkError> {
            self.inner.send(to, payload)
        }
        fn ask(&self, to: &str, payload: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, LinkError> {
            self.asked.lock().unwrap().push(payload.clone());
            let r = self.inner.ask(to, payload, timeout);
            if self.lose_first.swap(false, Ordering::SeqCst) {
                return Err(LinkError::Relay(dct_link::LinkError::NoAnswer));
            }
            r
        }
        fn reply(&self, to: &str, seq: u64, payload: Vec<u8>) -> Result<(), LinkError> {
            self.inner.reply(to, seq, payload)
        }
    }

    impl Two {
        fn lossy(&self, lose_first: bool) -> Lossy {
            Lossy {
                inner: self.hub.net_for(&self.a_ep),
                lose_first: AtomicBool::new(lose_first),
                asked: Mutex::new(Vec::new()),
            }
        }

        /// 用 B 的钥匙拆开 A 发出的一条 `RemoteCall`：(消息 id, body)。
        fn open_call(&self, payload: &[u8]) -> (String, CallBody) {
            let Payload::Sealed(s) = wire::decode(payload).unwrap() else {
                panic!("发出去的该是密封的");
            };
            let b = self.b.lock().unwrap();
            let m = seal::open(&s, &b.keys, &b.roster.as_ref().unwrap().roster, &self.a_ep)
                .unwrap();
            assert_eq!(m.kind, Kind::RemoteCall);
            (m.id, serde_json::from_str(&m.body).unwrap())
        }
    }

    #[test]
    fn remote_call_round_trips_a_screen() {
        let t = two(true);
        let net = t.lossy(false);
        let r = crate::mesh::call_remote(&t.a, &net, "B", Request::Screens { ids: vec![7] });
        match r {
            Response::Screens { screens } => {
                assert_eq!(screens.iter().map(|s| s.id).collect::<Vec<_>>(), [7])
            }
            other => panic!("该原样回对方的画面：{other:?}"),
        }
        assert_eq!(t.inbox.remote_calls(), [r#"{"Screens":{"ids":[7]}}"#]);
        assert_eq!(net.asked.lock().unwrap().len(), 1, "一次就成，不该重试");
        // 对方的错误码原样回来，不改写成「连不上」。
        t.b.lock()
            .unwrap()
            .store
            .as_ref()
            .unwrap()
            .set_remote_control(false)
            .unwrap();
        let r = crate::mesh::call_remote(&t.a, &net, "B", Request::List);
        assert!(matches!(r, Response::Error(ErrorCode::RemoteControlOff)), "{r:?}");
    }

    /// 第一次 ask 超时，但 B 其实已经执行了：重试用新信封、同一个 `seq`，
    /// 拿到的是 B 缓存的那份答复——`Create` 只建了一个会话。
    #[test]
    fn a_timeout_is_retried_with_the_same_seq() {
        let t = two(true);
        let net = t.lossy(true);
        let r = crate::mesh::call_remote(&t.a, &net, "B", create());
        assert!(matches!(r, Response::Created { id: 101 }), "{r:?}");
        assert_eq!(t.inbox.remote_calls().len(), 1, "Create 只该执行一次");
        let asked = net.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 2, "超时之后该重试一次");
        let (id1, b1) = t.open_call(&asked[0]);
        let (id2, b2) = t.open_call(&asked[1]);
        assert_eq!(b1.seq, b2.seq, "重试要同一个 seq");
        assert_ne!(id1, id2, "重试要新信封");

        // 下一次调用换下一个号。
        crate::mesh::call_remote(&t.a, &net, "B", Request::List);
        let asked = net.asked.lock().unwrap().clone();
        assert_eq!(t.open_call(&asked[2]).1.seq, b1.seq + 1);
    }

    /// 老版本的对方解不开 `RemoteCall`，一声不吭丢掉：试满三次就报连不上，
    /// 不挂着、不无限重试。
    #[test]
    fn an_old_peer_that_never_answers_is_reported_unavailable() {
        struct Deaf(std::sync::atomic::AtomicUsize);
        impl Net for Deaf {
            fn peers(&self) -> Result<Vec<String>, LinkError> {
                Ok(vec![])
            }
            fn send(&self, _: &str, _: Vec<u8>) -> Result<(), LinkError> {
                Ok(())
            }
            fn ask(&self, _: &str, _: Vec<u8>, _: Duration) -> Result<Vec<u8>, LinkError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(LinkError::Relay(dct_link::LinkError::NoAnswer))
            }
            fn reply(&self, _: &str, _: u64, _: Vec<u8>) -> Result<(), LinkError> {
                Ok(())
            }
        }
        let t = two(true);
        let net = Deaf(Default::default());
        let r = crate::mesh::call_remote(&t.a, &net, "B", Request::List);
        assert!(
            matches!(r, Response::Error(ErrorCode::RemoteUnavailable { ref peer }) if peer == "B"),
            "{r:?}"
        );
        assert_eq!(net.0.load(Ordering::SeqCst), crate::mesh::REMOTE_CALL_ATTEMPTS);
        assert_eq!(crate::mesh::REMOTE_CALL_ATTEMPTS, 3);
    }

    #[test]
    fn an_unknown_peer_name_is_reported_unavailable() {
        let t = two(true);
        let net = t.lossy(false);
        for name in ["Nope", "A"] {
            let r = crate::mesh::call_remote(&t.a, &net, name, Request::List);
            assert!(
                matches!(r, Response::Error(ErrorCode::RemoteUnavailable { ref peer }) if peer == name),
                "{name}: {r:?}"
            );
        }
        assert!(net.asked.lock().unwrap().is_empty(), "不认识的、自己，都不该打网络");
    }

    /// 同一台对方，几条连接同时调：号不重、不互相排队等网络。
    #[test]
    fn concurrent_calls_to_one_peer_get_distinct_seqs() {
        let t = Arc::new(two(true));
        let net = Arc::new(t.lossy(false));
        let hs: Vec<_> = (0..8)
            .map(|_| {
                let (t, net) = (t.clone(), net.clone());
                std::thread::spawn(move || {
                    // 按键，不用 `List`：积压的「看」会被顶掉（只留最新一条）。
                    let req = Request::Input {
                        id: 7,
                        text: "x".into(),
                    };
                    crate::mesh::call_remote(&t.a, net.as_ref(), "B", req)
                })
            })
            .collect();
        for h in hs {
            let r = h.join().unwrap();
            assert!(matches!(r, Response::Ok), "{r:?}");
        }
        assert_eq!(t.inbox.remote_calls().len(), 8, "每一条都执行了一次");
        let asked = net.asked.lock().unwrap().clone();
        let mut seqs: Vec<u64> = asked.iter().map(|p| t.open_call(p).1.seq).collect();
        seqs.sort();
        seqs.dedup();
        assert_eq!(seqs.len(), 8, "{seqs:?}");
    }

    /// `ask` 在锁外：等网络的那段 `Mesh` 锁是空的；几条连接调同一台时
    /// 同时在等，不是一条等完下一条才开始。
    #[test]
    fn calls_wait_on_the_network_without_the_mesh_lock_and_side_by_side() {
        use std::sync::atomic::AtomicUsize;
        struct Slow {
            mesh: Arc<Mutex<Mesh>>,
            now: AtomicUsize,
            most: AtomicUsize,
            locked: AtomicBool,
            /// 只在一条调用独自跑时查锁：几条并发时别的线程正好在锁里封信，
            /// `try_lock` 会失败，那不是 `ask` 攥着锁。
            check_lock: AtomicBool,
        }
        impl Net for Slow {
            fn peers(&self) -> Result<Vec<String>, LinkError> {
                Ok(vec![])
            }
            fn send(&self, _: &str, _: Vec<u8>) -> Result<(), LinkError> {
                Ok(())
            }
            fn ask(&self, _: &str, _: Vec<u8>, _: Duration) -> Result<Vec<u8>, LinkError> {
                if self.check_lock.load(Ordering::SeqCst) && self.mesh.try_lock().is_err() {
                    self.locked.store(true, Ordering::SeqCst);
                }
                let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
                self.most.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(100));
                self.now.fetch_sub(1, Ordering::SeqCst);
                Err(LinkError::Relay(dct_link::LinkError::NoAnswer))
            }
            fn reply(&self, _: &str, _: u64, _: Vec<u8>) -> Result<(), LinkError> {
                Ok(())
            }
        }
        let t = two(true);
        let net = Arc::new(Slow {
            mesh: t.a.clone(),
            now: AtomicUsize::new(0),
            most: AtomicUsize::new(0),
            locked: AtomicBool::new(false),
            check_lock: AtomicBool::new(true),
        });
        crate::mesh::call_remote(&t.a, net.as_ref(), "B", Request::List);
        assert!(!net.locked.load(Ordering::SeqCst), "等网络时攥着 Mesh 锁");
        net.check_lock.store(false, Ordering::SeqCst);
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let (a, net) = (t.a.clone(), net.clone());
                std::thread::spawn(move || {
                    crate::mesh::call_remote(&a, net.as_ref(), "B", Request::List)
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert!(net.most.load(Ordering::SeqCst) > 1, "几条调用排成了一队");
    }
}
