//! 多电脑：几台电脑上的智能体互相留言、派活。设计见
//! `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`。
//!
//! - `login`：跟网关换中转令牌、到期前续期；
//! - `store`：钥匙、电脑名、组名单落盘；
//! - `net`：往外发（`Net` trait，真的 `LinkNet` 和测试用的 `FakeHub`）；
//! - `group`：登录建组、加入、批准、移除这几个要跟别的电脑说话的流程；
//! - `deliver`：留言——`dct peers` 的详情、`dct send`、送进会话、忙时排队；
//! - `cli`：`dct login` / `dct join` / `dct peers` / `dct send`；
//! - 这里：`Mesh`，守护进程里这台电脑在组里的全部状态，以及电脑信封唯一的
//!   入口 `Mesh::on_envelope`。
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use dct_link::Envelope;
use dct_mesh::roster::{self, Member, SignedRoster};
use dct_mesh::seal::{self, Kind, Message, Sealed};
use dct_mesh::wire::{self, JoinRequest, Payload};
use dct_mesh::{id, sas, MachineKeys};

use crate::journal::Journal;
use crate::link::Handler;

pub mod cli;
pub mod deliver;
pub mod group;
pub mod login;
pub mod net;
pub mod store;

/// 中转的默认地址。`~/.dct/config.toml` 里 `[mesh] relay = "…"` 可以改。
///
/// 上线之前这个地址还连不上：连接线程会按退避一直重试，不刷屏、不报错，
/// 见 `link::Link::run`。
pub const DEFAULT_RELAY: &str = "https://dataclue.cn/dct-relay";

/// 电脑端点的前缀（`id::endpoint_for` 算出来的都长这样）。`Link` 收到的
/// 信封按它分流：电脑走 `Mesh`，别的走 proto 那一路。
pub const COMPUTER_PREFIX: &str = "c-";

/// 一条留言的 `sent_at` 跟我这边的时钟差多少以内才收。
///
/// `seal::open` 不防重放（见它的文档），这一层防：按 `(from, id)` 去重，
/// 再卡这个窗口。去重表是有界的，窗口保证一条被挤出表的旧 id 也回放不进来
/// ——只要表在窗口时间里装得下所有收到过的 id。
pub const SENT_AT_WINDOW_SECS: u64 = 10 * 60;

/// 去重表记多少条。
const SEEN_CAP: usize = 1024;

/// 一条加入请求等多久没人批就作废；新电脑那边也最多等这么久。
pub const JOIN_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// 最多同时挂几条等批准的加入请求。同账号里的一台电脑能一直发 `Join`，
/// 不设上限的话这张表就是一个谁都能灌的口子。
///
/// **满了就拒新的，不挤掉旧的。** 挤旧的话，攻击者灌一轮就能把用户正在
/// 核对的那条请求挤出去，再换上一条同名的冒牌货。
const MAX_PENDING: usize = 16;

/// 新电脑这边：一台回过我 `JoinPending` 的已有电脑。
#[derive(Debug, Clone)]
pub struct Invite {
    pub member: Member,
    /// 我给它算的 6 位数。
    pub code: String,
    /// 什么时候回的（`clock` 的 unix 秒）。过了 `JOIN_TTL` 作废。
    pub at: u64,
}

/// 一条等着投进会话的留言。
#[derive(Debug, Clone)]
pub struct QueuedMsg {
    pub msg: Message,
    /// 要敲进去的整段文字（标记加正文），收到时就算好。
    pub text: String,
    pub received_at: Instant,
}

/// 最近见过的 `(from, id)`，带着它的 `sent_at`。
///
/// 两种方式往外挤：按年龄（`sent_at` 已经落到窗口外面的，反正回放进来也会
/// 被窗口挡掉，不用再记），和按条数（`SEEN_CAP`，兜底，防一个发件人刷爆
/// 内存）。
#[derive(Default)]
struct Seen {
    order: VecDeque<(String, String, u64)>,
    set: HashSet<(String, String)>,
}

impl Seen {
    /// 第一次见返回 `true` 并记下；见过返回 `false`。
    fn first_time(&mut self, from: &str, id: &str, sent_at: u64, now: u64) -> bool {
        self.evict_older_than(now.saturating_sub(SENT_AT_WINDOW_SECS));
        let key = (from.to_string(), id.to_string());
        if self.set.contains(&key) {
            return false;
        }
        if self.order.len() >= SEEN_CAP {
            if let Some((f, i, _)) = self.order.pop_front() {
                self.set.remove(&(f, i));
            }
        }
        self.order
            .push_back((key.0.clone(), key.1.clone(), sent_at));
        self.set.insert(key);
        true
    }

    /// 扔掉 `sent_at` 早于 `cutoff` 的。`order` 按收到的顺序排，不按
    /// `sent_at` 排，所以要扫一遍而不是只看队头——一条未来时间的留言排在
    /// 前面，不该挡住后面已经过期的。
    fn evict_older_than(&mut self, cutoff: u64) {
        let set = &mut self.set;
        self.order.retain(|(f, i, t)| {
            let keep = *t >= cutoff;
            if !keep {
                set.remove(&(f.clone(), i.clone()));
            }
            keep
        });
    }
}

type Clock = Box<dyn Fn() -> u64 + Send>;
type Rand = Box<dyn Fn() -> [u8; 32] + Send>;

