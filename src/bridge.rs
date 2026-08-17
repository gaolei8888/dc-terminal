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

use crate::channel::{Channel, ChannelError, Event, Incoming, MsgId};
use crate::journal::{Delivery, Journal};
use crate::proto::{PhoneState, PhoneStatus};
use crate::session::recover;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// 出站事件队列的上限（Ruling 4）。`session.rs::tick()` 那一头是
/// unbounded 的 `mpsc::Sender`——`tick()` 绝不能因为投递阻塞，见那边
/// `SessionManager::event_tx` 的文档。有界这一半必须在消费端做，做在
/// 这里：`Bridge::enqueue` 满了就丢**最旧**的一条。
///
/// 丢最旧不是随便选的：对 stop/fail 通知来说，用户会先看到手机上最新
/// 收到的那条，旧的哪怕留着也多半已经不是当下最要紧的那件事了；反过来
/// 丢最新的话，用户会一直盯着一条早就过时的通知，看不到刚发生的事。
///
/// **这个数字是拍出来的**，跟 `channel::DEBOUNCE_WINDOW` 一样——如果
/// 有 32 条事件排在队列里还没发出去，说明手机通知这条链路本身卡住了
/// 很久，不是调大这一个数字能救的场面，先给个不至于让内存无限涨的
/// 上限。
pub const QUEUE_CAP: usize = 32;

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

/// 决定一条不带主人身份问题（那个问题 `accept()` 已经答过了）的入站消息
/// 该敲进哪个会话——或者拒绝回答。**纯函数，不做 IO、不碰任何共享状态**：
/// 调用方（Task 8）负责把这里此刻的真相（消息映射、`/use` 状态、谁在等）
/// 攒成这个结构体,再问 `route()`。
pub struct RouteInput<'a> {
    /// 这条消息是不是长按/回复了手机上此前收到的某条推送。
    pub reply_to: Option<MsgId>,
    /// 推送消息 id -> 那条推送来自哪个会话。守护进程重启后这份映射
    /// 是空的——这正是规则 1 里 `Gone` 分支存在的原因。
    pub map: &'a HashMap<MsgId, u32>,
    /// 用户上一次 `/use` 显式选中的会话，如果有的话。
    pub used: Option<u32>,
    /// 自从那次 `/use` 之后，用户是不是已经回复过至少一条推送——一旦
    /// 是，说明注意力已经转走，`/use` 的指定就作废了（规则 3）。
    pub replied_since_use: bool,
    /// 此刻正在等待输入、且没有被上面两条规则截住的候选会话集合。
    pub waiting: &'a [u32],
}

/// `route()` 的答案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// 敲进这一个会话，没有歧义。
    To(u32),
    /// 好几个候选，说不清该敲哪个——**问用户，不猜**。
    Ask(Vec<u32>),
    /// 这条回复对应的推送消息守护进程已经不认识了（多半是重启把映射
    /// 冲掉了）。**唯一正确的动作是什么都不敲**——退化成"发给当前会话"
    /// 正是把话敲进错误终端的那条路径，用户在手机上看不到终端，
    /// 不会发现敲错了。
    Gone,
    /// 没有 `/use`，也没有任何会话在等：告诉用户先去看看会话列表，
    /// 而不是替他瞎猜一个。
    NeedUse,
}

/// 决定一条入站消息该敲进哪个会话，或者拒绝回答。**五条规则，顺序固定，
/// 不能重排**——顺序本身就是设计的一部分：
///
/// 1. 带回复的消息永远直接定位到那条推送对应的会话，**永远不反问**；
///    如果映射里已经找不到（守护进程重启过），答案是 `Gone`，什么都不敲。
/// 2. 显式 `/use` 过、且那之后还没回复过任何推送：`/use` 压过"唯一在等
///    的那个会话"——用户切过去就是想跟那个会话说话，这一条必须排在
///    "唯一在等"前面，否则一个碰巧在等的会话会把他的话抢走。
/// 3. 但 `/use` 只在用户还没有用回复动作转移注意力之前有效——一旦他
///    回复过至少一条推送，说明注意力已经转走，`/use` 的指定作废，
///    不然一次 `/use` 会永久劫持所有后续不带回复的消息。
/// 4. 只有一个会话在等：直接给它。
/// 5. 好几个会话在等：`Ask`，不猜——敲错 agent 的代价远大于多问一句。
/// 6. 什么都没有（没有 `/use`，没有会话在等）：`NeedUse`，请用户自己去
///    看一眼会话列表。
pub fn route(i: &RouteInput) -> Route {
    // 1. 带回复的：直接定位，永远不反问。
    if let Some(m) = i.reply_to {
        return match i.map.get(&m) {
            Some(&s) => Route::To(s),
            // 守护进程重启过，映射没了。**绝不退化成"发给当前会话"**——
            // 那正是会把话敲进错误终端的路径。
            None => Route::Gone,
        };
    }
    // 2. 显式 /use 过、且那之后还没回复过任何推送。
    if let (Some(u), false) = (i.used, i.replied_since_use) {
        return Route::To(u);
    }
    // 3. 只有一个在等。
    if i.waiting.len() == 1 {
        return Route::To(i.waiting[0]);
    }
    // 4. 好几个在等：不猜。
    if i.waiting.len() > 1 {
        return Route::Ask(i.waiting.to_vec());
    }
    // 5. 没候选也没 /use 过。
    Route::NeedUse
}

/// 敲字这一半的能力：把文字真的敲进某个会话的 PTY，并且知道该拿什么名字
/// 称呼这个会话给用户听——回执要报「敲给了谁」，光有一个 id 用户在手机上
/// 看不懂，必须换成人话（Ruling 7）。
///
/// 生产环境这层包的是 `SessionManager`（见下面 `impl SessionWriter for
/// crate::session::SessionManager`）；测试用假实现记录写了什么、写给了谁
/// （`for_test_with_writer`），不碰真 PTY。
pub trait SessionWriter: Send + Sync {
    /// 把 `text` 敲进 `id` 对应的会话。会话已经不在了、或者敲的过程本身
    /// 出了错，返回 `Err`——**错误信息必须已经是能给用户看的人话**，
    /// `deliver` 不会再加工它，只会原样往回执里放（`Delivered::Failed`）。
    fn type_into(&self, id: u32, text: &str) -> std::result::Result<(), String>;
    /// 这个会话给用户看该叫什么名字。跟 `SessionInfo::tag` 同一条规则：
    /// 起过名字用名字，没起过退回 profile。会话已经不在了（比如决定敲给
    /// 它之后、真的敲之前那一小段时间窗口里没掉了）返回 `None`——调用方
    /// 退化成用编号称呼它，绝不编一个不存在的名字。
    fn name_of(&self, id: u32) -> Option<String>;
}

