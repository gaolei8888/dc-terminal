# dct 手机连接 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** agent 停下来时把消息推到用户手机上，用户在 Telegram 里回一句话，dct 把它敲进对应的 agent。

**Architecture:** 渠道住在守护进程里。`session.rs::tick` 只往队列投事件（**绝不碰网络**），bridge 自己的线程消费队列做出站、同时长轮询做入站。渠道在 trait 后面、传输层注入，全部零网络可测。

**Tech Stack:** Rust ≥ 1.80，`ureq`（已有依赖，阻塞式 HTTP），`serde_json`（已有），`ratatui`/`crossterm`（已有 TUI）。**不引入 async 运行时。**

**Spec:** `docs/superpowers/specs/2026-08-10-dct-phone-channel-design.md`

## Global Constraints

- **这份计划里的参考代码不是权威。** 它是意图的说明，不是可以照抄的成品。此前连续三轮，实施计划的参考代码里埋着真缺陷，而抓住它们的是变异测试，不是更仔细地阅读计划。**照抄之前先想它对不对；测试通过之后做变异测试。**
- **变异测试是每个任务的收尾动作**：把实现里的一个判断取反、一个边界 ±1、一个 `&&` 改成 `||`，跑测试。**没有测试失败 = 测试没写够，回去补。**
- `tick()` 线程绝不做网络 IO、绝不同步等模型。200ms 的循环卡住 = 整个守护进程卡住。
- 每一处 LLM 用法都有不依赖 LLM 的退路。退化方向永远是「dct 变回今天的样子」，不是「dct 坏了」。
- bridge 线程 panic 绝不能拖垮守护进程或任何会话。
- 每个用户能看到的字符串都写给没编过程序的人：不用黑话、不给栈追踪、不给原始 OS 错误。**错误信息不给出下一步就是没写完。**
- **不用 emoji 当图标。**
- 界面文案中英双语，走已有的 `src/i18n.rs`（`Key` 枚举 + `text()`）。
- **key 处理分支里永远不要 `continue`**（`ui/mod.rs`、`board.rs`、`settings_view.rs` 都有这条注释，理由是循环末尾要清理陈旧 message）。
- 提交信息用英文，**不要 AI 署名行**。
- 每个任务结束前跑：`cargo test -- --test-threads=1`、`cargo fmt --check`、`cargo clippy --all-targets`。
- 测试不碰网络、不碰真实 `~/.dct`（数据路径一律从 socket 路径派生，测试指向临时目录）。

---

## File Structure

| 文件 | 职责 |
|---|---|
| `src/channel/mod.rs`（新建） | 渠道 trait、`Incoming`、`ChannelError`、出站 `Event`、防抖、合并 |
| `src/channel/telegram.rs`（新建） | Telegram 适配器：`getUpdates` / `sendMessage` / `getMe` 的解析与构造 |
| `src/bridge.rs`（新建） | 连接层：配对、路由、出站线程、入站线程。唯一有状态的地方 |
| `src/session.rs`（改） | `tick()` 里投递事件；新增事件队列的发送端 |
| `src/proto.rs`（改） | `Request::Phone*` / `Response::Phone*` |
| `src/daemon.rs`（改） | 分发新请求；启动 bridge 线程 |
| `src/secrets.rs`（改） | 保留名字常量 |
| `src/ui/view.rs`（改） | `View::Settings` 改结构；新增 `View::Phone` |
| `src/ui/settings_view.rs`（改） | 从语言列表改成设置项列表 |
| `src/ui/phone.rs`（新建） | 手机通知页 |
| `src/i18n.rs`（改） | 新增文案 Key |

---

## Task 1: 渠道 trait 与出站事件类型

**Files:**
- Create: `src/channel/mod.rs`
- Modify: `src/lib.rs`（加 `pub mod channel;`）
- Test: 同文件内 `#[cfg(test)] mod tests`（本仓库惯例）

