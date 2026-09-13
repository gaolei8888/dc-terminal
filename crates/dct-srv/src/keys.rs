//! 发布密钥文件。见 `docs/superpowers/specs/2026-09-13-public-live-listing-design.md`
//! 「发布密钥与总开关」。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublishKeys {
    #[serde(default)]
    pub keys: Vec<KeyEntry>,
    /// 被运营方下线的房间号（`dct-srv takedown`）。停播之前都公开不了。
    #[serde(default)]
    pub blocked: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeyEntry {
    pub name: String,
    /// SHA-256 的小写 hex。**原文不落盘**，同直播那两把钥匙的做法。
    pub hash: String,
    /// 创建时间，Unix 秒。只给 `key list` 看。
    pub created: u64,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest_hex(key: &str) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(key.as_bytes()))
}

/// 常数时间比较两个等长 hex 串；长度不同直接不等（长度不是秘密）。
fn same_hex(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

impl PublishKeys {
    pub fn load(path: &Path) -> Result<PublishKeys, String> {
        let raw =
            std::fs::read_to_string(path).map_err(|e| format!("读不了 {}：{e}", path.display()))?;
        serde_json::from_str(&raw)
            .map_err(|e| format!("{} 不是合法的密钥文件：{e}", path.display()))
    }

    /// 先写同目录临时文件再改名：写到一半断电，读到的要么是旧文件要么是新文件。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        let body = serde_json::to_vec_pretty(self).expect("PublishKeys 总能序列化");
        {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            std::io::Write::write_all(&mut f, &body)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)
    }

    pub fn add(&mut self, name: &str, now: u64) -> Result<String, String> {
        if name.trim().is_empty() {
            return Err("名字不能为空".into());
        }
        if self.keys.iter().any(|k| k.name == name) {
            return Err(format!("已经有一把叫「{name}」的密钥了"));
        }
        let mut raw = [0u8; 32];
        getrandom::getrandom(&mut raw).map_err(|e| format!("系统随机数不可用：{e}"))?;
        let key = hex(&raw);
        self.keys.push(KeyEntry {
            name: name.to_string(),
            hash: digest_hex(&key),
            created: now,
        });
        Ok(key)
    }

    pub fn revoke(&mut self, name: &str) -> bool {
        let before = self.keys.len();
        self.keys.retain(|k| k.name != name);
        self.keys.len() != before
    }

    pub fn block(&mut self, id: &str) {
        if !self.is_blocked(id) {
            self.blocked.push(id.to_string());
        }
    }

    /// 这把密钥是谁的。**把每一条都比一遍**，不在第一条命中时提前返回。
    pub fn name_for(&self, key: &str) -> Option<String> {
        let want = digest_hex(key);
        let mut found = None;
        for k in &self.keys {
            if same_hex(&k.hash, &want) {
                found = Some(k.name.clone());
            }
        }
        found
    }

    pub fn is_blocked(&self, id: &str) -> bool {
        self.blocked.iter().any(|b| b == id)
    }
}

/// 运行中的中转手里那份密钥文件：记着上次读到的修改时间，变了才重读。
pub struct KeyFile {
    path: PathBuf,
    stamp: Option<SystemTime>,
    keys: PublishKeys,
}

impl KeyFile {
    /// 首次打开：读不了或者解析失败就报错——调用方据此拒绝启动。
    pub fn open(path: PathBuf) -> Result<KeyFile, String> {
        let keys = PublishKeys::load(&path)?;
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        Ok(KeyFile { path, stamp, keys })
    }

    pub fn keys(&self) -> &PublishKeys {
        &self.keys
    }

    /// 文件变了就重读。**解析失败保留上一份**并报错，由调用方写日志。
    pub fn reload_if_changed(&mut self) -> Result<bool, String> {
        let stamp = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        if stamp == self.stamp {
            return Ok(false);
        }
        self.stamp = stamp;
        self.keys = PublishKeys::load(&self.path)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file() -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("publish-keys.json");
        (d, p)
    }

    /// 生成的密钥只返回一次，文件里只有摘要；拿原文能认出是谁的。
    #[test]
    fn a_key_is_stored_as_a_digest_and_recognised_by_name() {
        let mut k = PublishKeys::default();
        let key = k.add("姜老师", 1).unwrap();
        assert_eq!(key.len(), 64);
        assert!(
            !serde_json::to_string(&k).unwrap().contains(&key),
            "文件里不许有原文"
        );
        assert_eq!(k.name_for(&key).as_deref(), Some("姜老师"));
        assert_eq!(k.name_for(&"0".repeat(64)), None);
        assert!(k.add("姜老师", 2).is_err(), "重名要拒");
    }

    #[test]
    fn revoking_and_blocking() {
        let mut k = PublishKeys::default();
        let key = k.add("a", 1).unwrap();
        assert!(k.revoke("a"));
        assert!(!k.revoke("a"), "吊销不存在的名字回 false");
        assert_eq!(k.name_for(&key), None);
        k.block("room1");
        k.block("room1");
        assert!(k.is_blocked("room1"));
        assert_eq!(k.blocked.len(), 1, "重复下线不重复记");
    }

    #[test]
    fn save_then_load_round_trips_with_owner_only_permissions() {
        let (_d, p) = tmp_file();
        let mut k = PublishKeys::default();
        k.add("a", 1).unwrap();
        k.save(&p).unwrap();
        assert_eq!(PublishKeys::load(&p).unwrap(), k);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    /// **写坏文件不能等于吊销全部，也不能等于全部放行**：保留上一份。
    #[test]
    fn a_broken_file_keeps_the_last_good_keys() {
        let (_d, p) = tmp_file();
        let mut k = PublishKeys::default();
        let key = k.add("a", 1).unwrap();
        k.save(&p).unwrap();
        let mut f = KeyFile::open(p.clone()).unwrap();
        // 保证 mtime 真的变了（有些文件系统精度是秒）
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&p, "{ not json").unwrap();
        assert!(f.reload_if_changed().is_err());
        assert_eq!(
            f.keys().name_for(&key).as_deref(),
            Some("a"),
            "坏文件不能冲掉旧密钥"
        );
    }

    #[test]
    fn a_changed_file_is_picked_up() {
        let (_d, p) = tmp_file();
        let mut k = PublishKeys::default();
        let key = k.add("a", 1).unwrap();
        k.save(&p).unwrap();
        let mut f = KeyFile::open(p.clone()).unwrap();
        assert_eq!(f.reload_if_changed(), Ok(false), "没变就不重读");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        k.revoke("a");
        k.save(&p).unwrap();
        assert_eq!(f.reload_if_changed(), Ok(true));
        assert_eq!(f.keys().name_for(&key), None);
    }

    #[test]
    fn opening_a_broken_file_fails_outright() {
        let (_d, p) = tmp_file();
        std::fs::write(&p, "nope").unwrap();
        assert!(KeyFile::open(p).is_err(), "首次启动就坏的文件：拒绝启动");
    }
}
