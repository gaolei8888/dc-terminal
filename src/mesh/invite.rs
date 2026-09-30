//! 用 6 位邀请码加电脑（dct-invite-v1）。设计见
//! `docs/superpowers/specs/2026-09-30-dct-invite-code-design.md`。
//!
//! - **老电脑（A，发邀请）**：`Mesh::start_invite` 出一个码，状态
//!   `Live → InFlight → 作废`。收到的三种 ask 在这里答：`InviteProbe`、
//!   `InviteJoin`、`InviteFinish`。
//! - **新电脑（B，加入）**：`join`——问一遍同账号在线的电脑、跟有码的那台走
//!   SPAKE2、验过确认值和名单才进组。
//!
//! 密码学（码、SPAKE2、确认值）在 `dct_mesh::invite`，这里只管状态和收发。
//!
//! **作废规则：** 码一进 `InFlight`，不管后面成不成都作废。进 `InFlight`
//! 之前就失败的（格式不对、签名不对、已经在组里）不作废——那几样跟码无关，
//! 中转伪造它们换不来任何关于码的信息。
use std::sync::Mutex;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use dct_link::Envelope;
use dct_mesh::invite::{Code, Confirmed, Handshake, MSG_LEN};
use dct_mesh::wire::{self, Payload};
use dct_mesh::Member;

use super::group;
use super::net::Net;
use super::Mesh;
use crate::proto::{InviteNote, InviteOutcome, InviteView, MeshProblem};

/// 一个码多久没人用就作废（秒）。
pub const INVITE_TTL_SECS: u64 = 10 * 60;
/// 码进了 `InFlight` 之后，B 必须在这么久里把 `InviteFinish` 送到（秒）。
pub const FINISH_WITHIN_SECS: u64 = 30;
/// B 问每台电脑（探问、加入、确认）等多久。A 都是当场答，这只是一次往返。
pub const INVITE_ASK_TIMEOUT: Duration = Duration::from_secs(5);
/// 同时有好几台发着码时，B 最多试几台（逐台试，对不上的那台码也作废）。
pub const MAX_INVITERS_TRIED: usize = 3;

/// A 这边发着的一个码。
pub(crate) struct Invite {
    id: u64,
    code: Code,
    expires: u64,
    state: State,
}

// 一台电脑同时只有一个码，两个变体差多大无所谓。
#[allow(clippy::large_enum_variant)]
enum State {
    Live,
    /// 有一台（`peer`）拿它来加入了。此后别人再来都失败；它必须在 `deadline`
    /// 之前送来对的 `cB`。
    InFlight {
        peer: String,
        joiner: Member,
        deadline: u64,
        /// SPAKE2 做完之后才有；算不出来的话码已经作废了，走不到要用它的地方。
        confirmed: Option<Confirmed>,
    },
}

impl Invite {
    fn view(&self) -> InviteView {
        InviteView {
            id: self.id,
            code: self.code.as_str().to_string(),
            expires_at: self.expires,
        }
    }
}

impl Mesh {
    /// 出一个新码（看板上按 `a`、`dct invite`）。已经有一个的话，旧的作废。
    /// 组里只有自己一台也行：第一台电脑就是这样拉第二台的。
    pub fn start_invite(&mut self) -> Result<InviteView, MeshProblem> {
        if self.roster.is_none() {
            return Err(MeshProblem::NotLoggedIn);
        }
        let now = (self.clock)();
        let rand = &self.rand;
        let code = Code::random(&mut || rand());
        self.invite_seq += 1;
        let inv = Invite {
            id: self.invite_seq,
            code,
            expires: now + INVITE_TTL_SECS,
            state: State::Live,
        };
        let view = inv.view();
        if let Some(old) = self.invite.replace(inv) {
            self.journal.mesh(&format!("invite_replaced id={}", old.id));
        }
        // 只记编号，**绝不记码**。
        self.journal.mesh(&format!("invite_created id={}", view.id));
        Ok(view)
    }

    /// 收回手上的码（`dct invite` 被 Ctrl-C）。没有就什么都不做。
    pub fn cancel_invite(&mut self) {
        if let Some(old) = self.invite.take() {
            self.journal.mesh(&format!("invite_cancelled id={}", old.id));
        }
    }

    /// 此刻发着的码（过期的先清掉）。`InFlight` 的也算：屏幕上的码还没结果。
    pub fn invite_view(&mut self) -> Option<InviteView> {
        self.expire_invite();
        self.invite.as_ref().map(Invite::view)
    }

