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
}

/// 真的走中转。跟 `Link` 共用同一个 `LinkConfig`（于是共用同一格令牌，
/// 续期一次两边都换上）。
pub struct LinkNet {
    cfg: LinkConfig,
    agent: ureq::Agent,
    seq: AtomicU64,
}

impl LinkNet {
    pub fn new(cfg: LinkConfig) -> LinkNet {
        let agent = link::agent(&cfg);
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
    use std::sync::{Arc, Mutex};

    /// 几台 `Mesh` 按端点登记在这里；`send`/`ask` 直接同步调对方的
    /// `on_envelope`，`peers` 列出其它登记过、且在线的端点。
    #[derive(Default)]
    pub struct FakeHub {
        nodes: Mutex<HashMap<String, Arc<Mutex<Mesh>>>>,
        offline: Mutex<HashSet<String>>,
        seq: AtomicU64,
    }

    impl FakeHub {
        pub fn new() -> Arc<FakeHub> {
            Arc::new(FakeHub::default())
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
                seq: self.seq.fetch_add(1, Ordering::Relaxed),
                payload,
                recipients: vec![],
            };
            let reply = target.lock().unwrap().on_envelope(&env);
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
            self.hub.deliver(&self.me, to, payload).map(|_| ())
        }

        fn ask(
            &self,
            to: &str,
            payload: Vec<u8>,
            _timeout: Duration,
        ) -> Result<Vec<u8>, LinkError> {
            self.hub
                .deliver(&self.me, to, payload)?
                .ok_or(LinkError::Relay(dct_link::LinkError::NoAnswer))
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

    #[test]
    fn a_bad_address_is_refused_before_anything_is_sent() {
        assert_eq!(
            net().send("not an id!", vec![]),
            Err(LinkError::BadEndpoint)
        );
    }
}
