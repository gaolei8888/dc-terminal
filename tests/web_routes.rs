//! 手机端第 0 期的路由，接在**真守护进程**上跑一遍。
//!
//! `src/web/routes.rs` 里的单测用的是假 dispatch，它们钉的是路由本身的判断
//! （哪个路径、哪个方法、id 怎么解）。这份不一样，它要回答的是另一个问题：
//! **手机网页收到的 JSON，真的是守护进程说的话吗。**
//!
//! 那个问题只有连真 daemon 才答得了——假 dispatch 里的 `Response` 是测试自己
//! 编的，形状对不对全凭作者记性。中间任何一环（`Request` 的序列化、
//! `handle()` 的分派、`Response` 的字段名）改了，这份测试会挂，而单测不会。

use std::sync::{Arc, Mutex};

use dct::client::Client;
use dct::proto::{Request, Response};
use dct::web::{self, routes::Routes};

mod common;

/// 把 `Request` 通过真 socket 送给真守护进程。
///
/// 生产环境不长这样——那边 web 服务活在守护进程**内部**，直接调
/// `daemon::handle`，不绕一圈 socket（见第 0 期计划的任务 5）。这里绕一圈是
/// 因为测试要的是「跨过编解码和线程边界之后，答复还对不对」，而这正是
/// `tests/common` 那份脚手架存在的理由。
struct OverSocket(Mutex<Client>);

impl web::routes::Dispatch for OverSocket {
    fn call(&self, req: Request) -> Response {
        let mut c = self.0.lock().unwrap();
        match c.call(req) {
            Ok(resp) => resp,
            // 连不上守护进程在这一层是「协议层的坏消息」，照样是一条正常答复——
            // 跟 `daemon::handle` 自己报错走的是同一条路。
            Err(e) => Response::Error(dct::proto::ErrorCode::BadRequest(e.to_string())),
        }
    }
}

/// 起真 daemon + 真 web 服务，返回 (daemon handle, web server, base_url, token)。
fn up() -> (common::DaemonHandle, web::Server, String, String) {
    let h = common::start_daemon();
    let dispatch = Arc::new(OverSocket(Mutex::new(h.client())));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let token = web::new_token().unwrap();
    let server = web::serve(listener, token.clone(), Arc::new(Routes::new(dispatch)));
    let base = format!("http://{}", server.addr());
    (h, server, base, token)
}

fn get(base: &str, path: &str, token: &str) -> (u16, String) {
    let resp = ureq::get(&format!("{base}{path}"))
        .set("Authorization", &format!("Bearer {token}"))
        .call();
    match resp {
        Ok(r) => {
            let status = r.status();
            (status, r.into_string().unwrap())
        }
        Err(ureq::Error::Status(code, r)) => (code, r.into_string().unwrap_or_default()),
        Err(e) => panic!("请求失败：{e}"),
    }
}

#[test]
fn the_phone_sees_the_same_sessions_the_daemon_reports() {
    let (h, server, base, token) = up();
    let workdir = tempfile::tempdir().unwrap();

    let mut c = h.client();
    let id = match c
        .call(Request::Create {
            dir: workdir.path().display().to_string(),
            profile: "shell".into(),
            remember: true,
        })
        .unwrap()
    {
        Response::Created { id } => id,
        other => panic!("预期 Created，实际 {other:?}"),
    };

    let (status, body) = get(&base, "/api/sessions", &token);
    assert_eq!(status, 200, "body: {body}");

    // **按协议的形状解回来**，不是在字符串里找子串：这一条要钉的正是
    // 「网页拿到的就是 `Response`」，用 contains 会让任何一次字段改名溜过去。
    let parsed: Response = serde_json::from_str(&body).unwrap();
    match parsed {
        Response::Sessions(v) => {
            assert_eq!(v.len(), 1, "该有一个会话：{v:?}");
            assert_eq!(v[0].id, id);
            assert_eq!(v[0].profile, "shell");
        }
        other => panic!("预期 Sessions，实际 {other:?}"),
    }

    let _ = c.call(Request::Stop { id });
    server.stop();
}

#[test]
fn a_screen_comes_back_as_the_protocol_screen_response() {
    let (h, server, base, token) = up();
    let workdir = tempfile::tempdir().unwrap();

    let mut c = h.client();
    let id = match c
        .call(Request::Create {
            dir: workdir.path().display().to_string(),
            profile: "shell".into(),
            remember: true,
        })
        .unwrap()
    {
        Response::Created { id } => id,
        other => panic!("预期 Created，实际 {other:?}"),
    };

    let (status, body) = get(&base, &format!("/api/screen?id={id}"), &token);
    assert_eq!(status, 200, "body: {body}");

    let parsed: Response = serde_json::from_str(&body).unwrap();
    assert!(
        matches!(parsed, Response::Screen { .. }),
        "预期 Screen，实际 {parsed:?}"
    );

    let _ = c.call(Request::Stop { id });
    server.stop();
}

/// 问一个不存在的会话：**HTTP 是 200，协议层说「没有这个会话」**。
/// 这条钉的是 `routes.rs` 里那句「HTTP 状态码只描述 HTTP 这一层」——
/// 网页只需要处理一条错误路径。
#[test]
fn asking_for_a_session_that_is_gone_is_a_protocol_error_not_an_http_one() {
    let (_h, server, base, token) = up();

    let (status, body) = get(&base, "/api/screen?id=4242", &token);
    assert_eq!(status, 200, "协议层的坏消息不该变成 HTTP 错误：{body}");

    let parsed: Response = serde_json::from_str(&body).unwrap();
    assert!(
        matches!(parsed, Response::Error(_)),
        "预期一条 Error 答复，实际 {parsed:?}"
    );

    server.stop();
}

/// 令牌不对，一个字节都拿不到——**哪怕守护进程就在那儿好好跑着**。
#[test]
fn a_wrong_token_gets_nothing_even_with_a_healthy_daemon() {
    let (_h, server, base, _token) = up();

    let (status, body) = get(&base, "/api/sessions", "not-the-token");
    assert_eq!(status, 401);
    assert!(body.is_empty(), "401 不该带任何内容：{body}");

    server.stop();
}
