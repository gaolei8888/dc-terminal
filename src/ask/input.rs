//! 请求校验（设计 §2.2）。CLI 和守护进程**各调一次同一个函数**：守护进程的 socket
//! 谁连得上谁能发 `Request::Ask`，不能只信 CLI 校验过。拒绝时只返回固定的原因词，
//! 不回显任何输入文字。

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const MAX_TEXT: usize = 500;
pub const MAX_PERSONA: usize = 300;
pub const DEFAULT_TIMEOUT_MS: u64 = 8000;
const TIMEOUT_MS: (u64, u64) = (1000, 30_000);
/// stdin 的上限：500 字 + 300 字人设，按每字 4 字节再加 JSON 外壳，远低于它。
pub const MAX_STDIN: u64 = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Voice,
    Typed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskInput {
    pub text: String,
    pub source: Source,
    pub conversation_id: String,
    pub persona: Option<String>,
    pub timeout: Duration,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Voice => "voice",
            Source::Typed => "typed",
        }
    }
}

/// stdin 上那一行 JSON，也是 socket 上 `Request::Ask` 的内容（一份定义）。未知字段
/// 忽略（设计 §6.1：只有 `v` 变了才不兼容）。
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct AskRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v: Option<u32>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub conversation_id: String,
    #[serde(default)]
    pub persona: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// 手写：听到的话和人设**不进** Debug（`serve()` 里任何一行 `{req:?}` 都不该把它们写进日志）。
impl std::fmt::Debug for AskRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AskRequest")
            .field("source", &self.source)
            .field("conversation_id", &self.conversation_id)
            .field("text_chars", &self.text.chars().count())
            .field("persona_chars", &self.persona.as_ref().map(|p| p.chars().count()))
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

/// 控制字符（含换行、制表、NUL、ANSI 的 ESC）一律换成空格再去首尾：念出来、拼进提示的都是干净文字。
fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// 错误是固定的原因词：`bad_version` `empty_input` `too_long` `bad_source`
/// `bad_conversation_id` `persona_too_long`。
pub fn validate(r: &AskRequest) -> Result<AskInput, &'static str> {
    if r.v.is_some_and(|v| v != 1) {
        return Err("bad_version");
    }
    let text = clean(&r.text);
    if text.is_empty() {
        return Err("empty_input");
    }
    // 超了拒绝，不静默截断。
    if text.chars().count() > MAX_TEXT {
        return Err("too_long");
    }
    let source = match r.source.as_str() {
        "voice" => Source::Voice,
        "typed" => Source::Typed,
        _ => return Err("bad_source"),
    };
    if !valid_conversation_id(&r.conversation_id) {
        return Err("bad_conversation_id");
    }
    let persona = r.persona.as_deref().map(clean).filter(|p| !p.is_empty());
    if persona.as_ref().is_some_and(|p| p.chars().count() > MAX_PERSONA) {
        return Err("persona_too_long");
    }
    let ms = r.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).clamp(TIMEOUT_MS.0, TIMEOUT_MS.1);
    Ok(AskInput {
        text,
        source,
        conversation_id: r.conversation_id.clone(),
        persona,
        timeout: Duration::from_millis(ms),
    })
}

