### Task 6: `HttpBackend`——直连 OpenAI / Anthropic 兼容端点

**Files:**
- Create: `src/llm/http.rs`
- Modify: `src/llm/mod.rs`（加 `pub mod http;`）

**Interfaces:**
- Consumes: `llm::{Backend, Prompt, LlmError}`、`llm::creds::Credential`、`profile::Wire`
- Produces:
  - `llm::http::Sender`：类型别名 `dyn Fn(&str, &Credential, &serde_json::Value) -> Result<(u16, String), String> + Send + Sync`
  - `llm::http::body_for(wire: Wire, model: &str, p: &Prompt) -> serde_json::Value`
  - `llm::http::extract_text(wire: Wire, body: &str) -> Option<String>`
  - `llm::http::HttpBackend { url, wire, model, cred, sender }`
  - `HttpBackend::new(url: String, wire: Wire, model: String, cred: Credential) -> HttpBackend`
  - `HttpBackend::with_sender(url: String, wire: Wire, model: String, cred: Credential, sender: Arc<Sender>) -> HttpBackend`

**说明：** 传输层注入，与 `verify.rs` 的 `verify_with` 是同一套路——401 / 超时 / 输出畸形全都能不打网络地测。超时沿用 `verify.rs` 的做法：`.timeout()` 和 `.timeout_connect()` **都要设**，只设前者会退回 ureq 默认的 30 秒（那个坑 `verify.rs` 已经踩过一次，注释在那儿）。

- [ ] **Step 1: 写失败的测试**

`src/llm/http.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> Prompt {
        Prompt { system: "s".into(), user: "u".into(), max_tokens: 128 }
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
            extract_text(Wire::Openai, r#"{"choices":[{"message":{"content":"答案"}}]}"#).as_deref(),
            Some("答案")
        );
        assert_eq!(
            extract_text(Wire::Anthropic, r#"{"content":[{"type":"text","text":"答案"}]}"#).as_deref(),
            Some("答案")
        );
    }

    #[test]
    fn unreadable_responses_yield_none_for_every_wire() {
        for w in [Wire::Openai, Wire::Anthropic] {
            for body in ["", "not json", "{}", r#"{"choices":[]}"#, r#"{"content":[]}"#] {
                assert!(extract_text(w, body).is_none(), "{w:?} 该读不出来: {body}");
            }
        }
    }

    #[test]
    fn a_401_is_unavailable_not_a_panic() {
        let b = HttpBackend::with_sender(
            "https://x/v1".into(), Wire::Openai, "m".into(), Credential::Key("k".into()),
            Arc::new(|_, _, _| Ok((401, "unauthorized".into()))),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Unavailable));
    }

    #[test]
    fn a_network_failure_is_unavailable() {
        let b = HttpBackend::with_sender(
            "https://x/v1".into(), Wire::Openai, "m".into(), Credential::Key("k".into()),
            Arc::new(|_, _, _| Err("connection refused".into())),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Unavailable));
    }

    #[test]
    fn a_200_with_unreadable_body_is_malformed() {
        // 读不懂 = 没把握。绝不猜一个答案出来。
        let b = HttpBackend::with_sender(
            "https://x/v1".into(), Wire::Openai, "m".into(), Credential::Key("k".into()),
            Arc::new(|_, _, _| Ok((200, "{\"unexpected\":1}".into()))),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Malformed));
    }

    #[test]
    fn a_good_response_comes_back_trimmed() {
        let b = HttpBackend::with_sender(
            "https://x/v1".into(), Wire::Anthropic, "m".into(), Credential::Bearer("t".into()),
            Arc::new(|_, _, _| Ok((200, r#"{"content":[{"type":"text","text":" 好了 \n"}]}"#.into()))),
        );
        assert_eq!(b.complete(&p()), Ok("好了".to_string()));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib llm::http`
Expected: 编译失败，`unresolved module 'http'`

- [ ] **Step 3: 写实现**

`src/llm/http.rs`：

```rust
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
        Wire::Openai => v.get("choices")?.get(0)?.get("message")?.get("content")?.as_str()?,
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
        HttpBackend { url, wire, model, cred, sender: Arc::new(send_real) }
    }

    pub fn with_sender(
        url: String,
        wire: Wire,
        model: String,
        cred: Credential,
        sender: Arc<Sender>,
    ) -> HttpBackend {
        HttpBackend { url, wire, model, cred, sender }
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

/// 真实传输。**没有单元测试覆盖**，在实测那一步验。
fn send_real(
    url: &str,
    cred: &Credential,
    body: &serde_json::Value,
) -> Result<(u16, String), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .timeout_connect(HTTP_TIMEOUT)
        .build();
    let mut req = agent.post(url).set("content-type", "application/json");
    match cred {
        Credential::Key(k) => {
            req = req.set("x-api-key", k).set("authorization", &format!("Bearer {k}"));
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
```

`src/llm/mod.rs` 加 `pub mod http;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib llm::http`
Expected: 8 passed

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && git add src/llm/http.rs src/llm/mod.rs
git commit -m "feat(llm): add an HTTP backend for OpenAI- and Anthropic-shaped endpoints

Transport is injected the way verify.rs already does it, so 401s, network
failures, and unreadable bodies are all covered without touching the network.

Both .timeout() and .timeout_connect() are set: setting only the former falls
back to ureq's 30s connect default, a trap verify.rs already documented."
```

---

