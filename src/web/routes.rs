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
            _ => Resp::status(404),
        }
    }
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
        code.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
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
