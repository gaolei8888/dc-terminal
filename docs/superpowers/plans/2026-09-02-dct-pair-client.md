# dct 配对客户端 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 学生在 dct 里选中 `DC`，浏览器里点一次确认，钥匙、两个 profile 的模型名和 `[llm]` 全部自动配好。

**Architecture:** 设备码流。daemon 打三个 HTTP 接口并在后台线程里轮询，UI 只发不阻塞的 `PairPoll` 读状态。`device_code` 只在 daemon 内存里，一次也不过 socket。领到钥匙后写 `secrets.toml` 两个键、两份 `~/.dct/profiles/*.toml` 覆盖层、以及（勾了才写）`config.toml` 的 `[llm]`。

**Tech Stack:** Rust 2021，ureq 2（经 `sys::tls::agent_builder()`），ratatui 0.28，serde/serde_json，toml 0.8。**不加新依赖。**

**Spec:** `docs/superpowers/specs/2026-09-02-dct-pair-with-dc-llm-design.md`

**网关那半边已经建完并绿了**（`dc-llm-01` 会话，22 个配对测试）。本计划只做 dc-terminal 这一半。

## Global Constraints

- **不加任何新依赖。** 整棵依赖树一行 C 都没有，这是 Windows 上不用装 Visual Studio Build Tools 的全部理由。HTTP 一律从 `crate::sys::tls::agent_builder()` 出来，别处不许 `ureq::AgentBuilder::new()`。
- **`admin-proxy/` 和 `console/` 一个字都不许改。** 那两处归 `dc-llm-01`。要改发消息。
- **不加任何 alembic 迁移。** 迁移只有一个头，在对方那边。
- `interval` **3 秒**，`expires_in` **900 秒**，429 退避翻倍封顶 **30 秒**。
- User-Agent 定死：`dct/<CARGO_PKG_VERSION> (<std::env::consts::OS>; <std::env::consts::ARCH>)`，例 `dct/0.2.5 (macos; aarch64)`。不带主机名、不带用户名。
- **`base_url` 收到也忽略**，origin 永远用配对时那个（从 profile 的 `[api].base_url` 推）。
- `verify_path` 是路径，dct 自己拼 `<origin><verify_path>?code=<user_code>`。
  **网关已上线并实测确认路径是 `/pair`**（`dc-llm-01`，提交 `790fa5f`，迁移
  `q8r9s0t1u2v3`）。页面会读 query 里的 code，且输入框可编辑——码敲错了在网页上改，
  不用回终端重来。
- **网关那边实测到的真实响应**，写测试时照这个形状，别照我编的：
  `user_code` 形如 `Y3BG-MDPQ`，`interval` 3，`expires_in` 900，钥匙 49 个字符，
  免费账号 `models` 是 `{"anthropic": {}, "openai": {"default": "qwen3.5:35b",
  "small_fast": "gemma4:31b"}}`。第二次 poll 回 `claimed` 且不带钥匙；
  拿 `user_code` 当凭据去 poll 回 404。
- **`key_unreadable` 那句文案由网关给，dct 原样显示**，不许自己造：
  「这个账号的密钥读不回来了，请到体验台点「重新生成」后重新配对」。
- 面向用户的文案一律走 `crate::i18n`，zh + en 两份，不许在 UI 里写字面量。
- 测试不打网络。传输层一律注入（照 `verify.rs:44` 的 `verify_with`）。
- 提交信息用英文，正文说清「为什么」，句子完整。

---

### Task 1: 配对状态机（纯逻辑，零 I/O）

**Files:**
- Create: `src/pair.rs`
- Modify: `src/lib.rs`（加 `pub mod pair;`）

**Interfaces:**
- Consumes: 无
- Produces: `pair::Started`、`pair::Poll`、`pair::Models`、`pair::Wire`、`pair::Quota`、`pair::Machine`、`pair::Machine::new(Started, Instant)`、`pair::Machine::step(&mut self, now: Instant, send: &dyn Fn(&str) -> Result<Poll, String>) -> pair::Tick`、`pair::user_agent() -> String`

- [ ] **Step 1: 写失败的测试**

在 `src/pair.rs` 末尾：

```rust
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
```

- [ ] **Step 2: 跑一遍，确认它编不过**

```bash
cd /Users/lei/Documents/work/dc/dc-terminal && cargo test --lib pair::
```

Expected: 编译失败，`cannot find type Started in this scope` 之类。

- [ ] **Step 3: 写最小实现**

`src/pair.rs` 开头（测试模块之前）：

```rust
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
    Expired { reason: String, message: String },
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
    Expired { retryable: bool, message: String },
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
```

`src/lib.rs` 里按字母序加一行：

```rust
pub mod pair;
```

- [ ] **Step 4: 跑测试，确认全绿**

```bash
cd /Users/lei/Documents/work/dc/dc-terminal && cargo test --lib pair::
```

Expected: 7 passed。

- [ ] **Step 5: 提交**

```bash
git add src/pair.rs src/lib.rs
git commit -m "feat(pair): the pairing state machine, with no I/O in it

Transport is injected the way verify.rs does it, and for a longer list of
reasons: the cases that matter here are expiry, denial, backoff, an empty
key, and a second approved arriving after the first, none of which a test
can produce against a real gateway.

Two of them encode decisions rather than mechanics. An empty api_key is a
failure, because writing one to secrets buys the student a 401 with nothing
on screen pointing at why. And an expiry carries whether retrying is
meaningful: a TTL expiry means press r for a fresh code, while an account
whose key cannot be read back would send them around the same loop forever."
```

---

### Task 2: 协议里的三条请求

**Files:**
- Modify: `src/proto.rs`（`Request` 枚举、`Response` 枚举、手写的 `Debug`、`PROTOCOL_VERSION`）
- Test: `src/proto.rs` 自带的 `#[cfg(test)]`

**Interfaces:**
- Consumes: Task 1 的 `pair::Started`、`pair::Tick`
- Produces: `Request::PairStart { profile }`、`Request::PairPoll { profile }`、`Request::PairCancel { profile }`、`Response::PairStarted(Result<pair::Started, String>)`、`Response::PairTick(PairTick)`；`PairTick` 是 `Tick` 的可序列化投影，**不含 `api_key`**

