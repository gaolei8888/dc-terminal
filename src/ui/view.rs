use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::i18n::Lang;
use crate::profile::ProfileStatus;
use crate::proto::{ProfileEntry, SecretPrompt};
use crate::session::SessionState;
use crate::verify::VerifyOutcome;

use super::Msg;

/// 九宫格里那行回复框正在攒的一句话。
#[derive(Clone)]
pub(crate) struct Draft {
    /// 收件人的**会话 id**，不是格子下标。
    ///
    /// 下标会漂：会话被停掉、翻页、切作用域，都会让同一个下标指向另一个
    /// 会话。用户是对着某一个 agent 打的这句话，中途下标漂了就发给了别人，
    /// 而发出去撤不回来。所以按下 `i` 的那一刻就把收件人钉死，之后不管
    /// 焦点怎么动都不改。
    pub id: u32,
    pub text: String,
}

/// 回复框收到一个键之后该干什么。
///
/// 抽成纯函数 + 这个枚举，是为了让「发什么、发完框关不关」能脱离守护进程
/// 直接测。真正容易写错的正是这一段：半句话被留在框里、下次开框一起发出去，
/// 或者取消了却还是发了——这些都是撤不回来的错，不能只靠手点。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Reply {
    /// 继续编辑，框里现在是这些字
    Typing(String),
    /// 关掉，什么都不发
    Cancel,
    /// 关掉，把这些字发给 agent，再替他按一下回车。空串 = 只按回车。
    Send(String),
    /// 关掉，发一个中断（Ctrl+C）给 agent
    Interrupt,
}

/// 一个键落进回复框里的结果。
///
/// 这里**不认**任何动作键：框开着的时候 `s`/`u`/`d` 就是三个字母，不是
/// 停止/回滚/看改动。否则用户打「so」就把会话停了——而停止不可撤销。
pub(crate) fn reply_key(text: &str, key: &KeyEvent) -> Reply {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => Reply::Cancel,
        // 跟会话视图里一样：Ctrl+C 是「打断它」，不是「关掉这个框」。
        // 用户在九宫格里看见某个 agent 跑偏了，这是最快的一脚刹车。
        KeyCode::Char('c') if ctrl => Reply::Interrupt,
        KeyCode::Enter => Reply::Send(text.to_string()),
        KeyCode::Backspace => {
            let mut t = text.to_string();
            // `pop` 按**字符**删，一个汉字一下。按字节删会把 UTF-8 切碎。
            t.pop();
            Reply::Typing(t)
        }
        // Ctrl/Alt 组合键不当字符收：它们在终端里是控制序列，收进来会变成
        // 一串看不懂的字，而用户以为自己只是按了个快捷键。
        KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
            Reply::Typing(format!("{text}{c}"))
        }
        // 其余（方向键、F 键、Tab…）一律吞掉、原样保留正在打的字。
        // 不转发给 agent：转发就等于把格子变成了半个附加视图，而格子的
        // PTY 尺寸根本撑不起 agent 的完整界面（见 `View::Grid` 的注释）。
        _ => Reply::Typing(text.to_string()),
    }
}

impl View {
    /// 九宫格视图，回复框关着。
    ///
    /// **所有「进/回九宫格」的地方都走这里。** 把 `reply` 显式写成 `None`
    /// 散落在几十个构造点上，只要漏一处就是「切个视图回来，刚打的半句话
    /// 还在框里」——而那半句话下一个回车就发给 agent 了。开框永远是用户的
    /// 一次显式动作（按 `i`），没有任何一条路径该顺手把它带开。
    pub(crate) fn grid(focus: usize) -> View {
        View::Grid { focus, reply: None }
    }
}

#[derive(Clone)]
pub(crate) enum View {
    Board,
    Attached(u32),
    /// 九宫格：平铺所有会话的实时画面。`focus` 是**全体会话**里的下标
    /// （不是当页内的），当前页从它推导，见 `grid::page_of`。
    ///
    /// **格子本身始终只读**，这是设计约束不是偷懒：一个会话的 PTY 只有一份
    /// 尺寸，要让 agent 的完整界面在格子里能用，就得把 PTY 缩到格子那么小
    /// ——80×24 终端下格子内区只有 25×5，Claude Code 光是底部那条状态栏就要
    /// 折三行，输入框根本放不进去。所以键盘不往格子里转发，`s`/`u`/`d` 这些
    /// 字母留给动作键。要完整交互按 Enter 放大成附加视图。
    ///
    /// `reply` 是那条约束**之外**的一个小口子：一行回复框，用来不离开九宫格
    /// 就回 agent 一句（批个计划、答个是非题）。它不碰 PTY 尺寸、不渲染
    /// agent 的光标、不改协议——光标是这个输入框自己的，文字靠现成的
    /// `Request::Input` 送出去。`None` = 框没开，键盘照旧全是动作键。
    Grid {
        focus: usize,
        reply: Option<Draft>,
    },
    PickProfile {
        entries: Vec<ProfileEntry>,
        state: ListState,
        /// 密钥文件读不了、自定义 profile 写错了。顶部红字。
        warning: Option<String>,
    },
    PickProject(ProjectPicker),
    /// 设置页：目前只有语言一项。跟 `Secrets` 分开是两码事——那边管的是
    /// 「哪个 agent 用哪把密钥」，这里管的是界面本身怎么显示。
    Settings {
        state: ListState,
    },
    EnterSecret {
        /// agent 的内部名字（比如 "kimi"），存密钥、建会话都要靠它
        profile: String,
        /// 界面上给用户看的名字（比如 "Kimi"）——profile 是内部标识，不能直接出现在标题里
        label: String,
        /// daemon 给的填写提示：一句人话 + 可能有的申领页链接
        prompt: SecretPrompt,
        /// 用户正在打的密钥，明文只活在这一份里，渲染时永远转成圆点
        buf: String,
        phase: SecretPhase,
        /// 从设置页进来的要回设置页（意图是改配置），从选择器进来的直接开会话
        /// （意图是开工）。两条路都会落到这同一个视图，成功之后该去哪不能靠
        /// 猜——建这个视图的地方必须显式填它，别指望靠别的字段反推。
        return_to_settings: bool,
    },
    /// 密钥设置页：看板按 `c` 进，只列声明了密钥的 profile（见 `secret_rows`）。
    /// 跟 `PickProfile` 分开是两码事——那边是「选一个能干活的 agent」，
    /// 这边是「管理密钥本身」，选中的动作也完全不同（改/删，而不是开会话）。
    Secrets {
        entries: Vec<ProfileEntry>,
        state: ListState,
        /// `d` 在密钥页是真删除，但物理按键跟看板上「看 diff」那个无害的 `d`
        /// 完全一样——肌肉记忆会跨屏幕迁移，反应性的一按不该直接删掉一份
        /// 用户可能只粘贴过一次、关掉网页就找不回来的密钥。两段式确认：
        /// 第一次 `d` 把这里填成 `Some(profile 名字)`（武装），行内画出
        /// 「再按 d 删除」；第二次 `d` 打在同一行上才真的发
        /// `Request::DeleteSecret`。存名字而不是下标是因为列表会因为增删
        /// 重新排列，下标会指错行，名字不会。
        ///
        /// 武装状态和「光标选中哪一行」必须永远同步——除了确认删除的第二次
        /// `d` 本身，任何按键（包括 ↑↓）都要把这里清回 `None`，不然挪开
        /// 光标之后按下的第二次 `d` 删的会是用户已经不记得自己武装过的
        /// 那一行。
        pending_delete: Option<String>,
    },
}

/// 填密钥这一屏正处在哪个阶段。`Verifying` 期间输入被冻结——buf 已经发给
/// 后台线程了，这时候改它不会影响正在飞的那次验证，只会让用户误以为
/// 下一次回车用的是新值。`Failed` 带着人话版的失败原因，渲染在圆点行下面。
#[derive(Clone)]
pub enum SecretPhase {
    Typing,
    Verifying,
    Failed(String),
}

/// 这个按键是「光板的」——没有按 Alt/Meta。
///
/// 用在看板和密钥页那些一个字母就干实事的分支上（退出、删除、跳走）。
/// 理由是语义本身：`Alt+c` 不是 `c`，让它触发 `c` 的动作是 bug——这个守卫
/// 只管这一件事，也正因为这件事本身就该管，所以要留着。
///
/// 这**不是**防「转义序列漏进 stdin 被当成按键」的第二道防线——曾经的
/// 注释这么说过，是错的。crossterm 0.28.1 只把 ESC 后紧跟的那一个字节
/// 标成 Alt（`event/sys/unix/parse.rs`），发出这个事件后立刻清空解析
/// 缓冲区（`event/source/unix/tty.rs`），后面的字节是从头重新解析、不带
/// 任何修饰符的。所以 `\x1b]11;rgb:cdcd/dddd/dddd\x07` 一旦漏出来，`]`
/// 是 `Alt+']'`，但紧跟着的 `cdcd`、`dddd` 里每个 `c`/`d` 都是光板
/// `Char('c')`/`Char('d')` 事件——这个守卫在那条路径上完全拦不住。真正
/// 防「回复变按键」的是 `theme.rs` 里的 DA1 哨兵和 isatty 判断，不是这
/// 里；别指望这层守卫替哨兵兜底，也别因为这层守卫在就去削弱哨兵。
pub(crate) fn is_plain_key(key: &KeyEvent) -> bool {
    !key.modifiers.contains(KeyModifiers::ALT) && !key.modifiers.contains(KeyModifiers::META)
}

/// Ctrl+Q —— dct 的全局逃生键。
///
/// crossterm 把它报成 `Char('q')` 带 `CONTROL` 修饰，有的终端送大写。
/// 判断必须放在任何 `Char(c)` 分支**之前**：项目选择器的打字过滤是靠
/// `Char(c)` 累加的，判晚了会往过滤框里塞一个 `q`。
pub(crate) fn is_ctrl_q(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
}