/// 守护进程里这台电脑在组里的状态。放在 `Arc<Mutex<..>>` 里共享。
pub struct Mesh {
    pub keys: MachineKeys,
    pub me: Member,
    pub roster: Option<SignedRoster>,
    /// 等人批准的加入请求：请求、6 位核对码、收到的时间。
    pub pending_joins: Vec<(JoinRequest, String, Instant)>,
    /// 这台电脑自己在请求加入：回过我 `JoinPending` 的已有电脑，和我给它
    /// 算的 6 位数。
    pub invites: Vec<Invite>,
    /// 用户核对过数字、认定的那一台（端点）。**只有它签的名单能成为我进组
    /// 的第一份**（`roster::accept_invite`）。回过话的电脑不止它一台时，其余
    /// 的谁都没被人核对过——中转可以塞进来一台自己的电脑，让它也回一句。
    pub confirmed: Option<String>,
    /// 用户还没认定之前，回过话的电脑先送来的名单。那边的人可能先点了同意，
    /// 这边的人还没敲名字；先存着，认定的那一刻再验。别的电脑送来的永远
    /// 用不上。每台最多一份。
    held: Vec<SignedRoster>,
    /// 按会话 id 排队的留言（`deliver`）。
    pub queues: HashMap<u32, VecDeque<QueuedMsg>>,
    /// 会话清单和敲字的那一头。没有（测试、还没接上）就不收留言。
    inbox: Option<Arc<dyn deliver::Inbox>>,
    /// 此刻正在（锁外）往里敲字的会话。标着的会话，新来的留言排到队里、
    /// 投递线程也不再给它送——一次只送一条，先来的先送。
    in_flight: HashSet<u32>,
    /// 这次运行期间送进每个会话的留言条数（`MeshView::messages`，看板上的
    /// 「✉ N」）。
    delivered: BTreeMap<u32, u32>,
    store: Option<store::Store>,
    journal: Arc<Journal>,
    seen: Seen,
    /// 这个进程从什么时候开始收（unix 秒）。`sent_at` 早于它的留言一律不收：
    /// 去重表只活在内存里，重启就空了，没有这一条，重启前收过的留言在窗口
    /// 之内可以原样再回放一遍。第一步没有离线留言，合法的东西不会因此丢。
    started_at: u64,
    clock: Clock,
    rand: Rand,
}

impl Mesh {
    /// `name` 是这台电脑想叫的名字；名单里已经有我，就以名单里那条记录为准
    /// （`added_at` 之类的字段是签进名单里的，自己另编一份就对不上了）。
    pub fn new(keys: MachineKeys, name: String, roster: Option<SignedRoster>) -> Mesh {
        let endpoint = id::endpoint_for(&keys.sign_pub());
        let me = roster
            .as_ref()
            .and_then(|r| r.roster.member(&endpoint).cloned())
            .unwrap_or_else(|| Member {
                name,
                endpoint,
                sign_pub: STANDARD.encode(keys.sign_pub()),
                kx_pub: STANDARD.encode(keys.kx_pub()),
                added_at: unix_now(),
            });
        Mesh {
            keys,
            me,
            roster,
            pending_joins: Vec::new(),
            invites: Vec::new(),
            confirmed: None,
            held: Vec::new(),
            queues: HashMap::new(),
            inbox: None,
            in_flight: HashSet::new(),
            delivered: BTreeMap::new(),
            store: None,
            journal: Arc::new(Journal::new()),
            seen: Seen::default(),
            started_at: unix_now(),
            clock: Box::new(unix_now),
            rand: Box::new(os_rand),
        }
    }

    /// 从磁盘读出（第一次就生成）钥匙、名字、名单。
    pub fn load(store: store::Store) -> anyhow::Result<Mesh> {
        let keys = store.load_or_create_keys(&os_rand)?;
        let name = store.name()?;
        let roster = store.roster()?;
        Ok(Mesh::new(keys, name, roster).with_store(store))
    }

    /// 名单变了就存到这里。没有 store（测试）就只改内存。
    pub fn with_store(mut self, s: store::Store) -> Mesh {
        self.store = Some(s);
        self
    }

    pub fn with_journal(mut self, j: Arc<Journal>) -> Mesh {
        self.journal = j;
        self
    }

    /// 换时钟。进程起点跟着换成这个时钟的「现在」——见 `started_at`。
    pub fn with_clock(mut self, c: impl Fn() -> u64 + Send + 'static) -> Mesh {
        self.started_at = c();
        self.clock = Box::new(c);
        self
    }

    /// 测试用：直接指定进程起点。
    #[cfg(test)]
    fn with_started_at(mut self, t: u64) -> Mesh {
        self.started_at = t;
        self
    }

    pub fn with_rand(mut self, r: impl Fn() -> [u8; 32] + Send + 'static) -> Mesh {
        self.rand = Box::new(r);
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.me.endpoint
    }

    /// 电脑信封的唯一入口。返回值是要回给发件方的 payload；`None` = 不回。
    ///
    /// **任何验证失败都是沉默**：丢掉、记一行 journal、不回任何东西。回一句
    /// 「签名不对」「你不在名单里」，就是在教伪造者下一次怎么改。
    ///
    /// 这是**攥着 `&mut self` 一口气做完**的版本（测试、没有别人抢锁的地方
    /// 用）。守护进程和 `FakeHub` 走的是 [`handle`]：往会话里敲字（里面有
    /// git 快照，能花好几秒）在放开 `Mesh` 锁之后才做。
    pub fn on_envelope(&mut self, env: &Envelope) -> Option<Vec<u8>> {
        match self.step(env) {
            Step::Done(r) => r,
            Step::Type(t) => {
                let r = match &self.inbox {
                    Some(i) => i.type_into(t.session, &t.text),
                    None => Err("no inbox".into()),
                };
                self.finish_incoming(&t, r)
            }
        }
    }

