### Task 3: 凭据来源（`Inherit` / `Key` / `Bearer`）

**Files:**
- Create: `src/llm/mod.rs`（本任务只放 `pub mod creds;` 一行占位，Task 4 填正文）
- Create: `src/llm/creds.rs`
- Modify: `src/lib.rs`（加 `pub mod llm;`）

**Interfaces:**
- Consumes: 无
- Produces:
  - `llm::creds::Credential { Inherit, Key(String), Bearer(String) }`
  - `llm::creds::parse_claude_oauth(json: &str) -> Option<String>`
  - `llm::creds::parse_codex_auth(json: &str) -> Option<Credential>`
  - `llm::creds::read_claude_oauth() -> Option<String>`（macOS 走 `security`，Linux 走文件）
  - `llm::creds::read_codex_auth() -> Option<Credential>`

**说明：** 这是整个计划里唯一碰用户凭据的地方，纪律最严：

1. **所有解析函数返回 `Option`，不返回 `Result`。** 厂商格式说变就变，格式变了要退化成「填个 key」，不是让 dct 报错。
2. **纯解析和真实读取分开。** 测试只测纯解析，喂**手写的**样本 JSON。**测试绝不调 `security`、绝不读真实 Keychain 或 `~/.codex/auth.json`。**
3. **`Credential` 不许实现 `Debug`/`Display` 打出明文。** 手写 `Debug` 打成 `Key(<redacted>)`。

本机实测到的形状（写死在测试样本里）：

- Claude Code (macOS)：Keychain service `Claude Code-credentials`，内容 `{"claudeAiOauth":{"accessToken":"...","refreshToken":"..."}}`
- Claude Code (Linux)：`~/.claude/.credentials.json`，同一形状
- Codex：`~/.codex/auth.json`，`{"auth_mode":"...","OPENAI_API_KEY":null,"tokens":{"access_token":"...","refresh_token":"...","account_id":"..."}}`
  —— `OPENAI_API_KEY` 非 null 时用它（`Key`），否则用 `tokens.access_token`（`Bearer`）。**`auth_mode` 字段本身就告诉我们走哪条，不用猜。**

- [ ] **Step 1: 写失败的测试**

`src/llm/creds.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // 样本全是手写的假数据。测试永远不读真实 Keychain 或 auth.json。
    const CLAUDE_SAMPLE: &str = r#"{"claudeAiOauth":{"accessToken":"at-fake","refreshToken":"rt-fake"}}"#;
    const CODEX_SSO_SAMPLE: &str = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,
        "tokens":{"id_token":"id","access_token":"at-fake","refresh_token":"rt","account_id":"acct"}}"#;
    const CODEX_KEY_SAMPLE: &str = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-fake","tokens":null}"#;

    #[test]
    fn reads_the_claude_oauth_access_token() {
        assert_eq!(parse_claude_oauth(CLAUDE_SAMPLE).as_deref(), Some("at-fake"));
    }

    #[test]
    fn codex_sso_login_yields_a_bearer() {
        assert_eq!(parse_codex_auth(CODEX_SSO_SAMPLE), Some(Credential::Bearer("at-fake".into())));
    }

    #[test]
    fn codex_api_key_login_yields_a_key() {
        assert_eq!(parse_codex_auth(CODEX_KEY_SAMPLE), Some(Credential::Key("sk-fake".into())));
    }

    /// 这组就是整个模块存在的理由：厂商改格式时**退化，不是报错**。
    #[test]
    fn any_unexpected_shape_yields_none_never_an_error() {
        let junk = [
            "",
            "not json at all",
            "{}",
            r#"{"claudeAiOauth":{}}"#,
            r#"{"claudeAiOauth":{"accessToken":null}}"#,
            r#"{"claudeAiOauth":{"accessToken":123}}"#,
            r#"{"renamedByVendor":{"accessToken":"x"}}"#,
            "[]",
        ];
        for j in junk {
            assert!(parse_claude_oauth(j).is_none(), "claude 该退化成 None: {j}");
            assert!(parse_codex_auth(j).is_none(), "codex 该退化成 None: {j}");
        }
    }

    #[test]
    fn an_empty_token_is_treated_as_absent() {
        // 空串是「有这个字段但没值」，当成没有——否则会拿空 token 去打端点，
        // 换来一个 401 和一句看不懂的错。
        assert!(parse_claude_oauth(r#"{"claudeAiOauth":{"accessToken":""}}"#).is_none());
    }

    #[test]
    fn debug_never_prints_the_secret() {
        // 凭据会跟着各种错误路径走，Debug 一旦打明文就会漏进 stderr 和日志。
        let k = Credential::Key("sk-super-secret".into());
        let b = Credential::Bearer("at-super-secret".into());
        assert!(!format!("{k:?}").contains("super-secret"), "Key 的 Debug 漏了明文");
        assert!(!format!("{b:?}").contains("super-secret"), "Bearer 的 Debug 漏了明文");
        assert_eq!(format!("{:?}", Credential::Inherit), "Inherit");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib llm::creds`
