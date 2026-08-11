//! 连接层：把守护进程里发生的事送到渠道上，把渠道上来的话敲进会话。
//!
//! **这是唯一有状态的地方**：谁是主人、哪条消息对应哪个会话、当前对着哪个
//! 会话。除此之外它什么都不存。
//!
//! **绝不 panic 到线程外面。** 手机通道死掉是遗憾，会话跟着死是灾难——
//! 同 `journal.rs` 那条「记不下来是记账的事，不该连累会话」一模一样的原则。
//!
//! ## `set_destination` 只从这里调用
//!
//! `Channel::set_destination` 早先有一版实现是渠道自己从收到的原始消息里
//! 现学出站目的地——先到先得。那个设计有个真实的安全漏洞：重新配对或换
//! 令牌之后，渠道的 `poll()` 读到的是「未经这里的 `accept()` 检查过的原始
//! 消息流」，一个抢在真正主人之前发消息的陌生人就能把出站目的地偷走，而
//! `accept()` 的「拒绝陌生人」只挡得住*入站*，挡不住这条独立的出站学习
//! 路径——入站安全测试全程照样绿，因为它们测的是入站。删掉了那条路，
//! `Bridge` 是**唯一**允许调用 `set_destination` 的地方，只有两个调用点：
//! `accept()` 配对成功时传 `Some(chat_id)`，`unpair()`（取消配对/换令牌）
//! 时传 `None`。别处如果也想调它，就是在重新引入同一个漏洞。
//!
//! ## 背景轮询线程没有语言可用
//!
//! `PhoneState::Broken` 装的是已经成文的人话，`i18n` 的组句照例要知道
//! `Lang`——但轮询线程不像 `PhoneSetToken` 那样绑在一次带着 `lang` 的请求
//! 上，它是守护进程自己起的后台活动，没有任何请求上下文可问。这里退回
//! 产品默认语言（中文，见 `CLAUDE.md`「默认中文 locale」），不是假装解决了
//! 这个问题——`BRIDGE_LANG` 就是这个决定唯一的落脚点，以后要是需要跟着
//! 用户在 TUI 里选的语言走，只用改这一个常量的取值来源。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::channel::{Channel, ChannelError, Incoming};
use crate::i18n::{msg, Lang};
use crate::proto::{PhoneBrokenReason, PhoneState, PhoneStatus};
use crate::session::recover;

/// 背景轮询线程自己组句时用的语言。见模块头注释「背景轮询线程没有语言
/// 可用」——这是一个拍出来的默认值，不是算出来的。
const BRIDGE_LANG: Lang = Lang::Zh;

/// 长轮询等一批更新最多等多久。Telegram 服务器自己会等到这个数字，
/// `channel::telegram::Telegram` 会在这个基础上再加读超时余量。
const POLL_TIMEOUT: Duration = Duration::from_secs(25);

/// 退避的上限。`worth_retrying()` 为真时才用得到——网络问题不该无限快地
/// 重试打爆日志，也不该真的等到用户以为功能死了。
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
    /// 配对之后只认这一个。`None` = 还没配对。
    owner: Mutex<Option<i64>>,
    /// 手机通知被用户整个关掉（`PhoneDisable`）之后置位。轮询线程在每次
    /// 循环顶端、以及每次处理完一条消息之后都会检查它——一旦为真就安静
    /// 退出，不再碰 `phone` 状态槽。**这不是「立刻停」**：线程可能正卡在
    /// 一次最多 25 秒的 `poll()` 网络调用里，这个标记只在下一次检查点才
    /// 生效。跟 `channel::Telegram::send` 文档里「重置之后不保证旧账号
    /// 收不到任何东西」是同一类有边界的承诺，不要假设比这更强。
    retired: AtomicBool,
}

impl Bridge {
    pub fn new(ch: Arc<dyn Channel>) -> Self {
        Bridge {
            ch,
            owner: Mutex::new(None),
            retired: AtomicBool::new(false),
        }
    }

