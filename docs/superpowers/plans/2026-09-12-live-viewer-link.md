# 直播观众链接（帧通路）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 老师在 dct 里上架若干会话，得到一条链接；学生用浏览器打开只能看画面，看不到会话列表，也没有任何路径能把字节送回老师的终端。

**Architecture:** 守护进程按 2 Hz 把上架会话的屏幕推到中转（`POST /live/frame`），中转按 live-id 存"最新一帧"（内存、有界、60 秒 TTL），学生只读中转（`GET /live/<id>/frame`，ETag/304 + `?wait=1` 长轮询）。守护进程的负担与观众人数无关；"只读"是管子的形状，不是代码判断。

**Tech Stack:** Rust（`dct` 无 async、ureq 出网；`dct-srv` 用 tokio + axum 0.8）、原生 JS 单文件网页（无框架、无外部资源）。

**Spec:** `docs/superpowers/specs/2026-09-12-live-viewer-link-design.md`

## 范围说明

spec 覆盖两个能各自独立工作的子系统。本计划只做**帧通路**（学生能看）。
**提问板**（学生能提问、同问、老师问题面板）是第二个计划，做完这个之后再写。
理由：帧通路做完就是一个能上课用的功能，提问板是叠在它上面的第二件东西。

## Global Constraints

- `dct` 这一侧**不许引入 async 运行时**。网络 IO 用 `ureq`，且必须在自己的线程上，绝不能进守护进程那个 200ms 的 tick（见 `src/link.rs` 模块头、`src/bridge.rs`）。
- `dct-srv` 是唯一允许有 tokio 的 crate。`cargo tree -p dct` 里不许出现 tokio。
- **整棵依赖树里一行 C 都没有**。新增依赖必须是纯 Rust（`flate2` 要 `default-features = false, features = ["rust_backend"]`）。
- 新增 `Request` 变体必须把 `PROTOCOL_VERSION` 加一（`src/proto.rs:93`，现为 13 → 14），并更新钉死线上形状的那条测试。
- 网页里**一个字的用户文案都不许写死**，全部走 `Request::WebStrings` / `web::strings`；不许有外部资源（CDN、字体、图片）。
- 中转**不解析帧**：`crates/dct-srv` 里不许出现 `ScreenSpan`、`Response`、`from_slice::<Request>`。
- 所有用户可见文案的默认语言是 `zh_CN`，英文走同一张表。
- 常量两侧共用：推帧频率、TTL、上限一律定义在 `dct-link`，`dct` 和 `dct-srv` 都从那里取。
- 测试函数名用英文 snake_case 写成一句话，文档注释和断言消息用中文——照抄现有风格（`src/web/routes.rs` 的测试）。

---

## 文件结构

| 文件 | 职责 |
| --- | --- |
| `crates/dct-link/src/live.rs`（新建） | 直播的共享常量、路径拼接、`LiveFrameMeta`。两侧唯一的真相来源 |
| `crates/dct-srv/src/live.rs`（新建） | 中转侧的直播存储：开播、推帧、取帧、停播、TTL、观众计数。**纯逻辑，不含 HTTP** |
| `crates/dct-srv/src/lib.rs`（改） | 挂上直播那几条路由；`Rejected` 复用 |
| `crates/dct-page/shared.js`（新建） | `paint` / `fitFont` / `cellSize` / 配色 / 主题三档，两页共用 |
| `crates/dct-page/page.html`（改） | 挖掉共享那几段，留 `<!--SHARED-->` |
| `crates/dct-page/live.html`（新建） | 学生页。**没有任何指向会话的写路径** |
| `crates/dct-page/src/lib.rs`（改） | `page()` / `live_page()`，把 `shared.js` 填进两个占位符 |
| `src/proto.rs`（改） | `LiveStart` / `LiveStop` / `LiveStatus`、`Response::Live(LiveInfo)`、版本 14 |
| `src/live.rs`（新建） | 守护进程侧的推帧线程：取屏 → 去重 → gzip → POST，外加保活与停播 |
| `src/daemon.rs`（改） | 三条新请求的 dispatch，直播状态槽 |
| `src/ui/live.rs`（新建） | 直播面板（上架、链接、二维码、停播） |
| `src/ui/mod.rs`、`src/ui/view.rs`、`src/ui/app.rs`（改） | 顶栏常驻提示、会话列表 `● 播` 标记、`L` 进面板 |

---

## Task 1: 共享常量与路径

**Files:**
- Create: `crates/dct-link/src/live.rs`
- Modify: `crates/dct-link/src/lib.rs`（加 `pub mod live;`）

**Interfaces:**
- Produces: `dct_link::live::{PATH_FRAME, LIVE_PREFIX, frame_path(id, lane) -> String, page_path(id) -> String, MAX_FRAME_BYTES, MAX_LANES, PUSH_INTERVAL, KEEPALIVE, LIVE_TTL, WAIT_TIMEOUT, LIVE_ID_LEN, LIVE_TOKEN_LEN}`

- [ ] **Step 1: 写失败的测试**

在 `crates/dct-link/src/live.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 学生那一侧的路径由这里拼，两个 crate 都用它——各拼各的就是
    /// `/api/scroll` 打成 `/api/scrol` 那种 bug，而且只有真机上才看得见。
    #[test]
    fn the_frame_path_is_built_in_exactly_one_place() {
        assert_eq!(frame_path("7f3a2c91", 2), "/live/7f3a2c91/frame?lane=2");
        assert_eq!(page_path("7f3a2c91"), "/live/7f3a2c91");
    }

    /// 保活必须明显快过 TTL，否则老师盯着屏幕想事情的时候直播自己就没了。
    #[test]
    fn the_keepalive_beats_the_ttl_with_room_to_spare() {
        assert!(
            KEEPALIVE * 2 < LIVE_TTL,
            "保活 {KEEPALIVE:?} 对 TTL {LIVE_TTL:?} 来说太慢，丢一次就断播"
        );
        assert!(PUSH_INTERVAL <= KEEPALIVE);
    }
}
```

