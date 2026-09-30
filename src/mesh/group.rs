//! 要跟别的电脑说话的那几件事：看一眼现状、移除，以及加电脑（`invite`）
//! 用到的签名单、广播。
//!
//! 全都是「先在锁里算好、放锁、再发」：**调用 `Net` 的时候绝不攥着 `Mesh`
//! 那把锁**（见 `net` 模块头）。`FakeHub` 上发出去是同步调到对方的
//! `on_envelope`，对方要是回头调到我，攥着锁就是死锁；真网络上一次 `ask`
//! 能挂好几秒，那段时间里收件的轮询线程也要这把锁。
use std::collections::HashSet;
use std::sync::Mutex;

use dct_mesh::roster::{self, Roster};
use dct_mesh::wire::{self, Payload};

use super::deliver::clean_name;
use super::net::Net;
use super::Mesh;
use crate::proto::{MemberView, MeshProblem, MeshView};

fn lock(m: &Mutex<Mesh>) -> std::sync::MutexGuard<'_, Mesh> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 现状。`online` 来自 `net.peers()`，问不到就当谁都不在线。
pub fn view(mesh: &Mutex<Mesh>, net: &dyn Net, logged_in: bool) -> MeshView {
    let online: HashSet<String> = net.peers().unwrap_or_default().into_iter().collect();
    let mut m = lock(mesh);
    let me = m.me.endpoint.clone();
    let members = m
        .roster
        .as_ref()
        .map(|r| {
            r.roster
                .members
                .iter()
                .map(|x| MemberView {
                    name: clean_name(&x.name),
                    endpoint: x.endpoint.clone(),
                    online: x.endpoint == me || online.contains(&x.endpoint),
                    is_me: x.endpoint == me,
                })
                .collect()
        })
        .unwrap_or_default();
    MeshView {
        logged_in,
        name: clean_name(&m.me.name),
        endpoint: me,
        in_group: m.roster.is_some(),
        members,
        messages: m.delivered_counts(),
        // 先 `invite_view`：它顺手把到点的码清掉、记下结果，下面读到的
        // `invite_note` 才是新的。
        invite: m.invite_view(),
        invite_note: m.invite_note.clone(),
    }
}

