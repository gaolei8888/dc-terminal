# dct 核对 CLAUDE.md 仓库清单 —— 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 开工前告诉用户，会被注入 agent 的那份 CLAUDE.md 漏掉了磁盘上哪些真实存在的仓库。

**Architecture:** 一个不碰文件系统的纯函数（`missing_repos`）+ 一个薄的收集层（向上走目录、读 CLAUDE.md、列 git 子目录），全部跑在**界面进程**里，不动协议、不进守护进程。结果作为一条底栏消息显示给**人**，不注入给 agent。

**Tech Stack:** Rust，标准库 `std::fs` / `std::path`，无新依赖。

## Global Constraints

- 提交信息用**英文**，不带 AI 署名（`Co-Authored-By` 一行都不要）。
- 所有用户可见文案走 `src/i18n.rs`，中英文各一份；带参的进 `msg` 模块（每条一个函数，不用 `{}` 模板）。
- 目标用户不是程序员：不出现 git / CLI 黑话，错误说人话。
- 每个 Task 结束前跑：`cargo fmt`、`cargo clippy --all-targets`（零告警）、`cargo test`。
- 用 `~/.cargo/bin/cargo`（PATH 里没有 `cargo`）。

## 相对 spec 的一处调整

Spec 的「何时检查」写的是「会话创建成功之后」。实现改为 **dct 启动时 + `switch_project` 之后**，理由：

1. `Response::Created` 在 `src/ui/mod.rs` 里有**两个**分散的调用点（约 261 行和 1012 行），两处各加一次容易漏掉一处。
2. 每开一次会话报一次会变成噪音；而 `current_dir` 只在这两个时刻确定或变化。
3. 「按 `p` 换到某个项目」正是用户要在那里开工的时刻，警告在这时最有用。

同时更新 spec 的那一节，保持两份文档一致（Task 3）。

---

### Task 1: `missing_repos` 纯函数与收集层

**Files:**
- Create: `src/claudemd.rs`
- Modify: `src/lib.rs`（加一行 `pub mod claudemd;`）
- Test: 同文件内 `#[cfg(test)] mod tests`（本仓库惯例，测试跟实现同文件）

**Interfaces:**
- Produces:
  - `pub struct Gap { pub doc: std::path::PathBuf, pub missing: Vec<String> }`
  - `pub fn missing_repos(claude_md: &str, repos: &[String]) -> Vec<String>`
  - `pub fn check(start: &std::path::Path, home: &std::path::Path) -> Vec<Gap>`

- [ ] **Step 1: 写失败的测试**

新建 `src/claudemd.rs`，先只写测试和空实现：

```rust
//! 核对项目清单：会被注入 agent 的那份 CLAUDE.md 里，有没有漏掉磁盘上
//! 真实存在的仓库。
//!
//! 2026-08-06 有一整轮工作照着一份漏了 `dc_desktop` 的清单跑进了错误的仓库，
//! 而旧实现还在、还能跑，全程正反馈，没有任何一刻撞墙。事后一比对发现
//! `dc-terminal` 自己也漏了——同一份文档同时漏了两个项目。
//!
//! 这里只查**硬缺口**（少了一整个仓库）。「某一行的描述过时了」查不了：
//! 那句话在语法上、在文件里都完全正常。那一半靠文档自己顶上那条
//! 「这张表会过时，动手前先 ls 一遍」的约定，不靠代码。

use std::path::{Path, PathBuf};

/// 一份 CLAUDE.md 漏掉的仓库。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    pub doc: PathBuf,
    pub missing: Vec<String>,
}

pub fn missing_repos(_claude_md: &str, _repos: &[String]) -> Vec<String> {
    unimplemented!()
}

pub fn check(_start: &Path, _home: &Path) -> Vec<Gap> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// 核心用例，就是 2026-08-06 那次事故：清单里没有 `dc_desktop`。
    #[test]
    fn missing_repos_finds_a_repo_the_doc_never_mentions() {
        let doc = "| `dc_workbench` | Go | 本地版全栈 | 47832 |";
        let got = missing_repos(doc, &names(&["dc_workbench", "dc_desktop"]));
        assert_eq!(got, names(&["dc_desktop"]));
    }

    /// 宽进严出：文档用什么形式提到都算数。要求「必须出现在表格行里」
    /// 会让一条正文里的路径引用被误报，而一条会误报的警告，用户看两次
    /// 就开始无视它——那时它连真的缺口也拦不住了。
    #[test]
    fn missing_repos_accepts_any_mention_not_just_a_table_row() {
        let doc = "所有推理走 dc_llm 网关；细节见 dc_llm/README.md。";
        assert!(missing_repos(doc, &names(&["dc_llm"])).is_empty());
    }

    /// 报出来的顺序跟传入顺序一致：底栏那句话每次都该长得一样，
    /// 顺序乱跳会让用户以为情况变了。
    #[test]
    fn missing_repos_keeps_the_order_it_was_given() {
        let got = missing_repos("", &names(&["b", "a", "c"]));
        assert_eq!(got, names(&["b", "a", "c"]));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test --lib claudemd 2>&1 | tail -20`
