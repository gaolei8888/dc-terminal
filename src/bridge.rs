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
use std::time::{Duration, Instant};

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

/// 推送消息 id -> 会话映射表的上限，跟 `QUEUE_CAP` 同一条丢最旧规则：
/// 长按回复的时效性本来就有限，一直没被回复的旧推送不值得无限占内存。
pub const MSG_MAP_CAP: usize = 256;

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

/// 发送线程每一轮醒来看一眼队列的间隔。**不需要跟 `DEBOUNCE_WINDOW`
/// 对齐**——去抖已经在 `session.rs::tick()` 往队列里放事件之前做完了
/// （`Session::last_notified`），发送线程这边只管"队列里现在有什么就
/// 发出去"，这个数字只是"多久看一眼"，选得比去抖窗口小得多，好让
/// 事件不会平白多等将近一整个间隔才被发出去。
const SEND_INTERVAL: Duration = Duration::from_millis(500);

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
    /// 把 `text` 敲进 `id` 对应的会话，**并且真的把这句话交出去**——不是
    /// 敲进输入框就算数。**这是安全评审的 C1 修复**：`session.rs::
    /// send_input` 把「写字符」和「按回车」拆成了两次调用（写字符本身
    /// 不会让 agent 开始干活，空字符串那一次才是回车、才会打检查点、
    /// 才会把状态推进 `Working`），`ui/grid.rs::send_reply` 的文档注释
    /// 明确写着这两步**不能合并、也不能反**——合并了就没有检查点，用户
    /// 按不了 `u` 回滚。以前这里只调用了第一步，手机那句回复会被敲进
    /// 输入框然后停在那里，agent 什么都不会跑，而用户收到的回执却说
    /// 「已经敲进」——这是能想到最贵的一种谎言：唯一看不见终端的那个人，
    /// 被明确告知一件没有发生的事已经发生了。
    ///
    /// **两步只要有一步失败就必须整体报 `Err`**——半句话被写进了 PTY
    /// 但没有真的提交，跟完全没敲没有本质区别（agent 都不会往下走），
    /// 绝不能因为「文字这一半成功了」就报 `Ok`：那样回执和 journal 记的
    /// `Typed` 依然是一句谎言。
    fn type_into(&self, id: u32, text: &str) -> std::result::Result<(), String>;
    /// 这个会话给用户看该叫什么名字。跟 `SessionInfo::tag` 同一条规则：
    /// 起过名字用名字，没起过退回 profile。会话已经不在了（比如决定敲给
    /// 它之后、真的敲之前那一小段时间窗口里没掉了）返回 `None`——调用方
    /// 退化成用编号称呼它，绝不编一个不存在的名字。
    fn name_of(&self, id: u32) -> Option<String>;
    /// 此刻正在等待用户输入的会话 id——`route()` 的 `RouteInput::waiting`
    /// 和 `/ls` 都要用它。**「等待」= `SessionState::Idle`**：干完一轮、
    /// 停在提示符前面，正是手机上那句「唯一在等的那个」该指的对象；
    /// `Working`/`Stopped`/`Failed`/`Unknown` 都不算——干着活的会话没有
    /// 「该敲给它」这回事，`Failed`/`Stopped` 也不是在等下一句话。
    fn waiting(&self) -> Vec<u32>;
}

