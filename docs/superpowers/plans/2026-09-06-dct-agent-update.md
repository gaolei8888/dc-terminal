# dct agent 有新版本就提示、按 u 一键更新 —— 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 守护进程每天问一次 npm 仓库，选 agent 的列表上显示「有新版本」，按 `u` 跑 `dct install <agent> --update`。

**Architecture:** 新模块 `src/update.rs` 装纯函数（包名解析、`package.json` 定位、版本比较、仓库 URL、JSON 解析）和一个真的发 HTTP 的 `fetch_latest`。守护进程持有一张 `UpdateTable`，后台线程按节奏填它，`Request::Profiles` 读它填进 `ProfileEntry.update` / `managed`。CLI 的 `--update` 和界面的 `u` 都是现有安装路径的分支，不另起一套。

**Tech Stack:** Rust（stable），ureq 2（已在依赖里，TLS 后端按平台分叉——**只能用 `sys::tls::agent_builder()`**，见 `Cargo.toml:50` 那段注释），serde_json（已在依赖里），ratatui。

**Spec:** `docs/superpowers/specs/2026-09-06-dct-agent-update-design.md`

## Global Constraints

- **不加新 crate。** semver 三段比较手写；JSON 用已有的 serde_json。
- **测试里不发真实 HTTP。** 需要「取 latest」的地方一律注入闭包。
- **不自动升级、不做配置开关、不动 `profiles/*.toml`、不改 web 端。**
- 仓库地址取 `DCT_NPM_REGISTRY`，默认 `https://registry.npmjs.org`；带作用域的包名里的 `/` 不转义。
- 请求整体 5 秒超时；查不到 = 没有提示，不是错误；守护进程 stderr 留一行。
- 界面文案中英各一份，走 `i18n::text` / `i18n::msg`，**不出现 npm / registry 这类词**。
- 提交信息中文、写清「为什么」，末尾带
  `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` 和
  `Claude-Session: https://claude.ai/code/session_01LpSh1dhbMaZVkc4rdDRTrM` 两行
  （照最近两条提交的样子）。
- 每个任务结束：`cargo fmt`，`cargo test --lib <模块名>` 通过，`git diff --check` 干净。
- ⚠️ **本计划里的参考代码不是权威。** 这个仓库的记录是：每一份计划的参考代码平均带三处真缺陷，抓住它们的是实施者自己核对源码、跑变异测试。看到跟真实源码对不上的签名，以源码为准，并在报告里指出来。

### 文件总览

| 文件 | 动作 | 责任 |
|---|---|---|
| `src/update.rs` | 新建 | 纯函数 + `fetch_latest` + `UpdateInfo`/`UpdateTable` + `check_updates` |
| `src/lib.rs` | 改 | `pub mod update;` |
| `src/proto.rs` | 改 | `UpdatePrompt`、`ProfileEntry.update`/`managed`、`Request::RecheckUpdates`、协议号 14 |
| `src/daemon.rs` | 改 | 后台线程；`handle()` 多一个 `updates` 参数；`Profiles` 填字段；`RecheckUpdates` 分支 |
| `src/cli.rs` | 改 | `run_install` 加 `--update` 分支；`npm_registry` 搬到 `update.rs` |
| `src/main.rs` | 改 | `dct install <name> [--update]` 参数解析 |
| `src/i18n.rs` | 改 | 新文案 |
| `src/ui/view.rs` | 改 | `update_action`、`UpdateAction`、`PickProfile` 的帮助行 |
| `src/ui/pick.rs` | 改 | `u` 键；列表行画「有新版本」 |
| `README.md` / `README.zh-CN.md` | 改 | 怎么更新、哪些管不了、更新时不能有会话在跑 |

---

### Task 1: `update.rs` 的纯函数

**Files:**
- Create: `src/update.rs`
- Modify: `src/lib.rs`（加一行 `pub mod update;`，放在 `pub mod runtime;` 旁边）
- Modify: `src/cli.rs:600-610`（把 `npm_registry()` 删掉，调用处改成 `crate::update::npm_registry_override()`）

**Interfaces:**
- Produces:
  - `pub fn npm_package_of(command: &[String]) -> Option<String>`
  - `pub fn installed_version(exe: &Path, package: &str) -> Option<String>`
  - `pub fn parse_latest(body: &str) -> Option<String>`
  - `pub fn newer(installed: &str, latest: &str) -> bool`
  - `pub fn npm_registry_override() -> Option<String>`（原 `cli::npm_registry`，原样搬）
  - `pub fn registry() -> String`（override 或 `DEFAULT_REGISTRY`，末尾 `/` 去掉）
  - `pub fn latest_url(registry: &str, package: &str) -> String`
  - `pub const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";`

- [ ] **Step 1: 写失败的测试**

