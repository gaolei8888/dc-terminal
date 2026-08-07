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

/// 这份凭据是从**哪个程序的登录态里借来的**。
///
/// 只有「借来的」才有出处。用户自己在 dct 里填的 key 根本进不了这个类型——
/// 那是他把某个 key 填给了某个 provider，要发到哪里是他自己的决定，dct 不拦。
/// 这条区别是结构性的，不靠谁记得去判断：`resolve::select_credential` 里
/// 密钥仓那一路直接返回 `Credential`，只有 OAuth 那一路带着 `BorrowedFrom`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowedFrom {
    /// Claude Code 的登录态（macOS Keychain / `~/.claude/.credentials.json`）
    ClaudeCli,
    /// Codex 的 `~/.codex/auth.json`
    CodexCli,
}

/// 一份借来的凭据 = 出处 + 凭据本身。
pub type Borrowed = (BorrowedFrom, Credential);

impl BorrowedFrom {
    /// 这份登录态**允许发给哪台主机**。
    ///
    /// 规则：借来的登录态只能发回它自己那一家。Claude Code 的 OAuth 只能打
    /// Anthropic 自己的主机，Codex 的只能打 OpenAI 自己的主机。别的地方
    /// 一律不给——哪怕 profile 名、`[api].base_url`、`base_url` 覆盖三者
    /// 怎么组合，都改变不了「这个 token 不是给那台服务器的」这个事实。
    ///
    /// 匹配的是**主机**，不是 profile 名：名字是可以手写的（
    /// `~/.dct/profiles/claude.toml` 里塞一个 `[api]`、或者设置文件里写一行
    /// `base_url = "https://收集器.example/"`），主机是凭据真正会去的地方。
    pub fn may_reach(self, host: &str) -> bool {
        let owned = match self {
            // ChatGPT 的后端也是 OpenAI 自己在运营，codex 的 SSO token 本来
            // 就是发给它的，所以两个域名都算「它自己家」。
            BorrowedFrom::CodexCli => ["openai.com", "chatgpt.com"].as_slice(),
            BorrowedFrom::ClaudeCli => ["anthropic.com"].as_slice(),
        };
        let host = host.trim().to_ascii_lowercase();
        owned.iter().any(|d| {
            // 后缀要连着那个点一起比：不带点的话 `evil-anthropic.com`
            // 也会被当成自己人。
            host == *d || host.ends_with(&format!(".{d}"))
        })
    }
}

/// 从一个 base URL 里抠出主机名（小写，不带端口、不带用户名密码）。
///
/// 自己写而不是引一个 URL 库：这里只需要「凭据会发给哪台机器」这一个事实，
/// 而且**判不出来就必须是 `None`**（调用方一律当成「不许发」），
/// 宁可把一个合法地址判成不许，也不能把一个看不懂的地址放行。
pub fn host_of(url: &str) -> Option<String> {
    // 没有 `scheme://` 就不是一个地址，别猜。
    let (_, rest) = url.trim().split_once("://")?;
    // 路径、查询、锚点都不算主机。反斜杠也在这个集合里：ureq 用 `url` 这个 crate
    // 按 WHATWG 规则解析，特殊 scheme（http/https 都在内）下 `\` 和 `/` 一样会把
    // authority 截断。不认反斜杠的话，`https://evil.test\@api.anthropic.com`
    // 这种地址会被这里整段当成 authority，`@` 后面的 `api.anthropic.com` 就被
    // 误判成主机、放行；而 `url` 那边在反斜杠这里就已经把 authority 切断了，
    // 真正解析出来发请求的主机是 `evil.test`——检查这边看见一个主机、连接那边
    // 去了另一个，这正是这道关要拦的攻击形状。
    let authority = rest
        .split(['/', '?', '#', '\\'])
        .next()
        .filter(|s| !s.is_empty())?;
    // `user:pass@host` 里 `@` 后面才是主机——前面那段是可以随便写的，
    // `https://api.anthropic.com@收集器.example/` 就是靠这一步不被认成
    // Anthropic 的。
    let host_port = authority.rsplit('@').next()?;
    // IPv6 字面量 `[::1]:443`
    let host = match host_port.strip_prefix('[') {
        Some(r) => r.split_once(']').map(|(h, _)| h)?,
        None => host_port.split(':').next()?,
    };
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
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
            .args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        parse_claude_oauth(String::from_utf8(out.stdout).ok()?.trim())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let home = std::env::var("HOME").ok()?;
        let p = std::path::Path::new(&home)
            .join(".claude")
            .join(".credentials.json");
        parse_claude_oauth(&std::fs::read_to_string(p).ok()?)
    }
}

