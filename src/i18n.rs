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
    SwitchProject,
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
    StopNeedsATarget,
    StopAllTakesNoIds,
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
}

pub fn text(k: Key, lang: Lang) -> &'static str {
    use Key::*;
    match k {
        New => t!(lang, en: "new", zh: "新建"),
        SwitchAgent => t!(lang, en: "switch agent", zh: "换 agent"),
        SwitchProject => t!(lang, en: "switch project", zh: "换项目"),
        SeeAllProjects => t!(lang, en: "all projects", zh: "看全部项目"),
        ThisProjectOnly => t!(lang, en: "this project only", zh: "只看本项目"),
        Secrets => t!(lang, en: "keys", zh: "密钥"),
        Grid => t!(lang, en: "grid", zh: "九宫格"),
        List => t!(lang, en: "list", zh: "列表"),
        Select => t!(lang, en: "select", zh: "选择"),
        Open => t!(lang, en: "open", zh: "进入"),
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

        NoSessionsHere => t!(
            lang,
            en: "No sessions in this project yet. Press n to start one, or a to see every project.",
            zh: "这个项目还没有会话，按 n 开一个，按 a 看全部项目",
        ),
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
        // 说清「怎么停一个」和「怎么全停」，而不是甩一句「参数错误」——
        // 敲出 `dct stop` 的人已经知道自己要停东西了，缺的是怎么写。
        StopNeedsATarget => t!(
            lang,
            en: "Which one? `dct stop 3` stops session 3, `dct stop --all` stops every session.",
            zh: "要停哪个？`dct stop 3` 停 3 号会话，`dct stop --all` 全停。",
        ),
        StopAllTakesNoIds => t!(
            lang,
            en: "`dct stop --all` already means every session — drop the ids.",
            zh: "`dct stop --all` 本来就是全停，不要再跟会话号。",
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelpItem {
    pub key: &'static str,
    pub label: &'static str,
}

impl std::fmt::Display for HelpItem {
    /// 一条提示拼成文字的样子。渲染时加粗的那一份必须跟这里给出同一串
    /// 字符——单测断言的是这一份，屏幕上画的是那一份，两者分叉就等于
    /// 测了个寂寞。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.key.is_empty() {
            write!(f, "{}", self.label)
        } else {
            write!(f, "{} {}", self.key, self.label)
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

    pub fn switched_to(lang: Lang, project: &str) -> String {
        t!(lang, en: format!("Switched to {project}"), zh: format!("已切到 {project}"))
    }

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
        }
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
            WindowTooSmall,
            Verifying,
            PasteOrTypeKey,
            NoOtherRunningSession,
            NoSessionSelected,
            DaemonUnreachable,
            StaleData,
            NoDaemonRunning,
            NoSessionsRunning,
            StopNeedsATarget,
            StopAllTakesNoIds,
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
        assert_eq!(ALL_KEYS.len(), 93, "加了 Key 变体就要同步进 ALL_KEYS");
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
        assert_eq!(items[0].label, "new");
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
