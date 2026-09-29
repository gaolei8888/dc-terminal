//! 要跟别的电脑说话的那几件事：看一眼现状、请求加入、批准、移除。
//!
//! 全都是「先在锁里算好、放锁、再发」：**调用 `Net` 的时候绝不攥着 `Mesh`
//! 那把锁**（见 `net` 模块头）。`FakeHub` 上发出去是同步调到对方的
//! `on_envelope`，对方要是回头调到我，攥着锁就是死锁；真网络上一次 `ask`
//! 能挂好几秒，那段时间里收件的轮询线程也要这把锁。
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use dct_mesh::roster::{self, Roster};
use dct_mesh::sas;
use dct_mesh::wire::{self, Payload};

use super::net::Net;
use super::Mesh;
use crate::proto::{MemberView, MeshProblem, MeshView, PendingJoin};

/// 新电脑问每台已有电脑时等多久。对方收到 `Join` 当场就回 `JoinPending`，
/// 不等用户，所以这只是一次往返的时间。
pub const JOIN_ASK_TIMEOUT: Duration = Duration::from_secs(10);

fn lock(m: &Mutex<Mesh>) -> std::sync::MutexGuard<'_, Mesh> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 现状。`online` 来自 `net.peers()`，问不到就当谁都不在线。
pub fn view(mesh: &Mutex<Mesh>, net: &dyn Net, logged_in: bool) -> MeshView {
    let online: HashSet<String> = net.peers().unwrap_or_default().into_iter().collect();
    let mut m = lock(mesh);
    m.prune_pending();
    m.prune_invites();
    let me = m.me.endpoint.clone();
    let members = m
        .roster
        .as_ref()
        .map(|r| {
            r.roster
                .members
                .iter()
                .map(|x| MemberView {
                    name: x.name.clone(),
                    endpoint: x.endpoint.clone(),
                    online: x.endpoint == me || online.contains(&x.endpoint),
                    is_me: x.endpoint == me,
                })
                .collect()
        })
        .unwrap_or_default();
    MeshView {
        logged_in,
        name: m.me.name.clone(),
        endpoint: me,
        in_group: m.roster.is_some(),
        members,
        pending: m
            .pending_joins
            .iter()
            .map(|(j, code, _)| PendingJoin {
                name: j.member.name.clone(),
                endpoint: j.member.endpoint.clone(),
                code: code.clone(),
            })
            .collect(),
        joining: joining(&m),
        messages: m.delivered_counts(),
    }
}

fn joining(m: &Mesh) -> Vec<PendingJoin> {
    m.invites
        .iter()
        .map(|i| PendingJoin {
            name: i.member.name.clone(),
            endpoint: i.member.endpoint.clone(),
            code: i.code.clone(),
        })
        .collect()
}

/// 新电脑请求加入：`name` 非空就先改名；问一遍每台在线的电脑，把回了
/// `JoinPending` 的记下来（`Mesh::invites`），返回每台的 6 位数。
///
/// **这一步还不认任何一台。** 回话的电脑谁都可能是中转塞进来的；要等用户
/// 看过那边屏幕上的数字、说出是哪一台（`confirm`），那一台签的名单才收。
///
/// 回答必须是对方自签的成员记录，而且记录里的端点就是我问的那个端点——
/// 中转认证过发件人，我问的是谁，答的就得是谁。
pub fn join(
    mesh: &Mutex<Mesh>,
    net: &dyn Net,
    name: Option<&str>,
) -> Result<Vec<PendingJoin>, MeshProblem> {
    let req = {
        let mut m = lock(mesh);
        if !m.is_alone() {
            return Err(MeshProblem::AlreadyInGroup);
        }
        if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
            m.rename(n)?;
        }
        m.join_request()
    };
    let peers = net.peers().unwrap_or_default();
    let payload = wire::encode(&Payload::Join(req));
    // 并排问：对方回得快，但真网络上有一台卡住不该拖着别的。
    let replies: Vec<(String, Vec<u8>)> = std::thread::scope(|s| {
        let asks: Vec<_> = peers
            .iter()
            .map(|p| {
                let payload = payload.clone();
                s.spawn(move || (p.clone(), net.ask(p, payload, JOIN_ASK_TIMEOUT)))
            })
            .collect();
        asks.into_iter()
            .filter_map(|h| h.join().ok())
            .filter_map(|(p, r)| r.ok().map(|b| (p, b)))
            .collect()
    });

    let mut m = lock(mesh);
    let mut found = Vec::new();
    for (peer, bytes) in replies {
        match wire::decode(&bytes) {
            Ok(Payload::JoinPending { member, sig })
                if member.endpoint == peer && wire::verify_member(&member, &sig) =>
            {
                let code = sas::code(&m.me, &member);
                found.push((member, code));
            }
            _ => m.journal.mesh(&format!("join_bad_reply from={peer}")),
        }
    }
    m.set_invites(found);
    if m.invites.is_empty() {
        return Err(MeshProblem::NoOneAnswered);
    }
    Ok(joining(&m))
}

/// 新电脑上：用户认定 `endpoint` 那台的数字一样。之后只收它签的名单
/// （它已经送来过的话，当场验、当场进组）。
pub fn confirm(mesh: &Mutex<Mesh>, endpoint: &str) -> Result<(), MeshProblem> {
    lock(mesh).confirm_inviter(endpoint)
}

