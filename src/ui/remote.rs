//! 看板到组里别的电脑：远程会话的路由层。
//!
//! **界面主循环永远不等网络。** 一次远程调用最坏要等二十几秒（守护进程
//! 那边三次 ask，见 `mesh::worst_case`），而主循环 16ms 一帧地取画面、
//! 每个按键都同步发一条请求——只要有一处在主循环里直接问了别的电脑，
//! 对方一掉线整个看板就冻住，连 `q` 都按不动。所以：
//!
//! - `Router::call` 只做两件事：本机的请求原样交给本机守护进程；远程的
//!   请求要么读 `Feed` 的缓存（画面、列表），要么进 `Feed` 的队列（按键、
//!   新建、停止……）立刻返回。**这个函数里不许出现任何会等网络的调用**
//!   （审查点）。
//! - 每台别的电脑一个 `Feed`：一条后台线程 + 自己的一条到本机守护进程的
//!   连接，所有 `RemoteCall` 都从这条线程上发，慢只慢它自己。
//!
//! **合成 id。** 界面上下几十处拿 `u32` 指一个会话（`View::Attached(u32)`、
//! 九宫格、F3 轮转……）。远程会话给一个 `REMOTE_BASE` 以上的号，那些地方
//! 一处都不用动；`Router` 自己记着这个号对应哪台电脑的哪个真号。号只增
//! 不减：会话没了，它的号也不给下一个会话用——界面某处还攥着旧号的话，
//! 按下去的键不能落进一个毫不相干的新会话。
//!
//! **每台电脑每秒最多 2 问**（画面、列表、按键加在一起）。被操作的那台
//! 按发件电脑只留最近 64 条答复做去重，问得太密，一条执行过的按键重试时
//! 就可能已经被挤出缓存。所以按键要合并，画面只取正在看的那几个。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::client::Client;
use crate::i18n::Lang;
use crate::proto::{coded, ErrorCode, MouseForward, ProfileEntry, Request, Response, ScreenEntry};
use crate::pty::ScreenSpan;
use crate::session::{recover, ScrollBy, ScrollState, SessionInfo, SessionState};

/// 合成 id 从这里起。本机守护进程的会话号从 1 往上数，一辈子数不到 1600 万。
pub const REMOTE_BASE: u32 = 1 << 24;

/// 一次 `RemoteCall` 最多等多久。守护进程那边最坏约 26 秒（三次 ask），
/// 本机连接默认的 5 秒读超时（`client::READ_TIMEOUT`）一定不够。
pub const REMOTE_CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// 同一批按键连续送不到几次就整队清空。清空比过一会儿一股脑打进去安全：
/// 用户早就以为那些字没了，接着在打别的。
const KEY_TRIES: u32 = 3;

fn is_synthetic(id: u32) -> bool {
    id >= REMOTE_BASE
}

/// 一条到本机守护进程的连接。抽成 trait 只为了测试能换成假的——生产上
/// 两头都是 `Client`。
pub trait Line: Send {
    fn call(&mut self, req: Request) -> Result<Response>;
    fn call_within(&mut self, req: Request, timeout: Duration) -> Result<Response>;
    /// 见 `Client::take_new_daemon`。
    fn take_new_daemon(&mut self) -> bool {
        false
    }
}

impl Line for Client {
    fn call(&mut self, req: Request) -> Result<Response> {
        Client::call(self, req)
    }
    fn call_within(&mut self, req: Request, timeout: Duration) -> Result<Response> {
        Client::call_within(self, req, timeout)
    }
    fn take_new_daemon(&mut self) -> bool {
        Client::take_new_daemon(self)
    }
}

/// 给 `Feed` 的后台线程开一条它自己的连接。**不能跟界面共用一条**：
/// 一问一答的连接上，排在一个 26 秒的远程调用后面的就是界面的下一帧。
pub type Dial = Arc<dyn Fn() -> Result<Box<dyn Line>> + Send + Sync>;

/// 后台线程的节奏。
#[derive(Debug, Clone, Copy)]
pub struct Pace {
    /// 两次远程调用的起点至少隔这么久。500ms = 每台电脑每秒最多 2 问。
    pub gap: Duration,
    /// 多久拉一次对方的会话列表。
    pub list_every: Duration,
    /// 连不上之后的退避：从 `backoff_min` 起每次翻倍，到 `backoff_max` 封顶。
    /// 对方离线或者是老版本（不认识 `RemoteCall`，每问都要等满超时），
    /// 不能一直按 2 问/秒去砸它。
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// 对方的项目、agent 列表缓存多久算旧。
    pub catalog_every: Duration,
}

impl Default for Pace {
    fn default() -> Pace {
        Pace {
            gap: Duration::from_millis(500),
            list_every: Duration::from_secs(3),
            backoff_min: Duration::from_secs(2),
            backoff_max: Duration::from_secs(30),
            catalog_every: Duration::from_secs(60),
        }
    }
}

/// 一台别的电脑此刻在看板上该怎么显示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerStatus {
    /// 还没问到过。
    Connecting,
    Online,
    /// 问不到（离线、超时、老版本）。会话照样留在看板上，只是灰掉。
    Offline,
    /// 问到了，但它没开「允许我的其他电脑远程操作」。
    ControlOff,
}

