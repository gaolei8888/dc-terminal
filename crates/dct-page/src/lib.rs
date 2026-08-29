//! 手机端那一页网页，**整个仓库里只有这一份**。
//!
//! 它有两个服务端：局域网模式下守护进程自己发（`dct` 的 `src/web`），经中转
//! 看家里电脑的时候由中转发（`dct-srv`）。两边发的必须是同一份字节——不然
//! 迟早会变成两份各自演化的网页，而其中一份的 bug 只有在另一种模式下才复现。
//!
//! 所以它既不住在 `dct` 里也不住在 `dct-srv` 里，而是住在一个两边都依赖的
//! 地方。`include_str!` 一次，谁要谁引用这个常量——想拷第二份出来，得先
//! 有人特意去新建一个文件。
//!
//! 页面本身怎么写（一个字的文案都不许写死、不许有外部资源、不许改 PTY 尺寸），
//! 规矩写在 `page.html` 开头那段注释里。
pub const PAGE: &str = include_str!("../page.html");

#[cfg(test)]
mod tests {
    /// 一份网页要是空的，两个服务端都会安静地发一张白纸出去。
    #[test]
    fn the_page_is_actually_here() {
        assert!(super::PAGE.contains("<!doctype html>"), "网页没被打包进来");
        assert!(super::PAGE.len() > 10_000, "网页短得不像话，是不是被截了");
    }
}
