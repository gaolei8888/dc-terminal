//! 从配置 + profile + 密钥里组装出一个后端。
//!
//! **凭据顺序在这里定死**：先用户显式填的 key，再从别人家里翻出来的 OAuth。
//! 理由是用户填过的东西优先于我们替他猜的东西。

use super::cli::CliBackend;
use super::creds::{host_of, Borrowed, Credential};
use super::http::HttpBackend;
use super::Backend;
use crate::config::{LlmConfig, Transport};
use crate::profile::{Profile, Wire};
use crate::secrets::SecretStore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 连不上的原因。**报码不组句**——句子在界面进程由 `i18n::msg::llm_problem`
/// 组（同 `proto::ErrorCode`：守护进程不知道用户选的是哪种语言，而且这个码
/// 会跟着 `WarningCode::LlmUnavailable` 走 socket 到界面那边去）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolveError {
    NoSuchProvider(String),
    NoHeadlessCommand(String),
    NoApiEndpoint(String),
    NoCredential(String),
    /// 直连端点存在，但设置里没写要用哪个模型——瞎猜一个默认值只会
    /// 换来一个 404，不如让用户自己填。
    NoModel(String),
    /// 手上唯一能用的凭据是**从别的程序的登录态里借来的**，而它要去的
    /// 主机不是那个程序自己家的。这不是「没配好」，是拒绝——见
    /// `select_credential`。
    BorrowedCredentialRefused {
        name: String,
        host: String,
    },
    /// 直连的地址根本不像个网址（主机名抠不出来）。看不懂就不发，
    /// 见 `creds::host_of`。
    BadBaseUrl {
        name: String,
        url: String,
    },
}

/// 凭据来源的顺序：**先用户显式填的 key，再从别人家里翻出来的 OAuth**。
/// 拆成一个独立、可直接测的小函数——`resolve` 剩下的部分（有没有这个
/// provider、走哪条 transport）跟这条顺序完全正交，没必要混在一起测。
///
/// **`host` 这个参数是整条 CRITICAL 防线的落点。** 从别的程序（Claude Code /
/// Codex）的登录态里借来的凭据，只能发给那个程序自己家的主机；用户自己在
/// dct 里填的 key 不受这条约束——他把那个 key 填给了这个 provider，要发到
/// 哪里是他自己的决定。这条区别是结构性的：密钥仓那一路直接返回
/// `Credential`，只有 OAuth 那一路带着 `BorrowedFrom` 出处（见
/// `creds::BorrowedFrom`），所以「用户填的 key 不受限」不需要谁记得去写
/// 一个 if。
///
/// 为什么要按主机判而不是按 provider 名判：`cli.rs::oauth_lookup` /
/// `daemon.rs::startup_oauth` 按**名字**只把 claude/codex 映射到各自的登录态，
/// 那条防线还在（纵深），但名字是用户可以手写的——
/// `~/.dct/profiles/claude.toml` 里塞一个 `[api] base_url = "https://收集器/"`，
/// 或者设置文件里写一行 `base_url = ...`，名字还叫 claude，token 却飞去了别处。
/// 主机是凭据真正会去的地方，判它才判到了点子上。
fn select_credential(
    name: &str,
    host: &str,
    secrets: &SecretStore,
    oauth: &dyn Fn(&str) -> Option<Borrowed>,
) -> Result<Credential, ResolveError> {
    if let Some(k) = secrets.get(name) {
        return Ok(Credential::Key(k.to_string()));
    }
    match oauth(name) {
        Some((from, cred)) if from.may_reach(host) => Ok(cred),
        Some(_) => Err(ResolveError::BorrowedCredentialRefused {
            name: name.to_string(),
            host: host.to_string(),
        }),
        None => Err(ResolveError::NoCredential(name.to_string())),
    }
}

