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
//!
//! **独立安全评审发现的三个 Critical，全部出在 `accept()` 外面的生命周期上**
//! （`accept()` 本身被确认是对的）：
//!
//! 1. **重启重新打开配对**（C1）：`owner` 以前只活在内存里，重启一次就
//!    忘掉主人是谁，又从头允许任何人配对——而一个全新的 `Telegram`
//!    从 offset 0 起步，第一次 `poll()` 会把 Telegram 攒了最多 24 小时的
//!    积压消息整批吐出来。攻击者只要在 dct 关着的时候，把消息发给这个
//!    公开可搜的用户名，重启后积压里他的消息排第一个，就会被 `accept()`
//!    判成主人。**修法**：`owner` 现在由调用方（`daemon.rs`）从密钥仓里
//!    读出持久化的值传进来；重启时如果读到了，`run()` 直接跳过配对和
//!    清空积压这两步，见 `run()` 和 `drain_backlog()`。真正首次配对
//!    （`owner` 是 `None`）时，先把积压清空再打开配对，见 `drain_backlog`
//!    的文档注释。
//! 2. / 3. **`PhoneUnpair`/`PhoneDisable` 够不着线程、重新填令牌能起出
//!    两个活的轮询线程**（C2/C3）：以前 `spawn()` 只管起、不管停，调用方
//!    拿不到任何句柄。现在 `spawn()` 返回 `BridgeHandle`，daemon 只通过
//!    `replace()`/`stop_current()` 这两个函数改它持有的那一个槽——保证
//!    任何时刻最多只有一条真的在跑的轮询线程，见这两个函数的文档注释。

use crate::channel::{Channel, ChannelError, Incoming};
use crate::proto::{PhoneState, PhoneStatus};
use crate::session::recover;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 长轮询一次最多挂多久。Telegram `getUpdates` 用同一个数字当查询参数。
const POLL_TIMEOUT: Duration = Duration::from_secs(25);
/// 退避的起点。
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
/// 退避的上限：五分钟。超过这个数就没有再翻倍的意义——用户等半小时和等
/// 五分钟已经没区别，翻倍下去只会让「网络恢复了但还要再等好久」变得更糟。
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// `sleep_or_stop` 检查停止信号的粒度。数字选得够小，`stop()` 之后线程能
/// 在这么久之内真的退出，而不是把 `MAX_BACKOFF` 那五分钟原样睡完。
const STOP_CHECK_GRANULARITY: Duration = Duration::from_millis(20);

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
    /// 配对之后只认这一个。`None` = 还没配对（这次是真的没配过，或者
    /// 调用方没能从密钥仓里读到持久化的主人——见 `daemon.rs` 的
    /// `startup_bridge_owner`）。
    owner: Mutex<Option<i64>>,
    /// `stop()` 置位之后，`run()` 在下一次能检查到的地方（每轮循环开头、
    /// 每次退避睡眠的每个小切片）主动退出。**不是抢占式的**：正在阻塞的
    /// 那一次网络调用不会被打断，但 `POLL_TIMEOUT` 本身只有 25 秒，
    /// 「不该拖累关掉手机通知这件事的响应速度」这条要求在这个量级下
    /// 站得住。
    stop: AtomicBool,
    /// 配对完成那一刻要做的持久化——把 chat id 写进密钥仓，好让重启之后
    /// `daemon.rs` 能把它读回来传给下一个 `Bridge::new`。**只在 `accept()`
    /// 返回 `Paired` 的那一次调用一次**，见 `dispatch()`。测试用的实现是
    /// 空操作，见 `Bridge::for_test`。
    persist_owner: Box<dyn Fn(i64) + Send + Sync>,
    // …… 消息映射与当前会话见 Task 7
}

impl Bridge {
    pub fn new(
        ch: Arc<dyn Channel>,
        phone: Arc<Mutex<PhoneStatus>>,
        owner: Option<i64>,
        persist_owner: Box<dyn Fn(i64) + Send + Sync>,
    ) -> Bridge {
        Bridge {
            ch,
            phone,
            owner: Mutex::new(owner),
            stop: AtomicBool::new(false),
            persist_owner,
        }
    }

