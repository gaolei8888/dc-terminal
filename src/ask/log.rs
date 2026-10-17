//! `~/.dct/ask.log`：每次 ask 一行**元数据**（设计 §4），没有任何对话文字、没有模型回复。
//! 字段固定：`t`（unix 秒）、`source`、`conv`（对话 id 的哈希前 4 位，不是 id 本身）、
//! `chars_in` / `chars_out`（只有字数）、`first_ms` / `total_ms`、`outcome`。
//! 这是整个 `src/ask/` 里**唯一**写盘的文件。记不下来不影响 ask：IO 错误一律吞掉。

use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Meta<'a> {
    pub source: &'a str,
    pub conversation_id: &'a str,
    pub chars_in: usize,
    pub chars_out: usize,
    pub first_ms: u64,
    pub total_ms: u64,
    pub outcome: &'a str,
}

fn conv_tag(id: &str) -> String {
    Sha256::digest(id.as_bytes()).iter().take(2).map(|b| format!("{b:02x}")).collect()
}

pub fn append(path: &Path, m: &Meta) {
    let line = json!({
        "t": SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "source": m.source, "conv": conv_tag(m.conversation_id),
        "chars_in": m.chars_in, "chars_out": m.chars_out,
        "first_ms": m.first_ms, "total_ms": m.total_ms, "outcome": m.outcome,
    });
    // 超长就整份换掉，同 `journal`：排查看的是近况。
    let truncate = std::fs::metadata(path).is_ok_and(|md| md.len() > crate::journal::MAX_BYTES);
    let new = !path.exists();
    let opened = std::fs::OpenOptions::new().create(true).append(!truncate).write(true).truncate(truncate).open(path);
    if let Ok(mut f) = opened {
        if new {
            let _ = crate::sys::fs::restrict_file_to_owner(path);
        }
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str) -> Meta<'_> {
        Meta { source: "voice", conversation_id: id, chars_in: 7, chars_out: 12, first_ms: 1800, total_ms: 3300, outcome: "end" }
    }

    #[test]
    fn one_json_line_of_metadata_with_only_a_hash_of_the_conversation_id() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("ask.log");
        append(&f, &meta("c-secret-id-7f3a"));
        append(&f, &meta("c-secret-id-7f3a"));
        let text = std::fs::read_to_string(&f).unwrap();
        assert_eq!(text.lines().count(), 2);
        let v: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["chars_in", "chars_out", "conv", "first_ms", "outcome", "source", "t", "total_ms"]);
        assert_eq!(v["conv"].as_str().unwrap().len(), 4);
        assert!(!text.contains("secret-id") && !text.contains("7f3a"), "{text}");
        assert_eq!((v["chars_in"].as_u64(), v["outcome"].as_str()), (Some(7), Some("end")));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("ask.log");
        append(&f, &meta("c"));
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn an_unwritable_path_is_swallowed() {
        append(Path::new("/nonexistent-dir-for-ask-log/ask.log"), &meta("c"));
    }

    #[test]
    fn a_huge_log_is_replaced_not_grown() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("ask.log");
        std::fs::write(&f, vec![b'x'; crate::journal::MAX_BYTES as usize + 1]).unwrap();
        append(&f, &meta("c"));
        assert!(std::fs::metadata(&f).unwrap().len() < 1000);
    }
}
