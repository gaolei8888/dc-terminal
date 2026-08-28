//! HTTP 请求 → 现有的 `Request` → 现有的 dispatch → JSON。
//!
//! **这一层不许有业务逻辑。** 它是一个翻译器：把 URL 翻成一个 `Request`，把
//! 回来的 `Response` 原样序列化。任何「顺手在这儿算一下」的念头都要挡回去——
//! 手机端看到的东西必须跟桌面看到的是同一个真相，多一处计算就多一处会跟
//! `daemon::handle` 分叉的地方。这也是第 1 期能只换一根管子的前提：
//! **手机网页消费的就是协议本身**。
//!
//! ## HTTP 状态码只描述 HTTP 这一层
//!
//! 路径不存在是 404，方法不对是 405，`?id=` 不是数字是 400，令牌不对是 401
//! （在 `web::serve` 那一层）。而**协议层的回答一律 200**，包括
//! `Response::Error`——那不是「HTTP 请求失败了」，那是守护进程给出的、
//! 一个完全正常的答复：「这个会话不在了」。
//!
//! 混着用的话，网页要写两条错误路径去处理同一件事，而且迟早有一条写得不一样。

use std::sync::Arc;

use crate::proto::{Request, Response};

use super::{Handler, Req, Resp};

/// 手机网页本体。`include_str!` 进二进制：装 dct 的人不该还要另外拷一个
/// 文件到某个目录里，而"找不到网页文件"是一个根本不需要存在的失败模式。
const PAGE: &str = include_str!("page.html");

/// 「把这个 `Request` 交给守护进程，给我 `Response`」。
///
/// 抽成 trait 是为了让 `web` 只依赖**一个函数**，而不是 `daemon::handle` 那
/// 八个参数（`SessionManager`、密钥仓、profiles 目录、手机状态……）。生产环境
/// 由 `daemon.rs` 塞一个把那些 `Arc` 捕获进去的闭包；测试塞一个假的，
/// 不用起守护进程也能测路由。
pub trait Dispatch: Send + Sync + 'static {
    fn call(&self, req: Request) -> Response;
}

impl<F> Dispatch for F
where
    F: Fn(Request) -> Response + Send + Sync + 'static,
{
    fn call(&self, req: Request) -> Response {
        self(req)
    }
}

/// 第 0 期的全部路由。
pub struct Routes {
    dispatch: Arc<dyn Dispatch>,
}

impl Routes {
    pub fn new(dispatch: Arc<dyn Dispatch>) -> Routes {
        Routes { dispatch }
    }

    /// 交给守护进程，把答复序列化成 JSON。
    ///
    /// 序列化失败是 500：那意味着 `Response` 里出现了 serde 表达不了的东西，
    /// 是我们自己的 bug，不是请求方的错——用 4xx 会把责任推给一个什么都没做错
    /// 的客户端。
    fn answer(&self, req: Request) -> Resp {
        let resp = self.dispatch.call(req);
        match serde_json::to_vec(&resp) {
            Ok(body) => Resp::json(body),
            Err(_) => Resp::status(500),
        }
    }
}

impl Handler for Routes {
    fn handle(&self, req: &Req) -> Resp {
        // **先按路径分，再按方法分。** 反过来的话，一个 POST 到不存在的路径会
        // 得到 405（「这个路径不接受 POST」），等于告诉对方这个路径存在。
        match req.path {
            // 网页本体。`/` 之外不给别的入口——一个静态文件服务器会长出
            // 路径穿越那一类问题，而这里总共只有一个文件。
            "/" => match req.method {
                "GET" => Resp::html(PAGE),
                _ => Resp::status(405),
            },
            // 网页上的每一句话都从这儿来，网页里一个字都不写死。
            // 语言认不出来就按 `Lang::resolve` 那套的最后一档（英文）——
            // 手机送来的是浏览器的 `navigator.language`，什么值都可能。
            "/api/strings" => match req.method {
                "GET" => {
                    let lang = query_get(req.query, "lang")
                        .map(|v| {
                            if v.to_ascii_lowercase().starts_with("zh") {
                                "zh"
                            } else {
                                "en"
                            }
                        })
                        .and_then(crate::i18n::Lang::from_code)
                        .unwrap_or(crate::i18n::Lang::En);
                    Resp::json(super::strings::bundle(lang))
                }
                _ => Resp::status(405),
            },
            "/api/sessions" => match req.method {
                "GET" => self.answer(Request::List),
                _ => Resp::status(405),
            },
            "/api/screen" => match req.method {
                // **`id` 解析不出来就地拒绝，绝不往下走。** 缺省成 1 号会话
                // 那种"容错"，会让一个打错的链接安静地显示另一个会话的画面——
                // 而用户没有任何线索知道自己在看谁。
                "GET" => match query_get(req.query, "id").and_then(|v| v.parse::<u32>().ok()) {
                    Some(id) => self.answer(Request::Screen { id }),
                    None => Resp::status(400),
                },
                _ => Resp::status(405),
            },
            // ——往会话里敲东西——————————————————————————————————————
            //
            // 这是手机端第一条**写**的路：前面几条都只是看。守门的还是同
            // 一个令牌（`web::serve` 那一层），这里只负责把请求翻成
            // `Request::Input`——**敲什么、敲给谁，一个字都不加工**。
            "/api/input" => match req.method {
                "POST" => match serde_json::from_slice::<TypeBody>(req.body) {
                    Ok(b) => self.answer(Request::Input {
                        id: b.id,
                        text: b.text,
                    }),
                    Err(_) => Resp::status(400),
                },
                _ => Resp::status(405),
            },
            // 虚拟键行按下去的那一下。名字翻字节走 `web::keys`，
            // 而那个模块自己不写表，转手交给桌面端同一个 `key_to_input`。
            "/api/key" => match req.method {
                "POST" => match serde_json::from_slice::<KeyBody>(req.body) {
                    Ok(b) => match super::keys::bytes_for(&b.key) {
                        Some(text) => self.answer(Request::Input { id: b.id, text }),
                        // 白名单之外的名字是 400，不是"当成没这回事"：
                        // 手机上按了一个键却什么都没发生，用户只会以为
                        // 是网络卡了，然后再按一次。
                        None => Resp::status(400),
                    },
                    Err(_) => Resp::status(400),
                },
                _ => Resp::status(405),
            },
            // 翻历史。**这条不是"往会话里敲 PageUp"**：桌面端的
            // PageUp/PageDown/End 在 dct 自己攥着历史时是被 dct 吃掉的
            // （`attach::key_scroll`），根本不会到 agent 那儿。手机上要
            // 一样，所以走 `Request::Scroll`，不走 `/api/key`。
            "/api/scroll" => match req.method {
                "POST" => match serde_json::from_slice::<ScrollBody>(req.body) {
                    Ok(b) => self.answer(Request::Scroll { id: b.id, by: b.by }),
                    Err(_) => Resp::status(400),
                },
                _ => Resp::status(405),
            },
            _ => Resp::status(404),
        }
    }
}

