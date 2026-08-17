use crate::proto::{coded, ErrorCode, Operation};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::channel::{debounce, Event, EventKind, DEBOUNCE_WINDOW};
use crate::git::{self, FileStat};
use crate::profile::Profile;
use crate::pty::{PtySession, ScreenSpan};

/// 一屏文字 + 光标 + 滚动状态 + 会话状态：`screen()` 的返回值，行的集合按
/// (行, 列) 排布 span，光标是 (行, 列)。
///
/// 状态挤在这里而不是让界面另发一次 `List`：贴在会话里时界面只调 `Screen`
/// （`List` 要逐个锁所有会话、取每个的最后一行，16ms 一轮太贵），所以进程
/// 死了它一无所知——会永远画那张空缓冲，底栏还写着「其余按键都发给 agent」。
/// 状态是这条 16ms 通路上唯一能捎回来的存活信号，而这里本来就已经持着锁了。
pub struct ScreenSnapshot {
    pub lines: Vec<Vec<ScreenSpan>>,
    pub cursor: (u16, u16),
    pub scroll: ScrollState,
    pub state: SessionState,
}

/// 界面画底栏要用的全部滚动事实。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrollState {
    #[serde(default)]
    pub agent_owns: bool,
    #[serde(default)]
    pub alt_screen: bool,
    #[serde(default)]
    pub max: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub new_lines: usize,
}

/// `SessionManager::scroll` 的入参：相对滚几行，或者干脆回到底部。
/// 派生 `Serialize`/`Deserialize`：`proto::Request::Scroll` 直接把它嵌进
/// 线上请求，协议层不重新定义一份平行的滚动语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScrollBy {
    Rows(i32),
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Working,
    /// 由后续的 Bridge 在 agent 调用 ask_human 时设置；本计划内不会出现
    Asking,
    Idle,
    Stopped,
    /// agent 报错了（`error_pattern` 命中）。会话还活着、进程还在，
    /// 但屏幕上摆着一句失败——这跟「空闲」是两回事。
    Failed,
    /// profile 没给任何 pattern，我们不知道它在干什么。
    /// 显示「—」而不是猜一个——`shell` 以前就是被猜成「干活中」的。
    Unknown,
}

/// 一屏文字 → 状态。返回 `None` 表示「这屏说明不了任何事」，调用方
/// 保持原状态不动。
///
/// 抽成纯函数是为了能拿真实截屏当输入直接测。判定顺序有讲究：
///
/// - **错误压过一切。** 出错时屏幕上同时有错误和输入框提示，`idle_pattern`
///   一样匹得上；顺序反过来的话，最要紧的那个事实会被一句「空闲」盖掉——
///   用户以为 agent 在等他，其实那一轮已经废了。
/// - **busy 优先于 idle。** agent 干活时的「按 esc 中断」提示是稳定的，
///   而空闲时的输入框占位符用户一打字就没了。
fn classify(
    text: &str,
    error_re: Option<&regex::Regex>,
    busy_re: Option<&regex::Regex>,
    idle_re: Option<&regex::Regex>,
) -> Option<SessionState> {
    if error_re.is_some_and(|re| re.is_match(text)) {
        return Some(SessionState::Failed);
    }
    if let Some(re) = busy_re {
        return Some(if re.is_match(text) {
            SessionState::Working
        } else {
            SessionState::Idle
        });
    }
    if let Some(re) = idle_re {
        return Some(if re.is_match(text) {
            SessionState::Idle
        } else {
            SessionState::Working
        });
    }
    None
}

/// 该不该为这个会话叫醒用户的手机？三道门，全 AND。
///
/// - `is_agent`：命令行会话（shell）从来不该推送——用户自己在敲的东西，
///   没有「停下来了」这个概念。
/// - `!first_input_empty`（这里传入的是 `first_input_empty` 本身）：**这道
///   是关键。** 真实 profile（claude/codex/glm/kimi/deepseek/qwen-api）
///   全都只声明 `busy_pattern`，`classify()` 在 busy 串不在屏幕上时就判
///   Idle——而刚创建、还停在启动画面上的会话正是这样。没有这道门，
///   **每开一个会话手机就响一次**。跟 `tick()` 里起名字用的是同一个
///   判据、同一个理由，见那边的长注释。
/// - `has_channel`：没配手机通知（`SessionManager::set_event_sink` 没被
///   调过）就没有地方可推，试都不用试。
pub fn should_notify(is_agent: bool, first_input_empty: bool, has_channel: bool) -> bool {
    is_agent && !first_input_empty && has_channel
}

/// 让模型把一屏失败翻译成一句人话。
///
/// **只送屏幕末尾**：整屏可能几千字，而错误一定在末尾。整屏送过去既慢又贵，
/// 还容易让模型抓错重点。
pub fn explain_prompt(screen: &str) -> crate::llm::Prompt {
    const TAIL: usize = 2000;
    let tail: String = {
        let chars: Vec<char> = screen.chars().collect();
        let start = chars.len().saturating_sub(TAIL);
        chars[start..].iter().collect()
    };
    crate::llm::Prompt {
        system: "你在帮一个完全不懂编程的人。用中文，一到两句话说清楚刚才那个\
                 命令行工具出了什么事、他现在该做什么。不要出现英文报错原文、\
                 不要栈追踪、不要术语、不要代码。"
            .into(),
        user: format!("这是屏幕上的最后一段内容：\n\n{tail}"),
        max_tokens: 200,
    }
}

/// 名字最多留这么多字符。**按字符数、不按显示宽度**：守护进程存的是
/// 一段文字，画多宽是界面那一侧按各自的位置算的（见 `widgets::truncate`）。
/// 24 是**字符数**上限，是给不听话的模型留的兜底余量——prompt 里要的是
/// 不超过 12 个字，24 给英文答案（字母比汉字窄得多，字符数天然要多留
/// 一截）留出呼吸空间。这不是排版决定，真正按显示宽度做裁剪的是界面
/// 各处自己的事，跟这个常数无关。
const NAME_MAX_CHARS: usize = 24;

/// 把模型回的东西洗成一个能直接画在标题上的名字。
///
/// 模型很少老老实实只给名字：会加引号、会加句号、会多写一句解释。
/// 洗不干净的话屏幕上就会出现「「修登录白屏」。」。洗完是空串表示
/// 这次没拿到可用的答案，调用方走兜底。
pub(crate) fn clean_name(raw: &str) -> String {
    const QUOTES: [char; 12] = [
        '"', '\'', '「', '」', '『', '』', '“', '”', '‘', '’', '《', '》',
    ];
    const TAIL: [char; 12] = [
        '。', '．', '.', '，', ',', '！', '!', '？', '?', '；', ';', '、',
    ];

    let line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    // 前后两端剥的字符集**不对称**，这是故意的：
    // - 开头只剥引号和空白，不剥 TAIL 标点——名字本来就可能拿标点开头
    //   （`.NET 迁移`、`.env 权限` 都是这个工具的域里说得通的会话名），
    //   把开头的 `.` 当噪音铲掉会把真实名字铲坏。
    // - 结尾把引号、TAIL 标点、空白放进同一个字符集里一次性剥：模型常把
    //   整句话包在引号里、句末再补一个句号，比如「修登录白屏」。如果结尾
    //   也分两轮、各管一种字符集，剥标点那一轮会在够到句号后停在引号上、
    //   剥引号那一轮又认不得标点，两轮各退半步，谁都剥不干净，会留下
    //   「修登录白屏」这种带着里层引号的残留。合成一个字符集一次性从
    //   结尾向里扫，才能把「引号叠标点」这种情况一次剥到底。
    let line = line.trim_start_matches(|c: char| QUOTES.contains(&c) || c.is_whitespace());
    let line = line
        .trim_end_matches(|c: char| QUOTES.contains(&c) || TAIL.contains(&c) || c.is_whitespace());
    line.chars().take(NAME_MAX_CHARS).collect()
}

/// 退格类按键：真实效果是撤销上一个字符，不是产出一个要显示的符号。
/// `\x7f`（DEL）是现在大多数终端 Backspace 键实际发送的字节，`\x08`（BS）
/// 是老式终端的写法——两个都可能出现，都按同一个语义处理。
fn is_backspace(ch: char) -> bool {
    ch == '\x7f' || ch == '\x08'
}

/// 把 `text` 里的按键效果应用到 `out` 上，`cap` 是 `out` 允许的最大字符数
/// （`None` = 不限）。这是 `sanitize` 和 `append_capped` 共用的核心：前者
/// 一次性洗一整段（模型答案），后者跨多次调用增量地攒（附着视图逐键转发，
/// `out` 是持续存在的 `first_input` 缓冲区）——退格要能弹掉**上一次调用**
/// 追加的字符，所以这段逻辑必须直接对着持久化的 `out` 操作，不能先在
/// `text` 内部独立处理一遍再拼接。
///
/// 三类字符分开处理：
/// - 退格（`\x7f`/`\x08`）弹出 `out` 的最后一个字符，不是简单丢弃——
///   用户按退格是真心想删掉上一个字，`out` 里留下的得是他最终想表达的
///   那句话，不是键入序列的字面重放。
/// - **完整的 CSI 转义序列**（ESC `[` 参数字节* 终止字节）整段丢弃，不是
///   只丢 ESC 本身。方向键、Home/End/PageUp/PageDown/Delete/Insert
///   （`ui/mod.rs::key_to_input`）全都是这个形状，比如上箭头是
///   `\x1b[A`——序列后半截的 `[` 和 `A` 单独看都是普通可打印 ASCII 字符，
///   `char::is_control()` 认不出它们，只丢 ESC 会让 `[A` 原样漏进 `out`。
///   终止字节是 ASCII `0x40..=0x7E`（字母、`~`），前面允许任意多个参数/
///   中间字节（`\x1b[5~` 翻页键就带一个参数字节 `5`）。裸 `Esc`（后面
///   不跟 `[`，agent 拿它取消/清空/关弹窗）不产出任何字符，但也不用
///   往下吃字符——它本来就是单字节序列。
/// - 其余控制字符（Ctrl+字母等）直接丢弃，不占字符预算。
fn apply_keystrokes(out: &mut String, text: &str, cap: Option<usize>) {
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if is_backspace(ch) {
            out.pop();
        } else if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next(); // 吃掉 CSI 的引导字符 '['
                for next in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&next) {
                        break; // 终止字节，序列到此结束
                    }
                    // 否则是参数/中间字节，继续吃，直到吃到终止字节
                    // 或者这段文本本身就在这里用完。
                }
            }
            // 裸 Esc：什么都不产出，也没有后续字节要吃。
        } else if !ch.is_control() {
            let room = match cap {
                Some(n) => out.chars().count() < n,
                None => true,
            };
            if room {
                out.push(ch);
            }
        }
    }
}

/// 把一段可能夹着控制字符/转义序列的文本洗成干净的、能安全画在标题上的
/// 文本。
///
/// 两个调用方喂给它的字符串，最终都会存进 `name_slot`、再顺着看板列表项/
/// 九宫格标题/附着视图块标题一路走 `Line` → `Span::render_ref` 画到用户
/// 终端上——这条渲染路径不像 `Buffer::set_stringn`/`Paragraph` 那样过滤
/// 控制字符，零宽的控制字符会原样穿过 `truncate`，再原样写进终端（细节见
/// fix-1-brief）。两条调用路各自的“脏”字符来源完全不同：`request_name`
/// 里的兜底源头是用户在附着视图里逐键敲出来的原始按键字节（方向键、Esc、
/// Ctrl+字母这些 README 明确记录“每一次按键都转发给 agent”的东西，虽然
/// `append_capped` 已经在收集阶段处理过一轮，这里再洗一次是防御性的，
/// 不依赖调用方记得先洗）；模型答的名字源头是 `clean_name`，它只管引号
/// 和标点、不管控制字符——一段被操纵过的屏幕内容可以诱导模型把控制字符
/// 原样吐回来。两条路落地前必须过同一道过滤，漏一条就是漏一条到用户
/// 终端的注入路径。
fn sanitize(text: &str) -> String {
    let mut out = String::new();
    apply_keystrokes(&mut out, text, None);
    out
}

