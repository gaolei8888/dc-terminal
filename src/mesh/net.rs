//! 电脑往外发东西的那一侧：问谁在线、投一个信封、投了等答复。
//!
//! 收的那一侧是 `link::Link`（长轮询，在自己的线程上），发的这一侧是这里。
//! 做成 trait 是为了多台电脑的流程（加入、批准、留言）能在一个进程里、不碰
//! 网络地跑完：测试里用 `testing::FakeHub`，几台内存里的 `Mesh` 直接互相调
//! `on_envelope`。
//!
//! **调用 `Net` 的时候别攥着自己那把 `Mesh` 锁。** 真网络上那只是让别的请求
//! 多等一会儿；`FakeHub` 上是同步调用，对方要是回头调到你，就是死锁。
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dct_link::{EndpointId, Envelope};

use crate::link::{self, LinkConfig, LinkError};

pub trait Net: Send + Sync {
    /// 跟我同账号、此刻在线的其它端点。
    fn peers(&self) -> Result<Vec<String>, LinkError>;
    /// 投一个信封，不等答复。
    fn send(&self, to: &str, payload: Vec<u8>) -> Result<(), LinkError>;
    /// 投一个信封，最多等 `timeout` 拿对面的答复。
    fn ask(&self, to: &str, payload: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, LinkError>;
    /// 晚一点再答对方的一次 `ask`：信封的 `seq` 用对方那封的 `seq`（中转靠
    /// `(to, seq)` 把答复配回那次 `ask`，跟 `link::Link::reply_to` 同一条规矩）。
    ///
    /// 远程调用在工作线程上做完才答，不在收件线程上同步答（`remote_host`）。
    /// 对方要是已经不等了，中转把它当普通信封投过去，对方当作不请自来的
    /// 答复丢掉。
    fn reply(&self, to: &str, seq: u64, payload: Vec<u8>) -> Result<(), LinkError>;
}

/// `peers`、`send` 一次最多等多久（连接加读写，整条请求）。`ask` 另按调用方
/// 给的 `timeout` 算。
///
/// 这一侧的调用全是命令行在等着的（`dct peers`、`dct join`、批准……），所以
/// 不能用连接线程那个为长轮询配的 40 秒读超时：守护进程在这里挂多久，命令行
/// 就得等多久，等不起就会在守护进程其实办成了之后报失败。各条请求最坏要花
/// 多久见 `mesh::worst_case`。
pub const CALL_TIMEOUT: Duration = Duration::from_secs(8);

/// 真的走中转。跟 `Link` 共用同一个 `LinkConfig`（于是共用同一格令牌，
/// 续期一次两边都换上），但 HTTP 客户端是自己的一个：超时短，见 `CALL_TIMEOUT`。
pub struct LinkNet {
    cfg: LinkConfig,
    agent: ureq::Agent,
    seq: AtomicU64,
}

impl LinkNet {
    pub fn new(cfg: LinkConfig) -> LinkNet {
        LinkNet::with_call_timeout(cfg, CALL_TIMEOUT)
    }

    /// 同 `new`，但 `peers`/`send` 的整条请求最多等 `timeout`。
    pub fn with_call_timeout(cfg: LinkConfig, timeout: Duration) -> LinkNet {
        let agent = crate::sys::tls::agent_builder().timeout(timeout).build();
        LinkNet {
            cfg,
            agent,
            seq: AtomicU64::new(seq_start()),
        }
    }

    fn envelope(&self, to: &str, payload: Vec<u8>) -> Result<Envelope, LinkError> {
        Ok(Envelope {
            from: self.cfg.endpoint.clone(),
            to: EndpointId::new(to).map_err(|_| LinkError::BadEndpoint)?,
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            payload,
            recipients: vec![],
        })
    }
}

/// `seq` 从毫秒时间戳起步，而不是从 0。中转按 `(from, seq)` 挂 `ask` 的号，
/// 而且**绝不顶掉已经挂着的号**（撞上就回 `Busy`）——守护进程重启之后要是
/// 又从 0 数起，上一个进程还挂在中转上的那几个号就会让头几次 `ask` 平白失败。
fn seq_start() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(1)
}

impl Net for LinkNet {
    fn peers(&self) -> Result<Vec<String>, LinkError> {
        link::peers(&self.cfg, &self.agent).map(|v| v.into_iter().map(String::from).collect())
    }