Expected: 三条测试全部 panic 在 `not implemented`。

- [ ] **Step 3: 实现 `missing_repos`**

替换掉 `unimplemented!()` 的那一版：

```rust
/// 这份文档里，哪些仓库名一次都没出现过。
///
/// 只做子串匹配，不解析 Markdown：文档可能用表格行、正文引用、路径
/// （`dc_llm/xxx`）等任何形式提到一个仓库，一律算「提到了」。
/// 宁可漏报不要误报——理由见 `missing_repos_accepts_any_mention...`。
pub fn missing_repos(claude_md: &str, repos: &[String]) -> Vec<String> {
    repos
        .iter()
        .filter(|r| !claude_md.contains(r.as_str()))
        .cloned()
        .collect()
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib claudemd 2>&1 | tail -10`
Expected: 三条 PASS。

- [ ] **Step 5: 给收集层写失败的测试**

在 `mod tests` 里追加：

```rust
    use std::fs;

    /// 造一个假仓库：`mkdir -p <dir>/<name>/.git`
    fn repo(dir: &Path, name: &str) {
        fs::create_dir_all(dir.join(name).join(".git")).unwrap();
    }

    /// 收集层把两半接起来：列出 CLAUDE.md 旁边的 git 仓库，报没被提到的那些。
    #[test]
    fn check_reports_a_repo_beside_the_doc_that_is_never_mentioned() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("CLAUDE.md"), "只写了 alpha").unwrap();
        repo(root, "alpha");
        repo(root, "beta");

        let gaps = check(root, root);
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert_eq!(gaps[0].doc, root.join("CLAUDE.md"));
        assert_eq!(gaps[0].missing, names(&["beta"]));
    }

    /// 不是 git 仓库的目录不算项目。`logs` / `tmp` / `node_modules` 跟真项目
    /// 的区别恰好就是有没有 `.git`——用它当筛子，就不用维护一张排除表。
    #[test]
    fn a_directory_without_git_is_not_a_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("CLAUDE.md"), "什么都没提").unwrap();
        fs::create_dir_all(root.join("logs")).unwrap();
        fs::create_dir_all(root.join("node_modules")).unwrap();

        assert!(check(root, root).is_empty(), "杂项目录不该报");
    }

    /// 当前目录自己那份也要查，不是只查上级。
    #[test]
    fn a_claude_md_beside_the_project_is_checked_too() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let proj = home.join("proj");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join("CLAUDE.md"), "空").unwrap();
        repo(&proj, "sub");

        let gaps = check(&proj, home);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].missing, names(&["sub"]));
    }

    /// 向上走到 home 就停：`/Users` 和 `/` 下面没有项目清单，只有噪音。
    #[test]
    fn the_walk_stops_at_home() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path();
        // home 之外放一份会报缺口的文档
        fs::write(outside.join("CLAUDE.md"), "空").unwrap();
        repo(outside, "should_not_be_seen");

        let home = outside.join("home");
        let proj = home.join("proj");
        fs::create_dir_all(&proj).unwrap();

        let gaps = check(&proj, &home);
        assert!(gaps.is_empty(), "越过 home 往上查了：{gaps:?}");
    }

    /// 找不到 CLAUDE.md 不是错误，什么都不说。
    #[test]
    fn no_claude_md_is_silent() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(check(tmp.path(), tmp.path()).is_empty());
    }

    /// 读不了（权限、目录没了）也绝不能挡住任何事：这是个提示功能。
    #[test]
    fn an_unreadable_path_never_panics() {
        let missing = Path::new("/nonexistent-dir-for-dct-test/deep/deeper");
        let _ = check(missing, Path::new("/nonexistent-dir-for-dct-test"));
    }
```

