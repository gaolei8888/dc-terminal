# 平稳重启 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `dct restart` 先等各会话停下（15 分钟，再兜底 3 分钟），记住每个会话的对话 id 和「被打断」状态，由脱离会话的小进程换后台，重启后每个会话接回自己的对话，被打断的收到「继续」，接不回的读交接。

**Architecture:** profile 声明 `session_id_args` / `resume_id_args`，dct 开会话时生成 UUID 并记进 `last-sessions.toml`；后台里新增一个纯状态机 `drain.rs` 决定「等 / 发兜底话 / 发交接请求 / 结束」，后台线程按它执行；结束时落盘、起 `dct __swap` 小进程、自己退出；新后台恢复会话后，`resume_notes.rs` 在各会话第一次空闲时投递「继续」或「先读交接」。

**Tech Stack:** Rust（现有 crate），`getrandom`（已在依赖里）生成 UUID，serde/toml。

**Spec:** `docs/superpowers/specs/2026-10-14-graceful-restart-design.md`（实施者必须先读完）

## Global Constraints

- 提交信息用英文；**不加** `Co-Authored-By` 或任何 AI 署名行（用户规矩）。
- 推送前必须 `cargo +1.99.0 clippy --all-targets -- -D warnings` 干净（CI 用最新 clippy）。
- 代码注释、面向用户的文字用中文，跟周围代码一个风格；面向用户的话要大白话，不出现 pty、SIGTERM 之类术语。
- dct 只放通用代码：**不许**在 Rust 里按 profile 名字（`"claude"`、`"codex"`）分支，能力一律由 profile 字段声明。
- 没实测过的 profile 不许声明恢复参数（`profile.rs` 里 `unverified_profiles_declare_no_resume_args` 那条规矩同样适用于新字段）。
- 新增 `Request` 变体要把 `PROTOCOL_VERSION` 从 24 升到 25（`src/proto.rs:145`），`Hello` 永远不改形状。
- 测试不许碰真实的 `~/.dct`：路径一律跟着 socket 走（同 `last_sessions_path_for_socket` 的模式）。
- **不许**在实施过程中重启用户真实的 dct 后台、不许装二进制到 `~/.local/bin`——用户有 18 个在跑的会话。真机验收只在 Task 10 用单独的 socket 做。
- 默认等待：`wait` 15 分钟，`nudge_wait` 3 分钟；重启后投递等空闲最多 2 分钟；`__swap` 等旧后台退出最多 10 秒；CLI 不来问进度超过 10 秒算取消。
- **本计划里的参考代码不是权威**：行号和函数名以实际源码为准，写之前先核对；发现计划和源码对不上，以源码为准并在报告里写明。

## Review Focus

1. **第一次升级时，在跑的后台是旧的（协议 24），不认识 `Drain`**——`dct restart` 必须认出来，退回今天的直接重启（先问 y/n），而不是报错或卡住。Task 8 测。
2. **发起者自己的会话**（在某个 dct 会话里敲的 `dct restart`）永远不会变空闲——不许等它、不许对它发兜底话，要直接记成被打断。Task 4 测。
3. **旧的 `last-sessions.toml`（没有新字段）**必须照读，照旧走 `--continue` 分组逻辑。Task 2 测。
4. **排空期间 CLI 中途没了**（终端被关、Ctrl+C）——后台要自己取消、放出存着的留言，不能永远停在「准备重启」。Task 6 测。
5. **兜底那句话发出去之后会话变成 `Working`，然后又变回 `Idle`**——要算作「停下了」但仍记 `interrupted = true`（它是被叫停的，活没干完）。Task 4 测。

---

## 文件结构

| 文件 | 改动 | 职责 |
|---|---|---|
| `src/profile.rs` | 改 | 新字段 `session_id_args`、`resume_id_args`；`fill_id()` 替换 `{id}` |
| `profiles/claude.toml` 等 5 个 claude 外壳 | 改 | 声明两个新字段 |
| `profiles/qwen.toml`、`profiles/dc.toml` | Task 3 实测通过后才改 | 同上 |
| `src/last_sessions.rs` | 改 | `RecordedSession` 加 `conversation_id`、`interrupted`、`handoff` |
| `src/session.rs` | 改 | `Session` 记 `conversation_id`；`create_inner` 生成/接回；`persist_last_sessions_with(..)`；`draining` 开关 |
| `src/conversation_id.rs` | 新 | 用 `getrandom` 生成 UUID v4 字符串 |
| `src/drain.rs` | 新 | 纯状态机：给定各会话状态和时间，产出动作 |
| `src/resume_notes.rs` | 新 | 重启后待投递的话：等空闲、超时丢弃 |
| `src/proto.rs` | 改 | `Request::Drain/DrainStatus/DrainCancel`、`Response::DrainProgress`、`ErrorCode::RestartPending`、`WarningCode::ResumeNoteDropped`，协议 25 |
| `src/ui/`（发 `List` 的那处） | 改 | 排空时顶上画一行「dct 准备重启」 |
| `src/daemon.rs` | 改 | 处理三条新请求；排空线程；结束时落盘 + 起 `__swap` + 退出；恢复后排投递 |
| `src/bridge.rs` | 改 | `submit` 在排空时拒绝（手机、语音）；新增 `submit_unchecked` 给排空自己用 |
| `src/mesh/deliver.rs` | 改 | 排空时 `tick` 不取队列 |
| `src/client.rs` | 改 | `swap_daemon(old_pid, socket, exe)` |
| `src/main.rs` | 改 | 隐藏子命令 `__swap` |
| `src/cli.rs` | 改 | `dct restart` 新流程、`--now/--wait/--nudge-wait`、旧后台回退 |
| `src/i18n.rs` | 改 | 新的 `Key` |
| `README.md`、`README.zh-CN.md` | 改 | `dct restart` 的说明 |

