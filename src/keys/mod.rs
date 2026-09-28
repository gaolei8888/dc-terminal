//! dct 的两把钥匙（设计第 2 节）：自动钥匙签 `self` / `physical`，用户钥匙签
//! `content` / `money`，每次都要当场按 Touch ID。私钥在 Mac 安全芯片里，磁盘上只有
//! 安全芯片加密过的「把手」（只有这台 Mac 能用）和公钥。
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use dct_brain::sign::{key_id, SignError, Signer, TrustedKey};
use dct_brain::tier::SignerRole;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod mac;

/// 安全芯片的三件事。抽出来是为了测试能换成假的——CI 的 Mac 是虚拟机，没有安全芯片。
pub trait SecureEnclave {
    fn available(&self) -> bool;
    /// 建一把新钥匙，返回（把手，SEC1 公钥）。`biometric` 为真时每次签名都要按 Touch ID。
    fn create(&self, biometric: bool) -> Result<(Vec<u8>, [u8; 65]), SignError>;
    fn sign(&self, blob: &[u8], msg: &[u8], reason: &str) -> Result<[u8; 64], SignError>;
}

#[cfg(not(target_os = "macos"))]
struct NoEnclave;

#[cfg(not(target_os = "macos"))]
impl SecureEnclave for NoEnclave {
    fn available(&self) -> bool {
        false
    }
    fn create(&self, _: bool) -> Result<(Vec<u8>, [u8; 65]), SignError> {
        Err(SignError::Unavailable)
    }
    fn sign(&self, _: &[u8], _: &[u8], _: &str) -> Result<[u8; 64], SignError> {
        Err(SignError::Unavailable)
    }
}