- [ ] **Step 6: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test --lib claudemd 2>&1 | tail -20`
Expected: 六条新测试 panic 在 `not implemented`（`missing_repos` 那三条仍 PASS）。

- [ ] **Step 7: 实现收集层**

```rust
/// 列出 `dir` 下**是 git 仓库**的直接子目录，按名字排序。
///
/// 判 git 用 `stat` 而不是 fork `git` 进程——同 `ui::view::list_dirs` 的理由：
/// 一个目录几十个子目录就是几十次 fork。判得没那么全（worktree、`GIT_DIR`
/// 会漏），但这里只是拿它当「是不是一个项目」的筛子，漏判的代价只是少报一条。
///
/// 读不了就返回空表，不报错：这是个提示功能，任何情况下都不该挡住用户。
fn git_repos_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| e.path().join(".git").exists())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// 从 `start` 往上走，每一级有 CLAUDE.md 就查一次，到 `home` 为止（含 `home`）。
///
/// 走这条链是因为 Claude Code 就是这么加载 CLAUDE.md 的：工作目录没有就往上找，
/// 找到的会被当成指令注入。要查的正是「将来会被注入的那几份」。
///
/// `home` 是参数不是读环境变量：环境变量是进程全局状态，测试里改它会互相打架
/// （同 `i18n::resolve` 的 `env` 闭包）。
pub fn check(start: &Path, home: &Path) -> Vec<Gap> {
    // 上限兜底：`start` 不在 `home` 底下时（比如用户在别处开的项目），
    // 靠 `dir == home` 是停不下来的，只能一路走到文件系统根。给个层数上限，
    // 免得在一台目录很深的机器上白扫十几层。
    const MAX_LEVELS: usize = 16;

    let mut out = Vec::new();
    let mut cur = Some(start);
    for _ in 0..MAX_LEVELS {
        let Some(dir) = cur else { break };
        let doc = dir.join("CLAUDE.md");
        if let Ok(text) = std::fs::read_to_string(&doc) {
            let missing = missing_repos(&text, &git_repos_in(dir));
            if !missing.is_empty() {
                out.push(Gap { doc, missing });
            }
        }
        if dir == home {
            break;
        }
        cur = dir.parent();
    }
    out
}
```

- [ ] **Step 8: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib claudemd 2>&1 | tail -10`
Expected: 九条全 PASS。

- [ ] **Step 9: 接进 crate**

在 `src/lib.rs` 里按字母序插一行（`cli` 和 `client` 之后、`clipboard` 之前）：

```rust
pub mod claudemd;
```

- [ ] **Step 10: 格式与静态检查**

```bash
~/.cargo/bin/cargo fmt
~/.cargo/bin/cargo clippy --all-targets 2>&1 | grep -E "^(warning|error)" | head
~/.cargo/bin/cargo test 2>&1 | grep -E "FAILED|^error" | head
git diff --check
```
Expected: clippy 无输出、测试无 FAILED。

- [ ] **Step 11: 提交**

