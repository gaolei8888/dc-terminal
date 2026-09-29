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

/// 换令牌用的是哪个 profile 的 `api_key`：DC 配对拿到的那一把。
pub const DC_PROFILE: &str = "dc";

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

/// 跟网关换一次令牌最多等多久（整条请求）。
pub const GATEWAY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// 生产路径的 `Transport`：真打 `ureq`。**测试不走这里**（仓库规矩：测试
/// 不碰网络），`fetch_token`/`renew_if_due` 的测试都注入假的。
pub fn http_transport(url: &str, bearer: &str, body: &str) -> Result<(u16, String), String> {
    let agent = crate::sys::tls::agent_builder()
        .timeout(GATEWAY_TIMEOUT)
        .build();
    match agent
        .post(url)
        .set("Authorization", &format!("Bearer {bearer}"))
        .set("Content-Type", "application/json")
        .send_string(body)
    {
        Ok(r) => {
            let code = r.status();
            Ok((code, r.into_string().unwrap_or_default()))
        }
        Err(ureq::Error::Status(code, r)) => Ok((code, r.into_string().unwrap_or_default())),
        Err(e) => Err(e.to_string()),
    }
}

/// `renew_if_due` 做了什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Renewal {
    /// 还剩一天以上，什么都没做。
    NotDue,
    /// 换到了新令牌：内存里那一格和 secrets 都换了。
    Renewed,
    /// 该换没换成。**旧令牌原样留着**，能用到它过期为止。
    Failed(String),
}

/// 令牌剩不到一天就跟网关换一张新的。在连接线程上调（它会打网络），
/// **不在 tick 上**。
///
/// 过期时间读不到（没存、存坏了）当成已经过期：宁可多换一次，也不要拿着
/// 一张不知道什么时候作废的令牌一直用到被中转拒掉。
///
/// 打网络的那一段**不攥着 secrets 的锁**：一次 HTTP 最长十几秒，那段时间里
/// 界面问密钥页、配对落盘都要这把锁。
pub fn renew_if_due(
    secrets: &std::sync::Mutex<crate::secrets::SecretStore>,
    token: &crate::link::Token,
    origin: Option<&str>,
    endpoint: &str,
    now: u64,
    send: Transport,
) -> Renewal {
    let api_key = {
        let s = secrets.lock().unwrap_or_else(|e| e.into_inner());
        let exp = s
            .get(RELAY_TOKEN_EXP_KEY)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        if !needs_renewal(exp, now) {
            return Renewal::NotDue;
        }
        match s.get(DC_PROFILE) {
            Some(k) => k.to_string(),
            None => return Renewal::Failed("没有 DC 账号的密钥，续不了中转令牌".into()),
        }
    };
    let Some(origin) = origin else {
        return Renewal::Failed("找不到 DC 网关的地址，续不了中转令牌".into());
    };
    let (new_token, exp) = match fetch_token(origin, &api_key, endpoint, send) {
        Ok(v) => v,
        Err(e) => return Renewal::Failed(e),
    };
    token.set(new_token.clone());
    let mut s = secrets.lock().unwrap_or_else(|e| e.into_inner());
    let saved = s
        .set(RELAY_TOKEN_KEY, &new_token)
        .and_then(|_| s.set(RELAY_TOKEN_EXP_KEY, &exp.to_string()));
    match saved {
        Ok(()) => Renewal::Renewed,
        // 内存里已经换上了，这次进程里照样能用；下次启动会发现过期时间还是
        // 旧的，再换一次。
        Err(e) => Renewal::Failed(format!("新令牌存不下：{e}")),
    }
}

