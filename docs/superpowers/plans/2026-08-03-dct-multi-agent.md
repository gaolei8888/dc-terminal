# dct 多 agent 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 dct 支持 codex / opencode / qwen 三个独立 CLI 和 kimi / glm / deepseek / qwen-api 四个「claude 换 base_url」形态，用户可以自己加 profile，密钥在界面里填，日常开会话压成一个按键。

**Architecture:** profile 文件回答「怎么起这个 agent」（命令、静态环境变量、声明需要哪种密钥），`~/.dct/secrets.toml` 回答「用户的私货是什么」，两者正交。daemon 把两边合起来交给 PTY。可用性判定在 daemon 侧做，因为它查 PATH 和 spawn 用的是同一个环境。

**Tech Stack:** Rust 2021、ratatui 0.28、crossterm 0.28、portable-pty 0.8、vt100 0.15、toml 0.8、serde、regex；新增 ureq（阻塞式 HTTP，只用于密钥验证）。

**设计文档：** `docs/superpowers/specs/2026-08-03-dct-multi-agent-design.md`

## Global Constraints

- **`cargo` 不在 PATH 上**，用 `~/.cargo/bin/cargo`。
- 每个任务结束前跑 `~/.cargo/bin/cargo fmt` 和 `~/.cargo/bin/cargo test`，全绿才提交。
- 界面文案一律中文，面向**非程序员**：不出现 git / CLI 黑话，错误要说人话，不给栈追踪。
- **不用 emoji 当图标。**
- 注释写「为什么」，不写「是什么」。仓库现有注释密度就是标准，照着写。
- 新增的用户可见文案先硬编码中文，跟着 i18n 那一期收进词条表（见设计文档「与 i18n 的关系」）。
- 磁盘路径一律走 `*_for_socket(socket)` 模式（见 `src/projects.rs:24`），测试才能隔离，不去动用户真实的 `~/.dct/`。
- **提交信息用中文，结尾带** `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`。

## 文件结构

| 文件 | 职责 | 动作 |
|---|---|---|
| `src/profile.rs` | Profile 数据结构、TOML 解析、内置清单、磁盘加载、可用性判定 | 大改 |
| `src/secrets.rs` | 密钥仓：0600 落盘、原子替换、读坏时拒写 | 新建 |
| `src/verify.rs` | 密钥验证：可注入传输层的纯逻辑 + ureq 实现 | 新建 |
| `profiles/*.toml` | 九个内置 profile | 新增 7 个 |
| `src/pty.rs` | `spawn` 接受环境变量 | 小改 |
| `src/session.rs` | env 合成、`busy_pattern`、`SessionState::Unknown` | 中改 |
| `src/proto.rs` | `ProfileEntry`、新增请求 | 中改 |
| `src/daemon.rs` | 新请求的处理、密钥仓接线 | 中改 |
| `src/projects.rs` | 存「上次用的 agent」 | 小改 |
| `src/ui.rs` | 选择器改造、填密钥视图、`n`/`N`、设置页 | 大改 |
| `src/lib.rs` | 注册两个新模块 | 小改 |

**为什么密钥和验证各自单独一个文件**：`src/profile.rs` 已经要扛数据结构 + 解析 + 内置清单 + 磁盘加载 + 可用性判定五件事。密钥落盘（文件权限、原子替换）和网络验证是两个完全独立的关注点，各有各的测试面，塞进去只会让这个文件失控。

---

## Task 1: Profile 数据结构扩展

**Files:**
- Modify: `src/profile.rs:1-45`（结构体与解析）
- Test: `src/profile.rs` 的 `mod tests`

**Interfaces:**
- Consumes: 无
- Produces: `Profile { name, command, is_agent, idle_pattern, busy_pattern, env, secret, install, label, note }`、`LocalizedText`、`SecretSpec`、`VerifySpec`、`InstallSpec`；方法 `Profile::from_toml(&str) -> Result<Profile>`（已有）、`LocalizedText::get(&self, lang: Lang) -> Option<&str>`、`Lang`（本期只有 `Zh`）

- [ ] **Step 1: 写失败的测试**

加到 `src/profile.rs` 的 `mod tests` 里：

```rust
#[test]
fn parses_env_and_secret() {
    let p = Profile::from_toml(
        r#"
        name = "kimi"
        command = ["claude"]
        is_agent = true

        [label]
        zh = "Kimi"

        [note]
        zh = "月之暗面"

        [env]
        ANTHROPIC_BASE_URL = "https://example.com/anthropic"

        [secret]
        env = "ANTHROPIC_AUTH_TOKEN"
        url = "https://example.com/keys"

        [secret.hint]
        zh = "去后台复制 API Key"

        [secret.verify]
        url = "https://example.com/anthropic/v1/messages"
        "#,
    )
    .unwrap();

    assert_eq!(p.env.get("ANTHROPIC_BASE_URL").map(String::as_str),
               Some("https://example.com/anthropic"));
    let s = p.secret.as_ref().unwrap();
    assert_eq!(s.env, "ANTHROPIC_AUTH_TOKEN");
    assert_eq!(s.hint.get(Lang::Zh), Some("去后台复制 API Key"));
    assert_eq!(s.url.as_deref(), Some("https://example.com/keys"));
    assert_eq!(s.verify.as_ref().unwrap().url,
               "https://example.com/anthropic/v1/messages");
    assert_eq!(p.label.get(Lang::Zh), Some("Kimi"));
    assert_eq!(p.note.get(Lang::Zh), Some("月之暗面"));
}

#[test]
fn parses_busy_pattern_and_install() {
    let p = Profile::from_toml(
        r#"
        name = "codex"
        command = ["codex"]
        is_agent = true
        busy_pattern = "esc to interrupt"

        [install]
        command = ["npm", "i", "-g", "@openai/codex"]

        [install.note]
        zh = "需要先装 Node.js"
        "#,
    )
    .unwrap();

    assert_eq!(p.busy_pattern.as_deref(), Some("esc to interrupt"));
    let i = p.install.as_ref().unwrap();
    assert_eq!(i.command, vec!["npm", "i", "-g", "@openai/codex"]);
    assert_eq!(i.note.get(Lang::Zh), Some("需要先装 Node.js"));
}

#[test]
fn new_fields_all_default_to_empty() {
    // 老 profile 文件（只有 name/command/is_agent）必须照样能解析
    let p = Profile::from_toml(
        r#"
        name = "shell"
        command = ["/bin/zsh"]
        "#,
    )
    .unwrap();
    assert!(p.env.is_empty());
    assert!(p.secret.is_none());
    assert!(p.install.is_none());
    assert!(p.busy_pattern.is_none());
    assert_eq!(p.label.get(Lang::Zh), None);
}

#[test]
fn busy_regex_compiles() {
    let p = Profile::from_toml(
        r#"
        name = "x"
        command = ["x"]
        busy_pattern = "esc to interrupt"
        "#,
    )
    .unwrap();
    let re = p.busy_regex().unwrap().unwrap();
    assert!(re.is_match("  (12s • esc to interrupt)"));
    assert!(!re.is_match("? for shortcuts"));
}

#[test]
fn bad_busy_pattern_is_an_error() {
    let p = Profile::from_toml(
        r#"
        name = "x"
        command = ["x"]
        busy_pattern = "["
        "#,
    )
    .unwrap();
    assert!(p.busy_regex().is_err(), "非法正则要报错，不能静默当没有");
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib profile`
Expected: 编译失败，`no field 'env' on type 'Profile'` 之类

- [ ] **Step 3: 改数据结构**

替换 `src/profile.rs` 顶部的 `Profile` 定义（第 1-15 行附近）：

```rust
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

/// 界面语言。i18n 那一期会把它挪进 `src/i18n.rs` 并加上其余语言；
/// 这里先立一个单变体的版本，好让 profile 的多语言字段现在就按最终结构落地——
/// profile 是**用户可编辑的数据文件**，进不了 i18n 的词条表，
/// 现在写成平字符串，i18n 落地时就是一次会打破用户文件的改动。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
}

/// 一段可翻译的文案。TOML 里写成子表：`[label]` 下面 `zh = "..."`。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LocalizedText {
    #[serde(default)]
    pub zh: Option<String>,
    #[serde(default)]
    pub en: Option<String>,
}

impl LocalizedText {
    pub fn get(&self, lang: Lang) -> Option<&str> {
        match lang {
            Lang::Zh => self.zh.as_deref(),
        }
    }
}

/// 这个 profile 需要用户提供一份密钥。
#[derive(Debug, Clone, Deserialize)]
pub struct SecretSpec {
    /// 密钥注到哪个环境变量
    pub env: String,
    #[serde(default)]
    pub hint: LocalizedText,
    /// 申领页面，密钥界面上 Ctrl+O 打开
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub verify: Option<VerifySpec>,
}

/// 存盘前拿这个端点探一下，确认密钥不是明显错的。
#[derive(Debug, Clone, Deserialize)]
pub struct VerifySpec {
    pub url: String,
}

/// 这个 agent 没装时怎么装。
#[derive(Debug, Clone, Deserialize)]
pub struct InstallSpec {
    pub command: Vec<String>,
    #[serde(default)]
    pub note: LocalizedText,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub is_agent: bool,
    #[serde(default)]
    pub idle_pattern: Option<String>,
    /// agent 干活时屏幕上一定有的串（比如 codex 的 `esc to interrupt`）。
    /// 比 `idle_pattern` 可靠：空闲时的输入框占位符用户一打字就没了。
    #[serde(default)]
    pub busy_pattern: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub secret: Option<SecretSpec>,
    #[serde(default)]
    pub install: Option<InstallSpec>,
    #[serde(default)]
    pub label: LocalizedText,
    #[serde(default)]
    pub note: LocalizedText,
}
```

在 `impl Profile` 里、`idle_regex` 旁边加：

```rust
    pub fn busy_regex(&self) -> Result<Option<regex::Regex>> {
        match &self.busy_pattern {
            None => Ok(None),
            Some(p) => Ok(Some(regex::Regex::new(p).with_context(|| {
                format!("busy_pattern 不是合法正则: {p}")
            })?)),
        }
    }

    /// 菜单上显示的名字。没写 label 就回落到 profile 名——那至少是个能认的词。
    pub fn display_label(&self, lang: Lang) -> String {
        self.label.get(lang).unwrap_or(&self.name).to_string()
    }

    /// 菜单上的一行说明。没写就回落到**空串**，不回落到 name——
    /// 说明栏里再显示一遍命令名是噪音，不是信息。
    pub fn display_note(&self, lang: Lang) -> String {
        self.note.get(lang).unwrap_or("").to_string()
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib profile`
Expected: PASS，包括原有的 5 个测试

- [ ] **Step 5: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/profile.rs
git commit -m "feat: Profile 支持 env / secret / install / busy_pattern / 多语言文案

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: 七个新的内置 profile

**Files:**
- Create: `profiles/codex.toml`、`profiles/opencode.toml`、`profiles/qwen.toml`、`profiles/kimi.toml`、`profiles/glm.toml`、`profiles/deepseek.toml`、`profiles/qwen-api.toml`
- Modify: `profiles/claude.toml`、`profiles/shell.toml`（补 label / note）
- Modify: `src/profile.rs` 的 `builtin()` 与 `builtin_names()`
- Test: `src/profile.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 1 的 `Profile`、`Lang`
- Produces: `Profile::builtin(name) -> Option<Profile>`（已有，扩容到 9 条）、`Profile::builtin_names() -> Vec<&'static str>`（已有，返回顺序即菜单顺序）、`Profile::builtins() -> Vec<Profile>`（新）