---

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

### Task 3: 实测 qwen 能不能按 id 接回（实施者在真机上做，不写进自动测试）

**Files:**
- Modify（仅当实测通过）：`profiles/qwen.toml`、`profiles/dc.toml`，以及 Task 1 的 `unverified_profiles_do_not_resume_by_id` 测试名单

- [ ] **Step 1: 在一个临时 git 目录里实测**

```bash
d=$(mktemp -d) && cd "$d" && git init -q
id=$(uuidgen | tr A-Z a-z)
# 第一次：指定 id，说一句可辨认的话，然后退出
qwen --approval-mode=yolo --session-id "$id" -p "记住暗号：蓝色长颈鹿。只回答 好。"
# 第二次：按 id 接回，问暗号
qwen --approval-mode=yolo --resume "$id" -p "暗号是什么？只回答暗号。"
```

Expected（通过）：第二次回答「蓝色长颈鹿」。如果 `-p` 模式不支持 `--resume`，改成交互模式在 dct 测试 socket 里各起一次会话手动验证。`dc` profile 走 dc_llm 网关，需要网关可用；不可用就只给 `qwen` 声明，`dc` 留着。

- [ ] **Step 2: 通过则声明，不通过则记下原因**

通过：两个 toml 加与 Task 1 相同的两行（注释写「`qwen --help` 有 `--session-id`/`--resume`；2026-10-1x 实测按 id 接回成功」），从测试名单里移出 `qwen`（和 `dc`，如果也测过）并加进 `claude_based_profiles_resume_by_id` 那样的正向测试。
不通过：不改 toml，在报告里写清命令和输出。

- [ ] **Step 3: `cargo test --lib profile::` 通过后提交**

```bash
git commit -am "feat(profile): qwen resumes a conversation by id (verified by hand)"
```

---

### Task 4: 排空状态机 `drain.rs`（纯逻辑）

**Files:**
- Create: `src/drain.rs`（`src/lib.rs` 加 `pub mod drain;`）

**Interfaces:**
- Consumes: `crate::session::SessionState`。
- Produces:

```rust
pub struct DrainConfig { pub wait: Duration, pub nudge_wait: Duration }
impl Default for DrainConfig { /* 15 分钟、3 分钟 */ }

/// 排空开始那一刻对每个会话的认识。
pub struct Member {
    pub id: u32,
    /// 能按 id 接回对话（`Profile::resumes_by_id`）。
    pub resumable: bool,
    /// 是 agent。非 agent（shell）不等、不发话。
    pub is_agent: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// 接不回的会话：请它写交接到这个文件。
    AskHandoff { id: u32, path: PathBuf },
    /// 到点还在忙：请它在安全处停下。
    Nudge { id: u32 },
    /// 排空结束，可以落盘重启了。
    Finish { interrupted: HashSet<u32>, handoffs: HashMap<u32, PathBuf> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// 还在等的会话 id。
    pub waiting: Vec<u32>,
    /// 距离下一个时限还剩多少秒。
    pub remaining_s: u64,
    /// 已经进入兜底阶段。
    pub nudged: bool,
}

pub struct Drain { /* 私有 */ }
impl Drain {
    pub fn new(members: Vec<Member>, initiator: Option<u32>, cfg: DrainConfig,
               handoff_dir: PathBuf, now: Instant) -> Drain;
    /// 每拍调一次。`states` 是此刻各会话状态（不在表里 = 会话已经没了，算停下）。
    pub fn step(&mut self, now: Instant, states: &HashMap<u32, SessionState>) -> Vec<Action>;
    pub fn progress(&self, now: Instant) -> Progress;
    pub fn finished(&self) -> bool;
}

/// 交接文件名：`<handoff_dir>/<id>-<YYYYmmdd-HHMMSS>.md`（时间由调用方给，便于测试）。
pub fn handoff_path(dir: &Path, id: u32, stamp: &str) -> PathBuf;
pub const NUDGE_TEXT: &str = "dct 马上要重启。请在安全的地方停下手上的事，然后用一句话写下你做到哪了、下一步是什么。";
pub fn handoff_text(path: &Path) -> String; // spec 第 5 节那句，带路径
```

规则（逐条对应 spec 第 3 节）：
- 「停下了」= 状态是 `Idle`、`Stopped`、`Failed`，或者不在 `states` 里。
- 非 agent 成员一开始就算停下，永不发话。
- 发起者（`initiator`）一开始就算停下，**直接进 `interrupted`**，不发任何话。
- 接不回（`!resumable`）的 agent：第一次被看到「停下了」时发一次 `AskHandoff`，然后**重新等它再停下一次**（它在写交接）；它的路径进 `handoffs`。
- 能接回的 agent：停下了就好，不发话。
- 到 `wait` 还没停下的：每个发一次 `Nudge`，进入兜底阶段，这些会话都进 `interrupted`（不管之后有没有停）。
- 兜底阶段再过 `nudge_wait` 还没停的：不再等，直接 `Finish`。
- 所有成员都停下 → `Finish`（只发一次，之后 `step` 返回空）。