/// 两次续期尝试之间最少隔多久，**不管上一次成没成**。
///
/// 只有「失败之后等一小时」是不够的：网关哪天把令牌有效期调到一天以内（或者
/// 它的时钟歪了，回一个已经过去的 `exp`），`needs_renewal` 每一轮都说「该续
/// 了」，而每一封信封、每一次退避重试都会结束一轮轮询——每台装了 dct 的电脑
/// 就成了打网关的压测机。
pub const RENEW_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// 续期失败之后多久再试。续期在令牌剩一天时就开始，一小时一次足够在过期
/// 前试上二十几回，又不至于在网关挂掉时每 30 秒敲它一下。
pub const RENEW_RETRY_AFTER_FAILURE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// 令牌**已经过期**、续期又失败时多久再试。这时手上没有能用的令牌（比如
/// 电脑睡过了 `exp`，醒来 Wi-Fi 还没连上），等一小时就是断一小时。
pub const RENEW_RETRY_WHEN_EXPIRED: std::time::Duration = std::time::Duration::from_secs(60);

/// 给 `renew_if_due` 限速：连接线程每一轮轮询前调一次 `tick`。成功之后隔
/// `RENEW_MIN_INTERVAL`；失败之后隔 `RENEW_RETRY_AFTER_FAILURE`，令牌已经
/// 过期的话只隔 `RENEW_RETRY_WHEN_EXPIRED`。
#[derive(Debug, Default)]
pub struct Renewer {
    /// 上一次真打了网关的时刻，以及那一次之后要等多久。
    last: Option<(std::time::Instant, std::time::Duration)>,
}