/// 把「已经封存的第一句话」变成一个能存进 `name_slot` 的兜底名字。
///
/// 抽成自由函数，跟 `collect_first_input` 同一个理由：这是一条能测的
/// 纯逻辑（截断 → 洗 → 判空），跟 `request_name` 里那圈锁、线程、模型
/// 调用无关，不该混在一起只能靠跑一整个 `SessionManager` 才测得到。
///
/// **不能假设 `first_input` 已经干净**：正常路径下它确实已经被
/// `append_capped` 洗过一轮（`collect_first_input` 是它唯一的写入口），
/// 但 `name_slot` 落地前的这道关卡不该依赖调用方记得先洗——`sanitize`
/// 在这里是防御性的最后一道，不是对上游的信任。
///
/// 洗完再 `trim()` 一次，重新判断是否为空：如果什么都不剩（比如用户
/// 只敲了个空格就回车），返回 `None`——调用方据此把 `name_slot` 留成
/// `None`，不写一个看不见的空字符串（`Some("")` 在 `session_label` 里
/// 跟真的没起出名字长得一模一样，`list()` 对两者做的都是
/// `unwrap_or_default()`）。
///
/// **`None` 不等于「还没试过」**：「问过没有」单独用
/// `Session::name_attempted` 记，不能再靠这个函数的返回值是不是 `None`
/// 来判断——细节和为什么不能靠 `name_slot`，见 `name_attempted` 自己的
/// 文档。
fn fallback_name(first_input: &str) -> Option<String> {
    let capped: String = first_input.chars().take(NAME_MAX_CHARS).collect();
    let cleaned = sanitize(&capped);
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

/// 把模型答的名字变成一个能存进 `name_slot` 的名字。
///
/// `clean_name` 只管引号和标点（见它自己的文档），不管控制字符——一段
/// 被操纵过的屏幕内容能诱导模型把控制字符原样吐回来，这里补上 `sanitize`
/// 那一道，跟 `fallback_name` 走的是同一份判空逻辑：洗完/去空白之后
/// 什么都不剩，返回 `None`，调用方不写、不覆盖已经在槽里的兜底。
///
/// 借用的是同一个 `sanitize`，所以模型答案里如果恰好带上 `\x7f`/`\x08`，
/// 也会被读成「退格，弹掉上一个字符」——`sanitize` 那套弹出语义原本是
/// 为**按键流**设计的（用户改错字），模型答案不是按键流，这里是把一个
/// 键盘领域的语义借到了文本领域。无害：退格类字节混进模型答案本来就
/// 极其罕见（那是屏幕内容被操纵之后模型复述出来的控制字节，不是正常
/// 语言输出的一部分），就算真的出现，按「弹出」还是按「丢弃」处理，
/// 结果都是「这个字节不会以原样留在名字里」——两种读法在这里要保的
/// 安全性质上没有区别，选「弹出」只是为了让 `sanitize` 保持单一实现、
/// 不用为两个调用方各写一套控制字符处理。
fn model_name(raw: &str) -> Option<String> {
    let cleaned = sanitize(&clean_name(raw));
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

/// 让模型给这个会话起个名字。
///
/// **只送屏幕末尾**，理由同 `explain_prompt`：整屏几千字，又慢又贵，
/// 还容易让模型抓错重点。
///
/// **语言写进 prompt，不做参数**：名字由守护进程生成并钉死，而界面语言
/// 用户随时能切（`l` 键，不重启 daemon）。跟着用户输入的语言走，切界面
/// 语言之后也不会留下一堆对不上的名字。
pub fn name_prompt(first_input: &str, screen: &str) -> crate::llm::Prompt {
    const TAIL: usize = 2000;
    let tail: String = {
        let chars: Vec<char> = screen.chars().collect();
        let start = chars.len().saturating_sub(TAIL);
        chars[start..].iter().collect()
    };
    crate::llm::Prompt {
        system: "给下面这个编程会话起一个名字，好让人在一屏几个会话里认出它。\
                 只回名字本身，不超过 12 个字。说的是这个会话在做的**任务**，\
                 不是它此刻的动作。不要引号、不要标点、不要「任务」「会话」\
                 这类没有信息的词。**用与用户那句话相同的语言。**"
            .into(),
        user: format!("用户说的第一句话：\n{first_input}\n\n屏幕上的最后一段内容：\n\n{tail}"),
        max_tokens: 64,
    }
}

/// 第一句输入最多留这么多字符。粘一大段需求时前 200 字足够喂模型，
/// 把几千字留在内存里没有意义。
const FIRST_INPUT_MAX: usize = 200;

/// 攒「用户对这个会话说的第一句话」。
///
/// 抽成自由函数是因为两个客户端送输入的形状完全不同（会话视图逐键、
/// 九宫格整段 + 一次空 `Input`），而这条规则必须对两条路给出同一个答案 ——
/// 那是能测的，`send_input` 里那一圈锁和 PTY 写入不是。
///
/// `text` 为空 = 按回车（见 `send_input` 的文档）。
pub(crate) fn collect_first_input(buf: &mut String, sealed: &mut bool, text: &str) {
    if *sealed {
        return;
    }
    if text.is_empty() {
        *sealed = true;
        return;
    }
    // `find` 给的是字节下标，而 `\r`/`\n` 都是 ASCII，切在这里一定是
    // 合法的字符边界。
    match text.find(['\r', '\n']) {
        Some(i) => {
            append_capped(buf, &text[..i]);
            *sealed = true;
        }
        None => append_capped(buf, text),
    }
}

/// 按**字符数**封顶追加，同时把退格、转义序列按真实按键语义处理（见
/// `apply_keystrokes`）。这里不按显示宽度算：这段字是喂给模型的原料，
/// 不是画在屏幕上的东西，宽度是界面那一侧的事。
///
/// 直接对 `buf` 操作、不能先在 `text` 内部独立处理一遍再拼接：附着视图
/// 是一个键一次 `send_input`（见 `collect_first_input` 的文档），退格键
/// 单独送过来的时候，`text` 里除了它自己什么都没有，要弹的字符在**上一次
/// 调用**追加进去的 `buf` 里。
fn append_capped(buf: &mut String, text: &str) {
    apply_keystrokes(buf, text, Some(FIRST_INPUT_MAX));
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: u32,
    pub profile: String,
    /// agent 干活的目录，就是用户指定的真实项目目录。
    pub dir: String,
    pub state: SessionState,
    /// 这个 agent 此刻在干什么（屏幕最后一行有内容的文字）。
    /// 看板靠它做"扫一眼全局"，不需要打开每个会话。
    pub activity: String,
    /// 是 agent 会话还是普通命令行。
    ///
    /// 界面**必须**知道这件事：`u 回滚` / `d 改动` 只对 agent 会话有效
    /// （`checkpoint_base` 对命令行会话直接返回 `NotAnAgentSession`），
    /// 底栏不能对着一个 shell 会话写这两个键——屏幕上写着做不到的操作
    /// 比不写更糟。
    ///
    /// 从守护进程侧的 `Session::is_agent` 原样带上来，不在界面侧靠 profile
    /// 名字猜：那是 profile.toml 里的一个声明（`profile.rs` 的 `is_agent`），
    /// 只有守护进程读得到，猜的迟早会跟真值分叉。
    pub is_agent: bool,
    /// 这个会话的稳定名字，守护进程在它第一次干完活时起一次，之后不变——
    /// 「只起一次」由守护进程侧的 `Session::name_attempted` 保证，不是靠
    /// 这个字段本身是不是空串（一个第一句话只有空白的会话也会被判定为
    /// 「已经问过」，即使问出来的结果是空）。
    ///
    /// 空串 = 还没起出来（刚建、没配 LLM、不是 agent 会话，或者对面是
    /// 认不得这个字段的旧守护进程）。**界面遇到空串一律退回 `profile`。**
    ///
    /// `#[serde(default)]` 是本版不升 `PROTOCOL_VERSION` 的依据：加纯读
    /// 字段时旧 JSON 补默认值，而 serde 反序列化本来就忽略不认识的字段，
    /// 所以新旧界面/守护进程怎么搭配都不会炸，只是没有名字。
    #[serde(default)]
    pub tag: String,
}

struct Session {
    id: u32,
    profile: Profile,
    dir: PathBuf,
    is_agent: bool,
    checkpoints: Vec<String>,
    state: SessionState,
    idle_re: Option<regex::Regex>,
    /// 干活时屏幕上一定有的串，tick() 里判定状态用。跟 idle_re 一起在
    /// 构造时编译好，profile 的正则错误在起会话这一刻就暴露，不拖到 tick。
    busy_re: Option<regex::Regex>,
    /// 出错时屏幕上一定有的串。跟上面两个一起在 `create()` 里编译一次，
    /// 不在 tick 里每轮重编——tick 每秒跑 5 次。
    error_re: Option<regex::Regex>,
    pty: PtySession,
    /// 出错原因的人话解释，由后台线程写回（见 `SessionManager::request_explanation`）。
    /// **必须是 `Arc<Mutex<_>>`**：那个线程拿不到 `Session` 的锁——`tick()`
    /// 正持着它。裸 `Option<String>` 编不过。
    explanation_slot: Arc<Mutex<Option<String>>>,
    /// 会话起名用的槽。跟 `explanation_slot` 平级、同一套用法。
    ///
    /// **`None` 不等于「还没触发过起名」**：`fallback_name` 允许兜底
    /// 本身就是 `None`（第一句话洗完/去空白之后什么都不剩，见它的
    /// 文档），这种会话被问过之后 `name_slot` 也是 `None`。「问没问过」
    /// 单独用 `name_attempted` 记，不能再靠这个字段是不是 `None` 判断。
    name_slot: Arc<Mutex<Option<String>>>,
    /// 用户对这个会话说的第一句话，起名用。只在 agent 会话上攒。
    first_input: String,
    /// 第一句攒完了没有。见 `collect_first_input`。
    first_input_sealed: bool,
    /// 起名有没有被**真正尝试过一次**——这是「只问一次」唯一的门槛。
    ///
    /// 不能拿 `name_slot.is_none()` 当门槛：`fallback_name` 允许兜底
    /// 本身就是 `None`，如果继续靠 `name_slot` 判断「问过没有」，一个
    /// 第一句话只有空白的会话会在**每一次** Working → Idle 都重新触发
    /// `request_name`——白白多打一次模型（`request_explanation` 的文档
    /// 里要躲的是同一种坑：一个失败会话能把额度烧光），而且会有两个
    /// 后台起名线程同时在飞：后触发的那次 `request_name` 会同步把
    /// `name_slot` 写回 `None`，把前一个线程已经写进去的真名字覆盖
    /// 掉——一次丢失更新。`request_name` 一进门就把这里设成 `true`，
    /// 之后这个会话再也不会被 `tick()` 认为该起名。
    name_attempted: bool,
    /// 第几次问过解释了。每次**进入** Failed 都自增，连同当时的号码一起
    /// 交给那一轮的后台线程——线程算完之后先比一遍号码还对不对，不对就
    /// 说明中途又失败过一次、有更新的问题在问，这份迟到的旧答案就不写了。
    /// 没有这道防线的话，一个卡了很久的旧回答有可能在新一轮的新回答
    /// 写回去**之后**才姗姗来迟，把新答案覆盖成旧的。
    explanation_gen: Arc<AtomicU64>,
    /// 用户上次**主动**滚动时的偏移。`new_lines` 靠它算：vt100 会在新行
    /// 推入时自动把偏移 +1（grid.rs:556-558，画面因此不动），所以
    /// 「偏移 - 这个标记」就正好是用户没看过的行数。
    ///
    /// 边界：偏移增长被历史总行数封顶，缓冲满 2000 行之后 new_lines 会
    /// 少算，画面也会开始往上飘（最老的行被挤掉了）。这是环形缓冲的
    /// 固有代价。
    scroll_mark: usize,
    /// 这个会话上次真的把事件送进手机通知队列的时刻，`debounce()` 的
    /// `last` 参数。相对 `SessionManager::started`，不是挂钟时间——
    /// 见那个字段的文档，理由跟 `channel::debounce` 用 `Duration` 而不是
    /// `Instant` 一样：这里是唯一需要拿它做减法的地方。
    last_notified: Option<Duration>,
}

/// `SessionManager` 内部可变——所有方法都是 `&self`，好让它以 `Arc<SessionManager>`
/// 的形式在多个连接线程之间共享，而不需要一把包住整个 manager 的外层大锁。
///
/// 关键设计：`create()` 里唯一的共享状态改动是最后把新 `Session` 插进 `sessions`
/// 这个 `HashMap`；开 worktree、跑 checkpoint、起 PTY 这些可能很慢的操作全部在
/// 拿到锁之前做完。这样一个客户端在建慢会话（比如仓库文件很多，`git worktree add`
/// 要跑上大半秒）的时候，其它客户端的 `list`/`screen`/`tick` 不会被一起拖住——
/// 它们最多等一次 `HashMap` 插入/查找的时间，跟文件数量无关。
///
/// 每个会话又单独包一层 `Mutex`，所以不同会话之间的操作（比如两个会话各自的
/// `send_input`）也互不阻塞；只有同一个会话的并发操作会互相排队，这本来就是
/// 应该的。
pub struct SessionManager {
    next_id: AtomicU32,
    sessions: Mutex<HashMap<u32, Arc<Mutex<Session>>>>,
    extra_profiles: Mutex<HashMap<String, Profile>>,
    /// 会话的生死记在这里。默认不落盘（见 `journal::Journal`），
    /// 只有 `daemon::run()` 会给它一个真实路径。
    pub journal: crate::journal::Journal,
    /// 出错解释要用的后端。`None` = 没配 LLM，功能安静下线（见
    /// `request_explanation`）。守护进程启动时 resolve 一次填进来。
    backend: Mutex<Option<Arc<dyn crate::llm::Backend>>>,
    /// 上面那次 resolve 为什么失败。**只有用户确实写了 `[llm]` 却接不上时
    /// 才是 `Some`**——没写 `[llm]` 是绝大多数人的正常状态，不是问题，
    /// 那种情况这里始终是 `None`。存下来是因为守护进程的 stderr 被丢弃了
    /// （见 `proto::WarningCode::LlmUnavailable`），这是这条原因唯一能走到
    /// 用户眼前的路。
    llm_problem: Mutex<Option<crate::llm::resolve::ResolveError>>,
    /// `tick()` 往手机通知队列投事件用的出口。**unbounded**——Ruling 4：
    /// `tick()` 绝不能因为投递这件事阻塞，一个 `mpsc::Sender` 的
    /// `send()` 本来就不会阻塞（它只会让底层队列变长），有界的那一半
    /// 由 `bridge.rs` 在消费端做，见那边的 `QUEUE_CAP`。`None` = 没配
    /// 手机通知（`set_event_sink` 没被调过），这时候 `should_notify` 的
    /// 第三道门（`has_channel`）直接判假，tick() 连 `send()` 都不会试。
    event_tx: Mutex<Option<mpsc::Sender<Event>>>,
    /// `Session::last_notified` 记的时刻的起点。用相对时长而不是挂钟
    /// 时间是为了配合 `channel::debounce`（`Instant` 造不出「10 秒前」，
    /// 测试需要确定的时间点）；这里只需要一个单调、进程存活期内不变的
    /// 参照系，`Instant` 正合适。
    started: Instant,
}

/// 统一处理锁中毒：某个持锁线程如果 panic 过一次，我们选择拿到里面的数据继续跑，
/// 而不是让后续所有请求都跟着报错卡死（守护进程没有 supervisor 帮忙重启，
/// 中毒了就是永久瘫痪，必须能自愈）。
pub(crate) fn recover<T>(r: std::sync::LockResult<T>) -> T {
    r.unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        SessionManager {
            next_id: AtomicU32::new(1),
            sessions: Mutex::new(HashMap::new()),
            extra_profiles: Mutex::new(HashMap::new()),
            journal: crate::journal::Journal::new(),
            backend: Mutex::new(None),
            llm_problem: Mutex::new(None),
            event_tx: Mutex::new(None),
            started: Instant::now(),
        }
    }

    /// 接上手机通知队列的入口。`daemon.rs` 在配好 `Bridge` 之后调用一次；
    /// 测试直接建一对 `mpsc::channel()` 传进来，不用起真的 bridge。
    /// 不调用这个方法的话，`should_notify` 的第三道门永远判假——没配
    /// 手机通知的人，`tick()` 不会试着往哪里发任何东西。
    pub fn set_event_sink(&self, tx: mpsc::Sender<Event>) {
        *recover(self.event_tx.lock()) = Some(tx);
    }

    /// 装上（或摘掉）出错解释要用的后端。守护进程启动时 resolve 一次调用，
    /// resolve 失败就传 `None`——功能安静下线，不影响会话本身跑不跑得起来。
    pub fn set_backend(&self, b: Option<Arc<dyn crate::llm::Backend>>) {
        *recover(self.backend.lock()) = b;
    }

    /// 记下（或清掉）「用户开了出错解释，但连不上」的原因。
    /// `Request::Profiles` 会把它当成一条警告顶到界面上——守护进程的
    /// stderr 是被丢弃的，不记下来就等于没说过。
    pub fn set_llm_problem(&self, p: Option<crate::llm::resolve::ResolveError>) {
        *recover(self.llm_problem.lock()) = p;
    }

    pub fn llm_problem(&self) -> Option<crate::llm::resolve::ResolveError> {
        recover(self.llm_problem.lock()).clone()
    }

    /// 读一个会话此刻的出错解释。没有后端、还没问完、或者问失败了，
    /// 都是 `None`——调用方（daemon/界面）不用区分这三种情况，
    /// 统一当作「暂时没有」处理。
    pub fn explanation(&self, id: u32) -> Option<String> {
        self.with_session(id, |s| Ok(recover(s.explanation_slot.lock()).clone()))
            .unwrap_or(None)
    }

    /// 只给测试用：不暴露真正的后端（没有理由把它 clone 出去），只答
    /// 「装没装上」这一个布尔值。`daemon.rs` 的启动测试要钉的正是「没写
    /// `[llm]` 时压根不该装」，这个问题不该靠一次真实网络调用去间接猜。
    #[cfg(test)]
    pub(crate) fn backend_is_set(&self) -> bool {
        recover(self.backend.lock()).is_some()
    }

    /// 注册内置之外的 profile（测试用，也是将来从磁盘加载自定义 profile 的入口）
    pub fn register_profile(&self, p: Profile) {
        recover(self.extra_profiles.lock()).insert(p.name.clone(), p);
    }

    /// `profiles` 是调用方（daemon）已经算好的「内置 + 磁盘」全集（见
    /// `profile::all_profiles`），排在最前面查——用户在磁盘上新建或覆盖的
    /// profile 必须能被 `create()` 找到，不然「UI 说这个 agent 能用」和
    /// 「create() 说没这个 profile」就对不上。`extra_profiles` 仍然保留在
    /// 它后面：那是测试专用的注册入口（见 `register_profile` 的注释），
    /// 不该因为这次改动而失效。最后才落到编译进二进制的内置表，
    /// 兜住 `profiles` 传空切片的调用方（比如本文件里一大堆不关心磁盘
    /// profile 的单元测试）。
    fn resolve_profile(&self, name: &str, profiles: &[Profile]) -> Result<Profile> {
        if let Some(p) = profiles.iter().find(|p| p.name == name) {
            return Ok(p.clone());
        }
        if let Some(p) = recover(self.extra_profiles.lock()).get(name) {
            return Ok(p.clone());
        }
        Profile::builtin(name).ok_or_else(|| coded(ErrorCode::NoSuchProfile(name.to_string())))
    }

    /// `secret` 是调用方已经查好的那一条密钥（如果这个 profile 需要密钥、且用户填过的话），
    /// 不是整个密钥仓。`create()` 本身只用得上这一条，让它捧着整仓密钥走完这段慢流程，
    /// 是在放大暴露面而不是缩小它；调用方（`daemon.rs`）在查这一条的时候也只需要
    /// 极短暂地持锁，不必在 PTY 起进程、git checkpoint 这些慢操作期间攥着锁不放
    /// （原则见下面「以下全是慢操作」那段注释，和调用方 `daemon.rs::handle` 的注释）。
    pub fn create(
        &self,
        dir: &Path,
        profile_name: &str,
        secret: Option<&str>,
        profiles: &[Profile],
    ) -> Result<u32> {
        let profile = self.resolve_profile(profile_name, profiles)?;

        if !dir.is_dir() {
            return Err(coded(ErrorCode::DirNotFound(dir.display().to_string())));
        }

        // 原子分配 id：即便后面的慢操作失败，这个 id 也不会被复用给另一次并发的
        // create() ——避免两个同时进行的 create() 撞到同一个 worktree 分支名。
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);

        // 以下全是慢操作（可能牵扯好几个 git 子进程），刻意不持有任何锁：
        // 这个新会话在插入 `sessions` 之前，对其它请求完全不可见，
        // 没有并发正确性需要靠锁来保护。
        // agent 直接在用户的真实项目里干活。检查点是隐藏快照，不动分支和历史，
        // 所以仍然要求是 git 仓库——没有 git 就没有撤销。
        if profile.is_agent && !git::is_repo(dir) {
            return Err(coded(ErrorCode::NotAGitRepo(dir.display().to_string())));
        }

        let idle_re = profile.idle_regex()?;
        let busy_re = profile.busy_regex()?;
        let error_re = profile.error_regex()?;
        let is_agent = profile.is_agent;

        // 有 pattern 才敢说「干活中」：agent 刚起来确实在初始化。
        // 没 pattern 就一直是 Unknown，tick 也不会改它。
        let state = if idle_re.is_some() || busy_re.is_some() {
            SessionState::Working
        } else {
            SessionState::Unknown
        };

        // profile 的静态 env 打底，密钥覆盖上去。密钥不在 profile 文件里，
        // 只在这一步才和命令合到一起——profile 文件因此可以随便拷贝分享。
        //
        // 密钥缺失在这里**不报错**：能不能用是可用性/UI 层的事（后续任务），
        // create() 拦一遍会让「先装上 CLI 试试能不能跑」这种路径莫名其妙失败。
        let mut env = profile.env.clone();
        if let Some(spec) = &profile.secret {
            if let Some(key) = secret {
                env.insert(spec.env.clone(), key.to_string());
            }
        }

        let pty = PtySession::spawn(&profile.command, &env, dir, 40, 120)?;

        let mut checkpoints = Vec::new();
        if is_agent {
            // IMPORTANT 5（最终整分支 code review）：`git::checkpoint` 失败时
            // 甩出来的是 git 命令行的原始英文 stderr——`git.rs` 的注释说
            // 「调用方负责给出中文的上下文」，这里补上，别让一句
            // 「fatal: detected dubious ownership in repository at …」
            // 原样飘到选择器/密钥失败提示上（后者尤其误导，会被用户读成
            // 「我的密钥不对」）。
            checkpoints.push(
                git::checkpoint(dir, id, 0)
                    .map_err(|_| coded(ErrorCode::OperationFailed(Operation::FirstCheckpoint)))?,
            );
        }

        let session = Session {
            id,
            profile,
            dir: dir.to_path_buf(),
            is_agent,
            checkpoints,
            state,
            idle_re,
            busy_re,
            error_re,
            pty,
            explanation_slot: Arc::new(Mutex::new(None)),
            name_slot: Arc::new(Mutex::new(None)),
            first_input: String::new(),
            first_input_sealed: false,
            name_attempted: false,
            explanation_gen: Arc::new(AtomicU64::new(0)),
            scroll_mark: 0,
            last_notified: None,
        };

        // 出生也记一笔：只有死亡记录的话，日志里满是「某某没了」却看不出
        // 它是什么时候、在哪个项目起来的，对不上「我刚才按了什么」。
        self.journal
            .born(id, &session.profile.name, dir, session.pty.process_id());

        // 唯一需要锁的地方，而且只做一次 HashMap 插入，跟慢操作耗时无关。
        recover(self.sessions.lock()).insert(id, Arc::new(Mutex::new(session)));
        Ok(id)
    }

    /// 测试专用：直接读一个会话此刻的整屏文本。不走协议、不用等 `screen()`
    /// 的样式分段，省得每条断言都要自己拼 spans。
    #[cfg(test)]
    pub fn screen_text_for_test(&self, id: u32) -> String {
        self.with_session(id, |s| Ok(s.pty.screen_text()))
            .unwrap_or_default()
    }

    fn get_arc(&self, id: u32) -> Result<Arc<Mutex<Session>>> {
        recover(self.sessions.lock())
            .get(&id)
            .cloned()
            .ok_or_else(|| coded(ErrorCode::NoSuchSession(id)))
    }

    /// 找到会话、拿到它自己的锁、跑 `f`——`sessions` 这个总表的锁只用来查一次
    /// `Arc`，不会在 `f`（可能是慢的 git 操作）执行期间被一直占着。
    fn with_session<R>(&self, id: u32, f: impl FnOnce(&mut Session) -> Result<R>) -> Result<R> {
        let arc = self.get_arc(id)?;
        let mut guard = recover(arc.lock());
        f(&mut guard)
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        let snapshot: Vec<Arc<Mutex<Session>>> =
            recover(self.sessions.lock()).values().cloned().collect();

        let mut v: Vec<SessionInfo> = snapshot
            .iter()
            .map(|s| {
                let s = recover(s.lock());
                let tag = recover(s.name_slot.lock()).clone().unwrap_or_default();
                SessionInfo {
                    id: s.id,
                    profile: s.profile.name.clone(),
                    dir: s.dir.display().to_string(),
                    state: s.state,
                    activity: s.pty.last_line(),
                    is_agent: s.is_agent,
                    tag,
                }
            })
            .collect();
        v.sort_by_key(|s| s.id);
        v
    }

    /// 送内容给 agent。`text` 为空表示回车，也就是一轮的开始。
    /// **只有回车才打检查点**——逐字符输入不能每敲一下就拍一次快照。
    ///
    /// 拍快照可能很慢（大仓库要跑好几个 git 子进程），所以**全程不持会话锁**：
    /// 先拿到需要的信息就放锁，慢活做完再回来把结果写进去。持锁做慢活会让
    /// 这个会话卡住整个看板——`list()` 要逐个锁会话取状态。
    pub fn send_input(&self, id: u32, text: &str) -> Result<()> {
        let arc = self.get_arc(id)?;

        {
            // 攒第一句。**在所有分支之前**——下面空串那一支会提早 return，
            // 挂在它后面就永远收不到回车。
            let mut guard = recover(arc.lock());
            let s = &mut *guard;
            if s.is_agent {
                collect_first_input(&mut s.first_input, &mut s.first_input_sealed, text);
            }
        }

        if text.is_empty() {
            let (dir, sid, seq, is_agent) = {
                let s = recover(arc.lock());
                (s.dir.clone(), s.id, s.checkpoints.len(), s.is_agent)
            };

            if is_agent {
                // 慢，无锁。失败时给中文上下文，理由同 create() 里那处——
                // 见那边的注释。
                let sha = git::checkpoint(&dir, sid, seq)
                    .map_err(|_| coded(ErrorCode::OperationFailed(Operation::Checkpoint)))?;
                let mut s = recover(arc.lock());
                if s.checkpoints.last() != Some(&sha) {
                    s.checkpoints.push(sha);
                }
                s.state = SessionState::Working;
            } else {
                recover(arc.lock()).state = SessionState::Working;
            }

            let mut g = recover(arc.lock());
            // 一敲键就回到底部。滚上去的时候打字，字会落在看不见的地方，
            // 用户会以为键盘坏了。归零之后字符照常送出去，不吞。
            g.pty.scroll_to_bottom();
            g.scroll_mark = 0;
            return g.pty.write(b"\r");
        }

        let mut g = recover(arc.lock());
        // 一敲键就回到底部。滚上去的时候打字，字会落在看不见的地方，
        // 用户会以为键盘坏了。归零之后字符照常送出去，不吞。
        g.pty.scroll_to_bottom();
        g.scroll_mark = 0;
        g.pty.write(text.as_bytes())
    }

    /// 返回 agent 屏幕文本、光标位置 (行, 列)、滚动状态、会话状态。光标必须
    /// 跟文本一起取，否则界面只是一张死截图，用户看不出自己打的字落在哪。
    pub fn screen(&self, id: u32) -> Result<ScreenSnapshot> {
        self.with_session(id, |s| {
            let v = s.pty.scroll_state();
            Ok(ScreenSnapshot {
                lines: s.pty.screen_spans(),
                cursor: s.pty.cursor(),
                scroll: state_of(v, s.scroll_mark),
                state: s.state,
            })
        })
    }

    /// 用户主动滚动：相对滚几行，或者直接回到底部。
    pub fn scroll(&self, id: u32, by: ScrollBy) -> Result<ScrollState> {
        self.with_session(id, |s| {
            let v = match by {
                ScrollBy::Rows(n) => s.pty.scroll_by(n),
                ScrollBy::Bottom => s.pty.scroll_to_bottom(),
            };
            // 用户主动滚过了，「没看过的行数」从这一刻重新算
            s.scroll_mark = v.offset;
            Ok(state_of(v, s.scroll_mark))
        })
    }

    /// 把界面转发过来的鼠标事件按 agent 当前的编码写进 PTY。
    /// 编不编、编成什么样，全由 `PtySession::write_mouse` 按 agent 当前
    /// 订阅的协议/编码决定——这里只是把线路接通。
    pub fn forward_mouse(&self, id: u32, ev: crate::proto::MouseForward) -> Result<()> {
        self.with_session(id, |s| s.pty.write_mouse(ev))
    }

    /// 一次取多个会话的屏幕，九宫格用。锁的纪律跟 `list()` 一致：
    /// 逐个短暂拿锁，不跨会话持有任何东西。不存在的 id 跳过——
    /// 会话可能在两次轮询之间被停掉，这不是错误。
    pub fn screens(&self, ids: &[u32]) -> Vec<crate::proto::ScreenEntry> {
        ids.iter()
            .filter_map(|id| {
                let arc = self.get_arc(*id).ok()?;
                let s = recover(arc.lock());
                Some(crate::proto::ScreenEntry {
                    id: *id,
                    lines: s.pty.screen_spans(),
                })
            })
            .collect()
    }

    /// 改会话的显示尺寸。界面尺寸变了就要跟着调，否则 agent 按错的宽度排版。
    pub fn resize(&self, id: u32, rows: u16, cols: u16) -> Result<()> {
        self.with_session(id, |s| {
            s.pty.resize(rows, cols)?;
            // vt100 会按新宽度重排，偏移指向的行跟改之前不是同一行了。
            // 与其显示一个错位的画面，不如老老实实回到底部。
            s.pty.scroll_to_bottom();
            s.scroll_mark = 0;
            Ok(())
        })
    }

    pub fn stop(&self, id: u32) -> Result<()> {
        self.with_session(id, |s| {
            // pid 要在 kill 之前取：杀完再问就已经被回收了。
            let pid = s.pty.process_id();
            s.pty.kill()?;
            s.state = SessionState::Stopped;
            // `requested` 和 `tick()` 里那条 `vanished` 是这本日志唯一
            // 分得开的两件事——见 `journal` 的模块注释。
            self.journal.died(id, crate::journal::Death::Requested, pid);
            Ok(())
        })
    }

    /// 强杀：跟 `stop` 同一个落点（`state` 置 `Stopped`），只是不给那 200ms。
    ///
    /// 状态必须跟 `stop` 一致，不能另立一个「被强杀的」状态：对用户来说
    /// 这两条命令的结果是同一件事——这个会话不跑了。多一个状态就要在看板、
    /// 九宫格、`dct ps` 三处各给它一种画法，而它们要表达的话是同一句。
    pub fn kill(&self, id: u32) -> Result<()> {
        self.with_session(id, |s| {
            s.pty.kill_now()?;
            s.state = SessionState::Stopped;
            Ok(())
        })
    }

    /// 把已经停掉的会话从名册上抹掉，返回抹掉了几个。
    ///
    /// **两趟，跟 `list()` 同一套锁纪律**：先逐个短暂拿会话锁挑出该删的 id，
    /// 再拿 map 锁删。倒过来（持 map 锁去逐个锁会话）会让整个看板卡在
    /// 一个正在做慢活的会话上——`list()` 每 150ms 就要走一遍同一批锁。
    ///
    /// 被删的 `Session` 在这里落地析构，`PtySession::Drop` 会兜底再回收一次
    /// 子进程。那是空操作（这些会话已经停了），但不能省：判成 `Stopped` 的
    /// 路径不止 `stop()` 一条，`tick()` 里那条「进程自己没了」也算。
    pub fn prune(&self) -> u32 {
        // 第一趟：拿 map 锁只做一次浅拷贝就放手，之后逐个锁会话——跟
        // `list()` 一字不差的顺序。反过来（攥着 map 锁去锁会话）会让整个
        // 看板卡在某个正在做慢活的会话上。
        let snapshot: Vec<Arc<Mutex<Session>>> =
            recover(self.sessions.lock()).values().cloned().collect();
        let dead: Vec<u32> = snapshot
            .iter()
            .filter_map(|arc| {
                let s = recover(arc.lock());
                (s.state == SessionState::Stopped).then_some(s.id)
            })
            .collect();

        // 第二趟：只做 HashMap 删除，不碰任何会话锁。
        // 用 `remove().is_some()` 数，不用 `dead.len()`：两趟之间没有锁，
        // 中途可能有别人删了同一个 id，报一个虚高的数字等于骗用户。
        let mut map = recover(self.sessions.lock());
        dead.iter().filter(|id| map.remove(id).is_some()).count() as u32
    }

    /// 恢复到最后一张快照。git 操作同样不持会话锁，理由见 `send_input`。
    pub fn undo(&self, id: u32) -> Result<()> {
        let (dir, sha) = self.checkpoint_base(id)?;
        // 失败时给中文上下文，理由同 create() 里那处——见那边的注释。
        git::restore(&dir, &sha).map_err(|_| coded(ErrorCode::OperationFailed(Operation::Undo)))
    }

    /// 相对最后一张快照改了哪些文件。git 操作不持会话锁。
    pub fn diff(&self, id: u32) -> Result<Vec<FileStat>> {
        let (dir, base) = self.checkpoint_base(id)?;
        // 失败时给中文上下文，理由同 create() 里那处——见那边的注释。
        git::diff_stat(&dir, &base).map_err(|_| coded(ErrorCode::OperationFailed(Operation::Diff)))
    }

    /// 取出做 git 操作需要的信息后立刻放锁。
    fn checkpoint_base(&self, id: u32) -> Result<(PathBuf, String)> {
        let arc = self.get_arc(id)?;
        let s = recover(arc.lock());
        if !s.is_agent {
            return Err(coded(ErrorCode::NotAnAgentSession));
        }
        let sha = s
            .checkpoints
            .last()
            .cloned()
            .ok_or_else(|| coded(ErrorCode::NoCheckpoint))?;
        Ok((s.dir.clone(), sha))
    }

    /// 扫一遍所有会话，更新状态。由守护进程定时调用。
    ///
    /// 判定本身在 [`classify`]，不在这里：那是一个「一屏文字 → 状态」的
    /// 纯函数，能拿真实截屏直接测，不用先支一个活着的 pty。
    pub fn tick(&self) {
        let snapshot: Vec<Arc<Mutex<Session>>> =
            recover(self.sessions.lock()).values().cloned().collect();

        for s in snapshot {
            let mut s = recover(s.lock());
            if s.state == SessionState::Stopped {
                continue;
            }
            if !s.pty.is_alive() {
                // **这一轮是回收子进程的最后机会。** 下一轮 tick 会在上面那个
                // `Stopped` 分支直接跳过它，`Session` 又一直留在 map 里、`Drop`
                // 不会跑——错过这里就再也没人管了。
                //
                // 自己退出的 agent（`/exit`、崩溃、shell 里 `exit`）没有任何
                // 一处 wait 过它：读线程读到 EOF 只是置了个 `alive` 标志
                // （见 `pty.rs` 里那段），而 `is_alive()` 一看标志就短路返回，
                // 里面的 `try_wait()` 根本走不到。于是子进程变成僵尸，一直挂到
                // 守护进程重启——而守护进程一活就是好几天，这正是它存在的理由。
                // 按 `s` 停止那条路没这个问题，`stop()` 走的是 `pty.kill()`。
                //
                // 用 `kill()` 而不是补一次 `try_wait()`：还有一种情况是子进程
                // 关掉了 PTY 却还活着，那时 `try_wait` 回收不到任何东西，而
                // 这个会话已经被判成停止、不会再被看第二眼了。`kill()` 先杀
                // 再等，两种情况一起收干净。
                let pid = s.pty.process_id();
                let _ = s.pty.kill();
                s.state = SessionState::Stopped;
                self.journal
                    .died(s.id, crate::journal::Death::Vanished, pid);
                self.maybe_notify(&mut s, EventKind::Vanished);
                continue;
            }
            if s.state == SessionState::Asking {
                continue;
            }
            // busy 优先：agent 干活时的「按 esc 中断」提示是稳定的，
            // 而空闲时的输入框占位符用户一打字就没了。
            // screen_text() 只取一次，三个分支共用——它要扫一遍整屏文字，
            // 每个会话每秒被 tick 5 次，没必要算三遍。
            if s.busy_re.is_some() || s.idle_re.is_some() || s.error_re.is_some() {
                let text = s.pty.screen_text();
                let next = classify(
                    &text,
                    s.error_re.as_ref(),
                    s.busy_re.as_ref(),
                    s.idle_re.as_ref(),
                );
                if let Some(next) = next {
                    let was = s.state;
                    s.state = next;
                    // 只在**进入** Failed 的那一刻问一次。条件写成「原来不是
                    // Failed」而不是「现在是 Failed」——后者会每 200ms 打一次
                    // 模型，一个失败会话能把额度烧光。
                    if next == SessionState::Failed && was != SessionState::Failed {
                        self.request_explanation(&mut s);
                        self.maybe_notify(&mut s, EventKind::Failed);
                    }
                    // 起名的时机是「干完一轮 **且用户已经说过话**」，两个条件
                    // 缺一不可。不在第一句输入送出去时起：那一刻信息最少，
                    // 正是「继续」「帮我看看」出现的地方；干完一轮之后屏幕上
                    // 才有它到底在做什么的实证。
                    //
                    // `!s.first_input.is_empty()` 这一半不是锦上添花，是必需的：
                    // 真实 profile（claude/codex/glm/kimi/deepseek/qwen-api）
                    // 全都只声明 `busy_pattern`，不声明 `idle_pattern`——`classify()`
                    // 在 busy_pattern 存在时，busy 串**不在**屏幕上就判 Idle，
                    // 而刚创建、还停在启动画面上的会话正是这样。没有这道判断，
                    // `create()` 之后的第一个 tick 就会把 `was == Working`（创建时
                    // 因为有 pattern 而置的初始状态）→ `next == Idle`（启动画面）
                    // 读成「干完一轮活」，用空的 `first_input` 把名字永久钉成空串。
                    // 注意这里特意用 `first_input`、不用 `first_input_sealed`：
                    // 用户粘一大段需求、还没敲回车封存，agent 却已经抢先干完一轮
                    // 活，这种情况下也该拿这段还没封存的话去起名，不该因为没封存
                    // 就被这道判断拦下。
                    //
                    // 「只起一次」的门槛是 `name_attempted`，**不是**
                    // `name_slot.is_none()`：`fallback_name` 允许兜底本身
                    // 就是 `None`（第一句话洗完/去空白之后什么都不剩），
                    // 如果拿 `name_slot` 当门槛，这种会话会在每一轮
                    // Working → Idle 都被重新读成「还没起过」，白白多打
                    // 一次模型，还可能让两个后台起名线程同时在飞——后
                    // 触发的那次会把先完成的线程刚写进去的真名字同步
                    // 覆盖回 `None`，一次丢失更新（细节见 `name_attempted`
                    // 自己的文档）。
                    if was == SessionState::Working
                        && matches!(next, SessionState::Idle | SessionState::Asking)
                    {
                        // 起名只问一次（`name_attempted` 那道门），但「停下来了」
                        // 这件事该发生几次就通知几次——一个 agent 干完第二轮活
                        // 一样值得让手机响一次，不能因为名字早就起过了就把这次
                        // 也拦下。所以通知跟起名分成两个独立的 if，共用同一个
                        // `was`/`next` 判断，但各自的门槛互不影响。
                        if s.is_agent && !s.first_input.is_empty() && !s.name_attempted {
                            self.request_name(&mut s);
                        }
                        self.maybe_notify(&mut s, EventKind::Stopped);
                    }
                }
            }
            // 两个都没有：状态不动，保持 Unknown
        }
    }

    /// 三道门 + 防抖，`Stopped`/`Failed`/`Vanished` 共用的唯一出口。
    ///
    /// **绝不阻塞。** `event_tx` 是 unbounded 的 `mpsc::Sender`——`send()`
    /// 只会让底层队列变长，不会等任何人来收；唯一可能的失败是接收端已经
    /// 掉了（没配手机通知，或者 bridge 那边的消费线程还没起来），这时候
    /// 安静丢掉，不是 tick() 该操心的事。有界、drop-oldest 那一半在
    /// `bridge.rs` 的消费端，见那边 `QUEUE_CAP` 的文档。
    fn maybe_notify(&self, s: &mut Session, kind: EventKind) {
        let tx = recover(self.event_tx.lock()).clone();
        if !should_notify(s.is_agent, s.first_input.is_empty(), tx.is_some()) {
            return;
        }
        let tx = tx.expect("should_notify 的第三道门刚判过 has_channel 为真");

        let now = self.started.elapsed();
        if !debounce(s.last_notified, now, DEBOUNCE_WINDOW) {
            return;
        }
        s.last_notified = Some(now);

        let name = recover(s.name_slot.lock()).clone().unwrap_or_default();
        // **隐私边界的回归修复。** `file_name()` 在根目录（`/`）或者以
        // `..` 结尾的路径上返回 `None`——这两种都是边缘情况，但退化成
        // `s.dir.display()` 会把整条本地文件系统路径原样送到手机上，
        // 正是 CLAUDE.md 那条「手机上的字绝不能带路径」要挡住的东西。
        // 这里不编一个假名字，也不泄露路径，退回一个诚实的占位词——
        // 跟 `fallback_name`/`event_label` 同一条「宁可平淡也不能露底」
        // 的规矩。
        let project = s
            .dir
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未命名项目".to_string());
        // 发不出去（接收端掉了）就丢：跟没配手机通知没有区别，tick() 不该
        // 因为这件事而报错或者重试。
        let _ = tx.send(Event {
            session: s.id,
            kind,
            name,
            project,
        });
    }

    /// **绝不在 tick 里同步等模型。** tick 每 200ms 一轮，一次同步调用就能
    /// 让整个守护进程卡住，而卡住的 dct 和死掉的 agent 长得一模一样。
    fn request_explanation(&self, s: &mut Session) {
        // 先清空、先占号，**在起线程之前**、也不管有没有后端：这一刻起
        // 「上一次失败」的解释就不再是关于*这次*失败的了，界面不该继续
        // 顶着一句过期的话，直到（如果有的话）新答案自己写进来。
        *recover(s.explanation_slot.lock()) = None;
        let my_gen = s.explanation_gen.fetch_add(1, Ordering::SeqCst) + 1;

        let Some(b) = recover(self.backend.lock()).clone() else {
            return; // 没配后端：功能安静下线，会话照跑
        };
        let p = explain_prompt(&s.pty.screen_text());
        let slot = s.explanation_slot.clone(); // Arc<Mutex<Option<String>>>
        let gen = s.explanation_gen.clone();
        std::thread::spawn(move || {
            if let Ok(text) =
                crate::llm::complete_with_timeout(b, p, std::time::Duration::from_secs(30))
            {
                // 只有这次问的还是「最新一次失败」才写回——一个卡了很久的
                // 旧线程，如果在更新的一轮已经问过之后才答完，这份迟到的
                // 旧答案就不写了，免得把新答案盖成旧的。
                if gen.load(Ordering::SeqCst) == my_gen {
                    if let Ok(mut g) = slot.lock() {
                        *g = Some(text);
                    }
                }
            }
            // 失败就什么都不做——界面显示今天就有的那句失败提示
        });
    }

    /// 给这个会话起个名字。**只在它第一次干完活时调用一次**——门槛是
    /// `Session::name_attempted`，`tick()` 在调用这里之前已经检查过
    /// （不是 `name_slot` 是否为 `None`：`fallback_name` 允许兜底本身
    /// 就是 `None`，拿 `name_slot` 当门槛会让这类会话每一轮都被重新
    /// 触发，细节见 `name_attempted` 的文档）。
    ///
    /// 跟 `request_explanation` 是同一条路，但**不需要 generation 计数器**：
    /// 失败会反复发生、迟到的旧解释会盖掉新解释，而这里的门槛在函数
    /// 一进门就同步立起来（见下面第一行），全程只有一个线程可能写
    /// `name_slot`，没有「迟到的旧答案盖掉新答案」这种事要防。
    fn request_name(&self, s: &mut Session) {
        // 先立门槛，**在做任何别的事之前**：不管兜底洗不洗得出东西、
        // 不管有没有配后端，「这个会话已经问过一次名字」从这一刻起就是
        // 定局，`tick()` 不会再为它调用这个函数第二次。放在最前面是为
        // 了不留窗口——换成先写兜底、后立门槛，两步之间那一刻门槛还
        // 没立起来。
        s.name_attempted = true;

        // 再把兜底同步写进去：模型答得出就覆盖，答不出就把第一句留在
        // 这儿。`fallback_name` 可能给 `None`（洗完/去空白之后什么都
        // 不剩），这种情况下 `name_slot` 就该留成 `None`，不能钉死一个
        // 看不见的空 tag——见它自己的文档。
        *recover(s.name_slot.lock()) = fallback_name(&s.first_input);

        let Some(b) = recover(self.backend.lock()).clone() else {
            return; // 没配后端：功能安静下线，兜底那句留着
        };
        let p = name_prompt(&s.first_input, &s.pty.screen_text());
        let slot = s.name_slot.clone();
        std::thread::spawn(move || {
            // 15 秒，比 `explanation` 的 30 秒短：那个是用户正等着看解释，
            // 这个是后台起名，没人等，等太久只是白占一个线程。
            if let Ok(text) =
                crate::llm::complete_with_timeout(b, p, std::time::Duration::from_secs(15))
            {
                if let Some(name) = model_name(&text) {
                    if let Ok(mut g) = slot.lock() {
                        *g = Some(name);
                    }
                }
            }
            // 失败就什么都不做——兜底那句已经在槽里了
        });
    }
}