```bash
git add src/claudemd.rs src/lib.rs
git commit -F - <<'MSG'
feat: spot a repository the shared CLAUDE.md forgot

CLAUDE.md is injected into the agent as an instruction, and the part of
it that rots first is the list of what exists — nobody remembers to edit
that table when a repository is added. On 2026-08-06 a whole round of
work followed such a table into the wrong repository, and never hit a
wall: the older implementation there still runs.

Walk the same chain Claude Code walks, list the git repositories beside
each CLAUDE.md, and report the ones the document never names. Git-ness
is the filter that keeps logs/ and tmp/ quiet with no exclusion list to
maintain.

Substring matching on purpose: a warning that cries wolf gets ignored
after the second time, and then it cannot stop a real gap either.
MSG
```

---

### Task 2: 显示给用户

**Files:**
- Modify: `src/i18n.rs`（`msg` 模块加一条带参函数）
- Modify: `src/ui/mod.rs`（`switch_project` 之后 + `run()` 启动时）
- Test: `src/i18n.rs` 与 `src/ui/mod.rs` 各自的 `mod tests`

**Interfaces:**
- Consumes: `crate::claudemd::{check, Gap}`（Task 1）
- Produces: `crate::i18n::msg::claude_md_missing(lang, doc_path, names) -> String`；
  `ui::mod` 内部函数 `check_claude_md(app: &mut App)`

- [ ] **Step 1: 写失败的 i18n 测试**

在 `src/i18n.rs` 的 `mod tests` 里追加：

```rust
    /// 文案必须点名**是哪一份**文档：链上可能有好几份 CLAUDE.md，
    /// 只说「CLAUDE.md 少了东西」用户不知道该去改哪个。
    #[test]
    fn the_claude_md_warning_names_the_document_and_the_repos() {
        let m = msg::claude_md_missing(
            Lang::Zh,
            "~/work/dc/CLAUDE.md",
            &["dc_desktop".to_string(), "dc-terminal".to_string()],
        );
        assert!(m.contains("~/work/dc/CLAUDE.md"), "{m}");
        assert!(m.contains("dc_desktop") && m.contains("dc-terminal"), "{m}");
        let en = msg::claude_md_missing(Lang::En, "~/w/CLAUDE.md", &["x".to_string()]);
        assert!(!en.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)), "英文里不许有汉字：{en}");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test --lib the_claude_md_warning 2>&1 | tail -10`
Expected: 编译失败，`msg::claude_md_missing` 不存在。

- [ ] **Step 3: 加文案**

在 `src/i18n.rs` 的 `pub mod msg` 里（`switched_to` 附近）加：

```rust
    /// 「这份清单漏了这几个仓库」。
    ///
    /// 点名是哪一份文档，因为一次可能查好几份；名字用顿号/逗号连起来，
    /// 不用换行——底栏只有一行。
    pub fn claude_md_missing(lang: Lang, doc: &str, names: &[String]) -> String {
        t!(
            lang,
            en: format!("{doc} never mentions: {}", names.join(", ")),
            zh: format!("{doc} 里没提到：{}", names.join("、")),
        )
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib the_claude_md_warning 2>&1 | tail -5`
Expected: PASS。

- [ ] **Step 5: 写调用点的失败测试**

在 `src/ui/mod.rs` 的 `mod tests` 里追加：

```rust
    /// 换到一个项目时，如果那条 CLAUDE.md 链漏了仓库，底栏要说出来——
    /// 而且盖过「已切到 X」那句常规反馈：项目名在边框标题里本来就看得见，
    /// 而这条警告不说就没有第二次机会。
    #[test]
    fn switching_to_a_project_warns_about_a_forgotten_repo() {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("CLAUDE.md"), "只提了 alpha").unwrap();
        fs::create_dir_all(root.join("alpha").join(".git")).unwrap();
        fs::create_dir_all(root.join("beta").join(".git")).unwrap();
        let proj = root.join("alpha");

        let (mut app, _dir) = App::test_app();
        app.home = root.to_path_buf();
        switch_project(&mut app, proj);

        assert!(app.message.text.contains("beta"), "该报缺口：{}", app.message.text);
        assert!(app.message.error, "警告要用红字，否则混在常规反馈里看不见");
    }

    /// 清单是全的就不该出声。每次换项目都弹一句话会变成噪音，
    /// 而噪音会让真的警告也被无视。
    #[test]
    fn switching_to_a_project_with_a_complete_list_says_nothing_extra() {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("CLAUDE.md"), "alpha 都写了").unwrap();
        fs::create_dir_all(root.join("alpha").join(".git")).unwrap();
        let proj = root.join("alpha");

        let (mut app, _dir) = App::test_app();
        app.home = root.to_path_buf();
        switch_project(&mut app, proj);

        assert!(!app.message.error, "没有缺口就不该报警告：{}", app.message.text);
        assert!(app.message.text.contains("已切到"), "常规反馈还要在：{}", app.message.text);
    }
```