/// 后台线程要告诉界面的事。界面每帧 `Router::take_events` 取走，变成底栏
/// 的一句话；id 都已经换成合成 id。
#[derive(Debug)]
pub enum FeedEvent {
    /// 按键连续 `KEY_TRIES` 次没送到，整队清空了。
    KeysLost { peer: String },
    /// 对方收到了按键，但拒绝了（没开远程、会话没了……）。
    Refused { peer: String, code: ErrorCode },
    /// `Create`/`Stop`/`Undo`/`Diff` 的答复。`Create` 答的 `Created { id }`
    /// 已换成合成 id。
    Answer {
        peer: String,
        id: Option<u32>,
        of: AnswerOf,
        resp: Response,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerOf {
    Create,
    Stop,
    Undo,
    Diff,
}

/// `(电脑名, 真号) ↔ 合成号` 的双向表。只增不减，见文件头。
#[derive(Default)]
pub(crate) struct Ids {
    to: HashMap<(String, u32), u32>,
    from: HashMap<u32, (String, u32)>,
}

impl Ids {
    pub(crate) fn synthetic(&mut self, peer: &str, real: u32) -> u32 {
        if let Some(&s) = self.to.get(&(peer.to_string(), real)) {
            return s;
        }
        // 表只增不减，所以「已发出去几个」就是下一个号——天然不复用。
        let s = REMOTE_BASE.saturating_add(self.from.len() as u32);
        self.to.insert((peer.to_string(), real), s);
        self.from.insert(s, (peer.to_string(), real));
        s
    }

    pub(crate) fn real(&self, synthetic: u32) -> Option<(&str, u32)> {
        self.from
            .get(&synthetic)
            .map(|(peer, real)| (peer.as_str(), *real))
    }
}

/// 排队等着发给对方的一件事。`id` 是对方的真号（`Create` 没有号，填 0）。
#[derive(Debug)]
pub(crate) struct Op {
    id: u32,
    kind: OpKind,
}

#[derive(Debug)]
pub(crate) enum OpKind {
    Input(String),
    Submit(String),
    Key(String),
    Mouse(MouseForward),
    Scroll(ScrollBy),
    Resize { rows: u16, cols: u16 },
    Stop,
    Undo,
    Diff,
    Create { dir: String, profile: String },
}

impl Op {
    pub(crate) fn new(id: u32, kind: OpKind) -> Op {
        Op { id, kind }
    }

    /// `Request` 不是 `Clone`（里面有密钥），失败重发时每次现拼一条。
    pub(crate) fn request(&self) -> Request {
        let id = self.id;
        match &self.kind {
            OpKind::Input(text) => Request::Input {
                id,
                text: text.clone(),
            },
            OpKind::Submit(text) => Request::Submit {
                id,
                text: text.clone(),
            },
            OpKind::Key(name) => Request::Key {
                id,
                name: name.clone(),
            },
            OpKind::Mouse(event) => Request::Mouse { id, event: *event },
            OpKind::Scroll(by) => Request::Scroll { id, by: *by },
            OpKind::Resize { rows, cols } => Request::Resize {
                id,
                rows: *rows,
                cols: *cols,
            },
            OpKind::Stop => Request::Stop { id },
            OpKind::Undo => Request::Undo { id },
            OpKind::Diff => Request::Diff { id },
            OpKind::Create { dir, profile } => Request::Create {
                dir: dir.clone(),
                profile: profile.clone(),
                // 用户在远程分组上按 `n` 亲手选的 agent，跟本机一样记下来。
                remember: true,
            },
        }
    }

    fn answer_of(&self) -> Option<AnswerOf> {
        match self.kind {
            OpKind::Create { .. } => Some(AnswerOf::Create),
            OpKind::Stop => Some(AnswerOf::Stop),
            OpKind::Undo => Some(AnswerOf::Undo),
            OpKind::Diff => Some(AnswerOf::Diff),
            _ => None,
        }
    }
}

/// 「普通字符」：没有任何控制字符（回车、退格、Esc 开头的方向键……）。
/// 只有这种能并：控制键各自一条，对方的 agent 才会按原来的节奏看到它们。
fn is_plain(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}

/// 从队首取一批去发：连续的普通字符并成一次 `Input`，连续的相对滚动加成
/// 一次，连续的改尺寸只留最后一次。**只并紧挨着的同一会话的**——跨过
/// 一个回车去并，字就跑到回车前面去了。
pub(crate) fn take_batch(q: &mut VecDeque<Op>) -> Option<Op> {
    let mut head = q.pop_front()?;
    while let Some(next) = q.front() {
        if next.id != head.id {
            break;
        }
        match (&mut head.kind, &next.kind) {
            (OpKind::Input(a), OpKind::Input(b)) if is_plain(a) && is_plain(b) => {
                a.push_str(b);
            }
            (OpKind::Scroll(ScrollBy::Rows(a)), OpKind::Scroll(ScrollBy::Rows(b))) => {
                *a = a.saturating_add(*b);
            }
            (OpKind::Resize { .. }, OpKind::Resize { .. }) => {
                head = q.pop_front().expect("刚看过队首");
                continue;
            }
            _ => break,
        }
        q.pop_front();
    }
    Some(head)
}

/// 一张缓存的画面。
struct Cached {
    lines: Vec<Vec<ScreenSpan>>,
    cursor: (u16, u16),
    cursor_hidden: bool,
    /// `Screens` 只带回行，不带状态——那时是 `None`，退回列表里的状态。
    state: Option<SessionState>,
    scroll: ScrollState,
    /// 对方说这个会话取不了画面（没了、不许）。
    error: Option<ErrorCode>,
    at: Instant,
}

impl Cached {
    fn blank(at: Instant) -> Cached {
        Cached {
            lines: Vec::new(),
            cursor: (0, 0),
            cursor_hidden: true,
            state: None,
            scroll: ScrollState::default(),
            error: None,
            at,
        }
    }
}

struct State {
    stop: bool,
    status: PeerStatus,
    /// 对方最近一次答的会话列表，真号。
    sessions: Vec<SessionInfo>,
    screens: HashMap<u32, Cached>,
    /// 正在看的会话（真号）。只给这几个取画面。
    watched: Vec<u32>,
    queue: VecDeque<Op>,
    events: Vec<FeedEvent>,
    projects: Option<(Vec<String>, Vec<String>)>,
    projects_at: Option<Instant>,
    profiles: Option<Vec<ProfileEntry>>,
    profiles_at: Option<Instant>,
    want_projects: bool,
    want_profiles: Option<Lang>,
}

struct Shared {
    st: Mutex<State>,
    wake: Condvar,
}

/// 一台别的电脑的后台线程：取列表、取画面、发按键，结果放进缓存。
///
/// 界面这一头只碰 `Shared` 里的锁，而后台线程**从不攥着锁去等网络**——
/// 读缓存最多等对方线程抄完一份数据。
pub struct Feed {
    shared: Arc<Shared>,
}

impl Feed {
    pub fn spawn(peer: String, dial: Dial, pace: Pace) -> Feed {
        let shared = Arc::new(Shared {
            st: Mutex::new(State {
                stop: false,
                status: PeerStatus::Connecting,
                sessions: Vec::new(),
                screens: HashMap::new(),
                watched: Vec::new(),
                queue: VecDeque::new(),
                events: Vec::new(),
                projects: None,
                projects_at: None,
                profiles: None,
                profiles_at: None,
                want_projects: false,
                want_profiles: None,
            }),
            wake: Condvar::new(),
        });
        let s = shared.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("dct-feed-{peer}"))
            .spawn(move || {
                // 同 `bridge.rs::spawn`：一台电脑的线程死了是遗憾，不能把
                // 看板一起带走。死了就标成离线，会话照旧留在看板上。
                let r = catch_unwind(AssertUnwindSafe(|| run(&peer, &s, &dial, pace)));
                if r.is_err() {
                    recover(s.st.lock()).status = PeerStatus::Offline;
                }
            });
        if spawned.is_err() {
            recover(shared.st.lock()).status = PeerStatus::Offline;
        }
        Feed { shared }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        recover(self.shared.st.lock())
    }

    /// 设置现在要刷画面的会话（真号）。空 = 不看任何一个，只取列表。
    pub fn watch(&self, ids: Vec<u32>) {
        let mut st = self.lock();
        if st.watched != ids {
            st.watched = ids;
            self.shared.wake.notify_all();
        }
    }

    fn enqueue(&self, op: Op) {
        self.lock().queue.push_back(op);
        self.shared.wake.notify_all();
    }

    /// 这个会话（真号）的画面是多久以前取回来的；还没取到过是 `None`。
    pub fn age(&self, id: u32) -> Option<Duration> {
        self.lock().screens.get(&id).map(|c| c.at.elapsed())
    }

    pub fn status(&self) -> PeerStatus {
        self.lock().status
    }
}

impl Drop for Feed {
    /// 不 join：线程可能正卡在一次 26 秒的调用里，看板不能陪它等。它醒来
    /// 看见 `stop` 自己走。
    fn drop(&mut self) {
        self.lock().stop = true;
        self.shared.wake.notify_all();
    }
}

enum Job {
    Keys(Op),
    Screens(Vec<u32>),
    List,
    Projects,
    Profiles(Lang),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Nothing,
    Keys,
    Screens,
    List,
    Catalog,
}

impl Job {
    fn kind(&self) -> JobKind {
        match self {
            Job::Keys(_) => JobKind::Keys,
            Job::Screens(_) => JobKind::Screens,
            Job::List => JobKind::List,
            Job::Projects | Job::Profiles(_) => JobKind::Catalog,
        }
    }

