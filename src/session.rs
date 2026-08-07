use crate::proto::{coded, ErrorCode, Operation};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::git::{self, FileStat};
use crate::profile::Profile;
use crate::pty::{PtySession, ScreenSpan};

/// 一屏文字、光标位置、会话状态：`screen()` 的返回值，行的集合按 (行, 列)
/// 排布 span，光标是 (行, 列)。type_complexity 报警要求给这个组合起个名字。
///
/// 状态挤在这里而不是让界面另发一次 `List`：贴在会话里时界面只调 `Screen`
/// （`List` 要逐个锁所有会话、取每个的最后一行，16ms 一轮太贵），所以进程
/// 死了它一无所知——会永远画那张空缓冲，底栏还写着「其余按键都发给 agent」。
/// 状态是这条 16ms 通路上唯一能捎回来的存活信号，而这里本来就已经持着锁了。
pub type ScreenSnapshot = (Vec<Vec<ScreenSpan>>, (u16, u16), SessionState);

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
        }
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
        };

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
                SessionInfo {
                    id: s.id,
                    profile: s.profile.name.clone(),
                    dir: s.dir.display().to_string(),
                    state: s.state,
                    activity: s.pty.last_line(),
                    is_agent: s.is_agent,
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

            let g = recover(arc.lock());
            return g.pty.write(b"\r");
        }

        let g = recover(arc.lock());
        g.pty.write(text.as_bytes())
    }

    /// 返回 agent 屏幕文本和光标位置 (行, 列)。光标必须跟文本一起取，
    /// 否则界面只是一张死截图，用户看不出自己打的字落在哪。
    pub fn screen(&self, id: u32) -> Result<ScreenSnapshot> {
        self.with_session(id, |s| Ok((s.pty.screen_spans(), s.pty.cursor(), s.state)))
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
        self.with_session(id, |s| s.pty.resize(rows, cols))
    }

    pub fn stop(&self, id: u32) -> Result<()> {
        self.with_session(id, |s| {
            s.pty.kill()?;
            s.state = SessionState::Stopped;
            Ok(())
        })
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
                let _ = s.pty.kill();
                s.state = SessionState::Stopped;
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
                    s.state = next;
                }
            }
            // 两个都没有：状态不动，保持 Unknown
        }
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
            label: Default::default(),
            note: Default::default(),
        }
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

        let (lines, _, _) = m.screen(id).unwrap();
        assert_eq!(lines.len(), 30, "行数应当跟着改");

        let width: usize = lines[0].iter().map(|sp| sp.text.chars().count()).sum();
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
            let (_, _, state) = m.screen(id).unwrap();
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
        let (_, _, state) = m.screen(id).unwrap();
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
}
