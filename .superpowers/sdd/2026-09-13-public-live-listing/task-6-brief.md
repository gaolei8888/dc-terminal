### Task 6: `dct` 协议 19 —— 类型、错误码、文案

**Files:**
- Modify: `src/proto.rs`
- Modify: `src/secrets.rs`
- Modify: `src/i18n.rs`
- Modify: 所有构造 `LiveInfo { ... }` 的地方（`grep -rn "LiveInfo {" src` 列全；当前在 `src/live.rs`、`src/ui/live.rs`、`src/ui/mod.rs`、`src/ui/app.rs`、`src/proto.rs` 测试里）

**Interfaces:**
- Consumes: 无（纯类型）。
- Produces:
  - `pub enum LivePublic { Private, Pending { title: String }, Listed { title: String }, Failed { title: String, reason: LiveFailure } }`（`Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default`，默认 `Private`）
  - `LiveInfo.public: LivePublic`（`#[serde(default)]`）
  - `Request::LivePublish { title: String }`、`Request::LiveUnpublish`、`Request::LivePublishGrant`
  - `pub struct LiveGrantToken(pub String)`（`Serialize/Deserialize` 透明、`Debug` 打码）与 `Response::LiveGrant(LiveGrantToken)`
  - `ErrorCode::LivePublishKeyMissing`
  - `pub const LIVE_PUBLISH_KEY: &str = "__live_publish__";`（`src/secrets.rs`）
  - `PROTOCOL_VERSION = 19`
  - i18n：`msg::live_on_air_public(lang, title: &str, routes: usize, viewers: u32) -> String`、`msg::live_publish_failed(lang, why: &LiveFailure) -> String`；`Key::LivePublishToggle`（「公开/取消公开」）、`Key::LiveChangeKey`（「换公开密钥」）、`Key::LiveTitlePrompt`（「公开标题：」）、`Key::LiveKeyPrompt`（「公开直播密钥：」）、`Key::LivePublicPending`（「正在公开…」）、`Key::LiveKeySaved`（「公开直播密钥已保存」）、`Key::LiveUnpublished`（「已取消公开」）、`Key::LiveTitleEmpty`（「标题不能为空」）

- [ ] **Step 1: 写失败的测试**

`src/proto.rs` 测试模块：
- 把 `the_request_shape_is_pinned_to_the_protocol_version` 的 `all` 列表末尾加 `Request::LivePublish { title: "t".into() }, Request::LiveUnpublish, Request::LivePublishGrant`，期望串末尾对应加 `,{"LivePublish":{"title":"t"}},"LiveUnpublish","LivePublishGrant"`，四处期望版本号 `18` 改 `19`（含 `the_no_relay_error_is_a_bare_string_on_the_wire`）。
- 追加：

```rust
    /// `LiveInfo` 的公开状态上线形状，以及旧 JSON（没有这个字段）读成 `Private`。
    #[test]
    fn the_live_public_shape_is_pinned() {
        let shape = |p: &LivePublic| serde_json::to_string(p).unwrap();
        assert_eq!(
            (PROTOCOL_VERSION, shape(&LivePublic::Private), shape(&LivePublic::Listed { title: "课".into() })),
            (19, r#""Private""#.to_string(), r#"{"Listed":{"title":"课"}}"#.to_string())
        );
        assert_eq!(
            shape(&LivePublic::Failed { title: "课".into(), reason: LiveFailure::Refused(401) }),
            r#"{"Failed":{"title":"课","reason":{"Refused":401}}}"#
        );
    }

    /// 凭证能当一次公开，不许在任何日志里原样出现。
    #[test]
    fn a_live_grant_is_redacted_in_debug() {
        let r = Response::LiveGrant(LiveGrantToken("deadbeef".repeat(8)));
        assert!(!format!("{r:?}").contains("deadbeef"));
        assert_eq!(serde_json::to_string(&r).unwrap(), format!(r#"{{"LiveGrant":"{}"}}"#, "deadbeef".repeat(8)));
    }
```

`src/i18n.rs`：`every_error_code_composes_in_both_languages` 的列表加 `LivePublishKeyMissing,`；追加：

```rust
    #[test]
    fn public_live_strings_compose_in_both_languages() {
        use crate::proto::LiveFailure;
        for l in Lang::all() {
            let on = msg::live_on_air_public(*l, "第3课", 2, 7);
            assert!(on.contains("第3课") && on.contains('7'), "{on}");
            for code in [401u16, 403, 413, 429, 500] {
                let s = msg::live_publish_failed(*l, &LiveFailure::Refused(code));
                assert!(!s.trim().is_empty());
                if *l == Lang::En {
                    assert!(!has_han(&s), "{s}");
                }
            }
            assert!(!msg::live_publish_failed(*l, &LiveFailure::Unreachable).is_empty());
        }
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib proto:: i18n::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`src/proto.rs`：
1. `PROTOCOL_VERSION` 改 19，文档注释追加一段：

```rust
/// 19 = 公开直播列表：多了 `Request::LivePublish`/`LiveUnpublish`/`LivePublishGrant`、
/// `Response::LiveGrant`、`ErrorCode::LivePublishKeyMissing`，`LiveInfo` 多了 `public`。
/// 新增 `Request` 变体那条规矩同 14。
```

2. `LiveFailure` 之后加：

```rust
/// 这场直播在公开列表上的状态。**以中转为准**：推帧线程每次保活时从
/// `GET /live/{id}/lanes` 的 `public` 字段读回来（包括管理台代为公开的情况）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LivePublic {
    #[default]
    Private,
    /// 本机请求了公开，中转还没答应。
    Pending { title: String },
    Listed { title: String },
    /// 本机请求的公开被拒了。原因是码，组句在界面（`msg::live_publish_failed`）。
    Failed { title: String, reason: LiveFailure },
}

