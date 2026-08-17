//! 连接层：把守护进程里发生的事送到渠道上，把渠道上来的话敲进会话。
//!
//! **这是唯一有状态的地方**：谁是主人、哪条消息对应哪个会话、当前对着哪个
//! 会话。除此之外它什么都不存——渠道层（`channel/mod.rs`）不认识、也不记着
//! 谁是主人，见那边 Ruling 8 的注释。
//!
//! **绝不 panic 到线程外面。** 手机通道死掉是遗憾，会话跟着死是灾难——
//! 同 `journal.rs` 那条「记不下来是记账的事，不该连累会话」。`spawn()` 把
//! 整个线程体包在 `catch_unwind` 里，就是为了这一条。
//!
//! bot 用户名是公开可搜的，任何人都能给它发消息，而这个功能会把消息敲进
//! 用户的终端。**第一个在填完令牌之后发消息的人成为主人，其余所有人的消息
//! 永远丢弃。** `accept()` 是这条规则唯一的实现——它错了，就是任何人都能
//! 往用户机器上敲字。

use crate::channel::{Channel, ChannelError, Incoming};
use crate::proto::{PhoneState, PhoneStatus};
use crate::session::recover;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 长轮询一次最多挂多久。Telegram `getUpdates` 用同一个数字当查询参数。
const POLL_TIMEOUT: Duration = Duration::from_secs(25);
/// 退避的起点。
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
/// 退避的上限：五分钟。超过这个数就没有再翻倍的意义——用户等半小时和等
/// 五分钟已经没区别，翻倍下去只会让「网络恢复了但还要再等好久」变得更糟。
const MAX_BACKOFF: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accepted {
    /// 这条消息完成了配对，发信人成为主人。
    Paired(i64),
    FromOwner,
    /// 不是主人发的，**丢弃**。
    Rejected,
}

pub struct Bridge {
    ch: Arc<dyn Channel>,
    /// Task 4 的共享状态槽——`Request::PhoneStatus` 唯一的答案来源。
    /// bridge 线程写，daemon 的请求处理线程读，同一把 `Mutex`，Ruling 3。
    phone: Arc<Mutex<PhoneStatus>>,
    /// 配对之后只认这一个。`None` = 还没配对。
    owner: Mutex<Option<i64>>,
    // …… 消息映射与当前会话见 Task 7
}

impl Bridge {
    pub fn new(ch: Arc<dyn Channel>, phone: Arc<Mutex<PhoneStatus>>) -> Bridge {
        Bridge {
            ch,
            phone,
            owner: Mutex::new(None),
        }
    }

    /// 只测 `accept()` 用——渠道和状态槽都是不会被读的占位符。
    #[cfg(test)]
    fn for_test() -> Bridge {
        Bridge::new(Arc::new(tests::NeverCalled), tests::blank_status())
    }

    /// 这条消息该不该数？**这是整个功能唯一真会伤到用户的地方。**
    pub fn accept(&self, msg: &Incoming) -> Accepted {
        let mut owner = recover(self.owner.lock());
        match *owner {
            None => {
                *owner = Some(msg.chat_id);
                Accepted::Paired(msg.chat_id)
            }
            Some(o) if o == msg.chat_id => Accepted::FromOwner,
            Some(_) => Accepted::Rejected,
        }
    }

    /// 一条消息落地之后，状态槽该跟着改什么。**只处理 `Paired`**——
    /// `FromOwner`/`Rejected` 不该动状态槽（往会话里敲字是 Task 7 的事，
    /// 丢弃陌生人不留任何痕迹是这条规则的全部意义）。
    fn dispatch(&self, msg: &Incoming) {
        if let Accepted::Paired(chat_id) = self.accept(msg) {
            let mut st = recover(self.phone.lock());
            st.state = PhoneState::Paired;
            // 这里没有真实姓名可用——`Incoming` 没带 Telegram 的
            // `message.from.username`（Task 2 没有解析它），能给的只有
            // chat id 本身。诚实地显示一个数字，好过编一个不存在的名字。
            st.owner = Some(chat_id.to_string());
        }
    }