/// Ruling 7 的落地：`SessionManager` 已经有 `send_input`（敲字）和 `list`
/// （取 `id -> tag/profile`），直接包一层就是完整的 `SessionWriter`——不用
/// 在 `SessionManager` 里另开一条路。`list()` 每次都拷一份快照，这里只找
/// 一个 id，代价跟 `dct ps` 刷新一次看板一样，不是热路径。
impl SessionWriter for crate::session::SessionManager {
    fn type_into(&self, id: u32, text: &str) -> std::result::Result<(), String> {
        self.send_input(id, text).map_err(|e| e.to_string())
    }

    fn name_of(&self, id: u32) -> Option<String> {
        self.list().into_iter().find(|s| s.id == id).map(|s| {
            if s.tag.is_empty() {
                s.profile
            } else {
                s.tag
            }
        })
    }
}

/// `deliver()` 的答案——四条 `Route` 各对应一个变体，`Failed` 是
/// `To(id)` 敲的时候真的出错那一支（会话在 `route()` 判定之后、真敲之前
/// 那道窄缝里没掉了，或者别的写入错误）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivered {
    /// 敲进了这一个会话。
    Typed(u32),
    /// 问了用户，这是问出去的候选。
    AskedWhich(Vec<u32>),
    /// 说了「这条消息对应的会话已经不在了」。
    SaidGone,
    /// 说了「先去看看会话列表」。
    SaidNeedUse,
    /// 敲的时候出错了，这是已经组好、发给用户看的那句人话。
    Failed(String),
}

