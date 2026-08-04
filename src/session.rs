use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::git::{self, FileStat};
use crate::profile::Profile;
use crate::pty::{PtySession, ScreenSpan};
use crate::secrets::SecretStore;

/// 一屏文字加光标位置：`screen()` 的返回值，行的集合按 (行, 列) 排布 span，
/// 光标是 (行, 列)。type_complexity 报警要求给这个组合起个名字。
pub type ScreenSnapshot = (Vec<Vec<ScreenSpan>>, (u16, u16));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Working,
    /// 由后续的 Bridge 在 agent 调用 ask_human 时设置；本计划内不会出现
    Asking,
    Idle,
    Stopped,
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
}

struct Session {
    id: u32,
    profile: Profile,
    dir: PathBuf,
    is_agent: bool,
    checkpoints: Vec<String>,
    state: SessionState,
    idle_re: Option<regex::Regex>,
    /// 干活时屏幕上一定有的串（Task 6 的 tick 才会读）。跟 idle_re 一起在
    /// 构造时编译好，profile 的正则错误在起会话这一刻就暴露，不拖到 tick。
    #[allow(dead_code)]
    busy_re: Option<regex::Regex>,
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

    fn resolve_profile(&self, name: &str) -> Result<Profile> {
        if let Some(p) = recover(self.extra_profiles.lock()).get(name) {
            return Ok(p.clone());
        }
        Profile::builtin(name).ok_or_else(|| anyhow::anyhow!("没有这个 profile: {name}"))
    }

    pub fn create(&self, dir: &Path, profile_name: &str, secrets: &SecretStore) -> Result<u32> {
        let profile = self.resolve_profile(profile_name)?;

        if !dir.is_dir() {
            bail!("目录不存在: {}", dir.display());
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
            bail!("{} 不是 git 仓库，无法开 agent 会话", dir.display());
        }

        let idle_re = profile.idle_regex()?;
        let busy_re = profile.busy_regex()?;
        let is_agent = profile.is_agent;

        // profile 的静态 env 打底，密钥覆盖上去。密钥不在 profile 文件里，
        // 只在这一步才和命令合到一起——profile 文件因此可以随便拷贝分享。
        //
        // 密钥缺失在这里**不报错**：能不能用是可用性/UI 层的事（后续任务），
        // create() 拦一遍会让「先装上 CLI 试试能不能跑」这种路径莫名其妙失败。
        let mut env = profile.env.clone();
        if let Some(spec) = &profile.secret {
            if let Some(key) = secrets.get(&profile.name) {
                env.insert(spec.env.clone(), key.to_string());
            }
        }

        let pty = PtySession::spawn(&profile.command, &env, dir, 40, 120)?;

        let mut checkpoints = Vec::new();
        if is_agent {
            checkpoints.push(git::checkpoint(dir, id, 0)?);
        }

        let session = Session {
            id,
            profile,
            dir: dir.to_path_buf(),
            is_agent,
            checkpoints,
            state: SessionState::Working,
            idle_re,
            busy_re,
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
            .ok_or_else(|| anyhow::anyhow!("没有这个会话: {id}"))
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
                let sha = git::checkpoint(&dir, sid, seq)?; // 慢，无锁
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
        self.with_session(id, |s| Ok((s.pty.screen_spans(), s.pty.cursor())))
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
        let (dir, sha) = self.checkpoint_base(id, "这个会话没有检查点")?;
        git::restore(&dir, &sha)
    }

    /// 相对最后一张快照改了哪些文件。git 操作不持会话锁。
    pub fn diff(&self, id: u32) -> Result<Vec<FileStat>> {
        let (dir, base) = self.checkpoint_base(id, "这个会话没有改动记录")?;
        git::diff_stat(&dir, &base)
    }

    /// 取出做 git 操作需要的信息后立刻放锁。
    fn checkpoint_base(&self, id: u32, not_agent: &str) -> Result<(PathBuf, String)> {
        let arc = self.get_arc(id)?;
        let s = recover(arc.lock());
        if !s.is_agent {
            anyhow::bail!("{not_agent}");
        }
        let sha = s
            .checkpoints
            .last()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("还没有检查点"))?;
        Ok((s.dir.clone(), sha))
    }

