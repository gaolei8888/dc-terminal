### Task 7: Mac 安全芯片的两把钥匙，`dct keys`

**Files:**
- Create: `build.rs`
- Create: `swift/Keys.swift`
- Create: `src/keys/mod.rs`
- Create: `src/keys/mac.rs`
- Modify: `src/lib.rs`（加 `pub mod keys;`）
- Modify: `src/main.rs`（加 `Some("keys")` 分支）

**Interfaces:**
- Consumes: `dct_brain::sign::{Signer, SignError, TrustedKey, key_id}`、`dct_brain::tier::SignerRole`、`crate::sys::fs::create_private`、`crate::proto::socket_path`。
- Produces:
  - `keys::SecureEnclave` trait：`available(&self) -> bool`、`create(&self, biometric: bool) -> Result<(Vec<u8>, [u8; 65]), SignError>`、`sign(&self, blob: &[u8], msg: &[u8], reason: &str) -> Result<[u8; 64], SignError>`。
  - `keys::platform_enclave() -> Box<dyn SecureEnclave>`：macOS 上是真的安全芯片，其他平台 `available()` 为 `false`、其余方法返回 `SignError::Unavailable`。
  - `keys::KeyStore`：`KeyStore::at(dir: PathBuf)`、`KeyStore::default_dir() -> PathBuf`（`~/.dct/keys`，跟 `daemon.sock` 同一个目录下）、`init(&self, se: &dyn SecureEnclave) -> anyhow::Result<PublicKeys>`（已有就原样返回，不重建）、`public_keys(&self) -> anyhow::Result<Option<PublicKeys>>`、`trusted(&self) -> anyhow::Result<Vec<TrustedKey>>`、`signer<'a>(&self, role: SignerRole, se: &'a dyn SecureEnclave) -> anyhow::Result<EnclaveSigner<'a>>`。
  - `keys::PublicKeys { auto: PublicEntry, user: PublicEntry }`，`PublicEntry { key_id: String, public_key: String /* base64 SEC1 */ }`；存在 `~/.dct/keys/public.json`（公钥，不是秘密，普通写入）。
  - `keys::EnclaveSigner<'a>`：实现 `dct_brain::sign::Signer`。
  - `keys::run_cli(args: &[String]) -> i32`：`dct keys init | show | test`。

**钥匙怎么存：** 用 CryptoKit 的 `SecureEnclave.P256.Signing.PrivateKey` 建钥匙，把它的 `dataRepresentation`（安全芯片加密过的「钥匙把手」，**只有这台 Mac 的安全芯片能用，拿走没用**）存成 `~/.dct/keys/auto.se`、`user.se`，用 `sys::fs::create_private` 写（只有属主能读）。用户钥匙建的时候加 `.biometryCurrentSet`：**每次签名都要当场按 Touch ID**，换过指纹就作废。

**⚠️ 跟设计的一处差距（要在交付时告诉用户）：** 设计里写「自动钥匙只有签过名的 dct 程序本身能用，agent 拿不到」。要做到这一点，钥匙得放进带访问组的钥匙串，这要求 dct 用 Apple 开发者证书签名、带 provisioning profile。这一步本计划不做：自动钥匙的把手文件只受文件权限保护，同一个系统账号下的 agent 如果去读它，可以自己签出 `self` / `physical` 档的票。**用户钥匙不受影响**：签名必须当场按 Touch ID，对外和动钱的票伪造不了。后续有开发者证书时，把两个把手换进钥匙串访问组即可，格式不变。

**CI 说明：** GitHub 的 macOS 机器是虚拟机，没有安全芯片。所以涉及真实安全芯片的只做手动验证（`dct keys test`），自动测试一律用假的 `SecureEnclave`（包一层 `SoftSigner`）。

- [ ] **Step 1: 写失败的测试**

`src/keys/mod.rs` 里先放测试（实现写在它上面）：

```rust
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
```

上面用到的 `make_signature_for_tests` 在 `dct-brain` 里补一个（`make_signature` 是 `pub(crate)`，dct 用不到）。在 `crates/dct-brain/src/sign.rs` 的 `soft` 模块**外面**加：

```rust
/// 测试用：对任意消息签一个 `Signature`，好在 crate 外面验证 `Signer` 的实现。
#[cfg(any(test, feature = "soft-signer"))]
pub fn make_signature_for_tests(s: &dyn Signer, msg: &[u8]) -> Signature {
    make_signature(s, msg, "").expect("sign")
}
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test --lib keys::`
Expected: 编译失败，`KeyStore` 等没有定义。

