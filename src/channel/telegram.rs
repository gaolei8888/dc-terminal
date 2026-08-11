//! Telegram 适配器。
//!
//! 它被排在第一个渠道，全部理由是 `getUpdates` 长轮询让 NAT 后面的笔记本
//! 不需要服务器、不需要公网域名、不需要隧道。**别把这条优势改掉。**

use super::{Channel, ChannelError, Incoming, MsgId};
use std::time::Duration;

const API: &str = "https://api.telegram.org";

/// 传输层的形状：(url, body, timeout) -> 响应正文。与 `verify.rs::send_probe`
/// 同一个路子——判定逻辑可以在不打网络的前提下被完整测试。
///
/// 命名 `Sender` 而不是 `Send`：后者是 `std::marker::Send`，跟自动 trait
/// 同名会在这条 bound 列表自己的定义里发生递归解析（`+ Send + Sync` 里的
/// `Send` 会指回这个类型别名本身，而不是标记 trait），编译不过。这是
/// 计划参考代码里的一个真错误，`llm/http.rs::Sender` 已经用的是这个名字。
///
/// `timeout` 是调用方算好传进来的，不是传输自己的常量：长轮询要等
/// `poll` 请求的秒数再加余量，`sendMessage` 只该等几秒。两者共用一个连接
/// 超时常量的话，要么把 `sendMessage` 的超时拖得离谱地长，要么在长轮询
/// 请求的秒数之前就把连接掐断——那会让每次轮询都被误判成 `Unreachable`。
pub type Sender = dyn Fn(&str, &str, Duration) -> Result<String, String> + Send + Sync;

/// `sendMessage` 是一次性的普通请求，不是长轮询，不需要跟着 `poll` 的
/// timeout 走。10 秒足够覆盖正常的网络抖动，同时不会让一次卡死的调用把
/// 出站线程拖住太久。
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// 长轮询请求的客户端读超时 = 请求里 `timeout=` 的秒数 + 这份余量。
/// Telegram 服务器自己会等到那个秒数才回；客户端超时若卡死在同一个数字上，
/// 网络抖一下就会被误判成 `Unreachable`，进而触发不必要的退避重试。
const POLL_TIMEOUT_MARGIN: Duration = Duration::from_secs(5);

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
    /// 发消息该发去哪个 chat。**这不是 Task 1 的 `Channel` trait 能表达
    /// 的东西**——`send(&self, text: &str)` 没有 chat_id 参数，接口就是这么
    /// 定的（见 `mod.rs`）。真正的「只认一个人」由 `bridge.rs`（Task 5）
    /// 在更高的一层把关；这里独立记一份的唯一理由是 `send` 拼请求体时
    /// 硬性需要一个目的地。
    ///
    /// 学习规则是「先到先得，学到就不再换」——和 `bridge.rs` 里
    /// 「第一个发消息的人成为主人」用的是同一个模型，只是这里发生在传输层，
    /// 不认识 bridge 的存在。没学到之前不知道该发给谁，`send` 会报
    /// `Unreachable`（见下面 `Channel` 的实现）。
    chat_id: std::sync::Mutex<Option<i64>>,
    send: Box<Sender>,
}

impl Telegram {
    /// 真实构造：走 `ureq`。
    pub fn new(token: &str) -> Telegram {
        Telegram::with_transport(token, Box::new(send_real))
    }

    pub fn with_transport(token: &str, send: Box<Sender>) -> Telegram {
        Telegram {
            token: token.to_string(),
            offset: std::sync::Mutex::new(0),
            chat_id: std::sync::Mutex::new(None),
            send,
        }
    }

    fn url(&self, method: &str) -> String {
        format!("{API}/bot{}/{method}", self.token)
    }
}

/// getUpdates 这批更新里最大的 `update_id`。调用方把它 + 1 存成下一次
/// 请求的 offset——Telegram 只在客户端明确用更大的 offset 问过一次之后
/// 才不再重发旧更新。拿不到就说明这批本来没有 update_id 可看，游标不动。
fn max_update_id(body: &str) -> Option<i64> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("result")?
        .as_array()?
        .iter()
        .filter_map(|it| it.get("update_id").and_then(|u| u.as_i64()))
        .max()
}