/// `POST /api/scroll` 的请求体。
///
/// `by` 直接用 `session::ScrollBy`——**协议层不重新定义一份平行的滚动语义**
/// （`Request::Scroll` 自己就是这么做的），网页那边送上来的也就是同一个形状。
#[derive(serde::Deserialize)]
struct ScrollBody {
    id: u32,
    by: crate::session::ScrollBy,
}

/// `POST /api/input` 的请求体。
///
/// **`text` 允许是空串**，而且那不是边界情况——空的那一次正是「回车」，
/// 也正是打检查点的那一次（见 `SessionWriter::type_into` 的两步约定）。
#[derive(serde::Deserialize)]
struct TypeBody {
    id: u32,
    text: String,
}

/// `POST /api/key` 的请求体。
#[derive(serde::Deserialize)]
struct KeyBody {
    id: u32,
    key: String,
}

/// 从 `a=1&id=3` 里取一个值。没有就是 `None`。
///
/// **没有做百分号解码**：这一期用到的值只有会话号（纯数字）。真要解码的那天
/// 再写，现在写等于凭空多一份没人验证过的解析——而它正是那种「看起来对、
/// 在某个边角上错」的代码。
fn query_get<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

#[cfg(test)]
mod tests {

    /// 网页里那套配色**必须两档都在**。
    ///
    /// 这一条挡的是一个很容易发生、而且发生了没人会立刻发现的编辑：
    /// 顺手删掉浅色那套调色板，或者把切换按钮拿掉。测试跑不了浏览器里的
    /// JS，所以这里只钉住「东西还在」——那正是丢了之后最贵的那一部分：
    /// 深色手机上一屏白底、或者浅色手机上一屏看不见的亮黄。
    #[test]
    fn the_page_ships_both_palettes_and_a_way_to_switch() {
        for needle in [
            "PALETTE",
            "dark:",
            "light:",
            "data-theme",
            "prefers-color-scheme",
            "id=\"theme\"",
            "dct-theme",
        ] {
            assert!(
                PAGE.contains(needle),
                "网页里少了 {needle}——配色这套东西缺一块就只剩一半能用"
            );
        }
    }

    /// 浅色那套里 7/15 号**不许是白的**。
    ///
    /// 它们在黑底终端上是默认前景，也就是正文。照字面画成白色的话，
    /// 白底上一整屏正文直接消失——这不是配色难看，是页面没内容。
    #[test]
    fn the_light_palette_never_paints_body_text_white() {
        let light = PAGE
            .split("light: [")
            .nth(1)
            .expect("浅色调色板不见了")
            .split(']')
            .next()
            .unwrap();
        for white in ["#ffffff", "#fff\"", "#e5e5e5"] {
            assert!(
                !light.contains(white),
                "浅色调色板里出现了 {white}：白底上的正文会整屏消失\n{light}"
            );
        }
    }
    use super::*;
    use std::sync::Mutex;

    /// 记下收到过哪些 `Request`，并按预设答复。
    ///
    /// 预设答复存成 JSON 再每次解回来，而不是 `Response::clone()`——`Response`
    /// 没有 `Clone`，而**为了一个假实现去给线协议加 derive 是本末倒置**：那个
    /// 类型的形状被守卫测试盯着，它该为协议服务，不该为测试脚手架服务。
    struct Fake {
        seen: Mutex<Vec<Request>>,
        reply: String,
    }

    fn routes(reply: Response) -> (Routes, Arc<Fake>) {
        let fake = Arc::new(Fake {
            seen: Mutex::new(Vec::new()),
            reply: serde_json::to_string(&reply).unwrap(),
        });
        let f = Arc::clone(&fake);
        let routes = Routes::new(Arc::new(move |req: Request| {
            f.seen.lock().unwrap().push(req);
            serde_json::from_str(&f.reply).unwrap()
        }));
        (routes, fake)
    }

