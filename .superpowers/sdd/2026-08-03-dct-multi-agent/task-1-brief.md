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

