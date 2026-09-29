//! 中转发给一台电脑的令牌：证明「这个 endpoint 属于这个账号，在 `exp` 之前
//! 有效」。中转自己不认识每台电脑的钥匙，只认发令牌的那几个 issuer（平台侧
//! 的 P-256 签名钥匙）——所以 `verify` 拿一串 issuer 公钥去试，只要有一把
//! 验过就算数。
use crate::canon::field;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature as EcSig, SigningKey};
use serde::{Deserialize, Serialize};

pub const TOKEN_VERSION_TAG: &str = "dct-relay-token-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub account: String,
    pub endpoint: String,
    pub exp: u64,
}

/// 令牌里实际装的 JSON：`Claims` 的三个字段，外加对它们的签名。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Envelope {
    account: String,
    endpoint: String,
    exp: u64,
    /// P-256 ECDSA(SHA-256) 对 `bytes(&claims)` 签的名，64 字节 `r||s`，
    /// 标准 base64。ECDSA 签名可延展（见 `keys::MachineKeys::sign` 的文
    /// 档），所以这个字段、乃至整个令牌字符串，都不能当 id 或去重键用。
    sig: String,
}

/// `dct-relay-token-v1` 规范字节：依次写 `field("dct-relay-token-v1")`、
/// `field(account)`、`field(endpoint)`、`field(exp 十进制)`。
pub fn bytes(c: &Claims) -> Vec<u8> {
    let mut out = Vec::new();
    field(&mut out, TOKEN_VERSION_TAG);
    field(&mut out, &c.account);
    field(&mut out, &c.endpoint);
    field(&mut out, &c.exp.to_string());
    out
}

/// 签发一个令牌：`base64url`（不带 padding）编码的 `{account, endpoint,
/// exp, sig}`。
pub fn issue(c: &Claims, issuer: &SigningKey) -> String {
    let sig: EcSig = issuer.sign(&bytes(c));
    let env = Envelope {
        account: c.account.clone(),
        endpoint: c.endpoint.clone(),
        exp: c.exp,
        sig: STANDARD.encode(sig.to_bytes()),
    };
    let json = serde_json::to_vec(&env).expect("Envelope always serializes");
    URL_SAFE_NO_PAD.encode(json)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    /// 不是合法的 base64url，或者解出来不是这个形状的 JSON。
    Malformed,
    /// 试了所有 issuer 公钥，没有一把验得过。
    BadSignature,
    /// 签名验过了，但 `exp` 已经不在未来。
    Expired,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TokenError::Malformed => "token is not a well-formed relay token",
            TokenError::BadSignature => "token signature does not verify under any issuer key",
            TokenError::Expired => "token has expired",
        })
    }
}

impl std::error::Error for TokenError {}

