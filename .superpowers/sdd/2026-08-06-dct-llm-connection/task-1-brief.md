### Task 1: `~/.dct/config.toml` 的 `[llm]` 段

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs`（加 `pub mod config;`）

**Interfaces:**
- Consumes: 无
- Produces:
  - `config::Transport { Cli, Http }`（`#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]`，serde rename 成小写）
  - `config::LlmConfig { pub provider: String, pub model: Option<String>, pub base_url: Option<String>, pub transport: Transport }`
  - `config::Config { pub llm: LlmConfig }`
  - `Config::from_toml(s: &str) -> anyhow::Result<Config>`
  - `Config::load(path: &Path) -> Config`（文件不存在 = 默认值，**不报错**）
  - `config::config_path_for_socket(socket: &Path) -> PathBuf`

**说明：** 默认 provider 是 `"claude"`、transport 是 `Cli`——那是用户最可能已经登录过的 CLI，且 `Cli` 这条路不需要任何凭据。配置整段缺失必须等价于默认值，否则老用户升级上来 dct 就起不来了。

- [ ] **Step 1: 写失败的测试**

在 `src/config.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_llm_section() {
        let c = Config::from_toml(
            r#"
            [llm]
            provider = "kimi"
            model = "kimi-k2"
            base_url = "https://example.test/v1"
            transport = "http"
            "#,
        )
        .unwrap();
        assert_eq!(c.llm.provider, "kimi");
        assert_eq!(c.llm.model.as_deref(), Some("kimi-k2"));
        assert_eq!(c.llm.base_url.as_deref(), Some("https://example.test/v1"));
        assert_eq!(c.llm.transport, Transport::Http);
    }

    #[test]
    fn an_empty_file_is_all_defaults() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c.llm.provider, "claude", "默认用最可能已登录的 CLI");
        assert_eq!(c.llm.transport, Transport::Cli, "默认走不需要凭据的那条路");
        assert!(c.llm.model.is_none());
        assert!(c.llm.base_url.is_none());
    }

    #[test]
    fn a_partial_llm_section_keeps_the_other_defaults() {
        let c = Config::from_toml("[llm]\nprovider = \"codex\"\n").unwrap();
        assert_eq!(c.llm.provider, "codex");
        assert_eq!(c.llm.transport, Transport::Cli);
    }

    #[test]
    fn a_missing_file_is_defaults_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("nope.toml"));
        assert_eq!(c.llm.provider, "claude");
    }

    #[test]
    fn a_broken_file_falls_back_to_defaults_and_does_not_panic() {
        // 配置坏了不该让 dct 起不来——LLM 是增强，不是地基。
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "[llm\nprovider =").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.llm.provider, "claude");
    }

    #[test]
    fn config_path_sits_next_to_the_socket() {
        let p = config_path_for_socket(std::path::Path::new("/home/x/.dct/daemon.sock"));
        assert_eq!(p, std::path::PathBuf::from("/home/x/.dct/config.toml"));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `env GOCACHE=/tmp/x cargo test --lib config:: 2>&1 | tail -20`
Expected: 编译失败，`unresolved module or unlinked crate 'config'`

- [ ] **Step 3: 写实现**

`src/config.rs` 顶部：

```rust
//! `~/.dct/config.toml`。目前只有 `[llm]` 一段。
//!
//! **配置坏了绝不能让 dct 起不来。** LLM 是增强，不是地基——解析失败
//! 一律退回默认值，只往 stderr 留一行痕迹。

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// 把 provider 的 CLI 用无界面模式拉起来。认证是那个 CLI 自己的事。
    Cli,
    /// 直接打 HTTP 端点。需要凭据。
    Http,
}

fn default_provider() -> String {
    "claude".to_string()
}

fn default_transport() -> Transport {
    Transport::Cli
}

#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default = "default_transport")]
    pub transport: Transport,
}

impl Default for LlmConfig {
    fn default() -> Self {
        LlmConfig {
            provider: default_provider(),
            model: None,
            base_url: None,
            transport: default_transport(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub llm: LlmConfig,
}

impl Config {
    pub fn from_toml(s: &str) -> anyhow::Result<Config> {
        Ok(toml::from_str(s)?)
    }

    /// 读不到、解析不了，一律是默认值。见模块头注释。
    pub fn load(path: &Path) -> Config {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
            Err(e) => {
                eprintln!("配置读取失败（{}）：{e}", path.display());
                return Config::default();
            }
        };
        match Config::from_toml(&src) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("配置解析失败（{}）：{e}", path.display());
                Config::default()
            }
        }
    }
}

/// 跟着 socket 走，测试自动隔离（同 `secrets_path_for_socket`）。
pub fn config_path_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("config.toml"),
        None => PathBuf::from("config.toml"),
    }
}
```

`src/lib.rs` 在 `pub mod client;` 后面加一行 `pub mod config;`（保持字母序）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib config::`
Expected: 6 passed

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib config:: && git add src/config.rs src/lib.rs
git commit -m "feat(config): add ~/.dct/config.toml with an [llm] section

Defaults to the claude profile over the CLI transport, which needs no
credential at all. A missing or broken config falls back to defaults
instead of failing to start: the LLM is an enhancement, not a foundation."
```

---