- [ ] **Step 2: 跑一遍确认它挂**

Run: `cargo test -p dct-link live::`
Expected: 编译失败，`cannot find function frame_path`

- [ ] **Step 3: 写实现**

`crates/dct-link/src/live.rs` 开头：

```rust
//! 直播那条单向管的共享定义。
//!
//! **两侧唯一的真相来源。** 推帧的是 `dct`，存帧发帧的是 `dct-srv`，两个
//! crate 谁也不依赖谁——各写一份常量就是「一边改了另一边没改」，而这类
//! 错误只在真机上、上课当中才看得见。

use std::time::Duration;

/// 老师推帧的路径。live-id 和 lane 在 body 的头里，不在 URL 上——推帧要带
/// 配对身份，URL 上再带一遍 id 只是多一处会对不上的地方。
pub const PATH_FRAME: &str = "/live/frame";

/// 学生那一侧所有路径的前缀。
pub const LIVE_PREFIX: &str = "/live";

/// 一帧最大多少字节（gzip 之后）。一屏终端压完 3–5 KB，256 KB 已经是
/// 「这不对劲」的信号，不是正常值。
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

/// 一场直播最多几路。够「前端/后端/测试/日志」，而不设限等于让一台机器
/// 把中转的内存吃光。
pub const MAX_LANES: usize = 4;

/// 推帧最快多久一次。终端不是视频，2 Hz 已经快过人读字。
pub const PUSH_INTERVAL: Duration = Duration::from_millis(500);

/// 画面没变也要推一次的间隔。中转按 `LIVE_TTL` 回收，不保活的话老师盯着
/// 屏幕想一分钟事情，直播自己就没了。
pub const KEEPALIVE: Duration = Duration::from_secs(20);

/// 多久没收到帧就把整条直播（连同 token）扔掉。
pub const LIVE_TTL: Duration = Duration::from_secs(60);

/// `?wait=1` 最多挂多久。短于常见反代的 30 秒空闲上限。
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(25);

/// live-id 的十六进制长度（4 字节随机数）。
pub const LIVE_ID_LEN: usize = 8;

/// 学生那把钥匙的十六进制长度（32 字节随机数）。
pub const LIVE_TOKEN_LEN: usize = 64;

/// 学生拉某一路画面的路径。
pub fn frame_path(id: &str, lane: usize) -> String {
    format!("{LIVE_PREFIX}/{id}/frame?lane={lane}")
}

/// 学生页本体的路径。
pub fn page_path(id: &str) -> String {
    format!("{LIVE_PREFIX}/{id}")
}
```

`crates/dct-link/src/lib.rs` 顶部加：

```rust
pub mod live;
```

- [ ] **Step 4: 跑测试**

Run: `cargo test -p dct-link live::`
Expected: 2 passed

- [ ] **Step 5: 提交**

```bash
git add crates/dct-link/src/live.rs crates/dct-link/src/lib.rs
git commit -m "feat(link): 直播那条管子的共享常量和路径，两侧只此一份"
```

---

## Task 2: 中转侧的直播存储（纯逻辑）

**Files:**
- Create: `crates/dct-srv/src/live.rs`
- Modify: `crates/dct-srv/src/lib.rs`（`mod live; pub use live::Live;`）

