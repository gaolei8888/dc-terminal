//! Telegram 适配器。
//!
//! 它被排在第一个渠道，全部理由是 `getUpdates` 长轮询让 NAT 后面的笔记本
//! 不需要服务器、不需要公网域名、不需要隧道。**别把这条优势改掉。**

use super::{Channel, ChannelError, Incoming, MsgId};
use std::time::Duration;

const API: &str = "https://api.telegram.org";

/// 传输层的形状：(url, body) -> 响应正文。与 `verify.rs::send_probe` 同一个
/// 路子——判定逻辑可以在不打网络的前提下被完整测试。
///
/// 命名 `Sender` 而不是 `Send`：后者是 `std::marker::Send`，跟自动 trait
/// 同名会在这条 bound 列表自己的定义里发生递归解析（`+ Send + Sync` 里的
/// `Send` 会指回这个类型别名本身，而不是标记 trait），编译不过。这是
/// 计划参考代码里的一个真错误，`llm/http.rs::Sender` 已经用的是这个名字。
pub type Sender = dyn Fn(&str, &str) -> Result<String, String> + Send + Sync;

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

pub struct Telegram {
    token: String,
    /// 长轮询的游标。Telegram 只在你确认过之后才丢弃旧更新，
    /// 不带它会把同一条消息反复取回来——那意味着同一句话被敲进 agent 好几遍。
    offset: std::sync::Mutex<i64>,
    send: Box<Sender>,
}

impl Telegram {
    pub fn with_transport(token: &str, send: Box<Sender>) -> Telegram {
        Telegram {
            token: token.to_string(),
            offset: std::sync::Mutex::new(0),
            send,
        }
    }

    fn url(&self, method: &str) -> String {
        format!("{API}/bot{}/{method}", self.token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
