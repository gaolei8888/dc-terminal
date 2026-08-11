//! Telegram 适配器。
//!
//! 它被排在第一个渠道，全部理由是 `getUpdates` 长轮询让 NAT 后面的笔记本
//! 不需要服务器、不需要公网域名、不需要隧道。**别把这条优势改掉。**

use super::{Channel, ChannelError, Incoming, MsgId};
use crate::session::recover;
use std::time::Duration;

const API: &str = "https://api.telegram.org";

/// 传输层的形状：(url, body, timeout) -> (状态码, 响应正文)。与
/// `llm/http.rs::Sender` 同一个路子——状态码必须留着，不能只留 body：
/// Telegram 自己的错误全靠 body 里的 JSON `error_code`，不看 HTTP 状态码，
/// 但一个代理或 CDN 在 5xx 时吐出的错误页根本不是 JSON，这时候只有状态码
/// 能说清楚「这是上游临时抽风」而不是「这条数据我们读不懂」（见 `reinterpret`）。
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
pub type Sender = dyn Fn(&str, &str, Duration) -> Result<(u16, String), String> + Send + Sync;

/// Telegram 的错误全部编码在 body 的 JSON 里，不靠 HTTP 状态码——所以
/// `parse_updates` / `parse_send_result` / `parse_get_me` 从不看状态码。
/// 但如果 body 本身不是 JSON（`Malformed`），状态码就是唯一能说清楚「这是
/// 什么情况」的线索：5xx 通常是代理、CDN 或 Telegram 自己在这次请求上出了
/// 临时的岔子，吐出一段 HTML 错误页——那是网络问题，不是「这条数据读不懂」。
/// `Malformed.worth_retrying() == false`，把这种情况留在 `Malformed` 会让
/// 一次性的上游抖动被 bridge 判定成永久损坏，显示 `Broken` 再也不重试。
/// 4xx 不豁免：那通常是我们自己拼的请求有问题，`Malformed` 判得对。
fn reinterpret(err: ChannelError, status: u16) -> ChannelError {
    if err == ChannelError::Malformed && (500..600).contains(&status) {
        ChannelError::Unreachable
    } else {
        err
    }
}

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
    /// 发消息该发去哪个 chat。**这不是渠道自己该猜的事**——`bridge.rs`
    /// 才是唯一决定「谁是主人」的地方（配对、重新配对、换令牌）。这个字段
    /// 只负责记住 `set_destination` 被告知的值。`None` = 还没被告知，
    /// `send` 会报 `Unreachable`（`worth_retrying() == true`：一旦
    /// `set_destination` 被调用过，重试就会成功）。
    ///
    /// **教训**：早期实现在 `poll()` 里自己从收到的消息学这个值（先到先得）。
    /// 那个设计有个真实的安全漏洞——重新配对或换令牌之后，`poll()` 读到的
    /// 是「未经 bridge 授权检查的原始消息流」，一个抢在真正主人之前发消息
    /// 的陌生人就能把出站目的地偷走，而 bridge 的「拒绝陌生人」只挡得住
    /// *入站*，挡不住这条独立的出站学习路径。删掉了，`bridge.rs` 是唯一
    /// 允许调用 `set_destination` 的地方。
    destination: std::sync::Mutex<Option<i64>>,
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
            destination: std::sync::Mutex::new(None),
            send,
        }
    }

    fn url(&self, method: &str) -> String {
        format!("{API}/bot{}/{method}", self.token)
    }

    /// 验证令牌，顺便拿 bot 用户名给设置页用——「在 Telegram 里搜
    /// @your_bot」那句话就靠它。走注入的传输，所以令牌验证也能在不打
    /// 网络的前提下测试，不用绕开 `self.send` 单独拼一个 HTTP 调用。
    pub fn get_me(&self) -> Result<String, ChannelError> {
        let url = self.url("getMe");
        // 空 body：`getMe` 不需要参数，同 `poll` 里的约定，空 body 代表 GET。
        let (status, body) =
            (self.send)(&url, "", SEND_TIMEOUT).map_err(|_| ChannelError::Unreachable)?;
        parse_get_me(&body).map_err(|e| reinterpret(e, status))
    }
}