- [ ] **Step 1: 写失败的测试**

加进 `src/proto.rs` 的测试模块：

```rust
/// **配对的响应里绝不许出现钥匙。** UI 不需要它——落盘在 daemon 那边做完了。
/// 一旦它过一次 socket，它就会出现在任何一个手滑加上的 `{resp:?}` 里。
#[test]
fn a_pair_tick_never_carries_the_key() {
    let t = PairTick::Done {
        anthropic_ready: true,
        openai_ready: true,
    };
    let json = serde_json::to_string(&t).unwrap();
    assert!(!json.contains("api_key"), "{json}");
    assert!(!json.contains("sk-"), "{json}");
}

/// `device_code` 是凭据，跟密钥一个待遇：手写的 Debug 要把它挡住。
#[test]
fn pair_requests_do_not_print_anything_sensitive() {
    let r = Request::PairStart {
        profile: "dc".into(),
    };
    let s = format!("{r:?}");
    assert!(s.contains("dc"), "profile 该照常打印，排查问题要用：{s}");
}
```

- [ ] **Step 2: 跑一遍确认失败**

```bash
cargo test --lib proto:: 2>&1 | tail -20
```

Expected: `cannot find type PairTick`。

- [ ] **Step 3: 实现**

`Request` 枚举里，跟在 `VerifySecret` 后面：

```rust
    /// 起一条配对：daemon 打 `/admin/api/pair/start`，成功就在自己内存里
    /// 开一个轮询线程。**`device_code` 不在响应里**，它一次也不过 socket。
    PairStart {
        profile: String,
    },
    /// 读一次配对的当前状态。非阻塞——真正的轮询在 daemon 的后台线程里跑，
    /// 因为它要跑 15 分钟，而界面这条连接 5 秒就超时（`client.rs:11`）。
    PairPoll {
        profile: String,
    },
    /// 取消。**必须真的停线程并丢掉 `device_code`**：不停的话，用户退出去了，
    /// 后台还在替他领钥匙，领到了写进 secrets，而他以为自己取消了。
    PairCancel {
        profile: String,
    },
```

`Response` 枚举里：

```rust
    /// `Err` 是一句已经本地化过的原因（网关关着、连不上）。
    PairStarted(Result<crate::pair::Started, String>),
    PairTick(PairTick),
```

`Response` 定义之后，加这个类型：

```rust
/// `pair::Tick` 给界面看的那一面。**故意不是 `Tick` 本身**：`Tick::Done`
/// 里装着 `api_key`，而界面一个字节都不需要它——钥匙落盘在 daemon 那边
/// 已经做完了。少一个能装钥匙的类型，就少一处它能漏出去的地方。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PairTick {
    Waiting,
    Done {
        /// 有没有拿到 Anthropic 那一组模型。免费账号是 false，
        /// 成功屏要据此换一句话说。
        anthropic_ready: bool,
        openai_ready: bool,
    },
    Expired {
        retryable: bool,
        message: String,
    },
    Failed(String),
}
```

`PROTOCOL_VERSION` 加一。手写的 `Debug` 不用动：三个新变体只有 `profile`，`derive` 不了但 `match` 里落到既有的兜底分支即可——**确认一下兜底分支存在**，不存在就照 `DeleteSecret` 那条写三行。

- [ ] **Step 4: 跑测试**

```bash
cargo test --lib proto::
```

Expected: 全绿，含新加的两条。

- [ ] **Step 5: 提交**

```bash
git add src/proto.rs
git commit -m "feat(proto): three pairing requests, and a tick that cannot carry a key

PairTick is deliberately not pair::Tick. Tick::Done holds the api_key, and
the UI needs none of it — the daemon has already written it to disk by the
time the UI hears anything. A type that cannot hold the key is one fewer
place it can escape from, and the test asserts that shape rather than
trusting it.

device_code stays daemon-side for the same reason and never appears in a
response at all."
```

---

### Task 3: daemon 侧的 HTTP 与轮询线程

**Files:**
- Create: `src/pair_http.rs`（真传输：三个接口的 ureq 调用 + JSON 解析）
- Modify: `src/daemon.rs`（配对状态表、后台线程、三条请求的分发）
- Test: `src/pair_http.rs` 的测试模块（只测 JSON 解析，不打网络）

**Interfaces:**
- Consumes: Task 1 的 `pair::{Started, Poll, Machine, Tick, user_agent}`
- Produces: `pair_http::start(origin: &str, agent: &ureq::Agent) -> Result<pair::Started, String>`、`pair_http::poll(origin: &str, device_code: &str, agent: &ureq::Agent) -> Result<pair::Poll, String>`、`pair_http::parse_poll(status: u16, body: &str) -> pair::Poll`

- [ ] **Step 1: 写失败的测试**

