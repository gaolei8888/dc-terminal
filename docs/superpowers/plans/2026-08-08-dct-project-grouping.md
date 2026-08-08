# dct 看板按项目分组 —— 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 dct 看板从「一个隐形的当前项目 + 一个过滤开关」改成「按项目分组、光标即当前项目」，并让每个项目记住自己上次用的 agent。

**Architecture:** 新增一个纯函数 `group_sessions`，把守护进程返回的全量会话按项目目录分组；`App` 用一个扁平的 `rows: Vec<Row>`（组头行 + 会话行）承载光标，当前项目由光标所在组派生，`App.current_dir` / `App.scope` / `App.visible` 三个字段连同 `a` 键一并删除。持久化层给 `projects.json` 加 `pinned` 和 `project_profiles` 两个字段，协议随之 +1。

**Tech Stack:** Rust 2021、ratatui、crossterm、serde / serde_json、anyhow。

**Spec:** `docs/superpowers/specs/2026-08-08-dct-project-grouping-design.md`

## Global Constraints

- 每个 Task 结束前必须跑：`cargo fmt`、`cargo clippy --all-targets -- -D warnings`、`cargo test`。三条全绿才提交。
- **提交信息用英文，不要 AI 署名行**（不加 `Co-Authored-By`）。
- 守护进程只报 `ErrorCode` / `WarningCode`，**句子一律由界面组装**（切语言不重启 daemon）。新文案一律进 `src/i18n.rs` 的 `Key` 词条表，用 `t!(lang, en: "...", zh: "...")`，两种语言都要给。
- **不用 emoji 当图标。**
- 排序一律**固定**（按路径字符串、按会话 id 升序），不得按活跃度或时间排——行在用户没按键时移动是本次要消灭的缺陷。
- 路径比较统一走 `std::fs::canonicalize`，失败时退化成原样；**归一只用于比较，不用于显示**。
- 底栏右段动作数**硬上限 3 条 + `? 更多`**，不随终端变宽增加。
- 每个 Task 结束时工作树必须能编译、测试全绿——不留「下个 Task 才编得过」的中间态。

---

### Task 1: `projects.rs` —— 每项目的 agent 记忆与 pinned 项目

**Files:**
- Modify: `src/projects.rs`

**Interfaces:**
- Consumes: 无（最底层）
- Produces:
  - `Store::last_profile_for(&self, dir: &Path) -> Option<String>`
  - `Store::set_last_profile_for(&mut self, dir: &Path, name: &str)`
  - `Store::pinned(&self) -> Vec<String>`
  - `Store::pin(&mut self, dir: &Path)`
  - `Store::unpin(&mut self, dir: &Path)`
  - 保留既有 `Store::list()`、`Store::touch()`、`Store::last_profile()`

- [ ] **Step 1: 写失败的测试 —— 每个项目各记各的 agent**

追加到 `src/projects.rs` 的 `mod tests` 里：

```rust
#[test]
fn each_project_remembers_its_own_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();

    let f = tmp.path().join("projects.json");
    let mut s = Store::load(&f);
    s.set_last_profile_for(&a, "claude");
    s.set_last_profile_for(&b, "codex");

    let s = Store::load(&f);
    assert_eq!(s.last_profile_for(&a).as_deref(), Some("claude"));
    assert_eq!(s.last_profile_for(&b).as_deref(), Some("codex"));
}

/// 老文件里只有一个全局 `last_profile`。升级之后每个项目都还没有自己的记录，
/// 这时候必须回退到那个全局值——否则老用户一升级，所有项目的 `n` 都变成
/// 「弹选择器」，看起来像是设置丢了。
#[test]
fn an_unknown_project_falls_back_to_the_old_global_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("projects.json");
    std::fs::write(&f, r#"{"recent":[],"last_profile":"kimi"}"#).unwrap();

    let s = Store::load(&f);
    assert_eq!(
        s.last_profile_for(tmp.path()).as_deref(),
        Some("kimi"),
        "没有单独记录的项目要吃全局兜底"
    );
}

#[test]
fn pinned_projects_dedupe_and_survive_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    std::fs::create_dir(&a).unwrap();
    let f = tmp.path().join("projects.json");

    let mut s = Store::load(&f);
    s.pin(&a);
    s.pin(&a);
    assert_eq!(Store::load(&f).pinned(), vec![canon(&a)], "重复 pin 不该出现两行");

    let mut s = Store::load(&f);
    s.unpin(&a);
    assert!(Store::load(&f).pinned().is_empty());
}

/// 老文件没有 `pinned` / `project_profiles` 两个字段，必须照常读出来，
/// 不能整份 JSON 解析失败把 `recent` 也一起丢掉。
#[test]
fn an_old_file_without_the_new_fields_still_loads() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("projects.json");
    std::fs::write(&f, r#"{"recent":["/x"],"last_profile":"claude"}"#).unwrap();

    let s = Store::load(&f);
    assert_eq!(s.list(), vec!["/x".to_string()]);
    assert!(s.pinned().is_empty());
}
```

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib projects::tests`
Expected: FAIL，编译错误 `no method named 'set_last_profile_for'`。

- [ ] **Step 3: 改 `Disk` 和 `Store` 的字段**

把 `src/projects.rs` 顶部的 `Disk` 与 `Store` 换成：

```rust
use std::collections::BTreeMap;

/// 磁盘格式。包一层对象而不是直接存数组，是为了将来加字段时老文件仍能读。
#[derive(Default, Serialize, Deserialize)]
struct Disk {
    #[serde(default)]
    recent: Vec<String>,
    /// **旧字段，留着当兜底。** 升级之前只有这一个全局值；升级之后新会话
    /// 一律写进 `project_profiles`，但还没开过会话的老项目要靠它，否则
    /// 一升级所有项目的 `n` 都退化成弹选择器。
    #[serde(default)]
    last_profile: Option<String>,
    /// 用户按 `p` 摆上看板、还没有会话的项目。落盘而不是只放内存里：
    /// 规则是「`x` 才能移除」，不落盘的话重启 dct 就自己没了，两句话对不上。
    #[serde(default)]
    pinned: Vec<String>,
    /// 项目目录 → 上次在这个项目里开会话用的 agent。
    /// 用 `BTreeMap` 不是 `HashMap`：落盘顺序稳定，`projects.json` 的 diff
    /// 才不会每次都乱跳。
    #[serde(default)]
    project_profiles: BTreeMap<String, String>,
}

pub struct Store {
    path: PathBuf,
    recent: Vec<String>,
    last_profile: Option<String>,
    pinned: Vec<String>,
    project_profiles: BTreeMap<String, String>,
}
```

`load` 里补上两个新字段：

```rust
Store {
    path: path.to_path_buf(),
    recent: disk.recent,
    last_profile: disk.last_profile,
    pinned: disk.pinned,
    project_profiles: disk.project_profiles,
}
```

`save` 里同样补齐：

```rust
let Ok(json) = serde_json::to_string(&Disk {
    recent: self.recent.clone(),
    last_profile: self.last_profile.clone(),
    pinned: self.pinned.clone(),
    project_profiles: self.project_profiles.clone(),
}) else {
    return;
};
```

- [ ] **Step 4: 抽出路径归一，加五个新方法**

`touch()` 里那段 canonicalize 抽成自由函数，让所有以路径为键的地方用同一套：

```rust
/// 以路径为键时统一走这里。`.` 和 `/abs/path` 必须落在同一个键上，
/// 否则同一个项目会在 `recent`、`pinned`、`project_profiles` 里各占一行。
///
/// 归一失败（目录刚被删）就用原样：丢掉这一条比存个粗糙的路径更糟。
fn key_of(dir: &Path) -> String {
    std::fs::canonicalize(dir)
        .unwrap_or_else(|_| dir.to_path_buf())
        .display()
        .to_string()
}
```

`touch()` 改成用它：

```rust
pub fn touch(&mut self, dir: &Path) {
    let key = key_of(dir);
    self.recent.retain(|p| p != &key);
    self.recent.insert(0, key);
    self.recent.truncate(MAX);
    self.save();
}
```

新增五个方法：

```rust
/// 这个项目上次用的 agent。没有单独记录就吃全局的旧值（见 `Disk::last_profile`）。
pub fn last_profile_for(&self, dir: &Path) -> Option<String> {
    self.project_profiles
        .get(&key_of(dir))
        .cloned()
        .or_else(|| self.last_profile.clone())
}

/// 记一笔「这个项目上次用的 agent」。同时刷新全局兜底值——一个刚被
/// `p` 摆上看板、从没开过会话的新项目，`n` 该给的是「你最近在用的那个」，
/// 而不是空。
pub fn set_last_profile_for(&mut self, dir: &Path, name: &str) {
    self.project_profiles
        .insert(key_of(dir), name.to_string());
    self.last_profile = Some(name.to_string());
    self.save();
}

pub fn pinned(&self) -> Vec<String> {
    self.pinned.clone()
}

/// 摆一个项目上看板。已经在里面就什么都不做——重复 pin 不该出现两行。
pub fn pin(&mut self, dir: &Path) {
    let key = key_of(dir);
    if !self.pinned.contains(&key) {
        self.pinned.push(key);
        self.save();
    }
}

pub fn unpin(&mut self, dir: &Path) {
    let key = key_of(dir);
    self.pinned.retain(|p| p != &key);
    self.save();
}
```

保留既有的 `last_profile()` 和 `set_last_profile()` 暂不删除——Task 2 会把调用点换掉，那时再删。

- [ ] **Step 5: 跑测试，确认通过**

Run: `cargo test --lib projects::tests`
Expected: PASS（含既有的 `touch_moves_existing_entry_to_front`、`last_profile_survives_reload`）。

- [ ] **Step 6: 全量检查并提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/projects.rs
git commit -m "feat: remember the agent per project, and which projects are pinned"
```