⚠️ **本任务的 base_url、命令行参数、安装包名，除 codex 外都未经实测。** 见设计文档的「未实测项」表。实施时**必须逐条实跑验证**，与本文档不符的以实跑为准，并回头更新设计文档那张表。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn every_builtin_parses_and_is_well_formed() {
    for name in Profile::builtin_names() {
        let p = Profile::builtin(name)
            .unwrap_or_else(|| panic!("{name} 应当是内置 profile"));
        assert_eq!(p.name, name, "{name}: 文件里的 name 必须和清单一致");
        assert!(!p.command.is_empty(), "{name}: command 不能为空");
        assert!(
            p.label.get(Lang::Zh).is_some(),
            "{name}: 必须有中文 label，九个选项摆在非程序员面前没说明等于没得选"
        );
        // 正则必须能编译，否则一到 tick 就报错
        p.idle_regex().unwrap();
        p.busy_regex().unwrap();
    }
}

#[test]
fn builtin_names_are_in_menu_order() {
    assert_eq!(
        Profile::builtin_names(),
        vec![
            "claude", "codex", "opencode", "qwen",
            "kimi", "glm", "deepseek", "qwen-api",
            "shell",
        ]
    );
}

#[test]
fn api_shaped_profiles_run_claude_and_need_a_secret() {
    for name in ["kimi", "glm", "deepseek", "qwen-api"] {
        let p = Profile::builtin(name).unwrap();
        assert_eq!(p.command[0], "claude", "{name}: API 形态跑的是 claude");
        assert!(p.env.contains_key("ANTHROPIC_BASE_URL"), "{name}: 要换 base_url");
        let s = p.secret.as_ref().unwrap_or_else(|| panic!("{name}: 要声明密钥"));
        assert_eq!(s.env, "ANTHROPIC_AUTH_TOKEN");
        assert!(s.verify.is_some(), "{name}: 要能验证密钥");
    }
}

#[test]
fn codex_detects_busy_not_idle() {
    // codex 空闲时屏幕上没有稳定的固定串，干活时一定有 esc to interrupt。
    // 实测自 codex v0.146.0。
    let p = Profile::builtin("codex").unwrap();
    assert!(p.busy_pattern.is_some());
    assert!(p.idle_pattern.is_none());
    assert!(p.busy_regex().unwrap().unwrap().is_match("(12s • esc to interrupt)"));
}