`src/pair_http.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 生命周期状态一律 200 + status 字段，不看错误体。契约见 spec。
    #[test]
    fn lifecycle_states_come_back_as_200() {
        assert!(matches!(
            parse_poll(200, r#"{"status":"pending"}"#),
            crate::pair::Poll::Pending
        ));
        assert!(matches!(
            parse_poll(200, r#"{"status":"denied"}"#),
            crate::pair::Poll::Denied
        ));
        assert!(matches!(
            parse_poll(200, r#"{"status":"claimed"}"#),
            crate::pair::Poll::Claimed
        ));
    }

    /// 开关关着的时候三个接口一律 404——不存在的功能就该像不存在。
    #[test]
    fn a_404_means_pairing_is_switched_off() {
        assert!(matches!(
            parse_poll(404, "{}"),
            crate::pair::Poll::NotEnabled
        ));
    }

    #[test]
    fn a_429_is_rate_limiting_not_an_error() {
        assert!(matches!(
            parse_poll(429, ""),
            crate::pair::Poll::RateLimited
        ));
    }

    /// approved 要把 platforms 也读出来：额度窗口按平台分，
    /// 没有这张表就不知道该显示哪一个。
    #[test]
    fn approved_carries_models_and_the_platform_map() {
        let body = r#"{"status":"approved","api_key":"sk-live",
          "models":{"anthropic":{},"openai":{"default":"qwen3.8:27b"}},
          "platforms":{"qwen3.8:27b":"local"}}"#;
        match parse_poll(200, body) {
            crate::pair::Poll::Approved {
                api_key,
                models,
                platforms,
                ..
            } => {
                assert_eq!(api_key, "sk-live");
                assert_eq!(models.openai.default.as_deref(), Some("qwen3.8:27b"));
                assert_eq!(models.anthropic.default, None, "免费账号这一组是空的");
                assert_eq!(platforms.get("qwen3.8:27b").map(String::as_str), Some("local"));
            }
            other => panic!("该是 Approved，实际 {other:?}"),
        }
    }

    /// expired 的两种 reason 要原样带出来，UI 靠它分文案。
    #[test]
    fn expired_keeps_its_reason_and_message() {
        let body = r#"{"status":"expired","reason":"key_unreadable","message":"请点「重新生成」"}"#;
        match parse_poll(200, body) {
            crate::pair::Poll::Expired { reason, message } => {
                assert_eq!(reason, "key_unreadable");
                assert!(message.contains("重新生成"));
            }
            other => panic!("该是 Expired，实际 {other:?}"),
        }
    }

    /// 看不懂的 body 不许 panic，也不许当成功——当 pending 接着等就行。
    #[test]
    fn garbage_is_treated_as_pending_not_as_success() {
        assert!(matches!(
            parse_poll(200, "<html>502</html>"),
            crate::pair::Poll::Pending
        ));
    }
}
```

- [ ] **Step 2: 跑一遍确认失败**

```bash
cargo test --lib pair_http::
```

Expected: `cannot find function parse_poll`。

- [ ] **Step 3: 实现**

`src/pair_http.rs` 主体：

```rust
//! 配对的真传输层。判定逻辑在 `pair.rs`，这里只管把 HTTP 变成 `pair::Poll`。

use crate::pair::{Poll, Started};
use serde_json::Value;
use std::time::Duration;

/// 单次请求的预算。比 `verify::PROBE_TIMEOUT` 略宽——配对这条路不挂在
/// 界面那 5 秒的连接上（轮询在 daemon 后台线程里），但也不该无限等。
const TIMEOUT: Duration = Duration::from_secs(8);

pub fn agent() -> ureq::Agent {
    crate::sys::tls::agent_builder()
        .timeout(TIMEOUT)
        .timeout_connect(TIMEOUT)
        .build()
}

pub fn start(origin: &str, agent: &ureq::Agent) -> Result<Started, String> {
    let url = format!("{}/admin/api/pair/start", origin.trim_end_matches('/'));
    let resp = agent
        .post(&url)
        .set("content-type", "application/json")
        .set("user-agent", &crate::pair::user_agent())
        .send_json(serde_json::json!({
            "client": "dct",
            "version": env!("CARGO_PKG_VERSION"),
        }));
    match resp {
        Ok(r) => r
            .into_json::<Started>()
            .map_err(|e| format!("bad_start_body: {e}")),
        Err(ureq::Error::Status(404, _)) => Err("not_enabled".into()),
        Err(ureq::Error::Status(429, _)) => Err("rate_limited".into()),
        Err(e) => Err(format!("unreachable: {e}")),
    }
}

pub fn poll(origin: &str, device_code: &str, agent: &ureq::Agent) -> Result<Poll, String> {
    let url = format!("{}/admin/api/pair/poll", origin.trim_end_matches('/'));
    let resp = agent
        .post(&url)
        .set("content-type", "application/json")
        .set("user-agent", &crate::pair::user_agent())
        .send_json(serde_json::json!({ "device_code": device_code }));
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.into_string().unwrap_or_default();
            Ok(parse_poll(status, &body))
        }
        // ureq 把 4xx/5xx 也当 Err，它们是有效状态码不是网络故障——
        // 同 `verify::send_probe` 里那条注释。
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            Ok(parse_poll(code, &body))
        }
        Err(e) => Err(format!("{e}")),
    }
}

pub fn parse_poll(status: u16, body: &str) -> Poll {
    match status {
        404 => return Poll::NotEnabled,
        429 => return Poll::RateLimited,
        _ => {}
    }
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        // 看不懂就当还没好。当成功会写进一把不存在的钥匙，当失败会把一次
        // 正常的 502 变成学生眼里的「配对坏了」。
        Err(_) => return Poll::Pending,
    };
    match v.get("status").and_then(Value::as_str).unwrap_or("") {
        "approved" => Poll::Approved {
            api_key: v
                .get("api_key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            models: serde_json::from_value(v.get("models").cloned().unwrap_or(Value::Null))
                .unwrap_or_default(),
            platforms: serde_json::from_value(
                v.get("platforms").cloned().unwrap_or(Value::Null),
            )
            .unwrap_or_default(),
            quota: serde_json::from_value(v.get("quota").cloned().unwrap_or(Value::Null)).ok(),
        },
        "denied" => Poll::Denied,
        "claimed" => Poll::Claimed,
        "expired" => Poll::Expired {
            reason: v
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("ttl")
                .to_string(),
            message: v
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        _ => Poll::Pending,
    }
}
```

`src/lib.rs` 加 `pub mod pair_http;`。

`src/daemon.rs`：在 daemon 的共享状态旁边加一张表，跟 `secrets`、`phone` 同样用 `Mutex` 包：

```rust
/// 正在进行的配对，按 profile 名。**`device_code` 只活在这里。**
/// `Option` 里那个 `JoinHandle` 是为了 `PairCancel` 能真的把线程停掉：
/// 停不掉的话，用户退出去了，后台还在替他领钥匙。
type PairTable = std::collections::BTreeMap<String, PairSlot>;

struct PairSlot {
    started: crate::pair::Started,
    tick: crate::proto::PairTick,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
```

三条分发（放在 `Request::VerifySecret` 那条后面）：