/// 守护进程签发给管理台的公开凭证（见 `dct_link::live::publish_grant`）。
/// 手写 `Debug`：它能拿去公开这场直播，不许原样进日志。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveGrantToken(pub String);

impl std::fmt::Debug for LiveGrantToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveGrantToken(<redacted>)")
    }
}
```

3. `LiveInfo` 加字段（在 `readiness` 之后）：

```rust
    /// 公开列表上的状态，见 [`LivePublic`]。旧 JSON 没有这个字段时读成 `Private`。
    #[serde(default)]
    pub public: LivePublic,
```

`impl Debug for LiveInfo` 加 `.field("public", &self.public)`。
4. `Request` 在 `LiveStatus` 之后加：

```rust
    /// 公开这场直播。守护进程用自己存的发布密钥（`secrets::LIVE_PUBLISH_KEY`），
    /// 密钥**不在这条请求里**。标题超过 `MAX_PUBLIC_TITLE_CHARS` 由守护进程截断。
    LivePublish { title: String },
    LiveUnpublish,
    /// 给管理台一张只能切换公开状态的凭证。没在播回 `LiveStagingRejected(NotLive)`。
    LivePublishGrant,
```

`Request` 的手写 `Debug` 对应加三支（`LivePublish` 显示标题，另两支显示名字）。
5. `Response` 加 `LiveGrant(LiveGrantToken),`；`ErrorCode` 加：

```rust
    /// 要公开直播，但本机还没填公开直播密钥。
    LivePublishKeyMissing,
```

6. 所有 `LiveInfo { ... }` 字面量加 `public: LivePublic::Private,`（测试里需要别的值的按需写）。

`src/secrets.rs` 在 `GATE_TOKEN_KEY` 之后加：

```rust
/// 公开直播的发布密钥（运营方在中转上用 `dct-srv key add` 签发）。同样用一个
/// profile 不可能占用的名字；它只在守护进程里用，不经过任何发给界面的响应。
pub const LIVE_PUBLISH_KEY: &str = "__live_publish__";
```

`src/i18n.rs`：
- `msg::error` 的 `match` 加：

```rust
            LivePublishKeyMissing => t!(
                lang,
                en: "no public live key yet — press K in the live panel to enter the one your operator gave you".to_string(),
                zh: "还没填公开直播密钥：在直播面板里按 K，填上管理员发给你的那把".to_string(),
            ),
```

- `msg` 里 `live_on_air` 之后加：

```rust
    pub fn live_on_air_public(lang: Lang, title: &str, routes: usize, viewers: u32) -> String {
        t!(
            lang,
            en: format!("\u{25cf} LIVE PUBLICLY \u{b7} {title} \u{b7} {routes} lane(s) \u{b7} {viewers} watching"),
            zh: format!("\u{25cf} 正在公开直播 \u{b7} {title} \u{b7} {routes} 路 \u{b7} {viewers} 人在看"),
        )
    }

    /// 公开失败的整句话。401/403/413/429 各有一句人话，别的码照实报出来。
    pub fn live_publish_failed(lang: Lang, why: &crate::proto::LiveFailure) -> String {
        use crate::proto::LiveFailure;
        let reason = match why {
            LiveFailure::Unreachable => t!(lang,
                en: "cannot reach the relay, will keep retrying".to_string(),
                zh: "连不上中转，稍后会自动重试".to_string()),
            LiveFailure::Refused(401) => t!(lang,
                en: "the public live key is wrong or has been revoked".to_string(),
                zh: "公开直播密钥不对，或者已被吊销".to_string()),
            LiveFailure::Refused(403) => t!(lang,
                en: "the server does not allow public lives, or this one was taken down".to_string(),
                zh: "服务器没有开启公开直播，或者这场直播已被下线".to_string()),
            LiveFailure::Refused(413) => t!(lang,
                en: "the title is too long".to_string(),
                zh: "标题太长".to_string()),
            LiveFailure::Refused(429) => t!(lang,
                en: "too many attempts, try again in a minute".to_string(),
                zh: "操作太频繁，一分钟后再试".to_string()),
            LiveFailure::Refused(code) => t!(lang,
                en: format!("the relay refused it (HTTP {code})"),
                zh: format!("中转拒绝了（状态码 {code}）")),
        };
        t!(lang, en: format!("Not public: {reason}"), zh: format!("没能公开：{reason}"))
    }
```

- `Key` 枚举加八个键，`text()` 对应中英文案（按 Interfaces 里写的中文；英文：`p public/private`、`K change key`、`Public title: `、`Public live key: `、`Making it public…`、`Public live key saved`、`No longer public`、`The title cannot be empty`）；词条完整性守卫测试（`every_key_has_text` 一类，`grep -n "fn every_" src/i18n.rs` 找）里把新键加进列表。

- [ ] **Step 4: 跑测试确认通过**

Run: `env -u TERM cargo test --workspace --no-fail-fast`
Expected: PASS。

- [ ] **Step 5: 变异检查**

`LiveGrantToken` 的 `Debug` 改成 `f.write_str(&self.0)`（`a_live_grant_is_redacted_in_debug` 红）；`PROTOCOL_VERSION` 退回 18（形状测试红）。改回。

- [ ] **Step 6: Commit**

```bash
git add src
git commit -m "feat(proto): protocol 19 carries the public live state, publish requests and grant"
```

---

