//! 把消息送到用户手机上、再把他的回复取回来。
//!
//! **这一层不认识会话，也不认识 dct 的任何状态。** 它只知道「发一段文字」
//! 和「取回一些文字」。谁该收到、敲给谁，全在 `bridge.rs`。

use std::time::Duration;

pub mod telegram;

/// 渠道那边的消息 id。长按回复靠它把回复关联回某个会话。
/// Telegram 的 `message_id` 是有符号整数，这里跟着它走。
pub type MsgId = i64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    pub text: String,
    /// 用户长按回复的是哪一条。直接发的话是 `None`。
    pub reply_to: Option<MsgId>,
    /// 谁发的。**配对之后只认一个**，见 `bridge.rs`。
    pub chat_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    /// 网络问题。**重试有意义。**
    Unreachable,
    /// 令牌无效或被撤销。**重试一万次还是这个结果**，退避重试是在浪费时间，
    /// 而且会把「该让用户去重填令牌」这件事永远拖着不说。
    BadToken,
    /// 回来了但读不懂。当作坏消息处理，不猜。
    Malformed,
}

impl ChannelError {
    pub fn worth_retrying(self) -> bool {
        matches!(self, ChannelError::Unreachable)
    }
}

pub trait Channel: Send + Sync {
    /// 发一条，返回渠道那边的消息 id。
    fn send(&self, text: &str) -> Result<MsgId, ChannelError>;
    /// 取新消息，最多阻塞 `timeout`。没有新消息就返回空 `Vec`，不是错误。
    fn poll(&self, timeout: Duration) -> Result<Vec<Incoming>, ChannelError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// 干完一轮停下来了
    Stopped,
    /// 报错了
    Failed,
    /// 会话自己没了
    Vanished,
}

/// 一个值得告诉用户的事。**字段全是已经成文的用户语言**——守护进程是
/// 唯一决定用户看到什么文字的地方，这条沿用 `proto.rs` 里
/// 「`ProfileEntry.label` 是 `String` 不是 `LocalizedText`」的同一个理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub session: u32,
    pub kind: EventKind,
    /// 会话名。自动命名功能早就生成好了，这里直接用。
    pub name: String,
    pub project: String,
}

/// 这个事件该发吗？
///
/// `last` 是这个会话上次发出去的时刻，`now` 是现在，都相对于同一个起点。
/// 用 `Duration` 而不是 `Instant` 是为了让测试能给出确定的时间点——
/// `Instant` 造不出「10 秒前」。
///
/// **边界算窗口内**（`<=`）：窗口是「这段时间内不再打扰」，端点上还是那段时间。
pub fn debounce(last: Option<Duration>, now: Duration, window: Duration) -> bool {
    match last {
        None => true,
        Some(last) => now.saturating_sub(last) > window,
    }
}

/// 防抖窗口的起点值。**这个数字是拍出来的**，spec 的「未验证」一节记着它：
/// 偏小会吵人，偏大会漏掉真正的第二个事件。实测之后回来调。
pub const DEBOUNCE_WINDOW: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 防抖只压快速抖动，不压真正的第二个事件。
    #[test]
    fn debounce_suppresses_only_inside_the_window() {
        let w = Duration::from_secs(30);
        // 从没发过：一定发
        assert!(debounce(None, Duration::from_secs(0), w));
        // 窗口内：压掉
        assert!(!debounce(
            Some(Duration::from_secs(10)),
            Duration::from_secs(20),
            w
        ));
        // 正好在窗口边界上：压掉（边界属于窗口内）
        assert!(!debounce(
            Some(Duration::from_secs(10)),
            Duration::from_secs(40),
            w
        ));
        // 窗口外：发
        assert!(debounce(
            Some(Duration::from_secs(10)),
            Duration::from_secs(41),
            w
        ));
    }

    /// `ChannelError` 必须把「重试有意义」和「重试没意义」分开——
    /// 这个区分是错误处理那一节的全部依据，合并成一个错误就没法写退避了。
    #[test]
    fn bad_token_is_not_retryable_but_unreachable_is() {
        assert!(ChannelError::Unreachable.worth_retrying());
        assert!(!ChannelError::BadToken.worth_retrying());
        assert!(!ChannelError::Malformed.worth_retrying());
    }
}