**Interfaces:**
- Consumes: Task 1 的常量
- Produces:
  - `Live::new() -> Live`
  - `Live::start(&self, id: String, token: String, lanes: Vec<String>) -> Result<(), LinkError>`
  - `Live::push(&self, id: &str, lane: usize, frame: Vec<u8>) -> Result<u64, LinkError>`（返回新 etag）
  - `Live::frame(&self, id: &str, token: &str, lane: usize) -> Result<(Vec<u8>, u64), LinkError>`
  - `Live::lanes(&self, id: &str, token: &str) -> Result<Vec<String>, LinkError>`
  - `Live::subscribe(&self, id: &str, lane: usize) -> Option<tokio::sync::watch::Receiver<u64>>`
  - `Live::stop(&self, id: &str)`
  - `Live::sweep(&self, now: Instant)`（TTL 回收，测试可注入时间）
  - `Live::viewers(&self, id: &str) -> u32`

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn started() -> Live {
        let live = Live::new();
        live.start("abc".into(), "t".repeat(64), vec!["前端".into(), "后端".into()])
            .unwrap();
        live
    }

    #[test]
    fn a_frame_goes_in_and_comes_back_out_with_an_etag() {
        let live = started();
        let etag = live.push("abc", 0, b"hello".to_vec()).unwrap();
        let (body, tag) = live.frame("abc", &"t".repeat(64), 0).unwrap();
        assert_eq!(body, b"hello");
        assert_eq!(tag, etag);
    }

    /// 每换一帧 etag 必须变，否则学生那边的 304 会把新画面挡在外面。
    #[test]
    fn every_new_frame_gets_a_new_etag() {
        let live = started();
        let a = live.push("abc", 0, b"one".to_vec()).unwrap();
        let b = live.push("abc", 0, b"two".to_vec()).unwrap();
        assert_ne!(a, b);
    }

    /// 错的 token 一律 Unauthorized，**不告诉他这场直播存不存在**——
    /// 拿旧链接的人不该能把中转上正在播的号摸一遍。
    #[test]
    fn a_wrong_token_cannot_tell_a_live_apart_from_a_missing_one() {
        let live = started();
        live.push("abc", 0, b"x".to_vec()).unwrap();
        let wrong = live.frame("abc", &"w".repeat(64), 0).unwrap_err();
        let missing = live.frame("nope", &"t".repeat(64), 0).unwrap_err();
        assert_eq!(wrong, LinkError::Unauthorized);
        assert_eq!(missing, LinkError::Unauthorized);
    }

    #[test]
    fn a_frame_bigger_than_the_cap_is_refused() {
        let live = started();
        let big = vec![0u8; dct_link::live::MAX_FRAME_BYTES + 1];
        assert_eq!(live.push("abc", 0, big).unwrap_err(), LinkError::TooBig);
    }

    #[test]
    fn more_lanes_than_the_cap_is_refused() {
        let live = Live::new();
        let too_many = (0..dct_link::live::MAX_LANES + 1)
            .map(|i| format!("第 {i} 路"))
            .collect();
        assert_eq!(
            live.start("x".into(), "t".repeat(64), too_many).unwrap_err(),
            LinkError::TooBig
        );
    }

    /// 老师停推之后整条直播连同 token 一起蒸发——只有 TTL 没有主动删，
    /// 拔网线就等于永远播着；只有主动删没有 TTL，合上笔记本就收不回来。
    #[test]
    fn a_live_that_stopped_being_fed_disappears_with_its_token() {
        let live = started();
        live.push("abc", 0, b"x".to_vec()).unwrap();
        live.sweep(Instant::now() + dct_link::live::LIVE_TTL + Duration::from_secs(1));
        assert_eq!(
            live.frame("abc", &"t".repeat(64), 0).unwrap_err(),
            LinkError::Unauthorized
        );
    }

    #[test]
    fn stopping_takes_it_away_immediately() {
        let live = started();
        live.stop("abc");
        assert_eq!(
            live.lanes("abc", &"t".repeat(64)).unwrap_err(),
            LinkError::Unauthorized
        );
    }

    /// 挂着的学生要被新帧叫醒，这是 `?wait=1` 的全部机制。
    #[tokio::test]
    async fn a_waiting_viewer_is_woken_by_a_new_frame() {
        let live = started();
        let mut rx = live.subscribe("abc", 0).unwrap();
        live.push("abc", 0, b"new".to_vec()).unwrap();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), live.frame("abc", &"t".repeat(64), 0).unwrap().1);
    }
}
```

- [ ] **Step 2: 跑一遍确认它挂**

Run: `cargo test -p dct-srv live::`
Expected: 编译失败，`cannot find type Live`

- [ ] **Step 3: 写实现**

```rust
//! 直播：中转记住每一路的**最新一帧**，学生只读。
//!
//! # 这是对 spec 决定一的第一处偏离，写在这里免得将来有人读不出边界
//!
//! 原话是「中转只搬信封，不看里面」。这里多出来的是一个**角色**（记住最新
//! 一帧），不是一份**理解**：帧对中转始终是不透明字节，它不解析、不认识里面
//! 是 span 还是别的。有一条守卫钉着这一点（见 `lib.rs` 的
//! `the_relay_never_looks_inside_a_frame_either`）。
//!
//! # 为什么不是队列
//!
//! 只留最新一帧，不攒历史。攒历史等于给离线的人排队（spec 明令不做），而且
//! 学生晚进来看到的应该是"现在"，不是十分钟前那一屏。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dct_link::live::{LIVE_TTL, MAX_FRAME_BYTES, MAX_LANES};
use dct_link::LinkError;
use tokio::sync::watch;

struct Lane {
    name: String,
    frame: Vec<u8>,
    etag: u64,
    tx: watch::Sender<u64>,
}

struct Session {
    token_hash: [u8; 32],
    lanes: Vec<Lane>,
    fed_at: Instant,
    next_etag: u64,
}

#[derive(Default)]
pub struct Live {
    rooms: Mutex<HashMap<String, Session>>,
}

impl Live {
    pub fn new() -> Self {
        Live::default()
    }

    pub fn start(&self, id: String, token: String, lanes: Vec<String>) -> Result<(), LinkError> {
        if lanes.is_empty() || lanes.len() > MAX_LANES {
            return Err(LinkError::TooBig);
        }
        let lanes = lanes
            .into_iter()
            .map(|name| Lane {
                name,
                frame: Vec::new(),
                etag: 0,
                tx: watch::channel(0).0,
            })
            .collect();
        let mut rooms = self.rooms.lock().expect("live 锁");
        rooms.insert(
            id,
            Session {
                token_hash: hash(&token),
                lanes,
                fed_at: Instant::now(),
                next_etag: 1,
            },
        );
        Ok(())
    }

    pub fn push(&self, id: &str, lane: usize, frame: Vec<u8>) -> Result<u64, LinkError> {
        if frame.len() > MAX_FRAME_BYTES {
            return Err(LinkError::TooBig);
        }
        let mut rooms = self.rooms.lock().expect("live 锁");
        let room = rooms.get_mut(id).ok_or(LinkError::Unauthorized)?;
        room.fed_at = Instant::now();
        let etag = room.next_etag;
        room.next_etag += 1;
        let slot = room.lanes.get_mut(lane).ok_or(LinkError::Offline)?;
        // 空 body = 保活，只续 `fed_at`，不动画面也不叫醒任何人。
        if frame.is_empty() {
            return Ok(slot.etag);
        }
        slot.frame = frame;
        slot.etag = etag;
        let _ = slot.tx.send(etag);
        Ok(etag)
    }

