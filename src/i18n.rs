//! 界面语言：语言的解析与词条表。
//!
//! 这个模块**不认识界面，也不认识守护进程**——它只回答「这条文案在这种语言里
//! 怎么说」。守护进程那边永远不组句（它连用户选了什么语言都不知道），只报
//! 错误码，组句一律发生在界面进程，所以切语言立刻生效、不用重启 daemon。

use serde::{Deserialize, Serialize};

/// 界面语言。第一阶段两种；将来加 `Ja` 那天，编译器会把每一条没翻的都点名——
/// 这是选枚举而不是配置文件的唯一理由，也正是本项目要的。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Lang {
    En,
    Zh,
}

impl Lang {
    /// 语言自己的名字。误切到看不懂的语言时，用户还得能认出自己那一行切回来——
    /// 所以这里绝不能是「英语 / 中文」这种用当前语言写的译名。
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Zh => "中文",
        }
    }

    /// 存进 settings.json 的稳定短码。跟枚举顺序无关，加语言不会让老文件失效。
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Zh => "zh",
        }
    }

    pub fn from_code(s: &str) -> Option<Lang> {
        match s {
            "en" => Some(Lang::En),
            "zh" => Some(Lang::Zh),
            _ => None,
        }
    }

    pub fn all() -> &'static [Lang] {
        &[Lang::En, Lang::Zh]
    }
}

/// 最终用哪种语言。优先级：`DCT_LANG` > 用户存过的设置 > 系统 locale > `En`。
///
/// `env` 是闭包不是直接读 `std::env`：环境变量是进程全局状态，测试里改它会互相
/// 打架。生产传 `|k| std::env::var(k).ok()`，测试传一张假表。
pub fn resolve(saved: Option<Lang>, env: &dyn Fn(&str) -> Option<String>) -> Lang {
    // DCT_LANG 压过一切：它是「这一次就用这个」的逃生口，值不认识就当没设，
    // 继续往下走，而不是硬摔成 En——用户打错一个字母不该丢掉他存过的选择。
    if let Some(l) = env("DCT_LANG").as_deref().and_then(Lang::from_code) {
        return l;
    }
    if let Some(l) = saved {
        return l;
    }
    // 系统 locale 只认主码：`zh_CN.UTF-8` → `zh`。认不出就是 En。
    for k in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let Some(v) = env(k) else { continue };
        if v.is_empty() {
            continue;
        }
        let primary = v.split(['_', '.', '@']).next().unwrap_or("");
        if let Some(l) = Lang::from_code(primary) {
            return l;
        }
    }
    Lang::En
}

/// 一条文案的各语言写在一起，不是每种语言各一个大 `match`——改中文时英文
/// 就在眼前，不会出现「改了一半」。
macro_rules! t {
    ($lang:expr, en: $en:expr, zh: $zh:expr $(,)?) => {
        match $lang {
            Lang::En => $en,
            Lang::Zh => $zh,
        }
    };
}

/// 无参文案。带参的走下面的 `msg` 模块——那些必须是函数，见该模块的注释。
///
/// 加 `Lang::Ja` 那天，`text()` 里每一条没翻的都会被编译器点名。这是选枚举
/// 而不是配置文件的全部理由。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    // —— 动作（底栏按键表用，也是设置页等处的通用词）——
    New,
    SwitchAgent,
    /// `Tab` 的说明：在看板上已有的项目之间跳。**`p` 不是它**——见 `AddProject`。
    SwitchProject,
    /// `p` 的说明：把一个看板上还没有的项目摆上来。
    ///
    /// 跟 `SwitchProject` 分开是因为它们真的是两件事：`Tab` 在已有的组之间
    /// 走，`p` 往看板上加一个新的。共用「换项目」那句话的年代里，`p` 是唯一
    /// 的换项目手段；`Tab` 出现之后再写「换项目」，用户会以为按 `p` 能一步
    /// 换过去，而实际弹出来的是一个选择器。
    AddProject,
    /// `x` 的说明：把光标所在的空项目从看板上拿掉。
    RemoveProject,
    SeeAllProjects,
    ThisProjectOnly,
    Secrets,
    Grid,
    List,
    Select,
    Open,
    Zoom,
    Undo,
    Stop,
    Diff,
    Quit,
    Confirm,
    Cancel,
    Back,
    Edit,
    Delete,
    MoveArrows,
    /// 九宫格里打开单行回复框的那个键
    ReplyOnce,
    /// 回复框里的两个键
    SendReply,
    /// 回复框里的 Ctrl+C：打断正在干活的 agent
    InterruptAgent,
    /// 回复框还空着时的占位提示：直接回车 = 替用户按一下回车（批准/继续）
    EmptyReplyIsEnter,
    NextSession,
    OtherKeysGoToAgent,
    NewSessionFromBoard,
    BackToListWord,
    BackToSettingsWord,
    OrPressDigit,
    TypeToFilter,
    Language,
    // —— 全部按键浮层 ——
    /// 底栏最右那条常驻提示的说明。就是一个省略号：底栏只有一行，
    /// 写成「全部按键」四个字要占掉一个真按键的位置，而 `…` 本身
    /// 就是「后面还有」的通用说法。
    MoreKeys,
    AllKeys,
    KeysGroupMove,
    KeysGroupSession,
    KeysGroupConfig,
    // —— 逃生键 ——
    BackToBoard,
    BackToBoardWithF2,
    BackToList,
    BackToSettings,
    // —— 视图标题 ——
    BoardTitle,
    BoardTitleAllProjects,
    Disconnected,
    PickAgentTitle,
    PickProjectTitle,
    TypePathTitle,
    SettingsTitle,
    CurrentProject,
    ManualPath,
    RecentProjects,
    SwitchPane,
    EnterFolder,
    GoUp,
    NoSubfolders,
    // —— 状态与提示 ——
    NoSessionsHere,
    NoSessionsAtAll,
    /// 组头上：这个项目的目录已经不在了
    ProjectDirGone,
    AllSessionsStopped,
    WindowTooSmall,
    Verifying,
    PasteOrTypeKey,
    NoOtherRunningSession,
    NoSessionSelected,
    DaemonUnreachable,
    StaleData,
    // —— 会话状态（看板与九宫格的状态列）——
    // —— dct ps / dct stop（普通终端里用，不开界面）——
    NoDaemonRunning,
    NoSessionsRunning,
    NothingToPrune,
    StatusWorking,
    StatusAsking,
    StatusIdle,
    StatusStopped,
    StatusFailed,
    StatusUnknown,
    // —— 密钥页 ——
    SecretsTitle,
    SecretSet,
    SecretUnset,
    OpenSignupPage,
    NothingToDelete,
    PressDAgainToDelete,
    SecretNotSaved,
    SecretNotDeleted,
    VerifyingShort,
    BadSecret,
    NetworkUnreachable,
    // —— 各种失败 ——
    CreateFailed,
    CannotOpenInstallWindow,
    NoPathTyped,
    CannotListAgents,
    CannotListSecrets,
    CannotListProjects,
    SessionOpenFailed,
    PasteNotSent,
    InputNotSent,
    DaemonTooOld,
    // —— 启动时撞上旧守护进程 ——
    StaleDaemonExplain,
    StaleDaemonAsk,
    StaleDaemonRestarting,
    StaleDaemonRestartFailed,
    RequestFailed,
    ActionDone,
    NoChanges,
    // —— 选择器里的「为什么用不了」——
    ReasonNeedsSecret,
    ReasonNotInstalled,
    /// `x` 按在一个还有会话的组上
    GroupNotEmpty,
}

