//! 配对状态机：跟训练营网关换一把钥匙。
//!
//! **这个文件不碰网络。** 传输层由调用方注入，理由跟 `verify.rs::verify_with`
//! 一模一样——那边一句注释说得很清楚：测试要能覆盖 401、网络错、奇怪返回码，
//! 而不用真打网络。这里要覆盖的更多：过期、拒绝、429 退避、空钥匙、重复 approved。
//!
//! 契约见 `docs/superpowers/specs/2026-09-02-dct-pair-with-dc-llm-design.md`
//! 的「三个接口：冻结的线上契约」。**字段名和类型以那一节为准**，网关那边
//! 是照着它实现的，这里改一个名字就是两个仓库对不上。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// `POST /admin/api/pair/start` 的响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Started {
    pub device_code: String,
    pub user_code: String,
    /// **路径，不是完整 URL。** origin 由 dct 自己拼——见 spec 里那段：
    /// `/pair/start` 无认证，而 dct 拿到这个字符串直接开浏览器。
    pub verify_path: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Wire {
    Anthropic,
    Openai,
}

/// 一个方言口下这个账号能用的两个模型。两个都可能没有：Claude 是付费限定，
/// 免费账号的 `anthropic` 这一组是空的。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireModels {
    pub default: Option<String>,
    pub small_fast: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Models {
    #[serde(default)]
    pub anthropic: WireModels,
    #[serde(default)]
    pub openai: WireModels,
}

/// 配对成功那一刻的额度快照。`window` 才是真正会拦住学生的那个限额——
/// 一轮 Claude Code 的对话在「贵」之前先「长」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quota {
    pub used_micro: i64,
    pub limit_micro: Option<i64>,
    #[serde(default)]
    pub window: BTreeMap<String, Window>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub used_tokens: i64,
    pub limit_tokens: i64,
    pub resets_at: Option<String>,
}

/// `POST /admin/api/pair/poll` 的响应，已经从 JSON 归一化过。
#[derive(Debug, Clone)]
pub enum Poll {
    Pending,
    Approved {
        api_key: String,
        models: Models,
        platforms: BTreeMap<String, String>,
        quota: Option<Quota>,
    },
    Denied,
    Claimed,
    Expired {
        reason: String,
        message: String,
    },
    RateLimited,
    /// 开关关着，网关三个接口一律 404。
    NotEnabled,
}

