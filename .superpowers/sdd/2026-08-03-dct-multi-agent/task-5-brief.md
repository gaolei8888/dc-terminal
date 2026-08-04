## Task 5: 环境变量注入到 PTY

**Files:**
- Modify: `src/pty.rs:40-106`（`spawn` 签名）、`src/pty.rs:280-320`（现有测试的调用）
- Modify: `src/session.rs:100-143`（`create`）
- Test: `src/pty.rs`、`src/session.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 1 的 `Profile.env`、Task 4 的 `SecretStore`
- Produces: `PtySession::spawn(cmd: &[String], env: &BTreeMap<String, String>, cwd: &Path, rows: u16, cols: u16) -> Result<PtySession>`；`SessionManager::create(&self, dir: &Path, profile_name: &str, secrets: &SecretStore) -> Result<u32>`

- [ ] **Step 1: 写失败的测试**

加到 `src/pty.rs` 的 `mod tests`：

```rust
#[test]
fn spawn_passes_env_to_the_child() {
    use std::collections::BTreeMap;
    let dir = tempfile::tempdir().unwrap();
    let mut env = BTreeMap::new();
    env.insert("DCT_TEST_MARKER".to_string(), "看得见我".to_string());

    let p = PtySession::spawn(
        &["/bin/sh".to_string(), "-c".to_string(),
          "echo $DCT_TEST_MARKER; sleep 5".to_string()],
        &env,
        dir.path(),
        24,
        80,
    )
    .unwrap();

    assert!(
        wait_for(&p, "看得见我"),
        "profile 里的 env 必须传给子进程，否则换 base_url 的 agent 全起不来"
    );
}
```

`wait_for` 是 `src/pty.rs` 测试里已有的辅助（见 `src/pty.rs:272` 附近）；如果它的签名对不上，照现有测试的等待写法自己等。

加到 `src/session.rs` 的 `mod tests`：

```rust
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
        assert!(std::time::Instant::now() < deadline, "没看到注入的环境变量：{text}");
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
```

`screen_text_for_test(id)` 如果不存在就加一个 `#[cfg(test)]` 的小方法，内容是取会话的 `pty.screen_text()`。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib`
Expected: 编译失败，`spawn` 参数个数不对

- [ ] **Step 3: 改 `PtySession::spawn`**

`src/pty.rs`，签名与 builder 部分：

```rust
    pub fn spawn(
        cmd: &[String],
        env: &std::collections::BTreeMap<String, String>,
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> Result<PtySession> {
```

在 `builder.cwd(cwd);` 后面加：

```rust
        // 只加不减：不清空继承来的环境。ANTHROPIC_BASE_URL 这类是覆盖上去的，
        // 但 PATH / HOME / 各家 CLI 自己的登录态都得留着，清了 agent 就起不来。
        for (k, v) in env {
            builder.env(k, v);
        }
```

`src/pty.rs` 里现有的四处 `PtySession::spawn(...)` 测试调用，第二个参数传 `&Default::default()`。

- [ ] **Step 4: 改 `SessionManager::create`**

`src/session.rs`：

```rust
    pub fn create(&self, dir: &Path, profile_name: &str, secrets: &SecretStore) -> Result<u32> {
        let profile = self.resolve_profile(profile_name)?;
        // ...（目录检查、id 分配、git 检查照旧）...

        let idle_re = profile.idle_regex()?;
        let busy_re = profile.busy_regex()?;
        let is_agent = profile.is_agent;

        // profile 的静态 env 打底，密钥覆盖上去。密钥不在 profile 文件里，
        // 只在这一步才和命令合到一起——profile 文件因此可以随便拷贝分享。
        let mut env = profile.env.clone();
        if let Some(spec) = &profile.secret {
            if let Some(key) = secrets.get(&profile.name) {
                env.insert(spec.env.clone(), key.to_string());
            }
        }

        let pty = PtySession::spawn(&profile.command, &env, dir, 40, 120)?;
        // ...
    }
```

`Session` 结构体加 `busy_re: Option<regex::Regex>` 字段并在构造时填上（`tick()` 下个任务才用）。

`src/daemon.rs` 里 `Request::Create` 的调用点跟着改（密钥仓在 Task 8 接线，这一步先传一个从 `secrets_path_for_socket` load 出来的 `SecretStore`，和 `store` 一样放进 `Arc<Mutex<_>>`）。

- [ ] **Step 5: 让起不来的命令说人话**

写测试：

```rust
#[test]
fn spawn_failure_says_what_to_do_not_enoent() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();

    let mgr = SessionManager::new();
    mgr.register_profile(
        Profile::from_toml(
            "name = \"gone\"\ncommand = [\"/绝对不存在/x9\"]\n",
        )
        .unwrap(),
    );
    let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
    let err = mgr.create(&proj, "gone", &secrets).unwrap_err().to_string();

    assert!(err.contains("启动不了"), "要说人话：{err}");
    assert!(!err.to_lowercase().contains("enoent"), "别把系统错误码甩给用户：{err}");
}
```

`src/pty.rs` 的 spawn 错误上下文（现在是 `format!("启动 {} 失败", cmd[0])`）改成：

```rust
            .with_context(|| {
                // 用户看得懂的话。命令确实在 PATH 上但起不来（权限不对、
                // 架构不匹配、脚本头写错），底层错误对非程序员没有意义。
                format!("启动不了 {}，它可能装坏了", cmd[0])
            })?;
```

⚠️ anyhow 默认会把整条 source 链打出来，`ENOENT` 还是会露。守护进程往 `Response::Error` 里塞的时候只取最外层：`format!("{e}")` 而不是 `format!("{e:#}")`。确认 `src/daemon.rs` 现有的错误转换用的是前者。

- [ ] **Step 6: 跑全部测试**

Run: `~/.cargo/bin/cargo test`
Expected: PASS。集成测试 `tests/*.rs` 不调用 `spawn`/`create`，只走协议，应当不受影响。

- [ ] **Step 7: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/pty.rs src/session.rs src/daemon.rs
git commit -m "feat: profile 的 env 与密钥注入子进程

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