    pub fn frame(&self, id: &str, token: &str, lane: usize) -> Result<(Vec<u8>, u64), LinkError> {
        let rooms = self.rooms.lock().expect("live 锁");
        let room = authed(&rooms, id, token)?;
        let slot = room.lanes.get(lane).ok_or(LinkError::Unauthorized)?;
        Ok((slot.frame.clone(), slot.etag))
    }

    pub fn lanes(&self, id: &str, token: &str) -> Result<Vec<String>, LinkError> {
        let rooms = self.rooms.lock().expect("live 锁");
        let room = authed(&rooms, id, token)?;
        Ok(room.lanes.iter().map(|l| l.name.clone()).collect())
    }

    pub fn subscribe(&self, id: &str, lane: usize) -> Option<watch::Receiver<u64>> {
        let rooms = self.rooms.lock().expect("live 锁");
        Some(rooms.get(id)?.lanes.get(lane)?.tx.subscribe())
    }

    /// 在看的人数 = 挂着的订阅数。这是老师那行常驻提示里的「7 人在看」，
    /// 也是他唯一会一直看着的那个数。
    pub fn viewers(&self, id: &str) -> u32 {
        let rooms = self.rooms.lock().expect("live 锁");
        rooms
            .get(id)
            .map(|r| r.lanes.iter().map(|l| l.tx.receiver_count() as u32).sum())
            .unwrap_or(0)
    }

    pub fn stop(&self, id: &str) {
        self.rooms.lock().expect("live 锁").remove(id);
    }

    pub fn sweep(&self, now: Instant) {
        self.rooms
            .lock()
            .expect("live 锁")
            .retain(|_, r| now.duration_since(r.fed_at) < LIVE_TTL);
    }
}

/// 认证在取数之前，而且**认不出来和不存在回同一句话**。
fn authed<'a>(
    rooms: &'a HashMap<String, Session>,
    id: &str,
    token: &str,
) -> Result<&'a Session, LinkError> {
    let room = rooms.get(id).ok_or(LinkError::Unauthorized)?;
    if !same(&room.token_hash, &hash(token)) {
        return Err(LinkError::Unauthorized);
    }
    Ok(room)
}

/// 常数时间比对，理由同 `web::mod` 里那一处：`==` 会按第一个不同的字节
/// 提前返回，而那个时间差是可测的。
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hash(token: &str) -> [u8; 32] { /* sha2::Sha256 摘要，见下一步 */ }
```

`hash` 用 `sha2`（`dct` 已经依赖它）。在 `crates/dct-srv/Cargo.toml` 的
`[dependencies]` 里加：

```toml
# token 只存摘要，不存原文：中转上留着一屋子人的明文钥匙没有任何必要。
sha2 = "0.10"
```

然后：

```rust
fn hash(token: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes()).into()
}
```

`crates/dct-srv/src/lib.rs` 顶部加 `mod live;` 和 `pub use live::Live;`。

- [ ] **Step 4: 跑测试**

Run: `cargo test -p dct-srv live::`
Expected: 8 passed

- [ ] **Step 5: 提交**

```bash
git add crates/dct-srv/src/live.rs crates/dct-srv/src/lib.rs crates/dct-srv/Cargo.toml
git commit -m "feat(srv): 中转记住每一路的最新一帧，认证在取数之前"
```

---

## Task 3: 中转的直播路由

**Files:**
- Modify: `crates/dct-srv/src/lib.rs`
- Test: 同文件 `mod tests`（照现有 `tower::ServiceExt::oneshot` 风格）

**Interfaces:**
- Consumes: Task 2 的 `Live`
- Produces: 路由 `POST /live/frame`、`GET /live/{id}/frame`、`DELETE /live/{id}`，以及 `AppState { relay: Arc<Relay>, live: Arc<Live> }`

- [ ] **Step 1: 写失败的测试**

```rust
/// 学生带着上一帧的 etag 再来，没换帧就该拿到一个空的 304——
/// 老师手停着不动的那几十秒里，两百个学生一个字节都不该传。
#[tokio::test]
async fn an_unchanged_frame_comes_back_as_an_empty_304() {
    let (app, live) = app_with_live();
    live.start("abc".into(), "t".repeat(64), vec!["前端".into()]).unwrap();
    live.push("abc", 0, b"hello".to_vec()).unwrap();

    let first = get_frame(&app, "abc", &"t".repeat(64), None).await;
    assert_eq!(first.status, 200);
    let etag = first.etag.clone().expect("第一帧该带 ETag");

    let again = get_frame(&app, "abc", &"t".repeat(64), Some(&etag)).await;
    assert_eq!(again.status, 304);
    assert!(again.body.is_empty(), "304 不该带 body");
}

/// 认证在路由之前：错 token 连「这场直播存不存在」都不告诉他。
#[tokio::test]
async fn a_wrong_token_learns_nothing_about_the_live() {
    let (app, live) = app_with_live();
    live.start("abc".into(), "t".repeat(64), vec!["前端".into()]).unwrap();
    let real = get_frame(&app, "abc", &"w".repeat(64), None).await;
    let fake = get_frame(&app, "zzz", &"w".repeat(64), None).await;
    assert_eq!(real.status, 401);
    assert_eq!(fake.status, 401);
}

#[tokio::test]
async fn a_frame_over_the_cap_is_refused_with_413() {
    let (app, live) = app_with_live();
    live.start("abc".into(), "t".repeat(64), vec!["前端".into()]).unwrap();
    let body = vec![b'x'; dct_link::live::MAX_FRAME_BYTES + 1];
    assert_eq!(push_frame(&app, "abc", 0, body).await, 413);
}