    /// 第一步，攥着锁做：验、去重、决定。要往会话里敲字的，只把「敲什么、
    /// 敲给谁」带出来（并把那个会话标成正在敲），不在这里敲。
    fn step(&mut self, env: &Envelope) -> Step {
        let payload = match wire::decode(&env.payload) {
            Ok(p) => p,
            Err(_) => return Step::Done(self.drop(env, "undecodable")),
        };
        match payload {
            Payload::Roster(r) => Step::Done(self.take_roster(env, r)),
            Payload::Sealed(s) => self.take_sealed(env, &s),
            Payload::Join(req) => Step::Done(self.take_join(env, req)),
            // `JoinPending` 只该作为 `ask` 的答复回来（`group::join` 在那里
            // 读它），不该从轮询里进来。
            Payload::JoinPending { .. } => Step::Done(self.drop(env, "unasked_join_pending")),
        }
    }

    /// 敲完之后（锁又拿回来了）：清掉「正在敲」，回回执。
    /// 看板上「✉ N」的底数。
    pub fn delivered_counts(&self) -> BTreeMap<u32, u32> {
        self.delivered.clone()
    }

    fn finish_incoming(
        &mut self,
        t: &deliver::Typing,
        r: Result<deliver::Typed, String>,
    ) -> Option<Vec<u8>> {
        let receipt = self.finish_typing(t, r);
        let body = serde_json::to_string(&receipt).unwrap_or_default();
        self.reply(&t.msg, Kind::Receipt, body)
    }

    /// 这台电脑还不跟任何别的电脑同组：没有名单，或者名单上只有自己
    /// （`dct login` 建的那一份）。只有这时候才能去加入别人的组。
    pub fn is_alone(&self) -> bool {
        match &self.roster {
            None => true,
            Some(r) => r.roster.members.iter().all(|m| m.endpoint == self.me.endpoint),
        }
    }

    /// 还没有名单就建一个只有自己的组（`dct login` 之后）。返回有没有新建。
    pub fn ensure_group(&mut self) -> anyhow::Result<bool> {
        if self.roster.is_some() {
            return Ok(false);
        }
        let g = roster::genesis(
            self.me.clone(),
            format!("mine-{}", self.me.endpoint),
            &self.keys,
        );
        self.commit(g)?;
        self.journal.mesh("group_created");
        Ok(true)
    }

    /// 改这台电脑的名字。名单上只有自己时，那份名单也按新名字重签一遍——
    /// 不然重启之后 `Mesh::new` 又会从名单里读回旧名字。已经跟别人同组时
    /// 不改：名单上的名字是别人签进去的。
    pub fn rename(&mut self, name: &str) -> Result<(), crate::proto::MeshProblem> {
        let name = name.trim();
        if !valid_name(name) {
            return Err(crate::proto::MeshProblem::BadName);
        }
        if !self.is_alone() {
            return Err(crate::proto::MeshProblem::AlreadyInGroup);
        }
        if let Some(s) = &self.store {
            s.set_name(name)
                .map_err(|_| crate::proto::MeshProblem::NotSaved)?;
        }
        self.me.name = name.to_string();
        if let Some(r) = &self.roster {
            let g = roster::genesis(self.me.clone(), r.roster.group.clone(), &self.keys);
            self.commit(g)
                .map_err(|_| crate::proto::MeshProblem::NotSaved)?;
        }
        Ok(())
    }

    /// 这台电脑请求加入时发出去的那一份：自己的成员记录，自己签名。
    pub fn join_request(&self) -> JoinRequest {
        JoinRequest {
            member: self.me.clone(),
            sig: wire::sign_member(&self.me, &self.keys),
        }
    }

    /// 换上一份已经验过的名单：先落盘，再换内存。存不下就不换（理由见
    /// `take_roster`）。已经在名单上的电脑，它的加入请求就不用再等了。
    fn commit(&mut self, r: SignedRoster) -> anyhow::Result<()> {
        if let Some(s) = &self.store {
            s.save_roster(&r)?;
        }
        if let Some(mine) = r.roster.member(&self.me.endpoint) {
            self.me = mine.clone();
        }
        self.pending_joins
            .retain(|(j, _, _)| r.roster.member(&j.member.endpoint).is_none());
        self.roster = Some(r);
        // 被移出组的电脑，它排着队的留言也一起作废。
        self.purge_non_members();
        Ok(())
    }

    /// 扔掉过期的加入请求。
    pub fn prune_pending(&mut self) {
        self.pending_joins.retain(|(_, _, t)| t.elapsed() < JOIN_TTL);
    }

    /// 一台电脑想加入：验它的自签名、确认它说的端点就是中转认证过的发件
    /// 端点，算出 6 位数挂起来等用户批，回一份我自己的自签成员记录——对方
    /// 拿它算同一个 6 位数。
    ///
    /// **这里不改名单。** 进名单只有一条路：用户看过两边的数字之后批准
    /// （`group::approve`）。
    fn take_join(&mut self, env: &Envelope, req: JoinRequest) -> Option<Vec<u8>> {
        let Some(current) = self.roster.as_ref() else {
            return self.drop(env, "join_without_group");
        };
        if req.member.endpoint != env.from.as_str() || !wire::verify_member(&req.member, &req.sig)
        {
            return self.drop(env, "join_bad_sig");
        }
        // 签得进名单的才挂：名字不合规、加密公钥解不出来的，批了也是白批。
        if !valid_name(&req.member.name) || !valid_kx_pub(&req.member.kx_pub) {
            return self.drop(env, "join_bad_member");
        }
        if current.roster.member(&req.member.endpoint).is_some() {
            return self.drop(env, "join_already_member");
        }
        let code = sas::code(&self.me, &req.member);
        self.prune_pending();
        let again = self
            .pending_joins
            .iter()
            .any(|(j, _, _)| j.member.endpoint == req.member.endpoint);
        if !again && self.pending_joins.len() >= MAX_PENDING {
            return self.drop(env, "join_pending_full");
        }
        self.pending_joins
            .retain(|(j, _, _)| j.member.endpoint != req.member.endpoint);
        self.journal
            .mesh(&format!("join_pending from={}", env.from));
        self.pending_joins.push((req, code, Instant::now()));
        Some(wire::encode(&Payload::JoinPending {
            member: self.me.clone(),
            sig: wire::sign_member(&self.me, &self.keys),
        }))
    }

