//! 从配置 + profile + 密钥里组装出一个后端。
//!
//! **凭据顺序在这里定死**：先用户显式填的 key，再从别人家里翻出来的 OAuth。
//! 理由是用户填过的东西优先于我们替他猜的东西。

use super::cli::CliBackend;
use super::creds::Credential;
use super::http::HttpBackend;
use super::Backend;
use crate::config::{Config, Transport};
use crate::profile::Profile;
use crate::secrets::SecretStore;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    NoSuchProvider(String),
    NoHeadlessCommand(String),
    NoApiEndpoint(String),
    NoCredential(String),
}

/// 用户看得懂、且说得出下一步的一句话。
pub fn describe(e: &ResolveError) -> String {
    match e {
        ResolveError::NoSuchProvider(n) => {
            format!("找不到叫「{n}」的 agent。请在配置里把 provider 改成一个已有的名字。")
        }
        ResolveError::NoHeadlessCommand(n) => {
            format!("「{n}」还不支持在后台单独回答问题。请换一个 provider，比如 claude。")
        }
        ResolveError::NoApiEndpoint(n) => {
            format!("「{n}」没有可直连的服务地址。把配置里的 transport 改回 cli 即可。")
        }
        ResolveError::NoCredential(n) => {
            format!("「{n}」还没有密钥。在主界面按 c 填一个，或把 transport 改回 cli。")
        }
    }
}

pub fn resolve(
    cfg: &Config,
    lookup: &dyn Fn(&str) -> Option<Profile>,
    secrets: &SecretStore,
    oauth: &dyn Fn(&str) -> Option<Credential>,
) -> Result<Arc<dyn Backend>, ResolveError> {
    let name = cfg.llm.provider.as_str();
    let p = lookup(name).ok_or_else(|| ResolveError::NoSuchProvider(name.to_string()))?;

    match cfg.llm.transport {
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
            let cred = secrets
                .get(name)
                .map(|k| Credential::Key(k.to_string()))
                .or_else(|| oauth(name))
                .ok_or_else(|| ResolveError::NoCredential(name.to_string()))?;
            let base = cfg
                .llm
                .base_url
                .clone()
                .unwrap_or_else(|| api.base_url.clone());
            let url = format!("{}/v1/messages", base.trim_end_matches('/'));
            let model = cfg
                .llm
                .model
                .clone()
                .unwrap_or_else(|| "claude-3-5-sonnet".to_string());
            Ok(Arc::new(HttpBackend::new(url, api.wire, model, cred)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, LlmConfig, Transport};

    fn cfg(provider: &str, transport: Transport) -> Config {
        Config {
            llm: LlmConfig {
                provider: provider.into(),
                model: Some("m".into()),
                base_url: None,
                transport,
            },
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

    #[test]
    fn a_profile_without_a_headless_command_is_refused_by_name() {
        // opencode 没验过无界面模式，不许假装能用。
        let e = resolve(
            &cfg("opencode", Transport::Cli),
            &builtin,
            &empty_secrets(),
            &no_oauth,
        )
        .unwrap_err();
        assert_eq!(e, ResolveError::NoHeadlessCommand("opencode".into()));
    }

    #[test]
    fn an_unknown_provider_is_named_in_the_error() {
        let e = resolve(
            &cfg("nope", Transport::Cli),
            &builtin,
            &empty_secrets(),
            &no_oauth,
        )
        .unwrap_err();
        assert_eq!(e, ResolveError::NoSuchProvider("nope".into()));
    }

    #[test]
    fn http_without_any_credential_is_refused() {
        let e = resolve(
            &cfg("kimi", Transport::Http),
            &builtin,
            &empty_secrets(),
            &no_oauth,
        )
        .unwrap_err();
        assert_eq!(e, ResolveError::NoCredential("kimi".into()));
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

    #[test]
    fn http_needs_an_api_block() {
        // claude 没有 [api]（它走官方端点，靠 CLI 自己登录），直连没地方去。
        let e = resolve(
            &cfg("claude", Transport::Http),
            &builtin,
            &empty_secrets(),
            &|_: &str| Some(Credential::Bearer("t".into())),
        )
        .unwrap_err();
        assert_eq!(e, ResolveError::NoApiEndpoint("claude".into()));
    }

    #[test]
    fn every_error_explains_itself_in_a_self_contained_sentence() {
        // 用户是零编程经验的人。每一句都得说得出下一步，且不带栈追踪腔。
        let all = [
            ResolveError::NoSuchProvider("x".into()),
            ResolveError::NoHeadlessCommand("x".into()),
            ResolveError::NoApiEndpoint("x".into()),
            ResolveError::NoCredential("x".into()),
        ];
        for e in all {
            let s = describe(&e);
            assert!(s.contains('x'), "要点名是哪个 provider: {s}");
            assert!(s.chars().count() > 8, "太短，说不清楚: {s}");
            assert!(!s.contains("Error"), "别把类型名漏给用户: {s}");
        }
    }
}
