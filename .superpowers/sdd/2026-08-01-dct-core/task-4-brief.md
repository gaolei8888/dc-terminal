### Task 4: Session 状态机与 SessionManager

**Files:**
- Create: `src/session.rs`
- Modify: `src/main.rs`（加 `mod session;`）

**Interfaces:**
- Consumes: `profile::Profile`、`git::{Worktree, FileStat, create_worktree, remove_worktree, checkpoint, reset_to, diff_stat, is_repo}`、`pty::PtySession`
- Produces: `session::SessionState { Working, Asking, Idle, Stopped }`；`session::SessionInfo { id: u32, profile: String, dir: String, state: SessionState }`；`session::SessionManager`；方法 `new()`、`create(&mut self, dir: &Path, profile_name: &str) -> Result<u32>`、`list(&self) -> Vec<SessionInfo>`、`send_input(&mut self, id: u32, text: &str) -> Result<()>`、`screen(&self, id: u32) -> Result<String>`、`stop(&mut self, id: u32) -> Result<()>`、`undo(&mut self, id: u32) -> Result<()>`、`diff(&self, id: u32) -> Result<Vec<FileStat>>`、`tick(&mut self)`

**说明：**
- `create` 对 `is_agent` 的 profile 建 worktree 并立刻打第一个检查点；shell profile 不建 worktree，直接在给的目录里跑。
- `send_input` 在把文字送进 agent **之前**打检查点——这就是「每轮前检查点」。
- `tick` 用 idle 正则扫屏幕更新状态；进程死了就是 `Stopped`。
- `undo` 重置到最后一个检查点，不弹栈，重复按不会越退越多（对应 spec 的「回滚到上一个检查点」）。

- [ ] **Step 1: 写失败的测试**

`src/session.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let run = |args: &[&str]| {
            Command::new("git").args(args).current_dir(p).output().unwrap();
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        fs::write(p.join("a.txt"), "hello\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }

    // 用 cat 冒充 agent：能收输入、不会自己退出
    fn fake_agent() -> Profile {
        Profile {
            name: "fake".into(),
            command: vec!["cat".into()],
            is_agent: true,
            idle_pattern: Some("READY".into()),
        }
    }

    #[test]
    fn agent_session_runs_in_worktree_not_main_tree() {
        let repo = init_repo();
        let mut m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake").unwrap();

        let dir = m.list().iter().find(|s| s.id == id).unwrap().dir.clone();
        assert!(dir.contains("dct-worktrees"), "会话必须跑在 worktree 里，实际是 {dir}");
    }

    #[test]
    fn rejects_agent_session_outside_repo() {
        let plain = tempfile::tempdir().unwrap();
        let mut m = SessionManager::new();
        m.register_profile(fake_agent());
        let err = m.create(plain.path(), "fake").unwrap_err().to_string();
        assert!(err.contains("不是 git 仓库"), "实际错误: {err}");
    }

    #[test]
    fn shell_session_runs_in_place() {
        let plain = tempfile::tempdir().unwrap();
        let mut m = SessionManager::new();
        let id = m.create(plain.path(), "shell").unwrap();
        let dir = m.list().iter().find(|s| s.id == id).unwrap().dir.clone();
        assert!(!dir.contains("dct-worktrees"));
    }

    #[test]
    fn tick_marks_idle_when_pattern_matches() {
        let repo = init_repo();
        let mut m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake").unwrap();

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
        let mut m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake").unwrap();

        let wt_dir: std::path::PathBuf =
            m.list().iter().find(|s| s.id == id).unwrap().dir.clone().into();

        // 模拟 agent 干活：改文件
        fs::write(wt_dir.join("a.txt"), "agent wrote this\n").unwrap();
        m.undo(id).unwrap();

        assert_eq!(fs::read_to_string(wt_dir.join("a.txt")).unwrap(), "hello\n");
    }

    #[test]
    fn diff_reports_agent_changes() {
        let repo = init_repo();
        let mut m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake").unwrap();
        let wt_dir: std::path::PathBuf =
            m.list().iter().find(|s| s.id == id).unwrap().dir.clone().into();

        fs::write(wt_dir.join("a.txt"), "hello\nmore\n").unwrap();
        let stats = m.diff(id).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].added, 1);
    }

    #[test]
    fn stop_marks_stopped() {
        let repo = init_repo();
        let mut m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake").unwrap();
        m.stop(id).unwrap();
        let st = m.list().iter().find(|s| s.id == id).unwrap().state;
        assert_eq!(st, SessionState::Stopped);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test session`
Expected: 编译失败，`SessionManager` 未定义。

- [ ] **Step 3: 实现 session 模块**

`src/session.rs`：