    /// 换一批回过话的电脑（新的一次 `dct join`）。之前认定的、存着的都作废。
    pub fn set_invites(&mut self, found: Vec<(Member, String)>) {
        let at = (self.clock)();
        self.invites = found
            .into_iter()
            .map(|(member, code)| Invite { member, code, at })
            .collect();
        self.confirmed = None;
        self.held.clear();
    }

    /// 扔掉过期的邀请，以及跟着它们的认定。（存着的名单不用跟着清：它只在
    /// `confirm_inviter` 里、那台的邀请还在时才会被拿出来验。）
    pub fn prune_invites(&mut self) {
        let now = (self.clock)();
        self.invites
            .retain(|i| now.saturating_sub(i.at) < JOIN_TTL.as_secs());
        let live: Vec<String> = self.invites.iter().map(|i| i.member.endpoint.clone()).collect();
        if self.confirmed.as_ref().is_some_and(|c| !live.contains(c)) {
            self.confirmed = None;
        }
    }

    /// 用户说「是这一台，数字一样」。它先前送来过名单的话，现在验。
    ///
    /// 认定不了时，错误里带的是给人看的电脑名（过期之前记得住的话），不是
    /// 端点——用户敲的就是名字。
    pub fn confirm_inviter(&mut self, endpoint: &str) -> Result<(), crate::proto::MeshProblem> {
        let name = self
            .invites
            .iter()
            .find(|i| i.member.endpoint == endpoint)
            .map(|i| i.member.name.clone())
            .unwrap_or_else(|| endpoint.to_string());
        self.prune_invites();
        if !self.invites.iter().any(|i| i.member.endpoint == endpoint) {
            return Err(crate::proto::MeshProblem::NoSuchInviter(name));
        }
        self.confirmed = Some(endpoint.to_string());
        if let Some(i) = self.held.iter().position(|r| r.signer == endpoint) {
            let r = self.held.remove(i);
            self.take_invite(endpoint, r);
        }
        Ok(())
    }

    fn take_roster(&mut self, env: &Envelope, incoming: SignedRoster) -> Option<Vec<u8>> {
        // 我正在请求加入：只有用户认定的那台签的名单走 `accept_invite`；
        // 别的回过话的电脑送来的，先存着（用户可能还没认定），永远不直接收。
        if self.is_alone() {
            self.prune_invites();
            if self.confirmed.as_deref() == Some(incoming.signer.as_str()) {
                self.take_invite(env.from.as_str(), incoming);
                return None;
            }
            if self.invites.iter().any(|i| i.member.endpoint == incoming.signer) {
                // `signer` 只是名单里的一个字段，还没验过。按它存的话，同账号
                // 里随便一台电脑送一份写着「signer = A」的垃圾，就能把 A 真正
                // 送来的那份顶掉，用户认定 A 的那一刻什么也验不出来。所以
                // 只存**签名者亲自送来的**：中转认证过发件端点，冒不了。
                if env.from.as_str() != incoming.signer {
                    return self.drop(env, "invite_not_from_signer");
                }
                self.journal
                    .mesh(&format!("invite_held from={}", env.from));
                self.held.retain(|r| r.signer != incoming.signer);
                self.held.push(incoming);
                return None;
            }
        }
        // 还没在任何组里的电脑，**不从网上接一份名单当自己的第一份**：
        // `accept(None, ..)` 只查「是一份自签的创世名单」，同账号里谁都造得
        // 出一份。加入别人的组走上面那条 `accept_invite`，它还要核对签名者
        // 就是给我算过 6 位数的那台电脑。
        let Some(current) = self.roster.as_ref() else {
            return self.drop(env, "roster_without_group");
        };
        if let Err(e) = roster::accept(Some(current), &incoming) {
            return self.drop(env, &format!("roster_{e:?}"));
        }
        if let Err(e) = self.commit(incoming) {
            // 存不下就不换：内存跟磁盘各说一套的话，重启之后会退回旧名单，
            // 而那时候谁也不记得发生过什么。对方下一次广播会再送来。
            self.journal
                .mesh(&format!("roster_not_saved from={} err={e}", env.from));
        }
        None
    }