/// 会话已经不在了的兜底称呼——**绝不编一个不存在的名字**，诚实地报一个
/// 编号，好过看着更亲切但是假的一句话。
fn fallback_name(id: u32) -> String {
    format!("{id} 号会话")
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
    /// Task 6 产的出站事件，有界、drop-oldest，见 `QUEUE_CAP`。**谁把
    /// `session.rs::tick()` 那头 `mpsc::Receiver<Event>` 里的事件搬进
    /// 这里，是接线的事**——这个字段和 `enqueue`/`queued` 这两个方法
    /// 只负责「进来一条、满了丢最旧的」这条规则本身，不管调用方是谁。
    outbound: Mutex<VecDeque<Event>>,
    // …… 消息映射与当前会话见 Task 9/10
    /// 敲字这一半的能力（Ruling 7）。`None` = 还没接线——`Bridge::new` 之后
    /// 默认没有，调用方（daemon.rs，接线是另一个任务）通过 `set_writer`
    /// 装进真的 `SessionManager`；测试用 `for_test_with_writer` 装假的。
    /// `deliver()` 在这是 `None` 的时候不会假装敲成功了，见那边的分支。
    writer: Mutex<Option<Arc<dyn SessionWriter>>>,
    /// 手机来的消息最终去了哪儿，记进这本 journal——默认没设路径（跟
    /// `SessionManager::journal` 的默认值一样），单测因此不会碰真实的
    /// `~/.dct`。`set_journal_path` 接进跟会话生死同一份文件的路径，
    /// 好让两类记录能对上时间线，接线同样是另一个任务的事。
    journal: Journal,
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
            outbound: Mutex::new(VecDeque::new()),
            writer: Mutex::new(None),
            journal: Journal::new(),
        }
    }

    /// 接进敲字的能力（Ruling 7）。生产环境传一个包着 `SessionManager` 的
    /// `Arc`（`SessionManager` 已经 `impl SessionWriter`，见上面那段），
    /// 测试传假的记录器。
    pub fn set_writer(&self, w: Arc<dyn SessionWriter>) {
        *recover(self.writer.lock()) = Some(w);
    }

    /// 接进 journal 该写去哪个文件。**故意跟 `SessionManager::journal` 用
    /// 同一个路径**——两本账本讲的是同一条时间线（"会话是不是这时候没的"
    /// 跟"手机上这句话是不是这时候敲进去的"），分开的文件没法互相印证。
    pub fn set_journal_path(&self, p: PathBuf) {
        self.journal.set_path(p);
    }

    /// 只测 `deliver()` 用——渠道和写入器都是会记录下发生了什么的假实现，
    /// 不碰真 PTY、也不碰网络。见 `tests::Spy`。
    #[cfg(test)]
    fn for_test_with_writer() -> (Bridge, Arc<tests::Spy>) {
        let spy = Arc::new(tests::Spy::default());
        let ch: Arc<dyn Channel> = spy.clone();
        let b = Bridge::new(ch, tests::blank_status(), Some(999), Box::new(|_| {}));
        b.set_writer(spy.clone() as Arc<dyn SessionWriter>);
        (b, spy)
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

    /// 消费队列这一半：把一条 `session.rs::tick()` 产的事件收进来。
    /// **满了丢最旧的一条**（`QUEUE_CAP`，Ruling 4）——不是拒收新的，
    /// 新的事件永远进得来，代价是队首那条最老的被挤掉。
    pub fn enqueue(&self, e: Event) {
        let mut q = recover(self.outbound.lock());
        if q.len() >= QUEUE_CAP {
            q.pop_front();
        }
        q.push_back(e);
    }

    /// 队列此刻的快照，按进队顺序（最老的在前）。只读、不消费——
    /// 测试和（后续任务的）实际发送都要看这份内容，谁都不该在看一眼的
    /// 同时把别人还没读到的事件顺手清空。
    pub fn queued(&self) -> Vec<Event> {
        recover(self.outbound.lock()).iter().cloned().collect()
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

    /// `route()` 已经决定了该往哪儿去，这里是把决定真的落地。**`To` 敲，
    /// 另外三支什么都不敲**——回执不是锦上添花：用户在外面看不见终端，
    /// 没有回执他不知道这句话到底进去没有；而 `Ask`/`Gone`/`NeedUse` 存在
    /// 的全部意义就是"这次不该猜"，回一句人话、绝不动 PTY 才是它们唯一
    /// 正确的做法。**全部记 journal**——手机来的这条消息最终去了哪儿，
    /// 跟会话自己怎么没的一样，得留得下痕迹。
    pub fn deliver(&self, route: Route, text: &str) -> Delivered {
        match route {
            Route::To(id) => self.deliver_to(id, text),
            Route::Ask(ids) => {
                self.reply(&self.ask_message(&ids));
                self.journal.delivered(Delivery::Asked(ids.len()));
                Delivered::AskedWhich(ids)
            }
            Route::Gone => {
                self.reply(
                    "这条消息对应的会话已经不在了，没有敲给任何人。先发 /ls 看看现在有哪些会话",
                );
                self.journal.delivered(Delivery::Gone);
                Delivered::SaidGone
            }
            Route::NeedUse => {
                self.reply("先发 /ls 看看有哪些会话，再回复其中一条，或者发 /use 加编号指定一个");
                self.journal.delivered(Delivery::NeedUse);
                Delivered::SaidNeedUse
            }
        }
    }

    /// `Route::To` 那一支：真的敲、再报一句回执。**唯一会碰 `writer` 的
    /// 地方**，`Ask`/`Gone`/`NeedUse` 三支绝不会走到这个方法里。
    fn deliver_to(&self, id: u32, text: &str) -> Delivered {
        let Some(writer) = recover(self.writer.lock()).clone() else {
            // 还没接线（`set_writer` 没被调用过）——不是用户的错，但也
            // 绝不能假装敲进去了，那正是"用户以为进去了、其实没有"这条
            // 最贵的错误路径。
            let msg = "这句话现在发不出去，稍后再试一次".to_string();
            self.reply(&msg);
            self.journal.delivered(Delivery::Failed(id));
            return Delivered::Failed(msg);
        };
        match writer.type_into(id, text) {
            Ok(()) => {
                let name = writer.name_of(id).unwrap_or_else(|| fallback_name(id));
                self.reply(&format!("已经敲进「{name}」"));
                self.journal.delivered(Delivery::Typed(id));
                Delivered::Typed(id)
            }
            Err(_) => {
                // `writer.type_into` 的错误信息不往回执里带——那是内部
                // 诊断用的字符串，不保证已经是人话（`SessionManager` 那边
                // 只是把 `anyhow::Error` 转成了 `to_string()`）。用户只需要
                // 知道"没进去"和"该敲给谁"，不需要知道底层是哪种失败。
                let name = writer.name_of(id).unwrap_or_else(|| fallback_name(id));
                let msg = format!("这句话没能敲进「{name}」，稍后再试一次");
                self.reply(&msg);
                self.journal.delivered(Delivery::Failed(id));
                Delivered::Failed(msg)
            }
        }
    }

    /// 好几个候选时回的那句话。**尽量点名字**——`9 号`比`9 号「装依赖」`
    /// 难认得多，写入器给不出名字（还没接线，或者那个会话恰好也没了）
    /// 就诚实地退回编号，不编。
    fn ask_message(&self, ids: &[u32]) -> String {
        let writer = recover(self.writer.lock()).clone();
        let list = ids
            .iter()
            .map(|&id| {
                let name = writer.as_ref().and_then(|w| w.name_of(id));
                match name {
                    Some(name) => format!("{id} 号「{name}」"),
                    None => format!("{id} 号"),
                }
            })
            .collect::<Vec<_>>()
            .join("、");
        format!("不确定该说给哪个：{list}？回复其中一条推送，或者发 /use 加编号指定一个")
    }

    /// 回一句给主人。**没有主人（理论上不该发生，`deliver` 只该在
    /// `accept()` 判过是主人之后被调用）就什么都不发**——发错人比不发
    /// 更糟，`Channel::send` 的错误也一并吞掉，同 `journal.rs` 那条
    /// 「记不下来/发不出去不该连累别的事」的规矩。
    fn reply(&self, text: &str) {
        if let Some(to) = *recover(self.owner.lock()) {
            let _ = self.ch.send(to, text);
        }
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

/// 会话没起名字（还没跑完过一轮）时，`merge` 用来称呼它的那句话——
/// 跟 `fallback_name` 是同一条规矩：诚实报一个编号，好过编一个不存在的
/// 名字。
fn event_label(e: &Event) -> String {
    let name = if e.name.trim().is_empty() {
        fallback_name(e.session)
    } else {
        format!("「{}」", e.name)
    };
    format!("{name}（{}）", e.project)
}

/// 这一类事件该用哪句人话收尾。
fn event_verb(kind: crate::channel::EventKind) -> &'static str {
    match kind {
        crate::channel::EventKind::Stopped => "干完停下来了",
        crate::channel::EventKind::Failed => "报错了",
        crate::channel::EventKind::Vanished => "自己不见了",
    }
}

/// 把攒了一段时间的几个事件合并成**一条**发给手机的消息。**不需要任何
/// 模型**——这是它存在的全部意义：断网八小时之后，用户重新连上手机
/// 通知，不该在那一瞬间收到几百条推送。`lang` 目前不参与分支：手机上
/// 的文案跟 `broken_message` 一样只写了中文这一种（已有评审认定这条
/// precedent 成立），参数留着只是为了让这个纯函数跟别处「输出人话、
/// 带 `Lang`」的签名保持一致，界面语言的切换（`l` 键）不该影响发到
/// 手机上的字。
///
/// **只有一件事的时候绝不排编号列表**——`a_single_event_is_not_dressed_
/// up_as_a_list` 钉的就是这条：一件事本来就不是一个「列表」，编上
/// 「1.」纯属多余的形式感，还会让用户以为还有别的事没显示全。
pub fn merge(events: &[Event], lang: crate::i18n::Lang) -> String {
    let _ = lang;
    if events.len() == 1 {
        let e = &events[0];
        return format!("{}{}", event_label(e), event_verb(e.kind));
    }
    let lines: Vec<String> = events
        .iter()
        .enumerate()
        .map(|(i, e)| format!("{}. {}{}", i + 1, event_label(e), event_verb(e.kind)))
        .collect();
    format!("有 {} 件事：\n{}", events.len(), lines.join("\n"))
}

/// 屏幕文字最多喂给模型这么多字符。跟 `session.rs::explain_prompt`/
/// `name_prompt` 用的是同一个数字、同一条理由：整屏几千字又慢又贵，
/// 还容易让模型抓错重点，只送末尾就够了。
const OPTIONS_TAIL: usize = 2000;

/// 问模型「这个屏幕是不是在等用户从几个选项里选一个」。跟
/// `session.rs::explain_prompt`/`name_prompt` 走的是同一条范式：纯函数、
/// 只喂屏幕末尾、prompt 里把隐私边界写死。
///
/// **这里的要求是双保险的一半**：prompt 明确禁止路径、代码块、diff——
/// 但 prompt 只是「请求」，模型不听话是常态，真正的保证在 `parse_options`
/// 那道过滤（见它的文档）。
pub fn options_prompt(screen: &str) -> crate::llm::Prompt {
    let tail: String = {
        let chars: Vec<char> = screen.chars().collect();
        let start = chars.len().saturating_sub(OPTIONS_TAIL);
        chars[start..].iter().collect()
    };
    crate::llm::Prompt {
        system: "你在帮一个完全不懂编程的人。看看这段命令行工具的屏幕内容，\
                 判断它是不是正停下来等用户从几个选项里选一个。如果是，把\
                 每个选项概括成几个字的大白话，编号列出，一行一个，格式是\
                 「1. 选项内容」。如果看不出是在等选择，就只回复「没有选项」。\
                 **绝不能出现文件路径、目录、代码块、反引号、diff、命令行\
                 原文**——用户在手机上看，这些东西对他没有意义，还可能\
                 泄露不该出现在这里的内容。"
            .into(),
        user: format!("这是屏幕上的最后一段内容：\n\n{tail}"),
        max_tokens: 200,
    }
}

/// 从模型的答案里洗出选项列表。**解析不出来就是没有选项，绝不猜**——
/// 这条规则比任何具体的格式细节都重要：模型答得含糊、跑题、或者干脆
/// 说「没有选项」，调用方都该拿到 `None`，退回只有元数据的兜底消息，
/// 而不是把模型那句话本身当成唯一的「选项」塞给用户。
///
/// **隐私过滤的另一半**（跟 `options_prompt` 的 prompt 要求配对）：任何
/// 一行选项只要带 `/` 或反引号，整行丢弃，不管前面编号解析得多干净——
/// prompt 只是请求模型别这么写，这里才是真正兜底的保证。丢弃的是那一
/// 行，不是整个答案：其余能用的选项还是照常返回。
pub fn parse_options(raw: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let Some(rest) = strip_numbered_prefix(line.trim()) else {
            continue;
        };
        let candidate = rest.trim();
        if candidate.is_empty() {
            continue;
        }
        if candidate.contains('/') || candidate.contains('`') {
            continue;
        }
        out.push(candidate.to_string());
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 剥掉一行开头的编号前缀（`1.`/`1、`/`1)`），剥不掉就说明这行根本不是
/// 编号列表的一部分。
fn strip_numbered_prefix(line: &str) -> Option<&str> {
    let digits_end = line
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit())
        .last()
        .map(|(i, c)| i + c.len_utf8())?;
    let rest = &line[digits_end..];
    rest.strip_prefix('.')
        .or_else(|| rest.strip_prefix('、'))
        .or_else(|| rest.strip_prefix(')'))
        .or_else(|| rest.strip_prefix('）'))
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

/// **Ruling 10 的落地点.** 队列的另一半：把 `session.rs::tick()` 那头的
/// `mpsc::Receiver<Event>` 接进来，一有事件到达就转手交给**此刻**槽里
/// 那个活着的 bridge。
///
/// **这是唯一常驻到整个守护进程生命周期的消费者，它不属于任何一个具体
/// 的 `Bridge` 实例。** `replace()`（换令牌）、`stop_current()`（关掉）
/// 都只改变槽里那个 `Option<BridgeHandle>`，这个消费者线程本身从不
/// 重启，`daemon.rs::run_with_manager` 只在启动时 `spawn` 一次——这正是
/// 为什么不会重演 C2/C3 那种「换一次令牌就多出一条线程，两条各管各的」：
/// 这里从头到尾只有一条消费者线程，它每次都重新看一眼槽里此刻是谁。
///
/// **没有活着的 bridge 时，事件被故意丢弃**，不是缺陷：没有令牌、
/// 刚被 `PhoneDisable`、或者正处在 `replace()` 换令牌那极短的窗口里，
/// 这时候「发给谁」这个问题本身没有答案。**故意不攒起来以后补发**——
/// 攒着的话，用户重新打开手机通知的那一刻会被一堆早就过期的「某会话
/// 停下来了」糊一脸，比什么都不说更糟；`should_notify`/`debounce` 保证
/// 的是「响的时候值得响」，不是「保证响过」。
///
/// **绝不阻塞 `tick()`，绝不向外 panic**：`rx.recv()` 会阻塞这**一个**
/// 消费者线程本身，但那正是它该做的事——`tick()` 只管往 unbounded 的
/// `mpsc::Sender` 里 `send()`，从不等在这里，两条线程之间没有反向的
/// 依赖。每一次真正的处理（查槽、`enqueue`）都包在 `catch_unwind` 里，
/// 单次失败不会把整条消费者线程带走，同 `spawn()` 那条「手机通道死掉是
/// 遗憾，会话死掉才是灾难」的规矩，也同这里「一个停掉的 bridge 不该
/// 继续被喂事件」——槽里读到 `None` 就是全部要做的事，不会去戳一个已经
/// `stop()` 过的旧 `Bridge`。
pub fn spawn_event_consumer(
    rx: mpsc::Receiver<Event>,
    slot: Arc<Mutex<Option<BridgeHandle>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || loop {
        let event = match rx.recv() {
            Ok(e) => e,
            // 发送端掉了：持着 `mpsc::Sender` 的 `SessionManager` 没了，
            // 说明整个 daemon 在关——这个消费者线程也该跟着退出，
            // 不是错误，不用报。
            Err(_) => return,
        };
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Some(handle) = recover(slot.lock()).as_ref() {
                handle.bridge.enqueue(event);
            }
            // `None`：没有活着的 bridge，见函数文档——故意丢弃，
            // 不攒、不重试。
        }));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::time::Instant;

    fn ev(session: u32) -> Event {
        Event {
            session,
            kind: crate::channel::EventKind::Stopped,
            name: String::new(),
            project: "p".into(),
        }
    }

    /// 基本形状：进去几条，`queued()` 原样按顺序吐回来。Task 11 会补
    /// 「满了丢最旧」那条更细的测试；这里先钉住没满的时候完全不该丢。
    #[test]
    fn enqueue_keeps_everything_under_the_cap() {
        let b = Bridge::for_test();
        b.enqueue(ev(1));
        b.enqueue(ev(2));
        b.enqueue(ev(3));
        let sessions: Vec<u32> = b.queued().iter().map(|e| e.session).collect();
        assert_eq!(sessions, vec![1, 2, 3]);
    }

    /// 满了之后再来一条，被挤掉的必须是**最旧**的那条，不是最新的——
    /// Ruling 4 的全部依据：对 stop/fail 通知来说，最新的事件才是有用的。
    #[test]
    fn enqueue_drops_the_oldest_when_the_queue_is_full() {
        let b = Bridge::for_test();
        for i in 0..QUEUE_CAP as u32 {
            b.enqueue(ev(i));
        }
        b.enqueue(ev(QUEUE_CAP as u32)); // 这一条让队列溢出一条

        let sessions: Vec<u32> = b.queued().iter().map(|e| e.session).collect();
        assert_eq!(sessions.len(), QUEUE_CAP, "队列不该超过上限");
        assert_eq!(sessions[0], 1, "最旧的那条（0号）该被挤掉");
        assert_eq!(
            *sessions.last().unwrap(),
            QUEUE_CAP as u32,
            "最新那条必须留着"
        );
    }

    // ---- spawn_event_consumer：Ruling 10 的接线本身 ----

    fn handle_around(b: Bridge) -> BridgeHandle {
        BridgeHandle {
            bridge: Arc::new(b),
        }
    }

    /// **走真实接线，不直接调 `enqueue`。** 槽里有一个活着的 bridge，
    /// `tx.send()` 之后事件必须真的出现在那个 bridge 的 `queued()` 里——
    /// 这条测试钉的是「消费者线程真的把 `mpsc::Receiver` 那头收到的东西
    /// 转手交给了槽里的 bridge」这件事本身，不是 `enqueue`/`should_notify`
    /// 各自的正确性（那两个已经有别的测试钉住了）。
    #[test]
    fn an_event_sent_through_the_channel_reaches_the_live_bridge() {
        let slot: Arc<Mutex<Option<BridgeHandle>>> =
            Arc::new(Mutex::new(Some(handle_around(Bridge::for_test()))));
        let (tx, rx) = mpsc::channel();
        spawn_event_consumer(rx, slot.clone());

        tx.send(ev(1)).unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let sessions: Vec<u32> = {
                let g = recover(slot.lock());
                g.as_ref()
                    .unwrap()
                    .bridge
                    .queued()
                    .iter()
                    .map(|e| e.session)
                    .collect()
            };
            if sessions == vec![1] {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "事件该经真实的消费者线程落到 bridge 的队列里，实际看到 {sessions:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// 槽里没有活着的 bridge（没配令牌、刚被 `PhoneDisable`）：事件必须
    /// 被**故意丢弃**，不阻塞、不 panic、也不偷偷攒在别的地方等以后补发
    /// ——之后哪怕真的接上一个 bridge，早先那些事件也不该突然冒出来。
    #[test]
    fn events_are_dropped_without_blocking_when_no_bridge_is_live() {
        let slot: Arc<Mutex<Option<BridgeHandle>>> = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::channel();
        spawn_event_consumer(rx, slot.clone());

        // 连发好几条：没有 bridge 接收，`send()` 本身是 unbounded 的，
        // 这几行不该卡住——超时用来给"万一真的卡住了"一个可观测的失败，
        // 而不是让测试本身悬在那里。
        let deadline = Instant::now() + Duration::from_secs(2);
        for i in 1..=5u32 {
            assert!(
                Instant::now() < deadline,
                "没有 bridge 时 send() 也不该卡住"
            );
            tx.send(ev(i)).unwrap();
        }

        // 给消费者线程一点时间，让它真的把这 5 条在槽还是 `None` 的时候
        // 处理掉（也就是丢掉）——不留这段余量的话，槽可能在消费者线程
        // 还没来得及看第一条之前就被下面这行改成 `Some`，测的就不再是
        // "没有 bridge 时会丢"，而是纯粹的线程调度赛跑。跟本文件别处
        // "sleep 一小段再断言" 的写法（比如 `replace_stops_the_old_bridge_
        // before_starting_the_new_one`）同一个理由。
        std::thread::sleep(Duration::from_millis(200));

        // 现在才接上一个 bridge：如果消费者线程在没有 bridge 那段时间
        // 把事件攒在了别的地方，这里就会看到 1..=5 一起冒出来；正确行为
        // 是那 5 条已经被丢了，只有**之后**发的这一条会出现。
        *recover(slot.lock()) = Some(handle_around(Bridge::for_test()));
        tx.send(ev(99)).unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let sessions: Vec<u32> = {
                let g = recover(slot.lock());
                g.as_ref()
                    .unwrap()
                    .bridge
                    .queued()
                    .iter()
                    .map(|e| e.session)
                    .collect()
            };
            if !sessions.is_empty() {
                assert_eq!(
                    sessions,
                    vec![99],
                    "没有 bridge 时发的那 5 条不该被攒起来、事后补发"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "接上 bridge 之后新发的事件也该正常送达，而不是消费者线程被卡住了"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

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

    // ---- route()：五条规则，来自 brief 的失败测试 ----

    fn input<'a>(
        reply_to: Option<MsgId>,
        map: &'a HashMap<MsgId, u32>,
        used: Option<u32>,
        replied_since_use: bool,
        waiting: &'a [u32],
    ) -> RouteInput<'a> {
        RouteInput {
            reply_to,
            map,
            used,
            replied_since_use,
            waiting,
        }
    }

    #[test]
    fn a_reply_goes_where_it_replied() {
        let map = HashMap::from([(42, 7)]);
        assert_eq!(
            route(&input(Some(42), &map, Some(3), false, &[9])),
            Route::To(7)
        );
    }

    /// **重启之后旧消息不能敲进任何地方。** 退化成「发给当前会话」正好是
    /// 敲错地方的那条路径。
    #[test]
    fn a_reply_to_a_message_we_no_longer_know_types_nothing() {
        let map = HashMap::new();
        assert_eq!(
            route(&input(Some(42), &map, Some(3), false, &[9])),
            Route::Gone
        );
    }

    /// `/use` 压过「唯一在等」：用户切过去就是想跟那个会话说话，
    /// 此刻另一个会话恰好在等，不能把他的话抢走。
    #[test]
    fn an_explicit_use_beats_a_waiting_session() {
        let map = HashMap::new();
        assert_eq!(
            route(&input(None, &map, Some(3), false, &[9])),
            Route::To(3)
        );
    }

    /// 但用户一旦长按回复过某条推送，注意力已经转走，`/use` 的指定作废——
    /// 否则一次 `/use` 会永久劫持所有不带回复的消息。
    #[test]
    fn use_expires_once_you_have_replied_to_a_push() {
        let map = HashMap::new();
        assert_eq!(route(&input(None, &map, Some(3), true, &[9])), Route::To(9));
    }

    #[test]
    fn the_only_one_waiting_gets_it() {
        let map = HashMap::new();
        assert_eq!(route(&input(None, &map, None, false, &[9])), Route::To(9));
    }

    /// 好几个在等就不猜。敲错 agent 的代价比多问一句大得多。
    #[test]
    fn several_waiting_means_ask_not_guess() {
        let map = HashMap::new();
        assert_eq!(
            route(&input(None, &map, None, false, &[9, 10])),
            Route::Ask(vec![9, 10])
        );
    }

    #[test]
    fn nothing_waiting_and_no_use_asks_for_ls() {
        let map = HashMap::new();
        assert_eq!(route(&input(None, &map, None, false, &[])), Route::NeedUse);
    }

    // ---- 额外的对抗性测试：钉住变异测试列表之外容易被忽略的角落 ----

    /// 带回复的消息即便此刻同时有 `/use` 和好几个会话在等，规则 1 也必须
    /// 第一个生效——回复动作本身就是最明确的指向,不该被后面任何一条
    /// 规则盖过。这条测试防的是"整体规则顺序被打乱"，而不仅仅是
    /// brief 里点名的"2、3 互换"。
    #[test]
    fn a_reply_wins_over_everything_else_even_with_use_and_many_waiting() {
        let map = HashMap::from([(1, 5)]);
        assert_eq!(
            route(&input(Some(1), &map, Some(3), false, &[9, 10])),
            Route::To(5)
        );
    }

    /// `Gone` 同样必须盖过 `/use` 和"在等"——找不到映射就是找不到,
    /// 不该因为凑巧有别的候选就悄悄改答案。
    #[test]
    fn gone_wins_over_use_and_waiting_too() {
        let map = HashMap::new();
        assert_eq!(
            route(&input(Some(999), &map, Some(3), false, &[9, 10])),
            Route::Gone
        );
    }

    /// 没有 `/use`、也没有回复，但已经"回复过推送"这件事本身对"唯一在等"
    /// 这条规则没有任何影响——`replied_since_use` 只管 `/use` 的生死,
    /// 不该意外地也去干扰规则 3/4。
    #[test]
    fn replied_since_use_does_not_affect_the_waiting_rules_when_there_is_no_use() {
        let map = HashMap::new();
        assert_eq!(route(&input(None, &map, None, true, &[9])), Route::To(9));
        assert_eq!(
            route(&input(None, &map, None, true, &[9, 10])),
            Route::Ask(vec![9, 10])
        );
    }

    // ---- deliver()：敲字、回执、journal，来自 brief 的失败测试 ----

    /// 一身兼二职的假实现：既是敲字的 `SessionWriter`，也是发回执的
    /// `Channel`——两边共用一份记录，测试只需要问一个对象"发生了什么"。
    /// **不碰真 PTY、不碰网络**，`poll`/`get_me`/`drain` 这条测试用不到，
    /// 真被调用就是测试写错了，照 `NeverCalled` 的先例让它 panic。
    #[derive(Default)]
    pub(super) struct Spy {
        written: Mutex<Vec<(u32, String)>>,
        replies: Mutex<Vec<String>>,
        names: Mutex<HashMap<u32, String>>,
        /// 敲字时该失败的会话号——空集合表示从不失败。
        fail: Mutex<std::collections::HashSet<u32>>,
    }

    impl Spy {
        /// 敲进了哪些会话、敲了什么，按发生顺序。
        pub(super) fn written(&self) -> Vec<(u32, String)> {
            self.written.lock().unwrap().clone()
        }

        /// 最后一条回给主人的话——回执/候选列表/Gone/NeedUse 都走这里。
        pub(super) fn last_reply(&self) -> String {
            self.replies
                .lock()
                .unwrap()
                .last()
                .cloned()
                .unwrap_or_default()
        }

        /// 目前一共回了几句——用来钉"这一支到底有没有回过话"。
        fn reply_count(&self) -> usize {
            self.replies.lock().unwrap().len()
        }

        /// 给某个 id 配一个名字，`name_of` 就答这个。
        pub(super) fn name(&self, id: u32, name: &str) {
            self.names.lock().unwrap().insert(id, name.to_string());
        }

        /// 让敲某个会话这件事失败——测 `Delivered::Failed` 那一支用。
        pub(super) fn fail_on(&self, id: u32) {
            self.fail.lock().unwrap().insert(id);
        }
    }

    impl Channel for Spy {
        fn send(&self, _to: i64, text: &str) -> Result<MsgId, ChannelError> {
            self.replies.lock().unwrap().push(text.to_string());
            Ok(0)
        }
        fn poll(&self, _timeout: Duration) -> Result<Vec<Incoming>, ChannelError> {
            panic!("deliver() 测试不该碰渠道的 poll()")
        }
        fn get_me(&self) -> Result<String, ChannelError> {
            panic!("deliver() 测试不该碰渠道的 get_me()")
        }
        fn drain(&self, _timeout: Duration) -> Result<usize, ChannelError> {
            panic!("deliver() 测试不该碰渠道的 drain()")
        }
    }

    impl SessionWriter for Spy {
        fn type_into(&self, id: u32, text: &str) -> std::result::Result<(), String> {
            if self.fail.lock().unwrap().contains(&id) {
                return Err(format!("模拟写入失败：会话 {id}"));
            }
            self.written.lock().unwrap().push((id, text.to_string()));
            Ok(())
        }
        fn name_of(&self, id: u32) -> Option<String> {
            self.names.lock().unwrap().get(&id).cloned()
        }
    }

    /// 回执不是锦上添花：用户在外面看不见终端，没有回执他不知道这句话
    /// 到底进去没有。
    #[test]
    fn typing_it_in_sends_a_receipt_naming_the_session() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.name(7, "修登录白屏");
        let d = b.deliver(Route::To(7), "先跑完");
        assert_eq!(d, Delivered::Typed(7));
        assert_eq!(spy.written(), vec![(7, "先跑完".to_string())]);
        assert!(
            spy.last_reply().contains("修登录白屏"),
            "回执里没说敲给了谁"
        );
    }

    /// `Gone` 什么都不敲。这是重启之后那条安全路径的落地，光有 `route()`
    /// 返回 `Gone` 不够，得确认真的没写出去。
    #[test]
    fn a_gone_route_writes_nothing_at_all() {
        let (b, spy) = Bridge::for_test_with_writer();
        assert_eq!(b.deliver(Route::Gone, "先跑完"), Delivered::SaidGone);
        assert!(spy.written().is_empty(), "旧消息被敲进了会话");
    }

    #[test]
    fn asking_which_writes_nothing_either() {
        let (b, spy) = Bridge::for_test_with_writer();
        assert_eq!(
            b.deliver(Route::Ask(vec![9, 10]), "先跑完"),
            Delivered::AskedWhich(vec![9, 10])
        );
        assert!(spy.written().is_empty());
    }

    /// **额外测试，brief 的三条只挑了 `Gone`/`Ask`。** `NeedUse` 是第三条
    /// "这次不该猜"的分支，同样必须一个字都不敲——只测这三条里的两条，
    /// 剩下这条会被漏掉。
    #[test]
    fn a_need_use_route_writes_nothing_at_all() {
        let (b, spy) = Bridge::for_test_with_writer();
        assert_eq!(b.deliver(Route::NeedUse, "先跑完"), Delivered::SaidNeedUse);
        assert!(spy.written().is_empty(), "NeedUse 也不该敲任何东西");
    }

    /// 三条"什么都不敲"的分支必须**照样回一句话**——不回话的话，用户在
    /// 手机上看到的是消息发出去之后死一般的沉默，跟真没收到没有区别。
    #[test]
    fn gone_ask_and_need_use_all_still_reply_something() {
        let (b, spy) = Bridge::for_test_with_writer();
        b.deliver(Route::Gone, "x");
        assert_eq!(spy.reply_count(), 1, "Gone 也要回一句话");
        b.deliver(Route::Ask(vec![1, 2]), "x");
        assert_eq!(spy.reply_count(), 2, "Ask 也要回一句话");
        b.deliver(Route::NeedUse, "x");
        assert_eq!(spy.reply_count(), 3, "NeedUse 也要回一句话");
    }

    /// 会话在 `route()` 判定"敲给它"之后、真的敲进去之前的那道窄缝里
    /// 没掉了（或者别的写入错误）：`deliver` 不能悄悄吞掉这件事，得诚实
    /// 报 `Failed`，回执也要说清楚没进去，而不是假装 `Typed`。
    #[test]
    fn a_write_failure_is_reported_honestly_not_swallowed() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.fail_on(7);
        let d = b.deliver(Route::To(7), "先跑完");
        match d {
            Delivered::Failed(msg) => assert!(!msg.is_empty()),
            other => panic!("期待 Failed，得到 {other:?}"),
        }
        assert!(spy.written().is_empty(), "写失败就不该出现在写入记录里");
        assert_eq!(
            spy.reply_count(),
            1,
            "失败也要回一句话，不能假装什么都没发生"
        );
    }

    /// 没接线（`set_writer` 没被调用过）：`To` 分支绝不能假装敲成功了。
    #[test]
    fn deliver_to_without_a_writer_fails_honestly() {
        let b = Bridge::for_test();
        let d = b.deliver(Route::To(7), "先跑完");
        match d {
            Delivered::Failed(msg) => assert!(!msg.is_empty()),
            other => panic!("期待 Failed，得到 {other:?}"),
        }
    }

    /// 手机端的字绝不能带路径、diff 或代码块——这是隐私边界，回执和候选
    /// 列表也不例外（见 CLAUDE.md 里那条约束）。这里钉住四条分支产出的
    /// 文案里没有明显的代码块/路径痕迹。
    #[test]
    fn phone_facing_text_never_looks_like_a_path_or_a_code_block() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.name(7, "修登录白屏");
        b.deliver(Route::To(7), "先跑完");
        b.deliver(Route::Ask(vec![9, 10]), "先跑完");
        b.deliver(Route::Gone, "先跑完");
        b.deliver(Route::NeedUse, "先跑完");
        // 只检查真正发给手机的回执/提示，不检查敲进 PTY 的原文——那是
        // 用户自己打的字，敲字这条路本来就不该也不能过滤它。
        for reply in spy.replies.lock().unwrap().iter() {
            assert!(!reply.contains("```"), "不该带代码块：{reply}");
            assert!(!reply.contains('\n'), "不该是多行/带缩进的东西：{reply}");
        }
    }

    /// 四条路径全部记 journal——手机来的这条消息最终去了哪儿，跟会话
    /// 自己怎么没的一样，得留得下痕迹。
    #[test]
    fn all_four_routes_are_journaled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.log");
        let (b, spy) = Bridge::for_test_with_writer();
        b.set_journal_path(path.clone());

        b.deliver(Route::To(7), "先跑完");
        b.deliver(Route::Ask(vec![9, 10]), "先跑完");
        b.deliver(Route::Gone, "先跑完");
        b.deliver(Route::NeedUse, "先跑完");
        spy.fail_on(8);
        b.deliver(Route::To(8), "再来一句");

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("typed session=7"), "{text}");
        assert!(text.contains("asked candidates=2"), "{text}");
        assert!(text.contains("gone"), "{text}");
        assert!(text.contains("need_use"), "{text}");
        assert!(text.contains("failed session=8"), "{text}");
        assert!(!text.contains("先跑完"), "journal 不该带消息原文：{text}");
    }

    // ---- 变异测试：按 brief 的两条手改一遍，确认测试真的会失败 ----
    //
    // 两处手改都直接在这份文件上做、跑指定测试确认失败、再撤销，跟
    // Task 7 报告里记录的手法一样。下面这两条测试本身就是"钉子"——
    // 留在代码里，不需要真的改一遍源码才能验证；变异过程记在 task-8
    // 报告里。

    /// **钉住"Gone 绝不能敲字"这件事本身。** 如果有人把 `Gone` 分支改成
    /// 也调用 `type_into`，这条测试必须失败——它就是 brief 要求的那个
    /// 变异要打中的靶子。
    #[test]
    fn mutation_guard_gone_must_never_call_type_into() {
        let (b, spy) = Bridge::for_test_with_writer();
        b.deliver(Route::Gone, "不该被敲进任何地方的一句话");
        assert!(
            spy.written().is_empty(),
            "Gone 分支一旦调用了 type_into，这里就会看到写入记录"
        );
    }

    /// **钉住"回执必须报名字，不能只报编号"这件事本身。** 如果有人把
    /// `deliver_to` 里的 `name` 换成 `id.to_string()`，这条测试必须失败。
    #[test]
    fn mutation_guard_receipt_must_name_the_session_not_just_the_number() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.name(7, "修登录白屏");
        b.deliver(Route::To(7), "先跑完");
        let reply = spy.last_reply();
        assert!(
            reply.contains("修登录白屏"),
            "回执必须点名会话叫什么：{reply}"
        );
        assert!(
            !reply.contains("7 号会话"),
            "回执不该退化成只报编号（这里明明有名字可用）：{reply}"
        );
    }

    // ---- merge：合并不需要模型 ----

    fn named_event(
        session: u32,
        kind: crate::channel::EventKind,
        name: &str,
        project: &str,
    ) -> Event {
        Event {
            session,
            kind,
            name: name.to_string(),
            project: project.to_string(),
        }
    }

    /// 合并不需要模型。断网八小时不该在恢复瞬间收到五百条。
    #[test]
    fn several_events_become_one_message() {
        let evs = vec![
            named_event(1, crate::channel::EventKind::Stopped, "修登录白屏", "web"),
            named_event(2, crate::channel::EventKind::Failed, "对账", "fin"),
        ];
        let m = merge(&evs, crate::i18n::Lang::Zh);
        assert!(m.contains("修登录白屏") && m.contains("对账"));
        // 一条消息，不是两条拼起来——两个会话名之间不该出现消息分隔
        assert_eq!(m.matches("\n\n\n").count(), 0);
    }

    #[test]
    fn a_single_event_is_not_dressed_up_as_a_list() {
        let evs = vec![named_event(
            1,
            crate::channel::EventKind::Stopped,
            "修登录白屏",
            "web",
        )];
        let m = merge(&evs, crate::i18n::Lang::Zh);
        assert!(!m.contains("1."), "只有一件事却排了个编号列表：{m}");
    }

    /// 没起过名字的会话不能编一个假名字——诚实退回编号，跟 `deliver_to`
    /// 用的是同一条 `fallback_name`。
    #[test]
    fn merge_falls_back_to_a_number_when_a_session_has_no_name() {
        let evs = vec![named_event(9, crate::channel::EventKind::Vanished, "", "x")];
        let m = merge(&evs, crate::i18n::Lang::Zh);
        assert!(m.contains("9 号会话"), "没有名字该退回编号：{m}");
    }

    /// 三件事排出的编号必须是 1、2、3——不是"钉住有编号"这么松，是钉住
    /// 编号真的按进队顺序对上号，不是随手拼出来的。
    #[test]
    fn merge_numbers_several_events_in_order() {
        let evs = vec![
            named_event(1, crate::channel::EventKind::Stopped, "甲", "p1"),
            named_event(2, crate::channel::EventKind::Failed, "乙", "p2"),
            named_event(3, crate::channel::EventKind::Vanished, "丙", "p3"),
        ];
        let m = merge(&evs, crate::i18n::Lang::Zh);
        assert!(
            m.contains("1. ") && m.contains("2. ") && m.contains("3. "),
            "{m}"
        );
        // 顺序必须对上：甲排在乙前面，乙排在丙前面。
        let i_a = m.find('甲').unwrap();
        let i_b = m.find('乙').unwrap();
        let i_c = m.find('丙').unwrap();
        assert!(i_a < i_b && i_b < i_c, "编号顺序跟进队顺序对不上：{m}");
    }

    // ---- parse_options：解析不出来就是没有选项，绝不猜 ----

    /// 模型答得不成形就当没有选项——**绝不猜**，退回只有元数据的消息。
    #[test]
    fn unparseable_options_mean_no_options() {
        assert_eq!(parse_options("我觉得他大概想问你要不要继续吧"), None);
        assert_eq!(parse_options(""), None);
    }

    #[test]
    fn options_come_back_in_order() {
        let got = parse_options("1. 先跑完\n2. 现在改").unwrap();
        assert_eq!(got, vec!["先跑完".to_string(), "现在改".to_string()]);
    }

    /// 隐私边界的第二道保险：prompt 只是请求，`parse_options` 才是真正
    /// 的保证——含 `/` 或反引号的候选项整行丢弃，不管编号解析得多干净。
    #[test]
    fn options_containing_a_path_or_a_backtick_are_discarded() {
        let got = parse_options("1. 直接改 src/main.rs\n2. 先跑完\n3. 用 `cargo test`").unwrap();
        assert_eq!(got, vec!["先跑完".to_string()]);
    }

    /// 模型说「没有选项」这类不带编号的大白话：0 条能用的候选，`None`，
    /// 不是 `Some(vec![])`——两者对调用方来说必须是同一件事（退回兜底），
    /// 但 `None` 才是这个函数唯一诚实的表达。
    #[test]
    fn a_plain_no_reply_yields_no_options() {
        assert_eq!(parse_options("没有选项"), None);
    }

    /// 只剩隐私过滤会挡掉的候选项：全部丢弃之后一条不剩，同样是 `None`，
    /// 不能因为"编号解析成功过"就返回一个空列表糊弄过去。
    #[test]
    fn options_that_are_all_filtered_out_mean_no_options() {
        assert_eq!(parse_options("1. 修改 /etc/hosts\n2. 用 `ls`"), None);
    }

    // ---- options_prompt：范式跟 explain_prompt/name_prompt 一致 ----

    #[test]
    fn options_prompt_forbids_paths_and_code_and_carries_the_screen() {
        let p = options_prompt("…… 是继续跑完，还是现在就改？ ……");
        assert!(p.system.contains("路径"), "{}", p.system);
        assert!(p.system.contains("代码块"), "{}", p.system);
        assert!(p.system.contains("反引号"), "{}", p.system);
        assert!(p.system.contains("diff"), "{}", p.system);
        assert!(p.user.contains("是继续跑完，还是现在就改？"));
        assert!(p.max_tokens <= 200);
    }

    #[test]
    fn options_prompt_only_carries_the_screen_tail() {
        let long = "x".repeat(OPTIONS_TAIL + 500);
        let p = options_prompt(&long);
        // 用户部分不该把全部 500+2000 个字符都塞进去，只留末尾那一段。
        assert!(p.user.chars().count() <= OPTIONS_TAIL + 50);
    }
}
