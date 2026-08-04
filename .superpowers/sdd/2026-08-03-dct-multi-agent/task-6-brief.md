## Task 6: busy_pattern 与 SessionState::Unknown

**Files:**
- Modify: `src/session.rs:16-23`（枚举）、`src/session.rs:129-138`（初始状态）、`src/session.rs:265-286`（`tick`）
- Modify: `src/ui.rs:20-36`（`status_label` / `status_color`）
- Test: `src/session.rs`、`src/ui.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 5 的 `Session.busy_re`
- Produces: `SessionState::Unknown`；`status_label(SessionState::Unknown) == "—"`

- [ ] **Step 1: 写失败的测试**

`src/session.rs`：

```rust
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
    let id = mgr.create(&proj, "busy-demo", &secrets).unwrap();

    // 屏幕上有 busy 串 → 干活中
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        mgr.tick();
        if state_of(&mgr, id) == SessionState::Working {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "busy 串在屏上就该是 Working");
        sleep(Duration::from_millis(50));
    }

    // 串消失 → 空闲
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        mgr.tick();
        if state_of(&mgr, id) == SessionState::Idle {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "busy 串没了就该是 Idle");
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn busy_pattern_wins_over_idle_pattern() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();

    let mgr = SessionManager::new();
    // 两个 pattern 同时命中。busy 优先 → Working。
    mgr.register_profile(
        Profile::from_toml(
            r#"
            name = "both"
            command = ["/bin/sh", "-c", "echo BUSY IDLE; sleep 5"]
            is_agent = false
            busy_pattern = "BUSY"
            idle_pattern = "IDLE"
            "#,
        )
        .unwrap(),
    );
    let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
    let id = mgr.create(&proj, "both", &secrets).unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        mgr.tick();
        if state_of(&mgr, id) == SessionState::Working {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
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
    let id = mgr.create(&proj, "quiet", &secrets).unwrap();

    assert_eq!(state_of(&mgr, id), SessionState::Unknown, "没 pattern 就别编状态");
    for _ in 0..5 {
        mgr.tick();
        sleep(Duration::from_millis(20));
    }
    assert_eq!(state_of(&mgr, id), SessionState::Unknown, "tick 也不该把它改成 Working");
}
```

`state_of` 是个测试辅助：`fn state_of(mgr: &SessionManager, id: u32) -> SessionState { mgr.list().into_iter().find(|s| s.id == id).unwrap().state }`。如果 `src/session.rs` 的测试里已有等价写法就复用。

`src/ui.rs`：

```rust
#[test]
fn unknown_state_shows_a_dash() {
    assert_eq!(status_label(SessionState::Unknown), "—");
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib`
Expected: FAIL，`no variant named 'Unknown'`

- [ ] **Step 3: 实现**

`src/session.rs` 枚举加一个变体：

```rust
pub enum SessionState {
    Working,
    /// 由后续的 Bridge 在 agent 调用 ask_human 时设置；本计划内不会出现
    Asking,
    Idle,
    Stopped,
    /// profile 没给任何 pattern，我们不知道它在干什么。
    /// 显示「—」而不是猜一个——`shell` 以前就是被猜成「干活中」的。
    Unknown,
}
```

`create()` 里的初始状态：

```rust
        // 有 pattern 才敢说「干活中」：agent 刚起来确实在初始化。
        // 没 pattern 就一直是 Unknown，tick 也不会改它。
        let state = if idle_re.is_some() || busy_re.is_some() {
            SessionState::Working
        } else {
            SessionState::Unknown
        };
```

`tick()` 里替换判定：

```rust
            // busy 优先：agent 干活时的「按 esc 中断」提示是稳定的，
            // 而空闲时的输入框占位符用户一打字就没了。
            if let Some(re) = &s.busy_re {
                s.state = if re.is_match(&s.pty.screen_text()) {
                    SessionState::Working
                } else {
                    SessionState::Idle
                };
            } else if let Some(re) = &s.idle_re {
                s.state = if re.is_match(&s.pty.screen_text()) {
                    SessionState::Idle
                } else {
                    SessionState::Working
                };
            }
            // 两个都没有：状态不动，保持 Unknown
```

`src/ui.rs`：

```rust
pub fn status_label(s: SessionState) -> &'static str {
    match s {
        SessionState::Working => "干活中",
        SessionState::Asking => "等你回答",
        SessionState::Idle => "空闲",
        SessionState::Stopped => "已停止",
        SessionState::Unknown => "—",
    }
}

pub fn status_color(s: SessionState) -> Color {
    match s {
        SessionState::Working => Color::Cyan,
        SessionState::Asking => Color::Yellow,
        SessionState::Idle => Color::Green,
        SessionState::Stopped => Color::DarkGray,
        SessionState::Unknown => Color::DarkGray,
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/session.rs src/ui.rs
git commit -m "feat: busy_pattern 判定状态；没 pattern 的会话显示「—」不再假装干活中

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