    /// 验、收一份邀请名单。`incoming.signer` 必须是用户认定的那台（调用方
    /// 已经对过）。
    fn take_invite(&mut self, from: &str, incoming: SignedRoster) {
        let Some(inviter) = self
            .invites
            .iter()
            .find(|i| Some(&i.member.endpoint) == self.confirmed.as_ref())
            .map(|i| i.member.clone())
        else {
            return self.drop_from(from, "invite_not_confirmed");
        };
        if let Err(e) = roster::accept_invite(&incoming, &self.me.endpoint, &inviter) {
            return self.drop_from(from, &format!("invite_{e:?}"));
        }
        // `accept_invite` 只拿得到我的端点（它绑着签名公钥）；我那一条的
        // 加密公钥对不对，只有我自己知道。换成别的，发给我的东西我就解不开，
        // 而能解开的是别人。
        let mine = incoming.roster.member(&self.me.endpoint);
        if mine.map(|m| (&m.sign_pub, &m.kx_pub)) != Some((&self.me.sign_pub, &self.me.kx_pub)) {
            return self.drop_from(from, "invite_not_my_keys");
        }
        match self.commit(incoming) {
            Ok(()) => {
                self.invites.clear();
                self.confirmed = None;
                self.held.clear();
                self.journal.mesh(&format!("joined via={from}"));
            }
            Err(e) => self
                .journal
                .mesh(&format!("roster_not_saved from={from} err={e}")),
        }
    }

    fn take_sealed(&mut self, env: &Envelope, s: &Sealed) -> Step {
        let Some(current) = self.roster.as_ref() else {
            return Step::Done(self.drop(env, "sealed_without_group"));
        };
        let m = match seal::open(s, &self.keys, &current.roster, env.from.as_str()) {
            Ok(m) => m,
            Err(e) => return Step::Done(self.drop(env, &format!("sealed_{e:?}"))),
        };
        let now = (self.clock)();
        if now.abs_diff(m.sent_at) > SENT_AT_WINDOW_SECS {
            return Step::Done(self.drop(env, "stale"));
        }
        if m.sent_at < self.started_at {
            return Step::Done(self.drop(env, "before_start"));
        }
        if !self.seen.first_time(&m.from, &m.id, m.sent_at, now) {
            return Step::Done(self.drop(env, "replay"));
        }
        match m.kind {
            // 上面的名单、签名、时间窗、去重全都过了，才走到这里：同一条
            // 留言再来一次在 `seen` 那里就停了，不会被敲进会话两次。
            Kind::Msg => match self.decide(&m) {
                deliver::Decision::Answer(r) => {
                    let body = serde_json::to_string(&r).unwrap_or_default();
                    Step::Done(self.reply(&m, Kind::Receipt, body))
                }
                deliver::Decision::TypeNow(t) => Step::Type(t),
            },
            Kind::StatusRequest => {
                let body = serde_json::to_string(&self.local_status()).unwrap_or_default();
                Step::Done(self.reply(&m, Kind::Status, body))
            }
            // 回执和状态只该作为 `ask` 的答复回来，不该从轮询里进来；进来了
            // 也不回——回的话两台电脑能互相回到天荒地老。
            Kind::Receipt | Kind::Status => Step::Done(None),
        }
    }

    /// 给 `to_what` 的发件人回一条加密的 `kind`。`id` 跟原消息一样，对方靠它配对。
    fn reply(&self, to_what: &Message, kind: Kind, body: String) -> Option<Vec<u8>> {
        let roster = self.roster.as_ref()?;
        let peer = roster.roster.member(&to_what.from)?;
        let kx: [u8; 32] = STANDARD.decode(&peer.kx_pub).ok()?.try_into().ok()?;
        let m = Message {
            id: to_what.id.clone(),
            kind,
            from: self.me.endpoint.clone(),
            from_session: to_what.to_session.clone(),
            to: to_what.from.clone(),
            to_session: to_what.from_session.clone(),
            body,
            sent_at: (self.clock)(),
        };
        let sealed = seal::seal(&m, &self.keys, &kx, (self.rand)());
        Some(wire::encode(&Payload::Sealed(sealed)))
    }

    fn drop(&self, env: &Envelope, why: &str) -> Option<Vec<u8>> {
        self.drop_from(env.from.as_str(), why);
        None
    }

    fn drop_from(&self, from: &str, why: &str) {
        self.journal.mesh(&format!("dropped from={from} why={why}"));
    }
}

/// 名单接受的名字（同 `roster` 里的规则、同 `store::set_name`）。
pub(crate) fn valid_name(n: &str) -> bool {
    !n.is_empty() && !n.contains('/') && n.chars().count() <= roster::MAX_NAME_LEN
}

fn valid_kx_pub(b64: &str) -> bool {
    STANDARD.decode(b64).map(|b| b.len() == 32).unwrap_or(false)
}

/// `on_envelope` 的第一步出来的东西：直接回的答复，或者一件要放了锁再做
/// 的「往会话里敲字」。
enum Step {
    Done(Option<Vec<u8>>),
    Type(deliver::Typing),
}

/// 一个电脑信封，锁分两段拿：验、决定在锁里（`Mesh::step`）；**往会话里敲字
/// 在锁外**——`type_into` 里有 git 快照，能花好几秒，那段时间里别的请求
/// （`dct peers`、投递线程、下一个信封）不该跟着等；敲完再拿锁回回执。
pub fn handle(mesh: &Mutex<Mesh>, env: &Envelope) -> Option<Vec<u8>> {
    let lock = || mesh.lock().unwrap_or_else(|e| e.into_inner());
    let (step, inbox) = {
        let mut m = lock();
        (m.step(env), m.inbox.clone())
    };
    match step {
        Step::Done(r) => r,
        Step::Type(t) => {
            // 敲字半路 panic 也得把「正在敲」清掉，见 `deliver::InFlight`。
            let guard = deliver::InFlight::new(mesh, t.session);
            let r = match &inbox {
                Some(i) => i.type_into(t.session, &t.text),
                None => Err("no inbox".into()),
            };
            guard.finish(|m| m.finish_incoming(&t, r))
        }
    }
}