    /// 懒着查的两种到点：`Live` 过了 10 分钟 → 过期；`InFlight` 过了 30 秒
    /// 还没等到 `InviteFinish` → 作废。
    ///
    /// 边界：`now == expires` 就算过期；`now == deadline` 还算来得及。
    pub(crate) fn expire_invite(&mut self) {
        let now = (self.clock)();
        let outcome = match &self.invite {
            Some(Invite {
                state: State::Live,
                expires,
                ..
            }) if now >= *expires => InviteOutcome::Expired,
            Some(Invite {
                state: State::InFlight { deadline, .. },
                ..
            }) if now > *deadline => InviteOutcome::Burned,
            _ => return,
        };
        self.end_invite(outcome, "invite_timed_out");
    }

    /// 码结束了：清掉、记下结果。
    fn end_invite(&mut self, outcome: InviteOutcome, why: &str) {
        if let Some(inv) = self.invite.take() {
            self.journal.mesh(&format!("{why} id={}", inv.id));
            self.invite_note = Some(InviteNote {
                id: inv.id,
                outcome,
            });
        }
    }

    /// `InviteProbe`：有 `Live` 的码就报上自己（自签的成员记录和组 id），
    /// 否则 `NoInvite`。不消耗码。
    pub(super) fn take_probe(&mut self, env: &Envelope) -> Option<Vec<u8>> {
        self.expire_invite();
        let live = matches!(
            self.invite,
            Some(Invite {
                state: State::Live,
                ..
            })
        );
        let group = self.roster.as_ref().map(|r| r.roster.group.clone());
        match (live, group) {
            (true, Some(group)) => {
                self.journal
                    .mesh(&format!("invite_probed from={}", env.from));
                Some(wire::encode(&Payload::InviteOpen {
                    sig: wire::sign_member(&self.me, &self.keys),
                    member: self.me.clone(),
                    group,
                }))
            }
            _ => Some(wire::encode(&Payload::NoInvite)),
        }
    }

    /// `InviteJoin`：检查都过了，**先**把码置成 `InFlight`，再算 SPAKE2、回
    /// `InviteKey`。
    pub(super) fn take_invite_join(
        &mut self,
        env: &Envelope,
        member: Member,
        sig: &str,
        spake: &str,
    ) -> Option<Vec<u8>> {
        self.expire_invite();
        let failed = Some(wire::encode(&Payload::InviteFailed));
        let from = env.from.as_str();
        // —— 进 `InFlight` 之前：失败不作废。
        let Some(inv) = self.invite.as_ref() else {
            self.drop_from(from, "invite_join_without_invite");
            return failed;
        };
        if !matches!(inv.state, State::Live) {
            self.drop_from(from, "invite_join_in_flight");
            return failed;
        }
        if member.endpoint != from || !wire::verify_member(&member, sig) {
            self.drop_from(from, "invite_join_bad_sig");
            return failed;
        }
        if !super::valid_name(&member.name) || !super::valid_kx_pub(&member.kx_pub) {
            self.drop_from(from, "invite_join_bad_member");
            return failed;
        }
        let Some(group_id) = self.roster.as_ref().map(|r| r.roster.group.clone()) else {
            self.drop_from(from, "invite_join_without_group");
            return failed;
        };
        if self
            .roster
            .as_ref()
            .is_some_and(|r| r.roster.member(from).is_some())
        {
            self.drop_from(from, "invite_join_already_member");
            return failed;
        }
        let Some(theirs) = wire::decode_len(spake, MSG_LEN) else {
            self.drop_from(from, "invite_join_bad_spake");
            return failed;
        };

        // —— 从这里起码已经用掉了：成不成都作废。
        let now = (self.clock)();
        let seed = (self.rand)();
        let me = self.me.clone();
        let inv = self.invite.as_mut().expect("checked above");
        inv.state = State::InFlight {
            peer: from.to_string(),
            joiner: member.clone(),
            deadline: now + FINISH_WITHIN_SECS,
            confirmed: None,
        };
        let hs = Handshake::inviter(&inv.code, &me, &member, seed);
        let msg_a = hs.message().to_vec();
        match hs.finish(&group_id, &theirs) {
            Ok(c) => {
                let tag = c.inviter_tag();
                if let State::InFlight { confirmed, .. } = &mut inv.state {
                    *confirmed = Some(c);
                }
                self.journal
                    .mesh(&format!("invite_in_flight id={} from={from}", inv.id));
                Some(wire::encode(&Payload::InviteKey {
                    spake: wire::encode_bytes(&msg_a),
                    confirm: wire::encode_bytes(&tag),
                }))
            }
            Err(_) => {
                self.end_invite(InviteOutcome::Burned, "invite_bad_spake_point");
                failed
            }
        }
    }

