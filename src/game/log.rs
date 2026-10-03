//! 每一步一行 JSON，追加到 `~/.dct/games/log/<日期>.jsonl`。每行写完就落盘：玩到一半被 Ctrl-C，
//! 已经走过的那些步不会丢。以后拿这些记录和大模型的选择比。
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct LogFile {
    path: PathBuf,
}

impl LogFile {
    /// 开始时就把当天的文件建好（追加方式）：写不了就在这里说清楚，而不是玩到一半才发现没有记录。
    /// 错误是一句人话，带着路径。
    pub fn open(home: &Path, secs_since_epoch: u64) -> Result<LogFile, String> {
        let dir = super::profile::games_dir(home).join("log");
        let (y, m, d) = crate::journal::civil_from_days((secs_since_epoch / 86_400) as i64);
        let path = dir.join(format!("{y:04}-{m:02}-{d:02}.jsonl"));
        let why = |e: std::io::Error| {
            let reason = match e.kind() {
                std::io::ErrorKind::PermissionDenied => "没有写入的权限",
                _ => "写不进去（可能是同名的文件夹挡着，或者磁盘满了）",
            };
            format!("记录文件开不了：{}，{reason}。", path.display())
        };
        std::fs::create_dir_all(&dir).map_err(why)?;
        std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(why)?;
        Ok(LogFile { path })
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
    fn an_unwritable_log_file_fails_at_open_with_a_plain_sentence_naming_the_path() {
        let h = tempfile::tempdir().unwrap();
        let dir = h.path().join(".dct/games/log");
        std::fs::create_dir_all(dir.join("2026-10-02.jsonl")).unwrap(); // 该是文件的地方是个文件夹
        let e = LogFile::open(h.path(), 1_790_942_400).err().expect("应当失败");
        assert!(e.contains("2026-10-02.jsonl") && e.starts_with("记录文件开不了"), "{e}");
        assert!(!e.to_lowercase().contains("directory") && !e.contains("os error"), "{e}");
    }

    #[test]
    fn a_new_day_is_a_new_file() {
        let h = tempfile::tempdir().unwrap();
        let a = LogFile::open(h.path(), 1_790_942_400).unwrap();
        let b = LogFile::open(h.path(), 1_790_942_400 + 86_400).unwrap();
        assert_ne!(a.path(), b.path());
    }
}