```rust
Request::PairStart { profile } => {
    let origin = pair_origin(profiles_dir, &profile);
    match origin {
        None => Ok(Response::PairStarted(Err("no_api_base_url".into()))),
        Some(origin) => {
            let agent = crate::pair_http::agent();
            match crate::pair_http::start(&origin, &agent) {
                Err(e) => Ok(Response::PairStarted(Err(e))),
                Ok(started) => {
                    spawn_pair_poller(&profile, &origin, started.clone(), pairs.clone(), secrets.clone(), profiles_dir.to_path_buf());
                    Ok(Response::PairStarted(Ok(started)))
                }
            }
        }
    }
}
Request::PairPoll { profile } => Ok(Response::PairTick(
    recover(pairs.lock())
        .get(&profile)
        .map(|s| s.tick.clone())
        .unwrap_or(crate::proto::PairTick::Waiting),
)),
Request::PairCancel { profile } => {
    if let Some(slot) = recover(pairs.lock()).remove(&profile) {
        slot.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(Response::Ok)
}
```

`spawn_pair_poller` 与 `pair_origin` 写在 `daemon.rs` 下部：

```rust
/// 配对用哪个 origin：从这个 profile 的 `[api].base_url` 里取，**只取 origin**。
/// 这是整条流程的信任锚——它随仓库发布，不来自网络（spec 里那段
/// 「origin 是信任锚，路径是配置」）。
fn pair_origin(profiles_dir: &std::path::Path, profile: &str) -> Option<String> {
    let (all, _) = all_profiles(profiles_dir);
    let base = all
        .iter()
        .find(|p| p.name == profile)?
        .api
        .as_ref()?
        .base_url
        .clone();
    let rest = base.split_once("://")?;
    let host = rest.1.split('/').next()?;
    Some(format!("{}://{}", rest.0, host))
}

fn spawn_pair_poller(
    profile: &str,
    origin: &str,
    started: crate::pair::Started,
    pairs: std::sync::Arc<std::sync::Mutex<PairTable>>,
    secrets: std::sync::Arc<std::sync::Mutex<crate::secrets::SecretStore>>,
    profiles_dir: std::path::PathBuf,
) {
    use std::sync::atomic::Ordering;
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    recover(pairs.lock()).insert(
        profile.to_string(),
        PairSlot {
            started: started.clone(),
            tick: crate::proto::PairTick::Waiting,
            cancel: cancel.clone(),
        },
    );
    let (profile, origin) = (profile.to_string(), origin.to_string());
    std::thread::spawn(move || {
        let agent = crate::pair_http::agent();
        let mut machine = crate::pair::Machine::new(started, std::time::Instant::now());
        loop {
            if cancel.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
            let send = |dc: &str| crate::pair_http::poll(&origin, dc, &agent);
            let tick = machine.step(std::time::Instant::now(), &send);
            let projected = match &tick {
                crate::pair::Tick::Waiting => crate::proto::PairTick::Waiting,
                crate::pair::Tick::Expired { retryable, message } => {
                    crate::proto::PairTick::Expired {
                        retryable: *retryable,
                        message: message.clone(),
                    }
                }
                crate::pair::Tick::Failed(e) => crate::proto::PairTick::Failed(e.clone()),
                crate::pair::Tick::Done(a) => {
                    // 落盘在这里做完，钥匙不过 socket。
                    let outcome = crate::pair_apply::apply(a, &profile, &secrets, &profiles_dir);
                    match outcome {
                        Ok(ready) => crate::proto::PairTick::Done {
                            anthropic_ready: ready.anthropic,
                            openai_ready: ready.openai,
                        },
                        Err(e) => crate::proto::PairTick::Failed(e),
                    }
                }
            };
            let done = !matches!(projected, crate::proto::PairTick::Waiting);
            if let Some(slot) = recover(pairs.lock()).get_mut(&profile) {
                slot.tick = projected;
            }
            if done {
                return;
            }
        }
    });
}
```

（`pair_apply::apply` 在 Task 4 建；本任务先让它编过——Task 4 之前用一个返回 `Err("todo".into())` 的桩，**并在 Task 4 里删掉**。）

- [ ] **Step 4: 跑测试与编译**

```bash
cargo test --lib pair_http:: && cargo build
```

Expected: 6 passed，build 成功。

- [ ] **Step 5: 提交**

```bash
git add src/pair_http.rs src/daemon.rs src/lib.rs
git commit -m "feat(pair): daemon-side transport and the polling thread

Polling lives in the daemon because it runs for fifteen minutes and the
UI's connection times out after five seconds; the UI reads a cached tick
instead. device_code never leaves this process.

An unparseable body is treated as pending. Reading it as success would
write a key that isn't there, and reading it as failure turns an ordinary
502 into 'pairing is broken' on a student's screen — waiting is the only
reading that costs nothing if it's wrong."
```

---

### Task 4: 领到之后写盘（两个 profile 的钥匙与模型名）

**Files:**
- Create: `src/pair_apply.rs`
- Modify: `src/daemon.rs`（删掉 Task 3 的桩）
- Test: `src/pair_apply.rs` 测试模块（`tempfile` 已是 dev-dependency）

