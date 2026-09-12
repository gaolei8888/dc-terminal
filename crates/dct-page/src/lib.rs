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

    /// **学生页里没有任何一条能把字节送回老师终端的路（黑名单这一半）。**
    ///
    /// 这条守卫是这个功能全部安全性的落点：只读不是某处 `if` 判出来的，是这份
    /// 字节里根本没有那些路径。开关式实现（同一页加个只读模式）做不到这一点——
    /// 那一页上「能不能送东西出去」有十几个调用点，漏一个就是学生能往老师终端
    /// 里敲字，而这件事老师在自己机器上永远试不出来。
    ///
    /// **黑名单只拦得住写得出来的那几种坏写法**——把 `"/api/input"` 拆成
    /// `"/ap" + "i/input"`，或者不叫 `wire.key` 而是随便起个变量名，字面量
    /// 子串匹配就什么都测不到了。这一条留着是因为它仍然拦得住最常见的低级
    /// 错误（抄一段桌面端代码没删干净），但真正钉住"将来会不会长出新出口"
    /// 这件事的是下面那条白名单守卫。
    #[test]
    fn the_student_page_has_no_way_to_send_anything_to_a_session() {
        let page = super::live_page();
        for banned in [
            "/api/input", "/api/key", "/api/mouse", "/api/scroll",
            "wire.key", "wire.input", "<input", "<textarea",
            // 这几个都是绕过 `fetch` 白名单的现成出口：`sendBeacon` 能悄悄
            // 发一次 POST 且不等答复，`<form` 能在没有 JS 的情况下提交，
            // `XMLHttpRequest`/`WebSocket`/`EventSource` 都是另一条能发
            // 请求或建连接的路，白名单守卫（下面那条）只数得到 `fetch(`，
            // 数不到这几个。
            "XMLHttpRequest", "sendBeacon", "WebSocket", "EventSource", "<form",
        ] {
            assert!(!page.contains(banned), "学生页里出现了 {banned:?}");
        }
    }

    /// **学生页里没有任何一条能把字节送回老师终端的路（白名单这一半）。**
    ///
    /// 黑名单只拦得住"想得到的坏写法"——把字符串拆开拼接、把方法名换成一个
    /// 不叫 `wire.key` 的变量，子串匹配就完全看不见。这条反过来：**这一页
    /// 唯一被允许的发请求方式是 `fetch(`**，把它出现的每一处都数出来（数量
    /// 写死，多一处就红——新增一个出口必须有人特意回来改这条测试才能通过），
    /// 逐一验证目标都是 `/live/` 开头，再确认页面里没有任何"请求方法不是
    /// GET"的痕迹（`fetch` 不传 `method` 就是 GET，这一页也不该有任何理由
    /// 去传别的方法）。
    ///
    /// 这条测试防的不是"今天这份代码里有没有坏词"，是"将来某个人顺手加了
    /// 一条新出口，会不会有东西替他喊一声"。
    #[test]
    fn every_exit_the_student_page_has_is_a_get_to_a_live_path() {
        let page = super::live_page();

        let exits: Vec<usize> = page.match_indices("fetch(").map(|(i, _)| i).collect();
        assert_eq!(
            exits.len(),
            2,
            "学生页里 fetch( 出现的次数变了（{} 处）——新增或删掉一处发请求的\
             出口，都要回来核对这条守卫是不是还盯得住",
            exits.len()
        );

        for at in &exits {
            // 不是一个真正的表达式解析器，只在 `fetch(` 后面一小段窗口里找
            // 目标：要么是拼在调用现场的字符串字面量（`"/live/" + id + ...`），
            // 要么是像 `frameUrl()` 这样转一手的函数——两种情况都该在窗口里
            // 见到 `/live/` 这个前缀，或者见到那个转手函数的名字（下面单独
            // 钉住 `frameUrl()` 自己的目标）。
            let window = &page[*at..(*at + 80).min(page.len())];
            assert!(
                window.contains("/live/") || window.contains("frameUrl()"),
                "第 {at} 个字节处的 fetch( 附近看不到 /live/ 也看不到 \
                 frameUrl()：{window:?}"
            );
        }

        assert!(
            page.contains("return \"/live/\" + LIVE_ID"),
            "frameUrl() 不见了，或者它的目标不再是 /live/ 开头——上面那条对 \
             fetch(frameUrl()) 的检查就失去意义了"
        );

        // `fetch` 不传第二个参数的 `method` 字段就是 GET。这一页不该有任何
        // 理由发 GET 之外的请求，所以整页不该出现 `method:` 这个 key——
        // 一旦出现，说明某处在悄悄发 POST/PUT/DELETE。
        assert!(
            !page.contains("method:"),
            "学生页里出现了 method:——是不是有请求换成了非 GET？"
        );
    }

    /// **`wait` 被中间代理掐掉之后，学生页不许变成一场自 DDoS。**
    ///
    /// 整个带宽模型建立在「中转真的挂 25 秒」上。`?wait=1` 只是查询串上的
    /// 一句请求，不是合同：学校出口、CDN、企业网关都可能把它吃掉或者自己
    /// 先超时。那时候 304 立刻回来，而「回来就立刻再发一次」就是 200 个
    /// 学生一起对中转不限速地打——spec 里「普通轮询那条路留着，作为 wait
    /// 被中间代理掐掉时的退路」说的就是这条路要真的能走。
    ///
    /// 这条测试钉的是那道地板还在：页面里有一个非零的最小间隔常量，而且
    /// 「什么都没变」的那几条路走的是它而不是 `schedule(0)`。
    #[test]
    fn a_student_page_that_gets_no_new_frame_backs_off_instead_of_hammering() {
        let page = super::live_page();

        assert!(
            page.contains("MIN_IDLE_GAP_MS = 1500"),
            "那道最小间隔的地板不见了——`wait` 一旦被中间代理掐掉，学生页会\
             以零延迟反复重拉"
        );
        assert!(
            page.contains("function scheduleIdle()"),
            "scheduleIdle 不见了，下面那条检查就失去意义了"
        );
        assert!(
            page.contains("scheduleIdle();"),
            "没有任何地方走那道地板"
        );
        // 304 那一支必须走地板。原来的写法是 `schedule(0)`，零延迟。
        let at = page.find("r.status === 304").expect("304 那一支不见了");
        let window = &page[at..(at + 400).min(page.len())];
        assert!(
            window.contains("scheduleIdle()"),
            "304 之后仍然是零延迟重拉：{window:?}"
        );
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