    fn send(&self, to: &str, payload: Vec<u8>) -> Result<(), LinkError> {
        let env = self.envelope(to, payload)?;
        link::send(&self.cfg, &self.agent, env)
    }

    fn reply(&self, to: &str, seq: u64, payload: Vec<u8>) -> Result<(), LinkError> {
        let env = Envelope {
            from: self.cfg.endpoint.clone(),
            to: EndpointId::new(to).map_err(|_| LinkError::BadEndpoint)?,
            seq,
            payload,
            recipients: vec![],
        };
        link::send(&self.cfg, &self.agent, env)
    }

    fn ask(&self, to: &str, payload: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, LinkError> {
        let env = self.envelope(to, payload)?;
        let (want_from, want_seq) = (env.to.clone(), env.seq);
        let reply = link::ask_within(&self.cfg, &self.agent, env, Some(timeout))?;
        // 中转只按 `(to, seq)` 配对，这里再对一遍：配错了的答复当成没答。
        if reply.from != want_from || reply.seq != want_seq {
            return Err(LinkError::Unreachable);
        }
        Ok(reply.payload)
    }
}

/// 内存里的中转，给多台电脑的流程测试用。
#[cfg(test)]
pub mod testing {
    use super::*;
    use crate::mesh::Mesh;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Condvar, Mutex};

    /// 一次挂着的 `ask`：答复到了就是 `Some`。
    type Slot = Option<Vec<u8>>;

    /// 几台 `Mesh` 按端点登记在这里；`send`/`ask` 直接同步调对方的
    /// `on_envelope`，`peers` 列出其它登记过、且在线的端点。
    ///
    /// 对方当场不答（`on_envelope` 返回 `None`）时，`ask` 跟真中转一样挂着
    /// 等同一个 `seq` 的 `reply`，等到 `timeout` 为止——远程调用就是这样在
    /// 工作线程上做完才答的。
    #[derive(Default)]
    pub struct FakeHub {
        nodes: Mutex<HashMap<String, Arc<Mutex<Mesh>>>>,
        offline: Mutex<HashSet<String>>,
        seq: AtomicU64,
        /// 正挂着等答复的 `ask`，键是 `(提问的端点, seq)`；值是到了的答复。
        waiting: Mutex<HashMap<(String, u64), Slot>>,
        arrived: Condvar,
        /// 设了就像真的 `Link::run` 那样，每台电脑收信封一次只处理一封
        /// （`one_link_thread`）。
        serial: AtomicBool,
        links: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    }

    impl FakeHub {
        pub fn new() -> Arc<FakeHub> {
            Arc::new(FakeHub::default())
        }

        /// 每台电脑只有一条收件线：同一台电脑上，前一封信封的 `on_envelope`
        /// 没回来，后一封就进不去。真守护进程就是这样（`Link::run` 一次处理
        /// 一封）；「慢的远程调用不挡留言」要在这个前提下测才有意义。默认
        /// 不开：有的测试在敲字回调里往同一台电脑再投一封，开了就是死锁。
        pub fn one_link_thread(&self) {
            self.serial.store(true, Ordering::SeqCst);
        }

        /// 把一台 `Mesh` 挂上来，端点就是它自己的端点。
        pub fn register(&self, mesh: Arc<Mutex<Mesh>>) {
            let ep = mesh.lock().unwrap().endpoint().to_string();
            self.nodes.lock().unwrap().insert(ep, mesh);
        }

        pub fn set_online(&self, endpoint: &str, online: bool) {
            let mut off = self.offline.lock().unwrap();
            if online {
                off.remove(endpoint);
            } else {
                off.insert(endpoint.to_string());
            }
        }

        /// 以 `me` 的身份往外发的那个 `Net`。
        pub fn net_for(self: &Arc<Self>, me: &str) -> FakeNet {
            FakeNet {
                hub: self.clone(),
                me: me.to_string(),
            }
        }