    /// `InviteFinish`：只认 `InFlight` 的那一台、只在 `deadline` 之前。`cB`
    /// 对了就编号、签名单 v+1、回 `InviteDone`，广播排进 `outbox`；不对就作废。
    pub(super) fn take_invite_finish(&mut self, env: &Envelope, confirm: &str) -> Option<Vec<u8>> {
        let from = env.from.as_str();
        let failed = Some(wire::encode(&Payload::InviteFailed));
        let Some(Invite {
            state: State::InFlight { peer, deadline, .. },
            ..
        }) = &self.invite
        else {
            self.drop_from(from, "invite_finish_not_in_flight");
            return None;
        };
        // 不是那一台：不理，也不改状态——不然谁都能替 B 把码烧掉之外，还能
        // 抢在 B 前面把它「完成」掉。
        if peer != from {
            self.drop_from(from, "invite_finish_from_stranger");
            return None;
        }
        if (self.clock)() > *deadline {
            self.end_invite(InviteOutcome::Burned, "invite_finish_too_late");
            return failed;
        }
        let Some(Invite {
            id,
            state: State::InFlight {
                joiner, confirmed, ..
            },
            ..
        }) = self.invite.take()
        else {
            unreachable!("matched above");
        };
        let ok = confirmed.as_ref().is_some_and(|c| {
            STANDARD
                .decode(confirm)
                .is_ok_and(|tag| c.joiner_tag_ok(&tag))
        });
        if !ok {
            self.journal.mesh(&format!("invite_wrong_code id={id} from={from}"));
            self.invite_note = Some(InviteNote {
                id,
                outcome: InviteOutcome::Burned,
            });
            return failed;
        }
        let Some(current) = self.roster.clone() else {
            return failed;
        };
        let taken: Vec<String> = current
            .roster
            .members
            .iter()
            .map(|m| m.name.clone())
            .collect();
        let mut joiner = joiner;
        if taken.contains(&joiner.name) {
            joiner.name = group::free_name(&joiner.name, &taken);
        }
        let mut members = current.roster.members.clone();
        members.push(joiner.clone());
        let next = group::sign_next(self, &current.roster, members);
        let to: Vec<String> = group::recipients(self, &next)
            .into_iter()
            .filter(|ep| *ep != joiner.endpoint)
            .collect();
        if let Err(e) = self.commit(next.clone()) {
            self.journal
                .mesh(&format!("roster_not_saved id={id} err={e}"));
            self.invite_note = Some(InviteNote {
                id,
                outcome: InviteOutcome::Burned,
            });
            return failed;
        }
        self.journal
            .mesh(&format!("invite_joined id={id} endpoint={}", joiner.endpoint));
        self.invite_note = Some(InviteNote {
            id,
            outcome: InviteOutcome::Joined { name: joiner.name },
        });
        if !to.is_empty() {
            self.outbox
                .push((to, wire::encode(&Payload::Roster(next.clone()))));
        }
        Some(wire::encode(&Payload::InviteDone { roster: next }))
    }
}

fn lock(m: &Mutex<Mesh>) -> std::sync::MutexGuard<'_, Mesh> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 把 `outbox` 里排着的广播发出去。**不攥着锁发**（`net` 模块头）。守护进程的
/// 投递线程每拍调一次；测试里手动调。
pub fn flush_outbox(mesh: &Mutex<Mesh>, net: &dyn Net) {
    let jobs = std::mem::take(&mut lock(mesh).outbox);
    for (to, payload) in jobs {
        group::broadcast(mesh, net, &to, &payload);
    }
}
#[cfg(test)]
mod tests {
    use super::super::net::testing::{FakeHub, FakeNet};
    use super::*;
    use dct_link::EndpointId;
    use dct_mesh::MachineKeys;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    pub(super) const T0: u64 = 1_800_000_000;

    pub(super) fn keys(b: u8) -> MachineKeys {
        MachineKeys::from_seeds([b; 32], [b.wrapping_add(100); 32]).unwrap()
    }

    /// 一台内存里的电脑：自己的 `Mesh`、挂在 `hub` 上的 `FakeNet`、一只拨得动的钟。
    pub(super) struct Node {
        pub mesh: Arc<Mutex<Mesh>>,
        pub net: FakeNet,
        pub ep: String,
        pub clock: Arc<AtomicU64>,
    }

    impl Node {
        pub fn new(hub: &Arc<FakeHub>, seed: u8, name: &str) -> Node {
            let clock = Arc::new(AtomicU64::new(T0));
            let c = clock.clone();
            let mesh = Mesh::new(keys(seed), name.into(), None)
                .with_clock(move || c.load(Ordering::SeqCst));
            let mesh = Arc::new(Mutex::new(mesh));
            hub.register(mesh.clone());
            let ep = mesh.lock().unwrap().endpoint().to_string();
            Node {
                net: hub.net_for(&ep),
                mesh,
                ep,
                clock,
            }
        }