---

### Task 2: 协议与守护进程 —— `LastProfile` 带上目录，`Projects` 带上 pinned

**Files:**
- Modify: `src/proto.rs`（`PROTOCOL_VERSION`、`Request`、`Response`、`Display`、JSON 快照测试）
- Modify: `src/daemon.rs:227`（`Projects`）、`:256`（`Create` 的 remember）、`:285`（`LastProfile`）
- Modify: `src/ui/mod.rs:1138`、`:1192`（调用点跟着改，保证能编译）

**Interfaces:**
- Consumes: Task 1 的 `Store::last_profile_for` / `pin` / `unpin` / `pinned`
- Produces:
  - `Request::LastProfile { dir: String }`
  - `Request::PinProject { dir: String }` / `Request::UnpinProject { dir: String }`
  - `Response::Projects { recent: Vec<String>, pinned: Vec<String> }`
  - `PROTOCOL_VERSION == 6`

- [ ] **Step 1: 写失败的测试 —— 新变体的 JSON 形状**

改 `src/proto.rs` 里那条 round-trip 快照测试（`src/proto.rs:601` 起的列表和 `:637` 的期望串）。在 `Request` 列表里把 `Request::Projects` 之后、`LastProfile` 那一项换掉并新增两项：

```rust
Request::LastProfile { dir: "d".into() },
Request::PinProject { dir: "d".into() },
Request::UnpinProject { dir: "d".into() },
```

期望串里对应把 `"LastProfile"` 换成：

```
{"LastProfile":{"dir":"d"}},{"PinProject":{"dir":"d"}},{"UnpinProject":{"dir":"d"}}
```

再加一条新测试：

```rust
#[test]
fn projects_response_carries_both_lists() {
    let r = Response::Projects {
        recent: vec!["/a".into()],
        pinned: vec!["/b".into()],
    };
    let s = serde_json::to_string(&r).unwrap();
    assert_eq!(s, r#"{"Projects":{"recent":["/a"],"pinned":["/b"]}}"#);
}

#[test]
fn protocol_version_was_bumped_for_the_project_grouping_change() {
    assert_eq!(PROTOCOL_VERSION, 6);
}
```

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib proto::tests`
Expected: FAIL，`Request::LastProfile` 不接受具名字段 / `PROTOCOL_VERSION` 是 5。

- [ ] **Step 3: 改协议定义**

`src/proto.rs:34`：

```rust
pub const PROTOCOL_VERSION: u32 = 6;
```

`Request` 枚举里，把 `LastProfile` 换成带目录的，并新增两个变体（放在 `Projects` 附近，让「项目」相关的挨在一起）：

```rust
    /// 这个项目上次用的 agent。**必须带目录**：记忆是按项目分的，
    /// 一个全局值会让你在 A 项目按 `n` 开出 B 项目上次用的那个 agent。
    LastProfile { dir: String },
    /// 把一个项目摆上看板（哪怕它一个会话都没有）。
    PinProject { dir: String },
    /// 从看板上拿掉一个项目。只对没有会话的项目有意义，
    /// 「有没有会话」由界面判断，daemon 不管——它不知道界面正在显示什么。
    UnpinProject { dir: String },
```

`Response::Projects` 换成具名字段：

```rust
    Projects {
        recent: Vec<String>,
        pinned: Vec<String>,
    },
```

`Display for Request`（`src/proto.rs:259` 附近）三条：

```rust
            Request::LastProfile { dir } => write!(f, "LastProfile {dir}"),
            Request::PinProject { dir } => write!(f, "PinProject {dir}"),
            Request::UnpinProject { dir } => write!(f, "UnpinProject {dir}"),
```

- [ ] **Step 4: 改守护进程三处**

`src/daemon.rs:227`：

```rust
        Request::Projects => {
            let st = recover(store.lock());
            Ok(Response::Projects {
                recent: st.list(),
                pinned: st.pinned(),
            })
        }
```

`src/daemon.rs:285`：

```rust
        Request::LastProfile { dir } => Ok(Response::LastProfile(
            recover(store.lock()).last_profile_for(std::path::Path::new(&dir)),
        )),
        Request::PinProject { dir } => {
            recover(store.lock()).pin(std::path::Path::new(&dir));
            Ok(Response::Ok)
        }
        Request::UnpinProject { dir } => {
            recover(store.lock()).unpin(std::path::Path::new(&dir));
            Ok(Response::Ok)
        }
```

`src/daemon.rs:256` 那段 `remember` 改成按目录记：

```rust
                if remember {
                    st.set_last_profile_for(std::path::Path::new(&dir), &profile);
                }