    /// 补 bot 用户名，带跟轮询同一套退避。**必须在进入轮询循环之前做**——
    /// 见 Task 4 的遗留发现：daemon 重启之后 `bot` 一直是 `None`，配对页
    /// 给不出「去找谁发消息」这句话，只要这段窗口够短就没关系。
    ///
    /// 返回 `false` 表示令牌本身就不能用（`BadToken`）或者回包读不懂
    /// （`Malformed`）——两种都已经把 `Broken` 写进状态槽了，调用方不该
    /// 再进轮询循环，那只会拿同一个坏令牌把 `getUpdates` 也打挂一遍。
    fn ensure_bot_known(&self) -> bool {
        let mut delay = INITIAL_BACKOFF;
        loop {
            match self.ch.get_me() {
                Ok(username) => {
                    recover(self.phone.lock()).bot = Some(username);
                    return true;
                }
                Err(e) if e.worth_retrying() => {
                    std::thread::sleep(delay);
                    delay = next_backoff(delay);
                }
                Err(e) => {
                    self.mark_broken(e);
                    return false;
                }
            }
        }
    }

    fn mark_broken(&self, e: ChannelError) {
        recover(self.phone.lock()).state = PhoneState::Broken(broken_message(e));
    }

    /// 轮询主循环。**不要直接调用它**——用模块级的 `spawn()`，那边包了
    /// `catch_unwind`；这个方法本身可能 panic（比如锁中毒之外的 bug），
    /// 隔离全靠调用方。
    fn run(&self) {
        if !self.ensure_bot_known() {
            return;
        }

        let mut delay = INITIAL_BACKOFF;
        loop {
            match self.ch.poll(POLL_TIMEOUT) {
                Ok(incoming) => {
                    delay = INITIAL_BACKOFF;
                    for msg in &incoming {
                        self.dispatch(msg);
                    }
                }
                Err(e) if e.worth_retrying() => {
                    std::thread::sleep(delay);
                    delay = next_backoff(delay);
                }
                Err(e) => {
                    self.mark_broken(e);
                    return;
                }
            }
        }
    }
}

/// 指数退避，上限五分钟。纯函数——不用真的睡一觉就能测。
fn next_backoff(current: Duration) -> Duration {
    (current.saturating_mul(2)).min(MAX_BACKOFF)
}

/// `PhoneState::Broken` 要装的那句人话。**绝不带原始错误、绝不带令牌**——
/// `ChannelError` 本身就不携带令牌或原始回包内容，从值的形状上就堵死了
/// 「手滑把令牌带出来」这条路，不用靠这里的作者自觉。
///
/// 只有 `worth_retrying()` 为假的两种（`BadToken`/`Malformed`）会走到这里
/// ——`Unreachable` 永远在退避循环里重试，不会被判定成「停下」。
fn broken_message(e: ChannelError) -> String {
    match e {
        ChannelError::BadToken => "手机通知的令牌不能用了，去设置页重新粘贴一遍".to_string(),
        ChannelError::Unreachable => "手机通知断开了，正在自动重连".to_string(),
        ChannelError::Malformed => "手机通知收到了读不懂的数据，去设置页重新连一下".to_string(),
    }
}