fn state_of(v: crate::pty::ScrollView, mark: usize) -> ScrollState {
    ScrollState {
        agent_owns: v.agent_owns,
        alt_screen: v.alt_screen,
        max: v.max,
        offset: v.offset,
        new_lines: v.offset.saturating_sub(mark),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::SecretStore;
    use std::fs;
    use std::process::Command;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// 一台真机上 Claude Code **停在提示符等人** 时的屏幕底部，照抄。
    ///
    /// 关键在最后一行：用 `--dangerously-skip-permissions` 起的 Claude Code
    /// 底栏常驻「bypass permissions on」，把 `? for shortcuts` 顶掉了。
    const CLAUDE_WAITING_FOR_YOU: &str = "\
● 362 个测试全绿，clippy 干净，已提交并重装到 ~/.local/bin/dct。

  用起来再有别扭的地方告诉我。

✳ Brewed for 9m 52s
                                        new task? /clear to save 612.1k tokens
❯
⚠ Transcript saving is off — inherited CLAUDE_CODE_CHILD_SESSION marker
~/work/dc/dc-terminal  main | \"实现项目选择的目录浏览器\" | Opus 5 | ctx:61%
▶▶ bypass permissions on (shift+tab to cycle)
";

    /// 同一台机器上，同一个 agent **正在干活**。
    const CLAUDE_WORKING: &str = "\
● 我来查一下这个。

✳ Brewing… (5s · ↓ 1.2k tokens · esc to interrupt)
❯
▶▶ bypass permissions on (shift+tab to cycle)
";

    /// claude 系的 profile 全都带 `--dangerously-skip-permissions` 起 agent，
    /// 于是它们用自己的启动参数保证了自己的 `idle_pattern` 永远不出现：
    /// 会话明明停在提示符上等人，格子标题却一直写着「干活中」。
    ///
    /// 这条测试钉的是结论，不是某一条正则：不管 profile 用什么 pattern，
    /// 「等人的屏幕」不许判成 Working。
    #[test]
    fn a_claude_family_session_waiting_at_the_prompt_is_idle() {
        for name in ["claude", "deepseek", "glm", "kimi", "qwen-api"] {
            let p = crate::profile::Profile::builtin(name).unwrap();
            let state = classify(
                CLAUDE_WAITING_FOR_YOU,
                p.error_regex().unwrap().as_ref(),
                p.busy_regex().unwrap().as_ref(),
                p.idle_regex().unwrap().as_ref(),
            );
            assert_eq!(
                state,
                Some(SessionState::Idle),
                "{name}：停在提示符等人的屏幕被判成了 {state:?}"
            );
        }
    }

    /// 上一条的守门人。少了它，把 pattern 全删光也能让上一条变绿——
    /// 那是把「永远说干活中」换成「永远说空闲」，一样错。
    #[test]
    fn a_claude_family_session_mid_turn_is_working() {
        for name in ["claude", "deepseek", "glm", "kimi", "qwen-api"] {
            let p = crate::profile::Profile::builtin(name).unwrap();
            let state = classify(
                CLAUDE_WORKING,
                p.error_regex().unwrap().as_ref(),
                p.busy_regex().unwrap().as_ref(),
                p.idle_regex().unwrap().as_ref(),
            );
            assert_eq!(
                state,
                Some(SessionState::Working),
                "{name}：正在干活的屏幕被判成了 {state:?}"
            );
        }
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(p)
                .output()
                .unwrap();
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        fs::write(p.join("a.txt"), "hello\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }

    /// 大多数测试不关心密钥，只是要满足 `create()` 新增的形参。
    fn empty_secrets() -> Option<&'static str> {
        None
    }

    /// 测试专用：查一个会话此刻的状态。跟 `screen_text_for_test` 一样，
    /// 省得每条断言都重新拼一遍 `list().iter().find(...)`。
    fn state_of(mgr: &SessionManager, id: u32) -> SessionState {
        mgr.list().into_iter().find(|s| s.id == id).unwrap().state
    }

    // 用 cat 冒充 agent：能收输入、不会自己退出
    fn fake_agent() -> Profile {
        Profile {
            name: "fake".into(),
            command: vec!["cat".into()],
            is_agent: true,
            idle_pattern: Some("READY".into()),
            busy_pattern: None,
            error_pattern: None,
            env: Default::default(),
            secret: None,
            install: None,
            headless: None,
            api: None,
            label: Default::default(),
            note: Default::default(),
        }
    }

    // 冒充一个会报错的 agent：跟 fake_agent 一样是常驻进程（先 echo BOOM 再
    // sleep），不然一次性输出完就退出，state 会被 tick() 判成 Stopped，
    // 抢在 Failed 前面。
    fn failing_agent() -> Profile {
        Profile {
            name: "failing".into(),
            command: vec!["/bin/sh".into(), "-c".into(), "echo BOOM; sleep 5".into()],
            is_agent: true,
            idle_pattern: None,
            busy_pattern: None,
            error_pattern: Some("BOOM".into()),
            env: Default::default(),
            secret: None,
            install: None,
            headless: None,
            api: None,
            label: Default::default(),
            note: Default::default(),
        }
    }

    // `fake_agent`（cat）的 idle_pattern 靠**回显用户敲的字**去命中——那对
    // `tick_marks_idle_when_pattern_matches` 这种直接打 "READY" 的测试没问题，
    // 但起名测试要送的是真实的第一句话（"修一下登录白屏"），screen 上永远不会
    // 出现 "READY"，Working → Idle 这一跳就永远不会发生，`request_name` 也就
    // 永远不会被触发到。所以起名测试需要一个不看输入内容、自己按时间线走到
    // Idle 的假 agent：开局先是 Working（`send_input` 直接置位），过一小会儿
    // 自己吐 "READY"，跟用户敲了什么无关。
    fn finishing_agent() -> Profile {
        Profile {
            name: "finishing".into(),
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                "sleep 0.2; echo READY; sleep 30".into(),
            ],
            is_agent: true,
            idle_pattern: Some("READY".into()),
            busy_pattern: None,
            error_pattern: None,
            env: Default::default(),
            secret: None,
            install: None,
            headless: None,
            api: None,
            label: Default::default(),
            note: Default::default(),
        }
    }

    // 真实 profile 的形状：claude/codex/glm/kimi/deepseek/qwen-api 全都只
    // 声明 `busy_pattern`（比如「esc to interrupt」），不声明 `idle_pattern`——
    // `profiles/*.toml` 里 `idle_pattern` 这个词只出现在解释「为什么故意不写」
    // 的注释里。`classify()` 在 busy_pattern 存在时，busy 串**不在**屏幕上
    // 就判 Idle：刚创建、还停在启动画面上的会话，第一个 tick 就是这个读法。
    fn busy_only_agent() -> Profile {
        Profile {
            name: "busy-only".into(),
            command: vec!["cat".into()],
            is_agent: true,
            idle_pattern: None,
            busy_pattern: Some("esc to interrupt".into()),
            error_pattern: None,
            env: Default::default(),
            secret: None,
            install: None,
            headless: None,
            api: None,
            label: Default::default(),
            note: Default::default(),
        }
    }

    #[test]
    fn the_explain_prompt_carries_the_tail_of_the_screen() {
        let long = "x".repeat(5000) + "API Error: Connection closed mid-response.";
        let p = explain_prompt(&long);
        assert!(p.user.contains("API Error"), "错误在末尾，必须送到");
        assert!(p.user.chars().count() < 2500, "整屏太长，要截尾");
        assert!(p.system.contains("中文"), "用户默认中文");
    }

    #[test]
    fn the_explain_prompt_asks_for_plain_language() {
        let p = explain_prompt("API Error: Connection closed mid-response.");
        // 目标用户零编程经验：不要栈追踪、不要术语。
        assert!(p.system.contains("不要"), "要明确禁止术语/栈追踪");
        assert!(p.max_tokens <= 200, "一句话就够，别让它写小作文");
    }

    /// 逐键送和整段送必须封存出同一句话 —— 会话视图是一个键一次
    /// `Input`，九宫格 `i` 是整段 + 一次空 `Input`。
    #[test]
    fn both_input_paths_seal_the_same_first_line() {
        let mut a = (String::new(), false);
        for k in ["h", "i", "\r"] {
            collect_first_input(&mut a.0, &mut a.1, k);
        }

        let mut b = (String::new(), false);
        collect_first_input(&mut b.0, &mut b.1, "hi");
        collect_first_input(&mut b.0, &mut b.1, "");

        assert_eq!(a.0, "hi");
        assert_eq!(b.0, "hi");
        assert!(a.1 && b.1, "两条路都要封存");
    }

    /// 封存之后再送字，第一句不再变 —— 它是「第一句」，不是「最近一句」。
    #[test]
    fn sealed_first_input_never_changes_again() {
        let mut buf = String::new();
        let mut sealed = false;
        collect_first_input(&mut buf, &mut sealed, "hi");
        collect_first_input(&mut buf, &mut sealed, "");
        collect_first_input(&mut buf, &mut sealed, "and more");
        assert_eq!(buf, "hi");
    }

    /// 粘一大段需求进来：只留前 200 个字符，剩下的不进内存。
    #[test]
    fn a_pasted_wall_of_text_is_capped() {
        let mut buf = String::new();
        let mut sealed = false;
        collect_first_input(&mut buf, &mut sealed, &"x".repeat(300));
        assert_eq!(buf.chars().count(), FIRST_INPUT_MAX);
        assert!(!sealed, "没按回车就不算封存");
    }

    /// 一次送进来的字里就带着回车（粘贴多行）：回车之前的算第一句，
    /// 回车本身封存。
    #[test]
    fn a_newline_inside_one_chunk_seals_at_the_newline() {
        let mut buf = String::new();
        let mut sealed = false;
        collect_first_input(&mut buf, &mut sealed, "fix login\nand also");
        assert_eq!(buf, "fix login");
        assert!(sealed);
    }

    /// 粘贴的中文句子后面跟一个换行：`find` 拿到的是字节下标，多字节字符的
    /// 字节永远不会跟 ASCII 的 `\n` 撞在一起，切在这里不会崩在字符中间。
    #[test]
    fn a_multibyte_utf8_sentence_before_the_newline_does_not_panic() {
        let mut buf = String::new();
        let mut sealed = false;
        collect_first_input(&mut buf, &mut sealed, "修复登录问题\n还有别的");
        assert_eq!(buf, "修复登录问题");
        assert!(sealed);
    }

    /// 附着视图是逐键转发（`ui/attach.rs` 每按一次键就一次 `send_input`），
    /// 真实 Backspace 键发的字节是 `\x7f`，不是删除键。改错字产出的字节流
    /// 是「打错的字 + 退格 + 改对的字」，记下来的 `first_input` 必须是用户
    /// 最终想说的那句话，不是这串字节的字面重放——这是 fix-1-brief 明确
    /// 要求的退格语义，也是本条修复要过的第一道测试。
    #[test]
    fn collect_first_input_applies_backspace_as_the_user_intended() {
        let mut buf = String::new();
        let mut sealed = false;
        collect_first_input(&mut buf, &mut sealed, "fix teh\x7f\x7f\x7fthe");
        assert_eq!(buf, "fix the");
    }

    /// 逐键转发场景下，退格弹的是**上一次调用**追加进 `buf` 的字符，
    /// 不是同一次调用里的字符——附着视图一个键一次 `send_input`，退格
    /// 键单独送过来的时候，`buf` 里除了它自己什么都没有，弹出必须能
    /// 够到 `buf` 本身，跨调用生效。
    #[test]
    fn collect_first_input_backspace_reaches_across_calls() {
        let mut buf = String::new();
        let mut sealed = false;
        for k in [
            "f", "i", "x", " ", "t", "e", "h", "\x7f", "\x7f", "\x7f", "t", "h", "e",
        ] {
            collect_first_input(&mut buf, &mut sealed, k);
        }
        assert_eq!(buf, "fix the");
    }

    /// README 表里那几种附着视图会原样转发的转义序列——上下左右、Esc、
    /// Ctrl+字母：它们是控制信号，不是文字，一个都不该进 `first_input`。
    /// 这条直接照抄 fix-1-brief 的按键表，覆盖「打字前先按了上箭头调
    /// 历史」这种真实会发生的场景。
    #[test]
    fn collect_first_input_drops_escape_sequences_and_control_codes() {
        let mut buf = String::new();
        let mut sealed = false;
        for k in ["\x1b[A", "\x1b[D", "\x1b", "\x01", "\x1a", "hi"] {
            collect_first_input(&mut buf, &mut sealed, k);
        }
        assert_eq!(buf, "hi");
    }

    /// 第一个键就是退格：没有上一个字符可弹，不能 panic，也不能把
    /// 后面正常敲的字弄丢。
    #[test]
    fn collect_first_input_backspace_on_an_empty_buffer_does_nothing_bad() {
        let mut buf = String::new();
        let mut sealed = false;
        collect_first_input(&mut buf, &mut sealed, "\x7f\x7fhi");
        assert_eq!(buf, "hi");
    }

    /// 模型多半会回一句带标点、带引号的话，不会老老实实只给名字。
    /// 洗不干净的话，格子标题上会出现「「修登录白屏」。」这种东西。
    #[test]
    fn clean_name_strips_quotes_punctuation_and_extra_lines() {
        assert_eq!(clean_name("「修登录白屏」。"), "修登录白屏");
        assert_eq!(clean_name("\"fix login blank\""), "fix login blank");
        assert_eq!(clean_name("修登录白屏\n（这个会话在修登录）"), "修登录白屏");
        assert_eq!(clean_name("  修登录白屏  "), "修登录白屏");
    }

    /// 洗完是空的就当模型没答上来，调用方走兜底。
    #[test]
    fn clean_name_returns_empty_when_there_is_nothing_left() {
        assert_eq!(clean_name(""), "");
        assert_eq!(clean_name("   \n  "), "");
        assert_eq!(clean_name("。。。"), "");
    }

    /// 模型不听话给了一长串：按字符数封顶，别让它撑爆标题。
    #[test]
    fn clean_name_caps_a_runaway_answer() {
        let long = "修".repeat(100);
        assert_eq!(clean_name(&long).chars().count(), NAME_MAX_CHARS);
    }

    /// 引号叠标点（引号包住整句、句末再补句号）必须在同一次 trim 里一起
    /// 剥掉，分两轮剥（先剥引号、再剥标点）会在两者交替出现时半途而废，
    /// 剥出「修登录白屏」这种带着里层引号的残留。这是 Step 3 最初实现的
    /// 真实 bug，被这条断言直接抓住，所以单独钉一下这个场景。
    #[test]
    fn clean_name_strips_a_quote_stacked_with_trailing_punctuation() {
        assert_eq!(clean_name("「修登录白屏」。"), "修登录白屏");
    }

    /// 名字中间原本就带引号（不在首尾）：trim 只从两端往里剥，中间的
    /// 引号不是「多余包装」，不该被当成噪音铲掉。
    #[test]
    fn clean_name_keeps_a_quote_that_sits_in_the_middle() {
        assert_eq!(clean_name("修复 \"login\" 白屏"), "修复 \"login\" 白屏");
    }

    /// 开头的标点不是包装、是名字的一部分：这个工具的域里 `.NET`、`.env`
    /// 这种以句点开头的名字说得通，掐头去尾的剥法不能把它们当噪音铲掉。
    /// 这是把结尾用的字符集错误地套到开头去会踩中的回归。
    #[test]
    fn clean_name_keeps_a_leading_tail_punctuation_character() {
        assert_eq!(clean_name(".NET 迁移"), ".NET 迁移");
        assert_eq!(clean_name(".env 权限"), ".env 权限");
    }

    /// 恰好等于上限字符数的名字必须原样保留，不多不少——`chars().take(n)`
    /// 按字符切，不按字节，不会在多字节字符中间切出半个字来，也不会因为
    /// “大于等于”之类的边界写错而多切/少切一个字符。
    #[test]
    fn clean_name_keeps_a_name_exactly_at_the_cap_intact() {
        let exact = "修".repeat(NAME_MAX_CHARS);
        let cleaned = clean_name(&exact);
        assert_eq!(cleaned, exact);
        assert_eq!(cleaned.chars().count(), NAME_MAX_CHARS);
    }

    /// `sanitize` 是 `request_name` 两处写入共用的最后一道过滤——README
    /// 表里那几种真实会被转发的转义序列，一个都不能留下来。这条覆盖
    /// 「非退格」的控制字符：方向键、Esc、Ctrl+字母。
    #[test]
    fn sanitize_strips_escape_sequences_and_ctrl_codes() {
        assert_eq!(sanitize("\x1b[A"), "");
        assert_eq!(sanitize("\x1b[D"), "");
        assert_eq!(sanitize("\x1b"), "");
        assert_eq!(sanitize("\x01"), ""); // Ctrl+a
        assert_eq!(sanitize("\x1a"), ""); // Ctrl+z
    }

    /// 退格（`\x7f`/`\x08`）按「弹出上一个字符」处理，不是简单丢弃——
    /// 这是 fix-1-brief 采纳的读法，记下来的文本要跟用户真正想打的话
    /// 一致。
    #[test]
    fn sanitize_pops_the_previous_character_on_backspace() {
        assert_eq!(sanitize("fix teh\x7f\x7f\x7fthe"), "fix the");
        assert_eq!(sanitize("a\x08b"), "b"); // 老式退格同样按弹出处理
    }

    /// 第一个字符就是退格：没有上一个字符可弹，不能 panic，后面的正常
    /// 字符照常保留。
    #[test]
    fn sanitize_backspace_on_an_empty_buffer_does_nothing() {
        assert_eq!(sanitize("\x7fhi"), "hi");
    }

    /// 没有控制字符的普通文本（含中文）原样穿过，`sanitize` 不该动它。
    #[test]
    fn sanitize_keeps_ordinary_text_untouched() {
        assert_eq!(sanitize("fix login bug"), "fix login bug");
        assert_eq!(sanitize("修复登录问题"), "修复登录问题");
    }

    /// `fallback_name` 是 `name_slot` 落地前的最后一道关卡，**不能假设
    /// `first_input` 已经干净**——正常路径下它确实已经被 `append_capped`
    /// 洗过一轮，但这条测试故意绕开那条路径，直接构造一个「万一没洗
    /// 干净」的输入，钉死这道关卡自己也会挡，不是单纯依赖上游。这也是
    /// 唯一能把 `request_name` 里 `sanitize(&fallback)` 这一步单独测出来
    /// 的地方：走完整的 `SessionManager` 全流程时，`first_input` 在到
    /// 这里之前早就是干净的，那条调用会被测试悄悄放过。
    #[test]
    fn fallback_name_strips_control_bytes_even_if_first_input_somehow_carries_them() {
        assert_eq!(
            fallback_name("fix\x1b[A the bug"),
            Some("fix the bug".to_string())
        );
    }

    /// 只有空白：洗完/去空白之后什么都不剩，必须是 `None`，不是
    /// `Some("")`——两者在 `list()` 里看起来一样（都读成空串），但只有
    /// `None` 才能让 `request_name` 的「只起一次」门槛重新打开。
    #[test]
    fn fallback_name_is_none_for_whitespace_only_input() {
        assert_eq!(fallback_name(" "), None);
        assert_eq!(fallback_name("   "), None);
        assert_eq!(fallback_name(""), None);
    }

    /// 普通情况：截到上限、去掉首尾空白。
    #[test]
    fn fallback_name_caps_and_trims() {
        assert_eq!(fallback_name("  hi  "), Some("hi".to_string()));
        let long = "x".repeat(NAME_MAX_CHARS + 10);
        assert_eq!(
            fallback_name(&long).unwrap().chars().count(),
            NAME_MAX_CHARS
        );
    }

    /// `model_name` 要把 `clean_name`（剥引号/标点）和 `sanitize`（洗
    /// 控制字符/转义序列）串起来——`clean_name` 自己不管控制字符（见它
    /// 的文档），漏了这一步，被操纵过的屏幕内容诱导模型吐回来的控制
    /// 字符就会原样进 `name_slot`。
    #[test]
    fn model_name_strips_control_bytes_after_clean_name() {
        assert_eq!(
            model_name("「修\x1b登录\x7f白屏」。"),
            Some("修登白屏".to_string())
        );
    }

    /// 模型答案洗完是空的（比如整句就是标点和控制字符）：`None`，不是
    /// 一个看不见的空字符串。
    #[test]
    fn model_name_is_none_when_nothing_survives_the_wash() {
        assert_eq!(model_name("。。。"), None);
        assert_eq!(model_name("\x1b\x01"), None);
    }

    /// prompt 必须带上第一句输入和屏幕末尾两样，缺一样模型就只能猜。
    #[test]
    fn name_prompt_carries_both_the_first_line_and_the_screen() {
        let p = name_prompt("修一下登录白屏", "…… 正在改 auth.ts ……");
        assert!(p.user.contains("修一下登录白屏"));
        assert!(p.user.contains("auth.ts"));
        assert!(p.max_tokens <= 64, "起个名字不需要长回答");
    }

    struct FixedBackend(String);
    impl crate::llm::Backend for FixedBackend {
        fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
            Ok(self.0.clone())
        }
    }

    struct DeadBackend;
    impl crate::llm::Backend for DeadBackend {
        fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
            Err(crate::llm::LlmError::Unavailable)
        }
    }

    /// 跟 `FixedBackend` 一样答得出名字，多做一件事：`complete()` 被调用
    /// 的那一刻往 channel 里发一个信号。`complete_with_timeout`
    /// （`llm/mod.rs`）会在**它自己另开的一个线程**里同步调用
    /// `Backend::complete`，所以收到这个信号就是「`request_name` 真的
    /// 走到了模型这一步」唯一站得住脚的证据——不是靠猜多久之后 `tag`
    /// 应该变了没变。
    struct SignalingBackend {
        name: String,
        called: std::sync::mpsc::Sender<()>,
    }
    impl crate::llm::Backend for SignalingBackend {
        fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
            let _ = self.called.send(());
            Ok(self.name.clone())
        }
    }

    /// 起名的正路：干完一轮活，名字就出来了。
    #[test]
    fn a_session_gets_named_after_its_first_round_of_work() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(finishing_agent());
        m.set_backend(Some(Arc::new(FixedBackend("「修登录白屏」。".into()))));
        let id = m
            .create(repo.path(), "finishing", empty_secrets(), &[])
            .unwrap();

        m.send_input(id, "修一下登录白屏").unwrap();
        m.send_input(id, "").unwrap(); // 空字符串 = 回车，状态进 Working

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            let tag = m.list().iter().find(|s| s.id == id).unwrap().tag.clone();
            if tag == "修登录白屏" {
                break;
            }
            assert!(Instant::now() < deadline, "一直没起出名字，最后是 {tag:?}");
            sleep(Duration::from_millis(50));
        }
    }

    /// **钉死**：再干一轮，名字不变。这是「只起一次」唯一测得到的地方。
    #[test]
    fn a_name_is_pinned_and_never_asked_for_twice() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(finishing_agent());
        m.set_backend(Some(Arc::new(FixedBackend("第一个名字".into()))));
        let id = m
            .create(repo.path(), "finishing", empty_secrets(), &[])
            .unwrap();

        m.send_input(id, "干活").unwrap();
        m.send_input(id, "").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            if m.list().iter().find(|s| s.id == id).unwrap().tag == "第一个名字" {
                break;
            }
            assert!(Instant::now() < deadline, "第一次就没起出来");
            sleep(Duration::from_millis(50));
        }

        // 换一个会给别的答案的后端，再走一轮 Working → Idle
        m.set_backend(Some(Arc::new(FixedBackend("第二个名字".into()))));
        m.send_input(id, "再干一轮").unwrap();
        m.send_input(id, "").unwrap();
        for _ in 0..20 {
            m.tick();
            sleep(Duration::from_millis(50));
        }

        assert_eq!(
            m.list().iter().find(|s| s.id == id).unwrap().tag,
            "第一个名字",
            "名字是钉死的，第二轮不该重起"
        );
    }

    /// 模型答不上来（或者压根没配后端）时，名字停在第一句输入上，
    /// 不是空着。
    #[test]
    fn a_dead_model_leaves_the_first_line_as_the_name() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(finishing_agent());
        m.set_backend(Some(Arc::new(DeadBackend)));
        let id = m
            .create(repo.path(), "finishing", empty_secrets(), &[])
            .unwrap();

        m.send_input(id, "修一下登录白屏").unwrap();
        m.send_input(id, "").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            let tag = m.list().iter().find(|s| s.id == id).unwrap().tag.clone();
            if tag == "修一下登录白屏" {
                break;
            }
            assert!(Instant::now() < deadline, "兜底没生效，最后是 {tag:?}");
            sleep(Duration::from_millis(50));
        }
    }

    /// **钉死这个仓库真正会踩的坑**：所有真实 profile（claude/codex/glm/
    /// kimi/deepseek/qwen-api）都只声明 `busy_pattern`，没有一个声明
    /// `idle_pattern`。`classify()` 在只有 busy_pattern 时，busy 串**不在**
    /// 屏幕上就判 Idle——刚创建、还停在启动画面上的会话，第一个 tick
    /// 就是这个读法：`was == Working`（创建时因为有 pattern 而置的初始
    /// 状态）→ `next == Idle`（启动画面，还没人跟它说过话）。没有
    /// `!s.first_input.is_empty()` 这道判断，这一跳会被当成「干完一轮活」，
    /// 用空的 first_input 把名字永久钉成空串。
    #[test]
    fn a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen() {
        // 跟 `recovering_from_a_failure_after_real_input_still_does_not_count`
        // 同一个理由：名字跟着 prompt 里有没有真实的第一句话走，这样才能
        // 把「是哪一次触发定下了这个名字」测出来，不受线程调度影响。
        struct ByPrompt;
        impl crate::llm::Backend for ByPrompt {
            fn complete(&self, p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
                if p.user.contains("修一下登录白屏") {
                    Ok("真实名字".into())
                } else {
                    Ok("启动画面误触发".into())
                }
            }
        }

        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(busy_only_agent());
        m.set_backend(Some(Arc::new(ByPrompt)));
        let id = m
            .create(repo.path(), "busy-only", empty_secrets(), &[])
            .unwrap();

        // 没送过任何输入：`cat` 的屏幕是空的，busy 串自然不在上面，
        // `classify()` 一上来就会把这读成 Idle。多 tick 几轮，确认名字
        // 一直是空的，也没有被偷偷钉死（后面真正干活那一段会把「偷偷
        // 钉死」这件事测出来——钉死了的话，真名字永远出不来）。
        for _ in 0..5 {
            m.tick();
            assert_eq!(
                m.list().iter().find(|s| s.id == id).unwrap().tag,
                "",
                "没人跟它说过话，不该有名字"
            );
            sleep(Duration::from_millis(20));
        }

        // 现在才是真正干活：送真实输入，走一轮 Working → Idle。
        m.send_input(id, "修一下登录白屏").unwrap();
        m.send_input(id, "").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            let tag = m.list().iter().find(|s| s.id == id).unwrap().tag.clone();
            if tag == "真实名字" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "真正干完一轮活之后应该起出真实名字，最后是 {tag:?}"
            );
            sleep(Duration::from_millis(50));
        }
    }

    /// **核心回归测试**（fix-1-brief）：附着视图逐键转发时，用户先按了
    /// 上箭头调历史，再打字、中途用退格改错字。这一串字节混进
    /// `first_input`，最终变成 `SessionInfo.tag`——那正是看板列表项、
    /// 九宫格标题、附着视图块标题渲染时读的字段，走的是 `Line` →
    /// `Span::render_ref` 那条不过滤控制字符的路（`ratatui` 只有
    /// `Buffer::set_stringn`/`Paragraph` 过滤，这几处都不走那两条）。
    /// 这条测试钉死：不管起名最后走的是兜底还是模型，落进 `tag` 的字符串
    /// 绝不含控制字符，从源头掐断这条到用户终端的注入路径，也钉死退格
    /// 的弹出语义——`"fix teh\x7f\x7f\x7fthe"` 最终应该读作 `"fix the"`，
    /// 是用户真正想说的话，不是按键序列的字面重放。
    #[test]
    fn a_tag_born_from_control_bytes_never_carries_them_into_the_render_path() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(finishing_agent());
        m.set_backend(Some(Arc::new(DeadBackend)));
        let id = m
            .create(repo.path(), "finishing", empty_secrets(), &[])
            .unwrap();

        for k in [
            "\x1b[A", "f", "i", "x", " ", "t", "e", "h", "\x7f", "\x7f", "\x7f", "t", "h", "e",
        ] {
            m.send_input(id, k).unwrap();
        }
        m.send_input(id, "").unwrap(); // 回车

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            let tag = m.list().iter().find(|s| s.id == id).unwrap().tag.clone();
            assert!(
                !tag.chars().any(|c| c.is_control()),
                "控制字符漏进了 tag：{tag:?}"
            );
            if tag == "fix the" {
                break;
            }
            assert!(Instant::now() < deadline, "兜底没生效，最后是 {tag:?}");
            sleep(Duration::from_millis(50));
        }
    }

    /// 同一道过滤也得覆盖模型那条路：屏幕内容可能来自仓库或网络，被
    /// 操纵过的屏幕可以诱导模型把控制字符原样吐回来，而 `clean_name`
    /// 只管引号和标点、不管控制字符（见它自己的文档）。这条测试直接让
    /// 模型答案里带上 Esc 和退格，钉死 `sanitize` 在 `request_name` 的
    /// 第二处写入（模型答案）也生效，不只是兜底那一处。
    #[test]
    fn the_model_named_path_is_sanitized_too() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(finishing_agent());
        m.set_backend(Some(Arc::new(FixedBackend("修\x1b登录\x7f白屏".into()))));
        let id = m
            .create(repo.path(), "finishing", empty_secrets(), &[])
            .unwrap();

        m.send_input(id, "修一下登录白屏").unwrap();
        m.send_input(id, "").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            let tag = m.list().iter().find(|s| s.id == id).unwrap().tag.clone();
            if tag == "修登白屏" {
                break;
            }
            assert!(
                !tag.chars().any(|c| c.is_control()),
                "模型答案里的控制字符没被洗掉：{tag:?}"
            );
            assert!(Instant::now() < deadline, "一直没起出名字，最后是 {tag:?}");
            sleep(Duration::from_millis(50));
        }
    }

    /// 只送空白（一个空格加回车）：洗完/去空白之后兜底是空的，`name_slot`
    /// 必须留 `None`，不能钉死一个看不见的空 tag——**但「问过没有」这件
    /// 事本身必须只成立一次**，不能因为 `name_slot` 还是 `None` 就被
    /// `tick()` 读成「还没问过」而反复重新触发。反复触发的代价是真实的：
    /// 每一轮 Working → Idle 都会再打一次模型（跟 `request_explanation`
    /// 的文档里要躲的是同一种坑——一个答不上来的会话能把额度烧光），
    /// 而且会有两个后台起名线程同时在飞——后触发的那次 `request_name`
    /// 会同步把 `name_slot` 写回 `None`，把前一个线程刚写进去的真名字
    /// 覆盖掉，一次丢失更新（`Session::name_attempted` 的文档记的是
    /// 同一件事）。
    ///
    /// 所以这条测试反过来验证：第一轮空白兜底问过之后，就算换一个真的
    /// 答得出名字的后端、再逼出一次 Working → Idle，也不该再起出
    /// 名字——`name_attempted` 得挡住第二次 `request_name`。
    #[test]
    fn whitespace_only_input_is_asked_about_exactly_once_not_forever() {
        let repo = init_repo();
        let m = SessionManager::new();
        // `fake_agent`（cat，只声明 idle_pattern）能按需要反复把 "READY"
        // 打上屏幕，从而反复触发状态判定——`finishing_agent` 的脚本打完
        // 一次 READY 就 `sleep 30` 不再吭声，逼不出第二次 Working → Idle。
        m.register_profile(fake_agent());
        m.set_backend(Some(Arc::new(DeadBackend)));
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();

        // 第一句话只有空白——封存的 first_input 就是 " "。
        m.send_input(id, " ").unwrap();
        m.send_input(id, "").unwrap();

        // 手动把 "READY" 打上屏幕，触发第一次 Working → Idle。
        m.send_input(id, "READY").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            if state_of(&m, id) == SessionState::Idle {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "一直没进入 Idle，没法验证起名逻辑"
            );
            sleep(Duration::from_millis(50));
        }
        // 这一轮的兜底是空白，`request_name` 是在这次 tick 里同步写的
        // （见它自己的文档），所以状态一到 Idle 就已经有答案：`tag`
        // 应该还是看不见的空串。
        assert_eq!(
            m.list().iter().find(|s| s.id == id).unwrap().tag,
            "",
            "空白兜底不该产出一个看不见但非空的 tag"
        );

        // 换一个答得出名字、但会在被真正调用时发信号的后端，再逼一次
        // Working → Idle：`send_input` 的空串分支会无条件把状态同步置回
        // Working（不管屏幕内容），"READY" 还留在屏幕上，下一次 tick 就
        // 会再判一次 Idle。
        let (called_tx, called_rx) = std::sync::mpsc::channel();
        m.set_backend(Some(Arc::new(SignalingBackend {
            name: "真实名字".into(),
            called: called_tx,
        })));
        m.send_input(id, "").unwrap();

        // 触发（如果 `name_attempted` 没挡住的话）第二次 Working → Idle。
        // 这几次 `tick()` 全是同步调用——`request_name` 会不会被**触发**
        // 在 `tick()` 内部就已经决定好了，不需要真实时间流逝，用不着
        // sleep。
        for _ in 0..5 {
            m.tick();
        }

        // 用 channel 而不是「等一会儿再看 tag 变没变」判定：旧版本的这条
        // 测试是十次 `tick()` + 30ms sleep 各自断言 `tag == ""`，负载重的
        // 时候后台线程可能没能在这固定 ~300ms 的预算里落地，测试因为
        // 错误的原因侥幸通过——那正是这一整个分支被打回的病根（真正
        // 「判别力强」的测试要能稳定地红，不能只是偶尔红）。
        //
        // `recv_timeout` 反过来用：`complete()` 一旦真被调用，
        // `SignalingBackend` 会几乎瞬间发信号（纯内存操作，没有网络
        // 延迟），5 秒给的是巨大的余量——如果 bug 真的在，信号早就该到了；
        // 如果 `name_attempted` 生效，信号**永远不会来**，超时本身就是
        // 这条测试要的证据，不是「等的时间不够长」。跟其余测试等一个
        // *会*发生的事件时用的 5 秒 deadline 是同一个量级，这里等的是
        // 「确认它不发生」。
        match called_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(()) => panic!(
                "name_attempted 没有挡住第二次 request_name：\
                 后端的 complete() 被再次调用了"
            ),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(e) => panic!("channel 出了意外：{e:?}"),
        }

        assert_eq!(
            m.list().iter().find(|s| s.id == id).unwrap().tag,
            "",
            "既然没有第二次 request_name，tag 也不该变"
        );
    }

    /// **钉死** `was == SessionState::Working` 这道判断：用户已经说过话
    /// （`first_input` 非空，`!s.first_input.is_empty()` 那道判断已经放行）
    /// 之后，agent 却报错又恢复——五个内置 profile 有 `error_pattern`
    /// （只有 codex.toml 没有），用户说完话之后 agent 报错、错误文案又从
    /// 屏幕上滚走，`classify()` 会把这读成 Idle。这不是「干完一轮活」，
    /// 不该拿它起名，起了就会把一辈子只有一次的 `name_attempted` 提前
    /// 烧掉，真正干完活也翻不了身。
    ///
    /// `!s.first_input.is_empty()` 那一半单独由
    /// `a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen`
    /// 钉住（刚创建、first_input 全程为空的场景）；这条测的是它放行之后
    /// 剩下那一半。曾经还有第三条测试，想在「first_input 全程为空」的
    /// 恢复场景里同时钉两道判断——删掉了：那个场景里两道判断本来就都能
    /// 单独拦住误触发，而测试断言的是最终状态、不是误触发那一刻的槽值，
    /// 结果两道判断分别单独删掉都测不出来，只有两个一起删才勉强测出来
    /// （用 mutation 验证过）。这两个「一起删」需要的分量，现在被这条和
    /// `a_freshly_created_busy_pattern_agent_is_not_named_off_its_splash_screen`
    /// 分别独立覆盖了，留着那第三条纯属重复，删掉不损失任何判别力。
    ///
    /// 区分「误触发」和「真起名」不能靠 `first_input`——两次触发时它都
    /// 非空、内容还一样，天生分不出谁是谁。只能靠 prompt 里屏幕尾巴那
    /// 一段：`cat` 要等脚本走完 BOOM → 清屏 → READY 才会真正开始读、
    /// 回显用户排在队列里的那句话，所以恢复那一刻的屏幕尾巴里还没有它，
    /// 真正干完一轮活之后的屏幕尾巴里才有。
    #[test]
    fn recovering_from_a_failure_after_real_input_still_does_not_count() {
        struct ByScreenTail;
        impl crate::llm::Backend for ByScreenTail {
            fn complete(&self, p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
                // `p.user` 里 first_input 是原样嵌进去的，不管屏幕上有没有
                // 这句话都会出现——不能拿整个 `p.user` 去 `contains`，那样
                // 两次触发永远都命中。真正能分出「误触发」和「真起名」的，
                // 是「屏幕上的最后一段内容：」这个分隔符**之后**那一段。
                let tail = p
                    .user
                    .split("屏幕上的最后一段内容：\n\n")
                    .nth(1)
                    .unwrap_or("");
                if tail.contains("修一下登录白屏") {
                    Ok("真实名字".into())
                } else {
                    Ok("恢复期间误触发".into())
                }
            }
        }

        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(Profile {
            name: "flaky-name-2".into(),
            // 清屏和打 READY 仍然要在同一次 write 里（理由同上一条测试）。
            //
            // 两处 sleep 都是故意留宽的安全边际，不是随手抄的数字：
            // - BOOM 停留 1 秒才清屏：这条测试的第一段轮询要能至少抓到
            //   一次 `Failed` 才有意义——`was` 记的是「上一次 tick() 看到
            //   的状态」，如果轮询碰巧一次都没抓到 `Failed`（比如系统繁忙、
            //   调度抖动让这次 tick() 迟迟排不上号），`was` 就会停在创建
            //   时的初始值 `Working` 上，跳过 `Failed` 直接读到 `Idle`——
            //   这时候即使生产代码完全正确，`was == Working` 也会误判为真，
            //   测试就会因为轮询granularity 不够而假红/假绿，测的不是
            //   生产代码。1 秒相对 20ms 的轮询间隔留了大约 50 倍余量。
            // - READY 上屏和 `cat` 起来之间垫 0.5 秒：`cat` fork/exec 完去
            //   读那句排队的输入，中间的间隔本来全凭系统调度，窄到跟
            //   tick() 轮询的间隔可能落进同一个窗口——那样恢复那一刻的
            //   屏幕说不定已经带着回显了，这条测试用来分辨「误触发」和
            //   「真起名」的判据就被冲没了。
            // 两处都是同一个思路：给状态转换留出一个测得准的窗口，
            // 不靠运气——跟上一条测试解决 split-write 那个 flake 一样。
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                "echo BOOM; sleep 1; printf '\\033[2J\\033[HREADY\\n'; sleep 0.5; cat".into(),
            ],
            is_agent: true,
            idle_pattern: Some("READY".into()),
            busy_pattern: None,
            error_pattern: Some("BOOM".into()),
            env: Default::default(),
            secret: None,
            install: None,
            headless: None,
            api: None,
            label: Default::default(),
            note: Default::default(),
        });
        m.set_backend(Some(Arc::new(ByScreenTail)));
        let id = m
            .create(repo.path(), "flaky-name-2", empty_secrets(), &[])
            .unwrap();

        // 用户先说话：`first_input` 在恢复发生之前就已经封存、非空，
        // `!s.first_input.is_empty()` 那道判断在恢复那一刻已经放行，
        // 剩下单独扛住误判的只有 `was == SessionState::Working`。
        m.send_input(id, "修一下登录白屏").unwrap();
        m.send_input(id, "").unwrap();

        // BOOM → 清屏 + READY 的恢复走完。
        let deadline = Instant::now() + Duration::from_secs(5);
        while state_of(&m, id) != SessionState::Idle {
            m.tick();
            assert!(Instant::now() < deadline, "该从 BOOM 恢复成 Idle");
            sleep(Duration::from_millis(20));
        }
        // 多 tick 几轮，把「本不该发生的误触发」的窗口喂饱，给它足够
        // 机会真的发生并把答案写回槽里。
        for _ in 0..10 {
            m.tick();
            sleep(Duration::from_millis(20));
        }

        // 等 `cat` 真把排队的那句话读出来、回显到屏幕上——只有这之后，
        // 屏幕尾巴里才会出现它，标志着「真正干完一轮活」的证据到位了。
        let deadline = Instant::now() + Duration::from_secs(5);
        while !m.screen_text_for_test(id).contains("修一下登录白屏") {
            m.tick();
            assert!(Instant::now() < deadline, "cat 该把排队的输入回显出来");
            sleep(Duration::from_millis(20));
        }

        // 现在才是真正的下一轮：会话已经在 Idle，重新逼一次 Working，
        // 让它在屏幕尾巴已经带着用户那句话的情况下再走一次
        // Working → Idle。
        m.send_input(id, "").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            let tag = m.list().iter().find(|s| s.id == id).unwrap().tag.clone();
            if tag == "真实名字" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "真正干完一轮活之后应该起出真实名字，最后是 {tag:?}"
            );
            sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn with_no_backend_the_explanation_stays_empty_and_nothing_breaks() {
        // 这是「非 LLM 退路」的回归点：没配后端时 dct 表现得和今天一模一样。
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();
        m.set_backend(None);
        m.tick();
        assert_eq!(m.explanation(id), None);
    }

    #[test]
    fn entering_failed_asks_the_backend_once_not_every_tick() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Counting(Arc<AtomicUsize>);
        impl crate::llm::Backend for Counting {
            fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok("网络断了，重开一次就行。".into())
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(failing_agent()); // error_pattern 命中的假 agent
        let id = m
            .create(repo.path(), "failing", empty_secrets(), &[])
            .unwrap();
        m.set_backend(Some(Arc::new(Counting(calls.clone()))));

        let deadline = Instant::now() + Duration::from_secs(5);
        while m.explanation(id).is_none() && Instant::now() < deadline {
            m.tick();
            sleep(Duration::from_millis(50));
        }
        assert_eq!(
            m.explanation(id).as_deref(),
            Some("网络断了，重开一次就行。")
        );

        // 再 tick 若干轮：还是 Failed，但**不许**再问模型。
        for _ in 0..10 {
            m.tick();
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "只在进入 Failed 那一刻问一次"
        );
    }

    /// **Important (b) 回归测试.** 第二次失败之后，界面不该继续顶着第一次
    /// 失败时那句解释；哪怕算第一次那句的线程运气不好、比第二次还慢，晚了
    /// 才答完，也不能让它把第二次的新答案覆盖回旧的（last-writer-wins 的
    /// 那种覆盖，赢的必须是「最新一次失败」，不是「最后答完的那个」）。
    #[test]
    fn a_second_failure_does_not_show_the_first_failures_stale_explanation() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Sequenced(Arc<AtomicUsize>);
        impl crate::llm::Backend for Sequenced {
            fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
                let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
                if n == 1 {
                    // 第一次失败问得慢，且是「旧」答案——故意让它比第二次的
                    // 新答案更晚才答完，用来验证它写不进去。
                    sleep(Duration::from_millis(700));
                    Ok("旧的解释，不该被看到。".into())
                } else {
                    Ok("新的解释。".into())
                }
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir(&proj).unwrap();
        let mgr = SessionManager::new();
        // 先 BOOM（第一次失败），clear 掉再打 READY（恢复成 Idle——手法同
        // `busy_pattern_marks_working_then_idle`：`clear` 把 BOOM 从可见屏幕
        // 上抹掉，error_re 才会真的不再匹配），再 BOOM 一次（第二次失败）。
        mgr.register_profile(
            Profile::from_toml(
                r#"
                name = "flaky"
                command = ["/bin/sh", "-c", "echo BOOM; sleep 0.3; clear; echo READY; sleep 0.3; echo BOOM; sleep 5"]
                is_agent = false
                idle_pattern = "READY"
                error_pattern = "BOOM"
                "#,
            )
            .unwrap(),
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let id = mgr.create(&proj, "flaky", empty_secrets(), &[]).unwrap();
        mgr.set_backend(Some(Arc::new(Sequenced(calls))));

        // 第一次失败
        let deadline = Instant::now() + Duration::from_secs(5);
        while state_of(&mgr, id) != SessionState::Failed {
            mgr.tick();
            assert!(Instant::now() < deadline, "第一次 BOOM 该判成 Failed");
            sleep(Duration::from_millis(50));
        }

        // clear + READY 之后恢复成 Idle
        let deadline = Instant::now() + Duration::from_secs(5);
        while state_of(&mgr, id) != SessionState::Idle {
            mgr.tick();
            assert!(
                Instant::now() < deadline,
                "clear 之后 BOOM 该从屏幕上消失，判成 Idle"
            );
            sleep(Duration::from_millis(50));
        }

        // 第二次失败
        let deadline = Instant::now() + Duration::from_secs(5);
        while state_of(&mgr, id) != SessionState::Failed {
            mgr.tick();
            assert!(Instant::now() < deadline, "第二次 BOOM 该再次判成 Failed");
            sleep(Duration::from_millis(50));
        }

        // 第二次（快）的答案落地
        let deadline = Instant::now() + Duration::from_secs(5);
        while mgr.explanation(id).is_none() {
            mgr.tick();
            assert!(Instant::now() < deadline, "第二次失败的解释迟迟没有出现");
            sleep(Duration::from_millis(50));
        }
        assert_eq!(mgr.explanation(id).as_deref(), Some("新的解释。"));

        // 给第一次那个慢线程留足时间答完——它的答案不许把上面这份新的盖掉。
        sleep(Duration::from_millis(900));
        assert_eq!(
            mgr.explanation(id).as_deref(),
            Some("新的解释。"),
            "第一次失败的旧答案迟到了，不该覆盖第二次的新答案"
        );
    }

    /// `stop()` 只把状态改成 `Stopped`，从不删——守护进程活得很久，于是
    /// `dct ps` 会越积越多的墓碑。`prune()` 是把它们抹掉的那一步，而且
    /// **只抹已经停了的**：还在跑的会话被顺手删掉，用户就再也够不着它了
    /// （pty 还在守护进程里活着，但名册上没有它，停都停不掉）。
    #[test]
    fn prune_removes_stopped_sessions_and_leaves_the_rest() {
        let plain = tempfile::tempdir().unwrap();
        let m = SessionManager::new();
        let dead = m
            .create(plain.path(), "shell", empty_secrets(), &[])
            .unwrap();
        let alive = m
            .create(plain.path(), "shell", empty_secrets(), &[])
            .unwrap();
        m.stop(dead).unwrap();

        assert_eq!(m.prune(), 1, "只该抹掉那个已经停了的");
        let left: Vec<u32> = m.list().iter().map(|s| s.id).collect();
        assert_eq!(left, vec![alive], "还在跑的必须留着");

        // 再来一次没东西可抹了——已经抹过的不该被数第二遍
        assert_eq!(m.prune(), 0);
    }

    #[test]
    fn prune_on_a_clean_manager_removes_nothing() {
        let m = SessionManager::new();
        assert_eq!(m.prune(), 0);
    }

    /// `kill()` 跟 `stop()` 落在同一个状态上。对用户来说这两条命令的结果
    /// 是同一件事——这个会话不跑了；多一个「被强杀的」状态，就要在看板、
    /// 九宫格、`dct ps` 三处各给它一种画法，而它们要说的是同一句话。
    #[test]
    fn kill_stops_the_session_just_like_stop_does() {
        let plain = tempfile::tempdir().unwrap();
        let m = SessionManager::new();
        let id = m
            .create(plain.path(), "shell", empty_secrets(), &[])
            .unwrap();

        m.kill(id).unwrap();

        let s = m.list().into_iter().find(|s| s.id == id).unwrap();
        assert_eq!(s.state, SessionState::Stopped);
        // 杀完就该能被 prune 掉，跟 stop 出来的墓碑一视同仁
        assert_eq!(m.prune(), 1);
    }

    #[test]
    fn agent_session_runs_in_the_real_project_dir() {
        // agent 就在用户的真项目里干活，不再是某个副本——不然干完的活
        // 躺在一条分支上，用户拿不回来。
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();

        let dir = m.list().iter().find(|s| s.id == id).unwrap().dir.clone();
        let want = repo.path().canonicalize().unwrap();
        assert_eq!(
            std::path::PathBuf::from(&dir).canonicalize().unwrap(),
            want,
            "会话目录必须就是用户给的项目目录，实际是 {dir}"
        );
        assert!(!dir.contains("dct-worktrees"), "不该再建副本了：{dir}");
    }
    #[test]
    fn rejects_agent_session_outside_repo() {
        let plain = tempfile::tempdir().unwrap();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        // 断言的是**码**，不是句子——句子是界面的事，而且会随语言变。
        let err = m
            .create(plain.path(), "fake", empty_secrets(), &[])
            .unwrap_err();
        let code = err
            .downcast::<crate::proto::CodedError>()
            .expect("要带上错误码")
            .0;
        assert!(
            matches!(code, ErrorCode::NotAGitRepo(_)),
            "实际错误: {code:?}"
        );
    }

    #[test]
    fn shell_session_runs_in_place() {
        let plain = tempfile::tempdir().unwrap();
        let m = SessionManager::new();
        let id = m
            .create(plain.path(), "shell", empty_secrets(), &[])
            .unwrap();
        let dir = m.list().iter().find(|s| s.id == id).unwrap().dir.clone();
        assert!(!dir.contains("dct-worktrees"));
    }

    #[test]
    fn rejects_shell_session_with_missing_dir() {
        let m = SessionManager::new();
        let missing = std::path::PathBuf::from("/definitely/does/not/exist/dct-test-dir");
        let err = m
            .create(&missing, "shell", empty_secrets(), &[])
            .unwrap_err();
        let code = err
            .downcast::<crate::proto::CodedError>()
            .expect("要带上错误码")
            .0;
        assert!(
            matches!(code, ErrorCode::DirNotFound(_)),
            "实际错误: {code:?}"
        );
    }

    /// 构造性验证：故意让持有 `sessions` 锁的线程 panic，把锁弄"中毒"。
    /// 没有 `recover()` 的话，接下来所有请求都会 `.unwrap()` 到那个 `PoisonError`
    /// 上一起 panic/失败，而且这个守护进程没有 supervisor，中毒了就永久瘫痪。
    /// 期望：中毒之后 `SessionManager` 依然可以正常创建、列出会话。
    #[test]
    fn recovers_from_poisoned_sessions_lock() {
        let m = SessionManager::new();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = m.sessions.lock().unwrap();
            panic!("模拟持锁期间的 panic，用来验证锁中毒后还能恢复");
        }));
        assert!(result.is_err(), "上面这次 panic 应该被 catch_unwind 接住");

        let plain = tempfile::tempdir().unwrap();
        let id = m
            .create(plain.path(), "shell", empty_secrets(), &[])
            .expect("锁中毒之后 create() 应该还能正常工作，而不是永远失败");
        assert_eq!(m.list().iter().find(|s| s.id == id).unwrap().id, id);
    }

    /// **出错时屏幕上同时有错误和输入框提示**——`idle_pattern` 一样匹得上。
    /// 判定顺序把 `Failed` 排在前面，否则最要紧的那个事实会被一句「空闲」
    /// 盖掉，而那正是用户实际撞到的 bug：以为 agent 在等他，其实那一轮废了。
    #[test]
    fn an_error_on_screen_wins_over_the_idle_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir(&proj).unwrap();
        let mgr = SessionManager::new();
        // 先只打 READY（空闲），一秒后追加错误行，两句同时留在屏幕上。
        // 先等出一次 Idle 是为了逼 tick() 真正算过一次——否则这条测试
        // 可能只是撞上了某个默认值（同 busy_pattern_wins_over_idle_pattern）。
        mgr.register_profile(
            Profile::from_toml(
                r#"
                name = "boom"
                command = ["/bin/sh", "-c", "echo READY; sleep 1; echo 'API Error: closed'; sleep 5"]
                is_agent = false
                idle_pattern = "READY"
                error_pattern = "API Error"
                "#,
            )
            .unwrap(),
        );
        let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
        let id = mgr.create(&proj, "boom", secrets.get("boom"), &[]).unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            mgr.tick();
            if state_of(&mgr, id) == SessionState::Idle {
                break;
            }
            assert!(Instant::now() < deadline, "只有 READY 时应当是 Idle");
            sleep(Duration::from_millis(50));
        }

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            mgr.tick();
            if state_of(&mgr, id) == SessionState::Failed {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "错误和空闲提示同屏时，error_pattern 必须压过 idle_pattern"
            );
            sleep(Duration::from_millis(50));
        }
    }

    /// 没写 `error_pattern` 的 profile 行为完全不变——功能对它是关着的。
    /// 这条保证给别的 agent 补文案之前，它们一点都不会被误伤。
    #[test]
    fn a_profile_without_an_error_pattern_never_reports_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir(&proj).unwrap();
        let mgr = SessionManager::new();
        mgr.register_profile(
            Profile::from_toml(
                r#"
                name = "quiet"
                command = ["/bin/sh", "-c", "echo 'API Error: closed'; echo READY; sleep 5"]
                is_agent = false
                idle_pattern = "READY"
                "#,
            )
            .unwrap(),
        );
        let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
        let id = mgr
            .create(&proj, "quiet", secrets.get("quiet"), &[])
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            mgr.tick();
            if state_of(&mgr, id) == SessionState::Idle {
                break;
            }
            assert!(Instant::now() < deadline, "该判成 Idle");
            sleep(Duration::from_millis(50));
        }
        assert_ne!(
            state_of(&mgr, id),
            SessionState::Failed,
            "没声明错误文案的 agent 不该被判失败"
        );
    }

    #[test]
    fn a_stopped_session_is_not_reclassified_as_failed() {
        let repo = init_repo();
        let m = SessionManager::new();
        let mut p = fake_agent();
        p.error_pattern = Some("API Error".into());
        m.register_profile(p);
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();
        m.send_input(id, "API Error").unwrap();
        m.send_input(id, "").unwrap();
        m.stop(id).unwrap();
        m.tick();

        assert_eq!(
            m.list().iter().find(|s| s.id == id).unwrap().state,
            SessionState::Stopped
        );
    }

    #[test]
    fn tick_marks_idle_when_pattern_matches() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();

        m.send_input(id, "READY").unwrap();
        m.send_input(id, "").unwrap(); // 空字符串 = 回车

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            let st = m.list().iter().find(|s| s.id == id).unwrap().state;
            if st == SessionState::Idle || Instant::now() > deadline {
                assert_eq!(st, SessionState::Idle);
                break;
            }
            sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn undo_restores_last_checkpoint() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();

        let wt_dir: std::path::PathBuf = m
            .list()
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .dir
            .clone()
            .into();

        // 模拟 agent 干活：改文件
        fs::write(wt_dir.join("a.txt"), "agent wrote this\n").unwrap();
        m.undo(id).unwrap();

        assert_eq!(fs::read_to_string(wt_dir.join("a.txt")).unwrap(), "hello\n");
    }

    #[test]
    fn diff_reports_agent_changes() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();
        let wt_dir: std::path::PathBuf = m
            .list()
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .dir
            .clone()
            .into();

        fs::write(wt_dir.join("a.txt"), "hello\nmore\n").unwrap();
        let stats = m.diff(id).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].added, 1);
    }

    #[test]
    fn resize_changes_the_screen_size() {
        // agent 必须按界面的真实宽度排版，否则窗口再宽也只用得到左边一块
        let dir = tempfile::tempdir().unwrap();
        let m = SessionManager::new();
        let id = m.create(dir.path(), "shell", empty_secrets(), &[]).unwrap();

        m.resize(id, 30, 200).unwrap();

        let snap = m.screen(id).unwrap();
        assert_eq!(snap.lines.len(), 30, "行数应当跟着改");

        let width: usize = snap.lines[0].iter().map(|sp| sp.text.chars().count()).sum();
        assert_eq!(width, 200, "列数应当跟着改，实际 {width}");
    }

    /// agent 自己退出（用户在 Claude Code 里敲 /exit、或 shell 里敲 exit）之后，
    /// `screen()` 必须把 `Stopped` 捎回去。界面贴在会话里时只调 `Screen`，这是它
    /// 唯一能知道进程已经没了的途径；捎不回来就会一直画那张空缓冲。
    ///
    /// 空缓冲本身是正常的：agent 在 alternate screen 里画，退出时恢复主屏，
    /// 而主屏从来没被写过。所以「屏是空的」不能用来判断会话死活，只有状态能。
    #[test]
    fn screen_reports_stopped_after_the_process_exits() {
        let repo = init_repo();
        let m = SessionManager::new();
        let mut exits = fake_agent();
        // 立刻退出的命令：模拟 agent 自己结束，而不是被 stop() 杀掉
        exits.command = vec!["true".into()];
        exits.idle_pattern = None;
        m.register_profile(exits);
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();

        // 进程退出要一点时间，tick() 是把 is_alive() 落成 Stopped 的那一步
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            let state = m.screen(id).unwrap().state;
            if state == SessionState::Stopped {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "进程早该退出了，screen() 却一直报 {state:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// 活着的会话不能被误报成 Stopped——否则界面会把用户从一个好端端的
    /// 会话里踢回看板。
    #[test]
    fn screen_reports_a_live_session_as_not_stopped() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();
        m.tick();
        let state = m.screen(id).unwrap().state;
        assert_ne!(state, SessionState::Stopped, "cat 还在跑，不该报 Stopped");
    }

    #[test]
    fn stop_marks_stopped() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();
        m.stop(id).unwrap();
        let st = m.list().iter().find(|s| s.id == id).unwrap().state;
        assert_eq!(st, SessionState::Stopped);
    }

    /// 守护进程常常是从某个 agent 自己的会话里被拉起来的——用户在 Claude Code
    /// 里敲 `dct`，dct 发现没有 daemon 就 `setsid` 拉起一个。那个 daemon 一活
    /// 就是好几天，于是启动它的那个会话留在环境里的「我是子会话」标记，会被
    /// 原样传给它之后开的**每一个** agent。表现是每个新会话顶上都挂着一句
    /// 「Transcript saving is off」，聊天记录一条都不存，而用户完全不知道
    /// 这跟他几天前在哪敲的那一下有关系。
    ///
    /// 环境是「只加不减」的——PATH、HOME、各家 CLI 的登录态都得留着——
    /// 但这类标记必须摘掉。
    #[test]
    fn agent_sessions_do_not_inherit_the_launching_agents_markers() {
        // 进程级的改动，但这个变量全仓库没有别处读，不会干扰并行跑的其他测试。
        std::env::set_var("CLAUDE_CODE_CHILD_SESSION", "contaminated");

        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir(&proj).unwrap();

        let mgr = SessionManager::new();
        mgr.register_profile(
            Profile::from_toml(
                r#"
            name = "fake-agent"
            command = ["/bin/sh", "-c", "echo MARK=[$CLAUDE_CODE_CHILD_SESSION] HOME=[$HOME]; sleep 5"]
            is_agent = false
            "#,
            )
            .unwrap(),
        );

        let id = mgr.create(&proj, "fake-agent", None, &[]).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let text = loop {
            let text = mgr.screen_text_for_test(id);
            if text.contains("MARK=") {
                break text;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "会话没打印出东西来：{text}"
            );
            sleep(Duration::from_millis(50));
        };

        assert!(
            text.contains("MARK=[]"),
            "启动 dct 的那个 agent 的会话标记漏给了新会话：{text}"
        );
        // 同一屏里验一下没有把环境清空——只减这一类标记，别的照传。
        assert!(text.contains("HOME=[/"), "把继承来的环境清过头了：{text}");

        std::env::remove_var("CLAUDE_CODE_CHILD_SESSION");
    }

    #[test]
    fn create_injects_the_secret_into_env() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir(&proj).unwrap();

        let mgr = SessionManager::new();
        mgr.register_profile(
            Profile::from_toml(
                r#"
            name = "fake-api"
            command = ["/bin/sh", "-c", "echo TOKEN=$MY_TOKEN BASE=$MY_BASE; sleep 5"]
            is_agent = false

            [env]
            MY_BASE = "https://example.com"

            [secret]
            env = "MY_TOKEN"
            "#,
            )
            .unwrap(),
        );

        let mut secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
        secrets.set("fake-api", "sk-xyz").unwrap();

        let id = mgr
            .create(&proj, "fake-api", secrets.get("fake-api"), &[])
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let text = mgr.screen_text_for_test(id);
            if text.contains("TOKEN=sk-xyz") && text.contains("BASE=https://example.com") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "没看到注入的环境变量：{text}"
            );
            sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn create_without_the_secret_still_starts() {
        // 没填密钥不该在 create 这一层拦住——可用性判定是 UI 的事，
        // create 拦一遍会让「装完 CLI 想先跑起来看看」这种路径莫名其妙失败。
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir(&proj).unwrap();

        let mgr = SessionManager::new();
        mgr.register_profile(
            Profile::from_toml(
                r#"
            name = "fake-api"
            command = ["/bin/sh", "-c", "sleep 5"]
            is_agent = false

            [secret]
            env = "MY_TOKEN"
            "#,
            )
            .unwrap(),
        );

        let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
        assert!(mgr
            .create(&proj, "fake-api", secrets.get("fake-api"), &[])
            .is_ok());
    }

    // 下面两个测试踩的是同一块地雷：只要 profile 配了任意 pattern，create() 就把初始状态
    // 直接定成 Working（见 create() 里「有 pattern 才敢说干活中」那段注释）。所以「刚建完号
    // 就轮询等 Working」这个动作本身证明不了 tick() 的判定逻辑真的跑对了——它完全可能是撞上
    // 构造函数给的默认值退出循环的，tick() 一次都没被断言检验过。想让测试真的验到 tick()，
    // 断言目标得选 Idle、Unknown，或者「状态没被 tick 动过」这类够不到默认值的东西。
    #[test]
    fn busy_pattern_marks_working_then_idle() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir(&proj).unwrap();

        let mgr = SessionManager::new();
        mgr.register_profile(
            Profile::from_toml(
                r#"
                name = "busy-demo"
                command = ["/bin/sh", "-c", "echo esc to interrupt; sleep 1; clear; echo done; sleep 5"]
                is_agent = false
                busy_pattern = "esc to interrupt"
                "#,
            )
            .unwrap(),
        );
        let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
        let id = mgr
            .create(&proj, "busy-demo", secrets.get("busy-demo"), &[])
            .unwrap();

        // 屏幕上有 busy 串 → 干活中
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            mgr.tick();
            if state_of(&mgr, id) == SessionState::Working {
                break;
            }
            assert!(Instant::now() < deadline, "busy 串在屏上就该是 Working");
            sleep(Duration::from_millis(50));
        }

        // 串消失 → 空闲
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            mgr.tick();
            if state_of(&mgr, id) == SessionState::Idle {
                break;
            }
            assert!(Instant::now() < deadline, "busy 串没了就该是 Idle");
            sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn busy_pattern_wins_over_idle_pattern() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir(&proj).unwrap();

        let mgr = SessionManager::new();
        // 先只打出 IDLE，等一秒再把 BUSY 追加上去（不清屏，两个串同时留在屏幕上）。
        // 不能一开始就把 BUSY 和 IDLE 一起打出来：那样的话 create() 的默认初始状态
        // 已经是 Working（见上面那条注释），下面等 Working 的循环第一轮就会命中，
        // 根本没逼 tick() 真正算过一次——busy 优先于 idle 这条规则完全没被验证。
        // 先等出一次 Idle，就是先逼一次相对默认值的真实翻转，证明 tick() 确实跑过；
        // 然后 BUSY 追加上去必须翻回 Working，只有「busy_re 先判定」才会翻回去，
        // 如果实现改成先看 idle_re，屏上 IDLE 还在，状态会一直卡在 Idle 直到超时。
        mgr.register_profile(
            Profile::from_toml(
                r#"
                name = "both"
                command = ["/bin/sh", "-c", "echo IDLE; sleep 1; echo BUSY; sleep 5"]
                is_agent = false
                busy_pattern = "BUSY"
                idle_pattern = "IDLE"
                "#,
            )
            .unwrap(),
        );
        let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
        let id = mgr.create(&proj, "both", secrets.get("both"), &[]).unwrap();

        // 只有 IDLE 在屏上 → Idle。这一步是相对 create() 默认值 Working 的真实翻转。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            mgr.tick();
            if state_of(&mgr, id) == SessionState::Idle {
                break;
            }
            assert!(Instant::now() < deadline, "只有 IDLE 串时应该是 Idle");
            sleep(Duration::from_millis(50));
        }

        // BUSY 追加上去，IDLE 仍在屏上（两个串同时可见）→ 必须翻回 Working。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            mgr.tick();
            if state_of(&mgr, id) == SessionState::Working {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "busy_pattern 必须压过 idle_pattern"
            );
            sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn no_pattern_stays_unknown() {
        // shell 就是这种。以前它永远显示「干活中」，是明确的假信息。
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir(&proj).unwrap();

        let mgr = SessionManager::new();
        mgr.register_profile(
            Profile::from_toml(
                r#"
                name = "quiet"
                command = ["/bin/sh", "-c", "sleep 5"]
                is_agent = false
                "#,
            )
            .unwrap(),
        );
        let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
        let id = mgr
            .create(&proj, "quiet", secrets.get("quiet"), &[])
            .unwrap();

        assert_eq!(
            state_of(&mgr, id),
            SessionState::Unknown,
            "没 pattern 就别编状态"
        );
        for _ in 0..5 {
            mgr.tick();
            sleep(Duration::from_millis(20));
        }
        assert_eq!(
            state_of(&mgr, id),
            SessionState::Unknown,
            "tick 也不该把它改成 Working"
        );
    }

    #[test]
    fn screens_returns_entries_for_known_ids_and_skips_unknown() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new();
        let id1 = mgr
            .create(dir1.path(), "shell", empty_secrets(), &[])
            .unwrap();
        let id2 = mgr
            .create(dir2.path(), "shell", empty_secrets(), &[])
            .unwrap();

        let entries = mgr.screens(&[id1, id2, 9999]);

        assert_eq!(entries.len(), 2, "9999 不存在，应该被跳过而不是报错");
        assert_eq!(entries[0].id, id1);
        assert_eq!(entries[1].id, id2);
        // 屏幕是 40 行的 vt100 缓冲，行数应该等于会话的行数
        assert_eq!(entries[0].lines.len(), 40);
    }

    #[test]
    fn spawn_failure_says_what_to_do_not_enoent() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir(&proj).unwrap();

        let mgr = SessionManager::new();
        mgr.register_profile(
            Profile::from_toml("name = \"gone\"\ncommand = [\"/绝对不存在/x9\"]\n").unwrap(),
        );
        let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
        let err = mgr
            .create(&proj, "gone", secrets.get("gone"), &[])
            .unwrap_err();
        let code = err
            .downcast::<crate::proto::CodedError>()
            .expect("要带上错误码")
            .0;
        // 码里只有命令名，**结构上就没有地方**能塞进 ENOENT——
        // 这比原来靠断言字符串不含 "enoent" 强，那种断言只能拦住已知的写法。
        let ErrorCode::CannotStart(ref cmd) = code else {
            panic!("应当是「启动不了」这一类：{code:?}");
        };
        assert_eq!(cmd, "/绝对不存在/x9", "要点名是哪个命令");
        let line = crate::i18n::msg::error(crate::i18n::Lang::Zh, &code);
        assert!(line.contains("启动不了"), "要说人话：{line}");
        assert!(
            !line.to_lowercase().contains("enoent"),
            "别把系统错误码甩给用户：{line}"
        );
    }

    /// 造一个吐 N 行然后挂着的 shell 会话
    fn scrolling_session(mgr: &SessionManager, dir: &Path, n: usize) -> u32 {
        let mut p = fake_agent();
        p.is_agent = false;
        p.command = vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("i=1; while [ $i -le {n} ]; do echo line-$i; i=$((i+1)); done; sleep 30"),
        ];
        mgr.register_profile(p.clone());
        mgr.create(dir, &p.name, empty_secrets(), &[]).unwrap()
    }

    fn wait_for_screen(mgr: &SessionManager, id: u32, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if mgr.screen_text_for_test(id).contains(needle) {
                return;
            }
            sleep(Duration::from_millis(50));
        }
        panic!("等不到 {needle}");
    }

    #[test]
    fn typing_jumps_back_to_the_bottom() {
        let dir = init_repo();
        let mgr = SessionManager::new();
        let id = scrolling_session(&mgr, dir.path(), 100);
        wait_for_screen(&mgr, id, "line-100");

        mgr.scroll(id, ScrollBy::Rows(30)).unwrap();
        assert!(mgr.screen(id).unwrap().scroll.offset > 0);

        mgr.send_input(id, "x").unwrap();
        assert_eq!(
            mgr.screen(id).unwrap().scroll.offset,
            0,
            "一敲键就该回到底部，否则用户看不见自己打的字"
        );
    }

    #[test]
    fn resizing_jumps_back_to_the_bottom() {
        let dir = init_repo();
        let mgr = SessionManager::new();
        let id = scrolling_session(&mgr, dir.path(), 100);
        wait_for_screen(&mgr, id, "line-100");

        mgr.scroll(id, ScrollBy::Rows(30)).unwrap();
        mgr.resize(id, 40, 100).unwrap();
        assert_eq!(
            mgr.screen(id).unwrap().scroll.offset,
            0,
            "重排之后偏移的含义就失效了，只能回底"
        );
    }

    #[test]
    fn scroll_to_bottom_works() {
        let dir = init_repo();
        let mgr = SessionManager::new();
        let id = scrolling_session(&mgr, dir.path(), 100);
        wait_for_screen(&mgr, id, "line-100");

        mgr.scroll(id, ScrollBy::Rows(30)).unwrap();
        let st = mgr.scroll(id, ScrollBy::Bottom).unwrap();
        assert_eq!(st.offset, 0);
    }

    #[test]
    fn new_lines_counts_only_what_arrived_since_the_user_last_scrolled() {
        let dir = init_repo();
        let mgr = SessionManager::new();
        let mut p = fake_agent();
        p.is_agent = false;
        p.command = vec![
            "/bin/sh".into(),
            "-c".into(),
            "i=1; while [ $i -le 60 ]; do echo line-$i; i=$((i+1)); done; \
             sleep 1; i=1; while [ $i -le 5 ]; do echo new-$i; i=$((i+1)); done; sleep 30"
                .into(),
        ];
        mgr.register_profile(p.clone());
        let id = mgr
            .create(dir.path(), &p.name, empty_secrets(), &[])
            .unwrap();
        wait_for_screen(&mgr, id, "line-60");

        // 刚滚完，底下没有新东西
        let st = mgr.scroll(id, ScrollBy::Rows(20)).unwrap();
        assert_eq!(st.new_lines, 0);

        // 滚上去之后新行不会出现在当前视口里——vt100 会自动把偏移往上顶，
        // 让画面看起来"没动"（这正是 new_lines 存在的理由：界面得靠这个
        // 数字告诉用户"底下有你还没看过的东西"，屏幕内容本身根本不会变）。
        // 所以这里不能像别处那样等屏幕文字出现，只能等 scroll.new_lines
        // 本身涨到 5。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let st = mgr.screen(id).unwrap().scroll;
            if st.new_lines == 5 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "5 行新内容进来了，得数得出来，实际 new_lines={}",
                st.new_lines
            );
            sleep(Duration::from_millis(50));
        }

        // 用户再滚一次，计数重新归零
        let st = mgr.scroll(id, ScrollBy::Rows(1)).unwrap();
        assert_eq!(st.new_lines, 0);
    }

    #[test]
    fn scrolling_a_session_that_does_not_exist_says_so() {
        let mgr = SessionManager::new();
        assert!(mgr.scroll(999, ScrollBy::Rows(1)).is_err());
    }

    /// 旧守护进程发来的 JSON 没有 `tag` 这个字段。必须补成空串而不是
    /// 反序列化失败 —— 这正是本版**不升协议号**的全部依据（同 `scroll`
    /// 字段当初的做法，见 `proto.rs` 里那条注释）。
    #[test]
    fn session_info_without_a_tag_field_still_parses() {
        let old = r#"{"id":3,"profile":"claude","dir":"/w/a",
                      "state":"Idle","activity":"","is_agent":true}"#;
        let s: SessionInfo = serde_json::from_str(old).expect("旧 JSON 必须还能读");
        assert_eq!(s.tag, "", "缺字段补空串");
        assert_eq!(s.id, 3);
    }

    // ---- should_notify：三道门，全 AND ----

    /// 是 agent、有渠道，但用户还没说过话——刚创建、还停在启动画面上的
    /// 会话正是这样。这是三道门里唯一真会被踩到的坑，见 `should_notify`
    /// 自己的文档。
    #[test]
    fn a_brand_new_session_does_not_page_you() {
        assert!(!should_notify(true, true, true));
    }

    #[test]
    fn a_plain_shell_never_pages_you() {
        assert!(!should_notify(false, false, true));
    }

    #[test]
    fn no_channel_means_no_page() {
        assert!(!should_notify(true, false, false));
    }

    #[test]
    fn an_agent_you_have_talked_to_pages_you() {
        assert!(should_notify(true, false, true));
    }

    // ---- tick()：完整走一遍，钉住「刚开会话不该震手机」这件事 ----

    /// `create()` 之后立刻 `tick()`：假 profile 只声明 `busy_pattern`
    /// （真实 profile 的形状），没人跟它说过话，屏幕自然读成 Idle。
    /// 没有 `should_notify` 那道 `first_input` 门，这一跳会被误判成
    /// 「干完一轮活」，事件队列里会多出一条 `Stopped`——这正是
    /// 「每开一个会话手机就响一次」那个 bug 的样子。多 tick 几轮，
    /// 确认这件事不是运气好躲过了一次，而是稳定地不会发生。
    #[test]
    fn a_brand_new_session_does_not_wake_your_phone() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(busy_only_agent());
        let (tx, rx) = mpsc::channel();
        m.set_event_sink(tx);

        let id = m
            .create(repo.path(), "busy-only", empty_secrets(), &[])
            .unwrap();

        for _ in 0..5 {
            m.tick();
            assert!(
                rx.try_recv().is_err(),
                "会话 {id} 还没人说过话，队列里不该有任何事件"
            );
            sleep(Duration::from_millis(20));
        }
    }

    /// 用户真的说过话、agent 干完一轮活之后，`Stopped` 事件必须真的
    /// 送到队列里——上一条测试钉住「不该响」的那一半，这条钉住
    /// 「该响的时候真的响」的那一半，免得把三道门全改成恒假也能让
    /// 上一条测试通过。
    #[test]
    fn an_agent_that_finishes_a_real_turn_wakes_your_phone() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(finishing_agent());
        let (tx, rx) = mpsc::channel();
        m.set_event_sink(tx);

        let id = m
            .create(repo.path(), "finishing", empty_secrets(), &[])
            .unwrap();
        m.send_input(id, "修一下登录白屏").unwrap();
        m.send_input(id, "").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            if let Ok(ev) = rx.try_recv() {
                assert_eq!(ev.session, id);
                assert_eq!(ev.kind, EventKind::Stopped);
                break;
            }
            assert!(Instant::now() < deadline, "真正干完一轮活该震一次手机");
            sleep(Duration::from_millis(20));
        }
    }
}