    /// **整个功能唯一真会伤到用户的判定。** bot 用户名是公开可搜的，任何
    /// 人都能给它发消息，而这条消息一旦被认成主人发的，就会被敲进用户的
    /// 终端——第一个发消息的人成为主人，配对之后别的任何 chat 一律
    /// `Rejected`，永远不会被再考虑一次。
    pub fn accept(&self, msg: &Incoming) -> Accepted {
        let mut owner = recover(self.owner.lock());
        match *owner {
            None => {
                *owner = Some(msg.chat_id);
                // 先放开锁再去碰 `ch`——`accept()` 之后不会再摸 `owner`，
                // 没有理由让 `set_destination`（哪怕它今天不做网络调用）
                // 跨在这把锁的持有期里，见 `apply_phone_set_token` 那条
                // 「两次 lock() 必须是两条分开的语句」的同类纪律。
                drop(owner);
                self.ch.set_destination(Some(msg.chat_id));
                Accepted::Paired(msg.chat_id)
            }
            Some(o) if o == msg.chat_id => Accepted::FromOwner,
            Some(_) => Accepted::Rejected,
        }
    }

    /// 取消配对 / 换令牌：忘掉当前主人，出站目的地清空回 `None`。
    /// `set_destination` 的第二个、也是最后一个合法调用点——见模块头注释。
    ///
    /// 调用方是 `daemon.rs` 的 `Request::PhoneUnpair`。这里不删除 bot 本身
    /// 是否还能用——那是 `send()`/长轮询下一次失败与否的事，不是这个方法
    /// 该管的。
    pub fn unpair(&self) {
        *recover(self.owner.lock()) = None;
        self.ch.set_destination(None);
    }

    /// 手机通知被整个关掉（`x`）。见 `retired` 字段上的文档注释。
    pub fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
    }

    fn is_retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }
}

/// 下一次重试前该等多久。指数退避，从 1 秒开始每次翻倍，`MAX_BACKOFF`
/// 封顶——纯函数，不用真的睡一觉就能测。`attempt` 是这是连续第几次失败
/// （从 1 开始数，0 表示还没失败过、不该被调用到这个分支）。
fn backoff_for(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1);
    let secs = 1u64.checked_shl(shift).unwrap_or(u64::MAX);
    Duration::from_secs(secs).min(MAX_BACKOFF)
}

/// `poll()`/长轮询这一层的错误分类。**这里没有 chat 上下文**——
/// `getUpdates` 跟 `getMe` 一样问的是"这个令牌是谁"，不是"哪个 chat 拉黑了
/// 我"，所以 403 在这里只能当 `BadToken` 处理，跟 `daemon.rs::
/// phone_verify_token` 对 `get_me` 结果的判定是同一个理由、同一种映射。
/// **真正的 `BotBlocked` 只从 `send()` 产出**，那时候手上是一个确凿无疑
/// 的 chat id，见 `send_pairing_confirmation`。
///
/// 返回 `None` 表示 `ChannelError::worth_retrying()`——调用方该退避重试，
/// 不该停线程、不该动 `phone` 状态槽。
fn terminal_reason(e: ChannelError) -> Option<(PhoneBrokenReason, String)> {
    match e {
        ChannelError::Unreachable => None,
        ChannelError::BadToken | ChannelError::Blocked => Some((
            PhoneBrokenReason::BadToken,
            msg::phone_token_invalid(BRIDGE_LANG),
        )),
        ChannelError::Malformed => Some((
            PhoneBrokenReason::Unreachable,
            msg::phone_unreachable(BRIDGE_LANG),
        )),
    }
}

/// 配对成功那一刻，往新认下的主人发一句确认——不然用户填完令牌、在
/// Telegram 里发完第一条消息之后，手机上什么反应都没有，只有回头打开 TUI
/// 才能看到「已连上」，而这个功能存在的全部意义就是不用回头看 TUI。
///
/// 这也是 `PhoneBrokenReason::BotBlocked` 今天**唯一**的产出点——见
/// `terminal_reason` 上面那条注释：只有这里手上真的攥着一个刚刚被
/// `accept()` 认下的 chat id，403 在这里才是「这个 chat 拉黑了 bot」的
/// 真话，不是像 `getMe`/`getUpdates` 那样的猜测。`PhoneState::
/// has_confirmed_token()` 把 `BotBlocked` 当成「磁盘上有一份确认有效的
/// 令牌」——这个承诺在这里天然成立：能走到这一步，前面已经先有一次成功
/// 的 `apply_phone_set_token`（令牌和 bot 名字早就落盘了），再有一次
/// 成功的 `accept()`（配对本身也已经发生），`BotBlocked` 不是凭空冒出来的
/// 断言，是这两件已经发生的事之上的第三件事。
///
/// 返回 `true` 表示这次配对到此为止，轮询线程该退出——被拉黑的 chat 既
/// 收不到消息也不会再被认为在发消息，继续 `poll()` 没有意义。
fn send_pairing_confirmation(bridge: &Bridge, phone: &Mutex<PhoneStatus>) -> bool {
    match bridge
        .ch
        .send(&msg::phone_pairing_confirmation(BRIDGE_LANG))
    {
        Ok(_) => false,
        // 网络抖动、令牌本身的问题、读不懂的回包——这几种不该推翻刚刚
        // 生效的配对，配对已经是事实（`accept()` 已经认下了这个主人），
        // 一句问候没发出去不该把它撤销。留给下一次事件通知（不是这个
        // 任务的范围）自己的 send() 再报一次。
        Err(ChannelError::Unreachable)
        | Err(ChannelError::BadToken)
        | Err(ChannelError::Malformed) => false,
        Err(ChannelError::Blocked) => {
            let mut ph = recover(phone.lock());
            ph.state = PhoneState::Broken {
                reason: PhoneBrokenReason::BotBlocked,
                message: msg::phone_blocked(BRIDGE_LANG),
            };
            true
        }
    }
}