    /// 只测 `accept()` 用——渠道和状态槽都是不会被读的占位符。
    #[cfg(test)]
    fn for_test() -> Bridge {
        Bridge::new(
            Arc::new(tests::NeverCalled),
            tests::blank_status(),
            None,
            Box::new(|_| {}),
        )
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
            // 落盘在改内存状态槽之前——`PhoneStatus` 只是给界面看的缓存，
            // 密钥仓那份才是重启之后唯一还在的真相。顺序反过来的话，一次
            // 「状态槽已经显示配对成功，但落盘失败」的窗口会比这里更长。
            (self.persist_owner)(chat_id);
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
    /// 也可能是 `stop()` 打断了重试——这种情况不写任何状态，调用方
    /// （`run()`）看到 `false` 直接退出即可，不该被误判成「令牌坏了」。
    fn ensure_bot_known(&self) -> bool {
        let mut delay = INITIAL_BACKOFF;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return false;
            }
            match self.ch.get_me() {
                Ok(username) => {
                    recover(self.phone.lock()).bot = Some(username);
                    return true;
                }
                Err(e) if e.worth_retrying() => {
                    self.sleep_or_stop(delay);
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

    /// **C1 的修复。** 配对还没完成（`owner` 是 `None`）时，先把 Telegram
    /// 在 dct 没跑期间攒下的所有旧消息倒空——不认，也不让它们参与配对。
    ///
    /// 不这么做的话：这个 bot 的用户名是公开可搜的，攻击者只要趁 dct 关着
    /// 提前把消息发过去；一旦 daemon 重新起来、`owner` 还是 `None`
    /// （从没配对过，或者调用方确实没有持久化的主人可以恢复），第一次
    /// `poll()` 拿回来的就是 Telegram 积压里的旧消息，`accept()` 会把
    /// 发第一条积压消息的人判成主人——这正是那条「第一个发消息的人」
    /// 规则的字面漏洞。
    ///
    /// 用 0 秒超时反复问一遍：Telegram 的 `getUpdates` 只要有积压就立刻
    /// 整批返回，不等到 timeout 才回；拿到一个**原始数量是 0** 的批次，
    /// 说明积压真的空了，从这一刻起才把"配对开着"这件事交给 `run()` 的
    /// 主循环——**只有在这之后到达的消息**才有资格配对。
    ///
    /// **必须用 `Channel::drain`，绝不能用 `Channel::poll`。** `poll()`
    /// 返回的是过滤之后的 `Vec<Incoming>`（没有 text 的更新——图片、贴纸、
    /// 加群通知——早被悄悄跳过，这条规则对 `poll()` 完全正确）；如果拿
    /// "过滤之后还剩几条"当"积压清空了没有"的判断依据，攻击者只要在 dct
    /// 关着的时候先发 100 张贴纸再发一条文字：贴纸那一批会被过滤成空，
    /// 这个函数就会把它误判成"积压空了"，排在贴纸后面的那条文字反而会
    /// 被当成"配对开着之后的第一条"接受下来——这是 C1 的原始漏洞借着
    /// 这个函数的终止条件原样复活（F1）。`drain()` 报的是原始条数，
    /// 不管有没有 text，这里没有"过滤"这一步可以被利用。
    ///
    /// 已经有持久化主人的重启路径（`owner` 是 `Some`）**不会走到这里**，
    /// 见 `run()`：那种情况下配对早就完成过了，不存在「谁会被误判成
    /// 主人」的问题，反而应该尽快进入正常轮询，不能平白丢掉主人此刻真的
    /// 发来的消息。
    fn drain_backlog(&self) -> bool {
        let mut delay = INITIAL_BACKOFF;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return false;
            }
            match self.ch.drain(Duration::ZERO) {
                Ok(0) => return true,
                Ok(_) => {
                    // 还有积压（不管这一批里有没有能解析出文字的消息），
                    // 一条都不处理、也不让它们进 accept()，继续问下一批
                    // 直到问出一个原始数量为 0 的批次。
                }
                Err(e) if e.worth_retrying() => {
                    self.sleep_or_stop(delay);
                    delay = next_backoff(delay);
                }
                Err(e) => {
                    self.mark_broken(e);
                    return false;
                }
            }
        }
    }

    /// 睡 `dur`，但每隔一小段就看一眼 `stop`——`stop()` 之后调用方不该
    /// 还要等一次完整的退避（最长五分钟）才能真的退出。纯粹的睡眠时长
    /// 之和还是 `dur`（除非提前被打断），语义上跟直接 `sleep(dur)`
    /// 一样，只是能被喊停。
    fn sleep_or_stop(&self, dur: Duration) {
        let mut waited = Duration::ZERO;
        while waited < dur {
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            let this = STOP_CHECK_GRANULARITY.min(dur - waited);
            std::thread::sleep(this);
            waited += this;
        }
    }

    /// 请下一次能检查到 `stop` 的地方（循环开头、退避睡眠中间）主动退出。
    /// **不是抢占式的**——正在进行的那一次网络调用不会被打断，见字段
    /// 文档。`BridgeHandle::stop` 是外部唯一该用的入口，这个方法本身
    /// 也是 `pub`，是因为 bridge 内部测试要在同一模块里直接摸到它。
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// 忘掉主人，重新打开配对。**`PhoneUnpair` 唯一要做的事**——不重启
    /// 线程、不碰 Telegram 的 offset，下一条到达的消息（不管是谁发的）
    /// 立刻重新触发 `accept()` 的 `None` 分支。密钥仓里持久化的那份由
    /// 调用方（`daemon.rs`）另外清掉，这里只管内存里这一份。
    pub fn clear_owner(&self) {
        *recover(self.owner.lock()) = None;
    }

    /// 轮询主循环。**不要直接调用它**——用模块级的 `spawn()`，那边包了
    /// `catch_unwind`；这个方法本身可能 panic（比如锁中毒之外的 bug），
    /// 隔离全靠调用方。
    fn run(&self) {
        if !self.ensure_bot_known() {
            return;
        }
        if self.stop.load(Ordering::Relaxed) {
            return;
        }

        // 只有还没配对的时候才需要清空积压——已经有主人的重启路径直接进
        // 正常轮询，见 `drain_backlog` 文档注释最后一段。
        if recover(self.owner.lock()).is_none() && !self.drain_backlog() {
            return;
        }

        let mut delay = INITIAL_BACKOFF;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            match self.ch.poll(POLL_TIMEOUT) {
                Ok(incoming) => {
                    // **F2 的修复。** `poll()` 可能挂了将近 `POLL_TIMEOUT`
                    // （25 秒）——`stop()` 完全可能在它阻塞期间被别的线程
                    // 调用（`PhoneUnpair`/`PhoneDisable`/重新填令牌）。不在
                    // 这里重新看一眼 `stop`，一条消息就可能在"线程已经被
                    // 判了死刑"之后还是被 `dispatch()`：往共享的 `phone`
                    // 状态槽里写一次配对成功（UI 显示"已配对"），还会调
                    // `persist_owner` 把 chat id 写回密钥仓——如果这发生在
                    // `PhoneDisable`/`PhoneSetToken` 已经删掉/换掉
                    // `PHONE_OWNER_KEY` 之后，等于用一条"来自快死的旧线程"
                    // 的消息把它写了回去，下次重启 `startup_bridge_owner`
                    // 就会把这个本该作废的 chat id 交给新的 bridge。
                    if self.stop.load(Ordering::Relaxed) {
                        return;
                    }
                    delay = INITIAL_BACKOFF;
                    for msg in &incoming {
                        self.dispatch(msg);
                    }
                }
                Err(e) if e.worth_retrying() => {
                    self.sleep_or_stop(delay);
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
/// 这不是靠这里的作者自觉守住的规矩，是结构上做不到：`ChannelError` 的
/// 三个变体都不携带任何字符串字段（见 `channel/mod.rs`），这个函数的输入
/// 类型上就不可能携带令牌或者原始回包内容，「手滑把令牌带出来」这条路
/// 从签名上就堵死了。
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

/// 一个正在跑的 bridge 的外部句柄。**除了它，外面不该有别的办法碰到
/// 内部那个 `Bridge`**——`replace()`/`stop_current()` 只通过它管线程的
/// 生死，`daemon.rs` 也只通过它转发 `PhoneUnpair`（`unpair()`）。
pub struct BridgeHandle {
    bridge: Arc<Bridge>,
}

impl BridgeHandle {
    /// 让轮询线程在下一次能检查到的地方退出。见 `Bridge::stop` 的文档。
    pub fn stop(&self) {
        self.bridge.stop();
    }

    /// `PhoneUnpair` 的落地点：忘掉主人，重新打开配对，线程继续跑，
    /// 不用重启。
    pub fn unpair(&self) {
        self.bridge.clear_owner();
    }

    /// 转发给内部的 `accept()`——Task 7 往会话里敲字之前要先问这一句，
    /// 现在先给测试用来验证 `unpair()` 真的改到了内部状态。
    pub fn accept(&self, msg: &Incoming) -> Accepted {
        self.bridge.accept(msg)
    }
}

/// 把 bridge 起在后台线程上。**整个线程体包在 `catch_unwind` 里**——
/// 一个手机通道死掉是遗憾，一个会话死掉是灾难，两者绝不能是同一件事。
///
/// 不要直接调用它来更换一个正在跑的 bridge——那样旧的线程没人管，
/// 会跟新的一起活着（C3）。改令牌/配对状态一律走 `replace()`。
pub fn spawn(
    ch: Arc<dyn Channel>,
    phone: Arc<Mutex<PhoneStatus>>,
    owner: Option<i64>,
    persist_owner: Box<dyn Fn(i64) + Send + Sync>,
) -> BridgeHandle {
    let bridge = Arc::new(Bridge::new(ch, phone, owner, persist_owner));
    let worker = bridge.clone();
    std::thread::spawn(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| worker.run()));
    });
    BridgeHandle { bridge }
}

/// **C2/C3 的修复。** 守护进程只通过这一个函数改变"当前是哪个 bridge 在跑"
/// ——重启、`PhoneSetToken` 换令牌，都调它，而不是各自直接调 `spawn()`。
/// 换之前先把槽里原来那个（如果有）真的停掉，保证任何时刻这个槽里最多
/// 只有一条活的轮询线程：不这么做的话，重新填一次令牌就会起出第二个
/// 长轮询同一个 bot 的线程，两条线程各自维护自己的 `owner`，谁先问到
/// `getUpdates` 谁就替自己的那份 `owner` 配上人——主人和攻击者可能各自
/// 配对到不同的 bridge 上，都以为自己是主人（C3）。
pub fn replace(
    slot: &Mutex<Option<BridgeHandle>>,
    ch: Arc<dyn Channel>,
    phone: Arc<Mutex<PhoneStatus>>,
    owner: Option<i64>,
    persist_owner: Box<dyn Fn(i64) + Send + Sync>,
) {
    let mut guard = recover(slot.lock());
    if let Some(old) = guard.take() {
        old.stop();
    }
    *guard = Some(spawn(ch, phone, owner, persist_owner));
}

/// `PhoneDisable` 的落地点：把槽里的 bridge（如果有）停掉，槽留空。
/// 跟 `replace()` 共用同一条「先停旧的」逻辑，只是不起新的。
pub fn stop_current(slot: &Mutex<Option<BridgeHandle>>) {
    if let Some(old) = recover(slot.lock()).take() {
        old.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::time::Instant;

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
        fn drain(&self, _timeout: Duration) -> Result<usize, ChannelError> {
            panic!("accept() 测试不该碰渠道的 drain()")
        }
    }

    /// 把 `JoinHandle::join()` 包一层超时——原生 API 没有这个能力。
    /// 用来断言"线程真的退出了"，而不是靠一次固定长度的 `sleep` 赌它
    /// 应该退出了。看门线程如果一直等不到（说明 `stop()` 没生效）会
    /// 泄漏，但测试本身会在 `timeout` 之后如实报失败，不会一直挂着。
    fn wait_for_join(handle: std::thread::JoinHandle<()>, timeout: Duration) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = handle.join();
            let _ = tx.send(());
        });
        rx.recv_timeout(timeout).is_ok()
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

