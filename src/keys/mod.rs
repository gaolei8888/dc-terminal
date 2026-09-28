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

    /// 已经有钥匙就原样返回，**不重建**：重建会换公钥，配对过的 dco 就全都不认了。
    pub fn init(&self, se: &dyn SecureEnclave) -> Result<PublicKeys> {
        if let Some(k) = self.public_keys()? {
            return Ok(k);
        }
        if !se.available() {
            return Err(anyhow!("这台电脑没有安全芯片，没法建钥匙"));
        }
        std::fs::create_dir_all(&self.dir)?;
        let mut made = Vec::new();
        for (role, biometric) in [(SignerRole::Auto, false), (SignerRole::User, true)] {
            let (blob, public) = se.create(biometric).map_err(|e| anyhow!("{e}"))?;
            let mut f = crate::sys::fs::create_private(&self.blob_path(role))?;
            f.write_all(&blob)?;
            made.push(entry(&public));
        }
        let keys = PublicKeys { user: made.pop().unwrap(), auto: made.pop().unwrap() };
        std::fs::write(self.public_path(), serde_json::to_vec_pretty(&keys)?)?;
        Ok(keys)
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
    }

    impl FakeEnclave {
        fn new() -> Self {
            FakeEnclave { next_seed: RefCell::new(1), cancel_user: false }
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
}