/// 批准（`yes`）或拒绝一条加入请求。返回那台的名字。
///
/// `endpoint` 和 `code` 必须是界面刚给用户看过的那一条：端点定位请求，
/// 数字再对一遍——用户点的「同意」只能落在他看过数字的那一条上。
///
/// 批准：新名单 = 旧名单加上它，版本加一，我签；存下来之后发给名单上
/// 除我以外的每一台（包括新来的那台——它靠这份名单进组）。
pub fn approve(
    mesh: &Mutex<Mesh>,
    net: &dyn Net,
    endpoint: &str,
    code: &str,
    yes: bool,
) -> Result<String, MeshProblem> {
    let (name, payload, to) = {
        let mut m = lock(mesh);
        m.prune_pending();
        let Some(i) = m
            .pending_joins
            .iter()
            .position(|(j, _, _)| j.member.endpoint == endpoint)
        else {
            return Err(MeshProblem::NoSuchRequest(endpoint.to_string()));
        };
        if yes && m.pending_joins[i].1 != code {
            return Err(MeshProblem::CodeMismatch);
        }
        let joiner = m.pending_joins[i].0.member.clone();
        if !yes {
            m.pending_joins.remove(i);
            m.journal
                .mesh(&format!("join_refused endpoint={}", joiner.endpoint));
            return Ok(joiner.name);
        }
        let current = m.roster.clone().ok_or(MeshProblem::NotLoggedIn)?;
        if current.roster.by_name(&joiner.name).is_some() {
            return Err(MeshProblem::NameTaken(joiner.name));
        }
        let mut members = current.roster.members.clone();
        members.push(joiner.clone());
        let next = sign_next(&m, &current.roster, members);
        let to = recipients(&m, &next);
        m.commit(next.clone()).map_err(|_| MeshProblem::NotSaved)?;
        m.journal
            .mesh(&format!("join_approved endpoint={}", joiner.endpoint));
        (joiner.name, wire::encode(&Payload::Roster(next)), to)
    };
    broadcast(mesh, net, &to, &payload);
    Ok(name)
}

/// 把一台电脑移出组。`who` 是电脑名或端点。不能是自己：名单规定签名者不能
/// 在自己签的那一版里把自己删掉，要走也得由别的电脑来移。
///
/// 新名单只发给留下来的；被移出的那台发来的东西，从这一版起在每台留下的
/// 电脑上都验不过（`seal::open` 按名单找发件人）。
pub fn remove(mesh: &Mutex<Mesh>, net: &dyn Net, who: &str) -> Result<String, MeshProblem> {
    let (name, payload, to) = {
        let mut m = lock(mesh);
        let current = m.roster.clone().ok_or(MeshProblem::NotLoggedIn)?;
        let target = current
            .roster
            .by_name(who)
            .or_else(|| current.roster.member(who))
            .cloned()
            .ok_or_else(|| MeshProblem::NoSuchMachine(who.to_string()))?;
        if target.endpoint == m.me.endpoint {
            return Err(MeshProblem::CannotRemoveSelf);
        }
        let members = current
            .roster
            .members
            .iter()
            .filter(|x| x.endpoint != target.endpoint)
            .cloned()
            .collect();
        let next = sign_next(&m, &current.roster, members);
        let to = recipients(&m, &next);
        m.commit(next.clone()).map_err(|_| MeshProblem::NotSaved)?;
        m.journal
            .mesh(&format!("removed endpoint={}", target.endpoint));
        (target.name, wire::encode(&Payload::Roster(next)), to)
    };
    broadcast(mesh, net, &to, &payload);
    Ok(name)
}

fn sign_next(m: &Mesh, current: &Roster, members: Vec<dct_mesh::Member>) -> dct_mesh::SignedRoster {
    let next = Roster {
        group: current.group.clone(),
        version: current.version + 1,
        members,
    };
    roster::sign(next, &m.me, &m.keys)
}

fn recipients(m: &Mesh, r: &dct_mesh::SignedRoster) -> Vec<String> {
    r.roster
        .members
        .iter()
        .filter(|x| x.endpoint != m.me.endpoint)
        .map(|x| x.endpoint.clone())
        .collect()
}

/// 发给每一台，不等答复。不在线的这一次收不到（第一步没有离线留言），
/// 记一行 journal。**调用时没攥着锁**，记 journal 时才短暂拿一下。
///
/// 并排发：一台卡到超时不拖着别的，整次广播只花一次 `send` 的时间
/// （`mesh::worst_case` 按这个算命令行该等多久）。
fn broadcast(mesh: &Mutex<Mesh>, net: &dyn Net, to: &[String], payload: &[u8]) {
    let failed: Vec<(String, crate::link::LinkError)> = std::thread::scope(|s| {
        let hs: Vec<_> = to
            .iter()
            .map(|ep| s.spawn(move || (ep.clone(), net.send(ep, payload.to_vec()))))
            .collect();
        hs.into_iter()
            .filter_map(|h| h.join().ok())
            .filter_map(|(ep, r)| r.err().map(|e| (ep, e)))
            .collect()
    });
    for (ep, e) in failed {
        lock(mesh)
            .journal
            .mesh(&format!("roster_not_delivered to={ep} err={e:?}"));
    }
}

