### Task 4: 新电脑那边——`join`、名单验收、对抗测试

**Files:**
- Modify: `src/mesh/invite.rs`（加 `join`、`attempt`、`settle`；测试模块尾部接着加 B 端的测试）
- Modify: `src/proto.rs`（`MeshProblem` 加 `BadInviteCode`、`WrongInviteCode`、`NoInvite { unanswered: u32 }`、`InviteRosterRefused`）
- Modify: `src/i18n.rs`（这四个的文案，和「每个错误码都组得出话、英文没有汉字」那张表）

**Interfaces:**
- Consumes: Task 1 的 `Code::parse`、`Handshake::joiner`、`Confirmed::inviter_tag_ok`/`joiner_tag`；Task 2 的 `Payload`；Task 3 的 `INVITE_ASK_TIMEOUT`、`MAX_INVITERS_TRIED`、`tests::{Node, raw_join, T0, keys}`、`EvilNet`；已有的 `mesh::MAX_JOIN_ASK`（16）、`Mesh::is_alone`、`Mesh::fresh_nonce`（还叫这个名字，返回 32 字节，Task 8 改签名）、`roster::accept_invite(incoming: &SignedRoster, me_endpoint: &str, inviter: &Member) -> Result<(), RosterError>`、`Store::set_name`。
- Produces：
  - `mesh::invite::join(mesh: &Mutex<Mesh>, net: &dyn Net, code: &str, name: Option<&str>) -> Result<(), MeshProblem>`
  - `MeshProblem::{BadInviteCode, WrongInviteCode, NoInvite { unanswered: u32 }, InviteRosterRefused}`

**`join` 的规矩：**
1. 码先 `Code::parse`（去空格、`-`、全角），不对 → `BadInviteCode`，一个字都不发；名字不合规 → `BadName`；已经跟别人同组 → `AlreadyInGroup`。
2. 在线列表排序去重，超过 16 台 → `TooManyAnswered`，一台都不问。
3. 并排 `InviteProbe`；`InviteOpen` 要 `member.endpoint == 问的那台`、自签名对、名字合法、`kx_pub` 合法、`group` 非空才算发邀请的电脑；`NoInvite` 不计；没回话/回了解不出来的/`InviteOpen` 验不过的算 `unanswered`。一台都没有 → `NoInvite { unanswered }`。
4. 按端点排、最多试 3 台：`InviteJoin` → `InviteKey` → 先验 `cA`，**不对就停，不发 `InviteFinish`** → `InviteFinish` → `InviteDone`。名单要过 `accept_invite(roster, 我, 那台A)`、`roster.group == InviteOpen 里的 group`、我那一条的两把公钥是我的。
5. 都没成：有一台走到最后名单被拒 → `InviteRosterRefused`，否则 `WrongInviteCode`。
6. **收到 `InviteDone` 之前本地一样都不改**；进组之后才落盘，`--name` 给过就 `set_name` 并把 `auto_name` 置 false。

- [ ] **Step 1: 写失败的测试**

在 `src/mesh/invite.rs` 测试模块的**最后一个 `}` 之前**接上：

```rust
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
    /// 信息）。B 重来一次，中转不捣乱，就进得来。
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
        assert_eq!(join_via(&b, &b.net, &v.code), Ok(()));
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
        assert_eq!(a.names(), ["A", "公司电脑"], "A 那边已经签进去了（已知：要 dct peers remove）");
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
    #[test]
    fn with_two_inviters_the_wrong_one_is_burned_and_the_right_one_takes_b_in() {
        let hub = FakeHub::new();
        let x = Node::grouped(&hub, 1, "X");
        let y = Node::grouped(&hub, 3, "Y");
        let b = Node::grouped(&hub, 2, "B");
        let (first, second) = if x.ep < y.ep { (&x, &y) } else { (&y, &x) };
        first.invite();
        let v = second.invite();
        let v_first = first.mesh.lock().unwrap().invite_view().unwrap();
        if v_first.code == v.code {
            return; // 百万分之一：两个码一样，这一轮测不出。
        }
        assert_eq!(join_via(&b, &b.net, &v.code), Ok(()));
        assert_eq!(b.roster().signer, second.ep);
        first.at(T0 + FINISH_WITHIN_SECS + 1);
        assert_eq!(first.note().unwrap().outcome, InviteOutcome::Burned);
        assert_eq!(first.names(), [first.me().name]);
    }

    #[test]
    fn at_most_three_inviters_are_tried() {
        let hub = FakeHub::new();
        let b = Node::grouped(&hub, 2, "B");
        let inviters: Vec<Node> = (10..14).map(|s| Node::grouped(&hub, s, &format!("I{s}"))).collect();
        for n in &inviters {
            n.invite();
        }
        let codes: Vec<String> = inviters
            .iter()
            .map(|n| n.mesh.lock().unwrap().invite_view().unwrap().code)
            .collect();
        let unused = (0..1_000_000)
            .map(|i| format!("{i:06}"))
            .find(|c| !codes.contains(c))
            .unwrap();
        let net = EvilNet::honest(hub.net_for(&b.ep));
        assert_eq!(join_via(&b, &net, &unused), Err(MeshProblem::WrongInviteCode));
        assert_eq!(net.count("invite_probe"), 4);
        assert_eq!(net.count("invite_join"), MAX_INVITERS_TRIED);
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
```

