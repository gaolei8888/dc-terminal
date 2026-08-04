## Task 3: 从磁盘加载自定义 profile

**Files:**
- Modify: `src/profile.rs`
- Test: `src/profile.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 1、2
- Produces: `profiles_dir_for_socket(socket: &Path) -> PathBuf`、`load_dir(dir: &Path) -> (Vec<Profile>, Vec<String>)`（返回解析成功的 profile 和每个失败文件的人话错误）、`all_profiles(dir: &Path) -> (Vec<Profile>, Vec<String>)`（内置 + 磁盘，同名磁盘覆盖内置，顺序保持内置在前、磁盘新增的按文件名排在后面）

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn profiles_dir_sits_next_to_socket() {
    let p = profiles_dir_for_socket(std::path::Path::new("/home/x/.dct/daemon.sock"));
    assert_eq!(p, std::path::PathBuf::from("/home/x/.dct/profiles"));
}

#[test]
fn disk_profile_overrides_builtin_of_same_name() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("claude.toml"),
        "name = \"claude\"\ncommand = [\"my-claude\"]\n",
    )
    .unwrap();

    let (all, errs) = all_profiles(tmp.path());
    assert!(errs.is_empty());
    let claude = all.iter().find(|p| p.name == "claude").unwrap();
    assert_eq!(claude.command, vec!["my-claude"], "磁盘的同名 profile 要覆盖内置");
    assert_eq!(
        all.iter().filter(|p| p.name == "claude").count(),
        1,
        "覆盖不是追加"
    );
}

#[test]
fn disk_profile_with_new_name_is_appended_after_builtins() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("mine.toml"),
        "name = \"mine\"\ncommand = [\"echo\"]\n",
    )
    .unwrap();

    let (all, _) = all_profiles(tmp.path());
    assert_eq!(all.last().unwrap().name, "mine", "新增的排在内置后面");
    assert_eq!(all[0].name, "claude", "内置顺序不受影响");
}

#[test]
fn broken_disk_profile_reports_the_filename_and_keeps_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("bad.toml"), "这不是 TOML {{{").unwrap();
    std::fs::write(
        tmp.path().join("good.toml"),
        "name = \"good\"\ncommand = [\"echo\"]\n",
    )
    .unwrap();

    let (all, errs) = all_profiles(tmp.path());
    assert!(all.iter().any(|p| p.name == "good"), "一个坏文件不能连累其它的");
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("bad.toml"), "错误里要说是哪个文件：{}", errs[0]);
}

#[test]
fn missing_dir_is_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (all, errs) = all_profiles(&tmp.path().join("根本没这个目录"));
    assert!(errs.is_empty(), "没建过自定义目录是常态，不是错误");
    assert_eq!(all.len(), 9, "只有内置");
}

#[test]
fn non_toml_files_are_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("README.md"), "随手放的笔记").unwrap();
    let (_, errs) = all_profiles(tmp.path());
    assert!(errs.is_empty(), "非 .toml 文件直接跳过，不该报错");
}
```

`tempfile` 已经在 `[dev-dependencies]` 里，`src/profile.rs` 的 `mod tests` 直接用。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib profile`
Expected: FAIL，`cannot find function 'all_profiles'`

- [ ] **Step 3: 实现**

加到 `src/profile.rs`（`impl Profile` 之外，模块级）：

```rust
use std::path::{Path, PathBuf};

/// 自定义 profile 目录，跟着 socket 走——测试把 socket 放临时目录就自动隔离，
/// 不会去读用户真实的 ~/.dct/profiles/（同 `projects::store_path_for_socket`）。
pub fn profiles_dir_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("profiles"),
        None => PathBuf::from("profiles"),
    }
}

/// 读一个目录下所有 `*.toml`。第二个返回值是每个读不了的文件的人话错误——
/// **不能静默跳过**：用户自己写的 profile 没出现在菜单里，他需要知道为什么。
pub fn load_dir(dir: &Path) -> (Vec<Profile>, Vec<String>) {
    let mut found = Vec::new();
    let mut errs = Vec::new();

    // 目录不存在是常态（大多数用户不会建），不是错误
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (found, errs);
    };

    // read_dir 的顺序由文件系统决定，不排序的话菜单每次启动都可能换序
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();

    for path in paths {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        match std::fs::read_to_string(&path) {
            Err(e) => errs.push(format!("{name} 读不了：{e}")),
            Ok(src) => match Profile::from_toml(&src) {
                Err(e) => errs.push(format!("{name} 写错了：{e}")),
                Ok(p) => found.push(p),
            },
        }
    }
    (found, errs)
}

/// 内置 + 磁盘。同名以磁盘为准（用户改了就是要改），新名字追加在后面。
pub fn all_profiles(dir: &Path) -> (Vec<Profile>, Vec<String>) {
    let (disk, errs) = load_dir(dir);
    let mut out = Profile::builtins();
    for p in disk {
        match out.iter_mut().find(|b| b.name == p.name) {
            Some(slot) => *slot = p,
            None => out.push(p),
        }
    }
    (out, errs)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib profile`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/profile.rs
git commit -m "feat: 从 ~/.dct/profiles/ 读自定义 profile

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

