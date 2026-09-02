use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::git::FileStat;
use crate::profile::ProfileStatus;
use crate::pty::ScreenSpan;
use crate::session::{ScrollBy, ScrollState, SessionInfo, SessionState};

/// 界面和守护进程之间的线上契约版本。**改了协议就要加一。**
///
/// 这两个东西是分开升级的：守护进程一活就是好几天（它活得久正是这个产品
/// 存在的理由），用户装了新版本 dct 之后，跟他说话的还是几天前那个进程。
/// 协议一改，新界面发的请求旧守护进程就解不出来——2026-08-05 的现场是
/// `Profiles` 加了 `lang` 字段之后，按 n 只弹一句「拿不到 agent 列表」，
/// 没有任何线索指向真正的原因。
///
/// `the_request_shape_is_pinned_to_the_protocol_version` 和
/// `the_session_info_shape_is_pinned_too` 会在形状变了而这个数字没变时变红。
///
/// 2 = `SessionInfo` 多了 `is_agent`：底栏要按「这是不是 agent 会话」决定
/// 写不写 `u 回滚` / `d 改动`，旧守护进程回的 JSON 里没有这个字段，新界面
/// 解不出来。
///
/// 3 = 多了 `Kill` / `Prune` 两条请求。这两条旧守护进程根本不认，发过去
/// 只会得到一句解析失败。
///
/// 4 = 多了 `Explanation` 请求 / `Response::Explanation`——问一个 `Failed`
/// 会话「出了什么事」。旧守护进程不认识这条请求，界面发过去只会得到一句
/// 解析失败；老实说这条不至于让界面整个用不了（不问就是了），但协议形状
/// 变了就要加一，理由同上面两条。
///
/// 5 = 多了 `Request::Scroll` / `Request::Mouse` / `Response::Scrolled`，
/// `Response::Screen` 加了 `scroll`（带 `#[serde(default)]`，旧 JSON 照样能解）。
///
/// 6 = `LastProfile` 从没有字段的单例变体换成了带 `dir` 的——记忆是按项目
/// 分的，一个全局值会让你在 A 项目按 `n` 开出 B 项目上次用的那个 agent。
/// 同时多了 `Request::PinProject` / `Request::UnpinProject`，
/// `Response::Projects` 从 `Vec<String>` 换成了带 `recent` / `pinned`
/// 两个具名字段的结构体。
///
/// 6（事后追加，没有加一）= `SessionInfo` 又多了 `tag`（会话的稳定名字，
/// 空串表示还没起出来）。没有跟着把版本号加一，是因为这个字段带
/// `#[serde(default)]` 且纯只读，没有新增或改动任何 `Request` 变体——
/// 旧守护进程不需要「懂」它，只是答复里多了一段旧进程从不读的文本。
/// 具体的允许条件和「这不能当先例」的警告见
/// `the_session_info_shape_is_pinned_too` 测试上的注释。
///
/// 7 = 手机通知。多了 `Request::PhoneStatus` / `PhoneSetToken` / `PhoneUnpair`
/// / `PhoneDisable` 四条，`Response::Phone(PhoneStatus)`。旧守护进程完全不
/// 认识这几条请求，界面发过去只会得到一句解析失败——跟 `Kill`/`Prune` 那次
/// 加一是同一个理由。
///
/// 7（事后追加，没有加一）= `Response::Screen` 多了 `cursor_hidden`（agent
/// 自己把光标关掉了没有）。走的是 `SessionInfo.tag` 那次例外的同一条口子，
/// 两个条件同样都满足：字段带 `#[serde(default)]`（旧守护进程不发它，新界面
/// 补成 `false`，正是「照今天这样一直画着光标」的老行为），且没有新增或
/// 改动任何 `Request` 变体——旧守护进程完全不需要「懂」这个字段。允许条件
/// 和「这不能当先例」的警告见 `the_session_info_shape_is_pinned_too` 上的注释。
/// 8 = 局域网手机端。多了 `Request::WebStatus` / `WebEnable` / `WebDisable`
/// 三条，`Response::Web(WebInfo)`。**加一，没有例外可讲**：新增 `Request`
/// 变体那条规矩没得商量——旧守护进程收到 `WebEnable` 只会回一句解析失败，
/// 而用户看到的是「按了开关什么都没发生」。同 `Kill`/`Prune`、同手机通知那次。
///
/// 9 = 手机网页的文案改走协议。多了 `Request::WebStrings { lang }` 和
/// `Response::Strings`。以前那张表是 `web::routes` 直接从 i18n 里取了就发的，
/// **绕过了 dispatch**——局域网模式下没问题，但经中转看家里电脑的时候，
/// 中转手上只有一个不透明的信封，它既不认识 i18n 也不该认识（spec 决定一）。
/// 文案要么走协议，要么在中转上再抄一份 i18n 表；后者会让「决定用户看到
/// 什么字的地方只有一个」这条当场作废，所以只能走协议。
///
/// 10 = 虚拟键行按下去的那一下也改走协议：多了 `Request::Key { id, name }`。
/// 跟版本 9 是同一件事的第二半——键名翻成字节原来在 `web::routes` 里做
/// （`web::keys::bytes_for`），那同样是中转做不了的翻译。**凡是路由层
/// 「顺手算一下」的东西，经中转那一期都要还这笔债**，这是最后一处。
///
/// 11 = 配对。多了 `Request::PairStart` / `PairPoll` / `PairCancel`，
/// `Response::PairStarted` / `PairTick`。旧守护进程完全不认识这三条请求，
/// 界面发过去只会得到一句解析失败——跟 `Kill`/`Prune` 那次加一是同一个理由。
///
/// 12 = 配对屏上那个 `[llm]` 勾选框真的能按了。`PairPoll` 多了一个
/// `opt_in_llm` 字段：学生是在 `PairStart` 已经发出去之后才看见那行文案的，
/// 改主意得有一条路送到 daemon，而每 500ms 就要发一次的 `PairPoll` 正是
/// 那条路（理由见 `ui::pair_view::pair_poll_request`）。旧守护进程解不出
/// 多了一个字段的 `PairPoll`——形状变了就得加一，这条规矩没有例外。
pub const PROTOCOL_VERSION: u32 = 12;

/// 对面那个守护进程能不能用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonStatus {
    /// 同一份协议，正常用。
    Same,
    /// 号对不上，或者老到连 `Hello` 都不认识。
    Stale,
}

/// `None` = 连 `Hello` 都答不上来，那是比握手本身还老的守护进程。
pub fn daemon_status(protocol: Option<u32>) -> DaemonStatus {
    match protocol {
        Some(v) if v == PROTOCOL_VERSION => DaemonStatus::Same,
        _ => DaemonStatus::Stale,
    }
}

/// 需要密钥时，UI 画输入界面要用的东西。
///
/// 只带**已经取好语言**的字符串，不把 `LocalizedText` 送过线：组句发生在哪
/// 一侧必须一致——如果协议里同时存在「原始多语言表」和「已选定语言的字符串」，
/// 迟早会有一条路径读错表，界面上冒出一句英文夹在中文里。daemon 端已经知道
/// 用户语言（目前只有 `Lang::Zh`），选定的动作只在那一侧发生一次。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretPrompt {
    pub hint: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPrompt {
    pub command: Vec<String>,
    pub note: String,
}

