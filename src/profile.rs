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

const CLAUDE: &str = include_str!("../profiles/claude.toml");
const CODEX: &str = include_str!("../profiles/codex.toml");
const OPENCODE: &str = include_str!("../profiles/opencode.toml");
const QWEN: &str = include_str!("../profiles/qwen.toml");
const KIMI: &str = include_str!("../profiles/kimi.toml");
const GLM: &str = include_str!("../profiles/glm.toml");
const DEEPSEEK: &str = include_str!("../profiles/deepseek.toml");
const QWEN_API: &str = include_str!("../profiles/qwen-api.toml");
const SHELL: &str = include_str!("../profiles/shell.toml");

impl Profile {
    pub fn from_toml(s: &str) -> Result<Profile> {
        toml::from_str(s).context("profile TOML 解析失败")
    }

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
            "claude", "codex", "opencode", "qwen", "kimi", "glm", "deepseek", "qwen-api", "shell",
        ]
    }

    pub fn builtins() -> Vec<Profile> {
        Profile::builtin_names()
            .into_iter()
            .filter_map(Profile::builtin)
            .collect()
    }

    pub fn idle_regex(&self) -> Result<Option<regex::Regex>> {
        match &self.idle_pattern {
            None => Ok(None),
            Some(p) => {
                Ok(Some(regex::Regex::new(p).with_context(|| {
                    format!("idle_pattern 不是合法正则: {p}")
                })?))
            }
        }
    }

    pub fn busy_regex(&self) -> Result<Option<regex::Regex>> {
        match &self.busy_pattern {
            None => Ok(None),
            Some(p) => {
                Ok(Some(regex::Regex::new(p).with_context(|| {
                    format!("busy_pattern 不是合法正则: {p}")
                })?))
            }
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
}

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
        assert!(p
            .command
            .contains(&"--dangerously-skip-permissions".to_string()));
        assert!(p.is_agent);
    }

    #[test]
    fn builtin_shell_is_not_agent() {
        let p = Profile::builtin("shell").unwrap();
        assert!(!p.is_agent);
        assert!(p.idle_pattern.is_none());
    }

    #[test]
    fn builtin_names_includes_claude_and_shell() {
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

        assert_eq!(
            p.env.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("https://example.com/anthropic")
        );
        let s = p.secret.as_ref().unwrap();
        assert_eq!(s.env, "ANTHROPIC_AUTH_TOKEN");
        assert_eq!(s.hint.get(Lang::Zh), Some("去后台复制 API Key"));
        assert_eq!(s.url.as_deref(), Some("https://example.com/keys"));
        assert_eq!(
            s.verify.as_ref().unwrap().url,
            "https://example.com/anthropic/v1/messages"
        );
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

    #[test]
    fn every_builtin_parses_and_is_well_formed() {
        for name in Profile::builtin_names() {
            let p = Profile::builtin(name).unwrap_or_else(|| panic!("{name} 应当是内置 profile"));
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
                "claude", "codex", "opencode", "qwen", "kimi", "glm", "deepseek", "qwen-api",
                "shell",
            ]
        );
    }

    #[test]
    fn api_shaped_profiles_run_claude_and_need_a_secret() {
        for name in ["kimi", "glm", "deepseek", "qwen-api"] {
            let p = Profile::builtin(name).unwrap();
            assert_eq!(p.command[0], "claude", "{name}: API 形态跑的是 claude");
            assert!(
                p.env.contains_key("ANTHROPIC_BASE_URL"),
                "{name}: 要换 base_url"
            );
            let s = p
                .secret
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: 要声明密钥"));
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
        assert!(p
            .busy_regex()
            .unwrap()
            .unwrap()
            .is_match("(12s • esc to interrupt)"));
    }

    #[test]
    fn unverified_profiles_have_no_pattern() {
        // opencode / qwen 的 TUI 没实测过。宁可状态显示「—」，不能瞎猜一个 pattern
        // 然后在看板上编状态。
        for name in ["opencode", "qwen"] {
            let p = Profile::builtin(name).unwrap();
            assert!(
                p.idle_pattern.is_none() && p.busy_pattern.is_none(),
                "{name}: 没实测就别填 pattern"
            );
        }
    }
}