/// 状态机对外的那一面。UI 认这个，不认 HTTP。
#[derive(Debug, Clone)]
pub enum Tick {
    Waiting,
    Done(Box<Approved>),
    /// `retryable` = 按 `r` 换一串码有意义。`ttl` 到点是 true，
    /// `key_unreadable` 是 false——那一种按多少次都走到同一个地方。
    Expired {
        retryable: bool,
        message: String,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Approved {
    pub api_key: String,
    pub models: Models,
    pub platforms: BTreeMap<String, String>,
    pub quota: Option<Quota>,
}

const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub struct Machine {
    pub started: Started,
    pub tick: Tick,
    pub interval: Duration,
    deadline: Instant,
    next_at: Instant,
    finished: bool,
}

impl Machine {
    pub fn new(started: Started, now: Instant) -> Machine {
        let interval = Duration::from_secs(started.interval.max(1));
        let deadline = now + Duration::from_secs(started.expires_in);
        Machine {
            interval,
            deadline,
            next_at: now + interval,
            started,
            tick: Tick::Waiting,
            finished: false,
        }
    }

    /// 到点了就问一次。**没到点、已经结束、已经过期，一个请求都不发。**
    pub fn step(&mut self, now: Instant, send: &dyn Fn(&str) -> Result<Poll, String>) -> Tick {
        if self.finished {
            return self.tick.clone();
        }
        if now >= self.deadline {
            self.finished = true;
            self.tick = Tick::Expired {
                retryable: true,
                message: String::new(),
            };
            return self.tick.clone();
        }
        if now < self.next_at {
            return self.tick.clone();
        }
        self.next_at = now + self.interval;

        match send(&self.started.device_code) {
            // 断网不是失败，只是还没成功。接着等到过期为止。
            Err(_) => {}
            Ok(Poll::Pending) => {}
            Ok(Poll::RateLimited) => {
                self.interval = (self.interval * 2).min(MAX_BACKOFF);
                self.next_at = now + self.interval;
            }
            Ok(Poll::Approved {
                api_key,
                models,
                platforms,
                quota,
            }) => {
                self.finished = true;
                // 空钥匙当失败。写一个空字符串进 secrets 会让学生下一步
                // 撞上一个 401，而屏幕上什么线索都没有。
                self.tick = if api_key.trim().is_empty() {
                    Tick::Failed("empty_key".into())
                } else {
                    Tick::Done(Box::new(Approved {
                        api_key,
                        models,
                        platforms,
                        quota,
                    }))
                };
            }
            Ok(Poll::Denied) => {
                self.finished = true;
                self.tick = Tick::Failed("denied".into());
            }
            Ok(Poll::Claimed) => {
                self.finished = true;
                self.tick = Tick::Failed("claimed".into());
            }
            Ok(Poll::Expired { reason, message }) => {
                self.finished = true;
                self.tick = Tick::Expired {
                    retryable: reason == "ttl",
                    message,
                };
            }
            Ok(Poll::NotEnabled) => {
                self.finished = true;
                self.tick = Tick::Failed("not_enabled".into());
            }
        }
        self.tick.clone()
    }
}

/// 确认页要把这一行显示给学生看，用处是「这台设备是不是我」。
/// **不带主机名、不带用户名**——那回答的是另一个问题。
pub fn user_agent() -> String {
    format!(
        "dct/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn started() -> Started {
        Started {
            device_code: "d".into(),
            user_code: "HJ4K-9QTZ".into(),
            verify_path: "/pair".into(),
            interval: 3,
            expires_in: 900,
        }
    }

    /// 一直 pending：到点转 Expired，而且**不再发请求**。
    /// `start` 是无认证接口，一个忘在那儿的 dct 就是一台每 3 秒敲一次网关的机器。
    #[test]
    fn pending_until_ttl_then_stops_asking() {
        let t0 = Instant::now();
        let mut m = Machine::new(started(), t0);
        let calls = std::cell::Cell::new(0);
        let send = |_: &str| {
            calls.set(calls.get() + 1);
            Ok(Poll::Pending)
        };
        let mut now = t0;
        for _ in 0..400 {
            now += Duration::from_secs(3);
            m.step(now, &send);
        }
        assert!(matches!(m.tick, Tick::Expired { .. }), "到点该过期");
        let after = calls.get();
        now += Duration::from_secs(3);
        m.step(now, &send);
        assert_eq!(calls.get(), after, "过期之后一次都不许再发");
    }

    /// 429 退避翻倍，封顶 30 秒，而且不放弃。
    #[test]
    fn rate_limited_backs_off_and_keeps_going() {
        let t0 = Instant::now();
        let mut m = Machine::new(started(), t0);
        let send = |_: &str| Ok(Poll::RateLimited);
        let mut now = t0;
        for _ in 0..10 {
            now += Duration::from_secs(31);
            m.step(now, &send);
        }
        assert_eq!(m.interval, Duration::from_secs(30), "封顶 30 秒");
        assert!(matches!(m.tick, Tick::Waiting), "429 不是失败");
    }

    /// 空钥匙当失败。绝不写一个空字符串进 secrets。
    #[test]
    fn an_empty_key_is_a_failure_not_a_success() {
        let t0 = Instant::now();
        let mut m = Machine::new(started(), t0);
        let send = |_: &str| {
            Ok(Poll::Approved {
                api_key: String::new(),
                models: Models::default(),
                platforms: Default::default(),
                quota: None,
            })
        };
        m.step(t0 + Duration::from_secs(3), &send);
        assert!(matches!(m.tick, Tick::Failed(_)), "空钥匙不许当成功");
    }

    /// 领到一次之后不再轮询，重复的 approved 也只认第一次。
    #[test]
    fn approved_once_is_final() {
        let t0 = Instant::now();
        let mut m = Machine::new(started(), t0);
        let calls = std::cell::Cell::new(0);
        let send = |_: &str| {
            calls.set(calls.get() + 1);
            Ok(Poll::Approved {
                api_key: "sk-live".into(),
                models: Models::default(),
                platforms: Default::default(),
                quota: None,
            })
        };
        m.step(t0 + Duration::from_secs(3), &send);
        m.step(t0 + Duration::from_secs(6), &send);
        assert_eq!(calls.get(), 1, "领到就停");
    }

    /// 网络断不算失败，接着轮询到过期为止。
    #[test]
    fn a_network_error_is_not_a_failure() {
        let t0 = Instant::now();
        let mut m = Machine::new(started(), t0);
        let send = |_: &str| Err("connection refused".to_string());
        m.step(t0 + Duration::from_secs(3), &send);
        assert!(matches!(m.tick, Tick::Waiting), "断网只是还没成功");
    }

    /// key_unreadable 不能显示成「过期」——那会让学生按 r 重来无数次，
    /// 而每一次都走到同一个地方。
    #[test]
    fn key_unreadable_is_not_reported_as_a_timeout() {
        let t0 = Instant::now();
        let mut m = Machine::new(started(), t0);
        let send = |_: &str| {
            Ok(Poll::Expired {
                reason: "key_unreadable".into(),
                message: "这个账号还没有可读取的密钥，请点「重新生成」".into(),
            })
        };
        m.step(t0 + Duration::from_secs(3), &send);
        match &m.tick {
            Tick::Expired { retryable, message } => {
                assert!(!retryable, "这一种按 r 换码没有用");
                assert!(message.contains("重新生成"), "要把网关那句话原样带出来");
            }
            other => panic!("该是 Expired，实际 {other:?}"),
        }
    }

    /// UA 里不许出现主机名和用户名：这一行要渲染在网页上给人看。
    #[test]
    fn the_user_agent_carries_no_identity() {
        let ua = user_agent();
        assert!(ua.starts_with("dct/"), "{ua}");
        assert!(ua.contains(std::env::consts::OS), "{ua}");
        let user = std::env::var("USER").unwrap_or_default();
        if !user.is_empty() {
            assert!(!ua.contains(&user), "UA 里不许有用户名：{ua}");
        }
    }
}