    fn request(&self) -> Request {
        match self {
            Job::Keys(op) => op.request(),
            // 只看一个会话时用 `Screen`：它带光标、状态、滚动，`Screens` 只带行。
            Job::Screens(ids) if ids.len() == 1 => Request::Screen { id: ids[0] },
            Job::Screens(ids) => Request::Screens { ids: ids.clone() },
            Job::List => Request::List,
            Job::Projects => Request::Projects,
            Job::Profiles(lang) => Request::Profiles { lang: *lang },
        }
    }
}

/// 后台线程自己的排程状态，不进锁。
struct Schedule {
    last_start: Option<Instant>,
    last: JobKind,
    next_list: Instant,
    not_before: Instant,
    backoff: Duration,
    /// 队首这批按键已经连续失败几次。
    key_failures: u32,
}

impl Schedule {
    fn earliest(&self, pace: Pace) -> Instant {
        let gap = self
            .last_start
            .map(|t| t + pace.gap)
            .unwrap_or(self.not_before);
        gap.max(self.not_before)
    }
}

/// 挑下一件事。按键优先（用户在等回显）；但正看着画面时按键和画面轮流，
/// 不然一直打字就一直看不到字落下去。列表到点就取，被挤掉太久（过了一整
/// 个周期）就插队——离线判断靠它。
fn pick(st: &mut State, s: &Schedule, now: Instant, pace: Pace) -> Option<Job> {
    let list_due = now >= s.next_list;
    let list_starved = list_due && now >= s.next_list + pace.list_every;
    // 没开远程的电脑取画面也只会被拒，只靠列表等它开。
    let watching = !st.watched.is_empty() && st.status != PeerStatus::ControlOff;
    let keys = !st.queue.is_empty();
    if list_starved {
        return Some(Job::List);
    }
    if keys && !(watching && s.last == JobKind::Keys) {
        return take_batch(&mut st.queue).map(Job::Keys);
    }
    if list_due && s.last != JobKind::List {
        return Some(Job::List);
    }
    if st.want_projects {
        return Some(Job::Projects);
    }
    if let Some(lang) = st.want_profiles {
        return Some(Job::Profiles(lang));
    }
    if watching {
        return Some(Job::Screens(st.watched.clone()));
    }
    if list_due {
        return Some(Job::List);
    }
    None
}

fn run(peer: &str, shared: &Shared, dial: &Dial, pace: Pace) {
    let now = Instant::now();
    let mut s = Schedule {
        last_start: None,
        last: JobKind::Nothing,
        next_list: now,
        not_before: now,
        backoff: Duration::ZERO,
        key_failures: 0,
    };
    let mut line: Option<Box<dyn Line>> = None;
    loop {
        let job = {
            let mut st = recover(shared.st.lock());
            loop {
                if st.stop {
                    return;
                }
                let now = Instant::now();
                let earliest = s.earliest(pace);
                if now < earliest {
                    st = recover(shared.wake.wait_timeout(st, earliest - now)).0;
                    continue;
                }
                if let Some(job) = pick(&mut st, &s, now, pace) {
                    break job;
                }
                let idle = s
                    .next_list
                    .saturating_duration_since(now)
                    .max(Duration::from_millis(1));
                st = recover(shared.wake.wait_timeout(st, idle)).0;
            }
        };
        // 锁已经放了，下面这一问爱等多久等多久。
        s.last_start = Some(Instant::now());
        s.last = job.kind();
        if line.is_none() {
            line = dial().ok();
        }
        let result = match line.as_mut() {
            Some(l) => l.call_within(
                Request::RemoteCall {
                    peer: peer.to_string(),
                    req: Box::new(job.request()),
                },
                REMOTE_CALL_TIMEOUT,
            ),
            None => Err(coded(ErrorCode::DaemonNotResponding)),
        };
        settle(peer, shared, &mut s, pace, job, result);
    }
}

/// 把一次调用的结果落进缓存、排程和事件。
fn settle(
    peer: &str,
    shared: &Shared,
    s: &mut Schedule,
    pace: Pace,
    job: Job,
    result: Result<Response>,
) {
    let now = Instant::now();
    let mut st = recover(shared.st.lock());
    if matches!(job, Job::List) {
        s.next_list = now + pace.list_every;
    }
    let resp = match result {
        Ok(Response::Error(ErrorCode::RemoteUnavailable { .. })) | Err(_) => {
            st.status = PeerStatus::Offline;
            s.backoff = (s.backoff * 2).clamp(pace.backoff_min, pace.backoff_max);
            s.not_before = now + s.backoff;
            if let Job::Keys(op) = job {
                s.key_failures += 1;
                if s.key_failures >= KEY_TRIES {
                    s.key_failures = 0;
                    st.queue.clear();
                    st.events.push(FeedEvent::KeysLost {
                        peer: peer.to_string(),
                    });
                } else {
                    // 留在队首重发，后面的不许插队。
                    st.queue.push_front(op);
                }
            }
            return;
        }
        Ok(resp) => resp,
    };
    s.backoff = Duration::ZERO;
    s.not_before = now;
    st.status = if matches!(resp, Response::Error(ErrorCode::RemoteControlOff)) {
        PeerStatus::ControlOff
    } else {
        PeerStatus::Online
    };
    match job {
        Job::List => {
            if let Response::Sessions(v) = resp {
                st.screens.retain(|id, _| v.iter().any(|s| s.id == *id));
                st.sessions = v;
            }
        }
        Job::Screens(ids) => match resp {
            Response::Screen {
                lines,
                cursor,
                cursor_hidden,
                state,
                scroll,
            } => {
                st.screens.insert(
                    ids[0],
                    Cached {
                        lines,
                        cursor,
                        cursor_hidden,
                        state: Some(state),
                        scroll,
                        error: None,
                        at: now,
                    },
                );
            }
            // 没带回来的屏 = 这轮没更新（对方装不下就略掉），留着上一张。
            Response::Screens { screens } => {
                for e in screens {
                    let c = st.screens.entry(e.id).or_insert_with(|| Cached::blank(now));
                    c.lines = e.lines;
                    c.error = None;
                    c.at = now;
                }
            }
            Response::Error(code) if ids.len() == 1 && code != ErrorCode::RemoteControlOff => {
                st.screens
                    .entry(ids[0])
                    .or_insert_with(|| Cached::blank(now))
                    .error = Some(code);
            }
            _ => {}
        },
        Job::Keys(op) => {
            s.key_failures = 0;
            match op.answer_of() {
                Some(of) => st.events.push(FeedEvent::Answer {
                    peer: peer.to_string(),
                    id: (op.id != 0).then_some(op.id),
                    of,
                    resp,
                }),
                None => {
                    if let Response::Error(code) = resp {
                        st.events.push(FeedEvent::Refused {
                            peer: peer.to_string(),
                            code,
                        });
                    }
                }
            }
        }
        Job::Projects => {
            st.want_projects = false;
            if let Response::Projects { recent, pinned } = resp {
                st.projects = Some((recent, pinned));
                st.projects_at = Some(now);
            }
        }
        Job::Profiles(_) => {
            st.want_profiles = None;
            if let Response::Profiles { entries, .. } = resp {
                st.profiles = Some(entries);
                st.profiles_at = Some(now);
            }
        }
    }
}

/// 看板手里的「守护进程连接」：本机的请求原样转给本机，远程会话的请求
/// 走各自的 `Feed`。`App::client()` 返回的就是它。
pub struct Router {
    local: Box<dyn Line>,
    /// 电脑名 → 它的后台线程。`BTreeMap`：看板上远程分组的顺序要稳。
    remotes: BTreeMap<String, Feed>,
    ids: Ids,
    dial: Dial,
    pace: Pace,
    lang: Lang,
}

impl From<Client> for Router {
    fn from(local: Client) -> Router {
        Router::new(local)
    }
}

impl Router {
    pub fn new(local: Client) -> Router {
        let socket = local.socket().to_path_buf();
        let dial: Dial = Arc::new(move || Ok(Box::new(Client::connect(&socket)?) as Box<dyn Line>));
        Router::with_parts(Box::new(local), dial, Pace::default())
    }