        fn deliver(
            &self,
            from: &str,
            to: &str,
            seq: u64,
            payload: Vec<u8>,
        ) -> Result<Option<Vec<u8>>, LinkError> {
            let to_id = EndpointId::new(to).map_err(|_| LinkError::BadEndpoint)?;
            if self.offline.lock().unwrap().contains(to) {
                return Err(LinkError::Relay(dct_link::LinkError::Offline));
            }
            // 先把对方拿出来、放掉表锁，再去锁对方——对方的 `on_envelope`
            // 里要是又用到这张表，不至于自己跟自己抢。
            let target = self.nodes.lock().unwrap().get(to).cloned();
            let Some(target) = target else {
                return Err(LinkError::Relay(dct_link::LinkError::Offline));
            };
            let env = Envelope {
                from: EndpointId::new(from).map_err(|_| LinkError::BadEndpoint)?,
                to: to_id,
                seq,
                payload,
                recipients: vec![],
            };
            let line = if self.serial.load(Ordering::SeqCst) {
                Some(self.links.lock().unwrap().entry(to.to_string()).or_default().clone())
            } else {
                None
            };
            let _one_at_a_time = line.as_ref().map(|l| l.lock().unwrap());
            // 跟 `route` 同一个入口：往会话里敲字在对方的锁外做。
            let reply = crate::mesh::handle(&target, &env);
            Ok(reply)
        }
    }

    pub struct FakeNet {
        hub: Arc<FakeHub>,
        me: String,
    }

    impl Net for FakeNet {
        fn peers(&self) -> Result<Vec<String>, LinkError> {
            let off = self.hub.offline.lock().unwrap().clone();
            let mut out: Vec<String> = self
                .hub
                .nodes
                .lock()
                .unwrap()
                .keys()
                .filter(|ep| **ep != self.me && !off.contains(*ep))
                .cloned()
                .collect();
            out.sort();
            Ok(out)
        }

        fn send(&self, to: &str, payload: Vec<u8>) -> Result<(), LinkError> {
            let seq = self.hub.seq.fetch_add(1, Ordering::Relaxed);
            self.hub.deliver(&self.me, to, seq, payload).map(|_| ())
        }

        fn ask(&self, to: &str, payload: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, LinkError> {
            let hub = &self.hub;
            let seq = hub.seq.fetch_add(1, Ordering::Relaxed);
            let key = (self.me.clone(), seq);
            // 跟真中转一样先挂号再投递：答得快的工作线程不会扑空。
            hub.waiting.lock().unwrap().insert(key.clone(), None);
            let forget = || {
                hub.waiting.lock().unwrap().remove(&key);
            };
            match hub.deliver(&self.me, to, seq, payload) {
                Err(e) => {
                    forget();
                    Err(e)
                }
                Ok(Some(r)) => {
                    forget();
                    Ok(r)
                }
                Ok(None) => {
                    let deadline = std::time::Instant::now() + timeout;
                    let mut w = hub.waiting.lock().unwrap();
                    loop {
                        if let Some(Some(_)) = w.get(&key) {
                            return Ok(w.remove(&key).flatten().unwrap());
                        }
                        let left = deadline.saturating_duration_since(std::time::Instant::now());
                        if left.is_zero() {
                            w.remove(&key);
                            return Err(LinkError::Relay(dct_link::LinkError::NoAnswer));
                        }
                        w = hub.arrived.wait_timeout(w, left).unwrap().0;
                    }
                }
            }
        }

        fn reply(&self, to: &str, seq: u64, payload: Vec<u8>) -> Result<(), LinkError> {
            let hub = &self.hub;
            {
                let mut w = hub.waiting.lock().unwrap();
                if let Some(slot @ None) = w.get_mut(&(to.to_string(), seq)) {
                    *slot = Some(payload);
                    hub.arrived.notify_all();
                    return Ok(());
                }
            }
            // 没人挂着等（已经超时走了）：真中转把它当普通信封投给对方。
            hub.deliver(&self.me, to, seq, payload).map(|_| ())
        }
    }

    /// 中转可以对经过它的东西做什么：发出去之前改（或者换成别的），答复回来
    /// 之后改（或者吞掉）。包在一台电脑自己的 `FakeNet` 外面，替它往外发。
    type Outgoing = Box<dyn Fn(&str, Vec<u8>) -> Vec<u8> + Send + Sync>;
    type Incoming = Box<dyn Fn(&str, Vec<u8>) -> Result<Vec<u8>, LinkError> + Send + Sync>;

    pub struct EvilNet {
        pub inner: FakeNet,
        pub outgoing: Outgoing,
        pub incoming: Incoming,
        /// 真的发出去的每一条（改过之后的）：`(发给谁, payload)`。
        pub sent: Mutex<Vec<(String, Vec<u8>)>>,
    }

