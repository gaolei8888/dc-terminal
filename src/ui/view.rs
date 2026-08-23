use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::i18n::{HelpItem, Lang};
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
    /// 下标会漂：会话被停掉、翻页、换项目，都会让同一个下标指向另一个
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
    /// 设置页：一张「设置项」列表（`SettingsItem`，见 `settings_view.rs`），
    /// 不再是纯语言列表——语言现在是列表里的一项，选中它才进语言子列表。
    /// 跟 `Secrets` 分开是两码事——那边管的是「哪个 agent 用哪把密钥」，
    /// 这里管的是界面本身怎么显示。
    Settings {
        /// 顶层「设置项」列表的光标。
        state: ListState,
        /// `None` = 停在顶层设置项列表；`Some` = 已经进了某个子列表，
        /// 里面存的是那个子列表自己的光标。**不新开一个 `View` 变体**
        /// 是因为子列表退出（`Esc`）之后要回到的是设置项列表，不是
        /// 看板——用同一个 `View::Settings` 装两层，`Esc` 才能一层一层退，
        /// 而不是一步退到底。
        ///
        /// 用枚举而不是「每个子列表一个 `Option<ListState>` 字段」：两个
        /// 字段能同时是 `Some`，那是个画不出来的状态，而编译器不会拦。
        sub: Option<SubList>,
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
    /// 「全部按键」浮层：底栏放不下的键都在这里。`?` 开，`Esc` 回。
    ///
    /// 底栏只有一行，装不下的键必须丢（见 `widgets::fit_help`）——但丢掉的
    /// 键仍然真的能按，而这个仓库反复警惕的正是「屏幕上没写却真管用的键」。
    /// 这个浮层就是那些键唯一的去处：底栏尾巴上那条 `? …` 是门，这里是门后。
    ///
    /// `from` 存的是**开门之前那一屏**，不是 `home_view()` 算出来的家：
    /// 从九宫格按 `?`，关掉之后必须回到刚才那个焦点格上，不能悄悄换成列表。
    Keys {
        from: Box<View>,
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
    /// 手机通知设置页：设置页选中「Phone」进。**只带 `status`**——「正在打字」
    /// 和「验证中」这两个临时态存在 `App`（`phone_buf`/`phone_verify_rx`）
    /// 而不是这里，理由跟 `verify_rx` 待在 `App` 上一模一样：`View` 要整体
    /// `Clone`，装着后台线程结果的 `Receiver` 进不去一个要 `Clone` 的枚举
    /// （见 `App::verify_rx` 的文档注释）。
    Phone {
        status: crate::proto::PhoneStatus,
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

// 这里以前有 `is_ctrl_q` 和 `back_one_level`：Ctrl+Q 曾是所有视图共用的
// 「退一层」全局键。它没了——每个视图退出的键都写在底栏上，而且每个视图
// 都已经有一个：会话视图是 F2（`attach.rs`），其余视图是 Esc（`pick.rs`、
// `settings_view.rs`、`secret.rs`、`keys.rs`），看板和九宫格是 `q`。
// 一个不写在屏幕上的第二条退路只会让 0x11 白白拿不回给 agent。

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
/// 用户完全可能已经按 Esc 退出这一屏，甚至绕回来在**另一个** agent
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
    // 问不出家目录时退到根：`~` 于是展开成一个一定存在、但一定不是项目的
    // 目录，调用方那句「这儿不是 git 仓库」照常说得出口。比展开成空路径强——
    // 那会变成相对路径，悄悄落在当前目录上。
    let home =
        || crate::sys::home().unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR_STR));

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

/// 贴在会话画面上的配色浮层的状态。见 `App::theme_pick`。
///
/// `prev` 是**打开浮层那一刻**的那一档，不是「上一次按方向键之前」的那一档：
/// 这一层的方向键当场就把 `App::bar` 换掉了（试穿的全部意义就是看真实画面），
/// 所以 `Esc` 要还回去的是最初那一件，不是上一件。
#[derive(Debug, Clone)]
pub struct ThemePick {
    pub state: ListState,
    pub prev: super::BarTheme,
}

/// 设置页里当前打开的子列表。见 `View::Settings::sub`。
#[derive(Debug, Clone)]
pub enum SubList {
    Language(ListState),
    /// 底栏/标题条的配色，见 `ui::BarTheme`。
    Theme(ListState),
}

/// 看板的两种画法。它们是**平级**的，不是「列表 + 一个附属页面」——
/// 所以 `q` 在两边都退出 dct（两边都是顶层，没有「上一层」可退），
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

/// 比较用的归一化。**只用于比较，不用于显示**——把 `/tmp` 显示成
/// `/private/tmp` 会让 macOS 上的界面凭空变丑，而用户并没有做错什么。
///
/// 解析失败（目录已被删）时退化成原样：一个指向已删目录的会话仍然应当
/// 待在它原本的项目组下，而不是从看板上凭空消失——那才是真的找不回来了。
pub(crate) fn canon(p: &Path) -> PathBuf {
    // 走 `sys::fs` 那一份而不是标准库：Windows 上标准库交出来的是
    // `\\?\C:\…`，而这个值不只是拿来比较——它会经 `app.current_dir()`
    // 传给 `create`，最后成为 git 和 pty 的工作目录，而那两个都不认这个
    // 前缀（见 `sys::fs::spawnable`）。
    crate::sys::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// 看板上的一个项目组。
///
/// `sessions` 是这一组要显示的会话——已停止的会话在列表里显示、在九宫格里
/// 不显示，这个差异由**调用方在传入前过滤**，分组函数本身不认识状态语义。
#[derive(Clone, Debug)]
pub(crate) struct ProjectGroup {
    /// 归一化后的绝对路径，也是分组键。**只用于比较，不用于显示**——
    /// 跟 `canon()` 的规矩一致。
    pub dir: PathBuf,
    /// 这个项目**未归一化**的那条路径——用户当初敲的拼写。`name`/`parent`
    /// 从它推出来，`p` 的目录浏览器也从它的上一级开起。
    ///
    /// 跟 `dir` 分开存而不是「要显示时再反推」：`parent` 已经 `short_path`
    /// 过（`~` 开头），拼不回一条真实路径；而 `dir` 是 canon 过的，
    /// macOS 上拿它去开浏览器，用户敲的 `/tmp/x` 会变成 `/private/tmp/x`。
    pub display_dir: PathBuf,
    /// 组头上的项目名（路径最后一段）。取自 `display_dir`，
    /// 理由见 `group_sessions` 里挑选 display 来源那段注释。
    pub name: String,
    /// 组头上那行灰字（父目录，已 `short_path`）。同上，来自原始路径。
    pub parent: String,
    pub sessions: Vec<crate::session::SessionInfo>,
    /// 这个项目上次用的 agent，底栏那条 `n 新建 <agent>` 要用。
    pub last_profile: Option<String>,
    /// 这个组被 pin 住了。三个来源：用户按 `p` 摆上来、开机时的启动目录补位、
    /// 以及**光标落到它上面**（`mod.rs::pin_cursor_group`——脚下那个组必须
    /// pin 住，否则它的最后一个会话自己停掉时整个组会在用户没按键的时候没了）。
    ///
    /// `x` 只能移除 pinned 且**没有在跑的会话**的组（`mod.rs::unpin_current`）。
    pub pinned: bool,
    pub collapsed: bool,
}

impl ProjectGroup {
    /// 组头上的 `claude×2 codex×1`。**现算不存**：存下来就有两份真相，
    /// 而它们只有一份是新的。按 agent 名排序，跟组的排序同一个理由——
    /// 顺序不能随会话生灭而跳动。
    pub fn agent_counts(&self) -> Vec<(String, usize)> {
        let mut m: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for s in &self.sessions {
            *m.entry(s.profile.clone()).or_insert(0) += 1;
        }
        m.into_iter().collect()
    }

    /// 这个项目在九宫格里的第一个格子（按 id 序），没有格子就是 `None`。
    ///
    /// **「有会话」不等于「有格子」**：九宫格不画已停止的会话（见
    /// `App::grid_sessions`），所以一个组可以行行都是会话、却一个格子都没有。
    /// `grid::focus_first_of_current_group` 靠它决定换项目时焦点落到哪一格，
    /// 以及「这个项目在九宫格里根本没有落点」时不要硬挪。
    ///
    /// **这个判断不能拿去回答「焦点是不是陈旧的」**——会话全停的项目同样
    /// 没有格子，但用户在那儿挪焦点是完全正当的动作。那个问题由
    /// `sync_board_cursor_from_grid` 自己的守卫回答，理由写在它的文档里。
    pub fn first_live(&self) -> Option<u32> {
        self.sessions
            .iter()
            .find(|s| s.state != SessionState::Stopped)
            .map(|s| s.id)
    }

    /// 这个项目里还有没有**在跑**的东西。
    ///
    /// 「还有会话吗」这个问题在看板上有两种答案，而要紧的是这一种：已停止的
    /// 会话没有进程，它留在列表里只为了 `u` 回滚和 `d` 看改动。所以：
    ///
    /// - `x` 拿不拿得掉这个组（`mod.rs::unpin_current` 的拒绝判据）
    /// - 底栏和 `?` 浮层写不写 `x 移除`（`help_ctx_for` 的 `can_remove`）
    /// - 这个组凭什么留在看板上（`group_sessions` 的成员规则）
    ///
    /// 三处问的是同一件事，所以只有这一个判据。三处各写一个
    /// `sessions.is_empty()` 的话，它们会分岔——这条分支已经因为完全一样的
    /// 形状产出过四条评审意见了。
    pub fn has_live_session(&self) -> bool {
        self.first_live().is_some()
    }

    /// 这个项目里有几个会话出错了。组头上要用红字点出来——
    /// 会话静默失败是 dct 最贵的失败模式。
    pub fn failed(&self) -> usize {
        self.sessions
            .iter()
            .filter(|s| s.state == SessionState::Failed)
            .count()
    }
}

/// 看板上出现哪些项目：**有在跑的会话的 ∪ pinned 的**。没有第三种。
///
/// 「在跑的」这个限定是 `x` 能真的拿掉东西的前提（见下面 `retain` 那一段），
/// 而它的代价——一个组可能在没人按键的时候没了——由「光标所在的组恒为
/// pinned」兜住（`mod.rs::pin_cursor_group`）：会消失的只有用户从来没去过
/// 的组。
///
/// 排序是 `BTreeMap<PathBuf, _>` 自带的、`PathBuf` 的 component-wise
/// `Ord`——不是裸字符串排序，两者在真实目录名上会分道扬镳：
/// `PathBuf::from("/w/a-b").cmp(&PathBuf::from("/w/a/c"))` 是
/// `Greater`（按分量比，`"a-b"` 整段 > `"a"`），同一对路径当 `&str`
/// 比较却是 `Less`（`'-'` 0x2D < `'/'` 0x2F）。这里用的是前者。
/// 不管是哪一种，它都是稳定的、与会话生灭无关——任何按活跃度或最后
/// 使用时间的排序，都会让行在用户没按键的时候移动，而「项目在我没
/// 按键的时候变了」正是这一版要消灭的东西。组内会话按 `id` 升序，
/// 同一个理由。
pub(crate) fn group_sessions(
    sessions: &[crate::session::SessionInfo],
    pinned: &[String],
    profiles: &BTreeMap<String, String>,
) -> Vec<ProjectGroup> {
    // 分组键统一走 canon：`/tmp` 和 `/private/tmp` 下的两个会话是同一个项目。
    // 这个 canon 后的 PathBuf 只当分组/比较的 key 用，绝不进 name/parent——
    // 见 canon() 自己的文档：归一化会把 `/tmp` 显示成 `/private/tmp`，
    // 界面凭空变丑，而用户什么都没做错。
    let mut buckets: BTreeMap<PathBuf, Vec<crate::session::SessionInfo>> = BTreeMap::new();
    for s in sessions {
        buckets
            .entry(canon(Path::new(&s.dir)))
            .or_default()
            .push(s.clone());
    }
    // pinned 项目的原始拼写（用户当初 `p` 摆上来时敲的那个字符串），
    // 按 canon 后的 key 存一份，专门留给 name/parent 用。
    let mut pinned_display: BTreeMap<PathBuf, String> = BTreeMap::new();
    let pinned_keys: Vec<PathBuf> = pinned
        .iter()
        .map(|p| {
            let key = canon(Path::new(p));
            // 同一个项目被 pin 了两种拼法（比如一次走符号链接一次没走）
            // 时，谁先出现在 `pinned` 里就用谁——固定优先级好过看起来随机。
            pinned_display
                .entry(key.clone())
                .or_insert_with(|| p.clone());
            key
        })
        .collect();
    for p in &pinned_keys {
        buckets.entry(p.clone()).or_default();
    }
    // **已停止的会话不足以让一个组留在看板上。** 它没有进程，留着只为了
    // `u`/`d`；一个只剩已停止会话、又没被 `p` 摆上来的项目，是用户从没
    // 要求过的一行——看板会这样一直攒下去，攒到再也找不到自己手头那几个。
    //
    // 这也是 `x` 能真的拿掉东西的前提：`x` 只做 unpin，而组的另一半来源是
    // 「有会话」。少了这一条，`x` 一个只剩已停止会话的项目会取消 pin、
    // 然后那一行原样留在屏幕上——按下去什么都没发生，正是这一版要消灭的
    // 「屏幕和状态各说各话」。
    //
    // 代价（用户已经权衡过并接受）：那些已停止会话的 `u`/`d` 从看板上没了
    // 入口，除非用 `p` 把这个项目重新摆回来——会话本身还在守护进程里，
    // 摆回来就一起回来，什么都没被删。
    buckets.retain(|dir, sessions| {
        pinned_keys.contains(dir)
            || sessions
                .iter()
                .any(|s| s.state != crate::session::SessionState::Stopped)
    });

    buckets
        .into_iter()
        .map(|(dir, mut sessions)| {
            sessions.sort_by_key(|s| s.id);
            // name/parent 的显示来源必须是未归一化的原始路径字符串，且
            // 挑选规则要固定，不能随会话生灭改变已显示的拼写：
            // pinned 就用 pinned 自己的拼写；没 pinned 就用组内 id 最小
            // 那个会话的 `dir`（上面已按 id 排过序，`sessions[0]` 就是它）。
            // 组存在就必属于「有会话」∪「pinned」之一，二者必居其一，
            // 所以这里总能找到一个原始字符串；空串分支只是防御性兜底，
            // 结构上不会被真正走到。
            let display_path: &str = pinned_display
                .get(&dir)
                .map(String::as_str)
                .unwrap_or_else(|| sessions.first().map(|s| s.dir.as_str()).unwrap_or(""));
            let name = Path::new(display_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                // 根目录没有 file_name。显示整条路径总比显示空白强。
                .unwrap_or_else(|| display_path.to_string());
            let parent = Path::new(display_path)
                .parent()
                .map(|p| super::widgets::short_path(&p.display().to_string()))
                .unwrap_or_default();
            let last_profile = profiles.get(&dir.display().to_string()).cloned();
            let pinned = pinned_keys.contains(&dir);
            ProjectGroup {
                dir,
                display_dir: PathBuf::from(display_path),
                name,
                parent,
                sessions,
                last_profile,
                pinned,
                collapsed: false,
            }
        })
        .collect()
}

/// 看板上的一行。分组之后光标不能再是「第几个会话」——它得能停在组头上，
/// 空组只有组头这一行。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Row {
    Header(usize),
    /// (组下标, 组内会话下标)
    Session(usize, usize),
}

/// 把分组展平成屏幕上的行。折叠的组只贡献组头那一行。
pub(crate) fn flatten(groups: &[ProjectGroup]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        rows.push(Row::Header(gi));
        if !g.collapsed {
            for si in 0..g.sessions.len() {
                rows.push(Row::Session(gi, si));
            }
        }
    }
    rows
}

/// 某一行属于哪个组。**「当前项目」就是这个函数的答案**——不再有一个
/// 可以跟屏幕不一致的 `current_dir` 字段。
pub(crate) fn group_of(rows: &[Row], i: usize) -> Option<usize> {
    match rows.get(i)? {
        Row::Header(g) => Some(*g),
        Row::Session(g, _) => Some(*g),
    }
}

/// 光标指着的那个东西的**语义身份**。重新分组之后靠它找回原位。
///
/// 存身份而不是存下标：下标在会话生灭时会指向别的东西，而那正好就是
/// 「项目在我没按键的时候变了」这个缺陷本身。
///
/// `Session` 除了 id 还带着建锚点那一刻它所在组的 `dir`：会话一旦真的没了
/// （结束并被 prune），新的 `groups` 里再也不会出现这个 id，光凭 id
/// 找不回「原属组」是谁——不存这份记忆，「退回它原来那个组的组头」这条
/// 兜底规则根本没法实现，只能瞎猜或者放弃。（这是本任务实现阶段发现的一处
/// 与参考实现的偏差，详见任务报告。）
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Anchor {
    Session(u32, PathBuf),
    Header(PathBuf),
}

pub(crate) fn anchor_of(groups: &[ProjectGroup], rows: &[Row], i: usize) -> Option<Anchor> {
    match rows.get(i)? {
        Row::Header(g) => Some(Anchor::Header(groups.get(*g)?.dir.clone())),
        Row::Session(g, s) => {
            let grp = groups.get(*g)?;
            Some(Anchor::Session(grp.sessions.get(*s)?.id, grp.dir.clone()))
        }
    }
}

/// 找回锚点。顺序：同 id 的会话行 → 该会话原属组的组头 → 同 dir 的组头。
/// 全找不到返回 `None`，调用方落到第 0 行。
pub(crate) fn find_anchor(groups: &[ProjectGroup], rows: &[Row], a: &Anchor) -> Option<usize> {
    match a {
        Anchor::Session(id, dir) => {
            // 会话还在：站回它身上
            if let Some(i) = rows.iter().position(|r| match r {
                Row::Session(g, s) => groups[*g].sessions[*s].id == *id,
                Row::Header(_) => false,
            }) {
                return Some(i);
            }
            // 会话没了：退回它原来那个组的组头。**不能就近落在下一行**——
            // 下一行可能已经是别的项目，那就等于项目在用户没按键时变了。
            // 这里靠的是锚点自带的 `dir`，不是在新 `groups` 里重新搜这个
            // id——会话真没了之后，新的 `groups` 里不会再有任何一个会话
            // 带着这个 id，搜也搜不到。
            let gi = groups.iter().position(|g| &g.dir == dir);
            match gi {
                Some(gi) => rows.iter().position(|r| *r == Row::Header(gi)),
                None => None,
            }
        }
        Anchor::Header(dir) => {
            let gi = groups.iter().position(|g| &g.dir == dir)?;
            rows.iter().position(|r| *r == Row::Header(gi))
        }
    }
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
/// 别的窗口 kill 进程，而 kill 会把终端留在 raw mode。文案必须跟各视图
/// 自己的按键处理逐行对上：底栏说什么就得真能做到什么，
/// 手输路径态退的是一层（回列表），不能写成「回看板」。
///
/// Ctrl+Q 没了之后这条提示是**唯一**的退路说明，不再有一个「猜得到的」
/// 全局键兜底。改这里的文案等于改用户唯一知道的逃生方式，慎重。
pub(crate) fn escape_hint(view: &View, lang: Lang) -> String {
    use crate::i18n::{text, Key};
    match view {
        View::Board => format!("q {}", text(Key::Quit, lang)),
        View::PickProject(p) if p.typing_path.is_some() => text(Key::BackToList, lang).to_string(),
        // 跟 `secret.rs` 的 Esc 分支保持一致：从密钥设置页进来的填密钥，退出
        // 回设置页，不是选择器，也不是看板——三条路各回各的，文案不能含糊成
        // 一句话。
        View::EnterSecret {
            return_to_settings: true,
            ..
        } => text(Key::BackToSettings, lang).to_string(),
        // 从选择器进来的填密钥，退出回的是选择器，不是看板
        View::EnterSecret { .. } => text(Key::BackToList, lang).to_string(),
        // 九宫格跟列表是**平级**的两个模式，它自己就是家——所以逃生键
        // 跟列表上一样是「q 退出」，而不是「回列表」：写成回列表就是在暗示
        // 有一条退回列表的路，而回列表的键是 `g`（换模式），不是逃生。
        // 框开着时左段写 Esc：这时候 `q` 只是个字母，写「q 退出」是假的。
        View::Grid { reply: Some(_), .. } => format!("Esc {}", text(Key::Cancel, lang)),
        View::Grid { .. } => format!("q {}", text(Key::Quit, lang)),
        // 全部按键浮层：`q` 在这里只是一张表上的一个字母，写「q 退出」是假的。
        View::Keys { .. } => format!("Esc {}", text(Key::Back, lang)),
        // 会话视图是唯一一个不写 Esc 的地方：Esc 必须原样发给 agent
        // （Claude Code 靠它取消/清空/关弹窗），所以逃生键是 F2，由
        // `attach.rs` 自己吃掉。其余视图没有 F2，不能照抄这句。
        View::Attached(_) => text(Key::BackToBoardF2, lang).to_string(),
        // 从设置页进来，退出回设置页——同 `EnterSecret` 的 `return_to_settings`
        // 分支一个道理，这一页没有别的来路。
        View::Phone { .. } => text(Key::BackToSettings, lang).to_string(),
        _ => text(Key::BackToBoard, lang).to_string(),
    }
}

/// 底栏画按键表时需要知道的「现在能干什么」。
///
/// 不传整个 `App` 是为了能单测：这一屏的规则全是「有没有选中 / 选中的是什么」
/// 的组合，拿两个字段就能穷举，拽进一个连着 socket 的 `App` 只会让测试
/// 写不出来。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HelpCtx {
    /// 光标（列表）或焦点（九宫格）现在落在哪个会话上。停在组头上就是
    /// `None`——那时候 `Enter` 按下去没有对象，写出来就是屏幕上写着一个
    /// 按不动的键。
    ///
    /// 带着会话本身而不只是一个 bool：`s`/`u`/`d` 能不能按要看它的状态和
    /// 是不是 agent 会话。底栏现在不写这三个键了（上限三条），但 `?` 浮层
    /// 写——而浮层同样不许宣传一个按不动的键。
    pub selected: Option<SelectedSession>,
    /// 光标所在的组是不是「pinned 且没有会话」——只有这种组能按 `x` 拿掉。
    pub can_remove: bool,
    /// 看板上有没有**第二个**项目可跳。
    ///
    /// 只有一个组时 `Tab` 什么都不做：`jump_project` 算的是
    /// `(cur + 1).rem_euclid(1)`，也就是 0，光标原地停在同一个组头上。
    /// 而「只有一个项目」正是第一次用 dct 时的默认状态——那一屏上写着
    /// `Tab 换项目`，按下去毫无反应，用户学到的第一件事就是底栏会骗人。
    pub can_switch_project: bool,
    /// 手机页的令牌输入框是不是开着（`App::phone_buf.is_some()`）——
    /// **最终整分支 review 的修复 6。** 这时候整页的物理键含义都变了：
    /// `Enter` 提交这一行输入，`x` 只是往输入框里敲一个字母 `x`，`Esc`
    /// 才是取消。`View::Phone` 本身不带这个临时态（见它的文档注释：
    /// `phone_buf` 因为要跟 `Receiver` 共存而只能待在 `App` 上），所以
    /// 这里单独收一个 `bool` 进来，而不是把整个 `App` 拽进签名。
    pub phone_editing: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SelectedSession {
    /// 是 agent 会话还是普通命令行。`u 回滚` / `d 改动` 只对前者有效。
    pub is_agent: bool,
    pub state: SessionState,
}

impl HelpCtx {
    /// 能不能停。已经停了的会话再写一个 `s 停止`，按下去只会得到一句错误。
    pub(crate) fn can_stop(&self) -> bool {
        matches!(self.selected, Some(s) if s.state != SessionState::Stopped)
    }

    /// 能不能回滚 / 看改动。命令行会话没有检查点，守护进程侧
    /// `checkpoint_base` 会直接返回 `NotAnAgentSession`——对着一个 shell
    /// 会话写 `u 回滚`，就是在说谎。
    pub(crate) fn can_checkpoint(&self) -> bool {
        matches!(self.selected, Some(s) if s.is_agent)
    }
}

/// 看板和九宫格共用的那张按键表。**硬上限三条动作 + 一个 `?`。**
///
/// 上限不是「放得下就多塞」：一行的内容随终端宽度变化，本身就是不可预期的
/// ——用户在窄终端上学会的键，到宽终端上位置全变了，同一个键在 120 列上
/// 看得见、在 80 列上无声消失。剩下的键全在 `?` 后面，那扇门永远在。
///
/// 三条选谁，按「此刻最可能做的」：光标停在会话行上，最可能是进去看看；
/// 停在组头上，最可能是在这里开一个新会话。
///
/// `rest` 是这个视图自己的候选键，按重要性排好序。**只许列这个视图真的
/// 绑了的键**——两个视图共用这个函数是为了共用**上限和挑选顺序**，不是为了
/// 共用一张假装两边一样的键表。九宫格曾经没绑 `Tab`/`x`，那时候把看板那两条
/// 照搬过去就是屏幕上写着按不动的键，而这个仓库把那当 bug 而不是小瑕疵；
/// 现在它绑了（见 `grid::handle_key`），所以这两条也就跟着回到了它的候选表里
/// ——键和这张表必须在同一次改动里一起走，先后脚都会留下一段说谎的底栏。
fn board_keys(
    ctx: HelpCtx,
    enter: (&'static str, crate::i18n::Key),
    rest: &[(&'static str, crate::i18n::Key)],
) -> Vec<(&'static str, crate::i18n::Key)> {
    use crate::i18n::Key;
    let mut keys: Vec<(&'static str, Key)> = Vec::new();
    if ctx.selected.is_some() {
        keys.push(enter);
    }
    keys.extend_from_slice(rest);
    keys.truncate(3);
    keys.push(("?", Key::MoreKeys));
    keys
}

/// 底部提示条：没有消息覆盖时，按当前视图告诉用户能按什么键。
///
/// **顺序就是优先级。** 底栏只有一行，放不下的从尾部开始丢（见
/// `widgets::fit_help`），所以越靠前的键越是「不写就找不到、找不到就干不了活」
/// 的那种。原来这张表是按「话题分组」排的（切换类挨着、配置类挨着），那在
/// 折两行的年代成立；现在窄终端上排在后面的直接不上屏，排序就变成了一个
/// 关于「丢哪个」的决定。
///
/// 看板和九宫格这两支还额外压着一条**硬上限：三条动作 + 一个 `?`**，见
/// `board_keys`。上限之外的键不是被丢了，是被搬到了 `?` 浮层里——底栏
/// 一行挤十来个键的结果是它们中的一半会随终端宽度忽隐忽现。
///
/// 尾巴上的 `? …` 是 `View::Keys` 浮层的门，永远不被截断——被丢掉的键全在
/// 门后面，丢了门它们就真的没有入口了。
///
/// **能不能按也决定写不写**（`ctx`）：一个会话都没有的看板上写 `s 停止`
/// `u 回滚` `d 改动`，一个 shell 会话上写 `u 回滚` `d 改动`，按下去只会得到
/// 一句错误。屏幕上写着做不到的操作比不写更糟——而底栏只剩一行之后，一个
/// 假键占掉的正是一个真键的位置。
///
/// 抽成纯函数是为了能单测（同 `escape_hint`、`back_one_level`）——不用把
/// `draw()` 整条渲染管线跑一遍，只为了断言一句文案里有没有「↑↓」。
pub(crate) fn idle_help(view: &View, lang: Lang, ctx: HelpCtx) -> Vec<HelpItem> {
    use crate::i18n::{help_items, Key};
    match view {
        // 不再写「F2 同效」：左段的逃生键本身就是「F2 回看板」，
        // 两个键都点了名，右段再说一遍是拿最稀缺的一行去重复已知信息。
        //
        // `F3`、`F4`、`F5`：中段让出去 18 列之后，80 列终端上右段只有 39 列，
        // 早年那三条（含两句整话）加起来六十多列，多出来的部分不是"折一行"
        // 而是被右端**静默截掉**——写成整话只会让第一条也读不完整。留在这里
        // 的三条都是**键名格**，短，而且放不下时由 `fit_help` 整格丢掉、不会
        // 截半句；它们也都是「不写就找不到」的键，其余的是说明，不是键，
        // 去 `?` 后面。
        //
        // 三条的**顺序是按重要性排的，不是按 F 键的号码**——`widgets::fit_help`
        // 永远保留列表里的最后一项，前面的按顺序能塞几条塞几条，塞不下的丢掉。
        // 于是「先丢谁」这件事只能靠排序表达，而它跟 F3/F4/F5 的数字顺序对不上：
        //
        // - `F4` 在最后（永远不丢）：这一层里唯一「不按就没法拖选复制」的键。
        // - `F5` 在最前（第二个丢）：唯一能把剪贴板里的图交给 agent 的键，
        //   而且这一层按 `?` 是打不开浮层的（附加视图里的键一律转发给 agent，
        //   见 `attach::handle_key` 头上那条注释）——底栏不写，它就是一个
        //   屏幕上完全不存在的键。
        // - `F3` 夹在中间：三条里唯一的**快捷方式**，退回看板再进另一个
        //   会话是等价的两步，丢了它只是慢，不是做不到。
        // - `F6` 排在 `F4` 前面，也就是**最先丢**的那一条：其余三条丢了就
        //   真的做不到（这一层按 `?` 打不开浮层），配色丢了还有设置页那条路
        //   （F2 回看板、`l`、配色），而且它是四条里唯一一条「不干活也行」的。
        View::Attached(_) => help_items(
            &[
                ("F5", Key::PasteImage),
                ("F3", Key::NextSession),
                ("F6", Key::BarTheme),
                ("F4", Key::EnterCopyMode),
            ],
            lang,
        ),
        // 浮层自己就是一整屏按键表，右段再列一遍是重复；左段的
        // 「Esc 返回」已经把这里唯一能按的键交代完了。
        View::Keys { .. } => Vec::new(),
        View::PickProfile { .. } => help_items(
            &[
                ("↑↓", Key::Select),
                ("Enter", Key::Confirm),
                ("", Key::OrPressDigit),
                ("Esc", Key::Cancel),
            ],
            lang,
        ),
        View::PickProject(p) if p.typing_path.is_some() => help_items(
            &[("Enter", Key::Confirm), ("Esc", Key::BackToListWord)],
            lang,
        ),
        // 目录浏览器的三个键（Tab/→/←）必须写出来：它们是这一层唯一
        // 「学过才知道」的部分，不写就等于没做浏览器。
        View::PickProject(_) => help_items(
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
        // 看板：进会话 / 新建 / 换项目，最多三条（见 `board_keys`）。
        //
        // `x 移除` 只在「pinned 且空」的组上写——它也只有在那种组上才真的
        // 管用（见 `mod.rs::unpin_current`）。它排在 `Tab` 前面是因为那时
        // 光标就停在那个空组上，`x` 说的正是眼前这一行的事。
        //
        // `s/u/d`、`g`、`N`、`c`、`l`、`p` 全部挪到 `?` 后面。挪走不是藏
        // 起来：门永远在尾巴上，而一行里挤十来个键的结果是它们中的一半会
        // 随终端宽度忽隐忽现。
        View::Board => {
            let mut rest: Vec<(&'static str, Key)> = vec![("n", Key::New)];
            if ctx.can_remove {
                rest.push(("x", Key::RemoveProject));
            }
            // 只有一个项目时 `Tab` 原地打转（见 `HelpCtx::can_switch_project`），
            // 不写。第一次用 dct 的那一屏正好就是这种状态。
            if ctx.can_switch_project {
                rest.push(("Tab", Key::SwitchProject));
            }
            help_items(&board_keys(ctx, ("Enter", Key::Open), &rest), lang)
        }
        // 格子只读，键盘不会送进 agent，所以这里可以放心列一张按键表——
        // 跟会话视图不同（那边除了 F2 全转发，列按键表等于教人按错）。
        //
        // 跟看板那一句列的是同一批键（它们在两个视图里做的是同一件事），
        // 只把不一样的两处换掉：选择靠方向键、Enter 是放大而不是进入。
        //
        // `q 退出` **不**写在这里：九宫格现在跟列表一样是顶层，左段的
        // escape_hint 已经常驻「q 退出」。原来要写是因为那时左段被
        // 「Esc 回列表」占着，q 没有别的地方交代。重复一遍只会挤掉
        // 句尾的 s/d——那两个是不可撤销的操作，比重复一次 q 重要得多。
        // 回复框开着时键盘整个归框，这时候再列动作键就是在教人按错——
        // 屏幕上写着做不到的操作比不写更糟。
        //
        // `Esc 取消` **不**写在这里：它已经常驻左段（`escape_hint`）。同一行
        // 里写两遍是拿最稀缺的一行去重复已知信息，而 `Ctrl+C 打断` 才是这里
        // 唯一「不写就找不到」的键——它跟 `s 停止` 一样是打断 agent 的动作，
        // 藏起来只会让用户眼看着它跑偏却不知道怎么喊停。
        View::Grid { reply: Some(_), .. } => help_items(
            &[("Enter", Key::SendReply), ("Ctrl+C", Key::InterruptAgent)],
            lang,
        ),
        // 九宫格：跟看板同一条上限（三条动作 + 门），候选键也几乎是同一批
        // ——`Tab`/`x` 现在两个视图都绑着（见 `grid::handle_key`），两处的
        // 前提也逐条相同（`can_switch_project` / `can_remove`）。
        //
        // `i 回一句` 排在 `n 新建` 前面：它是九宫格独有的能力，不写就找不到，
        // 而 `n` 在看板上、浮层里、到处都写着。没有聚焦会话时不写——回复框
        // 会开在一个不存在的会话上。
        //
        // 其余顺序跟看板那一支对齐（n → x → Tab），这样同一个键在两个视图里
        // 出现在同一个位置上，切模式不用重新找。
        View::Grid { .. } => {
            let mut rest: Vec<(&'static str, Key)> = Vec::new();
            if ctx.selected.is_some() {
                rest.push(("i", Key::ReplyOnce));
            }
            rest.push(("n", Key::New));
            if ctx.can_remove {
                rest.push(("x", Key::RemoveProject));
            }
            if ctx.can_switch_project {
                rest.push(("Tab", Key::SwitchProject));
            }
            help_items(&board_keys(ctx, ("Enter", Key::Zoom), &rest), lang)
        }
        // 验证中不接受任何操作，底部提示不该继续说「Enter 确认」——那会让人
        // 以为再按一次有用，其实这时候按键全被吞掉，只有 Esc 生效。
        View::EnterSecret {
            phase: SecretPhase::Verifying,
            ..
        } => help_items(&[("", Key::Verifying)], lang),
        // 跟 escape_hint 一样要分 return_to_settings：从设置页进来的 Esc
        // 回设置页，不是「列表」——两处文案哪怕只有半句话不一致，都是
        // 「底栏说什么就得真能做到什么」这条原则被破坏了一半。
        View::EnterSecret {
            return_to_settings: true,
            ..
        } => help_items(
            &[
                ("", Key::PasteOrTypeKey),
                ("Enter", Key::Confirm),
                ("Esc", Key::BackToSettingsWord),
            ],
            lang,
        ),
        View::EnterSecret { .. } => help_items(
            &[
                ("", Key::PasteOrTypeKey),
                ("Enter", Key::Confirm),
                ("Esc", Key::BackToListWord),
            ],
            lang,
        ),
        View::Secrets { .. } => help_items(
            &[
                ("↑↓", Key::Select),
                ("Enter", Key::Edit),
                ("d", Key::Delete),
                ("Esc", Key::Back),
            ],
            lang,
        ),
        View::Settings { .. } => help_items(
            &[
                ("↑↓", Key::Select),
                ("Enter", Key::Confirm),
                ("Esc", Key::Cancel),
            ],
            lang,
        ),
        // 手机页的按键随状态变化：没填过令牌只有 Enter 能按，配上人之后
        // 才谈得上 `r` 重新配对，`x` 只要还有令牌就能关掉。跟 `board_keys`
        // 那一档「能不能按也决定写不写」同一个道理——写一个此刻按下去
        // 只会报错的键，比不写更糟。
        View::Phone { status } => {
            use crate::proto::PhoneState;
            // **修复 6。** 令牌输入框开着的时候别再画 `status` 派生的那几个
            // 键——`Enter 填令牌`/`x 关掉` 在这个状态下是假的：`Enter` 会把
            // 这行提交成新令牌，`x` 只是敲进输入框的一个字母，真正能取消的
            // 是 `Esc`。继续画旧的那三个键，就是「底栏说什么就得真能做到
            // 什么」在这一页被破坏的样子。
            if ctx.phone_editing {
                return help_items(
                    &[
                        ("", Key::PasteOrTypeKey),
                        ("Enter", Key::Confirm),
                        ("Esc", Key::Cancel),
                    ],
                    lang,
                );
            }
            let mut items: Vec<(&'static str, Key)> = Vec::new();
            match status.state {
                PhoneState::Off | PhoneState::Broken(_) => {
                    items.push(("Enter", Key::PhoneEnterToken))
                }
                PhoneState::Paired => items.push(("r", Key::PhoneRepair)),
                PhoneState::WaitingForPairing => {}
            }
            if !matches!(status.state, PhoneState::Off) {
                items.push(("x", Key::PhoneTurnOff));
            }
            items.push(("Esc", Key::BackToSettingsWord));
            help_items(&items, lang)
        }
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

    /// 按键表拼成文字的样子。断言的是这一份，屏幕上画的是加粗版的同一份
    /// （`widgets::help_spans` 有单测钉住两者一致）。
    ///
    /// 注意：这里是**全表**，不是屏幕上真正显示的那几条——底栏会按宽度截断
    /// （`widgets::fit_help`）。「某个键在 80 列下真的看得见吗」是另一个问题，
    /// 由 `mod.rs` 里那几条画完整帧再数格子的测试回答。
    /// 默认按「光标停在一个会话上」算：那是键最全的一档，大多数断言问的是
    /// 「这个键在不在表里」，不是「什么时候不该在」。后者由 `help_when` 单独问。
    fn help_of(view: &View, lang: Lang) -> String {
        help_when(view, lang, on_a_session())
    }

    fn help_when(view: &View, lang: Lang, ctx: HelpCtx) -> String {
        crate::i18n::help_text(&idle_help(view, lang, ctx))
    }

    /// 默认那一档：光标停在一个正在跑的 agent 会话上，看板上不止一个项目。
    /// 这是键最全的一档——`Tab`/`s`/`u`/`d` 的前提都在，所以「某个键不该
    /// 出现」这类断言拿它当反例才有意义（前提不在的话，断言会因为前提不在
    /// 而通过，那是个假绿）。
    fn on_a_session() -> HelpCtx {
        HelpCtx {
            selected: Some(SelectedSession {
                is_agent: true,
                state: SessionState::Idle,
            }),
            can_remove: false,
            can_switch_project: true,
            phone_editing: false,
        }
    }

    /// 光标停在组头上（或者九宫格里一个会话都没有），但看板上有多个项目。
    fn on_a_header() -> HelpCtx {
        HelpCtx {
            selected: None,
            can_remove: false,
            can_switch_project: true,
            phone_editing: false,
        }
    }

    /// 光标停在一个**拿得掉**的组上（pinned 且一个会话都没有）。
    ///
    /// 上面两档都是 `can_remove: false`，于是 `idle_help` 里那两处
    /// `("x", RemoveProject)` 从来没被任何守卫扫到过——把它们写成中文
    /// 也是一路绿灯。这一档专门为它们而设。
    fn on_a_removable_group() -> HelpCtx {
        HelpCtx {
            selected: None,
            can_remove: true,
            can_switch_project: true,
            phone_editing: false,
        }
    }

    /// 底栏按键表的**键名列里不许出现汉字**，而且两种语言下必须一模一样。
    ///
    /// 为什么要单独一条：一条提示有两半，说明那半走 `text()`，`i18n` 的
    /// `no_english_entry_contains_han_characters` 管得着；键名那半是散落在
    /// `idle_help` 里的写死字面量，那条守卫**完全看不见**。于是把中文写进
    /// 键名列（`("←→/空格", ToggleCollapse)`）能一路走到英文界面上显示成
    /// `←→/空格 fold`，而全套测试都是绿的。这一条把另外那半也扫上。
    ///
    /// 键名列写的是**键盘上那个键叫什么**——`n`/`Tab`/`Esc`/`F3`/`↑↓`——
    /// 跟界面语言无关，所以「两种语言下完全相同」比「没有汉字」更强，
    /// 两条都断言。
    ///
    /// 视图是**逐个变体列全**的，不是挑几个有代表性的：漏掉的那一支正是
    /// 下一次会出事的地方。`idle_help` 的 match 加了新分支而这里没跟上时，
    /// 这条守卫不会自己报错——但下面 `assert!(!views.is_empty())` 旁边那句
    /// 注释会提醒改这里的人。
    #[test]
    fn no_key_column_is_ever_written_in_chinese() {
        use crate::i18n::has_han;
        use std::path::PathBuf;

        let secret = |return_to_settings, phase| View::EnterSecret {
            profile: "kimi".into(),
            label: "Kimi".into(),
            prompt: SecretPrompt {
                hint: String::new(),
                url: None,
            },
            buf: String::new(),
            phase,
            return_to_settings,
        };
        let mut typing = ProjectPicker::new(vec![], PathBuf::from("/"));
        typing.typing_path = Some(String::new());

        let views = vec![
            View::Board,
            View::Attached(1),
            View::grid(0),
            View::Grid {
                focus: 0,
                reply: Some(Draft {
                    id: 1,
                    text: String::new(),
                }),
            },
            View::PickProfile {
                entries: vec![entry("claude", ProfileStatus::Ready)],
                state: ListState::default(),
                warning: None,
            },
            View::PickProject(ProjectPicker::new(vec![], PathBuf::from("/"))),
            View::PickProject(typing),
            View::Settings {
                state: ListState::default(),
                sub: None,
            },
            secret(false, SecretPhase::Typing),
            secret(true, SecretPhase::Typing),
            secret(false, SecretPhase::Verifying),
            View::Keys {
                from: Box::new(View::Board),
            },
            View::Secrets {
                entries: vec![with_secret(entry("kimi", ProfileStatus::Ready))],
                state: ListState::default(),
                pending_delete: None,
            },
        ];
        // 加了 View 变体就往上面那张表里补一行——这条守卫只查得到被列出来的。
        assert!(!views.is_empty());

        // `View` 没有 `Debug`（里面挂着 `ListState` 之类），失败信息里用下标
        // 指回上面那张表就够了——表是按顺序写死的。
        for (i, v) in views.iter().enumerate() {
            for ctx in [on_a_session(), on_a_header(), on_a_removable_group()] {
                let en: Vec<&str> = idle_help(v, Lang::En, ctx)
                    .iter()
                    .map(|it| it.key)
                    .collect();
                let zh: Vec<&str> = idle_help(v, Lang::Zh, ctx)
                    .iter()
                    .map(|it| it.key)
                    .collect();
                for k in &en {
                    assert!(!has_han(k), "键名列里写了汉字：{k:?}（views[{i}]）");
                }
                assert_eq!(en, zh, "键名列跟着语言变了（views[{i}]）");
            }
        }
    }

    #[test]
    fn ctrl_q_now_reaches_the_agent_like_any_other_ctrl_combo() {
        // Ctrl+Q 曾是 dct 的全局逃生键，被 `key_to_input` 单独扣下不发。
        // 逃生键收敛成 F2 一个之后这条例外没了：0x11 跟别的 Ctrl 组合一样
        // 进 agent，屏幕上写着的 F2 才是唯一的退路。
        assert_eq!(key_to_input(&ctrl('q')).as_deref(), Some("\u{11}"));
        assert_eq!(key_to_input(&ctrl('Q')).as_deref(), Some("\u{11}"));
    }
    #[test]
    fn the_bottom_bar_offers_nothing_to_act_on_when_there_are_no_sessions() {
        let empty = HelpCtx::default();
        for view in [View::Board, View::grid(0)] {
            let help = help_when(&view, Lang::Zh, empty);
            for k in ["↑↓ 选择", "Enter", "s 停止", "u 回滚", "d 改动", "i 回一句"] {
                assert!(!help.contains(k), "一个会话都没有，不该写「{k}」：{help}");
            }
            assert!(help.contains("n 新建"), "这时候唯一该按的就是它：{help}");
            assert!(help.ends_with("? …"), "门永远在：{help}");
        }
    }

    /// **右段硬上限：三条动作 + 一个 `?`**，任何一档上下文都不许超。
    ///
    /// 这条替掉了原来那三条按 `s`/`u`/`d` 的可用性逐条问的测试
    /// （`..._never_offers_undo_on_a_shell_session` 等）。那三个键现在整个
    /// 不进底栏了，它们的可用性规则在这一层已经无从谈起——真正要守的变成
    /// 了「一行里到底能写几个键」，因为超出上限的后果不是报错，是排在后面
    /// 的键随终端宽度忽隐忽现。
    #[test]
    fn the_action_segment_is_capped_in_every_context() {
        let cases = [
            HelpCtx::default(),
            on_a_header(),
            on_a_session(),
            HelpCtx {
                can_remove: true,
                ..on_a_header()
            },
            // 结构上到不了的一档（选中会话的组不可能是空组），但上限本身
            // 不该依赖那个巧合——`truncate(3)` 就是靠这一档才被真正测到。
            HelpCtx {
                can_remove: true,
                ..on_a_session()
            },
        ];
        for view in [View::Board, View::grid(0)] {
            for ctx in cases {
                let items = idle_help(&view, Lang::Zh, ctx);
                assert!(
                    items.len() <= 4,
                    "{ctx:?} 下有 {} 条，超过 3 个动作 + ?",
                    items.len()
                );
                assert_eq!(items.last().map(|i| i.key), Some("?"), "门永远在尾巴上");
            }
        }
    }

    /// `x 移除` 只在**能按**的时候写：光标停在一个 pinned 且没有会话的组上。
    /// 别处写它，按下去只会得到一句「这个项目还有会话」——底栏在说谎。
    #[test]
    fn the_remove_key_only_shows_up_on_a_group_it_can_actually_remove() {
        let removable = HelpCtx {
            can_remove: true,
            ..on_a_header()
        };
        assert!(
            help_when(&View::Board, Lang::Zh, removable).contains("x 移除"),
            "空的 pinned 组上就该写它"
        );
        assert!(
            !help_when(&View::Board, Lang::Zh, on_a_session()).contains("x 移除"),
            "还有会话的组拿不掉，不该写"
        );
    }

    /// **修复 6 的回归测试。** 手机页令牌输入框开着的时候，`status` 派生
    /// 的那几个键（`Enter 填令牌`/`x 关掉`/`r 重新配对`）在这个状态下全是
    /// 假的：`Enter` 提交输入、`x` 只是敲进输入框的一个字母。底栏这时候
    /// 该说的是编辑态自己的那三个键，一个旧键都不该混进来。
    #[test]
    fn the_phone_bar_shows_editing_keys_while_the_token_field_is_open() {
        use crate::proto::{PhoneState, PhoneStatus};
        let editing = HelpCtx {
            phone_editing: true,
            ..on_a_header()
        };
        for state in [
            PhoneState::Off,
            PhoneState::WaitingForPairing,
            PhoneState::Paired,
            PhoneState::Broken("坏了".into()),
        ] {
            let view = View::Phone {
                status: PhoneStatus {
                    state,
                    bot: None,
                    owner: None,
                },
            };
            let help = help_when(&view, Lang::Zh, editing);
            assert!(
                help.contains("Enter") && help.contains("确认"),
                "编辑态该有 Enter 确认：{help}"
            );
            assert!(
                help.contains("Esc") && help.contains("取消"),
                "编辑态该有 Esc 取消：{help}"
            );
            for stale in ["填令牌", "关掉", "重新配对"] {
                assert!(
                    !help.contains(stale),
                    "输入框开着的时候不该再画「{stale}」这个旧键：{help}"
                );
            }
        }
    }

    /// `Tab` 和 `x` 现在两个视图都绑着，所以两边的按键表也得都写——键和表
    /// 必须在同一次改动里一起走。反过来，前提不成立时两边同样都不许写：
    /// 「屏幕上写着却按不动的键」这条规矩不分视图。
    ///
    /// 用「组头那一档」问（`selected: None`）：键最全的那一档里 `Enter`/`i`
    /// 会先占满三个位子，`Tab` 被 `truncate(3)` 截掉，断言就会因为截断而
    /// 通过——那是个假绿，问的根本不是「它在不在候选表里」。
    #[test]
    fn the_grid_advertises_the_project_keys_it_binds() {
        let removable = HelpCtx {
            can_remove: true,
            ..on_a_header()
        };
        let help = help_when(&View::grid(0), Lang::Zh, removable);
        for k in ["Tab 换项目", "x 移除"] {
            assert!(
                help.contains(k),
                "九宫格现在绑着「{k}」，就得写出来：{help}"
            );
        }

        // 前提不在就别写：只有一个项目时 `Tab` 原地打转，非空组 `x` 会被拒绝
        let alone = HelpCtx {
            can_remove: false,
            can_switch_project: false,
            ..on_a_header()
        };
        let help = help_when(&View::grid(0), Lang::Zh, alone);
        for k in ["Tab", "x 移除"] {
            assert!(!help.contains(k), "这一档按不动「{k}」：{help}");
        }
    }

    #[test]
    fn grid_hints_match_what_the_keys_actually_do() {
        // 底栏说什么就得真能做到什么。九宫格现在是顶层，逃生键是 q——
        // 「两个模式都是家」这条由 both_board_modes_are_top_level 单独钉住。
        let help = help_of(&View::grid(0), Lang::Zh);
        for k in [
            "Enter 放大",
            // `i 回一句` 是这个视图独有的能力，不写就找不到
            "i 回一句",
            "n 新建",
        ] {
            assert!(help.contains(k), "九宫格的按键表少了「{k}」：{help}");
        }
        // `q 退出` 不该出现在这一句里：它已经常驻左段（escape_hint），
        // 重复一遍是拿最稀缺的一行去重复已知信息。
        assert!(
            !help.contains("q 退出"),
            "左段已经写着 q 退出，这里不该重复：{help}"
        );
        // 尾巴上永远留着那扇门：上限之外的键全在门后。
        assert!(help.ends_with("? …"), "按键表尾巴上必须留着 `? …`：{help}");
        // 「这些键在 80 列终端上真的看得见吗」是另一个问题——这里是全表，
        // 屏幕上显示的是按宽度截过的一截。那个问题由 `mod.rs` 里把整帧画出来
        // 再数格子的几条测试回答（`the_three_actions_all_fit_at_eighty_columns`
        // 和 `the_door_to_the_rest_of_the_keys_is_always_on_screen`）。
    }

    /// 九宫格不再是列表的下一层：两个模式都是顶层，逃生键在两边都是
    /// 「q 退出」。写成「回列表」的话它就是 `g` 的一个隐藏同义词，
    /// 而屏幕上写的是「回列表」——用户会以为自己退出了什么。
    #[test]
    fn both_board_modes_are_top_level() {
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
        let help = help_of(
            &View::PickProject(ProjectPicker::new(
                Vec::new(),
                std::path::PathBuf::from("/tmp"),
            )),
            Lang::Zh,
        );
        for k in ["Tab 切换左右", "→ 进入文件夹", "← 上一级"] {
            assert!(help.contains(k), "帮助行少了「{k}」：{help}");
        }
    }

    /// `g` 现在不进底栏了（右段只有三个位子），它的去处是 `?` 浮层。
    /// 「两个模式各自把切过去的那个键写出来」这条要求没有放弃，只是搬了家
    /// ——由 `keys.rs` 的 `the_wording_follows_where_you_came_from` 盯着。
    /// 这里只保证底栏不会**假装**它在。
    #[test]
    fn the_bar_leaves_the_view_switch_key_to_the_overlay() {
        assert!(!help_of(&View::Board, Lang::Zh).contains("g "));
        assert!(!help_of(&View::grid(0), Lang::Zh).contains("g "));
    }

    #[test]
    fn expand_path_handles_tilde_and_relative() {
        let base = std::path::Path::new("/base");
        // 不直接读 `HOME`：Windows 上根本没有这个变量，家目录在
        // `USERPROFILE` 里。问 `sys::home()` 就是问被测代码问的同一个人。
        let home = crate::sys::home().unwrap();

        assert_eq!(expand_path("~/x", base), home.join("x"));
        assert_eq!(expand_path("~", base), home);
        assert_eq!(
            expand_path("rel/x", base),
            std::path::PathBuf::from("/base/rel/x")
        );
        // `~foo` 不是家目录展开，是个叫 ~foo 的相对路径
        assert_eq!(
            expand_path("~foo", base),
            std::path::PathBuf::from("/base/~foo")
        );

        // 「什么算绝对路径」是平台自己的规矩，两边写法不一样：`/abs/x` 在
        // Windows 上**不是**绝对路径（少了盘符），`Path::is_absolute` 对它
        // 返回 false，于是它会被当相对路径接到 base 后面。所以这一段分开写，
        // 而不是找一个两边都成立的写法——那种写法不存在。
        #[cfg(unix)]
        {
            assert_eq!(
                expand_path("/abs/x", base),
                std::path::PathBuf::from("/abs/x")
            );
            // 用户粘贴路径常带尾随空格
            assert_eq!(
                expand_path("  /abs/x  ", base),
                std::path::PathBuf::from("/abs/x")
            );
        }
        #[cfg(windows)]
        {
            assert_eq!(
                expand_path(r"C:\abs\x", base),
                std::path::PathBuf::from(r"C:\abs\x")
            );
            // 用户粘贴路径常带尾随空格
            assert_eq!(
                expand_path("  C:\\abs\\x  ", base),
                std::path::PathBuf::from(r"C:\abs\x")
            );
        }
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
        // Ctrl 组合照旧放过（Ctrl+Q 也不再例外——见
        // `ctrl_q_now_reaches_the_agent_like_any_other_ctrl_combo`），
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
        // 底栏说什么就必须真能做到什么。手输路径态的 Esc 是回列表
        // 不是回看板（见 `pick.rs` 的手输态分支），文案不能写成「回看板」。
        assert_eq!(escape_hint(&View::Board, Lang::Zh), "q 退出");
        // 会话视图的逃生键是 F2 独一份：Esc 归 agent，Ctrl+Q 已经没了
        assert_eq!(escape_hint(&View::Attached(1), Lang::Zh), "F2 回看板");
        assert_eq!(
            escape_hint(
                &View::PickProject(ProjectPicker {
                    filter: String::new(),
                    typing_path: None,
                    ..ProjectPicker::new(Vec::new(), std::path::PathBuf::from("/tmp"))
                }),
                Lang::Zh
            ),
            "Esc 回看板"
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
            "Esc 回列表"
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

    /// `n 新建` 是底栏三个位子里唯一无条件占一个的键——一个会话都没有时，
    /// 它是屏幕上唯一有意义的动作。`N 换 agent` 和 `c 密钥` 挪进了 `?` 浮层
    /// （由 `keys.rs` 的 `every_key_the_bar_drops_is_in_here` 盯着）。
    #[test]
    fn board_help_always_offers_a_new_session() {
        for ctx in [HelpCtx::default(), on_a_session()] {
            let help = help_when(&View::Board, Lang::Zh, ctx);
            assert!(help.contains("n 新建"), "{help}");
        }
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
        let help = help_of(
            &View::PickProfile {
                entries: vec![],
                state: ListState::default(),
                warning: None,
            },
            Lang::Zh,
        );
        assert!(help.contains("↑↓"));
        assert!(help.contains("数字"));
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
        let help = help_of(
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
        let help = help_of(
            &View::Secrets {
                entries: vec![],
                state: ListState::default(),
                pending_delete: None,
            },
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

    fn si(id: u32, dir: &str, profile: &str) -> crate::session::SessionInfo {
        crate::session::SessionInfo {
            id,
            dir: dir.into(),
            profile: profile.into(),
            state: SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        }
    }

    #[test]
    fn groups_are_sorted_by_path_and_sessions_by_id() {
        let all = vec![
            si(9, "/w/b", "claude"),
            si(2, "/w/a", "codex"),
            si(5, "/w/a", "claude"),
        ];
        let g = group_sessions(&all, &[], &BTreeMap::new());

        assert_eq!(g.len(), 2);
        assert_eq!(g[0].dir, PathBuf::from("/w/a"));
        assert_eq!(g[1].dir, PathBuf::from("/w/b"));
        assert_eq!(
            g[0].sessions.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![2, 5],
            "组内按 id 升序，固定"
        );
    }

    /// 规则 1：看板上的项目 = 有会话的 ∪ pinned 的。pinned 但没有会话的
    /// 项目必须以空组出现，否则光标没地方落、`n` 无处可去。
    #[test]
    fn a_pinned_project_with_no_sessions_still_gets_a_group() {
        let g = group_sessions(&[], &["/w/empty".to_string()], &BTreeMap::new());

        assert_eq!(g.len(), 1);
        assert!(g[0].sessions.is_empty());
        assert!(g[0].pinned);
    }

    #[test]
    fn a_project_that_is_both_pinned_and_busy_appears_once() {
        let all = vec![si(1, "/w/a", "claude")];
        let g = group_sessions(&all, &["/w/a".to_string()], &BTreeMap::new());

        assert_eq!(g.len(), 1, "pinned 和有会话是并集，不是两行");
        assert!(g[0].pinned);
        assert_eq!(g[0].sessions.len(), 1);
    }

    #[test]
    fn the_group_header_summarises_agents_and_failures() {
        let mut all = vec![
            si(1, "/w/a", "claude"),
            si(2, "/w/a", "claude"),
            si(3, "/w/a", "codex"),
        ];
        all[2].state = SessionState::Failed;
        let g = group_sessions(&all, &[], &BTreeMap::new());

        assert_eq!(
            g[0].agent_counts(),
            vec![("claude".to_string(), 2), ("codex".to_string(), 1)],
            "按 agent 名字排序，数量是这个项目里的会话数"
        );
        assert_eq!(g[0].failed(), 1);
    }

    #[test]
    fn a_group_carries_the_agent_that_project_used_last() {
        let mut profiles = BTreeMap::new();
        profiles.insert("/w/a".to_string(), "kimi".to_string());
        let g = group_sessions(&[], &["/w/a".to_string()], &profiles);

        assert_eq!(g[0].last_profile.as_deref(), Some("kimi"));
    }

    #[test]
    fn the_name_is_the_last_path_component_and_the_parent_is_shortened() {
        let g = group_sessions(&[], &["/w/dc/dc-terminal".to_string()], &BTreeMap::new());

        assert_eq!(g[0].name, "dc-terminal");
        assert_eq!(g[0].parent, "/w/dc");
    }

    #[test]
    fn grouping_nothing_at_all_yields_nothing() {
        assert!(group_sessions(&[], &[], &BTreeMap::new()).is_empty());
    }

    /// `canon()` 自己的文档写得很清楚：归一化只能用来比较，不能用来显示——
    /// 把 `/tmp` 显示成 `/private/tmp` 会让 macOS 上的界面凭空变丑。分组键
    /// (`dir`) 必须走 canon（否则同一个项目从两条拼法进来会被判成两个
    /// 项目），但 `name`/`parent` 必须来自用户敲的那条原始路径。这里用真
    /// 符号链接来验证，而不是拿两个字符串字面量摆样子——字面量根本不会
    /// 调用 `canonicalize`，测不出这个 bug。
    /// 符号链接：Windows 上建它要开发者模式或管理员权限，摆不出这个现场。
    #[test]
    #[cfg(unix)]
    fn display_name_and_parent_come_from_the_original_path_not_the_canonical_one() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let real = nested.join("actual-project");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("renamed-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        // 会话的 dir 是用户敲的那条链接路径，不是解析后的真实路径。
        let all = vec![si(1, &link.display().to_string(), "claude")];
        let g = group_sessions(&all, &[], &BTreeMap::new());

        assert_eq!(g.len(), 1);
        // 分组/比较键必须是归一化后的真实路径。
        assert_eq!(g[0].dir, std::fs::canonicalize(&real).unwrap());
        // 但显示必须保留用户敲的那条链接路径的拼写——如果错误地从归一化
        // 后的 `dir` 派生，这里会变成 "actual-project" / ".../nested"。
        assert_eq!(g[0].name, "renamed-link");
        assert_eq!(
            g[0].parent,
            crate::ui::widgets::short_path(&tmp.path().display().to_string())
        );
    }

    /// **同一个目录的两种拼法必须落进同一个组。**
    ///
    /// 用户从符号链接进项目、会话却是用真实路径建的（macOS 上 `/tmp` →
    /// `/private/tmp` 是现成的例子），字面比较会把它们劈成两个组：看板上多
    /// 一行同名项目，各自领着一半会话，而用户看不出任何原因。
    ///
    /// 上面那条 `display_name_and_parent_…` 盖不住它——它只有一个会话，
    /// 一个会话怎么分都是一个组。这条要的是**两条拼法各带一个会话**。
    /// （这份覆盖在改成分组模型时被一起删掉了，这里补回来。）
    /// 符号链接：Windows 上建它要开发者模式或管理员权限，摆不出这个现场。
    #[test]
    #[cfg(unix)]
    fn two_spellings_of_one_directory_land_in_a_single_group() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("proj");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let all = vec![
            si(1, &real.display().to_string(), "claude"),
            si(2, &link.display().to_string(), "claude"),
        ];
        let g = group_sessions(&all, &[], &BTreeMap::new());

        assert_eq!(g.len(), 1, "两种拼法指的是同一个项目，不能劈成两组");
        assert_eq!(
            g[0].sessions.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![1, 2],
            "两个会话都要归在这一个组底下"
        );
        // pinned 那一半同样要认得出：pin 的是链接、会话是用真实路径建的
        let g = group_sessions(&all, &[link.display().to_string()], &BTreeMap::new());
        assert_eq!(g.len(), 1, "pin 的拼法跟会话的拼法不同，也是同一个项目");
        assert!(g[0].pinned);
    }

    fn grp(dir: &str, ids: &[u32]) -> ProjectGroup {
        let sessions: Vec<_> = ids.iter().map(|i| si(*i, dir, "claude")).collect();
        let mut g = group_sessions(&sessions, &[dir.to_string()], &BTreeMap::new());
        g.remove(0)
    }

    #[test]
    fn flatten_puts_a_header_before_each_group() {
        let groups = vec![grp("/w/a", &[1, 2]), grp("/w/b", &[3])];
        assert_eq!(
            flatten(&groups),
            vec![
                Row::Header(0),
                Row::Session(0, 0),
                Row::Session(0, 1),
                Row::Header(1),
                Row::Session(1, 0),
            ]
        );
    }

    #[test]
    fn a_collapsed_group_contributes_only_its_header() {
        let mut groups = vec![grp("/w/a", &[1, 2]), grp("/w/b", &[3])];
        groups[0].collapsed = true;
        assert_eq!(
            flatten(&groups),
            vec![Row::Header(0), Row::Header(1), Row::Session(1, 0)]
        );
    }

    #[test]
    fn an_empty_group_still_contributes_its_header() {
        let groups = vec![grp("/w/empty", &[])];
        assert_eq!(flatten(&groups), vec![Row::Header(0)]);
    }

    #[test]
    fn group_of_answers_for_both_row_kinds() {
        let groups = vec![grp("/w/a", &[1]), grp("/w/b", &[2])];
        let rows = flatten(&groups);
        assert_eq!(group_of(&rows, 0), Some(0));
        assert_eq!(group_of(&rows, 1), Some(0));
        assert_eq!(group_of(&rows, 3), Some(1));
        assert_eq!(group_of(&rows, 99), None);
    }

    /// 本设计最关键的不变式：后台事件让行数变了，光标必须还站在同一个东西上。
    #[test]
    fn the_cursor_stays_on_the_same_session_when_another_group_grows() {
        let before = vec![grp("/w/a", &[1]), grp("/w/b", &[7])];
        let rows_before = flatten(&before);
        // 光标在 /w/b 的会话 7 上（第 3 行）
        let a = anchor_of(&before, &rows_before, 3).unwrap();
        assert_eq!(a, Anchor::Session(7, canon(Path::new("/w/b"))));

        // /w/a 里多开了两个会话，行数变了
        let after = vec![grp("/w/a", &[1, 4, 5]), grp("/w/b", &[7])];
        let rows_after = flatten(&after);
        let i = find_anchor(&after, &rows_after, &a).unwrap();

        assert_eq!(rows_after[i], Row::Session(1, 0), "还站在会话 7 上");
    }

    /// 会话没了（结束并被 prune）——退回它原来那个组的组头，不要滑到别的项目上。
    #[test]
    fn a_vanished_session_falls_back_to_its_own_group_header() {
        let before = vec![grp("/w/a", &[1]), grp("/w/b", &[7])];
        let rows_before = flatten(&before);
        let a = anchor_of(&before, &rows_before, 3).unwrap();

        let after = vec![grp("/w/a", &[1]), grp("/w/b", &[])];
        let rows_after = flatten(&after);
        let i = find_anchor(&after, &rows_after, &a).unwrap();

        assert_eq!(
            rows_after[i],
            Row::Header(1),
            "落在 /w/b 的组头上，不是 /w/a"
        );
    }

    #[test]
    fn a_header_anchor_finds_its_group_again_after_reordering() {
        let before = vec![grp("/w/b", &[7])];
        let rows_before = flatten(&before);
        let a = anchor_of(&before, &rows_before, 0).unwrap();
        assert_eq!(a, Anchor::Header(canon(Path::new("/w/b"))));

        // /w/a 是新出现的，排在前面，把 /w/b 挤到了第 2 行
        let after = vec![grp("/w/a", &[1]), grp("/w/b", &[7])];
        let rows_after = flatten(&after);
        let i = find_anchor(&after, &rows_after, &a).unwrap();

        assert_eq!(rows_after[i], Row::Header(1));
    }
}