/// 手机通知的状态。**只有四种，每一种都要给用户一条能做的下一步**——
/// `Paired` 除外，那是终点，不需要下一步（见 `ui::phone::next_step`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhoneState {
    /// 还没填令牌
    Off,
    /// 填了、验过了，在等用户给 bot 发第一条消息
    WaitingForPairing,
    Paired,
    /// 连不上。**装的是人话**，不是原始错误文本——守护进程是唯一决定
    /// 用户看到什么文字的地方（本文件顶上那条已有的约定）。
    ///
    /// 这个字段的内容**从不**直接显示给用户（见 `ui::phone::status_line`
    /// 和 `ui::phone::next_step`）：万一某处写这个值的代码手滑塞进了原始
    /// 错误甚至令牌本身，界面上显示的仍然是一句固定的人话，不是这里装的
    /// 原文——`the_token_never_appears_in_any_status_text` 钉的就是这一条。
    Broken(String),
}

/// 局域网手机端的状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebInfo {
    /// 正在监听吗。
    pub on: bool,
    /// 手机上要打开的完整地址，**含令牌**（放在 fragment 里）。
    ///
    /// **只走本机 socket**（见 `Request::WebStatus`）。关着的时候是 `None`——
    /// 不是空串：空串会被界面当成「有地址但没算出来」，而那是另一回事，
    /// 见 `address_unknown`。
    pub url: Option<String>,
    /// 口子开着，但算不出局域网地址（没连网、只有回环）。界面那时候要说
    /// 人话——手机跟这台电脑连同一个 WiFi、然后访问这个端口——而不是
    /// 摆一个连不上的地址装作一切正常。
    pub address_unknown: bool,
}

/// 手机通知这一整套的状态：连没连、是谁的手机、bot 叫什么名字。
///
/// **不带 chat id / token**——那两个是密钥级别的东西，`chat_id` 归
/// `bridge.rs`（Ruling 8：渠道层和 bridge 都不该把它泄到这一层），
/// `token` 只活在 `SecretStore` 里（见 `secrets::PHONE_TOKEN_KEY`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneStatus {
    pub state: PhoneState,
    /// bot 用户名，`getMe` 拿的。等配对那句话要用它——不带名字的
    /// 「去给它发条消息」是一句没法执行的话。
    pub bot: Option<String>,
    /// 配上的主人，显示用（比如「已连上 · lei」）。
    pub owner: Option<String>,
}

/// 菜单上一行的完整信息：UI 拿到它就能画出「名字 + 说明 + 能不能用 +
/// 需要的话怎么补」，不用再回头去问 daemon 第二遍。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub name: String,
    pub label: String,
    pub note: String,
    pub status: ProfileStatus,
    pub secret: Option<SecretPrompt>,
    pub install: Option<InstallPrompt>,
    /// 密钥仓里现在是不是真有这个 profile 的密钥。跟 `status` 分开存是因为
    /// `status_of` 里「装没装排在密钥前面」（见 profile.rs），一个 CLI 没装的
    /// profile 不管密钥填没填都会报 `NeedsDependency`/`NotInstalled`——从
    /// `status` 反推不出真实的密钥状态。密钥设置页要的是这个事实本身，
    /// 不是「现在能不能开会话」，两者在这种情况下会给出不同答案。
    pub has_secret: bool,
    /// 这个 profile 只做 dct 自己的 LLM 后端，不是能开会话的 agent。
    /// 界面靠它把这类条目从 agent 选择器里摘掉，同时留在密钥页上——
    /// 理由写在 `profile::Profile::backend_only` 上。
    #[serde(default)]
    pub backend_only: bool,
    /// 这个 profile 的密钥能不能靠配对拿到，而不是让用户粘贴——照抄自
    /// `profile::Profile::pairable`，理由写在那儿。UI 手上只有
    /// `ProfileEntry`，拿不到 `Profile`，所以这个 bool 必须跟着一起过桥，
    /// 不能让选择器/密钥页回去按名字特判（那正是这个字段要消灭的写法）。
    #[serde(default)]
    pub pairable: bool,
}

/// 九宫格一格的内容。跟 `Response::Screen` 不同，不带光标——
/// 只读的格子画光标只会误导人去打字。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenEntry {
    pub id: u32,
    pub lines: Vec<Vec<ScreenSpan>>,
}

/// 界面转发的一次鼠标事件，agent 当前的编码方式（哪种协议、SGR 与否）由
/// daemon 那一侧的 PTY 状态决定——界面只管「用户在哪个格子上做了什么」，
/// 不掺和编码。列/行 0 起算，跟 `Response::Screen` 的 `cursor` 一致。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MouseForward {
    pub col: u16,
    pub row: u16,
    pub kind: MouseForwardKind,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// 鼠标事件的种类。按下/松开带按钮号而不是分成三个变体各配一份——
/// 编码那一侧（Task 9）反正要按数字拼 SGR 序列，分开只会多一层 match。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum MouseForwardKind {
    WheelUp,
    WheelDown,
    /// 0 = 左键，1 = 中键，2 = 右键
    Press(u8),
    Release(u8),
}