**Interfaces:**
- Consumes: Task 1 的 `pair::Approved`、`crate::secrets::SecretStore`
- Produces: `pair_apply::apply(&pair::Approved, profile: &str, secrets: &Mutex<SecretStore>, profiles_dir: &Path) -> Result<Ready, String>`、`pair_apply::Ready { anthropic: bool, openai: bool }`、`pair_apply::render_override(name: &str, env: &BTreeMap<String,String>) -> String`

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// 一次配对填两个 profile。第 6 步「同一把钥匙再填一遍」是这整件事
    /// 想消灭的东西之一。
    #[test]
    fn one_pairing_fills_both_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Mutex::new(crate::secrets::SecretStore::load(
            &dir.path().join("secrets.toml"),
        ));
        let a = approved_with_both_wires();
        apply(&a, "dc", &store, dir.path()).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(s.get("dc").as_deref(), Some("sk-live"));
        assert_eq!(s.get("qwen").as_deref(), Some("sk-live"));
    }

    /// 两个 Anthropic 变量都要写。只钉主模型的话，起标题、扫文件那个便宜的
    /// 快模型会以课堂上没人查得出来的方式坏掉（`profiles/dc.toml` 里那段注释）。
    #[test]
    fn both_anthropic_model_variables_get_written() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Mutex::new(crate::secrets::SecretStore::load(
            &dir.path().join("secrets.toml"),
        ));
        apply(&approved_with_both_wires(), "dc", &store, dir.path()).unwrap();
        let dc = std::fs::read_to_string(dir.path().join("profiles/dc.toml")).unwrap();
        assert!(dc.contains("ANTHROPIC_MODEL = \"claude-x\""), "{dc}");
        assert!(dc.contains("ANTHROPIC_SMALL_FAST_MODEL = \"claude-small\""), "{dc}");
    }

    /// 免费账号：anthropic 那一组是空的。钥匙照写，**模型名一个都不许编**——
    /// 写一个跑不通的模型名比不写更坏，学生会撞上一个没有任何解释的 404。
    #[test]
    fn a_free_account_gets_the_key_but_no_invented_model_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Mutex::new(crate::secrets::SecretStore::load(
            &dir.path().join("secrets.toml"),
        ));
        let ready = apply(&approved_openai_only(), "dc", &store, dir.path()).unwrap();
        assert!(!ready.anthropic, "免费账号没有 Anthropic 那一路");
        assert!(ready.openai);
        assert_eq!(store.lock().unwrap().get("dc").as_deref(), Some("sk-live"));
        let dc = std::fs::read_to_string(dir.path().join("profiles/dc.toml")).unwrap();
        assert!(!dc.contains("ANTHROPIC_MODEL"), "没有就不许写：{dc}");
    }

    /// 覆盖层文件要带一行标记：下次配对认它才敢重写。用户手改过的文件
    /// （没有这行）绝不覆盖。
    #[test]
    fn a_hand_edited_override_is_never_clobbered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("profiles")).unwrap();
        let f = dir.path().join("profiles/dc.toml");
        std::fs::write(&f, "# 我自己写的\n[env]\nANTHROPIC_MODEL = \"mine\"\n").unwrap();
        let store = std::sync::Mutex::new(crate::secrets::SecretStore::load(
            &dir.path().join("secrets.toml"),
        ));
        apply(&approved_with_both_wires(), "dc", &store, dir.path()).unwrap();
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("mine"), "用户手写的东西不许动：{after}");
    }

    fn approved_with_both_wires() -> crate::pair::Approved {
        crate::pair::Approved {
            api_key: "sk-live".into(),
            models: crate::pair::Models {
                anthropic: crate::pair::WireModels {
                    default: Some("claude-x".into()),
                    small_fast: Some("claude-small".into()),
                },
                openai: crate::pair::WireModels {
                    default: Some("qwen3.8:27b".into()),
                    small_fast: Some("qwen-small".into()),
                },
            },
            platforms: BTreeMap::new(),
            quota: None,
        }
    }

    fn approved_openai_only() -> crate::pair::Approved {
        let mut a = approved_with_both_wires();
        a.models.anthropic = Default::default();
        a
    }
}
```

- [ ] **Step 2: 跑一遍确认失败**

```bash
cargo test --lib pair_apply::
```

Expected: `cannot find function apply`。

- [ ] **Step 3: 实现**

```rust
//! 配对成功之后落盘的那几件事。
//!
//! **不假装它是原子的。** `secrets.rs:134` 那套「save 失败就回滚内存」是按
//! 单键写的，两次 set 就是两次落盘。第二次失败**不回滚第一次**——回滚会把
//! 学生刚拿到的、网关那边已经标成 claimed 的钥匙扔掉，那把钥匙他再也领不
//! 回来了。领取是一次性的，这是这里唯一重要的约束。

use crate::pair::Approved;
use std::collections::BTreeMap;
use std::path::Path;

/// 这次配对给这个账号开了哪几条路。成功屏据此换话说。
pub struct Ready {
    pub anthropic: bool,
    pub openai: bool,
}

/// 覆盖层文件的第一行。认这行才敢重写——没有它的文件是用户自己写的。
const MARK: &str = "# 这个文件由 dct 配对自动生成，下次配对会重写。手改请删掉这一行。";

pub fn apply(
    a: &Approved,
    _profile: &str,
    secrets: &std::sync::Mutex<crate::secrets::SecretStore>,
    profiles_dir: &Path,
) -> Result<Ready, String> {
    {
        let mut s = secrets.lock().unwrap_or_else(|e| e.into_inner());
        s.set("dc", &a.api_key).map_err(|e| format!("{e}"))?;
        // 第二把失败不回滚第一把——见文件头那段。
        s.set("qwen", &a.api_key)
            .map_err(|_| "qwen_secret_write_failed".to_string())?;
    }

    let mut dc_env = BTreeMap::new();
    if let Some(m) = &a.models.anthropic.default {
        dc_env.insert("ANTHROPIC_MODEL".to_string(), m.clone());
    }
    if let Some(m) = &a.models.anthropic.small_fast {
        dc_env.insert("ANTHROPIC_SMALL_FAST_MODEL".to_string(), m.clone());
    }
    let mut qwen_env = BTreeMap::new();
    if let Some(m) = &a.models.openai.default {
        qwen_env.insert("OPENAI_MODEL".to_string(), m.clone());
    }

    write_override(profiles_dir, "dc", &dc_env)?;
    write_override(profiles_dir, "qwen", &qwen_env)?;

    Ok(Ready {
        anthropic: a.models.anthropic.default.is_some(),
        openai: a.models.openai.default.is_some(),
    })
}

/// 覆盖层只写 `[env]`。仓库里那两份 profile 一行都不动——它们是要能提交的
/// 文件，运行时的东西不该长在里面。
pub fn render_override(name: &str, env: &BTreeMap<String, String>) -> String {
    let mut out = format!("{MARK}\nname = \"{name}\"\n\n[env]\n");
    for (k, v) in env {
        out.push_str(&format!("{k} = \"{v}\"\n"));
    }
    out
}

