### Task 2: `SessionInfo` 加 `tag` 字段

守护进程侧先把字段和连线做出来，值恒为空串 —— 界面这一步完全不动，行为零变化。

**Files:**
- Modify: `src/session.rs:123-144`（`SessionInfo`）、`src/session.rs:417-432`（`list()`）
- Modify（补字段的测试 fixture）：`src/ui/app.rs`、`src/ui/grid.rs`、`src/ui/view.rs`
- Test: `src/session.rs` 的 `mod tests`

**Interfaces:**
- Produces: `SessionInfo.tag: String` —— 空串 = 还没起出来，界面退回 `profile`

- [ ] **Step 1: 写失败测试**

加在 `src/session.rs` 的 `mod tests` 里：

```rust
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
```

- [ ] **Step 2: 跑它，确认它红**

```bash
cargo test --lib session::tests::session_info_without_a_tag_field_still_parses
```

预期：FAIL，编译错误 `no field 'tag' on type 'SessionInfo'`。

- [ ] **Step 3: 最小实现**

`src/session.rs`，`SessionInfo` 里 `is_agent` 之后加：

```rust
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
```

`Session` 结构体（`src/session.rs:146` 起，`explanation_slot` 旁边）加：

```rust
    /// 会话起名用的槽。跟 `explanation_slot` 平级、同一套用法。
    /// `None` = 还没触发过起名；`Some(_)` = 已经触发过（**只触发一次**）。
    name_slot: Arc<Mutex<Option<String>>>,
```

构造处（`src/session.rs:375` 附近，`explanation_slot` 那一行旁边）：

```rust
            name_slot: Arc::new(Mutex::new(None)),
```

`list()` 里（`src/session.rs:421` 的 `SessionInfo { .. }`）加一行：

```rust
                    tag: recover(s.name_slot.lock()).clone().unwrap_or_default(),
```

- [ ] **Step 4: 补齐测试 fixture**

`cargo test` 会点名所有构造 `SessionInfo` 的地方。已知三处，各加 `tag: String::new(),`：
`src/ui/app.rs` 的 `fn sess`、`src/ui/grid.rs:848` 和 `:859` 的 fixture、`src/ui/view.rs:1360`。
编译器报到哪补到哪，不要漏。

- [ ] **Step 5: 跑测试**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

预期：全绿。

- [ ] **Step 6: 确认协议号真没动**

```bash
grep -n "PROTOCOL_VERSION: u32" src/proto.rs
```

预期：`pub const PROTOCOL_VERSION: u32 = 6;`

- [ ] **Step 7: 提交**

```bash
git add -A
git commit -m "feat: carry a per-session name on the wire, empty for now

serde(default) keeps this off the protocol version: an old daemon's JSON
parses into an empty tag, and a new daemon's extra field is ignored by an
old UI. Bumping the version would have been expensive here, because ps,
stop, kill and prune never joined the version handshake and would answer a
mismatch with raw serde text."
```

---

