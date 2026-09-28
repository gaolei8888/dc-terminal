//! 拿学生的 dc-llm `api_key` 跟网关换一张中转令牌。
//!
//! **这个文件不碰网络。** 传输层由调用方注入——理由跟 `pair_http.rs`、
//! `verify.rs::verify_with` 一样：测试要能覆盖 401、404、400、网络错，
//! 不用打真网络。
//!
//! 契约见 `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`
//! 末尾「附录：网关签中转令牌（冻结的线上契约）」。**状态码、字段名、
//! 错误文案以那一节为准**，网关那边（dc_llm 仓）是照着它实现的，这里
//! 改一个字都会让两个仓库对不上。
//!
//! **`api_key` 只走 `Authorization: Bearer` 头，永远不进请求体。** 它是
//! 网关那把钥匙，中转认的是这里换来的令牌，两把钥匙的用途、撤销方式、
//! 泄漏后果都不一样——同 `secrets.rs` 里那些保留键上反复出现的那条理由。

use serde::Deserialize;

/// `secrets.toml` 里存中转令牌的键，跟 `PHONE_TOKEN_KEY` / `WEB_TOKEN_KEY`
/// 一类，是一个 profile 不可能占用的名字，不会出现在密钥页（`c`）里。
pub const RELAY_TOKEN_KEY: &str = "__relay__";

/// 令牌的过期时间（unix 秒，十进制字符串），存在 `RELAY_TOKEN_KEY` 旁边。
pub const RELAY_TOKEN_EXP_KEY: &str = "__relay_exp__";

/// 一天的秒数。`needs_renewal` 的阈值。
const ONE_DAY_SECS: u64 = 24 * 60 * 60;

#[derive(Deserialize)]
struct TokenResponse {
    token: String,
    exp: u64,
}

/// `send` 的形状：url、Bearer、JSON 请求体 → 状态码、响应体。只是给
/// `fetch_token` 的签名起个名字（clippy 嫌三个 `&str` 参数的裸 `dyn Fn`
/// 太复杂，且这个名字不能叫 `Send`——那会挡住 `std::marker::Send`），
/// 类型本身跟契约里写的完全一样，不是另一套东西。
pub type Transport<'a> = &'a dyn Fn(&str, &str, &str) -> Result<(u16, String), String>;

/// 跟网关换一张中转令牌。
///
/// `send` 的三个参数依次是 url、Bearer（也就是 `api_key` 本身，调用方拼
/// `Authorization: Bearer <bearer>`）、JSON 请求体；返回状态码和响应体。
/// 生产路径传一个真打 `ureq` 的闭包（在后续任务里，`login` 目前只有纯
/// 状态机这一半），测试传一个记录调用、返回预设结果的假闭包。
///
/// 成功返回 `(token, exp)`。失败返回**给学生看的中文**，不是错误码——
/// 这个模块今天没有自己的 i18n key，往后接 TUI 时再决定要不要挪过去
/// （附录里原话）。
pub fn fetch_token(
    origin: &str,
    api_key: &str,
    endpoint: &str,
    send: Transport,
) -> Result<(String, u64), String> {
    let url = format!("{}/admin/api/relay/token", origin.trim_end_matches('/'));
    let body = serde_json::json!({ "endpoint": endpoint }).to_string();

    let (status, resp_body) =
        send(&url, api_key, &body).map_err(|_| "连不上网关，检查一下网络，然后重试".to_string())?;

    match status {
        200 => serde_json::from_str::<TokenResponse>(&resp_body)
            .map(|r| (r.token, r.exp))
            .map_err(|_| "网关返回的数据看不懂，请稍后再试".to_string()),
        401 => Err("登录已失效，请先在 dct 里重新配对 DC 账号".to_string()),
        404 => Err("服务器还没开放多电脑功能".to_string()),
        // 正常路径走不到：`endpoint` 由 dct 自己从本机签名公钥算出，恒
        // 合法（见附录）。这一支是网关对输入的防御，这里兜住不 panic。
        400 => Err("这台电脑的身份码不合法，没法申请多电脑令牌".to_string()),
        other => Err(format!("网关出错了（{other}），请稍后再试")),
    }
}

