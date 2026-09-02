//! 配对的真传输层。判定逻辑在 `pair.rs`，这里只管把 HTTP 变成 `pair::Poll`。

use crate::pair::{Poll, Started};
use serde_json::Value;
use std::time::Duration;

/// 单次请求的预算，4 秒，跟 `verify::PROBE_TIMEOUT` 同一个数字、同一个理由。
///
/// **`/pair/start` 是同步的：界面那条连接就在上面等着。** `daemon::handle`
/// 的 `PairStart` 分支在回话之前先真打一次 `/admin/api/pair/start`，而界面
/// 那条连接 5 秒就超时（`client::READ_TIMEOUT`）。8 秒的预算意味着一个慢
/// 网络下界面先判定「守护进程没响应」、屏上给出一个可以按 `r` 的失败，而
/// 守护进程还在那条请求上等着，随后照样起一条轮询线程——学生按 `r`，
/// 于是有两条。教室网络正是这份 spec 写给的那批人所在的网络，这不是边角
/// 情况。压进 5 秒预算之内，失败就在界面还在听的时候到达。
///
/// 轮询那一半（`poll`）跑在 daemon 的后台线程里，本来没有这个约束，但也
/// 没有理由要更长：`Machine` 的 `interval` 是 3 秒，一次比一轮间隔还久的
/// 请求只会让退避的节奏变形。两处共用一个数字，少一个会漂开的常量。
///
/// 跟 `verify` 那边一样，这个预算必须同时喂给 `.timeout()` 和
/// `.timeout_connect()`——ureq 的默认建连超时是 30 秒，且建连阶段优先认它
/// （理由的完整版在 `verify::PROBE_TIMEOUT` 上）。
const TIMEOUT: Duration = Duration::from_secs(4);

pub fn agent() -> ureq::Agent {
    crate::sys::tls::agent_builder()
        .timeout(TIMEOUT)
        .timeout_connect(TIMEOUT)
        .build()
}

pub fn start(origin: &str, agent: &ureq::Agent) -> Result<Started, String> {
    let url = format!("{}/admin/api/pair/start", origin.trim_end_matches('/'));
    let resp = agent
        .post(&url)
        .set("content-type", "application/json")
        .set("user-agent", &crate::pair::user_agent())
        .send_json(serde_json::json!({
            "client": "dct",
            "version": env!("CARGO_PKG_VERSION"),
        }));
    match resp {
        Ok(r) => r
            .into_json::<Started>()
            .map_err(|e| format!("bad_start_body: {e}")),
        Err(ureq::Error::Status(404, _)) => Err("not_enabled".into()),
        Err(ureq::Error::Status(429, _)) => Err("rate_limited".into()),
        Err(e) => Err(format!("unreachable: {e}")),
    }
}

pub fn poll(origin: &str, device_code: &str, agent: &ureq::Agent) -> Result<Poll, String> {
    let url = format!("{}/admin/api/pair/poll", origin.trim_end_matches('/'));
    let resp = agent
        .post(&url)
        .set("content-type", "application/json")
        .set("user-agent", &crate::pair::user_agent())
        .send_json(serde_json::json!({ "device_code": device_code }));
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.into_string().unwrap_or_default();
            Ok(parse_poll(status, &body))
        }
        // ureq 把 4xx/5xx 也当 Err，它们是有效状态码不是网络故障——
        // 同 `verify::send_probe` 里那条注释。
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            Ok(parse_poll(code, &body))
        }
        Err(e) => Err(format!("{e}")),
    }
}