/// 把 bridge 起在后台线程上。**整个线程体包在 `catch_unwind` 里**——
/// 一个手机通道死掉是遗憾，一个会话死掉是灾难，两者绝不能是同一件事。
pub fn spawn(ch: Arc<dyn Channel>, phone: Arc<Mutex<PhoneStatus>>) {
    std::thread::spawn(move || {
        let bridge = Bridge::new(ch, phone);
        let _ = catch_unwind(AssertUnwindSafe(|| bridge.run()));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn msg(chat: i64, text: &str) -> Incoming {
        Incoming {
            text: text.into(),
            reply_to: None,
            chat_id: chat,
        }
    }

    pub(super) fn blank_status() -> Arc<Mutex<PhoneStatus>> {
        Arc::new(Mutex::new(PhoneStatus {
            state: PhoneState::Off,
            bot: None,
            owner: None,
        }))
    }

    /// `Bridge::for_test()` 只用来测 `accept()`——这个渠道要是真被调用，
    /// 说明某处测试意外触发了轮询/发送，那是一个测试设计错误，让它 panic
    /// 比悄悄返回假数据更容易发现。
    pub(super) struct NeverCalled;
    impl Channel for NeverCalled {
        fn send(&self, _to: i64, _text: &str) -> Result<crate::channel::MsgId, ChannelError> {
            panic!("accept() 测试不该碰渠道的 send()")
        }
        fn poll(&self, _timeout: Duration) -> Result<Vec<Incoming>, ChannelError> {
            panic!("accept() 测试不该碰渠道的 poll()")
        }
        fn get_me(&self) -> Result<String, ChannelError> {
            panic!("accept() 测试不该碰渠道的 get_me()")
        }
    }

    // ---- 三条来自 brief 的失败测试，配对规则的全部依据 ----

    /// 第一个发消息的人成为主人。
    #[test]
    fn the_first_person_to_message_becomes_the_owner() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(111, "在吗")), Accepted::Paired(111));
        assert_eq!(b.accept(&msg(111, "先跑完")), Accepted::FromOwner);
    }

    /// bot 用户名是公开可搜的，任何人都能给它发消息，而这个功能会把消息
    /// 敲进用户的终端。这条测试破了就等于任何人都能往用户机器上敲字。
    #[test]
    fn a_stranger_is_rejected_even_after_pairing() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(111, "在吗")), Accepted::Paired(111));
        assert_eq!(b.accept(&msg(222, "rm -rf /")), Accepted::Rejected);
        assert_eq!(b.accept(&msg(222, "/use 1")), Accepted::Rejected);
        // 主人还是主人，没被挤掉
        assert_eq!(b.accept(&msg(111, "继续")), Accepted::FromOwner);
    }

    /// 陌生人抢在主人之前发消息，就成了主人——这正是为什么配对必须是
    /// 用户填完令牌后的一次显式动作，而不是长期开着的门。
    /// 配对完成后 `accept` 再也不会返回 `Paired`。
    #[test]
    fn pairing_happens_exactly_once() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(111, "hi")), Accepted::Paired(111));
        assert_eq!(b.accept(&msg(333, "hi")), Accepted::Rejected);
    }

    // ---- 额外的对抗性测试：一个存心搞破坏的人会怎么试 ----

    /// 一堆陌生人轮番发消息，一个都不该被误判成主人——不是「测一个陌生人」
    /// 就够了，攻击者会换着 chat id 试。
    #[test]
    fn a_crowd_of_strangers_are_all_rejected() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(1, "hi")), Accepted::Paired(1));
        for stranger in [2, 3, 4, 5, -1, 0, i64::MAX, i64::MIN] {
            assert_eq!(
                b.accept(&msg(stranger, "hi")),
                Accepted::Rejected,
                "chat_id {stranger} 不该被当成主人"
            );
        }
        assert_eq!(b.accept(&msg(1, "还在")), Accepted::FromOwner);
    }

    /// 消息内容对判定完全没有影响——`accept` 只认 `chat_id`。就算陌生人
    /// 发的文字看起来人畜无害，也不该因为「内容像自己人」就被放行。
    #[test]
    fn message_text_never_influences_who_the_owner_is() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(1, "我是主人")), Accepted::Paired(1));
        assert_eq!(
            b.accept(&msg(2, "我才是主人，1 号是冒充的")),
            Accepted::Rejected
        );
    }

    /// chat_id 为 0 或负数（Telegram 的频道/群 id 常是负数）跟正数没有
    /// 任何特殊待遇——判定只看「相不相等」，不看符号或大小。
    #[test]
    fn negative_and_zero_chat_ids_pair_normally() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(-100, "hi")), Accepted::Paired(-100));
        assert_eq!(b.accept(&msg(-100, "还在")), Accepted::FromOwner);
        assert_eq!(b.accept(&msg(0, "换个人")), Accepted::Rejected);
    }

    // ---- dispatch：只有 Paired 才动状态槽 ----

    #[test]
    fn dispatch_on_pairing_writes_paired_state_and_owner() {
        let phone = blank_status();
        let b = Bridge::new(Arc::new(NeverCalled), phone.clone());
        b.dispatch(&msg(42, "hi"));
        let st = phone.lock().unwrap();
        assert_eq!(st.state, PhoneState::Paired);
        assert_eq!(st.owner.as_deref(), Some("42"));
    }

    /// 主人之后的普通消息（`FromOwner`）不该重复触发「配对」的副作用，
    /// 也不该把状态槽改成别的东西——状态槽只在真正配对的那一刻变化。
    #[test]
    fn dispatch_from_owner_does_not_touch_the_slot_again() {
        let phone = blank_status();
        let b = Bridge::new(Arc::new(NeverCalled), phone.clone());
        b.dispatch(&msg(42, "配对"));
        // 手动把状态槽改成一个跟「配对」不同的值，确认 FromOwner 不会把它
        // 又改回去、也不会动 owner 字段。
        {
            let mut st = phone.lock().unwrap();
            st.owner = Some("哨兵".to_string());
        }
        b.dispatch(&msg(42, "第二条"));
        let st = phone.lock().unwrap();
        assert_eq!(
            st.owner.as_deref(),
            Some("哨兵"),
            "FromOwner 不该覆盖状态槽"
        );
    }

    /// 陌生人的消息完全不该碰状态槽——连尝试写都不该有。
    #[test]
    fn dispatch_from_a_stranger_leaves_the_slot_untouched() {
        let phone = blank_status();
        let b = Bridge::new(Arc::new(NeverCalled), phone.clone());
        b.dispatch(&msg(1, "先配对")); // 1 号成为主人
        {
            let mut st = phone.lock().unwrap();
            st.state = PhoneState::WaitingForPairing;
            st.owner = None;
        }
        b.dispatch(&msg(2, "我是陌生人"));
        let st = phone.lock().unwrap();
        assert_eq!(st.state, PhoneState::WaitingForPairing);
        assert_eq!(st.owner, None);
    }

    // ---- 退避：纯函数，不用真的睡 ----

    #[test]
    fn backoff_doubles_and_caps_at_five_minutes() {
        let mut d = INITIAL_BACKOFF;
        assert_eq!(d, Duration::from_secs(1));
        d = next_backoff(d);
        assert_eq!(d, Duration::from_secs(2));
        d = next_backoff(d);
        assert_eq!(d, Duration::from_secs(4));
        // 一路翻倍下去，最终必须停在上限，不能超过。
        for _ in 0..20 {
            d = next_backoff(d);
        }
        assert_eq!(d, MAX_BACKOFF);
    }

    // ---- broken_message：人话，绝不是原始错误或令牌 ----

    #[test]
    fn broken_message_is_prose_not_a_debug_dump() {
        for e in [ChannelError::BadToken, ChannelError::Malformed] {
            let text = broken_message(e);
            // 不是 `format!("{e:?}")` 那种调试输出——判定标准很简单：
            // 调试输出里没有中文，人话里必须有。
            assert!(
                text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "该是写给人看的一句话，不是原始错误: {text}"
            );
            assert!(!text.is_empty());
        }
    }

    /// 令牌本身根本没有任何入口能进到 `broken_message`——`ChannelError`
    /// 的每个变体都不携带字符串。这条测试把这个结构性事实钉在这里：
    /// 就算故意塞一个「看起来像令牌」的东西进错误值，也做不到，因为
    /// 类型上就不允许。这里改成对每种变体的输出做一次可疑片段扫描，
    /// 防止将来有人把 `ChannelError` 改成带 `String` 字段又在这里拼接。
    #[test]
    fn broken_message_never_contains_anything_token_shaped() {
        for e in [
            ChannelError::BadToken,
            ChannelError::Unreachable,
            ChannelError::Malformed,
        ] {
            let text = broken_message(e);
            assert!(!text.contains(':'), "冒号分隔的令牌形状不该出现: {text}");
            assert!(
                !text.to_lowercase().contains("token"),
                "不该出现英文 token 字样: {text}"
            );
        }
    }

    // ---- get_me / 轮询主循环：mock channel，不碰网络 ----

    /// 可编程的 mock：`get_me` 给定的一个结果，`poll` 是一串排好队的结果，
    /// 用完就返回空批次（不是错误）。两边都记调用次数，用来断言
    /// 「令牌一开始就是坏的，就不该再进轮询循环」。
    struct MockChannel {
        get_me_result: Result<String, ChannelError>,
        poll_results: Mutex<VecDeque<Result<Vec<Incoming>, ChannelError>>>,
        poll_calls: Mutex<u32>,
    }

    impl Channel for MockChannel {
        fn send(&self, _to: i64, _text: &str) -> Result<crate::channel::MsgId, ChannelError> {
            unimplemented!("这一路测试不需要 send")
        }
        fn poll(&self, _timeout: Duration) -> Result<Vec<Incoming>, ChannelError> {
            *self.poll_calls.lock().unwrap() += 1;
            self.poll_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(Vec::new()))
        }
        fn get_me(&self) -> Result<String, ChannelError> {
            self.get_me_result.clone()
        }
    }

    /// 正常路径：`get_me` 先补上 bot 名字，然后处理一条配对消息，
    /// 再收到一个不可重试的错误（`BadToken`）就停下、把 `Broken` 写进去。
    #[test]
    fn run_populates_bot_then_pairs_then_stops_on_bad_token() {
        let phone = blank_status();
        let ch = Arc::new(MockChannel {
            get_me_result: Ok("my_dct_bot".to_string()),
            poll_results: Mutex::new(VecDeque::from([
                Ok(vec![msg(111, "hi")]),
                Err(ChannelError::BadToken),
            ])),
            poll_calls: Mutex::new(0),
        });
        let bridge = Bridge::new(ch.clone(), phone.clone());
        bridge.run();

        let st = phone.lock().unwrap();
        assert_eq!(st.bot.as_deref(), Some("my_dct_bot"));
        match &st.state {
            PhoneState::Broken(_) => {}
            other => panic!("该停在 Broken，得到 {other:?}"),
        }
        assert_eq!(*ch.poll_calls.lock().unwrap(), 2);
    }

    /// 令牌一开始就是坏的：`get_me` 直接 `BadToken`，`run()` 必须
    /// **一次都不**调用 `poll()`——拿一个已知坏掉的令牌去打 `getUpdates`
    /// 纯属浪费，而且会让「令牌坏了」这件事被网络错误的退避逻辑掩盖掉。
    #[test]
    fn a_bad_token_at_startup_never_reaches_poll() {
        let phone = blank_status();
        let ch = Arc::new(MockChannel {
            get_me_result: Err(ChannelError::BadToken),
            poll_results: Mutex::new(VecDeque::new()),
            poll_calls: Mutex::new(0),
        });
        let bridge = Bridge::new(ch.clone(), phone.clone());
        bridge.run();

        assert_eq!(*ch.poll_calls.lock().unwrap(), 0);
        let st = phone.lock().unwrap();
        assert_eq!(st.bot, None);
        match &st.state {
            PhoneState::Broken(_) => {}
            other => panic!("该停在 Broken，得到 {other:?}"),
        }
    }

    /// `run()` 里的 panic 必须被 `catch_unwind` 接住，不能往外冒——这是
    /// `spawn()` 存在的全部理由：手机通道死了是遗憾，daemon 或者别的会话
    /// 死了才是灾难。
    #[test]
    fn a_panic_inside_run_is_caught_not_propagated() {
        struct PanicsOnPoll;
        impl Channel for PanicsOnPoll {
            fn send(&self, _to: i64, _text: &str) -> Result<crate::channel::MsgId, ChannelError> {
                unimplemented!()
            }
            fn poll(&self, _timeout: Duration) -> Result<Vec<Incoming>, ChannelError> {
                panic!("模拟渠道内部的一个 bug")
            }
            fn get_me(&self) -> Result<String, ChannelError> {
                Ok("bot".to_string())
            }
        }
        let phone = blank_status();
        let bridge = Bridge::new(Arc::new(PanicsOnPoll), phone);
        let result = catch_unwind(AssertUnwindSafe(|| bridge.run()));
        assert!(
            result.is_err(),
            "run() 里的 panic 该被外面的 catch_unwind 接住"
        );
    }
}