Expected: 编译失败，`unresolved module 'llm'`

- [ ] **Step 3: 写实现**

`src/llm/mod.rs`（本任务只要这两行）：

```rust
//! dct 自己用的 LLM 连接层。
pub mod creds;
```

`src/lib.rs` 在 `pub mod journal;` 后加 `pub mod llm;`（保持字母序）。

`src/llm/creds.rs`：

```rust
//! 凭据来源。
//!
//! **这是整个连接层里唯一碰用户凭据的地方，纪律因此最严：**
//!
//! 1. 所有解析函数返回 `Option`，**不返回 `Result`**。厂商的磁盘格式没有
//!    文档、说变就变；格式一变，用户该退化成「填个 key」，而不是看到
//!    dct 报一个他无能为力的错。
//! 2. **纯解析和真实读取分开。** 测试只喂手写样本，永远不碰真实
//!    Keychain / `~/.codex/auth.json`。
//! 3. `Debug` **不许打明文**——凭据会跟着错误路径走进 stderr 和日志。

use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    /// 什么都不用给：`CliBackend` 直接拉起，CLI 自己认证。
    /// **用户要的 SSO 在这里是零代码的。**
    Inherit,
    Key(String),
    Bearer(String),
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Credential::Inherit => f.write_str("Inherit"),
            Credential::Key(_) => f.write_str("Key(<redacted>)"),
            Credential::Bearer(_) => f.write_str("Bearer(<redacted>)"),
        }
    }
}

/// 非空才算数。空串是「有这个字段但没值」——拿空 token 去打端点只会换来
/// 一个 401 和一句用户看不懂的错。
fn non_empty(s: Option<&str>) -> Option<String> {
    match s {
        Some(v) if !v.is_empty() => Some(v.to_string()),
        _ => None,
    }
}

/// Claude Code 的 OAuth（macOS Keychain 内容 / Linux `.credentials.json`，同一形状）。
pub fn parse_claude_oauth(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    non_empty(v.get("claudeAiOauth")?.get("accessToken")?.as_str())
}

/// Codex 的 `~/.codex/auth.json`。
///
/// `OPENAI_API_KEY` 非 null 就是填 key 登录的，否则用 `tokens.access_token`。
pub fn parse_codex_auth(json: &str) -> Option<Credential> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if let Some(k) = non_empty(v.get("OPENAI_API_KEY").and_then(|k| k.as_str())) {
        return Some(Credential::Key(k));
    }
    let t = non_empty(v.get("tokens")?.get("access_token")?.as_str())?;
    Some(Credential::Bearer(t))
}

/// 真实读取。**没有单元测试覆盖**（会碰真实凭据），只在 `dct llm check`
/// 这条手动路径上跑。
pub fn read_claude_oauth() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        return parse_claude_oauth(String::from_utf8(out.stdout).ok()?.trim());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let home = std::env::var("HOME").ok()?;
        let p = std::path::Path::new(&home).join(".claude").join(".credentials.json");
        parse_claude_oauth(&std::fs::read_to_string(p).ok()?)
    }
}

pub fn read_codex_auth() -> Option<Credential> {
    let home = std::env::var("HOME").ok()?;
    let p = std::path::Path::new(&home).join(".codex").join("auth.json");
    parse_codex_auth(&std::fs::read_to_string(p).ok()?)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib llm::creds`
Expected: 6 passed

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && git add src/llm src/lib.rs
git commit -m "feat(llm): source credentials from keys, CLI OAuth, or inheritance

Parsing returns Option, never Result: vendor credential formats are
undocumented and change without notice, so a format change must degrade the
user to 'enter a key' rather than surface an error they cannot act on.

Pure parsing is split from real reads so tests never touch the real Keychain,
and Credential's Debug is hand-written to redact — credentials travel along
error paths into stderr and logs."
```

---