#[derive(Serialize, Deserialize)]
pub enum Request {
    /// 「你是几号协议？」界面连上之后问的第一句。放在最前面，且
    /// **永远不许改形状**——它是唯一一条必须让任何年纪的守护进程都能
    /// 解出来的请求（老到答不上来也没关系，答不上来本身就是答案）。
    Hello,
    List,
    Create {
        dir: String,
        profile: String,
        /// 这次开的会话算不算「用户选的 agent」。「帮你装 CLI」那条路径开的是一个
        /// 跑安装命令的 shell 会话，绝不能被记成上次用的 agent——否则用户下次
        /// 按 `n` 会直接掉进一个命令行，而不是回到他真正想用的那个 agent。
        remember: bool,
    },
    Input {
        id: u32,
        text: String,
    },
    Screen {
        id: u32,
    },
    /// 一次取多个会话的屏幕。九宫格九个格子要是一个个问，
    /// 一问一答的串行连接上就是九个来回。
    Screens {
        ids: Vec<u32>,
    },
    Resize {
        id: u32,
        rows: u16,
        cols: u16,
    },
    Stop {
        id: u32,
    },
    /// 强杀：不给那 200ms 宽限期。
    ///
    /// 跟 `Stop` 的差别真实但很窄——`Stop` 走的是 portable-pty 的
    /// SIGHUP → 约 200ms → SIGKILL，这里直接 SIGKILL。留着两条命令是因为
    /// 「敲了 stop 它还在」这件事必须有下一步可走，而那一步不能是让用户
    /// 自己去 `ps` 里认进程号。
    Kill {
        id: u32,
    },
    /// 把已经停掉的会话从名册上抹掉。
    ///
    /// `Stop` 只把状态改成 `Stopped`，不删——守护进程活得很久，于是
    /// `dct ps` 会越积越多的墓碑。删是一个**用户显式发起**的动作，不是
    /// 守护进程定时干的：定时清会让「刚才那个会话去哪了」变成新问题。
    Prune,
    Undo {
        id: u32,
    },
    Diff {
        id: u32,
    },
    /// 带上界面当前的语言。守护进程不知道、也不该知道谁在用什么语言——
    /// 它是常驻的、可能同时服务多个界面的进程，把「当前语言」存成它的状态
    /// 就等于假设只有一个客户端。取哪一份文案由请求方说了算。
    Profiles {
        lang: crate::i18n::Lang,
    },
    Projects,
    /// 这个项目上次用的 agent。**必须带目录**：记忆是按项目分的，
    /// 一个全局值会让你在 A 项目按 `n` 开出 B 项目上次用的那个 agent。
    LastProfile {
        dir: String,
    },
    /// 把一个项目摆上看板（哪怕它一个会话都没有）。
    PinProject {
        dir: String,
    },
    /// 从看板上拿掉一个项目。只对没有会话的项目有意义，
    /// 「有没有会话」由界面判断，daemon 不管——它不知道界面正在显示什么。
    UnpinProject {
        dir: String,
    },
    SetSecret {
        profile: String,
        value: String,
    },
    DeleteSecret {
        profile: String,
    },
    VerifySecret {
        profile: String,
        value: String,
    },
    /// 起一条配对：daemon 打 `/admin/api/pair/start`，成功就在自己内存里
    /// 开一个轮询线程。**`device_code` 不在响应里**，它一次也不过 socket。
    PairStart {
        profile: String,
        /// 学生在配对屏上勾没勾「报错看不懂时让 AI 解释」。**必须跟着这条请求走**：
        /// 落盘发生在 daemon 的后台线程里，那时候界面早已经不在这条调用栈上了。
        /// `config.rs` 开头那段说 `[llm]` 缺席是隐私边界而不是缺省值——这个 bool
        /// 就是那个边界上唯一一次人的点头，把它丢在界面里等于把边界拆了。
        opt_in_llm: bool,
    },
    /// 读一次配对的当前状态。非阻塞——真正的轮询在 daemon 的后台线程里跑，
    /// 因为它要跑 15 分钟，而界面这条连接 5 秒就超时（`client.rs:11`）。
    PairPoll {
        profile: String,
        /// 勾选框的**当前**值，每一轮捎带一次。
        ///
        /// `PairStart` 带的那一份是起步那一刻的，而学生是在那之后才看见
        /// 「会把终端上的报错原文发给训练营网关」这句话的——他取消勾选
        /// 时，起步请求早飞走了。捎在这条本来就要发的请求上，而不是新加
        /// 一条「改主意了」：捎带是幂等的，丢一次下一轮自己补上；一条
        /// 单发的通知丢了就再没有人会发现。daemon 一直读到批准落地为止。
        opt_in_llm: bool,
    },
    /// 取消。**必须真的停线程并丢掉 `device_code`**：不停的话，用户退出去了，
    /// 后台还在替他领钥匙，领到了写进 secrets，而他以为自己取消了。
    PairCancel {
        profile: String,
    },
    /// 「这个 `Failed` 会话到底出了什么事」，人话版。答案可能还没算出来
    /// （问模型是异步的，见 `session.rs::request_explanation`），也可能
    /// 压根没配 LLM——两种情况都回 `Response::Explanation(None)`，界面
    /// 该显示今天就有的那句失败提示。
    Explanation {
        id: u32,
    },
    /// 用户主动滚动：相对滚几行，或者直接回到底部。类型是
    /// `session::ScrollBy`——协议层不重新定义一份平行的滚动语义。
    Scroll {
        id: u32,
        by: ScrollBy,
    },
    /// 界面转发的鼠标事件（滚轮、点击、拖拽结束的松开）。是否真的转发给
    /// agent 由 daemon 按当前是不是在看历史（`ScrollState::alt_screen` /
    /// `offset`）决定，界面不用先猜。
    Mouse {
        id: u32,
        event: MouseForward,
    },
    /// 手机通知眼下是什么状态。答案来自守护进程一份共享的状态槽——
    /// 配对本身是异步的（守护进程一直轮询，直到用户在 Telegram 里给 bot
    /// 发第一条消息），这条请求就是界面用来看那份状态槽此刻写着什么的。
    PhoneStatus,
    /// 填一个新令牌。守护进程拿它去打一次 Telegram 的 `getMe`（顺便验证令牌、
    /// 拿到 bot 用户名），通过就存进密钥仓、状态槽写成 `WaitingForPairing`；
    /// 不通过就把状态槽写成 `Broken`，令牌不落盘。这条请求会打真网络，
    /// 界面必须像 `VerifySecret` 一样丢给后台线程，不能堵在按键循环里。
    PhoneSetToken {
        token: String,
    },
    /// 解除当前配对（清空 `owner`），状态槽退回 `WaitingForPairing`——
    /// 令牌还在，等下一个人发消息重新配对。用在「换一台手机」的场景。
    PhoneUnpair,
    /// 整个关掉：删掉令牌，状态槽退回 `Off`。
    PhoneDisable,
    /// 局域网手机端现在开着没有、地址是什么。
    ///
    /// **这三条只从本机的 unix socket 上答，永远不从 HTTP 上答**（见
    /// `daemon::handle` 的 `web` 参数）：手机自己不该能开关这个监听口，
    /// 更不该能问出那条带令牌的地址——那等于把钥匙挂在门上。
    /// 虚拟键行上按了哪个键。**送的是名字不是字节**。
    ///
    /// 翻译由守护进程做（`web::keys::bytes_for` → `ui::key_to_input`，
    /// 桌面端每一次按键走的就是那个函数）。让网页自己翻的话，仓库里就有了
    /// 第二张映射表，而两份表漂了的症状是「手机上按方向键，agent 收到别的
    /// 东西」——桌面端永远看不见，手机上也不报错。
    ///
    /// 名字不在白名单里就回一句 `BadRequest`：按了一个键什么都没发生，
    /// 用户只会以为是网卡了，然后再按一次。
    Key {
        id: u32,
        name: String,
    },
    /// 手机网页要那张文案表。
    ///
    /// `lang` 是**浏览器原样报的那个标记**（`navigator.language`），不是解析
    /// 好的 `Lang`：解析要么在网页里做（那就等于网页开始自己决定语言），
    /// 要么在路由层做（中转没有路由层）。所以由守护进程解析，认不出来退回
    /// 英文，见 `web::strings::bundle_for`。
    WebStrings {
        lang: String,
    },
    WebStatus,
    WebEnable,
    WebDisable,
}