**Interfaces:**
- Consumes: 无
- Produces: `MsgId`、`Incoming`、`ChannelError`、`Channel` trait、`EventKind`、`Event`、`debounce()`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 防抖只压快速抖动，不压真正的第二个事件。
    #[test]
    fn debounce_suppresses_only_inside_the_window() {
        let w = Duration::from_secs(30);
        // 从没发过：一定发
        assert!(debounce(None, Duration::from_secs(0), w));
        // 窗口内：压掉
        assert!(!debounce(Some(Duration::from_secs(10)), Duration::from_secs(20), w));
        // 正好在窗口边界上：压掉（边界属于窗口内）
        assert!(!debounce(Some(Duration::from_secs(10)), Duration::from_secs(40), w));
        // 窗口外：发
        assert!(debounce(Some(Duration::from_secs(10)), Duration::from_secs(41), w));
    }

    /// `ChannelError` 必须把「重试有意义」和「重试没意义」分开——
    /// 这个区分是错误处理那一节的全部依据，合并成一个错误就没法写退避了。
    #[test]
    fn bad_token_is_not_retryable_but_unreachable_is() {
        assert!(ChannelError::Unreachable.worth_retrying());
        assert!(!ChannelError::BadToken.worth_retrying());
        assert!(!ChannelError::Malformed.worth_retrying());
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `cargo test --lib channel:: -- --test-threads=1`
Expected: 编译失败，`cannot find function debounce` / `cannot find type ChannelError`

- [ ] **Step 3: 写最小实现**

```rust
//! 把消息送到用户手机上、再把他的回复取回来。
//!
//! **这一层不认识会话，也不认识 dct 的任何状态。** 它只知道「发一段文字」
//! 和「取回一些文字」。谁该收到、敲给谁，全在 `bridge.rs`。

pub mod telegram;

use std::time::Duration;

/// 渠道那边的消息 id。长按回复靠它把回复关联回某个会话。
/// Telegram 的 `message_id` 是有符号整数，这里跟着它走。
pub type MsgId = i64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    pub text: String,
    /// 用户长按回复的是哪一条。直接发的话是 `None`。
    pub reply_to: Option<MsgId>,
    /// 谁发的。**配对之后只认一个**，见 `bridge.rs`。
    pub chat_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    /// 网络问题。**重试有意义。**
    Unreachable,
    /// 令牌无效或被撤销。**重试一万次还是这个结果**，退避重试是在浪费时间，
    /// 而且会把「该让用户去重填令牌」这件事永远拖着不说。
    BadToken,
    /// 回来了但读不懂。当作坏消息处理，不猜。
    Malformed,
}

impl ChannelError {
    pub fn worth_retrying(self) -> bool {
        matches!(self, ChannelError::Unreachable)
    }
}

pub trait Channel: Send + Sync {
    /// 发一条，返回渠道那边的消息 id。
    fn send(&self, text: &str) -> Result<MsgId, ChannelError>;
    /// 取新消息，最多阻塞 `timeout`。没有新消息就返回空 `Vec`，不是错误。
    fn poll(&self, timeout: Duration) -> Result<Vec<Incoming>, ChannelError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// 干完一轮停下来了
    Stopped,
    /// 报错了
    Failed,
    /// 会话自己没了
    Vanished,
}

/// 一个值得告诉用户的事。**字段全是已经成文的用户语言**——守护进程是
/// 唯一决定用户看到什么文字的地方，这条沿用 `proto.rs` 里
/// 「`ProfileEntry.label` 是 `String` 不是 `LocalizedText`」的同一个理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub session: u32,
    pub kind: EventKind,
    /// 会话名。自动命名功能早就生成好了，这里直接用。
    pub name: String,
    pub project: String,
}

/// 这个事件该发吗？
///
/// `last` 是这个会话上次发出去的时刻，`now` 是现在，都相对于同一个起点。
/// 用 `Duration` 而不是 `Instant` 是为了让测试能给出确定的时间点——
/// `Instant` 造不出「10 秒前」。
///
/// **边界算窗口内**（`<=`）：窗口是「这段时间内不再打扰」，端点上还是那段时间。
pub fn debounce(last: Option<Duration>, now: Duration, window: Duration) -> bool {
    match last {
        None => true,
        Some(last) => now.saturating_sub(last) > window,
    }
}

/// 防抖窗口的起点值。**这个数字是拍出来的**，spec 的「未验证」一节记着它：
/// 偏小会吵人，偏大会漏掉真正的第二个事件。实测之后回来调。
pub const DEBOUNCE_WINDOW: Duration = Duration::from_secs(30);
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib channel:: -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把 `debounce` 里的 `>` 改成 `>=`，跑测试 —— `debounce_suppresses_only_inside_the_window` 里的边界那条必须失败。把 `worth_retrying` 的 `matches!` 改成 `!matches!`，第二个测试必须失败。**都改回去。任何一个没失败就是测试没写够。**

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/channel/mod.rs src/lib.rs
git commit -m "feat: a channel is something you can send to and poll from

The trait knows nothing about sessions. It sends text and takes text back;
who should receive it and which agent a reply belongs to lives in bridge.

ChannelError splits retryable from not: retrying a revoked token forever
would also mean never telling the user to go re-enter it."
```

---

## Task 2: Telegram 适配器

**Files:**
- Create: `src/channel/telegram.rs`
- Test: 同文件内

**Interfaces:**
- Consumes: Task 1 的 `Channel`、`Incoming`、`ChannelError`、`MsgId`
- Produces: `parse_updates(&str) -> Result<Vec<Incoming>, ChannelError>`、`parse_send_result(&str) -> Result<MsgId, ChannelError>`、`parse_get_me(&str) -> Result<String, ChannelError>`、`Telegram::new(token)`、`Telegram::with_transport(token, send)`

- [ ] **Step 1: 写失败测试**

判定逻辑与传输分离，沿用 `verify.rs::verify_with` 的套路。

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 真实回包的形状。少一个字段就该是 Malformed，不该 panic、不该猜。
    #[test]
    fn parses_a_normal_update() {
        let body = r#"{"ok":true,"result":[{"update_id":1,"message":{
            "message_id":42,"chat":{"id":777},"text":"先跑完"}}]}"#;
        let got = parse_updates(body).expect("正常回包该解得出来");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "先跑完");
        assert_eq!(got[0].chat_id, 777);
        assert_eq!(got[0].reply_to, None);
    }

    #[test]
    fn picks_up_the_message_being_replied_to() {
        let body = r#"{"ok":true,"result":[{"update_id":2,"message":{
            "message_id":43,"chat":{"id":777},"text":"就第二个",
            "reply_to_message":{"message_id":42}}}]}"#;
        let got = parse_updates(body).unwrap();
        assert_eq!(got[0].reply_to, Some(42));
    }

    /// 没有 text 的更新（图片、贴纸、有人进群）不是错误，是「这条没什么可读的」。
    /// 当成 Malformed 会让一张图片害得整轮轮询失败。
    #[test]
    fn updates_without_text_are_skipped_not_errors() {
        let body = r#"{"ok":true,"result":[{"update_id":3,"message":{
            "message_id":44,"chat":{"id":777}}}]}"#;
        assert_eq!(parse_updates(body).unwrap().len(), 0);
    }

    /// 令牌被撤销时 Telegram 回 ok:false + 401。**必须区分出 BadToken**，
    /// 否则退避重试会永远转下去，用户永远等不到「去重填令牌」这句话。
    #[test]
    fn a_revoked_token_is_bad_token_not_unreachable() {
        let body = r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#;
        assert_eq!(parse_updates(body), Err(ChannelError::BadToken));
    }

    #[test]
    fn garbage_is_malformed() {
        assert_eq!(parse_updates("not json at all"), Err(ChannelError::Malformed));
    }

    #[test]
    fn reads_the_new_message_id_back() {
        let body = r#"{"ok":true,"result":{"message_id":99,"chat":{"id":777}}}"#;
        assert_eq!(parse_send_result(body), Ok(99));
    }

    /// getMe 用来验证令牌，同时拿到 bot 用户名——界面上要显示
    /// 「在 Telegram 里搜 @your_bot」，没有这个名字那句话就没法写。
    #[test]
    fn get_me_returns_the_bot_username() {
        let body = r#"{"ok":true,"result":{"id":1,"is_bot":true,"username":"my_dct_bot"}}"#;
        assert_eq!(parse_get_me(body).unwrap(), "my_dct_bot");
    }

    #[test]
    fn get_me_with_a_bad_token_says_bad_token() {
        let body = r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#;
        assert_eq!(parse_get_me(body), Err(ChannelError::BadToken));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib channel::telegram -- --test-threads=1`
Expected: 编译失败，`cannot find function parse_updates`

- [ ] **Step 3: 写最小实现**

```rust
//! Telegram 适配器。
//!
//! 它被排在第一个渠道，全部理由是 `getUpdates` 长轮询让 NAT 后面的笔记本
//! 不需要服务器、不需要公网域名、不需要隧道。**别把这条优势改掉。**

use super::{Channel, ChannelError, Incoming, MsgId};
use std::time::Duration;

const API: &str = "https://api.telegram.org";

/// 传输层的形状：(url, body) -> 响应正文。与 `verify.rs::send_probe` 同一个
/// 路子——判定逻辑可以在不打网络的前提下被完整测试。
pub type Send = dyn Fn(&str, &str) -> Result<String, String> + Send + Sync;

/// 从 `ok:false` 的回包里判错误类型。401/403 是令牌的问题，其余当网络问题。
fn error_from(v: &serde_json::Value) -> ChannelError {
    match v.get("error_code").and_then(|c| c.as_i64()) {
        Some(401) | Some(403) => ChannelError::BadToken,
        _ => ChannelError::Unreachable,
    }
}

pub fn parse_updates(body: &str) -> Result<Vec<Incoming>, ChannelError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| ChannelError::Malformed)?;
    if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        return Err(error_from(&v));
    }
    let items = v
        .get("result")
        .and_then(|r| r.as_array())
        .ok_or(ChannelError::Malformed)?;

    let mut out = Vec::new();
    for it in items {
        let Some(m) = it.get("message") else { continue };
        // 没有 text 的更新（图片、贴纸、有人进群）跳过。**不是错误**——
        // 当成错误会让一张图片害得整轮轮询失败。
        let Some(text) = m.get("text").and_then(|t| t.as_str()) else {
            continue;
        };
        let Some(chat_id) = m.get("chat").and_then(|c| c.get("id")).and_then(|i| i.as_i64())
        else {
            continue;
        };
        out.push(Incoming {
            text: text.to_string(),
            reply_to: m
                .get("reply_to_message")
                .and_then(|r| r.get("message_id"))
                .and_then(|i| i.as_i64()),
            chat_id,
        });
    }
    Ok(out)
}