在 `src/i18n.rs` 的 `every_error_code_has_words_in_both_languages`（那张 `codes` 表，`Mesh(crate::proto::MeshProblem::BadAddress("pc".into())),` 那一行后面）加：

```rust
            Mesh(crate::proto::MeshProblem::BadInviteCode),
            Mesh(crate::proto::MeshProblem::WrongInviteCode),
            Mesh(crate::proto::MeshProblem::NoInvite { unanswered: 0 }),
            Mesh(crate::proto::MeshProblem::NoInvite { unanswered: 2 }),
            Mesh(crate::proto::MeshProblem::InviteRosterRefused),
```

- [ ] **Step 2: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test --lib mesh::invite`
Expected: 编译失败：`cannot find function `join``、`no variant `BadInviteCode``。

- [ ] **Step 3: 实现——`proto.rs`、`i18n.rs`**

先把 Step 1 里手改过的文件还原（补丁里已经包含同样的测试）：`git checkout -- src/i18n.rs`。

把下面整个补丁存成 `/tmp/dct-invite-t4.patch`（**只存 ```diff 块里的内容**），在 worktree 根目录应用：

```bash
git apply --check /tmp/dct-invite-t4.patch && git apply /tmp/dct-invite-t4.patch
```

`--check` 报错说明前面的任务跟本计划的代码有出入：改用 `git apply --3way`，还不行就照补丁逐块手改（上下文行用来定位），**不要**跳过任何一块。