    pub(crate) fn with_parts(local: Box<dyn Line>, dial: Dial, pace: Pace) -> Router {
        Router {
            local,
            remotes: BTreeMap::new(),
            ids: Ids::default(),
            dial,
            pace,
            lang: Lang::Zh,
        }
    }

    /// 「正在连接…」那一行用哪种语言。`App::client()` 每次顺手同步。
    pub fn set_lang(&mut self, lang: Lang) {
        self.lang = lang;
    }

    /// 见 `Client::take_new_daemon`。远程会话的合成号不受本机后台重启影响。
    pub fn take_new_daemon(&mut self) -> bool {
        self.local.take_new_daemon()
    }

    /// 组里**别的**电脑（不含本机）。新来的起一个 `Feed`，走了的停掉它的
    /// 线程；合成号表不动（号永不复用）。
    pub fn set_peers(&mut self, names: &[String]) {
        self.remotes.retain(|name, _| names.contains(name));
        for name in names {
            if !self.remotes.contains_key(name) {
                let feed = Feed::spawn(name.clone(), self.dial.clone(), self.pace);
                self.remotes.insert(name.clone(), feed);
            }
        }
    }

    /// 界面正在看哪些会话（进了会话就是那一个，九宫格就是那一页）。本机
    /// 号忽略；不在这里面的远程电脑一律停止取画面，只取列表。
    pub fn watch(&mut self, ids: &[u32]) {
        let mut by_peer: HashMap<&str, Vec<u32>> = HashMap::new();
        for &id in ids {
            if let Some((peer, real)) = self.ids.real(id) {
                by_peer.entry(peer).or_default().push(real);
            }
        }
        for (name, feed) in &self.remotes {
            feed.watch(by_peer.remove(name.as_str()).unwrap_or_default());
        }
    }

    pub fn status(&self, peer: &str) -> Option<PeerStatus> {
        self.remotes.get(peer).map(Feed::status)
    }

    /// 合成号那个会话的画面是多久以前的（给「n 秒前」）。本机会话、还没
    /// 取到过画面都是 `None`。
    pub fn age(&self, id: u32) -> Option<Duration> {
        let (peer, real) = self.ids.real(id)?;
        self.remotes.get(peer)?.age(real)
    }

    /// 这个号的会话在哪台电脑上；本机是 `None`。
    pub fn machine_of(&self, id: u32) -> Option<&str> {
        self.ids.real(id).map(|(peer, _)| peer)
    }

    /// F5 粘图之前问一句：远程会话只能粘文字（图要传文件，这一期不做）。
    pub fn check_image_paste(&self, id: u32) -> Result<()> {
        if is_synthetic(id) {
            return Err(coded(ErrorCode::RemoteForbidden));
        }
        Ok(())
    }

    /// 在 `peer` 上开一个会话。进队列立刻返回，结果是一条
    /// `FeedEvent::Answer { of: AnswerOf::Create, .. }`。
    pub fn create_on(&mut self, peer: &str, dir: String, profile: String) -> Result<()> {
        let feed = self.remotes.get(peer).ok_or_else(|| {
            coded(ErrorCode::RemoteUnavailable {
                peer: peer.to_string(),
            })
        })?;
        feed.enqueue(Op::new(0, OpKind::Create { dir, profile }));
        Ok(())
    }

    /// 对方的项目（`recent`, `pinned`）。读缓存；没有或旧了就让后台去取，
    /// 这一次答 `None`（或旧的那份）。
    pub fn remote_projects(&mut self, peer: &str) -> Option<(Vec<String>, Vec<String>)> {
        let feed = self.remotes.get(peer)?;
        let every = self.pace.catalog_every;
        let mut st = feed.lock();
        if st.projects_at.is_none_or(|t| t.elapsed() >= every) && !st.want_projects {
            st.want_projects = true;
            feed.shared.wake.notify_all();
        }
        st.projects.clone()
    }

    /// 对方的 agent 列表，同 `remote_projects`。
    pub fn remote_profiles(&mut self, peer: &str) -> Option<Vec<ProfileEntry>> {
        let feed = self.remotes.get(peer)?;
        let every = self.pace.catalog_every;
        let mut st = feed.lock();
        if st.profiles_at.is_none_or(|t| t.elapsed() >= every) && st.want_profiles.is_none() {
            st.want_profiles = Some(self.lang);
            feed.shared.wake.notify_all();
        }
        st.profiles.clone()
    }

