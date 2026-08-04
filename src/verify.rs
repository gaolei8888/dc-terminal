use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 探测请求的超时。**必须小于 `client::READ_TIMEOUT`（5 秒）**：
/// 守护进程在这里等多久，界面那条连接就等多久，超过 5 秒界面会判定
/// 连接错位并丢弃重连，用户看到的是「连不上守护进程」而不是验证结果。
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifyOutcome {
    Ok,
    BadKey,
    Unreachable,
}

/// 判定逻辑本身，传输层由调用方注入——测试才能覆盖 401 / 网络错 / 奇怪返回码，
/// 而不用真打网络。
pub fn verify_with(
    url: &str,
    key: &str,
    send: &dyn Fn(&str, &str) -> Result<u16, String>,
) -> VerifyOutcome {
    match send(url, key) {
        Err(_) => VerifyOutcome::Unreachable,
        Ok(401) | Ok(403) => VerifyOutcome::BadKey,
        // 其余一律放行，见测试 `anything_else_passes` 的注释
        Ok(_) => VerifyOutcome::Ok,
    }
}

/// 真的传输层。发一个最小的 Anthropic 风格请求，只看状态码，不读 body。
///
/// `model` 随便填一个：我们不在乎它认不认这个模型（那会返回 400，属于放行），
/// 只在乎它认不认这个 key。
pub fn send_probe(url: &str, key: &str) -> Result<u16, String> {
    let body = serde_json::json!({
        "model": "probe",
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "hi"}],
    });
    let resp = ureq::AgentBuilder::new()
        .timeout(PROBE_TIMEOUT)
        .build()
        .post(url)
        .set("content-type", "application/json")
        .set("x-api-key", key)
        .set("authorization", &format!("Bearer {key}"))
        .set("anthropic-version", "2023-06-01")
        .send_json(body);

    match resp {
        Ok(r) => Ok(r.status()),
        // ureq 把 4xx/5xx 也当 Err，得挑出来——它们是有效的状态码，不是网络故障
        Err(ureq::Error::Status(code, _)) => Ok(code),
        Err(e) => Err(format!("{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthorized_means_bad_key() {
        for code in [401, 403] {
            assert_eq!(
                verify_with("u", "k", &|_, _| Ok(code)),
                VerifyOutcome::BadKey,
                "{code} 是「这个 key 不行」"
            );
        }
    }

    #[test]
    fn network_failure_is_reported_as_unreachable() {
        assert_eq!(
            verify_with("u", "k", &|_, _| Err("connection refused".into())),
            VerifyOutcome::Unreachable
        );
    }

    #[test]
    fn anything_else_passes() {
        // 刻意放行。各家 Anthropic 兼容端点行为不一，不能因为返回码奇怪
        // 就把用户拦在门外——验证的职责是抓住「key 明显是错的」，不是当网关。
        for code in [200, 400, 404, 429, 500, 502] {
            assert_eq!(
                verify_with("u", "k", &|_, _| Ok(code)),
                VerifyOutcome::Ok,
                "{code} 不该拦人"
            );
        }
    }

    #[test]
    fn the_key_reaches_the_transport() {
        let seen = std::cell::RefCell::new(String::new());
        verify_with("https://x/v1/messages", "sk-abc", &|url, key| {
            assert_eq!(url, "https://x/v1/messages");
            *seen.borrow_mut() = key.to_string();
            Ok(200)
        });
        assert_eq!(*seen.borrow(), "sk-abc");
    }
}