/// Ruling 7 的落地：`SessionManager` 已经有 `send_input`（敲字）和 `list`
/// （取 `id -> tag/profile`），直接包一层就是完整的 `SessionWriter`——不用
/// 在 `SessionManager` 里另开一条路。`list()` 每次都拷一份快照，这里只找
/// 一个 id，代价跟 `dct ps` 刷新一次看板一样，不是热路径。
impl SessionWriter for crate::session::SessionManager {
    fn type_into(&self, id: u32, text: &str) -> std::result::Result<(), String> {
        // 跟 `ui/grid.rs::send_reply` 同一个两步约定，**顺序、拆分都不能
        // 变**：先把文字写进输入框，再单独送一次空字符串按回车——空字符
        // 串那一步才会打检查点、才会真的让 agent 开始干这一轮。手机来的
        // 文字不会是空串（Telegram 消息本身就带着 `text`），但同样照
        // `send_reply` 的分支写全，不假设调用方永远不会传空文本。
        if !text.is_empty() {
            self.send_input(id, text).map_err(|e| e.to_string())?;
        }
        // 这一步失败——文字可能已经躺在输入框里，但没有被提交——**必须
        // 让整体报错**，绝不能因为上一步成功了就在这里放行：那样回执和
        // journal 记的 `Typed` 会撒谎。
        self.send_input(id, "").map_err(|e| e.to_string())
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

    fn waiting(&self) -> Vec<u32> {
        self.list()
            .into_iter()
            .filter(|s| s.state == crate::session::SessionState::Idle)
            .map(|s| s.id)
            .collect()
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
    /// 推送消息 id -> 那条推送来自哪个会话（`RouteInput::map`）。**只有
    /// 只关涉一个会话的推送才会进这张表**——`route()` 的规则 1 只认识
    /// `u32`（单个会话）。一次合并了好几件事的推送走的是下面
    /// `ambiguous_pushes` 那张单独的表，见它的文档。
    ///
    /// 有界、drop-oldest，跟 `outbound` 同一条道理（`MSG_MAP_CAP`）：
    /// 守护进程一直跑下去，这张表不清理会无限长下去。
    outbound_map: Mutex<VecDeque<(MsgId, u32)>>,
    /// 推送消息 id -> 那条推送**合并**了哪几个会话。**这不是
    /// `outbound_map` 的备用方案，是它的必要补充**：早先的版本只给单
    /// 会话推送记映射，合并推送干脆不记，导致长按回复一条"有两件事"的
    /// 推送会落到 `Route::Gone`（"这条消息已经不认识了"）——可两个会话
    /// 明明都还活着、都还在等，这句话是在撒谎，而且偏偏撒在"好几个 agent
    /// 同时停下来"这种最需要分清楚该回给谁的场合。**正确答案是
    /// `Route::Ask`**："不确定该说给哪个，回复其中一条或者发 /use"——
    /// 跟"好几个会话同时在等"用的是同一条"不猜、问用户"的规则。
    ///
    /// `route_and_deliver` 在调用 `route()`（它只认识单会话的
    /// `outbound_map`）之前，先查这张表；两张表按 `MsgId` 互斥，同一个
    /// id 只会出现在其中一张里，见 `run_sender` 里写入时的分支。
    ///
    /// 同样有界、drop-oldest（`MSG_MAP_CAP`）。
    ambiguous_pushes: Mutex<VecDeque<(MsgId, Vec<u32>)>>,
    /// `/use <n>` 选中的会话，`None` = 没有显式选过，或者选过但已经
    /// 因为 `replied_since_use` 作废。
    used: Mutex<Option<u32>>,
    /// 自从上一次 `/use` 之后，用户是不是已经长按回复过至少一条推送——
    /// `route()` 规则 3 的依据。`/use` 每次被重新设置都清成 `false`。
    replied_since_use: Mutex<bool>,
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
    /// `options_prompt`/`map_answer`/`narrow` 要用的后端，**跟
    /// `session.rs::SessionManager` 出错解释共用同一份**（见
    /// `SessionManager::backend`）。`None` = 没配 `[llm]`（或者配了但连不
    /// 上）——三个功能全都安静下线：推送只带元数据、回复原样敲进去、
    /// 好几个候选照旧反问，这是 `Config::llm` 那道隐私边界在这里唯一
    /// 正确的落地（CLAUDE.md「每一处用法都必须有不依赖 LLM 的退路」）。
    backend: Mutex<Option<Arc<dyn crate::llm::Backend>>>,
    /// 会话 id -> （这份选项是什么时候推送的，选项列表）。**只在这一支被
    /// 用一次**：`deliver_to` 用它把这条回复交给 `map_answer` 转成 agent
    /// 要的序号，用完（或者被下一条更新的推送覆盖/清掉）就不该继续留着
    /// ——一份过期的选项拿去解读一条毫不相干的新回复，比没有选项更危险
    /// （见 `compose_outbound` 里"没拿到新选项就清掉旧的"那一段）。
    ///
    /// **`Instant` 是安全评审要求补的那道结构性保证**：只在"下一条推送
    /// 又提到这个会话"时才清掉不够——agent 完全可能在没有再产生一条事件
    /// 的情况下就翻过了这个问题（用户直接在终端里回答了它，或者这一轮
    /// 的通知被 debounce 掉了），那样条目会一直留着，直到这个会话的
    /// **下一次**含糊回复被错误地喂给 `map_answer`。一个不听话的模型
    /// 这时候答"1"，dct 就会把用户明明打的一整句话换成"1"——这正是红线
    /// 不允许发生的事：把"该不该相信模型"变成模型自己说了算，而不是
    /// 结构上就不给它这个机会。`deliver_to` 读取时额外检查
    /// `PENDING_OPTIONS_TTL`，超时的条目一律当没有，见那边的文档。
    pending_options: Mutex<HashMap<u32, (Instant, Vec<String>)>>,
}

/// `pending_options` 里一条选项记录还能被信任多久。**故意选得短**：
/// 真实使用场景里，用户看到带选项的推送到回复，通常是几十秒到几分钟
/// 的事——他在看手机、决定要不要跑完还是现在改。超过这个窗口更可能是
/// agent 已经翻篇了（用户从终端直接答的，或者这条推送根本没被打开），
/// 继续拿这份选项解读一条新回复的风险大于"多问一次/原样敲进去"的代价。
const PENDING_OPTIONS_TTL: Duration = Duration::from_secs(300);

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
            outbound_map: Mutex::new(VecDeque::new()),
            ambiguous_pushes: Mutex::new(VecDeque::new()),
            used: Mutex::new(None),
            replied_since_use: Mutex::new(false),
            writer: Mutex::new(None),
            journal: Journal::new(),
            backend: Mutex::new(None),
            pending_options: Mutex::new(HashMap::new()),
        }
    }

    /// 只给测试用：直接摆一条 `pending_options` 记录，绕开真正的
    /// `compose_outbound`/模型往返——`deliver_to` 对"过期条目一律当没有"
    /// 这条规则的测试需要摆出一个"很久以前推的"记录，走真实的发送线程
    /// 等 `PENDING_OPTIONS_TTL` 秒不现实，直接注入一个已经过期的时间戳
    /// 才测得到这条边界。
    #[cfg(test)]
    fn set_pending_options_for_test(&self, id: u32, at: Instant, opts: Vec<String>) {
        recover(self.pending_options.lock()).insert(id, (at, opts));
    }

    /// 接进敲字的能力（Ruling 7）。生产环境传一个包着 `SessionManager` 的
    /// `Arc`（`SessionManager` 已经 `impl SessionWriter`，见上面那段），
    /// 测试传假的记录器。
    pub fn set_writer(&self, w: Arc<dyn SessionWriter>) {
        *recover(self.writer.lock()) = Some(w);
    }

    /// 接进 `options_prompt`/`map_answer`/`narrow` 要用的后端。`None` =
    /// 没配 `[llm]`——三个功能安静下线，见 `backend` 字段的文档。
    pub fn set_backend(&self, b: Option<Arc<dyn crate::llm::Backend>>) {
        *recover(self.backend.lock()) = b;
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

    /// 一条消息落地之后该做什么。**这是唯一会把一条入站消息交给
    /// `route()`/`deliver()` 的地方，而且结构上只有两条腿能走到那里**：
    /// `Accepted::Paired` 和 `Accepted::FromOwner`。`Accepted::Rejected`
    /// 对应的是一个空分支——不是"这个分支里判断了什么都不做"，是
    /// **这个分支根本没有语句能碰到 `route_and_deliver`**。安全评审的
    /// 要求是"结构上明显"，不是"跑起来碰巧对"：把 `route_and_deliver`
    /// 挪到这个 `match` 外面统一调用一次、只在外面另加一个
    /// `if !matches!(.., Rejected)` 之类的守卫，都不满足这条要求——那样
    /// 的写法只要有人删掉那一行守卫就会把陌生人的话敲进用户的终端，
    /// 而编译器不会提醒。现在这样，想犯这个错误必须先把
    /// `route_and_deliver(msg)` 这行代码亲手打进 `Rejected` 分支里，
    /// 变动本身在 diff 里无所遁形（`security_review_the_rejected_arm_
    /// never_calls_route_or_deliver` 是这条不变量的测试）。
    fn dispatch(&self, msg: &Incoming) {
        match self.accept(msg) {
            Accepted::Paired(chat_id) => {
                // 落盘在改内存状态槽之前——`PhoneStatus` 只是给界面看的
                // 缓存，密钥仓那份才是重启之后唯一还在的真相。顺序反过来
                // 的话，一次「状态槽已经显示配对成功，但落盘失败」的窗口
                // 会比这里更长。
                (self.persist_owner)(chat_id);
                let mut st = recover(self.phone.lock());
                st.state = PhoneState::Paired;
                // 这里没有真实姓名可用——`Incoming` 没带 Telegram 的
                // `message.from.username`（Task 2 没有解析它），能给的
                // 只有 chat id 本身。诚实地显示一个数字，好过编一个不
                // 存在的名字。
                st.owner = Some(chat_id.to_string());
                drop(st);
                // 配对完成的这条消息本身也可能带着内容（比如直接就是
                // 一句 `/ls` 或者要说给某个会话的话）——不能因为这条
                // 消息"顺便"完成了配对就把它的内容扔掉不处理。
                self.route_and_deliver(msg);
            }
            Accepted::FromOwner => self.route_and_deliver(msg),
            // **空分支。绝不能在这里加任何调用 `route`/`deliver` 的代码**
            // ——见上面的文档注释，这正是安全评审要求"结构上明显"的落地。
            Accepted::Rejected => {}
        }
    }

    /// `Paired`/`FromOwner` 共用的处理：先认 `/use`、`/ls` 这两条命令，
    /// 都不是的话才交给 `route()`/`deliver()`。**只在 `dispatch()` 的
    /// 两条已认证分支里被调用**——见那边的文档注释，这个方法本身不做
    /// 任何身份判断，把它挪到 `accept()` 之前调用就是重新打开安全漏洞。
    fn route_and_deliver(&self, msg: &Incoming) {
        let text = msg.text.trim();
        if let Some(rest) = strip_command(text, "/use") {
            self.handle_use(rest.trim());
            return;
        }
        if strip_command(text, "/ls").is_some() {
            self.handle_ls();
            return;
        }

        // **I1 的修复。** 长按回复的是一条"合并了好几件事"的推送：`route()`
        // 只认识单会话的 `outbound_map`，会把这种回复判成 `Gone`（"这条
        // 消息已经不认识了"）——但两个会话可能都还活着、都还在等，那句
        // 话是在撒谎。正确答案是 `Route::Ask`：不确定该说给哪个，问一句，
        // 见 `ambiguous_pushes` 字段文档。这段检查必须在构造 `RouteInput`
        // 之前做——`route()` 本身不认识这张表，也不该认识（它是纯函数，
        // 五条规则不该再长出第六条特例）。
        if let Some(reply_id) = msg.reply_to {
            if let Some(sessions) = self.ambiguous_reply_sessions(reply_id) {
                self.deliver(Route::Ask(sessions), &msg.text);
                *recover(self.replied_since_use.lock()) = true;
                return;
            }
        }

        let map: HashMap<MsgId, u32> = recover(self.outbound_map.lock()).iter().copied().collect();
        let waiting: Vec<u32> = recover(self.writer.lock())
            .clone()
            .map(|w| w.waiting())
            .unwrap_or_default();
        let used = *recover(self.used.lock());
        let replied_since_use = *recover(self.replied_since_use.lock());

        let input = RouteInput {
            reply_to: msg.reply_to,
            map: &map,
            used,
            replied_since_use,
            waiting: &waiting,
        };
        let route = route(&input);
        // 规则 4（好几个在等）猜一把该敲给哪个候选——**猜不准还是反问**，
        // 见 `narrow` 自己的文档：模型说不好、答案不在候选里，两种都原样
        // 走 `Route::Ask`，绝不因为"模型听起来有把握"就跳过这一问。没配
        // 后端（`backend` 是 `None`）时 `and_then` 短路，行为退回到今天：
        // 好几个候选就是反问，不会因为这个功能而变得更敢猜。
        let route = match route {
            Route::Ask(ids) => {
                // **不能写成 `recover(self.backend.lock()).clone().and_then(...)`
                // 一整条链子。** 那样 `.lock()` 产生的 `MutexGuard` 会活到
                // 整条语句结束——包括后面 `narrow()` 那次最长 8 秒的模型
                // 调用——跟 `reply()` 自己文档里"绝不该在等网络调用的时候
                // 攥着一把锁"是同一条规矩（这里锁着的是 `backend`，不是
                // `owner`，但代价一样：`PhoneSetToken`/`PhoneDisable` 这类
                // 需要瞬间生效的操作会被平白拖住）。先在自己的语句里把
                // `Arc` 克隆出来、让锁在这一行结束时就释放，再拿这个局部
                // 变量去问模型。
                let backend = recover(self.backend.lock()).clone();
                let guess = backend.and_then(|b| narrow(&ids, &msg.text, &b));
                match guess {
                    Some(id) => Route::To(id),
                    None => Route::Ask(ids),
                }
            }
            other => other,
        };
        self.deliver(route, &msg.text);

        // 规则 3 的另一半：**这条**消息如果是一次长按回复，从这一刻起
        // `/use` 的指定作废——放在 `route()`/`deliver()` 之后才翻这个
        // 标记，这样这条消息本身仍然吃到了翻转之前的 `replied_since_use`
        // 快照（回复动作本身走的是规则 1，压根不看这个标记，但下一条
        // 不带回复的消息必须已经看到 `/use` 失效）。
        if msg.reply_to.is_some() {
            *recover(self.replied_since_use.lock()) = true;
        }
    }

    /// `/use <n>` 的落地：`n` 解析不出来就老实说清楚格式，解析出来就
    /// 记下来、把 `replied_since_use` 清零——这是一次新的显式选择，
    /// 不该继承上一次选择留下的"已经回复过"状态。**不校验 `n` 是不是
    /// 真的存在**：`route()`/`deliver_to` 已经会在真敲的时候诚实报
    /// "这个会话已经不在了"，这里重复校验一遍只是多一处可能跟那边
    /// 对不上的逻辑。
    fn handle_use(&self, rest: &str) {
        match rest.parse::<u32>() {
            Ok(id) => {
                *recover(self.used.lock()) = Some(id);
                *recover(self.replied_since_use.lock()) = false;
                self.reply(&format!("好，接下来的话默认说给 {id} 号"));
            }
            Err(_) => {
                self.reply("没看懂，格式是 /use 加编号，比如 /use 3");
            }
        }
    }

    /// `/ls`：报一遍此刻在等用户说话的会话。**没有真实姓名就退回编号**
    /// ——跟 `ask_message`/`fallback_name` 同一条规矩，绝不编一个不存在
    /// 的名字。
    fn handle_ls(&self) {
        let writer = recover(self.writer.lock()).clone();
        let Some(writer) = writer else {
            self.reply("这句话现在发不出去，稍后再试一次");
            return;
        };
        let ids = writer.waiting();
        if ids.is_empty() {
            self.reply("现在没有会话在等你说话");
            return;
        }
        let list = ids
            .iter()
            .map(|&id| match writer.name_of(id) {
                Some(name) => format!("{id} 号「{name}」", id = id, name = name),
                None => fallback_name(id),
            })
            .collect::<Vec<_>>()
            .join("、");
        self.reply(&format!(
            "在等你说话的有：{list}。回复其中一条，或者发 /use 加编号指定一个"
        ));
    }

    /// 一条推送发出去之后，把渠道回的 `MsgId` 记到它关涉的会话上——
    /// 长按回复靠这张表找到家（`RouteInput::map`）。**只有只关涉一个
    /// 会话的推送才配拥有这条记录**，见 `outbound_map` 字段文档。
    fn record_push(&self, id: MsgId, session: u32) {
        let mut m = recover(self.outbound_map.lock());
        if m.len() >= MSG_MAP_CAP {
            m.pop_front();
        }
        m.push_back((id, session));
    }

    /// 一条合并了好几件事的推送发出去之后，把它关涉的会话集合记到
    /// `ambiguous_pushes`——见该字段的文档。跟 `record_push` 同一条
    /// 丢最旧的上限规则。
    fn record_ambiguous_push(&self, id: MsgId, sessions: Vec<u32>) {
        let mut m = recover(self.ambiguous_pushes.lock());
        if m.len() >= MSG_MAP_CAP {
            m.pop_front();
        }
        m.push_back((id, sessions));
    }

    /// 一条长按回复对应的 `MsgId`，如果它当初是一条合并推送，答它关涉的
    /// 会话集合；不是（或者已经不记得了）就是 `None`——调用方据此决定
    /// 是走 `Route::Ask` 还是照常问 `route()`。
    fn ambiguous_reply_sessions(&self, id: MsgId) -> Option<Vec<u32>> {
        recover(self.ambiguous_pushes.lock())
            .iter()
            .find(|(mid, _)| *mid == id)
            .map(|(_, sessions)| sessions.clone())
    }

    /// 消费队列这一半：把一条 `session.rs::tick()` 产的事件收进来。
    /// **满了丢最旧的一条**（`QUEUE_CAP`，Ruling 4）——不是拒收新的，
    /// 新的事件永远进得来，代价是队首那条最老的被挤掉。
    ///
    /// **不对外公开**（最终整分支 review 的修复 2）：模块外没有任何合法调用者
    /// ——唯一的生产入口是本模块内的 `spawn_event_consumer`。留着 `pub` 只是给
    /// 「绕开 `accept()` 直接建 `Bridge` 再往里塞」这条路径开了一扇不该开的门,
    /// 而 `dispatch` 里那个刻意留空的 `Rejected => {}` 存在的全部意义就是不让
    /// 这条路径存在——把可见性收紧到跟事实相符,让编译器帮忙守住这条边界。
    fn enqueue(&self, e: Event) {
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

    /// 发送线程那一半：把队列里此刻攒着的全部事件**取走**，按进队顺序。
    /// 空队列拿到空 `Vec`，不是错误——发送线程每一轮都会问一遍，大多数
    /// 时候什么都没有。
    fn drain_outbound(&self) -> Vec<Event> {
        recover(self.outbound.lock()).drain(..).collect()
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

    /// 忘掉主人，重新打开配对，**并且清空旧手机留下的全部路由状态**
    /// （最终整分支 review 的修复 4）。`PhoneUnpair` 唯一要做的事——不
    /// 重启线程、不碰 Telegram 的 offset，下一条到达的消息（不管是谁
    /// 发的）立刻重新触发 `accept()` 的 `None` 分支。密钥仓里持久化的
    /// 那份由调用方（`daemon.rs`）另外清掉，这里只管内存里这一份。
    ///
    /// **不能只清 `owner`。** 以前这里就是这么做的：`used`（`/use`
    /// 选中的会话）、`replied_since_use`、`outbound_map`、
    /// `ambiguous_pushes`、`pending_options` 全都原样留着。新配对的
    /// 手机因此会直接继承上一台手机的 `/use` 目标——它发的第一条大白话
    /// 消息按 `route()` 规则 2 直接敲进旧目标，而不是老实地问一句"回给
    /// 哪个"。这正是"手机丢了，配一台新的"这条路径，也是猜错代价最大
    /// 的场景：新主人这次发的可能是完全不相干的一句话，却被当成回复
    /// 敲进了别的会话。`pending_options` 也不能留：那是"上一次推送猜出
    /// 的选项"，跟旧手机的会话身份绑在一起，留给新手机同样是拿旧问题
    /// 解读新回复。`outbound`（还没发出去的推送队列）不在这份清单里——
    /// 那些通知不属于任何一台手机，应该原样送给新配对的那台。
    pub fn clear_owner(&self) {
        *recover(self.owner.lock()) = None;
        *recover(self.used.lock()) = None;
        *recover(self.replied_since_use.lock()) = false;
        recover(self.outbound_map.lock()).clear();
        recover(self.ambiguous_pushes.lock()).clear();
        recover(self.pending_options.lock()).clear();
    }

    /// `route()` 已经决定了该往哪儿去，这里是把决定真的落地。**`To` 敲，
    /// 另外三支什么都不敲**——回执不是锦上添花：用户在外面看不见终端，
    /// 没有回执他不知道这句话到底进去没有；而 `Ask`/`Gone`/`NeedUse` 存在
    /// 的全部意义就是"这次不该猜"，回一句人话、绝不动 PTY 才是它们唯一
    /// 正确的做法。**全部记 journal**——手机来的这条消息最终去了哪儿，
    /// 跟会话自己怎么没的一样，得留得下痕迹。
    ///
    /// **不对外公开**（最终整分支 review 的修复 2）：`route()`/`dispatch()`
    /// 已经把「先过 `accept()` 的安检」这件事焊死在唯一的调用路径上；把
    /// `deliver` 留成 `pub` 等于允许模块外的代码拿着一个真 writer 直接调
    /// `deliver(Route::To(id), text)`，跳过安检去敲用户的会话——`Rejected
    /// => {}` 那个刻意留空的分支是这道安检唯一的实现,可见性也要护着它。
    fn deliver(&self, route: Route, text: &str) -> Delivered {
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
        // **红线在这里落地。** `pending_options` 里有这个会话上一次推送
        // 猜出的选项，就把这条回复交给 `map_answer_index` 转成 agent 要的
        // 序号；没有（绝大多数消息——没在等选择、没配后端、已经用过一次、
        // 或者已经过期）就是 `None`，模型压根不会被调用，`text` 原样往下
        // 走。**取一次就丢**：`remove` 而不是 `get`，同一份选项不该被拿去
        // 解读这个会话之后的第二条回复，见 `pending_options` 字段的文档。
        //
        // **过期的条目视同没有**——这是安全评审要求补的结构性保证：
        // agent 完全可能在没有再产生一条推送事件的情况下就翻过了这个
        // 问题（用户直接在终端里回答了它，或者这一轮通知被 debounce 掉
        // 了），`compose_outbound` 那边"下一条推送提到这个会话才清掉"这
        // 条规则单独存在时补不到这个缝——`PENDING_OPTIONS_TTL` 在这里补上：
        // 超过窗口，不管选项列表本身是不是还"新鲜"，一律当没有，`text`
        // 原样敲进去。
        let opts = {
            let mut slot = recover(self.pending_options.lock());
            slot.remove(&id).and_then(|(at, opts)| {
                if at.elapsed() <= PENDING_OPTIONS_TTL {
                    Some(opts)
                } else {
                    None
                }
            })
        };
        let backend = recover(self.backend.lock()).clone();
        // `chosen` 只在模型真的选中了某一项时才是 `Some`——见
        // `map_answer_index` 的文档：跟 `to_type` 分开算，不靠"结果是不是
        // 一串数字"去猜，那样会把恰好也是数字的自由文本误判成"选中了"。
        let (to_type, chosen) = match (&opts, &backend) {
            (Some(opts), Some(b)) => match map_answer_index(text, opts, b) {
                Some(n) => (n.to_string(), Some(opts[n - 1].clone())),
                None => (text.to_string(), None),
            },
            _ => (text.to_string(), None),
        };
        match writer.type_into(id, &to_type) {
            Ok(()) => {
                let name = writer.name_of(id).unwrap_or_else(|| fallback_name(id));
                // **红线的第三半，回执这一侧。** 模型把这句话换成了序号，
                // 用户在手机上唯一能看到的就是这条回执——不说清楚换成了
                // 什么，"他说了一句话、agent 收到了另一句"这件事就从头到
                // 尾没有一处让他知道。映射发生了就把选中的原文说回去；
                // 没发生（绝大多数情况）还是原来那句平淡的回执。
                match &chosen {
                    Some(opt) => {
                        self.reply(&format!("已经按你说的选了「{opt}」，敲进了「{name}」"))
                    }
                    None => self.reply(&format!("已经敲进「{name}」")),
                }
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
        // **不能写成 `if let Some(to) = *recover(self.owner.lock()) { ... }`**
        // ——`if let` 的临时对象（这里是 `MutexGuard`）活到整个分支体结束，
        // 那样 `owner` 这把锁会一直被攥着，直到 `ch.send()`（网络调用，
        // 慢的时候能到好几秒）返回才放。这段时间里 `PhoneUnpair` 的
        // `clear_owner()` 和发送线程读 `owner` 都会被卡住——手机通道
        // 自己在等网络，不该连累"忘掉主人"这种应该瞬间完成的操作。
        // 显式把值拷出来、让锁在这一行结束就释放，再去发网络请求。
        let to = *recover(self.owner.lock());
        if let Some(to) = to {
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

    /// 出站这一半的主循环：定期把 `enqueue()` 攒下的事件整批取走、合并成
    /// 一条、发给主人，把渠道回的 `MsgId` 记进 `outbound_map`。**不要
    /// 直接调用它**——用模块级的 `spawn_sender()`，那边包了
    /// `catch_unwind`，理由同 `run()`。
    ///
    /// **绝不阻塞 `tick()`。** 这是这条线程存在的全部理由：
    /// `session.rs::tick()` 只管往 unbounded 的 `mpsc::Sender<Event>`
    /// 里 `send()`，那条 channel 由常驻的 `spawn_event_consumer` 转手
    /// `enqueue()` 进这里的 `outbound` 队列（有界、drop-oldest）；这个
    /// 方法只从 `outbound` 这一头读，从不回头去等 `tick()`，两条线程
    /// 之间没有反向依赖，`tick()` 那 200ms 一轮的循环感知不到这里发生
    /// 了什么，哪怕这里因为网络卡住半天。
    ///
    /// **跟轮询线程共用同一个 `self.stop`**——`stop()`/`replace()`/
    /// `stop_current()` 一次调用同时喊停两条线程，不必再给发送线程
    /// 另开一面旗子，这正是"停/换令牌不会留下孤儿线程"这条要求在这里
    /// 的落地：把它系在轮询线程已经验证过能生效的同一根绳子上，而不是
    /// 自己发明一套新的生死开关。
    ///
    /// `compose_outbound()` 可能因为问模型而多花最多 15 秒——这没关系，
    /// 也是"绝不阻塞 tick()"这条不变量本来就允许的：卡住的是这条独立
    /// 的发送线程本身，`tick()`/daemon 的其它请求处理都在别的线程上，
    /// 感知不到这 15 秒。代价只是"这一批消息会晚最多 15 秒送到手机上"，
    /// 换来的是选项列表——`session.rs::request_explanation` 允许自己的
    /// 后台线程等模型 30 秒是同一条道理。
    fn run_sender(&self) {
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            self.sleep_or_stop(SEND_INTERVAL);
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            let events = self.drain_outbound();
            if events.is_empty() {
                continue;
            }
            // **F2 同款修复，出站这一半。** `drain_outbound()` 到
            // `ch.send()` 之间没有任何耗时操作，但 `stop()` 完全可能在
            // 这道窄缝里被别的线程调用（`PhoneDisable`/`PhoneUnpair`/
            // 重新填令牌）——不补这一道检查的话，一条已经被"判了死刑"
            // 的发送线程仍然可能把这批事件发到 Telegram 上，跟入站那边
            // `run()` 里 `poll()` 返回之后要重新看一眼 `stop` 是同一个
            // 理由（见那边 F2 的文档）。这批已经取出来的事件**照旧不放
            // 回队列**——跟"没有主人时故意丢弃"同一条道理，停下来之后
            // 这批消息也不值得攒着找机会补发。
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            // 还没配对：这批事件没有地方可送。**故意丢弃，不攒着等以后
            // 补发**——同 `spawn_event_consumer` 那条道理，攒着的话用户
            // 一旦配对成功会被一堆早就过期的通知糊一脸。
            let Some(to) = *recover(self.owner.lock()) else {
                continue;
            };
            let text = self.compose_outbound(&events);
            if let Ok(id) = self.ch.send(to, &text) {
                match events.as_slice() {
                    // 只关涉一个会话：进单会话映射，`route()` 的规则 1
                    // 能直接用。
                    [only] => self.record_push(id, only.session),
                    // 合并了好几件事：进 `ambiguous_pushes`，长按回复
                    // 这条推送该问"回给哪个"，不该判成"不认识这条消息"
                    // ——见该字段的文档。去重是因为同一个会话理论上不该
                    // 在同一批里出现两次，但"防御性去重"比"假设上游永远
                    // 不会重复"更值得信。
                    many => {
                        let mut sessions: Vec<u32> = many.iter().map(|e| e.session).collect();
                        sessions.sort_unstable();
                        sessions.dedup();
                        self.record_ambiguous_push(id, sessions);
                    }
                }
            }
            // 发送失败：吞掉，不重试。同 `journal.rs` 的规矩——手机通道
            // 这条线程自己没发出去，不该连累任何会话；下一轮醒来时，
            // 真正要紧的新事件早就把这条盖过去了。
        }
    }

    /// Task 9 遗留的那一半：把 `merge()` 产的纯元数据消息，跟"这个 agent
    /// 是不是在等一个选择"这件事拼到一起。
    ///
    /// **顺序是唯一的保证。** 先把 `merge()` 的兜底文案算完（`base`）——
    /// 这一步不需要模型，之后不管发生什么，`base` 都已经是一条能发出去
    /// 的、诚实的消息。只有算完 `base` 之后才去问模型；模型慢、模型没答
    /// 好、根本没配 `[llm]`，最坏的结果都只是"少了选项列表"，从来不是
    /// "没有消息"或者"消息里有一半是空的"——这正是"每一处 LLM 用法都要有
    /// 退路"（CLAUDE.md）在这里的落地。
    ///
    /// **只在"这一批只有一件事、且是 Stopped"的时候才问**：合并推送
    /// （好几件事拼成一条）没有单独的屏幕可看，`Failed`/`Vanished` 也没有
    /// "在等一个选择"这回事——`event_verb` 已经把这两种说清楚了，追加
    /// 选项只会让文案自相矛盾。
    fn compose_outbound(&self, events: &[Event]) -> String {
        let base = merge(events, crate::i18n::Lang::Zh);

        let mut fresh_options: Option<(u32, Vec<String>)> = None;
        if let [only] = events {
            if only.kind == crate::channel::EventKind::Stopped {
                // **不能写成 `if let Some(backend) = recover(self.backend.lock())
                // .clone() { ... }`.** Edition 2021 一直把 `if let` 的
                // scrutinee 临时对象活到整个分支体结束，那样 `backend`
                // 这把锁就会一路攥到下面最长 15 秒的模型调用返回——跟
                // `reply()` 自己文档里"绝不该在等网络调用的时候攥着一把
                // 锁"（那边管的是 `owner`）是同一条规矩，这里换成了
                // `backend`：`PhoneSetToken`/`PhoneDisable` 这类需要瞬间
                // 生效的操作会被这 15 秒平白拖住。先用一条独立的 `let`
                // 语句把 `Arc` 克隆出来、让锁在这一行结束时就释放。
                let backend = recover(self.backend.lock()).clone();
                if let Some(backend) = backend {
                    let p = options_prompt(&only.screen);
                    // 15 秒硬超时：`compose_outbound` 已经在发送线程上，
                    // 不是 `tick()`，慢一点没关系（见 `run_sender` 的
                    // 文档），但也不能真的无限等下去。
                    if let Ok(raw) =
                        crate::llm::complete_with_timeout(backend, p, Duration::from_secs(15))
                    {
                        if let Some(opts) = parse_options(&raw) {
                            fresh_options = Some((only.session, opts));
                        }
                    }
                }
            }
        }

        // 这一批里没有会话拿到新鲜的选项列表，就把它们各自留着的旧选项
        // 一并清掉——一份问过的旧问题的选项，不该被拿去解读一条跟它毫不
        // 相干的新回复（比如 agent 已经从"选 A 还是 B"翻过去到下一轮，
        // 手机上这条新推送干脆没有问题，用户这次的话就该原样敲进去）。
        let mut slot = recover(self.pending_options.lock());
        for e in events {
            match &fresh_options {
                Some((session, opts)) if *session == e.session => {
                    slot.insert(e.session, (Instant::now(), opts.clone()));
                }
                _ => {
                    slot.remove(&e.session);
                }
            }
        }
        drop(slot);

        match fresh_options {
            // **最终整分支 review 的修复 3。** 光甩一串 `1. …\n2. …` 没有
            // 任何提示——收信人从没写过程序，也看不见终端，光看这份列表
            // 猜不出「回个数字」是个选项；而 `map_answer_index` 那整条
            // 路径存在的意义就是也认自己的大白话，可从没有一处广播过这
            // 件事。补一句人话交代两种都行，别让免打字这条路白修。
            Some((_, opts)) => format!(
                "{base}\n{}\n\n回数字就行，或者直接说说你自己的想法也可以",
                render_numbered(&opts)
            ),
            None => base,
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

/// 单条选项允许的最长字符数（按字符数、不按字节，理由跟 `session.rs::
/// NAME_MAX_CHARS` 一样）。**这是最后一道、也是覆盖面最广的一道过滤**：
/// `options_prompt` 要的是"几个字的大白话"，一条真选项不该长过这个数字。
/// 一行只要超过它，不管有没有命中下面任何一个具体的字符类信号，本身就
/// 说明这不是一条被概括过的选项，而是屏幕上一整行原始内容（一条命令、
/// 一段配置、一句长长的错误原文）被原样搬了过来——这道过滤专门兜住
/// "内容本身够短、但仍然是屏幕原文"这类漏网的情况，比如一个不含 `/`、
/// 反引号、`=`、`\`、`--` 的敏感短语。
const OPTION_MAX_CHARS: usize = 24;

/// 一条推送最多带几个选项。**这不是审美限制，是隐私限制**：多于这个数
/// 字，要么是模型没有按"从几个选项里选一个"理解这段屏幕（把一堆本来
/// 不相干的行都编了号），要么屏幕内容本身就撑爆了这个功能的适用范围
/// ——两种情况都不该把一长串东西糊给用户，也不该给"屏幕原文被大段搬运
/// 出去"留更大的窗口。
const OPTIONS_MAX_CANDIDATES: usize = 6;

/// 从模型的答案里洗出选项列表。**解析不出来就是没有选项，绝不猜**——
/// 这条规则比任何具体的格式细节都重要：模型答得含糊、跑题、或者干脆
/// 说「没有选项」，调用方都该拿到 `None`，退回只有元数据的兜底消息，
/// 而不是把模型那句话本身当成唯一的「选项」塞给用户。
///
/// **隐私过滤的核心保证**（跟 `options_prompt` 的 prompt 要求配对）：
/// prompt 只是"请求"模型别写路径/代码块/diff/原文，模型不听话是常态，
/// 这里才是真正兜底的关卡，丢弃的是不合规的那一行，不是整个答案。
/// 字符类信号只挡得住"明显长得像代码/命令"的行——`/`、反引号是路径/
/// 代码块最直白的信号；`=` 挡的是 `KEY=value` 这种赋值（环境变量、配置，
/// 常常直接带着真实的密钥/密码，这是本次评审揪出的具体泄露：一行形如
/// "把 .env 里的 API_KEY=sk-live-... 改掉" 的屏幕内容，既不含 `/` 也不含
/// 反引号，能原样混进选项列表、被存进 Telegram 的云端）；`\` 挡转义/
/// Windows 路径/shell 续行；`--` 挡命令行参数。`OPTION_MAX_CHARS` 是最后
/// 一道，兜住"够短、但仍是屏幕原文"这类漏网的情况——见它自己的文档。
/// `OPTIONS_MAX_CANDIDATES` 则限制这份列表本身能有多长。
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
        if candidate.contains('/')
            || candidate.contains('`')
            || candidate.contains('=')
            || candidate.contains('\\')
            || candidate.contains("--")
            || candidate.contains(':')
            || candidate.contains('：')
            || candidate.chars().count() > OPTION_MAX_CHARS
        {
            continue;
        }
        out.push(candidate.to_string());
        // 超过这个数字**整份答案不采信**，不是只留前几个——见
        // `OPTIONS_MAX_CANDIDATES` 自己的文档：模型给出这么多编号，本身
        // 就说明它没有把这段屏幕读成"从几个选项里选一个"，那么这份答案
        // 作为整体就是不可信的，截断只留前六条既治不好这个问题，还会把
        // 那六条本该被判定为"读错了"的屏幕原文继续送到手机上。
        if out.len() > OPTIONS_MAX_CANDIDATES {
            return None;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 把一组选项渲染成能直接拼进推送文案的编号列表，`1. xxx` 一行一个。
/// `map_answer_prompt` 把同样的选项喂给模型判断答案时，用的是同一种
/// 编号格式——用户看到的编号和他回复时该用的编号必须是同一套。
fn render_numbered(items: &[String]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {s}", i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 问模型「用户这句话对应哪个选项」的 prompt。跟 `options_prompt` 是同
/// 一条流水线的另一端：那边把屏幕变成编号选项，这里把用户的大白话变回
/// 编号——中间的选项文字（`opts`）已经是 `options_prompt`/`parse_options`
/// 洗过一遍的干净大白话，不含路径/代码块，这里不需要重复设防。
fn map_answer_prompt(user: &str, opts: &[String]) -> crate::llm::Prompt {
    crate::llm::Prompt {
        system: "命令行工具给用户列了几个选项，用户没有照抄编号，而是用自己\
                 的话回复了其中一个意思。判断他说的对应哪个编号，只回复那个\
                 编号本身的数字，不要别的字。如果看不出他说的是哪一个、或者\
                 他说的是完全不相干的另一件事，就只回复「不确定」。"
            .into(),
        user: format!("选项：\n{}\n\n用户的回复：\n{user}", render_numbered(opts)),
        max_tokens: 16,
    }
}

/// 把用户的话变成 agent 要的形式。**只转格式，不造内容。**
///
/// **这个 early return 就是整条红线。** agent 要的是自由文本
/// （`options` 是 `None`）：模型完全不介入，函数在看到 `options` 之前，
/// 唯一能做的事就是原样把 `user` 还回去——没有润色，没有摘要，没有第二
/// 种可能。一个用户在手机上打出来的句子和最终敲进 agent 的句子必须逐字
/// 相同，因为这是唯一一处他自己看不见结果的地方（CLAUDE.md）。
///
/// 只有 `options` 非空时才会问模型，而且答案必须是候选里的合法序号
/// （`1..=opts.len()`）——序号本身就是数字，模型答错、答不出、超时、
/// 答案越界，全部原样退回 `user`，不猜、不编。
///
/// **`#[cfg(test)]`：生产路径不再调用这个函数本身。** `deliver_to` 需要
/// 分清"模型真的选中了某一项"和"没选中、原样退回的话恰好也是数字"这两
/// 种情况（见 `map_answer_index` 的文档），所以它直接调用返回结构化
/// `Option<usize>` 的 `map_answer_index`，不经过这层返回 `String` 的
/// 包装。留着这个函数是因为它是 brief 原文点名的接口形状（`map_answer(
/// user, options, backend) -> String`），也是红线测试最直接、最贴合
/// brief 断言写法的落点——`free_text_is_typed_verbatim_and_never_reaches_
/// the_model` 等测试钉的就是这个签名。不给它生产调用方就不该给它生产
/// 可见性，免得它在往后的改动里跟真实路径（`map_answer_index`）悄悄
/// 长出两套不同的行为。
#[cfg(test)]
pub(crate) fn map_answer(
    user: &str,
    options: Option<&[String]>,
    b: &Arc<dyn crate::llm::Backend>,
) -> String {
    let Some(opts) = options else {
        return user.to_string();
    };
    if opts.is_empty() {
        return user.to_string();
    }
    match map_answer_index(user, opts, b) {
        Some(n) => n.to_string(),
        None => user.to_string(),
    }
}

/// `map_answer` 的核心，拆出来单独给 `deliver_to` 用。**返回的是候选里
/// 那个合法的、从 1 开始的序号本身，不是 `opts` 的下标**——跟
/// `render_numbered`/`map_answer_prompt` 给用户看的编号是同一套。
///
/// 单独拆出来是因为 `deliver_to` 需要分清"模型真的选中了某一项"和
/// "模型没选中、`user` 原样退回去、这句话恰好也是一串数字"这两种情况
/// ——两者用 `map_answer` 的返回值（一个 `String`）本身分不开：如果
/// `deliver_to` 拿 `map_answer` 的结果去 `parse::<usize>()`，一句碰巧是
/// 数字的自由文本会被误当成"选中了第 n 项"，把不该出现的选项原文塞进
/// 回执里。这里返回结构化的 `Option<usize>`，"选中了" 和 "没选中" 在
/// 类型上就分开了，调用方不用去猜一个字符串的来历。
fn map_answer_index(
    user: &str,
    opts: &[String],
    b: &Arc<dyn crate::llm::Backend>,
) -> Option<usize> {
    let p = map_answer_prompt(user, opts);
    let raw = crate::llm::complete_with_timeout(b.clone(), p, Duration::from_secs(8)).ok()?;
    let n: usize = raw.trim().parse().ok()?;
    (n >= 1 && n <= opts.len()).then_some(n)
}

/// 问模型「用户这句话说的是哪个候选会话」的 prompt。**只带编号，不带
/// 名字/项目/屏幕内容**——`narrow` 的调用方（`route_and_deliver`）此刻
/// 手里只有 `Route::Ask` 携带的一串会话号，这本来就是它唯一能问的问题；
/// 猜不准（模型答不出、答案不在候选里）不是这个 prompt 的失职，是
/// `narrow` 本该有的边界，见它自己的文档。
fn narrow_prompt(candidates: &[u32], text: &str) -> crate::llm::Prompt {
    let list = candidates
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join("、");
    crate::llm::Prompt {
        system: "用户手机上有好几个编程会话同时在等他说话，编号是给出的候选\
                 列表。他刚发来一句话，可能用「第一个」「第二个」「最后那个」\
                 这类说法指代其中一个候选编号。判断他说的是候选列表里的哪个\
                 编号，只回复那个编号本身的数字，不要别的字。如果看不出\
                 线索、拿不准，就只回复「不确定」。绝不能回复候选列表之外\
                 的编号。"
            .into(),
        user: format!("候选编号：{list}\n\n用户刚发来的话：\n{text}"),
        max_tokens: 16,
    }
}

/// 猜「这条含糊的回复该敲给哪个候选会话」。**永远不因为模型看起来有
/// 把握就跳过反问**——调用方（`route_and_deliver`）只在 `Route::Ask`
/// 那一支调用这里（Task 7 规则 4：好几个会话同时在等），猜不出来
/// （`None`）就照旧走 `Route::Ask`，问用户，不猜。
///
/// **答案必须在 `candidates` 里，一律不采信越界的号码**——这条检查独立
/// 于 prompt 里"绝不能回复候选列表之外的编号"那句话：prompt 只是请求，
/// 模型不听话是常态，真正的保证在这里。
pub fn narrow(candidates: &[u32], text: &str, b: &Arc<dyn crate::llm::Backend>) -> Option<u32> {
    if candidates.is_empty() {
        return None;
    }
    let p = narrow_prompt(candidates, text);
    let raw = crate::llm::complete_with_timeout(b.clone(), p, Duration::from_secs(8)).ok()?;
    let n: u32 = raw.trim().parse().ok()?;
    candidates.contains(&n).then_some(n)
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

/// 认一条命令，允许 Telegram 那种 `/cmd@botname` 的写法。`cmd` 只带前导
/// `/`，比如 `"/use"`。**不能靠 `str::starts_with` 判断**——`/use` 是
/// `/user`、`/useless` 的前缀，`starts_with("/use")` 会把这些完全不相干
/// 的命令误判成 `/use` 然后把它们剩下的字母当成参数吞掉（`/user` 会被
/// 解析成 `/use` 带参数 `"r"`）。这里要求命令后面紧跟着的要么是字符串
/// 结尾、要么是空白、要么是 `@botname` 这种群聊里@机器人时客户端自动
/// 加的后缀，三种之外一律判定"这不是这个命令"，返回 `None`。
///
/// 返回值是命令（以及 `@botname`，如果有）之后剩下的部分，原样不 trim。
fn strip_command<'a>(text: &'a str, cmd: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(cmd)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        return Some(rest);
    }
    let after_at = rest.strip_prefix('@')?;
    let end = after_at.find(char::is_whitespace).unwrap_or(after_at.len());
    Some(&after_at[end..])
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

    /// 转发给内部的 `accept()`——只给测试用，验证 `unpair()`/`replace()`
    /// 之后内部状态真的变了。生产路径唯一的调用者是本模块内的
    /// `dispatch()`，直接摸 `Bridge::accept`，不经过这层转发；对外暴露
    /// 这条捷径只会多一条绕开 `dispatch` 那道安检去问 `accept()` 的路
    /// （最终整分支 review 的修复 2），`#[cfg(test)]` 收紧到测试构建。
    #[cfg(test)]
    pub(crate) fn accept(&self, msg: &Incoming) -> Accepted {
        self.bridge.accept(msg)
    }
}

/// 把 bridge 起在后台线程上——**两条**线程：轮询线程（入站）和发送线程
/// （出站，`run_sender`），共用同一个 `stop` 标志位，一次 `stop()` 两条
/// 都会退出。**每条线程体都包在 `catch_unwind` 里**——一个手机通道死掉
/// 是遗憾，一个会话死掉是灾难，两者绝不能是同一件事。
///
/// `writer`/`journal_path` 都是 `None` 就是测试里那种"还没接线"的
/// `Bridge`——生产环境（`daemon.rs`）总是传 `Some`，两者都在起线程*之前*
/// 装好，不留一个"线程已经在跑但还没接上敲字能力"的窗口。
///
/// 不要直接调用它来更换一个正在跑的 bridge——那样旧的线程没人管，
/// 会跟新的一起活着（C3）。改令牌/配对状态一律走 `replace()`。
pub fn spawn(
    ch: Arc<dyn Channel>,
    phone: Arc<Mutex<PhoneStatus>>,
    owner: Option<i64>,
    persist_owner: Box<dyn Fn(i64) + Send + Sync>,
    writer: Option<Arc<dyn SessionWriter>>,
    journal_path: Option<PathBuf>,
    backend: Option<Arc<dyn crate::llm::Backend>>,
) -> BridgeHandle {
    let bridge = Arc::new(Bridge::new(ch, phone, owner, persist_owner));
    if let Some(w) = writer {
        bridge.set_writer(w);
    }
    if let Some(p) = journal_path {
        bridge.set_journal_path(p);
    }
    // 跟 writer/journal_path 同一条道理：起线程之前就该接好，不留
    // "轮询/发送线程已经在跑、但还接不到后端"的窗口。
    bridge.set_backend(backend);
    let worker = bridge.clone();
    std::thread::spawn(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| worker.run()));
    });
    let sender = bridge.clone();
    std::thread::spawn(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| sender.run_sender()));
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
#[allow(clippy::too_many_arguments)]
pub fn replace(
    slot: &Mutex<Option<BridgeHandle>>,
    ch: Arc<dyn Channel>,
    phone: Arc<Mutex<PhoneStatus>>,
    owner: Option<i64>,
    persist_owner: Box<dyn Fn(i64) + Send + Sync>,
    writer: Option<Arc<dyn SessionWriter>>,
    journal_path: Option<PathBuf>,
    backend: Option<Arc<dyn crate::llm::Backend>>,
) {
    let mut guard = recover(slot.lock());
    if let Some(old) = guard.take() {
        old.stop();
    }
    *guard = Some(spawn(
        ch,
        phone,
        owner,
        persist_owner,
        writer,
        journal_path,
        backend,
    ));
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
            screen: String::new(),
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
        // `Spy` 不是 `NeverCalled`——`dispatch()` 现在**也**会把消息交给
        // `route_and_deliver`（Task 8/9 的接线），没有 `/use`/在等的会话时
        // 会回一句「先发 /ls」，这一句要走真的 `Channel::send`，用
        // `NeverCalled` 会直接 panic。
        let b = Bridge::new(
            Arc::new(Spy::default()),
            phone.clone(),
            None,
            Box::new(|_| {}),
        );
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
        let b = Bridge::new(
            Arc::new(Spy::default()),
            phone.clone(),
            None,
            Box::new(|_| {}),
        );
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
        let b = Bridge::new(
            Arc::new(Spy::default()),
            phone.clone(),
            None,
            Box::new(|_| {}),
        );
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
            Arc::new(Spy::default()),
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
        /// `send()` 被调用过几次、传的都是什么——`route_and_deliver` 现在
        /// 接在 `dispatch()` 里，`run()` 测试里配对/路由产生的回执都会走
        /// 到这里，不能再让 `send()` panic（以前"这一路测试不需要 send"
        /// 的年代已经过去，Task 8/9 把它接上了）。
        sends: Mutex<Vec<(i64, String)>>,
        next_msg_id: Mutex<MsgId>,
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
                sends: Mutex::new(Vec::new()),
                next_msg_id: Mutex::new(0),
            }
        }

        fn queue_poll(&self, r: Result<Vec<Incoming>, ChannelError>) {
            self.poll_results.lock().unwrap().push_back(r);
        }

        fn queue_drain(&self, r: Result<usize, ChannelError>) {
            self.drain_results.lock().unwrap().push_back(r);
        }

        fn sends(&self) -> Vec<(i64, String)> {
            self.sends.lock().unwrap().clone()
        }
    }

    impl Channel for MockChannel {
        fn send(&self, to: i64, text: &str) -> Result<crate::channel::MsgId, ChannelError> {
            self.sends.lock().unwrap().push((to, text.to_string()));
            let mut n = self.next_msg_id.lock().unwrap();
            let id = *n;
            *n += 1;
            Ok(id)
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
            None,
            None,
            None,
        );
        assert_eq!(handle.accept(&msg(111, "老主人")), Accepted::FromOwner);

        handle.unpair();

        assert_eq!(
            handle.accept(&msg(222, "新手机先发")),
            Accepted::Paired(222),
            "unpair 之后配对要重新打开，不能永远焊死在老主人身上"
        );
    }

    /// **最终整分支 review 修复 4 的回归测试。** `PhoneUnpair`（落地为
    /// `clear_owner()`）以前只忘掉 `owner`，`used`（`/use` 选中的目标）
    /// 原样留着——新配对的手机会直接继承旧手机的 `/use` 目标，它发的第一
    /// 条不带回复的大白话会被悄悄敲进旧目标，而不是老实地按"唯一在等"
    /// 这条规则走。这正是"手机丢了，配一台新的"这条路径，猜错代价最大。
    #[test]
    fn unpairing_does_not_leave_the_old_use_target_for_the_next_phone() {
        // `for_test_with_writer` 自带一个已经配好对的老主人（999），正好
        // 省去再走一次 `accept()` 配对流程。
        let (b, spy) = Bridge::for_test_with_writer();
        spy.set_waiting(&[9]);
        // 老主人选中了一个跟"唯一在等"完全不同的会话。
        b.dispatch(&msg(999, "/use 3"));

        b.clear_owner(); // PhoneUnpair 的落地
        assert_eq!(b.accept(&msg(222, "新手机")), Accepted::Paired(222));

        // 新手机发一条不带回复的大白话——旧手机的 /use 3 不该还在生效，
        // 该退回"唯一在等"这条规则，敲进 9 号，而不是悄悄敲进 3 号。
        b.dispatch(&msg(222, "继续"));
        assert_eq!(
            spy.written(),
            vec![(9, "继续".to_string())],
            "新手机不该继承旧手机的 /use 目标：{:?}",
            spy.written()
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
            None,
            None,
            None,
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
            None,
            None,
            None,
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
        replace(
            &slot,
            ch.clone(),
            phone,
            Some(1),
            Box::new(|_| {}),
            None,
            None,
            None,
        );

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
        /// `waiting()` 该答什么——`route_and_deliver`/`/ls` 的测试用
        /// `set_waiting` 摆好这份候选集合，不碰真的 `SessionManager`。
        waiting: Mutex<Vec<u32>>,
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

        /// 摆好 `waiting()` 该答的候选集合。
        pub(super) fn set_waiting(&self, ids: &[u32]) {
            *self.waiting.lock().unwrap() = ids.to_vec();
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
        fn waiting(&self) -> Vec<u32> {
            self.waiting.lock().unwrap().clone()
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

    /// **红线的第三半：用户必须能看见自己的话被换掉了。** 模型把这条
    /// 回复映射成了一个选项，回执必须把选中的选项原文说回去——不说的话
    /// 就是"他说了一句话、agent 收到了另一句"这件事全程没有一处让他知道，
    /// 而这正是红线要挡住的伤害本身。
    #[test]
    fn when_the_model_maps_an_answer_the_receipt_says_what_was_chosen() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.name(7, "改登录页");
        b.set_pending_options_for_test(
            7,
            Instant::now(),
            vec!["先跑完".to_string(), "现在改".to_string()],
        );
        let backend: Arc<dyn crate::llm::Backend> = FakeBackend::answering("2");
        b.set_backend(Some(backend));

        let d = b.deliver(Route::To(7), "就第二个吧");

        assert_eq!(d, Delivered::Typed(7));
        assert_eq!(spy.written(), vec![(7, "2".to_string())]);
        assert!(
            spy.last_reply().contains("现在改"),
            "回执该说清楚选中的是哪个选项：{}",
            spy.last_reply()
        );
    }

    /// 没有映射发生（没有候选、答不出来、答案越界……）：回执照旧是那句
    /// 平淡的"已经敲进「name」"，不该无中生有地提一个选项。
    #[test]
    fn without_a_mapping_the_receipt_stays_plain() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.name(7, "改登录页");
        b.set_pending_options_for_test(7, Instant::now(), vec!["先跑完".to_string()]);
        let backend: Arc<dyn crate::llm::Backend> = FakeBackend::answering("我不确定");
        b.set_backend(Some(backend));

        let d = b.deliver(Route::To(7), "等等我再想想");

        assert_eq!(d, Delivered::Typed(7));
        assert_eq!(spy.written(), vec![(7, "等等我再想想".to_string())]);
        assert_eq!(spy.last_reply(), "已经敲进「改登录页」");
    }

    /// **安全评审要求补的结构性保证。** 一条很久以前推送时留下的选项
    /// 记录，不该被拿去解读现在这句完全不相干的回复——即使模型这时候
    /// 老老实实答了一个候选里的合法序号，过期这件事本身就足以让整条
    /// 记录作废，`text` 必须原样敲进去，回执也必须是平淡的那句。
    #[test]
    fn a_stale_pending_options_entry_is_refused_and_the_text_goes_in_verbatim() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.name(7, "改登录页");
        let long_ago = Instant::now() - (PENDING_OPTIONS_TTL + Duration::from_secs(1));
        b.set_pending_options_for_test(
            7,
            long_ago,
            vec!["先跑完".to_string(), "现在改".to_string()],
        );
        // 就算模型老老实实答了一个合法序号，过期这件事本身就该让这条
        // 记录作废——`SpyBackend` 顺便验证了这种情况下模型压根不会被调用。
        let spy_backend = SpyBackend::new();
        let backend: Arc<dyn crate::llm::Backend> = spy_backend.clone();
        b.set_backend(Some(backend));

        let d = b.deliver(Route::To(7), "随便说句话");

        assert_eq!(d, Delivered::Typed(7));
        assert_eq!(spy.written(), vec![(7, "随便说句话".to_string())]);
        assert_eq!(spy.last_reply(), "已经敲进「改登录页」");
        assert_eq!(spy_backend.calls(), 0, "过期的选项不该被拿去问模型");
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
            screen: String::new(),
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

    /// **真实泄露场景。** 屏幕上一行"把 .env 里的 API_KEY=sk-live-...
    /// 改掉"——既不含 `/`，也不含反引号，靠这两个字符类信号完全挡不住。
    /// 一个真实密钥/密码经常长这个形状（`KEY=value`），这条测试钉的就是
    /// 这次评审揪出的具体泄露：`=` 必须单独被拦。
    #[test]
    fn options_containing_an_env_style_assignment_are_discarded() {
        // **故意选一个很短的例子**（远在 `OPTION_MAX_CHARS` 之内、也不含
        // `/`、反引号、`\`、`--`）——真实泄露往往更长，但如果这里也用一条
        // 长候选，这条测试就分不清是 `=` 本身被拦了，还是长度上限碰巧
        // 也拦住了它，变异测试测不出"删掉 `=` 检查"这一刀。
        let got = parse_options("1. 把 A=1 改了\n2. 先跑完").unwrap();
        assert_eq!(got, vec!["先跑完".to_string()]);
        // 更贴近真实泄露的例子，独立确认：即便是一整条像密钥赋值的内容，
        // 同样要被拦住（这里长度和 `=` 两道过滤同时命中，不影响上面那条
        // 测试对 `=` 单独负责）。
        let got = parse_options("1. 把 API_KEY=sk-live-abc123 改掉\n2. 先跑完").unwrap();
        assert_eq!(got, vec!["先跑完".to_string()]);
    }

    /// 反斜杠——转义、Windows 路径、shell 续行——同样是"这其实是原始
    /// 内容"的信号，即便它既不含 `/` 也不含 `=`。
    #[test]
    fn options_containing_a_backslash_are_discarded() {
        let got = parse_options("1. 编辑 C:\\Users\\config\n2. 先跑完").unwrap();
        assert_eq!(got, vec!["先跑完".to_string()]);
    }

    /// 双横线——命令行参数——同样该被拦下。
    #[test]
    fn options_containing_double_dash_flags_are_discarded() {
        let got = parse_options("1. 加上 --force 重跑\n2. 先跑完").unwrap();
        assert_eq!(got, vec!["先跑完".to_string()]);
    }

    /// **变异测试专用：冒号分隔的 `KEY: value`。** 跟 `=` 是同一类信号，
    /// 但字符不同——`token: abc123`、`密码: hunter2` 这类短语既不带 `/`
    /// `` ` `` `=` `\` `--`，也不会撞上长度上限，是这次评审揪出的
    /// "跟已修的漏洞形状相同但字符不同"的最近一个漏网之鱼。
    #[test]
    fn options_containing_an_ascii_colon_are_discarded() {
        let got = parse_options("1. token: abc123\n2. 先跑完").unwrap();
        assert_eq!(got, vec!["先跑完".to_string()]);
    }

    /// 全角冒号——屏幕内容可能是中文，`密码：hunter2` 这种写法一样常见，
    /// ASCII 版本的检查挡不住它。
    #[test]
    fn options_containing_a_fullwidth_colon_are_discarded() {
        let got = parse_options("1. 密码：hunter2\n2. 先跑完").unwrap();
        assert_eq!(got, vec!["先跑完".to_string()]);
    }

    /// **变异测试专用：长度上限。** 一条选项如果长过 `OPTION_MAX_CHARS`，
    /// 不管有没有命中任何具体的字符类信号，本身就该被当成"屏幕原文"
    /// 拦下——这是兜住"内容本身够短测试想不到、但仍然是原文"这类情况的
    /// 最后一道，也是覆盖面最广的一道。
    #[test]
    fn an_option_longer_than_the_char_limit_is_discarded() {
        let long_but_clean = "先跑完".repeat(10); // 30 个字符，不含任何被
                                                  // 单独拦截的符号，纯靠
                                                  // 长度本身触发过滤。
        assert!(long_but_clean.chars().count() > OPTION_MAX_CHARS);
        let got = parse_options(&format!("1. {long_but_clean}\n2. 先跑完")).unwrap();
        assert_eq!(got, vec!["先跑完".to_string()]);
    }

    /// 候选数量上限：模型答得再离谱，`parse_options` 也不该无限往外掏。
    #[test]
    fn parse_options_caps_the_number_of_candidates() {
        // 超过 `OPTIONS_MAX_CANDIDATES` 不是"截断只留前六条"，而是整份
        // 答案作废——这么多编号本身就说明模型没把这段屏幕读成"从几个
        // 选项里选一个"，那六条留下来的候选照样是没被正确理解的屏幕
        // 原文，截断治不好这个问题。
        let raw = (1..=20)
            .map(|i| format!("{i}. 选项{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(parse_options(&raw), None);
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

    // ---- map_answer/narrow：红线本身 ----
    //
    // `map_answer`/`narrow` 都接 `&Arc<dyn Backend>`，不是 brief 草稿里
    // 写的裸 `&dyn Backend`：`llm::complete_with_timeout` 需要一个
    // `'static` 的 `Arc<dyn Backend>` 才能安全地 `move` 进后台线程去跑
    // 硬超时（跟 `session.rs::request_explanation` 是同一条约束，见
    // `llm/mod.rs::complete_with_timeout` 的签名和文档）——一个借用如果
    // 允许被这样搬进一个可能在超时后继续跑下去的线程，就不再是安全的
    // 借用了。测试双打包成 `Arc<dyn Backend>` 而不是裸值，行为跟 brief
    // 里的断言完全一致，只是构造方式适配了这条真实存在的生命周期约束。

    /// 被调用就记一笔——`free_text_is_typed_verbatim_and_never_reaches_the_model`
    /// 唯一要问的问题就是「模型有没有被碰过」。
    #[derive(Default)]
    struct SpyBackend(std::sync::atomic::AtomicUsize);

    impl SpyBackend {
        fn new() -> Arc<SpyBackend> {
            Arc::new(SpyBackend::default())
        }
        fn calls(&self) -> usize {
            self.0.load(Ordering::Relaxed)
        }
    }

    impl crate::llm::Backend for SpyBackend {
        fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok("2".to_string())
        }
    }

    /// 答一个固定答案，或者（`timing_out`）睡到比任何超时都长，专门测
    /// 「模型太慢就当没答」这条路。
    struct FakeBackend {
        answer: String,
        delay: Option<Duration>,
    }

    impl FakeBackend {
        fn answering(a: &str) -> Arc<dyn crate::llm::Backend> {
            Arc::new(FakeBackend {
                answer: a.to_string(),
                delay: None,
            })
        }
        fn timing_out() -> Arc<dyn crate::llm::Backend> {
            Arc::new(FakeBackend {
                answer: "太晚了".to_string(),
                delay: Some(Duration::from_secs(30)),
            })
        }
    }

    impl crate::llm::Backend for FakeBackend {
        fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
            if let Some(d) = self.delay {
                std::thread::sleep(d);
            }
            Ok(self.answer.clone())
        }
    }

    /// **红线：agent 要的是自由文本时模型完全不介入。** 模型一旦开始
    /// 润色，敲进 agent 的就不再是用户说的话，而他在手机上看不见这件事。
    #[test]
    fn free_text_is_typed_verbatim_and_never_reaches_the_model() {
        let spy = SpyBackend::new();
        let backend: Arc<dyn crate::llm::Backend> = spy.clone();
        let out = map_answer("那个啥 你先把测试跑一下然后再说", None, &backend);
        assert_eq!(out, "那个啥 你先把测试跑一下然后再说");
        assert_eq!(spy.calls(), 0, "自由文本却调了模型");
    }

    #[test]
    fn a_spoken_ordinal_becomes_the_option_the_agent_wants() {
        let b = FakeBackend::answering("2");
        let opts = vec!["先跑完".to_string(), "现在改".to_string()];
        assert_eq!(map_answer("就第二个吧", Some(&opts), &b), "2");
    }

    /// 映射不确定就原样发。这是红线的另一半。
    #[test]
    fn an_unmappable_answer_is_sent_as_typed() {
        let b = FakeBackend::answering("我不确定");
        let opts = vec!["先跑完".to_string(), "现在改".to_string()];
        assert_eq!(map_answer("等等我再想想", Some(&opts), &b), "等等我再想想");
    }

    #[test]
    fn a_model_timeout_sends_what_the_user_typed() {
        let b = FakeBackend::timing_out();
        let opts = vec!["先跑完".to_string()];
        assert_eq!(map_answer("就第一个", Some(&opts), &b), "就第一个");
    }

    /// **变异测试专用**：下界如果被误改成放行 0（`n <= opts.len()`，
    /// 丢了 `n >= 1`），这条必须失败——0 从来不是候选序号，`opts` 从 1
    /// 开始编号，跟 `render_numbered`/`map_answer_prompt` 给用户看的
    /// 编号是同一套。
    #[test]
    fn a_model_answer_of_zero_is_out_of_range_and_sent_as_typed() {
        let b = FakeBackend::answering("0");
        let opts = vec!["先跑完".to_string()];
        assert_eq!(map_answer("就这个", Some(&opts), &b), "就这个");
    }

    /// 空选项列表（`parse_options` 从不产出，但调用方不该被信任到这个
    /// 地步）：没有什么可选，模型也不该被问，原样发。
    #[test]
    fn empty_options_are_treated_like_free_text() {
        let spy = SpyBackend::new();
        let backend: Arc<dyn crate::llm::Backend> = spy.clone();
        let opts: Vec<String> = vec![];
        assert_eq!(map_answer("随便", Some(&opts), &backend), "随便");
        assert_eq!(spy.calls(), 0);
    }

    /// 猜路由不确定就还是反问。**永远不因为「模型有把握」跳过那一
    /// 问**——敲错 agent 的代价比多问一句大得多。
    #[test]
    fn an_uncertain_narrow_still_asks() {
        let b = FakeBackend::answering("说不好");
        assert_eq!(narrow(&[9, 10], "先跑完", &b), None);
    }

    /// 模型答了一个不在候选里的会话号，一律不采信。
    #[test]
    fn a_narrow_outside_the_candidates_is_refused() {
        let b = FakeBackend::answering("77");
        assert_eq!(narrow(&[9, 10], "先跑完", &b), None);
    }

    /// 正常情况：答案确实在候选里，采信。
    #[test]
    fn a_narrow_inside_the_candidates_is_accepted() {
        let b = FakeBackend::answering("10");
        assert_eq!(narrow(&[9, 10], "最后一个", &b), Some(10));
    }

    /// 猜路由一样受同一个超时保护——`narrow` 也不该有把 `tick()`/发送
    /// 线程之外的什么东西卡住的能力。
    #[test]
    fn a_narrow_timeout_refuses_to_guess() {
        let b = FakeBackend::timing_out();
        assert_eq!(narrow(&[9, 10], "先跑完", &b), None);
    }

    // ==== 整合任务：把 enqueue/route/deliver/`/use`/`/ls` 真的接起来 ====

    /// **安全测试，整个整合任务里最重要的一条。** 陌生人的消息必须一次
    /// 都碰不到 `route()`/`deliver()`——不是"结果看起来正确"这么松，是
    /// 拿一个只要 `type_into`/`send` 被调用就会留痕的 `Spy` 直接问：
    /// 陌生人发的这条消息，会话里一个字都没多，回执也一条没多。
    ///
    /// **这条测试钉的就是 `dispatch()` 的结构**：`route_and_deliver` 只在
    /// `Accepted::Paired`/`Accepted::FromOwner` 两条分支里被调用，
    /// `Accepted::Rejected` 是一个空分支——见 `dispatch()` 的文档注释。
    #[test]
    fn security_a_rejected_stranger_never_reaches_route_or_deliver() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.set_waiting(&[1, 2]); // 故意摆出"好几个在等"，排除"反正没得选所以看起来没敲"这种巧合
        spy.name(1, "对账");

        // 主人是 999（`for_test_with_writer` 的默认值）。陌生人 111 无论
        // 发什么——`/use`、`/ls`、看起来像回复——都不该被接受。
        b.dispatch(&msg(111, "/use 1"));
        b.dispatch(&msg(111, "/ls"));
        b.dispatch(&Incoming {
            text: "冒充回复".into(),
            reply_to: Some(0),
            chat_id: 111,
        });

        assert!(
            spy.written().is_empty(),
            "陌生人的消息一个字都不该被敲进任何会话"
        );
        assert!(
            spy.replies.lock().unwrap().is_empty(),
            "陌生人不该收到任何回执——回执是发给主人的，`reply()` 只会在\
             `route_and_deliver`/`handle_use`/`handle_ls` 里被调用，\
             这几个方法从未在 Rejected 分支上被调用过"
        );
    }

    /// 出站发送线程：`enqueue()` 进去的事件真的会被合并、发给主人，渠道
    /// 回的 `MsgId` 落进 `outbound_map`。**走真实的 `run_sender()`**，
    /// 不是直接调用私有的合并/映射函数——这条测试钉的是"这条线程真的在
    /// 跑、真的在读队列"这件事本身。
    #[test]
    fn a_queued_event_is_sent_and_its_msg_id_is_recorded() {
        let (b, spy) = Bridge::for_test_with_writer();
        let bridge = Arc::new(b);
        bridge.enqueue(named_event(
            7,
            crate::channel::EventKind::Stopped,
            "修登录白屏",
            "p",
        ));

        let worker = bridge.clone();
        let handle = std::thread::spawn(move || worker.run_sender());

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if !spy.replies.lock().unwrap().is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "发送线程该把队列里的事件真的发出去"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            spy.last_reply().contains("修登录白屏"),
            "发出去的是 merge() 组好的那条人话：{}",
            spy.last_reply()
        );

        bridge.stop();
        assert!(wait_for_join(handle, Duration::from_secs(2)));

        // 长按回复：`Spy::send` 固定回 `0`，`record_push` 该已经把
        // `0 -> 7` 记进 `outbound_map`——用一条回复 `0` 的入站消息验证，
        // 而不是直接窥探私有字段。
        bridge.dispatch(&Incoming {
            text: "先跑完".into(),
            reply_to: Some(0),
            chat_id: 999,
        });
        assert_eq!(
            spy.written().last(),
            Some(&(7, "先跑完".to_string())),
            "长按回复该落到 MsgId 关涉的那个会话上"
        );
    }

    /// **变异测试的钉子之一（`record_push` 不写）。** 如果有人把
    /// `run_sender` 里那句 `self.record_push(id, only.session)` 删掉，
    /// 这条测试会失败：长按回复找不到映射，`route()` 只能答 `Gone`，
    /// 什么都不敲。手改一遍、跑这条测试确认失败，是整合报告里记录的
    /// 变异之一。
    #[test]
    fn mutation_guard_a_reply_without_a_recorded_mapping_types_nothing() {
        let (b, spy) = Bridge::for_test_with_writer();
        // 没有任何 enqueue/send 发生过，`outbound_map` 是空的。
        b.dispatch(&Incoming {
            text: "先跑完".into(),
            reply_to: Some(0),
            chat_id: 999,
        });
        assert!(
            spy.written().is_empty(),
            "映射里没有这条 MsgId，就不该敲进任何地方"
        );
    }

    /// `/use <n>` 之后一条不带回复的消息该敲给 `n`；一旦用户长按回复过
    /// 一条推送，`/use` 的指定作废，后续消息回到"唯一在等的那个"这条
    /// 规则。**这是 `route()` 五条规则第一次真的被 `dispatch()` 调用到**
    /// ——`route()` 自己的单元测试只测纯函数，这条测试测的是
    /// `route_and_deliver` 有没有把真实状态（`/use`、`replied_since_use`、
    /// `waiting()`）拼对。
    #[test]
    fn use_then_reply_then_use_expires() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.set_waiting(&[9]);
        b.record_push(55, 9); // 假装此前有一条推送来自 9 号会话

        b.dispatch(&msg(999, "/use 3"));
        b.dispatch(&Incoming {
            text: "继续".into(),
            reply_to: None,
            chat_id: 999,
        });
        assert_eq!(
            spy.written(),
            vec![(3, "继续".to_string())],
            "/use 选中之后，不带回复的消息该敲给 3 号"
        );

        // 长按回复一条推送——注意力已经转走，`/use` 从这一刻起作废。
        b.dispatch(&Incoming {
            text: "先跑完".into(),
            reply_to: Some(55),
            chat_id: 999,
        });

        b.dispatch(&Incoming {
            text: "接着".into(),
            reply_to: None,
            chat_id: 999,
        });
        assert_eq!(
            spy.written().last(),
            Some(&(9, "接着".to_string())),
            "/use 已经作废，只有 9 号在等，该退回「唯一在等」这条规则"
        );
    }

    /// `/use` 解析不出编号：老实说清楚格式，不猜、不崩。
    #[test]
    fn use_with_a_bad_number_explains_the_format_instead_of_guessing() {
        let (b, spy) = Bridge::for_test_with_writer();
        b.dispatch(&msg(999, "/use 三号"));
        assert!(spy.written().is_empty(), "解析不出来就不该敲任何地方");
        assert!(
            spy.last_reply().contains("/use"),
            "该告诉用户正确格式：{}",
            spy.last_reply()
        );
    }

    /// **回归测试。** `/use` 是 `/user`、`/useless` 的前缀——`strip_prefix`
    /// 判断会把这些完全不相干的命令误当成 `/use` 带了个奇怪的参数，
    /// 更糟的是解析失败之后老实回一句「格式不对」，把原本该敲进会话的
    /// 一句话变成了一句提示。这两条消息都该被当成**普通文字**敲进当前
    /// 唯一在等的会话，而不是被 `/use` 的前缀匹配吞掉。
    #[test]
    fn use_prefix_does_not_swallow_unrelated_commands() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.set_waiting(&[9]);
        b.dispatch(&msg(999, "/user"));
        b.dispatch(&msg(999, "/useless"));
        assert_eq!(
            spy.written(),
            vec![(9, "/user".to_string()), (9, "/useless".to_string())],
            "/user、/useless 不是 /use，该原样敲给唯一在等的会话"
        );
    }

    /// Telegram 群聊里 `/use@botname` 是合法写法（客户端@机器人时自动
    /// 加的后缀）——旧的 `text.strip_prefix("/use")` 认不出这种形式，
    /// 会把整句话（包括 `@botname`）当成普通文字敲进会话。
    #[test]
    fn use_and_ls_recognize_the_at_botname_suffix() {
        let (b, spy) = Bridge::for_test_with_writer();
        // 摆两个候选，不是一个——只有一个在等的话，"唯一在等"这条规则
        // 会掩盖 /use 解析失败：即使 /use@botname 没被认出来（老代码把
        // `@my_dct_bot 3` 整段当成解析失败的参数），下一条消息照样会
        // 靠规则 4 落到那唯一的候选上，测试会在应该失败的时候误判成功。
        spy.set_waiting(&[3, 9]);
        b.dispatch(&msg(999, "/use@my_dct_bot 3"));
        b.dispatch(&msg(999, "继续"));
        assert_eq!(
            spy.written(),
            vec![(3, "继续".to_string())],
            "/use@botname 该被认成 /use，选中 3 号——两个候选里唯一能\
             解释这个结果的路径就是 /use 解析成功了"
        );

        b.dispatch(&msg(999, "/ls@my_dct_bot"));
        assert_eq!(
            spy.written().len(),
            1,
            "/ls@botname 该被认成 /ls，不该被敲进任何会话"
        );
    }

    /// **I1 的回归测试。** 一次合并了好几件事的推送，长按回复它必须问
    /// "回给哪个"（`Route::Ask`），不能落到 `Route::Gone`（"这条消息
    /// 已经不认识了"）——两个会话明明都还活着、都还在等，`Gone` 那句
    /// 话是在撒谎，而且偏偏撒在最需要分清楚的场合。走真实的
    /// `run_sender()`，不直接摸私有字段。
    #[test]
    fn replying_to_a_merged_push_asks_which_one_instead_of_lying_gone() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.name(1, "对账");
        spy.name(2, "修登录白屏");
        let bridge = Arc::new(b);
        bridge.enqueue(named_event(
            1,
            crate::channel::EventKind::Stopped,
            "对账",
            "fin",
        ));
        bridge.enqueue(named_event(
            2,
            crate::channel::EventKind::Stopped,
            "修登录白屏",
            "web",
        ));

        let worker = bridge.clone();
        let handle = std::thread::spawn(move || worker.run_sender());
        let deadline = Instant::now() + Duration::from_secs(3);
        while spy.replies.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "该把合并的两件事发出去");
            std::thread::sleep(Duration::from_millis(10));
        }
        bridge.stop();
        assert!(wait_for_join(handle, Duration::from_secs(2)));

        bridge.dispatch(&Incoming {
            text: "先跑完".into(),
            reply_to: Some(0), // Spy::send 固定回 0
            chat_id: 999,
        });

        assert!(spy.written().is_empty(), "两个候选，不该猜着敲进任何一个");
        let reply = spy.last_reply();
        assert!(
            !reply.contains("已经不在了"),
            "两个会话都还活着，不该说成 Gone：{reply}"
        );
        assert!(
            reply.contains("对账") && reply.contains("修登录白屏"),
            "该报出两个候选的名字：{reply}"
        );
    }

    /// **C1 的回归测试（真实 `SessionManager`）。** `SessionWriter::
    /// type_into` 现在必须真的按回车（`send_input(id, "")`），不能只把
    /// 文字写进输入框——用 `cat` 起一个真会话，敲一句话之后确认
    /// `SessionState` 真的推进到了 `Working`（`session.rs::send_input`
    /// 空字符串分支才会改这个状态），而不是仅仅看屏幕上有没有字。
    #[test]
    fn session_manager_type_into_submits_not_just_types() {
        let mgr = crate::session::SessionManager::new();
        mgr.register_profile(crate::profile::Profile {
            name: "bridge-c1-fake".into(),
            command: vec![crate::sys::testing::tool("cat")],
            is_agent: false,
            idle_pattern: None,
            busy_pattern: None,
            error_pattern: None,
            env: Default::default(),
            secret: None,
            install: None,
            headless: None,
            api: None,
            label: Default::default(),
            note: Default::default(),
            resume_args: Default::default(),
            pairable: false,
            backend_only: false,
        });
        let dir = tempfile::tempdir().unwrap();
        let id = mgr.create(dir.path(), "bridge-c1-fake", None, &[]).unwrap();

        let writer: &dyn SessionWriter = &mgr;
        writer.type_into(id, "你好").unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let state = mgr.list().into_iter().find(|s| s.id == id).map(|s| s.state);
            if state == Some(crate::session::SessionState::Working) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "type_into 必须真的按回车，状态该推进到 Working，实际 {state:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// `/ls`：报一遍此刻在等的会话，没名字的退回编号。
    #[test]
    fn ls_lists_the_waiting_sessions_by_name() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.set_waiting(&[3, 9]);
        spy.name(3, "对账");
        b.dispatch(&msg(999, "/ls"));
        let reply = spy.last_reply();
        assert!(reply.contains("对账"), "{reply}");
        assert!(reply.contains("9 号"), "没名字的该退回编号：{reply}");
        assert!(spy.written().is_empty(), "/ls 不该敲任何地方");
    }

    /// 没有会话在等时 `/ls` 也要老实说清楚，不能什么都不回。
    #[test]
    fn ls_with_nothing_waiting_says_so() {
        let (b, spy) = Bridge::for_test_with_writer();
        b.dispatch(&msg(999, "/ls"));
        assert!(!spy.last_reply().is_empty());
        assert!(spy.written().is_empty());
    }

    /// **no-orphan 测试。** `stop()` 之后轮询线程和发送线程都必须真的
    /// 退出——用 `MockChannel` 起一个真的 `spawn()`（两条线程都在跑），
    /// 往队列里塞一个事件让发送线程有活干，确认它真的发过一次；`stop()`
    /// 之后再确认两边的调用计数都不再增长。
    #[test]
    fn stop_leaves_neither_the_poller_nor_the_sender_still_running() {
        let phone = blank_status();
        let ch = Arc::new(MockChannel::new(Ok("bot".to_string())));
        // owner 已知：跳过 drain_backlog，轮询线程立刻进入"每次都成功但
        // 什么都没有"的正常轮询，最能暴露"stop 没生效就会一直转下去"。
        let handle = spawn(
            ch.clone(),
            phone,
            Some(1),
            Box::new(|_| {}),
            None,
            None,
            None,
        );
        handle.bridge.enqueue(ev(1));

        let deadline = Instant::now() + Duration::from_secs(2);
        while *ch.poll_calls.lock().unwrap() == 0 || ch.sends().is_empty() {
            assert!(
                Instant::now() < deadline,
                "两条线程都该真的跑起来：poll_calls={} sends={}",
                *ch.poll_calls.lock().unwrap(),
                ch.sends().len()
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        handle.stop();
        // 给两条线程一点时间真的看到 stop 标志位并退出。
        std::thread::sleep(Duration::from_millis(100));
        let poll_count = *ch.poll_calls.lock().unwrap();
        let send_count = ch.sends().len();

        // **只看"计数不再增长"还不够**——发送线程本来就要睡满
        // `SEND_INTERVAL` 才会去看一眼队列，队列里如果什么都没有，
        // 一个"stop 没生效、还在傻循环"的线程和一个"已经真的退出"的
        // 线程在没有新事件时看起来一模一样。**往队列里塞一条新事件**，
        // 只有真的退出的线程才不会去碰它。
        handle.bridge.enqueue(ev(2));
        std::thread::sleep(SEND_INTERVAL * 3);

        assert_eq!(
            *ch.poll_calls.lock().unwrap(),
            poll_count,
            "stop() 之后轮询线程不该还在跑"
        );
        assert_eq!(
            ch.sends().len(),
            send_count,
            "stop() 之后发送线程不该还在跑——两条线程共用同一个 stop 标志位，\
             stop() 之后新塞进队列的事件也不该被发出去"
        );
    }

    // ==== Task 10 的接线：`options_prompt`/`parse_options`/`map_answer`/
    // `narrow` 真的被 `compose_outbound`/`deliver_to`/`route_and_deliver`
    // 用起来，不再是"写好了但没人调用"的死代码。====

    /// 按 prompt 的内容分流答案——同一个假后端要同时扮演"猜屏幕上是不是
    /// 在等选择"（`options_prompt`）和"这句话对应哪个序号"
    /// （`map_answer_prompt`/`narrow_prompt`）两个角色，靠 system 提示词
    /// 里的特征字符串区分是哪一次调用。
    struct ScriptedBackend(fn(&crate::llm::Prompt) -> String);
    impl crate::llm::Backend for ScriptedBackend {
        fn complete(&self, p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
            Ok((self.0)(p))
        }
    }

    /// **Task 9 遗留的那一半，真的接上了。** 一条 Stopped 事件带着"是
    /// 继续跑完还是现在就改"这样的屏幕内容，配了后端之后推送该带上模型
    /// 猜出的编号选项；用户接下来一句大白话回复（唯一在等，不用长按
    /// 回复），`map_answer` 该把它变成 agent 要的序号，而不是把这句话
    /// 原样敲进去。
    #[test]
    fn options_from_the_push_are_used_to_map_the_next_reply() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.set_waiting(&[9]);
        let backend: Arc<dyn crate::llm::Backend> = Arc::new(ScriptedBackend(|p| {
            if p.system.contains("从几个选项里选一个") {
                "1. 先跑完\n2. 现在改".to_string()
            } else {
                "2".to_string()
            }
        }));
        b.set_backend(Some(backend));
        let bridge = Arc::new(b);
        bridge.enqueue(Event {
            session: 9,
            kind: crate::channel::EventKind::Stopped,
            name: "改登录页".to_string(),
            project: "web".to_string(),
            screen: "…… 是继续跑完，还是现在就改？ ……".to_string(),
        });

        let worker = bridge.clone();
        let handle = std::thread::spawn(move || worker.run_sender());

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if !spy.replies.lock().unwrap().is_empty() {
                break;
            }
            assert!(Instant::now() < deadline, "发送线程该把带选项的推送发出去");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            spy.last_reply().contains("1. 先跑完") && spy.last_reply().contains("2. 现在改"),
            "推送该带上模型猜出的编号选项：{}",
            spy.last_reply()
        );
        // 修复 3：光有编号列表不够——收信人得知道自己也能不照抄编号回复。
        assert!(
            spy.last_reply().contains("回数字") && spy.last_reply().contains("自己的想法"),
            "选项列表旁边该有一句提示：数字或大白话都能回：{}",
            spy.last_reply()
        );

        bridge.dispatch(&msg(999, "就第二个吧"));
        assert_eq!(
            spy.written(),
            vec![(9, "2".to_string())],
            "该敲进 agent 的是序号 2，不是用户原话"
        );

        bridge.stop();
        assert!(wait_for_join(handle, Duration::from_secs(2)));
    }

    /// 没配后端（`for_test_with_writer` 默认就是这样）：推送只有元数据，
    /// 没有选项列表——这是"每一处 LLM 用法都要有退路"在推送这一侧的
    /// 落地，不该因为这个功能而让没写 `[llm]` 的人看到任何变化。
    #[test]
    fn without_a_backend_the_push_stays_metadata_only() {
        let (b, spy) = Bridge::for_test_with_writer();
        let bridge = Arc::new(b);
        bridge.enqueue(Event {
            session: 9,
            kind: crate::channel::EventKind::Stopped,
            name: "改登录页".to_string(),
            project: "web".to_string(),
            screen: "…… 是继续跑完，还是现在就改？ ……".to_string(),
        });

        let worker = bridge.clone();
        let handle = std::thread::spawn(move || worker.run_sender());

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if !spy.replies.lock().unwrap().is_empty() {
                break;
            }
            assert!(Instant::now() < deadline, "发送线程该照常把推送发出去");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !spy.last_reply().contains("1. "),
            "没配后端就不该出现编号选项：{}",
            spy.last_reply()
        );

        bridge.stop();
        assert!(wait_for_join(handle, Duration::from_secs(2)));
    }

    /// `narrow` 真的接进了 `route_and_deliver`：好几个会话在等、又没有
    /// 用回复/`＄use` 指明是哪个，模型给出一个在候选里的确定答案时，
    /// 不该再反问，直接敲给猜出来的那个。
    #[test]
    fn a_confident_narrow_guess_is_used_instead_of_asking() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.set_waiting(&[9, 10]);
        spy.name(10, "对账");
        let backend: Arc<dyn crate::llm::Backend> = Arc::new(ScriptedBackend(|_| "10".into()));
        b.set_backend(Some(backend));

        b.dispatch(&msg(999, "跟最后一个说继续"));

        assert_eq!(
            spy.written(),
            vec![(10, "跟最后一个说继续".to_string())],
            "确定的猜测该直接敲给猜出来的会话"
        );
        assert!(
            spy.replies
                .lock()
                .unwrap()
                .iter()
                .all(|r| !r.contains("不确定该说给哪个")),
            "猜准了就不该再反问"
        );
    }

    /// **红线的另一半：猜路由不确定就还是反问。** 模型说不上来的时候，
    /// `route_and_deliver` 不该把 `Route::Ask` 悄悄换成一次乱猜。
    #[test]
    fn an_uncertain_narrow_guess_still_asks_via_dispatch() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.set_waiting(&[9, 10]);
        let backend: Arc<dyn crate::llm::Backend> = Arc::new(ScriptedBackend(|_| "说不好".into()));
        b.set_backend(Some(backend));

        b.dispatch(&msg(999, "先跑完"));

        assert!(spy.written().is_empty(), "拿不准就不该敲进任何会话");
        assert!(
            spy.last_reply().contains("不确定该说给哪个"),
            "拿不准就该照旧反问：{}",
            spy.last_reply()
        );
    }

    /// 没配后端：好几个会话在等，行为必须跟今天完全一样——反问，不猜。
    /// 这是这整个功能"退路"的最后一道验证：`Config::llm` 是 `None` 时
    /// `dct` 该表现得像这个功能从未存在过。
    #[test]
    fn without_a_backend_several_waiting_still_just_asks() {
        let (b, spy) = Bridge::for_test_with_writer();
        spy.set_waiting(&[9, 10]);

        b.dispatch(&msg(999, "先跑完"));

        assert!(spy.written().is_empty());
        assert!(spy.last_reply().contains("不确定该说给哪个"));
    }
}
