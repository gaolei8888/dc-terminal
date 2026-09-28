//! 这台电脑在「我的电脑」组里的身份，落在磁盘上的那部分：两把钥匙、自己叫
//! 什么、最新的一份组名单。都在 `~/.dct/mesh/` 底下。
//!
//! **钥匙是文件，所有平台都一样，包括 Mac。** 第一步不用 Secure Enclave
//! （计划里的裁定）：这两把钥匙要能被 `seal`/`roster` 直接拿来算，而 SE 里
//! 的钥匙做不了 X25519。文件用 `sys::fs::create_private` 建（Unix 0600，
//! Windows 只有属主一条 ACE），目录收紧到只有属主。
//!
//! **写盘一律是「临时文件 + rename」**：名单被写坏一半，下一次启动读到的是
//! 一份解析不了的名单，这台电脑就从组里悄悄掉出去了；钥匙被写坏一半更糟，
//! 它的身份（端点 id 就是签名公钥的哈希）直接换了。
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use dct_mesh::roster::MAX_NAME_LEN;
use dct_mesh::{MachineKeys, SignedRoster};

const SIGN_KEY: &str = "sign.key";
const KX_KEY: &str = "kx.key";
const NAME: &str = "name";
const ROSTER: &str = "roster.json";

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn at(dir: PathBuf) -> Store {
        Store { dir }
    }

    /// `~/.dct/mesh`，跟 `KeyStore::default_dir()` 同一个父目录。
    pub fn default_dir() -> PathBuf {
        dir_for_socket(&crate::proto::socket_path())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 读出这台电脑的两把钥匙；还没有就用 `rand` 生成、落盘。
    ///
    /// 两个文件的写入顺序是有讲究的：**先落 `kx.key`，再落 `sign.key`**，于是
    /// 「`sign.key` 在」就意味着两把都完整写过。只有 `kx.key` 没有 `sign.key`
    /// 是上次写到一半断了——那时这台电脑的身份还从没被用过（端点 id 由
    /// `sign.key` 算出），整套重新生成是安全的。反过来「有 `sign.key` 没有
    /// `kx.key`」不可能由我们自己写出来，是有人动过这个目录，**不猜、报错**：
    /// 悄悄补一把新的 `kx.key` 等于让组里其它电脑加密给我的东西全都解不开。
    pub fn load_or_create_keys(&self, rand: &dyn Fn() -> [u8; 32]) -> Result<MachineKeys> {
        let sign_path = self.dir.join(SIGN_KEY);
        let kx_path = self.dir.join(KX_KEY);
        if sign_path.exists() {
            if !kx_path.exists() {
                bail!(
                    "{} 在，{} 却不见了——不会替你重新生成（那会换掉这台电脑的加密钥匙）",
                    sign_path.display(),
                    kx_path.display()
                );
            }
            let sign = read_seed(&sign_path)?;
            let kx = read_seed(&kx_path)?;
            return MachineKeys::from_seeds(sign, kx)
                .map_err(|e| anyhow!("{} 不是一把能用的钥匙：{e}", sign_path.display()));
        }

        self.ensure_dir()?;
        // P-256 的种子要落在 [1, n) 里，随机 32 字节落在外面的概率约 2^-32；
        // 连着好几次都落在外面，说明 `rand` 坏了，不是运气差。
        let mut attempt = 0;
        let (sign, kx, keys) = loop {
            let sign = rand();
            let kx = rand();
            if let Ok(k) = MachineKeys::from_seeds(sign, kx) {
                break (sign, kx, k);
            }
            attempt += 1;
            if attempt >= 8 {
                bail!("随机数给不出一把合法的签名钥匙，随机源可能坏了");
            }
        };
        write_atomic(&kx_path, STANDARD.encode(kx).as_bytes())?;
        write_atomic(&sign_path, STANDARD.encode(sign).as_bytes())?;
        Ok(keys)
    }

    /// 这台电脑在组里叫什么。没设过就是主机名（整理成名单接受的样子）。
    pub fn name(&self) -> Result<String> {
        match std::fs::read_to_string(self.dir.join(NAME)) {
            Ok(s) => Ok(s.trim().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(tidy_name(&hostname())),
            Err(e) => Err(e).context("读不了电脑名"),
        }
    }

    /// 改名。规则跟名单的一样（`roster::accept` 会拒掉不合规的名字），在这里
    /// 就拦住，免得存进去一个将来签不进名单的名字。
    pub fn set_name(&self, n: &str) -> Result<()> {
        let n = n.trim();
        if n.is_empty() || n.contains('/') || n.chars().count() > MAX_NAME_LEN {
            bail!("电脑名不能为空、不能带 /，最长 {MAX_NAME_LEN} 个字");
        }
        self.ensure_dir()?;
        write_atomic(&self.dir.join(NAME), n.as_bytes())
    }

    /// 最新的一份组名单；还没进过任何组就是 `None`。
    pub fn roster(&self) -> Result<Option<SignedRoster>> {
        let path = self.dir.join(ROSTER);
        match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b)
                .map(Some)
                .with_context(|| format!("{} 解析不了", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("读不了 {}", path.display())),
        }
    }

    /// 存名单。**不在这里验**——能走到这一步的名单必须已经被
    /// `roster::accept`（或者建组时的 `genesis`）放行过。
    pub fn save_roster(&self, r: &SignedRoster) -> Result<()> {
        self.ensure_dir()?;
        let bytes = serde_json::to_vec_pretty(r)?;
        write_atomic(&self.dir.join(ROSTER), &bytes)
    }

    fn ensure_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("建不了 {}", self.dir.display()))?;
        crate::sys::fs::restrict_dir_to_owner(&self.dir)
            .with_context(|| format!("收紧不了 {} 的权限", self.dir.display()))
    }
}