impl Channel for Telegram {
    fn send(&self, text: &str) -> Result<MsgId, ChannelError> {
        // 没学到过 chat_id 就没有目的地可发——见 `chat_id` 字段上的注释。
        // 一旦 `poll` 学到了，这里立刻就能成功，`Unreachable` 的
        // `worth_retrying() == true` 正好表达「现在不行，等一等会行」。
        let chat_id = self
            .chat_id
            .lock()
            .unwrap()
            .ok_or(ChannelError::Unreachable)?;
        let url = self.url("sendMessage");
        let body = serde_json::json!({ "chat_id": chat_id, "text": text }).to_string();
        let resp = (self.send)(&url, &body, SEND_TIMEOUT).map_err(|_| ChannelError::Unreachable)?;
        parse_send_result(&resp)
    }

    fn poll(&self, timeout: Duration) -> Result<Vec<Incoming>, ChannelError> {
        let secs = timeout.as_secs();
        let offset = *self.offset.lock().unwrap();
        let url = format!("{}?offset={offset}&timeout={secs}", self.url("getUpdates"));
        // 空 body：`getUpdates` 的参数全在查询串里。真传输 `send_real` 把
        // 空 body 当成「这是一个 GET」的信号，见它自己的注释。
        let net_timeout = timeout + POLL_TIMEOUT_MARGIN;
        let body = (self.send)(&url, "", net_timeout).map_err(|_| ChannelError::Unreachable)?;

        let incoming = parse_updates(&body)?;

        // 游标只在这批确实有 update_id 时才前进；`?` 已经在上面处理过
        // ok:false 的情况，这里的 body 一定是「一批（可能是空的）更新」。
        if let Some(max_id) = max_update_id(&body) {
            *self.offset.lock().unwrap() = max_id + 1;
        }

        // 先到先得：只在还没学到过 chat 的时候才记。
        let mut chat_id = self.chat_id.lock().unwrap();
        if chat_id.is_none() {
            if let Some(first) = incoming.first() {
                *chat_id = Some(first.chat_id);
            }
        }

        Ok(incoming)
    }
}

/// 长轮询和普通请求各自建一个超时匹配的 `Agent`——同 `verify.rs::build_probe_agent`
/// 与 `llm/http.rs::build_http_agent` 的理由：`.timeout()` 和
/// `.timeout_connect()` 都要设，只设前者建连阶段会退回 ureq 默认的 30 秒。
fn build_agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .timeout_connect(timeout)
        .build()
}