#[test]
fn unverified_profiles_have_no_pattern() {
    // opencode / qwen 的 TUI 没实测过。宁可状态显示「—」，不能瞎猜一个 pattern
    // 然后在看板上编状态。
    for name in ["opencode", "qwen"] {
        let p = Profile::builtin(name).unwrap();
        assert!(p.idle_pattern.is_none() && p.busy_pattern.is_none(),
                "{name}: 没实测就别填 pattern");
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib profile`
Expected: FAIL，`codex 应当是内置 profile`

- [ ] **Step 3: 写九个 profile 文件**

`profiles/claude.toml`（改）：

```toml
name = "claude"
command = ["claude", "--dangerously-skip-permissions"]
is_agent = true
idle_pattern = "\\? for shortcuts"

[label]
zh = "Claude"

[note]
zh = "Anthropic 官方"
```

`profiles/codex.toml`：

```toml
name = "codex"
command = ["codex", "--dangerously-bypass-approvals-and-sandbox"]
is_agent = true
# 实测自 codex v0.146.0：干活时底部一定有「(12s • esc to interrupt)」。
# 空闲时只有输入框占位符，用户一打字就没了，不能拿来当 idle_pattern。
busy_pattern = "esc to interrupt"

[label]
zh = "Codex"

[note]
zh = "OpenAI 官方"

[install]
command = ["npm", "i", "-g", "@openai/codex"]

[install.note]
zh = "需要先装 Node.js"
```

`profiles/opencode.toml`：

```toml
name = "opencode"
command = ["opencode"]
is_agent = true
# TUI 没实测过，不填 pattern —— 状态显示「—」比编一个假状态好

[label]
zh = "OpenCode"

[note]
zh = "开源，可接多种模型"

[install]
command = ["npm", "i", "-g", "opencode-ai"]

[install.note]
zh = "需要先装 Node.js"
```

`profiles/qwen.toml`：

```toml
name = "qwen"
command = ["qwen"]
is_agent = true
# 同 opencode，未实测

[label]
zh = "Qwen Code"

[note]
zh = "阿里通义，独立命令行"

[install]
command = ["npm", "i", "-g", "@qwen-code/qwen-code"]

[install.note]
zh = "需要先装 Node.js"
```

`profiles/kimi.toml`：

```toml
name = "kimi"
command = ["claude", "--dangerously-skip-permissions"]
is_agent = true
idle_pattern = "\\? for shortcuts"

[label]
zh = "Kimi"

[note]
zh = "月之暗面，套用 Claude 界面"

[env]
ANTHROPIC_BASE_URL = "https://api.moonshot.cn/anthropic"

[secret]
env = "ANTHROPIC_AUTH_TOKEN"
url = "https://platform.moonshot.cn/console/api-keys"

[secret.hint]
zh = "在 platform.moonshot.cn 开通后复制 API Key"

[secret.verify]
url = "https://api.moonshot.cn/anthropic/v1/messages"
```

`profiles/glm.toml`：

```toml
name = "glm"
command = ["claude", "--dangerously-skip-permissions"]
is_agent = true
idle_pattern = "\\? for shortcuts"

[label]
zh = "GLM"

[note]
zh = "智谱，套用 Claude 界面"

[env]
ANTHROPIC_BASE_URL = "https://open.bigmodel.cn/api/anthropic"

[secret]
env = "ANTHROPIC_AUTH_TOKEN"
url = "https://open.bigmodel.cn/usercenter/apikeys"

[secret.hint]
zh = "在 open.bigmodel.cn 的用户中心复制 API Key"

[secret.verify]
url = "https://open.bigmodel.cn/api/anthropic/v1/messages"
```

`profiles/deepseek.toml`：

```toml
name = "deepseek"
command = ["claude", "--dangerously-skip-permissions"]
is_agent = true
idle_pattern = "\\? for shortcuts"

[label]
zh = "DeepSeek"

[note]
zh = "深度求索，套用 Claude 界面"

[env]
ANTHROPIC_BASE_URL = "https://api.deepseek.com/anthropic"

[secret]
env = "ANTHROPIC_AUTH_TOKEN"
url = "https://platform.deepseek.com/api_keys"

[secret.hint]
zh = "在 platform.deepseek.com 开通后复制 API Key"

[secret.verify]
url = "https://api.deepseek.com/anthropic/v1/messages"
```

`profiles/qwen-api.toml`：

```toml
name = "qwen-api"
command = ["claude", "--dangerously-skip-permissions"]
is_agent = true
idle_pattern = "\\? for shortcuts"

[label]
zh = "Qwen API"

[note]
zh = "阿里通义，套用 Claude 界面"

[env]
ANTHROPIC_BASE_URL = "https://dashscope.aliyuncs.com/api/v2/apps/claude-code-proxy"

[secret]
env = "ANTHROPIC_AUTH_TOKEN"
url = "https://bailian.console.aliyun.com/?tab=model#/api-key"

[secret.hint]
zh = "在阿里云百炼控制台创建 API Key"

[secret.verify]
url = "https://dashscope.aliyuncs.com/api/v2/apps/claude-code-proxy/v1/messages"
```

`profiles/shell.toml`（改）：

```toml
name = "shell"
command = ["/bin/zsh"]
is_agent = false

[label]
zh = "命令行"

[note]
zh = "普通终端，不带 AI"
```

- [ ] **Step 4: 接上 builtin()**

替换 `src/profile.rs` 里的 `CLAUDE` / `SHELL` 常量与两个方法：

```rust
const CLAUDE: &str = include_str!("../profiles/claude.toml");
const CODEX: &str = include_str!("../profiles/codex.toml");
const OPENCODE: &str = include_str!("../profiles/opencode.toml");
const QWEN: &str = include_str!("../profiles/qwen.toml");
const KIMI: &str = include_str!("../profiles/kimi.toml");
const GLM: &str = include_str!("../profiles/glm.toml");
const DEEPSEEK: &str = include_str!("../profiles/deepseek.toml");
const QWEN_API: &str = include_str!("../profiles/qwen-api.toml");
const SHELL: &str = include_str!("../profiles/shell.toml");
```

```rust
    pub fn builtin(name: &str) -> Option<Profile> {
        let src = match name {
            "claude" => CLAUDE,
            "codex" => CODEX,
            "opencode" => OPENCODE,
            "qwen" => QWEN,
            "kimi" => KIMI,
            "glm" => GLM,
            "deepseek" => DEEPSEEK,
            "qwen-api" => QWEN_API,
            "shell" => SHELL,
            _ => return None,
        };
        Some(Profile::from_toml(src).expect("内置 profile 必须能解析"))
    }

    /// 返回顺序就是菜单顺序：先独立 CLI，再 API 形态，命令行垫底。
    /// 命令行放最后是因为它对目标用户价值最低——非程序员不需要裸终端。
    pub fn builtin_names() -> Vec<&'static str> {
        vec![
            "claude", "codex", "opencode", "qwen",
            "kimi", "glm", "deepseek", "qwen-api",
            "shell",
        ]
    }

    pub fn builtins() -> Vec<Profile> {
        Profile::builtin_names()
            .into_iter()
            .filter_map(Profile::builtin)
            .collect()
    }
```

原有的 `builtin_names_lists_both` 测试改名成 `builtin_names_includes_claude_and_shell`，断言改成 `contains`（`builtin_names_are_in_menu_order` 已经覆盖完整顺序）。

- [ ] **Step 5: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib profile`
Expected: PASS

- [ ] **Step 6: 实跑验证未实测项**

这一步是人工的，不能跳。逐条确认，与本文档不符就以实跑为准，同时更新设计文档的「未实测项」表：

```bash
# 独立 CLI：命令能不能起、TUI 干活时屏幕上有什么固定串
which opencode qwen
# 装了的话，各起一次，观察空闲屏和干活屏，补上 idle_pattern 或 busy_pattern

# 四个 base_url：拿一份真 key 探一下，401/403 以外都算通
curl -s -o /dev/null -w '%{http_code}\n' \
  -X POST 'https://api.moonshot.cn/anthropic/v1/messages' \
  -H 'content-type: application/json' -H "x-api-key: $KEY" \
  -d '{"model":"moonshot-v1-8k","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}'
```

- [ ] **Step 7: 提交**

```bash
~/.cargo/bin/cargo fmt
git add profiles src/profile.rs docs/superpowers/specs/2026-08-03-dct-multi-agent-design.md
git commit -m "feat: 内置 codex/opencode/qwen 与 kimi/glm/deepseek/qwen-api

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

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

## Task 4: 密钥仓

**Files:**
- Create: `src/secrets.rs`
- Modify: `src/lib.rs`（加 `pub mod secrets;`）
- Test: `src/secrets.rs` 的 `mod tests`

**Interfaces:**
- Consumes: 无
- Produces: `SecretStore`，方法 `secrets_path_for_socket(&Path) -> PathBuf`、`SecretStore::load(&Path) -> SecretStore`、`get(&self, profile: &str) -> Option<&str>`、`set(&mut self, profile: &str, value: &str) -> Result<()>`、`remove(&mut self, profile: &str) -> Result<()>`、`load_error(&self) -> Option<&str>`

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn secrets_path_sits_next_to_socket() {
        let p = secrets_path_for_socket(Path::new("/home/x/.dct/daemon.sock"));
        assert_eq!(p, PathBuf::from("/home/x/.dct/secrets.toml"));
    }

    #[test]
    fn set_then_get_survives_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");

        let mut s = SecretStore::load(&f);
        s.set("kimi", "sk-abc").unwrap();
        drop(s);

        let s2 = SecretStore::load(&f);
        assert_eq!(s2.get("kimi"), Some("sk-abc"));
    }

    #[test]
    fn file_is_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        let mut s = SecretStore::load(&f);
        s.set("kimi", "sk-abc").unwrap();

        let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "密钥文件只能属主可读写");
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        let mut s = SecretStore::load(&f);
        s.set("kimi", "sk-abc").unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "secrets.toml")
            .collect();
        assert!(leftovers.is_empty(), "原子写的临时文件要收干净：{leftovers:?}");
    }

    #[test]
    fn remove_deletes_the_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        let mut s = SecretStore::load(&f);
        s.set("kimi", "sk-abc").unwrap();
        s.set("glm", "sk-def").unwrap();
        s.remove("kimi").unwrap();

        assert_eq!(s.get("kimi"), None);
        assert_eq!(s.get("glm"), Some("sk-def"), "只删指定的那条");
    }

    #[test]
    fn missing_file_is_empty_and_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let s = SecretStore::load(&tmp.path().join("还没建过.toml"));
        assert_eq!(s.get("kimi"), None);
        assert!(s.load_error().is_none(), "文件还没建过是常态");
    }

    #[test]
    fn corrupt_file_refuses_to_write() {
        // 关键行为：读坏了**不能**当空。当空的话用户以为密钥丢了，
        // 接着一次写入就把本来还能手工救回的文件彻底覆盖。
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        std::fs::write(&f, "这不是 TOML {{{").unwrap();

        let mut s = SecretStore::load(&f);
        assert!(s.load_error().is_some(), "要记住读失败了");

        let err = s.set("kimi", "sk-abc").unwrap_err();
        assert!(
            err.to_string().contains("密钥文件"),
            "拒绝写入时要说人话：{err}"
        );
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "这不是 TOML {{{",
            "原文件必须一个字节都没动"
        );
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib secrets`
Expected: FAIL，模块不存在

- [ ] **Step 3: 实现**

`src/secrets.rs`：

```rust
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// 磁盘格式。包一层 `[secrets]` 表而不是把键平铺在顶层，
/// 是为了将来加别的配置段时老文件仍能读。
#[derive(Default, Serialize, Deserialize)]
struct Disk {
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

/// 按 profile 名索引的用户密钥。落盘在 `~/.dct/secrets.toml`，0600。
pub struct SecretStore {
    path: PathBuf,
    secrets: BTreeMap<String, String>,
    /// 读失败的原因。非 None 时**拒绝任何写入**——见 `set()` 的注释。
    load_error: Option<String>,
}

/// 跟着 socket 走，测试自动隔离（同 `projects::store_path_for_socket`）。
pub fn secrets_path_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("secrets.toml"),
        None => PathBuf::from("secrets.toml"),
    }
}

impl SecretStore {
    pub fn load(path: &Path) -> SecretStore {
        let (secrets, load_error) = match std::fs::read_to_string(path) {
            // 文件还没建过是常态，不是错误
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (BTreeMap::new(), None),
            Err(e) => (BTreeMap::new(), Some(format!("{e}"))),
            Ok(src) => match toml::from_str::<Disk>(&src) {
                Ok(d) => (d.secrets, None),
                Err(e) => (BTreeMap::new(), Some(format!("{e}"))),
            },
        };
        SecretStore {
            path: path.to_path_buf(),
            secrets,
            load_error,
        }
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    pub fn get(&self, profile: &str) -> Option<&str> {
        self.secrets.get(profile).map(String::as_str)
    }

    pub fn set(&mut self, profile: &str, value: &str) -> Result<()> {
        self.secrets.insert(profile.to_string(), value.to_string());
        self.save()
    }

    pub fn remove(&mut self, profile: &str) -> Result<()> {
        self.secrets.remove(profile);
        self.save()
    }

    /// 和 `projects::Store::save` 不同，这里**落盘失败要报错**：那边丢的是
    /// 「最近项目」这种便利性缓存，这边丢的是用户刚手打的密钥——静默失败
    /// 意味着他下次回来发现还得再填一遍，且不知道为什么。
    fn save(&self) -> Result<()> {
        // 读坏了就不写。当空覆盖的话，用户手改坏的文件（也许只是少个引号，
        // 完全能救回来）会被我们内存里那份残缺数据彻底盖掉。
        if let Some(e) = &self.load_error {
            bail!("密钥文件读不了（{e}），先修好 {} 再改", self.path.display());
        }

        let parent = self
            .path
            .parent()
            .context("密钥文件没有上级目录")?;
        std::fs::create_dir_all(parent).context("建不了密钥文件所在目录")?;

        let text = toml::to_string(&Disk {
            secrets: self.secrets.clone(),
        })
        .context("密钥序列化失败")?;

        // 原子写：先写同目录的临时文件再 rename。直接覆写的话写到一半断电
        // 会留下半截 TOML，下次 load 就走进「读坏了」分支。
        //
        // 临时文件从**创建那一刻**就是 0600，不是先建再 chmod ——
        // 那中间有一个别的账号能读到密钥的窗口。
        let tmp = self.path.with_extension("toml.tmp");
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .context("写不了密钥临时文件")?;
            f.write_all(text.as_bytes()).context("写密钥失败")?;
            f.sync_all().context("刷盘失败")?;
        }
        std::fs::rename(&tmp, &self.path).context("替换密钥文件失败")?;
        Ok(())
    }
}
```

`src/lib.rs` 加 `pub mod secrets;`。

⚠️ `mode(0o600)` 只在**创建**文件时生效。临时文件每次都是新建（前一次已经 rename 走了），所以没问题；但如果上一次 rename 失败留下了 tmp，`truncate` 会复用它的旧权限。这不是安全洞（旧的也是 0600），但值得知道。

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib secrets`
Expected: PASS，7 个测试

- [ ] **Step 5: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/secrets.rs src/lib.rs
git commit -m "feat: 密钥仓 ~/.dct/secrets.toml，0600 原子写，读坏时拒绝覆盖

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: 环境变量注入到 PTY

**Files:**
- Modify: `src/pty.rs:40-106`（`spawn` 签名）、`src/pty.rs:280-320`（现有测试的调用）
- Modify: `src/session.rs:100-143`（`create`）
- Test: `src/pty.rs`、`src/session.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 1 的 `Profile.env`、Task 4 的 `SecretStore`
- Produces: `PtySession::spawn(cmd: &[String], env: &BTreeMap<String, String>, cwd: &Path, rows: u16, cols: u16) -> Result<PtySession>`；`SessionManager::create(&self, dir: &Path, profile_name: &str, secrets: &SecretStore) -> Result<u32>`

- [ ] **Step 1: 写失败的测试**

加到 `src/pty.rs` 的 `mod tests`：

```rust
#[test]
fn spawn_passes_env_to_the_child() {
    use std::collections::BTreeMap;
    let dir = tempfile::tempdir().unwrap();
    let mut env = BTreeMap::new();
    env.insert("DCT_TEST_MARKER".to_string(), "看得见我".to_string());

    let p = PtySession::spawn(
        &["/bin/sh".to_string(), "-c".to_string(),
          "echo $DCT_TEST_MARKER; sleep 5".to_string()],
        &env,
        dir.path(),
        24,
        80,
    )
    .unwrap();

    assert!(
        wait_for(&p, "看得见我"),
        "profile 里的 env 必须传给子进程，否则换 base_url 的 agent 全起不来"
    );
}
```

`wait_for` 是 `src/pty.rs` 测试里已有的辅助（见 `src/pty.rs:272` 附近）；如果它的签名对不上，照现有测试的等待写法自己等。

加到 `src/session.rs` 的 `mod tests`：

```rust
#[test]
fn create_injects_the_secret_into_env() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();

    let mgr = SessionManager::new();
    mgr.register_profile(
        Profile::from_toml(
            r#"
            name = "fake-api"
            command = ["/bin/sh", "-c", "echo TOKEN=$MY_TOKEN BASE=$MY_BASE; sleep 5"]
            is_agent = false

            [env]
            MY_BASE = "https://example.com"

            [secret]
            env = "MY_TOKEN"
            "#,
        )
        .unwrap(),
    );

    let mut secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
    secrets.set("fake-api", "sk-xyz").unwrap();

    let id = mgr.create(&proj, "fake-api", &secrets).unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let text = mgr.screen_text_for_test(id);
        if text.contains("TOKEN=sk-xyz") && text.contains("BASE=https://example.com") {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "没看到注入的环境变量：{text}");
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn create_without_the_secret_still_starts() {
    // 没填密钥不该在 create 这一层拦住——可用性判定是 UI 的事，
    // create 拦一遍会让「装完 CLI 想先跑起来看看」这种路径莫名其妙失败。
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();

    let mgr = SessionManager::new();
    mgr.register_profile(
        Profile::from_toml(
            r#"
            name = "fake-api"
            command = ["/bin/sh", "-c", "sleep 5"]
            is_agent = false

            [secret]
            env = "MY_TOKEN"
            "#,
        )
        .unwrap(),
    );

    let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
    assert!(mgr.create(&proj, "fake-api", &secrets).is_ok());
}
```

`screen_text_for_test(id)` 如果不存在就加一个 `#[cfg(test)]` 的小方法，内容是取会话的 `pty.screen_text()`。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib`
Expected: 编译失败，`spawn` 参数个数不对

- [ ] **Step 3: 改 `PtySession::spawn`**

`src/pty.rs`，签名与 builder 部分：

```rust
    pub fn spawn(
        cmd: &[String],
        env: &std::collections::BTreeMap<String, String>,
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> Result<PtySession> {
```

在 `builder.cwd(cwd);` 后面加：

```rust
        // 只加不减：不清空继承来的环境。ANTHROPIC_BASE_URL 这类是覆盖上去的，
        // 但 PATH / HOME / 各家 CLI 自己的登录态都得留着，清了 agent 就起不来。
        for (k, v) in env {
            builder.env(k, v);
        }
```

`src/pty.rs` 里现有的四处 `PtySession::spawn(...)` 测试调用，第二个参数传 `&Default::default()`。

- [ ] **Step 4: 改 `SessionManager::create`**

`src/session.rs`：

```rust
    pub fn create(&self, dir: &Path, profile_name: &str, secrets: &SecretStore) -> Result<u32> {
        let profile = self.resolve_profile(profile_name)?;
        // ...（目录检查、id 分配、git 检查照旧）...

        let idle_re = profile.idle_regex()?;
        let busy_re = profile.busy_regex()?;
        let is_agent = profile.is_agent;

        // profile 的静态 env 打底，密钥覆盖上去。密钥不在 profile 文件里，
        // 只在这一步才和命令合到一起——profile 文件因此可以随便拷贝分享。
        let mut env = profile.env.clone();
        if let Some(spec) = &profile.secret {
            if let Some(key) = secrets.get(&profile.name) {
                env.insert(spec.env.clone(), key.to_string());
            }
        }

        let pty = PtySession::spawn(&profile.command, &env, dir, 40, 120)?;
        // ...
    }
```

`Session` 结构体加 `busy_re: Option<regex::Regex>` 字段并在构造时填上（`tick()` 下个任务才用）。

`src/daemon.rs` 里 `Request::Create` 的调用点跟着改（密钥仓在 Task 8 接线，这一步先传一个从 `secrets_path_for_socket` load 出来的 `SecretStore`，和 `store` 一样放进 `Arc<Mutex<_>>`）。

- [ ] **Step 5: 让起不来的命令说人话**

写测试：

```rust
#[test]
fn spawn_failure_says_what_to_do_not_enoent() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();

    let mgr = SessionManager::new();
    mgr.register_profile(
        Profile::from_toml(
            "name = \"gone\"\ncommand = [\"/绝对不存在/x9\"]\n",
        )
        .unwrap(),
    );
    let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
    let err = mgr.create(&proj, "gone", &secrets).unwrap_err().to_string();

    assert!(err.contains("启动不了"), "要说人话：{err}");
    assert!(!err.to_lowercase().contains("enoent"), "别把系统错误码甩给用户：{err}");
}
```

`src/pty.rs` 的 spawn 错误上下文（现在是 `format!("启动 {} 失败", cmd[0])`）改成：

```rust
            .with_context(|| {
                // 用户看得懂的话。命令确实在 PATH 上但起不来（权限不对、
                // 架构不匹配、脚本头写错），底层错误对非程序员没有意义。
                format!("启动不了 {}，它可能装坏了", cmd[0])
            })?;
```

⚠️ anyhow 默认会把整条 source 链打出来，`ENOENT` 还是会露。守护进程往 `Response::Error` 里塞的时候只取最外层：`format!("{e}")` 而不是 `format!("{e:#}")`。确认 `src/daemon.rs` 现有的错误转换用的是前者。

- [ ] **Step 6: 跑全部测试**

Run: `~/.cargo/bin/cargo test`
Expected: PASS。集成测试 `tests/*.rs` 不调用 `spawn`/`create`，只走协议，应当不受影响。

- [ ] **Step 7: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/pty.rs src/session.rs src/daemon.rs
git commit -m "feat: profile 的 env 与密钥注入子进程

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: busy_pattern 与 SessionState::Unknown

**Files:**
- Modify: `src/session.rs:16-23`（枚举）、`src/session.rs:129-138`（初始状态）、`src/session.rs:265-286`（`tick`）
- Modify: `src/ui.rs:20-36`（`status_label` / `status_color`）
- Test: `src/session.rs`、`src/ui.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 5 的 `Session.busy_re`
- Produces: `SessionState::Unknown`；`status_label(SessionState::Unknown) == "—"`

- [ ] **Step 1: 写失败的测试**

`src/session.rs`：

```rust
#[test]
fn busy_pattern_marks_working_then_idle() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();

    let mgr = SessionManager::new();
    mgr.register_profile(
        Profile::from_toml(
            r#"
            name = "busy-demo"
            command = ["/bin/sh", "-c", "echo esc to interrupt; sleep 1; clear; echo done; sleep 5"]
            is_agent = false
            busy_pattern = "esc to interrupt"
            "#,
        )
        .unwrap(),
    );
    let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
    let id = mgr.create(&proj, "busy-demo", &secrets).unwrap();

    // 屏幕上有 busy 串 → 干活中
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        mgr.tick();
        if state_of(&mgr, id) == SessionState::Working {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "busy 串在屏上就该是 Working");
        sleep(Duration::from_millis(50));
    }

    // 串消失 → 空闲
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        mgr.tick();
        if state_of(&mgr, id) == SessionState::Idle {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "busy 串没了就该是 Idle");
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn busy_pattern_wins_over_idle_pattern() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();

    let mgr = SessionManager::new();
    // 两个 pattern 同时命中。busy 优先 → Working。
    mgr.register_profile(
        Profile::from_toml(
            r#"
            name = "both"
            command = ["/bin/sh", "-c", "echo BUSY IDLE; sleep 5"]
            is_agent = false
            busy_pattern = "BUSY"
            idle_pattern = "IDLE"
            "#,
        )
        .unwrap(),
    );
    let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
    let id = mgr.create(&proj, "both", &secrets).unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        mgr.tick();
        if state_of(&mgr, id) == SessionState::Working {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "busy_pattern 必须压过 idle_pattern"
        );
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn no_pattern_stays_unknown() {
    // shell 就是这种。以前它永远显示「干活中」，是明确的假信息。
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();

    let mgr = SessionManager::new();
    mgr.register_profile(
        Profile::from_toml(
            r#"
            name = "quiet"
            command = ["/bin/sh", "-c", "sleep 5"]
            is_agent = false
            "#,
        )
        .unwrap(),
    );
    let secrets = SecretStore::load(&tmp.path().join("secrets.toml"));
    let id = mgr.create(&proj, "quiet", &secrets).unwrap();

    assert_eq!(state_of(&mgr, id), SessionState::Unknown, "没 pattern 就别编状态");
    for _ in 0..5 {
        mgr.tick();
        sleep(Duration::from_millis(20));
    }
    assert_eq!(state_of(&mgr, id), SessionState::Unknown, "tick 也不该把它改成 Working");
}
```

`state_of` 是个测试辅助：`fn state_of(mgr: &SessionManager, id: u32) -> SessionState { mgr.list().into_iter().find(|s| s.id == id).unwrap().state }`。如果 `src/session.rs` 的测试里已有等价写法就复用。

`src/ui.rs`：

```rust
#[test]
fn unknown_state_shows_a_dash() {
    assert_eq!(status_label(SessionState::Unknown), "—");
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib`
Expected: FAIL，`no variant named 'Unknown'`

- [ ] **Step 3: 实现**

`src/session.rs` 枚举加一个变体：

```rust
pub enum SessionState {
    Working,
    /// 由后续的 Bridge 在 agent 调用 ask_human 时设置；本计划内不会出现
    Asking,
    Idle,
    Stopped,
    /// profile 没给任何 pattern，我们不知道它在干什么。
    /// 显示「—」而不是猜一个——`shell` 以前就是被猜成「干活中」的。
    Unknown,
}
```

`create()` 里的初始状态：

```rust
        // 有 pattern 才敢说「干活中」：agent 刚起来确实在初始化。
        // 没 pattern 就一直是 Unknown，tick 也不会改它。
        let state = if idle_re.is_some() || busy_re.is_some() {
            SessionState::Working
        } else {
            SessionState::Unknown
        };
```

`tick()` 里替换判定：

```rust
            // busy 优先：agent 干活时的「按 esc 中断」提示是稳定的，
            // 而空闲时的输入框占位符用户一打字就没了。
            if let Some(re) = &s.busy_re {
                s.state = if re.is_match(&s.pty.screen_text()) {
                    SessionState::Working
                } else {
                    SessionState::Idle
                };
            } else if let Some(re) = &s.idle_re {
                s.state = if re.is_match(&s.pty.screen_text()) {
                    SessionState::Idle
                } else {
                    SessionState::Working
                };
            }
            // 两个都没有：状态不动，保持 Unknown
```

`src/ui.rs`：

```rust
pub fn status_label(s: SessionState) -> &'static str {
    match s {
        SessionState::Working => "干活中",
        SessionState::Asking => "等你回答",
        SessionState::Idle => "空闲",
        SessionState::Stopped => "已停止",
        SessionState::Unknown => "—",
    }
}

pub fn status_color(s: SessionState) -> Color {
    match s {
        SessionState::Working => Color::Cyan,
        SessionState::Asking => Color::Yellow,
        SessionState::Idle => Color::Green,
        SessionState::Stopped => Color::DarkGray,
        SessionState::Unknown => Color::DarkGray,
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/session.rs src/ui.rs
git commit -m "feat: busy_pattern 判定状态；没 pattern 的会话显示「—」不再假装干活中

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: 可用性判定

**Files:**
- Modify: `src/profile.rs`
- Test: `src/profile.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 1、2、3
- Produces:
  ```rust
  pub enum ProfileStatus {
      Ready,
      NeedsSecret,
      NeedsDependency { label: String },
      NotInstalled { command: String },
  }
  pub fn status_of(
      p: &Profile,
      all: &[Profile],
      has_secret: bool,
      installed: &dyn Fn(&str) -> bool,
      lang: Lang,
  ) -> ProfileStatus;
  pub fn command_exists(cmd: &str) -> bool;
  ```

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
fn status_fixture() -> Vec<Profile> {
    Profile::builtins()
}

#[test]
fn ready_when_installed_and_secret_present() {
    let all = status_fixture();
    let kimi = all.iter().find(|p| p.name == "kimi").unwrap();
    let st = status_of(kimi, &all, true, &|_| true, Lang::Zh);
    assert!(matches!(st, ProfileStatus::Ready));
}

#[test]
fn needs_secret_when_installed_but_no_key() {
    let all = status_fixture();
    let kimi = all.iter().find(|p| p.name == "kimi").unwrap();
    let st = status_of(kimi, &all, false, &|_| true, Lang::Zh);
    assert!(matches!(st, ProfileStatus::NeedsSecret));
}

#[test]
fn not_installed_when_the_command_owns_its_name() {
    let all = status_fixture();
    let codex = all.iter().find(|p| p.name == "codex").unwrap();
    let st = status_of(codex, &all, false, &|_| false, Lang::Zh);
    match st {
        ProfileStatus::NotInstalled { command } => assert_eq!(command, "codex"),
        other => panic!("codex 自己就是那个命令，应当报未安装，得到 {other:?}"),
    }
}

#[test]
fn dependency_is_reported_before_secret() {
    // 这条顺序是整个判定里最要紧的：kimi 跑的是 claude。claude 没装时
    // 如果先报「未填密钥」，用户会去填 key，填完还是起不来，
    // 然后以为是 key 的问题——被送进死胡同。
    let all = status_fixture();
    let kimi = all.iter().find(|p| p.name == "kimi").unwrap();
    let st = status_of(kimi, &all, false, &|_| false, Lang::Zh);
    match st {
        ProfileStatus::NeedsDependency { label } => assert_eq!(label, "Claude"),
        other => panic!("claude 没装时 kimi 要报依赖，不是密钥，得到 {other:?}"),
    }
}

#[test]
fn dependency_uses_the_owner_profiles_label_not_the_raw_command() {
    let all = status_fixture();
    let glm = all.iter().find(|p| p.name == "glm").unwrap();
    let st = status_of(glm, &all, true, &|c| c != "claude", Lang::Zh);
    match st {
        ProfileStatus::NeedsDependency { label } => {
            assert_eq!(label, "Claude", "给用户看 label，不是二进制名");
        }
        other => panic!("得到 {other:?}"),
    }
}

#[test]
fn profile_without_secret_is_ready_when_installed() {
    let all = status_fixture();
    let shell = all.iter().find(|p| p.name == "shell").unwrap();
    assert!(matches!(
        status_of(shell, &all, false, &|_| true, Lang::Zh),
        ProfileStatus::Ready
    ));
}

#[test]
fn command_exists_finds_sh_and_not_a_made_up_name() {
    assert!(command_exists("sh"), "PATH 上一定有 sh");
    assert!(!command_exists("dct-绝对没有这个命令-x9"));
}

#[test]
fn command_exists_handles_absolute_paths() {
    assert!(command_exists("/bin/sh"));
    assert!(!command_exists("/bin/根本没有这个"));
}
```

`ProfileStatus` 要 `#[derive(Debug)]`，否则 `panic!("{other:?}")` 编译不过。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib profile`
Expected: FAIL，`cannot find function 'status_of'`

- [ ] **Step 3: 实现**

加到 `src/profile.rs`：

```rust
/// 这个 profile 现在能不能用，不能的话卡在哪。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProfileStatus {
    Ready,
    /// 声明了 secret 但密钥仓里没有
    NeedsSecret,
    /// 跑的是别的 profile 的命令，而那个命令没装。`label` 是那个 profile 的显示名。
    NeedsDependency { label: String },
    /// `command[0]` 在 PATH 上找不到，而且这个命令就是它自己
    NotInstalled { command: String },
}

/// `cmd` 能不能执行。带斜杠当路径查，否则遍历 PATH。
///
/// **这个判断必须和实际 spawn 用同一个环境**，所以只能在守护进程里调用——
/// 界面进程的 PATH 可能不一样，那会导致「菜单说能用，一开就失败」。
pub fn command_exists(cmd: &str) -> bool {
    fn is_exec(p: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    if cmd.contains('/') {
        return is_exec(Path::new(cmd));
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':')
        .filter(|d| !d.is_empty())
        .any(|d| is_exec(&Path::new(d).join(cmd)))
}

/// `command[0]` 这个命令「归谁所有」——名字和命令名相同的那个 profile。
///
/// kimi/glm/deepseek/qwen-api 的 command[0] 都是 `claude`，归 `claude` 这个
/// profile 所有；`claude` 自己的名字就是 `claude`，所以它是自己的 owner。
/// 靠这个区分「我没装」和「我依赖的东西没装」。
fn dependency_owner<'a>(all: &'a [Profile], cmd: &str) -> Option<&'a Profile> {
    let base = Path::new(cmd)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| cmd.to_string());
    all.iter().find(|p| p.name == base)
}

pub fn status_of(
    p: &Profile,
    all: &[Profile],
    has_secret: bool,
    installed: &dyn Fn(&str) -> bool,
    lang: Lang,
) -> ProfileStatus {
    let Some(cmd) = p.command.first() else {
        // 解析层允许空 command（TOML 里写了 `command = []`），这里兜住，
        // 免得 spawn 的时候 panic
        return ProfileStatus::NotInstalled {
            command: String::new(),
        };
    };

    // 顺序不能换：装没装排在密钥前面。见测试
    // `dependency_is_reported_before_secret` 的注释。
    if !installed(cmd) {
        return match dependency_owner(all, cmd) {
            Some(owner) if owner.name != p.name => ProfileStatus::NeedsDependency {
                label: owner.display_label(lang),
            },
            _ => ProfileStatus::NotInstalled {
                command: cmd.clone(),
            },
        };
    }

    if p.secret.is_some() && !has_secret {
        return ProfileStatus::NeedsSecret;
    }

    ProfileStatus::Ready
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test --lib profile`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/profile.rs
git commit -m "feat: profile 可用性判定，依赖缺失优先于密钥缺失

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: 协议与守护进程

**Files:**
- Modify: `src/proto.rs`
- Modify: `src/daemon.rs`
- Modify: `src/projects.rs`（存「上次用的 agent」）
- Modify: `src/ui.rs:315`、`src/ui.rs:385`（跟上编译）
- Modify: `tests/concurrency.rs:76`、`tests/daemon_roundtrip.rs:30`、`tests/projects_flow.rs:46,73`（`Create` 多了一个字段）
- Test: `src/projects.rs` 的 `mod tests`；新建 `tests/profiles_flow.rs`

**Interfaces:**
- Consumes: Task 3、4、7
- Produces:
  ```rust
  // proto.rs
  pub struct SecretPrompt { pub hint: String, pub url: Option<String> }
  pub struct InstallPrompt { pub command: Vec<String>, pub note: String }
  pub struct ProfileEntry {
      pub name: String,
      pub label: String,
      pub note: String,
      pub status: ProfileStatus,
      pub secret: Option<SecretPrompt>,
      pub install: Option<InstallPrompt>,
  }
  Request::Create { dir: String, profile: String, remember: bool }
  Request::SetSecret { profile: String, value: String }
  Request::DeleteSecret { profile: String }
  Request::LastProfile
  Response::Profiles { entries: Vec<ProfileEntry>, warning: Option<String> }
  Response::LastProfile(Option<String>)
  // projects.rs
  Store::last_profile(&self) -> Option<&str>
  Store::set_last_profile(&mut self, name: &str)
  ```
- **`create` 的签名在本任务再变一次**：Task 5 定的是 `create(&self, dir, profile_name, secrets)`，这里加上磁盘 profile 变成 `create(&self, dir, profile_name, secrets, profiles: &[Profile])`。两次改动分开是因为 Task 5 只关心环境变量，磁盘 profile 是本任务才引入的。

`Request::VerifySecret` 在 Task 9 加，这里不做。

- [ ] **Step 1: 写失败的测试**

`src/projects.rs`：

```rust
#[test]
fn last_profile_survives_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("projects.json");
    let mut s = Store::load(&f);
    s.set_last_profile("kimi");
    drop(s);
    assert_eq!(Store::load(&f).last_profile(), Some("kimi"));
}

#[test]
fn old_file_without_last_profile_still_loads() {
    // 已经在用 dct 的人，projects.json 里没有这个字段
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("projects.json");
    std::fs::write(&f, r#"{"recent":["/a"]}"#).unwrap();
    let s = Store::load(&f);
    assert_eq!(s.list(), vec!["/a".to_string()]);
    assert_eq!(s.last_profile(), None);
}
```

新建 `tests/profiles_flow.rs`（照 `tests/projects_flow.rs` 的骨架起守护进程）：

```rust
//! Profiles / 密钥 / 上次用的 agent 走一遍真 socket。

use dct::profile::ProfileStatus;
use dct::proto::{Request, Response};

mod common;

#[test]
fn profiles_returns_entries_with_labels_and_status() {
    let h = common::start_daemon();
    let mut c = h.client();

    let Response::Profiles { entries, warning } = c.call(Request::Profiles).unwrap() else {
        panic!("应当返回 Profiles");
    };
    assert!(warning.is_none(), "干净环境不该有告警");
    assert_eq!(entries.len(), 9);
    assert_eq!(entries[0].name, "claude");
    assert_eq!(entries[0].label, "Claude", "要带中文 label");
    let shell = entries.iter().find(|e| e.name == "shell").unwrap();
    assert_eq!(shell.status, ProfileStatus::Ready, "/bin/zsh 一定在");
    let kimi = entries.iter().find(|e| e.name == "kimi").unwrap();
    assert!(
        kimi.secret.is_some(),
        "需要密钥的条目要把 hint / url 一起带过来，UI 才画得出输入界面"
    );
}

#[test]
fn set_secret_flips_kimi_off_needs_secret() {
    let h = common::start_daemon();
    let mut c = h.client();

    c.call(Request::SetSecret {
        profile: "kimi".into(),
        value: "sk-test".into(),
    })
    .unwrap();

    let Response::Profiles { entries, .. } = c.call(Request::Profiles).unwrap() else {
        panic!()
    };
    let kimi = entries.iter().find(|e| e.name == "kimi").unwrap();
    assert_ne!(
        kimi.status,
        ProfileStatus::NeedsSecret,
        "填了密钥就不该再报缺密钥"
    );
}

#[test]
fn delete_secret_puts_it_back() {
    let h = common::start_daemon();
    let mut c = h.client();
    c.call(Request::SetSecret {
        profile: "kimi".into(),
        value: "sk-test".into(),
    })
    .unwrap();
    c.call(Request::DeleteSecret {
        profile: "kimi".into(),
    })
    .unwrap();

    let Response::Profiles { entries, .. } = c.call(Request::Profiles).unwrap() else {
        panic!()
    };
    let kimi = entries.iter().find(|e| e.name == "kimi").unwrap();
    // claude 装没装取决于跑测试的机器，两种都算对——重点是密钥没了
    assert!(matches!(
        kimi.status,
        ProfileStatus::NeedsSecret | ProfileStatus::NeedsDependency { .. }
    ));
}

#[test]
fn create_with_remember_records_the_profile() {
    let h = common::start_daemon();
    let mut c = h.client();
    let dir = h.git_repo("proj");

    c.call(Request::Create {
        dir: dir.display().to_string(),
        profile: "shell".into(),
        remember: true,
    })
    .unwrap();

    assert!(matches!(
        c.call(Request::LastProfile).unwrap(),
        Response::LastProfile(Some(ref n)) if n == "shell"
    ));
}

#[test]
fn create_without_remember_does_not_record() {
    // 「帮你装 CLI」开的那个 shell 会话不能变成「上次用的 agent」——
    // 否则用户下次按 n 会直接掉进一个命令行。
    let h = common::start_daemon();
    let mut c = h.client();
    let dir = h.git_repo("proj");

    c.call(Request::Create {
        dir: dir.display().to_string(),
        profile: "shell".into(),
        remember: false,
    })
    .unwrap();

    assert!(matches!(
        c.call(Request::LastProfile).unwrap(),
        Response::LastProfile(None)
    ));
}
```

`mod common` 里的 `start_daemon()` / `client()` / `git_repo()`：`tests/projects_flow.rs` 里已经有等价的起守护进程代码，把它抽到 `tests/common/mod.rs` 供两个文件共用。抽的时候保持 `projects_flow.rs` 的行为不变，它的测试必须照样过。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test`
Expected: 编译失败

- [ ] **Step 3: 改 proto.rs**

```rust
use crate::profile::ProfileStatus;

/// 需要密钥时，UI 画输入界面要用的东西。
///
/// 只带**已经取好语言**的字符串，不把 `LocalizedText` 送过线：
/// 组句发生在哪一侧必须一致（见设计文档「与 i18n 的关系」）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretPrompt {
    pub hint: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPrompt {
    pub command: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub name: String,
    pub label: String,
    pub note: String,
    pub status: ProfileStatus,
    pub secret: Option<SecretPrompt>,
    pub install: Option<InstallPrompt>,
}
```

`Request` 加：

```rust
    Create { dir: String, profile: String, remember: bool },
    SetSecret { profile: String, value: String },
    DeleteSecret { profile: String },
    LastProfile,
```

`Response` 改 / 加：

```rust
    Profiles {
        entries: Vec<ProfileEntry>,
        /// 密钥文件读不了、自定义 profile 写错了之类。UI 顶部红字。
        warning: Option<String>,
    },
    LastProfile(Option<String>),
```

- [ ] **Step 4: 改 projects.rs**

`Disk` 加字段，`Store` 加字段与两个方法：

```rust
#[derive(Default, Serialize, Deserialize)]
struct Disk {
    #[serde(default)]
    recent: Vec<String>,
    /// 上次开会话用的 agent。`n` 键直连它。
    #[serde(default)]
    last_profile: Option<String>,
}
```

```rust
    pub fn last_profile(&self) -> Option<&str> {
        self.last_profile.as_deref()
    }

    pub fn set_last_profile(&mut self, name: &str) {
        self.last_profile = Some(name.to_string());
        self.save();
    }
```

`load()` 和 `save()` 里把新字段带上。

- [ ] **Step 5: 改 daemon.rs**

`run_with_manager` 里，`store` 旁边加密钥仓：

```rust
    let secrets = Arc::new(Mutex::new(SecretStore::load(&secrets_path_for_socket(socket))));
    let profiles_dir = profiles_dir_for_socket(socket);
```

两者都要传进 `serve` / `handle`。`handle` 的新分支：

```rust
        Request::Profiles => {
            let (all, mut warnings) = all_profiles(profiles_dir);
            let sec = recover(secrets.lock());
            if let Some(e) = sec.load_error() {
                // 密钥文件读不了要顶到界面上。静默的话用户会以为密钥丢了，
                // 而且这时候所有写入都被拒，他改什么都没反应。
                warnings.insert(0, format!("密钥文件读不了：{e}"));
            }
            let entries = all
                .iter()
                .map(|p| ProfileEntry {
                    name: p.name.clone(),
                    label: p.display_label(Lang::Zh),
                    note: p.display_note(Lang::Zh),
                    status: status_of(
                        p,
                        &all,
                        sec.get(&p.name).is_some(),
                        &command_exists,
                        Lang::Zh,
                    ),
                    secret: p.secret.as_ref().map(|s| SecretPrompt {
                        hint: s.hint.get(Lang::Zh).unwrap_or("").to_string(),
                        url: s.url.clone(),
                    }),
                    install: p.install.as_ref().map(|i| InstallPrompt {
                        command: i.command.clone(),
                        note: i.note.get(Lang::Zh).unwrap_or("").to_string(),
                    }),
                })
                .collect();
            Ok(Response::Profiles {
                entries,
                warning: if warnings.is_empty() {
                    None
                } else {
                    Some(warnings.join("；"))
                },
            })
        }
        Request::Create { dir, profile, remember } => {
            let dir = PathBuf::from(dir);
            let sec = recover(secrets.lock());
            let r = mgr
                .create(&dir, &profile, &sec)
                .map(|id| Response::Created { id });
            drop(sec);
            if r.is_ok() {
                let mut st = recover(store.lock());
                st.touch(&dir);
                // remember=false 是「帮你装 CLI」那条路径：它开的 shell 会话
                // 不是用户选的 agent，记了下次按 n 会掉进命令行
                if remember {
                    st.set_last_profile(&profile);
                }
            }
            r
        }
        Request::SetSecret { profile, value } => recover(secrets.lock())
            .set(&profile, &value)
            .map(|_| Response::Ok),
        Request::DeleteSecret { profile } => recover(secrets.lock())
            .remove(&profile)
            .map(|_| Response::Ok),
        Request::LastProfile => Ok(Response::LastProfile(
            recover(store.lock()).last_profile().map(str::to_string),
        )),
```

`SessionManager::resolve_profile` 也要认磁盘 profile。最省事的做法是在 `create` 之前，由 daemon 把磁盘 profile 用现有的 `register_profile` 灌进去；但那会在 manager 里越攒越多。改成给 `create` 多传一个 `profiles: &[Profile]`，`resolve_profile` 先查这个切片、再查 `extra_profiles`（测试入口）、最后查内置。

- [ ] **Step 6: 改调用点**

`src/ui.rs:315` 的 `Response::Profiles(p)` 改成解构新形状（这一步只求编译过，UI 的正式改造在 Task 10）；`src/ui.rs:385` 的 `Request::Create` 加 `remember: true`。四个集成测试的 `Create` 同样加 `remember: true`。

- [ ] **Step 7: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 8: 提交**

```bash
~/.cargo/bin/cargo fmt
git add -A
git commit -m "feat: 协议带上 profile 状态与密钥提示；记住上次用的 agent

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: 密钥验证

**Files:**
- Create: `src/verify.rs`
- Modify: `Cargo.toml`（加 ureq）、`src/lib.rs`、`src/proto.rs`、`src/daemon.rs`
- Test: `src/verify.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 1 的 `VerifySpec`、Task 8 的协议
- Produces:
  ```rust
  pub enum VerifyOutcome { Ok, BadKey, Unreachable }
  pub fn verify_with(url: &str, key: &str, send: &dyn Fn(&str, &str) -> Result<u16, String>) -> VerifyOutcome;
  pub fn send_probe(url: &str, key: &str) -> Result<u16, String>;
  Request::VerifySecret { profile: String, value: String }
  Response::Verify(VerifyOutcome)
  ```

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthorized_means_bad_key() {
        for code in [401, 403] {
            assert_eq!(
                verify_with("u", "k", &|_, _| Ok(code)),
                VerifyOutcome::BadKey,
                "{code} 是「这个 key 不行」"
            );
        }
    }

    #[test]
    fn network_failure_is_reported_as_unreachable() {
        assert_eq!(
            verify_with("u", "k", &|_, _| Err("connection refused".into())),
            VerifyOutcome::Unreachable
        );
    }

    #[test]
    fn anything_else_passes() {
        // 刻意放行。各家 Anthropic 兼容端点行为不一，不能因为返回码奇怪
        // 就把用户拦在门外——验证的职责是抓住「key 明显是错的」，不是当网关。
        for code in [200, 400, 404, 429, 500, 502] {
            assert_eq!(
                verify_with("u", "k", &|_, _| Ok(code)),
                VerifyOutcome::Ok,
                "{code} 不该拦人"
            );
        }
    }

    #[test]
    fn the_key_reaches_the_transport() {
        let seen = std::cell::RefCell::new(String::new());
        verify_with("https://x/v1/messages", "sk-abc", &|url, key| {
            assert_eq!(url, "https://x/v1/messages");
            *seen.borrow_mut() = key.to_string();
            Ok(200)
        });
        assert_eq!(*seen.borrow(), "sk-abc");
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib verify`
Expected: FAIL，模块不存在

- [ ] **Step 3: 加依赖**

`Cargo.toml`：

```toml
ureq = { version = "2", default-features = false, features = ["tls", "json"] }
```

- [ ] **Step 4: 实现**

`src/verify.rs`：

```rust
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 探测请求的超时。**必须小于 `client::READ_TIMEOUT`（5 秒）**：
/// 守护进程在这里等多久，界面那条连接就等多久，超过 5 秒界面会判定
/// 连接错位并丢弃重连，用户看到的是「连不上守护进程」而不是验证结果。
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifyOutcome {
    Ok,
    BadKey,
    Unreachable,
}

/// 判定逻辑本身，传输层由调用方注入——测试才能覆盖 401 / 网络错 / 奇怪返回码，
/// 而不用真打网络。
pub fn verify_with(
    url: &str,
    key: &str,
    send: &dyn Fn(&str, &str) -> Result<u16, String>,
) -> VerifyOutcome {
    match send(url, key) {
        Err(_) => VerifyOutcome::Unreachable,
        Ok(401) | Ok(403) => VerifyOutcome::BadKey,
        // 其余一律放行，见测试 `anything_else_passes` 的注释
        Ok(_) => VerifyOutcome::Ok,
    }
}

/// 真的传输层。发一个最小的 Anthropic 风格请求，只看状态码，不读 body。
///
/// `model` 随便填一个：我们不在乎它认不认这个模型（那会返回 400，属于放行），
/// 只在乎它认不认这个 key。
pub fn send_probe(url: &str, key: &str) -> Result<u16, String> {
    let body = serde_json::json!({
        "model": "probe",
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "hi"}],
    });
    let resp = ureq::AgentBuilder::new()
        .timeout(PROBE_TIMEOUT)
        .build()
        .post(url)
        .set("content-type", "application/json")
        .set("x-api-key", key)
        .set("authorization", &format!("Bearer {key}"))
        .set("anthropic-version", "2023-06-01")
        .send_json(body);

    match resp {
        Ok(r) => Ok(r.status()),
        // ureq 把 4xx/5xx 也当 Err，得挑出来——它们是有效的状态码，不是网络故障
        Err(ureq::Error::Status(code, _)) => Ok(code),
        Err(e) => Err(format!("{e}")),
    }
}
```

两个 auth 头都发：各家兼容端点认的不一样，多发一个头的代价远小于「明明 key 是对的却报无效」。

`src/lib.rs` 加 `pub mod verify;`。

`src/proto.rs`：

```rust
    VerifySecret { profile: String, value: String },
    // Response 侧
    Verify(crate::verify::VerifyOutcome),
```

`src/daemon.rs` 的 `handle`：

```rust
        Request::VerifySecret { profile, value } => {
            let (all, _) = all_profiles(profiles_dir);
            let spec = all
                .iter()
                .find(|p| p.name == profile)
                .and_then(|p| p.secret.as_ref())
                .and_then(|s| s.verify.as_ref());
            match spec {
                // 没声明 verify 的 profile 直接放行，不是错误
                None => Ok(Response::Verify(VerifyOutcome::Ok)),
                Some(v) => Ok(Response::Verify(verify_with(&v.url, &value, &send_probe))),
            }
        }
```

- [ ] **Step 5: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 6: 提交**

```bash
~/.cargo/bin/cargo fmt
git add -A
git commit -m "feat: 密钥存盘前先探一下端点，401/403 当场拦住

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: 选择器改造

**Files:**
- Modify: `src/ui.rs:66-80`（`View`）、`src/ui.rs:314-318`（`n` 键取列表）、`src/ui.rs:376-405`（`PickProfile` 按键）、`src/ui.rs:920-935`（渲染）、`src/ui.rs:1034`（`idle_help`）
- Test: `src/ui.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 8 的 `ProfileEntry` / `ProfileStatus`
- Produces:
  ```rust
  View::PickProfile { entries: Vec<ProfileEntry>, state: ListState, warning: Option<String> }
  /// 纯函数，好单测：按下第 i 项时该干什么
  pub enum PickAction {
      Start(String),                      // 建会话，profile 名
      AskSecret(usize),                   // 切到填密钥视图，条目下标
      Install { profile: String, command: Vec<String> },
      Blocked(String),                    // 底栏说一句话，不切视图
  }
  pub fn pick_action(e: &ProfileEntry) -> PickAction;
  ```

- [ ] **Step 1: 写失败的测试**

```rust
fn entry(name: &str, status: ProfileStatus) -> ProfileEntry {
    ProfileEntry {
        name: name.into(),
        label: name.into(),
        note: String::new(),
        status,
        secret: None,
        install: None,
    }
}

#[test]
fn ready_entry_starts_a_session() {
    let e = entry("claude", ProfileStatus::Ready);
    assert!(matches!(pick_action(&e), PickAction::Start(n) if n == "claude"));
}

#[test]
fn needs_secret_entry_opens_the_secret_view() {
    let e = entry("kimi", ProfileStatus::NeedsSecret);
    assert!(matches!(pick_action(&e), PickAction::AskSecret(_)));
}

#[test]
fn not_installed_with_an_installer_offers_to_install() {
    let mut e = entry("codex", ProfileStatus::NotInstalled { command: "codex".into() });
    e.install = Some(InstallPrompt {
        command: vec!["npm".into(), "i".into(), "-g".into(), "@openai/codex".into()],
        note: String::new(),
    });
    match pick_action(&e) {
        PickAction::Install { profile, command } => {
            assert_eq!(profile, "codex");
            assert_eq!(command[0], "npm");
        }
        other => panic!("有安装命令就该给一条路，得到 {other:?}"),
    }
}

#[test]
fn not_installed_without_an_installer_just_explains() {
    let e = entry("weird", ProfileStatus::NotInstalled { command: "weird".into() });
    match pick_action(&e) {
        PickAction::Blocked(msg) => {
            assert!(msg.contains("weird"), "要说清是哪个命令找不到：{msg}");
            assert!(!msg.contains("PATH"), "别对非程序员说 PATH");
        }
        other => panic!("得到 {other:?}"),
    }
}

#[test]
fn missing_dependency_names_what_to_install_first() {
    let e = entry("kimi", ProfileStatus::NeedsDependency { label: "Claude".into() });
    match pick_action(&e) {
        PickAction::Blocked(msg) => {
            assert!(msg.contains("Claude"), "要点名先装什么：{msg}");
        }
        other => panic!("得到 {other:?}"),
    }
}

#[test]
fn digit_keys_still_pick_the_first_nine() {
    // 数字保留是因为快；置灰项也占编号——编号跳号比编号漂移更难受
    assert_eq!(digit_index('1'), Some(0));
    assert_eq!(digit_index('9'), Some(8));
    assert_eq!(digit_index('0'), None);
    assert_eq!(digit_index('a'), None);
}

#[test]
fn picker_help_mentions_both_ways_to_choose() {
    let help = idle_help(&View::PickProfile {
        entries: vec![],
        state: ListState::default(),
        warning: None,
    });
    assert!(help.contains("↑↓"));
    assert!(help.contains("数字"));
}

#[test]
fn back_one_level_from_picker_goes_to_board() {
    assert!(matches!(
        back_one_level(View::PickProfile {
            entries: vec![],
            state: ListState::default(),
            warning: None,
        }),
        Some(View::Board)
    ));
}
```

`idle_help` 目前返回 `&'static str`，`PickProfile` 那条改成常量串即可，签名不用动。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib ui`
Expected: FAIL，`pick_action` 不存在

- [ ] **Step 3: 实现**

`View`：

```rust
    PickProfile {
        entries: Vec<ProfileEntry>,
        state: ListState,
        /// 密钥文件读不了、自定义 profile 写错了。顶部红字。
        warning: Option<String>,
    },
```

纯函数（放在 `back_one_level` 附近，和它一样是为了能单测才抽出来的）：

```rust
#[derive(Debug)]
pub enum PickAction {
    Start(String),
    AskSecret(usize),
    Install {
        profile: String,
        command: Vec<String>,
    },
    Blocked(String),
}

/// 按下某一项时该干什么。抽成纯函数是为了能单测——`run()` 的按键循环
/// 要连真 socket，测不了（同 `back_one_level`）。
pub fn pick_action(e: &ProfileEntry) -> PickAction {
    match &e.status {
        ProfileStatus::Ready => PickAction::Start(e.name.clone()),
        ProfileStatus::NeedsSecret => PickAction::AskSecret(0),
        ProfileStatus::NeedsDependency { label } => {
            PickAction::Blocked(format!("要先装 {label} 才能用 {}", e.label))
        }
        ProfileStatus::NotInstalled { command } => match &e.install {
            Some(i) => PickAction::Install {
                profile: e.name.clone(),
                command: i.command.clone(),
            },
            None => PickAction::Blocked(format!("本机没有找到 {command}")),
        },
    }
}

/// '1'..'9' → 0..8。'0' 不算——第 10 项要用 ↑↓ 选。
pub fn digit_index(c: char) -> Option<usize> {
    match c {
        '1'..='9' => Some(c as usize - '1' as usize),
        _ => None,
    }
}
```

`AskSecret(usize)` 的下标由调用方在按键分支里填成实际选中的行号；`pick_action` 里给 0 是占位，调用方一定会覆盖。**这个约定要写在 `PickAction::AskSecret` 的注释里**，否则下一个人会以为它有意义。

按键分支（替换 `src/ui.rs:376-405`）：

```rust
                View::PickProfile {
                    entries,
                    mut state,
                    warning,
                } => {
                    let chosen: Option<usize> = match key.code {
                        KeyCode::Esc => {
                            view = View::Board;
                            None
                        }
                        KeyCode::Down | KeyCode::Up => {
                            let d = if key.code == KeyCode::Down { 1 } else { -1 };
                            move_sel_n(&mut state, entries.len(), d);
                            view = View::PickProfile { entries, state, warning };
                            continue;
                        }
                        KeyCode::Enter => state.selected(),
                        KeyCode::Char(c) => digit_index(c).filter(|i| *i < entries.len()),
                        _ => None,
                    };
                    // ...（下面按 chosen 走 pick_action 的四个分支）
                }
```

四个分支的落点（`AskSecret` 那支在 Task 11 之前先写成 `Blocked("还没做")`，Task 11 再补上真视图）：

```rust
                    match chosen.map(|i| (i, pick_action(&entries[i]))) {
                        None => {}
                        Some((_, PickAction::Start(name))) => {
                            match client.call(Request::Create {
                                dir: current_dir.display().to_string(),
                                profile: name,
                                remember: true,
                            }) {
                                Ok(Response::Created { id }) => {
                                    view = View::Attached(id);
                                    need_sessions = true;
                                }
                                Ok(Response::Error(e)) => {
                                    message = Msg::err(e);
                                    view = View::PickProfile { entries, state, warning };
                                }
                                _ => {
                                    message = Msg::err("创建失败".into());
                                    view = View::PickProfile { entries, state, warning };
                                }
                            }
                        }
                        Some((i, PickAction::AskSecret(_))) => {
                            // pick_action 里那个下标是占位，真下标只有这里知道
                            let e = &entries[i];
                            view = View::EnterSecret {
                                profile: e.name.clone(),
                                label: e.label.clone(),
                                prompt: e.secret.clone().unwrap_or(SecretPrompt {
                                    hint: String::new(),
                                    url: None,
                                }),
                                buf: String::new(),
                                phase: SecretPhase::Typing,
                            };
                        }
                        Some((_, PickAction::Install { profile, command })) => {
                            // 用命令行会话跑安装命令。remember: false ——
                            // 这不是用户选的 agent，记了下次按 n 会掉进命令行。
                            match client.call(Request::Create {
                                dir: current_dir.display().to_string(),
                                profile: "shell".into(),
                                remember: false,
                            }) {
                                Ok(Response::Created { id }) => {
                                    let line = format!("{}\n", command.join(" "));
                                    let _ = client.call(Request::Input { id, text: line });
                                    message = format!("正在安装 {profile}，装完按 Ctrl+Q 回看板再按 N").into();
                                    view = View::Attached(id);
                                    need_sessions = true;
                                }
                                _ => {
                                    message = Msg::err("开不了安装窗口".into());
                                    view = View::PickProfile { entries, state, warning };
                                }
                            }
                        }
                        Some((_, PickAction::Blocked(msg))) => {
                            message = Msg::err(msg);
                            view = View::PickProfile { entries, state, warning };
                        }
                    }
```

`SecretPrompt` 要 `#[derive(Clone)]`（Task 8 已经加了）。

⚠️ `continue` 会跳过循环末尾的 `message_after_transition`。上面 `Down/Up` 那支用 `continue` 是为了少写一遍重建 `View` 的样板，但它同时也跳过了消息清理——`PickProject` 的对应分支没有用 `continue`，是逐支重建 `View` 的。**照 `PickProject` 的写法逐支重建，不要用 `continue`**，否则光标一动消息就永远清不掉。

渲染（替换 `src/ui.rs:920-935`）：

```rust
        View::PickProfile {
            entries,
            state,
            warning,
        } => {
            let items: Vec<ListItem> = entries
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let num = if i < 9 {
                        format!("{}. ", i + 1)
                    } else {
                        "   ".to_string()
                    };
                    let reason = match &e.status {
                        ProfileStatus::Ready => String::new(),
                        ProfileStatus::NeedsSecret => "（未填密钥）".into(),
                        ProfileStatus::NeedsDependency { label } => {
                            format!("（需要先装 {label}）")
                        }
                        ProfileStatus::NotInstalled { .. } => "（未安装）".into(),
                    };
                    // 不可用的整行压暗，不只是把原因压暗——用户是先看名字再看原因的，
                    // 名字亮着会让他先以为能用
                    let base = if matches!(e.status, ProfileStatus::Ready) {
                        Style::default()
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(num, base),
                        Span::styled(format!("{:<14}", truncate(&e.label, 14)), base),
                        Span::styled(
                            format!("{:<26}", truncate(&e.note, 26)),
                            base.fg(Color::DarkGray),
                        ),
                        Span::styled(reason, base.fg(Color::DarkGray)),
                    ]))
                })
                .collect();

            let title = match warning {
                Some(w) => format!("选 agent —— {w}"),
                None => "选 agent".to_string(),
            };
            let border = if warning.is_some() {
                Style::default().fg(Color::Red)
            } else {
                border_style
            };
            let mut s = state.clone();
            f.render_stateful_widget(
                List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(border)
                            .title(title),
                    )
                    .highlight_symbol("▶ "),
                chunks[0],
                &mut s,
            );
        }
```

`idle_help` 的 `PickProfile` 一行改成 `"↑↓ 选  Enter 确认  或直接按数字  Esc 取消"`。

`n` 键取列表处（`src/ui.rs:314-318`）改成解构 `Response::Profiles { entries, warning }` 并建新 `View`，`state.select(Some(0))`。

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 5: 手动看一眼**

```bash
~/.cargo/bin/cargo build && ./target/debug/dct
```
按 `n`，确认九行都在、置灰项带原因、↑↓ 和数字都能选、Esc 能退。

- [ ] **Step 6: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/ui.rs
git commit -m "feat: agent 选择器列出全部九个，置灰项带原因，↑↓ 与数字都能选

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: 填密钥界面

**Files:**
- Modify: `src/ui.rs`（`View`、按键循环、粘贴分支、渲染、`escape_hint`、`back_one_level`）
- Test: `src/ui.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 9 的 `VerifyOutcome`、Task 10 的 `PickAction::AskSecret`
- Produces:
  ```rust
  View::EnterSecret {
      profile: String,
      label: String,
      prompt: SecretPrompt,
      buf: String,
      phase: SecretPhase,
  }
  pub enum SecretPhase { Typing, Verifying, Failed(String) }
  pub fn clean_secret(s: &str) -> String;
  pub fn verify_message(o: VerifyOutcome) -> Option<String>;  // None = 放行
  ```

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn paste_is_trimmed() {
    assert_eq!(clean_secret("  sk-abc\n"), "sk-abc");
}

#[test]
fn paste_strips_surrounding_quotes() {
    assert_eq!(clean_secret("\"sk-abc\""), "sk-abc");
    assert_eq!(clean_secret("'sk-abc'"), "sk-abc");
}

#[test]
fn paste_strips_bearer_prefix() {
    // 从接口文档里整段拷贝经常带上它
    assert_eq!(clean_secret("Bearer sk-abc"), "sk-abc");
    assert_eq!(clean_secret("\"Bearer sk-abc\"\n"), "sk-abc");
}

#[test]
fn paste_leaves_a_normal_key_alone() {
    assert_eq!(clean_secret("sk-abc123"), "sk-abc123");
}

#[test]
fn bad_key_gets_a_human_message() {
    let m = verify_message(VerifyOutcome::BadKey).unwrap();
    assert!(m.contains("密钥"));
    assert!(!m.contains("401"), "别把状态码甩给用户：{m}");
}

#[test]
fn unreachable_blames_the_network_not_the_key() {
    let m = verify_message(VerifyOutcome::Unreachable).unwrap();
    assert!(m.contains("网络"), "连不上要说是网络，不能让用户去怀疑密钥：{m}");
}

#[test]
fn ok_has_no_message() {
    assert!(verify_message(VerifyOutcome::Ok).is_none());
}

#[test]
fn secret_view_escapes_back_to_the_picker() {
    // 回选择器而不是回看板：用户可能只是选错了 agent
    let back = back_one_level(View::EnterSecret {
        profile: "kimi".into(),
        label: "Kimi".into(),
        prompt: SecretPrompt { hint: String::new(), url: None },
        buf: String::new(),
        phase: SecretPhase::Typing,
    });
    assert!(matches!(back, Some(View::PickProfile { .. })));
}

#[test]
fn secret_view_escape_hint_says_back_to_the_list() {
    let h = escape_hint(&View::EnterSecret {
        profile: "kimi".into(),
        label: "Kimi".into(),
        prompt: SecretPrompt { hint: String::new(), url: None },
        buf: String::new(),
        phase: SecretPhase::Typing,
    });
    assert!(h.contains("列表"), "底栏说什么就得真能做到什么：{h}");
}
```

⚠️ `back_one_level` 返回 `View::PickProfile` 需要一份条目列表，而它是纯函数拿不到。做法：返回 `View::PickProfile { entries: vec![], state: ListState::default(), warning: None }`，主循环在 `Ctrl+Q` 之后发现是空列表就重新拉一次 `Request::Profiles` 填上。这个约定要写进 `back_one_level` 的注释。

`ESCAPE_HINT_COLS` 是写死的 13 列（`src/ui.rs:847`）。「Ctrl+Q 回列表」正好 13 列，新文案别超。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib ui`
Expected: FAIL

- [ ] **Step 3: 实现纯函数**

```rust
/// 粘进来的密钥清洗一遍。用户从网页或接口文档里拷贝，经常带上引号、
/// `Bearer ` 前缀和尾随换行——让他自己发现并删掉是不现实的。
pub fn clean_secret(s: &str) -> String {
    let t = s.trim();
    let t = t.strip_prefix('"').unwrap_or(t);
    let t = t.strip_suffix('"').unwrap_or(t);
    let t = t.strip_prefix('\'').unwrap_or(t);
    let t = t.strip_suffix('\'').unwrap_or(t);
    let t = t.trim();
    t.strip_prefix("Bearer ").unwrap_or(t).trim().to_string()
}

/// 验证结果给用户看的话。`None` 表示放行。
pub fn verify_message(o: VerifyOutcome) -> Option<String> {
    match o {
        VerifyOutcome::Ok => None,
        VerifyOutcome::BadKey => Some("这个密钥用不了，可能是复制的时候少了一段".into()),
        VerifyOutcome::Unreachable => Some("连不上服务器，检查一下网络".into()),
    }
}
```

- [ ] **Step 4: 接上视图与后台验证**

`View` 加：

```rust
    EnterSecret {
        profile: String,
        label: String,
        prompt: SecretPrompt,
        buf: String,
        phase: SecretPhase,
    },
```

```rust
#[derive(Clone)]
pub enum SecretPhase {
    Typing,
    Verifying,
    Failed(String),
}
```

⚠️ **`View` 要 `Clone`（`run()` 里 `match view.clone()`），所以 `mpsc::Receiver` 不能放进 `View`。** 在 `run()` 的局部变量区另起一个：

```rust
    // 密钥验证是网络调用，不能在按键循环里直接跑——会话视图 16ms 一刷，
    // 一次阻塞就是整个界面冻住。丢给后台线程，主循环每轮 try_recv。
    // 放在 View 外面是因为 View 要 Clone，而 Receiver 不能 Clone。
    let mut verify_rx: Option<std::sync::mpsc::Receiver<VerifyOutcome>> = None;
```

按 Enter 时：

```rust
                            let (tx, rx) = std::sync::mpsc::channel();
                            let sock = socket.to_path_buf();
                            let p = profile.clone();
                            let v = buf.clone();
                            std::thread::spawn(move || {
                                // 另开一条连接：主循环那条还要继续画界面
                                let outcome = Client::connect(&sock)
                                    .and_then(|mut c| {
                                        c.call(Request::VerifySecret { profile: p, value: v })
                                    })
                                    .map(|r| match r {
                                        Response::Verify(o) => o,
                                        _ => VerifyOutcome::Unreachable,
                                    })
                                    .unwrap_or(VerifyOutcome::Unreachable);
                                let _ = tx.send(outcome);
                            });
                            verify_rx = Some(rx);
                            phase = SecretPhase::Verifying;
```

主循环开头（`term.draw` 之前）收结果：

```rust
        if let Some(rx) = &verify_rx {
            if let Ok(outcome) = rx.try_recv() {
                verify_rx = None;
                // 通过就存盘 + 开会话 + 进去；不通过就留在原地显示原因
            }
        }
```

`Verifying` 期间不接受输入（Enter / 字符都忽略），只有 Esc 能退——退出时把 `verify_rx` 置 `None`，迟到的结果直接丢掉。

`Ctrl+O` 打开申领页：

```rust
                        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // 用 Ctrl+O 不用 o：o 得留给密钥输入
                            if let Some(url) = &prompt.url {
                                let _ = std::process::Command::new("open").arg(url).spawn();
                            }
                        }
```

粘贴分支（`src/ui.rs:258-275`）加一支：

```rust
                View::EnterSecret { buf, .. } => buf.push_str(&clean_secret(&text)),
```

渲染：

```rust
        View::EnterSecret {
            label,
            prompt,
            buf,
            phase,
            ..
        } => {
            let mut lines: Vec<Line> = Vec::new();
            if !prompt.hint.is_empty() {
                lines.push(Line::from(Span::styled(
                    prompt.hint.clone(),
                    Style::default().fg(Color::DarkGray),
                )));
                lines.push(Line::from(""));
            }
            // 显示成圆点：密钥不该以明文停在屏幕上，用户可能在录屏或在办公室
            lines.push(Line::from(format!("{}▌", "•".repeat(buf.chars().count()))));
            lines.push(Line::from(""));
            match phase {
                SecretPhase::Typing => {}
                SecretPhase::Verifying => lines.push(Line::from(Span::styled(
                    "正在验证…",
                    Style::default().fg(Color::Cyan),
                ))),
                SecretPhase::Failed(m) => lines.push(Line::from(Span::styled(
                    m.clone(),
                    Style::default().fg(Color::Red),
                ))),
            }
            if prompt.url.is_some() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Ctrl+O 打开申领页面",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            f.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style)
                        .title(format!("填 {label} 的密钥（Enter 确认，Esc 返回列表）")),
                ),
                chunks[0],
            );
        }
```

`escape_hint` 加一支：`View::EnterSecret { .. } => "Ctrl+Q 回列表"`。

- [ ] **Step 5: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 6: 手动走一遍**

```bash
~/.cargo/bin/cargo build && ./target/debug/dct
```
`n` → 选 Kimi → 粘一个假 key → 回车。确认「正在验证…」出现、界面**不冻**（这期间还能按 Esc）、最后红字说密钥用不了。

- [ ] **Step 7: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/ui.rs
git commit -m "feat: 就地填密钥，粘贴自动清洗，存盘前后台验证不冻界面

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: n 直连上次的 agent，N 才进选择器

**Files:**
- Modify: `src/ui.rs:310-318`（看板按键）、`src/ui.rs:1040`（看板 `idle_help`）
- Test: `src/ui.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 8 的 `Request::LastProfile`、Task 10 的 `ProfileEntry`
- Produces: `pub fn quick_start_target(last: Option<&str>, entries: &[ProfileEntry]) -> Option<String>`

- [ ] **Step 1: 写失败的测试**

用到 Task 10 在同一个 `mod tests` 里建的 `entry(name, status)` 辅助。它不在的话先补上：

```rust
fn entry(name: &str, status: ProfileStatus) -> ProfileEntry {
    ProfileEntry {
        name: name.into(),
        label: name.into(),
        note: String::new(),
        status,
        secret: None,
        install: None,
    }
}
```

```rust
#[test]
fn quick_start_uses_the_last_agent_when_it_is_ready() {
    let entries = vec![
        entry("claude", ProfileStatus::Ready),
        entry("kimi", ProfileStatus::Ready),
    ];
    assert_eq!(
        quick_start_target(Some("kimi"), &entries),
        Some("kimi".to_string())
    );
}

#[test]
fn quick_start_falls_back_when_the_last_agent_is_no_longer_usable() {
    // 密钥被删了、CLI 被卸了。直接开会话只会得到一个起不来的窗口，
    // 退回选择器让用户重新挑。
    let entries = vec![
        entry("claude", ProfileStatus::Ready),
        entry("kimi", ProfileStatus::NeedsSecret),
    ];
    assert_eq!(quick_start_target(Some("kimi"), &entries), None);
}

#[test]
fn quick_start_falls_back_when_the_last_agent_is_gone() {
    // 用户删掉了自己那个自定义 profile
    let entries = vec![entry("claude", ProfileStatus::Ready)];
    assert_eq!(quick_start_target(Some("mine"), &entries), None);
}

#[test]
fn quick_start_falls_back_on_first_ever_run() {
    let entries = vec![entry("claude", ProfileStatus::Ready)];
    assert_eq!(quick_start_target(None, &entries), None);
}

#[test]
fn board_help_mentions_both_n_and_capital_n() {
    let help = idle_help(&View::Board);
    assert!(help.contains("n 新建"));
    assert!(help.contains("N 换 agent"));
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib ui`
Expected: FAIL

- [ ] **Step 3: 实现**

```rust
/// `n` 该直接开哪个 agent。`None` = 没得直开，进选择器。
///
/// 目标用户是非程序员：让他每次在九个 agent 里挑一个是设计失败——他不知道区别。
/// 日常路径压成一个按键，想换的人按 N。
pub fn quick_start_target(last: Option<&str>, entries: &[ProfileEntry]) -> Option<String> {
    let last = last?;
    entries
        .iter()
        .find(|e| e.name == last && e.status == ProfileStatus::Ready)
        .map(|e| e.name.clone())
}
```

看板按键分支：

```rust
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        let Ok(Response::Profiles { entries, warning }) =
                            client.call(Request::Profiles)
                        else {
                            message = Msg::err("拿不到 agent 列表".into());
                            continue;
                        };
                        let last = match client.call(Request::LastProfile) {
                            Ok(Response::LastProfile(l)) => l,
                            _ => None,
                        };
                        // 小写 n 直连上次那个；大写 N 一定进选择器
                        let quick = if key.code == KeyCode::Char('n') {
                            quick_start_target(last.as_deref(), &entries)
                        } else {
                            None
                        };
                        match quick {
                            Some(name) => { /* Create + Attached，同 PickAction::Start */ }
                            None => {
                                let mut state = ListState::default();
                                state.select(Some(0));
                                view = View::PickProfile { entries, state, warning };
                            }
                        }
                    }