/// 手写 `Debug`，不能靠 `derive`——`SetSecret`/`VerifySecret` 两个变体的
/// `value` 是用户的明文密钥。今天没有任何地方真的 `{req:?}` 打印一个
/// `Request`（已核实），但 `serve()` 解析失败那条分支离
/// `eprintln!("bad request: {req:?}")` 只有一行距离——一旦有人图省事加了
/// 这行调试日志，密钥就会写进守护进程的 stderr（很可能重定向进一个存活
/// 比进程本身久得多的日志文件）。这里把 `value` 换成占位符，`profile`
/// 这种不敏感的字段照常打印，排查问题时还能看出是哪个 profile 出的请求。
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Request::Hello => write!(f, "Hello"),
            Request::List => write!(f, "List"),
            Request::Create {
                dir,
                profile,
                remember,
            } => f
                .debug_struct("Create")
                .field("dir", dir)
                .field("profile", profile)
                .field("remember", remember)
                .finish(),
            Request::Input { id, text } => f
                .debug_struct("Input")
                .field("id", id)
                .field("text", text)
                .finish(),
            Request::Screen { id } => f.debug_struct("Screen").field("id", id).finish(),
            Request::Screens { ids } => f.debug_struct("Screens").field("ids", ids).finish(),
            Request::Resize { id, rows, cols } => f
                .debug_struct("Resize")
                .field("id", id)
                .field("rows", rows)
                .field("cols", cols)
                .finish(),
            Request::Stop { id } => f.debug_struct("Stop").field("id", id).finish(),
            Request::Kill { id } => f.debug_struct("Kill").field("id", id).finish(),
            Request::Prune => write!(f, "Prune"),
            Request::Undo { id } => f.debug_struct("Undo").field("id", id).finish(),
            Request::Diff { id } => f.debug_struct("Diff").field("id", id).finish(),
            Request::Profiles { lang } => f.debug_struct("Profiles").field("lang", lang).finish(),
            Request::Projects => write!(f, "Projects"),
            Request::LastProfile { dir } => write!(f, "LastProfile {dir}"),
            Request::PinProject { dir } => write!(f, "PinProject {dir}"),
            Request::UnpinProject { dir } => write!(f, "UnpinProject {dir}"),
            Request::SetSecret { profile, .. } => f
                .debug_struct("SetSecret")
                .field("profile", profile)
                .field("value", &"<redacted>")
                .finish(),
            Request::DeleteSecret { profile } => f
                .debug_struct("DeleteSecret")
                .field("profile", profile)
                .finish(),
            Request::VerifySecret { profile, .. } => f
                .debug_struct("VerifySecret")
                .field("profile", profile)
                .field("value", &"<redacted>")
                .finish(),
            // 没有密钥可脱敏——`device_code` 从不出现在 `Request` 里，
            // 它只活在 daemon 自己的内存中（见 `PairStart` 上的注释）。
            Request::PairStart {
                profile,
                opt_in_llm,
            } => f
                .debug_struct("PairStart")
                .field("profile", profile)
                .field("opt_in_llm", opt_in_llm)
                .finish(),
            Request::PairPoll {
                profile,
                opt_in_llm,
            } => f
                .debug_struct("PairPoll")
                .field("profile", profile)
                .field("opt_in_llm", opt_in_llm)
                .finish(),
            Request::PairCancel { profile } => f
                .debug_struct("PairCancel")
                .field("profile", profile)
                .finish(),
            Request::Explanation { id } => f.debug_struct("Explanation").field("id", id).finish(),
            Request::Scroll { id, by } => f
                .debug_struct("Scroll")
                .field("id", id)
                .field("by", by)
                .finish(),
            // 没有密钥、没有用户输入的自由文本，坐标和按键状态照常打印。
            Request::Mouse { id, event } => f
                .debug_struct("Mouse")
                .field("id", id)
                .field("event", event)
                .finish(),
            Request::PhoneStatus => write!(f, "PhoneStatus"),
            // `token` 是密钥，同 `SetSecret`/`VerifySecret` 的道理，脱敏。
            Request::PhoneSetToken { .. } => f
                .debug_struct("PhoneSetToken")
                .field("token", &"<redacted>")
                .finish(),
            Request::PhoneUnpair => write!(f, "PhoneUnpair"),
            Request::PhoneDisable => write!(f, "PhoneDisable"),
            Request::Key { id, name } => write!(f, "Key({id}, {name})"),
            Request::WebStrings { lang } => write!(f, "WebStrings({lang:?})"),
            Request::WebStatus => write!(f, "WebStatus"),
            Request::WebEnable => write!(f, "WebEnable"),
            Request::WebDisable => write!(f, "WebDisable"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    /// 对 [`Request::Hello`] 的回答。同样不许改形状。
    Hello {
        protocol: u32,
    },
    Sessions(Vec<SessionInfo>),
    Created {
        id: u32,
    },
    Screen {
        lines: Vec<Vec<ScreenSpan>>,
        cursor: (u16, u16),
        /// agent 自己把光标藏起来了（`?25l`）。**界面必须跟着藏**——
        /// 干活中的 agent 差不多都藏着光标（Claude Code 画转圈的时候就是），
        /// 而 `cursor` 那个坐标仍然跟着它每一次重绘满屏乱跑；不问这件事就
        /// 每帧把真实终端的光标按上去，屏幕上多出来的是一个到处蹦的方块。
        ///
        /// `#[serde(default)]`：旧守护进程不发这个字段，补 `false` 正好是
        /// 「一直画着光标」的老行为，不会被误读成别的。
        #[serde(default)]
        cursor_hidden: bool,
        /// 贴在会话里时界面只调 `Screen`（`List` 太贵，见 `ui::run` 里的注释），
        /// 所以进程死了它只能从这里知道。少了它界面会永远画那张空缓冲——
        /// agent 退出时恢复主屏，主屏从来没被写过，所以「屏是空的」是正常的，
        /// 判断死活只能靠状态。
        state: SessionState,
        /// 底栏画滚动提示要用的全部事实。`#[serde(default)]`：往后再加字段
        /// 不用再动 `PROTOCOL_VERSION`——旧 JSON 没有这个字段时补一个
        /// `ScrollState::default()`（没在滚、没有未读行），跟真没滚动过的
        /// 会话是同一个状态，不会被误读成别的。
        #[serde(default)]
        scroll: ScrollState,
    },
    Screens {
        screens: Vec<ScreenEntry>,
    },
    Diff(Vec<FileStat>),
    Profiles {
        entries: Vec<ProfileEntry>,
        /// 密钥文件读不了、自定义 profile 写错了之类。UI 顶部红字。
        /// 报码不报句子——理由同 `ErrorCode`。
        warnings: Vec<WarningCode>,
    },
    Projects {
        recent: Vec<String>,
        pinned: Vec<String>,
    },
    /// 抹掉了几个。报数字而不是 `Ok`：用户敲 `dct prune` 想知道的正是
    /// 「清掉了多少」，而「一个都没有」和「清掉了 5 个」要说两句不同的话。
    Pruned(u32),
    LastProfile(Option<String>),
    Verify(crate::verify::VerifyOutcome),
    Ok,
    Error(ErrorCode),
    /// 对 [`Request::Explanation`] 的回答。`None` = 没有（还没算出来、
    /// 没配 LLM、或者算失败了——界面不用区分，统一显示今天就有的那句
    /// 失败提示）。
    Explanation(Option<String>),
    /// 对 [`Request::Scroll`] 的回答：滚完之后的状态。**目前没有任何调用点
    /// 读它**——界面所有发 `Request::Scroll` 的地方都用 `let _ = ...` 扔掉
    /// 返回值，底栏刷新靠的是下一轮 16ms 一次的 `Screen` 轮询自然带回来的
    /// `scroll` 字段，不是这个字段直接驱动的。留着 `Scrolled` 而不是改成
    /// `Ok`：它仍然是描述这次滚动的正确形状，往后要是哪个调用点想抄近路
    /// 立刻拿到滚完的状态（不等下一轮 `Screen`），数据已经现成。
    Scrolled(ScrollState),
    /// 对 `Request::PhoneStatus` / `PhoneSetToken` / `PhoneUnpair` /
    /// `PhoneDisable` 四条的共同回答：手机通知眼下是什么状态。
    Phone(PhoneStatus),
    /// `WebStatus` / `WebEnable` / `WebDisable` 三条的共同回答。
    Web(WebInfo),
    /// 对 [`Request::WebStrings`] 的回答：网页要显示的每一句话。
    ///
    /// `BTreeMap` 不是 `HashMap`：序列化出来的顺序要稳定，否则那条钉住线上
    /// 形状的测试每次跑都可能换个顺序。
    Strings(std::collections::BTreeMap<String, String>),
    /// 对 [`Request::PairStart`] 的回答。`Err` 是一句已经本地化过的原因
    /// （网关关着、连不上）。
    PairStarted(Result<PairStartedInfo, String>),
    /// 对 [`Request::PairPoll`] 的回答。
    PairTick(PairTick),
}