/// Ctrl+Q 从当前视图退到哪一层。`None` 表示「退到头了，该退出 dct」。
///
/// 抽成纯函数是为了能单测——`run()` 的按键循环要连真 socket，测不了。
///
/// `EnterSecret` 退回哪一层要看 `return_to_settings`：从密钥设置页进来的
/// 回设置页（用户在管理配置），从选择器进来的回选择器（用户很可能只是
/// 选错了 agent，回选择器比回看板更顺手）。但这里是个纯函数，拿不到 daemon
/// 连接，现查不出一份新的条目列表，只能给一个 `entries: vec![]` 的空壳。
/// **调用方必须知道这个约定**：`run()` 的按键循环里，只要处理完一次按键后
/// 发现 `view` 变成了空的 `PickProfile`/`Secrets`（不管是走这个函数的
/// Ctrl+Q，还是 `EnterSecret` 自己的 Esc 分支），就要补一次
/// `Request::Profiles` 把条目填上——不然用户看到的是一屏空白，会以为自己
/// 一个 agent/密钥都没有。
pub(crate) fn back_one_level(view: View) -> Option<View> {
    match view {
        View::Board => None,
        // 手输态退一层是回到两栏那一屏，不是关掉整个选择器
        View::PickProject(p) if p.typing_path.is_some() => Some(View::PickProject(ProjectPicker {
            typing_path: None,
            ..p
        })),
        View::EnterSecret {
            return_to_settings: true,
            ..
        } => Some(View::Secrets {
            entries: Vec::new(),
            state: ListState::default(),
            pending_delete: None,
        }),
        View::EnterSecret { .. } => Some(View::PickProfile {
            entries: Vec::new(),
            state: ListState::default(),
            warning: None,
        }),
        // 九宫格跟列表是**平级**的两个模式，它自己就是顶层，退无可退——
        // 跟 `View::Board` 一样返回 `None`。以前它落在下面那条兜底里，
        // 于是 Ctrl+Q 成了 `g` 的隐藏同义词。
        View::Grid { .. } => None,
        // Secrets 落在这条兜底里：它跟 Attached/PickProject 一样只有一层，
        // 退一层就是看板。
        _ => Some(View::Board),
    }
}

/// 选中某个 profile 之后该干什么。四种：能用的直接建会话；缺密钥的去填密钥；
/// 没装但有安装命令的去装；没装又没法自动装的、或者缺别的 profile 依赖的，
/// 只能告诉用户一句话，不切视图。
#[derive(Debug)]
pub enum PickAction {
    Start(String),
    /// 下标是占位——`pick_action` 只拿得到一个 `&ProfileEntry`，不知道它在
    /// 列表里排第几，这里永远填 0。真下标只有调用方知道（它是从 `entries[i]`
    /// 拿到这个 entry 的），必须由调用方在按键分支里覆盖，不能信这个值。
    AskSecret(usize),
    Install {
        profile: String,
        command: Vec<String>,
    },
    Blocked(String),
}

/// 按下某一项时该干什么。抽成纯函数是为了能单测——`run()` 的按键循环
/// 要连真 socket，测不了（同 `back_one_level`）。
pub fn pick_action(e: &ProfileEntry, lang: Lang) -> PickAction {
    match &e.status {
        ProfileStatus::Ready => PickAction::Start(e.name.clone()),
        ProfileStatus::NeedsSecret => PickAction::AskSecret(0),
        ProfileStatus::NeedsDependency { label } => {
            PickAction::Blocked(crate::i18n::msg::needs_dependency(lang, label, &e.label))
        }
        ProfileStatus::NotInstalled { command } => match &e.install {
            Some(i) => PickAction::Install {
                profile: e.name.clone(),
                command: i.command.clone(),
            },
            // 手写的自定义 profile 可能整个没填 command（TOML 里写
            // `command = []`），`status_of` 兜底成 `NotInstalled { command: "" }`。
            // 这时候「本机没有找到 」后面空着一截，用户看了不知道该找什么——
            // 干脆点名是这个 profile 本身没配置要跑什么，而不是暗示去装一个
            // 不存在的空名字命令。
            None if command.is_empty() => {
                PickAction::Blocked(crate::i18n::msg::no_command_configured(lang, &e.label))
            }
            None => PickAction::Blocked(crate::i18n::msg::command_not_found(lang, command)),
        },
    }
}

/// 密钥设置页要列哪些行：只列声明了密钥的 profile——`claude`/`codex`/`命令行`
/// 这种不需要密钥的东西出现在这一页只会让用户以为自己也得配点什么。
///
/// 「已配」读的是 `has_secret`，不是拿 `status != NeedsSecret` 反推。后者
/// 有个边界：`status_of` 里「装没装排在密钥前面」（见 profile.rs），一个
/// CLI 还没装的 profile，不管密钥填没填，`status` 都会报
/// `NeedsDependency`/`NotInstalled`，从它反推不出真实的密钥状态。
/// `has_secret` 是 daemon 直接从密钥仓查出来的事实，不掺这层判断，
/// 这也是为什么它作为独立字段搭在 `ProfileEntry` 上而不是从 `status` 算出来。
pub fn secret_rows(entries: &[ProfileEntry]) -> Vec<(String, bool)> {
    entries
        .iter()
        .filter(|e| e.secret.is_some())
        .map(|e| (e.name.clone(), e.has_secret))
        .collect()
}

/// 密钥页 `d` 键该干什么——判断这一半抽成纯函数，是因为它不碰网络，
/// 值得单测；真发 `Request::DeleteSecret` 那一半留在 `run()` 里，因为
/// 那需要 daemon 连接，这个模块里所有 `client.call` 分支都是这样处理的。
///
/// `d` 在这一页是真删除，但物理键跟看板上「看 diff」那个无害的 `d` 完全
/// 一样，肌肉记忆会带过来——所以这里是两段式：第一次按 `d` 只武装
/// （[`DeleteKeyAction::Arm`]），必须选中同一行再按第二次才会
/// [`DeleteKeyAction::Confirm`]。`target` 是当前选中行的
/// `(名字, 是否已配)`，`pending_delete` 是武装状态（存名字，不存下标，
/// 因为列表会因为增删重新排列，下标会指错行）。
#[derive(Debug, PartialEq, Eq)]
pub enum DeleteKeyAction {
    /// 光标没落在任何一行上（比如列表是空的）
    NoSelection,
    /// 选中的行还没配密钥，删不出什么名堂，只提示
    NotConfigured,
    /// 第一次按 d：武装到这个名字，不发任何请求
    Arm(String),
    /// 第二次按 d，且武装的名字跟当前选中行一致：真删
    Confirm(String),
}

pub fn decide_delete_key(
    target: Option<(String, bool)>,
    pending_delete: &Option<String>,
) -> DeleteKeyAction {
    match target {
        None => DeleteKeyAction::NoSelection,
        Some((_, false)) => DeleteKeyAction::NotConfigured,
        // 按名字比对而不是「只要武装了就删」：这条不变量（武装状态永远
        // 等于选中行）本该由「挪光标必须清空 pending_delete」保证，但
        // 名字比对是最后一道保险——就算别处哪天漏改了一条清空分支，
        // 这里也不会把武装的确认动作错按到另一行头上。
        Some((name, true)) => {
            if pending_delete.as_deref() == Some(name.as_str()) {
                DeleteKeyAction::Confirm(name)
            } else {
                DeleteKeyAction::Arm(name)
            }
        }
    }
}

/// `n` 该直接开哪个 agent。`None` = 没得直开，进选择器。
///
/// 目标用户是非程序员：让他每次在九个 agent 里挑一个是设计失败——他不知道区别。
/// 日常路径压成一个按键，想换的人按 N。只有「上次那个现在仍然 Ready」才直开：
/// 密钥被删、CLI 被卸、自定义 profile 被改没了，都不是 Ready，直开只会把人
/// 扔进一个起不来的窗口，还不如回选择器让他看见状态和原因。
pub fn quick_start_target(last: Option<&str>, entries: &[ProfileEntry]) -> Option<String> {
    let last = last?;
    entries
        .iter()
        .find(|e| e.name == last && e.status == ProfileStatus::Ready)
        .map(|e| e.name.clone())
}

/// `'1'..'9'` → `0..8`。`'0'` 不算——第 10 项要用 ↑↓ 选。
pub fn digit_index(c: char) -> Option<usize> {
    match c {
        '1'..='9' => Some(c as usize - '1' as usize),
        _ => None,
    }
}

/// 粘进来的密钥清洗一遍。用户从网页或接口文档里拷贝，经常带上引号、
/// `Bearer ` 前缀和尾随换行——让他自己发现并删掉是不现实的。
pub fn clean_secret(s: &str) -> String {
    let t = s.trim();
    let t = t.strip_prefix('"').unwrap_or(t);
    let t = t.strip_suffix('"').unwrap_or(t);
    let t = t.strip_prefix('\'').unwrap_or(t);
    let t = t.strip_suffix('\'').unwrap_or(t);
    let t = t.trim();
    t.strip_prefix("Bearer ").unwrap_or(t).trim().to_string()
}

/// 验证结果给用户看的话。`None` 表示放行。
///
/// `Unreachable` 必须说网络的问题，不能说密钥的问题——用户的密钥可能
/// 完全没问题，只是这台机器连不上服务器；把锅甩给密钥会让他白跑一趟
/// 去重新生成一个根本不需要换的 key。
pub fn verify_message(o: VerifyOutcome, lang: Lang) -> Option<String> {
    match o {
        VerifyOutcome::Ok => None,
        VerifyOutcome::BadKey => Some(crate::i18n::text(crate::i18n::Key::BadSecret, lang).into()),
        VerifyOutcome::Unreachable => {
            Some(crate::i18n::text(crate::i18n::Key::NetworkUnreachable, lang).into())
        }
    }
}

/// 一次密钥验证的结果，还能不能用在眼前这一屏上。
///
/// CRITICAL 1（最终整分支 code review）：验证是异步的——发起时把
/// `(profile, buf)` 交给后台线程，结果送回来可能是好几秒之后。这几秒里
/// 用户完全可能已经 Ctrl+Q/Esc 退出这一屏，甚至绕回来在**另一个** agent
/// 身上重新填了密钥。旧代码收结果时只看"现在还是不是 `EnterSecret`
/// 视图"，对不上具体是哪个 profile、填的是哪份密钥——于是一次迟到的
/// 「Kimi 的密钥验证通过了」被套在了此刻屏幕上「GLM，密钥框还是空的」
/// 这一份状态上，`SetSecret { profile: "glm", value: "" }` 直接把 GLM
/// 已经存好的密钥用空串冲掉，界面还告诉用户「已保存」。
///
/// 抽成纯函数是为了能直接单测这条判断本身——通过真实的 5 秒验证窗口去
/// 人工踩这个时间窗口不现实，但「发起时的身份」和「此刻屏幕上的身份」
/// 要不要相等，是一次纯粹的比较，不需要真连 daemon 就能覆盖。
pub fn verify_outcome_applies_to(
    issued_profile: &str,
    issued_buf: &str,
    current_profile: &str,
    current_buf: &str,
) -> bool {
    issued_profile == current_profile && issued_buf == current_buf
}

