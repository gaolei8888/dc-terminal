### Task 3: 界面用的三个纯函数

**Files:**
- Modify: `src/ui.rs`

**Interfaces:**
- Consumes: 无
- Produces:
  - `ui::expand_path(input: &str, base: &Path) -> PathBuf`
  - `ui::filter_projects(all: &[String], filter: &str) -> Vec<String>`
  - `ui::move_sel_n(st: &mut ListState, len: usize, delta: i32)`（现有 `move_sel` 改为委托给它）

**说明：** 选择器里真正有逻辑的部分全在这三个纯函数里，先单独做出来并测好，
Task 5 的交互代码就只剩接线。

- [ ] **Step 1: 写失败的测试**

在 `src/ui.rs` 的 `mod tests` 里追加（放在 `fn buffer_text` 之前）：

```rust
    #[test]
    fn expand_path_handles_tilde_and_relative() {
        let base = std::path::Path::new("/base");
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());

        assert_eq!(expand_path("/abs/x", base), std::path::PathBuf::from("/abs/x"));
        assert_eq!(expand_path("~/x", base), home.join("x"));
        assert_eq!(expand_path("~", base), home);
        assert_eq!(expand_path("rel/x", base), std::path::PathBuf::from("/base/rel/x"));
        // 用户粘贴路径常带尾随空格
        assert_eq!(expand_path("  /abs/x  ", base), std::path::PathBuf::from("/abs/x"));
        // `~foo` 不是家目录展开，是个叫 ~foo 的相对路径
        assert_eq!(expand_path("~foo", base), std::path::PathBuf::from("/base/~foo"));
    }

    #[test]
    fn filter_projects_is_case_insensitive_substring() {
        let all = vec![
            "/Users/lei/work/dc/dc-terminal".to_string(),
            "/Users/lei/work/dc/dc_workbench".to_string(),
            "/Users/lei/tmp/scratch".to_string(),
        ];

        assert_eq!(filter_projects(&all, "").len(), 3, "空过滤词返回全部");
        assert_eq!(filter_projects(&all, "WORK").len(), 3, "不区分大小写");
        assert_eq!(
            filter_projects(&all, "dc-term"),
            vec!["/Users/lei/work/dc/dc-terminal".to_string()],
            "匹配的是完整路径的任意位置"
        );
        assert_eq!(filter_projects(&all, "scratch").len(), 1);
        assert!(filter_projects(&all, "没有这个").is_empty());
    }

    #[test]
    fn move_sel_n_clamps_at_both_ends() {
        let mut st = ListState::default();
        st.select(Some(0));

        move_sel_n(&mut st, 3, -1);
        assert_eq!(st.selected(), Some(0), "顶端再往上不动");

        move_sel_n(&mut st, 3, 1);
        move_sel_n(&mut st, 3, 1);
        move_sel_n(&mut st, 3, 1);
        assert_eq!(st.selected(), Some(2), "底端再往下不动");

        // 空列表不能 panic，也不能选中不存在的行
        let mut empty = ListState::default();
        move_sel_n(&mut empty, 0, 1);
        assert_eq!(empty.selected(), None);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib ui -- --test-threads=1`
Expected: 编译失败，`expand_path` / `filter_projects` / `move_sel_n` 未定义。

- [ ] **Step 3: 实现**

`src/ui.rs` 顶部的 `use std::path::PathBuf;` 改成：

```rust
use std::path::{Path, PathBuf};
```

把现有的 `move_sel`（`src/ui.rs:362`）**整个替换**成下面两个函数：

```rust
/// 光标移动的通用版本：只认列表长度，不认列表里装的是什么。
/// 项目选择器和会话看板共用它。
fn move_sel_n(st: &mut ListState, len: usize, delta: i32) {
    if len == 0 {
        st.select(None);
        return;
    }
    let cur = st.selected().unwrap_or(0) as i32;
    let next = (cur + delta).clamp(0, len as i32 - 1);
    st.select(Some(next as usize));
}

fn move_sel(st: &mut ListState, sessions: &[SessionInfo], delta: i32) {
    move_sel_n(st, sessions.len(), delta);
}
```

在 `short_path`（`src/ui.rs:351`）后面加这两个：

```rust
/// 把用户敲进来的路径变成绝对路径：`~` 展开成家目录，相对路径按 `base` 解析。
/// 只做字符串层面的展开，**不做存在性校验**——调用方自己决定不存在时怎么办。
fn expand_path(input: &str, base: &Path) -> PathBuf {
    // 粘贴进来的路径经常带尾随空格
    let t = input.trim();
    let home = || PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));

    if t == "~" {
        return home();
    }
    // 只认 `~/`：`~foo` 是别人的家目录（我们不支持），当普通相对路径处理
    if let Some(rest) = t.strip_prefix("~/") {
        return home().join(rest);
    }
    let p = Path::new(t);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// 不区分大小写的子串过滤。匹配**完整路径**而不只是目录名，
/// 这样 `work` 和 `dc-term` 都能用来找同一个项目。
fn filter_projects(all: &[String], filter: &str) -> Vec<String> {
    if filter.is_empty() {
        return all.to_vec();
    }
    let f = filter.to_lowercase();
    all.iter()
        .filter(|p| p.to_lowercase().contains(&f))
        .cloned()
        .collect()
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -- --test-threads=1`
Expected: 全绿。

此时 `expand_path` / `filter_projects` 还没有调用点，`cargo build` 会报 `dead_code` 警告——
**这是预期的**，Task 5 接线后消失。不要为了消警告加 `#[allow(dead_code)]`。

- [ ] **Step 5: 提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt && git add -A
git commit -m "feat: 路径展开、项目过滤与通用光标移动"
```

---