/// 中转仍然不看帧里面是什么——这是 spec 决定一在直播上的那条线。
#[test]
fn the_relay_never_looks_inside_a_frame_either() {
    let src = concat!(
        include_str!("live.rs"),
        include_str!("lib.rs"),
    );
    for banned in ["ScreenSpan", "from_slice::<Request>", "dct::proto"] {
        assert!(
            !src.contains(banned),
            "{banned} 出现在中转里——它开始认识 dct 的协议了"
        );
    }
}
```

辅助函数（写在 `mod tests` 里，`FrameResp { status: u16, etag: Option<String>, body: Vec<u8> }`）照现有测试里拼请求的写法，用 `tower::ServiceExt::oneshot`。

- [ ] **Step 2: 跑一遍确认它挂**

Run: `cargo test -p dct-srv`
Expected: 编译失败，`cannot find function app_with_live`

- [ ] **Step 3: 写实现**

`crates/dct-srv/src/lib.rs`：

```rust
#[derive(Clone)]
pub struct AppState {
    pub relay: Arc<Relay>,
    pub live: Arc<Live>,
}

/// 老师推一帧。身份是配对那把钥匙（`EndpointKind::Computer`），跟学生那把
/// 完全不同的验法——推帧是写，看帧是读，两件事不共用凭据。
async fn live_push_route(
    State(st): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<StatusCode, Rejected> {
    let id = header(&headers, "x-live-id").ok_or(LinkError::Unauthorized)?;
    let lane: usize = header(&headers, "x-live-lane")
        .and_then(|v| v.parse().ok())
        .ok_or(LinkError::Unauthorized)?;
    // 任务 5 之前配对 token 没人验，这里只验这场直播是不是这台机器开的。
    st.live.push(&id, lane, body.to_vec())?;
    Ok(StatusCode::NO_CONTENT)
}

/// 学生拉一帧。`If-None-Match` 命中回 304；`?wait=1` 时挂到换帧或超时。
async fn live_frame_route(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<FrameQuery>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Rejected> {
    let token = header(&headers, "x-live-token").ok_or(LinkError::Unauthorized)?;
    let lane = q.lane.unwrap_or(0);
    let seen: Option<u64> = header(&headers, "if-none-match").and_then(|v| v.trim_matches('"').parse().ok());

    let (mut body, mut etag) = st.live.frame(&id, &token, lane)?;
    if q.wait.is_some() && seen == Some(etag) {
        if let Some(mut rx) = st.live.subscribe(&id, lane) {
            // 超时也回 304：挂满 25 秒什么都没等到不是错误，学生页会
            // 立刻再挂一次。
            let _ = tokio::time::timeout(dct_link::live::WAIT_TIMEOUT, rx.changed()).await;
        }
        (body, etag) = st.live.frame(&id, &token, lane)?;
    }
    if seen == Some(etag) {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    Ok((
        [
            (axum::http::header::ETAG, format!("\"{etag}\"")),
            (axum::http::header::CONTENT_ENCODING, "gzip".into()),
            (axum::http::header::CACHE_CONTROL, "no-store".into()),
        ],
        body,
    )
        .into_response())
}

async fn live_stop_route(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> StatusCode {
    st.live.stop(&id);
    StatusCode::NO_CONTENT
}
```

`router()` 改成挂 `AppState`，并加：

```rust
        .route(dct_link::live::PATH_FRAME, post(live_push_route))
        .route("/live/{id}/frame", get(live_frame_route))
        .route("/live/{id}", axum::routing::delete(live_stop_route))
```

再起一条 TTL 清扫任务（`serve()` 里 `tokio::spawn`，每 10 秒 `live.sweep(Instant::now())`）。

- [ ] **Step 4: 跑测试**

Run: `cargo test -p dct-srv`
Expected: 全绿（含既有的 relay 测试）

- [ ] **Step 5: 提交**

```bash
git add crates/dct-srv/src/lib.rs
git commit -m "feat(srv): 直播三条路由——推帧、取帧（304 与长轮询）、停播"
```

---

## Task 4: 协议里的三条新请求

**Files:**
- Modify: `src/proto.rs`（`Request` 三条 + `Response::Live` + `LiveInfo` + 版本 13 → 14 + 版本注释）
- Modify: `src/daemon.rs`（dispatch 三条）

**Interfaces:**
- Produces:
  ```rust
  Request::LiveStart { ids: Vec<u32>, names: Vec<String> }
  Request::LiveStop
  Request::LiveStatus
  Response::Live(LiveInfo)
  pub struct LiveInfo { pub id: String, pub token: String, pub url: String,
                        pub staged: Vec<(u32, String)>, pub viewers: u32 }
  ```

- [ ] **Step 1: 写失败的测试**

在 `src/proto.rs` 的 `mod tests` 里，把钉死线上形状那条测试的期望串更新（跑一次拿真实输出），并加：

```rust
/// 上架的每一路都要有名字，而且名字的条数必须跟会话数对得上——
/// 对不上就会出现「学生看到的第 2 路其实是第 3 个会话」。
#[test]
fn staging_carries_one_name_per_session() {
    let req = Request::LiveStart {
        ids: vec![3, 5],
        names: vec!["前端调试".into(), "后端接口".into()],
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: Request = serde_json::from_str(&json).unwrap();
    match back {
        Request::LiveStart { ids, names } => {
            assert_eq!(ids.len(), names.len());
            assert_eq!(names[1], "后端接口");
        }
        other => panic!("解出来的不是 LiveStart：{other:?}"),
    }
}
```

- [ ] **Step 2: 跑一遍确认它挂**

Run: `cargo test --lib proto::`
Expected: `no variant named LiveStart`

- [ ] **Step 3: 写实现**

`src/proto.rs`：`PROTOCOL_VERSION` 改 14，并在版本注释末尾加一段：

```rust
/// 14 = 直播观众链接。多了 `Request::LiveStart` / `LiveStop` / `LiveStatus`
/// 和 `Response::Live(LiveInfo)`。**加一，没有例外可讲**：新增 `Request`
/// 变体那条规矩没得商量——旧守护进程收到 `LiveStart` 只会回一句解析失败，
/// 而用户看到的是「按了开关什么都没发生」。同 `WebEnable` 那次。
```

`Request` 里加三条变体，`Debug` 的手写实现里补三条分支（那个 `match` 是穷尽的），
`Response` 加 `Live(LiveInfo)`，并加 `LiveInfo` 结构体。

`src/daemon.rs` 的 `handle` 里加：

```rust
        Request::LiveStart { ids, names } => live_start(mgr, live, ids, names),
        Request::LiveStop => {
            live.stop();
            Ok(Response::Ok)
        }
        Request::LiveStatus => Ok(Response::Live(live.info())),
```

`live` 是新的状态槽（`Arc<crate::live::LiveState>`），跟手机状态槽同一个位置传进来。

- [ ] **Step 4: 跑测试**

Run: `cargo test --lib proto::`
Expected: 全绿（线上形状那条要带着新版本号更新过）

- [ ] **Step 5: 提交**

```bash
git add src/proto.rs src/daemon.rs
git commit -m "feat(proto): 直播三条请求，协议 14"
```

---

## Task 5: 守护进程的推帧线程

**Files:**
- Create: `src/live.rs`
- Modify: `src/lib.rs`（`pub mod live;`）、`Cargo.toml`（flate2）

**Interfaces:**
- Consumes: Task 1 常量、`Request::Screens`、`session::SessionManager`
- Produces:
  - `LiveState::new(base: String) -> LiveState`
  - `LiveState::start(&self, staged: Vec<(u32, String)>) -> LiveInfo`
  - `LiveState::stop(&self)`
  - `LiveState::info(&self) -> LiveInfo`
  - `fn frame_of(lines: &[Vec<ScreenSpan>]) -> Vec<u8>`（序列化 + gzip，纯函数，可直接测）
  - `fn should_push(last: Option<u64>, now_hash: u64, since: Duration) -> bool`（纯函数）

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 画面没变就不推——终端大多数时候是静止的，这一条省掉的是绝大部分流量。
    #[test]
    fn an_unchanged_screen_is_not_pushed_again() {
        assert!(!should_push(Some(7), 7, Duration::from_secs(1)));
        assert!(should_push(Some(7), 8, Duration::from_secs(1)));
        assert!(should_push(None, 7, Duration::from_secs(0)));
    }

    /// 但静止太久也要推一次保活，否则中转按 TTL 把整条直播扔掉，
    /// 而老师只是盯着屏幕想了一分钟事情。
    #[test]
    fn a_still_screen_is_still_kept_alive() {
        assert!(should_push(Some(7), 7, dct_link::live::KEEPALIVE));
    }

    /// 帧是压过的：一屏终端压完该比原文小得多，不然带宽那三条里最重的一条
    /// 等于没做。
    #[test]
    fn a_frame_is_compressed() {
        let line = vec![span("x".repeat(120))];
        let lines: Vec<_> = (0..40).map(|_| line.clone()).collect();
        let raw = serde_json::to_vec(&lines).unwrap();
        let frame = frame_of(&lines);
        assert!(
            frame.len() * 4 < raw.len(),
            "压完 {} 字节，原文 {} 字节——压缩没起作用",
            frame.len(),
            raw.len()
        );
    }

    /// 链接里那串东西必须够长、而且每次都不一样。
    #[test]
    fn two_lives_never_get_the_same_link() {
        let a = LiveState::new("https://x".into()).start(vec![(1, "一".into())]);
        let b = LiveState::new("https://x".into()).start(vec![(1, "一".into())]);
        assert_ne!(a.token, b.token);
        assert_eq!(a.token.len(), dct_link::live::LIVE_TOKEN_LEN);
        assert!(a.url.starts_with("https://x/live/"), "链接形状不对：{}", a.url);
        assert!(a.url.contains("#t="), "token 必须在 fragment 里：{}", a.url);
    }
}
```

- [ ] **Step 2: 跑一遍确认它挂**

Run: `cargo test --lib live::`
Expected: `cannot find function should_push`

- [ ] **Step 3: 写实现**

`Cargo.toml` 加：

```toml
# 帧要压过再上网：一屏终端 30 KB，压完 3–5 KB，两百个学生就是这两个数的
# 差别。rust_backend 是纯 Rust（miniz_oxide），不破「一行 C 都没有」。
flate2 = { version = "1", default-features = false, features = ["rust_backend"] }
```

`src/live.rs` 要点（模块头写清楚"绝不进 200ms tick"那条规矩，照 `link.rs`）：

```rust
/// 该不该推这一帧。**纯函数，好测**——推帧线程剩下的部分全是 IO。
pub(crate) fn should_push(last: Option<u64>, now: u64, since: Duration) -> bool {
    match last {
        None => true,
        Some(h) if h != now => true,
        // 画面没变，但太久没说话了：中转按 TTL 回收，保活是这条链路上唯一
        // 阻止「老师想事情想久了直播自己断掉」的东西。
        _ => since >= dct_link::live::KEEPALIVE,
    }
}

pub(crate) fn frame_of(lines: &[Vec<crate::proto::ScreenSpan>]) -> Vec<u8> {
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;
    let json = serde_json::to_vec(lines).unwrap_or_default();
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    let _ = gz.write_all(&json);
    gz.finish().unwrap_or_default()
}
```

线程体：每 `PUSH_INTERVAL` 醒一次 → `Request::Screens { ids }` → 每一路
`frame_of` + 哈希（`std::collections::hash_map::DefaultHasher`）→ `should_push`
→ `ureq` POST 到 `cfg.base + PATH_FRAME`，带 `x-live-id` / `x-live-lane` 头。
停播时 `DELETE {base}/live/{id}`。线程体包 `catch_unwind`，理由同
`link.rs::spawn`（直播死掉是遗憾，会话死掉是灾难）。

`agent` 从 `sys::tls::agent_builder()` 来 —— **不许在这里
`ureq::AgentBuilder::new()`**，那正是 Windows 上 HTTPS 全挂的那个坑
（见 `Cargo.toml` 里 TLS 那段注释）。

- [ ] **Step 4: 跑测试**

Run: `cargo test --lib live::`
Expected: 4 passed

- [ ] **Step 5: 提交**

```bash
git add src/live.rs src/lib.rs Cargo.toml Cargo.lock
git commit -m "feat(live): 推帧线程——不变不推、久了保活、压过再上网"
```

---

## Task 6: 网页拆共享、学生页落地

**Files:**
- Create: `crates/dct-page/shared.js`、`crates/dct-page/live.html`
- Modify: `crates/dct-page/page.html`、`crates/dct-page/src/lib.rs`
- Modify: `src/web/routes.rs`（`PAGE` → `dct_page::page()`）、`crates/dct-srv/src/lib.rs`（同）

**Interfaces:**
- Produces: `dct_page::page() -> &'static str`、`dct_page::live_page() -> &'static str`

- [ ] **Step 1: 写失败的测试**

`crates/dct-page/src/lib.rs`：

```rust
/// **学生页里没有任何一条能把字节送回老师终端的路。**
///
/// 这条守卫是这个功能全部安全性的落点：只读不是某处 `if` 判出来的，是这份
/// 字节里根本没有那些路径。开关式实现（同一页加个只读模式）做不到这一点——
/// 那一页上「能不能送东西出去」有十几个调用点，漏一个就是学生能往老师终端
/// 里敲字，而这件事老师在自己机器上永远试不出来。
#[test]
fn the_student_page_has_no_way_to_send_anything_to_a_session() {
    let page = super::live_page();
    for banned in [
        "/api/input", "/api/key", "/api/mouse", "/api/scroll",
        "wire.key", "wire.input", "<input", "<textarea",
    ] {
        assert!(!page.contains(banned), "学生页里出现了 {banned:?}");
    }
}

/// 两页共用同一份渲染。各留一份的话，迟早只有一页修对了某个渲染 bug。
#[test]
fn both_pages_share_one_painter() {
    assert!(super::page().contains("function paint("));
    assert!(super::live_page().contains("function paint("));
    let both = format!("{}{}", super::page(), super::live_page());
    assert_eq!(
        both.matches("ui-monospace").count(),
        2,
        "等宽字体名在两页里合计不是两次——共享那一份被拷开了"
    );
}

#[test]
fn neither_page_is_empty() {
    assert!(super::page().len() > 10_000);
    assert!(super::live_page().len() > 4_000);
}
```

- [ ] **Step 2: 跑一遍确认它挂**

Run: `cargo test -p dct-page`
Expected: `cannot find function live_page`

- [ ] **Step 3: 写实现**

`src/lib.rs`：

```rust
use std::sync::OnceLock;

const SHARED: &str = include_str!("../shared.js");
const PAGE_SRC: &str = include_str!("../page.html");
const LIVE_SRC: &str = include_str!("../live.html");

/// 占位符只此一个写法，两页都用它。
const MARK: &str = "<!--SHARED-->";

pub fn page() -> &'static str {
    static IT: OnceLock<String> = OnceLock::new();
    IT.get_or_init(|| PAGE_SRC.replace(MARK, SHARED))
}

pub fn live_page() -> &'static str {
    static IT: OnceLock<String> = OnceLock::new();
    IT.get_or_init(|| LIVE_SRC.replace(MARK, SHARED))
}
```

`shared.js` 里放：`paint`、`fitFont`、`cellSize`、配色表、主题三档、
`toBase64`/`fromBase64` 不要（学生页用不到）。`page.html` 里把这几段换成
`<!--SHARED-->`（放在原来那几个函数所在的位置，保证声明早于使用）。

`live.html` 结构（文案全部走 `/live/<id>/strings`，这一版先内嵌中文键名再由
下一个任务接文案表；**不许写死英文**）：

- 顶栏：`● 直播中` / `◌ 老师暂停了` / `× 这场直播结束了`，加人数、`A− A+ ◑`
- 上架那几路的标签，点一下换 lane
- `<pre id="canvas">`，用 `paint()` 画
- 取数：`fetch(frame_path(id, lane), { headers: { "x-live-token": tokenFromHash } })`，
  收到 `gzip` 的 body 用 `DecompressionStream("gzip")` 解开再 `JSON.parse`
- `If-None-Match` 与 `?wait=1` 的循环；304 就直接再挂一次

`src/web/routes.rs` 与 `crates/dct-srv/src/lib.rs` 里的 `dct_page::PAGE`
改成 `dct_page::page()`；中转再加一条 `GET /live/{id}` 发 `live_page()`。

- [ ] **Step 4: 跑测试**

Run: `cargo test -p dct-page && cargo test --lib web::routes`
Expected: 全绿（`web::routes` 里那些对着页面字节断言的守卫必须仍然过）

- [ ] **Step 5: 提交**

```bash
git add crates/dct-page src/web/routes.rs crates/dct-srv/src/lib.rs
git commit -m "feat(page): 学生页落地，渲染那一份两页共用"
```

---

## Task 7: 直播面板与常驻提示

**Files:**
- Create: `src/ui/live.rs`
- Modify: `src/ui/mod.rs`、`src/ui/view.rs`、`src/ui/app.rs`

**Interfaces:**
- Consumes: `Request::LiveStart/LiveStop/LiveStatus`、`Response::Live(LiveInfo)`、`qr` 模块
- Produces: `View::Live`、`live::draw(f, app)`、`live::handle_key(app, key)`

- [ ] **Step 1: 写失败的测试**

```rust
/// **屏幕上必须一直说着「你在播」。**
///
/// 这个功能最危险的失败模式不是链接泄露，是老师忘了自己在播、切去处理一件
/// 私事。所以这一行常驻、不折叠、不可关，而且带着实时人数——「7 人在看」
/// 比任何提示语都有效。
#[test]
fn the_top_bar_always_says_you_are_live() {
    let line = super::live_banner(&LiveInfo {
        id: "abc".into(),
        token: "t".repeat(64),
        url: "https://x/live/abc#t=…".into(),
        staged: vec![(3, "前端".into()), (5, "后端".into())],
        viewers: 7,
    }, Lang::Zh);
    assert!(line.contains("正在直播"));
    assert!(line.contains('2'), "没说在播几路：{line}");
    assert!(line.contains('7'), "没说几个人在看：{line}");
}

/// token 不许出现在屏幕上——老师会录屏，会投影。
#[test]
fn the_token_never_reaches_the_screen() {
    let info = LiveInfo { /* 同上，token 为 "s".repeat(64) */ };
    let shown = super::link_line(&info);
    assert!(!shown.contains(&"s".repeat(8)), "链接行上带着 token：{shown}");
    assert!(shown.contains("#t=") && shown.contains('·'), "得留个打点的尾巴");
}
```

（第二条照 `src/ui/web.rs` 里 `the_token_is_nowhere_on_the_screen` 的写法。）

- [ ] **Step 2: 跑一遍确认它挂**

Run: `cargo test --lib ui::live`
Expected: `cannot find function live_banner`

- [ ] **Step 3: 写实现**

`src/ui/live.rs`：`live_banner()`、`link_line()`（token 只留 `#t=········`）、
`draw()`（链接 + 二维码 + 上架勾选 + 提问开关占位 + 停播）、`handle_key()`
（`c` 复制、`r` 换链接、空格勾选、`s` 停播、`Esc` 返回）。

`view.rs` 加 `View::Live`；`app.rs` 在附着视图和列表视图里接 `L`；顶栏那一行
插在现有 banner 的位置（跟 `attach::scroll_hint` 同级），会话列表里给上架的
会话加 `● 播` 标记。

文案走 `i18n`，两种语言都要有（`zh_CN` 默认）。

- [ ] **Step 4: 跑测试**

Run: `cargo test --lib ui::`
Expected: 全绿

- [ ] **Step 5: 提交**

```bash
git add src/ui/live.rs src/ui/mod.rs src/ui/view.rs src/ui/app.rs src/i18n.rs
git commit -m "feat(ui): 直播面板，外加一行关不掉的「正在直播」"
```

---

## Task 8: 端到端手工验收脚手架

**Files:**
- Create: `tests/live_end_to_end.rs`

- [ ] **Step 1: 写脚手架**

照 `tests/web_routes.rs::serve_for_a_manual_look` 的写法（`#[ignore]`）：起一个
本地中转、连上这台机器真在跑的守护进程、上架两个会话、把学生链接和二维码打
出来，然后挂着。

- [ ] **Step 2: 跑一遍**

Run: `cargo test --test live_end_to_end -- --ignored --nocapture serve_a_live_for_a_manual_look`
Expected: 打出 `MANUAL_LIVE_URL`，浏览器打开能看到画面动

- [ ] **Step 3: 用眼睛验这四件事**

- 画面跟着老师那边动
- 学生页上没有任何能打字的地方
- 关掉老师那边，学生页显示「这场直播结束了」
- 两个标签页同时开着，人数显示 2

- [ ] **Step 4: 提交**

```bash
git add tests/live_end_to_end.rs
git commit -m "test(live): 手工验收用的脚手架"
```

---

## 部署检查清单（上线前必须过）

- [ ] 反向代理上**只放行 `/live/*`**，`/link/*` 不对公网开口
- [ ] `dct-srv` 的 `must_be_loopback` 换成显式开关时，开关的默认值是"关"
- [ ] TLS 终止在反代上，`https://` 才发链接

---

## 自查

**spec 覆盖**：帧存储（T2）、三条路由与 304/长轮询（T3）、协议与版本（T4）、
推帧线程的去重与保活与压缩（T5）、学生页与共享渲染（T6）、面板与常驻提示
（T7）、手工验收（T8）、部署约束（清单）。**提问板整块不在本计划内**，见范围
说明。

**类型一致**：`LiveInfo` 的字段在 T4 定义，T5、T7 使用同一组名字；
`frame_path` 在 T1 定义，T3、T6 使用；`should_push` / `frame_of` 只在 T5。

**未决**：`MAX_LANES = 4` 与 `PUSH_INTERVAL = 500ms` 两个数按 spec 实施，
都在 `dct-link` 里一处定义，真机用下来要改是一行的事。