```rust
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::git::{self, FileStat, Worktree};
use crate::profile::Profile;
use crate::pty::PtySession;

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
    pub dir: String,
    pub state: SessionState,
}

struct Session {
    id: u32,
    profile: Profile,
    dir: PathBuf,
    worktree: Option<Worktree>,
    checkpoints: Vec<String>,
    state: SessionState,
    idle_re: Option<regex::Regex>,
    pty: PtySession,
}

pub struct SessionManager {
    next_id: u32,
    sessions: HashMap<u32, Session>,
    extra_profiles: HashMap<String, Profile>,
}

impl SessionManager {
    pub fn new() -> Self {
        SessionManager { next_id: 1, sessions: HashMap::new(), extra_profiles: HashMap::new() }
    }

    /// 注册内置之外的 profile（测试用，也是将来从磁盘加载自定义 profile 的入口）
    pub fn register_profile(&mut self, p: Profile) {
        self.extra_profiles.insert(p.name.clone(), p);
    }

    fn resolve_profile(&self, name: &str) -> Result<Profile> {
        if let Some(p) = self.extra_profiles.get(name) {
            return Ok(p.clone());
        }
        Profile::builtin(name).ok_or_else(|| anyhow::anyhow!("没有这个 profile: {name}"))
    }

    pub fn create(&mut self, dir: &Path, profile_name: &str) -> Result<u32> {
        let profile = self.resolve_profile(profile_name)?;
        let id = self.next_id;

        let (workdir, worktree) = if profile.is_agent {
            if !git::is_repo(dir) {
                bail!("{} 不是 git 仓库，无法开 agent 会话", dir.display());
            }
            let wt = git::create_worktree(dir, &format!("s{id}"))?;
            (wt.path.clone(), Some(wt))
        } else {
            (dir.to_path_buf(), None)
        };

        let idle_re = profile.idle_regex()?;
        let pty = PtySession::spawn(&profile.command, &workdir, 40, 120)?;

        let mut checkpoints = Vec::new();
        if let Some(wt) = &worktree {
            checkpoints.push(git::checkpoint(wt, "会话开始")?);
        }

        self.next_id += 1;
        self.sessions.insert(
            id,
            Session {
                id,
                profile,
                dir: workdir,
                worktree,
                checkpoints,
                state: SessionState::Working,
                idle_re,
                pty,
            },
        );
        Ok(id)
    }

    fn get(&self, id: u32) -> Result<&Session> {
        self.sessions.get(&id).ok_or_else(|| anyhow::anyhow!("没有这个会话: {id}"))
    }

    fn get_mut(&mut self, id: u32) -> Result<&mut Session> {
        self.sessions.get_mut(&id).ok_or_else(|| anyhow::anyhow!("没有这个会话: {id}"))
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        let mut v: Vec<SessionInfo> = self
            .sessions
            .values()
            .map(|s| SessionInfo {
                id: s.id,
                profile: s.profile.name.clone(),
                dir: s.dir.display().to_string(),
                state: s.state,
            })
            .collect();
        v.sort_by_key(|s| s.id);
        v
    }

    /// 送内容给 agent。`text` 为空表示回车，也就是一轮的开始。
    /// **只有回车才打检查点**——逐字符输入不能每敲一下就产生一个提交。
    pub fn send_input(&mut self, id: u32, text: &str) -> Result<()> {
        let is_enter = text.is_empty();
        let s = self.get_mut(id)?;

        if is_enter {
            if let Some(wt) = &s.worktree {
                let sha = git::checkpoint(wt, "turn")?;
                if s.checkpoints.last() != Some(&sha) {
                    s.checkpoints.push(sha);
                }
            }
            s.state = SessionState::Working;
        }

        let payload = if is_enter { "\r" } else { text };
        s.pty.write(payload.as_bytes())?;
        Ok(())
    }

    pub fn screen(&self, id: u32) -> Result<String> {
        Ok(self.get(id)?.pty.screen_text())
    }

    pub fn stop(&mut self, id: u32) -> Result<()> {
        let s = self.get_mut(id)?;
        s.pty.kill()?;
        s.state = SessionState::Stopped;
        Ok(())
    }

    pub fn undo(&mut self, id: u32) -> Result<()> {
        let s = self.get(id)?;
        let wt = s.worktree.as_ref().ok_or_else(|| anyhow::anyhow!("shell 会话没有检查点"))?;
        let sha = s.checkpoints.last().ok_or_else(|| anyhow::anyhow!("还没有检查点"))?;
        git::reset_to(wt, sha)
    }

    pub fn diff(&self, id: u32) -> Result<Vec<FileStat>> {
        let s = self.get(id)?;
        let wt = s.worktree.as_ref().ok_or_else(|| anyhow::anyhow!("shell 会话没有 diff"))?;
        let base = s.checkpoints.last().ok_or_else(|| anyhow::anyhow!("还没有检查点"))?;
        git::diff_stat(wt, base)
    }

    /// 扫一遍所有会话，更新状态。由守护进程定时调用。
    pub fn tick(&mut self) {
        for s in self.sessions.values_mut() {
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
```

`src/main.rs` 加 `mod session;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test session -- --test-threads=1`
Expected: 7 个测试 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/
git commit -m "feat: 会话状态机与 SessionManager"
```

---