pub fn read_codex_auth() -> Option<Credential> {
    let home = std::env::var("HOME").ok()?;
    let p = std::path::Path::new(&home).join(".codex").join("auth.json");
    parse_codex_auth(&std::fs::read_to_string(p).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 样本全是手写的假数据。测试永远不读真实 Keychain 或 auth.json。
    const CLAUDE_SAMPLE: &str =
        r#"{"claudeAiOauth":{"accessToken":"at-fake","refreshToken":"rt-fake"}}"#;
    const CODEX_SSO_SAMPLE: &str = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,
        "tokens":{"id_token":"id","access_token":"at-fake","refresh_token":"rt","account_id":"acct"}}"#;
    const CODEX_KEY_SAMPLE: &str =
        r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-fake","tokens":null}"#;

    #[test]
    fn reads_the_claude_oauth_access_token() {
        assert_eq!(
            parse_claude_oauth(CLAUDE_SAMPLE).as_deref(),
            Some("at-fake")
        );
    }

    #[test]
    fn codex_sso_login_yields_a_bearer() {
        assert_eq!(
            parse_codex_auth(CODEX_SSO_SAMPLE),
            Some(Credential::Bearer("at-fake".into()))
        );
    }

    #[test]
    fn codex_api_key_login_yields_a_key() {
        assert_eq!(
            parse_codex_auth(CODEX_KEY_SAMPLE),
            Some(Credential::Key("sk-fake".into()))
        );
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

    /// 借来的登录态只认**主机**，不认名字。这条钉的是整个 CRITICAL 类别：
    /// profile 名是用户可以手写的，主机是凭据真正会去的地方。
    #[test]
    fn a_borrowed_login_only_reaches_its_own_vendors_hosts() {
        use BorrowedFrom::*;
        for h in ["api.anthropic.com", "anthropic.com", "eu.api.anthropic.com"] {
            assert!(ClaudeCli.may_reach(h), "{h} 是 Anthropic 自己的主机");
            assert!(!CodexCli.may_reach(h), "codex 的登录态不该发给 {h}");
        }
        for h in ["api.openai.com", "openai.com", "chatgpt.com"] {
            assert!(CodexCli.may_reach(h), "{h} 是 OpenAI 自己的主机");
            assert!(!ClaudeCli.may_reach(h), "claude 的登录态不该发给 {h}");
        }
        for h in [
            "api.moonshot.cn",
            "open.bigmodel.cn",
            "api.deepseek.com",
            "dashscope.aliyuncs.com",
            // 后缀比较不带点就会把这两个放行——那正是最容易被人拿去
            // 骗凭据的形状。
            "evil-anthropic.com",
            "anthropic.com.example.cn",
            "notopenai.com",
            "",
        ] {
            assert!(!ClaudeCli.may_reach(h), "claude 的登录态不该发给「{h}」");
            assert!(!CodexCli.may_reach(h), "codex 的登录态不该发给「{h}」");
        }
    }

    #[test]
    fn the_host_is_taken_from_the_url_not_guessed() {
        assert_eq!(
            host_of("https://API.Anthropic.com/v1/messages").as_deref(),
            Some("api.anthropic.com")
        );
        assert_eq!(
            host_of("https://api.moonshot.cn:443/anthropic").as_deref(),
            Some("api.moonshot.cn")
        );
        assert_eq!(host_of("https://[::1]:8080/v1").as_deref(), Some("::1"));
        // `user@host` 这一手：`@` 前面那段可以随便写，真正的主机在后面。
        assert_eq!(
            host_of("https://api.anthropic.com@collector.example/v1").as_deref(),
            Some("collector.example")
        );
        // 反斜杠版的同一手：ureq 用的 `url` crate 按 WHATWG 规则会在 `\` 这里
        // 就把 authority 切断，真正的主机是 `evil.test`——如果这里不认反斜杠，
        // 会把 `@` 后面的 `api.anthropic.com` 误判成主机，让借来的 Anthropic
        // token 放行给 evil.test。
        assert_eq!(
            host_of(r"https://evil.test\@api.anthropic.com").as_deref(),
            Some("evil.test")
        );
        // 看不懂就是 None，调用方一律当成「不许发」。
        for bad in ["", "https://", "/v1/messages", "https:///v1"] {
            assert!(host_of(bad).is_none(), "看不懂的地址要是 None：{bad}");
        }
    }

    #[test]
    fn debug_never_prints_the_secret() {
        // 凭据会跟着各种错误路径走，Debug 一旦打明文就会漏进 stderr 和日志。
        let k = Credential::Key("sk-super-secret".into());
        let b = Credential::Bearer("at-super-secret".into());
        assert!(
            !format!("{k:?}").contains("super-secret"),
            "Key 的 Debug 漏了明文"
        );
        assert!(
            !format!("{b:?}").contains("super-secret"),
            "Bearer 的 Debug 漏了明文"
        );
        assert_eq!(format!("{:?}", Credential::Inherit), "Inherit");
    }
}
