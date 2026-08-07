# dct LLM 连接层 + 出错解释 实施计划（Plan A）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 dct 自己能调一个 LLM（CLI 无界面 / HTTP 两条路，Key / SSO 两种认证），并用它做第一件事——会话失败时说人话。

**Architecture:** `src/llm/` 三层：后端 trait（`CliBackend` 拉起 agent CLI 的无界面模式，`HttpBackend` 走 ureq）、凭据解析（`Inherit` / `Key` / `Bearer`，提取一律返回 `Option`）、供应商登记表长在已有的 `profiles/*.toml` 上。所有调用跑在工作线程上，带硬超时。

**Tech Stack:** Rust，已有依赖（`ureq` 2 带 tls+json、`serde`、`serde_json`、`toml`、`anyhow`、`regex`）。**不新增任何 crate。**

**Spec:** `docs/superpowers/specs/2026-08-06-dct-llm-connection-design.md`
**总纲:** `docs/superpowers/specs/2026-08-06-dct-supervisor-vision.md`
**Plan B（渠道 + 自答流水线）在本计划完成后另写。**

## Global Constraints

- Rust ≥ 1.80，edition 2021，单 crate，二进制 `dct`
- **不引入 async 运行时**。阻塞 IO + 线程
- **不新增 crate 依赖**
- **绝不进 TUI 重绘循环**。所有 LLM 调用在工作线程上，带硬超时
- **每一处 LLM 用法都有不依赖 LLM 的退路**，退化方向是「dct 变回今天的样子」
- 用户可见错误**报码不报句子**：新增 `ErrorCode` / `WarningCode` 变体 + `i18n` 词条，不在逻辑里拼中文
- **测试永远不读真实 Keychain、不写真实 `~/.dct/`**。路径一律走 `*_for_socket(socket)` 约定
- **不用 emoji 当图标**（项目规则）
- 每个任务结束：`cargo fmt --check`、`cargo test` 全绿、`git diff --check` 干净，然后提交
- **提交信息用英文，不加 AI 署名**

---

### Task 1: `~/.dct/config.toml` 的 `[llm]` 段

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs`（加 `pub mod config;`）

**Interfaces:**
- Consumes: 无
- Produces:
  - `config::Transport { Cli, Http }`（`#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]`，serde rename 成小写）
  - `config::LlmConfig { pub provider: String, pub model: Option<String>, pub base_url: Option<String>, pub transport: Transport }`
  - `config::Config { pub llm: Option<LlmConfig> }`
  - `Config::from_toml(s: &str) -> anyhow::Result<Config>`
  - `Config::load(path: &Path) -> Config`（文件不存在 = `llm: None`，**不报错**）
  - `config::config_path_for_socket(socket: &Path) -> PathBuf`

**说明：** `llm` 是 `Option`，不是默认开着的 `LlmConfig`——**这是隐私边界，不是随手选的类型**（2026-08-06 fix round 1，Critical）。出错解释（Task 8）会把一个失败会话屏幕上最后 2000 个字符原样送给配置里指定的模型，而那正是 `Invalid API key: sk-ant-...`、`Authorization: Bearer ...`、`.env` 内容、带 token 的 git 地址最容易出现的地方。把这功能打开必须是用户的一次主动动作，不能因为「用户什么都没配」就替他默认打开、把他终端里的东西发给第三方。

文件不存在、内容为空、这一段没写、整份文件解析坏了——一律落在 `llm: None` 上，功能整个关着，`daemon.rs` 连 `resolve()` 都不会调用（见 Task 8）。**只有用户显式写下 `[llm]`**（哪怕后面什么都不填）才算「我要开」，那一刻开始，段内每个字段该有什么默认值（`provider` 默认 `"claude"`、`transport` 默认 `Cli`——那是用户最可能已经登录过的 CLI，且 `Cli` 这条路不需要任何凭据）还是照旧生效。「要不要开」和「开了之后怎么配」是两件事，前者靠 `Option`，后者靠 `LlmConfig` 内部的 `#[serde(default = ...)]`。

⚠️ 下面 Step 1/Step 3 的代码块是本任务**最初**的设计（`llm: LlmConfig`，配置缺失 = 默认开着），已被 2026-08-06 fix round 1 的 Critical 修复取代。实现前请直接对照当前 `src/config.rs`（`llm: Option<LlmConfig>`），不要照抄下面这份旧代码。

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
Expected (post fix-round-1, `llm: Option<LlmConfig>`): 8 passed — no `[llm]` section /
empty file / missing file / broken file all assert `llm.is_none()`; a bare `[llm]` and a
full `[llm]` both assert `llm.is_some()` with the right fields.

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib config:: && git add src/config.rs src/lib.rs
git commit -m "feat(config): add ~/.dct/config.toml with an [llm] section

llm is Option<LlmConfig>, not a default-on LlmConfig: turning on the feature
that sends failed-session screen text to a model must be a deliberate act,
not something a user gets by having no config file at all. Writing a bare
[llm] opts in; the fields inside it still default to the claude profile
over the CLI transport, which needs no credential at all."
```

---

### Task 2: profile 的 `[headless]` 与 `[api]` 块

**Files:**
- Modify: `src/profile.rs`（`Profile` 结构 + 新增两个 spec 结构）
- Modify: `profiles/claude.toml`, `profiles/codex.toml`, `profiles/kimi.toml`, `profiles/glm.toml`, `profiles/deepseek.toml`, `profiles/qwen-api.toml`

**Interfaces:**
- Consumes: 无
- Produces:
  - `profile::HeadlessSpec { pub command: Vec<String> }`
  - `profile::Wire { Openai, Anthropic }`（`Deserialize`，rename 小写）
  - `profile::ApiSpec { pub base_url: String, pub wire: Wire }`
  - `Profile` 新增两个字段：`pub headless: Option<HeadlessSpec>`、`pub api: Option<ApiSpec>`

**说明：** 这是「登记表长在 profile 上，不另起平行清单」那条决定的落地。**只给实测过无界面模式的 profile 写 `[headless]`**——`opencode` 和 `qwen` 本机没装、没验过，留空，与既有的「没验过就不填 pattern」是同一条纪律。

`[api]` 的 `base_url` 与 profile 里已有的 `[env] ANTHROPIC_BASE_URL` 是同一个值，但**不能合并**：`env` 是给子进程用的，`api` 是给 dct 自己用的，两者将来会分叉（比如 dc_llm 只有 `[api]` 没有 `[env]`）。

- [ ] **Step 1: 写失败的测试**

在 `src/profile.rs` 的 `mod tests` 里追加：

```rust
#[test]
fn claude_and_codex_declare_a_headless_command() {
    // 这两个是本机实测过的：`claude -p` 和 `codex exec`。
    for name in ["claude", "codex"] {
        let p = Profile::builtin(name).unwrap();
        let h = p.headless.as_ref().unwrap_or_else(|| panic!("{name}: 要有 [headless]"));
        assert!(!h.command.is_empty(), "{name}: headless 命令不能为空");
    }
    assert_eq!(
        Profile::builtin("claude").unwrap().headless.unwrap().command,
        vec!["claude".to_string(), "-p".to_string()]
    );
    assert_eq!(
        Profile::builtin("codex").unwrap().headless.unwrap().command,
        vec!["codex".to_string(), "exec".to_string()]
    );
}

#[test]
fn unverified_clis_declare_no_headless_command() {
    // opencode / qwen 本机没装，无界面模式没验过。编一个出来 = 造一条
    // 用户按了就报错的路，和「没验过就不填 pattern」是同一条纪律。
    for name in ["opencode", "qwen"] {
        let p = Profile::builtin(name).unwrap();
        assert!(p.headless.is_none(), "{name}: 没实测过就别填 [headless]");
    }
}

#[test]
fn api_shaped_profiles_declare_an_api_block() {
    for name in ["kimi", "glm", "deepseek", "qwen-api"] {
        let p = Profile::builtin(name).unwrap();
        let api = p.api.as_ref().unwrap_or_else(|| panic!("{name}: 要有 [api]"));
        assert!(api.base_url.starts_with("https://"), "{name}: base_url 要是 https");
        assert_eq!(api.wire, Wire::Anthropic, "{name}: 这四个都是 Anthropic 兼容形态");
    }
}

#[test]
fn the_api_base_url_matches_the_env_base_url() {
    // 两个字段现在值相同但用途不同（env 给子进程，api 给 dct 自己）。
    // 不合并，但要一致——不一致意味着有人只改了一边。
    for name in ["kimi", "glm", "deepseek", "qwen-api"] {
        let p = Profile::builtin(name).unwrap();
        let env = p.env.get("ANTHROPIC_BASE_URL").unwrap();
        assert_eq!(&p.api.as_ref().unwrap().base_url, env, "{name}: 两处 base_url 不一致");
    }
}