    /// 取走各个后台线程攒下的事，号换成合成号。
    pub fn take_events(&mut self) -> Vec<FeedEvent> {
        let mut out = Vec::new();
        for (peer, feed) in &self.remotes {
            let events = std::mem::take(&mut feed.lock().events);
            for mut e in events {
                if let FeedEvent::Answer { id, of, resp, .. } = &mut e {
                    if let Some(real) = id {
                        *real = self.ids.synthetic(peer, *real);
                    }
                    if let (AnswerOf::Create, Response::Created { id }) = (*of, resp) {
                        *id = self.ids.synthetic(peer, *id);
                    }
                }
                out.push(e);
            }
        }
        out
    }

    /// **这里面不许有任何等网络的调用。** 远程的要么读缓存、要么进队列。
    pub fn call(&mut self, req: Request) -> Result<Response> {
        use Request as R;
        match req {
            R::List => self.list(),
            R::Screen { id } if is_synthetic(id) => Ok(self.cached_screen(id)),
            R::Screens { ids } if ids.iter().any(|&id| is_synthetic(id)) => self.screens(ids),
            R::Input { id, text } if is_synthetic(id) => Ok(self.enqueue(id, OpKind::Input(text))),
            R::Submit { id, text } if is_synthetic(id) => {
                Ok(self.enqueue(id, OpKind::Submit(text)))
            }
            R::Key { id, name } if is_synthetic(id) => Ok(self.enqueue(id, OpKind::Key(name))),
            R::Mouse { id, event } if is_synthetic(id) => {
                Ok(self.enqueue(id, OpKind::Mouse(event)))
            }
            R::Scroll { id, by } if is_synthetic(id) => Ok(self.enqueue(id, OpKind::Scroll(by))),
            R::Resize { id, rows, cols } if is_synthetic(id) => {
                Ok(self.enqueue(id, OpKind::Resize { rows, cols }))
            }
            R::Stop { id } if is_synthetic(id) => Ok(self.enqueue(id, OpKind::Stop)),
            R::Undo { id } if is_synthetic(id) => Ok(self.enqueue(id, OpKind::Undo)),
            R::Diff { id } if is_synthetic(id) => Ok(self.enqueue(id, OpKind::Diff)),
            // 出错解释要问对方的模型，这一期不做；界面退回今天就有的那句提示。
            R::Explanation { id } if is_synthetic(id) => Ok(Response::Explanation(None)),
            // 远程不给强杀（spec 第 2 节）；直播只播本机的会话。
            R::Kill { id } if is_synthetic(id) => Ok(Response::Error(ErrorCode::RemoteForbidden)),
            R::LiveStart { ref ids, .. } | R::LiveRestage { ref ids, .. }
                if ids.iter().any(|&id| is_synthetic(id)) =>
            {
                Ok(Response::Error(ErrorCode::RemoteForbidden))
            }
            other => self.local.call(other),
        }
    }

    fn list(&mut self) -> Result<Response> {
        let resp = self.local.call(Request::List)?;
        let Response::Sessions(mut all) = resp else {
            return Ok(resp);
        };
        for (peer, feed) in &self.remotes {
            let theirs = feed.lock().sessions.clone();
            for mut s in theirs {
                s.id = self.ids.synthetic(peer, s.id);
                s.machine = Some(peer.clone());
                all.push(s);
            }
        }
        Ok(Response::Sessions(all))
    }

    fn enqueue(&mut self, id: u32, kind: OpKind) -> Response {
        let Some((peer, real)) = self.ids.real(id) else {
            return Response::Error(ErrorCode::NoSuchSession(id));
        };
        match self.remotes.get(peer) {
            Some(feed) => {
                feed.enqueue(Op::new(real, kind));
                Response::Ok
            }
            None => Response::Error(ErrorCode::RemoteUnavailable {
                peer: peer.to_string(),
            }),
        }
    }

    fn connecting(&self, peer: &str) -> Vec<Vec<ScreenSpan>> {
        vec![vec![ScreenSpan {
            text: crate::i18n::msg::remote_connecting(self.lang, peer),
            style: Default::default(),
        }]]
    }

    fn cached_screen(&self, id: u32) -> Response {
        let Some((peer, real)) = self.ids.real(id) else {
            return Response::Error(ErrorCode::NoSuchSession(id));
        };
        let Some(feed) = self.remotes.get(peer) else {
            return Response::Error(ErrorCode::RemoteUnavailable {
                peer: peer.to_string(),
            });
        };
        let st = feed.lock();
        let listed = st
            .sessions
            .iter()
            .find(|s| s.id == real)
            .map(|s| s.state)
            .unwrap_or(SessionState::Unknown);
        match st.screens.get(&real) {
            Some(Cached {
                error: Some(code), ..
            }) => Response::Error(code.clone()),
            Some(c) => Response::Screen {
                lines: c.lines.clone(),
                cursor: c.cursor,
                cursor_hidden: c.cursor_hidden,
                state: c.state.unwrap_or(listed),
                scroll: c.scroll,
            },
            None => {
                drop(st);
                Response::Screen {
                    lines: self.connecting(peer),
                    cursor: (0, 0),
                    cursor_hidden: true,
                    state: listed,
                    scroll: ScrollState::default(),
                }
            }
        }
    }

