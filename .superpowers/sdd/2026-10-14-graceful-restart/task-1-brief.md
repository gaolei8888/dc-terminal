### Task 1: profile 声明「指定对话 id」和「按 id 接回」

**Files:**
- Modify: `src/profile.rs`（`Profile` 结构体 `resume_args` 字段旁，约 :165-176；测试模块约 :1520-1545）
- Modify: `profiles/claude.toml`、`profiles/deepseek.toml`、`profiles/glm.toml`、`profiles/kimi.toml`、`profiles/qwen-api.toml`

**Interfaces:**
- Produces:
  - `Profile.session_id_args: Vec<String>`（serde default 空）
  - `Profile.resume_id_args: Vec<String>`（serde default 空）
  - `pub fn fill_id(args: &[String], id: &str) -> Vec<String>`：把每个参数里的 `{id}` 换成 `id`
  - `impl Profile { pub fn resumes_by_id(&self) -> bool }`：两个字段都非空才为真

- [ ] **Step 1: 写失败的测试**（加在 `profile.rs` 的测试模块）

```rust
#[test]
fn fill_id_replaces_placeholder_only() {
    let args = vec!["--session-id".to_string(), "{id}".to_string(), "x{id}y".to_string()];
    assert_eq!(fill_id(&args, "abc"), vec!["--session-id", "abc", "xabcy"]);
}

#[test]
fn claude_based_profiles_resume_by_id() {
    for name in ["claude", "deepseek", "glm", "kimi", "qwen-api"] {
        let p = Profile::builtin(name).unwrap();
        assert_eq!(p.session_id_args, vec!["--session-id", "{id}"], "{name}");
        assert_eq!(p.resume_id_args, vec!["--resume", "{id}"], "{name}");
        assert!(p.resumes_by_id(), "{name}");
    }
}

#[test]
fn unverified_profiles_do_not_resume_by_id() {
    // qwen、dc 要等 Task 3 实测；codex、opencode 开会话时没法指定 id。
    for name in ["codex", "opencode", "qwen", "dc", "shell"] {
        let p = Profile::builtin(name).unwrap();
        assert!(p.session_id_args.is_empty() && p.resume_id_args.is_empty(), "{name}");
        assert!(!p.resumes_by_id(), "{name}");
    }
}

#[test]
fn profile_without_new_fields_still_parses() {
    let p = Profile::from_toml("name = \"x\"\ncommand = [\"x\"]\n").unwrap();
    assert!(p.session_id_args.is_empty() && p.resume_id_args.is_empty());
}
```

（最后一条测试里，`Profile` 的必填字段以源码为准，补齐到能解析为止。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib profile::`
Expected: 编译失败，提示没有 `fill_id`、`session_id_args`。

- [ ] **Step 3: 实现**

在 `resume_args` 字段后面加：

```rust
    /// 开会话时交给 agent 的「用这个对话 id」参数，`{id}` 由 dct 换成它
    /// 生成的 UUID。**只给实测过的 profile 写**，理由同 `resume_args`。
    /// 声明了它，dct 才记得住这个会话是哪一段对话，重启后才能各接各的——
    /// 不再靠 `--continue` 按目录去猜（同目录多个会话时只接得回一个）。
    #[serde(default)]
    pub session_id_args: Vec<String>,
    /// 重启后按 id 接回那段对话的参数，`{id}` 同上。
    #[serde(default)]
    pub resume_id_args: Vec<String>,
```

```rust
/// 把参数里的 `{id}` 换成真正的 id。
pub fn fill_id(args: &[String], id: &str) -> Vec<String> {
    args.iter().map(|a| a.replace("{id}", id)).collect()
}

impl Profile {
    /// 两个都声明了才算「能按 id 接回」：只会指定不会接回，或者反过来，
    /// 都接不回同一段对话。
    pub fn resumes_by_id(&self) -> bool {
        !self.session_id_args.is_empty() && !self.resume_id_args.is_empty()
    }
}
```

5 个 claude 外壳的 toml 在 `resume_args` 下面加（注释写清实测依据）：

```toml
# 开会话时由 dct 指定对话 id，重启后按 id 接回同一段对话。
# `claude --help`（2026-10-14）：`--session-id <uuid>`、`-r, --resume [value]`。
session_id_args = ["--session-id", "{id}"]
resume_id_args = ["--resume", "{id}"]
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib profile::`
Expected: 全部 PASS（包括原有的 `claude_based_profiles_declare_continue_as_resume_args`——`resume_args` 不动，留着给没有 id 的旧记录用）。

- [ ] **Step 5: 提交**

```bash
git add src/profile.rs profiles/
git commit -m "feat(profile): profiles can declare session_id_args and resume_id_args to pin and resume a conversation by id"
```

---