    /// 扫一遍所有会话，更新状态。由守护进程定时调用。
    pub fn tick(&self) {
        let snapshot: Vec<Arc<Mutex<Session>>> =
            recover(self.sessions.lock()).values().cloned().collect();

        for s in snapshot {
            let mut s = recover(s.lock());
            if s.state == SessionState::Stopped {
                continue;
            }
            if !s.pty.is_alive() {
                s.state = SessionState::Stopped;
                continue;
            }
            if s.state == SessionState::Asking {
                continue;
            }
            if let Some(re) = &s.idle_re {
                s.state = if re.is_match(&s.pty.screen_text()) {
                    SessionState::Idle
                } else {
                    SessionState::Working
                };
            }
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

    /// 大多数测试不关心密钥，只是要满足 `create()` 新增的形参。指向一个
    /// 从没写过的路径，`SecretStore::load` 对不存在的文件视为「空」，不是错误。
    fn empty_secrets() -> SecretStore {
        let tmp = tempfile::tempdir().unwrap();
        SecretStore::load(&tmp.path().join("secrets.toml"))
    }

    // 用 cat 冒充 agent：能收输入、不会自己退出
    fn fake_agent() -> Profile {
        Profile {
            name: "fake".into(),
            command: vec!["cat".into()],
            is_agent: true,
            idle_pattern: Some("READY".into()),
            busy_pattern: None,
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
        let id = m.create(repo.path(), "fake", &empty_secrets()).unwrap();

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
        let err = m
            .create(plain.path(), "fake", &empty_secrets())
            .unwrap_err()
            .to_string();
        assert!(err.contains("不是 git 仓库"), "实际错误: {err}");
    }

    #[test]
    fn shell_session_runs_in_place() {
        let plain = tempfile::tempdir().unwrap();
        let m = SessionManager::new();
        let id = m.create(plain.path(), "shell", &empty_secrets()).unwrap();
        let dir = m.list().iter().find(|s| s.id == id).unwrap().dir.clone();
        assert!(!dir.contains("dct-worktrees"));
    }

    #[test]
    fn rejects_shell_session_with_missing_dir() {
        let m = SessionManager::new();
        let missing = std::path::PathBuf::from("/definitely/does/not/exist/dct-test-dir");
        let err = m
            .create(&missing, "shell", &empty_secrets())
            .unwrap_err()
            .to_string();
        assert!(err.contains("目录不存在"), "实际错误: {err}");
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
            .create(plain.path(), "shell", &empty_secrets())
            .expect("锁中毒之后 create() 应该还能正常工作，而不是永远失败");
        assert_eq!(m.list().iter().find(|s| s.id == id).unwrap().id, id);
    }

    #[test]
    fn tick_marks_idle_when_pattern_matches() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake", &empty_secrets()).unwrap();

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
        let id = m.create(repo.path(), "fake", &empty_secrets()).unwrap();

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
        let id = m.create(repo.path(), "fake", &empty_secrets()).unwrap();
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
        let id = m.create(dir.path(), "shell", &empty_secrets()).unwrap();

        m.resize(id, 30, 200).unwrap();

        let (lines, _) = m.screen(id).unwrap();
        assert_eq!(lines.len(), 30, "行数应当跟着改");

        let width: usize = lines[0].iter().map(|sp| sp.text.chars().count()).sum();
        assert_eq!(width, 200, "列数应当跟着改，实际 {width}");
    }

    #[test]
    fn stop_marks_stopped() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake", &empty_secrets()).unwrap();
        m.stop(id).unwrap();
        let st = m.list().iter().find(|s| s.id == id).unwrap().state;
        assert_eq!(st, SessionState::Stopped);
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

        let id = mgr.create(&proj, "fake-api", &secrets).unwrap();

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
        assert!(mgr.create(&proj, "fake-api", &secrets).is_ok());
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
        let err = mgr.create(&proj, "gone", &secrets).unwrap_err().to_string();

        assert!(err.contains("启动不了"), "要说人话：{err}");
        assert!(
            !err.to_lowercase().contains("enoent"),
            "别把系统错误码甩给用户：{err}"
        );
    }
}