- [ ] **Step 3: 写 Swift 和 build.rs**

`swift/Keys.swift`：

```swift
// dct 的两把钥匙放在 Mac 的安全芯片里（设计：docs/superpowers/specs/2026-09-27-dct-brain-design.md 第 2 节）。
// 私钥永远不出安全芯片；我们拿到的 dataRepresentation 是它加密过的「把手」，只有这台 Mac 能用。
// 状态码：0 成功，1 没有安全芯片，2 把手不对，3 用户没通过指纹 / 取消，4 缓冲区太小，5 其它错误。
import CryptoKit
import Foundation
import LocalAuthentication
import Security

@_cdecl("dct_se_available")
public func dct_se_available() -> Bool {
    SecureEnclave.isAvailable
}

@_cdecl("dct_se_create")
public func dct_se_create(
    _ biometric: Bool,
    _ blobOut: UnsafeMutablePointer<UInt8>, _ blobCap: Int, _ blobLen: UnsafeMutablePointer<Int>,
    _ pubOut: UnsafeMutablePointer<UInt8>
) -> Int32 {
    guard SecureEnclave.isAvailable else { return 1 }
    var flags: SecAccessControlCreateFlags = [.privateKeyUsage]
    if biometric { flags.insert(.biometryCurrentSet) }
    var err: Unmanaged<CFError>?
    guard let ac = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, flags, &err) else {
        return 5
    }
    do {
        let key = try SecureEnclave.P256.Signing.PrivateKey(accessControl: ac)
        let blob = key.dataRepresentation
        guard blob.count <= blobCap else { return 4 }
        blob.copyBytes(to: blobOut, count: blob.count)
        blobLen.pointee = blob.count
        let pub = key.publicKey.x963Representation  // 65 字节，0x04 开头
        guard pub.count == 65 else { return 5 }
        pub.copyBytes(to: pubOut, count: 65)
        return 0
    } catch {
        return 5
    }
}

@_cdecl("dct_se_sign")
public func dct_se_sign(
    _ blob: UnsafePointer<UInt8>, _ blobLen: Int,
    _ msg: UnsafePointer<UInt8>, _ msgLen: Int,
    _ reason: UnsafePointer<CChar>,
    _ sigOut: UnsafeMutablePointer<UInt8>
) -> Int32 {
    guard SecureEnclave.isAvailable else { return 1 }
    let ctx = LAContext()
    ctx.localizedReason = String(cString: reason)  // Touch ID 弹窗里显示的那句人话
    let key: SecureEnclave.P256.Signing.PrivateKey
    do {
        key = try SecureEnclave.P256.Signing.PrivateKey(
            dataRepresentation: Data(bytes: blob, count: blobLen), authenticationContext: ctx)
    } catch {
        return 2
    }
    do {
        let sig = try key.signature(for: Data(bytes: msg, count: msgLen))
        sig.rawRepresentation.copyBytes(to: sigOut, count: 64)  // r||s
        return 0
    } catch {
        return 3
    }
}
```

`build.rs`（照 dc-octo 的做法，只在 macOS 上编，别的平台直接返回）：

```rust
//! 只在 macOS 上把 swift/*.swift 编成静态库链进 dct：安全芯片只能从 CryptoKit 用。
//! 别的平台什么都不做——Windows、Linux 的构建不需要 Swift，也不该需要。
use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        a => panic!("unsupported arch {a}"),
    };
    let mut sources: Vec<PathBuf> = std::fs::read_dir("swift")
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "swift"))
        .collect();
    sources.sort();
    println!("cargo:rerun-if-changed=swift");
    for s in &sources {
        println!("cargo:rerun-if-changed={}", s.display());
    }
    let lib = out.join("libDctMac.a");
    let status = Command::new("xcrun")
        .args(["swiftc", "-parse-as-library", "-emit-library", "-static", "-module-name", "DctMac", "-swift-version", "5", "-O"])
        .args(["-target", &format!("{arch}-apple-macos13.0"), "-o"])
        .arg(&lib)
        .args(&sources)
        .status()
        .expect("run xcrun swiftc");
    assert!(status.success(), "swiftc failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=DctMac");
    let sdk = Command::new("xcrun").args(["--show-sdk-path"]).output().expect("xcrun --show-sdk-path").stdout;
    println!("cargo:rustc-link-search=native={}/usr/lib/swift", String::from_utf8(sdk).unwrap().trim());
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    for f in ["CryptoKit", "Foundation", "LocalAuthentication", "Security"] {
        println!("cargo:rustc-link-lib=framework={f}");
    }
}
```