```

删掉 `Store::last_profile()` 和 `Store::set_last_profile()`（`src/projects.rs`）——现在没有调用点了。同时删掉只测它们的 `last_profile_survives_reload`，它的职责已经被 `each_project_remembers_its_own_agent` 覆盖。

- [ ] **Step 5: 修界面侧的两个调用点，保证能编译**

`src/ui/mod.rs:1138`（`open_new_session` 里问上次的 agent）——本 Task 只做**能编译的最小改动**，`current_dir` 在 Task 5 才会消失：

```rust
                let dir = app.current_dir.display().to_string();
                match app.client().and_then(|c| c.call(Request::LastProfile { dir })) {
```

`src/ui/mod.rs:1192`（`open_project_picker`）：

```rust
        Ok(Response::Projects { recent: mut all, .. }) => {
```

- [ ] **Step 6: 跑测试，确认通过**

Run: `cargo test`
Expected: PASS。

- [ ] **Step 7: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/proto.rs src/daemon.rs src/projects.rs src/ui/mod.rs
git commit -m "feat: ask for the last agent per project, protocol version 6"
```

---

### Task 3: `view.rs` —— `ProjectGroup` 与 `group_sessions`（纯函数，只新增）

**Files:**
- Modify: `src/ui/view.rs`

**Interfaces:**
- Consumes: `crate::session::SessionInfo`（字段 `id: u32`、`dir: String`、`profile: String`、`state: SessionState`、`activity: String`）
- Produces:
  - `pub(crate) struct ProjectGroup { dir, name, parent, sessions, last_profile, pinned, collapsed }`
  - `pub(crate) fn group_sessions(sessions: &[SessionInfo], pinned: &[String], profiles: &BTreeMap<String, String>) -> Vec<ProjectGroup>`
  - `impl ProjectGroup { fn agent_counts(&self) -> Vec<(String, usize)>, fn failed(&self) -> usize }`

**本 Task 只新增，不删任何东西。** `Scope` / `visible_sessions` 原样留着，Task 5 才动。

- [ ] **Step 1: 写失败的测试**

追加到 `src/ui/view.rs` 的 `mod tests`：

```rust
fn si(id: u32, dir: &str, profile: &str) -> crate::session::SessionInfo {
    crate::session::SessionInfo {
        id,
        dir: dir.into(),
        profile: profile.into(),
        state: SessionState::Idle,
        activity: String::new(),
    }
}

#[test]
fn groups_are_sorted_by_path_and_sessions_by_id() {
    let all = vec![si(9, "/w/b", "claude"), si(2, "/w/a", "codex"), si(5, "/w/a", "claude")];
    let g = group_sessions(&all, &[], &BTreeMap::new());

    assert_eq!(g.len(), 2);
    assert_eq!(g[0].dir, PathBuf::from("/w/a"));
    assert_eq!(g[1].dir, PathBuf::from("/w/b"));
    assert_eq!(
        g[0].sessions.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![2, 5],
        "组内按 id 升序，固定"
    );
}

/// 规则 1：看板上的项目 = 有会话的 ∪ pinned 的。pinned 但没有会话的
/// 项目必须以空组出现，否则光标没地方落、`n` 无处可去。
#[test]
fn a_pinned_project_with_no_sessions_still_gets_a_group() {
    let g = group_sessions(&[], &["/w/empty".to_string()], &BTreeMap::new());

    assert_eq!(g.len(), 1);
    assert!(g[0].sessions.is_empty());
    assert!(g[0].pinned);
}

#[test]
fn a_project_that_is_both_pinned_and_busy_appears_once() {
    let all = vec![si(1, "/w/a", "claude")];
    let g = group_sessions(&all, &["/w/a".to_string()], &BTreeMap::new());

    assert_eq!(g.len(), 1, "pinned 和有会话是并集，不是两行");
    assert!(g[0].pinned);
    assert_eq!(g[0].sessions.len(), 1);
}

#[test]
fn the_group_header_summarises_agents_and_failures() {
    let mut all = vec![
        si(1, "/w/a", "claude"),
        si(2, "/w/a", "claude"),
        si(3, "/w/a", "codex"),
    ];
    all[2].state = SessionState::Failed;
    let g = group_sessions(&all, &[], &BTreeMap::new());

    assert_eq!(
        g[0].agent_counts(),
        vec![("claude".to_string(), 2), ("codex".to_string(), 1)],
        "按 agent 名字排序，数量是这个项目里的会话数"
    );
    assert_eq!(g[0].failed(), 1);
}

#[test]
fn a_group_carries_the_agent_that_project_used_last() {
    let mut profiles = BTreeMap::new();
    profiles.insert("/w/a".to_string(), "kimi".to_string());
    let g = group_sessions(&[], &["/w/a".to_string()], &profiles);

    assert_eq!(g[0].last_profile.as_deref(), Some("kimi"));
}

#[test]
fn the_name_is_the_last_path_component_and_the_parent_is_shortened() {
    let g = group_sessions(&[], &["/w/dc/dc-terminal".to_string()], &BTreeMap::new());

    assert_eq!(g[0].name, "dc-terminal");
    assert_eq!(g[0].parent, "/w/dc");
}

#[test]
fn grouping_nothing_at_all_yields_nothing() {
    assert!(group_sessions(&[], &[], &BTreeMap::new()).is_empty());
}
```

文件顶部补上 `use std::collections::BTreeMap;`（测试和实现都要用）。

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib ui::view::tests`
Expected: FAIL，`cannot find function 'group_sessions'`。

- [ ] **Step 3: 写实现**

加在 `src/ui/view.rs` 里 `visible_sessions` 附近：

```rust
/// 看板上的一个项目组。
///
/// `sessions` 是这一组要显示的会话——已停止的会话在列表里显示、在九宫格里
/// 不显示，这个差异由**调用方在传入前过滤**，分组函数本身不认识状态语义。
#[derive(Clone, Debug)]
pub(crate) struct ProjectGroup {
    /// 归一化后的绝对路径，也是分组键。
    pub dir: PathBuf,
    /// 组头上的项目名（路径最后一段）。
    pub name: String,
    /// 组头上那行灰字（父目录，已 `short_path`）。
    pub parent: String,
    pub sessions: Vec<crate::session::SessionInfo>,
    /// 这个项目上次用的 agent，底栏 `n 新建 <agent>` 要用。
    pub last_profile: Option<String>,
    /// 由 `p` 摆上来的。`x` 只能移除 pinned 且没有会话的组。
    pub pinned: bool,
    pub collapsed: bool,
}

impl ProjectGroup {
    /// 组头上的 `claude×2 codex×1`。**现算不存**：存下来就有两份真相，
    /// 而它们只有一份是新的。按 agent 名排序，跟组的排序同一个理由——
    /// 顺序不能随会话生灭而跳动。
    pub fn agent_counts(&self) -> Vec<(String, usize)> {
        let mut m: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for s in &self.sessions {
            *m.entry(s.profile.clone()).or_insert(0) += 1;
        }
        m.into_iter().collect()
    }

    /// 这个项目里有几个会话出错了。组头上要用红字点出来——
    /// 会话静默失败是 dct 最贵的失败模式。
    pub fn failed(&self) -> usize {
        self.sessions
            .iter()
            .filter(|s| s.state == SessionState::Failed)
            .count()
    }
}

/// 看板上出现哪些项目：**有会话的 ∪ pinned 的**。没有第三种。
///
/// 排序按 `dir` 字符串升序，**固定**。任何按活跃度或最后使用时间的排序，
/// 都会让行在用户没按键的时候移动——而「项目在我没按键的时候变了」正是
/// 这一版要消灭的东西。组内会话按 `id` 升序，同一个理由。
pub(crate) fn group_sessions(
    sessions: &[crate::session::SessionInfo],
    pinned: &[String],
    profiles: &BTreeMap<String, String>,
) -> Vec<ProjectGroup> {
    // 分组键统一走 canon：`/tmp` 和 `/private/tmp` 下的两个会话是同一个项目。
    let mut buckets: BTreeMap<PathBuf, Vec<crate::session::SessionInfo>> = BTreeMap::new();
    for s in sessions {
        buckets
            .entry(canon(Path::new(&s.dir)))
            .or_default()
            .push(s.clone());
    }
    let pinned_keys: Vec<PathBuf> = pinned.iter().map(|p| canon(Path::new(p))).collect();
    for p in &pinned_keys {
        buckets.entry(p.clone()).or_default();
    }

    buckets
        .into_iter()
        .map(|(dir, mut sessions)| {
            sessions.sort_by_key(|s| s.id);
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                // 根目录没有 file_name。显示整条路径总比显示空白强。
                .unwrap_or_else(|| dir.display().to_string());
            let parent = dir
                .parent()
                .map(|p| super::widgets::short_path(&p.display().to_string()))
                .unwrap_or_default();
            let last_profile = profiles.get(&dir.display().to_string()).cloned();
            let pinned = pinned_keys.contains(&dir);
            ProjectGroup {
                dir,
                name,
                parent,
                sessions,
                last_profile,
                pinned,
                collapsed: false,
            }
        })
        .collect()
}
```

`canon`（`src/ui/view.rs:671`）现在有了组外调用者，把它从私有改成 `pub(crate) fn canon`。

- [ ] **Step 4: 跑测试，确认通过**

Run: `cargo test --lib ui::view::tests`
Expected: PASS，7 条新测试全绿。

- [ ] **Step 5: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui/view.rs
git commit -m "feat: group sessions by project, sorted so rows never move on their own"
```

---

### Task 4: `view.rs` —— 行展平与光标锚点（纯函数，只新增）

**Files:**
- Modify: `src/ui/view.rs`

**Interfaces:**
- Consumes: Task 3 的 `ProjectGroup`
- Produces:
  - `pub(crate) enum Row { Header(usize), Session(usize, usize) }`
  - `pub(crate) fn flatten(groups: &[ProjectGroup]) -> Vec<Row>`
  - `pub(crate) enum Anchor { Session(u32), Header(PathBuf) }`
  - `pub(crate) fn anchor_of(groups: &[ProjectGroup], rows: &[Row], i: usize) -> Option<Anchor>`
  - `pub(crate) fn find_anchor(groups: &[ProjectGroup], rows: &[Row], a: &Anchor) -> Option<usize>`
  - `pub(crate) fn group_of(rows: &[Row], i: usize) -> Option<usize>`

- [ ] **Step 1: 写失败的测试**

```rust
fn grp(dir: &str, ids: &[u32]) -> ProjectGroup {
    let sessions: Vec<_> = ids.iter().map(|i| si(*i, dir, "claude")).collect();
    let mut g = group_sessions(&sessions, &[dir.to_string()], &BTreeMap::new());
    g.remove(0)
}

#[test]
fn flatten_puts_a_header_before_each_group() {
    let groups = vec![grp("/w/a", &[1, 2]), grp("/w/b", &[3])];
    assert_eq!(
        flatten(&groups),
        vec![
            Row::Header(0),
            Row::Session(0, 0),
            Row::Session(0, 1),
            Row::Header(1),
            Row::Session(1, 0),
        ]
    );
}

#[test]
fn a_collapsed_group_contributes_only_its_header() {
    let mut groups = vec![grp("/w/a", &[1, 2]), grp("/w/b", &[3])];
    groups[0].collapsed = true;
    assert_eq!(
        flatten(&groups),
        vec![Row::Header(0), Row::Header(1), Row::Session(1, 0)]
    );
}

#[test]
fn an_empty_group_still_contributes_its_header() {
    let groups = vec![grp("/w/empty", &[])];
    assert_eq!(flatten(&groups), vec![Row::Header(0)]);
}

#[test]
fn group_of_answers_for_both_row_kinds() {
    let groups = vec![grp("/w/a", &[1]), grp("/w/b", &[2])];
    let rows = flatten(&groups);
    assert_eq!(group_of(&rows, 0), Some(0));
    assert_eq!(group_of(&rows, 1), Some(0));
    assert_eq!(group_of(&rows, 3), Some(1));
    assert_eq!(group_of(&rows, 99), None);
}

/// 本设计最关键的不变式：后台事件让行数变了，光标必须还站在同一个东西上。
#[test]
fn the_cursor_stays_on_the_same_session_when_another_group_grows() {
    let before = vec![grp("/w/a", &[1]), grp("/w/b", &[7])];
    let rows_before = flatten(&before);
    // 光标在 /w/b 的会话 7 上（第 3 行）
    let a = anchor_of(&before, &rows_before, 3).unwrap();
    assert_eq!(a, Anchor::Session(7));

    // /w/a 里多开了两个会话，行数变了
    let after = vec![grp("/w/a", &[1, 4, 5]), grp("/w/b", &[7])];
    let rows_after = flatten(&after);
    let i = find_anchor(&after, &rows_after, &a).unwrap();

    assert_eq!(rows_after[i], Row::Session(1, 0), "还站在会话 7 上");
}

/// 会话没了（结束并被 prune）——退回它原来那个组的组头，不要滑到别的项目上。
#[test]
fn a_vanished_session_falls_back_to_its_own_group_header() {
    let before = vec![grp("/w/a", &[1]), grp("/w/b", &[7])];
    let rows_before = flatten(&before);
    let a = anchor_of(&before, &rows_before, 3).unwrap();

    let after = vec![grp("/w/a", &[1]), grp("/w/b", &[])];
    let rows_after = flatten(&after);
    let i = find_anchor(&after, &rows_after, &a).unwrap();

    assert_eq!(rows_after[i], Row::Header(1), "落在 /w/b 的组头上，不是 /w/a");
}

#[test]
fn a_header_anchor_finds_its_group_again_after_reordering() {
    let before = vec![grp("/w/b", &[7])];
    let rows_before = flatten(&before);
    let a = anchor_of(&before, &rows_before, 0).unwrap();
    assert_eq!(a, Anchor::Header(canon(Path::new("/w/b"))));

    // /w/a 是新出现的，排在前面，把 /w/b 挤到了第 2 行
    let after = vec![grp("/w/a", &[1]), grp("/w/b", &[7])];
    let rows_after = flatten(&after);
    let i = find_anchor(&after, &rows_after, &a).unwrap();

    assert_eq!(rows_after[i], Row::Header(1));
}
```

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib ui::view::tests`
Expected: FAIL，`cannot find type 'Row'`。

- [ ] **Step 3: 写实现**

```rust
/// 看板上的一行。分组之后光标不能再是「第几个会话」——它得能停在组头上，
/// 空组只有组头这一行。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Row {
    Header(usize),
    /// (组下标, 组内会话下标)
    Session(usize, usize),
}

/// 把分组展平成屏幕上的行。折叠的组只贡献组头那一行。
pub(crate) fn flatten(groups: &[ProjectGroup]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        rows.push(Row::Header(gi));
        if !g.collapsed {
            for si in 0..g.sessions.len() {
                rows.push(Row::Session(gi, si));
            }
        }
    }
    rows
}