/// `[A-Za-z0-9_-]{1,64}`（`--forget` 也用它）。
pub fn valid_conversation_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// stdin 的一行 → 校验过的请求，连同要发给守护进程的原始请求。`not_json` 之外的原因词同 [`validate`]。
pub fn parse_line(line: &str) -> Result<(AskRequest, AskInput), &'static str> {
    let req: AskRequest = serde_json::from_str(line).map_err(|_| "not_json")?;
    let input = validate(&req)?;
    Ok((req, input))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(text: &str, source: &str, id: &str, persona: Option<&str>, timeout_ms: Option<u64>) -> AskRequest {
        AskRequest {
            v: None,
            text: text.into(),
            source: source.into(),
            conversation_id: id.into(),
            persona: persona.map(Into::into),
            timeout_ms,
        }
    }

    fn validate(text: &str, source: &str, id: &str, persona: Option<&str>, timeout_ms: Option<u64>) -> Result<AskInput, &'static str> {
        super::validate(&req(text, source, id, persona, timeout_ms))
    }

    fn ok(text: &str) -> Result<AskInput, &'static str> {
        validate(text, "voice", "c-7f3a", None, None)
    }

    #[test]
    fn a_normal_request_gets_the_default_timeout() {
        let i = ok("今天天气怎么样").unwrap();
        assert_eq!((i.text.as_str(), i.source, i.timeout), ("今天天气怎么样", Source::Voice, Duration::from_millis(8000)));
    }

    #[test]
    fn empty_and_whitespace_only_text_is_refused() {
        assert_eq!(ok(""), Err("empty_input"));
        assert_eq!(ok(" \n\t　"), Err("empty_input"));
        // 只有控制字符也是空。
        assert_eq!(ok("\u{0}\u{7}\u{1b}"), Err("empty_input"));
    }

    #[test]
    fn text_over_500_chars_is_refused_not_truncated() {
        assert!(ok(&"字".repeat(500)).is_ok());
        assert_eq!(ok(&"字".repeat(501)), Err("too_long"));
    }

    #[test]
    fn strange_unicode_is_accepted_and_control_characters_become_spaces() {
        let i = ok("你好\u{1b}[31m 😀 \u{200f}ok\u{0}\n下一行").unwrap();
        assert!(!i.text.chars().any(char::is_control), "{:?}", i.text);
        assert!(i.text.contains("😀") && i.text.contains("下一行"));
    }

    #[test]
    fn source_and_conversation_id_are_checked() {
        assert_eq!(validate("x", "mic", "c", None, None), Err("bad_source"));
        assert_eq!(validate("x", "typed", "", None, None), Err("bad_conversation_id"));
        assert_eq!(validate("x", "typed", &"a".repeat(65), None, None), Err("bad_conversation_id"));
        for bad in ["a b", "a/b", "../x", "中文", "a\nb", "a.b"] {
            assert_eq!(validate("x", "typed", bad, None, None), Err("bad_conversation_id"), "{bad:?}");
        }
        assert!(validate("x", "typed", &"a".repeat(64), None, None).is_ok());
        assert!(valid_conversation_id("c-7f3a_X") && !valid_conversation_id("a b"));
    }

    #[test]
    fn persona_is_optional_capped_at_300_and_blank_means_none() {
        let p = |s: Option<&str>| validate("x", "typed", "c", s, None);
        assert_eq!(p(None).unwrap().persona, None);
        assert_eq!(p(Some("  ")).unwrap().persona, None);
        assert_eq!(p(Some("爱笑的章鱼")).unwrap().persona.as_deref(), Some("爱笑的章鱼"));
        assert!(p(Some(&"字".repeat(300))).is_ok());
        assert_eq!(p(Some(&"字".repeat(301))), Err("persona_too_long"));
    }

    #[test]
    fn timeout_is_clamped_to_1_to_30_seconds() {
        let t = |ms| validate("x", "typed", "c", None, Some(ms)).unwrap().timeout;
        assert_eq!((t(1), t(5000), t(999_999)), (Duration::from_millis(1000), Duration::from_millis(5000), Duration::from_millis(30_000)));
    }

    #[test]
    fn the_debug_form_never_shows_the_words_or_the_persona() {
        let d = format!("{:?}", req("听到的秘密话", "voice", "c1", Some("人设机密"), None));
        assert!(!d.contains("秘密") && !d.contains("机密"), "{d}");
        assert!(d.contains("text_chars: 6") && d.contains("c1"), "{d}");
    }

    #[test]
    fn stdin_lines_parse_and_unknown_fields_are_ignored() {
        let l = r#"{"v":1,"text":"hi","source":"typed","conversation_id":"c1","persona":"p","timeout_ms":2000,"future":true}"#;
        let (sent, i) = parse_line(l).unwrap();
        assert_eq!(sent.text, "hi");
        assert_eq!((i.text.as_str(), i.persona.as_deref(), i.timeout), ("hi", Some("p"), Duration::from_millis(2000)));
        // v 缺省当 1；别的版本不兼容。
        assert!(parse_line(r#"{"text":"hi","source":"typed","conversation_id":"c1"}"#).is_ok());
        assert_eq!(parse_line(r#"{"v":2,"text":"hi","source":"typed","conversation_id":"c1"}"#).err(), Some("bad_version"));
        assert_eq!(parse_line("not json").err(), Some("not_json"));
        assert_eq!(parse_line("").err(), Some("not_json"));
        assert_eq!(parse_line(r#"{"text":5}"#).err(), Some("not_json"));
        assert_eq!(parse_line(r#"{"source":"typed","conversation_id":"c1"}"#).err(), Some("empty_input"));
    }
}