pub fn parse_send_result(body: &str) -> Result<MsgId, ChannelError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| ChannelError::Malformed)?;
    if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        return Err(error_from(&v));
    }
    v.get("result")
        .and_then(|r| r.get("message_id"))
        .and_then(|i| i.as_i64())
        .ok_or(ChannelError::Malformed)
}

/// 验证令牌，顺便拿 bot 用户名——界面要显示「在 Telegram 里搜 @your_bot」。
pub fn parse_get_me(body: &str) -> Result<String, ChannelError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| ChannelError::Malformed)?;
    if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        return Err(error_from(&v));
    }
    v.get("result")
        .and_then(|r| r.get("username"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string())
        .ok_or(ChannelError::Malformed)
}

pub struct Telegram {
    token: String,
    /// 长轮询的游标。Telegram 只在你确认过之后才丢弃旧更新，
    /// 不带它会把同一条消息反复取回来——那意味着同一句话被敲进 agent 好几遍。
    offset: std::sync::Mutex<i64>,
    send: Box<Send>,
}

impl Telegram {
    pub fn with_transport(token: &str, send: Box<Send>) -> Telegram {
        Telegram {
            token: token.to_string(),
            offset: std::sync::Mutex::new(0),
            send,
        }
    }

    fn url(&self, method: &str) -> String {
        format!("{API}/bot{}/{method}", self.token)
    }
}
```

`Channel` 的实现与真实传输（`ureq`，超时 = 长轮询秒数 + 5 秒余量）在 Step 5 补。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib channel::telegram -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 补 `Channel` 实现与真实传输**

`send()` 打 `sendMessage`，`poll()` 打 `getUpdates?offset={o}&timeout={s}`，成功后把 `offset` 更新为 `max(update_id) + 1`。真实传输用 `ureq`，超时取长轮询秒数 + 5 秒。

**用一个假传输写一个测试盖住 offset 递进**：连续两次 `poll`，第二次的 URL 必须带上 `offset=<上次最大 update_id + 1>`。这一条不测就会有「同一句话被敲进 agent 好几遍」。

- [ ] **Step 6: 变异测试**

把 `error_from` 里的 `Some(401) | Some(403)` 改成只有 `Some(401)`，`get_me_with_a_bad_token_says_bad_token` 该继续过（它用的是 401）—— **说明 403 没被测到，补一条 403 的测试**。把 offset 递进的 `+ 1` 去掉，Step 5 的测试必须失败。

- [ ] **Step 7: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/channel/telegram.rs
git commit -m "feat: talk to telegram without touching the network in tests

Parsing is split from transport the way verify.rs already does it, so a
revoked token, a timeout and a sticker someone sent the bot are all
covered without a socket.

The poll cursor matters more than it looks: without it telegram hands back
the same update forever, which would mean typing one sentence into an agent
over and over."
```

---

## Task 3: 设置页从「选语言」改成「选设置项」

**这是既有功能的重构，独立成一个任务，不掺任何手机通知逻辑。**

**Files:**
- Modify: `src/ui/view.rs`（`View::Settings`）
- Modify: `src/ui/settings_view.rs`
- Modify: `src/i18n.rs`
- Test: `src/ui/settings_view.rs` 内

**Interfaces:**
- Consumes: 无
- Produces: `SettingsItem { Language, Phone }`、`View::Settings { state: ListState }`（下标改为映射 `SettingsItem::all()`）

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 改结构之前，下标直接映射 Lang::all()。改完之后映射设置项。
    /// **这条是回归测试**：语言仍然切得动比手机通知能用更重要。
    #[test]
    fn the_first_item_is_language() {
        assert_eq!(SettingsItem::all()[0], SettingsItem::Language);
    }

    #[test]
    fn phone_is_a_settings_item_too() {
        assert!(SettingsItem::all().contains(&SettingsItem::Phone));
    }

    /// 下标越界不能 panic——`ListState` 的选中项在列表变短时会留在旧位置。
    #[test]
    fn an_out_of_range_index_selects_nothing() {
        assert_eq!(SettingsItem::at(99), None);
        assert_eq!(SettingsItem::at(0), Some(SettingsItem::Language));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib ui::settings_view -- --test-threads=1`
Expected: `cannot find type SettingsItem`

- [ ] **Step 3: 实现**

```rust
/// 设置页的条目。**加进第二项之前这一页是纯语言列表**，`ListState` 的下标
/// 直接映射 `Lang::all()`；现在映射这个枚举，选中语言那一项才进语言列表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsItem {
    Language,
    Phone,
}

impl SettingsItem {
    pub(crate) fn all() -> &'static [SettingsItem] {
        &[SettingsItem::Language, SettingsItem::Phone]
    }

    /// 越界返回 `None` 而不是兜底成第一项：`ListState` 的选中项可能停在
    /// 一个已经不存在的位置，那时候什么都不做，比默默把用户带进语言页好。
    pub(crate) fn at(i: usize) -> Option<SettingsItem> {
        SettingsItem::all().get(i).copied()
    }
}
```

`handle_key` 的 `Enter` 分支改成：按选中项分派 —— `Language` 进语言列表（把今天的语言选择逻辑原样搬进去），`Phone` 进 `View::Phone`（Task 4 建）。**方向键的 `move_sel_n` 长度参数从 `Lang::all().len()` 改成 `SettingsItem::all().len()`。**

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib ui:: -- --test-threads=1`
Expected: PASS，且既有的语言相关测试全部仍然通过

- [ ] **Step 5: 手动验证语言仍然切得动**

```bash
cargo run --release
```
进看板 → `l` → 选「界面语言」→ 选英文 → 界面变英文 → 退出重进，**仍然是英文**（`save_lang` 写盘生效）。

- [ ] **Step 6: 变异测试**

