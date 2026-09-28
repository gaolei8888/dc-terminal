//! 跟 `dct-brain` 的 `canon::field` 编码完全一样：每个字段写成「UTF-8 字节数的
//! 十进制 ASCII + `:` + 原始字节」。这个 crate 不依赖 dct-brain（两者互相独立
//! 的纯逻辑 crate），所以这段短短几行的编码在这里单独抄一份，而不是共享一个
//! `dct-common`——为几行代码去拉一个新的共享 crate 不值得。带长度前缀，所以
//! 字段里有什么字符都不会跟相邻字段粘在一起被误读。
pub fn field(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(s.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_length_prefixed_so_adjacent_values_cannot_smear_together() {
        let mut a = Vec::new();
        field(&mut a, "ab");
        field(&mut a, "c");
        let mut b = Vec::new();
        field(&mut b, "a");
        field(&mut b, "bc");
        assert_ne!(a, b);
        assert_eq!(a, b"2:ab1:c");
        assert_eq!(b, b"1:a2:bc");
    }
}
