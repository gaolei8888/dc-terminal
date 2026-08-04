## Task 13（二期）: 密钥设置页

**Files:**
- Modify: `src/ui.rs`（`View`、看板 `c` 键、按键循环、渲染、`escape_hint`、`back_one_level`、`idle_help`）
- Test: `src/ui.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 8 的 `Request::DeleteSecret`、Task 11 的 `View::EnterSecret`
- Produces: `View::Secrets { entries: Vec<ProfileEntry>, state: ListState }`、`pub fn secret_rows(entries: &[ProfileEntry]) -> Vec<(String, bool)>`（label 与「已配没配」）

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn secret_rows_only_lists_profiles_that_need_a_key() {
    let entries = vec![
        entry("claude", ProfileStatus::Ready),       // 不需要密钥
        with_secret(entry("kimi", ProfileStatus::Ready)),
        with_secret(entry("glm", ProfileStatus::NeedsSecret)),
    ];
    let rows = secret_rows(&entries);
    assert_eq!(rows.len(), 2, "claude 不该出现在密钥页");
    assert_eq!(rows[0], ("kimi".to_string(), true), "Ready 说明密钥已配");
    assert_eq!(rows[1], ("glm".to_string(), false));
}

#[test]
fn secrets_view_escapes_to_the_board() {
    assert!(matches!(
        back_one_level(View::Secrets {
            entries: vec![],
            state: ListState::default(),
        }),
        Some(View::Board)
    ));
}

#[test]
fn board_help_mentions_the_settings_key() {
    assert!(idle_help(&View::Board).contains("c 密钥"));
}
```

`with_secret` 是测试辅助：给 `ProfileEntry` 填一个 `SecretPrompt`。

⚠️ `secret_rows` 用 `status != NeedsSecret` 判断「已配」是有边界的：`NeedsDependency` 时密钥可能配了也可能没配，这个状态压过了密钥状态。二期实现时如果这个区分要紧，就在 `ProfileEntry` 上加一个 `has_secret: bool` 字段，别用状态反推。测试里只覆盖 `Ready` 和 `NeedsSecret` 两种就是因为这个。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib ui`
Expected: FAIL

- [ ] **Step 3: 实现**

纯函数：

```rust
/// 密钥页要列哪些行。只列声明了密钥的 profile——claude / codex / 命令行
/// 出现在这一页只会让用户以为它们也要配。
pub fn secret_rows(entries: &[ProfileEntry]) -> Vec<(String, bool)> {
    entries
        .iter()
        .filter(|e| e.secret.is_some())
        .map(|e| (e.name.clone(), e.status != ProfileStatus::NeedsSecret))
        .collect()
}
```

`View` 加：

```rust
    Secrets {
        entries: Vec<ProfileEntry>,
        state: ListState,
    },
```

`View::EnterSecret` 加一个字段，**成功后去哪不能靠猜**：

```rust
        /// 从设置页进来的要回设置页（意图是改配置），从选择器进来的直接开会话
        /// （意图是开工）。
        return_to_settings: bool,
```

按键：看板 `c` → 拉 `Request::Profiles` 进 `Secrets`；`↑↓` 移动；`Enter` → `View::EnterSecret { return_to_settings: true, .. }`；`d` → `Request::DeleteSecret` 后重拉列表；`Esc` / `Ctrl+Q` → 看板。

渲染每行：

```rust
                    let (name, configured) = row;
                    let label = entries.iter()
                        .find(|e| &e.name == name)
                        .map(|e| e.label.clone())
                        .unwrap_or_else(|| name.clone());
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{:<14}", truncate(&label, 14))),
                        Span::styled(
                            if *configured { "已配" } else { "未配" },
                            Style::default().fg(if *configured {
                                Color::Green
                            } else {
                                Color::DarkGray
                            }),
                        ),
                    ]))
```

`escape_hint` 加 `View::Secrets { .. } => "Ctrl+Q 回看板"`（`_ =>` 那支已经覆盖，确认一下即可）。
`idle_help` 加 `View::Secrets { .. } => "↑↓ 选  Enter 改  d 删  Esc 返回"`，看板那行加 `c 密钥`。

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/ui.rs
git commit -m "feat: 密钥设置页，看板按 c 进，可改可删

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## 收尾

- [ ] `~/.cargo/bin/cargo test` 全绿
- [ ] `~/.cargo/bin/cargo clippy -- -D warnings` 干净
- [ ] `git diff --check` 没有行尾空白
- [ ] 更新 `README.md`：九个 agent、`~/.dct/profiles/` 自定义、`n`/`N`/`c` 三个键
- [ ] 回头核对设计文档的「未实测项」表——Task 2 Step 6 实跑出来的结果要落回去，别让下一个人再猜一遍