/// `pair::Started` 给界面看的那一面。**故意不是 `Started` 本身**：那个类型里有
/// `device_code`，而它是这条流程的凭据——界面要的只是画屏用的三样东西。
/// 同 `PairTick` 之于 `Tick`：少一个能装凭据的类型，就少一处它能漏出去的地方。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairStartedInfo {
    /// 大字印在屏幕上，学生照着念或照着敲。
    pub user_code: String,
    /// 路径，不是完整 URL——origin 由 dct 自己拼，见 spec 里那段钓鱼面的分析。
    pub verify_path: String,
    /// 倒计时用。
    pub expires_in: u64,
}

/// `pair::Tick` 给界面看的那一面。**故意不是 `Tick` 本身**：`Tick::Done`
/// 里装着 `api_key`，而界面一个字节都不需要它——钥匙落盘在 daemon 那边
/// 已经做完了。少一个能装钥匙的类型，就少一处它能漏出去的地方。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PairTick {
    Waiting,
    Done {
        /// 有没有拿到 Anthropic 那一组模型。免费账号是 false，
        /// 成功屏要据此换一句话说。
        anthropic_ready: bool,
        openai_ready: bool,
    },
    Expired {
        retryable: bool,
        message: String,
    },
    Failed(String),
}

/// 守护进程报「哪一类错 + 参数」，**不组句**。
///
/// 组句一律发生在界面进程：daemon 是常驻的、可能同时服务多个界面的进程，
/// 它不知道也不该知道谁在用什么语言。报码的另一个好处是切语言立刻生效，
/// 不用重启 daemon——它存下来的东西里没有任何一句是某种语言的。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    NoSuchProfile(String),
    DirNotFound(String),
    NotAGitRepo(String),
    NoSuchSession(u32),
    NoCheckpoint,
    /// 「这个会话没有检查点」和「没有改动记录」成因其实是同一个：不是 agent
    /// 会话。措辞交给界面——它知道用户刚按的是 `u` 还是 `d`，比守护进程更有
    /// 资格决定这句话怎么说。
    NotAnAgentSession,
    /// 请求解析失败，带上原始错误供排查
    BadRequest(String),
    /// git 自己的 stderr。**刻意留的兜底**：那是 git 按它自己的 `LANG` 输出的，
    /// dct 翻不动也不该翻。界面显示成「操作失败：<原文>」——外面那半句是
    /// 翻译过的，里面照抄。
    Git(String),
    /// agent 的命令跑不起来。带上命令名——用户至少知道该去修哪个。
    CannotStart(String),
    /// 守护进程那边没反应了（连不上、超时、连接被关）。三种情况用户能做的
    /// 是同一件事，所以不分。
    DaemonNotResponding,
    /// 密钥文件坏了，所以拒绝写入——当空覆盖的话，用户手改坏的文件
    /// （也许只是少个引号，完全能救回来）会被内存里那份残缺数据彻底盖掉。
    SecretsFileBroken {
        path: String,
    },
    /// 某个具体操作失败了。这些都是罕见的文件系统 / git 故障，用户能做的
    /// 只有「再试一次」或者「知道自己的工作区可能停在半路」——所以按**操作**
    /// 分类就够，不必把底层 io 错误也搬上界面。
    OperationFailed(Operation),
    /// 还没归类的内部错误，同样照抄原文。有它才能一步步迁移，而不是等到
    /// 每一条都归好类才敢合并。
    Internal(String),
}

/// 把一个 `ErrorCode` 塞进 `anyhow::Error` 里带出去。
///
/// 不把各模块的错误类型整个换成 `ErrorCode`：它们内部到处在 `?` io/git
/// 错误，全换要动的地方远多于收益；而 daemon 边界那一处 `downcast` 就能把
/// 码取回来（见 `daemon.rs` 的 `to_code`）。取不回来的就是还没归类的内部
/// 错误，照抄原文。
pub fn coded(c: ErrorCode) -> anyhow::Error {
    anyhow::Error::new(CodedError(c))
}

/// `anyhow` 要求负载实现 `std::error::Error`，包一层。
///
/// **刻意不给 `ErrorCode` 实现 `Display`**：实现了就会有人顺手拿它当界面
/// 文案，而那正是这套机制要根除的事——句子只能由 `i18n::msg::error` 组。
#[derive(Debug)]
pub struct CodedError(pub ErrorCode);

impl std::fmt::Display for CodedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 这句只会进 stderr 日志，不上界面——界面拿的是码。
        write!(f, "{:?}", self.0)
    }
}

impl std::error::Error for CodedError {}

/// 哪个操作失败了。措辞的分寸各不相同——「撤销失败」必须提醒用户工作区
/// 可能停在改到一半的状态（这是他必须知道的后果），而「算不出改了哪些文件」
/// 只要说再试一次就够。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Operation {
    /// 建会话时拍第一个检查点
    FirstCheckpoint,
    /// 每一步之后拍检查点
    Checkpoint,
    /// 撤销（git restore）
    Undo,
    /// 算改动（git diff）
    Diff,
    /// 写密钥文件
    SaveSecret,
    /// 写设置文件
    SaveSettings,
    /// 起 PTY 子进程
    SpawnPty,
    /// 读剪贴板
    ReadClipboard,
}

