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
        // daemon 会在整个生命周期内保持这个 store 实例。内存改动必须和
        // 磁盘写保持同步，否则 save 失败后，get() 会虚报密钥已保存，
        // 用户以为没问题，但下次重启密钥就没了。这里先记住改动前的值，
        // 若 save 失败就恢复，保证内存状态始终只反映已落盘的数据。
        let old_value = self.secrets.get(profile).cloned();
        self.secrets.insert(profile.to_string(), value.to_string());
        match self.save() {
            Ok(()) => Ok(()),
            Err(e) => {
                // save 失败，回滚内存改动
                if let Some(v) = old_value {
                    self.secrets.insert(profile.to_string(), v);
                } else {
                    self.secrets.remove(profile);
                }
                Err(e)
            }
        }
    }

    pub fn remove(&mut self, profile: &str) -> Result<()> {
        // daemon 会在整个生命周期内保持这个 store 实例。内存改动必须和
        // 磁盘写保持同步，否则 save 失败后，密钥看起来被删了但其实没有，
        // 用户以为没问题，但下次重启密钥又回来了。这里先记住改动前是否
        // 存在该键，若 save 失败就恢复。
        let old_value = self.secrets.get(profile).cloned();
        self.secrets.remove(profile);
        match self.save() {
            Ok(()) => Ok(()),
            Err(e) => {
                // save 失败，回滚内存改动
                if let Some(v) = old_value {
                    self.secrets.insert(profile.to_string(), v);
                }
                Err(e)
            }
        }
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

        let parent = self.path.parent().context("密钥文件没有上级目录")?;
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
        let result = (|| -> Result<()> {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .context("写不了密钥临时文件")?;
            f.write_all(text.as_bytes()).context("写密钥失败")?;
            f.sync_all().context("刷盘失败")?;
            std::fs::rename(&tmp, &self.path).context("替换密钥文件失败")?;
            Ok(())
        })();

        // 写到一半失败（比如磁盘满了）不能把半成品 tmp 文件留在目录里——
        // 下次 `set` 会拿 truncate 复用它，看着没事，但目录里多一个来路
        // 不明的文件本身就是留给用户的一个疑惑。rename 成功时 tmp 已经不
        // 存在了，这里删不到东西，remove_file 的错误无需理会。
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

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
        assert!(
            leftovers.is_empty(),
            "原子写的临时文件要收干净：{leftovers:?}"
        );
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

    #[test]
    fn set_rolls_back_memory_on_save_failure() {
        // set 失败时必须回滚内存改动。这在 daemon 场景中很关键：
        // 如果 set 后 save 失败，内存里有新值但磁盘没有，get() 会虚报
        // 密钥已保存，用户以为没问题，但重启后发现密钥没了。
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        std::fs::write(&f, "这不是 TOML {{{").unwrap();

        let mut s = SecretStore::load(&f);
        // 预期：文件坏了，加载时 load_error 被设置，secrets 是空的
        assert_eq!(s.get("key"), None, "初始状态无此键");

        // set 会因为 load_error 而失败
        let err = s.set("key", "value").unwrap_err();
        assert!(err.to_string().contains("密钥文件"));

        // 关键检验：失败后内存应该恢复到改动前的状态
        assert_eq!(s.get("key"), None, "set 失败后，新键不能出现在内存");
    }

    #[test]
    fn remove_rolls_back_memory_on_save_failure() {
        // remove 失败时也必须回滚内存改动。这在 daemon 场景中很关键：
        // 如果 remove 后 save 失败，内存里没有该键但磁盘还有，get() 会虚报
        // 密钥已删除，但重启后发现密钥还在，造成混淆。
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        std::fs::write(&f, "这不是 TOML {{{").unwrap();

        let mut s = SecretStore::load(&f);
        // 预期：文件坏了，加载时 load_error 被设置，secrets 是空的
        assert_eq!(s.get("key"), None, "初始状态无此键");

        // remove 会因为 load_error 而失败
        let err = s.remove("key").unwrap_err();
        assert!(err.to_string().contains("密钥文件"));

        // 关键检验：失败后内存应该恢复到改动前的状态（依旧无此键）
        assert_eq!(s.get("key"), None, "remove 失败后，状态应该不变");
    }
}