fn write_override(dir: &Path, name: &str, env: &BTreeMap<String, String>) -> Result<(), String> {
    if env.is_empty() {
        return Ok(());
    }
    let profiles = dir.join("profiles");
    std::fs::create_dir_all(&profiles).map_err(|e| format!("{e}"))?;
    let f = profiles.join(format!("{name}.toml"));
    if let Ok(existing) = std::fs::read_to_string(&f) {
        // 用户手改过的文件不许动。他写在里面的东西比我们知道的多。
        if !existing.starts_with(MARK) {
            return Ok(());
        }
    }
    std::fs::write(&f, render_override(name, env)).map_err(|e| format!("{e}"))
}
```

`src/lib.rs` 加 `pub mod pair_apply;`，删掉 Task 3 里的桩。

- [ ] **Step 4: 跑测试**

```bash
cargo test --lib pair_apply::
```

Expected: 4 passed。

- [ ] **Step 5: 提交**

```bash
git add src/pair_apply.rs src/daemon.rs src/lib.rs
git commit -m "feat(pair): write the key to both profiles and the model names beside it

One pairing fills dc and qwen, which is the step this whole feature exists
to delete. The two writes are not atomic and the second failing does not
roll back the first: the gateway has already marked the key claimed, so a
rollback throws away a key the student cannot obtain again.

A free account gets the key and no Anthropic model name, because inventing
one buys a 404 with nothing on screen explaining that Claude is paid-only.
Generated overrides carry a marker line and a file without it is left
alone — whatever a student hand-wrote there, they know more about it
than we do."
```

---

### Task 5: `[llm]` 那个勾

**Files:**
- Create: `src/llm_optin.rs`
- Modify: `src/config.rs:6`（改那段隐私边界注释）
- Test: `src/llm_optin.rs` 测试模块

**Interfaces:**
- Consumes: Task 1 的 `pair::Models`
- Produces: `llm_optin::enable(config_path: &Path, provider: &str, model: &str) -> Result<bool, String>`（返回是否真的写了）

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 文件里已经有 [llm] 就一个字都不动。用户已经做过这个决定了。
    #[test]
    fn an_existing_llm_section_is_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("config.toml");
        std::fs::write(&f, "[llm]\nprovider = \"kimi\"\nmodel = \"k2\"\n").unwrap();
        assert!(!enable(&f, "dc", "claude-x").unwrap(), "不该写");
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("kimi"), "{after}");
    }

    /// 追加而不是重写：config.toml 里还有 [menu] 之类，而且用户写了注释。
    /// 整份重新序列化会把注释全吃掉。
    #[test]
    fn other_sections_and_comments_survive() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("config.toml");
        std::fs::write(&f, "# 我的注释\n[menu]\nshort = true\n").unwrap();
        assert!(enable(&f, "dc", "claude-x").unwrap());
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("# 我的注释"), "注释不许丢：{after}");
        assert!(after.contains("[menu]"), "{after}");
        assert!(after.contains("provider = \"dc\""), "{after}");
    }

    /// 写完要能被 Config::load 读回来，否则等于没写。
    #[test]
    fn what_we_write_parses_back() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("config.toml");
        enable(&f, "qwen", "qwen3.8:27b").unwrap();
        let c = crate::config::Config::load(&f);
        let llm = c.llm.expect("写完就该是 Some");
        assert_eq!(llm.provider, "qwen");
        assert_eq!(llm.model.as_deref(), Some("qwen3.8:27b"));
    }
}
```

- [ ] **Step 2: 跑一遍确认失败**

```bash
cargo test --lib llm_optin::
```

Expected: `cannot find function enable`。

- [ ] **Step 3: 实现**

```rust
//! 把 `[llm]` 打开——**只在配对屏上学生当面勾过的时候**。
//!
//! `config.rs` 开头那段注释说这是隐私边界不是默认值，那句话仍然成立：
//! 这里不给任何默认值，它只执行一个人刚刚看着文案做出的决定。
//!
//! **追加，不重写。** 整份反序列化再序列化会把用户的注释全吃掉，而
//! `~/.dct/config.toml` 是一份人手写、人要再读的文件。

use std::path::Path;

pub fn enable(config_path: &Path, provider: &str, model: &str) -> Result<bool, String> {
    let existing = std::fs::read_to_string(config_path).unwrap_or_default();
    // 已经有 [llm] 就不动：用户（或上一次配对）已经决定过了。
    if existing.lines().any(|l| l.trim_start().starts_with("[llm]")) {
        return Ok(false);
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!(
        "\n# 由 dct 配对写入：学生在配对屏上勾了「报错看不懂时让 AI 解释」。\n\
         [llm]\nprovider = \"{provider}\"\nmodel = \"{model}\"\ntransport = \"http\"\n"
    ));
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{e}"))?;
    }
    std::fs::write(config_path, out).map_err(|e| format!("{e}"))
        .map(|_| true)
}
```

`src/config.rs:6` 那段注释末尾补一句：

```rust
//! **2026-09-02 起多了第二条打开它的路**：配对屏上那个默认勾上的勾选框
//! （`llm_optin::enable`）。边界没变——仍然要一个人当面看着「会把报错原文
//! 发给训练营网关」这句话点头，只是那个人现在可能是在配对流程里点的。
```

- [ ] **Step 4: 跑测试**

```bash
cargo test --lib llm_optin:: && cargo test --lib config::
```

Expected: 3 passed + config 原有测试仍绿。

- [ ] **Step 5: 提交**

```bash
git add src/llm_optin.rs src/config.rs
git commit -m "feat(pair): turn on [llm] when the student ticked the box, by appending

config.rs says an absent [llm] is a privacy boundary rather than a missing
default, and that stays true: this writes nothing on its own, it carries
out a decision someone just made while reading what it costs. The comment
there now says so, because a comment that describes a rule the code no
longer follows is worse than no comment.

Appending rather than re-serialising: config.toml is hand-written and
hand-read, and a round-trip through the toml crate eats every comment in
it. An existing [llm] is left untouched — that decision is already made."
```

---

### Task 6: `View::Pair` 三屏与入口

**Files:**
- Modify: `src/ui/view.rs`（`View` 枚举加 `Pair`）、`src/ui/secret.rs`（入口与按键）、`src/ui/mod.rs`（tick 里发 `PairPoll`）、`src/i18n.rs`（新词条）
- Create: `src/ui/pair_view.rs`（画那三屏 + 按键处理）
- Test: `src/ui/pair_view.rs` 测试模块