/// 依次试每把 `issuers` 里的公钥，任意一把验过 `sig` 就算通过；再检查
/// `exp > now`。任何解不出来的输入都返回 `Malformed`，不 panic。
pub fn verify(token: &str, issuers: &[[u8; 65]], now: u64) -> Result<Claims, TokenError> {
    let json = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| TokenError::Malformed)?;
    let env: Envelope = serde_json::from_slice(&json).map_err(|_| TokenError::Malformed)?;
    let sig_raw = STANDARD
        .decode(&env.sig)
        .map_err(|_| TokenError::Malformed)?;
    let sig: [u8; 64] = sig_raw.try_into().map_err(|_| TokenError::Malformed)?;

    let claims = Claims {
        account: env.account,
        endpoint: env.endpoint,
        exp: env.exp,
    };
    let msg = bytes(&claims);

    let verified = issuers
        .iter()
        .any(|issuer| crate::keys::verify(issuer, &msg, &sig));
    if !verified {
        return Err(TokenError::BadSignature);
    }

    if claims.exp <= now {
        return Err(TokenError::Expired);
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issuer(byte: u8) -> (SigningKey, [u8; 65]) {
        let sk = SigningKey::from_slice(&[byte.max(1); 32]).unwrap();
        let point = sk.verifying_key().to_encoded_point(false);
        let mut pk = [0u8; 65];
        pk.copy_from_slice(point.as_bytes());
        (sk, pk)
    }

    fn claims() -> Claims {
        Claims {
            account: "gaolei8888@yahoo.com".into(),
            endpoint: "c-abc123".into(),
            exp: 100,
        }
    }

    #[test]
    fn issued_token_verifies_and_returns_claims() {
        let (sk, pk) = issuer(1);
        let c = claims();
        let token = issue(&c, &sk);
        assert_eq!(verify(&token, &[pk], 50), Ok(c));
    }

    #[test]
    fn expired_token_is_refused() {
        let (sk, pk) = issuer(1);
        let c = claims();
        let token = issue(&c, &sk);
        assert_eq!(verify(&token, &[pk], 100), Err(TokenError::Expired));
        assert_eq!(verify(&token, &[pk], 200), Err(TokenError::Expired));
    }

    #[test]
    fn token_from_an_unknown_issuer_is_refused() {
        let (sk, _pk) = issuer(1);
        let (_other_sk, other_pk) = issuer(2);
        let token = issue(&claims(), &sk);
        assert_eq!(
            verify(&token, &[other_pk], 50),
            Err(TokenError::BadSignature)
        );
    }

    #[test]
    fn verify_accepts_any_matching_issuer_in_the_list() {
        let (sk, pk) = issuer(1);
        let (_other_sk, other_pk) = issuer(2);
        let token = issue(&claims(), &sk);
        assert_eq!(verify(&token, &[other_pk, pk], 50), Ok(claims()));
    }

    #[test]
    fn changing_the_endpoint_inside_the_token_breaks_it() {
        let (sk, pk) = issuer(1);
        let token = issue(&claims(), &sk);

        let json = URL_SAFE_NO_PAD.decode(&token).unwrap();
        let mut env: Envelope = serde_json::from_slice(&json).unwrap();
        env.endpoint = "c-someoneelse".into();
        let tampered = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&env).unwrap());

        assert_eq!(verify(&tampered, &[pk], 50), Err(TokenError::BadSignature));
    }

    #[test]
    fn changing_the_account_inside_the_token_breaks_it() {
        let (sk, pk) = issuer(1);
        let token = issue(&claims(), &sk);

        let json = URL_SAFE_NO_PAD.decode(&token).unwrap();
        let mut env: Envelope = serde_json::from_slice(&json).unwrap();
        env.account = "someone-elses-account".into();
        let tampered = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&env).unwrap());

        assert_eq!(verify(&tampered, &[pk], 50), Err(TokenError::BadSignature));
    }

    #[test]
    fn changing_the_exp_inside_the_token_breaks_it() {
        let (sk, pk) = issuer(1);
        let token = issue(&claims(), &sk);

        let json = URL_SAFE_NO_PAD.decode(&token).unwrap();
        let mut env: Envelope = serde_json::from_slice(&json).unwrap();
        env.exp = 999_999;
        let tampered = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&env).unwrap());

        assert_eq!(verify(&tampered, &[pk], 50), Err(TokenError::BadSignature));
    }

    #[test]
    fn an_empty_issuer_list_never_verifies() {
        let (sk, _pk) = issuer(1);
        let token = issue(&claims(), &sk);
        assert_eq!(verify(&token, &[], 50), Err(TokenError::BadSignature));
    }

    #[test]
    fn bytes_matches_a_known_vector() {
        let c = Claims {
            account: "acct".into(),
            endpoint: "c-known0000000000000f".into(),
            exp: 1_700_000_000,
        };
        let expected: &[u8] = b"18:dct-relay-token-v14:acct21:c-known0000000000000f10:1700000000";
        assert_eq!(bytes(&c), expected);
    }

    #[test]
    fn garbage_is_malformed_not_a_panic() {
        assert_eq!(verify("", &[], 0), Err(TokenError::Malformed));
        assert_eq!(
            verify("not base64url!!", &[], 0),
            Err(TokenError::Malformed)
        );
        assert_eq!(
            verify(&URL_SAFE_NO_PAD.encode(b"{}"), &[], 0),
            Err(TokenError::Malformed)
        );
        assert_eq!(
            verify(&URL_SAFE_NO_PAD.encode(b"not json at all"), &[], 0),
            Err(TokenError::Malformed)
        );
    }

    /// **Pins the known-answer vector printed in the design doc's appendix**
    /// (`docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`,
    /// "附录：网关签中转令牌") so the doc cannot silently drift from what this
    /// code actually accepts. If `canon::field`'s encoding, `relay_token`'s
    /// JSON shape, or the base64 alphabet/padding used anywhere in this file
    /// ever changes, this test breaks — that is the point: whoever changes it
    /// must also update the appendix's worked example, because a separate
    /// (e.g. Python) implementation of the gateway has nothing else to check
    /// itself against but that worked example.
    ///
    /// The issuer key below (`sign_seed = [1u8; 32]`) is a fixed, published,
    /// test-only scalar — **never use it for anything real**, its private
    /// key is printed in this repository's history forever.
    #[test]
    fn the_appendix_known_answer_vector_is_accepted() {
        let claims = Claims {
            account: "424242".into(),
            endpoint: "c-0011223344556677889a".into(),
            exp: 1_735_689_600,
        };

        // Canonical bytes, exactly as the appendix shows them (all-ASCII, so
        // the doc can print them as text rather than hex).
        assert_eq!(
            bytes(&claims),
            b"18:dct-relay-token-v16:42424222:c-0011223344556677889a10:1735689600".to_vec()
        );

        // Issuer public key, exactly as the appendix's `--relay-keys` line.
        let issuer_pub_b64 =
            "BG/wO5SSQc4drdQ1GeaWDgqFtBppoFwygQOqK84VlMoWPE91OlW/AdxT9sCwx+7ni0DG/30lqW4igrmJzvccFEo=";
        let issuer_pub_bytes = STANDARD.decode(issuer_pub_b64).expect("valid base64");
        let mut issuer_pub = [0u8; 65];
        issuer_pub.copy_from_slice(&issuer_pub_bytes);

        // Token, exactly as the appendix prints it.
        let token = "eyJhY2NvdW50IjoiNDI0MjQyIiwiZW5kcG9pbnQiOiJjLTAwMTEyMjMzNDQ1NTY2Nzc4ODlhIiwiZXhwIjoxNzM1Njg5NjAwLCJzaWciOiJka2F6YklDdXhPRTBubkp5d1V6MmlTQ2FkOUR2b21ZeitRcCtGTDduNmtVejFvOHZwYldOZWpQUGJuWFJQQ050TWJBNlFNUzZDN3RKUDltU1c3SktTQT09In0";

        // now = 0: any exp in the vector (a 2025 date) is safely in the future.
        assert_eq!(verify(token, &[issuer_pub], 0), Ok(claims));
    }

    /// 网关（dc_llm，Python）用**生产**签名钥匙签的一张探针令牌，在这里用
    /// dct-mesh 的 `verify` 验：两边各自的实现对同一份契约是否真的一致，只有
    /// 拿对面真签出来的东西来验才算数。钥匙是公钥（`--relay-keys` 里那一行），
    /// 不是秘密；私钥只在网关的部署环境里。换了生产钥匙，这条就该跟着换。
    #[test]
    fn a_probe_token_signed_by_the_production_gateway_key_verifies() {
        let prod_pub_b64 =
            "BNEgy8UH1Twciieycr/xRP4lYu/Ks9nRwCbLGLeoCRvcis56hMc42dLDKVgg6/iSdg2RT+az0KqAFwznrDb5Ze0=";
        let raw = STANDARD.decode(prod_pub_b64).expect("valid base64");
        let mut prod_pub = [0u8; 65];
        prod_pub.copy_from_slice(&raw);

        let token = "eyJhY2NvdW50IjoidXNyXzAxUFJPQkUiLCJlbmRwb2ludCI6ImMtMDAxMTIyMzM0NDU1NjY3Nzg4OWEiLCJleHAiOjQxMDI0NDQ4MDAsInNpZyI6IkZsclBJbGpkTlpPN1J3NGlSTFZHYTdkRzMwN2lmdDNkZmV6SW93ZERJM2QwZzZralBlanRXMWhtcTlrWG84VHBjKzRGckJzSE1KYm5KdmZzUG80VmZBPT0ifQ";
        let claims = Claims {
            account: "usr_01PROBE".into(),
            endpoint: "c-0011223344556677889a".into(),
            exp: 4_102_444_800,
        };
        assert_eq!(verify(token, &[prod_pub], 1_790_000_000), Ok(claims));

        // 同一张令牌换一把别的公钥就过不了——上面那条不是「什么都放行」。
        let (_, other) = issuer(7);
        assert!(verify(token, &[other], 1_790_000_000).is_err());
    }
}