/// 轮询主体：一直 `poll()`，把配对/主人的消息交给 `accept()`，网络问题
/// 退避重试，不可恢复的错误就把 `Broken` 写进 `phone` 状态槽后退出——
/// 调用方（`daemon.rs`）该在那之后重新起一条线程才能恢复（换新令牌，或者
/// `PhoneUnpair` 从 `Broken { BotBlocked }` 里把同一个 `Bridge` 接回来）。
///
/// `sleep` 被注入而不是直接 `std::thread::sleep`——同 `telegram.rs::Sender`
/// 的道理，测试要能在毫秒级验证退避真的被调用过，而不是真的等上几分钟。
fn poll_forever(bridge: &Bridge, phone: &Mutex<PhoneStatus>, sleep: &dyn Fn(Duration)) {
    let mut attempt: u32 = 0;
    loop {
        if bridge.is_retired() {
            return;
        }
        match bridge.ch.poll(POLL_TIMEOUT) {
            Ok(incoming) => {
                attempt = 0;
                for m in incoming {
                    if bridge.is_retired() {
                        return;
                    }
                    match bridge.accept(&m) {
                        Accepted::Paired(_) => {
                            {
                                let mut ph = recover(phone.lock());
                                ph.state = PhoneState::Paired;
                            }
                            if send_pairing_confirmation(bridge, phone) {
                                return;
                            }
                        }
                        // 收到的话该敲进哪个会话是 Task 7 的活——这里只
                        // 负责认出「这是主人发的」，转发本身还没有地方接。
                        Accepted::FromOwner => {}
                        Accepted::Rejected => {}
                    }
                }
            }
            Err(e) if e.worth_retrying() => {
                attempt = attempt.saturating_add(1);
                sleep(backoff_for(attempt));
            }
            Err(e) => {
                // `terminal_reason` 对 `worth_retrying() == true` 的
                // `Unreachable` 返回 `None`，但那个分支已经在上面被
                // `e.worth_retrying()` 接走了，这里只会拿到 `Some`。
                if let Some((reason, message)) = terminal_reason(e) {
                    let mut ph = recover(phone.lock());
                    ph.state = PhoneState::Broken { reason, message };
                }
                return;
            }
        }
    }
}

