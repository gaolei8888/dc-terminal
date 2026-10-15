//! 给 agent 的对话起一个 id：dct 开会话时自己定，重启后按它接回。
//! 格式是 UUID v4——claude 的 `--session-id` 只收合法 UUID。

/// 新起一个对话 id：UUID v4，小写、带连字符、36 个字符。
pub fn new() -> String {
    let mut b = [0u8; 16];
    // 拿不到系统随机数是极端情况；退回时间戳拼一个，宁可接不回也不让开会话失败。
    if getrandom::getrandom(&mut b).is_err() {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        b.copy_from_slice(&n.to_le_bytes());
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_uuid_v4_and_differs() {
        let a = new();
        let b = new();
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
        assert!(a
            .chars()
            .all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(
            a.char_indices()
                .filter(|&(_, c)| c == '-')
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
            vec![8, 13, 18, 23]
        );
        assert_ne!(a, b);
    }
}