**Interfaces:**
- Consumes: Task 2 的 `Request::Pair*`、`proto::PairTick`
- Produces: `View::Pair { phase: PairPhase, profile: String }`；`PairPhase::{Starting, Waiting{user_code, url, deadline}, Failed{message, retryable}, Done{anthropic, openai}}`

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    /// **Esc 要能真的取消。** `phone.rs:442` 那条断言的配对版——不发
    /// PairCancel 的话，用户退出去了，后台还在替他领钥匙，领到了写进
    /// secrets，而他以为自己取消了。
    #[test]
    fn esc_sends_a_cancel_and_leaves_the_view() {
        let mut app = test_app_in_pair_waiting();
        handle_key(&mut app, key(KeyCode::Esc)).unwrap();
        assert!(
            app.sent.iter().any(|r| matches!(r, crate::proto::Request::PairCancel { .. })),
            "Esc 必须发 PairCancel，不能只是切视图"
        );
        assert!(!matches!(app.view, crate::ui::view::View::Pair { .. }));
    }

    /// 浏览器打不开是常态（SSH、WSL、没设默认浏览器），屏上必须有个能手抄的
    /// 地址，否则学生就卡死在这一屏。
    #[test]
    fn the_url_is_on_screen_not_only_in_the_browser() {
        let app = test_app_in_pair_waiting();
        let lines = render_lines(&app);
        assert!(
            lines.iter().any(|l| l.contains("dc-llm.tzspace.cn/pair")),
            "地址要印出来：{lines:?}"
        );
    }

    /// 过期的两种理由文案不一样：能重试的说按 r，不能的说去 /me 重新生成。
    #[test]
    fn the_two_expiries_do_not_share_one_sentence() {
        let a = phase_expired(true, String::new());
        let b = phase_expired(false, "请点「重新生成」".into());
        assert_ne!(render_phase(&a), render_phase(&b));
        assert!(render_phase(&b).contains("重新生成"));
    }

    /// 手动填那条退路一直在。老用户、离线课堂、网关配对坏掉的那天都要它。
    #[test]
    fn manual_entry_stays_reachable_from_every_phase() {
        for phase in [phase_waiting(), phase_expired(true, String::new())] {
            let mut app = test_app_with_phase(phase);
            handle_key(&mut app, key(KeyCode::Char('p'))).unwrap();
            assert!(
                matches!(app.view, crate::ui::view::View::EnterSecret { .. }),
                "p 应该进手动填"
            );
        }
    }
}
```

（`test_app_in_pair_waiting`、`render_lines`、`render_phase`、`phase_*` 按 `src/ui/phone.rs` 测试里现成的构造方式写——那个文件里已经有一套「造一个 App、塞一个 view、按一个键」的助手，照抄它的形状，不要另发明一套。）

- [ ] **Step 2: 跑一遍确认失败**

```bash
cargo test --lib ui::pair_view::
```

Expected: 编译失败。

- [ ] **Step 3: 实现**

`View` 枚举加：

```rust
    /// 配对：跟训练营网关换一把钥匙。三个阶段，每个阶段都必须有一条出路——
    /// 没有出路的错误屏等于死路。
    Pair {
        profile: String,
        phase: PairPhase,
    },
```

`PairPhase` 与按键：`Esc` → 发 `Request::PairCancel` 再退；`o` → 重开浏览器；`p` → 切 `View::EnterSecret`（现成的手动填）；`r` → 重发 `PairStart`（仅 `retryable` 的过期与失败）。

`ui/mod.rs` 的主循环：在 `verify_rx` 那段排空之后、`term.draw` 之前，加一段按 500ms 节流的 `PairPoll`（照 `phone_last_fetch` 那个节流字段的做法，加 `pair_last_fetch: Option<Instant>`）。

i18n 新词条（zh + en 各一份）：`PairContacting`、`PairEnterCodeInBrowser`、`PairCodeExpired`、`PairKeyUnreadable`、`PairDenied`、`PairNotEnabled`、`PairDoneBoth`、`PairDoneQwenOnly`、`PairManualHint`、`PairLlmOptIn`。

- [ ] **Step 4: 跑测试**

```bash
cargo test --lib ui:: && cargo clippy -- -D warnings
```

Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
git add src/ui/pair_view.rs src/ui/view.rs src/ui/secret.rs src/ui/mod.rs src/i18n.rs
git commit -m "feat(ui): the pairing screens, with an exit from every one of them

Esc actually cancels — it sends PairCancel rather than only changing the
view. Without that the student walks away believing they stopped, while a
thread behind them collects the key and writes it to disk. phone.rs:442
pins the same property for token verification; this is that assertion's
pairing twin.

The verify URL is printed as well as opened. A browser that doesn't open is
ordinary — SSH, WSL, no default browser — and without an address on screen
to copy by hand, that student is simply stuck.

The two expiries get different sentences. 'Expired' shown for an account
whose key can't be read back sends them around the r-for-a-new-code loop
forever, arriving at the same place every time."
```

---

### Task 7: 端到端集成测试（假网关）

**Files:**
- Create: `tests/pair_flow.rs`
- Modify: `tests/common/mod.rs`（加一个最小 HTTP responder）

**Interfaces:**
- Consumes: 前六个任务的全部
- Produces: 无（终点）

- [ ] **Step 1: 写测试**

