## Task 8: 协议与守护进程

**Files:**
- Modify: `src/proto.rs`
- Modify: `src/daemon.rs`
- Modify: `src/projects.rs`（存「上次用的 agent」）
- Modify: `src/ui.rs:315`、`src/ui.rs:385`（跟上编译）
- Modify: `tests/concurrency.rs:76`、`tests/daemon_roundtrip.rs:30`、`tests/projects_flow.rs:46,73`（`Create` 多了一个字段）
- Test: `src/projects.rs` 的 `mod tests`；新建 `tests/profiles_flow.rs`

**Interfaces:**
- Consumes: Task 3、4、7
- Produces:
  ```rust
  // proto.rs
  pub struct SecretPrompt { pub hint: String, pub url: Option<String> }
  pub struct InstallPrompt { pub command: Vec<String>, pub note: String }
  pub struct ProfileEntry {
      pub name: String,
      pub label: String,
      pub note: String,
      pub status: ProfileStatus,
      pub secret: Option<SecretPrompt>,
      pub install: Option<InstallPrompt>,
  }
  Request::Create { dir: String, profile: String, remember: bool }
  Request::SetSecret { profile: String, value: String }
  Request::DeleteSecret { profile: String }
  Request::LastProfile
  Response::Profiles { entries: Vec<ProfileEntry>, warning: Option<String> }
  Response::LastProfile(Option<String>)
  // projects.rs
  Store::last_profile(&self) -> Option<&str>
  Store::set_last_profile(&mut self, name: &str)
  ```
- **`create` 的签名在本任务再变一次**：Task 5 定的是 `create(&self, dir, profile_name, secrets)`，这里加上磁盘 profile 变成 `create(&self, dir, profile_name, secrets, profiles: &[Profile])`。两次改动分开是因为 Task 5 只关心环境变量，磁盘 profile 是本任务才引入的。

`Request::VerifySecret` 在 Task 9 加，这里不做。

- [ ] **Step 1: 写失败的测试**

`src/projects.rs`：

```rust
#[test]
fn last_profile_survives_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("projects.json");
    let mut s = Store::load(&f);
    s.set_last_profile("kimi");
    drop(s);
    assert_eq!(Store::load(&f).last_profile(), Some("kimi"));
}

#[test]
fn old_file_without_last_profile_still_loads() {
    // 已经在用 dct 的人，projects.json 里没有这个字段
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("projects.json");
    std::fs::write(&f, r#"{"recent":["/a"]}"#).unwrap();
    let s = Store::load(&f);
    assert_eq!(s.list(), vec!["/a".to_string()]);
    assert_eq!(s.last_profile(), None);
}
```

新建 `tests/profiles_flow.rs`（照 `tests/projects_flow.rs` 的骨架起守护进程）：

