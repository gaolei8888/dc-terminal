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
//! `Bridge` 是**唯一**允许调用 `set_destination` 的地方，一共三个调用点：
//! `accept()` 配对成功时传 `Some(chat_id)`，`unpair()`（取消配对/换令牌）
//! 时传 `None`，`Bridge::new_with_owner()`（daemon 重启后从磁盘恢复一个
//! 已经配对过的主人）时传 `Some(persisted_id)`。**第三个调用点不是重新
//! 引入那个漏洞**：它不是从原始消息流里现学出站目的地，恢复的值来自
//! `accept()` 自己早前已经做出的决定——那一刻 `on_owner_changed` 回调把
//! 它落了盘（见 `secrets::PHONE_OWNER_KEY`），这里只是把 daemon 自己已经
//! 授权过的一个事实重新装回内存，不是从任何未经检查的输入里现猜。别处
//! 如果也想调它，就是在重新引入同一个漏洞。
//!
//! ## 主人 id 要落盘，不然重启把配对窗口摆回过去（Critical 1）
//!
//! 一条新起的 `Bridge` 默认 `owner: None`——如果守护进程每次重启都用
//! `Bridge::new()`（全新配对），那么已经配对过的用户每次重启都会被悄悄
//! 重新打开一轮配对，而 `channel::telegram::Telegram` 的 `offset` 从 0
//! 开始意味着这轮"重新打开"看到的第一批消息不是"现在"，是 Telegram
//! 服务器上攒着的、最长将近 24 小时的历史积压——**配对窗口不但重新
//! 打开，还倒退回了过去某一刻**，一个几小时前发过消息的陌生人不需要
//! 在场、不需要够快，就能赢下配对。这直接违反 `pairing_happens_
//! exactly_once` 自己写的理由：「配对必须是用户填完令牌后的一次显式
//! 动作，而不是长期开着的门」。
//!
//! 两个互补的修复，缺一不可：
//! 1. **持久化 + 恢复**：配对那一刻 `accept()` 通过 `on_owner_changed`
//!    回调把 chat id 交给调用方落盘（`daemon.rs` 接的是
//!    `secrets::PHONE_OWNER_KEY`），下次启动 `daemon.rs` 用
//!    `Bridge::new_with_owner()` 把它接回来——**重启是恢复，不是重新
//!    配对**。
//! 2. **积压清零**：就算是全新配对（`owner` 确实是 `None`），第一次真正
//!    长轮询之前也要先把 Telegram 攒着的积压吃掉——`discard_backlog()`。
//!    只有第一条不够：一个用户从没配对过、刚填完令牌的场景下，`owner`
//!    本来就是 `None`，光靠"恢复持久化的主人"救不了这种情况，必须两条
//!    都做。
//!
//! ## 背景轮询线程没有语言可用
//!
//! `PhoneState::Broken` 装的是已经成文的人话，`i18n` 的组句照例要知道
//! `Lang`——但轮询线程不像 `PhoneSetToken` 那样绑在一次带着 `lang` 的请求
//! 上，它是守护进程自己起的后台活动，没有任何请求上下文可问。这里退回
//! 产品默认语言（中文，见 `CLAUDE.md`「默认中文 locale」），不是假装解决了
//! 这个问题——`BRIDGE_LANG` 就是这个决定唯一的落脚点，以后要是需要跟着
//! 用户在 TUI 里选的语言走，只用改这一个常量的取值来源。这也意味着一个
//! 选了英文的用户，手机上收到的第一条确认消息、以及后台线程自己写的
//! `Broken` 文案，今天都是中文——已知的、故意留下的缺口，不是漏改。

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

/// 什么都不干的持久化回调——`Bridge::new()`（测试、以及任何不关心重启后
/// 恢复的调用方）用它垫底，不必每次都手写一个空闭包。
fn no_persistence(_: Option<i64>) {}

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
    /// 主人变了（配对成功，或者取消配对）就调一次，带着新值——`Bridge`
    /// 自己不知道怎么落盘，也不该知道（它甚至不知道 `SecretStore` 存在），
    /// 这是它唯一「向外」的副作用，daemon.rs 接的是
    /// `secrets::PHONE_OWNER_KEY`。**只在 `accept()`/`unpair()` 里调用**
    /// ——跟 `set_destination` 同一条纪律：这不是渠道或别处该自己猜的事。
    /// `Bridge::retire()` **不**触发它：`PhoneDisable` 走的是 `retire()`
    /// 不是 `unpair()`，它自己另外负责清掉持久化的主人 id（daemon.rs 的
    /// `Request::PhoneDisable` 直接删 `PHONE_OWNER_KEY`）。
    on_owner_changed: Box<dyn Fn(Option<i64>) + Send + Sync>,
}