把 `at()` 的 `.get(i)` 换成 `all()[i.min(1)]`（越界兜底成最后一项），`an_out_of_range_index_selects_nothing` 必须失败。把 `move_sel_n` 的长度改回 `Lang::all().len()`，方向键会走不到第二项 —— **如果没有测试失败，补一条针对方向键能走到 `Phone` 的测试。**

- [ ] **Step 7: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/ui/view.rs src/ui/settings_view.rs src/i18n.rs
git commit -m "refactor: settings is a list of settings, not a list of languages

The list index mapped straight onto Lang::all(), which works right up until
there is a second thing to configure. Language moves one level down and the
page becomes what its name always claimed.

Nothing about the language behaviour changes; the regression test says so."
```

---

## Task 4: 协议、令牌存储、手机通知页

**Files:**
- Modify: `src/proto.rs`、`src/daemon.rs`、`src/secrets.rs`、`src/ui/view.rs`、`src/i18n.rs`
- Create: `src/ui/phone.rs`
- Test: `src/ui/phone.rs` 内 + `src/proto.rs` 内

**Interfaces:**
- Consumes: Task 3 的 `SettingsItem::Phone`
- Produces: `Request::PhoneStatus` / `PhoneSetToken { token }` / `PhoneUnpair` / `PhoneDisable`、`Response::Phone(PhoneStatus)`、`PhoneStatus { state: PhoneState, bot: Option<String>, owner: Option<String> }`、`PhoneState { Off, WaitingForPairing, Paired, Broken(String) }`、`View::Phone { status: PhoneStatus }`、`secrets::PHONE_TOKEN_KEY`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    /// 这一页存在的全部理由是那一行状态。四种取值，**每一种都要带下一步**——
    /// 一个不告诉用户下一步该干什么的错误，按房规就是没写完。
    #[test]
    fn every_state_tells_the_user_what_to_do_next() {
        for st in [
            PhoneState::Off,
            PhoneState::WaitingForPairing,
            PhoneState::Paired,
            PhoneState::Broken("token revoked".into()),
        ] {
            let s = status_line(
                &PhoneStatus { state: st.clone(), bot: Some("my_bot".into()), owner: None },
                Lang::Zh,
            );
            assert!(!s.is_empty(), "{st:?} 没有状态文案");
        }
        // 「已连上」是唯一不需要下一步的：它就是终点。其余三种都必须给出路。
        for st in [
            PhoneState::Off,
            PhoneState::WaitingForPairing,
            PhoneState::Broken("token revoked".into()),
        ] {
            let s = next_step(
                &PhoneStatus { state: st.clone(), bot: Some("my_bot".into()), owner: None },
                Lang::Zh,
            );
            assert!(s.is_some(), "{st:?} 没有给出下一步");
        }
        assert!(next_step(
            &PhoneStatus { state: PhoneState::Paired, bot: Some("my_bot".into()), owner: Some("lei".into()) },
            Lang::Zh
        ).is_none());
    }

    /// 等配对时必须把 bot 名字说出来，否则「去给它发条消息」是句没法执行的话。
    #[test]
    fn waiting_names_the_bot() {
        let s = status_line(
            &PhoneStatus { state: PhoneState::WaitingForPairing, bot: Some("my_dct_bot".into()), owner: None },
            Lang::Zh,
        );
        assert!(s.contains("my_dct_bot"), "等配对却没说是哪个 bot：{s}");
    }

    /// 令牌是密钥。**任何一处状态文案都不许把它带出来。**
    #[test]
    fn the_token_never_appears_in_any_status_text() {
        let st = PhoneStatus {
            state: PhoneState::Broken("123456:AAH-SECRET".into()),
            bot: None,
            owner: None,
        };
        let s = format!("{}{}", status_line(&st, Lang::Zh), next_step(&st, Lang::Zh).unwrap_or_default());
        assert!(!s.contains("AAH-SECRET"), "令牌漏进了界面文案：{s}");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib ui::phone -- --test-threads=1`
Expected: `cannot find type PhoneStatus`

- [ ] **Step 3: 实现**

`proto.rs` 加类型（`PhoneState::Broken` 里装的是**已经成文的人话**，不是原始错误）：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhoneState {
    /// 还没填令牌
    Off,
    /// 填了、验过了，在等用户给 bot 发第一条消息
    WaitingForPairing,
    Paired,
    /// 连不上。**装的是人话**，不是原始错误文本——守护进程是唯一决定
    /// 用户看到什么文字的地方（`proto.rs` 顶上那条已有的约定）。
    Broken(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneStatus {
    pub state: PhoneState,
    /// bot 用户名，`getMe` 拿的。等配对那句话要用它。
    pub bot: Option<String>,
    /// 配上的主人，显示用。
    pub owner: Option<String>,
}
```

`secrets.rs` 加保留名字：

```rust
/// 手机通知的令牌存在密钥仓里，用一个 profile 不可能占用的名字。
///
/// **它不会出现在密钥页（`c`）里**，因为那一页遍历的是 profiles 再查
/// `has_secret`（见 `ui/pick.rs`），不是遍历这个文件的键。
/// 将来谁把密钥页改成遍历 `secrets.toml`，这个名字就会作为一个不存在的
/// agent 冒出来——改那里的人请回来看这一句。
pub const PHONE_TOKEN_KEY: &str = "__phone__";
```

`ui/phone.rs` 写 `status_line()` / `next_step()` 两个**纯函数** + `draw()` + `handle_key()`（`Enter` 填令牌、`r` 重新配对、`x` 关掉、`Esc` 回设置页）。

`Request::PhoneSetToken` 的处理走 `getMe` 验证，复用 `EnterSecret` 的 `SecretPhase::Verifying` 反馈。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把 `next_step` 里 `Paired` 那条分支改成也返回 `Some(...)`，第一个测试的最后一句断言必须失败。把 `status_line` 里 `WaitingForPairing` 分支的 bot 名字插值去掉，`waiting_names_the_bot` 必须失败。

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/proto.rs src/daemon.rs src/secrets.rs src/ui/phone.rs src/ui/view.rs src/i18n.rs
git commit -m "feat: a page for the phone, because pairing is something you watch

Filling in a token is the easy half. The other half is that pairing is
asynchronous -- the daemon sits polling until you message the bot -- and
without somewhere to show that, you type a token and stare at a page that
does nothing.

Four states, each carrying its own next step, and a test that says the token
never reaches any of them."
```

---

## Task 5: bridge 骨架 —— 长轮询、配对、只认一个 chat id

**Files:**
- Create: `src/bridge.rs`
- Modify: `src/daemon.rs`（启动线程）、`src/lib.rs`
- Test: `src/bridge.rs` 内

**Interfaces:**
- Consumes: Task 1 的 `Channel`/`Incoming`、Task 4 的 `PhoneState`
- Produces: `Bridge::new(ch: Arc<dyn Channel>)`、`Bridge::accept(&self, msg: &Incoming) -> Accepted`、`Accepted { Paired(i64), FromOwner, Rejected }`

- [ ] **Step 1: 写失败测试**

**这是整个功能唯一真会伤到用户的地方，测试写在最前面。**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn msg(chat: i64, text: &str) -> Incoming {
        Incoming { text: text.into(), reply_to: None, chat_id: chat }
    }

    /// 第一个发消息的人成为主人。
    #[test]
    fn the_first_person_to_message_becomes_the_owner() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(111, "在吗")), Accepted::Paired(111));
        assert_eq!(b.accept(&msg(111, "先跑完")), Accepted::FromOwner);
    }

    /// **bot 用户名是公开可搜的，任何人都能给它发消息，而这个功能会把消息
    /// 敲进用户的终端。** 这条测试破了就等于任何人都能往用户机器上敲字。
    #[test]
    fn a_stranger_is_rejected_even_after_pairing() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(111, "在吗")), Accepted::Paired(111));
        assert_eq!(b.accept(&msg(222, "rm -rf /")), Accepted::Rejected);
        assert_eq!(b.accept(&msg(222, "/use 1")), Accepted::Rejected);
        // 主人还是主人，没被挤掉
        assert_eq!(b.accept(&msg(111, "继续")), Accepted::FromOwner);
    }

    /// 陌生人抢在主人之前发消息，就成了主人——这正是为什么配对必须是
    /// 用户填完令牌后的一次显式动作，而不是长期开着的门。
    /// 配对完成后 `accept` 再也不会返回 `Paired`。
    #[test]
    fn pairing_happens_exactly_once() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(111, "hi")), Accepted::Paired(111));
        assert_eq!(b.accept(&msg(333, "hi")), Accepted::Rejected);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge:: -- --test-threads=1`
Expected: `cannot find type Bridge`

- [ ] **Step 3: 实现**

```rust
//! 连接层：把守护进程里发生的事送到渠道上，把渠道上来的话敲进会话。
//!
//! **这是唯一有状态的地方**：谁是主人、哪条消息对应哪个会话、当前对着哪个
//! 会话。除此之外它什么都不存。
//!
//! **绝不 panic 到线程外面。** 手机通道死掉是遗憾，会话跟着死是灾难——
//! 同 `journal.rs` 那条「记不下来是记账的事，不该连累会话」。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accepted {
    /// 这条消息完成了配对，发信人成为主人。
    Paired(i64),
    FromOwner,
    /// 不是主人发的，**丢弃**。
    Rejected,
}

