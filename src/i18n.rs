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
    /// 选定之后还会接着问「用哪个 agent」并开一个新会话（见
    /// `ui::pin_project`）。这句词条只写前半句：底栏那一行按键表的宽度是
    /// 死的，而「加项目」正是用户按下去之前想的那件事——后半句他按完就看见
    /// 了，写进去只会把别的键挤掉。
    ///
    /// 跟 `SwitchProject` 分开是因为它们真的是两件事：`Tab` 在已有的组之间
    /// 走，`p` 往看板上加一个新的。共用「换项目」那句话的年代里，`p` 是唯一
    /// 的换项目手段；`Tab` 出现之后再写「换项目」，用户会以为按 `p` 能一步
    /// 换过去，而实际弹出来的是一个选择器。
    AddProject,
    /// `1`…`9` 的说明：组头前面印着号码，按下去一步落到那个项目上。
    /// 跟 `Tab` 是同一件事的两种走法，所以两条都得写：`Tab` 是挨个翻，
    /// 数字是看见号码直接跳。
    GotoProject,
    /// `x` 的说明：把光标所在的空项目从看板上拿掉。
    RemoveProject,
    /// `←` `→` `空格` 的说明：把光标所在的那个组收起来 / 摊开。
    /// 只有看板绑着——九宫格的左右键是移动焦点（`grid::handle_key`）。
    ToggleCollapse,
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
    /// `F4` 在会话视图里的动作名：进复制模式。**只是这一个动词**——
    /// 「鼠标交还终端」「按 F4 退出」这些解释已经在 `CopyMode`/`CopyModeShort`
    /// 里说了，这里跟 `("F4", ...)` 拼起来只用来填底栏按键表那一格
    /// （`F4 复制` / `F4 copy`），不是那条状态提示本身，命名和用途都不能跟
    /// 它们混：`CopyMode`/`CopyModeShort` 是「已经在复制模式里」时顶掉整条
    /// 右段的话，这一条是「还没进去、告诉你按哪个键能进去」。
    EnterCopyMode,
    /// `F5` 在会话视图里的动作名：把剪贴板里的图片交给 agent。跟
    /// `EnterCopyMode` 一样只是**一个动词**，拼在 `("F5", ...)` 后面填底栏
    /// 那一格；「为什么不是 Ctrl+V」那句解释在 `attach::paste_image` 的
    /// 文档注释里，不占底栏。
    PasteImage,
    OtherKeysGoToAgent,
    BackToListWord,
    BackToSettingsWord,
    OrPressDigit,
    Language,
    /// 设置页里那一项的名字，同时也是配色子列表的标题。
    BarTheme,
    ThemeGray,
    ThemeBlue,
    ThemeIndigo,
    ThemeTeal,
    ThemeGreen,
    ThemeOlive,
    ThemeAmber,
    ThemeCrimson,
    ThemeMagenta,
    ThemePurple,
    ThemeSlate,
    ThemeLight,
    ThemePaper,
    ThemeLines,
    /// 设置页顶层列表里的第二项。手机通知页本身（Task 4 的 `View::Phone`）
    /// 有自己一整套状态文案，这里只是设置项列表上的那一行标签。
    Phone,
    // —— 局域网手机端（同一个 WiFi 下扫码打开的网页） ——
    /// 设置页上那一小节的标题。**跟上面的「手机通知」是两件事**：那个是
    /// Telegram 推消息，这个是同一个 WiFi 下手机上打开的网页。同一页上
    /// 挨着放，所以标题必须让人一眼分得出来。
    WebSection,
    /// 没在监听
    WebOffLine,
    /// 在监听，而且算得出地址——具体地址由 `msg::web_address` 那句负责报，
    /// 这一句只说「开着」。
    WebOnLine,
    /// 在监听，但算不出局域网地址（没连 WiFi、只有回环）。**不能借用
    /// `WebOnLine`**：那句后面跟着一个地址，而这里根本没有地址可跟。
    WebAddressUnknownLine,
    /// 关着时的下一步：按 w 打开
    WebNextStepOff,
    /// 打开之前先说：系统会弹一个防火墙授权框。
    ///
    /// **必须在按下去之前说**。第一次绑到所有网卡上时，Windows 和 macOS 都会
    /// 问一句允不允许，而**系统在有人点它之前把那次调用按住**——不知情的用户
    /// 点了「取消」，之后只会看到手机连不上，屏幕上没有任何东西解释为什么。
    /// （这件事是被一条集成测试逼出来的：客户端正好 5 秒超时，而守护进程
    /// 日志显示它早就把回复写完了。）
    WebFirewall,
    /// 开着时的下一步：拿手机扫码；再按一次 w 关掉
    WebNextStepOn,
    /// 开着但算不出地址时的下一步：先把这台电脑连上 WiFi
    WebNextStepAddressUnknown,
    /// 手机网页上输入框的提示语：点屏幕上的一行，把那行文字放进输入框。
    ///
    /// 手机上没有桌面那个 `F4` 复制模式（临时把鼠标还给终端去拖选），
    /// 而长按选中在真机上不好使——于是「把屏幕上的字弄进输入框」这件事
    /// 在手机上原本无路可走。这句话是那条路的唯一说明书。
    TapLineToInsert,
    /// 底栏上「打开」那一格的动作名。
    WebTurnOn,
    /// 底栏上「关掉」那一格的动作名。
    WebTurnOff,
    /// 底栏上 `w` 那一格的动作名。**只是一个动词**，跟 `EnterCopyMode`
    /// 一样拼在 `("w", …)` 后面。
    WebToggle,
    /// 窗口太窄，二维码放不下。**这不是错误**——码画不下是个尺寸问题，
    /// 出路有两条（拉宽窗口，或者照着地址手输），这一句两条都得说。
    WebQrTooNarrow,
    /// 手机端画面上那两个字号按钮的名字。**图标也要有名字**——读屏软件
    /// 念不出「A−」，而手机上读屏用户很多（同网页里 `back` 那一条）。
    TextSmaller,
    TextBigger,
    /// 手机端那个「用键盘打字」开关的名字。接了实体键盘（iPad + 妙控键盘、
    /// 或者随便一个蓝牙键盘）之后，人会本能地直接打字，而不是先点那个单行框。
    KeyboardCapture,
    // —— 手机通知页 ——
    /// 还没填过令牌
    PhoneOffLine,
    /// 已经连上，但不知道是谁（没有 `owner`，理论上不该出现，兜底用）
    PhonePairedLine,
    /// `WaitingForPairing` 但还没拿到 bot 用户名——守护进程刚重启，令牌
    /// 还在但 bot 名字要等 bridge 重新查一次。**不能借用 `PhoneOffLine`**：
    /// 令牌没丢，这不是关着，见 `ui::phone::status_line` 的注释。
    PhoneReconnectingLine,
    /// 连不上——**不带任何原因**，原因由 `daemon.rs` 决定但故意不传到这里，
    /// 见 `PhoneState::Broken` 的文档注释和 `ui::phone::status_line`。
    PhoneBrokenLine,
    /// Off 状态的下一步：去填一个令牌
    PhoneNextStepOff,
    /// WaitingForPairing 状态的下一步：叫用户去 Telegram 给 bot 发消息
    /// （具体是哪个 bot 由 `status_line` 那句负责点名，这一句只说「发过去」）
    PhoneNextStepWaiting,
    /// Broken 状态的下一步：重新填一遍令牌
    PhoneNextStepBroken,
    /// `WaitingForPairing` 但还没拿到 bot 名字时的下一步：等一下，
    /// **不能**叫用户去给一个没有名字的 bot 发消息。
    PhoneNextStepReconnecting,
    /// 按键表：填令牌
    PhoneEnterToken,
    /// 按键表：重新配对（换一台手机）
    PhoneRepair,
    /// 按键表：关掉手机通知
    PhoneTurnOff,
    /// 手机页填令牌时的输入提示
    PhonePasteToken,
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
    BackToBoardF2,
    BackToList,
    BackToSettings,
    /// 选项目那一屏、搜索词非空时的逃生键：`Esc` 这时候清的是搜索词，
    /// 退不出这一屏。**底栏说什么就得真能做到什么**——写成「回看板」的话，
    /// 用户按下去只会看到列表变长，而他被告知的是自己会离开。
    ClearSearch,
    /// 同上，翻文件夹那一层的 `Esc`：退回最近项目那一层，不是回看板。
    BackToRecent,
    // —— 视图标题 ——
    BoardTitle,
    /// 底栏中段那块牌子前面的那个名词：`project` / `项目`。
    ///
    /// 反白的 `dc/dc-terminal` 无疑是**某个东西**，但没人告得诉你是哪个
    /// 东西——底栏分三段这件事本身就得先知道，而不知道它的人正是找不着
    /// 自己在哪的那个人。名词只在放得下时才写（`bar_chip`），让位的顺序是
    /// 父目录、名词、名字里的字符，牌子自己永远在场。
    ProjectChipLabel,
    Disconnected,
    PickAgentTitle,
    /// 选 agent 那一屏的标题后半句：当前项目不是 git 仓库。
    ///
    /// 短到能跟标题挤在一行是硬要求——标题只有一行，后面还要接守护进程
    /// 报上来的 warning（密钥文件读不了之类），那是「为什么这一项用不了」
    /// 的唯一出处。完整的理由（agent 直接改你的真文件、撤销靠 git）不写
    /// 在这里：用户按下 Enter 会拿到 `ErrorCode::NotAGitRepo` 那一整句。
    NotAGitRepoHint,
    /// `g` 的说明：在当前项目上建一个 git 仓库。
    ///
    /// **只在选 agent 那一屏、且当前项目确实不是 git 仓库时才写得出来**
    /// ——这个项目的规矩是「屏幕上不写按不动的键」。
    InitGitRepo,
    /// `g` 成功之后那句话。
    GitRepoCreated,
    PickProjectTitle,
    TypePathTitle,
    SettingsTitle,
    CurrentProject,
    ManualPath,
    /// 动作行：在这一屏当前停着的目录里新建一个项目。**跟 `ManualPath`
    /// 一样不参与搜索**，钉在列表底下——它是个动作，不是一条数据，而且
    /// 恰恰在搜不到东西的时候最该看得见。
    NewProject,
    /// 新建项目时那一行提示：名字建到哪个目录里去。目录名由
    /// `msg::new_project_in` 那句带上，这条只是它的兜底标题。
    NewProjectPrompt,
    /// 名字空着
    NewProjectNoName,
    /// 名字里有 `/` 或 `..`
    NewProjectBadName,
    RecentProjects,
    /// 搜索框空着时框里那句压暗的话。**这一屏唯一告诉用户「可以打字」的
    /// 地方**——上一版把它写在底栏（「直接打字过滤」），而打下去之后屏幕上
    /// 没有任何东西回显他打了什么：行悄悄少了，原因看不见，也没法确认自己
    /// 打错了哪个字母。现在提示和回显是同一个框，占位文案就得住在框里。
    SearchPlaceholder,
    /// 最近层的动作行：从最近项目切到翻文件夹那一层。
    BrowseFolders,
    /// 光标停在一个最近项目上时，`Enter` 那一格写的字。**不能用 `Open`**
    /// （「进会话」）：这一下打开的是一个**项目**，接着还要选 agent，
    /// 会话是再下一步的事。
    OpenProject,
    /// 浏览层钉在最上面那一行：就用现在停着的这个目录。
    UseThisFolder,
    /// 搜索词一条最近项目都没匹配上时，列表那块地方写的话。
    /// **不写「无结果」这种话**：用户要的是下一步，而下一步（翻文件夹 /
    /// 新建 / 手输路径）就钉在这句话底下。
    NoMatchingProject,
    EnterFolder,
    GoUp,
    NoSubfolders,
    /// 翻文件夹那一层里，目录名单靠肉眼分辨不出异常时贴的压暗提示（见 `pick.rs`
    /// 里挑目录那一段的说明）。POSIX 目录名里只有 `/` 和 NUL 不合法，
    /// 转义序列这类看不见的字节完全合法，`truncate` 又把它们从**显示**里
    /// 滤掉了——不贴这个提示，用户没有任何办法在选中之前发现这一行不对劲。
    /// 文案不许提「控制字符」「转义序列」这类编码构成，用户不需要知道
    /// 那是什么，只需要知道这里有点不对劲。
    HiddenCharsInName,
    // —— 状态与提示 ——
    NoSessionsHere,
    /// 手机端连不上这台电脑时的那句话。
    ///
    /// **不说「你的电脑睡着了」那么肯定**：手机这一侧分不出「笔记本休眠了」
    /// 「dct 被关掉了」「换了个 WiFi」这三种，而说死一种就有三分之二的概率
    /// 在骗人。把三种可能都摆出来，用户自己一眼就知道是哪种。
    PhoneOffline,
    /// 经中转时的第一种断法：那台电脑根本没在问中转要东西。
    PhoneComputerGone,
    /// 第二种：信封送到了，它没回话。
    PhoneComputerSilent,
    /// 空项目组头上接在「还没有会话」后面的那半句：`上次用 claude`
    LastUsedAgent,
    /// 九宫格的整屏空态：一个格子都没有，屏幕正中就这一句话。
    ///
    /// 空屏和底栏说的必须是**两个不同的事实**：这一条说「这里现在什么都没有，
    /// 下一步做这个」，底栏那边按 `s`/`i` 得到的是 `NoSessionSelected`
    /// （「这一下按键没有作用对象」）。原来两处各有一条措辞不同、意思一样的
    /// 词条，同屏出现时用户会以为是两件事。
    NoSessionsRunningPressN,
    /// 组头上：这个项目的目录已经不在了
    ProjectDirGone,
    AllSessionsStopped,
    WindowTooSmall,
    Verifying,
    PasteOrTypeKey,
    NoOtherRunningSession,
    /// 按了 F5，但剪贴板里没有图。**不是错误**——用户刚拷的是一段文字，
    /// 或者截图还没截成，这是最常见的一种「什么都没发生」，红字会把它说得
    /// 比实情严重。
    NoImageInClipboard,
    NoSessionSelected,
    DaemonUnreachable,
    StaleData,
    /// 复制模式下顶掉整条底栏右段的提示
    CopyMode,
    /// 同上，但给放不下长文案的窄终端用——必须放进 `ui::ACTION_MIN_COLS`，
    /// 因为这是全屏唯一写着 F4（怎么退出复制模式）的地方，容不下被静默截断。
    CopyModeShort,
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
    RestartExplain,
    RestartAsk,
    RestartCancelled,
    RestartDone,
    RestartFailed,
    /// `dct restart` 时后台本来就空着：这就给你起一个。
    RestartNothingToRestart,
    /// 同上，但没有终端可以开界面，只把守护进程拉起来了。
    RestartStartedDaemonOnly,
    /// 连守护进程都没拉起来。
    RestartStartFailed,
    // —— 守护进程重启后，问要不要接回上次的会话 ——
    ResumeSessionsExplain,
    ResumeSessionsAsk,
    /// 清单里某一条后面跟的标记：这一条会真的接回上次的对话。
    ResumeSessionsWillContinue,
    /// 清单里某一条后面跟的标记：同一个目录 + agent 下已经有别的会话
    /// 接回去了，这一条老老实实开一个新的。
    ResumeSessionsWillStartFresh,
    RequestFailed,
    ActionDone,
    NoChanges,
    // —— 选择器里的「为什么用不了」——
    ReasonNeedsSecret,
    ReasonNotInstalled,
    /// `x` 按在一个还有会话的组上
    GroupNotEmpty,

    // —— 配对（跟训练营网关换一把钥匙）——
    /// `EnterSecret` 屏幕上 Ctrl+A 的说明（profile 是 `"dc"` 时才会出现）。
    /// 不占用一个字母键——`o` 已经留给密钥输入本身（`Ctrl+O` 那条注释），
    /// 这里跟它同一个键位规矩。
    AutoPair,
    /// Starting 阶段：已经发出请求，真网络在飞，这一屏没有别的话可说。
    PairContacting,
    /// Waiting 阶段的说明句：在浏览器里，用这个码。
    PairEnterCodeInBrowser,
    /// 可重试的过期（`retryable == true`）：网关没告诉我们具体原因
    /// （到点的 ttl 过期一律是空 `message`），这句是 dct 自己给的人话。
    PairCodeExpired,
    /// `PairTick::Failed("empty_key")`：网关批了但给出的钥匙是空的——
    /// 学生这边什么都没做错，是网关那侧的账号还没有可读的钥匙。
    PairKeyUnreadable,
    /// `PairTick::Failed("denied")`：有人在确认页点了拒绝。
    PairDenied,
    /// 网关的配对开关关着（`not_enabled`，无论是起步时还是轮询时撞上）。
    PairNotEnabled,
    /// Done 阶段：两条路都开了（Anthropic + Qwen）。
    PairDoneBoth,
    /// Done 阶段：**免费账号只有这一条**——必须点名「Qwen 那一路」，也要
    /// 说清「Claude 需要付费升级」，不能让学生对着一个用不了的 Claude
    /// 猜为什么。见 `pair_view.rs` 头上关于这句话的分析。
    PairDoneQwenOnly,
    /// `p` 键的说明：手动填密钥这条退路，四个阶段都在。
    PairManualHint,
    /// Waiting/Done 屏上那一行小字：这次配对有没有顺手打开「报错时的 AI
    /// 解释」——读的是本地 `[llm]` 写没写（`pair_view::opt_in_llm` 的
    /// 文档注释），不是又开一屏问一遍。
    PairLlmOptIn,
    /// 底栏：重开浏览器（Waiting 阶段的 `o`）。
    ReopenBrowser,
    /// 底栏：重试（`Failed { retryable: true }` 阶段的 `r`）。
    Retry,
    /// 底栏：手动填密钥（配对屏任何阶段的 `p`）。
    ManualEntry,
}