impl Bridge {
    /// 全新配对：`owner` 从 `None` 开始，第一条消息的发信人赢得配对，
    /// 且不落盘——测试、以及任何不关心「重启后恢复」的调用方用这个。
    pub fn new(ch: Arc<dyn Channel>) -> Self {
        Self::new_with_owner(ch, None, Box::new(no_persistence))
    }

    /// 完整构造：`owner` 是重启时从磁盘恢复的主人（`None` = 全新配对，
    /// 跟 `Bridge::new()` 一样），`on_owner_changed` 是主人变化时的持久化
    /// 回调——见字段文档。**`owner` 是 `Some` 时立即调一次
    /// `set_destination`**（`set_destination` 的第三个合法调用点，见模块
    /// 头注释），但**不**调用 `on_owner_changed`：恢复不是变化，重新把
    /// 已经落盘的值写回同一个键没有意义，只会是一次多余的磁盘 I/O。
    pub fn new_with_owner(
        ch: Arc<dyn Channel>,
        owner: Option<i64>,
        on_owner_changed: Box<dyn Fn(Option<i64>) + Send + Sync>,
    ) -> Self {
        if let Some(id) = owner {
            ch.set_destination(Some(id));
        }
        Bridge {
            ch,
            owner: Mutex::new(owner),
            retired: AtomicBool::new(false),
            on_owner_changed,
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
                // 先放开锁再去碰 `ch`/`on_owner_changed`——`accept()` 之后
                // 不会再摸 `owner`，没有理由让这两个副作用（哪怕
                // `set_destination` 今天不做网络调用，`on_owner_changed`
                // 是真的磁盘 I/O）跨在这把锁的持有期里，见
                // `apply_phone_set_token` 那条「两次 lock() 必须是两条
                // 分开的语句」的同类纪律。
                drop(owner);
                self.ch.set_destination(Some(msg.chat_id));
                (self.on_owner_changed)(Some(msg.chat_id));
                Accepted::Paired(msg.chat_id)
            }
            Some(o) if o == msg.chat_id => Accepted::FromOwner,
            Some(_) => Accepted::Rejected,
        }
    }

    /// 取消配对 / 换令牌：忘掉当前主人，出站目的地清空回 `None`，落盘的
    /// 主人 id 也一并清掉。`set_destination` 的第二个合法调用点——见模块
    /// 头注释。
    ///
    /// 调用方是 `daemon.rs` 的 `Request::PhoneUnpair`。这里不删除 bot 本身
    /// 是否还能用——那是 `send()`/长轮询下一次失败与否的事，不是这个方法
    /// 该管的。
    pub fn unpair(&self) {
        *recover(self.owner.lock()) = None;
        self.ch.set_destination(None);
        (self.on_owner_changed)(None);
    }

    /// 手机通知被整个关掉（`x`）。见 `retired` 字段上的文档注释。
    pub fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
    }

    fn is_retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }

    /// `retire()` 的效果全在一个私有 `AtomicBool` 里，daemon.rs 那条
    /// 「`PhoneDisable` 真的让旧 Bridge 停下来」的测试没有别的办法确认
    /// 调用生效——同 `i18n::has_han` 那种 `#[cfg(test)] pub(crate)` 开孔，
    /// 只在测试构建里存在，不改变生产可见性。
    #[cfg(test)]
    pub(crate) fn is_retired_for_test(&self) -> bool {
        self.is_retired()
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

/// 把一个终态错误写成 `Broken`——`discard_backlog`/`poll_forever` 的错误
/// 分支共用同一份逻辑，不用各自拷贝一遍「转成 (reason, message)、检查
/// `is_retired`、再写」这三步。
///
/// **`is_retired()` 在拿到 `phone` 锁之后重新检查一遍，不是只信任调用方
/// 循环顶端那次检查**——这是 Critical 2 的通用修复：`PhoneDisable` 可能
/// 恰好在"进这个函数"和"拿到 `phone` 锁"这段空档里把状态清成 `Off` 并
/// 调用 `retire()`，不重新检查就会在锁释放之后把 `Off` 盖掉，页面在用户
/// 已经明确关掉之后又"复活"。同一条纪律应用到本文件**每一处**写
/// `phone` 的地方，不只是这一个函数——见 `record_pairing`/
/// `record_bot_blocked`/`record_bridge_panicked`。
fn record_terminal_error(bridge: &Bridge, phone: &Mutex<PhoneStatus>, e: ChannelError) {
    if let Some((reason, message)) = terminal_reason(e) {
        let mut ph = recover(phone.lock());
        if !bridge.is_retired() {
            ph.state = PhoneState::Broken { reason, message };
        }
    }
}

/// 配对成功，把 `phone` 状态槽推进到 `Paired`，`owner` 写成新主人的
/// chat id（**至少是 chat id**——真实显示名需要 `parse_updates` 多读
/// `from.first_name`/`username` 并且改 `Incoming` 的形状，这里先给出
/// 能让页面把"配对上了"和"配对上了谁"分开显示的最小版本，见
/// `ui/phone.rs::status_line`/`msg::phone_paired` 已经准备好的、原来
/// 没有生产者的那个分支）。
///
/// 同 `record_terminal_error`，拿到 `phone` 锁之后重新检查一遍
/// `is_retired()`——这是 Critical 2 点名的那两个场景之一。
fn record_pairing(bridge: &Bridge, phone: &Mutex<PhoneStatus>, id: i64) {
    let mut ph = recover(phone.lock());
    if bridge.is_retired() {
        return;
    }
    ph.state = PhoneState::Paired;
    ph.owner = Some(id.to_string());
}

/// 配对确认发送失败并且是 403——把 `phone` 状态槽写成
/// `Broken{BotBlocked}`。同 `record_terminal_error`，拿到 `phone` 锁之后
/// 重新检查一遍 `is_retired()`——这是 Critical 2 点名的另一个场景：配对
/// 确认这次网络调用最多能挂 10 秒，用户完全可能在这 10 秒之内按下 `x`。
fn record_bot_blocked(bridge: &Bridge, phone: &Mutex<PhoneStatus>) {
    let mut ph = recover(phone.lock());
    if !bridge.is_retired() {
        ph.state = PhoneState::Broken {
            reason: PhoneBrokenReason::BotBlocked,
            message: msg::phone_blocked(BRIDGE_LANG),
        };
    }
}

/// `run()` 兜住一次 panic 之后，把 `phone` 状态槽写成一个通用的
/// `Broken`——Important 3：不写的话，页面会永远停在 `WaitingForPairing`/
/// `Paired` 上，等一个再也不会来的更新，而背后其实什么都没有在监听。
/// `reason` 选 `Unreachable` 而不是编一个更具体的——panic 不是一个
/// `ChannelError`，编不出「令牌坏了」或者「被拉黑了」这种更精确的真话，
/// `Unreachable` 是三个原因里唯一不需要更多上下文就站得住的一个。
///
/// 同样重新检查 `is_retired()`：一次 panic 也可能恰好发生在用户按 `x`
/// 关掉之后的窗口里，不该把已经写好的 `Off` 又盖成 `Broken`。
fn record_bridge_panicked(bridge: &Bridge, phone: &Mutex<PhoneStatus>) {
    let mut ph = recover(phone.lock());
    if !bridge.is_retired() {
        ph.state = PhoneState::Broken {
            reason: PhoneBrokenReason::Unreachable,
            message: msg::phone_bridge_panicked(BRIDGE_LANG),
        };
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
            record_bot_blocked(bridge, phone);
            true
        }
    }
}