pub struct Bridge {
    /// 配对之后只认这一个。`None` = 还没配对。
    owner: Mutex<Option<i64>>,
    // …… 消息映射与当前会话见 Task 7
}

impl Bridge {
    pub fn accept(&self, msg: &Incoming) -> Accepted {
        let mut owner = recover(self.owner.lock());
        match *owner {
            None => {
                *owner = Some(msg.chat_id);
                Accepted::Paired(msg.chat_id)
            }
            Some(o) if o == msg.chat_id => Accepted::FromOwner,
            Some(_) => Accepted::Rejected,
        }
    }
}
```

再写轮询线程：`loop { ch.poll(25s) }`，`ChannelError::worth_retrying()` 为真就指数退避（上限 5 分钟），为假就停下并把 `PhoneState::Broken(人话)` 写进状态槽。**整个线程体包在 `catch_unwind` 里**，panic 只让手机通道停掉。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib bridge:: -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把 `Some(o) if o == msg.chat_id` 的 `==` 改成 `!=` —— `a_stranger_is_rejected_even_after_pairing` 必须失败。把 `None` 分支改成不写 `owner`（每次都返回 `Paired`）—— `pairing_happens_exactly_once` 必须失败。

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/bridge.rs src/daemon.rs src/lib.rs
git commit -m "feat: pair with exactly one person and ignore everyone else

A bot username is public and searchable, so anyone can message it -- and
this feature types what it receives into a terminal. The first message
after you enter the token claims ownership; every message from anyone else
is dropped, forever.

That is the one test in this feature that maps directly onto someone else
getting to type on your machine."
```

---

## Task 6: 出站 —— tick 投事件、三道门、防抖

**Files:**
- Modify: `src/session.rs`（`tick()`）
- Modify: `src/bridge.rs`（消费队列）
- Test: `src/session.rs` 内

**Interfaces:**
- Consumes: Task 1 的 `Event`/`EventKind`/`debounce`、Task 5 的 `Bridge`
- Produces: `Sessions::set_event_sink(mpsc::Sender<Event>)`、`should_notify(is_agent, first_input_empty, has_channel) -> bool`

- [ ] **Step 1: 写失败测试**

```rust
/// 三道门。第二道是关键：真实 profile（claude/codex/glm/kimi/deepseek/
/// qwen-api）**全都只声明 busy_pattern**，`classify()` 在 busy 串不在屏幕上
/// 时就判 Idle，而刚创建、还停在启动画面上的会话正是这样。没有这道门，
/// **每开一个会话手机就响一次**。
#[test]
fn a_brand_new_session_does_not_page_you() {
    // 是 agent、有渠道，但用户还没说过话
    assert!(!should_notify(true, true, true));
}

#[test]
fn a_plain_shell_never_pages_you() {
    assert!(!should_notify(false, false, true));
}

#[test]
fn no_channel_means_no_page() {
    assert!(!should_notify(true, false, false));
}

#[test]
fn an_agent_you_have_talked_to_pages_you() {
    assert!(should_notify(true, false, true));
}
```

再写一条走完整 tick 的集成测试：造一个假 profile（只有 `busy_pattern`），`create()` 之后立刻 `tick()`，**断言事件队列是空的**。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib session::tests::a_brand_new -- --test-threads=1`
Expected: `cannot find function should_notify`

- [ ] **Step 3: 实现**

`should_notify` 三个条件与；`tick()` 在三处投递事件：

1. 已有的 `was == Working && matches!(next, Idle | Asking)` 分支 → `EventKind::Stopped`
2. 已有的 `next == Failed && was != Failed` 分支 → `EventKind::Failed`
3. 已有的收尸分支（`journal.died(..., Vanished, ...)` 旁边）→ `EventKind::Vanished`

**投递用 `try_send` 语义，队列满了就丢，绝不阻塞 tick。** 防抖状态（每会话上次发送时刻）记在 `Session` 上。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把 `should_notify` 的 `!first_input_empty` 那一项去掉 —— `a_brand_new_session_does_not_page_you` 和那条 tick 集成测试**都**必须失败。把三个条件的 `&&` 改成 `||`，至少两条测试必须失败。

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/session.rs src/bridge.rs
git commit -m "feat: page the user when an agent stops, fails or dies

The transition that auto-naming already hangs on is the same one worth
sending to a phone, so there is no new detection here -- just a second
consumer and a queue.

The gate that matters is the one on first_input: every real profile declares
only busy_pattern, so a session still sitting on its splash screen reads as
'finished a round of work'. Without that gate your phone buzzes every time
you open a session.

tick never blocks on the queue. A full queue drops the event; a slow send
would freeze the daemon, and a frozen dct looks exactly like a dead agent."
```

