//! `dct-srv token keygen` / `dct-srv token mint`。
//!
//! **这两个子命令只给开发和本机端到端测试用。** 正式令牌由网关签发
//! （任务 4 的契约）——这里生成的 issuer 钥匙不进任何生产环境的信任列表。

use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use dct_mesh::relay_token::{issue, Claims};
use p256::ecdsa::SigningKey;

/// 生成一对 issuer 钥匙：32 字节签名种子和它的 65 字节 SEC1 公钥。
///
/// `SigningKey::from_slice` 只在种子是全零或者不小于曲线阶时才失败——
/// 撞上的概率小到可以忽略，重抽一遍比向调用方交代一个几乎不可能发生的
/// 错误更简单。
pub fn generate() -> ([u8; 32], [u8; 65]) {
    loop {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).expect("系统随机数不可用");
        if let Ok(sk) = SigningKey::from_slice(&seed) {
            let point = sk.verifying_key().to_encoded_point(false);
            let mut pk = [0u8; 65];
            pk.copy_from_slice(point.as_bytes());
            return (seed, pk);
        }
    }
}

/// `seed`/`pub_key` 编码成 base64 之后的样子——`write_issuer_files` 和
/// 单元测试都要这两行，拆出来避免重复。
pub fn encode_seed(seed: &[u8; 32]) -> String {
    STANDARD.encode(seed)
}

pub fn encode_pub(pub_key: &[u8; 65]) -> String {
    STANDARD.encode(pub_key)
}

/// `token keygen --out <dir>` 写出的两个文件：
/// - `<dir>/issuer.key`：32 字节标量的 base64，0600；
/// - `<dir>/issuer.pub`：可以直接当 relay-keys 文件用。
pub fn write_issuer_files(dir: &Path, seed: &[u8; 32], pub_key: &[u8; 65]) -> std::io::Result<()> {
    write_secret_file(&dir.join("issuer.key"), &encode_seed(seed))?;
    std::fs::write(dir.join("issuer.pub"), format!("{}\n", encode_pub(pub_key)))
}

/// 私钥落盘：`0600`，先写全部内容再关，跟 `PublishKeys::save` 同一个道理——
/// 这里不用临时文件改名那一套，因为这条命令只在本机、只跑这一次，不存在
/// 「运行中的进程正在读它」的并发场景。
fn write_secret_file(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    std::io::Write::write_all(&mut f, contents.as_bytes())
}

/// 把 `issuer.key` 文件的内容（一行 base64 标量）还原成签名钥匙。
pub fn signing_key_from_seed_b64(raw: &str) -> Result<SigningKey, String> {
    let bytes = STANDARD
        .decode(raw.trim())
        .map_err(|e| format!("不是合法的 base64：{e}"))?;
    SigningKey::from_slice(&bytes).map_err(|_| "不是合法的签名种子".to_string())
}

/// `token mint`：签一个令牌。`now` 由调用方给（同 `PublishKeys::add` 那条
/// 「不碰系统时钟」的规矩），好让这条纯函数不用起进程就能测。
pub fn mint(
    signing_key: &SigningKey,
    account: &str,
    endpoint: &str,
    ttl_secs: u64,
    now: u64,
) -> String {
    let claims = Claims {
        account: account.to_string(),
        endpoint: endpoint.to_string(),
        exp: now.saturating_add(ttl_secs),
    };
    issue(&claims, signing_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dct_mesh::relay_token::verify;

    #[test]
    fn generate_then_round_trips_through_base64() {
        let (seed, pk) = generate();
        let sk = signing_key_from_seed_b64(&encode_seed(&seed)).unwrap();
        assert_eq!(sk.verifying_key().to_encoded_point(false).as_bytes(), pk);
    }

    #[test]
    fn minted_token_verifies_under_the_generated_public_key() {
        let (seed, pk) = generate();
        let sk = signing_key_from_seed_b64(&encode_seed(&seed)).unwrap();
        let token = mint(&sk, "acct", "c-abc", 3600, 1_000);
        let claims = verify(&token, &[pk], 1_000).unwrap();
        assert_eq!(claims.account, "acct");
        assert_eq!(claims.endpoint, "c-abc");
        assert_eq!(claims.exp, 4_600);
    }

    #[test]
    fn a_minted_token_expires_after_its_ttl() {
        let (seed, pk) = generate();
        let sk = signing_key_from_seed_b64(&encode_seed(&seed)).unwrap();
        let token = mint(&sk, "acct", "c-abc", 10, 1_000);
        assert!(verify(&token, &[pk], 1_009).is_ok());
        assert!(
            verify(&token, &[pk], 1_010).is_err(),
            "exp 那一秒本身就该过期"
        );
    }

    #[test]
    fn write_issuer_files_writes_a_usable_key_and_pub_file() {
        let dir = tempfile::tempdir().unwrap();
        let (seed, pk) = generate();
        write_issuer_files(dir.path(), &seed, &pk).unwrap();

        let key_raw = std::fs::read_to_string(dir.path().join("issuer.key")).unwrap();
        let sk = signing_key_from_seed_b64(&key_raw).unwrap();
        assert_eq!(sk.verifying_key().to_encoded_point(false).as_bytes(), pk);

        let pub_raw = std::fs::read_to_string(dir.path().join("issuer.pub")).unwrap();
        let parsed = crate::keys::parse_relay_keys(&pub_raw).unwrap();
        assert_eq!(parsed, vec![pk], "issuer.pub 得能直接当 relay-keys 文件用");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("issuer.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn garbage_seed_file_is_refused_not_a_panic() {
        assert!(signing_key_from_seed_b64("not base64!!").is_err());
        assert!(signing_key_from_seed_b64(&STANDARD.encode([0u8; 32])).is_err());
    }
}