/// 一条新起的轮询线程,在第一次真正长轮询之前,先把 Telegram 手上攒着的
/// 积压吃掉——**Critical 1 的第二个必需部分**（另一半是 daemon.rs 侧
/// 持久化/恢复主人 id，见模块头注释「主人 id 要落盘」）。
///
/// **做法**：反复用 `Duration::ZERO` 调 `poll()`——Telegram 只要 offset
/// 之后还压着没确认的更新，不管请求里的 `timeout` 是多少都会立刻吐出来，
/// 不会真的等——直到拿到一个空批次（追上了"现在"）才停。收到的内容一律
/// 丢弃，**不经过 `accept()`**：这是丢弃，不是延迟处理，就算这批里混着
/// 主人自己发的旧消息也一样丢——积压这个概念本身就意味着"过时"，Task 7
/// 落地之后把它们当成"现在发生的事"处理，意味着几小时前的一句话被敲进
/// 一个当下活着的会话。
///
/// 错误处理跟主循环共用同一套判断（`terminal_reason`/`backoff_for`/
/// `record_terminal_error`）：网络问题退避重试，终态错误写 `Broken` 后
/// 返回。
///
/// 返回 `false` 表示吃积压时就已经结束（撞上终态错误，或者被 retire）
/// ——调用方不该再进主循环。
fn discard_backlog(bridge: &Bridge, phone: &Mutex<PhoneStatus>, sleep: &dyn Fn(Duration)) -> bool {
    let mut attempt: u32 = 0;
    loop {
        if bridge.is_retired() {
            return false;
        }
        match bridge.ch.poll(Duration::ZERO) {
            Ok(incoming) if incoming.is_empty() => return true,
            Ok(_) => attempt = 0, // 还有积压,继续吃,不退避
            Err(e) if e.worth_retrying() => {
                attempt = attempt.saturating_add(1);
                sleep(backoff_for(attempt));
            }
            Err(e) => {
                record_terminal_error(bridge, phone, e);
                return false;
            }
        }
    }
}