---

## Task 7: 入站路由五条规则

**Files:**
- Modify: `src/bridge.rs`
- Test: `src/bridge.rs` 内

**Interfaces:**
- Consumes: Task 1 的 `MsgId`、Task 5 的 `Bridge`
- Produces: `RouteInput`、`Route { To(u32), Ask(Vec<u32>), Gone, NeedUse }`、`route(&RouteInput) -> Route`

- [ ] **Step 1: 写失败测试**

```rust
fn input<'a>(
    reply_to: Option<MsgId>,
    map: &'a HashMap<MsgId, u32>,
    used: Option<u32>,
    replied_since_use: bool,
    waiting: &'a [u32],
) -> RouteInput<'a> {
    RouteInput { reply_to, map, used, replied_since_use, waiting }
}

#[test]
fn a_reply_goes_where_it_replied() {
    let map = HashMap::from([(42, 7)]);
    assert_eq!(route(&input(Some(42), &map, Some(3), false, &[9])), Route::To(7));
}

/// **重启之后旧消息不能敲进任何地方。** 退化成「发给当前会话」正好是
/// 敲错地方的那条路径。
#[test]
fn a_reply_to_a_message_we_no_longer_know_types_nothing() {
    let map = HashMap::new();
    assert_eq!(route(&input(Some(42), &map, Some(3), false, &[9])), Route::Gone);
}

/// `/use` 压过「唯一在等」：用户切过去就是想跟那个会话说话，
/// 此刻另一个会话恰好在等，不能把他的话抢走。
#[test]
fn an_explicit_use_beats_a_waiting_session() {
    let map = HashMap::new();
    assert_eq!(route(&input(None, &map, Some(3), false, &[9])), Route::To(3));
}

/// 但用户一旦长按回复过某条推送，注意力已经转走，`/use` 的指定作废——
/// 否则一次 `/use` 会永久劫持所有不带回复的消息。
#[test]
fn use_expires_once_you_have_replied_to_a_push() {
    let map = HashMap::new();
    assert_eq!(route(&input(None, &map, Some(3), true, &[9])), Route::To(9));
}

#[test]
fn the_only_one_waiting_gets_it() {
    let map = HashMap::new();
    assert_eq!(route(&input(None, &map, None, false, &[9])), Route::To(9));
}

/// 好几个在等就不猜。敲错 agent 的代价比多问一句大得多。
#[test]
fn several_waiting_means_ask_not_guess() {
    let map = HashMap::new();
    assert_eq!(route(&input(None, &map, None, false, &[9, 10])), Route::Ask(vec![9, 10]));
}

#[test]
fn nothing_waiting_and_no_use_asks_for_ls() {
    let map = HashMap::new();
    assert_eq!(route(&input(None, &map, None, false, &[])), Route::NeedUse);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge::tests -- --test-threads=1`
Expected: `cannot find function route`

- [ ] **Step 3: 实现**

**顺序就是 spec 里那五条，不要重排：**

```rust
pub fn route(i: &RouteInput) -> Route {
    // 1. 带回复的：直接定位，永远不反问
    if let Some(m) = i.reply_to {
        return match i.map.get(&m) {
            Some(&s) => Route::To(s),
            // 守护进程重启过，映射没了。**绝不退化成「发给当前会话」**
            None => Route::Gone,
        };
    }
    // 2. 显式 /use 过、且那之后还没回复过任何推送
    if let (Some(u), false) = (i.used, i.replied_since_use) {
        return Route::To(u);
    }
    // 3. 只有一个在等
    if i.waiting.len() == 1 {
        return Route::To(i.waiting[0]);
    }
    // 4. 好几个在等：不猜（模型在这一条介入，见 Task 10）
    if i.waiting.len() > 1 {
        return Route::Ask(i.waiting.to_vec());
    }
    // 5. 没候选也没 /use 过
    Route::NeedUse
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib bridge:: -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把规则 2 和规则 3 调换顺序 —— `an_explicit_use_beats_a_waiting_session` 必须失败。把 `Route::Gone` 改成 `Route::To(i.used.unwrap_or(0))` —— `a_reply_to_a_message_we_no_longer_know_types_nothing` 必须失败。把 `replied_since_use` 的 `false` 改成 `true` —— `use_expires_once_you_have_replied_to_a_push` 必须失败。

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/bridge.rs
git commit -m "feat: decide which agent a reply belongs to, or refuse to

Five rules in a fixed order. Two of them exist because of failure modes
rather than features: an explicit /use has to outrank the one session that
happens to be waiting, or your message gets stolen by it; and a reply to a
message from before a daemon restart types nothing at all, because falling
back to the current session is precisely the path that types into the wrong
terminal."
```

---

## Task 8: 入站落地 —— 敲进 PTY、回执、journal

**Files:**
- Modify: `src/bridge.rs`、`src/journal.rs`
- Test: `src/bridge.rs` 内

**Interfaces:**
- Consumes: Task 7 的 `Route`、`session::Sessions::send_input(id, text) -> Result<()>`
- Produces: `Bridge::deliver(&self, route: Route, text: &str) -> Delivered`、`Delivered { Typed(u32), AskedWhich(Vec<u32>), SaidGone, SaidNeedUse, Failed(String) }`

- [ ] **Step 1: 写失败测试**

用一个假的写入器（记录被写了什么、写给谁），不碰真 PTY。

```rust
/// 回执不是锦上添花：用户在外面看不见终端，没有回执他不知道这句话
/// 到底进去没有。
#[test]
fn typing_it_in_sends_a_receipt_naming_the_session() {
    let (b, spy) = Bridge::for_test_with_writer();
    let d = b.deliver(Route::To(7), "先跑完");
    assert_eq!(d, Delivered::Typed(7));
    assert_eq!(spy.written(), vec![(7, "先跑完".to_string())]);
    assert!(spy.last_reply().contains("修登录白屏"), "回执里没说敲给了谁");
}

/// `Gone` 什么都不敲。这是重启之后那条安全路径的落地，
/// 光有 `route()` 返回 `Gone` 不够，得确认真的没写出去。
#[test]
fn a_gone_route_writes_nothing_at_all() {
    let (b, spy) = Bridge::for_test_with_writer();
    assert_eq!(b.deliver(Route::Gone, "先跑完"), Delivered::SaidGone);
    assert!(spy.written().is_empty(), "旧消息被敲进了会话");
}

#[test]
fn asking_which_writes_nothing_either() {
    let (b, spy) = Bridge::for_test_with_writer();
    assert_eq!(b.deliver(Route::Ask(vec![9, 10]), "先跑完"), Delivered::AskedWhich(vec![9, 10]));
    assert!(spy.written().is_empty());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge::tests::typing -- --test-threads=1`
Expected: `cannot find method deliver`