/// 读文件失败的类别。只留用户**分得清、也做得了什么**的那几种；
/// 其余归到笼统的一条——`io::Error` 的 `Display` 是系统原话，
/// 常年带 `os error N` 这种只有程序员看得懂的后缀。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IoReason {
    PermissionDenied,
    NotADirectory,
    Other,
}

/// 顶到界面上的**警告**（不是错误：dct 照常能用，只是有东西没读进来）。
/// 跟 `ErrorCode` 同样的道理——守护进程报码，界面组句。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WarningCode {
    /// 自定义 profile 目录打不开
    ProfileDirUnreadable { name: String, reason: IoReason },
    /// 某个 profile 文件读不了
    ProfileUnreadable { name: String, reason: IoReason },
    /// 某个 profile 文件写错了。`line` 是行号（有的话），`reason` 是 toml 库
    /// 自己的说法——那半句可能是英文，但用户本来就在手改这份 TOML，
    /// 「expected ...」比吞掉更有用（同 `ErrorCode::Git` 的道理）。
    ProfileMalformed {
        name: String,
        line: Option<usize>,
        reason: String,
    },
    /// 密钥文件读不了
    SecretsUnreadable { path: String, reason: IoReason },
    /// 用户写了 `[llm]`（一次主动的「我要开」），但那条连接接不上。
    ///
    /// 守护进程启动时 resolve 一次，失败就把原因**记下来**而不是只往 stderr
    /// 打一行：界面进程拉起守护进程时把它的 stderr 接到了 `/dev/null`
    /// （`client::spawn_daemon`——不然每一行都会糊在 TUI 上），所以那一行
    /// 谁都看不见，用户开了功能却只会得到一片沉默。带的是
    /// `ResolveError` 这个**码**，不是句子，理由同本枚举其余各条。
    LlmUnavailable(crate::llm::resolve::ResolveError),
    /// 密钥文件坏了。**不给行号也不给 toml 的原文**：README 明说密钥不该手改，
    /// 而且这时候所有写入都被拒，照着行号去抠语法是把用户往错路上支。
    /// 唯一有效的下一步是删掉它重新粘贴一遍。
    SecretsCorrupt { path: String },
    /// 守护进程重启时，`last-sessions.toml` 里记的某一条没能接回来。
    ///
    /// **这条曾经只往 stderr 打一行**——而真正被 TUI 拉起来的那个守护进程，
    /// stdio 全被接到 `/dev/null`（`client::spawn_daemon`），用户按了 y
    /// 同意恢复，一个格子却悄无声息地没出现，没有任何地方告诉他是目录没了
    /// 还是 profile 被卸载了。跟 `LlmUnavailable` 同一个理由：守护进程的
    /// stderr 到不了用户眼前，唯一能到的路是这里，经 `Request::Profiles`
    /// 顶成一条警告。
    SessionResumeSkipped {
        dir: String,
        profile: String,
        reason: SessionResumeSkipReason,
    },
}

/// 恢复上次会话时，某一条为什么被跳过。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionResumeSkipReason {
    /// 记录的目录已经不存在了
    DirGone,
    /// 记录的 profile 已经不认识了（被删掉、改了名……）
    ProfileGone,
    /// 目录和 profile 都还在，但重新起这个会话本身失败了
    StartFailed,
}

pub fn socket_path() -> PathBuf {
    // 问不出家目录就退到临时目录。这不是一个能用的长期位置（临时目录会被
    // 清理），但它保证 dct 还能起来——而问不出家目录本身已经是这台机器
    // 出了别的问题。
    let home = crate::sys::home().unwrap_or_else(std::env::temp_dir);
    home.join(".dct").join("daemon.sock")
}