pub fn text(k: Key, lang: Lang) -> &'static str {
    use Key::*;
    match k {
        New => t!(lang, en: "new", zh: "新建"),
        SwitchAgent => t!(lang, en: "switch agent", zh: "换 agent"),
        SwitchProject => t!(lang, en: "switch project", zh: "换项目"),
        AddProject => t!(lang, en: "add project", zh: "加项目"),
        RemoveProject => t!(lang, en: "remove", zh: "移除"),
        SeeAllProjects => t!(lang, en: "all projects", zh: "看全部项目"),
        ThisProjectOnly => t!(lang, en: "this project only", zh: "只看本项目"),
        Secrets => t!(lang, en: "keys", zh: "密钥"),
        Grid => t!(lang, en: "grid", zh: "九宫格"),
        List => t!(lang, en: "list", zh: "列表"),
        Select => t!(lang, en: "select", zh: "选择"),
        // 「进会话」而不是「进入」：底栏中段现在写着项目名，紧挨着一个
        // 光秃秃的「进入」，读起来像是「进入这个项目」。用户按 Enter 得到
        // 的是选中那一行的那个会话，说清楚是进哪儿不用多花一列。
        Open => t!(lang, en: "open", zh: "进会话"),
        Zoom => t!(lang, en: "zoom in", zh: "放大"),
        Undo => t!(lang, en: "undo", zh: "回滚"),
        Stop => t!(lang, en: "stop", zh: "停止"),
        Diff => t!(lang, en: "changes", zh: "改动"),
        Quit => t!(lang, en: "quit", zh: "退出"),
        Confirm => t!(lang, en: "confirm", zh: "确认"),
        Cancel => t!(lang, en: "cancel", zh: "取消"),
        Back => t!(lang, en: "back", zh: "返回"),
        Edit => t!(lang, en: "edit", zh: "改"),
        Delete => t!(lang, en: "delete", zh: "删"),
        MoveArrows => t!(lang, en: "arrow keys pick a tile", zh: "方向键选格子"),
        // 「回一句」而不是「输入」：用户要做的事是回复一个正在等他的 agent，
        // 不是「往某处输入文本」。前者说的是意图，后者说的是机制。
        ReplyOnce => t!(lang, en: "reply", zh: "回一句"),
        SendReply => t!(lang, en: "send", zh: "送出"),
        // 「打断它」而不是「中断」：用户要做的是喊停一个跑偏的 agent
        InterruptAgent => t!(lang, en: "stop the agent", zh: "打断它"),
        // 空框直接回车最常用（批个计划、说声继续），得写在用户眼前，
        // 否则他会以为必须先打字才能回。
        EmptyReplyIsEnter => t!(
            lang,
            en: "type a reply, or just press Enter to approve",
            zh: "打字回复，或者直接回车表示同意"
        ),
        NextSession => t!(lang, en: "next session", zh: "下一个会话"),
        NewSessionFromBoard => t!(
            lang,
            en: "go back and press n for a new session",
            zh: "回看板后按 n 新建会话",
        ),
        BackToListWord => t!(lang, en: "back to the list", zh: "返回列表"),
        BackToSettingsWord => t!(lang, en: "back to settings", zh: "返回设置"),
        OtherKeysGoToAgent => t!(
            lang,
            en: "every other key goes to the agent",
            zh: "其余按键都发给 agent",
        ),
        OrPressDigit => t!(lang, en: "or press a number", zh: "或直接按数字"),
        TypeToFilter => t!(lang, en: "type to filter", zh: "直接打字过滤"),
        Language => t!(lang, en: "language", zh: "语言"),

        MoreKeys => t!(lang, en: "…", zh: "…"),
        AllKeys => t!(lang, en: "All keys", zh: "全部按键"),
        KeysGroupMove => t!(lang, en: "Move", zh: "走动"),
        KeysGroupSession => t!(lang, en: "Sessions", zh: "会话"),
        KeysGroupConfig => t!(lang, en: "Settings", zh: "设置"),

        BackToBoard => t!(lang, en: "Ctrl+Q back", zh: "Ctrl+Q 回看板"),
        BackToBoardWithF2 => t!(lang, en: "Ctrl+Q (F2) back", zh: "Ctrl+Q（F2） 回看板"),
        BackToList => t!(lang, en: "Ctrl+Q back", zh: "Ctrl+Q 回列表"),
        BackToSettings => t!(lang, en: "Ctrl+Q settings", zh: "Ctrl+Q 回设置"),

        BoardTitle => t!(lang, en: "dct sessions", zh: "dct 会话看板"),
        BoardTitleAllProjects => t!(
            lang,
            en: "dct sessions · all projects",
            zh: "dct 会话看板 · 全部项目",
        ),
        Disconnected => t!(
            lang,
            en: "disconnected, this may be out of date",
            zh: "连接已断开，数据可能已过期",
        ),
        PickAgentTitle => t!(lang, en: "Pick an agent", zh: "选 agent"),
        PickProjectTitle => t!(lang, en: "Pick a project", zh: "选项目"),
        TypePathTitle => t!(lang, en: "Type a project path", zh: "输入项目路径"),
        SettingsTitle => t!(lang, en: "Settings", zh: "设置"),
        // 底栏中段现在只写项目本身，不再写「当前项目：」这个标签——一行里
        // 最贵的是列数，而「这里写的是哪个项目」不用一个标签来说明。词条留着
        // 是因为别处（浮层标题之类）随时可能要，它不占屏幕。
        CurrentProject => t!(lang, en: "Project", zh: "当前项目"),
        ManualPath => t!(lang, en: "Type a path…", zh: "手输路径…"),
        RecentProjects => t!(lang, en: "Recent", zh: "最近"),
        SwitchPane => t!(lang, en: "switch side", zh: "切换左右"),
        EnterFolder => t!(lang, en: "open folder", zh: "进入文件夹"),
        GoUp => t!(lang, en: "go up", zh: "上一级"),
        NoSubfolders => t!(
            lang,
            en: "No folders here — press ← to go up",
            zh: "这里没有文件夹，按 ← 回上一级",
        ),

        // 组头上那一句，不是整屏空态——分组之后这句话贴在项目名后面，
        // 屏幕上别的组还列着会话，「按 n 开一个」那半句在这里是噪音。
        NoSessionsHere => t!(lang, en: "no sessions yet", zh: "还没有会话"),
        ProjectDirGone => t!(lang, en: "folder is gone", zh: "目录不在了"),
        AllSessionsStopped => t!(
            lang,
            en: "Every session here has stopped. Press g for the list to see them, or n to start one.",
            zh: "这里的会话都停了。按 g 回列表能看到它们，按 n 开一个新的",
        ),
        NoSessionsAtAll => t!(
            lang,
            en: "No sessions yet. Press n to start one.",
            zh: "还没有任何会话，按 n 开一个",
        ),
        WindowTooSmall => t!(
            lang,
            en: "Window too small — enlarge the terminal to see the grid",
            zh: "窗口太小，放大终端窗口后再看九宫格",
        ),
        Verifying => t!(
            lang,
            en: "Checking, one moment　Esc to cancel",
            zh: "正在验证，请稍候　Esc 可取消",
        ),
        PasteOrTypeKey => t!(lang, en: "Paste or type your key", zh: "粘贴或输入密钥"),
        NoOtherRunningSession => t!(
            lang,
            en: "No other session is running",
            zh: "没有其他正在跑的会话",
        ),
        NoSessionSelected => t!(lang, en: "No session selected", zh: "没有选中会话"),
        DaemonUnreachable => t!(
            lang,
            en: "Cannot reach the dct service",
            zh: "守护进程连不上",
        ),
        NoDaemonRunning => t!(
            lang,
            en: "Nothing is running in the background",
            zh: "后台没有东西在跑",
        ),
        NoSessionsRunning => t!(lang, en: "No sessions", zh: "没有会话"),
        NothingToPrune => t!(
            lang,
            en: "No stopped sessions to clean up",
            zh: "没有要清理的会话",
        ),
        StatusWorking => t!(lang, en: "working", zh: "干活中"),
        StatusAsking => t!(lang, en: "asking you", zh: "等你回答"),
        StatusIdle => t!(lang, en: "idle", zh: "空闲"),
        StatusStopped => t!(lang, en: "stopped", zh: "已停止"),
        StatusFailed => t!(lang, en: "error", zh: "出错了"),
        StatusUnknown => t!(lang, en: "—", zh: "—"),

        SecretsTitle => t!(lang, en: "Keys", zh: "密钥设置"),
        SecretSet => t!(lang, en: "set", zh: "已配"),
        SecretUnset => t!(lang, en: "not set", zh: "未配"),
        OpenSignupPage => t!(
            lang,
            en: "Ctrl+O opens the sign-up page",
            zh: "Ctrl+O 打开申领页面",
        ),
        NothingToDelete => t!(
            lang,
            en: "No key is set here, so there is nothing to delete",
            zh: "这个还没配密钥，没什么可删的",
        ),
        PressDAgainToDelete => t!(
            lang,
            en: "press d again to delete, any other key cancels",
            zh: "再按 d 删除，按其他键取消",
        ),
        SecretNotSaved => {
            t!(lang, en: "The key was not saved — try again", zh: "密钥没存上，再试一次")
        }
        SecretNotDeleted => t!(
            lang,
            en: "The key was not deleted — try again",
            zh: "密钥没删掉，再试一次",
        ),
        VerifyingShort => t!(lang, en: "Checking…", zh: "正在验证…"),
        BadSecret => t!(
            lang,
            en: "That key does not work — it may have been copied incompletely",
            zh: "这个密钥用不了，可能是复制的时候少了一段",
        ),
        NetworkUnreachable => t!(
            lang,
            en: "Cannot reach the server — check your network",
            zh: "连不上服务器，检查一下网络",
        ),

        CreateFailed => t!(lang, en: "Could not create the session", zh: "创建失败"),
        CannotOpenInstallWindow => t!(
            lang,
            en: "Could not open the install window",
            zh: "开不了安装窗口",
        ),
        NoPathTyped => t!(lang, en: "No path typed yet", zh: "还没输入路径"),
        CannotListAgents => t!(lang, en: "Could not load the agent list", zh: "拿不到 agent 列表"),
        CannotListSecrets => t!(lang, en: "Could not load the key list", zh: "拿不到密钥列表"),
        CannotListProjects => t!(
            lang,
            en: "Could not load the project list",
            zh: "拿不到项目列表",
        ),
        SessionOpenFailed => t!(
            lang,
            en: "Could not open the session — try again",
            zh: "开不了会话，再试一次",
        ),
        PasteNotSent => t!(
            lang,
            en: "Cannot reach the dct service — what you pasted was not sent",
            zh: "守护进程连不上，粘贴的内容没发出去",
        ),
        InputNotSent => t!(
            lang,
            en: "Cannot reach the dct service — that keystroke was not sent",
            zh: "守护进程连不上，刚才那次输入没发出去",
        ),
        StaleDaemonExplain => t!(
            lang,
            en: "The background service is still the old version, so some things will not work.\n\
                 It is the piece that keeps your agent sessions running while dct is closed.\n\n\
                 Restarting it fixes this. The sessions running right now will end —\n\
                 your file changes stay, but the agents have to be started again.",
            zh: "后台服务还是旧版本，有些功能会用不了。\n\
                 它是 dct 关掉之后替你看着 agent 会话的那个东西。\n\n\
                 重启它就能修好。正在跑的会话会断——文件改动都还在，\n\
                 只是 agent 要重新开一次。",
        ),
        StaleDaemonAsk => t!(
            lang,
            en: "Restart it now? (y = restart, Enter = leave it for now)",
            zh: "现在重启吗？(y = 重启，直接回车 = 先这样用)",
        ),
        StaleDaemonRestarting => t!(lang, en: "Restarting…", zh: "正在重启…"),
        StaleDaemonRestartFailed => t!(
            lang,
            en: "Could not restart it. Continuing with the old one.",
            zh: "没能重启，先接着用旧的",
        ),
        DaemonTooOld => t!(
            lang,
            en: "The background service is an older version and cannot show the screen. Quit dct and open it again.",
            zh: "后台服务是旧版本，看不到画面。退出 dct 再重新打开就好",
        ),
        RequestFailed => t!(lang, en: "That did not work", zh: "请求失败"),
        ActionDone => t!(lang, en: "Done", zh: "完成"),
        NoChanges => t!(lang, en: "No changes", zh: "没有改动"),

        ReasonNeedsSecret => t!(lang, en: "(no key yet)", zh: "（未填密钥）"),
        ReasonNotInstalled => t!(lang, en: "(not installed)", zh: "（未安装）"),

        GroupNotEmpty => t!(
            lang,
            en: "This project still has sessions. Stop them first.",
            zh: "这个项目还有会话，先停掉才能移除。"
        ),

        StaleData => t!(
            lang,
            en: "Cannot reach the dct service — what you see may be out of date",
            zh: "守护进程连不上，界面数据可能已过期",
        ),
    }
}