- [ ] **Step 3: 实现**

`deliver` 按 `Route` 分派：`To(id)` 调 `send_input` 再发回执；`Ask` 发候选列表；`Gone` 发「这条消息对应的会话已经不在了」；`NeedUse` 发「先 `/ls` 看看有哪些会话」。**全部记 journal。**

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib bridge:: -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把 `Gone` 分支改成也调 `send_input` —— `a_gone_route_writes_nothing_at_all` 必须失败。把回执里的会话名换成会话号 —— `typing_it_in_sends_a_receipt_naming_the_session` 必须失败。

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/bridge.rs src/journal.rs
git commit -m "feat: type the reply in, then say where it went

You cannot see the terminal from a train, so a receipt naming the session is
the only evidence the sentence landed. Two of the three routes deliberately
write nothing, and the tests assert the absence rather than the message --
that is the half that can go wrong quietly."
```

---

## Task 9: 智能（出站）—— 合并与编号选项

**Files:**
- Modify: `src/bridge.rs`、`src/llm/mod.rs`
- Test: `src/bridge.rs` 内

**Interfaces:**
- Consumes: Task 1 的 `Event`、`llm::complete_with_timeout`、`llm::Backend`
- Produces: `merge(&[Event], Lang) -> String`、`options_prompt(screen: &str) -> Prompt`、`parse_options(&str) -> Option<Vec<String>>`

- [ ] **Step 1: 写失败测试**

```rust
/// 合并不需要模型。断网八小时不该在恢复瞬间收到五百条。
#[test]
fn several_events_become_one_message() {
    let evs = vec![
        Event { session: 1, kind: EventKind::Stopped, name: "修登录白屏".into(), project: "web".into() },
        Event { session: 2, kind: EventKind::Failed, name: "对账".into(), project: "fin".into() },
    ];
    let m = merge(&evs, Lang::Zh);
    assert!(m.contains("修登录白屏") && m.contains("对账"));
    // 一条消息，不是两条拼起来——两个会话名之间不该出现消息分隔
    assert_eq!(m.matches("\n\n\n").count(), 0);
}

#[test]
fn a_single_event_is_not_dressed_up_as_a_list() {
    let evs = vec![Event { session: 1, kind: EventKind::Stopped, name: "修登录白屏".into(), project: "web".into() }];
    let m = merge(&evs, Lang::Zh);
    assert!(!m.contains("1."), "只有一件事却排了个编号列表：{m}");
}

/// 模型答得不成形就当没有选项——**绝不猜**，退回只有元数据的消息。
#[test]
fn unparseable_options_mean_no_options() {
    assert_eq!(parse_options("我觉得他大概想问你要不要继续吧"), None);
    assert_eq!(parse_options(""), None);
}