        /// 登录过：有一份只有自己的名单。
        pub fn grouped(hub: &Arc<FakeHub>, seed: u8, name: &str) -> Node {
            let n = Node::new(hub, seed, name);
            n.mesh.lock().unwrap().ensure_group().unwrap();
            n
        }

        pub fn at(&self, t: u64) {
            self.clock.store(t, Ordering::SeqCst);
        }

        pub fn invite(&self) -> InviteView {
            self.mesh.lock().unwrap().start_invite().unwrap()
        }

        /// 最近一个码的结果。先按钟把到点的清掉（守护进程里 `view` 也是这么做的）。
        pub fn note(&self) -> Option<InviteNote> {
            let mut m = self.mesh.lock().unwrap();
            m.expire_invite();
            m.invite_note.clone()
        }

        pub fn live(&self) -> bool {
            self.mesh.lock().unwrap().invite_view().is_some()
        }

        pub fn me(&self) -> Member {
            self.mesh.lock().unwrap().me.clone()
        }

        pub fn roster(&self) -> dct_mesh::SignedRoster {
            self.mesh.lock().unwrap().roster.clone().unwrap()
        }

        pub fn names(&self) -> Vec<String> {
            let mut v: Vec<String> = self
                .roster()
                .roster
                .members
                .iter()
                .map(|m| m.name.clone())
                .collect();
            v.sort();
            v
        }

        /// 发一条 ask 给 `to`，把答复解出来。没答就是 `None`。
        pub fn ask(&self, to: &Node, p: &Payload) -> Option<Payload> {
            self.net
                .ask(&to.ep, wire::encode(p), INVITE_ASK_TIMEOUT)
                .ok()
                .map(|b| wire::decode(&b).unwrap())
        }
    }

    /// 不走 `join()`，手工当一回加入方 B：用 `code` 跟 A 走 SPAKE2。返回 A 对
    /// `InviteJoin` 的答复，和（答复是 `InviteKey`、`cA` 验得过时）B 手里的
    /// 确认结果。
    pub(super) fn raw_join(b: &Node, a: &Node, code: &str) -> (Option<Payload>, Option<Confirmed>) {
        let Some(Payload::InviteOpen { member, group, .. }) = b.ask(a, &Payload::InviteProbe)
        else {
            panic!("A 该答 InviteOpen");
        };
        let me = b.me();
        let hs = Handshake::joiner(&Code::parse(code).unwrap(), &member, &me, [0x5b; 32]);
        let join = Payload::InviteJoin {
            sig: wire::sign_member(&me, &b.mesh.lock().unwrap().keys),
            member: me,
            spake: wire::encode_bytes(hs.message()),
        };
        let reply = b.ask(a, &join);
        let confirmed = match &reply {
            Some(Payload::InviteKey { spake, confirm }) => {
                let c = hs
                    .finish(&group, &wire::decode_len(spake, MSG_LEN).unwrap())
                    .unwrap();
                let tag = STANDARD.decode(confirm).unwrap();
                c.inviter_tag_ok(&tag).then_some(c)
            }
            _ => None,
        };
        (reply, confirmed)
    }

    fn finish(b: &Node, a: &Node, c: &Confirmed) -> Option<Payload> {
        b.ask(
            a,
            &Payload::InviteFinish {
                confirm: wire::encode_bytes(&c.joiner_tag()),
            },
        )
    }

    /// 一个信封，`from` 随便写——模拟中转冒充任何同账号的端点。
    fn env_from(from: &str, to: &Node, p: &Payload) -> Envelope {
        Envelope {
            from: EndpointId::new(from).unwrap(),
            to: EndpointId::new(to.ep.clone()).unwrap(),
            seq: 1,
            payload: wire::encode(p),
            recipients: vec![],
        }
    }

    fn deliver(to: &Node, env: &Envelope) -> Option<Payload> {
        to.mesh
            .lock()
            .unwrap()
            .on_envelope(env)
            .map(|b| wire::decode(&b).unwrap())
    }