/// 真实传输。**没有单元测试覆盖**，在实测那一步验——同 `llm/http.rs::send_real`
/// 的界限：这里只负责把字节送出去、把响应字节的原样读回来，判定逻辑全在
/// `parse_updates` / `parse_send_result` / `parse_get_me` / `error_from` 里，
/// 那些已经被测过了。
///
/// 空 body 当 GET（`getUpdates`，参数全在查询串里），非空 body 当 POST
/// JSON（`sendMessage`）。
fn send_real(url: &str, body: &str, timeout: Duration) -> Result<String, String> {
    let agent = build_agent(timeout);
    let resp = if body.is_empty() {
        agent.get(url).call()
    } else {
        agent
            .post(url)
            .set("content-type", "application/json")
            .send_string(body)
    };
    match resp {
        Ok(r) => r.into_string().map_err(|e| format!("{e}")),
        // ureq 把 4xx/5xx 也当 Err，得挑出来——它们是有效的响应体（Telegram
        // 的 `ok:false` 错误就长在这里面），不是网络故障。
        Err(ureq::Error::Status(_, r)) => r.into_string().map_err(|e| format!("{e}")),
        Err(e) => Err(format!("{e}")),
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

    /// Step 6 的变异测试要求：把 `error_from` 里的 `Some(401) | Some(403)`
    /// 砍成只剩 `Some(401)`，`get_me_with_a_bad_token_says_bad_token` 照样过
    /// （它用的是 401）——403 那半句判断根本没被任何测试盯着。这一条补上。
    #[test]
    fn a_403_is_also_bad_token_not_unreachable() {
        let body = r#"{"ok":false,"error_code":403,"description":"Forbidden: bot was blocked by the user"}"#;
        assert_eq!(parse_updates(body), Err(ChannelError::BadToken));
        assert_eq!(parse_get_me(body), Err(ChannelError::BadToken));
    }

    /// 假传输：记下每次被调用的 (url, body)，回放预先准备好的响应。
    /// 与 `verify.rs::the_key_reaches_the_transport` 同一个路子——判定逻辑
    /// 和一次性发生的副作用都要能在不打网络的前提下核实。
    struct FakeTransport {
        calls: std::sync::Mutex<Vec<(String, String)>>,
        replies: std::sync::Mutex<std::collections::VecDeque<String>>,
    }

    impl FakeTransport {
        fn new(replies: Vec<&str>) -> std::sync::Arc<FakeTransport> {
            std::sync::Arc::new(FakeTransport {
                calls: std::sync::Mutex::new(Vec::new()),
                replies: std::sync::Mutex::new(replies.into_iter().map(String::from).collect()),
            })
        }

        fn sender(self: &std::sync::Arc<Self>) -> Box<Sender> {
            let me = self.clone();
            Box::new(move |url: &str, body: &str, _timeout: Duration| {
                me.calls
                    .lock()
                    .unwrap()
                    .push((url.to_string(), body.to_string()));
                me.replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .ok_or_else(|| "FakeTransport ran out of replies".to_string())
            })
        }
    }

    /// **不测这一条就会有「同一句话被敲进 agent 好几遍」。** Telegram 只在
    /// 客户端明确用更大的 offset 请求过之后才不再重发旧更新；第二次 `poll`
    /// 必须带上「上一批最大 update_id + 1」，否则永远轮询回同一条消息。
    #[test]
    fn the_second_poll_carries_the_offset_past_the_last_update_id() {
        let first = r#"{"ok":true,"result":[
            {"update_id":5,"message":{"message_id":1,"chat":{"id":777},"text":"a"}},
            {"update_id":7,"message":{"message_id":2,"chat":{"id":777},"text":"b"}}
        ]}"#;
        let second = r#"{"ok":true,"result":[]}"#;
        let fake = FakeTransport::new(vec![first, second]);
        let tg = Telegram::with_transport("tok", fake.sender());

        tg.poll(Duration::from_secs(25)).unwrap();
        tg.poll(Duration::from_secs(25)).unwrap();

        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(
            calls[0].0.contains("offset=0"),
            "第一次轮询没理由带非零 offset: {}",
            calls[0].0
        );
        assert!(
            calls[1].0.contains("offset=8"),
            "第二次轮询该带上 7 + 1 = 8，实际 url: {}",
            calls[1].0
        );
    }

    /// 长轮询的 `timeout` 秒数要出现在请求的 URL 里——这是服务端等多久，
    /// 不是客户端超时（客户端超时是另一件事，靠真传输的 `net_timeout` 兜底）。
    #[test]
    fn poll_puts_the_requested_seconds_in_the_url() {
        let fake = FakeTransport::new(vec![r#"{"ok":true,"result":[]}"#]);
        let tg = Telegram::with_transport("tok", fake.sender());
        tg.poll(Duration::from_secs(25)).unwrap();
        let calls = fake.calls.lock().unwrap();
        assert!(calls[0].0.contains("timeout=25"), "{}", calls[0].0);
    }

    /// 没收到过任何消息之前，`send` 不知道该发给哪个 chat——Telegram bot
    /// 不能主动找陌生 chat 说话，这不是网络故障，但 `ChannelError` 里最贴切
    /// 的还是 `Unreachable`：一旦配对完成（poll 学到了 chat_id）重试就会成功，
    /// 这正是 `worth_retrying()` 想表达的意思。
    #[test]
    fn sending_before_any_chat_is_known_is_unreachable() {
        let fake = FakeTransport::new(vec![]);
        let tg = Telegram::with_transport("tok", fake.sender());
        assert_eq!(tg.send("先跑完").unwrap_err(), ChannelError::Unreachable);
        assert!(fake.calls.lock().unwrap().is_empty(), "不该打网络");
    }

    /// `poll` 学到的第一个 chat_id 就是以后 `send` 的目的地。与 `bridge.rs`
    /// 的「第一个发消息的人成为主人」是同一个「先到先得、只认一次」的模型——
    /// 这里独立实现一遍是因为 `Channel::send` 的签名里没有 chat_id 参数，
    /// Telegram 只能自己记住「该回给谁」。
    #[test]
    fn send_targets_the_chat_learned_from_poll() {
        let update = r#"{"ok":true,"result":[
            {"update_id":1,"message":{"message_id":1,"chat":{"id":777},"text":"在吗"}}
        ]}"#;
        let sent = r#"{"ok":true,"result":{"message_id":99,"chat":{"id":777}}}"#;
        let fake = FakeTransport::new(vec![update, sent]);
        let tg = Telegram::with_transport("tok", fake.sender());

        tg.poll(Duration::from_secs(25)).unwrap();
        let id = tg.send("先跑完").unwrap();
        assert_eq!(id, 99);

        let calls = fake.calls.lock().unwrap();
        assert!(calls[1].0.ends_with("/sendMessage"), "{}", calls[1].0);
        let body: serde_json::Value = serde_json::from_str(&calls[1].1).unwrap();
        assert_eq!(body["chat_id"], 777);
        assert_eq!(body["text"], "先跑完");
    }

    /// 第一个 chat 一旦学到就不再换——即便后来有陌生 chat 混进 `poll` 的结果。
    /// 这条防的是「先到先得」被静默覆盖，等价于 `bridge.rs` 里
    /// `pairing_happens_exactly_once` 想守住的同一件事，只是发生在传输层。
    #[test]
    fn the_learned_chat_does_not_get_overwritten_by_a_later_sender() {
        let first = r#"{"ok":true,"result":[
            {"update_id":1,"message":{"message_id":1,"chat":{"id":111},"text":"在吗"}}
        ]}"#;
        let second = r#"{"ok":true,"result":[
            {"update_id":2,"message":{"message_id":2,"chat":{"id":222},"text":"我也在"}}
        ]}"#;
        let sent = r#"{"ok":true,"result":{"message_id":50,"chat":{"id":111}}}"#;
        let fake = FakeTransport::new(vec![first, second, sent]);
        let tg = Telegram::with_transport("tok", fake.sender());

        tg.poll(Duration::from_secs(25)).unwrap();
        tg.poll(Duration::from_secs(25)).unwrap();
        tg.send("hi").unwrap();

        let calls = fake.calls.lock().unwrap();
        let body: serde_json::Value = serde_json::from_str(&calls[2].1).unwrap();
        assert_eq!(body["chat_id"], 111, "该一直发给最早那个 chat");
    }

    /// 令牌被撤销时 `send` 也要能把 `BadToken` 传上去，走 `poll` 那条链路
    /// 已经测过判定逻辑，这里核实 `Channel::send` 真的把结果原样透传。
    #[test]
    fn send_surfaces_bad_token() {
        let update = r#"{"ok":true,"result":[
            {"update_id":1,"message":{"message_id":1,"chat":{"id":777},"text":"在吗"}}
        ]}"#;
        let rejected = r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#;
        let fake = FakeTransport::new(vec![update, rejected]);
        let tg = Telegram::with_transport("tok", fake.sender());
        tg.poll(Duration::from_secs(25)).unwrap();
        assert_eq!(tg.send("hi").unwrap_err(), ChannelError::BadToken);
    }

    /// 网络层直接失败（连接被拒、DNS 解析不了……）要落到 `Unreachable`，
    /// 而不是被 `?` 悄悄传播成别的东西。
    #[test]
    fn a_transport_level_failure_is_unreachable() {
        let sender: Box<Sender> =
            Box::new(|_url: &str, _body: &str, _t: Duration| Err("connection refused".into()));
        let tg = Telegram::with_transport("tok", sender);
        assert_eq!(
            tg.poll(Duration::from_secs(25)).unwrap_err(),
            ChannelError::Unreachable
        );
    }
}