/// 令牌剩不到 1 天（86400 秒）就该续期。`exp <= now`（已经过期）也算数——
/// `saturating_sub` 把它归到「剩 0 秒」，同样小于一天。
pub fn needs_renewal(exp: u64, now: u64) -> bool {
    exp.saturating_sub(now) < ONE_DAY_SECS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 记录最后一次调用的 `(url, bearer, body)`，返回预设的 `(status, body)`。
    struct FakeSend {
        calls: RefCell<Vec<(String, String, String)>>,
        reply: Result<(u16, String), String>,
    }

    impl FakeSend {
        fn returning(reply: Result<(u16, String), String>) -> FakeSend {
            FakeSend {
                calls: RefCell::new(Vec::new()),
                reply,
            }
        }

        fn as_fn(&self) -> impl Fn(&str, &str, &str) -> Result<(u16, String), String> + '_ {
            move |url: &str, bearer: &str, body: &str| {
                self.calls.borrow_mut().push((
                    url.to_string(),
                    bearer.to_string(),
                    body.to_string(),
                ));
                self.reply.clone()
            }
        }

        fn last_call(&self) -> (String, String, String) {
            self.calls.borrow().last().cloned().expect("send 没被调用")
        }
    }

    #[test]
    fn a_200_returns_the_token_and_its_expiry() {
        let fake = FakeSend::returning(Ok((200, r#"{"token":"tok-abc","exp":1700000000}"#.into())));
        let got = fetch_token("https://gw.example", "sk-live", "c-aaaa", &fake.as_fn());
        assert_eq!(got, Ok(("tok-abc".to_string(), 1700000000)));
    }

    #[test]
    fn the_url_is_built_from_the_origin_with_no_double_slash() {
        let fake = FakeSend::returning(Ok((200, r#"{"token":"t","exp":1}"#.into())));
        fetch_token("https://gw.example/", "sk-live", "c-aaaa", &fake.as_fn())
            .expect("200 应该成功");
        let (url, ..) = fake.last_call();
        assert_eq!(url, "https://gw.example/admin/api/relay/token");
    }

    #[test]
    fn a_401_means_the_login_has_gone_stale() {
        let fake = FakeSend::returning(Ok((401, r#"{"error":"invalid_api_key"}"#.into())));
        let got = fetch_token("https://gw.example", "sk-dead", "c-aaaa", &fake.as_fn());
        assert_eq!(
            got,
            Err("登录已失效，请先在 dct 里重新配对 DC 账号".to_string())
        );
    }

    #[test]
    fn a_404_means_the_gateway_has_multi_machine_switched_off() {
        let fake = FakeSend::returning(Ok((404, String::new())));
        let got = fetch_token("https://gw.example", "sk-live", "c-aaaa", &fake.as_fn());
        assert_eq!(got, Err("服务器还没开放多电脑功能".to_string()));
    }

    #[test]
    fn a_400_means_the_endpoint_was_rejected() {
        let fake = FakeSend::returning(Ok((400, String::new())));
        let got = fetch_token(
            "https://gw.example",
            "sk-live",
            "not-an-endpoint",
            &fake.as_fn(),
        );
        assert_eq!(
            got,
            Err("这台电脑的身份码不合法，没法申请多电脑令牌".to_string())
        );
    }

    #[test]
    fn a_network_failure_is_reported_in_chinese_too() {
        let fake = FakeSend::returning(Err("connection refused".into()));
        let got = fetch_token("https://gw.example", "sk-live", "c-aaaa", &fake.as_fn());
        assert_eq!(got, Err("连不上网关，检查一下网络，然后重试".to_string()));
    }

    #[test]
    fn an_unexpected_status_code_is_reported_in_chinese_not_as_a_raw_number() {
        let fake = FakeSend::returning(Ok((500, "internal error".into())));
        let got = fetch_token("https://gw.example", "sk-live", "c-aaaa", &fake.as_fn());
        assert_eq!(got, Err("网关出错了（500），请稍后再试".to_string()));
    }

    #[test]
    fn a_garbage_200_body_does_not_panic_and_is_reported_in_chinese() {
        let fake = FakeSend::returning(Ok((200, "not json".into())));
        let got = fetch_token("https://gw.example", "sk-live", "c-aaaa", &fake.as_fn());
        assert_eq!(got, Err("网关返回的数据看不懂，请稍后再试".to_string()));
    }

    /// 核心的安全属性：`api_key` 只能出现在 Bearer 槽位里，请求体里一个
    /// 字符都不能有——中转往后只看得到令牌，看不到这把钥匙本身。
    #[test]
    fn the_api_key_only_appears_in_the_bearer_slot_never_in_the_body() {
        let fake = FakeSend::returning(Ok((200, r#"{"token":"t","exp":1}"#.into())));
        let api_key = "sk-super-secret-do-not-leak";
        fetch_token("https://gw.example", api_key, "c-aaaa", &fake.as_fn()).expect("200 应该成功");
        let (_, bearer, body) = fake.last_call();
        assert_eq!(bearer, api_key);
        assert!(!body.contains(api_key), "api_key 泄漏进了请求体：{body}");
        assert_eq!(body, r#"{"endpoint":"c-aaaa"}"#);
    }

    #[test]
    fn needs_renewal_is_false_with_exactly_one_day_left() {
        assert!(!needs_renewal(ONE_DAY_SECS, 0));
    }

    #[test]
    fn needs_renewal_is_true_one_second_short_of_a_day() {
        assert!(needs_renewal(ONE_DAY_SECS - 1, 0));
    }

    #[test]
    fn needs_renewal_is_true_once_already_expired() {
        assert!(needs_renewal(100, 200));
    }

    #[test]
    fn needs_renewal_is_false_with_plenty_of_time_left() {
        assert!(!needs_renewal(1_000_000, 0));
    }
}