/// 无界面子进程拿到的环境变量：profile 的 `[env]` 打底，密钥覆盖上去。
///
/// 跟 `session.rs` 起交互式会话时那几行是同一件事，理由也一样：密钥不写在
/// profile 文件里，只在起进程这一步才和命令合到一起。拆成独立函数是为了能
/// 直接断言「密钥真的进了环境变量」——`CliBackend` 把 env 捕获进闭包了
/// （那是对的，见它的注释），从外面看不到。
fn headless_env(
    p: &Profile,
    name: &str,
    secrets: &SecretStore,
) -> Result<std::collections::BTreeMap<String, String>, ResolveError> {
    let mut env = p.env.clone();
    if let Some(spec) = &p.secret {
        let key = secrets
            .get(name)
            .ok_or_else(|| ResolveError::NoCredential(name.to_string()))?;
        env.insert(spec.env.clone(), key.to_string());
    }
    Ok(env)
}

/// 两种 wire 说的不是同一条路径：Anthropic 是 `/v1/messages`，OpenAI 兼容
/// 端点是 `/v1/chat/completions`。写死前者会让所有 OpenAI 型端点 404——
/// 拆成一个小函数单独测，不用绕道整个 `resolve()` 才能验证路径对不对。
fn http_url(base: &str, wire: Wire) -> String {
    let path = match wire {
        Wire::Anthropic => "/v1/messages",
        Wire::Openai => "/v1/chat/completions",
    };
    format!("{}{path}", base.trim_end_matches('/'))
}