#[test]
fn a_profile_without_the_new_blocks_still_parses() {
    // 用户手写的老 profile 不能因为加了新字段就读不了。
    let p = Profile::from_toml("name = \"x\"\ncommand = [\"x\"]\n").unwrap();
    assert!(p.headless.is_none());
    assert!(p.api.is_none());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib profile::`
Expected: 编译失败，`no field 'headless' on type 'Profile'`

- [ ] **Step 3: 写实现**

`src/profile.rs`，在 `InstallSpec` 定义之后插入：

```rust
/// 怎么把这个 profile 用**无界面**方式跑一次（dct 自己要用模型时走这条）。
///
/// 命令后面会追加提示词，stdout 就是回答。
/// **只给实测过的 profile 写**——编一个出来等于造一条用户按了就报错的路。
#[derive(Debug, Clone, Deserialize)]
pub struct HeadlessSpec {
    pub command: Vec<String>,
}

/// HTTP 端点说的是哪种话。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wire {
    Openai,
    Anthropic,
}

/// 这个 profile 背后的 HTTP 端点，给 dct 自己直连用。
///
/// 和 `[env] ANTHROPIC_BASE_URL` 值相同但**不合并**：那个是给子进程的，
/// 这个是给 dct 的，将来会分叉（dc_llm 只有 `[api]`，没有 `[env]`）。
#[derive(Debug, Clone, Deserialize)]
pub struct ApiSpec {
    pub base_url: String,
    pub wire: Wire,
}
```

`Profile` 结构体在 `pub install: Option<InstallSpec>,` 之后加：

```rust
    #[serde(default)]
    pub headless: Option<HeadlessSpec>,
    #[serde(default)]
    pub api: Option<ApiSpec>,
```

`profiles/claude.toml` 末尾追加：

```toml
[headless]
# 实测：`claude -p/--print` 是官方的非交互模式，提示词作为参数追加。
command = ["claude", "-p"]
```

`profiles/codex.toml` 末尾追加：

```toml
[headless]
# 实测 codex v0.146.0：`codex exec` 是官方的非交互模式。
command = ["codex", "exec"]
```

`profiles/kimi.toml` 末尾追加：

```toml
[headless]
command = ["claude", "-p"]

[api]
base_url = "https://api.moonshot.cn/anthropic"
wire = "anthropic"
```

`profiles/glm.toml` 末尾追加：

```toml
[headless]
command = ["claude", "-p"]

[api]
base_url = "https://open.bigmodel.cn/api/anthropic"
wire = "anthropic"
```

`profiles/deepseek.toml` 与 `profiles/qwen-api.toml` 同样追加 `[headless]`（`["claude", "-p"]`）与 `[api]`，`base_url` **必须逐字照抄该文件 `[env] ANTHROPIC_BASE_URL` 的值**，`wire = "anthropic"`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib profile::`
Expected: 全绿，含新增 5 个

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && git add src/profile.rs profiles/
git commit -m "feat(profile): declare headless commands and API endpoints on profiles