/// `Link` 收到的信封怎么分：`c-` 开头的是电脑，交给 `Mesh`；别的交给
/// `proto`（手机那一路）。`proto` 是 `None` 就不回——这一步手机那一路还没
/// 接到中转上，但路留着。
pub fn route(mesh: Arc<Mutex<Mesh>>, proto: Option<Handler>) -> Handler {
    Arc::new(move |env: &Envelope| {
        if env.from.as_str().starts_with(COMPUTER_PREFIX) {
            // 一封信封处理到一半 panic（比如敲字那一下）不能把整条连接线
            // 带走：`link::spawn` 的 `catch_unwind` 包的是整个 `run`，线
            // 就停了，这台电脑从此收不到任何东西。这一封不回就是了。
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle(&mesh, env)))
                .unwrap_or(None)
        } else {
            proto.as_ref().and_then(|h| h(env))
        }
    })
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 操作系统的 CSPRNG。拿不到就没法造钥匙、没法加密，继续下去只会更糟。
pub fn os_rand() -> [u8; 32] {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).expect("系统随机数不可用");
    b
}

#[cfg(test)]
mod tests {
    use super::net::testing::FakeHub;
    use super::net::Net;
    use super::*;
    use dct_link::EndpointId;
    use std::time::Duration;

    const NOW: u64 = 1_800_000_000;

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

    /// A 建组、把 B 签进去（version 2）。返回两台都拿着 v2 名单的 `Mesh`。
    fn pair_ab() -> (Mesh, Mesh, SignedRoster) {
        let (ka, kb) = (keys(1), keys(2));
        let (ma, mb) = (member(&ka, "A"), member(&kb, "B"));
        let g = roster::genesis(ma.clone(), "mine".into(), &ka);
        let mut r2 = g.roster.clone();
        r2.version = 2;
        r2.members.push(mb);
        let v2 = roster::sign(r2, &ma, &ka);
        // 进程起点放在窗口之前，窗口那几条测试才测得到窗口本身。
        let a = Mesh::new(ka, "A".into(), Some(v2.clone()))
            .with_clock(|| NOW)
            .with_started_at(NOW - 2 * SENT_AT_WINDOW_SECS);
        // B 上有一个在忙的智能体会话 #7：留言进来就排队（`deliver` 的规矩），
        // 这里的测试看的是排没排进去、排了几次。
        let inbox = deliver::testing::FakeInbox::new();
        inbox.add(7, "", "/w/proj", crate::session::SessionState::Working, true);
        let b = Mesh::new(kb, "B".into(), Some(v2.clone()))
            .with_clock(|| NOW)
            .with_started_at(NOW - 2 * SENT_AT_WINDOW_SECS)
            .with_inbox(inbox);
        (a, b, v2)
    }

    fn env(from: &str, to: &str, payload: Vec<u8>) -> Envelope {
        Envelope {
            from: EndpointId::new(from).unwrap(),
            to: EndpointId::new(to).unwrap(),
            seq: 1,
            payload,
            recipients: vec![],
        }
    }

    fn msg(from: &Mesh, to: &Mesh, kind: Kind, id: &str, sent_at: u64) -> Message {
        Message {
            id: id.into(),
            kind,
            from: from.endpoint().into(),
            from_session: "#3".into(),
            to: to.endpoint().into(),
            to_session: "#7".into(),
            body: "hi".into(),
            sent_at,
        }
    }

    fn sealed_env(from: &Mesh, to: &Mesh, m: &Message) -> Envelope {
        let s = seal::seal(m, &from.keys, &to.keys.kx_pub(), [42; 32]);
        env(
            from.endpoint(),
            to.endpoint(),
            wire::encode(&Payload::Sealed(s)),
        )
    }

    fn open_reply(me: &Mesh, from: &Mesh, payload: &[u8]) -> Message {
        let Payload::Sealed(s) = wire::decode(payload).unwrap() else {
            panic!("回的该是密封的");
        };
        seal::open(
            &s,
            &me.keys,
            &me.roster.as_ref().unwrap().roster,
            from.endpoint(),
        )
        .unwrap()
    }

    fn journal() -> (tempfile::TempDir, Arc<Journal>, std::path::PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("sessions.log");
        let j = Journal::new();
        j.set_path(p.clone());
        (t, Arc::new(j), p)
    }