/// 把用户敲进来的路径变成绝对路径：`~` 展开成家目录，相对路径按 `base` 解析。
/// 只做字符串层面的展开，**不做存在性校验**——调用方自己决定不存在时怎么办。
pub(crate) fn expand_path(input: &str, base: &Path) -> PathBuf {
    // 粘贴进来的路径经常带尾随空格
    let t = input.trim();
    let home = || PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));

    if t == "~" {
        return home();
    }
    // 只认 `~/`：`~foo` 是别人的家目录（我们不支持），当普通相对路径处理
    if let Some(rest) = t.strip_prefix("~/") {
        return home().join(rest);
    }
    let p = Path::new(t);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// 不区分大小写的子串过滤。匹配**完整路径**而不只是目录名，
/// 这样 `work` 和 `dc-term` 都能用来找同一个项目。
pub(crate) fn filter_projects(all: &[String], filter: &str) -> Vec<String> {
    if filter.is_empty() {
        return all.to_vec();
    }
    let f = filter.to_lowercase();
    all.iter()
        .filter(|p| p.to_lowercase().contains(&f))
        .cloned()
        .collect()
}

/// 哪一栏有焦点。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Recent,
    Browse,
}

/// 选项目那一层浮层的全部状态。
///
/// 收成一个结构体而不是继续在 `View::PickProject` 上平铺字段：字段从 4 个
/// 涨到 8 个之后，每处 `match` 解构都要抄一长串，而且加一个字段就得改遍
/// 所有分支。
#[derive(Clone)]
pub struct ProjectPicker {
    /// 守护进程给的最近项目，过滤不改动它
    pub recent: Vec<String>,
    pub recent_state: ListState,
    /// 浏览器现在停在哪个目录
    pub cwd: PathBuf,
    pub entries: Vec<DirRow>,
    pub browse_state: ListState,
    pub focus: Pane,
    /// **只作用于当前焦点那一栏。** 两栏共用一个过滤词的话，用户在左边打字
    /// 找项目，右边的目录列表会跟着变空，而他并没有要求那件事。
    pub filter: String,
    /// Some 表示正处在「手输路径」的输入态
    pub typing_path: Option<String>,
}

impl ProjectPicker {
    pub fn new(recent: Vec<String>, cwd: PathBuf) -> ProjectPicker {
        let entries = list_dirs(&cwd);
        let mut recent_state = ListState::default();
        recent_state.select(Some(0));
        let mut browse_state = ListState::default();
        if !entries.is_empty() {
            browse_state.select(Some(0));
        }
        ProjectPicker {
            recent,
            recent_state,
            cwd,
            entries,
            browse_state,
            // 开在「最近」那一栏：绝大多数时候用户要的项目就在里面，
            // 浏览器是给「不在里面」那种情况准备的。
            focus: Pane::Recent,
            filter: String::new(),
            typing_path: None,
        }
    }

    /// 把浏览器挪到另一个目录，并把光标收回第一行——换了目录还留着旧行号，
    /// 光标会落在一个跟刚才毫无关系的条目上。
    pub fn browse_to(&mut self, dir: PathBuf) {
        self.entries = list_dirs(&dir);
        self.cwd = dir;
        self.browse_state.select(if self.entries.is_empty() {
            None
        } else {
            Some(0)
        });
        // 过滤词是对着上一个目录打的，换了目录就不成立了
        self.filter.clear();
    }

    /// 当前焦点那一栏里，过滤之后真正显示的行。
    pub fn shown_recent(&self) -> Vec<String> {
        match self.focus {
            Pane::Recent => filter_projects(&self.recent, &self.filter),
            Pane::Browse => self.recent.clone(),
        }
    }

    pub fn shown_entries(&self) -> Vec<DirRow> {
        match self.focus {
            Pane::Browse if !self.filter.is_empty() => {
                let f = self.filter.to_lowercase();
                self.entries
                    .iter()
                    .filter(|r| r.name.to_lowercase().contains(&f))
                    .cloned()
                    .collect()
            }
            _ => self.entries.clone(),
        }
    }
}

/// 目录浏览器里的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirRow {
    pub name: String,
    /// 是不是 git 仓库。**只是个提示标记，不是准入判断**——真正的判断在
    /// `session.rs` 建会话的时候。
    pub is_git: bool,
}

/// 这些目录不进浏览器。它们不是「被隐藏的内容」，而是本来就不该出现的噪音：
/// 目标用户是非程序员，`node_modules` 出现在选项目的列表里对他既没有意义
/// 也没有用处（见 `dc_classroom/CLAUDE.md` 的目标用户约束）。
const NOISE_DIRS: &[&str] = &["node_modules", "target", "build", "dist", "vendor"];

/// 列出一个目录下**可以当项目选**的子目录，按名字排序。
///
/// 读不了（权限、目录没了）就返回空表，不报错：用户可能浏览到任何地方，
/// 一个进不去的目录不该让整个界面倒下。调用方拿空表去显示「这里没有目录」，
/// 跟真的空目录同一个落点——对用户来说这两种情况能做的事完全一样。
///
/// **判 git 用 `stat` 而不是 `git::is_repo`**：后者要 fork 一个 git 进程，
/// 一个目录几十个子目录就是几十次 fork，翻目录会明显发卡。stat 判得没那么全
/// （worktree、`GIT_DIR` 都会漏），但这只是个提示标记。
pub(crate) fn list_dirs(dir: &Path) -> Vec<DirRow> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut rows: Vec<DirRow> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || NOISE_DIRS.contains(&name.as_str()) {
                return None;
            }
            let is_git = e.path().join(".git").exists();
            Some(DirRow { name, is_git })
        })
        .collect();
    // read_dir 的顺序由文件系统决定；不排的话同一个目录每次打开都可能换序，
    // 用户没法靠位置记住东西在哪。
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// 看板的两种画法。它们是**平级**的，不是「列表 + 一个附属页面」——
/// 所以 `q` 在两边都退出 dct、`Ctrl+Q` 在两边都无事可做（已经在顶层），
/// 而所有「回看板」的落点都得回到用户选的这一个（见 `mod.rs` 的 `home_view`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum ViewMode {
    List,
    Grid,
}

impl ViewMode {
    /// 存进 settings.json 的稳定短码，跟枚举顺序无关。
    pub fn code(self) -> &'static str {
        match self {
            ViewMode::List => "list",
            ViewMode::Grid => "grid",
        }
    }

    pub fn from_code(s: &str) -> Option<ViewMode> {
        match s {
            "list" => Some(ViewMode::List),
            "grid" => Some(ViewMode::Grid),
            _ => None,
        }
    }

    /// 切到另一个。只有两个模式，所以这是个全函数，不用兜底分支。
    pub fn toggled(self) -> ViewMode {
        match self {
            ViewMode::List => ViewMode::Grid,
            ViewMode::Grid => ViewMode::List,
        }
    }
}

/// 看板上到底显示哪些会话。跟着用户走而不是跟着视图走——列表和九宫格是
/// 同一批会话的两种画法，作用域在两边必须一致，否则切个视图会话数就变了。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Scope {
    CurrentProject,
    AllProjects,
}

/// 按作用域筛出真正上屏的会话。抽成纯函数是为了能单测（同 `filter_projects`）
/// ——过滤错了的代价是用户对着别的项目的会话按 `s` 停止，这必须在有守护
/// 进程之前就测得到。
pub(crate) fn visible_sessions(
    all: &[crate::session::SessionInfo],
    scope: Scope,
    project: &Path,
) -> Vec<crate::session::SessionInfo> {
    if scope == Scope::AllProjects {
        return all.to_vec();
    }
    all.iter()
        .filter(|s| same_project(Path::new(&s.dir), project))
        .cloned()
        .collect()
}

/// 两个路径是不是同一个项目。归一化只在这里定义一次——过滤和「进会话要不要
/// 切项目」必须用同一套判断，各写各的迟早会分叉成「列表里看不见、但进去了
/// 又说没换项目」这种自相矛盾的状态。
pub(crate) fn same_project(dir: &Path, project: &Path) -> bool {
    canon(dir) == canon(project)
}

/// 比较用的归一化。**只用于比较，不用于显示**——把 `/tmp` 显示成
/// `/private/tmp` 会让 macOS 上的界面凭空变丑，而用户并没有做错什么。
///
/// 解析失败（目录已被删）时退化成原样：一个指向已删目录的会话仍然应当
/// 待在它原本的项目下，而不是从「当前项目」和「全部项目」两个视图里
/// 同时消失——那才是真的找不回来了。
fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// 视图切换后，底部消息该不该清掉。抽成纯函数是因为 `run()` 的按键循环里有
/// 十几处给 `message` 赋值，没法在每处都补一行清空逻辑——漏一处就会有一个
/// 视图继续顶着上一屏的残留话术（比如看板「已开会话 3」被 `Enter` 带进会话
/// 视图后，盖住了「F2 回看板」的提示）。调用方在处理一次按键前后各拍一次
/// 「视图种类」和「消息内容」的快照，传进来比较：
///
/// - 视图没变：这条规则不归它管，原样保留消息——哪怕消息也在这次按键里
///   变了，那是当前视图自己的操作反馈（比如看板按 `d` 看改动）。
/// - 视图变了、消息也跟着变了：这条新消息就是这次切换本身的结果反馈
///   （比如手输路径 `Enter` 成功后的「已切到 X」），必须保留——清掉等于
///   用户按了 `Enter` 什么反馈都没有。
/// - 视图变了、消息没变：这条消息是切换前就挂在那儿的旧消息（比如
///   「已开会话 3」还没被看一眼就被 `Enter` 带进了新视图），新视图不该继续
///   顶着别的视图留下的话，清成空，好让「按视图给提示」的 `idle_help` 露出来。
pub(crate) fn message_after_transition(
    view_changed: bool,
    message_changed: bool,
    message: Msg,
) -> Msg {
    if view_changed && !message_changed {
        "".into()
    } else {
        message
    }
}

