//! Telegram 适配器。
//!
//! 它被排在第一个渠道，全部理由是 `getUpdates` 长轮询让 NAT 后面的笔记本
//! 不需要服务器、不需要公网域名、不需要隧道。**别把这条优势改掉。**

use super::{Channel, ChannelError, Incoming, MsgId};
use std::sync::Mutex;
use std::time::Duration;

const API: &str = "https://api.telegram.org";

/// 传输层的形状：(url, body) -> 响应正文。与 `verify.rs::send_probe` 同一个
/// 路子——判定逻辑可以在不打网络的前提下被完整测试。
pub type Send = dyn Fn(&str, &str) -> Result<String, String> + std::marker::Send + Sync;

/// 从 `ok:false` 的回包里判错误类型。401/403 是令牌的问题，其余当网络问题。
fn error_from(v: &serde_json::Value) -> ChannelError {
    match v.get("error_code").and_then(|c| c.as_i64()) {
        Some(401) | Some(403) => ChannelError::BadToken,
        _ => ChannelError::Unreachable,
    }
}

pub fn parse_updates(body: &str) -> Result<Vec<Incoming>, ChannelError> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|_| ChannelError::Malformed)?;
    if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        return Err(error_from(&v));
    }
    let items = v
        .get("result")
        .and_then(|r| r.as_array())
        .ok_or(ChannelError::Malformed)?;

    let mut out = Vec::new();
    for it in items {
        let Some(m) = it.get("message") else {
            continue;
        };
        // 没有 text 的更新（图片、贴纸、有人进群）跳过。**不是错误**——
        // 当成错误会让一张图片害得整轮轮询失败。
        let Some(text) = m.get("text").and_then(|t| t.as_str()) else {
            continue;
        };
        let Some(chat_id) = m
            .get("chat")
            .and_then(|c| c.get("id"))
            .and_then(|i| i.as_i64())
        else {
            continue;
        };
        out.push(Incoming {
            text: text.to_string(),
            reply_to: m
                .get("reply_to_message")
                .and_then(|r| r.get("message_id"))
                .and_then(|i| i.as_i64()),
            chat_id,
        });
    }
    Ok(out)
}

/// 从原始回包里读出这一批更新里最大的 `update_id`。**只有这个值加一之后
/// 才是下一次 `getUpdates` 该用的 `offset`**——用它才能让 Telegram 不再
/// 把这批更新重新递过来。
fn max_update_id(v: &serde_json::Value) -> Option<i64> {
    v.get("result")?
        .as_array()?
        .iter()
        .filter_map(|it| it.get("update_id").and_then(|u| u.as_i64()))
        .max()
}

pub fn parse_send_result(body: &str) -> Result<MsgId, ChannelError> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|_| ChannelError::Malformed)?;
    if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        return Err(error_from(&v));
    }
    v.get("result")
        .and_then(|r| r.get("message_id"))
        .and_then(|i| i.as_i64())
        .ok_or(ChannelError::Malformed)
}

/// 验证令牌，顺便拿 bot 用户名——界面要显示「在 Telegram 里搜 @your_bot」。
pub fn parse_get_me(body: &str) -> Result<String, ChannelError> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|_| ChannelError::Malformed)?;
    if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        return Err(error_from(&v));
    }
    v.get("result")
        .and_then(|r| r.get("username"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string())
        .ok_or(ChannelError::Malformed)
}