/// 一段文字里有没有汉字。
///
/// 提到模块顶层（原来埋在 `mod tests` 里）是因为「英文界面上不许冒出汉字」
/// 这条规矩管的**不止是 `text()`**。按键表的每一条有两半：键名那一列是
/// 写死的字面量（`n`、`Tab`、`↑↓`），说明那一列才走 `text()`。
/// `no_english_entry_contains_han_characters` 只看得见后一半，于是
/// `("←→/空格", ToggleCollapse)` 这种「把中文写进键名列」的错整个在它视野
/// 之外——英文用户看到的是 `←→/空格 fold`。现在 `view.rs` 和 `keys.rs`
/// 各有一条守卫把前一半也扫一遍，三条共用这一个判定。
#[cfg(test)]
pub(crate) fn has_han(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

pub fn text(k: Key, lang: Lang) -> &'static str {
    use Key::*;
    match k {
        New => t!(lang, en: "new", zh: "新建"),
        SwitchAgent => t!(lang, en: "switch agent", zh: "换 agent"),
        // 英文只有一个词。底栏三段切完，80 列终端的右段只剩 39 列，而
        // `Enter open  n new  Tab switch project  ? …` 要 42 列——多出来的
        // 三列不是折一行，是 `Tab` 整条被 `fit_help` 丢掉，于是英文用户的
        // 底栏又回到了「键随窗口宽度忽隐忽现」，正是这次改造要消灭的东西。
        // 名词而不是动词，也是因为这三列：紧挨着它左边的中段写的就是当前
        // 项目名，`Tab project` 读起来是「Tab → 项目」。中文双宽字符更省，
        // 「换项目」放得下，不必跟着缩。
        SwitchProject => t!(lang, en: "project", zh: "换项目"),
        AddProject => t!(lang, en: "add project", zh: "加项目"),
        GotoProject => t!(lang, en: "go to project", zh: "直达项目"),
        RemoveProject => t!(lang, en: "remove", zh: "移除"),
        ToggleCollapse => t!(lang, en: "fold", zh: "折叠"),
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
        // 只是动词，跟 `Undo`/`Diff` 那一档词条一个形状——句子留给
        // `CopyMode`/`CopyModeShort`。
        EnterCopyMode => t!(lang, en: "copy", zh: "复制"),
        PasteImage => t!(lang, en: "paste image", zh: "粘贴图片"),
        BackToListWord => t!(lang, en: "back to the list", zh: "返回列表"),
        BackToSettingsWord => t!(lang, en: "back to settings", zh: "返回设置"),
        OtherKeysGoToAgent => t!(
            lang,
            en: "every other key goes to the agent",
            zh: "其余按键都发给 agent",
        ),
        OrPressDigit => t!(lang, en: "or press a number", zh: "或直接按数字"),
        Language => t!(lang, en: "language", zh: "语言"),
        BarTheme => t!(lang, en: "colors", zh: "配色"),
        ThemeGray => t!(lang, en: "gray", zh: "灰"),
        ThemeIndigo => t!(lang, en: "indigo", zh: "靛"),
        ThemeTeal => t!(lang, en: "teal", zh: "青"),
        ThemeOlive => t!(lang, en: "olive", zh: "橄榄"),
        ThemeMagenta => t!(lang, en: "magenta", zh: "玫"),
        ThemeSlate => t!(lang, en: "slate", zh: "石板"),
        ThemeAmber => t!(lang, en: "amber", zh: "琥珀"),
        ThemeCrimson => t!(lang, en: "crimson", zh: "绯"),
        ThemePaper => t!(lang, en: "paper", zh: "纸"),
        ThemeBlue => t!(lang, en: "blue", zh: "蓝"),
        ThemeGreen => t!(lang, en: "green", zh: "绿"),
        ThemePurple => t!(lang, en: "purple", zh: "紫"),
        ThemeLight => t!(lang, en: "light", zh: "浅色"),
        ThemeLines => t!(lang, en: "lines", zh: "横线"),
        Phone => t!(lang, en: "phone notifications", zh: "手机通知"),
        WebSection => t!(lang, en: "Phone client on this WiFi", zh: "局域网手机端"),
        WebOffLine => t!(lang, en: "Not listening", zh: "还没打开"),
        WebOnLine => t!(
            lang,
            en: "On — scan this with the phone on the same WiFi",
            zh: "开着——用同一个 WiFi 下的手机扫这个码",
        ),
        WebAddressUnknownLine => t!(
            lang,
            en: "On, but this computer has no address on any network",
            zh: "开着，但这台电脑算不出自己在局域网里的地址",
        ),
        WebNextStepOff => t!(
            lang,
            en: "Press w to turn it on",
            zh: "按 w 打开",
        ),
        WebFirewall => t!(
            lang,
            en: "The first time, your system asks whether to allow it — say yes for private networks, or the phone cannot connect.",
            zh: "第一次打开时系统会问允不允许，选「允许·专用网络」，否则手机连不上。",
        ),
        WebNextStepOn => t!(
            lang,
            en: "Press w to turn it off again",
            zh: "再按一次 w 就关掉",
        ),
        WebNextStepAddressUnknown => t!(
            lang,
            en: "Put this computer on the same WiFi as the phone, then press w twice to restart it",
            zh: "先把这台电脑连上手机那个 WiFi，再按两下 w 重开",
        ),
        TapLineToInsert => t!(
            lang,
            en: "Type here, or tap a line above to copy it in",
            zh: "在这儿打字，或者点上面某一行把它放进来",
        ),
        WebTurnOn => t!(lang, en: "turn on", zh: "打开"),
        WebTurnOff => t!(lang, en: "turn off", zh: "关掉"),
        WebToggle => t!(lang, en: "phone client", zh: "手机端开关"),
        TextSmaller => t!(lang, en: "smaller text", zh: "字小一点"),
        TextBigger => t!(lang, en: "bigger text", zh: "字大一点"),
        KeyboardCapture => t!(lang, en: "type with your keyboard", zh: "用键盘打字"),
        WebQrTooNarrow => t!(
            lang,
            en: "The window is too narrow for the code — widen it, or type the address into the phone",
            zh: "窗口太窄，二维码放不下——把窗口拉宽，或者照着地址在手机上手输",
        ),
        PhoneOffLine => t!(lang, en: "Phone notifications are off", zh: "手机通知还没打开"),
        PhonePairedLine => t!(lang, en: "Connected", zh: "已连上"),
        PhoneReconnectingLine => t!(
            lang,
            en: "Reconnecting, one moment",
            zh: "正在重新接上，请稍候",
        ),
        PhoneBrokenLine => t!(
            lang,
            en: "Cannot reach the phone notification service right now",
            zh: "手机通知这会儿连不上",
        ),
        PhoneNextStepOff => t!(
            lang,
            en: "Press Enter and paste a Telegram bot token to turn this on",
            zh: "按 Enter 粘贴一个 Telegram bot 的令牌，打开这个功能",
        ),
        PhoneNextStepWaiting => t!(
            lang,
            en: "Open Telegram and send that bot any message to finish pairing",
            zh: "打开 Telegram，给那个 bot 发一条消息，就能完成配对",
        ),
        PhoneNextStepBroken => t!(
            lang,
            en: "Press Enter to paste the token again",
            zh: "按 Enter 重新粘贴一遍令牌",
        ),
        PhoneNextStepReconnecting => t!(
            lang,
            en: "Give it a moment, then check back here",
            zh: "稍等一下，过会儿再回来看看",
        ),
        PhoneEnterToken => t!(lang, en: "enter token", zh: "填令牌"),
        PhoneRepair => t!(lang, en: "re-pair", zh: "重新配对"),
        PhoneTurnOff => t!(lang, en: "turn off", zh: "关掉"),
        PhonePasteToken => t!(
            lang,
            en: "Paste or type the bot token from BotFather",
            zh: "粘贴或输入 BotFather 给的令牌",
        ),

        MoreKeys => t!(lang, en: "…", zh: "…"),
        AllKeys => t!(lang, en: "All keys", zh: "全部按键"),
        KeysGroupMove => t!(lang, en: "Move", zh: "走动"),
        KeysGroupSession => t!(lang, en: "Sessions", zh: "会话"),
        KeysGroupConfig => t!(lang, en: "Settings", zh: "设置"),

        BackToBoard => t!(lang, en: "Esc back", zh: "Esc 回看板"),
        // 会话视图专用：那里 Esc 归 agent（取消/清空/关弹窗），逃生键只能是
        // F2。别把这条跟上面那条合并——合并就意味着某一屏的底栏在说谎。
        // 英文这一格写的是**去哪儿**，不是「往回」：从会话视图上看，
        // 「back」没说清回到哪一屏，而 F2 的落点就是那块会话看板——
        // 中文那半早就点名了（回看板），英文补上。
        BackToBoardF2 => t!(lang, en: "F2 main", zh: "F2 回看板"),
        BackToList => t!(lang, en: "Esc back", zh: "Esc 回列表"),
        BackToSettings => t!(lang, en: "Esc settings", zh: "Esc 回设置"),
        ClearSearch => t!(lang, en: "Esc clear", zh: "Esc 清空"),
        BackToRecent => t!(lang, en: "Esc recent", zh: "Esc 回最近"),

        // 看板只有这一个标题了：它现在**永远**是全部项目——分组之后
        // 「只看本项目 / 看全部项目」这对模式整个消失，标题不再随模式变。
        BoardTitle => t!(lang, en: "dct sessions", zh: "dct 会话看板"),
        // 英文 7 列、中文 4 列：中文因此比英文多留一档父目录的余地
        ProjectChipLabel => t!(lang, en: "project", zh: "项目"),
        Disconnected => t!(
            lang,
            en: "disconnected, this may be out of date",
            zh: "连接已断开，数据可能已过期",
        ),
        PickAgentTitle => t!(lang, en: "Pick an agent", zh: "选 agent"),
        // **说的是「接下来会发生什么」，不是「你得先干什么」。**
        // 这句话以前写的是「按 g 初始化」——一句命令，而且要求用户先懂
        // git 是什么、仓库是什么、以及为什么开个 AI 助手要先有仓库。他一样
        // 都不需要懂：仓库现在由 `pick::prepare_repo` 在按下 Enter 那一刻
        // 自己建。留着这句话是为了**别让那个 `.git` 凭空冒出来**，不是为了
        // 派活给用户。
        NotAGitRepoHint => t!(
            lang,
            en: "not a git project yet — dct will set one up",
            zh: "还不是 git 仓库 —— 开 agent 时自动建",
        ),
        InitGitRepo => t!(lang, en: "init git", zh: "建仓库"),
        GitRepoCreated => t!(
            lang,
            en: "git project created, agents can work here now",
            zh: "git 仓库建好了，现在可以开 agent 了",
        ),
        PickProjectTitle => t!(lang, en: "Pick a project", zh: "选项目"),
        TypePathTitle => t!(lang, en: "Type a project path", zh: "输入项目路径"),
        SettingsTitle => t!(lang, en: "Settings", zh: "设置"),
        // 底栏中段现在只写项目本身，不再写「当前项目：」这个标签——一行里
        // 最贵的是列数，而「这里写的是哪个项目」不用一个标签来说明。词条留着
        // 是因为别处（浮层标题之类）随时可能要，它不占屏幕。
        CurrentProject => t!(lang, en: "Project", zh: "当前项目"),
        ManualPath => t!(lang, en: "Type a path…", zh: "手输路径…"),
        NewProject => t!(lang, en: "New project…", zh: "新建项目…"),
        NewProjectPrompt => t!(
            lang,
            en: "Name for the new folder",
            zh: "新目录叫什么名字",
        ),
        NewProjectNoName => t!(
            lang,
            en: "Type a name first",
            zh: "先打一个名字",
        ),
        NewProjectBadName => t!(
            lang,
            en: "A name only — no / and no .. (use Type a path… to build elsewhere)",
            zh: "只写名字——不要 / 也不要 ..（要建到别处用「手输路径…」）",
        ),

        RecentProjects => t!(lang, en: "Recent projects", zh: "最近的项目"),
        SearchPlaceholder => t!(lang, en: "type to search", zh: "打字搜索"),
        BrowseFolders => t!(lang, en: "Browse folders…", zh: "翻文件夹找…"),
        OpenProject => t!(lang, en: "open", zh: "打开"),
        UseThisFolder => t!(lang, en: "Use this folder", zh: "就用这个文件夹"),
        NoMatchingProject => t!(
            lang,
            en: "No project matches that",
            zh: "没有项目对得上",
        ),
        EnterFolder => t!(lang, en: "go in", zh: "进去"),
        GoUp => t!(lang, en: "go up", zh: "上一级"),
        NoSubfolders => t!(
            lang,
            en: "No folders here — press ← to go up",
            zh: "这里没有文件夹，按 ← 回上一级",
        ),
        HiddenCharsInName => t!(
            lang,
            en: "(something invisible in this name)",
            zh: "（这个名字里有看不见的东西）",
        ),

        // 组头上那一句，不是整屏空态——分组之后这句话贴在项目名后面，
        // 屏幕上别的组还列着会话，「按 n 开一个」那半句在这里是噪音。
        // 英文写 `no sessions` 不写 `no sessions yet`：这一格后面还要接
        // 「上次用 <agent>」，而 80 列的看板上这一格只有 34 列（组头前缀
        // 44 列，含 `List` 每行都预留的 `▶ `）。`yet` 那四列会把最长的
        // 内置 agent 名（`opencode`/`deepseek`/`qwen-api`，8 列）顶出边框。
        NoSessionsHere => t!(lang, en: "no sessions", zh: "还没有会话"),
        PhoneOffline => t!(
            lang,
            en: "can't reach your computer — it may be asleep, or dct was closed",
            zh: "连不上你的电脑——可能是它睡着了，或者 dct 已经关掉",
        ),
        // 上面那句是局域网上唯一说得出的话：连不上就是连不上，分不清是哪
        // 一种。经中转的时候中转分得清，下面这两句就该各说各的——一句让人
        // 去开机，一句让人去动一下那台机器。指错方向比不说更费时间。
        PhoneComputerGone => t!(
            lang,
            en: "your computer isn't connected — it may be off, or dct isn't running",
            zh: "你的电脑没连上——可能关机了，或者 dct 没在跑",
        ),
        PhoneComputerSilent => t!(
            lang,
            en: "your computer is connected but isn't answering — it may have gone to sleep",
            zh: "你的电脑连着，但没回话——多半是刚睡过去",
        ),
        // 空项目的组头上，这半句接在上面那条后面：`还没有会话 · 上次用 claude`。
        // 「哪个项目用哪个 agent」在别处只有底栏那一条，而底栏 80 列上会把
        // agent 名让掉——空项目于是全屏没有一处答得出这个问题。
        LastUsedAgent => t!(lang, en: "last used", zh: "上次用"),
        ProjectDirGone => t!(lang, en: "folder is gone", zh: "目录不在了"),
        AllSessionsStopped => t!(
            lang,
            en: "Every session here has stopped. Press g for the list to see them, or n to start one.",
            zh: "这里的会话都停了。按 g 回列表能看到它们，按 n 开一个新的",
        ),
        NoSessionsRunningPressN => t!(
            lang,
            en: "No sessions yet — press n to start one",
            zh: "还没有会话，按 n 新建",
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
        NoImageInClipboard => t!(
            lang,
            en: "The clipboard has no image",
            zh: "剪贴板里没有图片",
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
        // `dct restart` 用的那一套。跟上面 `StaleDaemon*` 分开写而不是复用：
        // 那一套的语境是「你手里这个守护进程是旧版本」（用户没打算重启，是
        // dct 发现问题拦下来问的），这一套的语境是「你自己敲了 restart」——
        // 前者要先解释为什么突然被问，后者只需要把代价说清楚。
        RestartExplain => t!(
            lang,
            en: "Restarting the background service ends the sessions running right now —\n\
                 your file changes stay, but the agents have to be started again.",
            zh: "重启后台服务会断掉正在跑的会话——文件改动都还在，\n\
                 只是 agent 要重新开一次。",
        ),
        RestartAsk => t!(
            lang,
            en: "Restart it now? (y = restart, Enter = leave it alone)",
            zh: "现在重启吗？(y = 重启，直接回车 = 不动它)",
        ),
        RestartCancelled => t!(lang, en: "Left it alone.", zh: "没动它"),
        RestartDone => t!(lang, en: "Restarted.", zh: "已重启"),
        RestartFailed => t!(
            lang,
            en: "Could not restart it. The old one is still running.",
            zh: "没能重启，旧的还在跑",
        ),
        // 后台空着的时候 `restart` 干的是「启动」，所以先把这句话说出来再
        // 动手——用户敲的是 restart，屏幕上直接冒出看板会让人以为敲错了。
        RestartNothingToRestart => t!(
            lang,
            en: "Nothing was running in the background. Starting a fresh one…",
            zh: "后台本来就没东西在跑，这就给你起一个…",
        ),
        RestartStartedDaemonOnly => t!(
            lang,
            en: "Background service started. Run `dct` to open the board.",
            zh: "后台服务已启动。敲 `dct` 打开看板",
        ),
        RestartStartFailed => t!(
            lang,
            en: "Could not start the background service.",
            zh: "没能启动后台服务",
        ),
        ResumeSessionsExplain => t!(
            lang,
            en: "The background service was not running. Before, it had these sessions open:",
            zh: "后台服务这次没在跑。它上次开着这些会话：",
        ),
        ResumeSessionsAsk => t!(
            lang,
            en: "Bring them back? (y = bring them back, Enter = start with an empty board)",
            zh: "要接回来吗？(y = 接回来，直接回车 = 从空白看板开始)",
        ),
        ResumeSessionsWillContinue => t!(lang, en: "continues", zh: "继续"),
        ResumeSessionsWillStartFresh => t!(lang, en: "starts fresh", zh: "新开"),
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

        AutoPair => t!(lang, en: "auto-pair with the camp gateway", zh: "自动配对训练营网关"),
        PairContacting => t!(lang, en: "Contacting the camp gateway…", zh: "正在联系训练营网关…"),
        PairEnterCodeInBrowser => t!(
            lang,
            en: "Enter this code in the page that just opened",
            zh: "在刚打开的页面里输入这个码",
        ),
        PairCodeExpired => t!(
            lang,
            en: "This code expired. Press r for a new one.",
            zh: "这个码过期了，按 r 换一个新的",
        ),
        PairKeyUnreadable => t!(
            lang,
            en: "The gateway approved this but the key came back unreadable — this is not something pressing r again will fix.",
            zh: "网关批了，但钥匙读不出来——这不是再按一次 r 能解决的",
        ),
        PairDenied => t!(lang, en: "Pairing was denied", zh: "配对被拒绝了"),
        PairNotEnabled => t!(
            lang,
            en: "Pairing is turned off on the camp gateway right now",
            zh: "训练营网关现在关着配对功能",
        ),
        PairDoneBoth => t!(
            lang,
            en: "Paired. Both Claude and Qwen are ready to use.",
            zh: "配对成功，Claude 和 Qwen 两条路都能用了",
        ),
        PairDoneQwenOnly => t!(
            lang,
            en: "Paired on the free plan: only Qwen is ready. Claude needs a paid upgrade on the camp gateway.",
            zh: "配对成功，但这是免费账号：只有 Qwen 那一路能用。Claude 需要在训练营网关上付费升级",
        ),
        PairManualHint => t!(lang, en: "p to fill in a key by hand instead", zh: "按 p 改成手动填密钥"),
        PairLlmOptIn => t!(
            lang,
            en: "AI error explanations: on (from your [llm] config)",
            zh: "报错时的 AI 解释：已开启（读的是你的 [llm] 配置）",
        ),
        ReopenBrowser => t!(lang, en: "reopen browser", zh: "重开浏览器"),
        Retry => t!(lang, en: "retry", zh: "重试"),
        ManualEntry => t!(lang, en: "fill in by hand", zh: "手动填"),

        StaleData => t!(
            lang,
            en: "Cannot reach the dct service — what you see may be out of date",
            zh: "守护进程连不上，界面数据可能已过期",
        ),

        CopyMode => t!(
            lang,
            en: "Copy mode · mouse released · F4 exits",
            zh: "复制模式 · 鼠标已交还终端 · F4 退出"
        ),

        CopyModeShort => t!(
            lang,
            en: "Copy mode · F4 exits",
            zh: "复制模式 · F4 退出"
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

    /// `dct restart` 后面跟了它不认识的东西。
    ///
    /// **不把不认识的参数当成「没参数」忽略掉**：`dct restart --all` 这种手滑
    /// 如果被当成裸 `dct restart` 执行，用户以为自己限定了范围，实际把整个
    /// 守护进程连所有会话一起换掉了。
    pub fn restart_takes_no_args(lang: Lang, arg: &str) -> String {
        t!(
            lang,
            en: format!("`dct restart` takes no arguments (`{arg}`) — only `-y` to skip the question."),
            zh: format!("`dct restart` 不接参数（`{arg}`），只认 `-y`（不问直接重启）。"),
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

    /// 「新建项目…」那一行，在用户已经打了搜索词的时候写的字。
    ///
    /// **搜索不到正是最该新建的时候**，而这时候用户想要的名字就是他刚打的
    /// 那个词。把它写进行里（而不是让他进输入态再打一遍），这一行就从
    /// 「一个泛泛的入口」变成「就叫这个，建吧」——Enter 之后名字也是预填好的。
    pub fn new_project_named(lang: Lang, name: &str) -> String {
        t!(
            lang,
            en: format!("New project “{name}”…"),
            zh: format!("新建项目「{name}」…"),
        )
    }

    /// 新建项目那一行的提示：**把目录说出来**。「新目录叫什么名字」少了
    /// 「建在哪儿」这半句，用户没法确认自己是不是先翻到了对的地方。
    pub fn new_project_in(lang: Lang, dir: &str) -> String {
        t!(
            lang,
            en: format!("Name for the new folder in {dir}"),
            zh: format!("在 {dir} 里新建一个目录，叫什么名字"),
        )
    }

    /// 名字撞车。**把名字带上**，否则用户不知道是哪个撞了——他刚打的那个
    /// 名字自己看得见，但列表里那个同名目录可能在过滤词外面，看不见。
    pub fn new_project_exists(lang: Lang, name: &str) -> String {
        t!(
            lang,
            en: format!("{name} is already there — pick it from the list instead"),
            zh: format!("{name} 已经有了——直接在列表里选它"),
        )
    }

    /// 建目录失败。**带上系统的原话**，理由同 `git_init_failed`。
    pub fn new_project_failed(lang: Lang, err: &str) -> String {
        t!(
            lang,
            en: format!("Could not create the folder: {err}"),
            zh: format!("目录没建成：{err}"),
        )
    }

    pub fn not_a_directory(lang: Lang, path: &str) -> String {
        t!(lang, en: format!("{path} is not a folder"), zh: format!("{path} 不是一个目录"))
    }

    /// `g` 建仓库失败。**把 git 的原话带上**：这一步是 dct 替用户敲的一条
    /// 命令，失败原因（磁盘满、目录只读、机器上压根没装 git）只有 git 自己
    /// 说得出来，吞掉它用户就只剩「没成功」三个字，连该去修什么都不知道。
    /// 这跟 `git.rs` 里那句「不要把英文原文甩到界面上」不冲突：那说的是
    /// 日常路径上的失败要有中文上下文，而这里中文上下文正是前半句。
    pub fn git_init_failed(lang: Lang, err: &str) -> String {
        t!(
            lang,
            en: format!("could not create the git project: {err}"),
            zh: format!("建 git 仓库失败：{err}"),
        )
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

    /// 等配对时的状态行——**必须点名 bot**，不然「去发条消息」没法执行
    /// （见 `ui::phone::status_line` 头上的注释和 `waiting_names_the_bot`）。
    pub fn phone_waiting_for_pairing(lang: Lang, bot: &str) -> String {
        t!(
            lang,
            en: format!("Token saved. Waiting for you to message @{bot} on Telegram"),
            zh: format!("令牌已保存，正在等你去 Telegram 给 @{bot} 发条消息"),
        )
    }

    /// 已连上时的状态行，带着主人的名字。
    pub fn phone_paired(lang: Lang, owner: &str) -> String {
        t!(
            lang,
            en: format!("Connected · {owner}"),
            zh: format!("已连上 · {owner}"),
        )
    }

    pub fn cannot_open_browser(lang: Lang, url: &str) -> String {
        t!(
            lang,
            en: format!("Could not open a browser — visit {url} yourself"),
            zh: format!("打不开浏览器，自己去访问 {url}"),
        )
    }

    /// 配对起步/轮询撞上的、dct 没有专门词条的失败原因（`PairStarted`
    /// 的 `Err` 字符串、`PairTick::Failed` 兜底那一支）。**原样带上原因码**
    /// ——报码不组句是 daemon 一侧的规矩（见 `proto::ErrorCode` 头上的
    /// 注释），这里在界面这一侧把它套进一句人话，但不替它编一个更具体的
    /// 说法：编错了比说不清更容易把人导向错误的下一步。
    pub fn pair_failed(lang: Lang, reason: &str) -> String {
        t!(
            lang,
            en: format!("Pairing failed ({reason})"),
            zh: format!("配对失败（{reason}）"),
        )
    }

    /// Waiting 屏上的倒计时。分:秒，`0` 封底——`deadline` 已经过了的那一帧
    /// 不该显示负数，轮询很快会把这一屏换成 `Failed`，这一帧只是过渡。
    pub fn pair_countdown(lang: Lang, remaining: std::time::Duration) -> String {
        let secs = remaining.as_secs();
        let (m, s) = (secs / 60, secs % 60);
        t!(
            lang,
            en: format!("{m:02}:{s:02} left"),
            zh: format!("剩余 {m:02}:{s:02}"),
        )
    }

    pub fn installing(lang: Lang, profile: &str) -> String {
        t!(
            lang,
            en: format!("Installing {profile}. When it finishes, press Esc then N."),
            zh: format!("正在安装 {profile}，装完按 Esc 回看板再按 N"),
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
            // 用户在开局提示里已经答应了「接回来」，一条没恢复成功就该有
            // 一句解释——不然一个格子悄悄消失，用户既不知道少了哪个，
            // 也不知道该拿它怎么办。三种原因对应三种不同的下一步，
            // 不能糊成一句「没恢复成功」。
            SessionResumeSkipped {
                dir,
                profile,
                reason,
            } => {
                let why = match reason {
                    crate::proto::SessionResumeSkipReason::DirGone => t!(
                        lang,
                        en: "that folder no longer exists",
                        zh: "那个目录已经不在了",
                    ),
                    crate::proto::SessionResumeSkipReason::ProfileGone => t!(
                        lang,
                        en: "that agent is no longer available",
                        zh: "那个 agent 已经不在了",
                    ),
                    crate::proto::SessionResumeSkipReason::StartFailed => t!(
                        lang,
                        en: "it could not be started again",
                        zh: "没能重新启动",
                    ),
                };
                t!(
                    lang,
                    en: format!("Could not bring back the “{profile}” session in {dir}: {why}."),
                    zh: format!("没能接回 {dir} 的「{profile}」会话：{why}。"),
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

    // ── `dct install`：自带运行时和装 agent 那条路上说的话 ──────────────
    //
    // 这一整组的读者是一个刚装完 dct、还没写过一行代码的学生。所以：
    // 不出现「运行时」「依赖」「注册表」这类词，不印任何原始报错，
    // 每一句话要么在报告进度，要么给出他此刻做得到的下一步。

    /// 开始下 Node 之前说一句。**说出体积**——几十 MB 在教室的网里可能要
    /// 几分钟，不说的话学生会以为卡死了，然后按 Ctrl+C。
    pub fn node_fetching(lang: Lang, version: &str) -> String {
        t!(
            lang,
            en: format!("Fetching the runtime the agents need (Node {version}, about 50 MB). This can take a few minutes."),
            zh: format!("正在下载 agent 要用的运行时（Node {version}，大约 50 MB），网慢的话要几分钟。"),
        )
    }

    pub fn node_ready(lang: Lang) -> String {
        t!(
            lang,
            en: "Runtime ready.",
            zh: "运行时准备好了。",
        )
        .to_string()
    }

    /// 下 git 之前那一句。**说清楚它是干嘛用的**：用户点的是「开一个
    /// agent」，屏幕上突然开始下一个 45 MB 的东西，不说一句的话那看起来
    /// 像是卡住了或者装错了什么。
    pub fn git_fetching(lang: Lang, version: &str) -> String {
        t!(
            lang,
            en: format!("This machine has no git. Fetching a portable copy for dct (git {version}, about 45 MB) — dct needs it to snapshot your project before each turn, which is what makes undo work."),
            zh: format!("这台电脑上没有 git，正在下一份便携版给 dct 用（git {version}，大约 45 MB）。dct 每轮对话前给你的项目拍快照靠的就是它，没有它撤销是死的。"),
        )
    }

    pub fn git_ready(lang: Lang) -> String {
        t!(
            lang,
            en: "git is ready.",
            zh: "git 准备好了。",
        )
        .to_string()
    }

    /// 按 `g` 之后发现缺的是 git 本身，开了个窗口装它。
    ///
    /// **说清楚「装完还要再按一次 g」**：用户按 `g` 的意图是「把仓库建起来」，
    /// 而这一步只完成了前一半，装完回到那一屏时他需要知道自己该干嘛。
    pub fn installing_git(lang: Lang) -> String {
        t!(
            lang,
            en: "This machine has no git — installing it first. When it finishes, press Esc and then g again to create the project.",
            zh: "这台电脑上没有 git，先装它。装完按 Esc 回去，再按一次 g 就能建仓库了。",
        )
        .to_string()
    }

    /// 替用户把 git 仓库建好了，会话正在起来。
    ///
    /// **说的是「做了什么」和「为什么」，不是「你得先做什么」**：这一句
    /// 出现的时候事情已经办完了，用户不需要采取任何行动。写清楚是因为
    /// 我们动了他的文件夹（多了一个 `.git`），他有权知道那是谁干的。
    pub fn git_repo_created_for_you(lang: Lang) -> String {
        t!(
            lang,
            en: "This folder was not a git project, so dct made it one — that is what lets you undo what the agent does.",
            zh: "这个文件夹还不是 git 仓库，dct 顺手建好了——撤销 agent 的改动要靠它。",
        )
        .to_string()
    }

    pub fn git_already_installed(lang: Lang) -> String {
        t!(
            lang,
            en: "git is already installed on this machine.",
            zh: "这台电脑上已经有 git 了。",
        )
        .to_string()
    }

    /// 那份便携 git 下不到时的镜像提示。
    ///
    /// 跟 `mirror_hint` 分开：那条说的是 `DCT_NODE_BASE` / `DCT_NPM_REGISTRY`，
    /// 对着它设一天也不会让 git 下下来。这里给的是**整个地址**而不是前缀，
    /// 理由见 `runtime::mingit_url`。
    pub fn git_mirror_hint(lang: Lang) -> String {
        t!(
            lang,
            en: "If your network cannot reach that address, put the same zip somewhere you can download from, set this, and run it again:\n    DCT_MINGIT_URL=<your address>",
            zh: "如果你这儿连不上那个地址，把同一个 zip 放到一个能下的地方，设上这个再跑一遍：\n    DCT_MINGIT_URL=<你的地址>",
        )
        .to_string()
    }

    /// 这台机器上没有 git，而 dct 也没法替他装（非 Windows）。
    ///
    /// **一定要带上那条能照抄的命令**：这个工具的用户是训练营的学生，
    /// 「请先安装 git」对他们等于什么都没说。命令按平台分，因为两个平台
    /// 的答案完全不一样，给错了比不给更糟。
    pub fn git_missing_install_it_yourself(lang: Lang) -> String {
        let how = match std::env::consts::OS {
            "macos" => "xcode-select --install",
            _ => "sudo apt install git",
        };
        t!(
            lang,
            en: format!("This machine has no git, and dct can only install it for you on Windows. Run this, then start dct again:\n    {how}"),
            zh: format!("这台电脑上没有 git，而 dct 只能在 Windows 上替你装。先敲这一条，再重新打开 dct：\n    {how}"),
        )
    }

    /// 下不到。**印出地址**：学生答不上「你连的是哪儿」，而老师一眼就能
    /// 看出他是不是漏设了镜像。
    pub fn download_unreachable(lang: Lang, url: &str) -> String {
        t!(
            lang,
            en: format!("Could not download from {url} — the network could not reach it."),
            zh: format!("下不到东西：{url} 连不上。"),
        )
    }

    pub fn download_corrupt(lang: Lang) -> String {
        t!(
            lang,
            en: "What came down does not match the official checksum, so dct did not install it. This is usually a download cut short — run the same command again.",
            zh: "下回来的文件跟官方校验和对不上，没有装它。多半是下到一半断了，把刚才那条命令再跑一次。",
        )
        .to_string()
    }

    pub fn no_node_for_platform(lang: Lang) -> String {
        t!(
            lang,
            en: "There is no ready-made runtime for this kind of computer. Install Node.js yourself from nodejs.org, then try again.",
            zh: "这种电脑没有现成的运行时可下。自己去 nodejs.org 装一个 Node.js，再回来试一次。",
        )
        .to_string()
    }

    pub fn cannot_unpack(lang: Lang) -> String {
        t!(
            lang,
            en: "The runtime came down but could not be unpacked. Run the same command again; if it keeps happening, tell whoever set this up.",
            zh: "运行时下回来了，但解不开。把刚才那条命令再跑一次；一直这样就告诉给你装这套东西的人。",
        )
        .to_string()
    }

    pub fn cannot_write_runtime(lang: Lang, dir: &str) -> String {
        t!(
            lang,
            en: format!("dct could not write to {dir}. Check the disk is not full and that you can write there."),
            zh: format!("写不进 {dir}。看一眼硬盘是不是满了，以及你有没有权限往那儿写。"),
        )
    }

    /// 网络这一类的失败之后补的一句。**把两个地址原样印出来**，让老师
    /// 抄一行给学生，而不是让学生拿着「换个镜像」四个字去搜。
    pub fn mirror_hint(lang: Lang, node_base: &str, registry: &str) -> String {
        t!(
            lang,
            en: format!(
                "If you are on a network that cannot reach the usual sources, set these two first and run it again:\n    DCT_NODE_BASE={node_base}\n    DCT_NPM_REGISTRY={registry}"
            ),
            zh: format!(
                "如果你这儿连不上默认的下载源，先设这两个再跑一遍：\n    DCT_NODE_BASE={node_base}\n    DCT_NPM_REGISTRY={registry}"
            ),
        )
    }

    pub fn installing_agent(lang: Lang, label: &str) -> String {
        t!(
            lang,
            en: format!("Installing {label}."),
            zh: format!("正在安装 {label}。"),
        )
    }

    /// 装完了，而且**真的去查了一遍**那个命令现在找得到——「npm 说成功了」
    /// 和「敲得出这个命令了」不是同一件事。
    pub fn install_succeeded(lang: Lang, label: &str) -> String {
        t!(
            lang,
            en: format!("{label} is installed. Press Esc to go back to the board, then N to start it."),
            zh: format!("{label} 装好了。按 Esc 回看板，再按 N 就能开它。"),
        )
    }

    /// npm 说它成功了，但那个命令还是找不到。这不是理论情况：npm 装到
    /// 别的 prefix 去、或者包本身没带 bin，都会长成这样。
    pub fn install_finished_but_missing(lang: Lang, command: &str) -> String {
        t!(
            lang,
            en: format!("The install finished without complaining, but `{command}` still is not there. Tell whoever set this up — this one is not something you can fix from here."),
            zh: format!("安装过程没报错，但 `{command}` 还是不在。这一条你自己修不了，告诉给你装这套东西的人。"),
        )
    }

    pub fn install_failed(lang: Lang, label: &str) -> String {
        t!(
            lang,
            en: format!("Could not install {label}. The lines above are what the installer itself said."),
            zh: format!("{label} 没装成。上面那几行是安装程序自己说的话。"),
        )
    }

    pub fn unknown_agent(lang: Lang, name: &str) -> String {
        t!(
            lang,
            en: format!("dct does not know an agent called `{name}`."),
            zh: format!("dct 不认识一个叫 `{name}` 的 agent。"),
        )
    }

    pub fn agent_has_no_installer(lang: Lang, label: &str) -> String {
        t!(
            lang,
            en: format!("{label} has no install command of its own, so dct cannot install it for you."),
            zh: format!("{label} 没有配安装命令，dct 没法替你装它。"),
        )
    }

    pub fn agent_already_installed(lang: Lang, label: &str) -> String {
        t!(
            lang,
            en: format!("{label} is already installed."),
            zh: format!("{label} 已经装好了。"),
        )
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
            GotoProject,
            RemoveProject,
            ToggleCollapse,
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
            EnterCopyMode,
            PasteImage,
            WebSection,
            WebOffLine,
            WebOnLine,
            WebAddressUnknownLine,
            WebNextStepOff,
            WebFirewall,
            WebNextStepOn,
            WebNextStepAddressUnknown,
            WebQrTooNarrow,
            TextSmaller,
            TextBigger,
            KeyboardCapture,
            WebToggle,
            WebTurnOn,
            TapLineToInsert,
            WebTurnOff,
            OtherKeysGoToAgent,
            BackToListWord,
            BackToSettingsWord,
            OrPressDigit,
            Language,
            BarTheme,
            ThemeGray,
            ThemeBlue,
            ThemeIndigo,
            ThemeTeal,
            ThemeGreen,
            ThemeOlive,
            ThemeAmber,
            ThemeCrimson,
            ThemeMagenta,
            ThemePurple,
            ThemeSlate,
            ThemeLight,
            ThemePaper,
            ThemeLines,
            Phone,
            BackToBoard,
            BackToBoardF2,
            BackToList,
            ClearSearch,
            BackToRecent,
            BackToSettings,
            BoardTitle,
            ProjectChipLabel,
            Disconnected,
            PickAgentTitle,
            NotAGitRepoHint,
            InitGitRepo,
            GitRepoCreated,
            PickProjectTitle,
            TypePathTitle,
            SettingsTitle,
            CurrentProject,
            ManualPath,
            NewProject,
            RecentProjects,
            SearchPlaceholder,
            BrowseFolders,
            OpenProject,
            UseThisFolder,
            NoMatchingProject,
            EnterFolder,
            GoUp,
            NoSubfolders,
            NewProjectPrompt,
            NewProjectNoName,
            NewProjectBadName,
            HiddenCharsInName,
            NoSessionsHere,
            PhoneOffline,
            PhoneComputerGone,
            PhoneComputerSilent,
            LastUsedAgent,
            NoSessionsRunningPressN,
            ProjectDirGone,
            // 早就在词条表里，却一直没进这份清单——于是两条守卫（两种语言都
            // 组得出话、英文里不许有汉字）从来没查过它。顺手补上：它正是九宫格
            // 另一种空态用的那一条，跟刚加的 NoSessionsRunningPressN 挨着。
            AllSessionsStopped,
            WindowTooSmall,
            Verifying,
            PasteOrTypeKey,
            NoOtherRunningSession,
            NoImageInClipboard,
            NoSessionSelected,
            DaemonUnreachable,
            StaleData,
            CopyMode,
            CopyModeShort,
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
            RestartExplain,
            RestartAsk,
            RestartCancelled,
            RestartDone,
            RestartFailed,
            RestartNothingToRestart,
            RestartStartedDaemonOnly,
            RestartStartFailed,
            ResumeSessionsExplain,
            ResumeSessionsAsk,
            ResumeSessionsWillContinue,
            ResumeSessionsWillStartFresh,
            RequestFailed,
            ActionDone,
            NoChanges,
            ReasonNeedsSecret,
            ReasonNotInstalled,
            GroupNotEmpty,
            AutoPair,
            PairContacting,
            PairEnterCodeInBrowser,
            PairCodeExpired,
            PairKeyUnreadable,
            PairDenied,
            PairNotEnabled,
            PairDoneBoth,
            PairDoneQwenOnly,
            PairManualHint,
            PairLlmOptIn,
            ReopenBrowser,
            Retry,
            ManualEntry,
        ]
    };

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
        assert_eq!(ALL_KEYS.len(), 181, "加了 Key 变体就要同步进 ALL_KEYS");
        let mut seen: Vec<String> = ALL_KEYS.iter().map(|k| format!("{k:?}")).collect();
        seen.sort();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "ALL_KEYS 里有重复项");
    }

    /// `CopyModeShort` 是复制模式提示放不下长文案时的退路，必须两种语言
    /// 都放得进 `ui::ACTION_MIN_COLS`——那是底栏右段唯一保证的宽度。这条提示
    /// 又是全屏唯一写着 F4（怎么退出复制模式）的地方：退路本身放不下，
    /// 用户就会卡在一个看不见也出不去的模式里，跟 `ESCAPE_HINT_COLS` 那条
    /// 守卫（`src/ui/mod.rs` 的 `escape_hint_cols_fits_every_view`）防的是
    /// 同一类事故。
    #[test]
    fn copy_mode_short_fits_the_action_floor_in_every_language() {
        use unicode_width::UnicodeWidthStr;

        for l in Lang::all() {
            let short = text(Key::CopyModeShort, *l);
            assert!(
                short.width() <= crate::ui::ACTION_MIN_COLS as usize,
                "{l:?} 下复制模式的短文案「{short}」宽 {} 列，放不进 ACTION_MIN_COLS = {}",
                short.width(),
                crate::ui::ACTION_MIN_COLS
            );
        }
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
            SessionResumeSkipped {
                dir: "/w/dc-terminal".into(),
                profile: "claude".into(),
                reason: crate::proto::SessionResumeSkipReason::DirGone,
            },
            SessionResumeSkipped {
                dir: "/w/dc-terminal".into(),
                profile: "claude".into(),
                reason: crate::proto::SessionResumeSkipReason::ProfileGone,
            },
            SessionResumeSkipped {
                dir: "/w/dc-terminal".into(),
                profile: "claude".into(),
                reason: crate::proto::SessionResumeSkipReason::StartFailed,
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