    /// 九宫格一页里可能本机、远程混着：本机的一次问本机，远程的读缓存，
    /// 按请求的顺序拼回去。
    fn screens(&mut self, ids: Vec<u32>) -> Result<Response> {
        let local: Vec<u32> = ids.iter().copied().filter(|&i| !is_synthetic(i)).collect();
        let mut got: HashMap<u32, Vec<Vec<ScreenSpan>>> = HashMap::new();
        if !local.is_empty() {
            match self.local.call(Request::Screens { ids: local })? {
                Response::Screens { screens } => {
                    got.extend(screens.into_iter().map(|e| (e.id, e.lines)));
                }
                other => return Ok(other),
            }
        }
        for &id in ids.iter().filter(|&&i| is_synthetic(i)) {
            // 取不了的屏不放进答复：缺席 = 没有更新，同对方装不下时一样。
            if let Response::Screen { lines, .. } = self.cached_screen(id) {
                got.insert(id, lines);
            }
        }
        let screens = ids
            .into_iter()
            .filter_map(|id| got.remove(&id).map(|lines| ScreenEntry { id, lines }))
            .collect();
        Ok(Response::Screens { screens })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::wait_up_to;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sess(id: u32, dir: &str) -> SessionInfo {
        SessionInfo {
            id,
            profile: "claude".into(),
            dir: dir.into(),
            state: SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
            last_active_ms: 0,
            machine: None,
        }
    }

    /// 本机那一头：记下收到的每一条，`List` 答一份固定的列表，别的答 `Ok`。
    struct FakeLocal {
        seen: Arc<Mutex<Vec<String>>>,
        sessions: Vec<SessionInfo>,
    }

    impl Line for FakeLocal {
        fn call(&mut self, req: Request) -> Result<Response> {
            self.seen.lock().unwrap().push(format!("{req:?}"));
            Ok(match req {
                Request::List => Response::Sessions(self.sessions.clone()),
                Request::Screens { ids } => Response::Screens {
                    screens: ids
                        .into_iter()
                        .map(|id| ScreenEntry { id, lines: vec![] })
                        .collect(),
                },
                _ => Response::Ok,
            })
        }
        fn call_within(&mut self, req: Request, _t: Duration) -> Result<Response> {
            self.call(req)
        }
    }

    type Script = dyn Fn(&Request) -> Result<Response> + Send + Sync;

    /// 远程那一头：`RemoteCall` 拆开，里面那条交给 `script` 答，并记下来。
    #[derive(Clone)]
    struct FakeRemote {
        seen: Arc<Mutex<Vec<String>>>,
        script: Arc<Script>,
        delay: Duration,
    }

    impl Line for FakeRemote {
        fn call(&mut self, req: Request) -> Result<Response> {
            self.call_within(req, Duration::from_secs(5))
        }
        fn call_within(&mut self, req: Request, _t: Duration) -> Result<Response> {
            std::thread::sleep(self.delay);
            let Request::RemoteCall { req, .. } = req else {
                panic!("Feed 只该发 RemoteCall");
            };
            self.seen.lock().unwrap().push(format!("{req:?}"));
            (self.script)(&req)
        }
    }

    fn fast() -> Pace {
        Pace {
            gap: Duration::from_millis(1),
            list_every: Duration::from_millis(20),
            backoff_min: Duration::from_millis(1),
            backoff_max: Duration::from_millis(5),
            catalog_every: Duration::from_secs(60),
        }
    }

    struct Rig {
        router: Router,
        local_seen: Arc<Mutex<Vec<String>>>,
        remote_seen: Arc<Mutex<Vec<String>>>,
    }

    fn rig(local: Vec<SessionInfo>, remote: FakeRemote, pace: Pace) -> Rig {
        let local_seen = Arc::new(Mutex::new(Vec::new()));
        let remote_seen = remote.seen.clone();
        let dial: Dial = Arc::new(move || Ok(Box::new(remote.clone()) as Box<dyn Line>));
        let router = Router::with_parts(
            Box::new(FakeLocal {
                seen: local_seen.clone(),
                sessions: local,
            }),
            dial,
            pace,
        );
        Rig {
            router,
            local_seen,
            remote_seen,
        }
    }

    fn remote(script: impl Fn(&Request) -> Result<Response> + Send + Sync + 'static) -> FakeRemote {
        FakeRemote {
            seen: Arc::new(Mutex::new(Vec::new())),
            script: Arc::new(script),
            delay: Duration::ZERO,
        }
    }

    fn sessions_of(r: &mut Router) -> Vec<SessionInfo> {
        match r.call(Request::List).unwrap() {
            Response::Sessions(v) => v,
            other => panic!("List 该答 Sessions，答了 {other:?}"),
        }
    }

    /// 等远程列表进了缓存，返回那台电脑上第一个会话的合成 id。
    fn first_remote_id(r: &mut Router, peer: &str) -> u32 {
        let mut found = None;
        assert!(
            wait_up_to(Duration::from_secs(5), || {
                found = sessions_of(r)
                    .into_iter()
                    .find(|s| s.machine.as_deref() == Some(peer))
                    .map(|s| s.id);
                found.is_some()
            }),
            "远程列表一直没进缓存"
        );
        found.unwrap()
    }

    fn inputs(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|s| !s.starts_with("List") && !s.starts_with("Screen"))
            .cloned()
            .collect()
    }

    #[test]
    fn local_requests_pass_straight_through() {
        let mut rig = rig(vec![sess(3, "/p")], remote(|_| Ok(Response::Ok)), fast());
        assert!(matches!(
            rig.router.call(Request::Screen { id: 3 }).unwrap(),
            Response::Ok
        ));
        rig.router
            .call(Request::Input {
                id: 3,
                text: "x".into(),
            })
            .unwrap();
        let seen = rig.local_seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(seen[0].starts_with("Screen"), "{seen:?}");
        assert!(seen[1].starts_with("Input"), "{seen:?}");
        // 没有远程电脑时，远程那头一条都不该收到
        assert!(rig.remote_seen.lock().unwrap().is_empty());
    }

    /// Review Focus 3：远程每一问都要睡 1 秒，主循环取画面、打字也必须是
    /// 读缓存/进队列，一帧都不等网络。
    #[test]
    fn a_slow_remote_never_blocks_a_frame() {
        let mut slow = remote(|req| {
            Ok(match req {
                Request::List => Response::Sessions(vec![sess(7, "/r")]),
                _ => Response::Ok,
            })
        });
        slow.delay = Duration::from_secs(1);
        let mut rig = rig(vec![], slow, fast());
        rig.router.set_peers(&["mac".to_string()]);
        let id = first_remote_id(&mut rig.router, "mac");
        rig.router.watch(&[id]);

        let start = Instant::now();
        for i in 0..50 {
            rig.router.call(Request::Screen { id }).unwrap();
            rig.router
                .call(Request::Input {
                    id,
                    text: format!("{i}"),
                })
                .unwrap();
        }
        let took = start.elapsed();
        assert!(took < Duration::from_millis(50), "50 帧花了 {took:?}");
    }

    #[test]
    fn plain_characters_coalesce_control_keys_do_not() {
        let mut q: VecDeque<Op> = VecDeque::new();
        for t in ["a", "b", "c"] {
            q.push_back(Op::new(1, OpKind::Input(t.into())));
        }
        q.push_back(Op::new(1, OpKind::Key("Enter".into())));
        q.push_back(Op::new(1, OpKind::Input("d".into())));
        q.push_back(Op::new(1, OpKind::Input("\r".into())));
        q.push_back(Op::new(1, OpKind::Input("e".into())));
        q.push_back(Op::new(2, OpKind::Input("f".into())));

        let mut got = Vec::new();
        while let Some(op) = take_batch(&mut q) {
            got.push(format!("{:?}", op.request()));
        }
        assert_eq!(
            got,
            vec![
                r#"Input { id: 1, text: "abc" }"#,
                "Key(1, Enter)",
                r#"Input { id: 1, text: "d" }"#,
                r#"Input { id: 1, text: "\r" }"#,
                // 回车之后的普通字符不能跨过回车并到前面去
                r#"Input { id: 1, text: "e" }"#,
                // 另一个会话的字不能并进来
                r#"Input { id: 2, text: "f" }"#,
            ]
        );
    }