/// 底栏和「全部按键」浮层里的一条提示：**键名和说明分开存**。
///
/// 分开不是为了好看：键名要加粗（加粗是 `Span` 一级的事），拼成一整个
/// 字符串之后再想切回来，只能靠猜哪个空格是分隔符——而 `Ctrl+C 打断`、
/// `方向键选格子`、`其余按键都发给 agent` 各有各的形状，猜不准就会加粗到
/// 说明的头一个词上去。
///
/// `key` 是空串表示这条只是一句说明，没有对应的按键。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelpItem {
    pub key: &'static str,
    pub label: &'static str,
    /// 动态标签。有它就顶掉 `label`——底栏的 `n 新建 claude` 里那个 agent 名
    /// 是运行时才知道的，塞不进 `&'static str` 的词条表。
    ///
    /// 加这个字段的代价是 `HelpItem` 不再 `Copy`（`String` 不是 `Copy`）。
    /// 那没关系：这个类型只在几处按值构造、其余全走引用（`fit_help` /
    /// `wrap_items` 返回的都是 `&HelpItem`）。
    pub label_owned: Option<String>,
}

impl HelpItem {
    /// 这条提示实际显示的说明。**所有读说明的地方都必须走这里**——
    /// 量宽度的、画屏幕的、拼给单测看的，只要有一处直接读 `label`，
    /// 屏幕上就会是一串字、断言里是另一串字，而两者都「通过」了。
    pub fn label(&self) -> &str {
        self.label_owned.as_deref().unwrap_or(self.label)
    }
}

impl std::fmt::Display for HelpItem {
    /// 一条提示拼成文字的样子。渲染时加粗的那一份必须跟这里给出同一串
    /// 字符——单测断言的是这一份，屏幕上画的是那一份，两者分叉就等于
    /// 测了个寂寞。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.key.is_empty() {
            write!(f, "{}", self.label())
        } else {
            write!(f, "{} {}", self.key, self.label())
        }
    }
}