```diff
diff --git a/src/i18n.rs b/src/i18n.rs
index 92f2894..9940c5d 100644
--- a/src/i18n.rs
+++ b/src/i18n.rs
@@ -2046,6 +2046,37 @@ pub mod msg {
                 en: format!("{to} is not an address. Write it as computer/session, for example: office-pc/dc-terminal"),
                 zh: format!("{to} 不是一个地址。写成 电脑名/会话名，比如：公司Windows/dc-terminal"),
             ),
+            BadInviteCode => t!(
+                lang,
+                en: "The invite code is 6 digits, for example: dct join 482913".to_string(),
+                zh: "邀请码是 6 位数字，比如：dct join 482913".to_string(),
+            ),
+            WrongInviteCode => t!(
+                lang,
+                en: "The invite code is wrong or no longer valid. On your other computer press a to get a new one".to_string(),
+                zh: "邀请码不对或已作废，请在老电脑上按 a 重新生成".to_string(),
+            ),
+            NoInvite { unanswered } => {
+                let base = t!(
+                    lang,
+                    en: "None of your computers online is offering an invite code (it may have expired or been used). On your other computer press a (or run dct invite) to get one. If that does not help, check that both computers use the same DC account and that dct is running on the other one".to_string(),
+                    zh: "没有在线的电脑发出邀请（邀请码可能已过期或已作废）。在老电脑上按 a（或运行 dct invite）出一个码；还不行就检查两台是不是同一个 DC 账号、老电脑的 dct 开着没有".to_string(),
+                );
+                if *unanswered == 0 {
+                    base
+                } else {
+                    t!(
+                        lang,
+                        en: format!("{base}. {unanswered} computer(s) did not answer at all: their dct may be too old, update it first"),
+                        zh: format!("{base}。有 {unanswered} 台电脑没回话：它们的 dct 可能太旧，先升级"),
+                    )
+                }
+            }
+            InviteRosterRefused => t!(
+                lang,
+                en: "Your other computer has already put this one in the group, but the group list it sent does not check out here, so this computer did not join. On the other computer run dct peers remove <this computer's name>, then press a for a new code".to_string(),
+                zh: "老电脑那边已经把这台电脑记进组里了，但它发来的名单在这边验不过，没有加入。在老电脑上运行 dct peers remove <这台电脑的名字>，再按 a 重新生成".to_string(),
+            ),
         }
     }
 
@@ -3268,6 +3299,11 @@ mod tests {
             Mesh(crate::proto::MeshProblem::CodeMismatch),
             Mesh(crate::proto::MeshProblem::TooLong),
             Mesh(crate::proto::MeshProblem::BadAddress("pc".into())),
+            Mesh(crate::proto::MeshProblem::BadInviteCode),
+            Mesh(crate::proto::MeshProblem::WrongInviteCode),
+            Mesh(crate::proto::MeshProblem::NoInvite { unanswered: 0 }),
+            Mesh(crate::proto::MeshProblem::NoInvite { unanswered: 2 }),
+            Mesh(crate::proto::MeshProblem::InviteRosterRefused),
             // `LoginFailed` 不在这里：它带的原因是网关那层给的中文，同 `Git`
             // 照抄原文，英文里会有汉字，见下面那条单独的测试。
         ];
diff --git a/src/proto.rs b/src/proto.rs
index 3983cfb..5b474ff 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -1227,6 +1227,16 @@ pub enum MeshProblem {
     TooLong,
     /// 地址不是 `电脑名/会话名`。
     BadAddress(String),
+    /// `dct join` 后面敲的不是 6 位数字（空格、`-` 已经去掉了）。
+    BadInviteCode,
+    /// 码不对、已过期或已被用掉：发邀请的那台已经把它作废了。
+    WrongInviteCode,
+    /// 同账号在线的电脑里没有一台发着邀请码。其中 `unanswered` 台连话都没回
+    /// ——多半是 dct 太旧，不认识 `InviteProbe`。
+    NoInvite { unanswered: u32 },
+    /// 老电脑已经把这台签进了名单，但发来的那份名单这边验不过（签名、组、
+    /// 我的钥匙对不上），没进组。
+    InviteRosterRefused,
 }
 
 /// 把一个 `ErrorCode` 塞进 `anyhow::Error` 里带出去。
```

- [ ] **Step 4: 实现——`join`**

放在 `src/mesh/invite.rs` 里 `flush_outbox` 之后、测试模块之前：

```rust
/// 新电脑上 `dct join <码> [--name 名字]`。
///
/// 1. 问一遍同账号在线的电脑（最多 `MAX_JOIN_ASK` 台，排序去重）：`InviteProbe`；
/// 2. 答 `InviteOpen` 的（自签对、端点就是问的那台）按端点排好，最多试
///    `MAX_INVITERS_TRIED` 台，逐台走 SPAKE2：`InviteJoin` → 验 `cA` →
///    `InviteFinish` → 收 `InviteDone` 里的名单；
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
    inviters.sort_by(|x, y| x.0.endpoint.cmp(&y.0.endpoint));
    inviters.truncate(MAX_INVITERS_TRIED);

    let mut refused = false;
    for (inviter, group) in &inviters {
        match attempt(mesh, net, &code, inviter, group, &me, &sig) {
            Ok(roster) => return settle(mesh, roster, inviter, chosen.is_some()),
            Err(Attempt::Mismatch) => {}
            Err(Attempt::Unreachable) => {}
            Err(Attempt::BadRoster) => refused = true,
        }
    }
    Err(if refused {
        MeshProblem::InviteRosterRefused
    } else {
        MeshProblem::WrongInviteCode
    })
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
```

- [ ] **Step 5: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib mesh::invite && ~/.cargo/bin/cargo test --lib i18n`
Expected: `mesh::invite::tests::` 44 条全过，i18n 全过。

- [ ] **Step 6: 全量检查**

```bash
~/.cargo/bin/cargo test --workspace -- --test-threads=4
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
```
Expected：`test result: ok.` 全绿（`pty::tests::*` / `session::tests::*` 偶发失败见 Global Constraints 的「已知 flaky」，单独重跑）；clippy `Finished`、无 warning；Windows check `Finished`（`src/student_projects.rs:669` 那条 `unused variable: path` 是本来就有的）。

- [ ] **Step 7: Commit**

```bash
git add src/mesh/invite.rs src/proto.rs src/i18n.rs
git commit -m "feat(mesh): joiner side of the invite code, with relay-tampering tests" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
