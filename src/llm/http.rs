//! 直连 OpenAI / Anthropic 兼容端点。
//!
//! 传输层注入，与 `verify.rs` 的 `verify_with` 是同一套路：401 / 网络故障 /
//! 输出畸形全都能不打网络地测。

use super::creds::Credential;
use super::{Backend, LlmError, Prompt};
use crate::profile::Wire;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// url + 凭据 + 请求体 → (状态码, 响应体)
pub type Sender =
    dyn Fn(&str, &Credential, &serde_json::Value) -> Result<(u16, String), String> + Send + Sync;

/// 与 `verify::PROBE_TIMEOUT` 同一个理由：**`.timeout()` 和
/// `.timeout_connect()` 都要设**，只设前者会退回 ureq 默认的 30 秒。
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

pub fn body_for(wire: Wire, model: &str, p: &Prompt) -> serde_json::Value {
    match wire {
        Wire::Openai => json!({
            "model": model,
            "max_tokens": p.max_tokens,
            "messages": [
                {"role": "system", "content": p.system},
                {"role": "user", "content": p.user},
            ],
        }),
        // Anthropic 的 system 是**顶层字段**，不是一条 message。放错位置
        // 端点不报错，只会安静忽略——所以有一条测试专门盯着。
        Wire::Anthropic => json!({
            "model": model,
            "max_tokens": p.max_tokens,
            "system": p.system,
            "messages": [{"role": "user", "content": p.user}],
        }),
    }
}

pub fn extract_text(wire: Wire, body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let s = match wire {
        Wire::Openai => v
            .get("choices")?
            .get(0)?
            .get("message")?
            .get("content")?
            .as_str()?,
        Wire::Anthropic => v.get("content")?.get(0)?.get("text")?.as_str()?,
    };
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    Some(t.to_string())
}

pub struct HttpBackend {
    url: String,
    wire: Wire,
    model: String,
    cred: Credential,
    sender: Arc<Sender>,
}

impl HttpBackend {
    pub fn new(url: String, wire: Wire, model: String, cred: Credential) -> HttpBackend {
        HttpBackend {
            url,
            wire,
            model,
            cred,
            sender: Arc::new(send_real),
        }
    }

    pub fn with_sender(
        url: String,
        wire: Wire,
        model: String,
        cred: Credential,
        sender: Arc<Sender>,
    ) -> HttpBackend {
        HttpBackend {
            url,
            wire,
            model,
            cred,
            sender,
        }
    }
}

impl Backend for HttpBackend {
    fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
        let body = body_for(self.wire, &self.model, p);
        let (status, text) = (self.sender)(&self.url, &self.cred, &body).map_err(|e| {
            eprintln!("LLM HTTP 调用失败：{e}");
            LlmError::Unavailable
        })?;
        if !(200..300).contains(&status) {
            eprintln!("LLM HTTP 返回 {status}");
            return Err(LlmError::Unavailable);
        }
        // 读不懂 = 没把握。绝不猜一个答案出来。
        extract_text(self.wire, &text).ok_or(LlmError::Malformed)
    }
}

/// 直连用的 `ureq::Agent`。单独拆出来是为了让 `.timeout_connect()` 这类
/// 配置能在不发真实请求的前提下被测试用 `Debug` 输出核实到——同
/// `verify.rs` 的 `build_probe_agent`：`send_real` 本身不能被测试调用
/// （会打真网络），但构建 `Agent` 这一步不涉及任何 I/O。
fn build_http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .timeout_connect(HTTP_TIMEOUT)
        .build()
}

