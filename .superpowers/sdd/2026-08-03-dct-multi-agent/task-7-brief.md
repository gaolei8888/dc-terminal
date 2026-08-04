## Task 7: 可用性判定

**Files:**
- Modify: `src/profile.rs`
- Test: `src/profile.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 1、2、3
- Produces:
  ```rust
  pub enum ProfileStatus {
      Ready,
      NeedsSecret,
      NeedsDependency { label: String },
      NotInstalled { command: String },
  }
  pub fn status_of(
      p: &Profile,
      all: &[Profile],
      has_secret: bool,
      installed: &dyn Fn(&str) -> bool,
      lang: Lang,
  ) -> ProfileStatus;
  pub fn command_exists(cmd: &str) -> bool;
  ```

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
fn status_fixture() -> Vec<Profile> {
    Profile::builtins()
}

#[test]
fn ready_when_installed_and_secret_present() {
    let all = status_fixture();
    let kimi = all.iter().find(|p| p.name == "kimi").unwrap();
    let st = status_of(kimi, &all, true, &|_| true, Lang::Zh);
    assert!(matches!(st, ProfileStatus::Ready));
}

#[test]
fn needs_secret_when_installed_but_no_key() {
    let all = status_fixture();
    let kimi = all.iter().find(|p| p.name == "kimi").unwrap();
    let st = status_of(kimi, &all, false, &|_| true, Lang::Zh);
    assert!(matches!(st, ProfileStatus::NeedsSecret));
}

#[test]
fn not_installed_when_the_command_owns_its_name() {
    let all = status_fixture();
    let codex = all.iter().find(|p| p.name == "codex").unwrap();
    let st = status_of(codex, &all, false, &|_| false, Lang::Zh);
    match st {
        ProfileStatus::NotInstalled { command } => assert_eq!(command, "codex"),
        other => panic!("codex 自己就是那个命令，应当报未安装，得到 {other:?}"),
    }
}

#[test]
fn dependency_is_reported_before_secret() {
    // 这条顺序是整个判定里最要紧的：kimi 跑的是 claude。claude 没装时
    // 如果先报「未填密钥」，用户会去填 key，填完还是起不来，
    // 然后以为是 key 的问题——被送进死胡同。
    let all = status_fixture();
    let kimi = all.iter().find(|p| p.name == "kimi").unwrap();
    let st = status_of(kimi, &all, false, &|_| false, Lang::Zh);
    match st {
        ProfileStatus::NeedsDependency { label } => assert_eq!(label, "Claude"),
        other => panic!("claude 没装时 kimi 要报依赖，不是密钥，得到 {other:?}"),
    }
}

#[test]
fn dependency_uses_the_owner_profiles_label_not_the_raw_command() {
    let all = status_fixture();
    let glm = all.iter().find(|p| p.name == "glm").unwrap();
    let st = status_of(glm, &all, true, &|c| c != "claude", Lang::Zh);
    match st {
        ProfileStatus::NeedsDependency { label } => {
            assert_eq!(label, "Claude", "给用户看 label，不是二进制名");
        }
        other => panic!("得到 {other:?}"),
    }
}

#[test]
fn profile_without_secret_is_ready_when_installed() {
    let all = status_fixture();
    let shell = all.iter().find(|p| p.name == "shell").unwrap();
    assert!(matches!(
        status_of(shell, &all, false, &|_| true, Lang::Zh),
        ProfileStatus::Ready
    ));
}

#[test]
fn command_exists_finds_sh_and_not_a_made_up_name() {
    assert!(command_exists("sh"), "PATH 上一定有 sh");
    assert!(!command_exists("dct-绝对没有这个命令-x9"));
}

#[test]
fn command_exists_handles_absolute_paths() {
    assert!(command_exists("/bin/sh"));
    assert!(!command_exists("/bin/根本没有这个"));
}
```

`ProfileStatus` 要 `#[derive(Debug)]`，否则 `panic!("{other:?}")` 编译不过。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib profile`
Expected: FAIL，`cannot find function 'status_of'`

- [ ] **Step 3: 实现**

加到 `src/profile.rs`：

```rust
/// 这个 profile 现在能不能用，不能的话卡在哪。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProfileStatus {
    Ready,
    /// 声明了 secret 但密钥仓里没有
    NeedsSecret,
    /// 跑的是别的 profile 的命令，而那个命令没装。`label` 是那个 profile 的显示名。
    NeedsDependency { label: String },
    /// `command[0]` 在 PATH 上找不到，而且这个命令就是它自己
    NotInstalled { command: String },
}

/// `cmd` 能不能执行。带斜杠当路径查，否则遍历 PATH。
///
/// **这个判断必须和实际 spawn 用同一个环境**，所以只能在守护进程里调用——
/// 界面进程的 PATH 可能不一样，那会导致「菜单说能用，一开就失败」。
pub fn command_exists(cmd: &str) -> bool {
    fn is_exec(p: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    if cmd.contains('/') {
        return is_exec(Path::new(cmd));
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':')
        .filter(|d| !d.is_empty())
        .any(|d| is_exec(&Path::new(d).join(cmd)))
}

/// `command[0]` 这个命令「归谁所有」——名字和命令名相同的那个 profile。
///
/// kimi/glm/deepseek/qwen-api 的 command[0] 都是 `claude`，归 `claude` 这个
/// profile 所有；`claude` 自己的名字就是 `claude`，所以它是自己的 owner。
/// 靠这个区分「我没装」和「我依赖的东西没装」。
fn dependency_owner<'a>(all: &'a [Profile], cmd: &str) -> Option<&'a Profile> {
    let base = Path::new(cmd)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| cmd.to_string());
    all.iter().find(|p| p.name == base)
}

pub fn status_of(
    p: &Profile,
    all: &[Profile],
    has_secret: bool,
    installed: &dyn Fn(&str) -> bool,
    lang: Lang,
) -> ProfileStatus {
    let Some(cmd) = p.command.first() else {
        // 解析层允许空 command（TOML 里写了 `command = []`），这里兜住，
        // 免得 spawn 的时候 panic
        return ProfileStatus::NotInstalled {
            command: String::new(),
        };
    };

    // 顺序不能换：装没装排在密钥前面。见测试
    // `dependency_is_reported_before_secret` 的注释。
    if !installed(cmd) {
        return match dependency_owner(all, cmd) {
            Some(owner) if owner.name != p.name => ProfileStatus::NeedsDependency {
                label: owner.display_label(lang),
            },
            _ => ProfileStatus::NotInstalled {
                command: cmd.clone(),
            },
        };
    }

    if p.secret.is_some() && !has_secret {
        return ProfileStatus::NeedsSecret;
    }

    ProfileStatus::Ready
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib profile`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/profile.rs
git commit -m "feat: profile 可用性判定，依赖缺失优先于密钥缺失

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

