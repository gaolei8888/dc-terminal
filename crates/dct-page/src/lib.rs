//! 两页网页，**整个仓库里只有这两份**。
//!
//! `page()` 是手机端那一页：局域网模式下守护进程自己发（`dct` 的 `src/web`），
//! 经中转看家里电脑的时候由中转发（`dct-srv`）。两边发的必须是同一份字节
//! ——不然迟早会变成两份各自演化的网页，而其中一份的 bug 只有在另一种模式
//! 下才复现。
//!
//! `live_page()` 是直播观众那一页：学生打开老师分享的链接看到的东西，
//! 只由中转发——它没有局域网那一档。
//!
//! 所以它们既不住在 `dct` 里也不住在 `dct-srv` 里，而是住在一个两边都依赖
//! 的地方。`include_str!` 一次，谁要谁引用这个函数——想拷第二份出来，得先
//! 有人特意去新建一个文件。
//!
//! 两页画的是同一块终端画布（配色、字号适配、把一屏 `Response::Screen`
//! 画成 `<pre>` 里的 `<span>`），那份渲染代码抽在 `shared.js` 里，两页各自
//! 用 `<!--SHARED-->` 这个占位符把它接进自己的 `<script>`——不是真正的模块
//! （这个 crate 没有依赖，也搭不出 import/export 那一套），是同一段文本被
//! 原样接进两个作用域。理由和对调用方的约定写在 `shared.js` 开头。
//!
//! 页面本身怎么写（一个字的文案都不许写死、不许有外部资源、不许改 PTY 尺寸），
//! 规矩写在 `page.html` 开头那段注释里；`live.html` 有它自己的一段，因为
//! 它是给学生看的只读页，规矩不完全一样——最要紧的一条是**它没有任何一条
//! 能把字节送回老师终端的路**，下面那条测试钉着这件事。

use std::sync::OnceLock;

const SHARED: &str = include_str!("../shared.js");
const PAGE_SRC: &str = include_str!("../page.html");
const LIVE_SRC: &str = include_str!("../live.html");

/// 占位符只此一个写法，两页都用它。
const MARK: &str = "<!--SHARED-->";

/// 手机端网页，两个服务端共用。
pub fn page() -> &'static str {
    static IT: OnceLock<String> = OnceLock::new();
    IT.get_or_init(|| PAGE_SRC.replace(MARK, SHARED))
}

/// 直播观众页，只读，只由中转发。
pub fn live_page() -> &'static str {
    static IT: OnceLock<String> = OnceLock::new();
    IT.get_or_init(|| LIVE_SRC.replace(MARK, SHARED))
}

#[cfg(test)]
mod tests {
    /// 一份网页要是空的，两个服务端都会安静地发一张白纸出去。
    #[test]
    fn the_page_is_actually_here() {
        assert!(super::page().contains("<!doctype html>"), "网页没被打包进来");
        assert!(super::page().len() > 10_000, "网页短得不像话，是不是被截了");
    }

    /// **学生页里没有任何一条能把字节送回老师终端的路。**
    ///
    /// 这条守卫是这个功能全部安全性的落点：只读不是某处 `if` 判出来的，是这份
    /// 字节里根本没有那些路径。开关式实现（同一页加个只读模式）做不到这一点——
    /// 那一页上「能不能送东西出去」有十几个调用点，漏一个就是学生能往老师终端
    /// 里敲字，而这件事老师在自己机器上永远试不出来。
    #[test]
    fn the_student_page_has_no_way_to_send_anything_to_a_session() {
        let page = super::live_page();
        for banned in [
            "/api/input", "/api/key", "/api/mouse", "/api/scroll",
            "wire.key", "wire.input", "<input", "<textarea",
        ] {
            assert!(!page.contains(banned), "学生页里出现了 {banned:?}");
        }
    }

    /// 两页共用同一份渲染。各留一份的话，迟早只有一页修对了某个渲染 bug。
    #[test]
    fn both_pages_share_one_painter() {
        assert!(super::page().contains("function paint("));
        assert!(super::live_page().contains("function paint("));
        let both = format!("{}{}", super::page(), super::live_page());
        assert_eq!(
            both.matches("ui-monospace").count(),
            2,
            "等宽字体名在两页里合计不是两次——共享那一份被拷开了"
        );
    }

    #[test]
    fn neither_page_is_empty() {
        assert!(super::page().len() > 10_000);
        assert!(super::live_page().len() > 4_000);
    }
}
