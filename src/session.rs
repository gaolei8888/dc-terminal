use crate::proto::{coded, ErrorCode, Operation};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

/// 按**字符数**封顶追加。这里不按显示宽度算：这段字是喂给模型的原料，
/// 不是画在屏幕上的东西，宽度是界面那一侧的事。
fn append_capped(buf: &mut String, text: &str) {
    for ch in text.chars() {
        if buf.chars().count() >= FIRST_INPUT_MAX {
            return;
        }
        buf.push(ch);
    }
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
    /// 这个会话的稳定名字，守护进程在它第一次干完活时起一次，之后不变。
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
    /// `None` = 还没触发过起名；`Some(_)` = 已经触发过（**只触发一次**）。
    name_slot: Arc<Mutex<Option<String>>>,
    /// 用户对这个会话说的第一句话，起名用。只在 agent 会话上攒。
    first_input: String,
    /// 第一句攒完了没有。见 `collect_first_input`。
    first_input_sealed: bool,
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
        }
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
            explanation_gen: Arc::new(AtomicU64::new(0)),
            scroll_mark: 0,
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
                    }
                }
            }
            // 两个都没有：状态不动，保持 Unknown
        }
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

    /// 新建的会话还没起过名。
    #[test]
    fn a_fresh_session_has_no_tag() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();

        let tag = m.list().iter().find(|s| s.id == id).unwrap().tag.clone();
        assert_eq!(tag, "");
    }
}