    #[test]
    fn a_message_from_a_member_is_queued_and_answered_with_a_sealed_receipt() {
        let (a, mut b, _) = pair_ab();
        let m = msg(&a, &b, Kind::Msg, "m1", NOW);
        let reply = b.on_envelope(&sealed_env(&a, &b, &m)).expect("该回回执");
        let r = open_reply(&a, &b, &reply);
        assert_eq!(r.kind, Kind::Receipt);
        assert_eq!(r.id, "m1");
        assert_eq!(r.to_session, "#3");
        let q = &b.queues[&7];
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].msg, m);
    }

    #[test]
    fn a_status_request_is_answered_with_an_encrypted_status() {
        let (a, mut b, _) = pair_ab();
        let m = msg(&a, &b, Kind::StatusRequest, "s1", NOW);
        let reply = b.on_envelope(&sealed_env(&a, &b, &m)).unwrap();
        let r = open_reply(&a, &b, &reply);
        assert_eq!(r.kind, Kind::Status);
        assert!(r.body.contains(std::env::consts::OS));
        assert!(b.queues.is_empty(), "状态查询不进排队");
    }

    #[test]
    fn a_sealed_message_from_a_machine_not_in_the_roster_is_dropped_without_reply() {
        let (_a, mut b, _) = pair_ab();
        let (t, j, path) = journal();
        b = b.with_journal(j);
        // 一台不在名单里的电脑：它自己的钥匙签、用 B 的真公钥加密——密码学上
        // 一切都对，只是名单里没有它。
        let stranger = Mesh::new(keys(9), "X".into(), None).with_clock(|| NOW);
        let m = msg(&stranger, &b, Kind::Msg, "x1", NOW);
        assert_eq!(b.on_envelope(&sealed_env(&stranger, &b, &m)), None);
        assert!(b.queues.is_empty());
        let log = std::fs::read_to_string(&path).unwrap();
        assert_eq!(log.lines().count(), 1, "丢掉要留一行痕迹：{log}");
        assert!(log.contains("dropped") && log.contains(stranger.endpoint()));
        drop(t);
    }

    #[test]
    fn a_replayed_message_is_dropped_the_second_time() {
        let (a, mut b, _) = pair_ab();
        let e = sealed_env(&a, &b, &msg(&a, &b, Kind::Msg, "m1", NOW));
        assert!(b.on_envelope(&e).is_some());
        assert_eq!(b.on_envelope(&e), None, "同一条再来一次该被丢掉");
        assert_eq!(b.queues[&7].len(), 1, "也不能排两次");
    }

    #[test]
    fn a_message_outside_the_time_window_is_dropped() {
        let (a, mut b, _) = pair_ab();
        let old = msg(&a, &b, Kind::Msg, "old", NOW - SENT_AT_WINDOW_SECS - 1);
        assert_eq!(b.on_envelope(&sealed_env(&a, &b, &old)), None);
        let future = msg(&a, &b, Kind::Msg, "fut", NOW + SENT_AT_WINDOW_SECS + 1);
        assert_eq!(b.on_envelope(&sealed_env(&a, &b, &future)), None);
        let edge = msg(&a, &b, Kind::Msg, "edge", NOW - SENT_AT_WINDOW_SECS);
        assert!(
            b.on_envelope(&sealed_env(&a, &b, &edge)).is_some(),
            "窗口边上还该收"
        );
    }

    #[test]
    fn the_seen_set_is_bounded() {
        let mut s = Seen::default();
        for i in 0..SEEN_CAP + 10 {
            assert!(s.first_time("c-a", &i.to_string(), NOW, NOW));
        }
        assert_eq!(s.order.len(), SEEN_CAP);
        assert_eq!(s.set.len(), SEEN_CAP);
        assert!(!s.first_time("c-a", &(SEEN_CAP + 9).to_string(), NOW, NOW));
        assert!(s.first_time("c-a", "0", NOW, NOW), "最老的已经被挤出去了");
        assert!(s.first_time("c-b", "5", NOW, NOW), "去重键里有 from");
    }

    /// 按年龄挤：`sent_at` 落到窗口外的条目被清掉，窗口内的留着。
    #[test]
    fn the_seen_set_forgets_entries_older_than_the_window() {
        let mut s = Seen::default();
        assert!(s.first_time("c-a", "old", NOW, NOW));
        // 一条未来时间的排在中间，不该挡住后面过期条目的清理。
        assert!(s.first_time("c-a", "future", NOW + 500, NOW));
        assert!(s.first_time("c-a", "young", NOW + 100, NOW + 100));
        let later = NOW + SENT_AT_WINDOW_SECS + 1;
        assert!(s.first_time("c-a", "x", later, later));
        let ids: Vec<&str> = s.order.iter().map(|(_, i, _)| i.as_str()).collect();
        assert_eq!(ids, ["future", "young", "x"]);
        assert_eq!(s.set.len(), 3);
    }

    /// 重启之后，去重表是空的——窗口之内的旧留言靠「早于进程起点」挡住。
    #[test]
    fn a_message_accepted_before_a_restart_is_refused_after_it() {
        let (a, mut b, v2) = pair_ab();
        let e = sealed_env(&a, &b, &msg(&a, &b, Kind::Msg, "m1", NOW));
        assert!(b.on_envelope(&e).is_some());

        // 「重启」：同一把钥匙、同一份名单，一个新的 Mesh，起点在 NOW 之后。
        let restarted_keys = keys(2);
        let mut b2 = Mesh::new(restarted_keys, "B".into(), Some(v2)).with_clock(|| NOW + 30);
        assert_eq!(b2.endpoint(), b.endpoint());
        assert_eq!(b2.on_envelope(&e), None, "重启前收过的不该再收一次");
        assert!(b2.queues.is_empty());

        // 重启之后新发的照收。
        let fresh = sealed_env(&a, &b2, &msg(&a, &b2, Kind::Msg, "m2", NOW + 30));
        assert!(b2.on_envelope(&fresh).is_some());
    }

    #[test]
    fn a_roster_with_a_bad_signature_is_dropped_and_the_old_one_kept() {
        let (a, mut b, v2) = pair_ab();
        let mut r3 = v2.roster.clone();
        r3.version = 3;
        r3.members.retain(|m| m.name != "B");
        let mut forged = roster::sign(r3, &a.me, &a.keys);
        // 签名换成另一把钥匙签的：形状完全合法，只是验不过。
        forged.sig = roster::sign(forged.roster.clone(), &a.me, &keys(9)).sig;
        let e = env(
            a.endpoint(),
            b.endpoint(),
            wire::encode(&Payload::Roster(forged)),
        );
        assert_eq!(b.on_envelope(&e), None);
        assert_eq!(b.roster, Some(v2));
    }

    #[test]
    fn a_valid_newer_roster_is_taken_and_saved() {
        let (a, b, v2) = pair_ab();
        let t = tempfile::tempdir().unwrap();
        let st = store::Store::at(t.path().join("mesh"));
        st.save_roster(&v2).unwrap();
        let mut b = b.with_store(store::Store::at(t.path().join("mesh")));
        let mut r3 = v2.roster.clone();
        r3.version = 3;
        r3.members.push(member(&keys(3), "C"));
        let v3 = roster::sign(r3, &a.me, &a.keys);
        let e = env(
            a.endpoint(),
            b.endpoint(),
            wire::encode(&Payload::Roster(v3.clone())),
        );
        assert_eq!(b.on_envelope(&e), None);
        assert_eq!(b.roster, Some(v3.clone()));
        assert_eq!(st.roster().unwrap(), Some(v3));
    }

    /// 没在组里的电脑不从网上接第一份名单——哪怕它是一份合法的创世名单。
    #[test]
    fn a_machine_with_no_group_does_not_take_a_roster_off_the_wire() {
        let stranger_keys = keys(9);
        let g = roster::genesis(member(&stranger_keys, "X"), "evil".into(), &stranger_keys);
        let mut lone = Mesh::new(keys(4), "L".into(), None);
        let e = env(
            &g.signer,
            lone.endpoint(),
            wire::encode(&Payload::Roster(g.clone())),
        );
        assert_eq!(lone.on_envelope(&e), None);
        assert_eq!(lone.roster, None);
    }

    #[test]
    fn garbage_is_dropped_silently() {
        let (a, mut b, _) = pair_ab();
        assert_eq!(
            b.on_envelope(&env(a.endpoint(), b.endpoint(), b"{nope".to_vec())),
            None
        );
    }

    #[test]
    fn computer_envelopes_go_to_the_mesh_and_others_to_proto() {
        let (a, b, _) = pair_ab();
        let a_ep = a.endpoint().to_string();
        let b_ep = b.endpoint().to_string();
        let b = Arc::new(Mutex::new(b));
        let proto_calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let pc = proto_calls.clone();
        let h = route(
            b.clone(),
            Some(Arc::new(move |e: &Envelope| {
                pc.lock().unwrap().push(e.from.to_string());
                Some(b"proto".to_vec())
            })),
        );

        // 电脑来的：Mesh 回了一个密封回执，proto 没被碰。
        let m = msg(&a, &b.lock().unwrap(), Kind::Msg, "m1", NOW);
        let e = sealed_env(&a, &b.lock().unwrap(), &m);
        let out = h(&e).unwrap();
        assert!(matches!(wire::decode(&out), Ok(Payload::Sealed(_))));
        assert!(proto_calls.lock().unwrap().is_empty());
        assert_eq!(b.lock().unwrap().queues[&7].len(), 1);

        // 手机来的：走 proto。
        assert_eq!(
            h(&env("phone:1", &b_ep, b"{}".to_vec())),
            Some(b"proto".to_vec())
        );
        assert_eq!(
            proto_calls.lock().unwrap().as_slice(),
            &["phone:1".to_string()]
        );

        // 没有 proto：不回，也不交给 Mesh。
        let h2 = route(b.clone(), None);
        assert_eq!(h2(&env("phone:1", &b_ep, b"{}".to_vec())), None);
        let _ = a_ep;
    }

    /// `FakeHub` 本身：两台内存里的电脑隔着它 `ask`，拿回对方的回执；
    /// `peers` 只列别人；下线的收不到。
    #[test]
    fn two_meshes_talk_through_the_fake_hub() {
        let (a, b, _) = pair_ab();
        let a_ep = a.endpoint().to_string();
        let b_ep = b.endpoint().to_string();
        let m = msg(&a, &b, Kind::Msg, "m1", NOW);
        let sealed = seal::seal(&m, &a.keys, &b.keys.kx_pub(), [5; 32]);
        let a = Arc::new(Mutex::new(a));
        let b = Arc::new(Mutex::new(b));
        let hub = FakeHub::new();
        hub.register(a.clone());
        hub.register(b.clone());
        let net = hub.net_for(&a_ep);

        assert_eq!(net.peers().unwrap(), vec![b_ep.clone()]);

        let reply = net
            .ask(
                &b_ep,
                wire::encode(&Payload::Sealed(sealed)),
                Duration::from_secs(3),
            )
            .unwrap();
        let r = open_reply(&a.lock().unwrap(), &b.lock().unwrap(), &reply);
        assert_eq!(r.kind, Kind::Receipt);

        hub.set_online(&b_ep, false);
        assert!(net.peers().unwrap().is_empty());
        assert_eq!(
            net.send(&b_ep, vec![]),
            Err(crate::link::LinkError::Relay(dct_link::LinkError::Offline))
        );
    }

    #[test]
    fn a_new_mesh_takes_its_own_record_from_the_roster_when_it_is_in_one() {
        let (a, _b, v2) = pair_ab();
        let again = Mesh::new(keys(1), "改了名但名单里还是 A".into(), Some(v2));
        assert_eq!(again.me, a.me);
    }

    #[test]
    fn load_creates_keys_once_and_keeps_the_endpoint() {
        let t = tempfile::tempdir().unwrap();
        let first = Mesh::load(store::Store::at(t.path().join("mesh"))).unwrap();
        let second = Mesh::load(store::Store::at(t.path().join("mesh"))).unwrap();
        assert_eq!(first.endpoint(), second.endpoint());
        assert!(first.endpoint().starts_with(COMPUTER_PREFIX));
    }
}
