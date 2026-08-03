use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 列表上限。20 条足够覆盖手头在做的项目，再多列表本身就难挑了。
const MAX: usize = 20;

/// 磁盘格式。包一层对象而不是直接存数组，是为了将来加字段时老文件仍能读。
#[derive(Default, Serialize, Deserialize)]
struct Disk {
    #[serde(default)]
    recent: Vec<String>,
}

/// 最近开过会话的项目目录，最近使用的在最前。
pub struct Store {
    path: PathBuf,
    recent: Vec<String>,
}

/// 存放位置跟着 socket 走，而不是直接拼 `$HOME`。生产环境 socket 在
/// `~/.dct/daemon.sock`，推出来就是 `~/.dct/projects.json`，与直接拼 `$HOME` 同一个
/// 文件；而集成测试把 socket 建在临时目录里，于是自动拿到一份隔离的 store，
/// 不会去动你真实的那份。
pub fn store_path_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("projects.json"),
        None => PathBuf::from("projects.json"),
    }
}

impl Store {
    /// 文件不存在、JSON 语法错、字段类型不对——一律当空列表。
    /// 这是便利性缓存，不值得为它让守护进程起不来。
    pub fn load(path: &Path) -> Store {
        let recent = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Disk>(&s).ok())
            .map(|d| d.recent)
            .unwrap_or_default();
        Store {
            path: path.to_path_buf(),
            recent,
        }
    }

    pub fn list(&self) -> Vec<String> {
        self.recent.clone()
    }

    /// 记一笔：去重、提到最前、截断、落盘。
    pub fn touch(&mut self, dir: &Path) {
        // 归一成绝对路径，免得 `.` 和 `/abs/path` 在列表里各占一行。
        // 归一失败（目录刚被删）就存原样——丢掉这一条比存个粗糙的路径更糟。
        let key = std::fs::canonicalize(dir)
            .unwrap_or_else(|_| dir.to_path_buf())
            .display()
            .to_string();
        self.recent.retain(|p| p != &key);
        self.recent.insert(0, key);
        self.recent.truncate(MAX);
        self.save();
    }

    /// 落盘失败一律忽略：丢的是便利性，不是数据。内存里的列表照常可用。
    fn save(&self) {
        let Some(parent) = self.path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let Ok(json) = serde_json::to_string(&Disk {
            recent: self.recent.clone(),
        }) else {
            return;
        };
        // 原子写：先写同目录的临时文件再 rename。直接覆写的话，写到一半断电
        // 会留下半截 JSON，下次 load 解析失败就把整个列表丢了。
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_err() {
            return;
        }
        let _ = std::fs::rename(&tmp, &self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// canonicalize 会把 macOS 上 `/var/...` 的临时目录解成 `/private/var/...`，
    /// 所以断言里的期望值必须做同样的归一，否则测试在 macOS 上必失败。
    fn canon(p: &std::path::Path) -> String {
        std::fs::canonicalize(p).unwrap().display().to_string()
    }

    #[test]
    fn touch_moves_existing_entry_to_front() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();

        let mut s = Store::load(&tmp.path().join("projects.json"));
        s.touch(&a);
        s.touch(&b);
        s.touch(&a);

        assert_eq!(
            s.list(),
            vec![canon(&a), canon(&b)],
            "重复项要去重并提到最前"
        );
    }

    #[test]
    fn touch_caps_at_twenty() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Store::load(&tmp.path().join("projects.json"));
        for i in 0..25 {
            let d = tmp.path().join(format!("p{i}"));
            std::fs::create_dir(&d).unwrap();
            s.touch(&d);
        }
        let list = s.list();
        assert_eq!(list.len(), 20, "上限 20 条");
        assert_eq!(list[0], canon(&tmp.path().join("p24")), "最新的在最前");
        assert!(
            !list.contains(&canon(&tmp.path().join("p0"))),
            "最旧的应当被挤掉"
        );
    }

    #[test]
    fn corrupt_json_degrades_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("projects.json");
        std::fs::write(&f, "{ 这不是 JSON").unwrap();
        let s = Store::load(&f);
        assert!(s.list().is_empty(), "损坏的文件必须当空列表，不能 panic");
    }

    #[test]
    fn missing_file_degrades_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::load(&tmp.path().join("没有这个文件.json"));
        assert!(s.list().is_empty());
    }

    #[test]
    fn touch_survives_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("projects.json");
        let d = tmp.path().join("proj");
        std::fs::create_dir(&d).unwrap();

        let mut s = Store::load(&f);
        s.touch(&d);
        drop(s);

        let s2 = Store::load(&f);
        assert_eq!(
            s2.list(),
            vec![canon(&d)],
            "touch 必须落盘，重新 load 读得回"
        );
    }

    #[test]
    fn touch_keeps_unresolvable_path_as_is() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("已经删掉了");
        let mut s = Store::load(&tmp.path().join("projects.json"));
        s.touch(&gone);
        assert_eq!(
            s.list(),
            vec![gone.display().to_string()],
            "canonicalize 失败时存原样，不能丢掉这一条"
        );
    }

    #[test]
    fn store_path_sits_next_to_socket() {
        let p = store_path_for_socket(std::path::Path::new("/home/x/.dct/daemon.sock"));
        assert_eq!(p, std::path::PathBuf::from("/home/x/.dct/projects.json"));
    }
}