/// 跟着 socket 走，测试把 socket 放临时目录就自动隔离（同 `secrets_path_for_socket`）。
pub fn dir_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("mesh"),
        None => PathBuf::from("mesh"),
    }
}

fn read_seed(path: &Path) -> Result<[u8; 32]> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("读不了 {}", path.display()))?;
    let raw = STANDARD
        .decode(text.trim())
        .with_context(|| format!("{} 不是 base64", path.display()))?;
    raw.try_into()
        .map_err(|_| anyhow!("{} 不是 32 字节", path.display()))
}

/// 写到旁边的临时文件、落盘、再 rename 到正式位置。rename 在同一个目录里是
/// 原子的（Windows 上 std 用的是 `MoveFileExW(REPLACE_EXISTING)`，
/// `create_private` 那边已经给全了共享位，见它的注释）。
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let tmp = tmp_path(path);
    let write = || -> std::io::Result<()> {
        let mut f = crate::sys::fs::create_private(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    };
    write().with_context(|| format!("写不了 {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("换不上 {}", path.display()))
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// 主机名整理成名单接受的样子：去掉 `.local` 这类后缀、去掉 `/`、截到上限。
/// 整理完是空的（问不出主机名）就叫 `dct`——总比建不了组好，用户随时能改。
fn tidy_name(raw: &str) -> String {
    let base = raw.trim().split('.').next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|c| *c != '/' && !c.is_control())
        .take(MAX_NAME_LEN)
        .collect();
    if cleaned.is_empty() {
        "dct".to_string()
    } else {
        cleaned
    }
}

#[cfg(unix)]
fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: 缓冲区够大，长度如实给出；gethostname 最多写 len 个字节。
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

#[cfg(windows)]
fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dct_mesh::roster::{genesis, Member};
    use std::cell::Cell;

    fn counter_rand() -> impl Fn() -> [u8; 32] {
        let n = Cell::new(0u8);
        move || {
            n.set(n.get() + 1);
            [n.get(); 32]
        }
    }

    fn store() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::at(tmp.path().join("mesh"));
        (tmp, s)
    }

    fn member_of(k: &MachineKeys, name: &str) -> Member {
        Member {
            name: name.into(),
            endpoint: dct_mesh::id::endpoint_for(&k.sign_pub()),
            sign_pub: STANDARD.encode(k.sign_pub()),
            kx_pub: STANDARD.encode(k.kx_pub()),
            added_at: 1,
        }
    }

    #[test]
    fn keys_are_created_once_and_loaded_back_the_same() {
        let (_t, s) = store();
        let a = s.load_or_create_keys(&counter_rand()).unwrap();
        // 第二次给一个完全不同的随机源：真用了它就会得到另一把钥匙。
        let b = s
            .load_or_create_keys(&|| panic!("钥匙已经在了，不该再要随机数"))
            .unwrap();
        assert_eq!(a.sign_pub(), b.sign_pub());
        assert_eq!(a.kx_pub(), b.kx_pub());
    }

    #[test]
    fn a_reopened_store_sees_the_same_keys() {
        let (t, s) = store();
        let a = s.load_or_create_keys(&counter_rand()).unwrap();
        let again = Store::at(t.path().join("mesh"));
        let b = again.load_or_create_keys(&|| [9u8; 32]).unwrap();
        assert_eq!(a.sign_pub(), b.sign_pub());
    }

    #[test]
    fn a_bad_signing_seed_is_retried_with_fresh_randomness() {
        let (_t, s) = store();
        // 第一对里签名种子是全 0（不合法），第二对才合法。
        let n = Cell::new(0u8);
        let rand = move || {
            n.set(n.get() + 1);
            if n.get() == 1 {
                [0u8; 32]
            } else {
                [n.get(); 32]
            }
        };
        let k = s.load_or_create_keys(&rand).unwrap();
        assert_eq!(
            k.sign_pub(),
            MachineKeys::from_seeds([3; 32], [4; 32])
                .unwrap()
                .sign_pub()
        );
    }

    #[test]
    fn a_half_written_key_pair_without_sign_key_is_regenerated() {
        let (_t, s) = store();
        std::fs::create_dir_all(s.dir()).unwrap();
        std::fs::write(s.dir().join(KX_KEY), STANDARD.encode([7u8; 32])).unwrap();
        let k = s.load_or_create_keys(&counter_rand()).unwrap();
        assert_eq!(
            k.kx_pub(),
            MachineKeys::from_seeds([1; 32], [2; 32]).unwrap().kx_pub()
        );
    }

    #[test]
    fn a_sign_key_without_its_kx_key_is_an_error_not_a_silent_new_key() {
        let (_t, s) = store();
        s.load_or_create_keys(&counter_rand()).unwrap();
        std::fs::remove_file(s.dir().join(KX_KEY)).unwrap();
        assert!(s.load_or_create_keys(&counter_rand()).is_err());
        assert!(!s.dir().join(KX_KEY).exists(), "不该悄悄补一把");
    }

    #[cfg(unix)]
    #[test]
    fn key_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_t, s) = store();
        s.load_or_create_keys(&counter_rand()).unwrap();
        for f in [SIGN_KEY, KX_KEY] {
            let mode = std::fs::metadata(s.dir().join(f))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{f}");
        }
        let dir_mode = std::fs::metadata(s.dir()).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700);
    }

    #[test]
    fn no_roster_yet_is_none() {
        let (_t, s) = store();
        assert!(s.roster().unwrap().is_none());
    }

    #[test]
    fn a_saved_roster_reads_back() {
        let (_t, s) = store();
        let k = s.load_or_create_keys(&counter_rand()).unwrap();
        let r = genesis(member_of(&k, "laptop"), "mine".into(), &k);
        s.save_roster(&r).unwrap();
        assert_eq!(s.roster().unwrap(), Some(r));
        assert!(
            !s.dir().join("roster.json.tmp").exists(),
            "临时文件该被 rename 走"
        );
    }

    /// 原子写的可观察后果：写新名单的过程中出了错，旧名单一个字节不动。
    /// 把临时文件的位置先占成一个目录，写临时文件那一步就必然失败——如果
    /// 实现是直接截断正式文件再写，这里旧名单早就没了。
    #[test]
    fn a_failed_roster_write_leaves_the_old_roster_intact() {
        let (_t, s) = store();
        let k = s.load_or_create_keys(&counter_rand()).unwrap();
        let old = genesis(member_of(&k, "laptop"), "mine".into(), &k);
        s.save_roster(&old).unwrap();

        std::fs::create_dir(s.dir().join("roster.json.tmp")).unwrap();
        let newer = genesis(member_of(&k, "desk"), "mine".into(), &k);
        assert!(s.save_roster(&newer).is_err());
        assert_eq!(s.roster().unwrap(), Some(old));
    }

    #[test]
    fn name_defaults_to_something_the_roster_accepts_and_can_be_changed() {
        let (_t, s) = store();
        let n = s.name().unwrap();
        assert!(!n.is_empty() && !n.contains('/') && n.chars().count() <= MAX_NAME_LEN);
        s.set_name("公司Windows").unwrap();
        assert_eq!(s.name().unwrap(), "公司Windows");
    }

    #[test]
    fn a_name_the_roster_would_refuse_is_refused_here() {
        let (_t, s) = store();
        assert!(s.set_name("").is_err());
        assert!(s.set_name("a/b").is_err());
        assert!(s.set_name(&"x".repeat(MAX_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn hostnames_are_tidied_into_valid_names() {
        assert_eq!(tidy_name("Leis-MacBook.local"), "Leis-MacBook");
        assert_eq!(tidy_name(""), "dct");
        assert_eq!(tidy_name("a/b"), "ab");
        assert_eq!(tidy_name(&"y".repeat(80)).chars().count(), MAX_NAME_LEN);
    }

    #[test]
    fn the_default_dir_sits_next_to_the_other_dct_files() {
        assert_eq!(
            dir_for_socket(Path::new("/h/.dct/daemon.sock")),
            PathBuf::from("/h/.dct/mesh")
        );
    }
}