/// 轮询主体：先吃一遍积压（`discard_backlog`），再一直 `poll()`，把配对/
/// 主人的消息交给 `accept()`，网络问题退避重试，不可恢复的错误就把
/// `Broken` 写进 `phone` 状态槽后退出——调用方（`daemon.rs`）该在那之后
/// 重新起一条线程才能恢复（换新令牌，或者 `PhoneUnpair` 从
/// `Broken { BotBlocked }` 里把同一个 `Bridge` 接回来）。
///
/// `sleep` 被注入而不是直接 `std::thread::sleep`——同 `telegram.rs::Sender`
/// 的道理，测试要能在毫秒级验证退避真的被调用过，而不是真的等上几分钟。
fn poll_forever(bridge: &Bridge, phone: &Mutex<PhoneStatus>, sleep: &dyn Fn(Duration)) {
    if !discard_backlog(bridge, phone, sleep) {
        return;
    }
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
                        Accepted::Paired(id) => {
                            record_pairing(bridge, phone, id);
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
                record_terminal_error(bridge, phone, e);
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
        // 吞掉了。这里不去猜一句更具体的 `Broken` 文案——panic 不是一个
        // `ChannelError`，编不出「令牌坏了」这种更精确的真话；
        // `record_bridge_panicked` 写的是一句通用但诚实、给得出下一步的
        // 话（Important 3），不是发明一个假原因。手机通道安静停掉，
        // 其余会话不受影响，才是这整条纪律唯一要保证的事。
        eprintln!("手机通知线程出了内部错误，已经停掉——不影响正在跑的会话");
        record_bridge_panicked(&bridge, &phone);
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
            (*recover(self.send_result.lock())).unwrap_or(Ok(0))
        }

        fn poll(&self, _timeout: Duration) -> Result<Vec<Incoming>, ChannelError> {
            *recover(self.poll_calls.lock()) += 1;
            // 脚本耗尽之后回一个**终态**错误，不是 `Ok(空批次)`——空批次
            // 会让 `poll_forever` 立刻回到循环顶端再 `poll()` 一次，如果
            // 测试正在验证的那个终止条件本身被 mutate 掉了（比如
            // `send_pairing_confirmation` 该在 `Blocked` 时叫停却没叫停），
            // 循环会在一个只回复空批次的假渠道上转成真正的死循环，把
            // mutation 跑成一次挂起而不是一次快速失败。回终态错误保证
            // 任何一条测试、任何一次 mutation 都在有限步内收敛。
            recover(self.poll_script.lock())
                .pop_front()
                .unwrap_or(Err(ChannelError::BadToken))
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
    /// 脚本第一条是空批次（喂给 `discard_backlog`——一条新起的轮询线程
    /// 在第一次真正长轮询之前先吃一遍积压，见模块头注释「主人 id 要落盘」
    /// 和 `discard_backlog` 自己的文档注释；空批次意味着"没有积压，立刻
    /// 追上现在"），第二条才是真正的配对消息，第三条终态错误让循环自己
    /// 退出——不然这条测试会真的转成死循环。
    #[test]
    fn a_pairing_message_flips_phone_status_to_paired_and_sends_a_confirmation() {
        let ch = Arc::new(FakeChannel::default());
        recover(ch.poll_script.lock()).push_back(Ok(Vec::new())); // discard_backlog: 没有积压
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
        // 循环靠第三次 poll() 的 BadToken 才停下来——停下来时状态是
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
    /// 证明"重试之后真的还会再 poll()"，而不是第一次失败就放弃。脚本第
    /// 一条同样是空批次，喂给 `discard_backlog`。
    #[test]
    fn a_retryable_error_backs_off_and_keeps_polling() {
        let ch = Arc::new(FakeChannel::default());
        {
            let mut script = recover(ch.poll_script.lock());
            script.push_back(Ok(Vec::new())); // discard_backlog: 没有积压
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

        assert_eq!(
            *recover(ch.poll_calls.lock()),
            6,
            "六次脚本都该被消费掉（含 discard_backlog 那一次）"
        );
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
    /// 唯一的产出点。有意思的地方在于这不需要"配对之后"再 `poll()`：
    /// `Blocked` 让 `send_pairing_confirmation` 直接报告"到此为止"，循环
    /// 立刻退出。脚本第一条仍然是空批次，喂给 `discard_backlog`。
    #[test]
    fn being_blocked_right_after_pairing_produces_bot_blocked_and_stops() {
        let ch = Arc::new(FakeChannel::default());
        recover(ch.poll_script.lock()).push_back(Ok(Vec::new())); // discard_backlog
        recover(ch.poll_script.lock()).push_back(Ok(vec![msg(111, "在吗")]));
        *recover(ch.send_result.lock()) = Some(Err(ChannelError::Blocked));
        let bridge = Bridge::new(ch.clone());
        let phone = test_phone();

        poll_forever(&bridge, &phone, &no_sleep());

        assert_eq!(
            *recover(ch.poll_calls.lock()),
            2,
            "该被消费的只有 discard_backlog 那一次和配对那一次,不该再 poll 第三次"
        );
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
    /// 可观察的副作用（不发消息、不动 `phone` 状态）。脚本第一条同样是
    /// 空批次，喂给 `discard_backlog`。
    #[test]
    fn a_strangers_message_after_pairing_produces_no_side_effects_in_the_loop() {
        let ch = Arc::new(FakeChannel::default());
        {
            let mut script = recover(ch.poll_script.lock());
            script.push_back(Ok(Vec::new())); // discard_backlog
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

    // ———— discard_backlog: 积压清零（Critical 1 的第二个必需部分） ————

    /// 没有积压：第一次 `poll(Duration::ZERO)` 就回空批次，立刻追上
    /// "现在"，只消费一次脚本，不碰 `accept()`（`FakeChannel` 没有真的
    /// `Bridge` 引用，`accept()` 会不会被调只能从 `poll_calls`/`sent`/
    /// `destinations` 全都没变化去反推——三者都不变，证明什么都没处理）。
    #[test]
    fn discard_backlog_with_nothing_waiting_consumes_one_empty_batch() {
        let ch = Arc::new(FakeChannel::default());
        recover(ch.poll_script.lock()).push_back(Ok(Vec::new()));
        let bridge = Bridge::new(ch.clone());
        let phone = test_phone();

        assert!(discard_backlog(&bridge, &phone, &no_sleep()));

        assert_eq!(*recover(ch.poll_calls.lock()), 1);
        assert!(
            recover(ch.destinations.lock()).is_empty(),
            "不该调用 set_destination"
        );
        assert!(recover(ch.sent.lock()).is_empty(), "不该发任何消息");
    }

    /// **这是 Critical 1 的直接验证。** 积压里混着一条"陌生人"消息（如果
    /// 直接进主循环，`accept()` 会把它认成主人）——`discard_backlog` 必须
    /// 把它连同后面那条真正的、"现在"发生的配对消息一起吃掉的前半段丢弃
    /// 干净，只留给主循环"现在"这一条。多批次积压（两批非空，第三批才
    /// 是空批次）也顺带证明了"不是只吃一批就算了"。
    #[test]
    fn discard_backlog_throws_away_old_messages_without_ever_calling_accept() {
        let ch = Arc::new(FakeChannel::default());
        {
            let mut script = recover(ch.poll_script.lock());
            script.push_back(Ok(vec![msg(222, "几小时前的陌生人")]));
            script.push_back(Ok(vec![msg(333, "另一条旧消息")]));
            script.push_back(Ok(Vec::new())); // 追上"现在"
        }
        let bridge = Bridge::new(ch.clone());
        let phone = test_phone();

        assert!(discard_backlog(&bridge, &phone, &no_sleep()));

        assert_eq!(*recover(ch.poll_calls.lock()), 3, "三批脚本都该被消费");
        assert!(
            recover(ch.destinations.lock()).is_empty(),
            "积压里的陌生人绝不能被 accept() 认成主人——set_destination 一次都不该被调"
        );
        assert_eq!(
            recover(phone.lock()).state,
            PhoneState::WaitingForPairing,
            "phone 状态不该被积压动过"
        );
    }

    /// 终态错误（令牌本身坏了）在吃积压阶段一样要写 `Broken` 并返回
    /// `false`——不能因为"这只是清积压"就假装没出错。
    #[test]
    fn discard_backlog_reports_a_terminal_error_and_stops() {
        let ch = Arc::new(FakeChannel::default());
        recover(ch.poll_script.lock()).push_back(Err(ChannelError::BadToken));
        let bridge = Bridge::new(ch.clone());
        let phone = test_phone();

        assert!(!discard_backlog(&bridge, &phone, &no_sleep()));

        assert!(matches!(
            recover(phone.lock()).state,
            PhoneState::Broken {
                reason: PhoneBrokenReason::BadToken,
                ..
            }
        ));
    }

    /// 网络问题在吃积压阶段一样要退避重试，不是直接放弃——用两次
    /// `Unreachable` 再一次成功证明"重试之后真的还会再 poll()"。
    #[test]
    fn discard_backlog_backs_off_on_a_retryable_error() {
        let ch = Arc::new(FakeChannel::default());
        {
            let mut script = recover(ch.poll_script.lock());
            script.push_back(Err(ChannelError::Unreachable));
            script.push_back(Err(ChannelError::Unreachable));
            script.push_back(Ok(Vec::new()));
        }
        let bridge = Bridge::new(ch.clone());
        let phone = test_phone();
        let slept = Mutex::new(Vec::new());

        assert!(discard_backlog(&bridge, &phone, &|d| recover(slept.lock()).push(d)));

        assert_eq!(
            *recover(slept.lock()),
            vec![Duration::from_secs(1), Duration::from_secs(2)]
        );
    }

    /// `retired` 在吃积压阶段的循环顶端也要生效——不需要任何脚本，标记
    /// 先置位，`poll()` 干脆不该被调用。
    #[test]
    fn discard_backlog_stops_immediately_once_retired() {
        let ch = Arc::new(FakeChannel::default());
        let bridge = Bridge::new(ch.clone());
        bridge.retire();
        let phone = test_phone();

        assert!(!discard_backlog(&bridge, &phone, &no_sleep()));

        assert_eq!(*recover(ch.poll_calls.lock()), 0);
    }

    // ———— record_*: 每一处写 `phone` 都要重新检查 is_retired（Critical 2 / Important 3） ————
    //
    // 真实场景里这是一场竞态：`PhoneDisable` 可能恰好在"进这些函数"和
    // "拿到 phone 锁"之间的空档把状态清成 Off 并调用 retire()。不需要
    // 真的起两条线程去赛跑才能测到这个判断——`retire()` 提前调用一次，
    // 是"竞态已经朝 PhoneDisable 那一边分出胜负"的确定性等价物：不管
    // 调用方是不是真的抢在了前面，这些函数看到的都是同一个 is_retired()
    // 为真的世界，行为必须一样。

    /// 没有 retire：`record_pairing` 真的把 `Paired` 和 chat id（**至少
    /// 是 chat id**，见函数文档注释）写进 `phone`。
    #[test]
    fn record_pairing_writes_paired_with_the_owner_id_when_not_retired() {
        let bridge = Bridge::for_test();
        let phone = test_phone();

        record_pairing(&bridge, &phone, 111);

        let ph = recover(phone.lock());
        assert_eq!(ph.state, PhoneState::Paired);
        assert_eq!(ph.owner.as_deref(), Some("111"));
    }

    /// **Critical 2，场景一。** 已经 retire 的 `Bridge` 上调
    /// `record_pairing` 必须是空操作——不然 `PhoneDisable` 刚写的 `Off`
    /// 会被这里的 `Paired` 盖掉，用户按了 `x` 却看见页面"复活"。
    #[test]
    fn record_pairing_does_nothing_once_retired() {
        let bridge = Bridge::for_test();
        let phone = test_phone(); // 起点 WaitingForPairing，不是 Off，只为了
                                  // 证明"原地不动"而不是巧合等于 Off
        bridge.retire();

        record_pairing(&bridge, &phone, 111);

        let ph = recover(phone.lock());
        assert_eq!(
            ph.state,
            PhoneState::WaitingForPairing,
            "retire 之后不该再写 Paired"
        );
        assert!(ph.owner.is_none());
    }

    #[test]
    fn record_bot_blocked_writes_broken_when_not_retired() {
        let bridge = Bridge::for_test();
        let phone = test_phone();

        record_bot_blocked(&bridge, &phone);

        assert!(matches!(
            recover(phone.lock()).state,
            PhoneState::Broken {
                reason: PhoneBrokenReason::BotBlocked,
                ..
            }
        ));
    }

    /// **Critical 2，场景二。** 配对确认这次 `send()` 最多能挂 10 秒，
    /// 用户完全可能在这 10 秒之内按下 `x`——已经 retire 的 `Bridge` 上
    /// 调 `record_bot_blocked` 必须是空操作。
    #[test]
    fn record_bot_blocked_does_nothing_once_retired() {
        let bridge = Bridge::for_test();
        let phone = test_phone();
        bridge.retire();

        record_bot_blocked(&bridge, &phone);

        assert_eq!(
            recover(phone.lock()).state,
            PhoneState::WaitingForPairing,
            "retire 之后不该再写 Broken{{BotBlocked}}"
        );
    }

    #[test]
    fn record_terminal_error_writes_broken_when_not_retired() {
        let bridge = Bridge::for_test();
        let phone = test_phone();

        record_terminal_error(&bridge, &phone, ChannelError::BadToken);

        assert!(matches!(
            recover(phone.lock()).state,
            PhoneState::Broken {
                reason: PhoneBrokenReason::BadToken,
                ..
            }
        ));
    }

    #[test]
    fn record_terminal_error_does_nothing_once_retired() {
        let bridge = Bridge::for_test();
        let phone = test_phone();
        bridge.retire();

        record_terminal_error(&bridge, &phone, ChannelError::BadToken);

        assert_eq!(recover(phone.lock()).state, PhoneState::WaitingForPairing);
    }

    /// **Important 3。** 没有 retire：`run()` 兜住的 panic 必须真的写一句
    /// 诚实的 `Broken`，不能让页面永远停在等一个再也不会来的更新上。
    #[test]
    fn record_bridge_panicked_writes_broken_when_not_retired() {
        let bridge = Bridge::for_test();
        let phone = test_phone();

        record_bridge_panicked(&bridge, &phone);

        let ph = recover(phone.lock());
        match &ph.state {
            PhoneState::Broken { reason, message } => {
                assert_eq!(*reason, PhoneBrokenReason::Unreachable);
                assert!(!message.is_empty());
            }
            other => panic!("期待 Broken{{Unreachable}}，得到 {other:?}"),
        }
    }

    #[test]
    fn record_bridge_panicked_does_nothing_once_retired() {
        let bridge = Bridge::for_test();
        let phone = test_phone();
        bridge.retire();

        record_bridge_panicked(&bridge, &phone);

        assert_eq!(recover(phone.lock()).state, PhoneState::WaitingForPairing);
    }

    // ———— Bridge::new_with_owner / on_owner_changed: 主人 id 落盘与恢复（Critical 1） ————

    /// 记录每一次调用的持久化回调，配 `Bridge::new_with_owner` 用——同
    /// `FakeChannel` 的道理，测试不该真的碰 `SecretStore`。
    type OwnerLog = Arc<Mutex<Vec<Option<i64>>>>;
    type OwnerChangedCb = Box<dyn Fn(Option<i64>) + Send + Sync>;

    fn owner_change_recorder() -> (OwnerLog, OwnerChangedCb) {
        let log: OwnerLog = Arc::new(Mutex::new(Vec::new()));
        let log_in_closure = log.clone();
        let cb: OwnerChangedCb = Box::new(move |v| recover(log_in_closure.lock()).push(v));
        (log, cb)
    }

    /// 配对那一刻，`on_owner_changed` 必须真的被调一次、带着新主人的 chat
    /// id——这是 daemon.rs 落盘 `PHONE_OWNER_KEY` 唯一的信号来源。
    #[test]
    fn accept_notifies_on_owner_changed_with_the_new_owner() {
        let ch = Arc::new(FakeChannel::default());
        let (log, cb) = owner_change_recorder();
        let bridge = Bridge::new_with_owner(ch, None, cb);

        bridge.accept(&msg(111, "在吗"));

        assert_eq!(*recover(log.lock()), vec![Some(111)]);

        // 之后不管是主人还是陌生人发消息，都不该再触发一次——owner 已经
        // 定了，`accept()` 的 `FromOwner`/`Rejected` 分支不碰这个回调。
        bridge.accept(&msg(111, "继续"));
        bridge.accept(&msg(222, "闯入"));
        assert_eq!(*recover(log.lock()), vec![Some(111)]);
    }

    /// 取消配对那一刻，`on_owner_changed` 必须带着 `None` 被调一次——不然
    /// daemon.rs 没法知道该把 `PHONE_OWNER_KEY` 从磁盘上删掉，下次重启
    /// 会把一个已经明确取消的配对又接回来。
    #[test]
    fn unpair_notifies_on_owner_changed_with_none() {
        let ch = Arc::new(FakeChannel::default());
        let (log, cb) = owner_change_recorder();
        let bridge = Bridge::new_with_owner(ch, None, cb);
        bridge.accept(&msg(111, "在吗"));

        bridge.unpair();

        assert_eq!(*recover(log.lock()), vec![Some(111), None]);
    }

    /// **恢复不是变化。** 从磁盘上已经持久化的主人 id 构造一个
    /// `Bridge`——`on_owner_changed` 不该被触发（重新把已经落盘的值写回
    /// 同一个键没有意义），但 `set_destination` 必须立刻被调一次：不然
    /// 一个重启后恢复的 `Bridge`，在配对本身真的发生（也就是"再收到一条
    /// 消息"）之前，压根不知道该往哪发确认/事件通知。
    #[test]
    fn new_with_owner_restores_the_destination_without_notifying_the_callback() {
        let ch = Arc::new(FakeChannel::default());
        let (log, cb) = owner_change_recorder();

        let bridge = Bridge::new_with_owner(ch.clone(), Some(111), cb);

        assert_eq!(*recover(ch.destinations.lock()), vec![Some(111)]);
        assert!(
            recover(log.lock()).is_empty(),
            "恢复主人不该触发持久化回调——那会是一次没有意义的重复写盘"
        );

        // 恢复之后，主人的消息立刻被认成 FromOwner，陌生人立刻被拒绝——
        // 不需要再重新"配对"一次。
        assert_eq!(bridge.accept(&msg(111, "回来了")), Accepted::FromOwner);
        assert_eq!(bridge.accept(&msg(999, "陌生人")), Accepted::Rejected);
    }

    /// `Bridge::new()`（没有持久化诉求的简便构造）默认就是空回调——不该
    /// panic，也不该意外碰到任何真实存储。
    #[test]
    fn new_defaults_to_no_persistence() {
        let bridge = Bridge::for_test();
        bridge.accept(&msg(111, "在吗"));
        bridge.unpair(); // 不 panic 就是这条测试的全部断言
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
    ///
    /// **Important 3 也在这条测试里钉住**：`run()` 兜住 panic 之后必须把
    /// 一句诚实的 `Broken` 写进 `phone`，不能让页面永远停在等一个再也
    /// 不会来的更新上。
    #[test]
    fn a_panic_inside_the_loop_never_escapes_run() {
        let bridge = Arc::new(Bridge::new(Arc::new(PanickingChannel)));
        let phone = Arc::new(test_phone());

        run(bridge, phone.clone()); // 不 panic 就是这条测试的主要断言

        assert!(matches!(
            recover(phone.lock()).state,
            PhoneState::Broken {
                reason: PhoneBrokenReason::Unreachable,
                ..
            }
        ));
    }
}
