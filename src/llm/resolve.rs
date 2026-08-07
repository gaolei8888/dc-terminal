//! 从配置 + profile + 密钥里组装出一个后端。
//!
//! **凭据顺序在这里定死**：先用户显式填的 key，再从别人家里翻出来的 OAuth。
//! 理由是用户填过的东西优先于我们替他猜的东西。

use super::cli::CliBackend;
use super::creds::Credential;
use super::http::HttpBackend;
use super::Backend;
use crate::config::{LlmConfig, Transport};
use crate::profile::{Profile, Wire};
use crate::secrets::SecretStore;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    NoSuchProvider(String),
    NoHeadlessCommand(String),
    NoApiEndpoint(String),
    NoCredential(String),
    /// 直连端点存在，但设置里没写要用哪个模型——瞎猜一个默认值只会
    /// 换来一个 404，不如让用户自己填。
    NoModel(String),
}

/// 用户看得懂、且说得出下一步的一句话。
///
/// **不许出现内部字段名/类型名**（provider / transport / cli / agent /
/// Error 这类）——那是 Rust 代码里的词，不是用户的词。见 review IMPORTANT
/// (c)：用户是零编程经验的人，每句话点名是哪个厂商，并且给一个他真做得
/// 到的下一步。
pub fn describe(e: &ResolveError) -> String {
    match e {
        ResolveError::NoSuchProvider(n) => {
            format!("设置文件里写的「{n}」不是 dct 认识的名字，把它换成 claude 试试。")
        }
        ResolveError::NoHeadlessCommand(n) => {
            format!("「{n}」还没法自己在后台回答问题，把设置文件里这一项换成 claude 试试。")
        }
        ResolveError::NoApiEndpoint(n) => {
            format!(
                "「{n}」没有可以直接连接的网址，把设置文件里“直连”这一项关掉，改回让它自己登录。"
            )
        }
        ResolveError::NoCredential(n) => {
            format!("「{n}」还没有密钥。在主界面按 c 填一个。")
        }
        ResolveError::NoModel(n) => {
            format!("「{n}」还没有指定用哪个型号，把设置文件里这一项填一个具体的型号名。")
        }
    }
}