```

⚠️ `continue` 会跳过循环末尾的 `message_after_transition`。上面那个 `else` 分支里的 `continue` 是在**没切视图**的情况下设消息，跳过清理正好是我们要的（消息该留着），但要在注释里写明这不是疏忽。

看板 `idle_help` 改成：
`"n 新建  N 换 agent  p 换项目  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动"`

原来的 `q 退出` 由 `escape_hint` 单独占左段（`src/ui.rs:835`），不在这一行里。

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 5: 手动走一遍**

起 dct，`N` 选 Claude 建一个会话，Ctrl+Q 回看板，按 `n`——应当**直接进**一个新的 Claude 会话，不弹菜单。

- [ ] **Step 6: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/ui.rs
git commit -m "feat: n 直连上次的 agent，N 才进选择器

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 13（二期）: 密钥设置页

**Files:**
- Modify: `src/ui.rs`（`View`、看板 `c` 键、按键循环、渲染、`escape_hint`、`back_one_level`、`idle_help`）
- Test: `src/ui.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 8 的 `Request::DeleteSecret`、Task 11 的 `View::EnterSecret`
- Produces: `View::Secrets { entries: Vec<ProfileEntry>, state: ListState }`、`pub fn secret_rows(entries: &[ProfileEntry]) -> Vec<(String, bool)>`（label 与「已配没配」）

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn secret_rows_only_lists_profiles_that_need_a_key() {
    let entries = vec![
        entry("claude", ProfileStatus::Ready),       // 不需要密钥
        with_secret(entry("kimi", ProfileStatus::Ready)),
        with_secret(entry("glm", ProfileStatus::NeedsSecret)),
    ];
    let rows = secret_rows(&entries);
    assert_eq!(rows.len(), 2, "claude 不该出现在密钥页");
    assert_eq!(rows[0], ("kimi".to_string(), true), "Ready 说明密钥已配");
    assert_eq!(rows[1], ("glm".to_string(), false));
}