/// 贴在会话里、`Screen` 捎回状态之后：这个会话是不是已经结束了，该把用户
/// 送回看板？返回 `Some(提示语)` 就是「回看板并把这句话显示在底栏」。
///
/// 为什么要有这一步：agent 自己退出（`/exit`、shell 里的 `exit`）之后，
/// 界面留在会话视图里是一片空白——agent 在 alternate screen 里画，退出时恢复
/// 主屏，主屏从来没被写过。用户看到的是一张没有任何信息的空页，底栏还写着
/// 「其余按键都发给 agent」，而他敲的每个键都掉进一个死掉的 pty 里无声消失。
/// 所以「屏是空的」不能用来判断会话死活，只有状态能。
///
/// 只认 `Stopped`。别的状态（包括 `Unknown`——profile 没给 pattern，我们不知道
/// 它在干什么）都得留在会话里：把一个好端端的会话判成结束，会把用户从他正在
/// 用的 agent 里踢出去，比空白页糟得多。
///
/// 抽成纯函数是为了能单测（同 `escape_hint`、`idle_help`、`back_one_level`）。
pub(crate) fn session_ended_notice(id: u32, state: SessionState, lang: Lang) -> Option<String> {
    match state {
        SessionState::Stopped => Some(crate::i18n::msg::session_ended(lang, id)),
        _ => None,
    }
}

/// 底栏左段：逃生键提示。
///
/// 这是唯一一条「不管出什么事都必须还在」的信息——用户找不到它就只能去
/// 别的窗口 kill 进程，而 kill 会把终端留在 raw mode。文案必须跟
/// `back_one_level` 逐行对上：底栏说什么就得真能做到什么，
/// 手输路径态退的是一层（回列表），不能写成「回看板」。
pub(crate) fn escape_hint(view: &View, lang: Lang) -> String {
    use crate::i18n::{text, Key};
    match view {
        View::Board => format!("q {}", text(Key::Quit, lang)),
        View::PickProject(p) if p.typing_path.is_some() => text(Key::BackToList, lang).to_string(),
        // 跟 back_one_level 保持一致：从密钥设置页进来的填密钥，退出回设置页，
        // 不是选择器，也不是看板——三条路各回各的，文案不能含糊成一句话。
        View::EnterSecret {
            return_to_settings: true,
            ..
        } => text(Key::BackToSettings, lang).to_string(),
        // 从选择器进来的填密钥，退出回的是选择器，不是看板
        View::EnterSecret { .. } => text(Key::BackToList, lang).to_string(),
        // 九宫格跟列表是**平级**的两个模式，它自己就是家——所以逃生键
        // 跟列表上一样是「q 退出」，而不是「回列表」。写成回列表的话，
        // Ctrl+Q 就成了 `g` 的一个隐藏同义词，用户还会以为自己退出了什么。
        // 框开着时左段写 Esc：这时候 `q` 只是个字母，写「q 退出」是假的。
        View::Grid { reply: Some(_), .. } => format!("Esc {}", text(Key::Cancel, lang)),
        View::Grid { .. } => format!("q {}", text(Key::Quit, lang)),
        // 会话视图是唯一两个键都能逃的地方：Ctrl+Q 被主循环截下
        // （`mod.rs` 的 `is_ctrl_q`），F2 由 `attach.rs` 自己吃掉，
        // 落点都是看板。只写一个键等于藏起另一半——而 F2 恰恰是
        // 手指不必离开主键区的那个。其余视图没有 F2，不能照抄这句。
        View::Attached(_) => text(Key::BackToBoardWithF2, lang).to_string(),
        _ => text(Key::BackToBoard, lang).to_string(),
    }
}