    #[test]
    fn keys_keep_their_order_across_a_retry() {
        let failed_once = Arc::new(AtomicUsize::new(0));
        let f = failed_once.clone();
        let r = remote(move |req| match req {
            Request::List => Ok(Response::Sessions(vec![sess(7, "/r")])),
            Request::Input { .. } if f.fetch_add(1, Ordering::SeqCst) == 0 => {
                Ok(Response::Error(ErrorCode::RemoteUnavailable {
                    peer: "mac".into(),
                }))
            }
            _ => Ok(Response::Ok),
        });
        let mut rig = rig(vec![], r, fast());
        rig.router.set_peers(&["mac".to_string()]);
        let id = first_remote_id(&mut rig.router, "mac");
        for req in [
            Request::Input {
                id,
                text: "a".into(),
            },
            Request::Key {
                id,
                name: "Enter".into(),
            },
            Request::Input {
                id,
                text: "b".into(),
            },
        ] {
            assert!(matches!(rig.router.call(req).unwrap(), Response::Ok));
        }
        assert!(wait_up_to(Duration::from_secs(5), || inputs(
            &rig.remote_seen
        )
        .len()
            >= 4));
        let got = inputs(&rig.remote_seen);
        assert_eq!(
            got,
            vec![
                r#"Input { id: 7, text: "a" }"#,
                r#"Input { id: 7, text: "a" }"#,
                "Key(7, Enter)",
                r#"Input { id: 7, text: "b" }"#,
            ],
            "失败的那条留在队首重发，后面的不许插队"
        );
        assert!(
            rig.router.take_events().is_empty(),
            "重试成功了就不该报丢键"
        );
    }

    #[test]
    fn three_failures_drop_the_queue_and_say_so() {
        let refuse = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let r_ = refuse.clone();
        let r = remote(move |req| match req {
            Request::List => Ok(Response::Sessions(vec![sess(7, "/r")])),
            _ if r_.load(Ordering::SeqCst) => Err(anyhow::anyhow!("relay down")),
            _ => Ok(Response::Ok),
        });
        let mut rig = rig(vec![], r, fast());
        rig.router.set_peers(&["mac".to_string()]);
        let id = first_remote_id(&mut rig.router, "mac");
        rig.router
            .call(Request::Input {
                id,
                text: "a".into(),
            })
            .unwrap();
        rig.router
            .call(Request::Key {
                id,
                name: "Enter".into(),
            })
            .unwrap();

        let mut events = Vec::new();
        assert!(wait_up_to(Duration::from_secs(5), || {
            events.extend(rig.router.take_events());
            !events.is_empty()
        }));
        assert!(
            matches!(&events[..], [FeedEvent::KeysLost { peer }] if peer == "mac"),
            "{events:?}"
        );
        let tries = inputs(&rig.remote_seen);
        assert_eq!(
            tries,
            vec![r#"Input { id: 7, text: "a" }"#; 3],
            "试三次就停，回车不能在清空之后再被打进去"
        );

        // 清空之后队列重新可用：下一条照常送到
        refuse.store(false, Ordering::SeqCst);
        rig.router
            .call(Request::Input {
                id,
                text: "z".into(),
            })
            .unwrap();
        assert!(wait_up_to(Duration::from_secs(5), || inputs(
            &rig.remote_seen
        )
        .last()
        .is_some_and(|s| s.contains("\"z\""))));
        assert_eq!(inputs(&rig.remote_seen).len(), 4);
    }

    #[test]
    fn list_merges_remote_sessions_with_synthetic_ids_and_machine_names() {
        let r = remote(|req| {
            Ok(match req {
                Request::List => Response::Sessions(vec![sess(1, "/r1"), sess(5, "/r5")]),
                _ => Response::Ok,
            })
        });
        let mut rig = rig(vec![sess(1, "/l1"), sess(2, "/l2")], r, fast());
        rig.router.set_peers(&["mac".to_string()]);
        first_remote_id(&mut rig.router, "mac");

        let all = sessions_of(&mut rig.router);
        let local: Vec<_> = all.iter().filter(|s| s.machine.is_none()).collect();
        let far: Vec<_> = all.iter().filter(|s| s.machine.is_some()).collect();
        assert_eq!(
            local.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![1, 2],
            "本机会话原样，排在前面"
        );
        assert_eq!(all[0].machine, None);
        assert_eq!(far.len(), 2);
        for s in &far {
            assert_eq!(s.machine.as_deref(), Some("mac"));
            assert!(s.id >= REMOTE_BASE, "远程会话必须换成合成 id：{}", s.id);
        }
        assert_eq!(far[0].dir, "/r1");
        assert_ne!(far[0].id, far[1].id);
        // 合成 id 是稳的：再拉一次还是同一个号
        let again = sessions_of(&mut rig.router);
        assert_eq!(
            again.iter().map(|s| s.id).collect::<Vec<_>>(),
            all.iter().map(|s| s.id).collect::<Vec<_>>()
        );
        // 进了这个合成 id，画面请求落在远程那台、用的是真 id
        rig.router.watch(&[far[1].id]);
        assert!(wait_up_to(Duration::from_secs(5), || rig
            .remote_seen
            .lock()
            .unwrap()
            .iter()
            .any(|s| s == "Screen { id: 5 }")));
    }

    #[test]
    fn an_offline_peer_keeps_its_sessions_marked_offline() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let r = remote(move |_| {
            if c.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Response::Sessions(vec![sess(9, "/r")]))
            } else {
                Ok(Response::Error(ErrorCode::RemoteUnavailable {
                    peer: "mac".into(),
                }))
            }
        });
        let mut pace = fast();
        pace.backoff_min = Duration::from_millis(50);
        pace.backoff_max = Duration::from_millis(200);
        let mut rig = rig(vec![], r, pace);
        rig.router.set_peers(&["mac".to_string()]);
        let id = first_remote_id(&mut rig.router, "mac");
        assert!(wait_up_to(Duration::from_secs(5), || rig
            .router
            .status("mac")
            == Some(PeerStatus::Offline)));
        assert!(
            sessions_of(&mut rig.router).iter().any(|s| s.id == id),
            "连不上的电脑，会话照样留在看板上"
        );