```rust
/// 完整一条：start → pending → approved。断言落盘的三处都对。
#[test]
fn a_full_pairing_writes_both_secrets_and_both_model_names() {
    let gw = common::fake_gateway(vec![
        // 第一次 poll 还没批准，第二次批准——真实节奏就是这样，
        // 一次就成的测试测不到「等」这件事。
        r#"{"status":"pending"}"#,
        r#"{"status":"approved","api_key":"sk-live",
            "models":{"anthropic":{"default":"claude-x","small_fast":"claude-small"},
                      "openai":{"default":"qwen3.8:27b"}},
            "platforms":{"qwen3.8:27b":"local"}}"#,
    ]);
    let home = tempfile::tempdir().unwrap();
    let d = common::daemon_with(home.path(), &gw.origin());

    d.call(Request::PairStart { profile: "dc".into() }).unwrap();
    let tick = common::wait_for_tick(&d, "dc", std::time::Duration::from_secs(10));
    assert!(matches!(tick, PairTick::Done { anthropic_ready: true, openai_ready: true }));

    let secrets = std::fs::read_to_string(home.path().join("secrets.toml")).unwrap();
    assert!(secrets.contains("dc = "), "{secrets}");
    assert!(secrets.contains("qwen = "), "{secrets}");
    let dc = std::fs::read_to_string(home.path().join("profiles/dc.toml")).unwrap();
    assert!(dc.contains("claude-x") && dc.contains("claude-small"), "{dc}");
}

/// 取消之后，哪怕网关随后批准了，也一个字节都不许落盘。
#[test]
fn cancelling_means_nothing_is_ever_written() {
    let gw = common::fake_gateway_slow_approve(std::time::Duration::from_secs(2));
    let home = tempfile::tempdir().unwrap();
    let d = common::daemon_with(home.path(), &gw.origin());

    d.call(Request::PairStart { profile: "dc".into() }).unwrap();
    d.call(Request::PairCancel { profile: "dc".into() }).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(4));

    assert!(
        !home.path().join("secrets.toml").exists()
            || !std::fs::read_to_string(home.path().join("secrets.toml"))
                .unwrap()
                .contains("sk-"),
        "取消之后落盘了，说明后台线程没停"
    );
}
```

- [ ] **Step 2: 跑一遍确认失败**

```bash
cargo test --test pair_flow
```

Expected: 编译失败（`fake_gateway` 还不存在）。

- [ ] **Step 3: 实现 `common::fake_gateway`**

用 `std::net::TcpListener` 手写，**不加依赖**：绑 `127.0.0.1:0`，读到空行为止，按预设队列逐条回 `HTTP/1.1 200 OK` + `content-type: application/json`。`/pair/start` 固定回 `{"device_code":"d","user_code":"HJ4K-9QTZ","verify_path":"/pair","interval":1,"expires_in":30}`（测试里 `interval` 用 1 秒，别让一条测试等 3 秒）。

- [ ] **Step 4: 跑测试**

```bash
cargo test --test pair_flow -- --nocapture
```

Expected: 2 passed。

- [ ] **Step 5: 提交**

```bash
git add tests/pair_flow.rs tests/common/mod.rs
git commit -m "test(pair): the whole flow against a fake gateway, including cancel

The happy path goes through a pending poll before the approved one,
because a test that succeeds on the first call never exercises waiting.

The cancel test lets the fake gateway approve two seconds after the cancel
and asserts nothing lands on disk. That is the only way to catch a poller
that outlives the screen the student closed."
```

---

### Task 8: README 与仓库里那两个 profile 的注释

**Files:**
- Modify: `README.zh-CN.md:264`、`README.md` 对应段、`profiles/dc.toml`、`profiles/qwen.toml`（**只改注释**）

- [ ] **Step 1: 改 README**

`README.zh-CN.md:264` 现在写着「DC 没有申领页面：它的密钥由疯狂AI训练营发给上课的人」。改成描述配对：选中 DC → 浏览器里点确认 → 钥匙和模型名自动配好，并说明手动填那条路仍在。英文 README 同步。

- [ ] **Step 2: 改两个 profile 的注释**

`profiles/dc.toml` 里那段「网关如果做不到……就把下面两行打开」改成：这两行现在由配对自动写进 `~/.dct/profiles/dc.toml` 的覆盖层，仓库这份保持注释状态。**`[env]` 里的值一个都不许改**——覆盖层的存在正是为了不动这份文件。

- [ ] **Step 3: 确认没动到代码**

```bash
git diff --stat
```

Expected: 只有 4 个文件，全是 md 与 toml 注释。

- [ ] **Step 4: 提交**

```bash
git add README.md README.zh-CN.md profiles/dc.toml profiles/qwen.toml
git commit -m "docs: describe pairing where the README still described copy-paste

README.zh-CN.md:264 said the camp hands out DC keys to people in class.
08ec3f5 already moved that to the student's own account page and pairing
moves it again — into a browser confirmation the student clicks once.

The two profiles keep their commented-out model lines. Those values are
written by pairing into the ~/.dct override layer now, and the comment says
so, because the next person to read this file will otherwise fill them in
by hand and wonder why pairing overwrote nothing."
```

---

## 上线顺序（spec 第 4 节，这里重复一遍因为它是执行顺序）

1. 网关先上（**已完成并已部署到生产**，`790fa5f`；`DC_ADMIN_PAIRING_ENABLED`
   在生产 .env 里没设，三个接口一律 404）
2. Task 1–7 做完，本地对着 QA 实例手工跑一遍全程——**这一步没法自动化**，要真开浏览器、真点确认
3. 告诉 `dc-llm-01` 要合并，它挂起部署
4. 合入 dc-terminal，`profiles/` 里两个文件只动注释
5. **两边同时在场**才开开关：对方改一行 .env 加重启容器（不用重建），我这边
   拿一个测试账号真跑一遍。这一步的不对称是硬的——我测不了它那半，它也部署不了
   我这半，所以第一次真实端到端必须两个人同时在。

## 自查

- **spec 覆盖**：第 1 节的接口契约 → Task 2/3；`models` 分组与 `platforms` → Task 1/3/4；额度快照 → Task 1（类型）+ Task 6（显示），`/v1/me/quota` 是二期不在本计划；第 2 节三屏、轮询、取消、超时、入口 → Task 6 + Task 3；第 3 节两处 secrets、覆盖层、`[llm]` → Task 4/5；第 4 节测试矩阵 → Task 1（状态机 7 条）+ Task 3（解析 6 条）+ Task 7（端到端 2 条）；失败路径出路 → Task 6。
- **无占位符**：每个步骤都带可运行命令与真实代码，Task 6 的助手函数指向 `phone.rs` 里现成的形状而不是「类似上面」。
- **类型一致**：`pair::Approved`（Task 1）→ `pair_apply::apply` 第一参数（Task 4）；`proto::PairTick`（Task 2）→ daemon 投影（Task 3）→ UI（Task 6）；`Models`/`WireModels` 字段名 `default`/`small_fast` 三处一致。