/// 某一行属于哪个组。**「当前项目」就是这个函数的答案**——不再有一个
/// 可以跟屏幕不一致的 `current_dir` 字段。
pub(crate) fn group_of(rows: &[Row], i: usize) -> Option<usize> {
    match rows.get(i)? {
        Row::Header(g) => Some(*g),
        Row::Session(g, _) => Some(*g),
    }
}

/// 光标指着的那个东西的**语义身份**。重新分组之后靠它找回原位。
///
/// 存身份而不是存下标：下标在会话生灭时会指向别的东西，而那正好就是
/// 「项目在我没按键的时候变了」这个缺陷本身。
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Anchor {
    Session(u32),
    Header(PathBuf),
}

pub(crate) fn anchor_of(groups: &[ProjectGroup], rows: &[Row], i: usize) -> Option<Anchor> {
    match rows.get(i)? {
        Row::Header(g) => Some(Anchor::Header(groups.get(*g)?.dir.clone())),
        Row::Session(g, s) => Some(Anchor::Session(groups.get(*g)?.sessions.get(*s)?.id)),
    }
}

/// 找回锚点。顺序：同 id 的会话行 → 该会话原属组的组头 → 同 dir 的组头。
/// 全找不到返回 `None`，调用方落到第 0 行。
pub(crate) fn find_anchor(groups: &[ProjectGroup], rows: &[Row], a: &Anchor) -> Option<usize> {
    match a {
        Anchor::Session(id) => {
            // 会话还在：站回它身上
            if let Some(i) = rows.iter().position(|r| match r {
                Row::Session(g, s) => groups[*g].sessions[*s].id == *id,
                Row::Header(_) => false,
            }) {
                return Some(i);
            }
            // 会话没了：退回它原来那个组的组头。**不能就近落在下一行**——
            // 下一行可能已经是别的项目，那就等于项目在用户没按键时变了。
            let gi = groups
                .iter()
                .position(|g| g.sessions.iter().any(|s| s.id == *id));
            match gi {
                Some(gi) => rows.iter().position(|r| *r == Row::Header(gi)),
                None => None,
            }
        }
        Anchor::Header(dir) => {
            let gi = groups.iter().position(|g| &g.dir == dir)?;
            rows.iter().position(|r| *r == Row::Header(gi))
        }
    }
}
```

- [ ] **Step 4: 跑测试，确认通过**

Run: `cargo test --lib ui::view::tests`
Expected: PASS，7 条新测试全绿。

- [ ] **Step 5: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui/view.rs
git commit -m "feat: pin the cursor to what it points at, not to a row index"
```

---

### Task 5: `app.rs` + `board.rs` —— 换成分组状态，删掉 `Scope` 和 `a` 键

这是整个计划里唯一一个大 Task：`App` 的字段一改，`board.rs`、`grid.rs`、`mod.rs` 必须在同一次提交里跟上，否则树编译不过。

**Files:**
- Modify: `src/ui/app.rs`（字段、`refresh_rows`、构造函数）
- Modify: `src/ui/board.rs`（渲染 + 按键）
- Modify: `src/ui/view.rs`（删 `Scope`、`visible_sessions`）
- Modify: `src/ui/mod.rs`（`toggle_scope`、`selected`、`move_sel`、`enter_session`、`switch_project`、`sync_board_cursor_from_grid`、`help_ctx` 等调用点）
- Modify: `src/ui/grid.rs`（先做到能编译即可，正式改造在 Task 8）

**Interfaces:**
- Consumes: Task 3 的 `group_sessions` / `ProjectGroup`，Task 4 的 `Row` / `flatten` / `anchor_of` / `find_anchor` / `group_of`
- Produces:
  - `App.groups: Vec<ProjectGroup>`、`App.rows: Vec<Row>`、`App.pinned: Vec<String>`、`App.profiles: BTreeMap<String, String>`
  - `App::refresh_rows(&mut self)`
  - `App::current_group(&self) -> Option<&ProjectGroup>`
  - `App::current_dir(&self) -> PathBuf`（方法，不再是字段）
  - `App::selected_session(&self) -> Option<&SessionInfo>`

- [ ] **Step 1: 写失败的测试 —— App 层的分组与光标**

追加到 `src/ui/app.rs` 的 `mod tests`：

```rust
#[test]
fn set_sessions_builds_groups_and_rows() {
    let (mut app, _d) = App::test_app();
    app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/b")]);

    assert_eq!(app.groups.len(), 2);
    assert_eq!(app.rows.len(), 4, "两个组头 + 两个会话行");
}

/// 规则 5：项目只在用户移动光标时变。后台多出来的会话不能把光标推走。
#[test]
fn a_new_session_in_another_project_does_not_move_the_cursor() {
    let (mut app, _d) = App::test_app();
    app.set_sessions(vec![sess(1, "/w/a"), sess(7, "/w/b")]);
    // 光标放到 /w/b 的会话 7 上
    app.list_state.select(Some(3));
    let before = app.current_dir();

    app.set_sessions(vec![sess(1, "/w/a"), sess(4, "/w/a"), sess(7, "/w/b")]);

    assert_eq!(app.current_dir(), before, "当前项目没变");
    assert_eq!(app.selected_session().map(|s| s.id), Some(7));
}

/// 组不塌陷：最后一个会话没了，组变空留在原地，光标落到它自己的组头上。
#[test]
fn a_group_that_loses_its_last_session_keeps_the_cursor() {
    let (mut app, _d) = App::test_app();
    app.pinned = vec!["/w/b".to_string()];
    app.set_sessions(vec![sess(1, "/w/a"), sess(7, "/w/b")]);
    app.list_state.select(Some(3));

    app.set_sessions(vec![sess(1, "/w/a")]);

    assert_eq!(app.groups.len(), 2, "pinned 的空组留在看板上");
    assert_eq!(
        app.current_group().map(|g| g.name.clone()),
        Some("b".to_string()),
        "光标还在 b 上，没有滑回 a"
    );
}

#[test]
fn the_current_project_is_whatever_group_the_cursor_is_in() {
    let (mut app, _d) = App::test_app();
    app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/b")]);

    app.list_state.select(Some(0));
    assert!(app.current_dir().ends_with("a"));
    app.list_state.select(Some(2));
    assert!(app.current_dir().ends_with("b"));
}

#[test]
fn the_grid_leaves_out_stopped_sessions_but_keeps_the_group() {
    let (mut app, _d) = App::test_app();
    app.set_sessions(vec![stopped(1, "/w/a"), sess(2, "/w/a")]);

    assert_eq!(app.groups[0].sessions.len(), 2, "列表里两个都在");
    assert_eq!(app.grid_sessions().len(), 1, "九宫格里只剩没停的那个");
}
```

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib ui::app::tests`
Expected: FAIL，`no field 'groups' on type 'App'`。

- [ ] **Step 3: 改 `App` 的字段**

`src/ui/app.rs`：删掉 `visible`、`grid_visible`、`scope`、`current_dir` 四个字段，换成：

```rust
    /// 守护进程返回的**全量**列表。界面不直接读它，读的是 `groups`。
    pub sessions: Vec<SessionInfo>,
    /// 按项目分好的组。看板画的是它。
    pub groups: Vec<super::view::ProjectGroup>,
    /// `groups` 展平成的行（组头 + 会话），`list_state` 选的是它的下标。
    pub rows: Vec<super::view::Row>,
    /// 用户 pin 上看板的项目（守护进程给的）。跟 `sessions` 一起决定
    /// 看板上出现哪些组——规则 1：有会话的 ∪ pinned 的。
    pub pinned: Vec<String>,
    /// 项目目录 → 上次用的 agent（守护进程给的），组头和底栏 `n` 要用。
    pub profiles: std::collections::BTreeMap<String, String>,
```

`new_inner` 里对应初始化：

```rust
            sessions: Vec::new(),
            groups: Vec::new(),
            rows: Vec::new(),
            pinned: Vec::new(),
            profiles: std::collections::BTreeMap::new(),
```

删掉 `scope: super::view::Scope::CurrentProject,` 和 `current_dir: default_dir,` 两行；`start_dir: default_dir` 保留（`default_dir.clone()` 的 `.clone()` 可以去掉了）。

- [ ] **Step 4: 把 `refresh_visible` 换成 `refresh_rows`**

```rust
    /// 从 `sessions` + `pinned` 重算分组和行，并把光标钉回原来那个东西上。
    ///
    /// **先取锚点再重算**：顺序反了的话锚点取的是新列表里的东西，
    /// 等于没锚。
    pub fn refresh_rows(&mut self) {
        let anchor = self
            .list_state
            .selected()
            .and_then(|i| super::view::anchor_of(&self.groups, &self.rows, i));
        // 折叠状态是用户的选择，重算不能把它抹掉
        let collapsed: Vec<PathBuf> = self
            .groups
            .iter()
            .filter(|g| g.collapsed)
            .map(|g| g.dir.clone())
            .collect();

        self.groups = super::view::group_sessions(&self.sessions, &self.pinned, &self.profiles);
        for g in &mut self.groups {
            g.collapsed = collapsed.contains(&g.dir);
        }
        self.rows = super::view::flatten(&self.groups);

        let next = anchor
            .and_then(|a| super::view::find_anchor(&self.groups, &self.rows, &a))
            // 找不回来（第一次、或者组真的没了）就落在第 0 行。
            // 行数为零时不选——`List` 在空列表上留着 `Some(0)` 会画一条悬空高亮。
            .or(if self.rows.is_empty() { None } else { Some(0) });
        self.list_state.select(next);

        let grid_last = self.grid_sessions().len().saturating_sub(1);
        if let View::Grid { focus, .. } = &mut self.view {
            *focus = (*focus).min(grid_last);
        }
    }

    /// 九宫格真正画出来的那些：所有组的会话按 (项目, id) 连排，去掉已停止的。
    ///
    /// 九宫格是「看几个 agent 此刻在干什么」的地方——停掉的会话没有「此刻」。
    /// 列表那边不筛：停掉的会话还剩唯一一点价值，`u` 回滚、`d` 看改动。
    pub fn grid_sessions(&self) -> Vec<SessionInfo> {
        self.groups
            .iter()
            .flat_map(|g| g.sessions.iter())
            .filter(|s| s.state != crate::session::SessionState::Stopped)
            .cloned()
            .collect()
    }

    /// 光标所在的组。**这是「当前项目」唯一的答案处。**
    pub fn current_group(&self) -> Option<&super::view::ProjectGroup> {
        let i = self.list_state.selected()?;
        let gi = super::view::group_of(&self.rows, i)?;
        self.groups.get(gi)
    }

    /// 新会话开在哪。没有任何组时（只可能发生在还没拉到列表的第一帧）
    /// 退回启动目录。
    pub fn current_dir(&self) -> PathBuf {
        self.current_group()
            .map(|g| g.dir.clone())
            .unwrap_or_else(|| self.start_dir.clone())
    }

    /// 光标停在会话行上时是哪个会话；停在组头上就是 `None`。
    pub fn selected_session(&self) -> Option<&SessionInfo> {
        let i = self.list_state.selected()?;
        match self.rows.get(i)? {
            super::view::Row::Header(_) => None,
            super::view::Row::Session(g, s) => self.groups.get(*g)?.sessions.get(*s),
        }
    }