#[test]
fn secrets_view_escapes_to_the_board() {
    assert!(matches!(
        back_one_level(View::Secrets {
            entries: vec![],
            state: ListState::default(),
        }),
        Some(View::Board)
    ));
}

#[test]
fn board_help_mentions_the_settings_key() {
    assert!(idle_help(&View::Board).contains("c 密钥"));
}
```

`with_secret` 是测试辅助：给 `ProfileEntry` 填一个 `SecretPrompt`。

⚠️ `secret_rows` 用 `status != NeedsSecret` 判断「已配」是有边界的：`NeedsDependency` 时密钥可能配了也可能没配，这个状态压过了密钥状态。二期实现时如果这个区分要紧，就在 `ProfileEntry` 上加一个 `has_secret: bool` 字段，别用状态反推。测试里只覆盖 `Ready` 和 `NeedsSecret` 两种就是因为这个。

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib ui`
Expected: FAIL

- [ ] **Step 3: 实现**

纯函数：

```rust
/// 密钥页要列哪些行。只列声明了密钥的 profile——claude / codex / 命令行
/// 出现在这一页只会让用户以为它们也要配。
pub fn secret_rows(entries: &[ProfileEntry]) -> Vec<(String, bool)> {
    entries
        .iter()
        .filter(|e| e.secret.is_some())
        .map(|e| (e.name.clone(), e.status != ProfileStatus::NeedsSecret))
        .collect()
}
```