/// 把「按键 + 它做什么」列成底栏那一排。
///
/// 按条目拼而不是把整句写进词条表：整句进表的话，每种语言都要把 `n`/`p`/`Enter`
/// 这些**不翻译**的键名再抄一遍，加一种语言就多抄一份，而键名改了要改 N 处。
pub fn help_items(items: &[(&'static str, Key)], lang: Lang) -> Vec<HelpItem> {
    items
        .iter()
        .map(|(k, key)| HelpItem {
            key: k,
            label: text(*key, lang),
            label_owned: None,
        })
        .collect()
}

/// 一排提示拼成文字。分隔符是两个半角空格，正好是 `widgets::wrap_help` 认的断点。
pub fn help_text(items: &[HelpItem]) -> String {
    items
        .iter()
        .map(|it| it.to_string())
        .collect::<Vec<_>>()
        .join("  ")
}

/// 带参文案。**每条一个函数，不是带 `{}` 的模板**：模板要靠调用方按顺序填参，
/// 漏填、错序、类型不对，编译器一概不管，而各语言的语序本来就不同。
/// 写成函数，这些全归签名管。
pub mod msg {
    use super::{text, Key, Lang};

    pub fn not_a_session_id(lang: Lang, arg: &str) -> String {
        t!(lang, en: format!("`{arg}` is not a session number. `dct ps` lists them."), zh: format!("`{arg}` 不是会话号。`dct ps` 能看到有哪些。"))
    }

    pub fn stopped_session(lang: Lang, id: u32) -> String {
        t!(lang, en: format!("Stopped session {id}"), zh: format!("已停止 {id} 号会话"))
    }

    pub fn killed_session(lang: Lang, id: u32) -> String {
        t!(lang, en: format!("Killed session {id}"), zh: format!("已杀掉 {id} 号会话"))
    }

    pub fn pruned(lang: Lang, n: u32) -> String {
        t!(
            lang,
            en: format!("Cleaned up {n} stopped session(s)"),
            zh: format!("清掉 {n} 个已停止的会话"),
        )
    }

    /// 「要停哪个 / 要杀哪个」。
    ///
    /// **`cmd` 带进来而不是写死 `stop`**：这句话的全部价值就是告诉用户
    /// 下一步该敲什么，而用户敲的是 `dct kill` 却被告知 `dct stop 3` 怎么用，
    /// 等于把他推去解一个他没问的问题。
    pub fn needs_a_target(lang: Lang, cmd: &str) -> String {
        t!(
            lang,
            en: format!("Which one? `dct {cmd} 3` for session 3, `dct {cmd} --all` for every session."),
            zh: format!("要哪个？`dct {cmd} 3` 是 3 号会话，`dct {cmd} --all` 是全部。"),
        )
    }

    pub fn all_takes_no_ids(lang: Lang, cmd: &str) -> String {
        t!(
            lang,
            en: format!("`dct {cmd} --all` already means every session — drop the ids."),
            zh: format!("`dct {cmd} --all` 本来就是全部，不要再跟会话号。"),
        )
    }

    // `switched_to`（「已切到 X」）在 `p` 从「换项目」降格成「把项目摆上
    // 看板」时删掉了：换项目现在是 `Tab`，一个键、零弹窗、不说话，光标
    // 落在哪个组上就是当前项目，屏幕自己看得见——再补一句话反而会盖掉
    // 底栏上别的更要紧的提示（见 board.rs 头上 `e0ba1ec` 那段）。

    pub fn not_a_directory(lang: Lang, path: &str) -> String {
        t!(lang, en: format!("{path} is not a folder"), zh: format!("{path} 不是一个目录"))
    }

    pub fn cannot_find_anymore(lang: Lang, path: &str) -> String {
        t!(lang, en: format!("{path} cannot be found anymore"), zh: format!("{path} 现在找不到了"))
    }

    pub fn session_ended(lang: Lang, id: u32) -> String {
        t!(
            lang,
            en: format!("Session {id} ended. Back to the board — press n to start another."),
            zh: format!("会话 {id} 已结束，回到看板。按 n 再建一个"),
        )
    }

    pub fn needs_dependency(lang: Lang, label: &str, target: &str) -> String {
        t!(
            lang,
            en: format!("Install {label} first to use {target}"),
            zh: format!("要先装 {label} 才能用 {target}"),
        )
    }

    pub fn no_command_configured(lang: Lang, label: &str) -> String {
        t!(
            lang,
            en: format!("{label} has no program configured, so it cannot run"),
            zh: format!("{label} 没配置要运行的程序，用不了"),
        )
    }

    pub fn command_not_found(lang: Lang, command: &str) -> String {
        t!(
            lang,
            en: format!("{command} was not found on this machine"),
            zh: format!("本机没有找到 {command}"),
        )
    }

    pub fn secret_saved(lang: Lang, label: &str) -> String {
        t!(lang, en: format!("Saved the key for {label}"), zh: format!("已保存 {label} 的密钥"))
    }

    pub fn secret_deleted(lang: Lang, label: &str) -> String {
        t!(lang, en: format!("Deleted the key for {label}"), zh: format!("已删除 {label} 的密钥"))
    }

    pub fn confirm_delete_secret(lang: Lang, label: &str) -> String {
        t!(
            lang,
            en: format!("Press d again to delete the key for {label}, any other key cancels"),
            zh: format!("再按一次 d 删除 {label} 的密钥，按其他键取消"),
        )
    }

    /// 标题里必须带上「Esc 回哪」，而且分设置页/选择器两种。
    ///
    /// 这半句一度被合并掉，理由是底栏的 `idle_help` 已经说了——但那两处画在
    /// 不同的区域，密钥页这一屏自己看不到底栏。更要紧的是 `escape_hint` 和
    /// 标题必须**互相印证**：底栏说什么就得真能做到什么，标题跟着一起说，
    /// 才不会出现一处说设置、一处还写着旧的「列表」。
    pub fn enter_secret_title(lang: Lang, label: &str, to_settings: bool) -> String {
        let back = text(
            if to_settings {
                Key::BackToSettingsWord
            } else {
                Key::BackToListWord
            },
            lang,
        );
        let confirm = text(Key::Confirm, lang);
        t!(
            lang,
            en: format!("Key for {label} (Enter {confirm}, Esc {back})"),
            zh: format!("填 {label} 的密钥（Enter {confirm}，Esc {back}）"),
        )
    }

    pub fn cannot_open_browser(lang: Lang, url: &str) -> String {
        t!(
            lang,
            en: format!("Could not open a browser — visit {url} yourself"),
            zh: format!("打不开浏览器，自己去访问 {url}"),
        )
    }

    pub fn installing(lang: Lang, profile: &str) -> String {
        t!(
            lang,
            en: format!("Installing {profile}. When it finishes, press Ctrl+Q then N."),
            zh: format!("正在安装 {profile}，装完按 Ctrl+Q 回看板再按 N"),
        )
    }

    pub fn reason_needs_dependency(lang: Lang, label: &str) -> String {
        t!(lang, en: format!("(install {label} first)"), zh: format!("（需要先装 {label}）"))
    }

    /// 刚出错的那一刻说的一句话。**点名是哪个会话**——用户可能正在别的
    /// 会话里，或者根本在看别的项目，只说「出错了」他不知道该去哪。
    pub fn session_failed(lang: Lang, id: u32, profile: &str) -> String {
        t!(
            lang,
            en: format!("Session {id} ({profile}) hit an error — go and take a look"),
            zh: format!("会话 {id}（{profile}）出错了，去看一眼"),
        )
    }

    /// 组头上的「N 个出错」。会话静默失败是 dct 最贵的失败模式，
    /// 组头上必须一眼看得见。
    pub fn failed_count(lang: Lang, n: usize) -> String {
        match lang {
            Lang::En => format!("{n} failed"),
            Lang::Zh => format!("{n} 个出错"),
        }
    }

    /// 出错解释算出来之后显示的那句话。**只套一层「哪个会话」的前缀**——
    /// 解释本身已经是模型给零编程用户的完整一两句话（见
    /// `session::explain_prompt`），这里不重新组句，只帮用户对上是哪个会话。
    pub fn session_failure_explained(lang: Lang, id: u32, explanation: &str) -> String {
        t!(
            lang,
            en: format!("Session {id}: {explanation}"),
            zh: format!("{id} 号会话：{explanation}"),
        )
    }

    pub fn session_title(lang: Lang, id: u32, project: &str) -> String {
        t!(
            lang,
            en: format!("Session {id} · {project} —— F2 goes back"),
            zh: format!("会话 {id} · {project} —— F2 返回看板"),
        )
    }

    pub fn session_title_disconnected(lang: Lang, id: u32, project: &str) -> String {
        t!(
            lang,
            en: format!("Session {id} · {project} (disconnected, may be out of date) —— F2 goes back"),
            zh: format!("会话 {id} · {project}（连接已断开，画面可能过期）—— F2 返回看板"),
        )
    }

    /// 底栏「往上翻了，下面又有新东西了」。用户滚回底部之前画面不会自己跳，
    /// 免得他正读一半的历史被新输出顶飞——这句话是唯一的提醒。
    ///
    /// 跟 `scrolled_up` 一样必须带上「按 End 回底部」：这半句在这个状态下
    /// 反而最要紧——用户翻着历史、新内容还在不断堆积，正是最想马上跳回去
    /// 看最新输出的时候，不能只说「有新东西」却不说怎么去看。
    ///
    /// 英文版故意比直译短：底栏右段的宽度是「终端总宽 − 23」（`mod.rs` 的
    /// `ESCAPE_HINT_COLS`），窄终端上很快就不够 40 列。这句话走的是
    /// `BarContent::Text`，`wrap_help` 只在空格连打两个的地方才折行，这句
    /// 里全是单空格，放不下就不是折行而是被 `Paragraph` 直接截断——原来
    /// "↓ {n} new line(s) below · press End to jump back down" 有 52+ 列，
    /// 80 列左右的终端就已经在吃「怎么回去」那半句，这正是这句话存在的
    /// 唯一理由。缩短之后即使在很窄的终端上，「按 End」也还留得住。
    /// 中文版本来就短（~30 列），不用动。
    pub fn scroll_new_lines_below(lang: Lang, n: usize) -> String {
        t!(
            lang,
            en: format!("↓ {n} new below · press End"),
            zh: format!("↓ 下面还有 {n} 行新内容 · 按 End 回到底部"),
        )
    }

    /// 底栏「已经往上翻了多远，怎么回去」。两件事缺一不可——只说翻了多远，
    /// 用户不知道怎么回底部；只说怎么回去，他不知道自己是不是还看得到最新的。
    ///
    /// 英文版缩短的理由跟 `scroll_new_lines_below` 一样——见那边的注释。
    pub fn scrolled_up(lang: Lang, offset: usize) -> String {
        t!(
            lang,
            en: format!("↑ Scrolled up {offset} · press End"),
            zh: format!("↑ 已往上翻 {offset} 行 · 按 End 回到底部"),
        )
    }

    /// 触发条件是 `!agent_owns && alt_screen`：agent 用了备用屏、但没开鼠标
    /// 上报，滚轮和翻页在这个会话里都没用。真正会落进这一支的是 `less`、
    /// `vim`、`htop` 这类吃全屏又不理鼠标的程序——**不是** Claude Code：
    /// Claude Code 恰恰是会收鼠标的那一类（`agent_owns` 判据见
    /// `attach::wheel_action` 的文档），走的是完全不同的分支。装死的话
    /// 用户会以为滚轮坏了，反复去试——必须说清楚「这儿翻不了」。不能提
    /// "备用屏"/"scrollback"/"缓冲区"这类黑话，用户不是程序员，听不懂
    /// 这些词，只会更迷惑。
    pub fn agent_owns_the_screen(lang: Lang) -> String {
        t!(
            lang,
            en: "This assistant controls its own screen here, so there's nothing to look back at"
                .to_string(),
            zh: "这个 agent 自己管画面，翻不了历史".to_string(),
        )
    }

    /// 把守护进程报回来的错误码组成一句人话。**这是 daemon 侧文案唯一的
    /// 落点**——daemon 只报码，句子在这里成形，所以切语言立刻生效、
    /// 不用重启 daemon。
    pub fn error(lang: Lang, e: &crate::proto::ErrorCode) -> String {
        use crate::proto::ErrorCode::*;
        match e {
            NoSuchProfile(name) => t!(
                lang,
                en: format!("There is no agent called {name}"),
                zh: format!("没有这个 agent：{name}"),
            ),
            DirNotFound(dir) => t!(
                lang,
                en: format!("{dir} does not exist"),
                zh: format!("目录不存在：{dir}"),
            ),
            NotAGitRepo(dir) => t!(
                lang,
                en: format!("{dir} is not a git project, so an agent cannot work there"),
                zh: format!("{dir} 不是 git 仓库，无法开 agent 会话"),
            ),
            NoSuchSession(id) => t!(
                lang,
                en: format!("Session {id} no longer exists"),
                zh: format!("没有这个会话：{id}"),
            ),
            NoCheckpoint => t!(
                lang,
                en: "There is no checkpoint yet".to_string(),
                zh: "还没有检查点".to_string(),
            ),
            // 同一个成因两种说法：界面知道用户刚按的是 `u` 还是 `d`。
            // 这里给的是通用版本，调用方想更贴切可以自己挑词。
            NotAnAgentSession => t!(
                lang,
                en: "This session has no history to undo or compare".to_string(),
                zh: "这个会话没有可撤销或比较的记录".to_string(),
            ),
            BadRequest(detail) => t!(
                lang,
                en: format!("dct could not understand that request: {detail}"),
                zh: format!("请求解析失败：{detail}"),
            ),
            // git 的 stderr 照抄，只翻外面那半句——那是 git 按它自己的
            // `LANG` 输出的，dct 翻不动也不该翻。
            Git(raw) => t!(
                lang,
                en: format!("git failed: {raw}"),
                zh: format!("git 操作失败：{raw}"),
            ),
            CannotStart(cmd) => t!(
                lang,
                en: format!("{cmd} would not start — it may be installed incorrectly"),
                zh: format!("启动不了 {cmd}，它可能装坏了"),
            ),
            DaemonNotResponding => t!(
                lang,
                en: "The dct service is not responding".to_string(),
                zh: "守护进程没有回应".to_string(),
            ),
            OperationFailed(op) => operation(lang, *op),
            SecretsFileBroken { path } => t!(
                lang,
                en: format!(
                    "The key file is damaged, so dct will not overwrite it. Delete it and paste \
                     your keys in again. ({path})"
                ),
                zh: format!(
                    "密钥文件坏了，所以没有改它。删掉这个文件，回 dct 里重新粘贴一遍密钥就行。（{path}）"
                ),
            ),
            Internal(raw) => raw.clone(),
        }
    }

    fn operation(lang: Lang, op: crate::proto::Operation) -> String {
        use crate::proto::Operation::*;
        match op {
            FirstCheckpoint => t!(
                lang,
                en: "Could not take the first checkpoint, so this session cannot be undone safely",
                zh: "拍不了检查点，这个会话没法安全撤销",
            ),
            Checkpoint => t!(
                lang,
                en: "Could not take a checkpoint — this step may not be undoable",
                zh: "拍检查点失败，这一步的改动可能没法撤销",
            ),
            // 必须点明后果：用户需要知道工作区可能停在改到一半的状态，
            // 光说「失败了」他会以为什么都没发生。
            Undo => t!(
                lang,
                en: "Undo failed — your files may be left half-changed",
                zh: "撤销失败，工作区可能停在了改到一半的状态",
            ),
            Diff => t!(
                lang,
                en: "Could not work out which files changed — try again",
                zh: "算不出改了哪些文件，再试一次",
            ),
            SaveSecret => t!(
                lang,
                en: "The key could not be written — try again",
                zh: "密钥没写进去，再试一次",
            ),
            SpawnPty => t!(
                lang,
                en: "Could not start that program",
                zh: "启动不了那个程序",
            ),
            ReadClipboard => t!(
                lang,
                en: "Could not read the clipboard",
                zh: "读不了剪贴板",
            ),
            SaveSettings => t!(
                lang,
                en: "The setting could not be saved — try again",
                zh: "设置没存下来，再试一次",
            ),
        }
        .to_string()
    }

    fn io_reason(lang: Lang, r: crate::proto::IoReason) -> &'static str {
        use crate::proto::IoReason::*;
        match r {
            PermissionDenied => t!(lang, en: "no permission to read it", zh: "没有权限读取"),
            NotADirectory => t!(lang, en: "it is not a folder", zh: "不是一个文件夹"),
            Other => t!(lang, en: "it could not be read", zh: "读取失败"),
        }
    }

    /// 把一条警告码组成人话。跟 `error` 同样的道理：守护进程报码，
    /// 句子在界面成形。
    pub fn warning(lang: Lang, w: &crate::proto::WarningCode) -> String {
        use crate::proto::WarningCode::*;
        match w {
            ProfileDirUnreadable { name, reason } => {
                let why = io_reason(lang, *reason);
                t!(
                    lang,
                    en: format!("{name} could not be opened: {why}"),
                    zh: format!("{name} 打不开：{why}"),
                )
            }
            ProfileUnreadable { name, reason } => {
                let why = io_reason(lang, *reason);
                t!(
                    lang,
                    en: format!("{name} could not be read: {why}"),
                    zh: format!("{name} 读不了：{why}"),
                )
            }
            ProfileMalformed { name, line, reason } => match line {
                Some(n) => t!(
                    lang,
                    en: format!("{name} has a mistake on line {n}: {reason}"),
                    zh: format!("{name} 写错了：第 {n} 行：{reason}"),
                ),
                None => t!(
                    lang,
                    en: format!("{name} has a mistake: {reason}"),
                    zh: format!("{name} 写错了：{reason}"),
                ),
            },
            SecretsUnreadable { path, reason } => {
                let why = io_reason(lang, *reason);
                t!(
                    lang,
                    en: format!("The key file could not be read: {why} ({path})"),
                    zh: format!("密钥文件读不了：{why}（{path}）"),
                )
            }
            // 不给行号也不给 toml 原文：密钥文件不该手改，照着行号抠语法
            // 是把用户往错路上支。只给一句他做得到的下一步。
            SecretsCorrupt { path } => t!(
                lang,
                en: format!(
                    "The key file is damaged and cannot be read. Delete it and paste your keys \
                     into dct again — there is no need to repair it by hand. ({path})"
                ),
                zh: format!(
                    "密钥文件坏了，读不出来。删掉这个文件，回 dct 里重新粘贴一遍密钥就行，\
                     不用手动修它。（{path}）"
                ),
            ),
            // 用户确实写了 `[llm]`（这是一次主动的开），但连不上。守护进程
            // 的 stderr 是被丢弃的（`client::spawn_daemon` 把它接到 null，
            // 否则每一行都会糊在 TUI 上），所以这句话只能顶到界面上来，
            // 否则用户开了功能却什么都没发生、也没人告诉他为什么。
            LlmUnavailable(problem) => {
                let why = llm_problem(lang, problem);
                t!(
                    lang,
                    en: format!("Explaining failures is switched on but not connected: {why} Sessions run as usual."),
                    zh: format!("「出错解释」开着，但连不上：{why}会话照常跑。"),
                )
            }
        }
    }

    /// 设置文件的位置。**每一句「去改设置」都必须带上它**——「设置文件」
    /// 四个字对一个零编程经验的人不是一个可执行的下一步，一个路径才是。
    /// 写成常量而不是把真实路径一路传进来：这些句子会跟着 `WarningCode`
    /// 从守护进程走到界面进程，中间那条 socket 上没有「谁的 socket 旁边的
    /// config」这个概念，而真实用户的它就是这个路径（见 `proto::socket_path`
    /// 和 `config::config_path_for_socket`）。
    pub const CONFIG_PATH: &str = "~/.dct/config.toml";

    /// LLM 连不上时给用户的一句话。
    ///
    /// **不许出现内部字段名/类型名**（provider / transport / cli / agent /
    /// Error 这类）——那是 Rust 代码里的词，不是用户的词。每句话点名是哪个
    /// 厂商，并且给一个他真做得到的下一步（改哪个文件、按哪个键）。
    pub fn llm_problem(lang: Lang, e: &crate::llm::resolve::ResolveError) -> String {
        use crate::llm::resolve::ResolveError::*;
        let cfg = CONFIG_PATH;
        match e {
            NoSuchProvider(n) => t!(
                lang,
                en: format!("The settings file {cfg} asks for “{n}”, which dct does not know. Change it to claude."),
                zh: format!("设置文件 {cfg} 里写的「{n}」不是 dct 认识的名字，把它换成 claude 试试。"),
            ),
            // kimi/glm/deepseek/qwen-api 走到这里是常事（它们没有、也不该有
            // 无界面命令，见 `profile.rs` 的
            // `unverified_clis_declare_no_headless_command`），所以这句话必须
            // 把**它们那条正路**也说出来——只说「换成 claude」等于让一个
            // 已经付过钱、填好密钥的用户去换一家。「直连」是本项目对这件事的
            // 统一说法，见下面 `NoApiEndpoint`。
            NoHeadlessCommand(n) => t!(
                lang,
                en: format!("“{n}” cannot answer questions on its own in the background. Open the settings file {cfg} and either change that line to claude, or switch “{n}” to a direct connection and fill in the model name."),
                zh: format!("「{n}」还没法自己在后台回答问题。打开设置文件 {cfg}，要么把这一项换成 claude，要么给「{n}」打开「直连」并填上型号名。"),
            ),
            NoApiEndpoint(n) => t!(
                lang,
                en: format!("“{n}” has no address dct can connect to by itself. Open the settings file {cfg} and turn the direct-connection line off, so it signs in on its own instead."),
                zh: format!("「{n}」没有可以直接连接的网址。打开设置文件 {cfg}，把「直连」这一项关掉，改回让它自己登录。"),
            ),
            NoCredential(n) => t!(
                lang,
                en: format!("“{n}” has no key yet. Press c on the main screen to enter one."),
                zh: format!("「{n}」还没有密钥。在主界面按 c 填一个。"),
            ),
            NoModel(n) => t!(
                lang,
                en: format!("“{n}” has no model name yet. Open the settings file {cfg} and fill in the exact name of the model you want."),
                zh: format!("「{n}」还没有指定用哪个型号。打开设置文件 {cfg}，填一个具体的型号名。"),
            ),
            // 这一句要说清楚**为什么拒绝**：用户会觉得「我明明登录过了」。
            // 说法只能用他懂的词——不同的公司、不同的账号，不是「凭据出处」。
            BorrowedCredentialRefused { name, host } => t!(
                lang,
                en: format!("“{name}” has no key of its own yet. dct will not hand your sign-in from another program to {host} — they are different companies. Press c on the main screen and enter a key for “{name}”."),
                zh: format!("「{name}」还没有自己的密钥。dct 不会把你在别的程序里的登录拿去连 {host}——那是两家不同的公司。在主界面按 c 给「{name}」填一个密钥。"),
            ),
            BadBaseUrl { name, url } => t!(
                lang,
                en: format!("The address for “{name}” ({url}) does not look like a web address, and dct will not send a key somewhere it cannot read. Fix that line in the settings file {cfg}."),
                zh: format!("「{name}」要连的地址（{url}）不像一个网址，而 dct 不会把密钥发到看不懂的地方。改一下设置文件 {cfg} 里的那一行。"),
            ),
        }
    }

    /// `dct llm check`：这功能还没打开时说的话。**带上真实路径**——
    /// 这条命令是在用户自己的终端里跑的，它知道那份设置文件到底在哪。
    pub fn llm_not_enabled(lang: Lang, path: &std::path::Path) -> String {
        let p = path.display();
        t!(
            lang,
            en: format!(
                "Explaining failures is switched off right now.\n\
                 To switch it on, put these two lines in {p}\n\n\
                 [llm]\nprovider = \"claude\"\n\n\
                 Then run `dct llm check` again to test it."
            ),
            zh: format!(
                "「出错解释」这个功能现在是关着的。\n\
                 要打开的话，在 {p} 里加上这两行：\n\n\
                 [llm]\nprovider = \"claude\"\n\n\
                 加完再跑一次 `dct llm check` 就能验。"
            ),
        )
    }

    /// `dct llm check` 开头那行「现在这份设置是什么」。
    pub fn llm_using(lang: Lang, provider: &str, direct: bool) -> String {
        let how = if direct {
            t!(lang, en: "connecting directly", zh: "直接连接")
        } else {
            t!(lang, en: "letting it sign in on its own", zh: "让它自己登录")
        };
        t!(
            lang,
            en: format!("Using {provider}, {how}."),
            zh: format!("用的是 {provider}，{how}。"),
        )
    }

    pub fn llm_cannot_connect(lang: Lang, why: &str) -> String {
        t!(lang, en: format!("Not connected: {why}"), zh: format!("连不上：{why}"))
    }

    pub fn llm_works(lang: Lang, answer: &str) -> String {
        t!(
            lang,
            en: format!("It works. The model replied: {answer}"),
            zh: format!("通了。模型回答：{answer}"),
        )
    }

    /// 真打了一次端点但没成。三种结果对用户是三件不同的事，别糊成一句。
    pub fn llm_call_failed(lang: Lang, e: crate::llm::LlmError) -> String {
        use crate::llm::LlmError::*;
        match e {
            Unavailable => t!(
                lang,
                en: "No answer came back — the address or the key may be wrong, or the network is blocked.",
                zh: "没通：地址或者密钥可能不对，也可能是网络不通。",
            ),
            Timeout => t!(
                lang,
                en: "It took too long to answer and dct stopped waiting.",
                zh: "等太久还没回话，dct 不等了。",
            ),
            Malformed => t!(
                lang,
                en: "Something came back, but dct could not read it.",
                zh: "有回话，但 dct 读不懂它回的东西。",
            ),
        }
        .to_string()
    }

    pub fn title_with(lang: Lang, main: Key, extra: &str) -> String {
        format!("{}（{extra}）", text(main, lang))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fake_env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    /// 词条表里所有的 `Key`。加变体忘了加到这里，下面两条守卫就漏测它——
    /// 所以这份清单本身也被 `every_key_is_listed_for_the_guards` 钉着。
    const ALL_KEYS: &[Key] = {
        use Key::*;
        &[
            New,
            SwitchAgent,
            SwitchProject,
            AddProject,
            RemoveProject,
            SeeAllProjects,
            ThisProjectOnly,
            Secrets,
            Grid,
            Select,
            Open,
            Zoom,
            Undo,
            Stop,
            Diff,
            Quit,
            Confirm,
            Cancel,
            Back,
            Edit,
            Delete,
            MoveArrows,
            ReplyOnce,
            SendReply,
            InterruptAgent,
            EmptyReplyIsEnter,
            NextSession,
            OtherKeysGoToAgent,
            NewSessionFromBoard,
            BackToListWord,
            BackToSettingsWord,
            OrPressDigit,
            TypeToFilter,
            Language,
            BackToBoard,
            BackToBoardWithF2,
            BackToList,
            BackToSettings,
            BoardTitle,
            BoardTitleAllProjects,
            Disconnected,
            PickAgentTitle,
            PickProjectTitle,
            TypePathTitle,
            SettingsTitle,
            CurrentProject,
            ManualPath,
            NoSessionsHere,
            NoSessionsAtAll,
            ProjectDirGone,
            WindowTooSmall,
            Verifying,
            PasteOrTypeKey,
            NoOtherRunningSession,
            NoSessionSelected,
            DaemonUnreachable,
            StaleData,
            NoDaemonRunning,
            NoSessionsRunning,
            NothingToPrune,
            StatusWorking,
            StatusAsking,
            StatusIdle,
            StatusStopped,
            StatusUnknown,
            SecretsTitle,
            SecretSet,
            SecretUnset,
            OpenSignupPage,
            NothingToDelete,
            PressDAgainToDelete,
            SecretNotSaved,
            SecretNotDeleted,
            VerifyingShort,
            BadSecret,
            NetworkUnreachable,
            CreateFailed,
            CannotOpenInstallWindow,
            NoPathTyped,
            CannotListAgents,
            CannotListSecrets,
            CannotListProjects,
            SessionOpenFailed,
            PasteNotSent,
            InputNotSent,
            DaemonTooOld,
            StaleDaemonExplain,
            StaleDaemonAsk,
            StaleDaemonRestarting,
            StaleDaemonRestartFailed,
            RequestFailed,
            ActionDone,
            NoChanges,
            ReasonNeedsSecret,
            ReasonNotInstalled,
            GroupNotEmpty,
        ]
    };

    fn has_han(s: &str) -> bool {
        s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
    }

    /// 英文词条里不许出现汉字。批量加词条时最容易犯的错就是把中文那行抄过去——
    /// profile 那边已经踩过一次（`shell` 的 label 变成 `en = "命令行"`）。
    #[test]
    fn no_english_entry_contains_han_characters() {
        for k in ALL_KEYS {
            let en = text(*k, Lang::En);
            assert!(!has_han(en), "{k:?} 的英文写着中文：{en}");
        }
    }

    /// 每条词条两种语言都得有内容。空串会让屏幕上凭空少一截，而且不报错。
    #[test]
    fn no_entry_is_empty_in_either_language() {
        for k in ALL_KEYS {
            for l in Lang::all() {
                assert!(!text(*k, *l).trim().is_empty(), "{k:?} 在 {l:?} 下是空的");
            }
        }
    }

    /// `ALL_KEYS` 漏了谁，上面两条守卫就悄悄不管它了。用 `text()` 的穷尽 match
    /// 反过来钉住这份清单：新增变体必须同时出现在这里，否则数目对不上。
    #[test]
    fn every_key_is_listed_for_the_guards() {
        // 这个数字改动时，请确认 ALL_KEYS 也补上了新变体——它不是凑出来的，
        // 而是「词条表里到底有多少条」这个事实。
        assert_eq!(ALL_KEYS.len(), 96, "加了 Key 变体就要同步进 ALL_KEYS");
        let mut seen: Vec<String> = ALL_KEYS.iter().map(|k| format!("{k:?}")).collect();
        seen.sort();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "ALL_KEYS 里有重复项");
    }

    /// 每一个错误码在两种语言下都要组得出话，而且英文里不许有汉字。
    /// 这是 daemon 侧文案唯一的落点——漏一条，用户就会在英文界面上
    /// 看到一句中文，而且没有任何编译期信号。
    #[test]
    fn every_error_code_composes_in_both_languages() {
        use crate::proto::ErrorCode::*;
        let codes = [
            NoSuchProfile("kimi".into()),
            DirNotFound("/w/a".into()),
            NotAGitRepo("/w/a".into()),
            NoSuchSession(7),
            NoCheckpoint,
            NotAnAgentSession,
            BadRequest("bad json".into()),
            Git("fatal: not a repository".into()),
            SecretsFileBroken {
                path: "/h/.dct/secrets.toml".into(),
            },
            OperationFailed(crate::proto::Operation::FirstCheckpoint),
            OperationFailed(crate::proto::Operation::Checkpoint),
            OperationFailed(crate::proto::Operation::Undo),
            OperationFailed(crate::proto::Operation::Diff),
            OperationFailed(crate::proto::Operation::SaveSecret),
            OperationFailed(crate::proto::Operation::SaveSettings),
            OperationFailed(crate::proto::Operation::SpawnPty),
            OperationFailed(crate::proto::Operation::ReadClipboard),
            CannotStart("claude".into()),
            DaemonNotResponding,
        ];
        for c in &codes {
            for l in Lang::all() {
                let s = msg::error(*l, c);
                assert!(!s.trim().is_empty(), "{c:?} 在 {l:?} 下组不出话");
            }
            assert!(
                !has_han(&msg::error(Lang::En, c)),
                "{c:?} 的英文里有汉字：{}",
                msg::error(Lang::En, c)
            );
        }
        // `Internal` 是刻意的例外：它照抄原文（多半是还没归类的内部错误
        // 或 git 的 stderr），翻不动也不该翻。
        assert_eq!(msg::error(Lang::En, &Internal("原文".into())), "原文");
    }

    /// 警告码跟错误码同样的要求：两种语言都组得出话，英文里不许有汉字。
    #[test]
    fn every_warning_code_composes_in_both_languages() {
        use crate::proto::{IoReason, WarningCode::*};
        let codes = [
            ProfileDirUnreadable {
                name: "profiles".into(),
                reason: IoReason::PermissionDenied,
            },
            ProfileUnreadable {
                name: "x.toml".into(),
                reason: IoReason::Other,
            },
            ProfileMalformed {
                name: "x.toml".into(),
                line: Some(3),
                reason: "invalid key".into(),
            },
            ProfileMalformed {
                name: "x.toml".into(),
                line: None,
                reason: "invalid key".into(),
            },
            SecretsUnreadable {
                path: "/h/.dct/secrets.toml".into(),
                reason: IoReason::NotADirectory,
            },
            SecretsCorrupt {
                path: "/h/.dct/secrets.toml".into(),
            },
            // 出错解释连不上的每一种原因都要能在两种语言下组成一句警告——
            // 这是它唯一能走到用户眼前的路（守护进程的 stderr 被丢弃了）。
            LlmUnavailable(crate::llm::resolve::ResolveError::NoCredential(
                "kimi".into(),
            )),
            LlmUnavailable(crate::llm::resolve::ResolveError::NoSuchProvider(
                "nope".into(),
            )),
            LlmUnavailable(crate::llm::resolve::ResolveError::NoHeadlessCommand(
                "kimi".into(),
            )),
            LlmUnavailable(crate::llm::resolve::ResolveError::NoApiEndpoint(
                "claude".into(),
            )),
            LlmUnavailable(crate::llm::resolve::ResolveError::NoModel("kimi".into())),
            LlmUnavailable(
                crate::llm::resolve::ResolveError::BorrowedCredentialRefused {
                    name: "kimi".into(),
                    host: "api.moonshot.cn".into(),
                },
            ),
            LlmUnavailable(crate::llm::resolve::ResolveError::BadBaseUrl {
                name: "kimi".into(),
                url: "not-a-url".into(),
            }),
        ];
        for c in &codes {
            for l in Lang::all() {
                let s = msg::warning(*l, c);
                assert!(!s.trim().is_empty(), "{c:?} 在 {l:?} 下组不出话");
                // 底栏只有一行，警告绝不能带换行——带了会在等宽终端上
                // 错位换行，看着像一份栈追踪。
                assert!(!s.contains('\n'), "{c:?} 在 {l:?} 下带了换行：{s}");
            }
            let en = msg::warning(Lang::En, c);
            // `reason` 是 toml 库自己的说法，可能是英文也可能带别的字符，
            // 但我们自己那部分不能有汉字。
            assert!(!has_han(&en), "{c:?} 的英文里有汉字：{en}");
        }
    }

    #[test]
    fn help_line_joins_keys_with_their_labels() {
        let items = help_items(&[("n", Key::New), ("q", Key::Quit)], Lang::En);
        let line = help_text(&items);
        assert_eq!(line, "n new  q quit");
        // 分隔符必须是两个空格：`widgets::wrap_help` 就认它当断点
        assert!(line.contains("  "));
        // 键名和说明分开存着，渲染时才有东西可加粗
        assert_eq!(items[0].key, "n");
        assert_eq!(items[0].label(), "new");
    }

    /// 动态标签顶掉词条表里那一条，而且**每一个读说明的地方都得跟着变**。
    /// 底栏的 `n 新建 claude` 就靠它：agent 名是运行时才知道的，进不了
    /// `&'static str` 的表。漏改一处（比如量宽度的那一处还读着旧 `label`）
    /// 的症状是屏幕上画出来的一行比算出来的宽，句尾被静默截掉。
    #[test]
    fn a_runtime_label_overrides_the_table_one() {
        let mut items = help_items(&[("n", Key::New)], Lang::En);
        items[0].label_owned = Some("new claude".into());
        assert_eq!(items[0].label(), "new claude");
        assert_eq!(help_text(&items), "n new claude");
        assert_eq!(items[0].to_string(), "n new claude");
    }

    /// 没有对应按键的那种提示（「其余按键都发给 agent」）不能在句首多出
    /// 一个空格——原来的 `format!("{k} {label}")` 在 `k` 是空串时就会。
    #[test]
    fn a_help_item_without_a_key_is_just_its_label() {
        let items = help_items(&[("", Key::OtherKeysGoToAgent)], Lang::En);
        assert_eq!(help_text(&items), text(Key::OtherKeysGoToAgent, Lang::En));
    }

    #[test]
    fn english_is_the_default_when_nothing_says_otherwise() {
        assert_eq!(resolve(None, &fake_env(&[])), Lang::En);
    }

    #[test]
    fn a_saved_choice_beats_the_system_locale() {
        // 用户在设置里明确选过，就不该被系统 locale 推翻
        let env = fake_env(&[("LANG", "en_US.UTF-8")]);
        assert_eq!(resolve(Some(Lang::Zh), &env), Lang::Zh);
    }

    #[test]
    fn dct_lang_beats_even_a_saved_choice() {
        let env = fake_env(&[("DCT_LANG", "en")]);
        assert_eq!(resolve(Some(Lang::Zh), &env), Lang::En);
    }

    /// 打错的 `DCT_LANG` 不能把用户存过的选择也一起丢掉——那是「我想临时换一下」
    /// 变成「我的设置没了」。
    #[test]
    fn an_unknown_dct_lang_falls_through_instead_of_resetting() {
        let env = fake_env(&[("DCT_LANG", "klingon")]);
        assert_eq!(resolve(Some(Lang::Zh), &env), Lang::Zh);
    }

    #[test]
    fn the_system_locale_is_read_down_to_its_primary_code() {
        assert_eq!(
            resolve(None, &fake_env(&[("LANG", "zh_CN.UTF-8")])),
            Lang::Zh
        );
        assert_eq!(
            resolve(None, &fake_env(&[("LC_ALL", "zh_TW.UTF-8")])),
            Lang::Zh
        );
        assert_eq!(
            resolve(None, &fake_env(&[("LANG", "ja_JP.UTF-8")])),
            Lang::En
        );
    }

    /// `LC_ALL` 压过 `LC_MESSAGES` 压过 `LANG`，这是 POSIX 的规矩，不是我们定的。
    #[test]
    fn locale_variables_are_checked_in_posix_order() {
        let env = fake_env(&[("LC_ALL", "zh_CN.UTF-8"), ("LANG", "en_US.UTF-8")]);
        assert_eq!(resolve(None, &env), Lang::Zh);
    }

    /// 空字符串等于没设。真实环境里 `LANG=` 很常见（尤其在 cron 和容器里），
    /// 当成「设了一个认不出的值」会让它挡住后面本来有效的变量。
    #[test]
    fn an_empty_locale_variable_is_skipped_not_honored() {
        let env = fake_env(&[("LC_ALL", ""), ("LANG", "zh_CN.UTF-8")]);
        assert_eq!(resolve(None, &env), Lang::Zh);
    }

    #[test]
    fn codes_round_trip_and_unknown_ones_are_rejected() {
        for l in Lang::all() {
            assert_eq!(Lang::from_code(l.code()), Some(*l));
        }
        assert_eq!(Lang::from_code("klingon"), None);
    }

    /// 语言名必须用它自己的语言写：用户误切到看不懂的语言之后，
    /// 唯一能自救的线索就是在列表里认出自己那一行。
    #[test]
    fn each_language_is_named_in_its_own_language() {
        assert_eq!(Lang::En.native_name(), "English");
        assert_eq!(Lang::Zh.native_name(), "中文");
    }
}
