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
const KEYS_LOCK: &str = ".keys.lock";

/// 等别的进程生成钥匙最多等多久。生成本身是几毫秒的事。
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
/// 锁文件多旧就当成是崩掉的进程留下的。
const LOCK_STALE: std::time::Duration = std::time::Duration::from_secs(60);

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
    ///
    /// **名单还在、钥匙却不全，一律报错，不重新生成。** 那种情况下生成一把新
    /// 钥匙，这台电脑的端点就悄悄换了：名单上写着的还是旧端点，组里谁发来
    /// 的东西都解不开、验不过，而用户看到的只是「多电脑不灵了」。要重来就让
    /// 用户自己删掉整个目录，那是一个看得见的决定。
    ///
    /// 生成的那一段在 `.keys.lock` 里做：`dct login` 和守护进程可能同一刻
    /// 都发现「还没有钥匙」，没有这把锁，两边各写一半，落盘的就是一把
    /// 进程 A 的 `kx.key` 配进程 B 的 `sign.key`。
    pub fn load_or_create_keys(&self, rand: &dyn Fn() -> [u8; 32]) -> Result<MachineKeys> {
        if let Some(k) = self.existing_keys()? {
            return Ok(k);
        }
        self.ensure_dir()?;
        let _lock = KeysLock::acquire(&self.dir.join(KEYS_LOCK), LOCK_WAIT, LOCK_STALE)?;
        // 等锁的时候别人可能已经生成好了。
        if let Some(k) = self.existing_keys()? {
            return Ok(k);
        }

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
        write_atomic(&self.dir.join(KX_KEY), STANDARD.encode(kx).as_bytes())?;
        write_atomic(&self.dir.join(SIGN_KEY), STANDARD.encode(sign).as_bytes())?;
        Ok(keys)
    }

    /// 磁盘上已经有完整的一对就读出来；还没有（可以生成）就是 `None`；
    /// 不能生成（见 `load_or_create_keys`）就报错。
    fn existing_keys(&self) -> Result<Option<MachineKeys>> {
        let sign_path = self.dir.join(SIGN_KEY);
        let kx_path = self.dir.join(KX_KEY);
        let (has_sign, has_kx) = (sign_path.exists(), kx_path.exists());
        if has_sign && has_kx {
            let sign = read_seed(&sign_path)?;
            let kx = read_seed(&kx_path)?;
            return MachineKeys::from_seeds(sign, kx)
                .map(Some)
                .map_err(|e| anyhow!("{} 不是一把能用的钥匙：{e}", sign_path.display()));
        }
        if self.dir.join(ROSTER).exists() {
            bail!(
                "这台电脑的多电脑钥匙丢了，但组名单还在；要重置请删除 {} 后重新 dct login",
                self.dir.display()
            );
        }
        if has_sign {
            bail!(
                "{} 在，{} 却不见了——不会替你重新生成（那会换掉这台电脑的加密钥匙）",
                sign_path.display(),
                kx_path.display()
            );
        }
        Ok(None)
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
        if !dct_mesh::roster::valid_name(n) {
            bail!("电脑名不能为空、不能带 / 和看不见的控制字符，最长 {MAX_NAME_LEN} 个字");
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

/// 跨进程的「我在生成钥匙」：独占创建一个文件，用完删掉。
///
/// 进程崩在中间会留下锁文件，所以超过 `stale` 的锁当成没人拿着、删掉重来；
/// 生成钥匙只要几毫秒，一把一分钟前的锁不可能还有人在用。
struct KeysLock {
    path: PathBuf,
}

impl KeysLock {
    fn acquire(
        path: &Path,
        wait: std::time::Duration,
        stale: std::time::Duration,
    ) -> Result<KeysLock> {
        let deadline = std::time::Instant::now() + wait;
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(_) => {
                    return Ok(KeysLock {
                        path: path.to_path_buf(),
                    })
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let age = std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok());
                    if age.is_some_and(|a| a > stale) {
                        let _ = std::fs::remove_file(path);
                        continue;
                    }
                    if std::time::Instant::now() >= deadline {
                        bail!(
                            "另一个 dct 正在生成多电脑钥匙，等不到它结束（{}）",
                            path.display()
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => return Err(e).with_context(|| format!("建不了 {}", path.display())),
            }
        }
    }
}

impl Drop for KeysLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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
        .filter(|c| *c != '/' && !c.is_control() && !dct_mesh::roster::is_format_char(*c))
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
    fn missing_keys_with_a_roster_present_is_an_error_not_a_new_identity() {
        let (_t, s) = store();
        let k = s.load_or_create_keys(&counter_rand()).unwrap();
        s.save_roster(&genesis(member_of(&k, "laptop"), "mine".into(), &k))
            .unwrap();

        for gone in [&[SIGN_KEY, KX_KEY][..], &[SIGN_KEY][..], &[KX_KEY][..]] {
            let (_t2, s2) = store();
            std::fs::create_dir_all(s2.dir()).unwrap();
            for f in [SIGN_KEY, KX_KEY, ROSTER] {
                std::fs::copy(s.dir().join(f), s2.dir().join(f)).unwrap();
            }
            for f in gone {
                std::fs::remove_file(s2.dir().join(f)).unwrap();
            }
            let err = s2
                .load_or_create_keys(&|| panic!("不该生成新钥匙"))
                .err()
                .unwrap_or_else(|| panic!("少了 {gone:?} 该报错"));
            assert!(
                err.to_string().contains("钥匙丢了，但组名单还在"),
                "{gone:?}: {err}"
            );
            for f in [SIGN_KEY, KX_KEY] {
                if gone.contains(&f) {
                    assert!(!s2.dir().join(f).exists(), "{f} 不该被补上");
                }
            }
        }
    }

    /// 写入顺序：`kx.key` 先落盘。让 `kx.key` 那一步失败，`sign.key` 就绝不
    /// 能出现——顺序反过来的话，这里会留下一把孤零零的 `sign.key`，下次启动
    /// 就是「有 sign 没 kx」那个报错。
    #[test]
    fn kx_key_is_written_before_sign_key() {
        let (_t, s) = store();
        std::fs::create_dir_all(s.dir()).unwrap();
        std::fs::create_dir(s.dir().join("kx.key.tmp")).unwrap();
        assert!(s.load_or_create_keys(&counter_rand()).is_err());
        assert!(!s.dir().join(SIGN_KEY).exists());
    }

    /// 别的进程拿着锁的时候，这边等它，然后读它生成的那一对，不自己再生成。
    #[test]
    fn a_held_keys_lock_makes_the_other_creator_wait_and_reuse_the_keys() {
        let (t, s) = store();
        std::fs::create_dir_all(s.dir()).unwrap();
        let lock = KeysLock::acquire(
            &s.dir().join(KEYS_LOCK),
            std::time::Duration::from_secs(1),
            LOCK_STALE,
        )
        .unwrap();
        let dir = t.path().join("mesh");
        let waiter = std::thread::spawn(move || {
            Store::at(dir)
                .load_or_create_keys(&|| [5u8; 32])
                .map(|k| k.sign_pub())
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!waiter.is_finished(), "锁还在，不该往下走");
        assert!(!s.dir().join(SIGN_KEY).exists());
        // 「另一个进程」在锁里把钥匙写好，然后放锁。
        let mine = MachineKeys::from_seeds([7; 32], [8; 32]).unwrap();
        write_atomic(&s.dir().join(KX_KEY), STANDARD.encode([8u8; 32]).as_bytes()).unwrap();
        write_atomic(
            &s.dir().join(SIGN_KEY),
            STANDARD.encode([7u8; 32]).as_bytes(),
        )
        .unwrap();
        drop(lock);
        assert_eq!(waiter.join().unwrap().unwrap(), mine.sign_pub());
        assert!(!s.dir().join(KEYS_LOCK).exists(), "用完要删锁");
    }

    #[test]
    fn a_stale_keys_lock_left_by_a_crash_is_broken() {
        let (_t, s) = store();
        std::fs::create_dir_all(s.dir()).unwrap();
        let f = std::fs::File::create(s.dir().join(KEYS_LOCK)).unwrap();
        f.set_modified(std::time::SystemTime::now() - LOCK_STALE * 2)
            .unwrap();
        drop(f);
        assert!(s.load_or_create_keys(&counter_rand()).is_ok());
    }

    /// 两个进程同时第一次建钥匙：拿到的必须是同一对，磁盘上也是那一对。
    #[test]
    fn concurrent_creators_end_up_with_one_matching_pair() {
        for _ in 0..20 {
            let t = tempfile::tempdir().unwrap();
            let dir = t.path().join("mesh");
            let spawn = |seed: u8| {
                let d = dir.clone();
                std::thread::spawn(move || {
                    let n = Cell::new(seed);
                    Store::at(d)
                        .load_or_create_keys(&move || {
                            n.set(n.get().wrapping_add(1));
                            [n.get(); 32]
                        })
                        .map(|k| (k.sign_pub(), k.kx_pub()))
                        .unwrap()
                })
            };
            let (a, b) = (spawn(10), spawn(100));
            let (a, b) = (a.join().unwrap(), b.join().unwrap());
            assert_eq!(a, b);
            let disk = Store::at(dir).load_or_create_keys(&|| [1; 32]).unwrap();
            assert_eq!((disk.sign_pub(), disk.kx_pub()), a);
        }
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
        assert!(s.set_name("a\x1bb").is_err());
        assert!(s.set_name("a\u{202e}b").is_err());
        assert!(s.set_name("a\u{200b}b").is_err());
    }

    #[test]
    fn hostnames_are_tidied_into_valid_names() {
        assert_eq!(tidy_name("Leis-MacBook.local"), "Leis-MacBook");
        assert_eq!(tidy_name(""), "dct");
        assert_eq!(tidy_name("a/b"), "ab");
        assert_eq!(tidy_name(&"y".repeat(80)).chars().count(), MAX_NAME_LEN);
        assert_eq!(tidy_name("mac\u{202e}\u{200b}\x1bbook"), "macbook");
    }

    #[test]
    fn the_default_dir_sits_next_to_the_other_dct_files() {
        assert_eq!(
            dir_for_socket(Path::new("/h/.dct/daemon.sock")),
            PathBuf::from("/h/.dct/mesh")
        );
    }
}