```

`set_sessions` 改成调用 `refresh_rows()`。

- [ ] **Step 5: 重写 `board.rs` 的渲染**

`src/ui/board.rs` 的 `draw` 换成按行画，组头带竖色条和序号：

```rust
pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let border_style = if app.connected {
        Style::default()
    } else {
        Style::default().fg(Color::Red)
    };
    let title = if app.connected {
        text(Key::BoardTitle, app.lang).to_string()
    } else {
        msg::title_with(app.lang, Key::BoardTitle, text(Key::Disconnected, app.lang))
    };
    // 当前项目：整组左侧一条竖色条。不靠光标行——光标只标「哪一行」，
    // 项目要的是「哪一片」，隔着屏幕就得认得出来。
    let current = app
        .list_state
        .selected()
        .and_then(|i| super::view::group_of(&app.rows, i));

    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| {
            let (gi, bar) = match row {
                super::view::Row::Header(g) | super::view::Row::Session(g, _) => (
                    *g,
                    if Some(*g) == current { "┃" } else { " " },
                ),
            };
            let g = &app.groups[gi];
            let mut spans = vec![Span::styled(bar, Style::default().fg(Color::Cyan))];
            match row {
                super::view::Row::Header(_) => {
                    // 序号只给前九个组：`1`…`9` 直达。第十个起靠 Tab，
                    // 印一个按不动的号码等于在屏幕上说谎。
                    let num = if gi < 9 {
                        format!(" {} ", gi + 1)
                    } else {
                        "   ".to_string()
                    };
                    spans.push(Span::styled(num, dim()));
                    spans.push(Span::raw(if g.collapsed { "▸ " } else { "▾ " }));
                    // 目录被删了：名字标灰并点出来。会话本身还活着（进程的 cwd
                    // 已经打开），组照常留在看板上——让它消失才是真的找不回来了。
                    let gone = !g.dir.exists();
                    spans.push(Span::styled(
                        pad_to(&g.name, 18),
                        if gone {
                            dim()
                        } else {
                            Style::default().add_modifier(Modifier::BOLD)
                        },
                    ));
                    spans.push(Span::styled(pad_to(&truncate(&g.parent, 16), 18), dim()));
                    if gone {
                        spans.push(Span::styled(text(Key::ProjectDirGone, app.lang), dim()));
                    } else if g.sessions.is_empty() {
                        spans.push(Span::styled(text(Key::NoSessionsHere, app.lang), dim()));
                    } else {
                        let agents: Vec<String> = g
                            .agent_counts()
                            .into_iter()
                            .map(|(name, n)| format!("{name}×{n}"))
                            .collect();
                        spans.push(Span::raw(pad_to(&agents.join(" "), 22)));
                        let failed = g.failed();
                        if failed > 0 {
                            spans.push(Span::styled(
                                msg::failed_count(app.lang, failed),
                                Style::default().fg(Color::Red),
                            ));
                        }
                    }
                }
                super::view::Row::Session(_, si) => {
                    let s = &g.sessions[*si];
                    spans.push(Span::raw(format!("  {:>3}  ", s.id)));
                    spans.push(Span::styled(
                        pad_to(status_label(s.state, app.lang), 8),
                        status_style(s.state),
                    ));
                    spans.push(Span::raw(pad_to(&s.profile, 10)));
                    // 会话行不重复项目名——组头已经说了，宽度还给 activity，
                    // 它是屏幕上最先被截断的信息。
                    spans.push(Span::raw(truncate(&s.activity, 76)));
                }
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(title),
            )
            .highlight_symbol("▶ "),
        area,
        &mut app.list_state,
    );
}
```

`use` 里补 `ratatui::prelude::Modifier`（`prelude::*` 已含）与 `super::view`。

- [ ] **Step 6: 改 `board.rs` 的按键**

```rust
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Char('q') if is_plain_key(&key) => app.quit = true,
        KeyCode::Down => super::move_row(app, 1),
        KeyCode::Up => super::move_row(app, -1),
        // 一步换项目。这是日常换项目的**主路径**——`p` 只在要去一个
        // 看板上还没有的项目时才用。
        KeyCode::Tab => super::jump_project(app, 1),
        KeyCode::BackTab => super::jump_project(app, -1),
        KeyCode::Char(c @ '1'..='9') if is_plain_key(&key) => {
            super::goto_project(app, c as usize - '1' as usize)
        }
        // 折叠/展开当前组。看板上左右键原来没有用途，九宫格那边是移动焦点，
        // 两个视图各自的方向语义不冲突。
        KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => super::toggle_collapse(app),
        KeyCode::Char('n') | KeyCode::Char('N') if is_plain_key(&key) => {
            open_new_session(app, key.code)
        }
        KeyCode::Char('p') if is_plain_key(&key) => open_project_picker(app),
        KeyCode::Char('x') if is_plain_key(&key) => super::unpin_current(app),
        KeyCode::Char('c') if is_plain_key(&key) => open_secrets(app),
        KeyCode::Char('l') if is_plain_key(&key) => super::open_settings(app),
        KeyCode::Enter => {
            if let Some(id) = app.selected_session().map(|s| s.id) {
                super::enter_session(app, id);
            }
        }
        KeyCode::Char('g') if is_plain_key(&key) => super::toggle_view_mode(app),
        KeyCode::Char('?') if is_plain_key(&key) => super::keys::open(app),
        KeyCode::Char('u') | KeyCode::Char('s') | KeyCode::Char('d') if is_plain_key(&key) => {
            app.message = match app.selected_session().map(|s| s.id) {
                Some(id) => session_action(app, key.code, id),
                None => text(Key::NoSessionSelected, app.lang).into(),
            };
        }
        _ => {}
    }
    Ok(())
}
```

`'a'` 那一支删掉。

- [ ] **Step 7: 在 `mod.rs` 里补上四个新的移动函数，删掉旧的**

```rust
/// 上下走一行。行里既有组头也有会话行，两种都能停——空组只有组头，
/// 停不上去的话那个项目就永远选不中，`n` 也就去不了。
pub(crate) fn move_row(app: &mut App, delta: i32) {
    move_sel_n(&mut app.list_state, app.rows.len(), delta);
}

/// 跳到下 / 上一个项目的组头。到头回绕：四个项目里 `Tab` 转一圈回到起点，
/// 比「按到底就不动了」好解释。
pub(crate) fn jump_project(app: &mut App, delta: i32) {
    if app.groups.is_empty() {
        return;
    }
    let cur = app
        .list_state
        .selected()
        .and_then(|i| view::group_of(&app.rows, i))
        .unwrap_or(0) as i32;
    let n = app.groups.len() as i32;
    let next = (cur + delta).rem_euclid(n) as usize;
    goto_project(app, next);
}

/// 直达第 N 个项目（0 基）。越界什么都不做——按了 `7` 而只有三个项目时，
/// 不动比跳到最后一个更好懂。
pub(crate) fn goto_project(app: &mut App, gi: usize) {
    if gi >= app.groups.len() {
        return;
    }
    if let Some(i) = app.rows.iter().position(|r| *r == view::Row::Header(gi)) {
        app.list_state.select(Some(i));
    }
}