#[cfg(test)]
mod tests {
    use super::super::net::testing::{FakeHub, FakeNet};
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use dct_link::{EndpointId, Envelope};
    use dct_mesh::seal::{self, Kind, Message};
    use dct_mesh::wire::JoinRequest;
    use dct_mesh::{MachineKeys, Member};
    use std::sync::Arc;

    fn keys(b: u8) -> MachineKeys {
        MachineKeys::from_seeds([b; 32], [b.wrapping_add(100); 32]).unwrap()
    }

    struct Node {
        mesh: Arc<Mutex<Mesh>>,
        net: FakeNet,
        ep: String,
    }

    impl Node {
        fn new(hub: &Arc<FakeHub>, seed: u8, name: &str) -> Node {
            Node::with_mesh(hub, Mesh::new(keys(seed), name.into(), None))
        }

        fn with_mesh(hub: &Arc<FakeHub>, mesh: Mesh) -> Node {
            let mesh = Arc::new(Mutex::new(mesh));
            hub.register(mesh.clone());
            let ep = mesh.lock().unwrap().endpoint().to_string();
            Node {
                net: hub.net_for(&ep),
                mesh,
                ep,
            }
        }

        /// `dct login` 在守护进程里做的那一半（令牌之外）：没有组就建一个。
        fn login(&self) -> bool {
            self.mesh.lock().unwrap().ensure_group().unwrap()
        }

        fn join(&self) -> Result<Vec<PendingJoin>, MeshProblem> {
            join(&self.mesh, &self.net, None)
        }

        /// 跟 `dct peers approve <who>` 一样：先从现状里按名字或端点找到
        /// 那一条，把它的端点**和数字**一起送去批。
        fn approve(&self, who: &str) -> Result<String, MeshProblem> {
            let pending = self.view().pending;
            let hits: Vec<&PendingJoin> = pending
                .iter()
                .filter(|p| p.endpoint == who || p.name == who)
                .collect();
            match hits.as_slice() {
                [] => approve(&self.mesh, &self.net, who, "", true),
                [p] => approve(&self.mesh, &self.net, &p.endpoint, &p.code, true),
                _ => Err(MeshProblem::Ambiguous(who.to_string())),
            }
        }

        /// 新电脑上的用户说「是 `inviter` 那台，数字一样」。
        fn confirm(&self, inviter: &Node) -> Result<(), MeshProblem> {
            confirm(&self.mesh, &inviter.ep)
        }

        fn view(&self) -> MeshView {
            view(&self.mesh, &self.net, true)
        }

        fn roster(&self) -> dct_mesh::SignedRoster {
            self.mesh.lock().unwrap().roster.clone().unwrap()
        }

        fn names(&self) -> Vec<String> {
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

        fn me(&self) -> Member {
            self.mesh.lock().unwrap().me.clone()
        }
    }

    /// A 建组、B 加入、A 批准。
    fn ab(hub: &Arc<FakeHub>) -> (Node, Node) {
        let a = Node::new(hub, 1, "A");
        let b = Node::new(hub, 2, "B");
        assert!(a.login());
        assert!(b.login());
        b.join().unwrap();
        b.confirm(&a).unwrap();
        assert_eq!(a.approve("B").unwrap(), "B");
        (a, b)
    }

    #[test]
    fn login_makes_a_group_of_one_only_the_first_time() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        assert!(a.login(), "第一次登录建组");
        assert!(!a.login(), "再登录不重建");
        let v = a.view();
        assert!(v.in_group);
        assert_eq!(v.members.len(), 1);
        assert!(v.members[0].is_me);
        assert_eq!(a.roster().roster.version, 1);
        assert_eq!(a.roster().roster.group, format!("mine-{}", a.ep));
    }

    /// 整条路：A 登录 → B 登录、加入 → A 批准 → B 进组 → C 加入 → B 批准
    /// → A 也收到新名单。
    #[test]
    fn three_computers_join_one_after_another() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        let c = Node::new(&hub, 3, "C");
        assert!(a.login());
        assert!(b.login());

