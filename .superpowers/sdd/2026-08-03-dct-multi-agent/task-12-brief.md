## Task 12: n 直连上次的 agent，N 才进选择器

**Files:**
- Modify: `src/ui.rs:310-318`（看板按键）、`src/ui.rs:1040`（看板 `idle_help`）
- Test: `src/ui.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 8 的 `Request::LastProfile`、Task 10 的 `ProfileEntry`
- Produces: `pub fn quick_start_target(last: Option<&str>, entries: &[ProfileEntry]) -> Option<String>`

- [ ] **Step 1: 写失败的测试**

用到 Task 10 在同一个 `mod tests` 里建的 `entry(name, status)` 辅助。它不在的话先补上：

```rust
fn entry(name: &str, status: ProfileStatus) -> ProfileEntry {
    ProfileEntry {
        name: name.into(),
        label: name.into(),
        note: String::new(),
        status,
        secret: None,
        install: None,
    }
}
```

```rust
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
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib ui`
Expected: FAIL

- [ ] **Step 3: 实现**

```rust
/// `n` 该直接开哪个 agent。`None` = 没得直开，进选择器。
///
/// 目标用户是非程序员：让他每次在九个 agent 里挑一个是设计失败——他不知道区别。
/// 日常路径压成一个按键，想换的人按 N。
pub fn quick_start_target(last: Option<&str>, entries: &[ProfileEntry]) -> Option<String> {
    let last = last?;
    entries
        .iter()
        .find(|e| e.name == last && e.status == ProfileStatus::Ready)
        .map(|e| e.name.clone())
}
```

看板按键分支：

```rust
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        let Ok(Response::Profiles { entries, warning }) =
                            client.call(Request::Profiles)
                        else {
                            message = Msg::err("拿不到 agent 列表".into());
                            continue;
                        };
                        let last = match client.call(Request::LastProfile) {
                            Ok(Response::LastProfile(l)) => l,
                            _ => None,
                        };
                        // 小写 n 直连上次那个；大写 N 一定进选择器
                        let quick = if key.code == KeyCode::Char('n') {
                            quick_start_target(last.as_deref(), &entries)
                        } else {
                            None
                        };
                        match quick {
                            Some(name) => { /* Create + Attached，同 PickAction::Start */ }
                            None => {
                                let mut state = ListState::default();
                                state.select(Some(0));
                                view = View::PickProfile { entries, state, warning };
                            }
                        }
                    }
```

⚠️ `continue` 会跳过循环末尾的 `message_after_transition`。上面那个 `else` 分支里的 `continue` 是在**没切视图**的情况下设消息，跳过清理正好是我们要的（消息该留着），但要在注释里写明这不是疏忽。

看板 `idle_help` 改成：
`"n 新建  N 换 agent  p 换项目  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动"`

原来的 `q 退出` 由 `escape_hint` 单独占左段（`src/ui.rs:835`），不在这一行里。

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 5: 手动走一遍**

起 dct，`N` 选 Claude 建一个会话，Ctrl+Q 回看板，按 `n`——应当**直接进**一个新的 Claude 会话，不弹菜单。

- [ ] **Step 6: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/ui.rs
git commit -m "feat: n 直连上次的 agent，N 才进选择器

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