```rust
//! Profiles / 密钥 / 上次用的 agent 走一遍真 socket。

use dct::profile::ProfileStatus;
use dct::proto::{Request, Response};

mod common;

#[test]
fn profiles_returns_entries_with_labels_and_status() {
    let h = common::start_daemon();
    let mut c = h.client();

    let Response::Profiles { entries, warning } = c.call(Request::Profiles).unwrap() else {
        panic!("应当返回 Profiles");
    };
    assert!(warning.is_none(), "干净环境不该有告警");
    assert_eq!(entries.len(), 9);
    assert_eq!(entries[0].name, "claude");
    assert_eq!(entries[0].label, "Claude", "要带中文 label");
    let shell = entries.iter().find(|e| e.name == "shell").unwrap();
    assert_eq!(shell.status, ProfileStatus::Ready, "/bin/zsh 一定在");
    let kimi = entries.iter().find(|e| e.name == "kimi").unwrap();
    assert!(
        kimi.secret.is_some(),
        "需要密钥的条目要把 hint / url 一起带过来，UI 才画得出输入界面"
    );
}

#[test]
fn set_secret_flips_kimi_off_needs_secret() {
    let h = common::start_daemon();
    let mut c = h.client();

    c.call(Request::SetSecret {
        profile: "kimi".into(),
        value: "sk-test".into(),
    })
    .unwrap();

    let Response::Profiles { entries, .. } = c.call(Request::Profiles).unwrap() else {
        panic!()
    };
    let kimi = entries.iter().find(|e| e.name == "kimi").unwrap();
    assert_ne!(
        kimi.status,
        ProfileStatus::NeedsSecret,
        "填了密钥就不该再报缺密钥"
    );
}

#[test]
fn delete_secret_puts_it_back() {
    let h = common::start_daemon();
    let mut c = h.client();
    c.call(Request::SetSecret {
        profile: "kimi".into(),
        value: "sk-test".into(),
    })
    .unwrap();
    c.call(Request::DeleteSecret {
        profile: "kimi".into(),
    })
    .unwrap();

    let Response::Profiles { entries, .. } = c.call(Request::Profiles).unwrap() else {
        panic!()
    };
    let kimi = entries.iter().find(|e| e.name == "kimi").unwrap();
    // claude 装没装取决于跑测试的机器，两种都算对——重点是密钥没了
    assert!(matches!(
        kimi.status,
        ProfileStatus::NeedsSecret | ProfileStatus::NeedsDependency { .. }
    ));
}

#[test]
fn create_with_remember_records_the_profile() {
    let h = common::start_daemon();
    let mut c = h.client();
    let dir = h.git_repo("proj");

    c.call(Request::Create {
        dir: dir.display().to_string(),
        profile: "shell".into(),
        remember: true,
    })
    .unwrap();

    assert!(matches!(
        c.call(Request::LastProfile).unwrap(),
        Response::LastProfile(Some(ref n)) if n == "shell"
    ));
}

#[test]
fn create_without_remember_does_not_record() {
    // 「帮你装 CLI」开的那个 shell 会话不能变成「上次用的 agent」——
    // 否则用户下次按 n 会直接掉进一个命令行。
    let h = common::start_daemon();
    let mut c = h.client();
    let dir = h.git_repo("proj");

    c.call(Request::Create {
        dir: dir.display().to_string(),
        profile: "shell".into(),
        remember: false,
    })
    .unwrap();

    assert!(matches!(
        c.call(Request::LastProfile).unwrap(),
        Response::LastProfile(None)
    ));
}
```

`mod common` 里的 `start_daemon()` / `client()` / `git_repo()`：`tests/projects_flow.rs` 里已经有等价的起守护进程代码，把它抽到 `tests/common/mod.rs` 供两个文件共用。抽的时候保持 `projects_flow.rs` 的行为不变，它的测试必须照样过。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test`
Expected: 编译失败

- [ ] **Step 3: 改 proto.rs**

```rust
use crate::profile::ProfileStatus;

/// 需要密钥时，UI 画输入界面要用的东西。
///
/// 只带**已经取好语言**的字符串，不把 `LocalizedText` 送过线：
/// 组句发生在哪一侧必须一致（见设计文档「与 i18n 的关系」）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretPrompt {
    pub hint: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPrompt {
    pub command: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub name: String,
    pub label: String,
    pub note: String,
    pub status: ProfileStatus,
    pub secret: Option<SecretPrompt>,
    pub install: Option<InstallPrompt>,
}
```

`Request` 加：

```rust
    Create { dir: String, profile: String, remember: bool },
    SetSecret { profile: String, value: String },
    DeleteSecret { profile: String },
    LastProfile,
```

`Response` 改 / 加：

```rust
    Profiles {
        entries: Vec<ProfileEntry>,
        /// 密钥文件读不了、自定义 profile 写错了之类。UI 顶部红字。
        warning: Option<String>,
    },
    LastProfile(Option<String>),
```

- [ ] **Step 4: 改 projects.rs**

`Disk` 加字段，`Store` 加字段与两个方法：

```rust
#[derive(Default, Serialize, Deserialize)]
struct Disk {
    #[serde(default)]
    recent: Vec<String>,
    /// 上次开会话用的 agent。`n` 键直连它。
    #[serde(default)]
    last_profile: Option<String>,
}
```

```rust
    pub fn last_profile(&self) -> Option<&str> {
        self.last_profile.as_deref()
    }

    pub fn set_last_profile(&mut self, name: &str) {
        self.last_profile = Some(name.to_string());
        self.save();
    }
