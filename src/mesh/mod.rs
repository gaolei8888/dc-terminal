//! 多电脑：几台电脑上的智能体互相留言、派活。设计见
//! `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`。
//!
//! - `login`：跟网关换中转令牌、到期前续期；
//! - `store`：钥匙、电脑名、组名单落盘；
//! - `net`：往外发（`Net` trait，真的 `LinkNet` 和测试用的 `FakeHub`）；
//! - 这里：`Mesh`，守护进程里这台电脑在组里的全部状态，以及电脑信封唯一的
//!   入口 `Mesh::on_envelope`。
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use dct_link::Envelope;
use dct_mesh::roster::{self, Member, SignedRoster};
use dct_mesh::seal::{self, Kind, Message, Sealed};
use dct_mesh::wire::{self, JoinRequest, Payload};
use dct_mesh::{id, MachineKeys};

use crate::journal::Journal;
use crate::link::Handler;

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

/// 还没解析到具体会话的留言放在这个键下面。会话 id 从 1 数起，0 不会撞上。
/// Task 7 按会话名解析、投递。
pub const UNRESOLVED_SESSION: u32 = 0;

/// 一条等着投进会话的留言。
#[derive(Debug, Clone)]
pub struct QueuedMsg {
    pub msg: Message,
    pub received_at: Instant,
}

/// 最近见过的 `(from, id)`，先进先出、有上限。
#[derive(Default)]
struct Seen {
    order: VecDeque<(String, String)>,
    set: HashSet<(String, String)>,
}

impl Seen {
    /// 第一次见返回 `true` 并记下；见过返回 `false`。
    fn first_time(&mut self, from: &str, id: &str) -> bool {
        let key = (from.to_string(), id.to_string());
        if self.set.contains(&key) {
            return false;
        }
        if self.order.len() >= SEEN_CAP {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        self.order.push_back(key.clone());
        self.set.insert(key);
        true
    }
}

type Clock = Box<dyn Fn() -> u64 + Send>;
type Rand = Box<dyn Fn() -> [u8; 32] + Send>;

/// 守护进程里这台电脑在组里的状态。放在 `Arc<Mutex<..>>` 里共享。
pub struct Mesh {
    pub keys: MachineKeys,
    pub me: Member,
    pub roster: Option<SignedRoster>,
    /// 等人批准的加入请求：请求、6 位核对码、收到的时间。Task 6 填。
    pub pending_joins: Vec<(JoinRequest, String, Instant)>,
    /// 按会话 id 排队的留言。Task 7 投递。
    pub queues: HashMap<u32, VecDeque<QueuedMsg>>,
    store: Option<store::Store>,
    journal: Arc<Journal>,
    seen: Seen,
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
            queues: HashMap::new(),
            store: None,
            journal: Arc::new(Journal::new()),
            seen: Seen::default(),
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