/// getUpdates 这批更新里最大的 `update_id`。调用方把它 + 1 存成下一次
/// 请求的 offset——Telegram 只在客户端明确用更大的 offset 问过一次之后
/// 才不再重发旧更新。拿不到就说明这批本来没有 update_id 可看，游标不动。
///
/// **故意直接扫原始 body，不经过 `parse_updates` 的结果**：一条没有
/// text 的更新（贴纸、图片）在 `parse_updates` 里被跳过，不会出现在
/// 它返回的 `Vec<Incoming>` 里，但它的 `update_id` 依然存在、依然必须
/// 被越过——否则那张贴纸会被 Telegram 永远重新投递，长轮询每次都立刻
/// 返回，把轮询循环空转成忙等。
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
        // 没被告知过目的地就没有地方可发——见 `destination` 字段上的注释。
        // 一旦 `set_destination` 被调用过，这里立刻就能成功，`Unreachable`
        // 的 `worth_retrying() == true` 正好表达「现在不行，等一等会行」。
        let chat_id = *recover(self.destination.lock());
        let chat_id = chat_id.ok_or(ChannelError::Unreachable)?;
        let url = self.url("sendMessage");
        let body = serde_json::json!({ "chat_id": chat_id, "text": text }).to_string();
        let (status, resp) =
            (self.send)(&url, &body, SEND_TIMEOUT).map_err(|_| ChannelError::Unreachable)?;
        parse_send_result(&resp).map_err(|e| reinterpret(e, status))
    }

    fn poll(&self, timeout: Duration) -> Result<Vec<Incoming>, ChannelError> {
        let secs = timeout.as_secs();
        let offset = *recover(self.offset.lock());
        let url = format!("{}?offset={offset}&timeout={secs}", self.url("getUpdates"));
        // 空 body：`getUpdates` 的参数全在查询串里。真传输 `send_real` 把
        // 空 body 当成「这是一个 GET」的信号，见它自己的注释。
        let net_timeout = timeout + POLL_TIMEOUT_MARGIN;
        let (status, body) =
            (self.send)(&url, "", net_timeout).map_err(|_| ChannelError::Unreachable)?;

        let incoming = parse_updates(&body).map_err(|e| reinterpret(e, status))?;

        // 游标只在这批确实有 update_id 时才前进；`?` 已经在上面处理过
        // ok:false 的情况，这里的 body 一定是「一批（可能是空的）更新」。
        if let Some(max_id) = max_update_id(&body) {
            *recover(self.offset.lock()) = max_id + 1;
        }

        Ok(incoming)
    }

    fn set_destination(&self, chat: Option<i64>) {
        *recover(self.destination.lock()) = chat;
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
/// 的界限：这里只负责把字节送出去、把响应字节和状态码原样读回来，判定逻辑全在
/// `parse_updates` / `parse_send_result` / `parse_get_me` / `error_from` /
/// `reinterpret` 里，那些已经被测过了。
///
/// 空 body 当 GET（`getUpdates` / `getMe`，参数全在查询串里或不需要参数），
/// 非空 body 当 POST JSON（`sendMessage`）。
fn send_real(url: &str, body: &str, timeout: Duration) -> Result<(u16, String), String> {
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
        Ok(r) => {
            let status = r.status();
            r.into_string()
                .map(|body| (status, body))
                .map_err(|e| format!("{e}"))
        }
        // ureq 把 4xx/5xx 也当 Err，得挑出来——它们是有效的状态码 + 响应体
        // （Telegram 的 `ok:false` 错误、以及代理/CDN 的 5xx 错误页都长在
        // 这里面），不是网络故障。状态码留着交给 `reinterpret`。
        Err(ureq::Error::Status(code, r)) => r
            .into_string()
            .map(|body| (code, body))
            .map_err(|e| format!("{e}")),
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

    /// 只用单元素批次测跳过逻辑，测不出「跳过用的是 `continue` 还是
    /// `break`」——两者对单元素输入的结果一样。把 `continue` 换成 `break`
    /// 会让所有其余 16 条测试照样绿，却在真实场景里（一批里先来一张贴纸，
    /// 后面跟着真正的文字消息）安静丢掉贴纸之后的每一条消息。这条测试
    /// 用两元素批次撑住这个区别。
    #[test]
    fn a_text_less_update_does_not_stop_the_rest_of_the_batch_from_being_read() {
        let body = r#"{"ok":true,"result":[
            {"update_id":1,"message":{"message_id":1,"chat":{"id":777}}},
            {"update_id":2,"message":{"message_id":2,"chat":{"id":777},"text":"看得到我吗"}}
        ]}"#;
        let got = parse_updates(body).unwrap();
        assert_eq!(got.len(), 1, "贴纸之后那条 text 消息不该被漏掉");
        assert_eq!(got[0].text, "看得到我吗");
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

    /// 假传输：记下每次被调用的 (url, body, timeout)，回放预先准备好的响应
    /// （状态码 + body，或者一次网络层失败）。与
    /// `verify.rs::the_key_reaches_the_transport` 同一个路子——判定逻辑
    /// 和一次性发生的副作用都要能在不打网络的前提下核实。
    struct FakeTransport {
        calls: std::sync::Mutex<Vec<(String, String, Duration)>>,
        replies: std::sync::Mutex<std::collections::VecDeque<Result<(u16, String), String>>>,
    }

    impl FakeTransport {
        /// 便捷构造：绝大多数测试只关心「回包内容」，状态码统一当 200。
        fn new(bodies: Vec<&str>) -> std::sync::Arc<FakeTransport> {
            FakeTransport::with_replies(
                bodies
                    .into_iter()
                    .map(|b| Ok((200, b.to_string())))
                    .collect(),
            )
        }

        /// 完整构造：需要非 200 状态码（比如验证 `reinterpret` 的 5xx 豁免）
        /// 时用这个。
        fn with_replies(
            replies: Vec<Result<(u16, String), String>>,
        ) -> std::sync::Arc<FakeTransport> {
            std::sync::Arc::new(FakeTransport {
                calls: std::sync::Mutex::new(Vec::new()),
                replies: std::sync::Mutex::new(replies.into_iter().collect()),
            })
        }

        fn sender(self: &std::sync::Arc<Self>) -> Box<Sender> {
            let me = self.clone();
            Box::new(move |url: &str, body: &str, timeout: Duration| {
                me.calls
                    .lock()
                    .unwrap()
                    .push((url.to_string(), body.to_string(), timeout));
                me.replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| Err("FakeTransport ran out of replies".to_string()))
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
            calls[1].0.contains("offset=8&"),
            "第二次轮询该带上 7 + 1 = 8，实际 url: {}",
            calls[1].0
        );
    }

    /// 游标必须越过没有 text 的更新（贴纸），不只是越过被解析成 `Incoming`
    /// 的那些——否则那张贴纸会被 Telegram 永远重新投递，长轮询每次都立刻
    /// 返回，把轮询循环空转成忙等。
    #[test]
    fn the_cursor_advances_past_a_text_less_update_too() {
        let sticker_only = r#"{"ok":true,"result":[
            {"update_id":9,"message":{"message_id":1,"chat":{"id":777}}}
        ]}"#;
        let second = r#"{"ok":true,"result":[]}"#;
        let fake = FakeTransport::new(vec![sticker_only, second]);
        let tg = Telegram::with_transport("tok", fake.sender());

        let got = tg.poll(Duration::from_secs(25)).unwrap();
        assert!(got.is_empty(), "贴纸没有 text，不该出现在 Incoming 里");

        tg.poll(Duration::from_secs(25)).unwrap();
        let calls = fake.calls.lock().unwrap();
        assert!(
            calls[1].0.contains("offset=10&"),
            "游标该越过贴纸的 update_id（9 + 1 = 10）: {}",
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

    /// `poll` 和 `send` 各自该把哪个超时预算传给传输层——这两个常量
    /// (`POLL_TIMEOUT_MARGIN`、`SEND_TIMEOUT`) 加进接口之前完全没有测试
    /// 盯着，删掉它们全部测试照样绿。一个长轮询请求 25 秒，客户端超时
    /// 该是 30 秒（25 + 5 秒余量）；`sendMessage` 不是长轮询，该是固定
    /// 的 10 秒，不该被拖着等 25 秒。
    #[test]
    fn poll_and_send_each_carry_their_own_client_timeout() {
        let fake = FakeTransport::new(vec![
            r#"{"ok":true,"result":[]}"#,
            r#"{"ok":true,"result":{"message_id":1,"chat":{"id":777}}}"#,
        ]);
        let tg = Telegram::with_transport("tok", fake.sender());
        tg.set_destination(Some(777));

        tg.poll(Duration::from_secs(25)).unwrap();
        tg.send("hi").unwrap();

        let calls = fake.calls.lock().unwrap();
        assert_eq!(
            calls[0].2,
            Duration::from_secs(30),
            "poll(25s) 的客户端超时该是 25 + 5 秒余量"
        );
        assert_eq!(
            calls[1].2,
            Duration::from_secs(10),
            "send 不是长轮询，不该借用 poll 的超时"
        );
    }

    /// 没被告知过目的地之前，`send` 不知道该发给哪个 chat——Telegram bot
    /// 不能主动找陌生 chat 说话，这不是网络故障，但 `ChannelError` 里最贴切
    /// 的还是 `Unreachable`：一旦 `set_destination` 被调用过重试就会成功，
    /// 这正是 `worth_retrying()` 想表达的意思。
    #[test]
    fn sending_before_any_destination_is_set_is_unreachable() {
        let fake = FakeTransport::new(vec![]);
        let tg = Telegram::with_transport("tok", fake.sender());
        assert_eq!(tg.send("先跑完").unwrap_err(), ChannelError::Unreachable);
        assert!(fake.calls.lock().unwrap().is_empty(), "不该打网络");
    }

    /// `poll` 绝不能替 `send` 挑目的地——那是 `bridge.rs` 的职责（配对、
    /// 重新配对、换令牌都会改写它）。这里核实哪怕收到了消息，`chat_id` 也
    /// 不会被 `poll` 偷偷学走：一个抢在真正主人之前发消息的陌生人，不该
    /// 只是被挡在「入站」那一侧——他也绝不能成为「出站」的默认收件人。
    /// 这正是本轮修复要关掉的那个安全缺口。
    #[test]
    fn polling_a_strangers_message_never_sets_a_destination() {
        let stranger = r#"{"ok":true,"result":[
            {"update_id":1,"message":{"message_id":1,"chat":{"id":666},"text":"rm -rf /"}}
        ]}"#;
        let fake = FakeTransport::new(vec![stranger]);
        let tg = Telegram::with_transport("tok", fake.sender());

        let got = tg.poll(Duration::from_secs(25)).unwrap();
        assert_eq!(got.len(), 1, "消息本身照样要交给调用方——拒绝是 bridge 的事");

        // send 依然没有目的地：poll 没有替我们做这个决定。
        assert_eq!(tg.send("hi").unwrap_err(), ChannelError::Unreachable);
        assert_eq!(fake.calls.lock().unwrap().len(), 1, "send 不该打网络");
    }

    /// `set_destination` 是 `bridge.rs` 唯一被允许调用的入口——这条测试
    /// 只核实它确实生效：设置之后 `send` 用它拼请求体。
    #[test]
    fn send_targets_the_configured_destination() {
        let sent = r#"{"ok":true,"result":{"message_id":99,"chat":{"id":777}}}"#;
        let fake = FakeTransport::new(vec![sent]);
        let tg = Telegram::with_transport("tok", fake.sender());

        tg.set_destination(Some(777));
        let id = tg.send("先跑完").unwrap();
        assert_eq!(id, 99);

        let calls = fake.calls.lock().unwrap();
        assert!(calls[0].0.ends_with("/sendMessage"), "{}", calls[0].0);
        let body: serde_json::Value = serde_json::from_str(&calls[0].1).unwrap();
        assert_eq!(body["chat_id"], 777);
        assert_eq!(body["text"], "先跑完");
    }

    /// `set_destination(None)` 是重新配对 / 换令牌那条路径要用的——目的地
    /// 必须能被清掉，不然「取消配对」在界面上看着生效了，出站消息其实还在
    /// 送去旧账号。
    #[test]
    fn set_destination_can_be_cleared() {
        let fake = FakeTransport::new(vec![]);
        let tg = Telegram::with_transport("tok", fake.sender());
        tg.set_destination(Some(777));
        tg.set_destination(None);
        assert_eq!(tg.send("hi").unwrap_err(), ChannelError::Unreachable);
    }

    /// 令牌被撤销时 `send` 也要能把 `BadToken` 传上去，走 `poll` 那条链路
    /// 已经测过判定逻辑，这里核实 `Channel::send` 真的把结果原样透传。
    #[test]
    fn send_surfaces_bad_token() {
        let rejected = r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#;
        let fake = FakeTransport::new(vec![rejected]);
        let tg = Telegram::with_transport("tok", fake.sender());
        tg.set_destination(Some(777));
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

    /// 代理或 CDN 在 5xx 时经常吐一段 HTML 错误页，不是 Telegram 自己的
    /// JSON。`Malformed` 不会重试（`worth_retrying() == false`），把这种
    /// 一次性的上游抖动落在 `Malformed` 上会让 bridge 判定成永久损坏、
    /// 显示 `Broken`，再也不重试——`reinterpret` 存在就是为了防这个。
    #[test]
    fn a_5xx_with_an_unparseable_body_is_unreachable_not_malformed() {
        let fake =
            FakeTransport::with_replies(vec![Ok((502, "<html>Bad Gateway</html>".to_string()))]);
        let tg = Telegram::with_transport("tok", fake.sender());
        assert_eq!(
            tg.poll(Duration::from_secs(25)).unwrap_err(),
            ChannelError::Unreachable
        );
    }

    /// 4xx 不该被同一条豁免规则捞走——那通常是我们自己拼的请求有问题，
    /// `Malformed` 判得对。这条测试划清 `reinterpret` 只豁免 5xx 的边界，
    /// 防止有人「顺手」把条件放宽成所有非 2xx。
    #[test]
    fn a_4xx_with_an_unparseable_body_is_still_malformed() {
        let fake = FakeTransport::with_replies(vec![Ok((400, "not json".to_string()))]);
        let tg = Telegram::with_transport("tok", fake.sender());
        assert_eq!(
            tg.poll(Duration::from_secs(25)).unwrap_err(),
            ChannelError::Malformed
        );
    }

    /// `get_me` 走注入的传输，不是绕开它单独拼一个 HTTP 调用——Task 4
    /// 要用它验证令牌，如果这条路走不通，令牌验证就会成为唯一测不到的
    /// 东西。同时核实它用的是 `SEND_TIMEOUT`（不是长轮询）。
    #[test]
    fn get_me_goes_through_the_injected_transport() {
        let body = r#"{"ok":true,"result":{"id":1,"is_bot":true,"username":"my_dct_bot"}}"#;
        let fake = FakeTransport::new(vec![body]);
        let tg = Telegram::with_transport("tok", fake.sender());

        assert_eq!(tg.get_me().unwrap(), "my_dct_bot");

        let calls = fake.calls.lock().unwrap();
        assert!(calls[0].0.ends_with("/getMe"), "{}", calls[0].0);
        assert_eq!(
            calls[0].2,
            Duration::from_secs(10),
            "getMe 不是长轮询，不该借用 poll 的超时"
        );
    }

    #[test]
    fn get_me_surfaces_bad_token_through_the_channel() {
        let rejected = r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#;
        let fake = FakeTransport::new(vec![rejected]);
        let tg = Telegram::with_transport("tok", fake.sender());
        assert_eq!(tg.get_me().unwrap_err(), ChannelError::BadToken);
    }
}
