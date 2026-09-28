//! 三份规范（流程步骤、执行票、批准记录）共用的编码：每个字段写成
//! 「UTF-8 字节数的十进制 ASCII + `:` + 原始字节」。这是 dcv 定的 dcv-steps-v1 的编码，
//! 执行票和批准记录沿用同一种，只是开头的版本串不同。带长度前缀，所以字段里有什么
//! 字符都不会跟相邻字段粘在一起被误读。
use sha2::{Digest, Sha256};

pub fn field(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(s.as_bytes());
}

/// `sha256:<64 位小写十六进制>`，跟 dcv 的 `body_sha256`、`approved_body` 同一种写法。
pub fn sha256_id(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