/// **调用方负责先问「用户开了没有」。** 这个函数只回答「开了之后，该接
/// 哪个后端」——它接的是 `LlmConfig`，不是 `Config`，是故意的：`Config::llm`
/// 是 `Option`，`None` 就是「没开」，那不是这个函数该处理的一种情况，是
/// 调用方压根不该调它的一种情况（daemon 启动时不 resolve、`dct llm check`
/// 打一句「还没开」就退出，两边都在调用前就分了叉，见各自的调用点）。
/// 这里如果也认一个 `Option` 就会长出第二条「没开」的路径，两条路径迟早
/// 会说不一样的话。
pub fn resolve(
    llm: &LlmConfig,
    lookup: &dyn Fn(&str) -> Option<Profile>,
    secrets: &SecretStore,
    oauth: &dyn Fn(&str) -> Option<Borrowed>,
) -> Result<Arc<dyn Backend>, ResolveError> {
    let name = llm.provider.as_str();
    let p = lookup(name).ok_or_else(|| ResolveError::NoSuchProvider(name.to_string()))?;

    match llm.transport {
        Transport::Cli => {
            let h = p
                .headless
                .as_ref()
                .ok_or_else(|| ResolveError::NoHeadlessCommand(name.to_string()))?;
            // 同一条「A 家凭据不许发去 B 家」的纪律，在这条路上长成另一个样子。
            //
            // 子进程这边我们不塞任何 Authorization 头，凭据是靠环境变量传的
            // （`[secret].env`，跟 `session.rs` 起交互式会话时做的完全一样）。
            // profile 声明了 `[secret]` 却一个都没注入的话，那个 CLI 会转头去
            // 读**它自己的**登录态（Claude Code 读 Keychain），而 `[env]` 里的
            // `ANTHROPIC_BASE_URL` 已经把它指向了第三方端点——这就是用另一条
            // 路走到了同一个「把 A 家的登录态发给 B 家」。
            //
            // 所以：声明了要密钥就必须有密钥，没有就明确拒绝，绝不让子进程
            // 自己去凑一个。没声明 `[secret]` 的（claude / codex 自己）行为
            // 不变——它们打的本来就是自己家的端点，登录归它们自己管。
            let env = headless_env(&p, name, secrets)?;
            Ok(Arc::new(CliBackend::new(h.command.clone(), env)))
        }
        Transport::Http => {
            let api = p
                .api
                .as_ref()
                .ok_or_else(|| ResolveError::NoApiEndpoint(name.to_string()))?;
            let base = llm.base_url.clone().unwrap_or_else(|| api.base_url.clone());
            // 先把「凭据会发给哪台机器」算出来，再决定给不给凭据。
            // 抠不出主机名就一个字节都不发——看不懂的地址不是「大概没事」。
            let host = host_of(&base).ok_or_else(|| ResolveError::BadBaseUrl {
                name: name.to_string(),
                url: base.clone(),
            })?;
            let cred = select_credential(name, &host, secrets, oauth)?;
            // 猜一个默认模型（比如写死 claude-3-5-sonnet）会在非 Anthropic
            // 端点上稳定换来 404——这是要用户自己拍板的事，不是能替他猜的。
            let model = llm
                .model
                .clone()
                .ok_or_else(|| ResolveError::NoModel(name.to_string()))?;
            let url = http_url(&base, api.wire);
            Ok(Arc::new(HttpBackend::new(url, api.wire, model, cred)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LlmConfig, Transport};
    use crate::llm::creds::BorrowedFrom;

    // `resolve()` 只接 `LlmConfig`（「开了之后配什么」），不接 `Config`
    // （「开没开」）——见 `resolve` 上的注释，这里的测试也就不需要
    // `Config`/`Option` 那一层了。
    fn cfg(provider: &str, transport: Transport) -> LlmConfig {
        LlmConfig {
            provider: provider.into(),
            model: Some("m".into()),
            base_url: None,
            transport,
        }
    }

    fn builtin(name: &str) -> Option<Profile> {
        Profile::builtin(name)
    }

    fn no_oauth(_: &str) -> Option<Borrowed> {
        None
    }

    /// 假装在用户机器上翻到了一份 Claude Code 的登录态。**测试永远不碰
    /// 真实 Keychain / `auth.json`**，全靠注入。
    fn claude_oauth(_: &str) -> Option<Borrowed> {
        Some((
            BorrowedFrom::ClaudeCli,
            Credential::Bearer("claude-keychain-token".into()),
        ))
    }

    fn empty_secrets() -> SecretStore {
        SecretStore::load(std::path::Path::new("/nonexistent/secrets.toml"))
    }

    /// 一个只在这条测试里存在的密钥仓（临时目录，不碰 `~/.dct`）。
    fn secrets_with(name: &str, key: &str) -> (tempfile::TempDir, SecretStore) {
        let dir = tempfile::tempdir().unwrap();
        let mut s = SecretStore::load(&dir.path().join("secrets.toml"));
        s.set(name, key).unwrap();
        (dir, s)
    }

    #[test]
    fn the_cli_transport_needs_no_credential_at_all() {
        // SSO 在这条路上是零代码的：密钥空的、OAuth 也翻不到，照样成。
        let r = resolve(
            &cfg("claude", Transport::Cli),
            &builtin,
            &empty_secrets(),
            &no_oauth,
        );
        assert!(r.is_ok(), "CLI 这条路不该需要凭据");
    }

    /// 无界面这条路上的同一类问题：profile 说了要密钥，我们却一个都没给，
    /// 那个 CLI 就会转头去读**它自己的**登录态（Claude Code 读 Keychain），
    /// 而 `[env]` 里的 `ANTHROPIC_BASE_URL` 已经把它指向第三方端点——
    /// 又一次把 A 家的登录态发给了 B 家。声明了要密钥就必须有密钥。
    #[test]
    fn a_headless_profile_that_needs_a_key_is_refused_without_one() {
        let p = Profile::from_toml(
            r#"
            name = "mykimi"
            command = ["claude"]
            [env]
            ANTHROPIC_BASE_URL = "https://api.moonshot.cn/anthropic"
            [secret]
            env = "ANTHROPIC_AUTH_TOKEN"
            [headless]
            command = ["claude", "-p"]
            "#,
        )
        .unwrap();
        let r = resolve(
            &cfg("mykimi", Transport::Cli),
            &|_: &str| Some(p.clone()),
            &empty_secrets(),
            &claude_oauth,
        );
        assert!(
            matches!(r, Err(ResolveError::NoCredential(ref n)) if n == "mykimi"),
            "没有厂商密钥就不能让子进程自己去借一个别家的登录态"
        );
    }

    /// 有密钥就注进环境变量，跟起交互式会话时一模一样（`session.rs`）。
    #[test]
    fn the_headless_child_gets_the_users_key_in_its_environment() {
        let p = Profile::from_toml(
            r#"
            name = "mykimi"
            command = ["claude"]
            [env]
            ANTHROPIC_BASE_URL = "https://api.moonshot.cn/anthropic"
            [secret]
            env = "ANTHROPIC_AUTH_TOKEN"
            [headless]
            command = ["claude", "-p"]
            "#,
        )
        .unwrap();
        let (_dir, secrets) = secrets_with("mykimi", "sk-user-typed");
        let env = headless_env(&p, "mykimi", &secrets).unwrap();
        assert_eq!(
            env.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str),
            Some("sk-user-typed"),
            "密钥要注到 profile 指定的那个环境变量上"
        );
        assert_eq!(
            env.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("https://api.moonshot.cn/anthropic"),
            "profile 自己的 env 要留着"
        );
    }

    /// 没声明 `[secret]` 的 profile（claude / codex 自己）行为不变：
    /// 它们打的本来就是自己家的端点，登录归它们自己管。
    #[test]
    fn a_headless_profile_without_a_secret_block_is_untouched() {
        let p = Profile::builtin("claude").unwrap();
        let env = headless_env(&p, "claude", &empty_secrets()).unwrap();
        assert!(env.is_empty(), "claude 自己没有 env 要注");
    }

    // `Result<Arc<dyn Backend>, ResolveError>` 的 `T` 没有（也不该有）
    // `Debug`——见 review IMPORTANT (b)：给 `dyn Backend` 加一个只为了测试
    // 而存在的 `Debug` 实现，是拿一个编译期的“没人打印 backend”防线去换
    // 一个测试的方便。改用 `matches!`，不需要 `T: Debug`。

    #[test]
    fn a_profile_without_a_headless_command_is_refused_by_name() {
        // opencode 没验过无界面模式，不许假装能用。
        let r = resolve(
            &cfg("opencode", Transport::Cli),
            &builtin,
            &empty_secrets(),
            &no_oauth,
        );
        assert!(matches!(
            r,
            Err(ResolveError::NoHeadlessCommand(ref n)) if n == "opencode"
        ));
    }

    #[test]
    fn an_unknown_provider_is_named_in_the_error() {
        let r = resolve(
            &cfg("nope", Transport::Cli),
            &builtin,
            &empty_secrets(),
            &no_oauth,
        );
        assert!(matches!(r, Err(ResolveError::NoSuchProvider(ref n)) if n == "nope"));
    }

    #[test]
    fn http_without_any_credential_is_refused() {
        let r = resolve(
            &cfg("kimi", Transport::Http),
            &builtin,
            &empty_secrets(),
            &no_oauth,
        );
        assert!(matches!(r, Err(ResolveError::NoCredential(ref n)) if n == "kimi"));
    }

    #[test]
    fn http_uses_an_oauth_token_when_there_is_no_key() {
        // 目的地必须是这份登录态自己家的主机，OAuth 才用得上——所以这里
        // 把 base_url 覆盖成 Anthropic 官方端点。发往别处的情形见
        // `a_borrowed_login_is_refused_when_the_destination_is_not_its_own`。
        let mut c = cfg("kimi", Transport::Http);
        c.base_url = Some("https://api.anthropic.com".into());
        let r = resolve(&c, &builtin, &empty_secrets(), &claude_oauth);
        assert!(r.is_ok(), "翻得到 OAuth、而且是发回它自己家，就该能用");
    }

    /// **CRITICAL 类别的落点。** 从 Claude Code 的登录态里借来的 token，
    /// 发往 moonshot / bigmodel / deepseek / dashscope 一律拒绝——不管
    /// provider 叫什么名字。
    #[test]
    fn a_borrowed_login_is_refused_when_the_destination_is_not_its_own() {
        for name in ["kimi", "glm", "deepseek", "qwen-api"] {
            let r = resolve(
                &cfg(name, Transport::Http),
                &builtin,
                &empty_secrets(),
                &claude_oauth,
            );
            assert!(
                matches!(r, Err(ResolveError::BorrowedCredentialRefused { name: ref n, .. }) if n == name),
                "{name}: 借来的 Anthropic 登录态不该发给第三方端点"
            );
        }
    }

    /// 名字骗不过这道关：`~/.dct/profiles/claude.toml` 里手写一个 `[api]`，
    /// 或者设置文件里写一行 `base_url`，名字还叫 claude，token 却飞去别处。
    /// `oauth_lookup` 那条按名字的防线在这里是失效的（名字确实是 claude），
    /// 只有按主机判才拦得住。
    #[test]
    fn a_hand_written_claude_profile_cannot_aim_the_keychain_token_elsewhere() {
        let mine = Profile::from_toml(
            r#"
            name = "claude"
            command = ["claude"]
            [api]
            base_url = "https://收集器.example/anthropic"
            wire = "anthropic"
            "#,
        )
        .unwrap();
        let lookup = |_: &str| Some(mine.clone());
        let r = resolve(
            &cfg("claude", Transport::Http),
            &lookup,
            &empty_secrets(),
            &claude_oauth,
        );
        assert!(
            matches!(r, Err(ResolveError::BorrowedCredentialRefused { ref host, .. }) if host == "收集器.example"),
            "profile 名叫 claude 不等于目的地是 Anthropic"
        );

        // 设置文件里的 base_url 覆盖是同一个洞的另一个入口。
        let mut c = cfg("claude", Transport::Http);
        c.base_url = Some("https://collector.example/v1".into());
        let r = resolve(&c, &lookup, &empty_secrets(), &claude_oauth);
        assert!(matches!(
            r,
            Err(ResolveError::BorrowedCredentialRefused { .. })
        ));
    }

    /// 同一个洞的反斜杠变体：ureq 用的 `url` crate 按 WHATWG 规则解析，
    /// `\` 和 `/` 一样会把 authority 切断，所以
    /// `https://evil.test\@api.anthropic.com` 真正会连去 `evil.test`，
    /// 而 `@` 前面那段看着像 `api.anthropic.com`。如果 `creds::host_of`
    /// 不认反斜杠，就会把这个地址判成「发回 Anthropic 自己家」，把 Keychain
    /// 里借来的 Bearer 放行给 evil.test。断言必须落在这道关（`resolve`
    /// 拒绝、错误是 `BorrowedCredentialRefused`），不能只测 `host_of` 本身——
    /// 拦不拦得住凭据才是这条防线真正要回答的问题。
    #[test]
    fn a_borrowed_login_is_refused_when_the_backslash_hides_the_real_host() {
        let mut c = cfg("kimi", Transport::Http);
        c.base_url = Some(r"https://evil.test\@api.anthropic.com".into());
        let r = resolve(&c, &builtin, &empty_secrets(), &claude_oauth);
        // `Result<Arc<dyn Backend>, ResolveError>` 的 `T` 没有 `Debug`（见上面
        // review IMPORTANT (b) 那条注释），断言失败信息只能是静态文案。
        assert!(
            matches!(
                r,
                Err(ResolveError::BorrowedCredentialRefused { ref host, .. }) if host == "evil.test"
            ),
            "反斜杠不该被当成 `api.anthropic.com` 的一部分放行"
        );
    }

    /// 用户自己填的 key **不受**上面那条约束：他把这个 key 填给了这个
    /// provider，要发到哪里是他自己的决定。同一个目的地、同一个 provider，
    /// 借来的 token 被拒、他自己的 key 照走。
    #[test]
    fn a_key_the_user_typed_in_is_still_allowed_to_that_same_host() {
        let (_dir, secrets) = secrets_with("kimi", "sk-user-typed");
        let r = resolve(
            &cfg("kimi", Transport::Http),
            &builtin,
            &secrets,
            &claude_oauth,
        );
        assert!(r.is_ok(), "用户自己填给 kimi 的 key 就该发给 kimi");
        // 而且真正被带走的是用户那把 key，不是 Keychain 里那份。
        let cred = select_credential("kimi", "api.moonshot.cn", &secrets, &claude_oauth).unwrap();
        assert_eq!(cred, Credential::Key("sk-user-typed".into()));
    }

    /// 抠不出主机名 = 不知道会发给谁。一个字节都不发。
    #[test]
    fn an_address_we_cannot_read_gets_no_credential_at_all() {
        let mine = Profile::from_toml(
            r#"
            name = "weird"
            command = ["x"]
            [api]
            base_url = "这不是网址"
            wire = "anthropic"
            "#,
        )
        .unwrap();
        let r = resolve(
            &cfg("weird", Transport::Http),
            &|_: &str| Some(mine.clone()),
            &empty_secrets(),
            &claude_oauth,
        );
        assert!(matches!(r, Err(ResolveError::BadBaseUrl { .. })));
    }

    /// 核心决定钉在这——之前 7 个测试全绿，但没有一个同时给 key 和
    /// OAuth：把 `select_credential` 里的 `.or_else()` 顺序反过来，
    /// 那 7 个测试照样全过。这条专门堵这个回归。见 review IMPORTANT (a)。
    #[test]
    fn an_explicit_key_outranks_an_oauth_token_found_elsewhere() {
        let (_dir, secrets) = secrets_with("kimi", "sk-explicit-key");
        // 目的地故意选成这份 OAuth 去得了的主机，好让「顺序」成为唯一的
        // 变量——否则这条测试会被主机检查悄悄替它通过。
        let with_oauth = |_: &str| {
            Some((
                BorrowedFrom::ClaudeCli,
                Credential::Bearer("oauth-token".into()),
            ))
        };

        let cred = select_credential("kimi", "api.anthropic.com", &secrets, &with_oauth).unwrap();

        // `==` 比较，不格式化——`Credential` 的 `Debug` 是刻意打码的。
        assert_eq!(cred, Credential::Key("sk-explicit-key".into()));
    }

    #[test]
    fn http_needs_an_api_block() {
        // claude 没有 [api]（它走官方端点，靠 CLI 自己登录），直连没地方去。
        let r = resolve(
            &cfg("claude", Transport::Http),
            &builtin,
            &empty_secrets(),
            &claude_oauth,
        );
        assert!(matches!(r, Err(ResolveError::NoApiEndpoint(ref n)) if n == "claude"));
    }

    /// 直连端点存在、凭据也有，但没写型号——瞎猜一个默认模型（比如写死
    /// claude-3-5-sonnet）在非 Anthropic 端点上会稳定 404。
    #[test]
    fn http_without_a_model_is_refused_instead_of_guessing_one() {
        let mut c = cfg("kimi", Transport::Http);
        c.model = None;
        // 凭据用用户自己填的 key（kimi 的正路），免得这条测试其实是被
        // 凭据那一关挡下来的。
        let (_dir, secrets) = secrets_with("kimi", "sk-user-typed");
        let r = resolve(&c, &builtin, &secrets, &no_oauth);
        assert!(matches!(r, Err(ResolveError::NoModel(ref n)) if n == "kimi"));
    }

    #[test]
    fn anthropic_and_openai_wires_hit_different_paths() {
        // 写死 /v1/messages 会让所有 OpenAI 型端点 404——Task 9 的实测
        // 就靠这条路径是对的。
        assert_eq!(
            http_url("https://x.test", Wire::Anthropic),
            "https://x.test/v1/messages"
        );
        assert_eq!(
            http_url("https://x.test", Wire::Openai),
            "https://x.test/v1/chat/completions"
        );
        // 结尾的斜杠不能翻倍。
        assert_eq!(
            http_url("https://x.test/", Wire::Anthropic),
            "https://x.test/v1/messages"
        );
    }

    /// 每一条原因的**两种语言**都要说得清楚。
    ///
    /// 文案本身现在长在 `i18n::msg::llm_problem` 上（这一层只报码，不组句，
    /// 同 `proto::ErrorCode`），但这条守卫留在这里：它守的是「这个枚举里
    /// 每加一个变体，都得有一句人话」，而这个枚举就在本文件里。
    #[test]
    fn every_reason_explains_itself_in_both_languages_with_a_real_next_step() {
        use crate::i18n::msg::llm_problem;
        use crate::i18n::Lang;

        // 用户是零编程经验的人：不许夹带内部字段名/类型名，每句话都要
        // 点名是哪个厂商、并且给一句他真做得到的下一步。查具体禁词
        // （大小写不敏感）+ 查真实存在的动作词——只查长度的话
        // "x xxxxxxxxx" 这种字符串也能混过去。
        let all = [
            ResolveError::NoSuchProvider("x".into()),
            ResolveError::NoHeadlessCommand("x".into()),
            ResolveError::NoApiEndpoint("x".into()),
            ResolveError::NoCredential("x".into()),
            ResolveError::NoModel("x".into()),
            ResolveError::BorrowedCredentialRefused {
                name: "x".into(),
                host: "api.moonshot.cn".into(),
            },
            ResolveError::BadBaseUrl {
                name: "x".into(),
                url: "这不是网址".into(),
            },
        ];
        for e in &all {
            for lang in [Lang::Zh, Lang::En] {
                let s = llm_problem(lang, e);
                assert!(s.contains('x'), "要点名是哪个厂商: {s}");
                assert!(s.chars().count() > 8, "太短，说不清楚: {s}");
                for jargon in ["provider", "transport", "cli", "agent", "error"] {
                    assert!(
                        !s.to_lowercase().contains(jargon),
                        "不能夹带内部字段名/类型名「{jargon}」，那是代码里的词不是用户的词: {s}"
                    );
                }
            }
        }
        // 指向「去改设置」的那几条都要带上**真实路径**——「设置文件」
        // 四个字对一个非程序员不是一个可执行的下一步。
        for e in [
            ResolveError::NoSuchProvider("x".into()),
            ResolveError::NoHeadlessCommand("x".into()),
            ResolveError::NoApiEndpoint("x".into()),
            ResolveError::NoModel("x".into()),
            ResolveError::BadBaseUrl {
                name: "x".into(),
                url: "u".into(),
            },
        ] {
            for lang in [Lang::Zh, Lang::En] {
                let s = llm_problem(lang, &e);
                assert!(
                    s.contains(crate::i18n::msg::CONFIG_PATH),
                    "要给出真实路径，不能只说「设置文件」: {s}"
                );
            }
        }
        assert!(
            llm_problem(Lang::Zh, &ResolveError::NoCredential("x".into())).contains("按 c"),
            "要指向真实存在的那个按键"
        );
        // 被拒的那条要说清楚**为什么**——只说「没密钥」，用户会一直觉得
        // 「我明明登录过了」。
        let refused = llm_problem(
            Lang::Zh,
            &ResolveError::BorrowedCredentialRefused {
                name: "x".into(),
                host: "api.moonshot.cn".into(),
            },
        );
        assert!(refused.contains("api.moonshot.cn"), "要点名是发给哪台机器");
        assert!(refused.contains("按 c"), "要给出他做得到的下一步");
    }
}
