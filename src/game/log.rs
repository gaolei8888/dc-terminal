//! 每一步一行 JSON，追加到 `~/.dct/games/log/<日期>.jsonl`。每行写完就落盘：玩到一半被 Ctrl-C，
//! 已经走过的那些步不会丢。以后拿这些记录和大模型的选择比。
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct LogFile {
    path: PathBuf,
}

impl LogFile {
    pub fn open(home: &Path, secs_since_epoch: u64) -> std::io::Result<LogFile> {
        let dir = super::profile::games_dir(home).join("log");
        std::fs::create_dir_all(&dir)?;
        let (y, m, d) = crate::journal::civil_from_days((secs_since_epoch / 86_400) as i64);
        Ok(LogFile { path: dir.join(format!("{y:04}-{m:02}-{d:02}.jsonl")) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, v: &Value) -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(f, "{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lines_are_appended_to_a_file_named_by_the_utc_date() {
        let h = tempfile::tempdir().unwrap();
        // 2026-10-02 12:00:00 UTC
        let l = LogFile::open(h.path(), 1_790_942_400).unwrap();
        assert!(l.path().ends_with("2026-10-02.jsonl"), "{:?}", l.path());
        l.append(&json!({"a": 1})).unwrap();
        l.append(&json!({"b": 2})).unwrap();
        let again = LogFile::open(h.path(), 1_790_942_400 + 60).unwrap();
        again.append(&json!({"c": 3})).unwrap();
        let text = std::fs::read_to_string(l.path()).unwrap();
        let lines: Vec<Value> = text.lines().map(|x| serde_json::from_str(x).unwrap()).collect();
        assert_eq!(lines, vec![json!({"a": 1}), json!({"b": 2}), json!({"c": 3})]);
    }

    #[test]
    fn a_new_day_is_a_new_file() {
        let h = tempfile::tempdir().unwrap();
        let a = LogFile::open(h.path(), 1_790_942_400).unwrap();
        let b = LogFile::open(h.path(), 1_790_942_400 + 86_400).unwrap();
        assert_ne!(a.path(), b.path());
    }
}
