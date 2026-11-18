### Task 2: 记住每个会话的对话 id，重启后各接各的

**Files:**
- Create: `src/conversation_id.rs`（在 `src/lib.rs` 里 `pub mod conversation_id;`）
- Modify: `src/last_sessions.rs`（`RecordedSession`，:40 附近）
- Modify: `src/session.rs`（`struct Session` :476、`create_inner` :934 起、`persist_last_sessions` :822）
- Modify: `src/daemon.rs`（`restore_last_sessions` :739 起）

**Interfaces:**
- Consumes: Task 1 的 `fill_id`、`resumes_by_id`、两个新字段。
- Produces:
  - `conversation_id::new() -> String`：UUID v4，小写、带连字符、36 个字符。
  - `RecordedSession` 新字段（都是 `#[serde(default)]`）：`conversation_id: Option<String>`、`interrupted: bool`、`handoff: Option<PathBuf>`
  - `SessionManager::create_resuming(dir, profile, secret, profiles, resume: bool, restore_tag: Option<&str>, conversation: Option<&str>) -> Result<u32>`（**多一个参数**，所有调用点同步改）
  - `SessionManager::conversation_of(id: u32) -> Option<String>`（测试和 Task 6 用）
  - `SessionManager::persist_last_sessions_with(&self, interrupted: &HashSet<u32>, handoffs: &HashMap<u32, PathBuf>)`：`persist_last_sessions()` 改成调用它并传两个空集合。

- [ ] **Step 1: 写失败的测试**

`src/conversation_id.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn looks_like_uuid_v4_and_differs() {
        let a = new();
        let b = new();
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
        assert!(a.chars().all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b);
    }
}
```

`src/last_sessions.rs` 测试模块：

```rust
#[test]
fn old_file_without_new_fields_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("last-sessions.toml");
    std::fs::write(&p, "[[sessions]]\ndir = \"/tmp\"\nprofile = \"claude\"\ntag = \"\"\nlast_active = 5\n").unwrap();
    let v = load(&p);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].conversation_id, None);
    assert!(!v[0].interrupted);
    assert_eq!(v[0].handoff, None);
}

#[test]
fn new_fields_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("last-sessions.toml");
    let r = RecordedSession {
        dir: "/tmp".into(), profile: "claude".into(), tag: "t".into(), last_active: 1,
        conversation_id: Some("11111111-1111-4111-8111-111111111111".into()),
        interrupted: true,
        handoff: Some("/tmp/h.md".into()),
    };
    save(&p, &[r.clone()]).unwrap();
    assert_eq!(load(&p), vec![r]);
}
```

（`tempfile` 是否在 dev-dependencies 以 `Cargo.toml` 为准；不在就照这个文件已有测试建临时目录的办法。）

`src/session.rs` 测试模块——用一个测试专用 profile（`register_profile`，命令是 `/bin/sh -c 'echo "$@"; sleep 30' sh`，这样参数会被打印到屏幕上，可以从屏幕断言拼出来的命令）：

```rust
#[test]
fn new_session_gets_a_conversation_id_and_passes_it() {
    // profile：is_agent = false（免 git），session_id_args = ["--session-id", "{id}"]，
    // resume_id_args = ["--resume", "{id}"]，command 回显参数。
    let mgr = /* 按本文件已有测试的方式建 SessionManager + 注册上面的 profile */;
    let id = mgr.create(dir, "echo-id", None, &[]).unwrap();
    let conv = mgr.conversation_of(id).expect("应当分配 id");
    wait_until_screen_contains(&mgr, id, &format!("--session-id {conv}"));
}

#[test]
fn resuming_with_a_conversation_uses_resume_id_args_and_keeps_the_id() {
    let id = mgr.create_resuming(dir, "echo-id", None, &[], false, None, Some("abc")).unwrap();
    wait_until_screen_contains(&mgr, id, "--resume abc");
    assert_eq!(mgr.conversation_of(id).as_deref(), Some("abc"));
}

#[test]
fn profile_without_id_args_has_no_conversation() {
    let id = mgr.create(dir, "shell", None, &[]).unwrap();
    assert_eq!(mgr.conversation_of(id), None);
}

#[test]
fn persisted_record_carries_conversation_and_flags() {
    // 装 last_sessions_path 到临时目录；建一个 echo-id 会话；
    // persist_last_sessions_with({id}, {id: "/tmp/h.md"})；load 回来：
    // conversation_id 是 Some(conv)，interrupted = true，handoff = Some("/tmp/h.md")。
}
```

（`wait_until_screen_contains` 若本文件没有现成的等屏幕辅助函数，就写一个：轮询 `mgr` 取屏幕文本，最多 5 秒。）

`src/daemon.rs` 测试模块（`restore_last_sessions` 附近已有恢复相关测试，照它们的写法）：