    fn get(path: &str, query: &str) -> Req<'static> {
        // 借用的生命周期在测试里不重要，路径都是字面量。
        Req {
            method: "GET",
            path: Box::leak(path.to_string().into_boxed_str()),
            query: Box::leak(query.to_string().into_boxed_str()),
            body: &[],
        }
    }

    #[test]
    fn the_session_list_goes_through_as_a_plain_list_request() {
        let (r, fake) = routes(Response::Sessions(Vec::new()));
        let resp = r.handle(&get("/api/sessions", ""));

        assert_eq!(resp.status, 200);
        assert!(matches!(
            fake.seen.lock().unwrap().as_slice(),
            [Request::List]
        ));
    }

    /// 答复必须是**协议本身**的 JSON，不是这一层另编的格式。手机端和桌面端
    /// 消费同一份形状，是第 1 期能只换传输的前提。
    #[test]
    fn the_body_is_the_protocol_response_verbatim() {
        let (r, _) = routes(Response::Ok);
        let resp = r.handle(&get("/api/sessions", ""));

        assert_eq!(
            String::from_utf8(resp.body).unwrap(),
            serde_json::to_string(&Response::Ok).unwrap()
        );
    }

    #[test]
    fn a_screen_request_carries_the_session_id() {
        let (r, fake) = routes(Response::Ok);
        let resp = r.handle(&get("/api/screen", "id=7"));

        assert_eq!(resp.status, 200);
        assert!(matches!(
            fake.seen.lock().unwrap().as_slice(),
            [Request::Screen { id: 7 }]
        ));
    }

    #[test]
    fn the_id_is_found_among_other_query_parameters() {
        let (r, fake) = routes(Response::Ok);
        r.handle(&get("/api/screen", "cache=0&id=42&x=1"));

        assert!(matches!(
            fake.seen.lock().unwrap().as_slice(),
            [Request::Screen { id: 42 }]
        ));
    }

    /// **一个坏 id 不许变成一次真的调用。** 缺省成某个会话的话，一条打错的
    /// 链接会安静地显示另一个会话的画面，而屏幕上没有任何线索。
    #[test]
    fn a_missing_or_broken_id_is_refused_without_asking_the_daemon() {
        for query in ["", "id=", "id=abc", "id=-1", "id=99999999999999999999"] {
            let (r, fake) = routes(Response::Ok);
            let resp = r.handle(&get("/api/screen", query));

            assert_eq!(resp.status, 400, "query {query:?} 该被拒");
            assert!(
                fake.seen.lock().unwrap().is_empty(),
                "query {query:?} 不该惊动守护进程"
            );
        }
    }

    /// 协议层的错误是**正常答复**，走 200。它不是「HTTP 请求失败」，
    /// 而是守护进程说「这个会话不在了」。
    #[test]
    fn a_protocol_error_is_still_a_successful_http_answer() {
        let (r, _) = routes(Response::Error(crate::proto::ErrorCode::NoSuchSession(9)));
        let resp = r.handle(&get("/api/screen", "id=9"));

        assert_eq!(resp.status, 200, "协议错误不该变成 HTTP 错误");
        let body: Response = serde_json::from_slice(&resp.body).unwrap();
        assert!(matches!(
            body,
            Response::Error(crate::proto::ErrorCode::NoSuchSession(9))
        ));
    }

    #[test]
    fn the_page_is_served_at_the_root() {
        let (r, fake) = routes(Response::Ok);
        let resp = r.handle(&get("/", ""));

        assert_eq!(resp.status, 200);
        assert!(resp.content_type.starts_with("text/html"));
        assert!(!resp.body.is_empty());
        assert!(
            fake.seen.lock().unwrap().is_empty(),
            "发一个静态页面不该惊动守护进程"
        );
    }

    #[test]
    fn the_strings_bundle_follows_the_requested_language() {
        use crate::i18n::{text, Key, Lang};
        let (r, _) = routes(Response::Ok);
        let zh = r.handle(&get("/api/strings", "lang=zh-CN"));
        let en = r.handle(&get("/api/strings", "lang=en-US"));

        assert_eq!(zh.status, 200);
        assert_ne!(zh.body, en.body, "两种语言的表不该一模一样");
        let zh: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&zh.body).unwrap();
        assert_eq!(zh["idle"], text(Key::StatusIdle, Lang::Zh));
    }

    /// 浏览器送来的 `navigator.language` 什么值都可能（空串、`ja`、`zh_Hant`…）。
    /// **认不出来就退回英文，绝不失败**——一个因为语言标记看不懂而白屏的页面，
    /// 用户完全无从下手。
    #[test]
    fn an_unknown_language_falls_back_instead_of_failing() {
        use crate::i18n::{text, Key, Lang};
        let (r, _) = routes(Response::Ok);
        for q in ["", "lang=", "lang=ja", "lang=xx-YY", "lang=%00"] {
            let resp = r.handle(&get("/api/strings", q));
            assert_eq!(resp.status, 200, "lang {q:?} 不该失败");
            let map: std::collections::BTreeMap<String, String> =
                serde_json::from_slice(&resp.body).unwrap();
            assert_eq!(
                map["idle"],
                text(Key::StatusIdle, Lang::En),
                "lang {q:?} 该退回英文"
            );
        }
    }

    /// 把注释剥掉，只留会被执行/显示的部分。
    ///
    /// 三种注释都要剥：HTML 的 `<!-- -->`、CSS 的 `/* */`、JS 的 `//`。
    /// **注释里写中文是允许的**（它不会显示给用户），所以漏剥哪一种，
    /// 下面那条"网页里不许有汉字"的守卫就会误报——这个坑第一次写就踩了。
    fn page_without_comments() -> String {
        fn strip(src: &str, open: &str, close: &str) -> String {
            let mut out = String::new();
            let mut rest = src;
            while let Some(start) = rest.find(open) {
                out.push_str(&rest[..start]);
                rest = match rest[start..].find(close) {
                    Some(end) => &rest[start + end + close.len()..],
                    None => "",
                };
            }
            out.push_str(rest);
            out
        }
        let code = strip(PAGE, "<!--", "-->");
        let code = strip(&code, "/*", "*/");
        // 行尾注释也要剥，不只是整行注释——`var x = 1;  // 说明` 里的中文
        // 同样不会显示给用户。`://` 不算（`http://…` 这种），那是 URL 的一部分。
        code.lines()
            .map(|l| match l.find("//") {
                Some(0) => "",
                Some(i) if l.as_bytes()[i - 1] != b':' => &l[..i],
                _ => l,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 网页里 `t("…")` 取过的键。
    ///
    /// **前面那个字符必须不是标识符的一部分**：不加这一条的话
    /// `createElement("div")` 也会被算成一次 `t("div")`——`…Element(` 正好以
    /// `t(` 结尾。第一次写就是这么误报的。
    fn strings_the_page_asks_for() -> Vec<String> {
        let code = page_without_comments();
        let bytes = code.as_bytes();
        let mut asked = Vec::new();
        let mut i = 0;
        while let Some(hit) = code[i..].find("t(\"") {
            let at = i + hit;
            let ok = at == 0
                || !matches!(bytes[at - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.');
            i = at + 3;
            if !ok {
                continue;
            }
            if let Some(end) = code[i..].find('"') {
                asked.push(code[i..i + end].to_string());
            }
        }
        asked
    }

    /// **网页里不许写死任何用户可见的文案。** 这一条光靠自觉守不住，所以在这里
    /// 扫一遍：出现汉字，或者出现文案表里任何一句英文的字面量，就挂。
    ///
    /// 写死了会怎样：`l` 键切了界面语言，手机上不跟着变；`i18n.rs` 那两条守卫
    /// （两种语言都组得出话、英文里不许有汉字）查不到它；同一个状态词在电脑上
    /// 和手机上写得不一样，而没有任何东西会报错。
    #[test]
    fn the_page_carries_no_user_facing_copy_of_its_own() {
        let code = page_without_comments();

        let han: Vec<char> = code
            .chars()
            .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
            .collect();
        assert!(han.is_empty(), "网页里出现了汉字，应该走文案表：{han:?}");

        for (name, key) in super::super::strings::NEEDED {
            let en = crate::i18n::text(*key, crate::i18n::Lang::En);
            // 一两个词的短语（"idle"）会跟代码里的标识符撞，只查长句子。
            if en.len() > 12 {
                assert!(!code.contains(en), "网页里写死了 {name} 那句话");
            }
        }
    }

    /// 网页取的每个键都得在表里。少一个的症状是手机上某处空着一块，
    /// 而没有任何报错——**这正是没人会主动发现的那类缺陷**。
    #[test]
    fn the_page_asks_only_for_strings_that_exist() {
        let asked = strings_the_page_asks_for();
        assert!(!asked.is_empty(), "一个文案键都没扫到，扫描逻辑坏了");

        for key in &asked {
            assert!(
                super::super::strings::NEEDED.iter().any(|(n, _)| n == key),
                "网页要 {key:?}，但 strings::NEEDED 里没有"
            );
        }
    }

    /// 网页认识协议里的一串名字：`Sessions`、`Screen`、`lines`、`cursor_hidden`、
    /// 颜色的三种形状……**这些名字在 Rust 这边改了，网页不会报错，只会静静地
    /// 显示错东西**——一屏没有颜色的字，或者一个永远空着的列表。
    ///
    /// 所以在这里把两边对一遍：拿真类型序列化出来，名字必须在网页里出现过。
    /// 这条守卫拦不住"网页用了一个协议里没有的名字"（那种要靠打开浏览器看），
    /// 但它拦得住反过来的那半边：**协议改名，而网页留在原地**。
    #[test]
    fn the_page_knows_the_protocol_by_its_real_names() {
        use crate::pty::{ScreenColor, ScreenSpan, ScreenStyle};
        let code = page_without_comments();

        // 颜色的三种形状。`Idx` 那一支尤其要紧：网页里判它用的是
        // `typeof c.Idx === "number"`，因为 0 号色是黑色，真值判断会把
        // 一整屏黑字当成"没上色"。
        for sample in [
            serde_json::to_string(&ScreenColor::Default).unwrap(),
            serde_json::to_string(&ScreenColor::Idx(3)).unwrap(),
            serde_json::to_string(&ScreenColor::Rgb(1, 2, 3)).unwrap(),
        ] {
            // 形如 "Default" 或 {"Idx":3}，取出里面那个名字。
            let name = sample.trim_matches(|c: char| !c.is_alphanumeric());
            let name = name.split('"').next().unwrap_or(name);
            assert!(
                code.contains(name),
                "协议里的颜色形状 {name:?} 在网页里找不到——改了名字就要改网页"
            );
        }

        // 一段文字的字段，以及样式里网页真的读过的那几个。
        let span = serde_json::to_string(&ScreenSpan {
            text: String::new(),
            style: ScreenStyle::default(),
        })
        .unwrap();
        for field in [
            "text",
            "style",
            "fg",
            "bg",
            "bold",
            "italic",
            "underline",
            "inverse",
        ] {
            assert!(
                span.contains(&format!("\"{field}\"")),
                "协议里没有 {field} 了，而网页还在读它"
            );
            assert!(code.contains(field), "网页没读 {field}");
        }

        // 会话列表那边网页读的字段。
        for field in ["id", "tag", "profile", "dir", "state", "activity"] {
            assert!(
                code.contains(field),
                "网页没读会话的 {field}——列表上会缺一块"
            );
        }

        // 两个答复的外层标签。
        for tag in ["Sessions", "Screen"] {
            assert!(code.contains(tag), "网页不认识 {tag} 这个答复");
        }
        for field in ["lines", "cursor_hidden"] {
            assert!(code.contains(field), "网页没读 Screen 的 {field}");
        }
    }

    /// **屏幕上的内容永远不许当 HTML 用。** 那是 agent 打出来的字节，
    /// 里面什么都可能有；`innerHTML` 一用，一段 `<img onerror=…>` 就在这一页
    /// 里跑起来了，而这一页的同源里躺着能往终端敲字的接口。
    ///
    /// 规矩定成"整个文件里不许出现 innerHTML"，而不是"用的地方要小心"：
    /// 前者一眼看得出真假，后者要人每次都想一遍。
    #[test]
    fn the_page_never_turns_data_into_html() {
        let code = page_without_comments();
        for forbidden in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
        ] {
            assert!(
                !code.contains(forbidden),
                "网页里出现了 {forbidden}——屏幕内容只能走 textContent"
            );
        }
    }

    /// 「该不该发请求」只有一处说得算。
    ///
    /// 抄成好几处的话，漏改一处的症状是**手机锁着屏，家里的 dct 每秒被问
    /// 三次**——在手机上完全看不出来，只体现在电池和流量上。所以这里钉两件事：
    /// 判断只写了一遍，而且真的有人在用它。
    #[test]
    fn only_one_place_decides_whether_to_poll() {
        let code = page_without_comments();
        assert_eq!(
            code.matches("visibilityState").count(),
            1,
            "「在不在前台」这个判断被抄了不止一遍"
        );
        // 一处定义 + 至少两处调用（进画面时的 `tick`、定时器里那一次）。
        // 只写 `>= 2` 的话，**定义本身就占掉一处**，定时器把判断整个删掉照样能过
        // ——这个洞是拿变异测试当场试出来的。
        assert!(
            code.matches("shouldPoll()").count() >= 3,
            "那个唯一的判断没被调够两处——定时器多半绕过它了"
        );
    }

    fn post(path: &str, body: &str) -> Req<'static> {
        Req {
            method: "POST",
            path: Box::leak(path.to_string().into_boxed_str()),
            query: "",
            body: Box::leak(body.to_string().into_bytes().into_boxed_slice()),
        }
    }

    #[test]
    fn typing_goes_through_as_an_input_request() {
        let (r, fake) = routes(Response::Ok);
        let resp = r.handle(&post("/api/input", r#"{"id":3,"text":"你好"}"#));

        assert_eq!(resp.status, 200);
        let seen = fake.seen.lock().unwrap();
        match seen.as_slice() {
            [Request::Input { id: 3, text }] => assert_eq!(text, "你好"),
            other => panic!("预期一条 Input，实际 {other:?}"),
        }
    }

    /// **空 `text` 要照发**。那不是"没内容所以跳过"，那是回车——
    /// 而回车那一次才打检查点（见 `web::keys` 里 `Enter` 那条测试）。
    #[test]
    fn an_empty_text_is_still_sent_because_it_is_the_enter() {
        let (r, fake) = routes(Response::Ok);
        r.handle(&post("/api/input", r#"{"id":3,"text":""}"#));

        let seen = fake.seen.lock().unwrap();
        match seen.as_slice() {
            [Request::Input { id: 3, text }] => assert_eq!(text, ""),
            other => panic!("空回车没发出去：{other:?}"),
        }
    }

    /// 虚拟键行按下去的那一下，翻成的字节必须跟桌面端按同一个键一样。
    #[test]
    fn a_virtual_key_sends_the_same_bytes_the_desktop_would() {
        let (r, fake) = routes(Response::Ok);
        r.handle(&post("/api/key", r#"{"id":7,"key":"Up"}"#));

        let seen = fake.seen.lock().unwrap();
        match seen.as_slice() {
            [Request::Input { id: 7, text }] => assert_eq!(text, "\x1b[A"),
            other => panic!("预期一条 Input，实际 {other:?}"),
        }
    }

    /// 白名单之外的键名是 400，**而且不许惊动守护进程**。手机上按了一个键
    /// 却什么都没发生的话，用户只会以为是网络卡了，然后再按一次。
    #[test]
    fn an_unknown_key_name_is_refused_and_never_reaches_the_daemon() {
        for body in [
            r#"{"id":1,"key":"F2"}"#,
            r#"{"id":1,"key":"ctrl+c"}"#,
            r#"{"id":1,"key":""}"#,
            r#"{"id":1,"key":"\u001b[A"}"#,
        ] {
            let (r, fake) = routes(Response::Ok);
            let resp = r.handle(&post("/api/key", body));
            assert_eq!(resp.status, 400, "{body} 该被拒");
            assert!(fake.seen.lock().unwrap().is_empty(), "{body} 不该往下走");
        }
    }

    /// 请求体读不懂就 400，**同样不许惊动守护进程**。
    #[test]
    fn a_malformed_body_never_reaches_the_daemon() {
        for (path, body) in [
            ("/api/input", "not json"),
            ("/api/input", r#"{"id":"three","text":"x"}"#),
            ("/api/input", r#"{"text":"没有 id"}"#),
            ("/api/key", "{}"),
        ] {
            let (r, fake) = routes(Response::Ok);
            let resp = r.handle(&post(path, body));
            assert_eq!(resp.status, 400, "{path} {body} 该被拒");
            assert!(fake.seen.lock().unwrap().is_empty(), "{body} 不该往下走");
        }
    }

    /// 敲字只认 `POST`。`GET` 能敲字的话，一条链接就能往别人的终端里
    /// 送东西——而链接是会被点的。
    #[test]
    fn typing_is_not_something_a_link_can_do() {
        let (r, fake) = routes(Response::Ok);
        for path in ["/api/input", "/api/key"] {
            let resp = r.handle(&get(path, "id=1&text=x"));
            assert_eq!(resp.status, 405, "{path} 不该认 GET");
        }
        assert!(fake.seen.lock().unwrap().is_empty());
    }

    /// **虚拟键行上的每一个键，服务端都得认。**
    ///
    /// 不对账的话，屏幕上会出现一个按下去只会 400 的键——而手机上按了没反应
    /// 的表现跟"网络卡了"一模一样，用户只会再按一次。这条跟文案那两条守卫
    /// 是同一个形状：**JS 那边对 Rust 的每一处依赖，都要能从 Rust 这边查。**
    #[test]
    fn every_key_on_the_virtual_row_is_one_the_daemon_accepts() {
        let code = page_without_comments();
        let row = code
            .split_once("var ROW = [")
            .expect("网页里没有那一排虚拟键了？")
            .1
            .split_once(']')
            .expect("ROW 那一行没闭合")
            .0;

        let names: Vec<String> = row
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert!(names.len() >= 5, "虚拟键行怎么只剩 {names:?}");

        for name in &names {
            assert!(
                super::super::keys::bytes_for(name).is_some(),
                "虚拟键行上的 {name:?} 服务端不认——按下去只会 400"
            );
        }
    }

    /// **虚拟键行上不许出现桌面端自己会吃掉的键。**
    ///
    /// `PageUp`/`PageDown` 在桌面上有历史可翻时是 dct 用来翻滚屏的，根本不到
    /// agent 那儿；`End` 在滚上去之后是「回到底部」。把它们放进这一排，
    /// 同一个键就成了「桌面翻历史、手机敲给 agent」两回事——而这条链路的
    /// 全部前提是手机做到的跟桌面一样。手机翻历史有自己的路（`/api/scroll`），
    /// 那条路走的正是桌面被吃掉之后走的同一个 `Request::Scroll`。
    #[test]
    fn the_virtual_row_never_offers_a_key_the_desktop_would_swallow() {
        let code = page_without_comments();
        let row = code
            .split_once("var ROW = [")
            .expect("网页里没有那一排虚拟键了？")
            .1
            .split_once(']')
            .expect("ROW 那一行没闭合")
            .0;

        // **名单本身也要钉住。** 只遍历那个常量的话，把它清空就让这条守卫
        // 变成空转、测试照样绿——跟 `qr` 那条静区守卫犯过的是同一个错，
        // 也是变异测试当场抓到的。桌面端真改了拦截规则，这里就该一起改。
        let list = super::super::keys::INTERCEPTED_ON_DESKTOP;
        for name in ["PageUp", "PageDown", "End"] {
            assert!(
                list.contains(&name),
                "{name} 从「桌面会吃掉的键」名单里没了——桌面端真改规则了？"
            );
        }

        for name in list {
            assert!(
                !row.contains(name),
                "虚拟键行上有 {name:?}——桌面端会把它吃掉去翻历史，两边就不一样了"
            );
        }
    }

    /// **实体键盘上的那几个键也不许当按键发下去。**
    ///
    /// 捕获模式收的是真键盘，`PageUp`/`PageDown`/`End` 一按一个准。桌面端
    /// 对它们的处理是"dct 自己吃掉去翻历史"，所以这一页也得走
    /// `/api/scroll`——跟 ⇞⇟⤓ 三个按钮同一条路。翻成按键发下去的话，
    /// 同一块键盘在两个客户端上就是两种行为。
    #[test]
    fn a_physical_keyboard_scrolls_history_instead_of_sending_those_keys() {
        let code = page_without_comments();

        // 键名映射表里不许出现它们……
        let table = code
            .split_once("var KEYNAMES = {")
            .expect("键名映射表没了？")
            .1
            .split_once('}')
            .expect("KEYNAMES 没闭合")
            .0;
        for name in ["PageUp", "PageDown", "End"] {
            assert!(
                !table.contains(name),
                "{name} 跑进键名映射表了——它该走滚动，不该当按键发给 agent"
            );
        }

        // ……而且它们确实各自有一条滚动的去处。
        // 收尾找的是行首那个 `};`，不是第一个——每一条分支自己就是一个
        // `function () { … };`，按第一个切的话只能拿到第一行。
        let scrolls = code
            .split_once("var SCROLLS_INSTEAD = {")
            .expect("那三个键的滚动分支没了？")
            .1
            .split_once(
                "
  };",
            )
            .expect("SCROLLS_INSTEAD 没闭合")
            .0;
        for name in ["PageUp", "PageDown", "End"] {
            assert!(
                scrolls.contains(name),
                "{name} 没有滚动的去处，按下去会没反应"
            );
        }
    }

    /// 粘滞 `Ctrl` 拼出来的名字也得认。网页那边拼的是 `"Ctrl+" + 大写字母`，
    /// 这条把那个拼法本身钉住。
    #[test]
    fn the_sticky_ctrl_builds_a_name_the_daemon_accepts() {
        let code = page_without_comments();
        assert!(
            code.contains(r#""Ctrl+" + e.key.toUpperCase()"#),
            "粘滞 Ctrl 的拼法变了，这条守卫要跟着改"
        );
        for c in ['A', 'C', 'D', 'U', 'Z'] {
            let name = format!("Ctrl+{c}");
            assert!(
                super::super::keys::bytes_for(&name).is_some(),
                "{name} 服务端不认"
            );
        }
    }

    /// 翻历史走 `Request::Scroll`，**不是往会话里敲 PageUp**。
    ///
    /// 桌面端的 PageUp/PageDown 在 dct 自己攥着历史时是被 dct 吃掉的，
    /// 根本不会到 agent 那儿。手机上要是翻成按键发下去，同一个手势在两个
    /// 客户端上就是两件事——而这条链路的全部前提是"手机看到的跟桌面一样"。
    #[test]
    fn scrolling_history_is_a_scroll_request_not_a_keypress() {
        use crate::session::ScrollBy;
        let (r, fake) = routes(Response::Ok);
        r.handle(&post("/api/scroll", r#"{"id":5,"by":{"Rows":-20}}"#));
        r.handle(&post("/api/scroll", r#"{"id":5,"by":"Bottom"}"#));

        let seen = fake.seen.lock().unwrap();
        match seen.as_slice() {
            [Request::Scroll {
                id: 5,
                by: ScrollBy::Rows(-20),
            }, Request::Scroll {
                id: 5,
                by: ScrollBy::Bottom,
            }] => {}
            other => panic!("预期两条 Scroll，实际 {other:?}"),
        }
    }

    #[test]
    fn a_malformed_scroll_never_reaches_the_daemon() {
        for body in [
            r#"{"id":1}"#,
            r#"{"id":1,"by":"Sideways"}"#,
            r#"{"id":1,"by":{"Rows":"lots"}}"#,
            "[]",
        ] {
            let (r, fake) = routes(Response::Ok);
            assert_eq!(r.handle(&post("/api/scroll", body)).status, 400, "{body}");
            assert!(fake.seen.lock().unwrap().is_empty(), "{body} 不该往下走");
        }
    }

    /// 「现在能不能往会话里送东西」只有一处说得算，而且真的被用着。
    ///
    /// 两件事都在这一处判：有没有打开的会话、**输入法在不在组合中**。
    /// 抄成好几处的话，漏改的那一处就是"某个按钮在打拼音的时候把半截字
    /// 送了出去"——而这件事在英文键盘上永远试不出来。
    ///
    /// 数字跟 `only_one_place_decides_whether_to_poll` 同一个道理：一处定义
    /// 加至少三处调用（打字、按键、翻历史）。
    #[test]
    fn only_one_place_decides_whether_input_may_be_sent() {
        let code = page_without_comments();
        // **盯的是「读」，不是出现次数。** 写它的地方会随着输入口增加
        // （单行框一对、捕获键盘一对 composition 监听），那是正常的；
        // 不正常的是**有第二处地方去判断它**——那才是将来会漂的东西。
        //
        // 读 = 总出现次数减去「赋值」的次数（声明也算一次赋值）。只查
        // `!composing` 的话，写成 `if (composing)` 的第二处判断就溜过去了
        // ——变异测试当场试出来的。
        let total = code.matches("composing").count();
        let writes = code.matches("composing = ").count();
        assert_eq!(
            total - writes,
            1,
            "「输入法在不在组合中」被判断了 {} 处，而它只该在 canSend 里判一次",
            total - writes
        );

        // 两个输入口各自都要报告组合状态。少一处的症状：那个口子上打中文，
        // 拼音会被逐字送进 agent。
        for surface in ["lineEl", "captureEl"] {
            for ev in ["compositionstart", "compositionend"] {
                assert!(
                    code.contains(&format!("{surface}.addEventListener(\"{ev}\"")),
                    "{surface} 没接 {ev}"
                );
            }
        }
        // 一处定义 + 四处调用：送按键、粘滞 Ctrl 那一下、提交打的字、翻历史。
        // **定义本身占一处**——写 `>= 4` 的话，把其中一处的判断删掉照样能过
        // （变异测试当场抓到的，跟 `shouldPoll` 那条犯的是同一个错）。
        assert!(
            code.matches("canSend()").count() >= 5,
            "那条唯一的判断没被调够四处——送键、Ctrl、打字、翻历史都得过它"
        );
    }

    /// **能不能送，要在动那个输入框之前问。**
    ///
    /// 顺序反过来的话，组合期那一下会先把框清空再发现"现在不能送"——
    /// 用户正在打的拼音当场消失，而他什么都没做错。这种事没有任何测试
    /// 跑得到（那是浏览器里的输入法），所以在源码顺序上钉住。
    #[test]
    fn the_capture_field_is_never_cleared_before_we_know_we_can_send() {
        let code = page_without_comments();
        let body = code
            .split_once("function flushCapture() {")
            .expect("flushCapture 没了？")
            .1;
        let gate = body
            .find("canSend()")
            .expect("flushCapture 里没问过能不能送");
        let clear = body
            .find("captureEl.value = \"\"")
            .expect("flushCapture 里没清空输入框？");
        assert!(
            gate < clear,
            "先清空了输入框才判断能不能送——组合期这一下会把用户的拼音吞掉"
        );
    }

    /// **底栏不许跟着内容滚。**
    ///
    /// 输入框和那排虚拟键是这一页最要紧的东西，画面一长就被顶出视口的话，
    /// 用户要先滚回去才能打字——而他多半以为是页面坏了。做法是整页一个
    /// flex 外壳：头和底栏定高，中间那块自己滚。原来那版用的是
    /// `position: sticky`，在 body 自己滚的布局里它救不了底栏。
    ///
    /// 这条守卫查的是"外壳还在不在"。改布局的人会撞到它——那正是目的：
    /// 撞到之后回来读这段话，而不是发现手机上底栏又跑了。
    #[test]
    fn the_footer_never_scrolls_away_with_the_content() {
        let code = page_without_comments();
        assert!(
            code.contains("100dvh"),
            "外壳没有按视口高度撑开——手机地址栏一伸缩，底栏就掉到屏幕外面"
        );
        assert!(
            code.contains("#typing { flex: 0 0 auto"),
            "底栏不再是 flex 里定高的那一项了"
        );
        assert!(
            !code.contains("#typing { position: sticky"),
            "底栏又回到 sticky 了——在 body 自己滚的布局里它吊不住"
        );
    }

    /// **画面按宽和高一起适配。**
    ///
    /// 只按宽度算的话，竖屏上一行放得下、24 行却装不下——而少看的那几行
    /// 正是 agent 此刻在写的那几行。画面是一整块固定尺寸的画布（手机不改
    /// PTY 尺寸），所以两个方向各算一个上限、取小的那个。
    #[test]
    fn the_screen_is_fitted_by_height_as_well_as_width() {
        let code = page_without_comments();
        assert!(
            code.contains("Math.min(byWidth, byHeight)"),
            "又变回只按宽度适配了"
        );
    }

    /// 用户调过的字号要活过一次关页面。每次打开都要重调一遍的话，
    /// 这个功能等于没有。
    #[test]
    fn the_chosen_text_size_is_remembered() {
        let code = page_without_comments();
        assert!(
            code.contains("localStorage.setItem(\"dct-zoom\""),
            "字号不落盘了"
        );
        assert!(
            code.contains("catch"),
            "localStorage 没包 try——隐私模式下它会抛异常，而那不该让整页白屏"
        );
    }

    /// **图标按钮也要有名字。** 读屏软件念不出「A−」和「‹」，而这一页是
    /// 给手机用的，手机上读屏用户很多。名字跟别的文案一样从文案表来。
    #[test]
    fn the_icon_buttons_have_names_for_a_screen_reader() {
        let code = page_without_comments();
        for (id, key) in [("zoomout", "smaller"), ("zoomin", "bigger")] {
            assert!(
                code.contains(&format!(
                    "document.getElementById(\"{id}\").setAttribute(\"aria-label\", t(\"{key}\"))"
                )),
                "{id} 没有无障碍名字"
            );
        }
        assert!(
            code.contains("backEl.setAttribute(\"aria-label\", t(\"back\"))"),
            "返回键没有无障碍名字"
        );
    }

    #[test]
    fn an_unknown_path_is_a_404_whatever_the_method() {
        let (r, fake) = routes(Response::Ok);
        for method in ["GET", "POST", "DELETE"] {
            let mut req = get("/nope", "");
            req.method = method;
            assert_eq!(r.handle(&req).status, 404, "{method} /nope");
        }
        assert!(fake.seen.lock().unwrap().is_empty());
    }

    /// 已知路径上用错方法是 405；**不存在的路径永远是 404**——405 会告诉对方
    /// 「这个路径存在，只是不收这个方法」。
    #[test]
    fn a_wrong_method_on_a_real_path_is_405_but_never_leaks_a_fake_one() {
        let (r, _) = routes(Response::Ok);
        let mut real = get("/api/sessions", "");
        real.method = "POST";
        assert_eq!(r.handle(&real).status, 405);

        let mut fake_path = get("/api/secret-admin", "");
        fake_path.method = "POST";
        assert_eq!(r.handle(&fake_path).status, 404);
    }
}
