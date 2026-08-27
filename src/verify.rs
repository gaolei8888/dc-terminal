use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 探测请求的超时（TCP 建连 + 请求/响应）为 4 秒，在 `client::READ_TIMEOUT`（5 秒）之内。
/// 守护进程在这里等多久，界面那条连接就等多久，
/// 超过 5 秒界面会判定连接错位并丢弃重连，用户看到的是「连不上守护进程」。
///
/// 这个预算必须同时喂给 `.timeout()` 和 `.timeout_connect()`（见 `build_probe_agent`）：
/// ureq 的 `AgentBuilder` 默认把 `timeout_connect` 设成 30 秒，且建连阶段优先认它而不是
/// `.timeout()` 的整体截止时间（`ureq-2.12.1/src/stream.rs` `connect_host`）。两个字段
/// 共用同一个 `PROBE_TIMEOUT` 不会把预算翻倍——ureq 内部对整条请求只算一个起点相同的
/// `Instant` 截止时间，建连阶段跑掉的时间会从后续读写阶段的剩余预算里扣。
///
/// **DNS 查询不被这个超时保护**。ureq 2.12.1 的 `stream.rs:364` 无法为 DNS 设置截止时间
/// （代码注释里有 TODO），所以如果 resolver 响应缓慢或 UDP 丢包，发送可能会卡超过 5 秒。
/// 为了完全挡住这个风险需要实现自定义 `Resolver`，但实际好处有限：(1) resolver 本身有超时，
/// 不会无限卡；(2) UI 在独立后台线程验证（见 Task 11 设计），不会冻结主界面，最坏情况是
/// 这条后台连接超时、用户刷新再试。
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

/// 探测用的 `ureq::Agent`。单独拆出来是为了让 `.timeout_connect()` 这类
/// 配置能在不发真实请求的前提下被测试用 `Debug` 输出核实到——
/// `send_probe` 本身不能被测试调用（会打真网络），但构建 `Agent` 这一步
/// 不涉及任何 I/O。
fn build_probe_agent() -> ureq::Agent {
    crate::sys::tls::agent_builder()
        .timeout(PROBE_TIMEOUT)
        .timeout_connect(PROBE_TIMEOUT)
        .build()
}

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
    let resp = build_probe_agent()
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

    /// 建 `Agent` 不发请求，不碰网络——`ureq::Agent` 派生了 `Debug`，
    /// 内部的 `AgentConfig` 字段虽是 `pub(crate)`，但会原样进 `Debug`
    /// 输出，所以能在不打真网络的前提下核实 `timeout_connect` 真的被
    /// 设置了，而不只是相信注释。这就是本次修复要守住的回归点：
    /// 之前只设了 `.timeout()`，建连阶段会退回 ureq 默认的 30 秒。
    #[test]
    fn probe_agent_bounds_the_connect_phase_too() {
        let debug = format!("{:?}", build_probe_agent());
        assert!(
            debug.contains("timeout_connect: Some(4s)"),
            "connect 阶段没有被 PROBE_TIMEOUT 兜住，可能退回 ureq 默认的 30 秒: {debug}"
        );
        assert!(
            debug.contains("timeout: Some(4s)"),
            "整体超时没有被设置: {debug}"
        );
    }
}