`View` 加：

```rust
    Secrets {
        entries: Vec<ProfileEntry>,
        state: ListState,
    },
```

`View::EnterSecret` 加一个字段，**成功后去哪不能靠猜**：

```rust
        /// 从设置页进来的要回设置页（意图是改配置），从选择器进来的直接开会话
        /// （意图是开工）。
        return_to_settings: bool,
```

按键：看板 `c` → 拉 `Request::Profiles` 进 `Secrets`；`↑↓` 移动；`Enter` → `View::EnterSecret { return_to_settings: true, .. }`；`d` → `Request::DeleteSecret` 后重拉列表；`Esc` / `Ctrl+Q` → 看板。

渲染每行：

```rust
                    let (name, configured) = row;
                    let label = entries.iter()
                        .find(|e| &e.name == name)
                        .map(|e| e.label.clone())
                        .unwrap_or_else(|| name.clone());
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{:<14}", truncate(&label, 14))),
                        Span::styled(
                            if *configured { "已配" } else { "未配" },
                            Style::default().fg(if *configured {
                                Color::Green
                            } else {
                                Color::DarkGray
                            }),
                        ),
                    ]))
```

`escape_hint` 加 `View::Secrets { .. } => "Ctrl+Q 回看板"`（`_ =>` 那支已经覆盖，确认一下即可）。
`idle_help` 加 `View::Secrets { .. } => "↑↓ 选  Enter 改  d 删  Esc 返回"`，看板那行加 `c 密钥`。

- [ ] **Step 4: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
~/.cargo/bin/cargo fmt
git add src/ui.rs
git commit -m "feat: 密钥设置页，看板按 c 进，可改可删

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## 收尾

- [ ] `~/.cargo/bin/cargo test` 全绿
- [ ] `~/.cargo/bin/cargo clippy -- -D warnings` 干净
- [ ] `git diff --check` 没有行尾空白
- [ ] 更新 `README.md`：九个 agent、`~/.dct/profiles/` 自定义、`n`/`N`/`c` 三个键
- [ ] 回头核对设计文档的「未实测项」表——Task 2 Step 6 实跑出来的结果要落回去，别让下一个人再猜一遍