```

`load()` 和 `save()` 里把新字段带上。

- [ ] **Step 5: 改 daemon.rs**

`run_with_manager` 里，`store` 旁边加密钥仓：

```rust
    let secrets = Arc::new(Mutex::new(SecretStore::load(&secrets_path_for_socket(socket))));
    let profiles_dir = profiles_dir_for_socket(socket);
```

两者都要传进 `serve` / `handle`。`handle` 的新分支：

```rust
        Request::Profiles => {
            let (all, mut warnings) = all_profiles(profiles_dir);
            let sec = recover(secrets.lock());
            if let Some(e) = sec.load_error() {
                // 密钥文件读不了要顶到界面上。静默的话用户会以为密钥丢了，
                // 而且这时候所有写入都被拒，他改什么都没反应。
                warnings.insert(0, format!("密钥文件读不了：{e}"));
            }
            let entries = all
                .iter()
                .map(|p| ProfileEntry {
                    name: p.name.clone(),
                    label: p.display_label(Lang::Zh),
                    note: p.display_note(Lang::Zh),
                    status: status_of(
                        p,
                        &all,
                        sec.get(&p.name).is_some(),
                        &command_exists,
                        Lang::Zh,
                    ),
                    secret: p.secret.as_ref().map(|s| SecretPrompt {
                        hint: s.hint.get(Lang::Zh).unwrap_or("").to_string(),
                        url: s.url.clone(),
                    }),
                    install: p.install.as_ref().map(|i| InstallPrompt {
                        command: i.command.clone(),
                        note: i.note.get(Lang::Zh).unwrap_or("").to_string(),
                    }),
                })
                .collect();
            Ok(Response::Profiles {
                entries,
                warning: if warnings.is_empty() {
                    None
                } else {
                    Some(warnings.join("；"))
                },
            })
        }
        Request::Create { dir, profile, remember } => {
            let dir = PathBuf::from(dir);
            let sec = recover(secrets.lock());
            let r = mgr
                .create(&dir, &profile, &sec)
                .map(|id| Response::Created { id });
            drop(sec);
            if r.is_ok() {
                let mut st = recover(store.lock());
                st.touch(&dir);
                // remember=false 是「帮你装 CLI」那条路径：它开的 shell 会话
                // 不是用户选的 agent，记了下次按 n 会掉进命令行
                if remember {
                    st.set_last_profile(&profile);
                }
            }
            r
        }
        Request::SetSecret { profile, value } => recover(secrets.lock())
            .set(&profile, &value)
            .map(|_| Response::Ok),
        Request::DeleteSecret { profile } => recover(secrets.lock())
            .remove(&profile)
            .map(|_| Response::Ok),
        Request::LastProfile => Ok(Response::LastProfile(
            recover(store.lock()).last_profile().map(str::to_string),
        )),
```

`SessionManager::resolve_profile` 也要认磁盘 profile。最省事的做法是在 `create` 之前，由 daemon 把磁盘 profile 用现有的 `register_profile` 灌进去；但那会在 manager 里越攒越多。改成给 `create` 多传一个 `profiles: &[Profile]`，`resolve_profile` 先查这个切片、再查 `extra_profiles`（测试入口）、最后查内置。

- [ ] **Step 6: 改调用点**

`src/ui.rs:315` 的 `Response::Profiles(p)` 改成解构新形状（这一步只求编译过，UI 的正式改造在 Task 10）；`src/ui.rs:385` 的 `Request::Create` 加 `remember: true`。四个集成测试的 `Create` 同样加 `remember: true`。

- [ ] **Step 7: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 8: 提交**

```bash
~/.cargo/bin/cargo fmt
git add -A
git commit -m "feat: 协议带上 profile 状态与密钥提示；记住上次用的 agent

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