#[test]
fn options_come_back_in_order() {
    let got = parse_options("1. 先跑完\n2. 现在改").unwrap();
    assert_eq!(got, vec!["先跑完".to_string(), "现在改".to_string()]);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge::tests::several_events -- --test-threads=1`
Expected: `cannot find function merge`

- [ ] **Step 3: 实现**

`merge` 纯函数。`options_prompt` 走 `request_explanation` 已建立的范式：**兜底（只有元数据的消息）同步先就位**，再起线程问模型，硬超时 15 秒，超时/畸形就发兜底。

**消息里绝不出现路径、diff、代码块** —— prompt 里明确要求，且 `parse_options` 把含 `/`、`\`` 的候选项丢弃。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把 `parse_options` 的失败分支改成返回 `Some(vec![原文])` —— `unparseable_options_mean_no_options` 必须失败。把 `merge` 单条那一支去掉（永远排编号列表）—— `a_single_event_is_not_dressed_up_as_a_list` 必须失败。

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/bridge.rs src/llm/mod.rs
git commit -m "feat: one message for several agents, numbered options for one

Merging needs no model at all, which is why it is the one piece of this that
still works with [llm] unset.

Options do need one, so the fallback is written synchronously before the
thread starts: a slow model makes the message plainer, never later. An answer
that does not parse yields no options rather than a guessed list."
```

---

## Task 10: 智能（入站）—— 听懂回复、猜路由

**Files:**
- Modify: `src/bridge.rs`
- Test: `src/bridge.rs` 内

**Interfaces:**
- Consumes: Task 7 的 `route`/`Route`、`llm::complete_with_timeout`
- Produces: `map_answer(user: &str, options: Option<&[String]>, backend) -> String`、`narrow(candidates: &[u32], text: &str, backend) -> Option<u32>`

- [ ] **Step 1: 写失败测试**

**红线在这里，测试就是红线本身。**

```rust
/// agent 要的是自由文本时模型完全不介入。模型一旦开始润色，敲进 agent 的
/// 就不再是用户说的话，而他在手机上看不见这件事。
#[test]
fn free_text_is_typed_verbatim_and_never_reaches_the_model() {
    let spy = SpyBackend::new(); // 被调用就记一笔
    let out = map_answer("那个啥 你先把测试跑一下然后再说", None, &spy);
    assert_eq!(out, "那个啥 你先把测试跑一下然后再说");
    assert_eq!(spy.calls(), 0, "自由文本却调了模型");
}

#[test]
fn a_spoken_ordinal_becomes_the_option_the_agent_wants() {
    let b = FakeBackend::answering("2");
    let opts = vec!["先跑完".to_string(), "现在改".to_string()];
    assert_eq!(map_answer("就第二个吧", Some(&opts), &b), "2");
}

/// 映射不确定就原样发。这是红线的另一半。
#[test]
fn an_unmappable_answer_is_sent_as_typed() {
    let b = FakeBackend::answering("我不确定");
    let opts = vec!["先跑完".to_string(), "现在改".to_string()];
    assert_eq!(map_answer("等等我再想想", Some(&opts), &b), "等等我再想想");
}

#[test]
fn a_model_timeout_sends_what_the_user_typed() {
    let b = FakeBackend::timing_out();
    let opts = vec!["先跑完".to_string()];
    assert_eq!(map_answer("就第一个", Some(&opts), &b), "就第一个");
}

/// 猜路由不确定就还是反问。**永远不因为「模型有把握」跳过那一问**——
/// 敲错 agent 的代价比多问一句大得多。
#[test]
fn an_uncertain_narrow_still_asks() {
    let b = FakeBackend::answering("说不好");
    assert_eq!(narrow(&[9, 10], "先跑完", &b), None);
}

/// 模型答了一个不在候选里的会话号，一律不采信。
#[test]
fn a_narrow_outside_the_candidates_is_refused() {
    let b = FakeBackend::answering("77");
    assert_eq!(narrow(&[9, 10], "先跑完", &b), None);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge::tests::free_text -- --test-threads=1`
Expected: `cannot find function map_answer`

- [ ] **Step 3: 实现**

```rust
/// 把用户的话变成 agent 要的形式。**只转格式，不造内容。**
pub fn map_answer(user: &str, options: Option<&[String]>, b: &dyn Backend) -> String {
    // agent 要的是自由文本：模型完全不介入。**这个 early return 就是红线。**
    let Some(opts) = options else {
        return user.to_string();
    };
    if opts.is_empty() {
        return user.to_string();
    }
    match complete_with_timeout(/* … 8 秒硬超时 … */) {
        // 答案必须是候选里的序号，别的一律不采信
        Ok(a) => match a.trim().parse::<usize>() {
            Ok(n) if n >= 1 && n <= opts.len() => n.to_string(),
            _ => user.to_string(),
        },
        Err(_) => user.to_string(),
    }
}
```

`narrow` 只在 `Route::Ask` 那一条被调用（Task 7 规则 4），答案不在候选里就返回 `None`，调用方照常反问。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

**这一步在这个任务里最重要：**
- 去掉 `options` 为 `None` 时的 early return（让自由文本也过模型）—— `free_text_is_typed_verbatim_and_never_reaches_the_model` 必须失败
- 把序号范围 `n >= 1 && n <= opts.len()` 改成 `n <= opts.len()`（放进 0）—— **如果没有测试失败，补一条 `answering("0")` 的测试**
- 把 `narrow` 的越界检查去掉 —— `a_narrow_outside_the_candidates_is_refused` 必须失败

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/bridge.rs
git commit -m "feat: understand 'the second one' without rewriting what you said

The model only ever converts a spoken ordinal into the token the agent is
waiting for. When the agent wants free text the model is not called at all --
that early return is the whole guarantee, because a polished version of your
sentence is something you cannot see from a phone.

Every failure path sends what you typed: no options, no mapping, a timeout,
a number outside the list."
```

---

## Task 11: 错误处理收尾与端到端实测

**Files:**
- Modify: `src/bridge.rs`、`src/ui/phone.rs`
- Test: `src/bridge.rs` 内

**Interfaces:**
- Consumes: 前面全部
- Produces: `backoff(attempt: u32) -> Duration`、`QUEUE_CAP`

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn backoff_grows_then_stops_growing() {
    assert!(backoff(0) < backoff(1));
    assert!(backoff(1) < backoff(2));
    // 上限 5 分钟：再久用户就会以为功能坏了
    assert_eq!(backoff(99), Duration::from_secs(300));
}

/// 队列满了丢最老的，**绝不阻塞 tick**。
#[test]
fn a_full_queue_drops_instead_of_blocking() {
    let b = Bridge::for_test();
    for i in 0..(QUEUE_CAP + 10) {
        b.enqueue(Event { session: i as u32, kind: EventKind::Stopped, name: "x".into(), project: "p".into() });
    }
    assert_eq!(b.queued(), QUEUE_CAP);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge::tests::backoff -- --test-threads=1`
Expected: `cannot find function backoff`

- [ ] **Step 3: 实现**

指数退避 + 5 分钟上限；`BadToken` 不退避，直接置 `PhoneState::Broken("令牌被撤销了，按 Enter 重填")`；队列有界。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 全绿

- [ ] **Step 5: 变异测试**

把上限 `min(300)` 去掉 —— `backoff_grows_then_stops_growing` 必须失败。把队列改成无界 —— 第二条必须失败。

- [ ] **Step 6: 端到端实测（需要真 bot token）**

**这一步不能跳，也不能用「单测全绿」代替。** 按 dct 的惯例，没跑过的一律记成没跑过。

1. 在 Telegram 找 @BotFather 建一个 bot，拿 token
2. `dct` → `l` → 手机通知 → 填 token → **断言页面显示「等你给 @xxx 发一条消息」并带上真实 bot 名**
3. 给 bot 发一句话 → **断言页面变成「已连上」**
4. 在某个项目里开一个 claude 会话，跟它说一句话，等它干完一轮 → **断言手机收到消息，且消息里没有路径、没有代码块**
5. 长按回复一句话 → **断言 agent 真的收到了，且手机上收到回执**
6. 直接发一句不带回复的话 → 断言按五条规则走
7. 发一个陌生账号给这个 bot 发消息 → **断言什么都没发生**（这条最重要）
8. 杀掉守护进程重启，长按回复步骤 5 那条旧消息 → **断言回「会话已经不在了」，且没有任何东西被敲进任何会话**

把每一条的真实结果写回 spec 的「未验证 / 风险」表。**跑不通的写成跑不通，不要写成待验证。**

- [ ] **Step 7: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add -A
git commit -m "feat: back off, cap the queue, and record what actually ran

A revoked token skips the backoff entirely -- retrying it forever would also
mean never telling the user to go re-enter it.

The end-to-end results go back into the spec's unverified table, including
the ones that failed. A green test suite is not evidence that the telegram
endpoints were ever called."
```

---

## Self-Review

**Spec 覆盖检查：**

| spec 小节 | 任务 |
|---|---|
| 架构：渠道住 daemon | Task 5 |
| 组件与边界 | Task 1, 2, 5 |
| 出站三事件 + 三道门 + 防抖 | Task 6 |
| 出站内容分两档（隐私边界） | Task 9 |
| 消息格式（无路径无 diff 无代码块） | Task 9 |
| 不碰 `Asking` | Task 6（tick 里不新增 `Asking` 的写入；本计划没有任何任务设置它） |
| 路由五条规则 | Task 7 |
| 主动发 `/ls` `/use` | Task 7（规则 2、5）+ Task 8（`NeedUse` 的回话） |
| 安全：只认一个 chat id | Task 5 |
| 回执 | Task 8 |
| journal | Task 8 |
| 智能四项 | Task 9（合并、编号选项）、Task 10（听懂回复、猜路由） |
| 红线：只转格式不造内容 | Task 10 |
| 错误处理表 | Task 11 |
| 重启后旧消息 | Task 7（`Gone`）+ Task 8（不写出去）+ Task 11 步骤 6.8（实测） |
| 界面：设置页改结构 | Task 3 |
| 界面：手机通知页四状态 | Task 4 |
| 令牌存 `SecretStore` 保留名 | Task 4 |
| 测试三条回归套 | Task 5（陌生人）、Task 6（新建会话）、Task 7+8（重启旧消息） |

**无遗漏。**

**类型一致性：** `MsgId`（Task 1）→ Task 2 解析、Task 7 映射键，一致。`Event`/`EventKind`（Task 1）→ Task 6 投递、Task 9 合并，一致。`Route`（Task 7）→ Task 8 `deliver`、Task 10 `narrow` 只作用于 `Ask`，一致。`PhoneState`（Task 4）→ Task 5 写 `Broken`、Task 11 写 `Broken`，一致。

**占位符扫描：** 无 TBD/TODO；每个代码步骤都有可运行的代码或明确的行为约定；Task 2 Step 5 与 Task 4 Step 3 描述的是补全动作，但都给了具体的判定条件和必须存在的测试。