The provider registry lives on profiles/*.toml rather than beside it, so
there is never a second vendor list to keep in sync.

Only claude and codex get a [headless] block; opencode and qwen have never
been run non-interactively here, and inventing a command would create a path
that errors the moment a user takes it."
```

---

### Task 3: 凭据来源（`Inherit` / `Key` / `Bearer`）

**Files:**
- Create: `src/llm/mod.rs`（本任务只放 `pub mod creds;` 一行占位，Task 4 填正文）
- Create: `src/llm/creds.rs`
- Modify: `src/lib.rs`（加 `pub mod llm;`）

**Interfaces:**
- Consumes: 无
- Produces:
  - `llm::creds::Credential { Inherit, Key(String), Bearer(String) }`
  - `llm::creds::parse_claude_oauth(json: &str) -> Option<String>`
  - `llm::creds::parse_codex_auth(json: &str) -> Option<Credential>`
  - `llm::creds::read_claude_oauth() -> Option<String>`（macOS 走 `security`，Linux 走文件）
  - `llm::creds::read_codex_auth() -> Option<Credential>`

**说明：** 这是整个计划里唯一碰用户凭据的地方，纪律最严：

1. **所有解析函数返回 `Option`，不返回 `Result`。** 厂商格式说变就变，格式变了要退化成「填个 key」，不是让 dct 报错。
2. **纯解析和真实读取分开。** 测试只测纯解析，喂**手写的**样本 JSON。**测试绝不调 `security`、绝不读真实 Keychain 或 `~/.codex/auth.json`。**
3. **`Credential` 不许实现 `Debug`/`Display` 打出明文。** 手写 `Debug` 打成 `Key(<redacted>)`。

本机实测到的形状（写死在测试样本里）：

- Claude Code (macOS)：Keychain service `Claude Code-credentials`，内容 `{"claudeAiOauth":{"accessToken":"...","refreshToken":"..."}}`
- Claude Code (Linux)：`~/.claude/.credentials.json`，同一形状
- Codex：`~/.codex/auth.json`，`{"auth_mode":"...","OPENAI_API_KEY":null,"tokens":{"access_token":"...","refresh_token":"...","account_id":"..."}}`
  —— `OPENAI_API_KEY` 非 null 时用它（`Key`），否则用 `tokens.access_token`（`Bearer`）。**`auth_mode` 字段本身就告诉我们走哪条，不用猜。**

- [ ] **Step 1: 写失败的测试**

`src/llm/creds.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // 样本全是手写的假数据。测试永远不读真实 Keychain 或 auth.json。
    const CLAUDE_SAMPLE: &str = r#"{"claudeAiOauth":{"accessToken":"at-fake","refreshToken":"rt-fake"}}"#;
    const CODEX_SSO_SAMPLE: &str = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,
        "tokens":{"id_token":"id","access_token":"at-fake","refresh_token":"rt","account_id":"acct"}}"#;
    const CODEX_KEY_SAMPLE: &str = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-fake","tokens":null}"#;

    #[test]
    fn reads_the_claude_oauth_access_token() {
        assert_eq!(parse_claude_oauth(CLAUDE_SAMPLE).as_deref(), Some("at-fake"));
    }

    #[test]
    fn codex_sso_login_yields_a_bearer() {
        assert_eq!(parse_codex_auth(CODEX_SSO_SAMPLE), Some(Credential::Bearer("at-fake".into())));
    }

    #[test]
    fn codex_api_key_login_yields_a_key() {
        assert_eq!(parse_codex_auth(CODEX_KEY_SAMPLE), Some(Credential::Key("sk-fake".into())));
    }

    /// 这组就是整个模块存在的理由：厂商改格式时**退化，不是报错**。
    #[test]
    fn any_unexpected_shape_yields_none_never_an_error() {
        let junk = [
            "",
            "not json at all",
            "{}",
            r#"{"claudeAiOauth":{}}"#,
            r#"{"claudeAiOauth":{"accessToken":null}}"#,
            r#"{"claudeAiOauth":{"accessToken":123}}"#,
            r#"{"renamedByVendor":{"accessToken":"x"}}"#,
            "[]",
        ];
        for j in junk {
            assert!(parse_claude_oauth(j).is_none(), "claude 该退化成 None: {j}");
            assert!(parse_codex_auth(j).is_none(), "codex 该退化成 None: {j}");
        }
    }

    #[test]
    fn an_empty_token_is_treated_as_absent() {
        // 空串是「有这个字段但没值」，当成没有——否则会拿空 token 去打端点，
        // 换来一个 401 和一句看不懂的错。
        assert!(parse_claude_oauth(r#"{"claudeAiOauth":{"accessToken":""}}"#).is_none());
    }

    #[test]
    fn debug_never_prints_the_secret() {
        // 凭据会跟着各种错误路径走，Debug 一旦打明文就会漏进 stderr 和日志。
        let k = Credential::Key("sk-super-secret".into());
        let b = Credential::Bearer("at-super-secret".into());
        assert!(!format!("{k:?}").contains("super-secret"), "Key 的 Debug 漏了明文");
        assert!(!format!("{b:?}").contains("super-secret"), "Bearer 的 Debug 漏了明文");
        assert_eq!(format!("{:?}", Credential::Inherit), "Inherit");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib llm::creds`
Expected: 编译失败，`unresolved module 'llm'`

- [ ] **Step 3: 写实现**

`src/llm/mod.rs`（本任务只要这两行）：

```rust
//! dct 自己用的 LLM 连接层。
pub mod creds;
```

`src/lib.rs` 在 `pub mod journal;` 后加 `pub mod llm;`（保持字母序）。

`src/llm/creds.rs`：

```rust
//! 凭据来源。
//!
//! **这是整个连接层里唯一碰用户凭据的地方，纪律因此最严：**
//!
//! 1. 所有解析函数返回 `Option`，**不返回 `Result`**。厂商的磁盘格式没有
//!    文档、说变就变；格式一变，用户该退化成「填个 key」，而不是看到
//!    dct 报一个他无能为力的错。
//! 2. **纯解析和真实读取分开。** 测试只喂手写样本，永远不碰真实
//!    Keychain / `~/.codex/auth.json`。
//! 3. `Debug` **不许打明文**——凭据会跟着错误路径走进 stderr 和日志。

use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    /// 什么都不用给：`CliBackend` 直接拉起，CLI 自己认证。
    /// **用户要的 SSO 在这里是零代码的。**
    Inherit,
    Key(String),
    Bearer(String),
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Credential::Inherit => f.write_str("Inherit"),
            Credential::Key(_) => f.write_str("Key(<redacted>)"),
            Credential::Bearer(_) => f.write_str("Bearer(<redacted>)"),
        }
    }
}

/// 非空才算数。空串是「有这个字段但没值」——拿空 token 去打端点只会换来
/// 一个 401 和一句用户看不懂的错。
fn non_empty(s: Option<&str>) -> Option<String> {
    match s {
        Some(v) if !v.is_empty() => Some(v.to_string()),
        _ => None,
    }
}

/// Claude Code 的 OAuth（macOS Keychain 内容 / Linux `.credentials.json`，同一形状）。
pub fn parse_claude_oauth(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    non_empty(v.get("claudeAiOauth")?.get("accessToken")?.as_str())
}

/// Codex 的 `~/.codex/auth.json`。
///
/// `OPENAI_API_KEY` 非 null 就是填 key 登录的，否则用 `tokens.access_token`。
pub fn parse_codex_auth(json: &str) -> Option<Credential> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if let Some(k) = non_empty(v.get("OPENAI_API_KEY").and_then(|k| k.as_str())) {
        return Some(Credential::Key(k));
    }
    let t = non_empty(v.get("tokens")?.get("access_token")?.as_str())?;
    Some(Credential::Bearer(t))
}

/// 真实读取。**没有单元测试覆盖**（会碰真实凭据），只在 `dct llm check`
/// 这条手动路径上跑。
pub fn read_claude_oauth() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        return parse_claude_oauth(String::from_utf8(out.stdout).ok()?.trim());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let home = std::env::var("HOME").ok()?;
        let p = std::path::Path::new(&home).join(".claude").join(".credentials.json");
        parse_claude_oauth(&std::fs::read_to_string(p).ok()?)
    }
}

pub fn read_codex_auth() -> Option<Credential> {
    let home = std::env::var("HOME").ok()?;
    let p = std::path::Path::new(&home).join(".codex").join("auth.json");
    parse_codex_auth(&std::fs::read_to_string(p).ok()?)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib llm::creds`
Expected: 6 passed

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && git add src/llm src/lib.rs
git commit -m "feat(llm): source credentials from keys, CLI OAuth, or inheritance

Parsing returns Option, never Result: vendor credential formats are
undocumented and change without notice, so a format change must degrade the
user to 'enter a key' rather than surface an error they cannot act on.

Pure parsing is split from real reads so tests never touch the real Keychain,
and Credential's Debug is hand-written to redact — credentials travel along
error paths into stderr and logs."
```

---

### Task 4: 后端 trait、提示词、硬超时

**Files:**
- Modify: `src/llm/mod.rs`

**Interfaces:**
- Consumes: 无
- Produces:
  - `llm::Prompt { pub system: String, pub user: String, pub max_tokens: u32 }`
  - `llm::LlmError { Unavailable, Timeout, Malformed }`（`Debug, Clone, Copy, PartialEq, Eq`）
  - `llm::Backend: Send + Sync`，方法 `fn complete(&self, p: &Prompt) -> Result<String, LlmError>`
  - `llm::complete_with_timeout(b: Arc<dyn Backend>, p: Prompt, d: Duration) -> Result<String, LlmError>`

**说明：** `complete_with_timeout` 是「绝不进 TUI 重绘循环」这条硬约束的落地：调用方拿到的最坏情况是 `d` 之后的一个 `Timeout`，不是无限等待。用 `std::sync::mpsc` 的 `recv_timeout`，不引入 async。

超时后**工作线程会继续跑到自己结束**（Rust 杀不掉线程）——这是可以接受的：它只是在等一个 HTTP 响应或一个子进程，结束后往一个没人听的 channel 里送一次就退了。**关键是调用方已经不等它了。**

- [ ] **Step 1: 写失败的测试**

`src/llm/mod.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    struct Fixed(Result<String, LlmError>);
    impl Backend for Fixed {
        fn complete(&self, _p: &Prompt) -> Result<String, LlmError> {
            self.0.clone()
        }
    }

    struct Slow(Duration);
    impl Backend for Slow {
        fn complete(&self, _p: &Prompt) -> Result<String, LlmError> {
            std::thread::sleep(self.0);
            Ok("too late".into())
        }
    }

    fn p() -> Prompt {
        Prompt { system: "s".into(), user: "u".into(), max_tokens: 64 }
    }

    #[test]
    fn a_fast_backend_returns_its_answer() {
        let b: Arc<dyn Backend> = Arc::new(Fixed(Ok("hello".into())));
        assert_eq!(complete_with_timeout(b, p(), Duration::from_secs(5)), Ok("hello".into()));
    }

    #[test]
    fn a_backend_error_passes_through() {
        let b: Arc<dyn Backend> = Arc::new(Fixed(Err(LlmError::Unavailable)));
        assert_eq!(
            complete_with_timeout(b, p(), Duration::from_secs(5)),
            Err(LlmError::Unavailable)
        );
    }

    /// 这条是「绝不冻住界面」的回归点。一个冻住的 dct 和一个死掉的 agent
    /// 在屏幕上长得一模一样——这是用户最恨的失败模式。
    #[test]
    fn a_slow_backend_gives_up_instead_of_blocking_forever() {
        let b: Arc<dyn Backend> = Arc::new(Slow(Duration::from_secs(30)));
        let started = std::time::Instant::now();
        let r = complete_with_timeout(b, p(), Duration::from_millis(150));
        assert_eq!(r, Err(LlmError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(2), "调用方没有及时放手");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib llm::tests`
Expected: 编译失败，`cannot find type 'Prompt'`

- [ ] **Step 3: 写实现**

`src/llm/mod.rs` 改成：

```rust
//! dct 自己用的 LLM 连接层。
//!
//! **每一处用法都必须有不依赖 LLM 的退路。** 这一层的错误都是「算了，
//! 当没有这个功能」，不是「dct 坏了」。

pub mod creds;

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Prompt {
    pub system: String,
    pub user: String,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmError {
    /// 连不上、没配、凭据拿不到。功能安静下线。
    Unavailable,
    Timeout,
    /// 回来了但读不懂。**当作「没把握」处理**，绝不猜。
    Malformed,
}

pub trait Backend: Send + Sync {
    fn complete(&self, p: &Prompt) -> Result<String, LlmError>;
}

/// 在工作线程上跑，最多等 `d`。
///
/// 超时后那个线程会继续跑到自己结束（Rust 杀不掉线程）——可以接受：
/// 它只是在等一个 HTTP 响应或一个子进程，完事往没人听的 channel 送一次就退。
/// **关键是调用方已经不等它了**，而这正是「绝不冻住界面」要保的东西。
pub fn complete_with_timeout(
    b: Arc<dyn Backend>,
    p: Prompt,
    d: Duration,
) -> Result<String, LlmError> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(b.complete(&p));
    });
    match rx.recv_timeout(d) {
        Ok(r) => r,
        Err(_) => Err(LlmError::Timeout),
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib llm::`
Expected: 全绿（Task 3 的 6 个 + 本任务 3 个）

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && git add src/llm/mod.rs
git commit -m "feat(llm): add the Backend trait and a hard call timeout

complete_with_timeout is how the 'never block the TUI' constraint is
enforced: the worst a caller can experience is a Timeout after the budget.
A frozen dct and a dead agent look identical on screen, which makes blocking
the redraw loop the most expensive failure this tool has."
```

---

### Task 5: `CliBackend`——把 agent CLI 当模型用

**Files:**
- Create: `src/llm/cli.rs`
- Modify: `src/llm/mod.rs`（加 `pub mod cli;`）

**Interfaces:**
- Consumes: `llm::{Backend, Prompt, LlmError}`
- Produces:
  - `llm::cli::Runner`：类型别名 `dyn Fn(&[String], &str) -> Result<String, String> + Send + Sync`
  - `llm::cli::CliBackend { command: Vec<String>, env: BTreeMap<String, String>, runner: Arc<Runner> }`
  - `CliBackend::new(command: Vec<String>, env: BTreeMap<String, String>) -> CliBackend`（用真实子进程 runner）
  - `CliBackend::with_runner(command: Vec<String>, runner: Arc<Runner>) -> CliBackend`（测试用）

**说明：** 用户要的 SSO 在这条路上是**零代码**的——`claude -p` 自己就会去读它自己的登录态，dct 一个 token 都不用碰。

runner 注入是为了能不拉真子进程地测：真实 runner 单独一个函数，**不被单元测试覆盖**，在 Task 9 的实测里验。

提示词的送法：`system` 与 `user` 拼成一段文本，**从 stdin 送**，不作为命令行参数——参数会进 `ps` 输出、可能超长度上限，而且要处理引号转义。

- [ ] **Step 1: 写失败的测试**

`src/llm/cli.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn p() -> Prompt {
        Prompt { system: "你是个助手".into(), user: "出了什么事？".into(), max_tokens: 64 }
    }

    #[test]
    fn the_stdout_of_the_cli_is_the_answer() {
        let b = CliBackend::with_runner(
            vec!["claude".into(), "-p".into()],
            Arc::new(|_cmd: &[String], _input: &str| Ok("  磁盘满了。\n".to_string())),
        );
        // 首尾空白要修掉：CLI 普遍带一个尾随换行，原样传下去会污染界面。
        assert_eq!(b.complete(&p()), Ok("磁盘满了。".to_string()));
    }

    #[test]
    fn the_prompt_reaches_the_cli_on_stdin() {
        let seen = Arc::new(Mutex::new((Vec::new(), String::new())));
        let sink = seen.clone();
        let b = CliBackend::with_runner(
            vec!["claude".into(), "-p".into()],
            Arc::new(move |cmd: &[String], input: &str| {
                *sink.lock().unwrap() = (cmd.to_vec(), input.to_string());
                Ok("ok".into())
            }),
        );
        b.complete(&p()).unwrap();
        let (cmd, input) = seen.lock().unwrap().clone();
        assert_eq!(cmd, vec!["claude".to_string(), "-p".to_string()]);
        assert!(input.contains("你是个助手"), "system 没送到");
        assert!(input.contains("出了什么事？"), "user 没送到");
    }

    #[test]
    fn a_failing_cli_is_unavailable_not_a_crash() {
        let b = CliBackend::with_runner(
            vec!["nope".into()],
            Arc::new(|_: &[String], _: &str| Err("command not found".into())),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Unavailable));
    }

    #[test]
    fn empty_output_is_malformed_not_an_empty_answer() {
        // 空回答比没回答更糟：界面会显示一片空白，用户以为功能坏了。
        // 当成 Malformed，让调用方走退路。
        let b = CliBackend::with_runner(
            vec!["claude".into()],
            Arc::new(|_: &[String], _: &str| Ok("   \n  ".into())),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Malformed));
    }
}
```

> **Fix round 1 补记：** 上面这 4 条是最初的 TDD 记录，原样保留。审查发现
> `run_real` 有双向管道死锁（写 stdin 早于读 stdout/stderr，提示词一超过
> 管道缓冲区就互相卡死），修完之后在 `tests` 模块末尾（`}` 之前）多加了
> 第 5 条 `run_real_does_not_deadlock_when_prompt_exceeds_the_pipe_buffer`——
> 用 `cat` 拉一个真子进程，喂进去几百 KB 数据验证不卡死，用
> `mpsc::recv_timeout` 兜底，回归了就报「卡死」而不是把测试进程挂住。
> 见下面 Step 3 修正后的 `run_real`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib llm::cli`
Expected: 编译失败，`unresolved module 'cli'`

- [ ] **Step 3: 写实现**

`src/llm/cli.rs`：

```rust
//! 把一个 agent CLI 的无界面模式当成模型来用。
//!
//! **用户要的 SSO 在这条路上是零代码的**：`claude -p` 自己会去读它自己的
//! 登录态，dct 一个 token 都不碰，也就没有任何厂商格式可以变坏。

use super::{Backend, LlmError, Prompt};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

/// 命令 + stdin → stdout。注入是为了能不拉真子进程地测。
pub type Runner = dyn Fn(&[String], &str) -> Result<String, String> + Send + Sync;

pub struct CliBackend {
    command: Vec<String>,
    runner: Arc<Runner>,
}

impl CliBackend {
    /// `env` 不进结构体——它只被真实 runner 用到，闭包捕获走就够了。
    /// 存一份在字段里没有任何读者，是纯粹的死重量。
    pub fn new(command: Vec<String>, env: BTreeMap<String, String>) -> CliBackend {
        CliBackend {
            command,
            runner: Arc::new(move |cmd, input| run_real(cmd, input, &env)),
        }
    }

    pub fn with_runner(command: Vec<String>, runner: Arc<Runner>) -> CliBackend {
        CliBackend { command, runner }
    }
}

impl Backend for CliBackend {
    fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
        let input = format!("{}\n\n{}", p.system, p.user);
        let out = (self.runner)(&self.command, &input).map_err(|e| {
            eprintln!("LLM CLI 调用失败：{e}");
            LlmError::Unavailable
        })?;
        let trimmed = out.trim();
        if trimmed.is_empty() {
            // 空回答比没回答更糟：界面会显示一片空白，用户以为功能坏了。
            return Err(LlmError::Malformed);
        }
        Ok(trimmed.to_string())
    }
}

/// 真实子进程。**跟具体 agent CLI（`claude` 之类）的集成没有单元测试覆盖**，
/// 那部分在实测那一步验；但收发管道本身的正确性（不跟真 CLI 绑定）有一条
/// 用 `cat` 做的回归测试，见下面 `run_real_does_not_deadlock_...`。
///
/// 提示词走 stdin 不走参数：参数会进 `ps` 输出、可能超长度上限，
/// 还要处理引号转义。
fn run_real(cmd: &[String], input: &str, env: &BTreeMap<String, String>) -> Result<String, String> {
    let (head, rest) = cmd.split_first().ok_or_else(|| "空命令".to_string())?;
    let mut child = std::process::Command::new(head)
        .args(rest)
        .envs(env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{head} 起不来：{e}"))?;

    // 写 stdin 得放到单独的线程上，跟 wait_with_output 读 stdout/stderr
    // 并发进行：如果提示词超过管道缓冲区（macOS 16KB / Linux 64KB），而
    // 子进程这时候正往 stdout 写东西没人读，父进程堵在 write_all、子进程
    // 堵在写 stdout，就是经典的双向管道死锁。两条管道得同时有人伺候。
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "拿不到 stdin".to_string())?;
    let input = input.to_string();
    let writer = std::thread::spawn(move || -> Result<(), String> {
        let result = stdin.write_all(input.as_bytes());
        // `stdin` 在这里出作用域被 drop，子进程收到 EOF——这个行为必须保留：
        // 父进程不主动关，子进程读 stdin 会永远等下去。
        match result {
            Ok(()) => Ok(()),
            // 子进程提前退出（参数错、没登录）会自己关掉 stdin，父进程这时候
            // 写入会拿到 BrokenPipe——这不是真的错误，退出码和 stderr 才是。
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            Err(e) => Err(format!("写 stdin 失败：{e}")),
        }
    });

    let out = child
        .wait_with_output()
        .map_err(|e| format!("等待失败：{e}"))?;
    // 线程 panic 不能 unwrap 带崩——转成错误字符串正常传回去。
    let write_result = writer
        .join()
        .unwrap_or_else(|_| Err("写 stdin 的线程 panic 了".to_string()));

    if !out.status.success() {
        return Err(format!(
            "{head} 退出码非零：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    write_result?;
    String::from_utf8(out.stdout).map_err(|e| format!("输出不是 UTF-8：{e}"))
}
```

`src/llm/mod.rs` 的 `pub mod creds;` 下面加 `pub mod cli;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib llm::cli`
Expected: 5 passed（含一条真拉子进程验证不死锁的 `run_real` 回归测试）

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && git add src/llm/cli.rs src/llm/mod.rs
git commit -m "feat(llm): run an agent CLI headlessly as a model backend

This is the path where the user's SSO works with zero code: claude -p reads
its own login, so dct never handles a token and no vendor format can rot.

The prompt goes over stdin rather than argv — arguments show up in ps output,
can exceed length limits, and would need quote escaping."
```

---

### Task 6: `HttpBackend`——直连 OpenAI / Anthropic 兼容端点

**Files:**
- Create: `src/llm/http.rs`
- Modify: `src/llm/mod.rs`（加 `pub mod http;`）

**Interfaces:**
- Consumes: `llm::{Backend, Prompt, LlmError}`、`llm::creds::Credential`、`profile::Wire`
- Produces:
  - `llm::http::Sender`：类型别名 `dyn Fn(&str, &Credential, &serde_json::Value) -> Result<(u16, String), String> + Send + Sync`
  - `llm::http::body_for(wire: Wire, model: &str, p: &Prompt) -> serde_json::Value`
  - `llm::http::extract_text(wire: Wire, body: &str) -> Option<String>`
  - `llm::http::HttpBackend { url, wire, model, cred, sender }`
  - `HttpBackend::new(url: String, wire: Wire, model: String, cred: Credential) -> HttpBackend`
  - `HttpBackend::with_sender(url: String, wire: Wire, model: String, cred: Credential, sender: Arc<Sender>) -> HttpBackend`

**说明：** 传输层注入，与 `verify.rs` 的 `verify_with` 是同一套路——401 / 超时 / 输出畸形全都能不打网络地测。超时沿用 `verify.rs` 的做法：`.timeout()` 和 `.timeout_connect()` **都要设**，只设前者会退回 ureq 默认的 30 秒（那个坑 `verify.rs` 已经踩过一次，注释在那儿）。

- [ ] **Step 1: 写失败的测试**

`src/llm/http.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> Prompt {
        Prompt { system: "s".into(), user: "u".into(), max_tokens: 128 }
    }

    #[test]
    fn openai_body_puts_the_system_prompt_in_messages() {
        let b = body_for(Wire::Openai, "gpt-x", &p());
        assert_eq!(b["model"], "gpt-x");
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][0]["content"], "s");
        assert_eq!(b["messages"][1]["role"], "user");
        assert_eq!(b["messages"][1]["content"], "u");
    }

    #[test]
    fn anthropic_body_puts_the_system_prompt_at_top_level() {
        // Anthropic 的 system 是顶层字段，不是一条 message。放错位置
        // 端点不会报错，只会安静地忽略它——所以必须有测试盯着。
        let b = body_for(Wire::Anthropic, "claude-x", &p());
        assert_eq!(b["model"], "claude-x");
        assert_eq!(b["system"], "s");
        assert_eq!(b["max_tokens"], 128);
        assert_eq!(b["messages"][0]["role"], "user");
        assert!(b["messages"][0].get("system").is_none());
    }

    #[test]
    fn extracts_text_from_each_wire_format() {
        assert_eq!(
            extract_text(Wire::Openai, r#"{"choices":[{"message":{"content":"答案"}}]}"#).as_deref(),
            Some("答案")
        );
        assert_eq!(
            extract_text(Wire::Anthropic, r#"{"content":[{"type":"text","text":"答案"}]}"#).as_deref(),
            Some("答案")
        );
    }

    #[test]
    fn unreadable_responses_yield_none_for_every_wire() {
        for w in [Wire::Openai, Wire::Anthropic] {
            for body in ["", "not json", "{}", r#"{"choices":[]}"#, r#"{"content":[]}"#] {
                assert!(extract_text(w, body).is_none(), "{w:?} 该读不出来: {body}");
            }
        }
    }

    #[test]
    fn a_401_is_unavailable_not_a_panic() {
        let b = HttpBackend::with_sender(
            "https://x/v1".into(), Wire::Openai, "m".into(), Credential::Key("k".into()),
            Arc::new(|_, _, _| Ok((401, "unauthorized".into()))),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Unavailable));
    }

    #[test]
    fn a_network_failure_is_unavailable() {
        let b = HttpBackend::with_sender(
            "https://x/v1".into(), Wire::Openai, "m".into(), Credential::Key("k".into()),
            Arc::new(|_, _, _| Err("connection refused".into())),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Unavailable));
    }

    #[test]
    fn a_200_with_unreadable_body_is_malformed() {
        // 读不懂 = 没把握。绝不猜一个答案出来。
        let b = HttpBackend::with_sender(
            "https://x/v1".into(), Wire::Openai, "m".into(), Credential::Key("k".into()),
            Arc::new(|_, _, _| Ok((200, "{\"unexpected\":1}".into()))),
        );
        assert_eq!(b.complete(&p()), Err(LlmError::Malformed));
    }

    #[test]
    fn a_good_response_comes_back_trimmed() {
        let b = HttpBackend::with_sender(
            "https://x/v1".into(), Wire::Anthropic, "m".into(), Credential::Bearer("t".into()),
            Arc::new(|_, _, _| Ok((200, r#"{"content":[{"type":"text","text":" 好了 \n"}]}"#.into()))),
        );
        assert_eq!(b.complete(&p()), Ok("好了".to_string()));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib llm::http`
Expected: 编译失败，`unresolved module 'http'`

- [ ] **Step 3: 写实现**

`src/llm/http.rs`：

```rust
//! 直连 OpenAI / Anthropic 兼容端点。
//!
//! 传输层注入，与 `verify.rs` 的 `verify_with` 是同一套路：401 / 网络故障 /
//! 输出畸形全都能不打网络地测。

use super::creds::Credential;
use super::{Backend, LlmError, Prompt};
use crate::profile::Wire;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// url + 凭据 + 请求体 → (状态码, 响应体)
pub type Sender =
    dyn Fn(&str, &Credential, &serde_json::Value) -> Result<(u16, String), String> + Send + Sync;

/// 与 `verify::PROBE_TIMEOUT` 同一个理由：**`.timeout()` 和
/// `.timeout_connect()` 都要设**，只设前者会退回 ureq 默认的 30 秒。
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

pub fn body_for(wire: Wire, model: &str, p: &Prompt) -> serde_json::Value {
    match wire {
        Wire::Openai => json!({
            "model": model,
            "max_tokens": p.max_tokens,
            "messages": [
                {"role": "system", "content": p.system},
                {"role": "user", "content": p.user},
            ],
        }),
        // Anthropic 的 system 是**顶层字段**，不是一条 message。放错位置
        // 端点不报错，只会安静忽略——所以有一条测试专门盯着。
        Wire::Anthropic => json!({
            "model": model,
            "max_tokens": p.max_tokens,
            "system": p.system,
            "messages": [{"role": "user", "content": p.user}],
        }),
    }
}

pub fn extract_text(wire: Wire, body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let s = match wire {
        Wire::Openai => v.get("choices")?.get(0)?.get("message")?.get("content")?.as_str()?,
        Wire::Anthropic => v.get("content")?.get(0)?.get("text")?.as_str()?,
    };
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    Some(t.to_string())
}

pub struct HttpBackend {
    url: String,
    wire: Wire,
    model: String,
    cred: Credential,
    sender: Arc<Sender>,
}

impl HttpBackend {
    pub fn new(url: String, wire: Wire, model: String, cred: Credential) -> HttpBackend {
        HttpBackend { url, wire, model, cred, sender: Arc::new(send_real) }
    }

    pub fn with_sender(
        url: String,
        wire: Wire,
        model: String,
        cred: Credential,
        sender: Arc<Sender>,
    ) -> HttpBackend {
        HttpBackend { url, wire, model, cred, sender }
    }
}

impl Backend for HttpBackend {
    fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
        let body = body_for(self.wire, &self.model, p);
        let (status, text) = (self.sender)(&self.url, &self.cred, &body).map_err(|e| {
            eprintln!("LLM HTTP 调用失败：{e}");
            LlmError::Unavailable
        })?;
        if !(200..300).contains(&status) {
            eprintln!("LLM HTTP 返回 {status}");
            return Err(LlmError::Unavailable);
        }
        // 读不懂 = 没把握。绝不猜一个答案出来。
        extract_text(self.wire, &text).ok_or(LlmError::Malformed)
    }
}

/// 真实传输。**没有单元测试覆盖**，在实测那一步验。
fn send_real(
    url: &str,
    cred: &Credential,
    body: &serde_json::Value,
) -> Result<(u16, String), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .timeout_connect(HTTP_TIMEOUT)
        .build();
    let mut req = agent.post(url).set("content-type", "application/json");
    match cred {
        Credential::Key(k) => {
            req = req.set("x-api-key", k).set("authorization", &format!("Bearer {k}"));
        }
        Credential::Bearer(t) => {
            req = req.set("authorization", &format!("Bearer {t}"));
        }
        // 直连没有凭据可继承。resolve 那一层不会走到这里，兜底也不猜。
        Credential::Inherit => return Err("HTTP 直连需要凭据".into()),
    }
    req = req.set("anthropic-version", "2023-06-01");
    match req.send_json(body.clone()) {
        Ok(r) => {
            let s = r.status();
            Ok((s, r.into_string().unwrap_or_default()))
        }
        // ureq 把 4xx/5xx 也当 Err，得挑出来——它们是有效状态码，不是网络故障
        Err(ureq::Error::Status(code, r)) => Ok((code, r.into_string().unwrap_or_default())),
        Err(e) => Err(format!("{e}")),
    }
}
```

`src/llm/mod.rs` 加 `pub mod http;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib llm::http`
Expected: 8 passed

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && git add src/llm/http.rs src/llm/mod.rs
git commit -m "feat(llm): add an HTTP backend for OpenAI- and Anthropic-shaped endpoints

Transport is injected the way verify.rs already does it, so 401s, network
failures, and unreadable bodies are all covered without touching the network.

Both .timeout() and .timeout_connect() are set: setting only the former falls
back to ureq's 30s connect default, a trap verify.rs already documented."
```

---

### Task 7: 组装——从配置 + profile + 密钥选出一个后端，加 `dct llm check`

**Files:**
- Create: `src/llm/resolve.rs`
- Modify: `src/llm/mod.rs`（加 `pub mod resolve;`）
- Modify: `src/cli.rs`（加 `llm check` 子命令）

**Interfaces:**
- Consumes: `config::{Config, Transport}`、`profile::Profile`、`secrets::SecretStore`、`llm::{Backend, cli::CliBackend, http::HttpBackend, creds::Credential}`
- Produces:
  - `llm::resolve::ResolveError { NoSuchProvider(String), NoHeadlessCommand(String), NoApiEndpoint(String), NoCredential(String) }`
  - `llm::resolve::describe(e: &ResolveError) -> String`（**中文，一句自足的话，说得出下一步**）
  - `llm::resolve::resolve(cfg: &Config, lookup: &dyn Fn(&str) -> Option<Profile>, secrets: &SecretStore, oauth: &dyn Fn(&str) -> Option<Credential>) -> Result<Arc<dyn Backend>, ResolveError>`

**说明：** 凭据来源的**顺序**在这里定死，这是设计里那条「落到下一个来源」的落地：

1. `Transport::Cli` → `Credential::Inherit`，**不查任何凭据**（CLI 自己管）
2. `Transport::Http` → 先查 `SecretStore`（用户填的 key）→ 再查该 profile 的 OAuth（`oauth` 闭包）→ 都没有则 `NoCredential`

先 key 后 OAuth 的理由：**用户显式填过的东西优先于我们从别人家里翻出来的东西。**

`oauth` 闭包注入是为了测试不碰真实凭据；真实实现在 `cli.rs` 的 `llm check` 里用 `creds::read_claude_oauth` / `read_codex_auth` 拼出来。

- [ ] **Step 1: 写失败的测试**

`src/llm/resolve.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, LlmConfig, Transport};

    fn cfg(provider: &str, transport: Transport) -> Config {
        Config {
            llm: LlmConfig {
                provider: provider.into(),
                model: Some("m".into()),
                base_url: None,
                transport,
            },
        }
    }

    fn builtin(name: &str) -> Option<Profile> {
        Profile::builtin(name)
    }

    fn no_oauth(_: &str) -> Option<Credential> {
        None
    }

    fn empty_secrets() -> SecretStore {
        SecretStore::load(std::path::Path::new("/nonexistent/secrets.toml"))
    }

    #[test]
    fn the_cli_transport_needs_no_credential_at_all() {
        // SSO 在这条路上是零代码的：密钥空的、OAuth 也翻不到，照样成。
        let r = resolve(&cfg("claude", Transport::Cli), &builtin, &empty_secrets(), &no_oauth);
        assert!(r.is_ok(), "CLI 这条路不该需要凭据");
    }

    #[test]
    fn a_profile_without_a_headless_command_is_refused_by_name() {
        // opencode 没验过无界面模式，不许假装能用。
        let e = resolve(&cfg("opencode", Transport::Cli), &builtin, &empty_secrets(), &no_oauth)
            .unwrap_err();
        assert_eq!(e, ResolveError::NoHeadlessCommand("opencode".into()));
    }

    #[test]
    fn an_unknown_provider_is_named_in_the_error() {
        let e = resolve(&cfg("nope", Transport::Cli), &builtin, &empty_secrets(), &no_oauth)
            .unwrap_err();
        assert_eq!(e, ResolveError::NoSuchProvider("nope".into()));
    }

    #[test]
    fn http_without_any_credential_is_refused() {
        let e = resolve(&cfg("kimi", Transport::Http), &builtin, &empty_secrets(), &no_oauth)
            .unwrap_err();
        assert_eq!(e, ResolveError::NoCredential("kimi".into()));
    }

    #[test]
    fn http_uses_an_oauth_token_when_there_is_no_key() {
        let with_oauth = |_: &str| Some(Credential::Bearer("t".into()));
        let r = resolve(&cfg("kimi", Transport::Http), &builtin, &empty_secrets(), &with_oauth);
        assert!(r.is_ok(), "翻得到 OAuth 就该能用");
    }

    #[test]
    fn http_needs_an_api_block() {
        // claude 没有 [api]（它走官方端点，靠 CLI 自己登录），直连没地方去。
        let e = resolve(&cfg("claude", Transport::Http), &builtin, &empty_secrets(),
                        &|_: &str| Some(Credential::Bearer("t".into())))
            .unwrap_err();
        assert_eq!(e, ResolveError::NoApiEndpoint("claude".into()));
    }

    #[test]
    fn every_error_explains_itself_in_a_self_contained_sentence() {
        // 用户是零编程经验的人。每一句都得说得出下一步，且不带栈追踪腔。
        let all = [
            ResolveError::NoSuchProvider("x".into()),
            ResolveError::NoHeadlessCommand("x".into()),
            ResolveError::NoApiEndpoint("x".into()),
            ResolveError::NoCredential("x".into()),
        ];
        for e in all {
            let s = describe(&e);
            assert!(s.contains('x'), "要点名是哪个 provider: {s}");
            assert!(s.chars().count() > 8, "太短，说不清楚: {s}");
            assert!(!s.contains("Error"), "别把类型名漏给用户: {s}");
        }
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib llm::resolve`
Expected: 编译失败，`unresolved module 'resolve'`

- [ ] **Step 3: 写实现**

`src/llm/resolve.rs`：

```rust
//! 从配置 + profile + 密钥里组装出一个后端。
//!
//! **凭据顺序在这里定死**：先用户显式填的 key，再从别人家里翻出来的 OAuth。
//! 理由是用户填过的东西优先于我们替他猜的东西。

use super::cli::CliBackend;
use super::creds::Credential;
use super::http::HttpBackend;
use super::Backend;
use crate::config::{Config, Transport};
use crate::profile::Profile;
use crate::secrets::SecretStore;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    NoSuchProvider(String),
    NoHeadlessCommand(String),
    NoApiEndpoint(String),
    NoCredential(String),
}

/// 用户看得懂、且说得出下一步的一句话。
pub fn describe(e: &ResolveError) -> String {
    match e {
        ResolveError::NoSuchProvider(n) => {
            format!("找不到叫「{n}」的 agent。请在配置里把 provider 改成一个已有的名字。")
        }
        ResolveError::NoHeadlessCommand(n) => {
            format!("「{n}」还不支持在后台单独回答问题。请换一个 provider，比如 claude。")
        }
        ResolveError::NoApiEndpoint(n) => {
            format!("「{n}」没有可直连的服务地址。把配置里的 transport 改回 cli 即可。")
        }
        ResolveError::NoCredential(n) => {
            format!("「{n}」还没有密钥。在主界面按 c 填一个，或把 transport 改回 cli。")
        }
    }
}

pub fn resolve(
    cfg: &Config,
    lookup: &dyn Fn(&str) -> Option<Profile>,
    secrets: &SecretStore,
    oauth: &dyn Fn(&str) -> Option<Credential>,
) -> Result<Arc<dyn Backend>, ResolveError> {
    let name = cfg.llm.provider.as_str();
    let p = lookup(name).ok_or_else(|| ResolveError::NoSuchProvider(name.to_string()))?;

    match cfg.llm.transport {
        Transport::Cli => {
            let h = p
                .headless
                .as_ref()
                .ok_or_else(|| ResolveError::NoHeadlessCommand(name.to_string()))?;
            // Inherit：一个凭据都不查，CLI 自己管登录。
            Ok(Arc::new(CliBackend::new(h.command.clone(), p.env.clone())))
        }
        Transport::Http => {
            let api = p
                .api
                .as_ref()
                .ok_or_else(|| ResolveError::NoApiEndpoint(name.to_string()))?;
            let cred = secrets
                .get(name)
                .map(|k| Credential::Key(k.to_string()))
                .or_else(|| oauth(name))
                .ok_or_else(|| ResolveError::NoCredential(name.to_string()))?;
            let base = cfg.llm.base_url.clone().unwrap_or_else(|| api.base_url.clone());
            let url = format!("{}/v1/messages", base.trim_end_matches('/'));
            let model = cfg.llm.model.clone().unwrap_or_else(|| "claude-3-5-sonnet".to_string());
            Ok(Arc::new(HttpBackend::new(url, api.wire, model, cred)))
        }
    }
}
```

`src/llm/mod.rs` 加 `pub mod resolve;`。

在 `src/cli.rs` 里加一个 `llm check` 子命令，按该文件已有的子命令写法接上；它做三件事并把结果打到 stdout：

```rust
/// `dct llm check`：把配置里那条 LLM 连接真的跑一次。
///
/// 这条命令**就是**「配置写完还要真打端点验过」那条验收标准的载体。
pub fn llm_check() -> i32 {
    let socket = crate::proto::socket_path();
    let cfg = crate::config::Config::load(&crate::config::config_path_for_socket(&socket));
    let secrets = crate::secrets::SecretStore::load(&crate::secrets::secrets_path_for_socket(&socket));
    let profiles_dir = crate::profile::profiles_dir_for_socket(&socket);
    let (custom, _) = crate::profile::all_profiles(&profiles_dir);
    let lookup = |n: &str| {
        custom
            .iter()
            .find(|p| p.name == n)
            .cloned()
            .or_else(|| crate::profile::Profile::builtin(n))
    };
    // 修正（fix round 1 code review，CRITICAL）：这里原来把 kimi/glm/
    // deepseek/qwen-api 也映射到 claude 的 OAuth。那四家的 `[api].base_url`
    // 是它们自己的第三方服务器（api.moonshot.cn / open.bigmodel.cn /
    // api.deepseek.com / dashscope.aliyuncs.com），`send_real` 会把凭据塞进
    // `Authorization: Bearer` 头直接打过去——等于把用户的 Anthropic 登录态
    // 发给了四家跟 Anthropic 毫无关系的第三方。规则钉死：一个 CLI 的
    // OAuth 只能给它自己的端点用，不能给别家。kimi/glm/deepseek/qwen-api
    // 跟用户没有任何 OAuth 关系，只能走用户自己填的 key。
    //
    // 映射拆成一个独立、可注入测试的 `oauth_lookup(name, claude, codex)`
    // 函数（在 `cli.rs` 里），有一条测试钉死这四个名字永远拿不到别家的
    // token，见 task-7-report.md 的 fix round 1 记录。
    let oauth = |n: &str| {
        oauth_lookup(
            n,
            &|| crate::llm::creds::read_claude_oauth().map(crate::llm::creds::Credential::Bearer),
            &crate::llm::creds::read_codex_auth,
        )
    };

    println!("provider: {}", cfg.llm.provider);
    println!("transport: {:?}", cfg.llm.transport);

    let backend = match crate::llm::resolve::resolve(&cfg, &lookup, &secrets, &oauth) {
        Ok(b) => b,
        Err(e) => {
            println!("连不上：{}", crate::llm::resolve::describe(&e));
            return 1;
        }
    };

    let p = crate::llm::Prompt {
        system: "你是一个只回答一个词的助手。".into(),
        user: "回答「好」这一个字，不要别的。".into(),
        max_tokens: 16,
    };
    match crate::llm::complete_with_timeout(backend, p, std::time::Duration::from_secs(60)) {
        Ok(answer) => {
            println!("通了。模型回答：{answer}");
            0
        }
        Err(e) => {
            println!("没通：{e:?}");
            1
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib llm::resolve && cargo build`
Expected: 7 passed，构建成功

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && cargo build && git add src/llm/resolve.rs src/llm/mod.rs src/cli.rs
git commit -m "feat(llm): resolve a backend from config, profile, and secrets

Credential order is fixed here: an explicitly entered key beats an OAuth
token we found in another program's storage, because what the user typed
should outrank what we guessed on their behalf.

The CLI transport resolves to Inherit and consults no credential store at
all. Adds 'dct llm check', which runs the configured connection for real —
this is what makes the 'verified against a live endpoint' bar checkable."
```

**Fix round 1（code review）：** 除了上面已经改掉的 vendor→Claude-OAuth 映射
（CRITICAL）之外，同一轮还发现并修了三处，全部记在
`.superpowers/sdd/2026-08-06-dct-llm-connection/task-7-report.md`：

- 凭据顺序（key 优先于 OAuth）之前没有测试同时喂 key 和 OAuth，翻转
  `.or_else()` 顺序也能全绿——补了 `an_explicit_key_outranks_an_oauth_token_found_elsewhere`，把这条顺序拆成独立的 `select_credential` 函数以便直接测。
- 为了让 `.unwrap_err()` 编译通过，曾经给 `dyn Backend` 加了一个只为测试
  存在的 `Debug` 实现——已撤掉，改用 `matches!` 断言，不需要 `T: Debug`。
- `describe()` 的四句话原样带着 `provider`/`transport`/`cli` 这些内部
  字段名，测试也弱到 `"x xxxxxxxxx"` 都能过——文案改成不含内部字段名/
  类型名的大白话，测试改成查具体禁词 + 查真实存在的下一步动作。

另外顺手修了两处「便宜」的问题：HTTP 路径按 `Wire` 区分
（`/v1/messages` vs `/v1/chat/completions`，写死前者会让 OpenAI 型端点
404），以及 `cfg.llm.model` 为空时不再瞎猜 `claude-3-5-sonnet`，改成
`ResolveError::NoModel` 让用户自己填。

---

### Task 8: 出错解释——会话失败时说人话

**Files:**
- Modify: `src/session.rs`（`Session` 加字段；`tick()` 里检测进入 `Failed` 的那一刻）
- Modify: `src/proto.rs`（`Request::Explanation { id }`、`Response::Explanation(Option<String>)`）
- Modify: `src/daemon.rs`（接线；启动时 resolve 一次后端，仅当 `cfg.llm` 是 `Some`）
- Modify: `src/ui/mod.rs`（`Failed` 会话上显示解释；`src/ui/app.rs` 加一个每会话缓存字段）
- Modify: `src/i18n.rs`（新词条）
- fix round 1 追加触及：`src/llm/resolve.rs`（`resolve()` 改接 `LlmConfig` 而不是
  `Config`）、`src/cli.rs`（`llm_check` 处理 `cfg.llm` 是 `None` 的情形）——见下面的
  fix round 1 附注。

**Interfaces:**
- Consumes: `llm::{Backend, Prompt, complete_with_timeout}`、`llm::resolve::resolve`
- Produces:
  - `session::Session` 新字段 `explanation_slot: Arc<Mutex<Option<String>>>`
    （**必须是 `Arc<Mutex<_>>`**：解释由后台线程写回，而那个线程拿不到
    `Session` 的锁——`tick()` 正持着它。裸 `Option<String>` 编不过。）
  - `session::explain_prompt(screen: &str) -> Prompt`
  - `SessionManager::set_backend(&self, b: Option<Arc<dyn Backend>>)`
  - `SessionManager::explanation(&self, id: u32) -> Option<String>`

**说明：** 触发点是**状态迁移进 `Failed` 的那一刻**，不是「只要还是 `Failed` 就一直问」——后者会每 200ms 打一次模型。

**退路：** 后端没配 / 调不通 / 超时，`explanation` 保持 `None`，界面显示今天就有的那句失败提示。**功能安静下线，不打扰用户。**

截屏文本要**截尾**再送：整屏可能几千字，只要最后 2000 字符——错误一定在末尾。

> **2026-08-06 fix round 1（code review：1 Critical + 2 Important）追加，实现前必读：**
>
> - **Critical——「没配后端」必须真的意味着「没配」，不能靠 `Config` 默认值假装没配。**
>   `daemon.rs` 启动时装后端这一步，**必须先看 `cfg.llm.is_some()`**（Task 1 的 fix：`Config::llm`
>   已经是 `Option<LlmConfig>`，缺 `[llm]` 就是 `None`）——`None` 时压根不调 `resolve()`、
>   不装后端、不打印任何东西；只有用户确实写了 `[llm]` 却连不上时才值得往 stderr 留一行。
>   这一步值得抽成一个独立函数（比如 `install_llm_backend(socket, profiles_dir, mgr)`），
>   好让「没写 `[llm]` 就不该装后端」这条能在不起真实 socket/listener 的情况下直接单测——
>   断言 `SessionManager` 有没有装上后端，不要靠等一个真实网络调用的结果去反推，那样测试会
>   被「这台机器上到底装没装某个 CLI」这类环境噪音污染。
> - **Important (a)——UI 不能每帧都重发 `Request::Explanation`、每帧都重写 `app.message`。**
>   附加视图 16ms 一轮；`App` 需要一个「这个会话这一次失败已经拿到解释了」的缓存（比如
>   `explained_failure: Option<(u32, String)>`），拿到过就不再问、也不再碰 `app.message`
>   ——不然粘贴失败、Ctrl+C 打断这类别处设的消息，下一帧就被这句话原样盖掉，用户永远看不见。
>   `enter_session`（或等价的「进入这个会话」入口）要清空这份缓存：不清的话，一个「恢复了、
>   又坏了一次」的会话会一直顶着用户上一次看到的旧解释，永远问不出新的。
> - **Important (b)——第二次失败不能显示第一次失败的解释，也不能被第一次的慢答案覆盖。**
>   `request_explanation` 在**起线程之前**（不管有没有配后端）就要清空 `explanation_slot`
>   并给这一轮失败发一个号（`explanation_gen: Arc<AtomicU64>` 之类，自增一次）；后台线程
>   算完之后先比一遍「我领到的号还是不是最新那个」，不是才不写——这样一个卡了很久的旧线程，
>   哪怕在新一轮失败已经有了新答案之后才姗姗来迟，也没法把新答案盖成旧的。
>
> 这三条把原本只列在 Files 里的 `src/session.rs` / `src/daemon.rs` / `src/ui/mod.rs`
> 的实现细节改深了；`src/config.rs`（Task 1）、`src/llm/resolve.rs`、`src/cli.rs`
> 的 `llm_check` 也要跟着 `Config::llm: Option<LlmConfig>` 一起改（`resolve()` 改成只接
> `LlmConfig`，「开没开」的判断留给调用方；`llm_check` 在 `cfg.llm` 是 `None` 时打印一句
> 明确的中文——功能没开、该在配置文件里加什么——然后非零退出，不能走「连不上」那条话术）。

- [ ] **Step 1: 写失败的测试**

`src/session.rs` 的 `mod tests` 里追加：

```rust
#[test]
fn the_explain_prompt_carries_the_tail_of_the_screen() {
    let long = "x".repeat(5000) + "API Error: Connection closed mid-response.";
    let p = explain_prompt(&long);
    assert!(p.user.contains("API Error"), "错误在末尾，必须送到");
    assert!(p.user.chars().count() < 2500, "整屏太长，要截尾");
    assert!(p.system.contains("中文"), "用户默认中文");
}

#[test]
fn the_explain_prompt_asks_for_plain_language() {
    let p = explain_prompt("API Error: Connection closed mid-response.");
    // 目标用户零编程经验：不要栈追踪、不要术语。
    assert!(p.system.contains("不要"), "要明确禁止术语/栈追踪");
    assert!(p.max_tokens <= 200, "一句话就够，别让它写小作文");
}

#[test]
fn with_no_backend_the_explanation_stays_empty_and_nothing_breaks() {
    // 这是「非 LLM 退路」的回归点：没配后端时 dct 表现得和今天一模一样。
    let repo = init_repo();
    let m = SessionManager::new();
    m.register_profile(fake_agent());
    let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();
    m.set_backend(None);
    m.tick();
    assert_eq!(m.explanation(id), None);
}

#[test]
fn entering_failed_asks_the_backend_once_not_every_tick() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Counting(Arc<AtomicUsize>);
    impl crate::llm::Backend for Counting {
        fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok("网络断了，重开一次就行。".into())
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let repo = init_repo();
    let m = SessionManager::new();
    m.register_profile(failing_agent()); // error_pattern 命中的假 agent
    let id = m.create(repo.path(), "failing", empty_secrets(), &[]).unwrap();
    m.set_backend(Some(Arc::new(Counting(calls.clone()))));

    let deadline = Instant::now() + Duration::from_secs(5);
    while m.explanation(id).is_none() && Instant::now() < deadline {
        m.tick();
        sleep(Duration::from_millis(50));
    }
    assert_eq!(m.explanation(id).as_deref(), Some("网络断了，重开一次就行。"));

    // 再 tick 若干轮：还是 Failed，但**不许**再问模型。
    for _ in 0..10 {
        m.tick();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "只在进入 Failed 那一刻问一次");
}
```

在 `mod tests` 里加一个 `failing_agent()` 辅助函数，照现有 `fake_agent()` 的写法，profile 带 `error_pattern = "BOOM"`，命令是一个会打印 `BOOM` 的 shell。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib session::`
Expected: 编译失败，`cannot find function 'explain_prompt'`

- [ ] **Step 3: 写实现**

`src/session.rs`：

1. `Session` 结构加 `explanation_slot: Arc<Mutex<Option<String>>>`（`new` 里初始化为
   `Arc::new(Mutex::new(None))`）。`SessionManager::explanation(id)` 读它并 clone 出来。
2. `SessionManager` 加 `backend: Mutex<Option<Arc<dyn crate::llm::Backend>>>`
3. 加两个方法与一个纯函数：

```rust
/// 让模型把一屏失败翻译成一句人话。
///
/// **只送屏幕末尾**：整屏可能几千字，而错误一定在末尾。整屏送过去既慢又贵，
/// 还容易让模型抓错重点。
pub fn explain_prompt(screen: &str) -> crate::llm::Prompt {
    const TAIL: usize = 2000;
    let tail: String = {
        let chars: Vec<char> = screen.chars().collect();
        let start = chars.len().saturating_sub(TAIL);
        chars[start..].iter().collect()
    };
    crate::llm::Prompt {
        system: "你在帮一个完全不懂编程的人。用中文，一到两句话说清楚刚才那个\
                 命令行工具出了什么事、他现在该做什么。不要出现英文报错原文、\
                 不要栈追踪、不要术语、不要代码。"
            .into(),
        user: format!("这是屏幕上的最后一段内容：\n\n{tail}"),
        max_tokens: 200,
    }
}
```

4. `tick()` 里，在 `s.state = next;` 那一步**之前**记下 `let was = s.state;`，赋值之后加：

```rust
// 只在**进入** Failed 的那一刻问一次。条件写成「原来不是 Failed」而不是
// 「现在是 Failed」——后者会每 200ms 打一次模型，一个失败会话能把额度烧光。
if next == SessionState::Failed && was != SessionState::Failed {
    self.request_explanation(&mut s);
}
```

5. `request_explanation` 把工作丢到后台线程（**绝不在 tick 里同步等模型**），完成后写回 `explanation`：

```rust
/// **绝不在 tick 里同步等模型。** tick 每 200ms 一轮，一次同步调用就能
/// 让整个守护进程卡住，而卡住的 dct 和死掉的 agent 长得一模一样。
fn request_explanation(&self, s: &mut Session) {
    let Some(b) = self.backend.lock().ok().and_then(|g| g.clone()) else {
        return; // 没配后端：功能安静下线，会话照跑
    };
    let p = explain_prompt(&s.pty.screen_text());
    let slot = s.explanation_slot.clone(); // Arc<Mutex<Option<String>>>
    std::thread::spawn(move || {
        if let Ok(text) =
            crate::llm::complete_with_timeout(b, p, std::time::Duration::from_secs(30))
        {
            if let Ok(mut g) = slot.lock() {
                *g = Some(text);
            }
        }
        // 失败就什么都不做——界面显示今天就有的那句失败提示
    });
}
```

（把 `explanation` 实现成 `Arc<Mutex<Option<String>>>` 字段 `explanation_slot`，`explanation(id)` 读它。）

`src/proto.rs` 加 `Request::Explanation { id: u32 }` 与 `Response::Explanation(Option<String>)`（加在各自枚举**末尾**，不动既有变体的顺序）。`src/daemon.rs` 接上这条请求，并在启动时 resolve 一次后端调用 `set_backend`，resolve 失败只往 stderr 写一行、`set_backend(None)`。

`src/ui/mod.rs`：会话是 `Failed` 且拿得到解释时，把那句话显示在既有的失败提示位置；拿不到就维持现状。`src/i18n.rs` 加对应词条。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib && cargo build`
Expected: 全绿

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && cargo build && git diff --check
git add src/session.rs src/proto.rs src/daemon.rs src/ui/mod.rs src/i18n.rs
git commit -m "feat: explain in plain language why a session failed

Fires once on the transition into Failed, not while Failed: the latter would
hit the model every 200ms and burn a quota on a single broken session.

The call runs on a worker thread — tick() must never wait on a model, since
a stalled daemon is indistinguishable from a dead agent on screen. With no
backend configured, or on any failure, the explanation stays empty and dct
behaves exactly as it does today."
```

---

### Task 9: 实测验证

**Files:**
- Modify: `docs/superpowers/specs/2026-08-06-dct-llm-connection-design.md`（更新「实测验证」那张表）

**Interfaces:**
- Consumes: `dct llm check`
- Produces: 一份如实的实测记录

**说明：** 用户的验收标准是「配置写完还要真打端点验过」。**跑不通的一律记成「未验证」，不许当作跑过。**

- [ ] **Step 1: 验 CLI 这条路（走已有 SSO，本机应当能通）**

```bash
mkdir -p ~/.dct
printf '[llm]\nprovider = "claude"\ntransport = "cli"\n' > ~/.dct/config.toml
cargo run -- llm check
```

Expected: `通了。模型回答：好`（或类似的一个词）

- [ ] **Step 2: 验 codex 那条路**

```bash
printf '[llm]\nprovider = "codex"\ntransport = "cli"\n' > ~/.dct/config.toml
cargo run -- llm check
```

Expected: 通，或**如实记录失败原因**

- [ ] **Step 3: 验拒绝路径的措辞**

```bash
printf '[llm]\nprovider = "opencode"\ntransport = "cli"\n' > ~/.dct/config.toml
cargo run -- llm check
printf '[llm]\nprovider = "nope"\ntransport = "cli"\n' > ~/.dct/config.toml
cargo run -- llm check
```

Expected: 两句中文提示，各自说得出下一步；退出码 1

- [ ] **Step 4: 验 HTTP 这条路（需要真 key 或本地端点）**

若用户提供了某家的真实 key：在主界面按 `c` 填进去，然后

```bash
printf '[llm]\nprovider = "kimi"\ntransport = "http"\nmodel = "moonshot-v1-8k"\n' > ~/.dct/config.toml
cargo run -- llm check
```

**若拿不到 key、且 `:8700` / `:11434` 都起不来，这一项如实记成「未验证」，不许猜一个结论。**

- [ ] **Step 5: 把结果写回设计文档并提交**

更新 spec 里「实测验证」那张表的每一行为真实结果，然后：

```bash
git add docs/superpowers/specs/2026-08-06-dct-llm-connection-design.md
git commit -m "docs: record what the LLM connection was actually verified against

Anything that could not be run is recorded as unverified rather than assumed
to work."
```

---

## 自检

**Spec 覆盖：**

| Spec 段落 | 落在哪个任务 |
|---|---|
| 第一层 后端 trait | Task 4 |
| `CliBackend` | Task 5 |
| `HttpBackend` | Task 6 |
| 第二层 凭据（Inherit/Key/Bearer） | Task 3 |
| Keychain / auth.json 提取 + 可失败 | Task 3 |
| 第三层 登记表长在 profile 上 | Task 2 |
| 配置 `[llm]` | Task 1 |
| 凭据来源顺序 | Task 7 |
| 出错解释（第 2 件事） | Task 8 |
| 错误处理表 | Task 4/5/6/8 |
| 测试纪律（不碰真 Keychain） | Task 3 |
| 实测验证表 | Task 9 |

**未覆盖（有意留给 Plan B）：** 渠道（Telegram / macOS 通知）、自答流水线、「重要」分类器、journal 记账。这些在 spec 里属于第 1、3 件事，本计划的 Goal 已声明只做连接层 + 第 2 件事。

**类型一致性核对：** `Credential` / `Prompt` / `LlmError` / `Backend` / `Wire` / `Transport` 在 Task 3–8 中签名一致；`Wire` 定义在 `profile.rs`（Task 2）、被 `llm/http.rs`（Task 6）与 `llm/resolve.rs`（Task 7）引用；`SecretStore::get` 与 `Profile::builtin` 用的是既有签名。