    #[test]
    fn without_a_live_code_a_probe_gets_no_invite_and_with_one_it_gets_a_signed_open() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        assert_eq!(b.ask(&a, &Payload::InviteProbe), Some(Payload::NoInvite));
        a.invite();
        for _ in 0..2 {
            let Some(Payload::InviteOpen { member, sig, group }) = b.ask(&a, &Payload::InviteProbe)
            else {
                panic!("该答 InviteOpen");
            };
            assert_eq!(member, a.me());
            assert!(wire::verify_member(&member, &sig));
            assert_eq!(group, a.roster().roster.group);
        }
        assert!(a.live(), "探问不消耗码");
    }

    #[test]
    fn a_machine_without_a_group_cannot_invite() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        assert_eq!(
            a.mesh.lock().unwrap().start_invite(),
            Err(MeshProblem::NotLoggedIn)
        );
    }

    #[test]
    fn a_code_is_six_digits_valid_for_ten_minutes_and_ids_count_up() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let v1 = a.invite();
        assert_eq!(v1.code.len(), 6);
        assert!(v1.code.bytes().all(|c| c.is_ascii_digit()));
        assert_eq!(v1.expires_at, T0 + INVITE_TTL_SECS);
        assert_eq!(v1.id, 1);
        assert_eq!(a.invite().id, 2);
    }

    /// 整条路，手工走：InviteJoin → InviteKey（cA 验得过）→ InviteFinish →
    /// InviteDone。名单 v2 = {A, B}，A 签；码用掉了；结果记下来。
    #[test]
    fn the_right_code_brings_the_joiner_in_and_uses_the_code_up() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (reply, confirmed) = raw_join(&b, &a, &v.code);
        assert!(matches!(reply, Some(Payload::InviteKey { .. })));
        let c = confirmed.expect("码对，cA 该验得过");
        let Some(Payload::InviteDone { roster }) = finish(&b, &a, &c) else {
            panic!("该答 InviteDone");
        };
        assert_eq!(roster, a.roster());
        assert_eq!(roster.roster.version, 2);
        assert_eq!(roster.signer, a.ep);
        assert_eq!(a.names(), ["A", "B"]);
        assert!(!a.live(), "用过一次就作废");
        assert_eq!(
            a.note(),
            Some(InviteNote {
                id: v.id,
                outcome: InviteOutcome::Joined { name: "B".into() }
            })
        );
        assert!(
            a.mesh.lock().unwrap().outbox.is_empty(),
            "组里没有别人，不用广播"
        );
    }

    /// 码一进 `InFlight`：探问答 `NoInvite`，别人再来加入一律失败，而且不打扰
    /// 在飞的那一个。
    #[test]
    fn once_in_flight_nobody_else_can_use_the_code() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let c = Node::grouped(&hub, 3, "C");
        let v = a.invite();
        let (_, confirmed) = raw_join(&b, &a, &v.code);
        let conf = confirmed.unwrap();
        assert_eq!(c.ask(&a, &Payload::InviteProbe), Some(Payload::NoInvite));
        let me = c.me();
        let hs = Handshake::joiner(&Code::parse(&v.code).unwrap(), &a.me(), &me, [9; 32]);
        let join = Payload::InviteJoin {
            sig: wire::sign_member(&me, &c.mesh.lock().unwrap().keys),
            member: me,
            spake: wire::encode_bytes(hs.message()),
        };
        assert_eq!(c.ask(&a, &join), Some(Payload::InviteFailed));
        assert!(matches!(
            finish(&b, &a, &conf),
            Some(Payload::InviteDone { .. })
        ));
        assert_eq!(a.names(), ["A", "B"]);
    }

    #[test]
    fn a_wrong_code_burns_it_and_a_retry_with_the_right_one_fails() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let wrong = if v.code == "000000" { "000001" } else { "000000" };
        let (reply, confirmed) = raw_join(&b, &a, wrong);
        assert!(matches!(reply, Some(Payload::InviteKey { .. })), "A 还不知道码错了");
        assert!(confirmed.is_none(), "B 验 cA 验不过");
        // B 不发 InviteFinish：到点之后 A 把码作废。
        a.at(T0 + FINISH_WITHIN_SECS + 1);
        assert!(!a.live());
        assert_eq!(
            a.note(),
            Some(InviteNote {
                id: v.id,
                outcome: InviteOutcome::Burned
            })
        );
        assert_eq!(b.ask(&a, &Payload::InviteProbe), Some(Payload::NoInvite));
        assert_eq!(a.names(), ["A"]);
    }

    /// B 那边码错了还硬发一个 cB（中转也能这么干）：当场作废。
    #[test]
    fn a_wrong_joiner_tag_burns_the_code_at_once() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        raw_join(&b, &a, &v.code);
        let bad = Payload::InviteFinish {
            confirm: wire::encode_bytes(&[0u8; 32]),
        };
        assert_eq!(b.ask(&a, &bad), Some(Payload::InviteFailed));
        assert!(!a.live());
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
        assert_eq!(a.names(), ["A"]);
    }

    /// `InviteFinish` 不是从 `InFlight` 的那一台来的：不理、不改状态；真的 B
    /// 随后照样完成。
    #[test]
    fn a_finish_from_another_endpoint_is_ignored() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let m = Node::grouped(&hub, 9, "M");
        let v = a.invite();
        let (_, confirmed) = raw_join(&b, &a, &v.code);
        let conf = confirmed.unwrap();
        // 连 B 的 cB 都偷到了，从 M 的端点送：照样不理。
        let stolen = Payload::InviteFinish {
            confirm: wire::encode_bytes(&conf.joiner_tag()),
        };
        assert_eq!(deliver(&a, &env_from(&m.ep, &a, &stolen)), None);
        assert!(a.live(), "还在飞，没作废");
        assert!(matches!(
            finish(&b, &a, &conf),
            Some(Payload::InviteDone { .. })
        ));
        assert_eq!(a.names(), ["A", "B"]);
    }

    #[test]
    fn a_finish_at_the_deadline_counts_and_one_second_later_burns() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (_, c) = raw_join(&b, &a, &v.code);
        a.at(T0 + FINISH_WITHIN_SECS);
        assert!(matches!(
            finish(&b, &a, &c.unwrap()),
            Some(Payload::InviteDone { .. })
        ));

        let c2 = Node::grouped(&hub, 3, "C");
        a.at(T0 + 100);
        let v = a.invite();
        let (_, c) = raw_join(&c2, &a, &v.code);
        a.at(T0 + 100 + FINISH_WITHIN_SECS + 1);
        assert_eq!(finish(&c2, &a, &c.unwrap()), Some(Payload::InviteFailed));
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
        assert_eq!(a.names(), ["A", "B"]);
    }

    /// 10 分钟整：第 599 秒还能用，第 600 秒作废。
    #[test]
    fn a_code_expires_at_exactly_ten_minutes() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        a.at(T0 + INVITE_TTL_SECS - 1);
        assert!(matches!(
            b.ask(&a, &Payload::InviteProbe),
            Some(Payload::InviteOpen { .. })
        ));
        a.at(T0 + INVITE_TTL_SECS);
        assert_eq!(b.ask(&a, &Payload::InviteProbe), Some(Payload::NoInvite));
        assert_eq!(
            a.note(),
            Some(InviteNote {
                id: v.id,
                outcome: InviteOutcome::Expired
            })
        );
    }

    /// 进 `InFlight` 之前的失败都不作废：格式错、签名错、端点不是自己的、
    /// 名字不合规、已经在组里。之后真的 B 照样进得来。
    #[test]
    fn failures_before_in_flight_do_not_burn_the_code() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let m = Node::grouped(&hub, 9, "M");
        let v = a.invite();
        let code = Code::parse(&v.code).unwrap();
        let bm = b.me();
        let b_sig = wire::sign_member(&bm, &b.mesh.lock().unwrap().keys);
        let good_spake =
            wire::encode_bytes(Handshake::joiner(&code, &a.me(), &bm, [1; 32]).message());

        let mut renamed = bm.clone();
        renamed.name = "B2".into();
        let mut bad_name = bm.clone();
        bad_name.name = "a/b".into();
        let bad_name_sig = wire::sign_member(&bad_name, &b.mesh.lock().unwrap().keys);
        let cases: Vec<(&str, Envelope)> = vec![
            (
                "签名跟记录对不上",
                env_from(
                    &b.ep,
                    &a,
                    &Payload::InviteJoin {
                        member: renamed,
                        sig: b_sig.clone(),
                        spake: good_spake.clone(),
                    },
                ),
            ),
            (
                "从别的端点发来",
                env_from(
                    &m.ep,
                    &a,
                    &Payload::InviteJoin {
                        member: bm.clone(),
                        sig: b_sig.clone(),
                        spake: good_spake.clone(),
                    },
                ),
            ),
            (
                "名字不合规",
                env_from(
                    &b.ep,
                    &a,
                    &Payload::InviteJoin {
                        member: bad_name,
                        sig: bad_name_sig,
                        spake: good_spake.clone(),
                    },
                ),
            ),
            (
                "SPAKE2 消息不是 33 字节",
                env_from(
                    &b.ep,
                    &a,
                    &Payload::InviteJoin {
                        member: bm.clone(),
                        sig: b_sig.clone(),
                        spake: wire::encode_bytes(&[b'B'; 32]),
                    },
                ),
            ),
            (
                "SPAKE2 消息不是 base64",
                env_from(
                    &b.ep,
                    &a,
                    &Payload::InviteJoin {
                        member: bm.clone(),
                        sig: b_sig.clone(),
                        spake: "!!".into(),
                    },
                ),
            ),
        ];
        for (what, e) in &cases {
            assert_eq!(deliver(&a, e), Some(Payload::InviteFailed), "{what}");
            assert!(a.live(), "{what}：不该作废");
            assert_eq!(a.note(), None, "{what}");
        }
        // 已经在组里的那一台再来：也不作废。
        {
            let mut r = a.roster().roster.clone();
            r.version = 2;
            r.members.push(m.me());
            let v2 = dct_mesh::roster::sign(r, &a.me(), &keys(1));
            a.mesh.lock().unwrap().commit(v2).unwrap();
        }
        let mm = m.me();
        let already = Payload::InviteJoin {
            sig: wire::sign_member(&mm, &m.mesh.lock().unwrap().keys),
            member: mm.clone(),
            spake: wire::encode_bytes(Handshake::joiner(&code, &a.me(), &mm, [2; 32]).message()),
        };
        assert_eq!(m.ask(&a, &already), Some(Payload::InviteFailed));
        assert!(a.live(), "已经在组里：不该作废");

        let (_, c) = raw_join(&b, &a, &v.code);
        assert!(matches!(
            finish(&b, &a, &c.unwrap()),
            Some(Payload::InviteDone { .. })
        ));
        assert_eq!(a.names(), ["A", "B", "M"]);
    }

    /// 长度、角色字节都对，但不是曲线上的点：这时码已经进了 `InFlight`，作废。
    #[test]
    fn a_spake_message_that_is_not_a_point_burns_the_code() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        a.invite();
        let bm = b.me();
        let mut not_a_point = vec![b'B'];
        not_a_point.extend_from_slice(&[0x02; 32]);
        let join = Payload::InviteJoin {
            sig: wire::sign_member(&bm, &b.mesh.lock().unwrap().keys),
            member: bm,
            spake: wire::encode_bytes(&not_a_point),
        };
        assert_eq!(b.ask(&a, &join), Some(Payload::InviteFailed));
        assert!(!a.live());
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
    }

    /// 再按一次 `a`：旧码作废，只有新码能用。
    #[test]
    fn a_second_invite_voids_the_first_code() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let first = a.invite();
        let second = a.invite();
        assert_ne!(first.id, second.id);
        if first.code != second.code {
            let (_, c) = raw_join(&b, &a, &first.code);
            assert!(c.is_none(), "旧码对不上");
            assert_eq!(a.names(), ["A"]);
        }
        let third = a.invite();
        let (_, c) = raw_join(&b, &a, &third.code);
        assert!(matches!(
            finish(&b, &a, &c.unwrap()),
            Some(Payload::InviteDone { .. })
        ));
    }

    #[test]
    fn cancelling_takes_the_code_away_without_a_note() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        a.invite();
        a.mesh.lock().unwrap().cancel_invite();
        assert!(!a.live());
        assert_eq!(a.note(), None);
        assert_eq!(b.ask(&a, &Payload::InviteProbe), Some(Payload::NoInvite));
    }

    /// 撞名就编号（`Mac` → `Mac 2`），结果里报的是编过号的名字。
    #[test]
    fn a_joiner_whose_name_is_taken_is_numbered() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "Mac");
        let b = Node::grouped(&hub, 2, "Mac");
        let v = a.invite();
        let (_, c) = raw_join(&b, &a, &v.code);
        let Some(Payload::InviteDone { roster }) = finish(&b, &a, &c.unwrap()) else {
            panic!()
        };
        assert_eq!(a.names(), ["Mac", "Mac 2"]);
        assert_eq!(roster.roster.member(&b.ep).unwrap().name, "Mac 2");
        assert_eq!(
            a.note().unwrap().outcome,
            InviteOutcome::Joined {
                name: "Mac 2".into()
            }
        );
    }

    /// 码对了，但 A 的名单存不下（目录是个文件）：不换名单，码作废，答
    /// `InviteFailed`——B 那边也就不会以为自己进来了。
    #[test]
    fn an_invite_that_cannot_be_saved_changes_nothing() {
        let hub = FakeHub::new();
        let t = tempfile::tempdir().unwrap();
        let blocker = t.path().join("mesh");
        std::fs::write(&blocker, b"not a dir").unwrap();
        let a = Node::grouped(&hub, 1, "A");
        {
            let mut m = a.mesh.lock().unwrap();
            let taken = std::mem::replace(&mut *m, Mesh::new(keys(1), "A".into(), None));
            *m = taken.with_store(super::super::store::Store::at(blocker));
        }
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (_, c) = raw_join(&b, &a, &v.code);
        assert_eq!(finish(&b, &a, &c.unwrap()), Some(Payload::InviteFailed));
        assert_eq!(a.names(), ["A"]);
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
        assert!(a.mesh.lock().unwrap().outbox.is_empty());
    }

    #[test]
    fn numbering_a_long_name_stays_a_valid_name() {
        let long: String = "长".repeat(dct_mesh::roster::MAX_NAME_LEN);
        let n = group::free_name(&long, std::slice::from_ref(&long));
        assert!(super::super::valid_name(&n), "{n}");
        assert!(n.ends_with(" 2"));
        assert_eq!(group::free_name("Mac", &["Mac".into(), "Mac 2".into()]), "Mac 3");
    }

    /// 组里原来还有 C：B 进来之后，新名单排进 `outbox`，发出去 C 也收到。
    #[test]
    fn the_new_roster_is_broadcast_to_the_other_members() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let c = Node::grouped(&hub, 3, "C");
        let v = a.invite();
        let (_, conf) = raw_join(&c, &a, &v.code);
        let Some(Payload::InviteDone { roster }) = finish(&c, &a, &conf.unwrap()) else {
            panic!()
        };
        c.mesh.lock().unwrap().commit(roster).unwrap();

        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (_, conf) = raw_join(&b, &a, &v.code);
        finish(&b, &a, &conf.unwrap());
        assert_eq!(c.names(), ["A", "C"], "还没发");
        let queued = a.mesh.lock().unwrap().outbox.clone();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].0, vec![c.ep.clone()], "只发给其余成员，不发给 B");
        flush_outbox(&a.mesh, &a.net);
        assert_eq!(c.names(), ["A", "B", "C"]);
        assert!(a.mesh.lock().unwrap().outbox.is_empty());
    }

    /// 守护进程重启：码只在内存里。同一把钥匙、同一份名单起一个新的 `Mesh`
    /// （换掉 hub 上那一台），它不认得重启前的码。
    #[test]
    fn a_restart_forgets_the_code() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        a.invite();
        let restarted = Arc::new(Mutex::new(Mesh::new(keys(1), "A".into(), Some(a.roster()))));
        assert!(restarted.lock().unwrap().invite.is_none());
        hub.register(restarted);
        assert_eq!(b.ask(&a, &Payload::InviteProbe), Some(Payload::NoInvite));
    }

    /// 重放上一轮的 `InviteJoin` / `InviteFinish`：码已经用掉了，都不管用。
    #[test]
    fn replaying_a_finished_round_gets_nothing() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let bm = b.me();
        let hs = Handshake::joiner(&Code::parse(&v.code).unwrap(), &a.me(), &bm, [3; 32]);
        let join = Payload::InviteJoin {
            sig: wire::sign_member(&bm, &b.mesh.lock().unwrap().keys),
            member: bm,
            spake: wire::encode_bytes(hs.message()),
        };
        let Some(Payload::InviteKey { spake, .. }) = b.ask(&a, &join) else {
            panic!()
        };
        let c = hs
            .finish(
                &a.roster().roster.group,
                &wire::decode_len(&spake, MSG_LEN).unwrap(),
            )
            .unwrap();
        let fin = Payload::InviteFinish {
            confirm: wire::encode_bytes(&c.joiner_tag()),
        };
        assert!(matches!(b.ask(&a, &fin), Some(Payload::InviteDone { .. })));
        let before = a.roster();
        assert_eq!(b.ask(&a, &join), Some(Payload::InviteFailed));
        assert_eq!(b.ask(&a, &fin), None);
        // A 又出了一个新码：重放旧的 InviteJoin，B 已经在组里，被拒，也不连累新码。
        a.invite();
        assert_eq!(b.ask(&a, &join), Some(Payload::InviteFailed));
        assert!(a.live());
        assert_eq!(a.roster(), before);
    }

    /// 答复类的消息不该从轮询里进来。
    #[test]
    fn unasked_invite_replies_are_dropped() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        for p in [
            Payload::NoInvite,
            Payload::InviteFailed,
            Payload::InviteKey {
                spake: "x".into(),
                confirm: "y".into(),
            },
            Payload::InviteOpen {
                member: b.me(),
                sig: "s".into(),
                group: "g".into(),
            },
            Payload::InviteDone {
                roster: b.roster(),
            },
        ] {
            assert_eq!(deliver(&a, &env_from(&b.ep, &a, &p)), None, "{p:?}");
        }
        assert_eq!(a.names(), ["A"]);
    }

    /// 码不进日志：走完一整轮，journal 里找不到它。
    #[test]
    fn the_code_never_reaches_the_journal() {
        let hub = FakeHub::new();
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("sessions.log");
        let j = crate::journal::Journal::new();
        j.set_path(path.clone());
        let a = Node::grouped(&hub, 1, "A");
        {
            let mut m = a.mesh.lock().unwrap();
            let taken = std::mem::replace(&mut *m, Mesh::new(keys(1), "A".into(), None));
            *m = taken.with_journal(Arc::new(j));
        }
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (_, c) = raw_join(&b, &a, &v.code);
        finish(&b, &a, &c.unwrap());
        let log = std::fs::read_to_string(&path).unwrap();
        assert!(log.contains("invite_created"), "{log}");
        assert!(!log.contains(&v.code), "{log}");
    }
}