/// 这台机器上的安全芯片。没有 passkey 的平台上，对外和动钱两档直接关闭，
/// 不退化成「点一下同意」（控制层设计）。
pub fn platform_enclave() -> Box<dyn SecureEnclave> {
    #[cfg(target_os = "macos")]
    {
        Box::new(mac::MacEnclave)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(NoEnclave)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicEntry {
    pub key_id: String,
    /// base64 的 SEC1 未压缩公钥。
    pub public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicKeys {
    pub auto: PublicEntry,
    pub user: PublicEntry,
}

pub struct KeyStore {
    dir: PathBuf,
}

fn entry(public: &[u8; 65]) -> PublicEntry {
    PublicEntry { key_id: key_id(public), public_key: STANDARD.encode(public) }
}

fn decode(e: &PublicEntry) -> Result<[u8; 65]> {
    let raw = STANDARD.decode(&e.public_key).context("公钥文件坏了")?;
    raw.try_into().map_err(|_| anyhow!("公钥长度不对"))
}

/// `init` 建钥匙失败时给用户看的话：`SignError` 的 `Display` 是从「签名」的角度写的
/// （比如 `Other` 变体套着「签名失败：」），这里是在「建钥匙」，得换个说法。
fn create_failed_message(e: &SignError) -> String {
    match e {
        SignError::Other(m) => format!("创建钥匙失败：{m}"),
        other => format!("创建钥匙失败：{other}"),
    }
}

impl KeyStore {
    pub fn at(dir: PathBuf) -> Self {
        KeyStore { dir }
    }

    /// `~/.dct/keys`，跟 daemon.sock 同一个目录下。
    pub fn default_dir() -> PathBuf {
        crate::proto::socket_path()
            .parent()
            .map(|p| p.join("keys"))
            .unwrap_or_else(|| PathBuf::from("keys"))
    }

    fn blob_path(&self, role: SignerRole) -> PathBuf {
        self.dir.join(match role {
            SignerRole::Auto => "auto.se",
            SignerRole::User => "user.se",
        })
    }

    fn public_path(&self) -> PathBuf {
        self.dir.join("public.json")
    }

    pub fn public_keys(&self) -> Result<Option<PublicKeys>> {
        match std::fs::read(self.public_path()) {
            Ok(b) => Ok(Some(serde_json::from_slice(&b).context("公钥文件坏了")?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 有钥匙把手文件、却没有 public.json 的那些角色的文件名。正常情况下永远是空的
    /// （两者一起建、一起在的），非空说明状态半途而废——可能是 public.json 被误删，
    /// 也可能是有人手动拷过来一半。这种时候不能猜着重建，猜错了就把还能用的把手
    /// 文件覆盖掉了。
    fn stray_handles(&self) -> Vec<&'static str> {
        [(SignerRole::Auto, "auto.se"), (SignerRole::User, "user.se")]
            .into_iter()
            .filter(|(role, _)| self.blob_path(*role).exists())
            .map(|(_, name)| name)
            .collect()
    }

    /// 已经有钥匙就原样返回，**不重建**：重建会换公钥，配对过的 dco 就全都不认了。
    pub fn init(&self, se: &dyn SecureEnclave) -> Result<PublicKeys> {
        if let Some(k) = self.public_keys()? {
            return Ok(k);
        }
        let stray = self.stray_handles();
        if !stray.is_empty() {
            return Err(anyhow!(
                "发现钥匙文件（{}），但公钥记录 public.json 不见了。为了不覆盖已经有的钥匙，\
                 这里不会自动建新的，请先检查 {}",
                stray.join("、"),
                self.dir.display()
            ));
        }
        if !se.available() {
            return Err(anyhow!("这台电脑没有安全芯片，没法建钥匙"));
        }
        std::fs::create_dir_all(&self.dir)?;
        self.create_both(se).inspect_err(|_| {
            // 别把没建完的钥匙留在磁盘上：下次 init 才能干净地重来，
            // 不然会被上面那条 stray_handles 检查拦住。
            for role in [SignerRole::Auto, SignerRole::User] {
                let _ = std::fs::remove_file(self.blob_path(role));
            }
        })
    }

    fn create_both(&self, se: &dyn SecureEnclave) -> Result<PublicKeys> {
        let mut made = Vec::new();
        for (role, biometric) in [(SignerRole::Auto, false), (SignerRole::User, true)] {
            let (blob, public) = se.create(biometric).map_err(|e| anyhow!(create_failed_message(&e)))?;
            let mut f = crate::sys::fs::create_private(&self.blob_path(role))?;
            f.write_all(&blob)?;
            made.push(entry(&public));
        }
        let keys = PublicKeys { user: made.pop().unwrap(), auto: made.pop().unwrap() };
        self.write_public_atomic(&keys)?;
        Ok(keys)
    }

    /// 先写临时文件、再原地改名覆盖 public.json：改名在同一个文件系统上是原子的，
    /// 半路崩了也不会留下一份写了一半、解析不出来的 public.json。公钥不是秘密，
    /// 权限沿用 `std::fs::write` 的默认行为，不额外收紧。
    fn write_public_atomic(&self, keys: &PublicKeys) -> Result<()> {
        let tmp = self.dir.join(format!(".public.json.tmp.{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(keys)?)?;
        std::fs::rename(&tmp, self.public_path())?;
        Ok(())
    }

    /// 两把公钥，给验签用（批准记录、执行票；以后配对 dco 时也交给它）。
    pub fn trusted(&self) -> Result<Vec<TrustedKey>> {
        let k = self.public_keys()?.ok_or_else(|| anyhow!("还没建钥匙，先运行 dct keys init"))?;
        Ok(vec![
            TrustedKey { role: SignerRole::Auto, public_key: decode(&k.auto)? },
            TrustedKey { role: SignerRole::User, public_key: decode(&k.user)? },
        ])
    }

    pub fn signer<'a>(&self, role: SignerRole, se: &'a dyn SecureEnclave) -> Result<EnclaveSigner<'a>> {
        let keys = self.public_keys()?.ok_or_else(|| anyhow!("还没建钥匙，先运行 dct keys init"))?;
        let public = decode(match role {
            SignerRole::Auto => &keys.auto,
            SignerRole::User => &keys.user,
        })?;
        let blob = std::fs::read(self.blob_path(role)).context("读不到钥匙文件")?;
        Ok(EnclaveSigner { role, blob, public, se })
    }
}

pub struct EnclaveSigner<'a> {
    role: SignerRole,
    blob: Vec<u8>,
    public: [u8; 65],
    se: &'a dyn SecureEnclave,
}

impl Signer for EnclaveSigner<'_> {
    fn role(&self) -> SignerRole {
        self.role
    }
    fn public_key(&self) -> [u8; 65] {
        self.public
    }
    fn sign(&self, msg: &[u8], reason: &str) -> Result<[u8; 64], SignError> {
        self.se.sign(&self.blob, msg, reason)
    }
}

/// `dct keys init | show | test`。
pub fn run_cli(args: &[String]) -> i32 {
    let ks = KeyStore::at(KeyStore::default_dir());
    let se = platform_enclave();
    let r = match args.first().map(String::as_str) {
        Some("init") => ks.init(se.as_ref()).map(|k| {
            println!("两把钥匙已就绪：\n  自动钥匙 {}\n  指纹钥匙 {}", k.auto.key_id, k.user.key_id);
        }),
        Some("show") => ks.public_keys().and_then(|k| {
            let k = k.ok_or_else(|| anyhow!("还没建钥匙，先运行 dct keys init"))?;
            println!("{}", serde_json::to_string_pretty(&k)?);
            Ok(())
        }),
        Some("test") => (|| {
            let s = ks.signer(SignerRole::User, se.as_ref())?;
            let sig = s
                .sign(b"dct keys test", "测试：dct 用指纹签一条测试消息")
                .map_err(|e| anyhow!("{e}"))?;
            let signature = dct_brain::sign::Signature {
                key_id: key_id(&s.public_key()),
                alg: dct_brain::sign::ALG.into(),
                sig: STANDARD.encode(sig),
            };
            dct_brain::sign::verify_sig(&signature, b"dct keys test", &ks.trusted()?)
                .map_err(|e| anyhow!("{e}"))?;
            println!("指纹钥匙能用：签名已核对");
            Ok(())
        })(),
        _ => {
            eprintln!("用法：dct keys init | show | test");
            return 2;
        }
    };
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dct_brain::sign::soft::SoftSigner;
    use dct_brain::sign::Signer;
    use std::cell::RefCell;

    /// 假的安全芯片：「把手」就是一个字节的种子，签名用软件钥匙。
    struct FakeEnclave {
        next_seed: RefCell<u8>,
        cancel_user: bool,
        /// 建用户钥匙（`biometric == true`）时故意失败一次，用来测「自动钥匙建好了、
        /// 用户钥匙没建成」这一半路失败要怎么收场。
        fail_user_create: bool,
    }

    impl FakeEnclave {
        fn new() -> Self {
            FakeEnclave { next_seed: RefCell::new(1), cancel_user: false, fail_user_create: false }
        }
        fn soft(blob: &[u8]) -> SoftSigner {
            let role = if blob[1] == 1 { SignerRole::User } else { SignerRole::Auto };
            SoftSigner::from_seed(role, blob[0])
        }
    }

    impl SecureEnclave for FakeEnclave {
        fn available(&self) -> bool {
            true
        }
        fn create(&self, biometric: bool) -> Result<(Vec<u8>, [u8; 65]), SignError> {
            if biometric && self.fail_user_create {
                return Err(SignError::Other("模拟：安全芯片建用户钥匙失败".into()));
            }
            let mut s = self.next_seed.borrow_mut();
            let blob = vec![*s, biometric as u8];
            *s += 1;
            Ok((blob.clone(), Self::soft(&blob).public_key()))
        }
        fn sign(&self, blob: &[u8], msg: &[u8], reason: &str) -> Result<[u8; 64], SignError> {
            if blob[1] == 1 && self.cancel_user {
                return Err(SignError::Cancelled);
            }
            Self::soft(blob).sign(msg, reason)
        }
    }

    #[test]
    fn init_creates_both_keys_once() {
        let d = tempfile::tempdir().unwrap();
        let ks = KeyStore::at(d.path().join("keys"));
        let se = FakeEnclave::new();
        let first = ks.init(&se).unwrap();
        assert_ne!(first.auto.key_id, first.user.key_id);
        assert!(d.path().join("keys/auto.se").exists());
        assert!(d.path().join("keys/user.se").exists());
        // 第二次不重建：公钥不变，否则配对过的 dco 全都不认了。
        let second = ks.init(&se).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn signers_sign_with_the_right_role_and_verify_against_the_public_file() {
        let d = tempfile::tempdir().unwrap();
        let ks = KeyStore::at(d.path().join("keys"));
        let se = FakeEnclave::new();
        ks.init(&se).unwrap();
        let trusted = ks.trusted().unwrap();
        for role in [SignerRole::Auto, SignerRole::User] {
            let s = ks.signer(role, &se).unwrap();
            assert_eq!(s.role(), role);
            let sig = dct_brain::sign::make_signature_for_tests(&s, b"hello");
            assert_eq!(dct_brain::sign::verify_sig(&sig, b"hello", &trusted), Ok(role));
        }
    }

    #[test]
    fn a_cancelled_touch_id_is_reported_as_cancelled() {
        let d = tempfile::tempdir().unwrap();
        let ks = KeyStore::at(d.path().join("keys"));
        let mut se = FakeEnclave::new();
        ks.init(&se).unwrap();
        se.cancel_user = true;
        let s = ks.signer(SignerRole::User, &se).unwrap();
        assert_eq!(s.sign(b"x", "发到 TikTok"), Err(SignError::Cancelled));
    }

    #[test]
    fn no_keys_yet_means_no_signer() {
        let d = tempfile::tempdir().unwrap();
        let ks = KeyStore::at(d.path().join("keys"));
        assert!(ks.public_keys().unwrap().is_none());
        assert!(ks.signer(SignerRole::Auto, &FakeEnclave::new()).is_err());
    }

    #[test]
    fn a_failed_create_leaves_no_trace_and_a_retry_succeeds() {
        let d = tempfile::tempdir().unwrap();
        let keys_dir = d.path().join("keys");
        let ks = KeyStore::at(keys_dir.clone());
        let mut se = FakeEnclave::new();
        se.fail_user_create = true;

        // 自动钥匙建成了，用户钥匙没建成：这次 init 必须整体失败，且不留下
        // 半成品——否则下一次 init 会被 stray_handles 检查拦住，永远建不成。
        assert!(ks.init(&se).is_err());
        assert!(!keys_dir.join("public.json").exists());
        assert!(!keys_dir.join("auto.se").exists());
        assert!(!keys_dir.join("user.se").exists());

        se.fail_user_create = false;
        let keys = ks.init(&se).unwrap();
        assert_ne!(keys.auto.key_id, keys.user.key_id);
        assert!(keys_dir.join("public.json").exists());
        assert!(keys_dir.join("auto.se").exists());
        assert!(keys_dir.join("user.se").exists());
    }

    #[test]
    fn init_refuses_when_a_handle_file_exists_without_public_json() {
        let d = tempfile::tempdir().unwrap();
        let keys_dir = d.path().join("keys");
        std::fs::create_dir_all(&keys_dir).unwrap();
        std::fs::write(keys_dir.join("auto.se"), b"leftover from somewhere").unwrap();
        let ks = KeyStore::at(keys_dir.clone());

        let err = ks.init(&FakeEnclave::new()).unwrap_err();
        assert!(err.to_string().contains("auto.se"), "message was: {err}");

        // 拒绝之后什么都不该改：把手文件原样在，没有新建 user.se 或 public.json。
        assert!(keys_dir.join("auto.se").exists());
        assert!(!keys_dir.join("user.se").exists());
        assert!(!keys_dir.join("public.json").exists());
    }

    #[test]
    fn chip_message_maps_locked_terminal_status_to_an_actionable_sentence() {
        assert_eq!(
            dct_brain::sign::chip_message(-25308),
            "安全芯片现在不能用：请在已解锁的 Mac 上、从普通终端窗口运行"
        );
        assert_eq!(dct_brain::sign::chip_message(-1), "安全芯片出错（代码 -1）");
    }
}