- [ ] **Step 6: 跑测试确认失败**

Run: `~/.cargo/bin/cargo test --lib switching_to_a_project 2>&1 | tail -10`
Expected: 编译失败，`App` 没有 `home` 字段。

- [ ] **Step 7: 给 App 加 `home`**

`home` 做成字段而不是每次读 `std::env::var("HOME")`：环境变量是进程全局状态，
测试里改它会互相打架（同 `i18n::resolve` 的 `env` 闭包）。

在 `src/ui/app.rs` 的 `pub struct App` 里加字段（跟 `current_dir` 挨着）：

```rust
    /// 向上找 CLAUDE.md 走到哪儿为止（见 `claudemd::check`）。
    /// 是字段不是现读环境变量，为的是测试能指定一个临时目录。
    pub home: PathBuf,
```

`App` 有**两个**构造函数（`App::new` 和 `App::new_disconnected`，
`grep -n "current_dir:" src/ui/app.rs` 正好两处），**两处都要**加上同一行：

```rust
    home: std::env::var("HOME").map(PathBuf::from).unwrap_or_default(),
```

`unwrap_or_default()` 给出空 `PathBuf`：那时 `check` 的 `dir == home` 永远
不成立，靠 `MAX_LEVELS` 兜底停下来。没有 `HOME` 的环境（某些 cron/容器）
下这个功能退化成「多查几层」，而不是崩掉。

- [ ] **Step 8: 实现检查并接进 `switch_project`**

在 `src/ui/mod.rs` 里，`switch_project` 函数下方加：

```rust
/// 核对会被注入 agent 的那几份 CLAUDE.md，漏了仓库就在底栏说一句。
///
/// **报给人，不注入给 agent。** 把这份事实塞进 agent 的系统提示，只会让它
/// 同时拿到两个互相不知道对方存在的权威来源（一个说某功能在某仓库、还带着
/// MUST FOLLOW，一个说磁盘上有另一个仓库），而它没有任何依据判断哪个更新。
/// 那不是修好了信息，是又加了一个信源。人在回路里才是这次事故缺的那一环。
///
/// 用红字（`Msg::err`）盖掉「已切到 X」那句常规反馈：项目名在底部边框标题里
/// 本来就看得见，而这条警告不说就没有第二次机会。
fn check_claude_md(app: &mut App) {
    let gaps = crate::claudemd::check(&app.current_dir, &app.home);
    let Some(gap) = gaps.first() else { return };
    app.message = Msg::err(crate::i18n::msg::claude_md_missing(
        app.lang,
        &short_path(&gap.doc.display().to_string()),
        &gap.missing,
    ));
}
```

在 `switch_project` 末尾（`app.view = home_view(app);` 之后）加一行：

```rust
    check_claude_md(app);
```

- [ ] **Step 9: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib switching_to_a_project 2>&1 | tail -5`
Expected: 两条 PASS。

- [ ] **Step 10: 启动时也查一次**

只在换项目时查的话，「一直待在同一个项目里」的用户永远看不到。在
`src/ui/mod.rs` 里 `let mut app = App::new(...)` 那一行（约 181 行）**之后**、
主循环开始之前，加一行：

```rust
    check_claude_md(&mut app);