/// 折叠 / 展开光标所在的组。折完把光标收到组头上——不然它会停在一行
/// 已经不存在的会话上。
pub(crate) fn toggle_collapse(app: &mut App) {
    let Some(gi) = app
        .list_state
        .selected()
        .and_then(|i| view::group_of(&app.rows, i))
    else {
        return;
    };
    app.groups[gi].collapsed = !app.groups[gi].collapsed;
    app.rows = view::flatten(&app.groups);
    goto_project(app, gi);
}
```

删掉 `toggle_scope`、`switch_project`、`selected()`、`move_sel()`。`enter_session` 里那段跨项目改写 `current_dir` 的代码（`src/ui/mod.rs:887-900`）整段删掉，只留 `need_sessions`、`view`、`explained_failure`、`Scroll::Bottom` 四件事。

`open_new_session` 里的 `let dir = app.current_dir.display().to_string();` 改成 `app.current_dir().display().to_string()`；`Request::LastProfile { dir }` 同样用它。`open_project_picker` 里的 `app.current_dir.parent()` 改成 `app.current_dir().parent()`。

`help_ctx` 里 `has_sessions` 改成 `!app.rows.is_empty()`，`selected` 改成走 `app.selected_session()`。

`unpin_current` 在 Task 6 实现，本 Task 先放一个占位实现让树编译：

```rust
/// `x`：把空组从看板上拿掉。真正的落盘在 Task 6 接上守护进程。
pub(crate) fn unpin_current(app: &mut App) {
    let _ = app;
}
```

- [ ] **Step 8: 删掉 `Scope`，并让 `grid.rs` / `view.rs` 编译通过**

`src/ui/view.rs`：删除 `enum Scope`、`fn visible_sessions`，以及它们的测试
（`current_project_scope_keeps_only_that_project_in_order`、`all_projects_scope_returns_everything_untouched`、
`a_project_with_no_sessions_yields_an_empty_list`、`a_symlinked_project_dir_still_matches_its_sessions`、
`a_session_whose_dir_was_deleted_stays_under_its_own_project`）。`same_project` 也一并删除——
唯一的调用点是刚被删掉的那段 `enter_session` 逻辑。`canon` 留着，`group_sessions` 在用。

`idle_help` 和 `board_keys` 的 `scope` 参数暂时改成不传（`idle_help(view, lang, ctx)`），
`board_keys` 里 `("a", scope_key)` 那一条删掉。底栏的正式改造在 Task 7。

`src/ui/grid.rs`：把所有 `app.grid_visible` 换成 `app.grid_sessions()`，
`app.scope` 相关的两处（`draw` 的 `scope` 参数、`'a'` 按键）删掉，
`Scope::AllProjects` 那个条件分支改成**无条件**加项目名（正是 Task 8 要的行为，这里顺手做到）。
`sync_board_cursor_from_grid` 改成按会话 id 在 `app.rows` 里找对应的 `Row::Session`。

- [ ] **Step 9: 加这个 Task 用到的两条文案**

每个 Task 自带它引用的词条，否则树编译不过。`src/i18n.rs` 的 `Key` 枚举加一条，
并在译文表和 `src/i18n.rs:1120` 那份穷举列表里同步补上：

```rust
    /// 组头上：这个项目的目录已经不在了
    ProjectDirGone,
```

```rust
        ProjectDirGone => t!(lang, en: "folder is gone", zh: "目录不在了"),
```

`msg` 模块加一条带数字的（跟 `session_failed` 同一套做法）：

```rust
    /// 组头上的「N 个出错」。会话静默失败是 dct 最贵的失败模式，
    /// 组头上必须一眼看得见。
    pub fn failed_count(lang: Lang, n: usize) -> String {
        match lang {
            Lang::En => format!("{n} failed"),
            Lang::Zh => format!("{n} 个出错"),
        }
    }
```

`NoSessionsHere` 的译文改成组头用的短句（原来是整屏空态的长句）：

```rust
        NoSessionsHere => t!(lang, en: "no sessions yet", zh: "还没有会话"),
```

- [ ] **Step 10: 跑测试**

Run: `cargo test`
Expected: PASS。既有测试里凡是断言 `a` 键、`Scope`、`visible` 的一律随之删除或改写——
`src/ui/mod.rs` 的 `bottom_bar_shows_current_project` 改成断言底栏中段有项目名（Task 7 会再动它，
本 Task 先让它指向 `app.current_dir()`）。

- [ ] **Step 11: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui/ src/i18n.rs
git commit -m "feat: the board groups by project and the cursor decides which one is current"
```

---

### Task 6: `p` 摆项目、`x` 拿掉、启动时补上 `start_dir`

**Files:**
- Modify: `src/ui/mod.rs`（`run` 的拉取循环、`open_project_picker` 的确认路径、`unpin_current`）
- Modify: `src/ui/pick.rs`（`handle_pick_project` 的两条确认路径）

**Interfaces:**
- Consumes: Task 2 的 `Request::PinProject` / `UnpinProject` / `Response::Projects { recent, pinned }`
- Produces: `pub(crate) fn pin_project(app: &mut App, dir: PathBuf)`

- [ ] **Step 1: 写失败的测试**

追加到 `src/ui/pick.rs` 的 `mod tests`：

```rust
/// `p` 选定一个项目之后，它必须以一个组的形式出现在看板上，并且光标落进去——
/// 否则用户按完 `p` 什么都没发生，`n` 也去不了那儿。
#[test]
fn confirming_a_project_puts_it_on_the_board_and_moves_the_cursor_there() {
    let (mut app, d) = App::test_app();
    let target = d.path().join("newproj");
    std::fs::create_dir(&target).unwrap();
    app.set_sessions(vec![]);

    super::super::pin_project(&mut app, target.clone());

    assert!(
        app.pinned.iter().any(|p| p.ends_with("newproj")),
        "要进 pinned"
    );
    assert_eq!(
        app.current_group().map(|g| g.name.clone()),
        Some("newproj".to_string()),
        "光标要落在新组上"
    );
}

/// `x` 只能拿掉空组。有会话的组必须拒绝——「顺便停掉所有会话」是个
/// 用户没要求过的复合动作，而 `s` 已经能一个一个停。
#[test]
fn removing_a_group_that_still_has_sessions_is_refused() {
    let (mut app, _d) = App::test_app();
    app.pinned = vec!["/w/a".to_string()];
    app.set_sessions(vec![sess_in(1, "/w/a")]);
    app.list_state.select(Some(0));

    super::super::unpin_current(&mut app);

    assert_eq!(app.groups.len(), 1, "组还在");
    assert!(app.message.error, "要给一句红字提示");
}
```

（`sess_in` 沿用 `pick.rs` 测试里已有的会话构造助手；没有就照 `board.rs::sess` 抄一个。）

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib ui::pick::tests`
Expected: FAIL，`cannot find function 'pin_project'`。

- [ ] **Step 3: 实现 `pin_project` 与 `unpin_current`**

`src/ui/mod.rs`：

```rust
/// `p` 选定之后：告诉守护进程记下来、更新本地 `pinned`、重算行、把光标
/// 送到那个组上、回家视图。
///
/// 五步必须整套发生。分开写的话，漏掉重算的那条路会让屏幕停在旧的一屏，
/// 而用户刚刚明确选了一个项目——那正是上一版被判为「混乱」的手感。
pub(crate) fn pin_project(app: &mut App, dir: std::path::PathBuf) {
    let d = dir.display().to_string();
    // 落盘失败不拦路：pinned 是便利性状态，本地先摆上，用户这一次照样能用。
    let _ = app
        .client()
        .and_then(|c| c.call(Request::PinProject { dir: d.clone() }));
    if !app.pinned.contains(&d) {
        app.pinned.push(d);
    }
    app.refresh_rows();
    if let Some(gi) = app.groups.iter().position(|g| g.dir == view::canon(&dir)) {
        goto_project(app, gi);
    }
    app.view = home_view(app);
}

/// `x`：把光标所在的空组从看板上拿掉。
pub(crate) fn unpin_current(app: &mut App) {
    let Some(g) = app.current_group() else {
        return;
    };
    if !g.sessions.is_empty() {
        app.message = Msg::err(crate::i18n::text(crate::i18n::Key::GroupNotEmpty, app.lang).into());
        return;
    }
    let d = g.dir.display().to_string();
    let _ = app
        .client()
        .and_then(|c| c.call(Request::UnpinProject { dir: d.clone() }));
    app.pinned.retain(|p| p != &d);
    app.refresh_rows();
}
```

- [ ] **Step 4: 把 `pick.rs` 的两条确认路径接过去**

`src/ui/pick.rs` 的 `handle_pick_project` 里，原来调用 `super::super::switch_project(app, dir)`
的两处（列表选中确认、浏览器 `Enter` 确认）全部改成 `super::super::pin_project(app, dir)`。

- [ ] **Step 5: 每轮拉取时带回 `pinned` 和 `profiles`**

`src/ui/mod.rs::run` 里拉 `Request::List` 的那一段之后，补一次 `Request::Projects`——
但**不是每帧都拉**：它只在 `need_sessions` 为真时跟着 `List` 一起拉，跟会话列表同一个节奏。

```rust
        if app.need_sessions {
            if let Ok(Response::Projects { recent: _, pinned }) =
                app.client().and_then(|c| c.call(Request::Projects))
            {
                app.pinned = pinned;
            }
        }
```

`profiles` 用同一次往返拿不到（`Response::Projects` 不带它），改由 `ProjectGroup.last_profile`
在需要时现问：底栏画 `n 新建 <agent>` 之前，`App` 里缓存一次结果即可。**为避免每帧一次
阻塞往返**，在 `refresh_rows` 之后、`pinned` 有变化或组集合有变化时才问一次：

```rust
/// 把每个组「上次用的 agent」补齐。只在组集合变化时调用一次——
/// 每帧一次阻塞往返会让界面在守护进程忙的时候一顿一顿。
pub(crate) fn refresh_project_profiles(app: &mut App) {
    let dirs: Vec<String> = app.groups.iter().map(|g| g.dir.display().to_string()).collect();
    for d in dirs {
        if app.profiles.contains_key(&d) {
            continue;
        }
        if let Ok(Response::LastProfile(Some(p))) = app
            .client()
            .and_then(|c| c.call(Request::LastProfile { dir: d.clone() }))
        {
            app.profiles.insert(d, p);
        }
    }
    app.refresh_rows();
}
```

在 `run()` 里紧跟着 `set_sessions` 之后调用它。开会话成功之后（`open_new_session` 的
`Response::Created` 分支）把 `app.profiles` 里那一项直接更新成刚用的 agent，省一次往返。

- [ ] **Step 6: 启动时保证至少有一个组**

`run()` 第一次拉完列表之后：

```rust
    // 全新安装、或者第一次跑这一版：pinned 是空的，把启动目录补进去。
    // 这保证看板永远至少有一个组——光标永远有地方落、`n` 永远有目标，
    // 「一个组都没有」这个状态在结构上不存在。
    if app.pinned.is_empty() && app.groups.is_empty() {
        pin_project(&mut app, app.start_dir.clone());
    }