/// 会话生死簿的位置，跟着 socket 走（同 `store_path_for_socket`），
/// 测试把 socket 放临时目录就自动隔离。
pub fn journal_path_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("sessions.log"),
        None => PathBuf::from("sessions.log"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_the_secret_on_set_secret() {
        let req = Request::SetSecret {
            profile: "kimi".into(),
            value: "sk-super-secret-value".into(),
        };
        let s = format!("{req:?}");
        assert!(
            !s.contains("sk-super-secret-value"),
            "密钥不能出现在 Debug 输出里：{s}"
        );
        assert!(s.contains("kimi"), "profile 名字留着帮排查：{s}");
    }

    #[test]
    fn debug_redacts_the_secret_on_verify_secret() {
        let req = Request::VerifySecret {
            profile: "glm".into(),
            value: "sk-another-secret-value".into(),
        };
        let s = format!("{req:?}");
        assert!(
            !s.contains("sk-another-secret-value"),
            "密钥不能出现在 Debug 输出里：{s}"
        );
        assert!(s.contains("glm"), "profile 名字留着帮排查：{s}");
    }

    /// 同 `debug_redacts_the_secret_on_set_secret`：手机令牌也是密钥，
    /// 不能出现在 Debug 输出里。
    #[test]
    fn debug_redacts_the_token_on_phone_set_token() {
        let req = Request::PhoneSetToken {
            token: "123456:AAH-super-secret-token".into(),
        };
        let s = format!("{req:?}");
        assert!(
            !s.contains("123456:AAH-super-secret-token"),
            "手机令牌不能出现在 Debug 输出里：{s}"
        );
    }

    #[test]
    fn screens_request_round_trips() {
        let req = Request::Screens { ids: vec![1, 3, 7] };
        let s = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&s).unwrap();
        match back {
            Request::Screens { ids } => assert_eq!(ids, vec![1, 3, 7]),
            other => panic!("解回来不是 Screens：{other:?}"),
        }
    }

    /// 答不上「你是几号协议」的守护进程，就是老到连这个问题都不认识的那种。
    /// 现场那个 daemon 正是如此：它比 `Hello` 还老。
    #[test]
    fn a_daemon_that_cannot_answer_is_stale() {
        assert_eq!(daemon_status(None), DaemonStatus::Stale);
    }

    /// 答得上但号对不上，一样不能用——两边编译自不同的源码。
    #[test]
    fn a_daemon_on_another_protocol_is_stale() {
        assert_eq!(
            daemon_status(Some(PROTOCOL_VERSION + 1)),
            DaemonStatus::Stale
        );
        assert_eq!(
            daemon_status(Some(PROTOCOL_VERSION.wrapping_sub(1))),
            DaemonStatus::Stale
        );
    }

    #[test]
    fn a_daemon_on_the_same_protocol_is_usable() {
        assert_eq!(daemon_status(Some(PROTOCOL_VERSION)), DaemonStatus::Same);
    }

    /// 协议的线上形状，钉死在 `PROTOCOL_VERSION` 上。
    ///
    /// 改了任何一个变体的名字或字段，这条会红。红了就**必须**把
    /// `PROTOCOL_VERSION` 一起加一——否则新界面又会去跟一个解不出它请求的
    /// 旧守护进程说话，而用户看到的只有一句「拿不到 agent 列表」，
    /// 没有任何线索指向真正的原因。这正是 2026-08-05 那次现场事故。
    ///
    /// 只钉请求：请求是**新界面发给旧 daemon** 的那一半，也就是会炸的那一半。
    #[test]
    fn the_request_shape_is_pinned_to_the_protocol_version() {
        // 每个变体都要在这里出现一次。漏一个不会被这条测试发现，但会被
        // `impl Debug for Request` 那个穷举 match 拦下来——加变体必须
        // 同时改那里。
        let all = vec![
            Request::Hello,
            Request::List,
            Request::Create {
                dir: "d".into(),
                profile: "p".into(),
                remember: true,
            },
            Request::Input {
                id: 1,
                text: "t".into(),
            },
            Request::Screen { id: 1 },
            Request::Screens { ids: vec![1] },
            Request::Resize {
                id: 1,
                rows: 2,
                cols: 3,
            },
            Request::Stop { id: 1 },
            Request::Kill { id: 1 },
            Request::Prune,
            Request::Undo { id: 1 },
            Request::Diff { id: 1 },
            Request::Profiles {
                lang: crate::i18n::Lang::Zh,
            },
            Request::Projects,
            Request::SetSecret {
                profile: "p".into(),
                value: "v".into(),
            },
            Request::DeleteSecret {
                profile: "p".into(),
            },
            Request::LastProfile { dir: "d".into() },
            Request::PinProject { dir: "d".into() },
            Request::UnpinProject { dir: "d".into() },
            Request::VerifySecret {
                profile: "p".into(),
                value: "v".into(),
            },
            Request::PairStart {
                profile: "p".into(),
                opt_in_llm: true,
            },
            Request::PairPoll {
                profile: "p".into(),
                opt_in_llm: true,
            },
            Request::PairCancel {
                profile: "p".into(),
            },
            Request::Explanation { id: 1 },
            Request::Scroll {
                id: 1,
                by: ScrollBy::Rows(3),
            },
            Request::Mouse {
                id: 1,
                event: MouseForward {
                    col: 10,
                    row: 20,
                    kind: MouseForwardKind::Press(0),
                    shift: false,
                    alt: false,
                    ctrl: false,
                },
            },
            Request::PhoneStatus,
            Request::PhoneSetToken { token: "t".into() },
            Request::PhoneUnpair,
            Request::PhoneDisable,
            Request::Key {
                id: 1,
                name: "Up".into(),
            },
            Request::WebStrings {
                lang: "zh-CN".into(),
            },
            Request::WebStatus,
            Request::WebEnable,
            Request::WebDisable,
        ];

        let shape = serde_json::to_string(&all).unwrap();
        assert_eq!(
            (PROTOCOL_VERSION, shape.as_str()),
            (
                12,
                r#"["Hello","List",{"Create":{"dir":"d","profile":"p","remember":true}},{"Input":{"id":1,"text":"t"}},{"Screen":{"id":1}},{"Screens":{"ids":[1]}},{"Resize":{"id":1,"rows":2,"cols":3}},{"Stop":{"id":1}},{"Kill":{"id":1}},"Prune",{"Undo":{"id":1}},{"Diff":{"id":1}},{"Profiles":{"lang":"Zh"}},"Projects",{"SetSecret":{"profile":"p","value":"v"}},{"DeleteSecret":{"profile":"p"}},{"LastProfile":{"dir":"d"}},{"PinProject":{"dir":"d"}},{"UnpinProject":{"dir":"d"}},{"VerifySecret":{"profile":"p","value":"v"}},{"PairStart":{"profile":"p","opt_in_llm":true}},{"PairPoll":{"profile":"p","opt_in_llm":true}},{"PairCancel":{"profile":"p"}},{"Explanation":{"id":1}},{"Scroll":{"id":1,"by":{"Rows":3}}},{"Mouse":{"id":1,"event":{"col":10,"row":20,"kind":{"Press":0},"shift":false,"alt":false,"ctrl":false}}},"PhoneStatus",{"PhoneSetToken":{"token":"t"}},"PhoneUnpair","PhoneDisable",{"Key":{"id":1,"name":"Up"}},{"WebStrings":{"lang":"zh-CN"}},"WebStatus","WebEnable","WebDisable"]"#
            ),
            "协议的线上形状变了。把 PROTOCOL_VERSION 加一，再把这里的期望值更新成新的形状。"
        );
    }

    /// **配对的响应里绝不许出现钥匙。** UI 不需要它——落盘在 daemon 那边做完了。
    /// 一旦它过一次 socket，它就会出现在任何一个手滑加上的 `{resp:?}` 里。
    #[test]
    fn a_pair_tick_never_carries_the_key() {
        let t = PairTick::Done {
            anthropic_ready: true,
            openai_ready: true,
        };
        let json = serde_json::to_string(&t).unwrap();
        assert!(!json.contains("api_key"), "{json}");
        assert!(!json.contains("sk-"), "{json}");
    }

    /// **`PairStarted` 的响应里绝不许出现 `device_code`。** 它是这条流程的
    /// 凭据，spec 说它只活在 daemon 里、一次也不许过 socket——`PairStartedInfo`
    /// 存在正是为了让这条 socket 上的类型物理上装不下它。
    #[test]
    fn a_pair_started_response_never_carries_the_device_code() {
        let r = Response::PairStarted(Ok(PairStartedInfo {
            user_code: "HJ4K-9QTZ".into(),
            verify_path: "/pair".into(),
            expires_in: 900,
        }));
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("device_code"), "{json}");
    }

    /// `device_code` 是凭据，跟密钥一个待遇：手写的 Debug 要把它挡住。
    ///
    /// **这条测试要断言的是"没有"，不是"有"。** 它原来只查了
    /// `s.contains("dc")`——那句话证明的是 profile 名照常打印，跟脱敏
    /// 半点关系没有，一个把凭据原样打出来的 `Debug` 也照样能让它变绿。
    /// 一条只断言正面的脱敏测试比没有测试更坏：它让人以为这件事有人守着。
    #[test]
    fn pair_requests_do_not_print_anything_sensitive() {
        // 配对这三条请求里**装不下**凭据——`device_code` 只活在 daemon
        // 自己的内存里，`Request` 的类型本身就没有这个字段。所以这里能
        // 断言的负面是：任何一条配对请求的 `Debug` 输出里都不许出现
        // 密钥形状的东西，而 `SetSecret`/`VerifySecret` 这两条真装着密钥的
        // 请求必须把它盖掉。三条一起测，是因为脱敏这件事一旦漏，
        // 漏的一定是新加的那一条。
        for r in [
            Request::PairStart {
                profile: "dc".into(),
                opt_in_llm: true,
            },
            Request::PairPoll {
                profile: "dc".into(),
                opt_in_llm: true,
            },
            Request::PairCancel {
                profile: "dc".into(),
            },
        ] {
            let s = format!("{r:?}");
            assert!(s.contains("dc"), "profile 该照常打印，排查问题要用：{s}");
            assert!(!s.contains("device_code"), "凭据的字段名都不该出现：{s}");
            assert!(!s.contains("sk-"), "{s}");
        }

        let s = format!(
            "{:?}",
            Request::SetSecret {
                profile: "dc".into(),
                value: "sk-live-secret".into(),
            }
        );
        assert!(!s.contains("sk-live-secret"), "密钥漏进了 Debug：{s}");
        let s = format!(
            "{:?}",
            Request::VerifySecret {
                profile: "dc".into(),
                value: "sk-live-secret".into(),
            }
        );
        assert!(!s.contains("sk-live-secret"), "密钥漏进了 Debug：{s}");
    }

    /// 请求那条 pin 只钉了**发出去**的形状，回来的没人管——而 2026-08-06
    /// 给 `SessionInfo` 加 `is_agent` 正是改在回程上：旧守护进程回的 JSON
    /// 里没这个字段，新界面 `from_str` 直接失败，症状是看板一个会话都没有。
    /// 回程的形状同样是契约，同样要钉。
    ///
    /// **2026-08-09 的例外，写清楚不然会被误读成先例**：这条测试的期望字符串
    /// 尾部多了 `,"tag":""`，`PROTOCOL_VERSION` 却没跟着加一。这不是「顺手
    /// 把期望值改掉就让测试重新变绿」——2026-08-05 那次事故（见本文件顶部
    /// 的版本变更记录）正是这个动作本身，这条测试存在的全部目的就是让那个
    /// 动作红。允许这次例外只因为同时满足两个条件：
    ///
    /// 1. `SessionInfo.tag` 带 `#[serde(default)]`——旧界面解新守护进程的
    ///    JSON，多出来的字段被 serde 直接忽略；新界面解旧守护进程的 JSON，
    ///    缺的字段补成空串。两边都不会解析失败。
    /// 2. 这次没有新增或改动任何 `Request` 变体——旧守护进程完全不需要
    ///    「懂」这个新字段，它甚至不知道对面在问它，`tag` 只是它答复里顺带
    ///    多出来的一段旧进程从不读的文本。
    ///
    /// 这条规则**不能推广**：只要对面必须**理解**一个新字段或新变体才能
    /// 正常应答（而不是可以安全无视），版本号就要加一，不管那个字段本身
    /// 带不带 `#[serde(default)]`。下次想跳过版本号，先证明满足上面两条，
    /// 不是从一个空字符串开始编故事。
    #[test]
    fn the_session_info_shape_is_pinned_too() {
        let info = SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/d".into(),
            state: SessionState::Idle,
            activity: "a".into(),
            is_agent: true,
            tag: String::new(),
        };
        let shape = serde_json::to_string(&info).unwrap();
        assert_eq!(
            (PROTOCOL_VERSION, shape.as_str()),
            (
                12,
                r#"{"id":1,"profile":"claude","dir":"/d","state":"Idle","activity":"a","is_agent":true,"tag":""}"#
            ),
            "会话信息的线上形状变了。把 PROTOCOL_VERSION 加一，再把这里的期望值更新成新的形状。"
        );
    }

    #[test]
    fn screens_response_round_trips() {
        use crate::pty::{ScreenSpan, ScreenStyle};
        let resp = Response::Screens {
            screens: vec![ScreenEntry {
                id: 4,
                lines: vec![vec![ScreenSpan {
                    text: "干活中".into(),
                    style: ScreenStyle::default(),
                }]],
            }],
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&s).unwrap();
        match back {
            Response::Screens { screens } => {
                assert_eq!(screens.len(), 1);
                assert_eq!(screens[0].id, 4);
                assert_eq!(screens[0].lines[0][0].text, "干活中");
            }
            other => panic!("解回来不是 Screens：{other:?}"),
        }
    }

    /// 新加的滚动字段必须能从「没有这个字段」的旧 JSON 里解出来。
    /// 这是 `#[serde(default)]` 的意义所在：往后再加字段就不用再动版本号。
    ///
    /// `state` 带上而不是照抄 brief 原样的 `{"lines":[],"cursor":[0,0]}}`：
    /// 那是 3 版之前的形状，`state` 早就是这个变体里不带默认值的必填字段
    /// （见它自己的文档注释——少了它界面就没法判断进程死活）。这里只测
    /// `scroll` 这一个新字段的向后兼容，不重新测已经钉在别处的旧字段。
    #[test]
    fn a_screen_response_without_scroll_still_parses() {
        let old = r#"{"Screen":{"lines":[],"cursor":[0,0],"state":"Idle"}}"#;
        let r: Response = serde_json::from_str(old).unwrap();
        match r {
            Response::Screen { scroll, .. } => {
                assert_eq!(scroll, crate::session::ScrollState::default());
            }
            _ => panic!("解成了别的变体"),
        }
    }

    /// 同上，换 `cursor_hidden` 这个后加的字段：旧守护进程回的 JSON 里
    /// 没有它，新界面必须解得出来，而且补出来的 `false` 就是「照旧一直
    /// 画着光标」——跟旧守护进程的真实行为对得上。
    #[test]
    fn a_screen_response_without_cursor_hidden_still_parses() {
        let old = r#"{"Screen":{"lines":[],"cursor":[0,0],"state":"Idle"}}"#;
        let r: Response = serde_json::from_str(old).unwrap();
        match r {
            Response::Screen { cursor_hidden, .. } => {
                assert!(!cursor_hidden, "缺字段要补成「没藏」，也就是照旧画光标");
            }
            _ => panic!("解成了别的变体"),
        }
    }

    #[test]
    fn scroll_requests_survive_a_round_trip() {
        for by in [ScrollBy::Rows(3), ScrollBy::Rows(-3), ScrollBy::Bottom] {
            let req = Request::Scroll { id: 7, by };
            let s = serde_json::to_string(&req).unwrap();
            let back: Request = serde_json::from_str(&s).unwrap();
            assert!(matches!(back, Request::Scroll { id: 7, .. }));
        }
    }

    /// 手写的 Debug 漏一条 arm 会编译不过，但漏了密钥脱敏不会。
    /// 顺手确认新变体没有把什么敏感东西带进 Debug。
    #[test]
    fn mouse_debug_has_no_surprises() {
        let req = Request::Mouse {
            id: 1,
            event: MouseForward {
                col: 10,
                row: 20,
                kind: MouseForwardKind::WheelUp,
                shift: false,
                alt: false,
                ctrl: false,
            },
        };
        let s = format!("{req:?}");
        assert!(s.contains("Mouse"));
    }

    /// `Projects` 的回程形状同样是契约：`pinned` 是这一版新加的字段，旧守护
    /// 进程回的 JSON 里没有它。跟上面两条一样钉在 `PROTOCOL_VERSION` 上——
    /// 光钉一个裸字符串的话，谁把这个变体改了形状、只顺手更新这里的期望值，
    /// 就能一路绿灯地把版本号留在原地，而那正是 2026-08-05 那次事故的形状。
    #[test]
    fn projects_response_carries_both_lists() {
        let r = Response::Projects {
            recent: vec!["/a".into()],
            pinned: vec!["/b".into()],
        };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(
            (PROTOCOL_VERSION, s.as_str()),
            (12, r#"{"Projects":{"recent":["/a"],"pinned":["/b"]}}"#),
            "协议的线上形状变了。把 PROTOCOL_VERSION 加一，再把这里的期望值更新成新的形状。"
        );
    }
}
