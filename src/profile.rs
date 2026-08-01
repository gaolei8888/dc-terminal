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