```

放在这里而不是循环里：循环每 16~150ms 转一圈，在里面查等于每秒读十几次
文件系统，而这件事的答案一整个会话都不会变。

- [ ] **Step 11: 全量检查**

```bash
~/.cargo/bin/cargo fmt
~/.cargo/bin/cargo clippy --all-targets 2>&1 | grep -E "^(warning|error)" | head
~/.cargo/bin/cargo test 2>&1 | grep -E "FAILED|^error" | head
git diff --check
```
Expected: clippy 无输出、测试无 FAILED。

- [ ] **Step 12: 真机验一次**

```bash
~/.cargo/bin/cargo build
cd /Users/lei/work/dc/dc-terminal && ./target/debug/dct
```
在真实的 `~/work/dc` 下启动，底栏应当**不再**报缺口（`dc_desktop` 和
`dc-terminal` 两行 2026-08-06 已补进那份 CLAUDE.md）。要看到警告长什么样，
临时把清单里某一行注释掉再起一次，看完改回来。

- [ ] **Step 13: 提交**

```bash
git add src/i18n.rs src/ui/mod.rs src/ui/app.rs
git commit -F - <<'MSG'
feat: say which repositories the project list forgot

Report the gap in the status bar when dct starts and whenever the
project changes — the two moments the current project is decided, and
the moment someone is about to start working in it. Red, over the
ordinary "switched to X" line: the project name is already in the border
title, while this warning gets no second chance.

It goes to the person, not to the agent. Injecting it into the system
prompt would hand the agent two authorities that cannot see each other,
with nothing to decide between them.
MSG
```

---

### Task 3: 让 spec 跟实现一致

**Files:**
- Modify: `docs/superpowers/specs/2026-08-07-dct-claude-md-repo-check-design.md`

- [ ] **Step 1: 改「何时检查」那一节**

把「会话创建成功之后，不阻塞创建」改成实际做法，并写清为什么变：

```markdown
### 何时检查

**dct 启动时**查一次，之后**每次换项目**（`switch_project`）再查一次。这两处
是 `current_dir` 唯一确定和变化的时刻，也正是用户要在一个项目里开工的时刻。

原设计写的是「会话创建成功之后」，实现时改掉了：`Response::Created` 在
`ui/mod.rs` 里有两个分散的调用点，两处各加一次容易漏；而且每开一次会话报一次
会变成噪音，噪音会让真的警告也被无视。

检查失败（没有 CLAUDE.md、读不了、目录没权限）一律静默跳过——这是个提示功能，
任何情况下都不该挡住用户。
```

- [ ] **Step 2: 把状态行改成已实现**

```markdown
**状态：** 已实现，待 review
```

- [ ] **Step 3: 提交**

```bash
git add docs/superpowers/specs/2026-08-07-dct-claude-md-repo-check-design.md
git commit -m "docs: record where the CLAUDE.md check actually runs"
```

---

## 自查

**Spec 覆盖：**

| Spec 要求 | 落在哪 |
|---|---|
| 走 CLAUDE.md 加载链 | Task 1 Step 7 `check` |
| 只认 git 仓库当项目 | Task 1 Step 7 `git_repos_in` |
| 子串匹配、宽进严出 | Task 1 Step 3 + 测试 |
| 报给人不报给 agent | Task 2 Step 8（`check_claude_md` 的文档注释写明理由） |
| 文案带上是哪份文档 | Task 2 Step 3 + Step 1 的测试 |
| 走 i18n、中英文各一份 | Task 2 Step 3 |
| 在界面侧、不动协议 | Task 2 全部改动都在 `ui/` 与 `i18n.rs`，没碰 `proto.rs` |
| 静默跳过读不了的情况 | Task 1 Step 7（`let Ok(..) else`）+ `an_unreadable_path_never_panics` |
| 何时检查 | Task 2 Step 8/10，与 spec 的差异由 Task 3 收口 |

**类型一致性：** `Gap { doc: PathBuf, missing: Vec<String> }` 在 Task 1 定义，
Task 2 Step 8 只读这两个字段；`missing_repos` / `check` / `claude_md_missing`
三个签名在 Task 1、Task 2 各出现一次，参数与返回类型一致。

**已知不做：** 不自动改 CLAUDE.md、不检查描述对不对、不解析表格、不做成阻塞式确认
（理由都在 spec 的「不做」一节）。