    pub fn with_clock(mut self, c: impl Fn() -> u64 + Send + 'static) -> Mesh {
        self.clock = Box::new(c);
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
    pub fn on_envelope(&mut self, env: &Envelope) -> Option<Vec<u8>> {
        let payload = match wire::decode(&env.payload) {
            Ok(p) => p,
            Err(_) => return self.drop(env, "undecodable"),
        };
        match payload {
            Payload::Roster(r) => self.take_roster(env, r),
            Payload::Sealed(s) => self.take_sealed(env, &s),
            // 加入这一路是 Task 6 的事；在那之前收到就当没收到。
            Payload::Join(_) | Payload::JoinPending { .. } => None,
        }
    }

    fn take_roster(&mut self, env: &Envelope, incoming: SignedRoster) -> Option<Vec<u8>> {
        // 还没在任何组里的电脑，**不从网上接一份名单当自己的第一份**：
        // `accept(None, ..)` 只查「是一份自签的创世名单」，同账号里谁都造得
        // 出一份。加入别人的组要走 `accept_invite`（Task 6），它还要核对
        // 签名者就是给我算过 6 位数的那台电脑。
        let Some(current) = self.roster.as_ref() else {
            return self.drop(env, "roster_without_group");
        };
        if let Err(e) = roster::accept(Some(current), &incoming) {
            return self.drop(env, &format!("roster_{e:?}"));
        }
        if let Some(s) = &self.store {
            if let Err(e) = s.save_roster(&incoming) {
                // 存不下就不换：内存跟磁盘各说一套的话，重启之后会退回旧名单，
                // 而那时候谁也不记得发生过什么。对方下一次广播会再送来。
                self.journal
                    .mesh(&format!("roster_not_saved from={} err={e}", env.from));
                return None;
            }
        }
        if let Some(mine) = incoming.roster.member(&self.me.endpoint) {
            self.me = mine.clone();
        }
        self.roster = Some(incoming);
        None
    }

    fn take_sealed(&mut self, env: &Envelope, s: &Sealed) -> Option<Vec<u8>> {
        let Some(current) = self.roster.as_ref() else {
            return self.drop(env, "sealed_without_group");
        };
        let m = match seal::open(s, &self.keys, &current.roster, env.from.as_str()) {
            Ok(m) => m,
            Err(e) => return self.drop(env, &format!("sealed_{e:?}")),
        };
        let now = (self.clock)();
        if now.abs_diff(m.sent_at) > SENT_AT_WINDOW_SECS {
            return self.drop(env, "stale");
        }
        if !self.seen.first_time(&m.from, &m.id) {
            return self.drop(env, "replay");
        }
        match m.kind {
            Kind::Msg => {
                let key = session_key(&m.to_session);
                self.queues.entry(key).or_default().push_back(QueuedMsg {
                    msg: m.clone(),
                    received_at: Instant::now(),
                });
                self.reply(&m, Kind::Receipt, "queued".to_string())
            }
            Kind::StatusRequest => {
                let body = serde_json::json!({ "os": std::env::consts::OS }).to_string();
                self.reply(&m, Kind::Status, body)
            }
            // 回执和状态只该作为 `ask` 的答复回来，不该从轮询里进来；进来了
            // 也不回——回的话两台电脑能互相回到天荒地老。
            Kind::Receipt | Kind::Status => None,
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
        self.journal
            .mesh(&format!("dropped from={} why={why}", env.from));
        None
    }
}

/// `#12` 这种写法直接就是会话 id；会话名要等 Task 7 按看板上的名字解析。
fn session_key(to_session: &str) -> u32 {
    to_session
        .strip_prefix('#')
        .and_then(|n| n.parse().ok())
        .filter(|n| *n != UNRESOLVED_SESSION)
        .unwrap_or(UNRESOLVED_SESSION)
}

/// `Link` 收到的信封怎么分：`c-` 开头的是电脑，交给 `Mesh`；别的交给
/// `proto`（手机那一路）。`proto` 是 `None` 就不回——这一步手机那一路还没
/// 接到中转上，但路留着。
pub fn route(mesh: Arc<Mutex<Mesh>>, proto: Option<Handler>) -> Handler {
    Arc::new(move |env: &Envelope| {
        if env.from.as_str().starts_with(COMPUTER_PREFIX) {
            mesh.lock()
                .unwrap_or_else(|e| e.into_inner())
                .on_envelope(env)
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
        let a = Mesh::new(ka, "A".into(), Some(v2.clone())).with_clock(|| NOW);
        let b = Mesh::new(kb, "B".into(), Some(v2.clone())).with_clock(|| NOW);
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
            assert!(s.first_time("c-a", &i.to_string()));
        }
        assert_eq!(s.order.len(), SEEN_CAP);
        assert_eq!(s.set.len(), SEEN_CAP);
        assert!(!s.first_time("c-a", &(SEEN_CAP + 9).to_string()));
        assert!(s.first_time("c-a", "0"), "最老的已经被挤出去了");
        assert!(s.first_time("c-b", "5"), "去重键里有 from");
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

    #[test]
    fn session_keys_parse_hash_ids_and_park_names() {
        assert_eq!(session_key("#12"), 12);
        assert_eq!(session_key("dc-terminal"), UNRESOLVED_SESSION);
        assert_eq!(session_key("#0"), UNRESOLVED_SESSION);
        assert_eq!(session_key("#x"), UNRESOLVED_SESSION);
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