        // 退避：连不上之后不许每 1ms 就问一次
        let before = calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(500));
        let asked = calls.load(Ordering::SeqCst) - before;
        assert!(asked <= 6, "离线的电脑 500ms 里被问了 {asked} 次");
    }

    #[test]
    fn synthetic_ids_are_never_reused() {
        let mut ids = Ids::default();
        let a = ids.synthetic("mac", 1);
        let b = ids.synthetic("mac", 2);
        let c = ids.synthetic("pc", 1);
        assert!(a >= REMOTE_BASE && b >= REMOTE_BASE && c >= REMOTE_BASE);
        assert_eq!(ids.synthetic("mac", 1), a, "同一个会话永远是同一个号");
        assert_eq!(ids.real(b), Some(("mac", 2)));
        // 会话没了再来新的：拿新号，旧号不回收
        let d = ids.synthetic("mac", 3);
        let all = [a, b, c, d];
        for (i, x) in all.iter().enumerate() {
            for y in &all[i + 1..] {
                assert_ne!(x, y);
            }
        }
        assert_eq!(ids.real(REMOTE_BASE - 1), None, "本机号不是合成号");
    }

    #[test]
    fn a_screen_missing_from_a_screens_reply_keeps_the_last_one() {
        let round = Arc::new(AtomicUsize::new(0));
        let r_ = round.clone();
        let line = |t: &str| {
            vec![vec![crate::pty::ScreenSpan {
                text: t.into(),
                style: Default::default(),
            }]]
        };
        let r = remote(move |req| {
            Ok(match req {
                Request::List => Response::Sessions(vec![sess(1, "/a"), sess(2, "/b")]),
                Request::Screens { .. } => {
                    // 第一轮两张都给，之后只给 2 号（1 号「这轮装不下」）
                    let first = r_.fetch_add(1, Ordering::SeqCst) == 0;
                    let mut screens = vec![ScreenEntry {
                        id: 2,
                        lines: line("two"),
                    }];
                    if first {
                        screens.push(ScreenEntry {
                            id: 1,
                            lines: line("one"),
                        });
                    }
                    Response::Screens { screens }
                }
                _ => Response::Ok,
            })
        });
        let mut rig = rig(vec![], r, fast());
        rig.router.set_peers(&["mac".to_string()]);
        first_remote_id(&mut rig.router, "mac");
        let ids: Vec<u32> = sessions_of(&mut rig.router).iter().map(|s| s.id).collect();
        rig.router.watch(&ids);
        assert!(wait_up_to(Duration::from_secs(5), || round
            .load(Ordering::SeqCst)
            >= 3));
        let Response::Screens { screens } = rig
            .router
            .call(Request::Screens { ids: ids.clone() })
            .unwrap()
        else {
            panic!("Screens 该答 Screens");
        };
        let one = screens.iter().find(|s| s.id == ids[0]).unwrap();
        assert_eq!(one.lines[0][0].text, "one", "这轮没带回来的屏要留着上一张");
        assert!(rig.router.age(ids[0]).is_some());
        assert!(
            rig.local_seen
                .lock()
                .unwrap()
                .iter()
                .all(|s| !s.starts_with("Screens")),
            "全是远程 id 时不该去问本机"
        );
    }

    /// 被操作的那台去重缓存只有 64 格：每台电脑每秒最多 2 问，画面、列表、
    /// 按键加在一起。
    #[test]
    fn calls_to_one_peer_stay_within_two_per_second() {
        let r = remote(|req| {
            Ok(match req {
                Request::List => Response::Sessions(vec![sess(1, "/a")]),
                _ => Response::Ok,
            })
        });
        let mut rig = rig(vec![], r, Pace::default());
        rig.router.set_peers(&["mac".to_string()]);
        let id = first_remote_id(&mut rig.router, "mac");
        rig.router.watch(&[id]);
        let start = Instant::now();
        let before = rig.remote_seen.lock().unwrap().len();
        while start.elapsed() < Duration::from_millis(1600) {
            rig.router
                .call(Request::Input {
                    id,
                    text: "k".into(),
                })
                .unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        let n = rig.remote_seen.lock().unwrap().len() - before;
        // 1.6 秒、间隔 500ms：最多 4 问（起点那一问可能紧贴着列表那一问）
        assert!(n <= 4, "1.6 秒里问了 {n} 次");
    }

    #[test]
    fn create_on_a_peer_goes_through_its_feed_and_answers_with_a_synthetic_id() {
        let r = remote(|req| {
            Ok(match req {
                Request::List => Response::Sessions(vec![]),
                Request::Create { .. } => Response::Created { id: 42 },
                _ => Response::Ok,
            })
        });
        let mut rig = rig(vec![], r, fast());
        rig.router.set_peers(&["mac".to_string()]);
        rig.router
            .create_on("mac", "/r/proj".into(), "claude".into())
            .unwrap();
        let mut events = Vec::new();
        assert!(wait_up_to(Duration::from_secs(5), || {
            events.extend(rig.router.take_events());
            !events.is_empty()
        }));
        match &events[..] {
            [FeedEvent::Answer {
                peer,
                of: AnswerOf::Create,
                resp: Response::Created { id },
                ..
            }] => {
                assert_eq!(peer, "mac");
                assert!(*id >= REMOTE_BASE, "新会话的号要换成合成 id");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            rig.local_seen.lock().unwrap().is_empty(),
            "远程新建不经本机"
        );
        assert!(rig
            .router
            .create_on("pc", "/x".into(), "claude".into())
            .is_err());
    }

    #[test]
    fn remote_catalogs_are_fetched_when_first_asked() {
        let r = remote(|req| {
            Ok(match req {
                Request::List => Response::Sessions(vec![]),
                Request::Projects => Response::Projects {
                    recent: vec!["/r/a".into()],
                    pinned: vec![],
                },
                Request::Profiles { .. } => Response::Profiles {
                    entries: vec![],
                    warnings: vec![],
                },
                _ => Response::Ok,
            })
        });
        let mut rig = rig(vec![], r, fast());
        rig.router.set_peers(&["mac".to_string()]);
        assert!(
            rig.router.remote_projects("mac").is_none(),
            "第一次问只是去取"
        );
        assert!(wait_up_to(Duration::from_secs(5), || rig
            .router
            .remote_projects("mac")
            .is_some_and(|(recent, _)| recent == ["/r/a".to_string()])));
        assert!(rig.router.remote_profiles("mac").is_none());
        assert!(wait_up_to(Duration::from_secs(5), || rig
            .router
            .remote_profiles("mac")
            .is_some()));
    }

    #[test]
    fn an_image_paste_into_a_remote_session_is_refused() {
        let r = remote(|req| {
            Ok(match req {
                Request::List => Response::Sessions(vec![sess(1, "/a")]),
                _ => Response::Ok,
            })
        });
        let mut rig = rig(vec![], r, fast());
        rig.router.set_peers(&["mac".to_string()]);
        let id = first_remote_id(&mut rig.router, "mac");
        let err = rig.router.check_image_paste(id).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<crate::proto::CodedError>(),
            Some(crate::proto::CodedError(ErrorCode::RemoteForbidden))
        ));
        assert!(rig.router.check_image_paste(3).is_ok(), "本机会话照常粘图");
    }
}