`src/update.rs` 先只放测试模块和函数签名（函数体 `todo!()`），让编译通过、测试失败：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn package_name_comes_from_the_npm_install_command() {
        assert_eq!(
            npm_package_of(&v(&["npm", "i", "-g", "@anthropic-ai/claude-code"])).as_deref(),
            Some("@anthropic-ai/claude-code")
        );
        assert_eq!(
            npm_package_of(&v(&["npm", "install", "-g", "opencode-ai", "--foo"])).as_deref(),
            Some("opencode-ai")
        );
        // 参数顺序：`-g` 在包名后面也认。
        assert_eq!(
            npm_package_of(&v(&["npm", "i", "@qwen-code/qwen-code", "-g"])).as_deref(),
            Some("@qwen-code/qwen-code")
        );
    }

    #[test]
    fn non_npm_installers_have_no_package() {
        assert_eq!(npm_package_of(&v(&["brew", "install", "x"])), None);
        assert_eq!(npm_package_of(&v(&["npx", "x"])), None);
        assert_eq!(npm_package_of(&v(&["npm", "i", "-g"])), None);
        assert_eq!(npm_package_of(&v(&[])), None);
    }

    fn write_pkg(dir: &Path, package: &str, body: &str) {
        let d = dir.join(package);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("package.json"), body).unwrap();
    }

    #[test]
    fn installed_version_is_read_from_the_unix_global_layout() {
        let t = tempfile::tempdir().unwrap();
        let prefix = t.path();
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        write_pkg(
            &prefix.join("lib/node_modules"),
            "@anthropic-ai/claude-code",
            r#"{"name":"@anthropic-ai/claude-code","version":"2.1.251"}"#,
        );
        let exe = prefix.join("bin/claude");
        std::fs::write(&exe, "").unwrap();
        assert_eq!(
            installed_version(&exe, "@anthropic-ai/claude-code").as_deref(),
            Some("2.1.251")
        );
    }

    #[test]
    fn installed_version_is_read_from_the_windows_global_layout() {
        let t = tempfile::tempdir().unwrap();
        let prefix = t.path();
        write_pkg(
            &prefix.join("node_modules"),
            "@qwen-code/qwen-code",
            r#"{"version":"0.22.3"}"#,
        );
        let exe = prefix.join("qwen.cmd");
        std::fs::write(&exe, "").unwrap();
        assert_eq!(
            installed_version(&exe, "@qwen-code/qwen-code").as_deref(),
            Some("0.22.3")
        );
    }

    #[test]
    fn a_launcher_with_no_package_json_nearby_is_not_managed() {
        // 官方安装器装的 claude：~/.local/bin/claude 旁边什么都没有。
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("bin/claude");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "").unwrap();
        assert_eq!(installed_version(&exe, "@anthropic-ai/claude-code"), None);
    }

    #[test]
    fn a_package_json_without_a_version_field_is_not_a_version() {
        let t = tempfile::tempdir().unwrap();
        let prefix = t.path();
        write_pkg(&prefix.join("lib/node_modules"), "x", r#"{"name":"x"}"#);
        let exe = prefix.join("bin/x");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "").unwrap();
        assert_eq!(installed_version(&exe, "x"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_relocated_prefix_is_found_by_following_the_symlink() {
        // bin/claude -> ../lib/node_modules/@a/b/cli.js，但 bin 目录被挪到了
        // 别处（比如 prefix 改过）。相对路径找不到，顺着链接能找到。
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("elsewhere/lib/node_modules");
        write_pkg(&real, "@a/b", r#"{"version":"1.0.0"}"#);
        std::fs::write(real.join("@a/b/cli.js"), "").unwrap();
        let bin = t.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("b");
        std::os::unix::fs::symlink(real.join("@a/b/cli.js"), &exe).unwrap();
        assert_eq!(installed_version(&exe, "@a/b").as_deref(), Some("1.0.0"));
    }

    #[test]
    fn latest_is_the_version_field_of_the_registry_json() {
        assert_eq!(
            parse_latest(r#"{"name":"x","version":"2.1.260","dist":{}}"#).as_deref(),
            Some("2.1.260")
        );
        assert_eq!(parse_latest(r#"{"name":"x"}"#), None);
        assert_eq!(parse_latest("not json"), None);
        assert_eq!(parse_latest(r#"{"version":42}"#), None);
    }

    #[test]
    fn newer_compares_three_numeric_parts() {
        assert!(newer("1.2.3", "1.2.10"));
        assert!(newer("1.9.0", "1.10.0"));
        assert!(newer("0.22.3", "1.0.0"));
        assert!(!newer("1.2.3", "1.2.3"));
        assert!(!newer("1.2.10", "1.2.3"));
    }

    #[test]
    fn prereleases_and_garbage_never_prompt() {
        assert!(!newer("1.2.3", "1.3.0-beta.1"));
        assert!(!newer("1.2.3-rc.1", "1.2.3"));
        assert!(!newer("1.2", "1.3.0"));
        assert!(!newer("1.2.3", "latest"));
        assert!(!newer("", "1.0.0"));
    }

    #[test]
    fn latest_url_keeps_the_scope_slash_and_trims_the_registry() {
        assert_eq!(
            latest_url("https://registry.npmjs.org/", "@anthropic-ai/claude-code"),
            "https://registry.npmjs.org/@anthropic-ai/claude-code/latest"
        );
        assert_eq!(
            latest_url("https://registry.npmmirror.com", "opencode-ai"),
            "https://registry.npmmirror.com/opencode-ai/latest"
        );
    }
}
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test --lib update`
Expected: 编译通过（函数体 `todo!()`），每条测试 panic。

- [ ] **Step 3: 实现**

```rust
//! agent 有没有新版本。
//!
//! **为什么有这个模块：** 看板上能干活的那几个 agent 都是 npm 装的，发版
//! 很勤，而 `dct install` 看到命令找得到就说「装好了」，装完再也不看版本。
//! 学生停在装那天的版本上，直到某天撞上一个新版才修好的问题。
//!
//! 这里只回答三个问题：这个 profile 装的是 npm 上哪个包、本机装的是几、
//! 仓库上最新是几。**不自动升**——`runtime.rs` 钉死 Node 版本的理由在这儿
//! 同样成立：一个教室里所有人手上应该是同一份。
//!
//! ## 已经验过的事实
//!
//! （实施 Task 2 时用 curl 验，把结果写在这儿：两个仓库对
//! `/@anthropic-ai/claude-code/latest` 是否直接回 200 + JSON。）

use std::path::{Path, PathBuf};

pub const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";

/// `DCT_NPM_REGISTRY`，跟 `runtime.rs` 的 `DCT_NODE_BASE` 分成两个变量，
/// 理由写在原来 `cli.rs` 那份注释里（镜像站可能只镜像了其中一样）。
pub fn npm_registry_override() -> Option<String> {
    std::env::var("DCT_NPM_REGISTRY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn registry() -> String {
    npm_registry_override().unwrap_or_else(|| DEFAULT_REGISTRY.to_string())
}

pub fn latest_url(registry: &str, package: &str) -> String {
    format!("{}/{package}/latest", registry.trim_end_matches('/'))
}

/// `[install].command` 里那个 npm 包名：`npm i -g <pkg>` / `npm install -g <pkg>`
/// 里 `i`/`install` 之后第一个不以 `-` 开头的参数。不是 npm 的安装命令
/// （brew、pip、npx）→ `None`，那种 agent 不归 dct 管。
pub fn npm_package_of(command: &[String]) -> Option<String> {
    let mut it = command.iter();
    if it.next().map(String::as_str) != Some("npm") {
        return None;
    }
    match it.next().map(String::as_str) {
        Some("i") | Some("install") | Some("add") => {}
        _ => return None,
    }
    it.find(|a| !a.starts_with('-')).cloned()
}

/// 本机装的是几。`exe` 是 `sys::fs::find_command` 找到的那个文件。
///
/// 先按 npm 全局安装的两种布局找 `package.json`（Unix `../lib/node_modules`，
/// Windows 同级 `node_modules`），都找不到再顺着符号链接走到真实文件、往上
/// 找名为 `<package>` 的目录。三条都落空 → `None`，意思是「这份不是 npm 装的」。
pub fn installed_version(exe: &Path, package: &str) -> Option<String> {
    let dir = exe.parent()?;
    let candidates = [
        dir.join("../lib/node_modules").join(package),
        dir.join("node_modules").join(package),
    ];
    for c in candidates {
        if let Some(v) = read_version(&c.join("package.json")) {
            return Some(v);
        }
    }
    let real = std::fs::canonicalize(exe).ok()?;
    let mut cur = real.parent();
    while let Some(d) = cur {
        if d.ends_with(package) {
            return read_version(&d.join("package.json"));
        }
        cur = d.parent();
    }
    None
}

fn read_version(path: &Path) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    parse_latest(&body)
}

/// 仓库 `/<pkg>/latest` 和 `package.json` 长得一样：都是一个带 `version`
/// 字段的对象。一个函数两处用。
pub fn parse_latest(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("version")?.as_str().map(|s| s.to_string())
}

/// 三段数字严格比较。任一边不是 `a.b.c` 纯数字（带 `-beta`、只有两段、
/// 空的）都返回 false——不认识的东西不提示，比提示错了好。
pub fn newer(installed: &str, latest: &str) -> bool {
    match (parts(installed), parts(latest)) {
        (Some(a), Some(b)) => b > a,
        _ => false,
    }
}

fn parts(v: &str) -> Option<[u64; 3]> {
    let mut it = v.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some([a, b, c])
}
```

`cli.rs` 里：删掉 `fn npm_registry()` 及其注释，两处调用改成 `crate::update::npm_registry_override()`。

- [ ] **Step 4: 跑测试**

Run: `cargo test --lib update && cargo test --lib cli`
Expected: 全部 PASS。

- [ ] **Step 5: 变异检查（必做，不是可选）**

各改坏一处再跑一次，确认对应测试真的红：`newer` 里把 `b > a` 改成 `b >= a`；`npm_package_of` 里去掉 `starts_with('-')` 过滤；`installed_version` 里删掉 Windows 那条候选。改回来。

- [ ] **Step 6: Commit**

```bash
cargo fmt && git diff --check
git add src/update.rs src/lib.rs src/cli.rs
git commit -F - <<'EOF'
feat: 认出一个 agent 是 npm 装的、装的是几、仓库上最新是几

dct install 看到命令找得到就说装好了，装完再也不看版本。这一步先把三个
问题的纯函数写出来：包名从 [install].command 里读，装的版本从可执行
文件旁边的 package.json 读（Unix 和 Windows 两种全局布局，找不到再顺
符号链接），最新版本从仓库的 /<pkg>/latest 读。

追溯不到 package.json 的（官方安装器装的 claude）一律 None——那份不是
dct 装的，dct 不碰。版本比较手写三段数字，带预发布后缀的不提示，不为
这二十行加 semver crate。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LpSh1dhbMaZVkc4rdDRTrM
EOF
```

---

### Task 2: 守护进程：`UpdateTable`、后台检查、协议字段

**Files:**
- Modify: `src/update.rs`（加 `UpdateInfo`、`UpdateTable`、`check_updates`、`fetch_latest`）
- Modify: `src/proto.rs:93`（协议号 13 → 14，加一段「14 = …」注释）、`ProfileEntry`（`update`、`managed`）、`Request`（`RecheckUpdates`）、`Request` 的 `Debug` impl 和它那条 `Debug` 测试
- Modify: `src/daemon.rs`：`run_with_manager`（起线程、传表）、`serve`/`handle`（多一个参数）、`Profiles` 分支、`RecheckUpdates` 分支；`tests` 里所有 `handle(` 调用补一个 `&test_updates()`
- Modify: 仓库里所有手写 `ProfileEntry { … }` 的地方（`grep -rn "ProfileEntry {" src | grep -v "pub struct"`，约 18 处）补 `update: None, managed: false`

**Interfaces:**
- Consumes: Task 1 的全部函数；`sys::fs::find_command`；`profile::all_profiles`。
- Produces:
  - `pub struct UpdateInfo { pub package: String, pub installed: String, pub latest: Option<String>, pub checked_at: Option<std::time::SystemTime> }`
  - `pub type UpdateTable = std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, UpdateInfo>>>;`
  - `pub fn new_table() -> UpdateTable`
  - `pub fn check_updates(profiles_dir: &Path, table: &UpdateTable, only: Option<&str>, fetch: &dyn Fn(&str) -> Option<String>)`
  - `pub fn fetch_latest(package: &str) -> Option<String>`（真发 HTTP）
  - `pub fn prompt_for(info: &UpdateInfo) -> Option<crate::proto::UpdatePrompt>`
  - `proto::UpdatePrompt { pub installed: String, pub latest: String }`
  - `proto::ProfileEntry { …, pub update: Option<UpdatePrompt>, pub managed: bool }`
  - `proto::Request::RecheckUpdates { profile: Option<String> }` → `Response::Ok`

- [ ] **Step 1: 写失败的测试**

`src/update.rs` 的 tests 里追加：

```rust
    fn managed_profile_dir(t: &tempfile::TempDir) -> (PathBuf, PathBuf) {
        // 一份假的「npm 全局前缀」：bin/fakeagent + lib/node_modules/fake-agent/package.json
        let prefix = t.path().join("prefix");
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        write_pkg(
            &prefix.join("lib/node_modules"),
            "fake-agent",
            r#"{"version":"1.0.0"}"#,
        );
        let exe = prefix.join("bin/fakeagent");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // 一份 profile，command 用绝对路径，这样 find_command 不用改 PATH
        let profiles = t.path().join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(
            profiles.join("fake.toml"),
            format!(
                "name = \"fake\"\ncommand = [{:?}]\nis_agent = true\n[install]\ncommand = [\"npm\", \"i\", \"-g\", \"fake-agent\"]\n",
                exe.display()
            ),
        )
        .unwrap();
        (profiles, exe)
    }

    #[test]
    fn check_updates_records_installed_and_latest_for_a_managed_agent() {
        let t = tempfile::tempdir().unwrap();
        let (profiles, _) = managed_profile_dir(&t);
        let table = new_table();
        check_updates(&profiles, &table, None, &|pkg| {
            assert_eq!(pkg, "fake-agent");
            Some("1.2.0".into())
        });
        let g = table.lock().unwrap();
        let info = g.get("fake").expect("fake 应该被记进表里");
        assert_eq!(info.installed, "1.0.0");
        assert_eq!(info.latest.as_deref(), Some("1.2.0"));
        assert!(info.checked_at.is_some());
        let p = prompt_for(info).expect("1.0.0 → 1.2.0 该提示");
        assert_eq!((p.installed.as_str(), p.latest.as_str()), ("1.0.0", "1.2.0"));
    }

    #[test]
    fn a_failed_fetch_leaves_no_prompt_but_keeps_the_installed_version() {
        let t = tempfile::tempdir().unwrap();
        let (profiles, _) = managed_profile_dir(&t);
        let table = new_table();
        check_updates(&profiles, &table, None, &|_| None);
        let g = table.lock().unwrap();
        let info = g.get("fake").unwrap();
        assert_eq!(info.installed, "1.0.0");
        assert_eq!(info.latest, None);
        assert!(prompt_for(info).is_none());
    }

    #[test]
    fn an_up_to_date_agent_has_no_prompt() {
        let info = UpdateInfo {
            package: "x".into(),
            installed: "1.0.0".into(),
            latest: Some("1.0.0".into()),
            checked_at: None,
        };
        assert!(prompt_for(&info).is_none());
    }

    #[test]
    fn unmanaged_and_uninstalled_profiles_are_not_in_the_table() {
        // 内置 claude 在这台测试机上要么没装、要么不是 npm 装的（不能假定），
        // 但 shell 没有 [install]，一定不在表里。
        let t = tempfile::tempdir().unwrap();
        let profiles = t.path().join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        let table = new_table();
        check_updates(&profiles, &table, None, &|_| panic!("没有归 dct 管的 agent，不该发请求"));
        assert!(!table.lock().unwrap().contains_key("shell"));
    }

    #[test]
    fn only_rechecks_the_named_profile() {
        let t = tempfile::tempdir().unwrap();
        let (profiles, _) = managed_profile_dir(&t);
        let table = new_table();
        let calls = std::cell::Cell::new(0);
        check_updates(&profiles, &table, Some("shell"), &|_| {
            calls.set(calls.get() + 1);
            None
        });
        assert_eq!(calls.get(), 0, "只查 shell，fake 一次都不该问");
        assert!(table.lock().unwrap().is_empty());
    }
```

`src/daemon.rs` 的 tests 里追加（照 `profiles_are_labelled_in_the_language_the_client_asked_for` 的写法）：

```rust
    fn test_updates() -> crate::update::UpdateTable {
        crate::update::new_table()
    }

    /// Profiles 响应里的 update 字段来自共享表：表里有 → 有；表里没有 → None。
    #[test]
    fn profiles_carry_the_update_prompt_from_the_shared_table() {
        let (mgr, store, secrets, profiles_dir) = bare_handle_deps();
        let updates = test_updates();
        updates.lock().unwrap().insert(
            "shell".into(),
            crate::update::UpdateInfo {
                package: "fake".into(),
                installed: "1.0.0".into(),
                latest: Some("2.0.0".into()),
                checked_at: None,
            },
        );
        let resp = handle(
            Request::Profiles { lang: crate::i18n::Lang::Zh },
            &mgr, &store, &secrets, profiles_dir.path(),
            &test_phone(), &test_bridge(), &test_event_tx(), None, &test_pairs(),
            &updates,
        );
        let Response::Profiles { entries, .. } = resp else { panic!("{resp:?}") };
        let shell = entries.iter().find(|e| e.name == "shell").unwrap();
        assert!(shell.managed, "表里有它就是归 dct 管");
        let p = shell.update.as_ref().expect("1.0.0 → 2.0.0 该带提示");
        assert_eq!(p.latest, "2.0.0");
        let claude = entries.iter().find(|e| e.name == "claude").unwrap();
        assert!(claude.update.is_none() && !claude.managed);
    }
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test --lib update && cargo test --lib daemon::tests::profiles_carry`
Expected: 编译错误（类型和字段不存在）。

- [ ] **Step 3: 实现协议**

`src/proto.rs`：

```rust
/// 14 = agent 有新版本。`ProfileEntry` 多了 `update` / `managed`，
/// 多了 `Request::RecheckUpdates`。旧守护进程不认识这条请求；旧界面
/// 看不见新字段（`serde(default)`），不影响它。
pub const PROTOCOL_VERSION: u32 = 14;

/// 这个 agent 装的是几、仓库上最新是几。只在后者更新时出现。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatePrompt {
    pub installed: String,
    pub latest: String,
}
```

`ProfileEntry` 末尾加：

```rust
    /// 有新版本。`None` = 不归 dct 管 / 还没查到 / 已经是最新——三种情况界面
    /// 上都一样：什么都不显示。
    #[serde(default)]
    pub update: Option<UpdatePrompt>,
    /// 守护进程追溯到了 npm 包，也就是 `dct install --update` 能更新它。
    /// 界面靠它在用户按 `u` 时区分「已经是最新」和「不是 dct 装的」。
    #[serde(default)]
    pub managed: bool,
```

`Request` 加（放在 `PairCancel` 后面）：

```rust
    /// 立刻重查一遍有没有新版本。`dct install` 装完发一条，不然提示要挂到
    /// 下一次每日检查才消失。`None` = 全部。回 `Response::Ok`，查是异步的。
    RecheckUpdates {
        profile: Option<String>,
    },
```

`Request` 的 `Debug` impl 加一条 `RecheckUpdates { profile } => f.debug_struct("RecheckUpdates").field("profile", profile).finish()`，那条枚举遍历的 Debug 测试列表里加 `Request::RecheckUpdates { profile: None }`。

- [ ] **Step 4: 实现 `update.rs` 的表和检查**

```rust
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub package: String,
    pub installed: String,
    pub latest: Option<String>,
    pub checked_at: Option<SystemTime>,
}

pub type UpdateTable = Arc<Mutex<BTreeMap<String, UpdateInfo>>>;

pub fn new_table() -> UpdateTable {
    Arc::new(Mutex::new(BTreeMap::new()))
}

/// 守护进程起来多久之后第一次查，以及之后隔多久查一次。
pub const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// 表里有它 = 归 dct 管。提示只在 latest 真的更新时给。
pub fn prompt_for(info: &UpdateInfo) -> Option<crate::proto::UpdatePrompt> {
    let latest = info.latest.as_deref()?;
    newer(&info.installed, latest).then(|| crate::proto::UpdatePrompt {
        installed: info.installed.clone(),
        latest: latest.to_string(),
    })
}

/// 把 `profiles_dir` 里归 dct 管的 agent 逐个查一遍，写进表。`only` 限定
/// 一个 profile 名。`fetch` 是「取 latest」，注入进来是为了测试不发 HTTP。
///
/// **不归 dct 管的从表里删掉**：用户把 npm 那份卸了换成官方安装器，
/// 表里不能还留着旧提示。
pub fn check_updates(
    profiles_dir: &Path,
    table: &UpdateTable,
    only: Option<&str>,
    fetch: &dyn Fn(&str) -> Option<String>,
) {
    let (all, _) = crate::profile::all_profiles(profiles_dir);
    for p in all.iter().filter(|p| only.map_or(true, |o| o == p.name)) {
        let managed = p
            .install
            .as_ref()
            .and_then(|i| npm_package_of(&i.command))
            .and_then(|pkg| {
                let exe = crate::sys::fs::find_command(p.command.first()?)?;
                let installed = installed_version(&exe, &pkg)?;
                Some((pkg, installed))
            });
        let Some((package, installed)) = managed else {
            table.lock().unwrap_or_else(|e| e.into_inner()).remove(&p.name);
            continue;
        };
        let latest = fetch(&package);
        if latest.is_none() {
            eprintln!("update check: {} ({package}) 没查到最新版本", p.name);
        }
        table.lock().unwrap_or_else(|e| e.into_inner()).insert(
            p.name.clone(),
            UpdateInfo {
                package,
                installed,
                latest,
                checked_at: Some(SystemTime::now()),
            },
        );
    }
}

/// 真的去问仓库。查不到就是 `None`，理由见模块头。
pub fn fetch_latest(package: &str) -> Option<String> {
    let url = latest_url(&registry(), package);
    let agent = crate::sys::tls::agent_builder()
        .timeout(FETCH_TIMEOUT)
        .timeout_connect(FETCH_TIMEOUT)
        .build();
    let body = agent.get(&url).call().ok()?.into_string().ok()?;
    parse_latest(&body)
}
```

`find_command` 在 `sys::fs` 里的可见性：如果它不是 `pub` 到 crate 级，改成 `pub`（它已经是 `pub fn`）。

- [ ] **Step 5: 实现守护进程**

`run_with_manager`：在 `let pairs = …` 后面加：

```rust
    // agent 有没有新版本。起来 30 秒后查第一次——不在启动路径上，第一个连
    // 上来的界面不用等它；之后一天一次。查的过程发 HTTP，放在自己的线程里。
    let updates = crate::update::new_table();
    {
        let t = updates.clone();
        let pd = profiles_dir.clone();
        std::thread::spawn(move || {
            std::thread::sleep(crate::update::FIRST_CHECK_AFTER);
            loop {
                crate::update::check_updates(&pd, &t, None, &crate::update::fetch_latest);
                std::thread::sleep(crate::update::CHECK_EVERY);
            }
        });
    }
```

`for conn in listener.incoming()` 里 clone 一份 `let up = updates.clone();` 传给 `serve`，`serve` 签名和 `handle` 签名各加最后一个参数 `updates: &crate::update::UpdateTable`（`serve` 是 owned，`handle` 是借用，照 `pairs` 的样子）。

`Profiles` 分支构造 `ProfileEntry` 时：

```rust
                    let info = recover(updates.lock()).get(&p.name).cloned();
                    ProfileEntry {
                        …,
                        update: info.as_ref().and_then(crate::update::prompt_for),
                        managed: info.is_some(),
                    }
```

（`recover` 是 daemon.rs 里已有的「锁中毒也继续」的帮助函数；锁只握到 clone 完，不带进 map 闭包外层——先在闭包外 `let table = recover(updates.lock()).clone();` 再在闭包里 `table.get(&p.name)` 更干净。）

新分支：

```rust
        Request::RecheckUpdates { profile } => {
            let t = updates.clone();
            let pd = profiles_dir.to_path_buf();
            std::thread::spawn(move || {
                crate::update::check_updates(&pd, &t, profile.as_deref(), &crate::update::fetch_latest);
            });
            Ok(Response::Ok)
        }
```

`handle` 上头的 `Web*` 那段路由（HTTP 进来的 `handle` 调用，`web: None` 那一路）也要传 `updates`——找所有 `handle(` 调用点补参数，包括 `src/web/` 下面如果有的话（`grep -rn "daemon::handle\|handle(" src/web`）。

- [ ] **Step 6: 修所有构造点**

`grep -rn "ProfileEntry {" src | grep -v "pub struct"` 列出的每一处补 `update: None, managed: false,`；daemon tests 里每个 `handle(` 调用末尾补 `&test_updates()`。

- [ ] **Step 7: 验两个仓库的 URL，写进模块头**

```bash
curl -sS -o /dev/null -w '%{http_code}\n' https://registry.npmjs.org/@anthropic-ai/claude-code/latest
curl -sS https://registry.npmjs.org/@anthropic-ai/claude-code/latest | head -c 200; echo
curl -sS -o /dev/null -w '%{http_code}\n' https://registry.npmmirror.com/@anthropic-ai/claude-code/latest
```

把三个结果（状态码、`version` 字段在不在）写进 `update.rs` 头注释「已经验过的事实」那一段。**如果 npmmirror 对不转义的 `/` 回 404，改 `latest_url` 转成 `%2F`，并改那条测试。**

- [ ] **Step 8: 跑测试**

Run: `cargo test --lib update && cargo test --lib daemon && cargo test --lib proto && cargo test`
Expected: 全部 PASS。

- [ ] **Step 9: 变异检查**

`prompt_for` 里把 `newer` 换成 `!=`，`an_up_to_date_agent_has_no_prompt` 仍绿但 `newer_compares…` 不覆盖这条路——补一条 `prompt_for` 在 `installed > latest`（本机比仓库新，比如装了预览版）时也 `None` 的断言。`check_updates` 里删掉 `remove(&p.name)` 那句，确认有测试红；没有的话补一条：先插一条 `shell` 假记录，跑 `check_updates`，断言它被删掉了。

- [ ] **Step 10: Commit**

```bash
cargo fmt && git diff --check
git add src/update.rs src/proto.rs src/daemon.rs $(git diff --name-only)
git commit -F - <<'EOF'
feat: 守护进程每天问一次仓库，Profiles 响应带上「有新版本」

查的地方只能是守护进程：「装没装」在它的 PATH 里问（profile.rs 里定过
的规矩），「装的是几」是同一个问题的延伸。起来 30 秒后查第一次，不在
启动路径上；之后一天一次；dct install 装完发一条 RecheckUpdates 立刻
重查，不然提示要挂到明天。

查不到就是没有提示，不是错误：没网的教室里打开 dct 不该看见一行关于
npm 仓库的红字。协议号 13 → 14。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LpSh1dhbMaZVkc4rdDRTrM
EOF
```

---

### Task 3: `dct install <agent> --update`

**Files:**
- Modify: `src/cli.rs`（`run_install` 签名加 `update: bool`；新增 `parse_install_args`、`live_sessions_of`、`run_update`）
- Modify: `src/main.rs:68-74`（解析 `--update`）
- Modify: `src/i18n.rs`（`msg` 模块加 6 条）

**Interfaces:**
- Consumes: `update::{npm_package_of, installed_version, npm_registry_override}`、`proto::Request::{List, RecheckUpdates}`、`client::Client::connect`、`session::SessionInfo{profile,state}`、`SessionState::Stopped`。
- Produces:
  - `pub fn run_install(name: &str, update: bool, lang: Lang) -> i32`
  - `pub enum InstallArgs { Install(String), Update(String), Usage(String) }`
  - `pub fn parse_install_args(args: &[String], lang: Lang) -> InstallArgs`
  - `pub(crate) fn live_sessions_of(sessions: &[SessionInfo], profile: &str) -> usize`
  - `i18n::msg::{update_refused_sessions_running(lang, n, label), not_managed_by_dct(lang, label), updating_agent(lang, label), update_succeeded(lang, label, version), already_latest(lang, label), install_usage(lang)}`

- [ ] **Step 1: 写失败的测试**

`src/cli.rs` tests：

```rust
    #[test]
    fn install_args_accept_only_the_update_flag() {
        let lang = Lang::Zh;
        assert!(matches!(parse_install_args(&args(&["claude"]), lang), InstallArgs::Install(n) if n == "claude"));
        assert!(matches!(parse_install_args(&args(&["claude", "--update"]), lang), InstallArgs::Update(n) if n == "claude"));
        assert!(matches!(parse_install_args(&args(&["--update", "claude"]), lang), InstallArgs::Update(n) if n == "claude"));
        assert!(matches!(parse_install_args(&args(&[]), lang), InstallArgs::Usage(_)));
        assert!(matches!(parse_install_args(&args(&["claude", "--foo"]), lang), InstallArgs::Usage(_)));
        assert!(matches!(parse_install_args(&args(&["claude", "codex"]), lang), InstallArgs::Usage(_)));
    }

    #[test]
    fn live_sessions_count_only_this_profile_and_only_the_living() {
        use crate::session::{SessionInfo, SessionState};
        let s = |profile: &str, state| SessionInfo {
            profile: profile.into(),
            state,
            ..test_session_info()
        };
        let list = vec![
            s("claude", SessionState::Working),
            s("claude", SessionState::Idle),
            s("claude", SessionState::Stopped),
            s("codex", SessionState::Working),
        ];
        assert_eq!(live_sessions_of(&list, "claude"), 2);
        assert_eq!(live_sessions_of(&list, "codex"), 1);
        assert_eq!(live_sessions_of(&list, "qwen"), 0);
    }
```

`test_session_info()` 造一份最小 `SessionInfo`——实施者看 `session.rs:432` 的字段填齐（`id`、`dir`、`activity`、`is_agent` 等），别的测试里可能已经有类似的 helper（`grep -n "SessionInfo {" src/ui/mod.rs`），有就复用。

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test --lib cli`
Expected: 编译错误。

- [ ] **Step 3: 实现文案**

`src/i18n.rs` 的 `pub mod msg` 里加：

```rust
    /// `--update` 撞上正在跑的会话。**拒绝，不是警告**：正在跑的 node 进程
    /// 底下换文件，Qwen Code 这种几百个文件的包会在下一次 require 时读到
    /// 新旧混杂的东西。
    pub fn update_refused_sessions_running(lang: Lang, n: usize, label: &str) -> String {
        t!(
            lang,
            en: format!("Close the {n} running {label} session(s) first, then update."),
            zh: format!("先关掉正在跑的 {n} 个 {label} 会话再更新。"),
        )
    }

    /// 命令找得到，但追溯不到 npm 包：官方安装器、brew、自己编的。不猜他
    /// 用的是哪种，只说「用原来的方式」。
    pub fn not_managed_by_dct(lang: Lang, label: &str) -> String {
        t!(
            lang,
            en: format!("This {label} was not installed by dct. Update it the way you installed it."),
            zh: format!("这份 {label} 不是 dct 装的，请用你原来装它的方式更新。"),
        )
    }

    pub fn updating_agent(lang: Lang, label: &str) -> String {
        t!(
            lang,
            en: format!("Updating {label}."),
            zh: format!("正在更新 {label}。"),
        )
    }

    pub fn update_succeeded(lang: Lang, label: &str, version: &str) -> String {
        t!(
            lang,
            en: format!("{label} is now {version}. Press Esc to go back to the board, then N to start it."),
            zh: format!("{label} 已更新到 {version}。按 Esc 回看板，再按 N 就能开它。"),
        )
    }

    pub fn already_latest(lang: Lang, label: &str) -> String {
        t!(
            lang,
            en: format!("{label} is already the latest version."),
            zh: format!("{label} 已经是最新的了。"),
        )
    }

    pub fn install_usage(lang: Lang) -> String {
        t!(
            lang,
            en: "Usage: dct install <agent> [--update]   e.g. dct install claude",
            zh: "用法：dct install <agent> [--update]   比如：dct install claude",
        )
        .to_string()
    }
```

- [ ] **Step 4: 实现 CLI**

`src/cli.rs`：

```rust
#[derive(Debug)]
pub enum InstallArgs {
    Install(String),
    Update(String),
    Usage(String),
}

/// `dct install <agent> [--update]`。跟 `parse_restart_args` 一个脾气：
/// 不认识的参数是用法错误，不是忽略。
pub fn parse_install_args(args: &[String], lang: Lang) -> InstallArgs {
    let mut name: Option<String> = None;
    let mut update = false;
    for a in args {
        match a.as_str() {
            "--update" if !update => update = true,
            s if !s.starts_with('-') && name.is_none() => name = Some(s.to_string()),
            _ => return InstallArgs::Usage(crate::i18n::msg::install_usage(lang)),
        }
    }
    match (name, update) {
        (Some(n), false) => InstallArgs::Install(n),
        (Some(n), true) => InstallArgs::Update(n),
        (None, _) => InstallArgs::Usage(crate::i18n::msg::install_usage(lang)),
    }
}

/// 这个 profile 有几个还活着的会话。`Stopped` 不算——进程已经没了，
/// 底下换文件伤不到它。
pub(crate) fn live_sessions_of(sessions: &[crate::session::SessionInfo], profile: &str) -> usize {
    sessions
        .iter()
        .filter(|s| s.profile == profile && s.state != crate::session::SessionState::Stopped)
        .count()
}

/// 问守护进程要会话列表。守护进程没起 → 空列表：没有守护进程就没有会话。
fn sessions_now(socket: &Path) -> Vec<crate::session::SessionInfo> {
    let Ok(mut c) = crate::client::Client::connect(socket) else { return Vec::new() };
    match c.call(crate::proto::Request::List) {
        Ok(crate::proto::Response::Sessions(v)) => v,
        _ => Vec::new(),
    }
}

/// 装完（或更新完）告诉守护进程重查这一个。发不出去不算错——守护进程没起
/// 的话，它起来 30 秒后自己会查。
fn recheck(socket: &Path, name: &str) {
    if let Ok(mut c) = crate::client::Client::connect(socket) {
        let _ = c.call(crate::proto::Request::RecheckUpdates { profile: Some(name.to_string()) });
    }
}
```

`run_install(name, update, lang)` 里，在 `if crate::profile::command_exists(&cmd0)` 那个「已经装好了」分支之前插：

```rust
    if update {
        return run_update(&socket, &runtime, &p, &label, &cmd0, lang);
    }
```

并在原有 `install_succeeded` 打印之前加一句 `recheck(&socket, name);`。

`run_update`：

```rust
fn run_update(
    socket: &Path,
    runtime: &Path,
    p: &crate::profile::Profile,
    label: &str,
    cmd0: &str,
    lang: Lang,
) -> i32 {
    let n = live_sessions_of(&sessions_now(socket), &p.name);
    if n > 0 {
        eprintln!("{}", crate::i18n::msg::update_refused_sessions_running(lang, n, label));
        return 1;
    }
    let Some(package) = p.install.as_ref().and_then(|i| crate::update::npm_package_of(&i.command)) else {
        eprintln!("{}", crate::i18n::msg::not_managed_by_dct(lang, label));
        return 1;
    };
    let Some(exe) = crate::sys::fs::find_command(cmd0) else {
        // 没装就走安装路——`--update` 对一个没装的 agent 就是「装上」。
        return run_install(&p.name, false, lang);
    };
    let Some(before) = crate::update::installed_version(&exe, &package) else {
        eprintln!("{}", crate::i18n::msg::not_managed_by_dct(lang, label));
        return 1;
    };

    if !crate::profile::command_exists("npm") {
        if let Err(e) = crate::runtime::ensure_node(runtime, lang, &TermProgress::default()) {
            eprintln!("{}", fetch_problem(lang, &e));
            return 1;
        }
    }
    let mut argv = vec!["npm".to_string(), "i".into(), "-g".into(), format!("{package}@latest")];
    if let Some(reg) = crate::update::npm_registry_override() {
        argv.push("--registry".into());
        argv.push(reg);
    }
    println!("{}", crate::i18n::msg::updating_agent(lang, label));
    let argv = crate::sys::shell::launch_argv(&argv);
    let ok = std::process::Command::new(&argv[0]).args(&argv[1..]).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        eprintln!("{}", crate::i18n::msg::install_failed(lang, label));
        return 1;
    }
    // 跑完再读一遍。npm 说成功但版本没动 = 仓库上就是这一版。
    let after = crate::sys::fs::find_command(cmd0)
        .and_then(|e| crate::update::installed_version(&e, &package));
    recheck(socket, &p.name);
    match after {
        Some(v) if v != before => {
            println!("{}", crate::i18n::msg::update_succeeded(lang, label, &v));
            0
        }
        Some(_) => {
            println!("{}", crate::i18n::msg::already_latest(lang, label));
            0
        }
        None => {
            eprintln!("{}", crate::i18n::msg::install_finished_but_missing(lang, cmd0));
            1
        }
    }
}
```

**核对：** `run_install` 开头 `if name == "git"` 那条路和 `--update` 不相干，`dct install git --update` 走 `parse_install_args` 得到 `Update("git")`，`run_install("git", true, …)` 里 git 分支在前，照旧装 git——可以接受，不必特判。

`src/main.rs`：

```rust
        Some("install") => match dct::cli::parse_install_args(&args[1..], cli_lang()) {
            dct::cli::InstallArgs::Install(n) => std::process::exit(dct::cli::run_install(&n, false, cli_lang())),
            dct::cli::InstallArgs::Update(n) => std::process::exit(dct::cli::run_install(&n, true, cli_lang())),
            dct::cli::InstallArgs::Usage(u) => {
                eprintln!("{u}");
                std::process::exit(2)
            }
        },
```

`HELP` 常量里 `dct install <agent>` 那一行后面加一行 `dct install <agent> --update   把它升到最新版`。`args` 在 main 里是什么类型（`Vec<String>` 还是 `&[String]`）以源码为准。

- [ ] **Step 5: 跑测试**

Run: `cargo test --lib cli && cargo build`
Expected: PASS；`cargo run -- install claude --foo` 印用法、退出码 2；`cargo run -- install` 印用法。

- [ ] **Step 6: 手工跑一次真实更新（本机）**

```bash
cargo run -- install qwen --update
```

记下输出。三种结果都算通过：「已更新到 x」「已经是最新的了」「不是 dct 装的」。**不能接受的是**：英文栈追踪、npm 原话单独出现而没有 dct 的那句话。把结果写进任务报告。

- [ ] **Step 7: Commit**

```bash
cargo fmt && git diff --check
git add src/cli.rs src/main.rs src/i18n.rs
git commit -F - <<'EOF'
feat: dct install <agent> --update 把一个 agent 升到最新版

跟安装走同一条路：缺 npm 先补自带的 Node，装完真的再读一遍版本，
「npm 说成功」和「版本变了」不是一回事。有会话在跑就拒绝——正在跑的
进程底下换文件，几百个文件的包会读到新旧混杂的东西。

追溯不到 npm 包的那份（官方安装器装的 claude）明说「不是 dct 装的，
用原来的方式更新」，不去猜他用的是哪种。装完发 RecheckUpdates 让守护
进程立刻重查，提示不用等到明天才消失。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LpSh1dhbMaZVkc4rdDRTrM
EOF
```

---

### Task 4: 界面：列表上的「有新版本」和 `u` 键

**Files:**
- Modify: `src/ui/view.rs`（`pick_action` 旁边加 `UpdateAction` + `update_action`；`help_items` 的 `PickProfile` 两条）
- Modify: `src/ui/pick.rs`（`handle_pick_profile` 加 `u`；`draw_pick_profile` 画提示）
- Modify: `src/i18n.rs`（`Key::PressUToUpdate`、`Key::Update`；`msg::update_available`、`msg::updating`）

**Interfaces:**
- Consumes: `proto::ProfileEntry{update, managed}`、`proto::UpdatePrompt`、`PickAction::Install` 那条现成的开窗口路径。
- Produces:
  - `pub enum UpdateAction { Run { profile: String, command: Vec<String> }, Say(String), Nothing }`
  - `pub fn update_action(e: &ProfileEntry, lang: Lang) -> UpdateAction`

- [ ] **Step 1: 写失败的测试**

`src/ui/view.rs` tests（用那里已有的 `entry(name, status)` helper）：

```rust
    #[test]
    fn u_on_an_entry_with_an_update_runs_dct_install_update() {
        let mut e = entry("claude", ProfileStatus::Ready);
        e.managed = true;
        e.update = Some(UpdatePrompt { installed: "1.0.0".into(), latest: "1.1.0".into() });
        match update_action(&e, Lang::Zh) {
            UpdateAction::Run { profile, command } => {
                assert_eq!(profile, "claude");
                assert_eq!(command, vec!["dct", "install", "claude", "--update"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn u_on_a_managed_entry_without_an_update_says_it_is_current() {
        let mut e = entry("claude", ProfileStatus::Ready);
        e.managed = true;
        let UpdateAction::Say(s) = update_action(&e, Lang::Zh) else { panic!() };
        assert!(s.contains("最新"), "{s}");
    }

    #[test]
    fn u_on_an_unmanaged_entry_says_dct_did_not_install_it() {
        let e = entry("claude", ProfileStatus::Ready);
        let UpdateAction::Say(s) = update_action(&e, Lang::Zh) else { panic!() };
        assert!(s.contains("不是 dct 装的"), "{s}");
    }

    #[test]
    fn u_on_a_blocked_entry_does_nothing() {
        for st in [
            ProfileStatus::NotInstalled { command: "x".into() },
            ProfileStatus::NeedsSecret,
            ProfileStatus::NeedsDependency { label: "Claude".into() },
        ] {
            assert!(matches!(update_action(&entry("x", st), Lang::Zh), UpdateAction::Nothing));
        }
    }

    #[test]
    fn the_help_line_mentions_u_only_when_the_highlighted_entry_has_an_update() {
        // help_items 的 PickProfile 分支要看得到「高亮那一项有没有 update」。
        // 实施者看 help 是怎么从 View 拿数据的（`fn help_line`/`help_items` 的
        // 调用处），造两份 View::PickProfile：一份高亮项带 update，一份不带，
        // 断言前者的帮助文字含 "u"+更新字样、后者不含。
    }
```

`src/ui/pick.rs` tests（用那里的 `press` helper 和一份带 `update` 的 entries）：

```rust
    #[test]
    fn pressing_u_on_an_updatable_agent_opens_a_window_running_the_update() {
        // 照 `an_install_window_never_becomes_the_projects_agent`（ui/mod.rs:2915）
        // 那条测试搭环境：假 daemon / 假 client 记录收到的 Request::Input。
        // 断言：view 变成 Attached，Input 的 text 是 "dct install claude --update\n"，
        // 且 last profile 没被记成 claude（remember: false）。
    }

    #[test]
    fn pressing_u_on_an_unmanaged_agent_only_shows_a_message() {
        // entries[0] Ready 且 managed=false；按 u；view 仍是 PickProfile，
        // app.message 含「不是 dct 装的」。
    }
```

两条 pick 测试的骨架照 `ui/mod.rs:2915` 和 `pick.rs:2035` 附近的测试写——**先读那两条**，它们已经解决了「不连真 socket 怎么测按键」。

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test --lib ui::view && cargo test --lib ui::pick`
Expected: 编译错误。

- [ ] **Step 3: 实现文案**

`i18n.rs`：`Key` 枚举加 `Update`（`en: "update", zh: "更新"`，帮助行用）；`msg` 里加：

```rust
    /// 列表那一行尾巴上的一句。压暗显示，不抢「没装」「没密钥」的眼。
    pub fn update_available(lang: Lang, installed: &str, latest: &str) -> String {
        t!(
            lang,
            en: format!("update {installed} → {latest}, press u"),
            zh: format!("有新版本 {installed} → {latest}，按 u 更新"),
        )
    }

    pub fn updating(lang: Lang, profile: &str) -> String {
        t!(
            lang,
            en: format!("Updating {profile}. When it finishes, press Esc then N."),
            zh: format!("正在更新 {profile}，装完按 Esc 回看板再按 N"),
        )
    }
```

- [ ] **Step 4: 实现 `update_action`**

`src/ui/view.rs`，紧挨 `pick_action`：

```rust
/// 在选 agent 的列表上按 `u` 该干什么。跟 `pick_action` 一样是纯函数。
#[derive(Debug)]
pub enum UpdateAction {
    /// 开一个 shell 会话跑这条命令，走 `PickAction::Install` 那条现成的路。
    Run { profile: String, command: Vec<String> },
    /// 只说一句话，不切视图。
    Say(String),
    /// 这一行的状态各有各的出路（没装 → Enter 装；没密钥 → Enter 填），
    /// `u` 不抢它们的活。
    Nothing,
}

pub fn update_action(e: &ProfileEntry, lang: Lang) -> UpdateAction {
    if e.status != ProfileStatus::Ready {
        return UpdateAction::Nothing;
    }
    match (&e.update, e.managed) {
        (Some(_), _) => UpdateAction::Run {
            profile: e.name.clone(),
            command: vec!["dct".into(), "install".into(), e.name.clone(), "--update".into()],
        },
        (None, true) => UpdateAction::Say(crate::i18n::msg::already_latest(lang, &e.label)),
        (None, false) => UpdateAction::Say(crate::i18n::msg::not_managed_by_dct(lang, &e.label)),
    }
}
```

`help_items` 的两条 `View::PickProfile` 分支：要拿到高亮项。看 `help_items` 的调用方是不是拿得到 `entries`/`state`（`View::PickProfile { entries, state, .. }`），拿得到就：

```rust
        View::PickProfile { entries, state, no_git, .. } => {
            let has_update = state.selected().and_then(|i| entries.get(i)).map_or(false, |e| e.update.is_some());
            let mut items: Vec<(&str, Key)> = vec![("↑↓", Key::Select), ("Enter", Key::Confirm)];
            if *no_git { items.push(("g", Key::InitGitRepo)); }
            if has_update { items.push(("u", Key::Update)); }
            items.push(("", Key::OrPressDigit));
            items.push(("Esc", Key::Cancel));
            help_items(&items, lang)
        }
```

（把原来两条合成一条；`help_items` 的参数类型以源码为准。）

- [ ] **Step 5: 实现 `u` 键和画法**

`handle_pick_profile`，在 `KeyCode::Char('g') && no_git` 那条后面加一条：

```rust
    } else if key.code == KeyCode::Char('u') {
        let act = state
            .selected()
            .and_then(|i| entries.get(i))
            .map(|e| super::view::update_action(e, app.lang));
        app.view = match act {
            Some(super::view::UpdateAction::Run { profile, command }) => {
                // 跟「没装 → 回车 → 开窗口装」一模一样的路，remember: false 的理由也一样。
                let dir = app.current_dir().display().to_string();
                match super::create_session(app, &dir, "shell", false) {
                    Ok(Response::Created { id }) => {
                        let line = format!("{}\n", command.join(" "));
                        let _ = app.client().and_then(|c| c.call(Request::Input { id, text: line }));
                        app.message = msg::updating(app.lang, &profile).into();
                        app.need_sessions = true;
                        View::Attached(id)
                    }
                    _ => {
                        app.message = Msg::err(text(Key::CannotOpenInstallWindow, app.lang).into());
                        same(entries, state, no_git)
                    }
                }
            }
            Some(super::view::UpdateAction::Say(s)) => {
                app.message = s.into();
                same(entries, state, no_git)
            }
            _ => same(entries, state, no_git),
        };
    }
```

**注意** `KeyCode::Char(c) => digit_index(c)` 那条在后面，`u` 不是数字，不会撞；但这条 `else if` 必须排在那个 `match` 之前。`profile` 在 `Run` 里是 name 不是 label，`msg::updating` 要 label——从 `entries[i].label` 取，不要拿 name 显示。

`draw_pick_profile`：`reason` 算完之后加一段，`Ready` 且 `e.update` 是 `Some(p)` 时 `reason = msg::update_available(app.lang, &p.installed, &p.latest)`。它跟 `reason` 用同一个 Span、同样的 `base.patch(dim())`。

- [ ] **Step 6: 跑测试**

Run: `cargo test --lib ui && cargo test`
Expected: PASS。

- [ ] **Step 7: 真机看一眼**

`cargo run` 开界面，按 `N`。如果本机有一个 npm 装的 agent 且仓库有新版，那一行该带「有新版本 … 按 u 更新」、底部帮助行有 `u 更新`；按 `u` 开窗口跑更新。没有的话至少确认：Ready 的 agent 上按 `u` 出现「已经是最新」或「不是 dct 装的」，没装的 agent 上按 `u` 无反应。把看到的写进报告。

- [ ] **Step 8: Commit**

```bash
cargo fmt && git diff --check
git add src/ui/view.rs src/ui/pick.rs src/i18n.rs
git commit -F - <<'EOF'
feat: 选 agent 的列表上显示「有新版本」，按 u 一键更新

提示放在选 agent 的那一屏而不是设置页：学生开工的时候在这儿，看见就
能按。u 走的是「没装 → 回车 → 开窗口装」那条现成的路，只是敲进去的
是 dct install <agent> --update。

u 在 Ready 的行上永远有回应：有新版本就更新，没有就说「已经是最新」，
不是 dct 装的就说「用原来的方式更新」——用户那句「要告诉别人怎么装」。
帮助行只在高亮那一项真的有新版本时才写 u，不挂一个按了没反应的键。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LpSh1dhbMaZVkc4rdDRTrM
EOF
```

---

### Task 5: README 中英各一段

**Files:**
- Modify: `README.zh-CN.md`（「十个 agent，一个入口」一节，`dc.toml` 覆盖那段之后、`<details>` 之前；「会踩到的坑」一节末尾）
- Modify: `README.md`（「Ten agents, one door」同位置；「Things that will annoy you」末尾）

- [ ] **Step 1: 中文**

「十个 agent，一个入口」加：

```markdown
**agent 旧了会告诉你。** `dct` 每天问一次 npm 仓库，哪个 agent 有新版本，选 agent 的
列表上那一行就会多一句「有新版本 2.1.251 → 2.1.260，按 u 更新」。按 `u`，`dct` 开一个
窗口把它升上去，你看着它升。不进界面的话，`dct install claude --update` 是同一件事。
它不会自动替你升：一个教室里所有人手上应该是同一份，什么时候升是你按的那一下。

管不了的它会明说。只有 `dct`（或者 npm）装的那份 `dct` 才认得——用官方安装器装的
Claude、brew 装的 Codex，`dct` 看不见它们的版本、也不会去碰；在那样的行上按 `u`，
它会说「这份 Claude 不是 dct 装的，请用你原来装它的方式更新」。国内课堂把
`DCT_NPM_REGISTRY` 指到镜像上，查版本和装都走那个地址。
```

「会踩到的坑」加：

```markdown
**更新的时候那个 agent 不能有会话在跑。** `dct` 会拒绝，并告诉你关掉哪几个。正在跑的
进程底下换文件，下一次它读自己的文件时拿到的可能是新旧混杂的东西。
```

- [ ] **Step 2: 英文**

「Ten agents, one door」加：

```markdown
**It tells you when an agent is stale.** Once a day `dct` asks the npm registry, and any
agent with a newer version gets a note on its line in the picker: "update 2.1.251 → 2.1.260,
press u". Press `u` and `dct` opens a window and runs the update while you watch. Outside
the interface, `dct install claude --update` is the same thing. It never updates on its
own: everyone in a classroom should be on the same build, and the moment it changes is
the keypress you make.

It also says when it can't. `dct` only recognises the copy that it (or npm) installed —
a Claude from the official installer or a Codex from brew is invisible to it and never
touched; press `u` on one of those and it says so and tells you to update it the way you
installed it. Point `DCT_NPM_REGISTRY` at a mirror and both the version check and the
install go there.
```

「Things that will annoy you」加：

```markdown
**No sessions of that agent may be running while it updates.** `dct` refuses and names
the ones to close. Swapping files under a live process means its next read of itself
can land on a mix of old and new.
```

- [ ] **Step 3: 核对文案跟实际行为一致**

对着 Task 3/4 的输出：提示句的措辞、`u` 键、命令名、拒绝时那句话——README 里引用的每一句都要在 `i18n.rs` 里找得到对应。不一致就改 README，不改代码。

- [ ] **Step 4: Commit**

```bash
git diff --check
git add README.md README.zh-CN.md
git commit -F - <<'EOF'
docs: README 说清楚 agent 怎么更新、哪些 dct 管不了

用户那句「你要告诉别人怎么装」：列表上看到「有新版本」按 u；不进界面
用 dct install <agent> --update；官方安装器装的那份 dct 看不见也不碰，
用原来的方式更新。「会踩到的坑」加一条：更新时那个 agent 不能有会话在跑。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LpSh1dhbMaZVkc4rdDRTrM
EOF
```

---

## 自查记录

**Spec 覆盖：** 一节（归谁管、package.json 两种布局 + 符号链接、包名解析、不归管时的两句话）→ Task 1 + Task 4；二节（守护进程查、URL、超时、30 秒/24 小时/RecheckUpdates、内存表、查不到不报错、版本比较）→ Task 1 + Task 2；三节（`update`/`managed` 字段、画法、`u`、帮助行、`update_action`）→ Task 2 + Task 4；四节（`--update` 五步、参数解析、普通 install 也发 Recheck）→ Task 3；五节（README 三件事 + 坑）→ Task 5；测试段每一条都有对应测试，curl 验证在 Task 2 Step 7。

**已知不确定、由实施者以源码为准的地方：** `help_items` 能不能拿到 `state`；`handle()` 在 `src/web/` 里有没有别的调用点；`SessionInfo` 的完整字段；`main.rs` 里 `args` 的类型；`Request` 的 `Debug` 测试长什么样。这些在各任务里都点名了。
