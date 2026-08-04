use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::profile::ProfileStatus;
use crate::proto::{ProfileEntry, SecretPrompt};
use crate::verify::VerifyOutcome;

use super::Msg;

#[derive(Clone)]
pub(crate) enum View {
    Board,
    Attached(u32),
    /// 九宫格：平铺所有会话的实时画面，只读。`focus` 是**全体会话**里的
    /// 下标（不是当页内的），当前页从它推导，见 `grid::page_of`。
    ///
    /// 只读是设计约束不是偷懒：一个会话的 PTY 只有一份尺寸，格子里能打字
    /// 就得把会话缩到格子那么小（见 tile-grid 设计文档）。要交互按 Enter
    /// 放大成附加视图。
    Grid {
        focus: usize,
    },
    PickProfile {
        entries: Vec<ProfileEntry>,
        state: ListState,
        /// 密钥文件读不了、自定义 profile 写错了。顶部红字。
        warning: Option<String>,
    },
    PickProject {
        /// 守护进程返回的完整列表，过滤不改动它
        all: Vec<String>,
        /// 用户打的字
        filter: String,
        state: ListState,
        /// Some 表示正处在「手输路径」的输入态
        typing_path: Option<String>,
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
        View::PickProject {
            all,
            filter,
            state,
            typing_path: Some(_),
        } => Some(View::PickProject {
            all,
            filter,
            state,
            typing_path: None,
        }),
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
        // Secrets 和 Grid 落在这条兜底里：它们跟 Attached/PickProject 一样
        // 只有一层，退一层就是看板。
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
pub fn pick_action(e: &ProfileEntry) -> PickAction {
    match &e.status {
        ProfileStatus::Ready => PickAction::Start(e.name.clone()),
        ProfileStatus::NeedsSecret => PickAction::AskSecret(0),
        ProfileStatus::NeedsDependency { label } => {
            PickAction::Blocked(format!("要先装 {label} 才能用 {}", e.label))
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
                PickAction::Blocked(format!("{} 没配置要运行的程序，用不了", e.label))
            }
            None => PickAction::Blocked(format!("本机没有找到 {command}")),
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
pub fn verify_message(o: VerifyOutcome) -> Option<String> {
    match o {
        VerifyOutcome::Ok => None,
        VerifyOutcome::BadKey => Some("这个密钥用不了，可能是复制的时候少了一段".into()),
        VerifyOutcome::Unreachable => Some("连不上服务器，检查一下网络".into()),
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

/// 底栏左段：逃生键提示。
///
/// 这是唯一一条「不管出什么事都必须还在」的信息——用户找不到它就只能去
/// 别的窗口 kill 进程，而 kill 会把终端留在 raw mode。文案必须跟
/// `back_one_level` 逐行对上：底栏说什么就得真能做到什么，
/// 手输路径态退的是一层（回列表），不能写成「回看板」。
pub(crate) fn escape_hint(view: &View) -> &'static str {
    match view {
        View::Board => "q 退出",
        View::PickProject {
            typing_path: Some(_),
            ..
        } => "Ctrl+Q 回列表",
        // 跟 back_one_level 保持一致：从密钥设置页进来的填密钥，退出回设置页，
        // 不是选择器，也不是看板——三条路各回各的，文案不能含糊成一句话。
        View::EnterSecret {
            return_to_settings: true,
            ..
        } => "Ctrl+Q 回设置",
        // 从选择器进来的填密钥，退出回的是选择器，不是看板
        View::EnterSecret { .. } => "Ctrl+Q 回列表",
        // 九宫格退回的也是看板，但对用户来说那一屏就是「列表」——
        // 站在格子里说「回看板」，用户会以为格子不算看板的一部分。
        View::Grid { .. } => "Ctrl+Q 回列表",
        _ => "Ctrl+Q 回看板",
    }
}

/// 底部提示条：没有消息覆盖时，按当前视图告诉用户能按什么键。
///
/// 抽成纯函数是为了能单测（同 `escape_hint`、`back_one_level`）——不用把
/// `draw()` 整条渲染管线跑一遍，只为了断言一句文案里有没有「↑↓」。
pub(crate) fn idle_help(view: &View) -> &'static str {
    match view {
        View::Attached(_) => "F2 同效　回看板后按 n 新建会话　其余按键都发给 agent",
        View::PickProfile { .. } => "↑↓ 选  Enter 确认  或直接按数字  Esc 取消",
        View::PickProject {
            typing_path: Some(_),
            ..
        } => "输入路径后 Enter 确认，Esc 返回列表",
        View::PickProject { .. } => "↑↓ 选  Enter 确认  直接打字过滤  Esc 取消",
        // `g 九宫格` 插在切换类按键那一段里，不放句尾：这一整句在 80 列
        // 终端上放不下会被右端截断，而 `g` 是九宫格唯一的入口——排在被截
        // 掉的那一截里等于没写。
        View::Board => {
            "n 新建  N 换 agent  p 换项目  c 密钥  g 九宫格  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动"
        }
        // 格子只读，键盘不会送进 agent，所以这里可以放心列一张按键表——
        // 跟会话视图不同（那边除了 F2 全转发，列按键表等于教人按错）。
        //
        // 跟看板那一句列的是同一批键（它们在两个视图里做的是同一件事），
        // 只把不一样的两处换掉：选择靠方向键、Enter 是放大而不是进入。
        // 九宫格独有的两个排在最前——`Ctrl+Q 回列表` 已经常驻左段，
        // 这里不再重复。
        View::Grid { .. } => {
            "方向键移动  Enter 放大  n 新建  N 换 agent  p 换项目  c 密钥  u 回滚  s 停止  d 改动"
        }
        // 验证中不接受任何操作，底部提示不该继续说「Enter 确认」——那会让人
        // 以为再按一次有用，其实这时候按键全被吞掉，只有 Esc 生效。
        View::EnterSecret {
            phase: SecretPhase::Verifying,
            ..
        } => "正在验证，请稍候　Esc 可取消",
        // 跟 escape_hint 一样要分 return_to_settings：从设置页进来的 Esc
        // 回设置页，不是「列表」——两处文案哪怕只有半句话不一致，都是
        // 「底栏说什么就得真能做到什么」这条原则被破坏了一半。
        View::EnterSecret {
            return_to_settings: true,
            ..
        } => "粘贴或输入密钥　Enter 确认　Esc 返回设置",
        View::EnterSecret { .. } => "粘贴或输入密钥　Enter 确认　Esc 返回列表",
        View::Secrets { .. } => "↑↓ 选  Enter 改  d 删  Esc 返回",
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
            back_one_level(View::PickProject {
                all: Vec::new(),
                filter: String::new(),
                state: ListState::default(),
                typing_path: None,
            }),
            Some(View::Board)
        ));
    }

    #[test]
    fn ctrl_q_leaves_the_typing_state_before_leaving_the_picker() {
        // 手输路径态退一层是回列表，不是一步退回看板
        let back = back_one_level(View::PickProject {
            all: vec!["/tmp/a".into()],
            filter: "a".into(),
            state: ListState::default(),
            typing_path: Some("/tmp/b".into()),
        });
        match back {
            Some(View::PickProject {
                typing_path,
                filter,
                all,
                ..
            }) => {
                assert_eq!(typing_path, None, "应当退出手输态");
                assert_eq!(filter, "a", "退一层不该顺手清掉过滤词");
                assert_eq!(all, vec!["/tmp/a".to_string()], "项目列表不该丢");
            }
            other => panic!("手输态应当退回列表态，实际是 {:?}", other.is_some()),
        }
    }

    #[test]
    fn grid_backs_out_to_the_board() {
        // 九宫格跟会话视图一样只有一层，Ctrl+Q 退回列表
        assert!(matches!(
            back_one_level(View::Grid { focus: 3 }),
            Some(View::Board)
        ));
    }

    #[test]
    fn grid_hints_match_what_the_keys_actually_do() {
        // 底栏说什么就得真能做到什么：九宫格的 Ctrl+Q 回的是列表那一屏
        assert_eq!(escape_hint(&View::Grid { focus: 0 }), "Ctrl+Q 回列表");
        let help = idle_help(&View::Grid { focus: 0 });
        for k in [
            "方向键移动",
            "Enter 放大",
            "n 新建",
            "N 换 agent",
            "p 换项目",
            "c 密钥",
            "u 回滚",
            "s 停止",
            "d 改动",
        ] {
            assert!(help.contains(k), "九宫格的按键表少了「{k}」：{help}");
        }
    }

    #[test]
    fn board_help_mentions_the_grid() {
        // 不写出来就没人会去按 g——九宫格是第二视图，没有别的入口
        assert!(idle_help(&View::Board).contains("g 九宫格"));
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

    #[test]
    fn escape_hint_matches_what_the_key_actually_does() {
        // 底栏说什么就必须真能做到什么。手输路径态的 Ctrl+Q 是回列表
        // 不是回看板（见 back_one_level），文案不能写成「回看板」。
        assert_eq!(escape_hint(&View::Board), "q 退出");
        assert_eq!(escape_hint(&View::Attached(1)), "Ctrl+Q 回看板");
        assert_eq!(
            escape_hint(&View::PickProject {
                all: Vec::new(),
                filter: String::new(),
                state: ListState::default(),
                typing_path: None,
            }),
            "Ctrl+Q 回看板"
        );
        assert_eq!(
            escape_hint(&View::PickProject {
                all: Vec::new(),
                filter: String::new(),
                state: ListState::default(),
                typing_path: Some(String::new()),
            }),
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
        assert!(matches!(pick_action(&e), PickAction::Start(n) if n == "claude"));
    }

    #[test]
    fn needs_secret_entry_opens_the_secret_view() {
        let e = entry("kimi", ProfileStatus::NeedsSecret);
        assert!(matches!(pick_action(&e), PickAction::AskSecret(_)));
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
        match pick_action(&e) {
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
        match pick_action(&e) {
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
        match pick_action(&e) {
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
        match pick_action(&e) {
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
        let help = idle_help(&View::Board);
        assert!(help.contains("n 新建"));
        assert!(help.contains("N 换 agent"));
    }

    // ———— Task 13：密钥设置页 ————

    #[test]
    fn board_help_mentions_the_settings_key() {
        assert!(idle_help(&View::Board).contains("c 密钥"));
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
        let help = idle_help(&View::PickProfile {
            entries: vec![],
            state: ListState::default(),
            warning: None,
        });
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
        let m = verify_message(VerifyOutcome::BadKey).unwrap();
        assert!(m.contains("密钥"));
        assert!(!m.contains("401"), "别把状态码甩给用户：{m}");
    }

    #[test]
    fn unreachable_blames_the_network_not_the_key() {
        let m = verify_message(VerifyOutcome::Unreachable).unwrap();
        assert!(
            m.contains("网络"),
            "连不上要说是网络，不能让用户去怀疑密钥：{m}"
        );
    }

    #[test]
    fn ok_has_no_message() {
        assert!(verify_message(VerifyOutcome::Ok).is_none());
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
        let h = escape_hint(&View::EnterSecret {
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
        let h = escape_hint(&View::EnterSecret {
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
        assert!(h.contains("设置"), "底栏说什么就得真能做到什么：{h}");
    }

    #[test]
    fn secret_view_from_settings_idle_help_also_says_back_to_settings() {
        // escape_hint 和 idle_help 都提了「Esc 回哪」，两处不能一处说设置、
        // 一处还说着旧的「列表」。
        let help = idle_help(&View::EnterSecret {
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
        let help = idle_help(&View::Secrets {
            entries: vec![],
            state: ListState::default(),
            pending_delete: None,
        });
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
}
