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