- [ ] **Step 1: 写失败的测试**（`src/drain.rs` 底部）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use SessionState::*;

    fn cfg() -> DrainConfig { DrainConfig { wait: Duration::from_secs(900), nudge_wait: Duration::from_secs(180) } }
    fn m(id: u32, resumable: bool) -> Member { Member { id, resumable, is_agent: true } }
    fn st(v: &[(u32, SessionState)]) -> HashMap<u32, SessionState> { v.iter().cloned().collect() }
    fn finish(a: &[Action]) -> Option<(&HashSet<u32>, &HashMap<u32, PathBuf>)> {
        a.iter().find_map(|x| match x { Action::Finish { interrupted, handoffs } => Some((interrupted, handoffs)), _ => None })
    }

    #[test]
    fn all_idle_finishes_immediately_with_no_interruptions() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true), m(2, true)], None, cfg(), "/h".into(), t0);
        let a = d.step(t0, &st(&[(1, Idle), (2, Idle)]));
        let (i, h) = finish(&a).unwrap();
        assert!(i.is_empty() && h.is_empty());
        assert!(d.finished());
        assert!(d.step(t0, &st(&[])).is_empty());
    }

    #[test]
    fn waits_for_working_then_finishes_when_it_stops() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true)], None, cfg(), "/h".into(), t0);
        assert!(d.step(t0, &st(&[(1, Working)])).is_empty());
        assert_eq!(d.progress(t0).waiting, vec![1]);
        let a = d.step(t0 + Duration::from_secs(60), &st(&[(1, Idle)]));
        assert!(finish(&a).unwrap().0.is_empty());
    }

    #[test]
    fn nudges_at_deadline_and_marks_interrupted_even_if_it_then_stops() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true), m(2, true)], None, cfg(), "/h".into(), t0);
        d.step(t0, &st(&[(1, Working), (2, Idle)]));
        let a = d.step(t0 + Duration::from_secs(900), &st(&[(1, Working), (2, Idle)]));
        assert_eq!(a, vec![Action::Nudge { id: 1 }]);
        assert!(d.progress(t0 + Duration::from_secs(900)).nudged);
        // 兜底话发出去后它先变 Working 再变 Idle（Review Focus 5）
        d.step(t0 + Duration::from_secs(910), &st(&[(1, Working), (2, Idle)]));
        let a = d.step(t0 + Duration::from_secs(950), &st(&[(1, Idle), (2, Idle)]));
        let (i, _) = finish(&a).unwrap();
        assert_eq!(i, &HashSet::from([1]));
    }

    #[test]
    fn hard_deadline_after_nudge_finishes_anyway() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(1, true)], None, cfg(), "/h".into(), t0);
        d.step(t0 + Duration::from_secs(900), &st(&[(1, Working)]));
        assert!(d.step(t0 + Duration::from_secs(1079), &st(&[(1, Working)])).is_empty());
        let a = d.step(t0 + Duration::from_secs(1080), &st(&[(1, Working)]));
        assert_eq!(finish(&a).unwrap().0, &HashSet::from([1]));
    }

    #[test]
    fn initiator_is_not_waited_for_and_counts_as_interrupted() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(7, true)], Some(7), cfg(), "/h".into(), t0);
        let a = d.step(t0, &st(&[(7, Working)]));
        assert_eq!(finish(&a).unwrap().0, &HashSet::from([7]));
        assert!(!a.iter().any(|x| matches!(x, Action::Nudge { .. })));
    }

    #[test]
    fn unresumable_idle_session_is_asked_for_handoff_then_waited_again() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(3, false)], None, cfg(), "/h".into(), t0);
        let a = d.step(t0, &st(&[(3, Idle)]));
        assert!(matches!(&a[..], [Action::AskHandoff { id: 3, .. }]));
        // 它开始写交接
        assert!(d.step(t0 + Duration::from_secs(1), &st(&[(3, Working)])).is_empty());
        let a = d.step(t0 + Duration::from_secs(30), &st(&[(3, Idle)]));
        let (i, h) = finish(&a).unwrap();
        assert!(i.is_empty());
        assert!(h.contains_key(&3));
    }

    #[test]
    fn unresumable_busy_session_is_asked_only_after_it_stops() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(3, false)], None, cfg(), "/h".into(), t0);
        assert!(d.step(t0, &st(&[(3, Working)])).is_empty());
        let a = d.step(t0 + Duration::from_secs(5), &st(&[(3, Idle)]));
        assert!(matches!(&a[..], [Action::AskHandoff { id: 3, .. }]));
    }

    #[test]
    fn vanished_or_failed_sessions_count_as_stopped_and_non_agents_are_ignored() {
        let t0 = Instant::now();
        let shell = Member { id: 9, resumable: false, is_agent: false };
        let mut d = Drain::new(vec![m(1, true), m(2, true), shell], None, cfg(), "/h".into(), t0);
        let a = d.step(t0, &st(&[(2, Failed), (9, Working)]));
        assert!(finish(&a).is_some());
        assert!(!a.iter().any(|x| matches!(x, Action::AskHandoff { id: 9, .. })));
    }

    #[test]
    fn handoff_path_is_under_dir() {
        assert_eq!(handoff_path(Path::new("/h"), 3, "20261014-120000"), PathBuf::from("/h/3-20261014-120000.md"));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib drain::`
Expected: 编译失败。

- [ ] **Step 3: 实现** `Drain`（每个成员记一个小状态：`Waiting` / `HandoffAsked{seen_busy: bool}` / `Done`；全局记 `deadline = t0 + wait`、`nudged_at: Option<Instant>`、`interrupted`、`handoffs`、`finished`）。`AskHandoff` 的时间戳用 `std::time::SystemTime` 格式化成 `YYYYmmdd-HHMMSS`（不引新依赖：从 UNIX 秒手算 UTC 日期，或沿用仓库里已有的时间格式化函数——先 grep `fn .*stamp\|format_time`）。`HandoffAsked` 的会话要等「先看到过一次忙、或者已经过了 10 秒」之后再看到停下才算完——防止同一拍里刚发完话、屏幕还没变就被判成写完了。为这一条补测试：

```rust
    #[test]
    fn handoff_not_considered_written_in_the_same_breath() {
        let t0 = Instant::now();
        let mut d = Drain::new(vec![m(3, false)], None, cfg(), "/h".into(), t0);
        d.step(t0, &st(&[(3, Idle)]));
        // 屏幕还没来得及变忙
        assert!(d.step(t0 + Duration::from_millis(500), &st(&[(3, Idle)])).is_empty());
        // 10 秒还没变忙，当它不打算写了，算完
        assert!(finish(&d.step(t0 + Duration::from_secs(10), &st(&[(3, Idle)]))).is_some());
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib drain::`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/drain.rs src/lib.rs
git commit -m "feat(drain): pure state machine that waits for sessions to stop, nudges late ones and asks unresumable ones for a handoff"
```

---

### Task 5: 排空期间，自动来源不往会话里打字

**Files:**
- Modify: `src/session.rs`（`SessionManager` 加 `draining: AtomicBool` 和 `set_draining(bool)` / `is_draining()`）
- Modify: `src/bridge.rs`（`submit` :214）
- Modify: `src/mesh/deliver.rs`（`Inbox` trait :318、`LocalInbox` :341、`tick` :834）
- Modify: `src/proto.rs`（`ErrorCode::RestartPending`）、`src/i18n.rs`（它的中文/英文说法）

**Interfaces:**
- Produces:
  - `SessionManager::set_draining(&self, on: bool)`、`SessionManager::is_draining(&self) -> bool`
  - `bridge::submit` 在 `is_draining()` 时返回 `coded(ErrorCode::RestartPending)`，不写任何字节。
  - `pub(crate) fn bridge::submit_unchecked(mgr, id, text) -> anyhow::Result<()>`：原来 `submit` 的本体，排空自己发话和重启后投递用。
  - `Inbox::draining(&self) -> bool`（默认 `false`；`LocalInbox` 返回 `self.mgr.is_draining()`）。

- [ ] **Step 1: 写失败的测试**

`bridge.rs` 测试模块：

```rust
#[test]
fn submit_refuses_while_draining_and_writes_nothing() {
    let (mgr, id) = /* 本文件已有的建会话辅助，起一个 shell 会话 */;
    mgr.set_draining(true);
    let e = submit(&mgr, id, "hello").unwrap_err();
    assert!(matches!(e.downcast_ref::<CodedError>(), Some(CodedError(ErrorCode::RestartPending))));
    // 屏幕上没有 hello
    mgr.set_draining(false);
    submit(&mgr, id, "echo ok").unwrap();
}

#[test]
fn submit_unchecked_ignores_draining() {
    let (mgr, id) = /* 同上 */;
    mgr.set_draining(true);
    submit_unchecked(&mgr, id, "echo ok").unwrap();
}
```

`mesh/deliver.rs` 测试模块（已有假 `Inbox`，约 :925）：给假 `Inbox` 加一个 `draining` 开关，测：

```rust
#[test]
fn tick_keeps_queue_while_draining_and_delivers_after() {
    // 排一条留言给一个 Idle 会话；inbox.draining = true；tick → 没有 type_into 调用，队列里还在；
    // draining = false；tick → 送达一次。
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge:: mesh::deliver::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

```rust
pub(crate) fn submit(mgr: &crate::session::SessionManager, id: u32, text: &str) -> anyhow::Result<()> {
    // 准备重启时，手机、语音这些自动来源一律不打：它们没有队列，当场
    // 告诉对方「稍后再发」比悄悄吞掉强。排空自己要说的话走 `submit_unchecked`。
    if mgr.is_draining() {
        return Err(crate::proto::coded(crate::proto::ErrorCode::RestartPending));
    }
    submit_unchecked(mgr, id, text)
}
```

`mesh::deliver::tick` 开头：

```rust
    // 准备重启时不取队列：留言原样留着，取消或者重启之后照常送。
    if lock(mesh).inbox.as_ref().is_some_and(|i| i.draining()) {
        return;
    }
```

`ErrorCode::RestartPending` 的用户说法：「dct 正在准备重启，稍后再发」。核对手机桥（Telegram）和语音把 `submit` 的错误转成给用户的话的那一处，确认这句能到用户眼前（不行就在那里补一个分支）。若 `ErrorCode` 有「必须穷举」的匹配（i18n、HTTP 状态映射），一并补上。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/
git commit -m "feat(drain): while dct prepares to restart, phone and voice are told to retry later and mesh messages stay queued"
```

---

### Task 6: 后台执行排空：协议、线程、落盘、交给小进程、退出

**Files:**
- Modify: `src/proto.rs`（`Request` :309、`Response` :717、`PROTOCOL_VERSION` :145）
- Modify: `src/daemon.rs`（`handle` :939；新增排空线程）
- Modify: `src/client.rs`（新增 `spawn_swap`，见 Task 7 的 `__swap`）

**Interfaces:**
- Consumes: Task 2 的 `persist_last_sessions_with`、`conversation_of`；Task 4 的 `Drain`/`Action`/`Progress`/`NUDGE_TEXT`/`handoff_text`；Task 5 的 `set_draining`、`submit_unchecked`。
- Produces（协议 25）：

```rust
// Request
Drain { wait_s: u64, nudge_wait_s: u64, initiator: Option<u32> },
/// `renew = true` 只有发起重启的 CLI 发（续租约）；界面画横幅时发 `false`，不续。
DrainStatus { renew: bool },
DrainCancel,
// Response
DrainProgress { progress: crate::drain::Progress, done: bool },
```

  - `Drain` 已在排空时再发一次：返回当前进度，不重新开始。
  - `DrainStatus { renew: true }`：返回进度，同时刷新「CLI 还在」的租约。超过 10 秒没收到 `renew: true` → 取消。`renew: false` 只读不续。
  - `DrainCancel`：取消，`set_draining(false)`，返回 `Ok`。
  - 没在排空时 `DrainStatus` 返回 `DrainProgress { done: false, progress: 空 }`——CLI 据此知道被取消了（见 Task 8）。
  - `done: true` 表示已经落盘、`__swap` 已起，后台马上退出；CLI 收到后去等新后台。

- [ ] **Step 1: 写失败的测试**（`daemon.rs` 测试模块；照已有的 `run_with_manager` + 临时 socket 测试的写法，用 Task 2 那种回显参数的测试 profile，再加 `busy_pattern = "BUSY"`、`idle_pattern = "IDLE"`，命令是一个小 shell 脚本：读一行输入就打印 BUSY、睡 1 秒、再打印 IDLE）

```rust
#[test]
fn drain_with_all_idle_persists_ids_and_reports_done() {
    // 起两个会话，等它们 Idle；发 Drain{wait_s:5,nudge_wait_s:2,initiator:None}；
    // 轮询 DrainStatus 直到 done；读 last-sessions.toml：两条都有 conversation_id，interrupted 都是 false。
    // 测试模式下不真的起 __swap、不退出进程（见 Step 3 的 `DrainHooks`）。
}

#[test]
fn drain_cancel_restores_normal_input() {
    // 发 Drain；发 DrainCancel；mgr.is_draining() == false；DrainStatus 返回 done:false 且 waiting 为空。
}

#[test]
fn drain_lease_expires_without_status_polls() {
    // 用可注入的时钟或把租约改小（DrainHooks.lease = 200ms）；发 Drain 后不再发 DrainStatus；
    // 等 500ms，mgr.is_draining() == false。
}

#[test]
fn drain_nudges_busy_session_and_marks_it_interrupted() {
    // 一个会话一直 BUSY（脚本不打印 IDLE）；Drain{wait_s:1,nudge_wait_s:1}；
    // 等 done；屏幕上出现 NUDGE_TEXT 的开头；last-sessions.toml 里它 interrupted = true。
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib daemon::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

- `PROTOCOL_VERSION` 改 25，注释写「25：平稳重启（Drain/DrainStatus/DrainCancel）」。
- 后台持有 `Arc<Mutex<Option<DrainRun>>>`，`DrainRun { drain: Drain, last_poll: Instant }`。
- 收到 `Drain`：若已有就返回进度；否则从 `mgr.list()` 建 `Member`（`resumable` = 该会话 profile 的 `resumes_by_id()`，需要 `SessionInfo` 或 `SessionManager` 提供 profile 能力——加一个 `SessionManager::resumes_by_id(id) -> bool`），`handoff_dir = <socket 所在目录>/handoffs`（`create_dir_all`），`mgr.set_draining(true)`，起一个线程每 500ms：
  1. 取各会话状态 → `drain.step(now, &states)`；
  2. `AskHandoff` → `submit_unchecked(&mgr, id, &handoff_text(&path))`；`Nudge` → `submit_unchecked(&mgr, id, NUDGE_TEXT)`；发话失败只记 stderr，不中断；
  3. 租约过期 → 取消（同 `DrainCancel`）并结束线程；
  4. `Finish` → `mgr.persist_last_sessions_with(&interrupted, &handoffs)`，然后调 `hooks.after_finish()`。
- `DrainHooks`（trait 或函数指针结构体，`run()` 里装真的、测试装假的）：真的 `after_finish` = `crate::client::spawn_swap(std::process::id(), socket, current_exe)` 成功后 `std::process::exit(0)`；若 `spawn_swap` 失败：写 stderr，`set_draining(false)`，把 run 清掉，**不退出**（宁可不重启也不能把所有会话杀了却起不来新的）。测试的假 hooks 记一下「被调用了」。
- **界面横幅**（spec 第 3 节第 1 步）：界面（`src/ui/` 里每帧发 `List` 的地方，先 grep `Request::List`）每秒发一次 `DrainStatus { renew: false }`，有排空在跑就在顶上画一行「dct 准备重启：还在等 N 个会话」（新 `Key`）；后台是旧协议、不认这条请求时安静地不画。补一条测试：`renew: false` 的轮询不会让租约续上（照 `drain_lease_expires_without_status_polls`，期间每 100ms 发一次 `renew: false`，租约照样过期）。
- **落盘之后到退出之前不许再落盘**：`persist_last_sessions` 在 `is_draining()` 且已 finish 时直接返回（加一个 `frozen: AtomicBool`），防止会话被 SIGHUP 时 tick 把 `interrupted` 冲掉。为此补一条测试：finish 后调一次 `mgr.persist_last_sessions()`，文件内容不变。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/
git commit -m "feat(daemon): Drain request runs the graceful-restart state machine, persists ids and interruptions, then hands over to a detached swapper (protocol 25)"
```

---

### Task 7: `dct __swap`：脱离会话的小进程换后台

**Files:**
- Modify: `src/client.rs`（新增 `spawn_swap`、`swap_daemon`）
- Modify: `src/main.rs`（`Some("__swap")` 分支，不进 `--help`）

**Interfaces:**
- Produces:
  - `pub fn spawn_swap(old_pid: u32, socket: &Path, exe: &Path) -> Result<()>`：用 `sys::proc::spawn_detached`（与 `spawn_daemon` 同法）起 `exe __swap <old_pid> <socket>`。
  - `pub fn swap_daemon(old_pid: u32, socket: &Path, exe: &Path, grace: Duration) -> Result<()>`：等 `old_pid` 退出最多 `grace`（真的是 10 秒），超时 `hard_kill` 再等 2 秒；然后 `spawn_daemon(exe, socket)`；等 `Hello` 版本对上最多 10 秒（**这里比 `restart_daemon` 的 5 秒长**：新后台要先把所有会话接回来才开始应答）。失败往 `<socket 目录>/daemon.log` 追加一行中文。

- [ ] **Step 1: 写失败的测试**（`client.rs`）

```rust
#[test]
fn swap_waits_for_old_process_then_hard_kills_it() {
    // 起一个 `sleep 30` 子进程当「旧后台」；swap_daemon 的等待部分抽成
    // `wait_old_gone(pid, grace) -> bool`（true = 自己走了，false = 被硬杀）单测：
    // grace = 200ms → 返回 false，且进程已不在。
    // 再起一个 `sleep 0.1` → 返回 true。
}
```

（起新后台那一半由 Task 10 的真机验收覆盖。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib client::`
Expected: 编译失败。

- [ ] **Step 3: 实现** `wait_old_gone`、`swap_daemon`、`spawn_swap`；`main.rs` 加：

```rust
        // 平稳重启的最后一步，由旧后台起、脱离任何会话。不给用户敲，不进帮助。
        Some("__swap") => {
            let pid: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let sock = args.get(2).map(PathBuf::from).unwrap_or_else(socket_path);
            let exe = std::env::current_exe()?;
            std::process::exit(match dct::client::swap_daemon(pid, &sock, &exe, Duration::from_secs(10)) {
                Ok(()) => 0,
                Err(_) => 1,
            })
        }
```

注意 `current_exe()`：用户刚把新二进制 `mv` 到 `~/.local/bin/dct` 时，旧后台的 `current_exe` 在 macOS 上仍指向那个路径（文件名没变），起的就是新文件——这是想要的。Linux 上 `current_exe` 可能带 ` (deleted)`：沿用 `restart_daemon` 调用方取 `exe` 的办法（先 grep `current_exe` 看仓库里怎么处理的）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib client::`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/client.rs src/main.rs
git commit -m "feat(restart): detached __swap helper waits for the old daemon to exit and starts the new one"
```

---

### Task 8: `dct restart` 新流程

**Files:**
- Modify: `src/cli.rs`（`Restart` 参数解析、`run_restart` :386）
- Modify: `src/i18n.rs`（新 `Key`）
- Modify: `src/main.rs`（`restart` 参数解析处，若在 main 里）

**Interfaces:**
- Consumes: Task 6 的三条请求。
- Produces: `Restart` 参数变为 `{ ask: bool, now: bool, wait_s: u64, nudge_wait_s: u64 }`（`-y` → `ask=false`；`--now`；`--wait <分钟>`；`--nudge-wait <分钟>`；坏参数 → `Usage`）。

流程：
1. 没有后台 → 照旧 `StartFresh`。
2. `ask` → 照旧列会话、问 y/n。说明文字改成：「会先等正在干活的会话停下（最多 N 分钟），再重启；重启后每个会话接回原来的对话。」`--now` 时用旧说明。
3. 发 `Drain { wait_s, nudge_wait_s, initiator }`，`initiator` 取环境变量 `DCT_SESSION_ID`（`session::SESSION_ID_ENV`）解析成 `u32`；`--now` 时 `wait_s = nudge_wait_s = 0`。
4. **旧后台回退（Review Focus 1）**：先 `Hello` 拿协议号；`< 25` → 打印「这次后台太旧，不会等会话停下，直接重启」，走原来的 `restart_daemon`。
5. 每秒发 `DrainStatus { renew: true }`，打印进度一行（同一行刷新：`\r` + 清行），例如「还在等 2 个会话（dc-octo、dc_llm），剩 9 分 12 秒」，兜底阶段说「已请它们停下，最多再等 2 分 40 秒」。会话名字从 `List` 里取，取不到用编号。
6. Ctrl+C：装 `SIGINT` 处理（看 `sys::signal` 现有的封装能不能用于非 raw 模式；不能就用 `AtomicBool` + `libc::signal`，Windows 用 `SetConsoleCtrlHandler`——先 grep 仓库里 CLI 有没有现成的 Ctrl+C 处理），收到就发 `DrainCancel`，打印「已取消，一切照旧」，退出 0。
7. `done: true` → 打印「正在换新的后台……」，用 `wait_up_to(20s)` 等新后台 `Hello` 协议号对上；成功打印 `RestartDone`，失败打印 `RestartFailed`（附一句「看 ~/.dct/daemon.log」）。
8. 若命令是在 dct 会话里敲的（有 `DCT_SESSION_ID`），第 7 步这个进程大概率会随后台退出而被杀掉——这是预期的，`__swap` 会完成剩下的事。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn parse_restart_args() {
    assert_eq!(parse_restart(&[]), Restart::Run { ask: true, now: false, wait_s: 900, nudge_wait_s: 180 });
    assert_eq!(parse_restart(&["-y", "--wait", "5"]), Restart::Run { ask: false, now: false, wait_s: 300, nudge_wait_s: 180 });
    assert_eq!(parse_restart(&["--now"]), Restart::Run { ask: true, now: true, wait_s: 0, nudge_wait_s: 0 });
    assert!(matches!(parse_restart(&["--wait", "abc"]), Restart::Usage(_)));
}

#[test]
fn progress_line_names_sessions_and_time() {
    let p = Progress { waiting: vec![2, 9], remaining_s: 552, nudged: false };
    let names = HashMap::from([(2, "dc-octo".to_string())]);
    assert_eq!(progress_line(&p, &names, Lang::Zh), "还在等 2 个会话（dc-octo、9 号），剩 9 分 12 秒");
}

#[test]
fn old_daemon_falls_back_to_plain_restart() {
    // 抽出 `fn plan_for(protocol: Option<u32>) -> Plan { Graceful, Plain }`：
    // Some(24) → Plain；None → Plain；Some(25) → Graceful。
}
```

（`Restart` 原来是 `Usage/Ask/Yes` 三态，改形状时把原有测试一起改；类型名以源码为准。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib cli::`
Expected: 编译失败。

- [ ] **Step 3: 实现** 上面的流程和三个可测函数（`parse_restart`、`progress_line`、`plan_for`）。i18n 新增 key 只写中文和英文两份（其他语言照仓库现有做法：看 `i18n.rs` 是否要求每个 key 都有全语言）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib cli::`，再 `cargo test`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/
git commit -m "feat(restart): dct restart waits for sessions to stop with live progress and Ctrl+C cancel; --now, --wait and --nudge-wait; falls back on old daemons"
```

---

### Task 9: 重启后投递「继续」和「先读交接」

**Files:**
- Create: `src/resume_notes.rs`（`lib.rs` 加 mod）
- Modify: `src/daemon.rs`（`restore_last_sessions` 收集待投递；投递线程）
- Modify: `src/proto.rs`（`WarningCode::ResumeNoteDropped { dir: String }`）、`src/i18n.rs`

**Interfaces:**
- Consumes: Task 2 的 `RecordedSession.interrupted/handoff/conversation_id`；Task 5 的 `submit_unchecked`。
- Produces:

```rust
pub const CONTINUE_TEXT: &str = "dct 重启完了。请先检查一下你做到哪了（看看文件和改动的状态），再接着做重启前没做完的事。";
pub fn read_handoff_text(path: &Path) -> String; // 「dct 刚重启。你上次的交接在 <路径>，先读它，再接着做。」

pub struct Note { pub id: u32, pub text: String, pub deadline: Instant, pub dir: String }

/// 一条恢复出来的会话该收到什么话：
/// - interrupted 且按 id 接回了 → CONTINUE_TEXT
/// - 有 handoff 且文件存在 → read_handoff_text
/// - 其余 → None
pub fn note_for(record: &RecordedSession, resumed_by_id: bool) -> Option<String>;

/// 每拍：Idle 的投递并移除；过了 deadline 的移除并返回它们（给警告用）。
pub fn step(notes: &mut Vec<Note>, now: Instant, states: &HashMap<u32, SessionState>,
            send: &mut dyn FnMut(u32, &str) -> bool) -> Vec<Note>;
```

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn note_for_rules() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path().join("h.md");
    let mut r = /* 一条 RecordedSession，conversation_id = Some("a") */;
    assert_eq!(note_for(&r, true), None);
    r.interrupted = true;
    assert_eq!(note_for(&r, true).as_deref(), Some(CONTINUE_TEXT));
    assert_eq!(note_for(&r, false), None); // 没接回原对话，叫它「继续」它也不知道继续什么
    r.interrupted = false;
    r.handoff = Some(h.clone());
    assert_eq!(note_for(&r, false), None); // 文件不存在
    std::fs::write(&h, "x").unwrap();
    assert_eq!(note_for(&r, false), Some(read_handoff_text(&h)));
}

#[test]
fn step_waits_for_idle_and_drops_after_deadline() {
    let t0 = Instant::now();
    let mut notes = vec![
        Note { id: 1, text: "a".into(), deadline: t0 + Duration::from_secs(120), dir: "/x".into() },
        Note { id: 2, text: "b".into(), deadline: t0 + Duration::from_secs(120), dir: "/y".into() },
    ];
    let mut sent = vec![];
    let mut send = |id: u32, t: &str| { sent.push((id, t.to_string())); true };
    let states = HashMap::from([(1, SessionState::Working), (2, SessionState::Idle)]);
    assert!(step(&mut notes, t0, &states, &mut send).is_empty());
    assert_eq!(sent, vec![(2, "b".to_string())]);
    let dropped = step(&mut notes, t0 + Duration::from_secs(120), &states, &mut send);
    assert_eq!(dropped.len(), 1);
    assert!(notes.is_empty());
}

#[test]
fn failed_send_keeps_note_for_next_tick() {
    // send 返回 false → 留在 notes 里，下拍再试。
}
```

`daemon.rs`：

```rust
#[test]
fn restore_queues_continue_for_interrupted_sessions_only() {
    // 两条记录，都有 conversation_id，一条 interrupted；恢复后（脚本 profile 一起来就 Idle）
    // 只有 interrupted 那个会话的屏幕上出现 CONTINUE_TEXT 的开头。
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib resume_notes:: daemon::`
Expected: 编译失败。

- [ ] **Step 3: 实现**：`restore_last_sessions` 在每条成功恢复后算 `note_for(record, resumed_by_id)`（`resumed_by_id` = 记录有 `conversation_id` 且 profile `resumes_by_id()`），收集成 `Vec<Note>`（deadline = now + 2 分钟）返回给调用方；`run_with_manager` 有待投递时起一个线程每 500ms `step`，`send` 用 `submit_unchecked`；被丢弃的变成 `WarningCode::ResumeNoteDropped`（走 `set_resume_skips` 那条路顶到界面，说法：「<目录> 的会话重启后一直没准备好，没能叫它继续，请手动告诉它接着做」）。投递完把对应记录的 `interrupted`/`handoff` 清掉——最简单的办法是恢复完成后立刻 `persist_last_sessions()` 一次（新会话的记录本来就不带这两个标记）；核对恢复路径上 `create_resuming` 已经触发了落盘，若是，就不用另加。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/
git commit -m "feat(restart): after a graceful restart, interrupted sessions are told to continue and unresumable ones to read their handoff"
```

---

### Task 10: 文档与真机验收

**Files:**
- Modify: `README.md`、`README.zh-CN.md`（`dct restart` 那段）

- [ ] **Step 1: 写文档**：说明 `dct restart` 会先等、`--now`、`--wait`、`--nudge-wait`、Ctrl+C 取消、重启后接回对话、codex/opencode 会写交接。大白话，不出现术语。

- [ ] **Step 2: 全量检查**

```bash
cargo test
cargo +1.99.0 clippy --all-targets -- -D warnings
```

Expected: 全绿、零警告。

- [ ] **Step 3: 真机验收（隔离 socket，不碰用户真实后台）**

```bash
cargo build --release
T=$(mktemp -d); SOCK=$T/daemon.sock; BIN=$PWD/target/release/dct
$BIN daemon $SOCK &   # 前台起一个测试后台
```

用测试 socket（若 CLI 不支持指定 socket，就用 `HOME=$T` 跑——沙箱拦 HOME 的话请主会话来做这一步）在一个临时 git 目录里：
1. 起两个 claude 会话（同目录），各说一句可辨认的话（「暗号 A」「暗号 B」）；起一个 codex 会话；让其中一个 claude 做一件要 1–2 分钟的事。
2. `dct restart -y --wait 1 --nudge-wait 1`，看进度行、兜底话、codex 收到交接请求。
3. 重启后：两个 claude 各问「暗号是什么」，各答自己的；被打断那个收到「继续」；codex 收到「先读交接」且文件存在。
4. 在其中一个 claude 会话里让它自己执行 `dct restart -y --wait 1`，确认不卡、新后台起来、它被叫继续。
5. 起一次排空后按 Ctrl+C，确认会话照常、手机/语音以外的输入正常。

把每一步的结果（截屏文字或命令输出）写进报告。**不要**对用户真实的后台做任何一步。

- [ ] **Step 4: 提交**

```bash
git add README.md README.zh-CN.md
git commit -m "docs(restart): explain graceful restart in the READMEs"
```