/// 轮询线程的真正入口。**整个线程体包在 `catch_unwind` 里**——手机通道
/// 死掉是遗憾，会话跟着 panic 死是灾难，同模块头注释、同 `journal.rs`
/// 头注释那条一模一样的原则。`AssertUnwindSafe`：`poll_forever` 只通过
/// `Mutex`/`Arc` 触碰共享状态，两者本身已经是 panic 安全的（`Mutex`
/// 中毒之后走 `recover()`，不会把毒扩散到调用方）。
pub fn run(bridge: Arc<Bridge>, phone: Arc<Mutex<PhoneStatus>>) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        poll_forever(&bridge, &phone, &|d| std::thread::sleep(d));
    }));
    if outcome.is_err() {
        // 吞掉了。这里不去猜一句 `Broken` 文案该写什么原因——panic 不是
        // 一个 `ChannelError`，编不出真话。手机通道安静停掉，其余会话
        // 不受影响,才是这整条纪律唯一要保证的事。
        eprintln!("手机通知线程出了内部错误，已经停掉——不影响正在跑的会话");
    }
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

    /// 记录每一次调用的假渠道：`accept()`/轮询循环两组测试共用。
    /// `poll_script` 空了之后一律回 `Ok(空批次)`——这不是每条测试都要用到
    /// 的默认值，只是让"脚本比实际调用次数短"这种笔误不会连带炸穿别的
    /// 断言。
    #[derive(Default)]
    struct FakeChannel {
        destinations: Mutex<Vec<Option<i64>>>,
        sent: Mutex<Vec<String>>,
        send_result: Mutex<Option<Result<crate::channel::MsgId, ChannelError>>>,
        poll_script: Mutex<VecDeque<Result<Vec<Incoming>, ChannelError>>>,
        poll_calls: Mutex<u32>,
    }

    impl Channel for FakeChannel {
        fn send(&self, text: &str) -> Result<crate::channel::MsgId, ChannelError> {
            recover(self.sent.lock()).push(text.to_string());
            recover(self.send_result.lock()).clone().unwrap_or(Ok(0))
        }

        fn poll(&self, _timeout: Duration) -> Result<Vec<Incoming>, ChannelError> {
            *recover(self.poll_calls.lock()) += 1;
            recover(self.poll_script.lock())
                .pop_front()
                .unwrap_or(Ok(Vec::new()))
        }

        fn set_destination(&self, chat: Option<i64>) {
            recover(self.destinations.lock()).push(chat);
        }
    }

    impl Bridge {
        fn for_test() -> Self {
            Bridge::new(Arc::new(FakeChannel::default()))
        }
    }

    fn test_phone() -> Mutex<PhoneStatus> {
        Mutex::new(PhoneStatus {
            state: PhoneState::WaitingForPairing,
            bot: Some("my_dct_bot".into()),
            owner: None,
        })
    }

    // ———— accept(): 配对与拒绝，见模块头注释 ————

    /// 第一个发消息的人成为主人。
    #[test]
    fn the_first_person_to_message_becomes_the_owner() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(111, "在吗")), Accepted::Paired(111));
        assert_eq!(b.accept(&msg(111, "先跑完")), Accepted::FromOwner);
    }

    /// **bot 用户名是公开可搜的，任何人都能给它发消息，而这个功能会把消息
    /// 敲进用户的终端。** 这条测试破了就等于任何人都能往用户机器上敲字。
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

    // ———— set_destination: Bridge 是唯一调用方 ————

    /// 配对那一刻必须把出站目的地告诉渠道，否则确认消息、以后的事件通知
    /// 都没地方发——`accept()` 是 `set_destination(Some(_))` 唯一的调用点。
    #[test]
    fn accepting_the_first_message_tells_the_channel_where_to_send() {
        let ch = Arc::new(FakeChannel::default());
        let b = Bridge::new(ch.clone());
        b.accept(&msg(111, "在吗"));
        assert_eq!(*recover(ch.destinations.lock()), vec![Some(111)]);

        // 之后不管是主人还是陌生人发消息，都不该再调一次——目的地已经
        // 定了，`accept()` 的 `FromOwner`/`Rejected` 分支不碰 `ch`。
        b.accept(&msg(111, "继续"));
        b.accept(&msg(222, "闯入"));
        assert_eq!(*recover(ch.destinations.lock()), vec![Some(111)]);
    }

    /// 取消配对 / 换令牌必须清空出站目的地，不然旧主人换了新令牌之后还能
    /// 收到不该收到的东西。`unpair()` 是 `set_destination(None)` 唯一的
    /// 调用点，见模块头注释「`set_destination` 只从这里调用」。
    #[test]
    fn unpair_clears_the_channel_destination_and_lets_someone_new_pair() {
        let ch = Arc::new(FakeChannel::default());
        let b = Bridge::new(ch.clone());
        b.accept(&msg(111, "在吗"));
        b.unpair();
        assert_eq!(*recover(ch.destinations.lock()), vec![Some(111), None]);

        // 忘掉主人之后，下一条消息重新触发配对——不是永远锁死在第一个人。
        assert_eq!(b.accept(&msg(999, "换我了")), Accepted::Paired(999));
        assert_eq!(
            *recover(ch.destinations.lock()),
            vec![Some(111), None, Some(999)]
        );
    }

    // ———— backoff_for: 纯函数，退避策略 ————

    #[test]
    fn backoff_starts_at_one_second_and_doubles() {
        assert_eq!(backoff_for(1), Duration::from_secs(1));
        assert_eq!(backoff_for(2), Duration::from_secs(2));
        assert_eq!(backoff_for(3), Duration::from_secs(4));
        assert_eq!(backoff_for(9), Duration::from_secs(256));
    }

    #[test]
    fn backoff_is_capped_at_five_minutes() {
        assert_eq!(backoff_for(10), MAX_BACKOFF);
        assert_eq!(backoff_for(1000), MAX_BACKOFF, "不能溢出，也不能超过上限");
    }

    // ———— terminal_reason: 哪些错误值得停线程 ————

    #[test]
    fn unreachable_is_not_terminal_it_is_worth_retrying() {
        assert_eq!(terminal_reason(ChannelError::Unreachable), None);
    }

    #[test]
    fn bad_token_and_blocked_both_mean_bad_token_here_no_chat_context() {
        let (bad_token_reason, _) = terminal_reason(ChannelError::BadToken).unwrap();
        let (blocked_reason, _) = terminal_reason(ChannelError::Blocked).unwrap();
        assert_eq!(bad_token_reason, PhoneBrokenReason::BadToken);
        assert_eq!(
            blocked_reason,
            PhoneBrokenReason::BadToken,
            "poll() 没有 chat 上下文，403 在这里不能变成 BotBlocked"
        );
    }

    #[test]
    fn malformed_is_reported_as_unreachable_not_a_dead_end() {
        let (reason, _) = terminal_reason(ChannelError::Malformed).unwrap();
        assert_eq!(reason, PhoneBrokenReason::Unreachable);
    }

    // ———— poll_forever: 轮询循环本身 ————

    fn no_sleep() -> Box<dyn Fn(Duration)> {
        Box::new(|_| {})
    }

    /// 主线：收到第一条消息就配对、发确认、把 `phone` 状态推进到 `Paired`。
    /// 脚本第二次 `poll()` 回一个终态错误让循环自己退出——不然这条测试
    /// 会真的转成死循环。
    #[test]
    fn a_pairing_message_flips_phone_status_to_paired_and_sends_a_confirmation() {
        let ch = Arc::new(FakeChannel::default());
        recover(ch.poll_script.lock()).push_back(Ok(vec![msg(111, "在吗")]));
        recover(ch.poll_script.lock()).push_back(Err(ChannelError::BadToken));
        let bridge = Bridge::new(ch.clone());
        let phone = test_phone();

        poll_forever(&bridge, &phone, &no_sleep());

        assert_eq!(
            recover(ch.sent.lock()).len(),
            1,
            "配对成功要发一句确认，不然用户看不到任何反应"
        );
        assert_eq!(*recover(ch.destinations.lock()), vec![Some(111)]);
        // 循环靠第二次 poll() 的 BadToken 才停下来——停下来时状态是
        // Broken，不是 Paired，但发送日志已经证明 Paired 分支真的跑过了。
        assert!(matches!(
            recover(phone.lock()).state,
            PhoneState::Broken {
                reason: PhoneBrokenReason::BadToken,
                ..
            }
        ));
    }

    /// 网络问题（`worth_retrying() == true`）退避重试，不写 `Broken`，
    /// 不停线程——脚本用两次 `Unreachable` 之后一次成功再一次终态错误来
    /// 证明"重试之后真的还会再 poll()"，而不是第一次失败就放弃。
    #[test]
    fn a_retryable_error_backs_off_and_keeps_polling() {
        let ch = Arc::new(FakeChannel::default());
        {
            let mut script = recover(ch.poll_script.lock());
            script.push_back(Err(ChannelError::Unreachable));
            script.push_back(Err(ChannelError::Unreachable));
            script.push_back(Ok(Vec::new()));
            // 第三次失败紧跟在一次成功后面：如果 `attempt` 没有在那次成功
            // 时清零，这次该睡的秒数会接着前面的 2 秒继续涨到 4 秒，而不是
            // 重新从 1 秒数起——这是 `attempt = 0` 那一行唯一会被这条测试
            // 揪出来的地方,少了它前两条断言（消费次数、终态原因）照样绿。
            script.push_back(Err(ChannelError::Unreachable));
            script.push_back(Err(ChannelError::BadToken));
        }
        let bridge = Bridge::new(ch.clone());
        let phone = test_phone();
        let slept = Mutex::new(Vec::new());

        poll_forever(&bridge, &phone, &|d| recover(slept.lock()).push(d));

        assert_eq!(*recover(ch.poll_calls.lock()), 5, "五次脚本都该被消费掉");
        assert_eq!(
            *recover(slept.lock()),
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(1),
            ],
            "一次成功之后重新失败，退避必须从 1 秒重新数起，不能接着涨"
        );
        assert!(matches!(
            recover(phone.lock()).state,
            PhoneState::Broken {
                reason: PhoneBrokenReason::BadToken,
                ..
            }
        ));
    }

    /// `retired` 在下一次循环顶端就该生效——不需要脚本准备任何消息，
    /// 只要标记先置位，`poll()` 干脆不该被调用。
    #[test]
    fn retiring_before_the_loop_starts_stops_it_from_polling_at_all() {
        let ch = Arc::new(FakeChannel::default());
        let bridge = Bridge::new(ch.clone());
        bridge.retire();
        let phone = test_phone();

        poll_forever(&bridge, &phone, &no_sleep());

        assert_eq!(*recover(ch.poll_calls.lock()), 0);
    }

    /// 配对确认发送失败并且是 403（这个 chat 拉黑了 bot）——`BotBlocked`
    /// 唯一的产出点。有意思的地方在于这不需要第二次 `poll()`：`Blocked`
    /// 让 `send_pairing_confirmation` 直接报告"到此为止"，循环立刻退出。
    #[test]
    fn being_blocked_right_after_pairing_produces_bot_blocked_and_stops() {
        let ch = Arc::new(FakeChannel::default());
        recover(ch.poll_script.lock()).push_back(Ok(vec![msg(111, "在吗")]));
        *recover(ch.send_result.lock()) = Some(Err(ChannelError::Blocked));
        let bridge = Bridge::new(ch.clone());
        let phone = test_phone();

        poll_forever(&bridge, &phone, &no_sleep());

        assert_eq!(*recover(ch.poll_calls.lock()), 1, "不该再 poll 第二次");
        let ph = recover(phone.lock());
        match &ph.state {
            PhoneState::Broken { reason, message } => {
                assert_eq!(*reason, PhoneBrokenReason::BotBlocked);
                assert!(!message.is_empty());
            }
            other => panic!("期待 Broken{{BotBlocked}}，得到 {other:?}"),
        }
    }

    /// 陌生人的消息永远不会让 `Accepted::Paired` 冒出来，`poll_forever`
    /// 也就永远不会为它发确认——`accept()` 自己的安全测试已经钉住了
    /// `Rejected` 本身，这条钉的是循环层面：陌生人的消息不产生任何
    /// 可观察的副作用（不发消息、不动 `phone` 状态）。
    #[test]
    fn a_strangers_message_after_pairing_produces_no_side_effects_in_the_loop() {
        let ch = Arc::new(FakeChannel::default());
        {
            let mut script = recover(ch.poll_script.lock());
            script.push_back(Ok(vec![msg(111, "在吗")]));
            script.push_back(Ok(vec![msg(222, "我也要")]));
            script.push_back(Err(ChannelError::BadToken));
        }
        let bridge = Bridge::new(ch.clone());
        let phone = test_phone();

        poll_forever(&bridge, &phone, &no_sleep());

        // 一次是配对确认；陌生人那条不该再让 sent 多一条。
        assert_eq!(recover(ch.sent.lock()).len(), 1);
    }

    // ———— run(): panic 不许冒出线程 ————

    struct PanickingChannel;
    impl Channel for PanickingChannel {
        fn send(&self, _text: &str) -> Result<crate::channel::MsgId, ChannelError> {
            Ok(0)
        }
        fn poll(&self, _timeout: Duration) -> Result<Vec<Incoming>, ChannelError> {
            panic!("模拟渠道内部炸了")
        }
        fn set_destination(&self, _chat: Option<i64>) {}
    }

    /// **这是模块头注释「绝不 panic 到线程外面」的直接验证。** 如果
    /// `run()` 少了 `catch_unwind`，这条测试自己就会因为一次未捕获的
    /// panic 而失败（`#[test]` 的默认行为）——测得到，不是靠读代码相信。
    #[test]
    fn a_panic_inside_the_loop_never_escapes_run() {
        let bridge = Arc::new(Bridge::new(Arc::new(PanickingChannel)));
        let phone = Arc::new(test_phone());

        run(bridge, phone); // 不 panic 就是这条测试的全部断言
    }
}