pub fn parse_poll(status: u16, body: &str) -> Poll {
    match status {
        404 => return Poll::NotEnabled,
        429 => return Poll::RateLimited,
        _ => {}
    }
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        // 看不懂就当还没好。当成功会写进一把不存在的钥匙，当失败会把一次
        // 正常的 502 变成学生眼里的「配对坏了」。
        Err(_) => return Poll::Pending,
    };
    match v.get("status").and_then(Value::as_str).unwrap_or("") {
        "approved" => Poll::Approved {
            api_key: v
                .get("api_key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            models: serde_json::from_value(v.get("models").cloned().unwrap_or(Value::Null))
                .unwrap_or_default(),
            platforms: serde_json::from_value(v.get("platforms").cloned().unwrap_or(Value::Null))
                .unwrap_or_default(),
            quota: serde_json::from_value(v.get("quota").cloned().unwrap_or(Value::Null)).ok(),
        },
        "denied" => Poll::Denied,
        "claimed" => Poll::Claimed,
        "expired" => Poll::Expired {
            reason: v
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("ttl")
                .to_string(),
            message: v
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        _ => Poll::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **`/pair/start` 的预算必须严格小于界面愿意等的时间。** 大于它的话，
    /// 慢网络下界面先超时、屏上给出一个能按 `r` 的失败，而守护进程还在
    /// 等那条请求、随后照样起一条轮询线程；学生按 `r` 就有了两条。
    /// 抄一个 `5` 在这里没有意义——引用 `client::READ_TIMEOUT` 本身，
    /// 改那一边的人才会看到红。
    #[test]
    fn the_start_budget_fits_inside_what_the_ui_will_wait_for() {
        assert!(
            TIMEOUT < crate::client::READ_TIMEOUT,
            "配对起步 {TIMEOUT:?} 比界面的读超时 {:?} 还长",
            crate::client::READ_TIMEOUT
        );
    }

    /// 建连阶段也要被这个预算兜住，否则会退回 ureq 默认的 30 秒——
    /// 同 `verify::probe_agent_bounds_the_connect_phase_too`，那条测试
    /// 就是从这个 bug 上长出来的。建 `Agent` 不发请求，不碰网络。
    #[test]
    fn the_agent_bounds_the_connect_phase_too() {
        let debug = format!("{:?}", agent());
        assert!(
            debug.contains("timeout_connect: Some(4s)"),
            "建连阶段没有被 TIMEOUT 兜住：{debug}"
        );
        assert!(debug.contains("timeout: Some(4s)"), "{debug}");
    }

    /// 生命周期状态一律 200 + status 字段，不看错误体。契约见 spec。
    #[test]
    fn lifecycle_states_come_back_as_200() {
        assert!(matches!(
            parse_poll(200, r#"{"status":"pending"}"#),
            crate::pair::Poll::Pending
        ));
        assert!(matches!(
            parse_poll(200, r#"{"status":"denied"}"#),
            crate::pair::Poll::Denied
        ));
        assert!(matches!(
            parse_poll(200, r#"{"status":"claimed"}"#),
            crate::pair::Poll::Claimed
        ));
    }

    /// 开关关着的时候三个接口一律 404——不存在的功能就该像不存在。
    #[test]
    fn a_404_means_pairing_is_switched_off() {
        assert!(matches!(
            parse_poll(404, "{}"),
            crate::pair::Poll::NotEnabled
        ));
    }

    #[test]
    fn a_429_is_rate_limiting_not_an_error() {
        assert!(matches!(
            parse_poll(429, ""),
            crate::pair::Poll::RateLimited
        ));
    }

    /// approved 要把 platforms 也读出来：额度窗口按平台分，
    /// 没有这张表就不知道该显示哪一个。
    #[test]
    fn approved_carries_models_and_the_platform_map() {
        let body = r#"{"status":"approved","api_key":"sk-live",
          "models":{"anthropic":{},"openai":{"default":"qwen3.8:27b"}},
          "platforms":{"qwen3.8:27b":"local"}}"#;
        match parse_poll(200, body) {
            crate::pair::Poll::Approved {
                api_key,
                models,
                platforms,
                ..
            } => {
                assert_eq!(api_key, "sk-live");
                assert_eq!(models.openai.default.as_deref(), Some("qwen3.8:27b"));
                assert_eq!(models.anthropic.default, None, "免费账号这一组是空的");
                assert_eq!(
                    platforms.get("qwen3.8:27b").map(String::as_str),
                    Some("local")
                );
            }
            other => panic!("该是 Approved，实际 {other:?}"),
        }
    }

    /// expired 的两种 reason 要原样带出来，UI 靠它分文案。
    #[test]
    fn expired_keeps_its_reason_and_message() {
        let body = r#"{"status":"expired","reason":"key_unreadable","message":"请点「重新生成」"}"#;
        match parse_poll(200, body) {
            crate::pair::Poll::Expired { reason, message } => {
                assert_eq!(reason, "key_unreadable");
                assert!(message.contains("重新生成"));
            }
            other => panic!("该是 Expired，实际 {other:?}"),
        }
    }

    /// 看不懂的 body 不许 panic，也不许当成功——当 pending 接着等就行。
    #[test]
    fn garbage_is_treated_as_pending_not_as_success() {
        assert!(matches!(
            parse_poll(200, "<html>502</html>"),
            crate::pair::Poll::Pending
        ));
    }
}
