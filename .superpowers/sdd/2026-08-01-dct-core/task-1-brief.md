### Task 1: 项目骨架与 Profile

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/profile.rs`
- Create: `profiles/claude.toml`
- Create: `profiles/shell.toml`

**Interfaces:**
- Consumes: 无
- Produces: `profile::Profile { name: String, command: Vec<String>, idle_pattern: Option<String>, is_agent: bool }`；`Profile::from_toml(&str) -> anyhow::Result<Profile>`；`Profile::builtin(&str) -> Option<Profile>`；`Profile::builtin_names() -> Vec<&'static str>`；`Profile::idle_regex(&self) -> anyhow::Result<Option<regex::Regex>>`

- [ ] **Step 1: 建 Cargo 项目**

```bash
cd /Users/lei/work/dc/dc-terminal
cargo init --name dct
```

把 `Cargo.toml` 的 `[dependencies]` 写成：

```toml
[dependencies]
anyhow = "1"
regex = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
portable-pty = "0.8"
vt100 = "0.15"
ratatui = "0.28"
crossterm = "0.28"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: 写内置 profile 文件**

`profiles/claude.toml`：

```toml
name = "claude"
command = ["claude", "--dangerously-skip-permissions"]
is_agent = true
idle_pattern = "\\? for shortcuts"
```

`profiles/shell.toml`：

```toml
name = "shell"
command = ["/bin/zsh"]
is_agent = false
```

- [ ] **Step 3: 写失败的测试**

在 `src/profile.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_toml() {
        let p = Profile::from_toml(
            r#"
            name = "demo"
            command = ["echo", "hi"]
            is_agent = true
            idle_pattern = "\\$ $"
            "#,
        )
        .unwrap();
        assert_eq!(p.name, "demo");
        assert_eq!(p.command, vec!["echo", "hi"]);
        assert!(p.is_agent);
    }

    #[test]
    fn builtin_claude_uses_bypass_flag() {
        let p = Profile::builtin("claude").unwrap();
        assert!(p.command.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(p.is_agent);
    }

    #[test]
    fn builtin_shell_is_not_agent() {
        let p = Profile::builtin("shell").unwrap();
        assert!(!p.is_agent);
        assert!(p.idle_pattern.is_none());
    }

    #[test]
    fn builtin_names_lists_both() {
        let names = Profile::builtin_names();
        assert!(names.contains(&"claude"));
        assert!(names.contains(&"shell"));
    }

    #[test]
    fn unknown_builtin_is_none() {
        assert!(Profile::builtin("nope").is_none());
    }

    #[test]
    fn idle_regex_compiles() {
        let p = Profile::builtin("claude").unwrap();
        let re = p.idle_regex().unwrap().unwrap();
        assert!(re.is_match("  ? for shortcuts"));
    }
}
```

- [ ] **Step 4: 跑测试确认失败**

Run: `cargo test profile`
Expected: 编译失败，`Profile` 未定义。

- [ ] **Step 5: 实现 Profile**

`src/profile.rs`：

```rust
use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub is_agent: bool,
    #[serde(default)]
    pub idle_pattern: Option<String>,
}

const CLAUDE: &str = include_str!("../profiles/claude.toml");
const SHELL: &str = include_str!("../profiles/shell.toml");

impl Profile {
    pub fn from_toml(s: &str) -> Result<Profile> {
        toml::from_str(s).context("profile TOML 解析失败")
    }

    pub fn builtin(name: &str) -> Option<Profile> {
        let src = match name {
            "claude" => CLAUDE,
            "shell" => SHELL,
            _ => return None,
        };
        Some(Profile::from_toml(src).expect("内置 profile 必须能解析"))
    }

    pub fn builtin_names() -> Vec<&'static str> {
        vec!["claude", "shell"]
    }

    pub fn idle_regex(&self) -> Result<Option<regex::Regex>> {
        match &self.idle_pattern {
            None => Ok(None),
            Some(p) => Ok(Some(
                regex::Regex::new(p).with_context(|| format!("idle_pattern 不是合法正则: {p}"))?,
            )),
        }
    }
}
```

`src/main.rs`：

```rust
mod profile;

fn main() {
    println!("dct");
}
```

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test profile`
Expected: 6 个测试全部 PASS。

- [ ] **Step 7: 提交**

```bash
git add Cargo.toml Cargo.lock src/ profiles/
git commit -m "feat: profile 结构与内置 claude/shell profile"
```

---