    impl EvilNet {
        /// 什么都不改，只记账。
        pub fn honest(inner: FakeNet) -> EvilNet {
            EvilNet {
                inner,
                outgoing: Box::new(|_, p| p),
                incoming: Box::new(|_, r| Ok(r)),
                sent: Mutex::new(Vec::new()),
            }
        }

        pub fn outgoing(mut self, f: impl Fn(&str, Vec<u8>) -> Vec<u8> + Send + Sync + 'static) -> EvilNet {
            self.outgoing = Box::new(f);
            self
        }

        pub fn incoming(
            mut self,
            f: impl Fn(&str, Vec<u8>) -> Result<Vec<u8>, LinkError> + Send + Sync + 'static,
        ) -> EvilNet {
            self.incoming = Box::new(f);
            self
        }

        /// 发出去的 payload 里 `"t"` 是 `tag` 的有几条。
        pub fn count(&self, tag: &str) -> usize {
            let want = format!("\"t\":\"{tag}\"");
            self.sent
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, p)| String::from_utf8_lossy(p).contains(&want))
                .count()
        }
    }

    impl Net for EvilNet {
        fn peers(&self) -> Result<Vec<String>, LinkError> {
            self.inner.peers()
        }

        fn send(&self, to: &str, payload: Vec<u8>) -> Result<(), LinkError> {
            let p = (self.outgoing)(to, payload);
            self.sent.lock().unwrap().push((to.to_string(), p.clone()));
            self.inner.send(to, p)
        }

        fn ask(&self, to: &str, payload: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, LinkError> {
            let p = (self.outgoing)(to, payload);
            self.sent.lock().unwrap().push((to.to_string(), p.clone()));
            let r = self.inner.ask(to, p, timeout)?;
            (self.incoming)(to, r)
        }

        fn reply(&self, to: &str, seq: u64, payload: Vec<u8>) -> Result<(), LinkError> {
            let p = (self.outgoing)(to, payload);
            self.sent.lock().unwrap().push((to.to_string(), p.clone()));
            self.inner.reply(to, seq, p)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net() -> LinkNet {
        LinkNet::new(LinkConfig::new(
            "http://127.0.0.1:9",
            EndpointId::new("c-aaaa").unwrap(),
            "t",
        ))
    }

    #[test]
    fn envelopes_come_from_me_and_their_seq_goes_up() {
        let n = net();
        let a = n.envelope("c-bbbb", b"x".to_vec()).unwrap();
        let b = n.envelope("c-bbbb", b"y".to_vec()).unwrap();
        assert_eq!(a.from.as_str(), "c-aaaa");
        assert_eq!(a.to.as_str(), "c-bbbb");
        assert_eq!(b.seq, a.seq + 1);
    }

    /// 不从 0 数起，见 `seq_start`。
    #[test]
    fn seq_does_not_restart_from_zero() {
        assert!(net().envelope("c-bbbb", vec![]).unwrap().seq > 1_000_000);
    }

    /// 中转收了连接却一个字也不回（防火墙吞包、中转卡死）：`peers`/`send`
    /// 要在 `CALL_TIMEOUT` 左右放弃，不能挂满连接线程那 40 秒的读超时——
    /// 守护进程里一次 `dct peers approve` 要是挂 40 秒，命令行早就报「没
    /// 响应」了，而名单其实已经签好、发出去了。
    #[test]
    fn a_relay_that_never_answers_is_given_up_on_within_the_call_timeout() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // 收下连接、攥着不放、一个字不回。
        let held = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let held = held.clone();
            std::thread::spawn(move || {
                for c in listener.incoming().flatten() {
                    held.lock().unwrap().push(c);
                }
            });
        }
        let n = LinkNet::with_call_timeout(
            LinkConfig::new(
                format!("http://{addr}"),
                EndpointId::new("c-aaaa").unwrap(),
                "t",
            ),
            Duration::from_millis(300),
        );
        let t = std::time::Instant::now();
        assert_eq!(n.peers(), Err(LinkError::Unreachable));
        assert_eq!(n.send("c-bbbb", vec![]), Err(LinkError::Unreachable));
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "两次调用花了 {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn the_production_call_timeout_is_short() {
        assert!(CALL_TIMEOUT <= Duration::from_secs(10));
    }

    #[test]
    fn a_bad_address_is_refused_before_anything_is_sent() {
        assert_eq!(
            net().send("not an id!", vec![]),
            Err(LinkError::BadEndpoint)
        );
    }
}