impl Renewer {
    /// 离上一次尝试太近就什么都不做，返回 `None`；否则照 `renew_if_due` 办。
    /// `NotDue` 没打网络，不算一次尝试。
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        at: std::time::Instant,
        secrets: &std::sync::Mutex<crate::secrets::SecretStore>,
        token: &crate::link::Token,
        origin: Option<&str>,
        endpoint: &str,
        now: u64,
        send: Transport,
    ) -> Option<Renewal> {
        if let Some((t, wait)) = self.last {
            if at.saturating_duration_since(t) < wait {
                return None;
            }
        }
        let r = renew_if_due(secrets, token, origin, endpoint, now, send);
        match &r {
            Renewal::NotDue => {}
            Renewal::Renewed => self.last = Some((at, RENEW_MIN_INTERVAL)),
            Renewal::Failed(_) => {
                // 过期时间读不到也当已经过期（同 `renew_if_due`）。
                let expired = secrets
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(RELAY_TOKEN_EXP_KEY)
                    .and_then(|v| v.parse::<u64>().ok())
                    .is_none_or(|exp| exp <= now);
                let wait = if expired {
                    RENEW_RETRY_WHEN_EXPIRED
                } else {
                    RENEW_RETRY_AFTER_FAILURE
                };
                self.last = Some((at, wait));
            }
        }
        Some(r)
    }
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

    mod renewal {
        use super::*;
        use crate::link::Token;
        use crate::secrets::SecretStore;
        use std::sync::Mutex;

        const NOW: u64 = 1_800_000_000;

        fn secrets(
            exp: Option<u64>,
            dc_key: Option<&str>,
        ) -> (tempfile::TempDir, Mutex<SecretStore>) {
            let t = tempfile::tempdir().unwrap();
            let mut s = SecretStore::load(&t.path().join("secrets.toml"));
            s.set(RELAY_TOKEN_KEY, "old").unwrap();
            if let Some(e) = exp {
                s.set(RELAY_TOKEN_EXP_KEY, &e.to_string()).unwrap();
            }
            if let Some(k) = dc_key {
                s.set(DC_PROFILE, k).unwrap();
            }
            (t, Mutex::new(s))
        }

        fn never(_: &str, _: &str, _: &str) -> Result<(u16, String), String> {
            panic!("不该打网络")
        }

        #[test]
        fn a_token_with_more_than_a_day_left_is_left_alone() {
            let (_t, s) = secrets(Some(NOW + 2 * ONE_DAY_SECS), Some("sk"));
            let tok = Token::new("old");
            assert_eq!(
                renew_if_due(&s, &tok, Some("https://gw"), "c-aa", NOW, &never),
                Renewal::NotDue
            );
            assert_eq!(tok.get(), "old");
        }

        #[test]
        fn a_token_about_to_expire_is_renewed_in_memory_and_on_disk() {
            let (t, s) = secrets(Some(NOW + 60), Some("sk-live"));
            let tok = Token::new("old");
            let fake =
                FakeSend::returning(Ok((200, r#"{"token":"fresh","exp":1900000000}"#.into())));
            let got = renew_if_due(&s, &tok, Some("https://gw"), "c-aa", NOW, &fake.as_fn());
            assert_eq!(got, Renewal::Renewed);
            assert_eq!(tok.get(), "fresh");
            let (url, bearer, body) = fake.last_call();
            assert_eq!(url, "https://gw/admin/api/relay/token");
            assert_eq!(bearer, "sk-live");
            assert_eq!(body, r#"{"endpoint":"c-aa"}"#);
            let disk = SecretStore::load(&t.path().join("secrets.toml"));
            assert_eq!(disk.get(RELAY_TOKEN_KEY), Some("fresh"));
            assert_eq!(disk.get(RELAY_TOKEN_EXP_KEY), Some("1900000000"));
        }

        #[test]
        fn a_failed_renewal_keeps_the_old_token() {
            let (t, s) = secrets(Some(NOW + 60), Some("sk"));
            let tok = Token::new("old");
            let fake = FakeSend::returning(Ok((401, String::new())));
            let got = renew_if_due(&s, &tok, Some("https://gw"), "c-aa", NOW, &fake.as_fn());
            assert!(matches!(got, Renewal::Failed(_)), "{got:?}");
            assert_eq!(tok.get(), "old");
            let disk = SecretStore::load(&t.path().join("secrets.toml"));
            assert_eq!(disk.get(RELAY_TOKEN_KEY), Some("old"));
        }

        #[test]
        fn a_missing_expiry_counts_as_due() {
            let (_t, s) = secrets(None, Some("sk"));
            let tok = Token::new("old");
            let fake = FakeSend::returning(Ok((200, r#"{"token":"fresh","exp":1}"#.into())));
            let got = renew_if_due(&s, &tok, Some("https://gw"), "c-aa", NOW, &fake.as_fn());
            assert_eq!(got, Renewal::Renewed);
        }

        #[test]
        fn no_dc_key_or_no_origin_fails_without_touching_the_network() {
            let (_t, s) = secrets(Some(NOW), None);
            let tok = Token::new("old");
            assert!(matches!(
                renew_if_due(&s, &tok, Some("https://gw"), "c-aa", NOW, &never),
                Renewal::Failed(_)
            ));
            let (_t2, s2) = secrets(Some(NOW), Some("sk"));
            assert!(matches!(
                renew_if_due(&s2, &tok, None, "c-aa", NOW, &never),
                Renewal::Failed(_)
            ));
            assert_eq!(tok.get(), "old");
        }

        /// 网关发的令牌只有一小时（比「剩一天就续」的阈值还短）：每一轮轮询
        /// 前都「该续了」。节流之前，每一轮都打一次网关、重写一次 secrets。
        #[test]
        fn a_short_lived_token_is_renewed_at_most_once_per_interval() {
            let (_t, s) = secrets(Some(NOW + 60), Some("sk"));
            let tok = Token::new("old");
            let body = format!(r#"{{"token":"fresh","exp":{}}}"#, NOW + 3600);
            let fake = FakeSend::returning(Ok((200, body)));
            let send = fake.as_fn();
            let mut r = Renewer::default();
            let t0 = std::time::Instant::now();
            let mut renewed = 0;
            for i in 0..200u64 {
                let at = t0 + std::time::Duration::from_secs(i);
                if let Some(Renewal::Renewed) =
                    r.tick(at, &s, &tok, Some("https://gw"), "c-aa", NOW + i, &send)
                {
                    renewed += 1;
                }
            }
            assert_eq!(fake.calls.borrow().len(), 1, "200 秒里只该打一次网关");
            assert_eq!(renewed, 1);

            let later = t0 + RENEW_MIN_INTERVAL;
            r.tick(later, &s, &tok, Some("https://gw"), "c-aa", NOW, &send);
            assert_eq!(fake.calls.borrow().len(), 2, "过了最短间隔可以再试");
        }

        #[test]
        fn a_failed_renewal_waits_the_long_retry_before_trying_again() {
            let (_t, s) = secrets(Some(NOW + 60), Some("sk"));
            let tok = Token::new("old");
            let fake = FakeSend::returning(Ok((500, String::new())));
            let send = fake.as_fn();
            let mut r = Renewer::default();
            let t0 = std::time::Instant::now();
            r.tick(t0, &s, &tok, Some("https://gw"), "c-aa", NOW, &send);
            r.tick(
                t0 + RENEW_MIN_INTERVAL,
                &s,
                &tok,
                Some("https://gw"),
                "c-aa",
                NOW,
                &send,
            );
            assert_eq!(fake.calls.borrow().len(), 1, "失败之后一小时内不再试");
            r.tick(
                t0 + RENEW_RETRY_AFTER_FAILURE,
                &s,
                &tok,
                Some("https://gw"),
                "c-aa",
                NOW,
                &send,
            );
            assert_eq!(fake.calls.borrow().len(), 2);
        }

        /// 令牌已经过期（比如电脑睡过了 `exp`），醒来第一次续期又失败了
        /// （Wi-Fi 还没连上）：这时手上什么能用的令牌都没有，等一小时太久，
        /// 一分钟后就再试。
        #[test]
        fn an_expired_token_whose_renewal_fails_is_retried_after_a_minute() {
            let (_t, s) = secrets(Some(NOW - 60), Some("sk"));
            let tok = Token::new("old");
            let fake = FakeSend::returning(Err("offline".into()));
            let send = fake.as_fn();
            let mut r = Renewer::default();
            let t0 = std::time::Instant::now();
            r.tick(t0, &s, &tok, Some("https://gw"), "c-aa", NOW, &send);
            r.tick(
                t0 + std::time::Duration::from_secs(30),
                &s,
                &tok,
                Some("https://gw"),
                "c-aa",
                NOW + 30,
                &send,
            );
            assert_eq!(fake.calls.borrow().len(), 1, "一分钟之内不再试");
            r.tick(
                t0 + RENEW_RETRY_WHEN_EXPIRED,
                &s,
                &tok,
                Some("https://gw"),
                "c-aa",
                NOW + 60,
                &send,
            );
            assert_eq!(fake.calls.borrow().len(), 2, "过期了，一分钟后再试");
            assert!(RENEW_RETRY_WHEN_EXPIRED <= std::time::Duration::from_secs(5 * 60));
        }

        #[test]
        fn a_token_that_is_not_due_does_not_start_the_clock() {
            // 没到续期的时候，这一拍不算一次尝试：不打网络，也不该把下一次
            // 真该续的时候往后推。
            let (_t, s) = secrets(Some(NOW + 2 * ONE_DAY_SECS), Some("sk"));
            let tok = Token::new("old");
            let mut r = Renewer::default();
            let t0 = std::time::Instant::now();
            assert_eq!(
                r.tick(t0, &s, &tok, Some("https://gw"), "c-aa", NOW, &never),
                Some(Renewal::NotDue)
            );
            let fake = FakeSend::returning(Ok((200, r#"{"token":"fresh","exp":1}"#.into())));
            let got = r.tick(
                t0 + std::time::Duration::from_secs(1),
                &s,
                &tok,
                Some("https://gw"),
                "c-aa",
                NOW + 2 * ONE_DAY_SECS,
                &fake.as_fn(),
            );
            assert_eq!(got, Some(Renewal::Renewed));
        }
    }
}