/// 凭据来源的顺序：**先用户显式填的 key，再从别人家里翻出来的 OAuth**。
/// 拆成一个独立、可直接测的小函数——`resolve` 剩下的部分（有没有这个
/// provider、走哪条 transport）跟这条顺序完全正交，没必要混在一起测。
fn select_credential(
    name: &str,
    secrets: &SecretStore,
    oauth: &dyn Fn(&str) -> Option<Credential>,
) -> Result<Credential, ResolveError> {
    secrets
        .get(name)
        .map(|k| Credential::Key(k.to_string()))
        .or_else(|| oauth(name))
        .ok_or_else(|| ResolveError::NoCredential(name.to_string()))
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
    oauth: &dyn Fn(&str) -> Option<Credential>,
) -> Result<Arc<dyn Backend>, ResolveError> {
    let name = llm.provider.as_str();
    let p = lookup(name).ok_or_else(|| ResolveError::NoSuchProvider(name.to_string()))?;

    match llm.transport {
        Transport::Cli => {
            let h = p
                .headless
                .as_ref()
                .ok_or_else(|| ResolveError::NoHeadlessCommand(name.to_string()))?;
            // Inherit：一个凭据都不查，CLI 自己管登录。
            Ok(Arc::new(CliBackend::new(h.command.clone(), p.env.clone())))
        }
        Transport::Http => {
            let api = p
                .api
                .as_ref()
                .ok_or_else(|| ResolveError::NoApiEndpoint(name.to_string()))?;
            let cred = select_credential(name, secrets, oauth)?;
            // 猜一个默认模型（比如写死 claude-3-5-sonnet）会在非 Anthropic
            // 端点上稳定换来 404——这是要用户自己拍板的事，不是能替他猜的。
            let model = llm
                .model
                .clone()
                .ok_or_else(|| ResolveError::NoModel(name.to_string()))?;
            let base = llm.base_url.clone().unwrap_or_else(|| api.base_url.clone());
            let url = http_url(&base, api.wire);
            Ok(Arc::new(HttpBackend::new(url, api.wire, model, cred)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LlmConfig, Transport};

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

    fn no_oauth(_: &str) -> Option<Credential> {
        None
    }

    fn empty_secrets() -> SecretStore {
        SecretStore::load(std::path::Path::new("/nonexistent/secrets.toml"))
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
        let with_oauth = |_: &str| Some(Credential::Bearer("t".into()));
        let r = resolve(
            &cfg("kimi", Transport::Http),
            &builtin,
            &empty_secrets(),
            &with_oauth,
        );
        assert!(r.is_ok(), "翻得到 OAuth 就该能用");
    }

    /// 核心决定钉在这——之前 7 个测试全绿，但没有一个同时给 key 和
    /// OAuth：把 `select_credential` 里的 `.or_else()` 顺序反过来，
    /// 那 7 个测试照样全过。这条专门堵这个回归。见 review IMPORTANT (a)。
    #[test]
    fn an_explicit_key_outranks_an_oauth_token_found_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let mut secrets = SecretStore::load(&dir.path().join("secrets.toml"));
        secrets.set("kimi", "sk-explicit-key").unwrap();
        let with_oauth = |_: &str| Some(Credential::Bearer("oauth-token".into()));

        let cred = select_credential("kimi", &secrets, &with_oauth).unwrap();

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
            &|_: &str| Some(Credential::Bearer("t".into())),
        );
        assert!(matches!(r, Err(ResolveError::NoApiEndpoint(ref n)) if n == "claude"));
    }

    /// 直连端点存在、凭据也有，但没写型号——瞎猜一个默认模型（比如写死
    /// claude-3-5-sonnet）在非 Anthropic 端点上会稳定 404。
    #[test]
    fn http_without_a_model_is_refused_instead_of_guessing_one() {
        let mut c = cfg("kimi", Transport::Http);
        c.model = None;
        let with_oauth = |_: &str| Some(Credential::Bearer("t".into()));
        let r = resolve(&c, &builtin, &empty_secrets(), &with_oauth);
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

    #[test]
    fn every_error_explains_itself_in_plain_chinese_with_a_real_next_step() {
        // 用户是零编程经验的人：不许夹带内部字段名/类型名，每句话都要
        // 点名是哪个厂商、并且给一句他真做得到的下一步。见 review
        // IMPORTANT (c)：原来的测试只查长度和「不含 Error」，
        // "x xxxxxxxxx" 这种字符串也能混过去——这里换成查具体禁词
        // （大小写不敏感）+ 查真实存在的动作词。
        let all = [
            ResolveError::NoSuchProvider("x".into()),
            ResolveError::NoHeadlessCommand("x".into()),
            ResolveError::NoApiEndpoint("x".into()),
            ResolveError::NoCredential("x".into()),
            ResolveError::NoModel("x".into()),
        ];
        for e in &all {
            let s = describe(e);
            assert!(s.contains('x'), "要点名是哪个厂商: {s}");
            assert!(s.chars().count() > 8, "太短，说不清楚: {s}");
            for jargon in ["provider", "transport", "cli", "agent", "error"] {
                assert!(
                    !s.to_lowercase().contains(jargon),
                    "不能夹带内部字段名/类型名「{jargon}」，那是代码里的词不是用户的词: {s}"
                );
            }
        }
        // 前三条都指向同一个真实存在的动作——去改设置文件；
        // 第四条指向真实存在的按键（按 c 填密钥）。
        assert!(describe(&ResolveError::NoSuchProvider("x".into())).contains("设置文件"));
        assert!(describe(&ResolveError::NoHeadlessCommand("x".into())).contains("设置文件"));
        assert!(describe(&ResolveError::NoApiEndpoint("x".into())).contains("设置文件"));
        assert!(describe(&ResolveError::NoModel("x".into())).contains("设置文件"));
        assert!(
            describe(&ResolveError::NoCredential("x".into())).contains("按 c"),
            "要指向真实存在的那个按键"
        );
    }
}