```

- [ ] **Step 7: 加这个 Task 用到的文案**

`src/i18n.rs`：`Key` 枚举加一条，译文表和穷举列表同步补上。

```rust
    /// `x` 按在一个还有会话的组上
    GroupNotEmpty,
```

```rust
        GroupNotEmpty => t!(
            lang,
            en: "This project still has sessions. Stop them first.",
            zh: "这个项目还有会话，先停掉才能移除。"
        ),
```

- [ ] **Step 8: 跑测试，确认通过**

Run: `cargo test`
Expected: PASS。

- [ ] **Step 9: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui/mod.rs src/ui/pick.rs src/i18n.rs
git commit -m "feat: p puts a project on the board, x takes an empty one off"
```

---

### Task 7: 底栏三段

**Files:**
- Modify: `src/ui/mod.rs:1331-1485`（`draw` 的底栏部分）
- Modify: `src/ui/view.rs`（`idle_help`、`board_keys`）

**Interfaces:**
- Consumes: `App::current_group()`
- Produces: `const PROJECT_COLS: u16 = 16;`、`idle_help(view, lang, ctx) -> Vec<HelpItem>`（≤ 4 条）

- [ ] **Step 1: 写失败的测试**

追加到 `src/ui/mod.rs` 的 `mod tests`：