```rust
#[test]
fn restore_prefers_conversation_id_over_continue_grouping() {
    // 两条记录：同目录、同 profile（echo-id，同时声明 resume_args = ["--continue"]），
    // conversation_id 分别是 "a"、"b"。恢复后两个会话屏幕分别出现 "--resume a"、"--resume b"，
    // 都没有 "--continue"。
}

#[test]
fn restore_without_conversation_id_falls_back_to_continue_grouping() {
    // 两条旧记录（conversation_id = None），同 (dir, profile)：只有 last_active 大的那条带 "--continue"，
    // 另一条拿到一个新分配的 "--session-id <新 id>"。
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib conversation_id last_sessions session:: daemon::`
Expected: 编译失败（缺字段、缺函数）。

- [ ] **Step 3: 实现**

`src/conversation_id.rs`：

```rust
//! 给 agent 的对话起一个 id：dct 开会话时自己定，重启后按它接回。
//! 格式是 UUID v4——claude 的 `--session-id` 只收合法 UUID。

pub fn new() -> String {
    let mut b = [0u8; 16];
    // 拿不到系统随机数是极端情况；退回时间戳拼一个，宁可接不回也不让开会话失败。
    if getrandom::getrandom(&mut b).is_err() {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        b.copy_from_slice(&n.to_le_bytes());
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}
```

`RecordedSession` 加三个字段（文件头那段「不记 agent 自己的对话 id」注释改写：说明 2026-10-14 起改为记，理由是同目录多会话只接得回一个，见 spec）：

```rust
    /// agent 自己的对话 id（profile 声明了 `session_id_args` 才有）。
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// 平稳重启时这个会话是被打断的（没干完就被叫停或硬停）。
    #[serde(default)]
    pub interrupted: bool,
    /// 接不回对话的会话，重启前写的交接文件。
    #[serde(default)]
    pub handoff: Option<PathBuf>,
```

`struct Session` 加 `conversation_id: Option<String>`。`create_inner` 多一个参数 `conversation: Option<&str>`，把原来拼 `command` 那段换成：

```rust
        // 对话 id：恢复时带着旧的就按 id 接回；否则 profile 声明了就新起一个。
        // 都没有才退回 `resume_args`（`--continue` 按目录猜）。
        let mut command = profile.command.clone();
        let conversation_id = match conversation {
            Some(c) if profile.resumes_by_id() => {
                command.extend(crate::profile::fill_id(&profile.resume_id_args, c));
                Some(c.to_string())
            }
            _ => {
                if resume {
                    command.extend(profile.resume_args.iter().cloned());
                }
                if profile.resumes_by_id() && !resume {
                    let c = crate::conversation_id::new();
                    command.extend(crate::profile::fill_id(&profile.session_id_args, &c));
                    Some(c)
                } else {
                    None
                }
            }
        };
```

注意：`resume = true` 走 `--continue` 的那一条**不能**再加 `--session-id`（claude 不允许两者同时给），所以那一条的 `conversation_id` 是 `None`，下次重启照旧按目录猜——只此一次，之后新开的会话都有 id。把 `conversation_id` 存进 `Session`。

`create()` 传 `None`；`create_resuming` 多一个 `conversation: Option<&str>` 透传。

`persist_last_sessions_with`：在原 `persist_last_sessions` 的循环里，每条记录填 `conversation_id: s.conversation_id.clone()`、`interrupted: interrupted.contains(&s.id)`、`handoff: handoffs.get(&s.id).cloned()`；原函数改为 `self.persist_last_sessions_with(&HashSet::new(), &HashMap::new())`。

`conversation_of(id)`：取会话的 `conversation_id.clone()`。

`daemon.rs::restore_last_sessions`：调 `create_resuming` 时传 `record.conversation_id.as_deref()`；有 `conversation_id` 的记录**不参与** `group_for_resume` 的抢位（它们不需要 `--continue`）——实现上：先把有 id 的记录的 `resume` 判定改为 `false`（反正有 id 时不看它），`group_for_resume` 只喂没有 id 的那些，再按原下标配回。为此在 `last_sessions.rs` 新增：

```rust
/// 有对话 id 的记录各接各的，不进 `--continue` 的分组；没有的照旧分组。
/// 返回值跟输入逐位对应，语义同 `group_for_resume`。
pub fn continue_flags(sessions: &[RecordedSession]) -> Vec<bool> {
    let idx: Vec<usize> = (0..sessions.len()).filter(|&i| sessions[i].conversation_id.is_none()).collect();
    let sub: Vec<RecordedSession> = idx.iter().map(|&i| sessions[i].clone()).collect();
    let flags = group_for_resume(&sub);
    let mut out = vec![false; sessions.len()];
    for (k, &i) in idx.iter().enumerate() {
        out[i] = flags[k];
    }
    out
}
```

并加测试 `continue_flags_skips_records_with_ids`：三条同组记录，一条有 id——有 id 的判 `false`，另两条里 `last_active` 大的判 `true`。`restore_last_sessions` 改用 `continue_flags`。`main.rs::offer_to_resume_last_sessions` 若也调了 `group_for_resume` 来展示，同步改用 `continue_flags`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/
git commit -m "feat(session): record each agent conversation id and resume every session into its own conversation after a daemon restart"
```

---