/// 真实传输。**没有单元测试覆盖**，在实测那一步验。
fn send_real(
    url: &str,
    cred: &Credential,
    body: &serde_json::Value,
) -> Result<(u16, String), String> {
    let agent = build_http_agent();
    let mut req = agent.post(url).set("content-type", "application/json");
    match cred {
        Credential::Key(k) => {
            req = req
                .set("x-api-key", k)
                .set("authorization", &format!("Bearer {k}"));
        }
        Credential::Bearer(t) => {
            req = req.set("authorization", &format!("Bearer {t}"));
        }
        // 直连没有凭据可继承。resolve 那一层不会走到这里，兜底也不猜。
        Credential::Inherit => return Err("HTTP 直连需要凭据".into()),
    }
    req = req.set("anthropic-version", "2023-06-01");
    match req.send_json(body.clone()) {
        Ok(r) => {
            let s = r.status();
            Ok((s, r.into_string().unwrap_or_default()))
        }
        // ureq 把 4xx/5xx 也当 Err，得挑出来——它们是有效状态码，不是网络故障
        Err(ureq::Error::Status(code, r)) => Ok((code, r.into_string().unwrap_or_default())),
        Err(e) => Err(format!("{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn p() -> Prompt {
        Prompt {
            system: "s".into(),
            user: "u".into(),
            max_tokens: 128,
        }
    }

    #[test]
    fn openai_body_puts_the_system_prompt_in_messages() {
        let b = body_for(Wire::Openai, "gpt-x", &p());
        assert_eq!(b["model"], "gpt-x");
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][0]["content"], "s");
        assert_eq!(b["messages"][1]["role"], "user");
        assert_eq!(b["messages"][1]["content"], "u");
    }

    #[test]
    fn anthropic_body_puts_the_system_prompt_at_top_level() {
        // Anthropic 的 system 是顶层字段，不是一条 message。放错位置
        // 端点不会报错，只会安静地忽略它——所以必须有测试盯着。
        let b = body_for(Wire::Anthropic, "claude-x", &p());
        assert_eq!(b["model"], "claude-x");
        assert_eq!(b["system"], "s");
        assert_eq!(b["max_tokens"], 128);
        assert_eq!(b["messages"][0]["role"], "user");
        assert!(b["messages"][0].get("system").is_none());
    }

    #[test]
    fn extracts_text_from_each_wire_format() {
        assert_eq!(
            extract_text(
                Wire::Openai,
                r#"{"choices":[{"message":{"content":"答案"}}]}"#
            )
            .as_deref(),
            Some("答案")
        );
        assert_eq!(
            extract_text(
                Wire::Anthropic,
                r#"{"content":[{"type":"text","text":"答案"}]}"#
            )
            .as_deref(),
            Some("答案")
        );
    }

    #[test]
    fn unreadable_responses_yield_none_for_every_wire() {
        for w in [Wire::Openai, Wire::Anthropic] {
            for body in [
                "",
                "not json",
                "{}",
                r#"{"choices":[]}"#,
                r#"{"content":[]}"#,
            ] {
                assert!(extract_text(w, body).is_none(), "{w:?} 该读不出来: {body}");
            }
        }
    }

    #[test]
    fn a_401_is_unavailable_not_a_panic() {
        let b = HttpBackend::with_sender(
            "https://x/v1".into(),
            Wire::Openai,
            "m".into(),
            Credential::Key("k".into()),
            Arc::new(|_, _, _| Ok((401, "unauthorized".into()))),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Unavailable));
    }

    #[test]
    fn a_network_failure_is_unavailable() {
        let b = HttpBackend::with_sender(
            "https://x/v1".into(),
            Wire::Openai,
            "m".into(),
            Credential::Key("k".into()),
            Arc::new(|_, _, _| Err("connection refused".into())),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Unavailable));
    }

    #[test]
    fn a_200_with_unreadable_body_is_malformed() {
        // 读不懂 = 没把握。绝不猜一个答案出来。
        let b = HttpBackend::with_sender(
            "https://x/v1".into(),
            Wire::Openai,
            "m".into(),
            Credential::Key("k".into()),
            Arc::new(|_, _, _| Ok((200, "{\"unexpected\":1}".into()))),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Malformed));
    }

    #[test]
    fn a_good_response_comes_back_trimmed() {
        let b = HttpBackend::with_sender(
            "https://x/v1".into(),
            Wire::Anthropic,
            "m".into(),
            Credential::Bearer("t".into()),
            Arc::new(|_, _, _| {
                Ok((
                    200,
                    r#"{"content":[{"type":"text","text":" 好了 \n"}]}"#.into(),
                ))
            }),
        );
        assert_eq!(b.complete(&p()), Ok("好了".to_string()));
    }

    /// 建 `Agent` 不发请求，不碰网络——同 `verify.rs` 的
    /// `probe_agent_bounds_the_connect_phase_too`，能在不打真网络的前提下
    /// 核实 `timeout_connect` 真的被设置了，而不只是相信注释。这就是
    /// 本次要守住的回归点：之前只设了 `.timeout()`，建连阶段会退回 ureq
    /// 默认的 30 秒；`send_real` 本身没有单测覆盖，所以这个防护必须扎在
    /// 单独拆出来的 `build_http_agent` 上。
    #[test]
    fn http_agent_bounds_the_connect_phase_too() {
        let debug = format!("{:?}", build_http_agent());
        assert!(
            debug.contains("timeout_connect: Some(20s)"),
            "connect 阶段没有被 HTTP_TIMEOUT 兜住，可能退回 ureq 默认的 30 秒: {debug}"
        );
        assert!(
            debug.contains("timeout: Some(20s)"),
            "整体超时没有被设置: {debug}"
        );
    }

    /// `body_for` 单测只能证明这个函数本身对，证不了 `complete()` 真的把
    /// `self.wire`（而不是写死的某个值）传给了它。这条测试盯住 `complete()`
    /// 实际调用 sender 时传出去的 url / 凭据 / 请求体，同 `verify.rs` 的
    /// `the_key_reaches_the_transport`。
    ///
    /// 用 `Wire::Anthropic` 构造是关键：断言请求体有一个**顶层** `system`
    /// 字段，才是真正能抓住「`complete()` 里把 wire 写死成 `Openai`」这类
    /// 回归的地方——那种改动不会碰到 `body_for` 自己的单测，只会在这个
    /// 接缝上露出来。
    #[test]
    fn the_backend_sends_its_own_url_wire_and_credential() {
        let seen: Arc<Mutex<(String, Credential, serde_json::Value)>> = Arc::new(Mutex::new((
            String::new(),
            Credential::Inherit,
            serde_json::Value::Null,
        )));
        let sink = seen.clone();
        let b = HttpBackend::with_sender(
            "https://x/v1/messages".into(),
            Wire::Anthropic,
            "m".into(),
            Credential::Key("sk-abc".into()),
            Arc::new(move |url, cred, body| {
                *sink.lock().unwrap() = (url.to_string(), cred.clone(), body.clone());
                Ok((200, r#"{"content":[{"type":"text","text":"ok"}]}"#.into()))
            }),
        );
        b.complete(&p()).unwrap();
        let (url, cred, body) = seen.lock().unwrap().clone();
        assert_eq!(url, "https://x/v1/messages");
        // `Credential` 的 `Debug` 是刻意打码的——比较用 `==`（`Credential`
        // 派生了 `PartialEq`），断言失败信息里绝不能把明文格式化出来。
        assert_eq!(cred, Credential::Key("sk-abc".into()));
        assert_eq!(body["system"], "s");
        assert!(body["messages"][0].get("system").is_none());
    }
}