/// 底部提示条：没有消息覆盖时，按当前视图告诉用户能按什么键。
///
/// 抽成纯函数是为了能单测（同 `escape_hint`、`back_one_level`）——不用把
/// `draw()` 整条渲染管线跑一遍，只为了断言一句文案里有没有「↑↓」。
pub(crate) fn idle_help(view: &View, scope: Scope, lang: Lang) -> String {
    use crate::i18n::{help_line, text, Key};
    // 作用域键的说明随范围反转——`a` 是个开关，只写一个方向的话，
    // 另一个方向上屏幕就在说谎。
    let scope_key = match scope {
        Scope::CurrentProject => Key::SeeAllProjects,
        Scope::AllProjects => Key::ThisProjectOnly,
    };
    match view {
        // 不再写「F2 同效」：左段的逃生键已经是「Ctrl+Q（F2） 回看板」，
        // 两个键都点了名，右段再说一遍是拿最稀缺的一行去重复已知信息。
        View::Attached(_) => help_line(
            &[
                ("F3", Key::NextSession),
                ("", Key::NewSessionFromBoard),
                ("", Key::OtherKeysGoToAgent),
            ],
            lang,
        )
        .replace("  ", "　"),
        View::PickProfile { .. } => help_line(
            &[
                ("↑↓", Key::Select),
                ("Enter", Key::Confirm),
                ("", Key::OrPressDigit),
                ("Esc", Key::Cancel),
            ],
            lang,
        ),
        View::PickProject(p) if p.typing_path.is_some() => help_line(
            &[("Enter", Key::Confirm), ("Esc", Key::BackToListWord)],
            lang,
        ),
        // 目录浏览器的三个键（Tab/→/←）必须写出来：它们是这一层唯一
        // 「学过才知道」的部分，不写就等于没做浏览器。
        View::PickProject(_) => help_line(
            &[
                ("Tab", Key::SwitchPane),
                ("↑↓", Key::Select),
                ("→", Key::EnterFolder),
                ("←", Key::GoUp),
                ("Enter", Key::Confirm),
                ("", Key::TypeToFilter),
                ("Esc", Key::Cancel),
            ],
            lang,
        ),
        // `g 九宫格` 插在切换类按键那一段里，不放句尾：`g` 是九宫格唯一的
        // 入口，排在最容易被挤到第二行末尾的位置等于藏起来。
        // `a` 紧跟着 `p 换项目`：两个键都在回答「我现在在看哪些会话」，
        // 挨着放用户才会把它们当成一组。
        View::Board => help_line(
            &[
                ("n", Key::New),
                ("N", Key::SwitchAgent),
                ("p", Key::SwitchProject),
                ("a", scope_key),
                ("c", Key::Secrets),
                // `l 设置` 紧跟 `c 密钥`：两个都是「配置」类入口。原来两份
                // 按键表里都没有它，而 `l` 一直是能按的——屏幕上没写却真管用
                // 的键，用户只能靠撞见。
                ("l", Key::SettingsTitle),
                ("g", Key::Grid),
                ("↑↓", Key::Select),
                ("Enter", Key::Open),
                ("u", Key::Undo),
                ("s", Key::Stop),
                ("d", Key::Diff),
            ],
            lang,
        ),
        // 格子只读，键盘不会送进 agent，所以这里可以放心列一张按键表——
        // 跟会话视图不同（那边除了 F2 全转发，列按键表等于教人按错）。
        //
        // 跟看板那一句列的是同一批键（它们在两个视图里做的是同一件事），
        // 只把不一样的两处换掉：选择靠方向键、Enter 是放大而不是进入。
        //
        // `q 退出` **不**写在这里：九宫格现在跟列表一样是顶层，左段的
        // escape_hint 已经常驻「q 退出」。原来要写是因为那时左段被
        // 「Ctrl+Q 回列表」占着，q 没有别的地方交代。重复一遍只会挤掉
        // 句尾的 s/d——那两个是不可撤销的操作，比重复一次 q 重要得多。
        // 回复框开着时键盘整个归框，这时候再列动作键就是在教人按错——
        // 屏幕上写着做不到的操作比不写更糟。
        //
        // `Esc 取消` **不**写在这里：它已经常驻左段（`escape_hint`）。同一行
        // 里写两遍是拿最稀缺的一行去重复已知信息，而 `Ctrl+C 打断` 才是这里
        // 唯一「不写就找不到」的键——它跟 `s 停止` 一样是打断 agent 的动作，
        // 藏起来只会让用户眼看着它跑偏却不知道怎么喊停。
        View::Grid { reply: Some(_), .. } => help_line(
            &[("Enter", Key::SendReply), ("Ctrl+C", Key::InterruptAgent)],
            lang,
        ),
        // `N 换 agent` 不写在这一份里：加了 `i 回一句` 之后这张表会折到
        // 第三行，底栏多吃一行，内容区在 80×24 下跌破 `grid.rs` 的 MIN_ROWS，
        // 整个九宫格换成一句「窗口太小」。`N` 是 `n` 的变体（按 n 就会看到
        // agent 选择器），而 `i` 是全新能力——挤掉谁很清楚。
        // 有 `the_bottom_bar_never_squeezes_the_grid_off_the_screen` 盯着。
        View::Grid { .. } => help_line(
            &[
                ("", Key::MoveArrows),
                ("Enter", Key::Zoom),
                ("i", Key::ReplyOnce),
                ("g", Key::List),
                ("n", Key::New),
                ("p", Key::SwitchProject),
                ("a", scope_key),
                ("c", Key::Secrets),
                // 九宫格这份**不**跟着加 `l 设置`。加了就要折到第三行，底栏
                // 多吃一行，内容区在 80×24 下只剩 19 行——正好跌破 grid.rs 的
                // `MIN_ROWS`，整个九宫格换成一句「窗口太小」。少写一个键，比在
                // 最常见的终端尺寸上把这个视图整个关掉划算。
                // 有 `the_bottom_bar_never_squeezes_the_grid_off_the_screen` 盯着。
                ("u", Key::Undo),
                ("s", Key::Stop),
                ("d", Key::Diff),
            ],
            lang,
        ),
        // 验证中不接受任何操作，底部提示不该继续说「Enter 确认」——那会让人
        // 以为再按一次有用，其实这时候按键全被吞掉，只有 Esc 生效。
        View::EnterSecret {
            phase: SecretPhase::Verifying,
            ..
        } => text(Key::Verifying, lang).to_string(),
        // 跟 escape_hint 一样要分 return_to_settings：从设置页进来的 Esc
        // 回设置页，不是「列表」——两处文案哪怕只有半句话不一致，都是
        // 「底栏说什么就得真能做到什么」这条原则被破坏了一半。
        View::EnterSecret {
            return_to_settings: true,
            ..
        } => help_line(
            &[
                ("", Key::PasteOrTypeKey),
                ("Enter", Key::Confirm),
                ("Esc", Key::BackToSettingsWord),
            ],
            lang,
        ),
        View::EnterSecret { .. } => help_line(
            &[
                ("", Key::PasteOrTypeKey),
                ("Enter", Key::Confirm),
                ("Esc", Key::BackToListWord),
            ],
            lang,
        ),
        View::Secrets { .. } => help_line(
            &[
                ("↑↓", Key::Select),
                ("Enter", Key::Edit),
                ("d", Key::Delete),
                ("Esc", Key::Back),
            ],
            lang,
        ),
        View::Settings { .. } => help_line(
            &[
                ("↑↓", Key::Select),
                ("Enter", Key::Confirm),
                ("Esc", Key::Cancel),
            ],
            lang,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::InstallPrompt;
    use crate::ui::key_to_input;

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_q_is_never_forwarded_to_the_agent() {
        // 调用点已经拦了 Ctrl+Q，这层是兜底：万一哪天调用点漏改，
        // 也不能把 0x11 悄悄发进 agent——那会变成一个「按了逃生键，
        // 结果字符落进了 Claude Code 输入框」的怪现象。
        assert_eq!(key_to_input(&ctrl('q')), None);
        assert_eq!(key_to_input(&ctrl('Q')), None);
    }

    #[test]
    fn ctrl_q_is_recognised_in_both_cases() {
        // 有的终端在 Ctrl 组合里送大写字母
        assert!(is_ctrl_q(&ctrl('q')));
        assert!(is_ctrl_q(&ctrl('Q')));
        // 不带 Ctrl 的裸 q 不算——否则在项目选择器里打字过滤会退出界面
        assert!(!is_ctrl_q(&KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn ctrl_q_backs_out_one_level_at_a_time() {
        // 会话 / 两个选择器 -> 看板
        assert!(matches!(
            back_one_level(View::Attached(1)),
            Some(View::Board)
        ));
        assert!(matches!(
            back_one_level(View::PickProfile {
                entries: Vec::new(),
                state: ListState::default(),
                warning: None,
            }),
            Some(View::Board)
        ));
        assert!(matches!(
            back_one_level(View::PickProject(ProjectPicker {
                filter: String::new(),
                typing_path: None,
                ..ProjectPicker::new(Vec::new(), std::path::PathBuf::from("/tmp"))
            })),
            Some(View::Board)
        ));
    }

    #[test]
    fn ctrl_q_leaves_the_typing_state_before_leaving_the_picker() {
        // 手输路径态退一层是回列表，不是一步退回看板
        let back = back_one_level(View::PickProject(ProjectPicker {
            filter: "a".into(),
            typing_path: Some("/tmp/b".into()),
            ..ProjectPicker::new(vec!["/tmp/a".into()], std::path::PathBuf::from("/tmp"))
        }));
        match back {
            Some(View::PickProject(p)) => {
                assert_eq!(p.typing_path, None, "应当退出手输态");
                assert_eq!(p.filter, "a", "退一层不该顺手清掉过滤词");
                assert_eq!(p.recent, vec!["/tmp/a".to_string()], "项目列表不该丢");
            }
            other => panic!("手输态应当退回列表态，实际是 {:?}", other.is_some()),
        }
    }

    #[test]
    fn grid_hints_match_what_the_keys_actually_do() {
        // 底栏说什么就得真能做到什么。九宫格现在是顶层，逃生键是 q——
        // 「两个模式都是家」这条由 both_board_modes_are_top_level 单独钉住。
        let help = idle_help(&View::grid(0), Scope::CurrentProject, Lang::Zh);
        for k in [
            "方向键选格子",
            "Enter 放大",
            "i 回一句",
            "n 新建",
            "p 换项目",
            "c 密钥",
            "u 回滚",
            "s 停止",
            "d 改动",
            // `g 列表` 是九宫格唯一交代「怎么切回去」的地方
            "g 列表",
        ] {
            assert!(help.contains(k), "九宫格的按键表少了「{k}」：{help}");
        }
        // `N 换 agent` 有意不写：见 idle_help 里那段注释。按 `n` 就会看到
        // agent 选择器，所以它不是「屏幕上没写就找不到」的能力。
        assert!(
            !help.contains("N 换"),
            "九宫格按键表放不下 N，写了就会把 d 改动挤出屏幕：{help}"
        );
        // `q 退出` 不该再出现在这一句里：它已经常驻左段（escape_hint），
        // 重复一遍只会把句尾的 s/d 挤出屏幕，而那两个不可撤销。
        assert!(
            !help.contains("q 退出"),
            "左段已经写着 q 退出，这里不该重复：{help}"
        );
        // 「这些键在 80 列终端上真的看得见吗」这个问题，原来是靠在这里
        // 手算一遍截断宽度来回答的（`truncate(help, 63)`）。那个算式随着
        // 底栏改成两行自动换行已经失效，而且它算的是文案、不是屏幕。
        // 真正的断言搬到了 `mod.rs`——那里是把整帧画出来再数格子：
        // `every_board_key_is_actually_on_screen_at_eighty_columns` 和
        // `every_grid_key_is_actually_on_screen_at_eighty_columns`。
    }

    #[test]
    fn board_help_mentions_the_grid() {
        // 不写出来就没人会去按 g——九宫格是第二视图，没有别的入口
        assert!(idle_help(&View::Board, Scope::CurrentProject, Lang::Zh).contains("g 九宫格"));
    }

    /// 九宫格不再是列表的下一层：两个模式都是顶层，`Ctrl+Q` 在两边都无事
    /// 可做。留着「Ctrl+Q 回列表」的话，它就是 `g` 的一个隐藏同义词，
    /// 而屏幕上写的是「回列表」——用户会以为自己退出了什么。
    #[test]
    fn both_board_modes_are_top_level() {
        assert!(back_one_level(View::Board).is_none());
        assert!(back_one_level(View::grid(0)).is_none());
        assert_eq!(escape_hint(&View::Board, Lang::Zh), "q 退出");
        assert_eq!(
            escape_hint(&View::grid(0), Lang::Zh),
            "q 退出",
            "九宫格也是家，不是列表的下一层"
        );
    }

    /// 目录浏览器的三个键必须写在屏幕上。它们是这一层唯一「学过才知道」
    /// 的部分——不写就等于做了个浏览器但没人知道怎么用。
    #[test]
    fn the_browser_advertises_its_three_keys() {
        let help = idle_help(
            &View::PickProject(ProjectPicker::new(
                Vec::new(),
                std::path::PathBuf::from("/tmp"),
            )),
            Scope::CurrentProject,
            Lang::Zh,
        );
        for k in ["Tab 切换左右", "→ 进入文件夹", "← 上一级"] {
            assert!(help.contains(k), "帮助行少了「{k}」：{help}");
        }
    }

    /// `g` 在两个模式里都真的管用，所以两边的帮助行都得写出来。
    /// 原来九宫格那句里没有 `g`——正是这个仓库反复警惕的
    /// 「屏幕上没写却真管用的键」，只是这次犯在自己身上。
    #[test]
    fn both_modes_advertise_the_key_that_switches_them() {
        let list = idle_help(&View::Board, Scope::CurrentProject, Lang::Zh);
        let grid = idle_help(&View::grid(0), Scope::CurrentProject, Lang::Zh);
        assert!(
            list.contains("g 九宫格"),
            "列表要告诉用户怎么去九宫格：{list}"
        );
        assert!(
            grid.contains("g 列表"),
            "九宫格要告诉用户怎么回列表：{grid}"
        );
    }

    #[test]
    fn ctrl_q_on_the_board_quits() {
        // 退到头了。看板上退出不杀会话，守护进程继续跑。
        assert!(back_one_level(View::Board).is_none());
    }

    #[test]
    fn expand_path_handles_tilde_and_relative() {
        let base = std::path::Path::new("/base");
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());

        assert_eq!(
            expand_path("/abs/x", base),
            std::path::PathBuf::from("/abs/x")
        );
        assert_eq!(expand_path("~/x", base), home.join("x"));
        assert_eq!(expand_path("~", base), home);
        assert_eq!(
            expand_path("rel/x", base),
            std::path::PathBuf::from("/base/rel/x")
        );
        // 用户粘贴路径常带尾随空格
        assert_eq!(
            expand_path("  /abs/x  ", base),
            std::path::PathBuf::from("/abs/x")
        );
        // `~foo` 不是家目录展开，是个叫 ~foo 的相对路径
        assert_eq!(
            expand_path("~foo", base),
            std::path::PathBuf::from("/base/~foo")
        );
    }

    #[test]
    fn expand_path_of_empty_string_is_base_itself() {
        // 空串不是绝对路径，走 base.join("")，结果就是 base 本身——而且
        // base 本身通常是存在的目录，is_dir() 照样为真。这不是 bug，是
        // Path::join 的正常语义，但意味着调用方（手输路径的 Enter 处理）
        // 必须自己在展开之前挡住空输入，不能指望 expand_path 或
        // is_dir() 帮忙识别"用户什么都没输"。
        let base = std::path::Path::new("/base");
        assert_eq!(expand_path("", base), base);
    }

    fn sess(id: u32, dir: &str) -> crate::session::SessionInfo {
        crate::session::SessionInfo {
            id,
            profile: "claude".into(),
            dir: dir.into(),
            state: SessionState::Idle,
            activity: String::new(),
        }
    }

    #[test]
    fn current_project_scope_keeps_only_that_project_in_order() {
        // 这条就是用户报的症状一：底栏写着 A，屏幕上却有 B 的会话。
        let all = [
            sess(1, "/w/a"),
            sess(2, "/w/b"),
            sess(3, "/w/a"),
            sess(4, "/w/c"),
        ];
        let got = visible_sessions(&all, Scope::CurrentProject, Path::new("/w/a"));
        assert_eq!(
            got.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![1, 3],
            "只留当前项目的会话，且保持原顺序"
        );
    }

    /// `a` 是个开关，两个方向做的事相反。只写一句「a 全部项目」的话，
    /// 用户已经在全部项目视图里时，屏幕会让他以为再按一次还是看全部。
    #[test]
    fn the_scope_key_hint_says_where_a_will_take_you() {
        let board = View::Board;
        assert!(
            idle_help(&board, Scope::CurrentProject, Lang::Zh).contains("a 看全部项目"),
            "只看本项目时，a 通向全部项目"
        );
        assert!(
            idle_help(&board, Scope::AllProjects, Lang::Zh).contains("a 只看本项目"),
            "全部项目时，a 通向本项目"
        );
        // 九宫格是看板的另一种画法，同一个键必须给同一套说明
        let grid = View::grid(0);
        assert!(idle_help(&grid, Scope::CurrentProject, Lang::Zh).contains("a 看全部项目"));
        assert!(idle_help(&grid, Scope::AllProjects, Lang::Zh).contains("a 只看本项目"));
    }

    #[test]
    fn all_projects_scope_returns_everything_untouched() {
        let all = [sess(1, "/w/a"), sess(2, "/w/b")];
        let got = visible_sessions(&all, Scope::AllProjects, Path::new("/w/a"));
        assert_eq!(
            got.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![1, 2],
            "全部项目视图不筛任何东西，当前项目是什么都不影响它"
        );
    }

    #[test]
    fn a_project_with_no_sessions_yields_an_empty_list() {
        // 空不是异常：新项目、或者刚把会话全停了，都会走到这里。
        // 返回空 Vec 而不是 panic，是九宫格空状态文案能上屏的前提。
        let all = [sess(1, "/w/a")];
        assert!(visible_sessions(&all, Scope::CurrentProject, Path::new("/w/b")).is_empty());
    }

    #[test]
    fn a_session_whose_dir_was_deleted_stays_under_its_own_project() {
        // canonicalize 对已删目录会失败。退化成字面比较，让这个会话仍然
        // 归在它原本的项目下——否则它会从两个视图里同时消失，用户再也
        // 没有任何入口去停掉它。
        let gone = "/definitely/does/not/exist/dct-scope-test";
        let all = [sess(1, gone)];
        let got = visible_sessions(&all, Scope::CurrentProject, Path::new(gone));
        assert_eq!(got.len(), 1, "目录没了，会话还在，仍要看得见");
    }

    #[test]
    fn a_symlinked_project_dir_still_matches_its_sessions() {
        // 会话是用真实路径建的，用户却可能从一个符号链接进同一个项目
        // （macOS 上 /tmp -> /private/tmp 就是现成的例子）。字面比较会让
        // 整个项目的会话凭空消失，而用户看不出任何原因。
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("proj");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let all = [sess(1, &real.display().to_string())];
        let got = visible_sessions(&all, Scope::CurrentProject, &link);
        assert_eq!(
            got.len(),
            1,
            "从符号链接进来的当前项目，必须仍然认得出它自己的会话"
        );
    }

    /// 浏览器只列目录，而且要把「本来就不该出现的东西」挡掉：`.` 开头的
    /// 隐藏目录、依赖目录、构建产物。目标用户是非程序员，`node_modules`
    /// 出现在选项目的列表里对他既没有意义也没有用处。
    #[test]
    fn browsing_lists_only_meaningful_directories() {
        let tmp = tempfile::tempdir().unwrap();
        for d in ["proj", "node_modules", "target", ".hidden", ".git", "dist"] {
            std::fs::create_dir(tmp.path().join(d)).unwrap();
        }
        std::fs::write(tmp.path().join("readme.md"), "x").unwrap();

        let rows = list_dirs(tmp.path());
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["proj"],
            "只该剩下真正的项目目录"
        );
    }

    /// 目录按名字排序。`read_dir` 的顺序由文件系统决定，不排的话同一个
    /// 目录每次打开都可能换序，用户没法靠位置记住东西在哪。
    #[test]
    fn browsing_sorts_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        for d in ["zeta", "alpha", "Mid"] {
            std::fs::create_dir(tmp.path().join(d)).unwrap();
        }
        let names: Vec<String> = list_dirs(tmp.path()).into_iter().map(|r| r.name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "顺序必须稳定，不能听文件系统的");
    }

    /// git 仓库要标出来——agent 会话要求项目是 git 仓库（`session.rs`），
    /// 在选之前就看得见，省掉一次注定失败的尝试。
    #[test]
    fn browsing_marks_git_repositories() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let plain = tmp.path().join("plain");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(&plain).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();

        let rows = list_dirs(tmp.path());
        let git_of = |n: &str| rows.iter().find(|r| r.name == n).unwrap().is_git;
        assert!(git_of("repo"), "有 .git 的要标出来");
        assert!(!git_of("plain"), "没有的不能标");
    }

    /// 读不了的目录返回空表，不 panic。用户可能浏览到任何地方，
    /// 一个没权限的目录不该让整个 dct 倒下。
    #[test]
    fn browsing_an_unreadable_directory_yields_nothing_instead_of_panicking() {
        assert!(list_dirs(std::path::Path::new("/definitely/not/here/dct")).is_empty());
    }

    #[test]
    fn filter_projects_is_case_insensitive_substring() {
        let all = vec![
            "/Users/lei/work/dc/dc-terminal".to_string(),
            "/Users/lei/work/dc/dc_workbench".to_string(),
            "/Users/lei/tmp/scratch".to_string(),
        ];

        assert_eq!(filter_projects(&all, "").len(), 3, "空过滤词返回全部");
        assert_eq!(filter_projects(&all, "WORK").len(), 2, "不区分大小写");
        assert_eq!(
            filter_projects(&all, "dc-term"),
            vec!["/Users/lei/work/dc/dc-terminal".to_string()],
            "匹配的是完整路径的任意位置"
        );
        assert_eq!(filter_projects(&all, "scratch").len(), 1);
        assert!(filter_projects(&all, "没有这个").is_empty());
    }

    /// 这个守卫**不是**防「转义序列漏进 stdin 被当成按键」的：crossterm 只把
    /// ESC 后紧跟的那一个字节标成 Alt，后面的字节都是光板 `Char` 事件，这个
    /// 守卫拦不住；防漏进按键靠的是 `theme.rs` 里的 DA1 哨兵。这里只测它该
    /// 管的语义：挡住 Alt/Meta，放行正常按键（含 Shift、Ctrl）——挡了正常
    /// 按键就是把用户的键吃掉。
    #[test]
    fn is_plain_key_rejects_alt_but_passes_normal_keypresses() {
        assert!(is_plain_key(&KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE
        )));
        // Shift 必须放过，否则大写 N 这个独立分支永远进不去
        assert!(is_plain_key(&KeyEvent::new(
            KeyCode::Char('N'),
            KeyModifiers::SHIFT
        )));
        // Ctrl 组合照旧放过：Ctrl+Q 在这层之前就被 is_ctrl_q 接走了，
        // 这里再挡一遍只会改掉和本次修复无关的行为。
        assert!(is_plain_key(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));

        assert!(!is_plain_key(&KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::ALT
        )));
        assert!(!is_plain_key(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::ALT
        )));
        // 有的终端把 Option/Command 报成 META
        assert!(!is_plain_key(&KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::META
        )));
    }

    /// agent 自己退出之后必须把用户送回看板：留在会话视图里就是一张空白页
    /// （agent 在 alternate screen 里画，退出时恢复的主屏从来没被写过），
    /// 底栏还写着「其余按键都发给 agent」，而键全掉进死掉的 pty。
    #[test]
    fn stopped_session_sends_the_user_back_to_the_board() {
        let notice =
            session_ended_notice(4, SessionState::Stopped, Lang::Zh).expect("Stopped 必须回看板");
        assert!(notice.contains('4'), "提示要说清是哪个会话结束了：{notice}");
    }

    /// 活着的会话一个都不能被判成结束——把用户从正在用的 agent 里踢出去，
    /// 比停在空白页糟得多。`Unknown` 尤其要留下：那只是 profile 没给 pattern，
    /// 我们不知道它在干什么，不是它死了。
    #[test]
    fn live_sessions_are_never_treated_as_ended() {
        for state in [
            SessionState::Working,
            SessionState::Asking,
            SessionState::Idle,
            SessionState::Unknown,
        ] {
            assert_eq!(
                session_ended_notice(1, state, Lang::Zh),
                None,
                "{state:?} 不该被当成已结束"
            );
        }
    }

    #[test]
    fn escape_hint_matches_what_the_key_actually_does() {
        // 底栏说什么就必须真能做到什么。手输路径态的 Ctrl+Q 是回列表
        // 不是回看板（见 back_one_level），文案不能写成「回看板」。
        assert_eq!(escape_hint(&View::Board, Lang::Zh), "q 退出");
        // 会话视图两个键都真的能回看板，所以两个都要写出来
        assert_eq!(
            escape_hint(&View::Attached(1), Lang::Zh),
            "Ctrl+Q（F2） 回看板"
        );
        assert_eq!(
            escape_hint(
                &View::PickProject(ProjectPicker {
                    filter: String::new(),
                    typing_path: None,
                    ..ProjectPicker::new(Vec::new(), std::path::PathBuf::from("/tmp"))
                }),
                Lang::Zh
            ),
            "Ctrl+Q 回看板"
        );
        assert_eq!(
            escape_hint(
                &View::PickProject(ProjectPicker {
                    filter: String::new(),
                    typing_path: Some(String::new()),
                    ..ProjectPicker::new(Vec::new(), std::path::PathBuf::from("/tmp"))
                }),
                Lang::Zh
            ),
            "Ctrl+Q 回列表"
        );
    }

    #[test]
    fn message_after_transition_keeps_message_when_view_unchanged() {
        // 视图没变：即便这次按键也顺手改了消息（比如看板按 d 看改动），
        // 这条规则不该插手，原样保留。
        let m = message_after_transition(false, true, "完成".into());
        assert_eq!(m.text, "完成");
    }

    #[test]
    fn message_after_transition_clears_stale_message_when_view_changes() {
        // 视图变了，但消息跟按键之前一模一样——说明是更早挂上的旧消息
        // （比如「已开会话 3」还没被看一眼就被 Enter 带进了会话），要清掉，
        // 好让新视图自己的 idle_help 露出来。
        let m = message_after_transition(true, false, "已开会话 3".into());
        assert_eq!(m.text, "", "视图变了、消息是旧的，就该清空");
    }

    #[test]
    fn message_after_transition_keeps_message_that_is_the_transition_result() {
        // 视图变了，消息也跟着变了——这条新消息就是这次切换本身的结果反馈
        // （比如手输路径 Enter 成功后的「已切到 X」），必须保留，
        // 不然用户按了 Enter 什么反馈都看不到。
        let m = message_after_transition(true, true, "已切到 ~/work/x".into());
        assert_eq!(m.text, "已切到 ~/work/x");
    }

    // ———— pick_action / digit_index：选择器四种状态各自路由到哪 ————

    fn entry(name: &str, status: ProfileStatus) -> ProfileEntry {
        ProfileEntry {
            name: name.into(),
            label: name.into(),
            note: String::new(),
            has_secret: status != ProfileStatus::NeedsSecret,
            status,
            secret: None,
            install: None,
        }
    }

    /// 给一个 entry 挂上密钥提示——`secret_rows` 只列 `secret.is_some()` 的
    /// 行，光靠 `status` 不够，得真的声明了密钥这件事才会出现在密钥页上。
    /// `has_secret` 不在这里动，沿用 `entry()` 按 `status` 给的默认值——
    /// 两个测试用例恰好落在 `has_secret` 跟 `status` 一致的那一半（见
    /// `secret_rows` 的注释里 `NeedsDependency`/`NotInstalled` 那个反例）。
    fn with_secret(mut e: ProfileEntry) -> ProfileEntry {
        e.secret = Some(SecretPrompt {
            hint: String::new(),
            url: None,
        });
        e
    }

    #[test]
    fn ready_entry_starts_a_session() {
        let e = entry("claude", ProfileStatus::Ready);
        assert!(matches!(pick_action(&e, Lang::Zh), PickAction::Start(n) if n == "claude"));
    }

    #[test]
    fn needs_secret_entry_opens_the_secret_view() {
        let e = entry("kimi", ProfileStatus::NeedsSecret);
        assert!(matches!(
            pick_action(&e, Lang::Zh),
            PickAction::AskSecret(_)
        ));
    }

    #[test]
    fn not_installed_with_an_installer_offers_to_install() {
        let mut e = entry(
            "codex",
            ProfileStatus::NotInstalled {
                command: "codex".into(),
            },
        );
        e.install = Some(InstallPrompt {
            command: vec![
                "npm".into(),
                "i".into(),
                "-g".into(),
                "@openai/codex".into(),
            ],
            note: String::new(),
        });
        match pick_action(&e, Lang::Zh) {
            PickAction::Install { profile, command } => {
                assert_eq!(profile, "codex");
                assert_eq!(command[0], "npm");
            }
            other => panic!("有安装命令就该给一条路，得到 {other:?}"),
        }
    }

    #[test]
    fn not_installed_without_an_installer_just_explains() {
        let e = entry(
            "weird",
            ProfileStatus::NotInstalled {
                command: "weird".into(),
            },
        );
        match pick_action(&e, Lang::Zh) {
            PickAction::Blocked(msg) => {
                assert!(msg.contains("weird"), "要说清是哪个命令找不到：{msg}");
                assert!(!msg.contains("PATH"), "别对非程序员说 PATH");
            }
            other => panic!("得到 {other:?}"),
        }
    }

    #[test]
    fn not_installed_with_empty_command_names_the_profile_not_a_blank_command() {
        // 手写 profile 可能整个没填 command（TOML 里 `command = []`），
        // status_of 兜底成 NotInstalled { command: "" }。这时候不能拼出
        // 「本机没有找到 」这种后面空着一截的死胡同文案。
        let e = entry(
            "weird",
            ProfileStatus::NotInstalled {
                command: String::new(),
            },
        );
        match pick_action(&e, Lang::Zh) {
            PickAction::Blocked(msg) => {
                assert!(msg.contains("weird"), "要点名是哪个 profile：{msg}");
                assert!(
                    !msg.trim_end().ends_with("找到"),
                    "不能留一截空白收尾：{msg}"
                );
            }
            other => panic!("得到 {other:?}"),
        }
    }

    #[test]
    fn missing_dependency_names_what_to_install_first() {
        let e = entry(
            "kimi",
            ProfileStatus::NeedsDependency {
                label: "Claude".into(),
            },
        );
        match pick_action(&e, Lang::Zh) {
            PickAction::Blocked(msg) => {
                assert!(msg.contains("Claude"), "要点名先装什么：{msg}");
            }
            other => panic!("得到 {other:?}"),
        }
    }

    // ———— Task 12：n 直连上次的 agent 、N 才进选择器 ————

    #[test]
    fn quick_start_uses_the_last_agent_when_it_is_ready() {
        let entries = vec![
            entry("claude", ProfileStatus::Ready),
            entry("kimi", ProfileStatus::Ready),
        ];
        assert_eq!(
            quick_start_target(Some("kimi"), &entries),
            Some("kimi".to_string())
        );
    }

    #[test]
    fn quick_start_falls_back_when_the_last_agent_is_no_longer_usable() {
        // 密钥被删了、CLI 被卸了。直接开会话只会得到一个起不来的窗口，
        // 退回选择器让用户重新挑。
        let entries = vec![
            entry("claude", ProfileStatus::Ready),
            entry("kimi", ProfileStatus::NeedsSecret),
        ];
        assert_eq!(quick_start_target(Some("kimi"), &entries), None);
    }

    #[test]
    fn quick_start_falls_back_when_the_last_agent_is_gone() {
        // 用户删掉了自己那个自定义 profile
        let entries = vec![entry("claude", ProfileStatus::Ready)];
        assert_eq!(quick_start_target(Some("mine"), &entries), None);
    }

    #[test]
    fn quick_start_falls_back_on_first_ever_run() {
        let entries = vec![entry("claude", ProfileStatus::Ready)];
        assert_eq!(quick_start_target(None, &entries), None);
    }

    #[test]
    fn board_help_mentions_both_n_and_capital_n() {
        let help = idle_help(&View::Board, Scope::CurrentProject, Lang::Zh);
        assert!(help.contains("n 新建"));
        assert!(help.contains("N 换 agent"));
    }

    // ———— Task 13：密钥设置页 ————

    #[test]
    fn board_help_mentions_the_settings_key() {
        assert!(idle_help(&View::Board, Scope::CurrentProject, Lang::Zh).contains("c 密钥"));
    }

    #[test]
    fn digit_keys_still_pick_the_first_nine() {
        // 数字保留是因为快；置灰项也占编号——编号跳号比编号漂移更难受
        assert_eq!(digit_index('1'), Some(0));
        assert_eq!(digit_index('9'), Some(8));
        assert_eq!(digit_index('0'), None);
        assert_eq!(digit_index('a'), None);
    }

    #[test]
    fn picker_help_mentions_both_ways_to_choose() {
        let help = idle_help(
            &View::PickProfile {
                entries: vec![],
                state: ListState::default(),
                warning: None,
            },
            Scope::CurrentProject,
            Lang::Zh,
        );
        assert!(help.contains("↑↓"));
        assert!(help.contains("数字"));
    }

    #[test]
    fn back_one_level_from_picker_goes_to_board() {
        assert!(matches!(
            back_one_level(View::PickProfile {
                entries: vec![],
                state: ListState::default(),
                warning: None,
            }),
            Some(View::Board)
        ));
    }

    #[test]
    fn secrets_view_escapes_to_the_board() {
        assert!(matches!(
            back_one_level(View::Secrets {
                entries: vec![],
                state: ListState::default(),
                pending_delete: None,
            }),
            Some(View::Board)
        ));
    }

    // ———— Task 11：填密钥界面 ————

    #[test]
    fn paste_is_trimmed() {
        assert_eq!(clean_secret("  sk-abc\n"), "sk-abc");
    }

    #[test]
    fn paste_strips_surrounding_quotes() {
        assert_eq!(clean_secret("\"sk-abc\""), "sk-abc");
        assert_eq!(clean_secret("'sk-abc'"), "sk-abc");
    }

    #[test]
    fn paste_strips_bearer_prefix() {
        // 从接口文档里整段拷贝经常带上它
        assert_eq!(clean_secret("Bearer sk-abc"), "sk-abc");
        assert_eq!(clean_secret("\"Bearer sk-abc\"\n"), "sk-abc");
    }

    #[test]
    fn paste_leaves_a_normal_key_alone() {
        assert_eq!(clean_secret("sk-abc123"), "sk-abc123");
    }

    #[test]
    fn bad_key_gets_a_human_message() {
        let m = verify_message(VerifyOutcome::BadKey, Lang::Zh).unwrap();
        assert!(m.contains("密钥"));
        assert!(!m.contains("401"), "别把状态码甩给用户：{m}");
    }

    #[test]
    fn unreachable_blames_the_network_not_the_key() {
        let m = verify_message(VerifyOutcome::Unreachable, Lang::Zh).unwrap();
        assert!(
            m.contains("网络"),
            "连不上要说是网络，不能让用户去怀疑密钥：{m}"
        );
    }

    #[test]
    fn ok_has_no_message() {
        assert!(verify_message(VerifyOutcome::Ok, Lang::Zh).is_none());
    }

    // ———— CRITICAL 1（最终整分支 code review）：验证结果不能套错屏幕 ————
    //
    // `verify_outcome_applies_to` 是这条修复的核心：验证异步跑完之后，
    // 只有发起时的 (profile, buf) 跟此刻屏幕上的 (profile, buf) 完全一样，
    // 这条结果才有落点。下面三条测试直接覆盖它的判断本身——真实的时间
    // 窗口（几秒钟的网络探测 + 用户手速）没法在单测里稳定踩中，但这条
    // 判断是一次纯粹的相等性比较，值得也应该被直接单测覆盖。

    #[test]
    fn verify_outcome_applies_when_profile_and_buffer_still_match() {
        assert!(verify_outcome_applies_to(
            "kimi", "sk-abc", "kimi", "sk-abc"
        ));
    }

    #[test]
    fn verify_outcome_does_not_apply_to_a_different_profile() {
        // CRITICAL 1 的复现：Kimi 的验证还在飞，用户已经绕回来在 GLM 身上
        // 重新填了密钥——这条结果绝不能套在 GLM 头上，哪怕两边这时候都是
        // `EnterSecret` 视图。
        assert!(!verify_outcome_applies_to(
            "kimi", "sk-abc", "glm", "sk-abc"
        ));
    }

    #[test]
    fn verify_outcome_does_not_apply_when_the_buffer_changed_on_the_same_profile() {
        // profile 没变，但填的密钥不是当初那一份——同样不能用这条结果去
        // 决定"这份新密钥"能不能存。
        assert!(!verify_outcome_applies_to(
            "kimi", "sk-old", "kimi", "sk-new"
        ));
    }

    #[test]
    fn secret_view_escapes_back_to_the_picker() {
        // 回选择器而不是回看板：用户可能只是选错了 agent
        let back = back_one_level(View::EnterSecret {
            profile: "kimi".into(),
            label: "Kimi".into(),
            prompt: SecretPrompt {
                hint: String::new(),
                url: None,
            },
            buf: String::new(),
            phase: SecretPhase::Typing,
            return_to_settings: false,
        });
        assert!(matches!(back, Some(View::PickProfile { .. })));
    }

    #[test]
    fn secret_view_escape_hint_says_back_to_the_list() {
        // 底栏说什么就得真能做到什么
        let h = escape_hint(
            &View::EnterSecret {
                profile: "kimi".into(),
                label: "Kimi".into(),
                prompt: SecretPrompt {
                    hint: String::new(),
                    url: None,
                },
                buf: String::new(),
                phase: SecretPhase::Typing,
                return_to_settings: false,
            },
            Lang::Zh,
        );
        assert!(h.contains("列表"), "底栏说什么就得真能做到什么：{h}");
    }

    #[test]
    fn secret_view_from_settings_escapes_back_to_settings_not_the_picker() {
        // return_to_settings 是「从哪儿进来的」——从密钥设置页进来的填密钥，
        // 退出必须回设置页，不能像从选择器进来的那样落回 PickProfile。
        let back = back_one_level(View::EnterSecret {
            profile: "kimi".into(),
            label: "Kimi".into(),
            prompt: SecretPrompt {
                hint: String::new(),
                url: None,
            },
            buf: String::new(),
            phase: SecretPhase::Typing,
            return_to_settings: true,
        });
        assert!(matches!(back, Some(View::Secrets { .. })));
    }

    #[test]
    fn secret_view_from_settings_escape_hint_says_back_to_settings() {
        let h = escape_hint(
            &View::EnterSecret {
                profile: "kimi".into(),
                label: "Kimi".into(),
                prompt: SecretPrompt {
                    hint: String::new(),
                    url: None,
                },
                buf: String::new(),
                phase: SecretPhase::Typing,
                return_to_settings: true,
            },
            Lang::Zh,
        );
        assert!(h.contains("设置"), "底栏说什么就得真能做到什么：{h}");
    }

    #[test]
    fn secret_view_from_settings_idle_help_also_says_back_to_settings() {
        // escape_hint 和 idle_help 都提了「Esc 回哪」，两处不能一处说设置、
        // 一处还说着旧的「列表」。
        let help = idle_help(
            &View::EnterSecret {
                profile: "kimi".into(),
                label: "Kimi".into(),
                prompt: SecretPrompt {
                    hint: String::new(),
                    url: None,
                },
                buf: String::new(),
                phase: SecretPhase::Typing,
                return_to_settings: true,
            },
            Scope::CurrentProject,
            Lang::Zh,
        );
        assert!(
            help.contains("返回设置"),
            "底栏说什么就得真能做到什么：{help}"
        );
    }

    #[test]
    fn secret_rows_only_lists_profiles_that_need_a_key() {
        let entries = vec![
            entry("claude", ProfileStatus::Ready), // 不需要密钥
            with_secret(entry("kimi", ProfileStatus::Ready)),
            with_secret(entry("glm", ProfileStatus::NeedsSecret)),
        ];
        let rows = secret_rows(&entries);
        assert_eq!(rows.len(), 2, "claude 不该出现在密钥页");
        assert_eq!(rows[0], ("kimi".to_string(), true), "Ready 说明密钥已配");
        assert_eq!(rows[1], ("glm".to_string(), false));
    }

    #[test]
    fn secrets_page_help_lists_its_own_keys() {
        let help = idle_help(
            &View::Secrets {
                entries: vec![],
                state: ListState::default(),
                pending_delete: None,
            },
            Scope::CurrentProject,
            Lang::Zh,
        );
        assert!(help.contains("Enter 改"));
        assert!(help.contains("d 删"));
    }

    // decide_delete_key 是「按 d 该做什么」的判断骨架（见其文档注释）：
    // 不碰网络，run() 里的 KeyCode::Char('d') 分支直接调用它来分类，这里
    // 覆盖它的四条分支。真正发 DeleteSecret 请求那半留在 run() 里，需要
    // daemon 连接，测不到——跟这个文件里所有别的 client.call 分支一样。

    #[test]
    fn decide_delete_key_arms_on_first_press() {
        // 第一次按 d：没有武装状态，选中的行已配——应该武装到这个名字，
        // 而不是直接判定为「确认删除」。
        let action = decide_delete_key(Some(("kimi".into(), true)), &None);
        assert_eq!(action, DeleteKeyAction::Arm("kimi".into()));
    }

    #[test]
    fn decide_delete_key_confirms_on_second_press_of_the_same_row() {
        // 第二次按 d，且武装的名字和当前选中行一致——这才是真正的删除信号。
        let action = decide_delete_key(Some(("kimi".into(), true)), &Some("kimi".to_string()));
        assert_eq!(action, DeleteKeyAction::Confirm("kimi".into()));
    }

    #[test]
    fn moving_the_cursor_must_disarm_pending_delete() {
        // 这是 finding 里最强调的一条：光标挪到另一行之后，即使武装状态
        // 字面上还留着旧名字（模拟「移动后没有及时清空」的疏漏），只要
        // 选中行换了，decide_delete_key 也必须判定成「重新武装」而不是
        // 「确认删除」——武装状态和选中行绝不能分叉。run() 里 Up/Down
        // 分支额外把 pending_delete 显式清成 None，这里验证的是就算那道
        // 防线失效，名字比对这道防线也不会把新行误判成确认。
        let action = decide_delete_key(
            Some(("glm".into(), true)), // 光标挪到了 glm 这一行
            &Some("kimi".to_string()),  // 但武装状态还留着 kimi
        );
        assert_eq!(
            action,
            DeleteKeyAction::Arm("glm".into()),
            "光标挪开之后，旧的武装状态不该跨行生效"
        );
    }

    #[test]
    fn decide_delete_key_on_unconfigured_row_just_notifies() {
        // 对着一个「未配」的行按 d：不武装、不删，只提示——照抄原有行为，
        // 这条不是本次 finding 新加的，但纳入同一组判断骨架的测试里。
        let action = decide_delete_key(Some(("glm".into(), false)), &None);
        assert_eq!(action, DeleteKeyAction::NotConfigured);
    }

    #[test]
    fn decide_delete_key_with_nothing_selected_is_a_no_op() {
        let action = decide_delete_key(None, &Some("kimi".to_string()));
        assert_eq!(action, DeleteKeyAction::NoSelection);
    }

    #[test]
    fn back_one_level_from_secrets_clears_any_armed_delete() {
        // Esc/Ctrl+Q 从密钥页退到看板，整个 View::Secrets 都被扔掉，
        // 武装状态自然作废——这里确认 back_one_level 走的是「退到看板」
        // 这条通用兜底，而不是哪天有人给 Secrets 加了专属分支却忘了清
        // pending_delete。
        let armed = View::Secrets {
            entries: vec![with_secret(entry("kimi", ProfileStatus::Ready))],
            state: {
                let mut s = ListState::default();
                s.select(Some(0));
                s
            },
            pending_delete: Some("kimi".to_string()),
        };
        assert!(matches!(back_one_level(armed), Some(View::Board)));
    }

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_k(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn typing_accumulates_into_the_draft() {
        assert_eq!(
            reply_key("", &k(KeyCode::Char('h'))),
            Reply::Typing("h".into())
        );
        assert_eq!(
            reply_key("继续", &k(KeyCode::Char('，'))),
            Reply::Typing("继续，".into())
        );
    }

    /// 退格按字符删。按字节删会把一个汉字切成半截 UTF-8，剩下的字节
    /// 一渲染就是乱码，而中文是这个界面的主语言。
    #[test]
    fn backspace_deletes_one_character_not_one_byte() {
        assert_eq!(
            reply_key("方案B", &k(KeyCode::Backspace)),
            Reply::Typing("方案".into())
        );
        assert_eq!(
            reply_key("方案", &k(KeyCode::Backspace)),
            Reply::Typing("方".into())
        );
        // 空框退格不该炸，也不该变成别的动作
        assert_eq!(
            reply_key("", &k(KeyCode::Backspace)),
            Reply::Typing(String::new())
        );
    }

    #[test]
    fn esc_throws_the_draft_away_without_sending() {
        assert_eq!(reply_key("别发这句", &k(KeyCode::Esc)), Reply::Cancel);
    }

    #[test]
    fn enter_sends_what_is_in_the_box() {
        assert_eq!(
            reply_key("继续，用方案 B", &k(KeyCode::Enter)),
            Reply::Send("继续，用方案 B".into())
        );
    }

    /// 空框直接回车 = 替用户按一下回车（批准/继续）。这是最高频的交互，
    /// 不该逼他先打点什么。守护进程侧空串本来就是这个意思（见
    /// `session.rs::send_input`），所以这里原样传下去就行。
    #[test]
    fn enter_on_an_empty_box_is_a_bare_enter() {
        assert_eq!(
            reply_key("", &k(KeyCode::Enter)),
            Reply::Send(String::new())
        );
    }

    #[test]
    fn ctrl_c_interrupts_the_agent_instead_of_closing_the_box() {
        assert_eq!(reply_key("半句话", &ctrl_k('c')), Reply::Interrupt);
    }

    /// **框开着的时候动作键必须失效。** 不这么做的话，用户打「so」的第一个
    /// 字母就把会话停了——而停止不可撤销。这是整个功能里最贵的一个错。
    #[test]
    fn action_keys_are_just_letters_while_the_box_is_open() {
        for c in ['s', 'u', 'd', 'q', 'n', 'p', 'a', 'c', 'l', 'g', 'i'] {
            assert_eq!(
                reply_key("", &k(KeyCode::Char(c))),
                Reply::Typing(c.to_string()),
                "框开着时 `{c}` 只能是个字母"
            );
        }
    }

    /// 组合键不当字符收：终端里它们是控制序列，收进框会变成一串看不懂的
    /// 字，而用户以为自己只是按了个快捷键。
    #[test]
    fn modifier_combos_do_not_land_in_the_draft() {
        assert_eq!(
            reply_key("在打字", &ctrl_k('a')),
            Reply::Typing("在打字".into())
        );
        assert_eq!(
            reply_key(
                "在打字",
                &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)
            ),
            Reply::Typing("在打字".into())
        );
    }

    /// 方向键在框里不动焦点——焦点动了，屏幕上「发给 4 claude」那行字就
    /// 跟着变，而用户正对着它打字。收件人在按 `i` 那一刻就钉死了。
    #[test]
    fn arrows_do_not_move_the_focus_while_the_box_is_open() {
        for code in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Tab,
        ] {
            assert_eq!(reply_key("草稿", &k(code)), Reply::Typing("草稿".into()));
        }
    }
}