```rust
/// 左段和中段永不让位：一条长消息、断连状态，都不能把「我在哪个项目」顶掉。
/// 老版本正是「已切到 X」这类消息把项目信息整个盖掉的。
#[test]
fn the_project_segment_survives_a_long_message_and_a_disconnect() {
    let (mut app, _d) = app_with_one_agent_session(View::Board);
    let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();

    app.message = "x".repeat(400).into();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let name = app.current_group().unwrap().name.clone();
    assert!(bar_text(&term).contains(&name), "长消息不能盖掉项目名");

    app.connected = false;
    term.draw(|f| draw(f, &mut app)).unwrap();
    assert!(bar_text(&term).contains(&name), "断连也不能盖掉项目名");
}

/// 右段硬上限 3 条动作 + 一个 `?`。终端再宽也不多塞——一行的内容随宽度
/// 变化本身就是不可预期。
#[test]
fn the_action_segment_never_exceeds_three_keys_plus_the_door() {
    for w in [80u16, 120, 200] {
        let (mut app, _d) = app_with_one_agent_session(View::Board);
        let mut term = Terminal::new(TestBackend::new(w, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let items = idle_help(&app.view, app.lang, help_ctx(&app));
        assert!(
            items.len() <= 4,
            "{w} 列下右段有 {} 条，超过 3 个动作 + ?",
            items.len()
        );
    }
}

/// 光标停在组头上时不该写 `Enter 进会话`——按下去没有对象。
#[test]
fn a_header_row_does_not_advertise_entering_a_session() {
    let (mut app, _d) = app_with_one_agent_session(View::Board);
    app.list_state.select(Some(0));
    let items = idle_help(&app.view, app.lang, help_ctx(&app));
    let joined = crate::i18n::help_text(&items);
    assert!(!joined.contains("Enter"), "组头行上不写 Enter：{joined}");
}
```

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib ui::tests`
Expected: FAIL（`idle_help` 还接收 `scope` 参数 / 右段条数超限）。

- [ ] **Step 3: 重写 `idle_help` 的看板分支**

`src/ui/view.rs`，把 `board_keys` 换成一个**按上限挑三条**的版本：

```rust
/// 看板和九宫格共用的那张按键表。**硬上限三条动作 + 一个 `?`。**
///
/// 上限不是「放得下就多塞」：一行的内容随终端宽度变化，本身就是不可预期的
/// ——用户在窄终端上学会的键，到宽终端上位置全变了。剩下的键全在 `?` 后面，
/// 那扇门永远在。
///
/// 三条选谁，按「此刻最可能做的」：光标停在会话行上，最可能是进去看看；
/// 停在组头上，最可能是在这里开一个新会话。
fn board_keys(
    ctx: HelpCtx,
    enter: (&'static str, crate::i18n::Key),
    can_remove: bool,
) -> Vec<(&'static str, crate::i18n::Key)> {
    use crate::i18n::Key;
    let mut keys: Vec<(&'static str, Key)> = Vec::new();
    if ctx.selected.is_some() {
        keys.push(enter);
    }
    keys.push(("n", Key::New));
    if can_remove {
        keys.push(("x", Key::RemoveProject));
    }
    keys.push(("Tab", Key::SwitchProject));
    keys.truncate(3);
    keys.push(("?", Key::MoreKeys));
    keys
}
```

`idle_help` 的签名去掉 `scope`，看板/九宫格两支改成：

```rust
        View::Board => help_items(&board_keys(ctx, ("Enter", Key::Enter), ctx.can_remove), lang),
        View::Grid { .. } => help_items(&board_keys(ctx, ("Enter", Key::Zoom), ctx.can_remove), lang),
```

`HelpCtx` 加一个字段：

```rust
    /// 光标所在的组是不是「pinned 且没有会话」——只有这种组能按 `x` 拿掉。
    pub can_remove: bool,
```

`mod.rs::help_ctx` 填它：

```rust
        can_remove: app
            .current_group()
            .map(|g| g.pinned && g.sessions.is_empty())
            .unwrap_or(false),
```

`View::Attached` 那一支的右段收成一条：

```rust
        View::Attached(_) => help_items(&[("F3", Key::NextSession)], lang),
```

`src/i18n.rs` 加三条新 `Key`（译文表和 `src/i18n.rs:1120` 的穷举列表同步补上）：

```rust
    /// `x` 的说明
    RemoveProject,
    /// 看板 `Enter` 的说明
    Enter,
    /// 九宫格 `Enter` 的说明
    Zoom,
```

```rust
        RemoveProject => t!(lang, en: "remove", zh: "移除"),
        Enter => t!(lang, en: "open", zh: "进会话"),
        Zoom => t!(lang, en: "zoom in", zh: "放大"),
```

`SwitchProject` 的译文保持「换项目」不变——`Tab` 用的就是它。

- [ ] **Step 4: 底栏改成三段**

`src/ui/mod.rs`，在 `ESCAPE_HINT_COLS` 旁边加：

```rust
/// 底栏中段：当前项目名占的列数。按显示宽度截断，CJK 项目名同样算两列。
const PROJECT_COLS: u16 = 16;
```

`help_cols` 的计算把中段也减掉：

```rust
    let help_cols = f
        .area()
        .width
        .saturating_sub(2 + ESCAPE_HINT_COLS + 2 + PROJECT_COLS + 2)
        as usize;
```

删掉边框标题里那句「当前项目：…」（`src/ui/mod.rs:1450-1454`），`block` 改成
`Block::default().borders(Borders::ALL)`。布局拆三段：

```rust
    let bar = Layout::horizontal([
        Constraint::Length(ESCAPE_HINT_COLS + 2),
        Constraint::Length(PROJECT_COLS + 2),
        Constraint::Min(0),
    ])
    .split(inner);
    f.render_widget(
        Paragraph::new(escape_hint(&app.view, app.lang)).style(Style::default().fg(Color::Cyan)),
        bar[0],
    );
    // 中段：当前项目。**永不让位**——消息和断连提示只能吃掉右段。
    // 老版本里「已切到 X」这类操作反馈会把项目信息整个顶掉，用户于是
    // 既不知道自己在哪，也不知道 `n` 会开在哪。
    let project = app
        .current_group()
        .map(|g| g.name.clone())
        .unwrap_or_default();
    f.render_widget(
        Paragraph::new(widgets::truncate(&project, PROJECT_COLS as usize))
            .style(Style::default().add_modifier(Modifier::BOLD)),
        bar[1],
    );
    f.render_widget(Paragraph::new(help_lines).style(style), bar[2]);
```

`debug_assert_eq!(help_cols, bar[1].width as usize, ...)` 改成对 `bar[2]`。

- [ ] **Step 5: 让 `n` 那一条带上 agent 名**

`board_keys` 给出的是静态词条，agent 名是动态的。在 `mod.rs` 画之前替换：

```rust
    // `n 新建 claude` —— 括号去掉，agent 名直接跟在后面。这个项目没用过
    // agent 就只写 `n 新建`（按下去会弹选择器，那正是该有的行为）。
    let agent = app.current_group().and_then(|g| g.last_profile.clone());
    let items = idle_help(&app.view, app.lang, help_ctx(app));
    let items: Vec<crate::i18n::HelpItem> = items
        .into_iter()
        .map(|mut it| {
            if it.key == "n" {
                if let Some(a) = &agent {
                    it.label_owned = Some(format!("{} {}", it.label, a));
                }
            }
            it
        })
        .collect();
```

为此 `HelpItem`（`src/i18n.rs:482`）加一个可选的动态标签：

```rust
pub struct HelpItem {
    pub key: &'static str,
    pub label: &'static str,
    /// 动态标签。有它就顶掉 `label`——底栏的 `n 新建 claude` 里那个 agent 名
    /// 是运行时才知道的，塞不进 `&'static str` 的词条表。
    pub label_owned: Option<String>,
}
```

`Display`、`help_items`、`fit_help`、`help_spans` 里凡是读 `label` 的，改成
`self.label_owned.as_deref().unwrap_or(self.label)`。

- [ ] **Step 6: 跑测试，确认通过**

Run: `cargo test`
Expected: PASS。既有的 `the_keys_that_survive_eighty_columns_are_the_ones_that_matter`、
`a_wide_terminal_shows_more_keys` 与新的上限规则冲突——前者改成断言 `n`/`Tab`/`?` 都在，
后者删除（「宽终端多显示几个键」正是这次要取消的行为，在提交信息里说明）。

- [ ] **Step 7: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui/mod.rs src/ui/view.rs src/i18n.rs
git commit -m "feat: a three-part status bar where the project never gets pushed off"
```

---

### Task 8: 九宫格 —— 按项目连排、格子恒带项目名、`Tab` 与数字键

**Files:**
- Modify: `src/ui/grid.rs`

**Interfaces:**
- Consumes: `App::grid_sessions()`（已按 (项目, id) 排好）、`jump_project` / `goto_project`
- Produces: 无新公开接口

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn tiles_are_ordered_by_project_then_id() {
    let (mut app, _d) = App::test_app();
    app.set_sessions(vec![
        sess(9, "/w/b"),
        sess(2, "/w/a"),
        sess(5, "/w/a"),
    ]);
    assert_eq!(
        app.grid_sessions().iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![2, 5, 9],
        "同一项目的格子必须连排，翻页不打散"
    );
}

#[test]
fn every_tile_names_its_project() {
    let (mut app, _d) = App::test_app();
    app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/b")]);
    app.view = View::grid(0);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| draw(f, term_area(&term), &mut app)).unwrap();

    let text = buffer_text(term.backend().buffer());
    assert!(text.contains('a') && text.contains('b'), "两个格子各自写着自己的项目");
}

#[test]
fn tab_moves_the_focus_to_the_next_project() {
    let (mut app, _d) = App::test_app();
    app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/b")]);
    app.view = View::grid(0);

    handle_key(&mut app, key(KeyCode::Tab)).unwrap();

    assert_eq!(
        app.current_group().map(|g| g.name.clone()),
        Some("b".to_string())
    );
}
```

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib ui::grid::tests`
Expected: FAIL（`Tab` 未绑定）。

- [ ] **Step 3: 加 `Tab` / `BackTab` / 数字键**

`src/ui/grid.rs::handle_key`，在方向键之后插入：

```rust
        // 跟看板同一个键、同一个语义。九宫格里换项目 = 焦点跳到那个项目的
        // 第一个格子，同时看板那边的光标也跟着走（两个模式共用一个光标）。
        KeyCode::Tab => {
            super::jump_project(app, 1);
            focus_first_of_current_group(app);
        }
        KeyCode::BackTab => {
            super::jump_project(app, -1);
            focus_first_of_current_group(app);
        }
        KeyCode::Char(c @ '1'..='9') if is_plain_key(&key) => {
            super::goto_project(app, c as usize - '1' as usize);
            focus_first_of_current_group(app);
        }
        KeyCode::Char('x') if is_plain_key(&key) => super::unpin_current(app),
```

删掉 `KeyCode::Char('a')` 那一支。新增：

```rust
/// 把焦点挪到当前组的第一个活会话上。找不到（这个组全停了、或者是空组）
/// 就不动——空组在九宫格里没有格子，硬挪会指到别人家的格子上去。
fn focus_first_of_current_group(app: &mut App) {
    let Some(g) = app.current_group() else { return };
    let Some(first) = g
        .sessions
        .iter()
        .find(|s| s.state != SessionState::Stopped)
        .map(|s| s.id)
    else {
        return;
    };
    if let Some(i) = app.grid_sessions().iter().position(|s| s.id == first) {
        app.view = View::grid(i);
    }
}
```

- [ ] **Step 4: 格子标题恒带项目名**

Task 5 已经把 `Scope::AllProjects` 那个条件去掉了。这里确认标题拼法并把项目名放在末尾、
用 `dim()`：

```rust
        let project = std::path::Path::new(&info.dir)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| info.dir.clone());
        title.push(Span::styled(format!("{project} "), dim()));
```

九宫格的空屏文案改成一句（`src/ui/grid.rs:303-306` 那个 `match scope`）：

```rust
        // 「一个组都没有」不可能发生（启动时会补上 start_dir），所以这里
        // 只剩一种情况：有项目，但没有一个活着的会话。
        let empty = text(Key::NoSessionsRunningPressN, lang);
```

`src/i18n.rs` 加这一条（译文表和穷举列表同步）：

```rust
    /// 九宫格空屏
    NoSessionsRunningPressN,
```

```rust
        NoSessionsRunningPressN => t!(
            lang,
            en: "No sessions yet — press n to start one",
            zh: "还没有会话，按 n 新建"
        ),
```

- [ ] **Step 5: 跑测试，确认通过**

Run: `cargo test --lib ui::grid::tests`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui/grid.rs src/i18n.rs
git commit -m "feat: grid tiles name their project and keep each project together"
```

---

### Task 9: i18n 收尾与文档

**Files:**
- Modify: `src/i18n.rs`
- Modify: `README.md`、`README.zh-CN.md`

**Interfaces:**
- Consumes: 前面所有 Task 引用的 `Key` 变体
- Produces: 完整的词条表

- [ ] **Step 1: 删掉废弃词条，补上 `p` 专用的那一条**

Task 5–8 各自带来了它们引用的词条，这里只做收尾。

删除四条（枚举、译文表、以及 `src/i18n.rs:1120` 起那份「所有 Key 都有译文」的穷举列表
三处都要删，漏一处编译不过）：

```
SeeAllProjects
ThisProjectOnly
BoardTitleAllProjects
NoSessionsAtAll
```

新增一条 —— `Tab` 是「换项目」、`p` 是「加项目」，两件事不能共用一句话：

```rust
    /// `p` 的说明：把一个看板上还没有的项目摆上来
    AddProject,
```

```rust
        AddProject => t!(lang, en: "add project", zh: "加项目"),
```

- [ ] **Step 2: 跑穷举测试**

Run: `cargo test --lib i18n`
Expected: PASS（`src/i18n.rs:1120` 那份列表必须跟枚举完全对齐，缺一个就红）。

- [ ] **Step 3: 更新 `?` 浮层的完整按键表**

`src/ui/keys.rs` 里补上 `Tab 换项目`、`1…9 直达项目`、`x 移除项目`、`←→/空格 折叠`，
删掉 `a` 那一行。

- [ ] **Step 4: 更新两份 README**

`README.zh-CN.md` 的「看板」小节按新键表重写：

```markdown
## 看板

看板按**项目分组**。同一个项目的会话聚在一起，组头一行写着这个项目在用哪些 agent、
有没有出错。左侧那条竖线标出你现在在哪个项目——`n` 就开在那儿。

| 键 | |
|---|---|
| `Tab` `Shift+Tab` | 换项目，一步到位 |
| `1`…`9` | 直达第 N 个项目 |
| `n` | 在当前项目新建会话，用这个项目上次那个 agent |
| `N` | 新建会话，自己选 agent |
| `p` | 把一个新项目摆上看板 |
| `x` | 把一个还没有会话的项目从看板上拿掉 |
| `←` `→` `空格` | 折叠 / 展开当前项目 |
| `↑` `↓` | 上下走 |
| `Enter` | 进会话 |
| `u` | 撤销，退回上一张快照 |
| `s` | 停掉会话 |
| `d` | 这个会话改了什么 |
| `c` | 管密钥 |
| `g` | 九宫格 |
| `q` | 退出看板，会话继续跑 |
| `Ctrl+Q` | 不管在哪，退一层 |

每个项目**各自**记着上次用的 agent：在 A 项目按 `n` 开 claude、在 B 项目按 `n` 开 codex，
底栏在你按之前就写着这一下会开哪个。
```

删掉 `README.zh-CN.md:158` 那句「换项目目前只有一个『最近用过』的列表加手动粘路径，
没有目录浏览器。这块要重做。」——目录浏览器早已做完，而换项目现在根本不走浮层。
`README.md` 做同样的改动。

- [ ] **Step 5: 全量检查并提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/i18n.rs src/ui/keys.rs README.md README.zh-CN.md
git commit -m "docs: the board is grouped by project now, and every project keeps its own agent"
```

---

## 自查

**Spec 覆盖：**

| Spec 小节 | Task |
|---|---|
| 一、分组纯函数 `ProjectGroup` / `group_sessions` | 3 |
| 二、光标模型 `Row` / `rows` | 4、5 |
| 三、光标钉住不变式 | 4（纯函数）、5（`refresh_rows`） |
| 四、规则 1（有会话 ∪ pinned） | 3、6 |
| 四、规则 2（竖色条） | 5 |
| 四、规则 3（Tab / 数字 / p / x / 折叠） | 5、6、8 |
| 四、规则 4（`n` 落点 + 底栏写 agent） | 6、7 |
| 四、规则 5（只有移动光标才变） | 5（删 `enter_session` 改写）、4（锚点） |
| 五、底栏三段 | 7 |
| 六、九宫格 | 8（排序/标签/键）、5（去掉 scope 条件） |
| 七、持久化与协议 | 1、2 |
| 八、i18n | 9 |
| 要删的东西 | 5（Scope/visible/current_dir/switch_project）、9（词条） |
| 错误处理：目录被删 | 9（`ProjectDirGone` 词条）+ 3（`canon` 退化保留） |
| 错误处理：`x` 非空组 | 6 |
| 破坏性变更：`a` 消失、协议 +1 | 5、2 |

**词条归属：** 每个 Task 自带它引用的 `Key`（Task 5 带 `ProjectDirGone` 和 `msg::failed_count`，
Task 6 带 `GroupNotEmpty`，Task 7 带 `RemoveProject`/`Enter`/`Zoom`，Task 8 带
`NoSessionsRunningPressN`），Task 9 只做删除和 `AddProject`。这样每个 Task 结束时
树都编译得过——把词条全堆到最后一个 Task，中间七个 Task 全部编译失败。

**执行顺序不能改：** 1 → 2 是存储到协议，3 → 4 是两组纯函数（只新增，树始终是绿的），
5 是唯一一次大切换（`App` 字段一改，`board`/`grid`/`mod` 必须同一次提交跟上），
6 → 9 都是在 5 的地基上各修一面墙，彼此独立。