- [ ] **Step 4: 写 Rust 实现**

`src/keys/mac.rs`：

```rust
//! swift/Keys.swift 的绑定。Swift 的 `Int` 是 64 位有符号，对应 `isize`。
use dct_brain::sign::SignError;
use std::ffi::CString;
use std::os::raw::c_char;

extern "C" {
    fn dct_se_available() -> bool;
    fn dct_se_create(biometric: bool, blob_out: *mut u8, blob_cap: isize, blob_len: *mut isize, pub_out: *mut u8) -> i32;
    fn dct_se_sign(blob: *const u8, blob_len: isize, msg: *const u8, msg_len: isize, reason: *const c_char, sig_out: *mut u8) -> i32;
}

fn status(code: i32) -> SignError {
    match code {
        1 => SignError::Unavailable,
        3 => SignError::Cancelled,
        2 => SignError::Other("钥匙文件不对，可能是从别的电脑拷来的".into()),
        4 => SignError::Other("钥匙太大".into()),
        c => SignError::Other(format!("安全芯片返回 {c}")),
    }
}

pub struct MacEnclave;

impl super::SecureEnclave for MacEnclave {
    fn available(&self) -> bool {
        unsafe { dct_se_available() }
    }

    fn create(&self, biometric: bool) -> Result<(Vec<u8>, [u8; 65]), SignError> {
        let mut blob = vec![0u8; 1024];
        let mut len: isize = 0;
        let mut public = [0u8; 65];
        let rc = unsafe { dct_se_create(biometric, blob.as_mut_ptr(), blob.len() as isize, &mut len, public.as_mut_ptr()) };
        if rc != 0 {
            return Err(status(rc));
        }
        blob.truncate(len as usize);
        Ok((blob, public))
    }

    fn sign(&self, blob: &[u8], msg: &[u8], reason: &str) -> Result<[u8; 64], SignError> {
        // 人话里不该有 NUL；有就去掉，别让整次签名失败。
        let reason = CString::new(reason.replace('\0', "")).unwrap();
        let mut sig = [0u8; 64];
        let rc = unsafe {
            dct_se_sign(blob.as_ptr(), blob.len() as isize, msg.as_ptr(), msg.len() as isize, reason.as_ptr(), sig.as_mut_ptr())
        };
        if rc != 0 {
            return Err(status(rc));
        }
        Ok(sig)
    }
}
```

`src/keys/mod.rs`（放在 Step 1 的测试模块上面）：

```rust
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

struct NoEnclave;

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
```

`src/lib.rs` 加 `pub mod keys;`（放在字母序对应的位置）。`src/main.rs` 的子命令 `match` 里、`Some("llm")` 那一行前面加：

```rust
        Some("keys") => std::process::exit(dct::keys::run_cli(&args[1..])),
```

如果 `anyhow` / `base64` 还不在 dct 的 `[dependencies]` 里，按 Cargo.lock 已有的版本加上（`base64 = "0.22"`）。

- [ ] **Step 5: 跑测试，确认通过；检查各平台都能编**

Run: `cargo test --lib keys:: && cargo test --workspace -q 2>&1 | grep -E "test result|FAILED" && cargo clippy --workspace --all-targets -q -- -D warnings && cargo check --target x86_64-pc-windows-msvc --all-targets -q`
Expected: 4 个 keys 测试 PASS；workspace 全绿；clippy 没有警告；Windows 检查能过（Windows 上 `build.rs` 直接返回，没有 Swift）。

- [ ] **Step 6: 在真 Mac 上手动验证安全芯片**

Run: `cargo build -q && ./target/debug/dct keys init && ./target/debug/dct keys show && ./target/debug/dct keys test`
Expected: `init` 打印两个 `sha256:` 开头的指纹；`show` 打印公钥 JSON；`test` **弹出 Touch ID**，提示文字是「测试：dct 用指纹签一条测试消息」，按下后打印「指纹钥匙能用：签名已核对」；取消则打印「没有通过指纹确认」、退出码 1。
注意：这一步会在**你自己的** `~/.dct/keys` 里建真钥匙。由执行任务的 agent 把命令和结果贴给主会话，Touch ID 那一下由用户本人按。

- [ ] **Step 7: Commit**

```bash
git add build.rs swift/Keys.swift src/keys src/lib.rs src/main.rs Cargo.toml Cargo.lock crates/dct-brain/src/sign.rs
git commit -m "feat(keys): an automatic key and a Touch ID key in the Mac's Secure Enclave"
```

---