/// `send_real` 从 URL 里取回 `timeout=` 这个查询参数，算出 ureq 客户端该等
/// 多久：长轮询秒数 + 5 秒余量，同 `verify.rs::PROBE_TIMEOUT` 一个理由——
/// 只设 `.timeout()` 不设 `.timeout_connect()` 建连阶段会退回 ureq 默认的
/// 30 秒。没有 `timeout=` 参数（`sendMessage` 走这条路）时余量本身就是超时。
fn timeout_from_url(url: &str) -> Duration {
    let secs = url
        .split("timeout=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    Duration::from_secs(secs + 5)
}

/// 真实传输。**没有单元测试覆盖**，走注入的 `Send` 那条缝才能测。
fn send_real(url: &str, body: &str) -> Result<String, String> {
    let t = timeout_from_url(url);
    let agent = ureq::AgentBuilder::new()
        .timeout(t)
        .timeout_connect(t)
        .build();
    let req = agent.post(url).set("content-type", "application/json");
    let resp = if body.is_empty() {
        req.call()
    } else {
        req.send_string(body)
    };
    match resp {
        Ok(r) => Ok(r.into_string().unwrap_or_default()),
        // ureq 把 4xx/5xx 也当 Err，得挑出来——它们是有效回包，不是网络故障，
        // `error_from` 需要读到里面的 `error_code` 才能分清 BadToken。
        Err(ureq::Error::Status(_, r)) => Ok(r.into_string().unwrap_or_default()),
        Err(e) => Err(format!("{e}")),
    }
}

pub struct Telegram {
    token: String,
    /// 长轮询的游标。Telegram 只在你确认过之后才丢弃旧更新，
    /// 不带它会把同一条消息反复取回来——那意味着同一句话被敲进 agent 好几遍。
    offset: Mutex<i64>,
    send: Box<Send>,
}

impl Telegram {
    pub fn new(token: &str) -> Telegram {
        Telegram::with_transport(token, Box::new(send_real))
    }

    pub fn with_transport(token: &str, send: Box<Send>) -> Telegram {
        Telegram {
            token: token.to_string(),
            offset: Mutex::new(0),
            send,
        }
    }

    fn url(&self, method: &str) -> String {
        format!("{API}/bot{}/{method}", self.token)
    }
}

impl Channel for Telegram {
    fn send(&self, to: i64, text: &str) -> Result<MsgId, ChannelError> {
        let body = serde_json::json!({"chat_id": to, "text": text}).to_string();
        let resp =
            (self.send)(&self.url("sendMessage"), &body).map_err(|_| ChannelError::Unreachable)?;
        parse_send_result(&resp)
    }

    fn poll(&self, timeout: Duration) -> Result<Vec<Incoming>, ChannelError> {
        let offset = *self.offset.lock().unwrap();
        let url = format!(
            "{}?offset={}&timeout={}",
            self.url("getUpdates"),
            offset,
            timeout.as_secs()
        );
        let resp = (self.send)(&url, "").map_err(|_| ChannelError::Unreachable)?;
        let incoming = parse_updates(&resp)?;

        // 更新 offset 用的是原始回包里的 update_id，parse_updates 的
        // Incoming 里没有这个字段——两次解析同一段 JSON 换来的是
        // 「判定逻辑」和「游标推进」互不干扰。上面 `parse_updates` 的
        // `?` 已经保证这里的 JSON 一定是合法且 ok:true 的。
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp) {
            if let Some(max_id) = max_update_id(&v) {
                *self.offset.lock().unwrap() = max_id + 1;
            }
        }

        Ok(incoming)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 真实回包的形状。少一个字段就该是 Malformed，不该 panic、不该猜。
    #[test]
    fn parses_a_normal_update() {
        let body = r#"{"ok":true,"result":[{"update_id":1,"message":{
            "message_id":42,"chat":{"id":777},"text":"先跑完"}}]}"#;
        let got = parse_updates(body).expect("正常回包该解得出来");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "先跑完");
        assert_eq!(got[0].chat_id, 777);
        assert_eq!(got[0].reply_to, None);
    }

    #[test]
    fn picks_up_the_message_being_replied_to() {
        let body = r#"{"ok":true,"result":[{"update_id":2,"message":{
            "message_id":43,"chat":{"id":777},"text":"就第二个",
            "reply_to_message":{"message_id":42}}}]}"#;
        let got = parse_updates(body).unwrap();
        assert_eq!(got[0].reply_to, Some(42));
    }

    /// 没有 text 的更新（图片、贴纸、有人进群）不是错误，是「这条没什么可读的」。
    /// 当成 Malformed 会让一张图片害得整轮轮询失败。
    #[test]
    fn updates_without_text_are_skipped_not_errors() {
        let body = r#"{"ok":true,"result":[{"update_id":3,"message":{
            "message_id":44,"chat":{"id":777}}}]}"#;
        assert_eq!(parse_updates(body).unwrap().len(), 0);
    }

    /// 令牌被撤销时 Telegram 回 ok:false + 401。**必须区分出 BadToken**，
    /// 否则退避重试会永远转下去，用户永远等不到「去重填令牌」这句话。
    #[test]
    fn a_revoked_token_is_bad_token_not_unreachable() {
        let body = r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#;
        assert_eq!(parse_updates(body), Err(ChannelError::BadToken));
    }

    /// 401 和 403 都是「令牌不行」——变异测试把 `error_from` 缩小成只认
    /// 401 之后，`get_me_with_a_bad_token_says_bad_token` 照样能过（它用
    /// 的正好是 401），说明 403 一直没被真的测到。补在这里。
    #[test]
    fn a_forbidden_token_is_also_bad_token_not_unreachable() {
        let body = r#"{"ok":false,"error_code":403,"description":"Forbidden"}"#;
        assert_eq!(parse_updates(body), Err(ChannelError::BadToken));
    }

    #[test]
    fn garbage_is_malformed() {
        assert_eq!(
            parse_updates("not json at all"),
            Err(ChannelError::Malformed)
        );
    }

    #[test]
    fn reads_the_new_message_id_back() {
        let body = r#"{"ok":true,"result":{"message_id":99,"chat":{"id":777}}}"#;
        assert_eq!(parse_send_result(body), Ok(99));
    }

    /// getMe 用来验证令牌，同时拿到 bot 用户名——界面上要显示
    /// 「在 Telegram 里搜 @your_bot」，没有这个名字那句话就没法写。
    #[test]
    fn get_me_returns_the_bot_username() {
        let body = r#"{"ok":true,"result":{"id":1,"is_bot":true,"username":"my_dct_bot"}}"#;
        assert_eq!(parse_get_me(body).unwrap(), "my_dct_bot");
    }

    #[test]
    fn get_me_with_a_bad_token_says_bad_token() {
        let body = r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#;
        assert_eq!(parse_get_me(body), Err(ChannelError::BadToken));
    }

    /// 连续两次 `poll`：第二次的 URL 必须带上「上一次最大 update_id + 1」
    /// 这个 offset。这一条不测就会有「同一句话被敲进 agent 好几遍」——
    /// Telegram 只在你确认过之后才丢弃旧更新。
    #[test]
    fn the_second_poll_carries_the_offset_forward() {
        let seen_urls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen_urls.clone();
        let call = Mutex::new(0u32);
        let tg = Telegram::with_transport(
            "tok",
            Box::new(move |url, _body| {
                sink.lock().unwrap().push(url.to_string());
                let mut c = call.lock().unwrap();
                *c += 1;
                if *c == 1 {
                    Ok(r#"{"ok":true,"result":[
                        {"update_id":5,"message":{"message_id":1,"chat":{"id":1},"text":"a"}},
                        {"update_id":7,"message":{"message_id":2,"chat":{"id":1},"text":"b"}}
                    ]}"#
                    .to_string())
                } else {
                    Ok(r#"{"ok":true,"result":[]}"#.to_string())
                }
            }),
        );

        tg.poll(Duration::from_secs(10)).unwrap();
        tg.poll(Duration::from_secs(10)).unwrap();

        let urls = seen_urls.lock().unwrap();
        assert_eq!(urls.len(), 2);
        assert!(
            urls[0].contains("offset=0"),
            "第一次轮询该从 0 开始: {}",
            urls[0]
        );
        assert!(
            urls[1].contains("offset=8"),
            "第二次轮询该带上上一批最大 update_id(7) + 1: {}",
            urls[1]
        );
    }

    /// `send` 的 `to` 参数必须原样送进请求体的 `chat_id` 字段——渠道自己
    /// 不记着任何 chat id，收件人完全由调用方每次决定。这一条要是被
    /// 破坏（比如又悄悄记住了某个 chat id、或者忽略 `to` 用了别的常量），
    /// 发往「用户私人会话」的通知就可能被送到错的聊天里，这正是
    /// `chat_id: Mutex<Option<i64>>` 这个字段被删掉要防的问题。
    #[test]
    fn send_posts_to_the_chat_id_the_caller_passed() {
        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let tg = Telegram::with_transport(
            "tok",
            Box::new(move |url, body| {
                sink.lock()
                    .unwrap()
                    .push((url.to_string(), body.to_string()));
                Ok(r#"{"ok":true,"result":{"message_id":1,"chat":{"id":0}}}"#.to_string())
            }),
        );

        tg.send(4242, "hello").unwrap();
        tg.send(9999, "hello again").unwrap();

        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(
            calls[0].1.contains("\"chat_id\":4242"),
            "第一次 send 的请求体该带上调用方传入的 4242: {}",
            calls[0].1
        );
        assert!(
            calls[1].1.contains("\"chat_id\":9999"),
            "第二次 send 换了目标，请求体该跟着换成 9999，不是复用上一次: {}",
            calls[1].1
        );
    }
}