    /// 一堆线程同时对着一个全新的 bridge 发第一条消息——`accept()` 的
    /// 判定和写入必须在同一把锁下完成，不然两个线程都可能读到 `None`，
    /// 都以为自己该配对成功。多线程跑一遍，钉住"两次 `Paired` 加起来
    /// 也只有一次"这条并发下的不变量。
    #[test]
    fn only_one_thread_ever_wins_pairing_when_racing() {
        let b = Arc::new(Bridge::for_test());
        let handles: Vec<_> = (0..64)
            .map(|i| {
                let b = b.clone();
                std::thread::spawn(move || b.accept(&msg(i, "hi")))
            })
            .collect();
        let results: Vec<Accepted> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let paired_count = results
            .iter()
            .filter(|r| matches!(r, Accepted::Paired(_)))
            .count();
        assert_eq!(paired_count, 1, "64 个线程抢着发第一条消息，只能有一个赢");
        let rejected_count = results
            .iter()
            .filter(|r| matches!(r, Accepted::Rejected))
            .count();
        assert_eq!(rejected_count, 63, "剩下的必须全部被拒绝，一个都不能漏");
    }

    /// `owner` 那把锁在持锁期间 panic 会中毒——`recover()` 就是为了这个。
    /// 钉住中毒之后 `accept()` 照样能认出原来的主人，不会因为一次无关的
    /// panic 就把"谁是主人"这件事忘掉、也不会让陌生人趁虚而入。
    /// 手法照抄 `session.rs::recovers_from_poisoned_sessions_lock`。
    #[test]
    fn owner_survives_a_panic_while_the_lock_was_held() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(111, "hi")), Accepted::Paired(111));

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = b.owner.lock().unwrap();
            panic!("模拟持锁期间的 panic，用来验证 owner 锁中毒后还能恢复");
        }));
        assert!(result.is_err(), "上面这次 panic 应该被 catch_unwind 接住");

        assert_eq!(
            b.accept(&msg(111, "还在吗")),
            Accepted::FromOwner,
            "锁中毒之后不能把主人忘掉"
        );
        assert_eq!(
            b.accept(&msg(222, "陌生人")),
            Accepted::Rejected,
            "锁中毒也不能让陌生人趁虚而入"
        );
    }

    /// 重启时如果密钥仓里已经有持久化的主人，`Bridge::new` 直接拿这个
    /// 值当 `owner`——**配对不会重新打开**。这是 C1 的另一半：光靠
    /// `drain_backlog` 清空积压还不够，如果每次重启都把 `owner` 焊死成
    /// `None`，清空积压之后照样是"谁先发消息谁是主人"，攻击者只要在
    /// 积压清空之后抢在真主人前面发一条就赢了。真正的修法是重启根本
    /// 不该重新进入"谁先发消息"这个状态。
    #[test]
    fn a_bridge_restored_with_a_known_owner_never_reopens_pairing() {
        let b = Bridge::new(
            Arc::new(NeverCalled),
            blank_status(),
            Some(111),
            Box::new(|_| {}),
        );
        assert_eq!(
            b.accept(&msg(222, "先到")),
            Accepted::Rejected,
            "owner 已经从密钥仓恢复，陌生人抢先发消息也不该被判成主人"
        );
        assert_eq!(b.accept(&msg(111, "真主人")), Accepted::FromOwner);
    }

    // ---- dispatch：只有 Paired 才动状态槽，也只有 Paired 才落盘 ----

    #[test]
    fn dispatch_on_pairing_writes_paired_state_and_owner() {
        let phone = blank_status();
        let b = Bridge::new(Arc::new(NeverCalled), phone.clone(), None, Box::new(|_| {}));
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
        let b = Bridge::new(Arc::new(NeverCalled), phone.clone(), None, Box::new(|_| {}));
        b.dispatch(&msg(42, "配对"));
        // 手动把状态槽改成一个跟「配对」不同的值，确认 FromOwner 不会把它
        // 又改回去、也不会动 owner 字段。
        {
            let mut st = phone.lock().unwrap();
            st.owner = Some("哨兵".to_string());
            st.state = PhoneState::WaitingForPairing;
        }
        b.dispatch(&msg(42, "第二条"));
        let st = phone.lock().unwrap();
        assert_eq!(
            st.owner.as_deref(),
            Some("哨兵"),
            "FromOwner 不该覆盖状态槽的 owner"
        );
        assert_eq!(
            st.state,
            PhoneState::WaitingForPairing,
            "FromOwner 也不该覆盖状态槽的 state"
        );
    }

    /// 陌生人的消息完全不该碰状态槽——连尝试写都不该有。
    #[test]
    fn dispatch_from_a_stranger_leaves_the_slot_untouched() {
        let phone = blank_status();
        let b = Bridge::new(Arc::new(NeverCalled), phone.clone(), None, Box::new(|_| {}));
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

    /// **C1 修复链条上最要紧的一环**：`persist_owner` 只在真正配对的
    /// 那一刻被调用一次，`FromOwner`/`Rejected` 都不该碰它——如果陌生人
    /// 的每次尝试都触发一次落盘调用，磁盘上最终写的是谁完全看运气；
    /// 如果主人后续的普通消息重复触发，语义上没错但浪费磁盘 IO，也可能
    /// 掩盖"配对只应该发生一次"这个事实。
    #[test]
    fn dispatch_on_pairing_calls_persist_owner_exactly_once() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let sink = calls.clone();
        let phone = blank_status();
        let b = Bridge::new(
            Arc::new(NeverCalled),
            phone,
            None,
            Box::new(move |id| sink.lock().unwrap().push(id)),
        );
        b.dispatch(&msg(42, "配对"));
        b.dispatch(&msg(42, "第二条")); // FromOwner，不该再落盘
        b.dispatch(&msg(99, "陌生人")); // Rejected，不该落盘
        assert_eq!(
            *calls.lock().unwrap(),
            vec![42],
            "只有真正配对那一刻才落盘 owner，且只落一次"
        );
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

    /// `stop()` 必须能打断一次正在进行的退避睡眠，而不是让调用方等它
    /// 睡完——真等完的话，「关掉手机通知」这个操作在最坏情况下（正好
    /// 撞上退避顶到 5 分钟的那一刻）要等 5 分钟才有反应。
    #[test]
    fn stop_interrupts_a_long_backoff_sleep_instead_of_waiting_it_out() {
        // 用 2 秒（不是真正的 `MAX_BACKOFF` = 5 分钟）代表"一次很长的退避
        // 睡眠"：这条测试要验证的是"stop() 能不能打断"，不是"5 分钟" 这个
        // 具体数字本身——`backoff_doubles_and_caps_at_five_minutes` 已经
        // 钉住了那个上限。用一个小得多但仍然远大于"该被打断掉的那一点点
        // 时间"的数字，能让"如果哪天 `stop()` 被改回没用"这个变异在几百
        // 毫秒内就失败，而不是要等 5 分钟才真正能看到断言失败——测试本身
        // 的运行时间不该跟它想防住的那个 bug 的严重程度绑在一起。
        const A_LONG_SLEEP: Duration = Duration::from_secs(2);
        let b = Arc::new(Bridge::for_test());
        let b2 = b.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            b2.stop();
        });
        let start = Instant::now();
        b.sleep_or_stop(A_LONG_SLEEP);
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "stop() 该能打断退避睡眠，而不是等满 {A_LONG_SLEEP:?}：实际睡了 {:?}",
            start.elapsed()
        );
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

    // ---- get_me / 轮询主循环：mock channel，不碰网络 ----

    /// 可编程的 mock：`get_me` 给定的一个结果，`poll`/`drain` 各自是一串
    /// 排好队的结果，用完就返回空批次/0（不是错误）——这是两条**独立**
    /// 的队列，不是同一条：真实的 `Telegram` 也是这样，`drain_backlog`
    /// 只会调用 `drain()`，`run()` 的正常轮询只会调用 `poll()`，谁调了
    /// 哪个、调了几次，靠各自的调用计数分辨，不用再像以前那样去比较
    /// 传进来的 timeout。`on_poll_return` 是给 F2 的回归测试用的钩子：
    /// 在 `poll()` 即将返回之前跑一下，让测试能在"poll() 已经返回，但
    /// `run()` 还没来得及 `dispatch()`"这个窗口里做点什么（比如喊停）。
    struct MockChannel {
        get_me_result: Result<String, ChannelError>,
        poll_results: Mutex<VecDeque<Result<Vec<Incoming>, ChannelError>>>,
        drain_results: Mutex<VecDeque<Result<usize, ChannelError>>>,
        poll_calls: Mutex<u32>,
        drain_calls: Mutex<u32>,
        on_poll_return: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    }

    impl MockChannel {
        fn new(get_me_result: Result<String, ChannelError>) -> MockChannel {
            MockChannel {
                get_me_result,
                poll_results: Mutex::new(VecDeque::new()),
                drain_results: Mutex::new(VecDeque::new()),
                poll_calls: Mutex::new(0),
                drain_calls: Mutex::new(0),
                on_poll_return: Mutex::new(None),
            }
        }

        fn queue_poll(&self, r: Result<Vec<Incoming>, ChannelError>) {
            self.poll_results.lock().unwrap().push_back(r);
        }

        fn queue_drain(&self, r: Result<usize, ChannelError>) {
            self.drain_results.lock().unwrap().push_back(r);
        }
    }

    impl Channel for MockChannel {
        fn send(&self, _to: i64, _text: &str) -> Result<crate::channel::MsgId, ChannelError> {
            unimplemented!("这一路测试不需要 send")
        }
        fn poll(&self, _timeout: Duration) -> Result<Vec<Incoming>, ChannelError> {
            *self.poll_calls.lock().unwrap() += 1;
            let result = self
                .poll_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(Vec::new()));
            if let Some(f) = self.on_poll_return.lock().unwrap().as_ref() {
                f();
            }
            result
        }
        fn get_me(&self) -> Result<String, ChannelError> {
            self.get_me_result.clone()
        }
        fn drain(&self, _timeout: Duration) -> Result<usize, ChannelError> {
            *self.drain_calls.lock().unwrap() += 1;
            self.drain_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(0))
        }
    }

    /// 正常路径（首次配对，`owner` 是 `None`）：`get_me` 先补上 bot 名字，
    /// 接着 `drain_backlog` 问一次积压（这里没有），再进正常轮询：处理
    /// 一条配对消息，然后收到一个不可重试的错误（`BadToken`）就停下、
    /// 把 `Broken` 写进去。
    #[test]
    fn run_populates_bot_then_pairs_then_stops_on_bad_token() {
        let phone = blank_status();
        let ch = Arc::new(MockChannel::new(Ok("my_dct_bot".to_string())));
        ch.queue_drain(Ok(0)); // drain_backlog：没有积压
        ch.queue_poll(Ok(vec![msg(111, "hi")])); // 正常轮询：第一条真消息完成配对
        ch.queue_poll(Err(ChannelError::BadToken));

        let bridge = Bridge::new(ch.clone(), phone.clone(), None, Box::new(|_| {}));
        bridge.run();

        let st = phone.lock().unwrap();
        assert_eq!(st.bot.as_deref(), Some("my_dct_bot"));
        match &st.state {
            PhoneState::Broken(_) => {}
            other => panic!("该停在 Broken，得到 {other:?}"),
        }
        assert_eq!(*ch.drain_calls.lock().unwrap(), 1, "积压只问了一次就问出空");
        assert_eq!(*ch.poll_calls.lock().unwrap(), 2, "正常轮询该走两次");
    }

    /// **C1 的回归测试。** 重启前（`owner` 还是 `None`，没能从密钥仓恢复）
    /// Telegram 已经攒了一条陌生人的积压消息——这正是"攻击者趁 dct 关着
    /// 抢先发消息"那个场景。`drain_backlog` 必须把它原样倒掉，配对只能
    /// 落在积压清空之后到达的第一条真消息上。**这里的积压完全不经过
    /// `poll()`**——`drain()` 只报数量，攻击者的消息内容从头到尾都没有
    /// 被解析成 `Incoming`，比"解析了但丢弃"更进一步地堵死了这条路。
    #[test]
    fn a_backlog_message_present_before_pairing_opens_is_discarded_not_paired() {
        let phone = blank_status();
        let ch = Arc::new(MockChannel::new(Ok("my_dct_bot".to_string())));
        ch.queue_drain(Ok(1)); // 积压：还有一条原始更新（攻击者的消息，drain 不关心内容）
        ch.queue_drain(Ok(0)); // 积压问干净了
        ch.queue_poll(Ok(vec![msg(111, "真主人上线")])); // 配对开着之后的第一条
        ch.queue_poll(Err(ChannelError::BadToken));

        let bridge = Bridge::new(ch.clone(), phone.clone(), None, Box::new(|_| {}));
        bridge.run();

        let st = phone.lock().unwrap();
        assert_eq!(
            st.owner.as_deref(),
            Some("111"),
            "配对必须落在真主人身上，不能是积压里那条陌生人的消息"
        );
        drop(st);
        assert_eq!(*ch.drain_calls.lock().unwrap(), 2, "积压该被问两次才问出空");
        // 陌生人 999 从始至终没有拿到过 `Paired`——直接问 bridge 本身也确认一遍。
        assert_eq!(bridge.accept(&msg(999, "还想再试一次")), Accepted::Rejected);
    }

    /// **F1 的回归测试。** 积压里那一批全是没有 text 的更新（贴纸/图片/
    /// 加群通知）——如果 `drain_backlog` 拿"parse_updates 过滤之后还剩
    /// 几条"当判断依据（而不是原始条数），这一批会被误判成"积压已经
    /// 清空"：`drain()` 报"原始 100 条"，`drain_backlog` 必须老实再问
    /// 一轮，不能因为"这批东西看起来都不是消息"就提前放行。
    #[test]
    fn drain_backlog_does_not_stop_on_a_batch_that_is_all_non_text_updates() {
        let phone = blank_status();
        let ch = Arc::new(MockChannel::new(Ok("bot".to_string())));
        ch.queue_drain(Ok(100)); // 100 条原始更新——这批全是贴纸，但 drain() 老实报了原始数量
        ch.queue_drain(Ok(0)); // 下一批才真的空
        ch.queue_poll(Ok(vec![msg(111, "真主人上线")]));
        ch.queue_poll(Err(ChannelError::BadToken));

        let bridge = Bridge::new(ch.clone(), phone.clone(), None, Box::new(|_| {}));
        bridge.run();

        assert_eq!(
            *ch.drain_calls.lock().unwrap(),
            2,
            "第一批报了 100 条，必须再问一轮，不能一次就判定积压空了"
        );
        assert_eq!(
            *ch.poll_calls.lock().unwrap(),
            2,
            "只有积压真的空了才进正常轮询"
        );
        let st = phone.lock().unwrap();
        assert_eq!(st.owner.as_deref(), Some("111"));
    }

    /// 令牌一开始就是坏的：`get_me` 直接 `BadToken`，`run()` 必须
    /// **一次都不**调用 `poll()`——拿一个已知坏掉的令牌去打 `getUpdates`
    /// 纯属浪费，而且会让「令牌坏了」这件事被网络错误的退避逻辑掩盖掉。
    #[test]
    fn a_bad_token_at_startup_never_reaches_poll() {
        let phone = blank_status();
        let ch = Arc::new(MockChannel::new(Err(ChannelError::BadToken)));
        let bridge = Bridge::new(ch.clone(), phone.clone(), None, Box::new(|_| {}));
        bridge.run();

        assert_eq!(*ch.poll_calls.lock().unwrap(), 0);
        assert_eq!(*ch.drain_calls.lock().unwrap(), 0);
        let st = phone.lock().unwrap();
        assert_eq!(st.bot, None);
        match &st.state {
            PhoneState::Broken(_) => {}
            other => panic!("该停在 Broken，得到 {other:?}"),
        }
    }

    /// 重启且 `owner` 已知：`run()` 必须直接进正常轮询，**跳过**
    /// `drain_backlog`——不然重启一次，主人自己此刻真的发来的消息都可能
    /// 被当成"积压"平白丢掉。`drain_calls` 必须是 0，`poll_calls` 是 1。
    #[test]
    fn run_with_a_known_owner_skips_backlog_draining_and_polls_directly() {
        let phone = blank_status();
        let ch = Arc::new(MockChannel::new(Ok("bot".to_string())));
        ch.queue_poll(Err(ChannelError::BadToken));

        let bridge = Bridge::new(ch.clone(), phone, Some(111), Box::new(|_| {}));
        bridge.run();

        assert_eq!(
            *ch.drain_calls.lock().unwrap(),
            0,
            "已经有主人时不该先走清空积压那一步"
        );
        assert_eq!(*ch.poll_calls.lock().unwrap(), 1);
    }

    /// **F2 的回归测试。** `poll()` 返回了一条陌生人消息之后、`run()`
    /// 还没来得及 `dispatch()` 之前，`stop()` 恰好被调用（模拟
    /// `PhoneDisable`/`PhoneUnpair`/重新填令牌发生在这个窗口里）。这条
    /// 消息不该被派发——不然一个已经被判了死刑的线程还能在临死前把
    /// 陌生人写成主人、把 chat id 落盘。用 `on_poll_return` 钩子精确
    /// 制造"poll() 刚返回、dispatch() 还没跑"这个时刻。
    #[test]
    fn a_stop_right_after_poll_returns_prevents_the_message_from_being_dispatched() {
        let phone = blank_status();
        let ch = Arc::new(MockChannel::new(Ok("bot".to_string())));
        ch.queue_drain(Ok(0));
        ch.queue_poll(Ok(vec![msg(999, "陌生人趁 stop() 生效前那一刻发消息")]));

        let bridge = Arc::new(Bridge::new(
            ch.clone(),
            phone.clone(),
            None,
            Box::new(|_| {}),
        ));
        let b2 = bridge.clone();
        *ch.on_poll_return.lock().unwrap() = Some(Box::new(move || b2.stop()));

        bridge.run();

        let st = phone.lock().unwrap();
        assert_eq!(
            st.owner, None,
            "poll() 返回之后、dispatch() 之前 stop() 已经生效——这条消息不该被派发/配对"
        );
        assert_eq!(*ch.poll_calls.lock().unwrap(), 1, "只该问这一次就该退出了");
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
            fn drain(&self, _timeout: Duration) -> Result<usize, ChannelError> {
                Ok(0)
            }
        }
        let phone = blank_status();
        let bridge = Bridge::new(Arc::new(PanicsOnPoll), phone, None, Box::new(|_| {}));
        let result = catch_unwind(AssertUnwindSafe(|| bridge.run()));
        assert!(
            result.is_err(),
            "run() 里的 panic 该被外面的 catch_unwind 接住"
        );
    }

    // ---- stop：C2/C3 修复的地基 ----

    /// `stop()` 之后轮询线程必须真的退出，不是只改一个没人看的标志位。
    #[test]
    fn stop_actually_stops_the_polling_thread() {
        let phone = blank_status();
        // owner 已知：跳过 drain_backlog，直接进那个"每次都成功但什么都
        // 没有"的正常轮询，循环转得飞快，最能暴露"stop 没生效就会一直
        // 转下去"这件事。
        let ch = Arc::new(MockChannel::new(Ok("bot".to_string())));
        let bridge = Arc::new(Bridge::new(ch.clone(), phone, Some(1), Box::new(|_| {})));
        let worker = bridge.clone();
        let handle = std::thread::spawn(move || worker.run());

        let deadline = Instant::now() + Duration::from_secs(2);
        while *ch.poll_calls.lock().unwrap() == 0 {
            assert!(Instant::now() < deadline, "轮询线程一直没跑起来");
            std::thread::sleep(Duration::from_millis(2));
        }

        bridge.stop();
        assert!(
            wait_for_join(handle, Duration::from_secs(2)),
            "stop() 之后线程该在有限时间内退出，而不是继续轮询下去"
        );
    }

    /// `BridgeHandle::unpair()` 必须真的改到内部 `Bridge` 的状态——
    /// **C2 的回归测试**：以前 `PhoneUnpair` 只改了 `PhoneStatus` 这个
    /// 给界面看的缓存，`Bridge::owner` 毫不知情，旧主人事实上继续掌握
    /// 着通道，新手机永远配不上。
    #[test]
    fn bridge_handle_unpair_clears_the_owner_and_reopens_pairing() {
        let handle = spawn(
            Arc::new(NeverCalled),
            blank_status(),
            Some(111),
            Box::new(|_| {}),
        );
        assert_eq!(handle.accept(&msg(111, "老主人")), Accepted::FromOwner);

        handle.unpair();

        assert_eq!(
            handle.accept(&msg(222, "新手机先发")),
            Accepted::Paired(222),
            "unpair 之后配对要重新打开，不能永远焊死在老主人身上"
        );
    }

    /// **C3 的回归测试。** `replace()` 必须先把槽里旧的 bridge 停掉，
    /// 再让新的顶上——不然旧的和新的会同时长轮询同一个 bot，一个把
    /// 攻击者配成主人、一个把真主人配成主人，两边都以为自己赢了。
    #[test]
    fn replace_stops_the_old_bridge_before_starting_the_new_one() {
        let slot: Mutex<Option<BridgeHandle>> = Mutex::new(None);
        let phone = blank_status();

        let old_ch = Arc::new(MockChannel::new(Ok("old_bot".to_string())));
        replace(
            &slot,
            old_ch.clone(),
            phone.clone(),
            Some(1),
            Box::new(|_| {}),
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        while *old_ch.poll_calls.lock().unwrap() == 0 {
            assert!(Instant::now() < deadline, "旧的 bridge 一直没跑起来");
            std::thread::sleep(Duration::from_millis(2));
        }

        let new_ch = Arc::new(MockChannel::new(Ok("new_bot".to_string())));
        replace(
            &slot,
            new_ch.clone(),
            phone.clone(),
            Some(2),
            Box::new(|_| {}),
        );

        // 给旧线程一点时间真的退出，再看它是不是真停了——不是只停了
        // "看起来"，是接下来这段时间里 poll 调用次数彻底不再增长。
        std::thread::sleep(Duration::from_millis(150));
        let old_count = *old_ch.poll_calls.lock().unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            *old_ch.poll_calls.lock().unwrap(),
            old_count,
            "replace() 之后旧的 bridge 不该还在轮询——两条活着的轮询线程\
             会让攻击者和主人各自配对到不同的 bridge 上（C3）"
        );
        assert!(
            *new_ch.poll_calls.lock().unwrap() > 0,
            "新的 bridge 该顶上开始跑"
        );
    }

    /// `stop_current()`：跟 `replace()` 共用"先停旧的"这条逻辑，只是
    /// 不起新的——`PhoneDisable` 用的就是这个。
    #[test]
    fn stop_current_stops_the_bridge_and_leaves_the_slot_empty() {
        let slot: Mutex<Option<BridgeHandle>> = Mutex::new(None);
        let phone = blank_status();
        let ch = Arc::new(MockChannel::new(Ok("bot".to_string())));
        replace(&slot, ch.clone(), phone, Some(1), Box::new(|_| {}));

        let deadline = Instant::now() + Duration::from_secs(2);
        while *ch.poll_calls.lock().unwrap() == 0 {
            assert!(Instant::now() < deadline, "bridge 一直没跑起来");
            std::thread::sleep(Duration::from_millis(2));
        }

        stop_current(&slot);

        std::thread::sleep(Duration::from_millis(150));
        let count = *ch.poll_calls.lock().unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            *ch.poll_calls.lock().unwrap(),
            count,
            "stop_current() 之后不该还在轮询"
        );
        assert!(
            recover(slot.lock()).is_none(),
            "槽里不该留着一个已经停掉的句柄"
        );
    }
}