/// `Mac` 撞了名就是 `Mac 2`，再撞 `Mac 3`……名字本来就接近上限
/// （`roster::MAX_NAME_LEN` 个字）的，先截短再编号，编出来的还是合法名字。
pub(super) fn free_name(base: &str, taken: &[String]) -> String {
    (2..)
        .map(|n| {
            let suffix = format!(" {n}");
            let keep = dct_mesh::roster::MAX_NAME_LEN - suffix.chars().count();
            let head: String = base.chars().take(keep).collect();
            format!("{}{suffix}", head.trim_end())
        })
        .find(|c| !taken.contains(c))
        .unwrap_or_default()
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

pub(super) fn sign_next(m: &Mesh, current: &Roster, members: Vec<dct_mesh::Member>) -> dct_mesh::SignedRoster {
    let next = Roster {
        group: current.group.clone(),
        version: current.version + 1,
        members,
    };
    roster::sign(next, &m.me, &m.keys)
}

pub(super) fn recipients(m: &Mesh, r: &dct_mesh::SignedRoster) -> Vec<String> {
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
pub(super) fn broadcast(mesh: &Mutex<Mesh>, net: &dyn Net, to: &[String], payload: &[u8]) {
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
    use dct_mesh::{MachineKeys, Member};
    use std::sync::Arc;
    use std::time::Duration;

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
            let mesh = Arc::new(Mutex::new(Mesh::new(keys(seed), name.into(), None)));
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

        /// 我出一个码，`other` 拿它 `dct join`；新名单再发给组里其余的电脑
        /// （守护进程的投递线程做的那一下）。
        fn invite_in(&self, other: &Node) {
            let v = self.mesh.lock().unwrap().start_invite().unwrap();
            crate::mesh::invite::join(&other.mesh, &other.net, &v.code, None).unwrap();
            crate::mesh::invite::flush_outbox(&self.mesh, &self.net);
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

    /// A 建组、用邀请码把 B 拉进来。
    fn ab(hub: &Arc<FakeHub>) -> (Node, Node) {
        let a = Node::new(hub, 1, "A");
        let b = Node::new(hub, 2, "B");
        assert!(a.login());
        assert!(b.login());
        a.invite_in(&b);
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

    /// 整条路：A 登录 → A 出码、B 加入 → B 出码、C 加入 → A 也收到 B 签的
    /// 新名单。组里任何一台都能发邀请。
    #[test]
    fn three_computers_join_one_after_another() {
        let hub = FakeHub::new();
        let (a, b) = ab(&hub);
        assert_eq!(a.names(), ["A", "B"]);
        assert_eq!(b.roster(), a.roster(), "B 从 A 那里拿到了同一份名单");
        assert_eq!(b.roster().roster.version, 2);
        let bv = b.view();
        assert_eq!(bv.members.len(), 2);
        assert!(bv.members.iter().all(|m| m.online));

        let c = Node::new(&hub, 3, "C");
        assert!(c.login());
        b.invite_in(&c);
        assert_eq!(b.names(), ["A", "B", "C"]);
        assert_eq!(a.roster(), b.roster(), "A 也收到了新名单");
        assert_eq!(c.roster(), b.roster());
        assert_eq!(a.roster().roster.version, 3);
        assert_eq!(a.roster().signer, b.ep);
    }

    /// 只有自己一台的电脑，不从网上接别人签的名单——哪怕名单里有它、签得
    /// 完全正确。进别人的组只有 `invite::join` 一条路。
    #[test]
    fn a_lone_machine_does_not_take_a_roster_off_the_wire() {
        let hub = FakeHub::new();
        let b = Node::new(&hub, 2, "B");
        let m = Node::new(&hub, 9, "M");
        b.login();
        m.login();
        let r = Roster {
            group: m.roster().roster.group.clone(),
            version: 2,
            members: vec![m.me(), b.me()],
        };
        let signed = roster::sign(r, &m.me(), &keys(9));
        m.net
            .send(&b.ep, wire::encode(&Payload::Roster(signed)))
            .unwrap();
        assert_eq!(b.names(), ["B"]);
    }

    /// 移出之后：名单上没有它；它再发来密封留言，一律丢掉。不能移除自己。
    #[test]
    fn a_removed_machine_is_dropped_and_self_removal_is_refused() {
        let hub = FakeHub::new();
        let (a, b) = ab(&hub);
        let c = Node::new(&hub, 3, "C");
        c.login();
        a.invite_in(&c);
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

        // 对照：拿「移出之前」的名单副本验同一条——证明丢掉是因为移出，
        // 不是因为这条留言本身造得不对。
        let mut before = Mesh::new(keys(1), "A".into(), Some(c.roster()));
        assert!(before.on_envelope(&env).is_some(), "移出之前这条是会收的");
    }

    /// 一台卡住不拖着别的：广播 6 台只花一次 `send` 的时间——
    /// `mesh::worst_case` 是这么算的。
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

    /// 名单里别的电脑的名字，交出去之前洗干净。
    #[test]
    fn the_view_hands_out_cleaned_names() {
        let ka = keys(1);
        let me = Mesh::new(keys(1), "A".into(), None).me.clone();
        let mut other = Mesh::new(keys(2), "B".into(), None).me.clone();
        other.name = "B\x1b[8m\u{202e}x".into();
        let r = Roster {
            group: "g".into(),
            version: 2,
            members: vec![me.clone(), other],
        };
        let signed = roster::sign(r, &me, &ka);
        let mesh = Mutex::new(Mesh::new(ka, "A".into(), Some(signed)));
        let hub = FakeHub::new();
        let v = view(&mesh, &hub.net_for(&me.endpoint), true);
        let names: Vec<&str> = v.members.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"Bx"), "{names:?}");
    }

    /// 现状里带着发着的码；码用掉之后码没了、结果在。
    #[test]
    fn the_view_carries_the_live_code_and_then_its_outcome() {
        let hub = FakeHub::new();
        let a = Node::new(&hub, 1, "A");
        let b = Node::new(&hub, 2, "B");
        a.login();
        b.login();
        let v = a.mesh.lock().unwrap().start_invite().unwrap();
        assert_eq!(a.view().invite, Some(v.clone()));
        assert_eq!(a.view().invite_note, None);
        crate::mesh::invite::join(&b.mesh, &b.net, &v.code, None).unwrap();
        let after = a.view();
        assert_eq!(after.invite, None);
        assert_eq!(
            after.invite_note,
            Some(crate::proto::InviteNote {
                id: v.id,
                outcome: crate::proto::InviteOutcome::Joined { name: "B".into() }
            })
        );
        assert_eq!(b.view().invite, None, "B 那边没有码");
    }
}