        // B 问到 A（C 还没登录，没组，不回）。B 屏幕上的数字和 A 屏幕上的一样。
        let codes = b.join().unwrap();
        assert_eq!(codes.len(), 1);
        assert_eq!(codes[0].endpoint, a.ep);
        assert_eq!(codes[0].name, "A");
        let expected = sas::code(&b.me(), &a.me());
        assert_eq!(codes[0].code, expected);
        let pending = a.view().pending;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].name, "B");
        assert_eq!(pending[0].endpoint, b.ep);
        assert_eq!(pending[0].code, expected, "两边屏幕上的数字必须一样");
        assert_eq!(a.names(), ["A"], "没批之前名单不变");

        b.confirm(&a).unwrap();
        assert_eq!(a.approve("B").unwrap(), "B");
        assert_eq!(a.names(), ["A", "B"]);
        assert_eq!(b.roster(), a.roster(), "B 从 A 那里拿到了同一份名单");
        assert_eq!(b.roster().roster.version, 2);
        assert!(a.view().pending.is_empty());
        let bv = b.view();
        assert!(bv.joining.is_empty(), "进组之后不再算在「正在加入」");
        assert_eq!(bv.members.len(), 2);
        assert!(bv.members.iter().all(|m| m.online));

        // C 加入：A、B 都问到，各算一个数字。
        assert!(c.login());
        let codes = c.join().unwrap();
        assert_eq!(codes.len(), 2);
        for code in &codes {
            let other = if code.endpoint == a.ep { &a } else { &b };
            assert_eq!(code.code, sas::code(&c.me(), &other.me()));
        }
        assert_eq!(a.view().pending.len(), 1);
        assert_eq!(b.view().pending.len(), 1);

        // C 的用户看过 B 屏幕上的数字，认定 B；B 批（按端点指）。A 收到
        // B 签的 v3，C 按邀请收下。
        c.confirm(&b).unwrap();
        assert_eq!(b.approve(&c.ep).unwrap(), "C");
        assert_eq!(b.names(), ["A", "B", "C"]);
        assert_eq!(a.roster(), b.roster(), "A 也收到了新名单");
        assert_eq!(c.roster(), b.roster());
        assert_eq!(a.roster().roster.version, 3);
        assert_eq!(a.roster().signer, b.ep);
        assert!(
            a.view().pending.is_empty(),
            "C 已经进组，A 那边的请求跟着消掉"
        );
    }

    /// 中转把 B 的公钥换成了自己的：A 屏幕上的数字跟 B 屏幕上的对不上；
    /// 没人批，它就永远进不了名单；拒绝就只是删掉请求。
    #[test]
    fn a_join_with_swapped_keys_shows_a_different_code_and_never_enters_unapproved() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        // 冒充者：自己的钥匙，名字写成 B。
        let m = Node::new(&hub, 9, "B");
        assert!(a.login());
        assert!(b.login());
        assert!(m.login());

        let b_codes = b.join().unwrap();
        let b_code_for_a = b_codes
            .iter()
            .find(|x| x.endpoint == a.ep)
            .unwrap()
            .code
            .clone();
        // 冒充者只问 A。
        hub.set_online(&b.ep, false);
        let m_codes = m.join().unwrap();
        hub.set_online(&b.ep, true);
        assert_eq!(m_codes.len(), 1);

        let pending = a.view().pending;
        assert_eq!(pending.len(), 2);
        let forged = pending.iter().find(|p| p.endpoint == m.ep).unwrap();
        assert_eq!(forged.name, "B", "名字谁都能起");
        assert_ne!(forged.code, b_code_for_a, "钥匙换了，数字就对不上");

        // 两台都叫 B：按名字批是有歧义的，不能替用户猜。
        assert_eq!(a.approve("B"), Err(MeshProblem::Ambiguous("B".into())));
        assert_eq!(a.names(), ["A"], "没批就进不来");
        // 冒充者自己发一份名单把自己加进去：签名者不在 A 的名单里，丢掉。
        let mut r = a.roster().roster.clone();
        r.version = 2;
        r.members.push(m.me());
        let self_signed = roster::sign(r, &m.me(), &keys(9));
        m.net
            .send(&a.ep, wire::encode(&Payload::Roster(self_signed)))
            .unwrap();
        assert_eq!(a.names(), ["A"]);

        // 冒牌货的端点配上 B 的数字（用户以为自己批的是 B）：数字对不上，不批。
        assert_eq!(
            approve(&a.mesh, &a.net, &m.ep, &b_code_for_a, true),
            Err(MeshProblem::CodeMismatch)
        );
        assert_eq!(a.names(), ["A"]);

        // 用户看数字不对，拒绝。
        assert_eq!(approve(&a.mesh, &a.net, &m.ep, "", false).unwrap(), "B");
        let pending = a.view().pending;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].endpoint, b.ep);
        assert_eq!(a.names(), ["A"]);
    }

    /// 一个 `Join`：成员记录说自己是 B（B 的端点、B 的公钥），签名却是
    /// 冒充者的钥匙签的——或者从冒充者的端点发来。都不挂，也不回。
    #[test]
    fn a_join_that_is_not_self_signed_by_the_sender_is_dropped_silently() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        let m = Node::new(&hub, 9, "M");
        assert!(a.login());

        let forged = JoinRequest {
            member: b.me(),
            sig: wire::sign_member(&b.me(), &keys(9)),
        };
        let r = m.net.ask(
            &a.ep,
            wire::encode(&Payload::Join(forged)),
            JOIN_ASK_TIMEOUT,
        );
        assert!(r.is_err(), "不回任何东西");

        // 从 B 自己的端点发来（中转认证过），但签名不是 B 的钥匙签的。
        let bad_sig = JoinRequest {
            member: b.me(),
            sig: wire::sign_member(&b.me(), &keys(9)),
        };
        let r = b.net.ask(
            &a.ep,
            wire::encode(&Payload::Join(bad_sig)),
            JOIN_ASK_TIMEOUT,
        );
        assert!(r.is_err(), "签名不对，不回");

        // B 的真签名，但从冒充者的端点发来。
        let replayed = b.mesh.lock().unwrap().join_request();
        let r = m.net.ask(
            &a.ep,
            wire::encode(&Payload::Join(replayed)),
            JOIN_ASK_TIMEOUT,
        );
        assert!(r.is_err());
        assert!(a.view().pending.is_empty());
    }

    #[test]
    fn a_join_from_a_member_or_a_machine_without_a_group_gets_no_answer() {
        let hub = FakeHub::new();
        let (a, b) = ab(&hub);
        let again = b.mesh.lock().unwrap().join_request();
        assert!(b
            .net
            .ask(&a.ep, wire::encode(&Payload::Join(again)), JOIN_ASK_TIMEOUT)
            .is_err());
        assert!(a.view().pending.is_empty());

        // 没登录（没组）的电脑不回 `JoinPending`。
        let lone = Node::new(&hub, 5, "L");
        let x = Node::new(&hub, 6, "X");
        let req = x.mesh.lock().unwrap().join_request();
        assert!(x
            .net
            .ask(
                &lone.ep,
                wire::encode(&Payload::Join(req)),
                JOIN_ASK_TIMEOUT
            )
            .is_err());
    }

    /// 答我的是谁，记下的就得是谁：一台电脑拿**别人**的（真的、签得对的）
    /// `JoinPending` 来答，不算数——不然中转可以把 A 的答复换成 M 的，
    /// 我就会把 M 当成核对过数字的邀请人。
    #[test]
    fn a_join_reply_must_come_from_the_machine_that_was_asked() {
        struct Swapped {
            asked: String,
            reply: Vec<u8>,
        }
        impl Net for Swapped {
            fn peers(&self) -> Result<Vec<String>, crate::link::LinkError> {
                Ok(vec![self.asked.clone()])
            }
            fn send(&self, _: &str, _: Vec<u8>) -> Result<(), crate::link::LinkError> {
                Ok(())
            }
            fn ask(
                &self,
                _: &str,
                _: Vec<u8>,
                _: Duration,
            ) -> Result<Vec<u8>, crate::link::LinkError> {
                Ok(self.reply.clone())
            }
        }
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        let m = Node::new(&hub, 9, "M");
        m.login();
        let m_me = m.me();
        let net = Swapped {
            asked: a.ep.clone(),
            reply: wire::encode(&Payload::JoinPending {
                sig: wire::sign_member(&m_me, &keys(9)),
                member: m_me,
            }),
        };
        b.login();
        assert_eq!(join(&b.mesh, &net, None), Err(MeshProblem::NoOneAnswered));
        assert!(b.mesh.lock().unwrap().invites.is_empty());

        // 端点对得上，但不是它自己的钥匙签的：也不算数。
        let a_me = a.me();
        let net = Swapped {
            asked: a.ep.clone(),
            reply: wire::encode(&Payload::JoinPending {
                sig: wire::sign_member(&a_me, &keys(9)),
                member: a_me,
            }),
        };
        assert_eq!(join(&b.mesh, &net, None), Err(MeshProblem::NoOneAnswered));
        assert!(b.mesh.lock().unwrap().invites.is_empty());
    }

    /// 已经跟别人同组的电脑，不再走「邀请」那条路收名单——哪怕手上还留着
    /// 一条邀请记录（比如问过两台、只有一台批了）。
    #[test]
    fn a_grouped_machine_never_takes_an_invite_roster() {
        let hub = FakeHub::new();
        let (a, b) = ab(&hub);
        let m = Node::new(&hub, 9, "M");
        m.login();
        let code = sas::code(&b.me(), &m.me());
        {
            let mut bm = b.mesh.lock().unwrap();
            bm.set_invites(vec![(m.me(), code)]);
            bm.confirm_inviter(&m.ep).unwrap();
        }
        let r = Roster {
            group: "evil".into(),
            version: 2,
            members: vec![m.me(), b.me()],
        };
        let signed = roster::sign(r, &m.me(), &keys(9));
        m.net
            .send(&b.ep, wire::encode(&Payload::Roster(signed)))
            .unwrap();
        assert_eq!(b.roster(), a.roster(), "还是跟 A 的那一组");
    }

    #[test]
    fn the_same_machine_asking_twice_is_one_pending_request() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        a.login();
        b.login();
        b.join().unwrap();
        b.join().unwrap();
        assert_eq!(a.view().pending.len(), 1);
    }

    #[test]
    fn pending_requests_are_capped() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        a.login();
        for seed in 10..(10 + super::super::MAX_PENDING as u8 + 3) {
            let n = Node::new(&hub, seed, &format!("n{seed}"));
            let req = n.mesh.lock().unwrap().join_request();
            let _ = n
                .net
                .ask(&a.ep, wire::encode(&Payload::Join(req)), JOIN_ASK_TIMEOUT);
        }
        assert_eq!(a.view().pending.len(), super::super::MAX_PENDING);
    }

    #[test]
    fn joining_is_refused_once_already_grouped_with_others() {
        let hub = FakeHub::new();
        let (_a, b) = ab(&hub);
        assert_eq!(b.join(), Err(MeshProblem::AlreadyInGroup));
    }

    #[test]
    fn joining_with_no_one_answering_says_so() {
        let hub = FakeHub::new();
        let b = Node::new(&hub, 2, "B");
        b.login();
        assert_eq!(b.join(), Err(MeshProblem::NoOneAnswered));
    }

    /// `dct join --name`：改名之后发出去的请求带新名字，只有自己的那份名单
    /// 也跟着改；不合规的名字当场拒掉。
    #[test]
    fn join_can_rename_first() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        a.login();
        b.login();
        join(&b.mesh, &b.net, Some("公司Windows")).unwrap();
        assert_eq!(a.view().pending[0].name, "公司Windows");
        assert_eq!(b.names(), ["公司Windows"]);
        assert_eq!(
            join(&b.mesh, &b.net, Some("a/b")),
            Err(MeshProblem::BadName)
        );
        a.approve("公司Windows").unwrap();
        assert_eq!(a.names(), ["A", "公司Windows"]);
    }

    #[test]
    fn approving_a_name_already_in_the_group_is_refused() {
        let hub = FakeHub::new();
        let (a, _b) = ab(&hub);
        let b2 = Node::new(&hub, 7, "B");
        b2.login();
        b2.join().unwrap();
        assert_eq!(a.approve(&b2.ep), Err(MeshProblem::NameTaken("B".into())));
        assert_eq!(a.names(), ["A", "B"]);
    }

    #[test]
    fn approving_nobody_is_refused() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        a.login();
        assert_eq!(
            a.approve("nobody"),
            Err(MeshProblem::NoSuchRequest("nobody".into()))
        );
    }

    /// 正在加入的电脑，只收它核对过数字的那台签的名单：别的电脑（哪怕同
    /// 账号、哪怕签得完全正确）塞过来一份，也不收。
    #[test]
    fn a_joining_machine_only_takes_a_roster_from_a_machine_it_compared_codes_with() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        let m = Node::new(&hub, 9, "M");
        a.login();
        b.login();
        hub.set_online(&m.ep, false);
        b.join().unwrap();
        hub.set_online(&m.ep, true);
        b.confirm(&a).unwrap();

        // M 签一份把 B 加进 M 组的名单，直接发给 B。
        let mut r = Roster {
            group: "evil".into(),
            version: 2,
            members: vec![m.me(), b.me()],
        };
        let signed = roster::sign(r.clone(), &m.me(), &keys(9));
        m.net
            .send(&b.ep, wire::encode(&Payload::Roster(signed)))
            .unwrap();
        assert_eq!(b.names(), ["B"], "没核对过 M，不收 M 的名单");

        // A 签的，但把 B 的加密公钥换掉了：B 解不开发给它的东西，不收。
        let mut b_swapped = b.me();
        b_swapped.kx_pub = STANDARD.encode(keys(8).kx_pub());
        r = Roster {
            group: a.roster().roster.group.clone(),
            version: 2,
            members: vec![a.me(), b_swapped],
        };
        let signed = roster::sign(r, &a.me(), &keys(1));
        a.net
            .send(&b.ep, wire::encode(&Payload::Roster(signed)))
            .unwrap();
        assert_eq!(b.names(), ["B"]);
        assert_eq!(b.view().joining.len(), 1, "还在等");
    }

    /// 移出之后：名单上没有它；它再发来密封留言，一律丢掉。不能移除自己。
    #[test]
    fn a_removed_machine_is_dropped_and_self_removal_is_refused() {
        let hub = FakeHub::new();
        let (a, b) = ab(&hub);
        let c = Node::new(&hub, 3, "C");
        c.login();
        c.join().unwrap();
        c.confirm(&a).unwrap();
        a.approve("C").unwrap();
        assert_eq!(b.names(), ["A", "B", "C"]);
        assert_eq!(c.names(), ["A", "B", "C"]);

        assert_eq!(
            remove(&a.mesh, &a.net, "A"),
            Err(MeshProblem::CannotRemoveSelf)
        );
        assert_eq!(
            remove(&a.mesh, &a.net, "Z"),
            Err(MeshProblem::NoSuchMachine("Z".into()))
        );

        assert_eq!(remove(&a.mesh, &a.net, "C").unwrap(), "C");
        assert_eq!(a.names(), ["A", "B"]);
        assert_eq!(b.roster(), a.roster(), "B 也收到了");
        assert_eq!(a.roster().roster.version, 4);

        // C 手上还是旧名单，照旧给 A 发一条密封留言：A 不认它了。
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let msg = Message {
            id: "m1".into(),
            kind: Kind::Msg,
            from: c.ep.clone(),
            from_session: "#1".into(),
            to: a.ep.clone(),
            to_session: "#2".into(),
            body: "hi".into(),
            sent_at: now + 5,
        };
        let a_kx: [u8; 32] = STANDARD.decode(&a.me().kx_pub).unwrap().try_into().unwrap();
        let sealed = seal::seal(&msg, &keys(3), &a_kx, [7; 32]);
        let env = Envelope {
            from: EndpointId::new(c.ep.clone()).unwrap(),
            to: EndpointId::new(a.ep.clone()).unwrap(),
            seq: 1,
            payload: wire::encode(&Payload::Sealed(sealed)),
            recipients: vec![],
        };
        assert_eq!(a.mesh.lock().unwrap().on_envelope(&env), None);
        assert!(a.mesh.lock().unwrap().queues.is_empty());

        // 对照：同样造的一条，发给一台还认 C 的电脑（C 自己名单上的 B 是
        // 旧的，但换个角度——让 C 发给还没收到新名单的一台）会收。这里用
        // 「移出之前的 A」的名单副本来验：证明丢掉是因为移出，不是因为这条
        // 留言本身造得不对。
        let before = Mesh::new(keys(1), "A".into(), Some(c.roster()));
        let mut before = before;
        assert!(before.on_envelope(&env).is_some(), "移出之前这条是会收的");
    }

    /// 名单存不下（目录是个文件）：不换名单，报 `NotSaved`，请求还挂着。
    #[test]
    fn an_approval_that_cannot_be_saved_changes_nothing() {
        let hub = FakeHub::new();
        let t = tempfile::tempdir().unwrap();
        let blocker = t.path().join("mesh");
        std::fs::write(&blocker, b"not a dir").unwrap();
        let a = Node::new(&hub, 1, "A");
        a.login();
        let b = Node::new(&hub, 2, "B");
        b.login();
        b.join().unwrap();
        {
            let mut m = a.mesh.lock().unwrap();
            let taken = std::mem::replace(&mut *m, Mesh::new(keys(1), "A".into(), None));
            *m = taken.with_store(super::super::store::Store::at(blocker.clone()));
        }
        assert_eq!(a.approve("B"), Err(MeshProblem::NotSaved));
        assert_eq!(a.names(), ["A"]);
        assert_eq!(a.view().pending.len(), 1);
        assert_eq!(b.names(), ["B"], "没存下就没发");
    }

    // —— Fix round 1：两边的人都认过数字，才进得了组 ——

    /// 审查给的 PoC（C1）：中转往账号里塞一台自己的电脑 M，让它也回一句
    /// `JoinPending`，然后推一份把 B 签进 M 组的名单。B 的用户只核对了 A
    /// 的数字，M 的名单一律不收——认定之前、认定之后都不收。
    #[test]
    fn a_second_responder_that_was_never_compared_cannot_pull_the_joiner_in() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        let m = Node::new(&hub, 9, "M");
        a.login();
        b.login();
        m.login();

        let codes = b.join().unwrap();
        assert_eq!(codes.len(), 2, "A 和 M 都回了话");
        let evil = || {
            let r = Roster {
                group: m.roster().roster.group.clone(),
                version: 2,
                members: vec![m.me(), b.me()],
            };
            let signed = roster::sign(r, &m.me(), &keys(9));
            m.net
                .send(&b.ep, wire::encode(&Payload::Roster(signed)))
                .unwrap();
        };
        // 用户还没认定哪一台：M 的名单不收。
        evil();
        assert_eq!(b.names(), ["B"]);
        // 用户认定 A：M 先前送来的那份不会因此被收下，再送一份也不收。
        b.confirm(&a).unwrap();
        assert_eq!(b.names(), ["B"]);
        evil();
        assert_eq!(b.names(), ["B"]);
        assert!(b.mesh.lock().unwrap().is_alone());

        // A 批了：进的是 A 的组。
        a.approve("B").unwrap();
        assert_eq!(b.names(), ["A", "B"]);
        assert_eq!(b.roster(), a.roster());
    }

    /// 那边先点了同意、这边后认定：名单先存着，认定那一刻验过就进组。
    #[test]
    fn a_roster_from_the_inviter_that_arrives_before_confirmation_is_taken_on_confirm() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        a.login();
        b.login();
        b.join().unwrap();
        a.approve("B").unwrap();
        assert_eq!(b.names(), ["B"], "没认定之前不进组");
        b.confirm(&a).unwrap();
        assert_eq!(b.names(), ["A", "B"]);
        assert!(b.view().joining.is_empty());
    }

    /// 审查给的 m1：B 还没认定，A 先点了同意，A 的真名单存在 B 那里。这时
    /// M（同账号的另一台，也回过话）送一份自称 `signer = A` 的垃圾名单——
    /// 它不能顶掉 A 那份；B 认定 A 的时候照样进 A 的组。
    #[test]
    fn a_junk_roster_claiming_the_inviter_as_signer_does_not_clobber_the_held_one() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        let m = Node::new(&hub, 9, "M");
        a.login();
        b.login();
        m.login();
        assert_eq!(b.join().unwrap().len(), 2, "A 和 M 都回了话");
        a.approve("B").unwrap();
        assert_eq!(b.names(), ["B"], "没认定之前不进组");

        // M 签的名单，但 `signer` 字段写成 A。
        let r = Roster {
            group: m.roster().roster.group.clone(),
            version: 2,
            members: vec![m.me(), b.me()],
        };
        let mut junk = roster::sign(r, &m.me(), &keys(9));
        junk.signer = a.ep.clone();
        m.net
            .send(&b.ep, wire::encode(&Payload::Roster(junk)))
            .unwrap();

        b.confirm(&a).unwrap();
        assert_eq!(b.names(), ["A", "B"], "A 的真名单还在，认定时进了 A 的组");
        assert_eq!(b.roster(), a.roster());
    }

    /// 邀请过了 `JOIN_TTL` 就作废：认定不了；认定过的，那台再送名单也不收。
    #[test]
    fn invites_expire_after_the_join_ttl() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let t0 = 1_800_000_000;
        let ttl = super::super::JOIN_TTL.as_secs();

        for confirm_first in [false, true] {
            let hub = FakeHub::new();
            let now = Arc::new(AtomicU64::new(t0));
            let clock = now.clone();
            let a = Node::new(&hub, 1, "A");
            let b = Node::with_mesh(
                &hub,
                Mesh::new(keys(2), "B".into(), None)
                    .with_clock(move || clock.load(Ordering::SeqCst)),
            );
            a.login();
            b.login();
            b.join().unwrap();
            if confirm_first {
                b.confirm(&a).unwrap();
            }
            now.store(t0 + ttl, Ordering::SeqCst);
            if !confirm_first {
                // 报的是电脑名，不是端点：用户敲的是名字。
                assert_eq!(b.confirm(&a), Err(MeshProblem::NoSuchInviter("A".into())));
            }
            a.approve("B").unwrap();
            assert_eq!(
                b.names(),
                ["B"],
                "过期的邀请不收（confirm_first={confirm_first}）"
            );
            assert!(b.view().joining.is_empty());
        }

        // 对照：期限之内一秒，照收。
        let hub = FakeHub::new();
        let now = Arc::new(AtomicU64::new(t0));
        let clock = now.clone();
        let a = Node::new(&hub, 1, "A");
        let b = Node::with_mesh(
            &hub,
            Mesh::new(keys(2), "B".into(), None).with_clock(move || clock.load(Ordering::SeqCst)),
        );
        a.login();
        b.login();
        b.join().unwrap();
        b.confirm(&a).unwrap();
        now.store(t0 + ttl - 1, Ordering::SeqCst);
        a.approve("B").unwrap();
        assert_eq!(b.names(), ["A", "B"]);
    }

    #[test]
    fn confirming_a_machine_that_never_answered_is_refused() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        let m = Node::new(&hub, 9, "M");
        a.login();
        b.login();
        hub.set_online(&m.ep, false);
        b.join().unwrap();
        assert_eq!(b.confirm(&m), Err(MeshProblem::NoSuchInviter(m.ep.clone())));
        assert_eq!(b.mesh.lock().unwrap().confirmed, None);
    }

    /// 审查给的 PoC（I1）：B 的请求挂着，攻击者灌一轮 `Join` 想把它挤掉，
    /// 再换上一条同名的冒牌货。表满了就拒新的：B 那一条还在，冒牌货连
    /// 回话都拿不到；用户按屏幕上的端点和数字批，批的就是 B。
    #[test]
    fn flooding_joins_cannot_evict_a_pending_request_or_slip_in_an_impostor() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        a.login();
        b.login();
        b.join().unwrap();
        let shown = a.view().pending[0].clone();
        assert_eq!(shown.endpoint, b.ep);

        let cap = super::super::MAX_PENDING;
        for seed in 10..(10 + cap as u8 + 5) {
            let n = Node::new(&hub, seed, &format!("n{seed}"));
            let req = n.mesh.lock().unwrap().join_request();
            let _ = n
                .net
                .ask(&a.ep, wire::encode(&Payload::Join(req)), JOIN_ASK_TIMEOUT);
        }
        let impostor = Node::new(&hub, 99, "B");
        let req = impostor.mesh.lock().unwrap().join_request();
        assert!(
            impostor
                .net
                .ask(&a.ep, wire::encode(&Payload::Join(req)), JOIN_ASK_TIMEOUT)
                .is_err(),
            "满了，冒牌货拿不到回话"
        );
        let pending = a.view().pending;
        assert_eq!(pending.len(), cap);
        assert!(pending.iter().any(|p| p == &shown), "B 那一条没被挤掉");
        assert!(!pending.iter().any(|p| p.endpoint == impostor.ep));

        // 已经挂着的那台再问一次（比如重跑 dct join）：表满也照样更新。
        b.join().unwrap();
        assert_eq!(a.view().pending.len(), cap);

        b.confirm(&a).unwrap();
        assert_eq!(
            approve(&a.mesh, &a.net, &shown.endpoint, &shown.code, true).unwrap(),
            "B"
        );
        assert_eq!(b.names(), ["A", "B"]);
    }

    #[test]
    fn approving_with_a_code_other_than_the_pending_one_is_refused() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        a.login();
        b.login();
        b.join().unwrap();
        let shown = a.view().pending[0].clone();
        let wrong: String = shown
            .code
            .chars()
            .map(|c| {
                if c == '9' {
                    '0'
                } else {
                    char::from(c as u8 + 1)
                }
            })
            .collect();
        assert_eq!(
            approve(&a.mesh, &a.net, &b.ep, &wrong, true),
            Err(MeshProblem::CodeMismatch)
        );
        assert_eq!(a.names(), ["A"]);
        assert_eq!(a.view().pending.len(), 1, "请求还挂着");
    }

    /// 广播名单是并排发的：一台卡住（中转那头挂满超时）不拖着别的，整次
    /// 广播也就只花一次 `send` 的时间——`mesh::worst_case` 是这么算的。
    #[test]
    fn a_broadcast_takes_one_send_not_one_per_member() {
        struct Slow;
        impl Net for Slow {
            fn peers(&self) -> Result<Vec<String>, crate::link::LinkError> {
                Ok(vec![])
            }
            fn send(&self, _: &str, _: Vec<u8>) -> Result<(), crate::link::LinkError> {
                std::thread::sleep(Duration::from_millis(300));
                Ok(())
            }
            fn ask(
                &self,
                _: &str,
                _: Vec<u8>,
                _: Duration,
            ) -> Result<Vec<u8>, crate::link::LinkError> {
                Err(crate::link::LinkError::Unreachable)
            }
        }
        let mesh = Mutex::new(Mesh::new(keys(1), "A".into(), None));
        let to: Vec<String> = (0..6).map(|i| format!("c-{i:020x}")).collect();
        let t = std::time::Instant::now();
        broadcast(&mesh, &Slow, &to, b"x");
        assert!(
            t.elapsed() < Duration::from_millis(1200),
            "6 台花了 {:?}",
            t.elapsed()
        );
    }
}
