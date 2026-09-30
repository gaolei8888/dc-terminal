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
use sha2::Digest as _;
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
/// B 每次 `dct join` 最多试几台发邀请的电脑。是 1：同时有好几台答 `InviteOpen`
/// 就一台都不试（`MeshProblem::SeveralInviters`），否则中转冒充几台就多几次
/// 猜码的机会。常数留着，守护进程按它算最坏要等多久。
pub const MAX_INVITERS_TRIED: usize = 1;

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
        // 已经在名单上、钥匙一模一样的：上一轮 `InviteDone` 在路上丢了，B 自己
        // 不知道进了组。照常走一遍码（码对不上一样作废），对上了就把现在的
        // 名单再给它一份，不再加一条。钥匙不一样的还是拒。
        if self
            .roster
            .as_ref()
            .and_then(|r| r.roster.member(from))
            .is_some_and(|m| m.sign_pub != member.sign_pub || m.kx_pub != member.kx_pub)
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
        if let Some(already) = current.roster.member(&joiner.endpoint) {
            self.journal
                .mesh(&format!("invite_rejoined id={id} endpoint={}", joiner.endpoint));
            self.invite_note = Some(InviteNote {
                id,
                outcome: InviteOutcome::Joined {
                    name: already.name.clone(),
                },
            });
            return Some(wire::encode(&Payload::InviteDone { roster: current }));
        }
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
/// 新电脑上 `dct join <码> [--name 名字]`。
///
/// 1. 问一遍同账号在线的电脑（最多 `MAX_JOIN_ASK` 台，排序去重）：`InviteProbe`；
/// 2. 答 `InviteOpen` 的（自签对、端点就是问的那台）只能有一台——多于一台就
///    一台都不试（`SeveralInviters`）；跟它走 SPAKE2：`InviteJoin` → 验 `cA` →
///    `InviteFinish` → 收 `InviteDone` 里的名单。同一个码在这台电脑上只送一次
///    （`Mesh::spent_codes`）；
/// 3. 名单验过（签名者就是 PAKE 里绑定的那台 A、组对得上、我那一条是我的
///    钥匙）才进组。
///
/// **收到 `InviteDone` 之前不改任何本地状态**：`name` 只用在发出去的那份
/// 记录里，进组之后才存下来。
pub fn join(
    mesh: &Mutex<Mesh>,
    net: &dyn Net,
    code: &str,
    name: Option<&str>,
) -> Result<(), MeshProblem> {
    let code = Code::parse(code).ok_or(MeshProblem::BadInviteCode)?;
    let spent: [u8; 32] = sha2::Sha256::digest(dct_mesh::invite::password(&code)).into();
    if lock(mesh).spent_codes.contains(&spent) {
        return Err(MeshProblem::WrongInviteCode);
    }
    let chosen = name.map(str::trim).filter(|n| !n.is_empty());
    let (me, sig) = {
        let m = lock(mesh);
        if !m.is_alone() {
            return Err(MeshProblem::AlreadyInGroup);
        }
        let mut me = m.me.clone();
        if let Some(n) = chosen {
            if !super::valid_name(n) {
                return Err(MeshProblem::BadName);
            }
            me.name = n.to_string();
        }
        let sig = wire::sign_member(&me, &m.keys);
        (me, sig)
    };

    let mut peers = net.peers().unwrap_or_default();
    peers.sort();
    peers.dedup();
    if peers.len() > super::MAX_JOIN_ASK {
        lock(mesh)
            .journal
            .mesh(&format!("join_too_many_peers n={}", peers.len()));
        return Err(MeshProblem::TooManyAnswered);
    }

    // 并排探问：有一台卡住不该拖着别的。
    let answers: Vec<(String, Option<Payload>)> = std::thread::scope(|s| {
        let hs: Vec<_> = peers
            .iter()
            .map(|p| {
                s.spawn(move || {
                    let r = net
                        .ask(p, wire::encode(&Payload::InviteProbe), INVITE_ASK_TIMEOUT)
                        .ok()
                        .and_then(|b| wire::decode(&b).ok());
                    (p.clone(), r)
                })
            })
            .collect();
        hs.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    let mut unanswered = 0u32;
    let mut inviters: Vec<(Member, String)> = Vec::new();
    for (peer, answer) in answers {
        match answer {
            Some(Payload::InviteOpen { member, sig, group })
                if member.endpoint == peer
                    && wire::verify_member(&member, &sig)
                    && super::valid_name(&member.name)
                    && super::valid_kx_pub(&member.kx_pub)
                    && !group.is_empty() =>
            {
                inviters.push((member, group))
            }
            Some(Payload::NoInvite) => {}
            // 没回话、回了认不出的东西（旧版 dct 不认识 `InviteProbe`，
            // 直接丢掉不回），或者回的 `InviteOpen` 验不过。
            _ => unanswered += 1,
        }
    }
    if inviters.is_empty() {
        return Err(MeshProblem::NoInvite { unanswered });
    }
    if inviters.len() > MAX_INVITERS_TRIED {
        return Err(MeshProblem::SeveralInviters {
            n: inviters.len() as u32,
        });
    }
    let (inviter, group) = &inviters[0];
    // 送出 `InviteJoin` 之前就记下：这一次不管成不成，都算用掉了一次猜的机会。
    lock(mesh).spent_codes.push(spent);
    match attempt(mesh, net, &code, inviter, group, &me, &sig) {
        Ok(roster) => settle(mesh, roster, inviter, chosen.is_some()),
        Err(Attempt::BadRoster) => Err(MeshProblem::InviteRosterRefused),
        Err(Attempt::Mismatch | Attempt::Unreachable) => Err(MeshProblem::WrongInviteCode),
    }
}

enum Attempt {
    /// 码对不上，或者那台说不行（码已作废、正有别人在用）。
    Mismatch,
    /// 没回话、回了认不出的东西。
    Unreachable,
    /// 走完了，A 也签了名单，但名单这边验不过。
    BadRoster,
}

/// 跟一台发着码的电脑走完 SPAKE2，拿回它签的名单（已经验过）。
fn attempt(
    mesh: &Mutex<Mesh>,
    net: &dyn Net,
    code: &Code,
    inviter: &Member,
    group: &str,
    me: &Member,
    sig: &str,
) -> Result<dct_mesh::SignedRoster, Attempt> {
    let seed = lock(mesh).fresh_nonce();
    let hs = Handshake::joiner(code, inviter, me, seed);
    let join = Payload::InviteJoin {
        member: me.clone(),
        sig: sig.to_string(),
        spake: wire::encode_bytes(hs.message()),
    };
    let to = inviter.endpoint.as_str();
    let reply = net
        .ask(to, wire::encode(&join), INVITE_ASK_TIMEOUT)
        .map_err(|_| Attempt::Unreachable)?;
    let (msg_a, tag_a) = match wire::decode(&reply) {
        Ok(Payload::InviteKey { spake, confirm }) => (
            wire::decode_len(&spake, MSG_LEN).ok_or(Attempt::Mismatch)?,
            STANDARD.decode(confirm).map_err(|_| Attempt::Mismatch)?,
        ),
        Ok(Payload::InviteFailed) => return Err(Attempt::Mismatch),
        _ => return Err(Attempt::Unreachable),
    };
    let confirmed = hs.finish(group, &msg_a).map_err(|_| Attempt::Mismatch)?;
    // 先验 A：对不上就停，不发 `InviteFinish`（A 那边到点自己作废）。
    if !confirmed.inviter_tag_ok(&tag_a) {
        lock(mesh)
            .journal
            .mesh(&format!("join_wrong_code via={to}"));
        return Err(Attempt::Mismatch);
    }
    let finish = Payload::InviteFinish {
        confirm: wire::encode_bytes(&confirmed.joiner_tag()),
    };
    let reply = net
        .ask(to, wire::encode(&finish), INVITE_ASK_TIMEOUT)
        .map_err(|_| Attempt::Unreachable)?;
    let roster = match wire::decode(&reply) {
        Ok(Payload::InviteDone { roster }) => roster,
        Ok(Payload::InviteFailed) => return Err(Attempt::Mismatch),
        _ => return Err(Attempt::Unreachable),
    };
    // 名单：签名者 == PAKE 里绑定的 A、签名在 A 的钥匙下成立、A 和我都在上面
    // 且钥匙没被换（`accept_invite`）；组就是 `T` 里的那个；我那一条的加密
    // 公钥也是我的（`accept_invite` 只看得到我的端点）。
    let mine_ok = roster
        .roster
        .member(&me.endpoint)
        .is_some_and(|m| m.sign_pub == me.sign_pub && m.kx_pub == me.kx_pub);
    if dct_mesh::roster::accept_invite(&roster, &me.endpoint, inviter).is_err()
        || roster.roster.group != group
        || !mine_ok
    {
        lock(mesh)
            .journal
            .mesh(&format!("join_bad_roster via={to}"));
        return Err(Attempt::BadRoster);
    }
    Ok(roster)
}

/// 验过的名单落盘、换上。`chose_name`：用户用 `--name` 起过名字，从此不再算
/// dct 起的。
fn settle(
    mesh: &Mutex<Mesh>,
    roster: dct_mesh::SignedRoster,
    inviter: &Member,
    chose_name: bool,
) -> Result<(), MeshProblem> {
    let mut m = lock(mesh);
    if !m.is_alone() {
        return Err(MeshProblem::AlreadyInGroup);
    }
    m.commit(roster).map_err(|_| MeshProblem::NotSaved)?;
    if chose_name {
        let name = m.me.name.clone();
        if let Some(s) = &m.store {
            let _ = s.set_name(&name);
        }
        m.auto_name = false;
    }
    m.journal
        .mesh(&format!("joined via={}", inviter.endpoint));
    Ok(())
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
        // 已经在组里、但钥匙跟名单上不一样的那一台再来：拒，也不作废。
        {
            let mut r = a.roster().roster.clone();
            r.version = 2;
            r.members.push(m.me());
            let v2 = dct_mesh::roster::sign(r, &a.me(), &keys(1));
            a.mesh.lock().unwrap().commit(v2).unwrap();
        }
        let mut mm = m.me();
        mm.kx_pub = STANDARD.encode(keys(8).kx_pub());
        assert_ne!(mm.kx_pub, m.me().kx_pub);
        let already = Payload::InviteJoin {
            sig: wire::sign_member(&mm, &m.mesh.lock().unwrap().keys),
            member: mm.clone(),
            spake: wire::encode_bytes(Handshake::joiner(&code, &a.me(), &mm, [2; 32]).message()),
        };
        assert_eq!(m.ask(&a, &already), Some(Payload::InviteFailed));
        assert!(a.live(), "已经在组里、钥匙不一样：不该作废");

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
        // A 又出了一个新码：重放旧的 InviteJoin（B 已经在组里、钥匙一样，算重进）
        // 会把新码拖进 InFlight；旧的 InviteFinish 对不上新一轮，码作废、名单不变。
        // 跟中转抢先用错码一样，只是捣乱，进不来。
        a.invite();
        assert!(matches!(b.ask(&a, &join), Some(Payload::InviteKey { .. })));
        assert_eq!(b.ask(&a, &fin), Some(Payload::InviteFailed));
        assert!(!a.live());
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

    impl Node {
        /// 自己造好的 `Mesh`（带 store、带 journal 的）挂上来。钟换成可拨的。
        pub fn from_mesh(hub: &Arc<FakeHub>, mesh: Mesh) -> Node {
            let clock = Arc::new(AtomicU64::new(T0));
            let c = clock.clone();
            let mesh = Arc::new(Mutex::new(
                mesh.with_clock(move || c.load(Ordering::SeqCst)),
            ));
            hub.register(mesh.clone());
            let ep = mesh.lock().unwrap().endpoint().to_string();
            Node {
                net: hub.net_for(&ep),
                mesh,
                ep,
                clock,
            }
        }
    }

    // —— 新电脑这边：`join` ——

    use super::super::net::testing::EvilNet;
    use super::super::store::Store;

    fn join_via(b: &Node, net: &dyn Net, code: &str) -> Result<(), MeshProblem> {
        join(&b.mesh, net, code, None)
    }

    /// 一个跟 `code` 不同的 6 位码。
    fn other_than(code: &str) -> String {
        if code == "000000" { "000001".into() } else { "000000".into() }
    }

    /// B 的整份本地状态：名单、自己那条记录、名字是不是 dct 起的。
    fn state_of(n: &Node) -> (Option<dct_mesh::SignedRoster>, Member, bool) {
        let m = n.mesh.lock().unwrap();
        (m.roster.clone(), m.me.clone(), m.auto_name)
    }

    #[test]
    fn joining_with_the_right_code_takes_the_inviters_roster() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        assert_eq!(join_via(&b, &b.net, &v.code), Ok(()));
        assert_eq!(b.roster(), a.roster());
        assert_eq!(b.names(), ["A", "B"]);
        assert_eq!(
            a.note().unwrap().outcome,
            InviteOutcome::Joined { name: "B".into() }
        );
        assert!(!b.mesh.lock().unwrap().is_alone());
    }

    /// 人敲的码：中间带空格、带 `-`、全角数字、前导 0，都认。
    #[test]
    fn a_code_typed_with_spaces_dashes_or_full_width_digits_still_works() {
        for style in 0..3 {
            let hub = FakeHub::new();
            let a = Node::grouped(&hub, 1, "A");
            let b = Node::grouped(&hub, 2, "B");
            let v = a.invite();
            let typed = match style {
                0 => format!("{} {}", &v.code[..3], &v.code[3..]),
                1 => format!("{}-{}", &v.code[..3], &v.code[3..]),
                _ => v
                    .code
                    .chars()
                    .map(|c| char::from_u32(c as u32 - '0' as u32 + 0xFF10).unwrap())
                    .collect(),
            };
            assert_eq!(join_via(&b, &b.net, &typed), Ok(()), "{typed:?}");
        }
    }

    #[test]
    fn a_malformed_code_or_name_is_refused_before_anything_is_sent() {
        let hub = FakeHub::new();
        let _a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let net = EvilNet::honest(hub.net_for(&b.ep));
        for bad in ["48291", "4829134", "48291x", ""] {
            assert_eq!(join_via(&b, &net, bad), Err(MeshProblem::BadInviteCode), "{bad:?}");
        }
        assert_eq!(
            join(&b.mesh, &net, "482913", Some("a/b")),
            Err(MeshProblem::BadName)
        );
        assert!(net.sent.lock().unwrap().is_empty());
    }

    #[test]
    fn a_wrong_code_fails_burns_the_inviters_code_and_changes_nothing_here() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let before = state_of(&b);
        let net = EvilNet::honest(hub.net_for(&b.ep));
        assert_eq!(
            join_via(&b, &net, &other_than(&v.code)),
            Err(MeshProblem::WrongInviteCode)
        );
        assert_eq!(net.count("invite_finish"), 0, "cA 验不过就不再往下走");
        assert_eq!(state_of(&b), before);
        a.at(T0 + FINISH_WITHIN_SECS + 1);
        assert!(!a.live());
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
        // 作废之后，对的码也进不来。
        assert_eq!(
            join_via(&b, &b.net, &v.code),
            Err(MeshProblem::NoInvite { unanswered: 0 })
        );
        assert_eq!(a.names(), ["A"]);
    }

    /// 同账号的另一台（或中转冒充它）抢先拿错码试一次：码作废，真的 B
    /// 随后进不来，提示重新生成。
    #[test]
    fn a_front_running_wrong_guess_burns_the_code_for_the_real_joiner() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let m = Node::grouped(&hub, 9, "M");
        let v = a.invite();
        let (_, c) = raw_join(&m, &a, &other_than(&v.code));
        assert!(c.is_none());
        assert_eq!(
            join_via(&b, &b.net, &v.code),
            Err(MeshProblem::NoInvite { unanswered: 0 })
        );
        a.at(T0 + FINISH_WITHIN_SECS + 1);
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
        assert_eq!(a.names(), ["A"]);
        assert_eq!(b.mesh.lock().unwrap().roster.as_ref().unwrap().roster.version, 1);
    }

    /// 中转把 B 的 SPAKE2 消息换成自己的（拿一个猜的码算的），B 的记录原样
    /// 转：A 进 `InFlight`，但 B 验 `cA` 验不过，不发 `InviteFinish`；A 的码
    /// 到点作废。
    #[test]
    fn a_relay_that_swaps_the_joiners_spake_message_gets_nowhere() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (am, bm) = (a.me(), b.me());
        let guess = Code::parse(&other_than(&v.code)).unwrap();
        let net = EvilNet::honest(hub.net_for(&b.ep)).outgoing(move |_, p| match wire::decode(&p) {
            Ok(Payload::InviteJoin { member, sig, .. }) => {
                let evil = Handshake::joiner(&guess, &am, &bm, [0xee; 32]);
                wire::encode(&Payload::InviteJoin {
                    member,
                    sig,
                    spake: wire::encode_bytes(evil.message()),
                })
            }
            _ => p,
        });
        assert_eq!(join_via(&b, &net, &v.code), Err(MeshProblem::WrongInviteCode));
        assert_eq!(net.count("invite_finish"), 0);
        a.at(T0 + FINISH_WITHIN_SECS + 1);
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Burned);
        assert_eq!(a.names(), ["A"]);
        assert_eq!(b.names(), ["B"]);
    }

    /// 中转冒充 A 回 `InviteKey`（自己拿一个猜的码跟 B 的消息走完 SPAKE2）：
    /// B 的 `cA` 验不过，停下，不发 `InviteFinish`。
    #[test]
    fn a_relay_that_answers_for_the_inviter_fails_the_inviter_tag() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let (am, bm) = (a.me(), b.me());
        let group = a.roster().roster.group.clone();
        let guess = Code::parse(&other_than(&v.code)).unwrap();
        let joined = Arc::new(Mutex::new(None::<Vec<u8>>));
        let seen = joined.clone();
        let net = EvilNet::honest(hub.net_for(&b.ep))
            .outgoing(move |_, p| {
                if let Ok(Payload::InviteJoin { spake, .. }) = wire::decode(&p) {
                    *seen.lock().unwrap() = wire::decode_len(&spake, MSG_LEN);
                }
                p
            })
            .incoming(move |_, r| match wire::decode(&r) {
                Ok(Payload::InviteKey { .. }) => {
                    let msg_b = joined.lock().unwrap().clone().unwrap();
                    let evil = Handshake::inviter(&guess, &am, &bm, [0xaa; 32]);
                    let msg_a = evil.message().to_vec();
                    let c = evil.finish(&group, &msg_b).unwrap();
                    Ok(wire::encode(&Payload::InviteKey {
                        spake: wire::encode_bytes(&msg_a),
                        confirm: wire::encode_bytes(&c.inviter_tag()),
                    }))
                }
                _ => Ok(r),
            });
        assert_eq!(join_via(&b, &net, &v.code), Err(MeshProblem::WrongInviteCode));
        assert_eq!(net.count("invite_finish"), 0);
        assert_eq!(b.names(), ["B"]);
    }

    /// 中转在 `InviteJoin` 里把 B 的一把公钥换成自己的：B 的自签名就不成立，
    /// A 在进 `InFlight` 之前就拒——**码不作废**（这一下换不来任何关于码的
    /// 信息）。但 B 分不出这一次是不是给了中转一次猜码的机会，所以同一个码
    /// 不再送第二次；老电脑按 a 换个新码，中转不捣乱，就进得来。
    #[test]
    fn a_relay_that_edits_the_joiners_keys_is_refused_without_burning() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let evil_kx = STANDARD.encode(keys(9).kx_pub());
        let net = EvilNet::honest(hub.net_for(&b.ep)).outgoing(move |_, p| match wire::decode(&p) {
            Ok(Payload::InviteJoin { mut member, sig, spake }) => {
                member.kx_pub = evil_kx.clone();
                wire::encode(&Payload::InviteJoin { member, sig, spake })
            }
            _ => p,
        });
        assert_eq!(join_via(&b, &net, &v.code), Err(MeshProblem::WrongInviteCode));
        assert!(a.live(), "签名不对不作废");
        assert_eq!(join_via(&b, &b.net, &v.code), Err(MeshProblem::WrongInviteCode));
        assert!(a.live(), "同一个码没再送出去");
        let v2 = a.invite();
        assert_eq!(join_via(&b, &b.net, &v2.code), Ok(()));
        let listed = a.roster().roster.member(&b.ep).cloned().unwrap();
        assert_eq!(listed.kx_pub, b.me().kx_pub);
    }

    /// 中转伪造 `InviteOpen`：换成另一台电脑的自签记录（端点不是问的那台）、
    /// 或者改了记录里的字段（自签不成立）——都不算发着码的电脑。
    #[test]
    fn a_forged_invite_open_is_not_an_inviter() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let m = Node::grouped(&hub, 9, "M");
        hub.set_online(&m.ep, false);
        let v = a.invite();
        let mm = m.me();
        let m_sig = wire::sign_member(&mm, &m.mesh.lock().unwrap().keys);
        for forge in 0..2 {
            let (mm, m_sig) = (mm.clone(), m_sig.clone());
            let net = EvilNet::honest(hub.net_for(&b.ep)).incoming(move |_, r| match wire::decode(&r) {
                Ok(Payload::InviteOpen { mut member, sig, group }) => Ok(wire::encode(&if forge == 0 {
                    Payload::InviteOpen { member: mm.clone(), sig: m_sig.clone(), group }
                } else {
                    member.name = "改过的名字".into();
                    Payload::InviteOpen { member, sig, group }
                })),
                _ => Ok(r),
            });
            assert_eq!(
                join_via(&b, &net, &v.code),
                Err(MeshProblem::NoInvite { unanswered: 1 }),
                "forge {forge}"
            );
            assert_eq!(net.count("invite_join"), 0, "forge {forge}");
        }
        assert!(a.live());
    }

    /// `InviteDone` 里的名单被动过：组不对、签名不对、我的加密公钥被换——都
    /// 不收，本地什么都不变，告诉用户去老电脑上把这台移掉。后两种是 A 自己
    /// 签的（签名验得过），只有「组对不上 `T`」「我那一条不是我的钥匙」拦得住。
    #[test]
    fn a_tampered_invite_done_roster_is_refused() {
        for tamper in 0..5 {
            let hub = FakeHub::new();
            let a = Node::grouped(&hub, 1, "A");
            let b = Node::grouped(&hub, 2, "B");
            let v = a.invite();
            let before = state_of(&b);
            let (b_ep, evil_kx) = (b.ep.clone(), STANDARD.encode(keys(9).kx_pub()));
            let net = EvilNet::honest(hub.net_for(&b.ep)).incoming(move |_, r| match wire::decode(&r) {
                Ok(Payload::InviteDone { mut roster }) => {
                    match tamper {
                        0 => roster.roster.group = "evil".into(),
                        1 => roster.sig = STANDARD.encode([0u8; 64]),
                        2 => {
                            for m in &mut roster.roster.members {
                                if m.endpoint == b_ep {
                                    m.kx_pub = evil_kx.clone();
                                }
                            }
                        }
                        3 => {
                            let signer = roster.roster.member(&roster.signer).cloned().unwrap();
                            let mut r = roster.roster.clone();
                            r.group = "mine-somewhere-else".into();
                            roster = dct_mesh::roster::sign(r, &signer, &keys(1));
                        }
                        _ => {
                            let signer = roster.roster.member(&roster.signer).cloned().unwrap();
                            let mut r = roster.roster.clone();
                            for m in &mut r.members {
                                if m.endpoint == b_ep {
                                    m.kx_pub = evil_kx.clone();
                                }
                            }
                            roster = dct_mesh::roster::sign(r, &signer, &keys(1));
                        }
                    }
                    Ok(wire::encode(&Payload::InviteDone { roster }))
                }
                _ => Ok(r),
            });
            assert_eq!(
                join_via(&b, &net, &v.code),
                Err(MeshProblem::InviteRosterRefused),
                "tamper {tamper}"
            );
            assert_eq!(state_of(&b), before, "tamper {tamper}");
        }
    }

    /// 走到最后一步、`InviteDone` 没回来：本地一样也没改（名单、名字、
    /// `--name` 起的名字都没落盘）。
    /// 老电脑已经把 B 签进去了，`InviteDone` 却在路上丢了：B 照提示在老电脑上
    /// 按 a 换个新码再敲，就能进组；名单不多一条，版本也不变。
    #[test]
    fn a_joiner_whose_invite_done_was_lost_gets_in_with_a_new_code() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let lossy = EvilNet::honest(hub.net_for(&b.ep)).incoming(|_, r| match wire::decode(&r) {
            Ok(Payload::InviteDone { .. }) => Err(crate::link::LinkError::Unreachable),
            _ => Ok(r),
        });
        assert_eq!(join_via(&b, &lossy, &v.code), Err(MeshProblem::WrongInviteCode));
        assert_eq!(a.names(), ["A", "B"]);
        let version = a.roster().roster.version;
        let v2 = a.invite();
        assert_eq!(join_via(&b, &b.net, &v2.code), Ok(()));
        assert_eq!(b.roster(), a.roster());
        assert_eq!(a.names(), ["A", "B"]);
        assert_eq!(a.roster().roster.version, version);
        assert!(!a.live(), "新码用掉了");
    }

    #[test]
    fn nothing_changes_locally_until_invite_done_arrives() {
        let hub = FakeHub::new();
        let t = tempfile::tempdir().unwrap();
        let a = Node::grouped(&hub, 1, "A");
        let mesh = Mesh::new(keys(2), "Mac".into(), None).with_store(Store::at(t.path().join("mesh")));
        let b = Node::from_mesh(&hub, mesh);
        b.mesh.lock().unwrap().auto_name = true;
        b.mesh.lock().unwrap().ensure_group().unwrap();
        let before = state_of(&b);
        let st = Store::at(t.path().join("mesh"));
        let roster_before = st.roster().unwrap();
        let v = a.invite();
        let net = EvilNet::honest(hub.net_for(&b.ep)).incoming(|_, r| match wire::decode(&r) {
            Ok(Payload::InviteDone { .. }) => Err(crate::link::LinkError::Unreachable),
            _ => Ok(r),
        });
        assert_eq!(
            join(&b.mesh, &net, &v.code, Some("公司电脑")),
            Err(MeshProblem::WrongInviteCode)
        );
        assert_eq!(state_of(&b), before);
        assert_eq!(st.roster().unwrap(), roster_before);
        assert!(!st.has_chosen_name(), "--name 没落盘");
        assert_eq!(a.names(), ["A", "公司电脑"], "A 那边已经签进去了；换个新码再敲就能进，见 a_joiner_whose_invite_done_was_lost_gets_in_with_a_new_code");
    }

    #[test]
    fn a_chosen_name_is_used_and_saved_once_joined() {
        let hub = FakeHub::new();
        let t = tempfile::tempdir().unwrap();
        let a = Node::grouped(&hub, 1, "A");
        let mesh = Mesh::new(keys(2), "Mac".into(), None).with_store(Store::at(t.path().join("mesh")));
        let b = Node::from_mesh(&hub, mesh);
        b.mesh.lock().unwrap().ensure_group().unwrap();
        let v = a.invite();
        assert_eq!(join(&b.mesh, &b.net, &v.code, Some("  公司电脑 ")), Ok(()));
        assert_eq!(a.names(), ["A", "公司电脑"]);
        let st = Store::at(t.path().join("mesh"));
        assert!(st.has_chosen_name());
        assert_eq!(st.name().unwrap(), "公司电脑");
        assert_eq!(st.roster().unwrap(), Some(a.roster()));
    }

    /// B 的名字跟组里的撞了：A 编号，B 进组之后自己也叫编过号的名字。
    #[test]
    fn a_clashing_name_is_numbered_by_the_inviter() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "Mac");
        let b = Node::grouped(&hub, 2, "Mac");
        let v = a.invite();
        assert_eq!(join_via(&b, &b.net, &v.code), Ok(()));
        assert_eq!(b.me().name, "Mac 2");
        assert_eq!(b.names(), ["Mac", "Mac 2"]);
    }

    #[test]
    fn an_expired_code_finds_no_invite() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        a.at(T0 + INVITE_TTL_SECS);
        assert_eq!(
            join_via(&b, &b.net, &v.code),
            Err(MeshProblem::NoInvite { unanswered: 0 })
        );
        assert_eq!(a.note().unwrap().outcome, InviteOutcome::Expired);
    }

    /// 两台同时发着码：B 逐台试（按端点排），对不上的那台码也作废；用的是
    /// 哪台的码就进哪台的组。
    /// 同账号有两台以上在发邀请码（可能有一台是中转冒充的）：一台都不试——试
    /// 一台就是给中转一次猜码的机会。两台的码都还活着。
    #[test]
    fn with_several_inviters_nobody_is_tried() {
        let hub = FakeHub::new();
        let x = Node::grouped(&hub, 1, "X");
        let y = Node::grouped(&hub, 3, "Y");
        let b = Node::grouped(&hub, 2, "B");
        x.invite();
        let v = y.invite();
        let net = EvilNet::honest(hub.net_for(&b.ep));
        assert_eq!(
            join_via(&b, &net, &v.code),
            Err(MeshProblem::SeveralInviters { n: 2 })
        );
        assert_eq!(net.count("invite_probe"), 2);
        assert_eq!(net.count("invite_join"), 0);
        assert!(x.live() && y.live());
    }

    /// 一个码在这台电脑上只送出去一次：送错了（或者中转冒充发邀请的那台接了
    /// 这一次），再敲同一个码——换个写法也一样——不再发 `InviteJoin`，直接说码
    /// 已作废。
    #[test]
    fn a_code_already_tried_here_is_not_sent_again() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        let wrong = other_than(&v.code);
        let net = EvilNet::honest(hub.net_for(&b.ep));
        assert_eq!(join_via(&b, &net, &wrong), Err(MeshProblem::WrongInviteCode));
        let v2 = a.invite();
        if v2.code == wrong {
            return; // 百万分之一：新码恰好是刚才那个错码，这一轮测不出。
        }
        let spaced = format!("{} {}", &wrong[..3], &wrong[3..]);
        assert_eq!(join_via(&b, &net, &spaced), Err(MeshProblem::WrongInviteCode));
        assert_eq!(net.count("invite_join"), 1);
        assert!(a.live(), "第二次没送出去，A 的新码还活着");
        assert_eq!(join_via(&b, &net, &v2.code), Ok(()));
    }

    /// 中转说同账号在线的有 17 台：一台都不问。
    #[test]
    fn more_than_sixteen_computers_online_asks_nobody() {
        struct Crowd;
        impl Net for Crowd {
            fn peers(&self) -> Result<Vec<String>, crate::link::LinkError> {
                Ok((0..=super::super::MAX_JOIN_ASK).map(|i| format!("c-{i:020}")).collect())
            }
            fn send(&self, _: &str, _: Vec<u8>) -> Result<(), crate::link::LinkError> {
                panic!("不该发任何东西")
            }
            fn ask(&self, _: &str, _: Vec<u8>, _: Duration) -> Result<Vec<u8>, crate::link::LinkError> {
                panic!("不该问任何一台")
            }
        }
        let hub = FakeHub::new();
        let b = Node::grouped(&hub, 2, "B");
        assert_eq!(join_via(&b, &Crowd, "482913"), Err(MeshProblem::TooManyAnswered));
    }

    /// 正好 16 台：每台都问（上限是「超过 16」）。
    #[test]
    fn exactly_sixteen_computers_online_are_all_asked() {
        struct Sixteen(Mutex<usize>);
        impl Net for Sixteen {
            fn peers(&self) -> Result<Vec<String>, crate::link::LinkError> {
                Ok((0..super::super::MAX_JOIN_ASK).map(|i| format!("c-{i:020}")).collect())
            }
            fn send(&self, _: &str, _: Vec<u8>) -> Result<(), crate::link::LinkError> {
                panic!("不该 send")
            }
            fn ask(&self, _: &str, _: Vec<u8>, _: Duration) -> Result<Vec<u8>, crate::link::LinkError> {
                *self.0.lock().unwrap() += 1;
                Ok(wire::encode(&Payload::NoInvite))
            }
        }
        let hub = FakeHub::new();
        let b = Node::grouped(&hub, 2, "B");
        let net = Sixteen(Mutex::new(0));
        assert_eq!(
            join_via(&b, &net, "482913"),
            Err(MeshProblem::NoInvite { unanswered: 0 })
        );
        assert_eq!(*net.0.lock().unwrap(), super::super::MAX_JOIN_ASK);
    }

    /// 发邀请那台的名字不合规（控制字符、双向控制符）：它的 `InviteOpen` 不算。
    #[test]
    fn an_inviter_with_a_bad_name_is_not_an_inviter() {
        let hub = FakeHub::new();
        let a = Node::from_mesh(&hub, Mesh::new(keys(1), "A\u{202e}x".into(), None));
        a.mesh.lock().unwrap().ensure_group().unwrap();
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        assert_eq!(
            join_via(&b, &b.net, &v.code),
            Err(MeshProblem::NoInvite { unanswered: 1 })
        );
        assert!(a.live(), "B 一句都没跟它多说");
    }

    /// 两边比确认值只走 `verify_slice`（常数时间），这一层不自己拿 `==` 比。
    #[test]
    fn tags_are_only_checked_through_the_constant_time_helpers() {
        let src = include_str!("invite.rs");
        let body = &src[..src.find("#[cfg(test)]").unwrap()];
        assert!(!body.contains("_tag() =="), "别拿 == 比确认值");
        assert!(!body.contains("== tag"), "别拿 == 比确认值");
        assert!(body.contains("inviter_tag_ok(") && body.contains("joiner_tag_ok("));
    }

    #[test]
    fn a_machine_already_grouped_with_others_is_refused_without_asking() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        let v = a.invite();
        join_via(&b, &b.net, &v.code).unwrap();
        a.invite();
        let net = EvilNet::honest(hub.net_for(&b.ep));
        assert_eq!(join_via(&b, &net, "482913"), Err(MeshProblem::AlreadyInGroup));
        assert!(net.sent.lock().unwrap().is_empty());
    }

    /// 老电脑的 dct 太旧，不认识 `InviteProbe`（解不出来就丢，不回）：
    /// 算一台没回话的，提示可能太旧。
    #[test]
    fn an_old_dct_that_does_not_know_invite_probe_counts_as_unanswered() {
        let hub = FakeHub::new();
        let a = Node::grouped(&hub, 1, "A");
        let b = Node::grouped(&hub, 2, "B");
        a.invite();
        let net = EvilNet::honest(hub.net_for(&b.ep)).outgoing(|_, p| {
            if matches!(wire::decode(&p), Ok(Payload::InviteProbe)) {
                br#"{"t":"something_this_version_does_not_know"}"#.to_vec()
            } else {
                p
            }
        });
        assert_eq!(
            join_via(&b, &net, "482913"),
            Err(MeshProblem::NoInvite { unanswered: 1 })
        );
    }
}
