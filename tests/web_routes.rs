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

/// **手工看一眼**用的脚手架，默认不跑（`#[ignore]`）。
///
/// 网页那一层的渲染逻辑是 JS，而这个仓库的测试只有 cargo——引一个 JS 运行时
/// 进来，等于给所有人的 `cargo test` 加一个 node 依赖，为一件本来就该用眼睛
/// 验收的事。所以这里给一条明路：接上**这台机器上真在跑的**守护进程，
/// 起一个本地端口，把地址印出来，然后挂着，让人（或者带浏览器的 agent）
/// 去看真实数据长什么样。
///
/// ```text
/// cargo test --test web_routes -- --ignored --nocapture serve_for_a_manual_look
/// ```
#[test]
#[ignore]
fn serve_for_a_manual_look() {
    let sock = dirs_home().join(".dct").join("daemon.sock");
    let client = Client::connect(&sock).expect("这台机器上得有个跑着的 dct 守护进程");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let token = web::new_token().unwrap();
    let server = web::serve(
        listener,
        token.clone(),
        Arc::new(Routes::new(Arc::new(OverSocket(Mutex::new(client))))),
    );
    // **给这个脚手架自己开一个会话**，别拿用户正在跑的 agent 当试验田：
    // 往一个活着的 agent 会话里敲字节是有后果的（Esc 关掉他的弹窗、
    // 方向键动他的选择、Ctrl+C 打断他干了一半的活），而验收输入这件事
    // 只需要一个能回显的普通终端。
    let scratch = tempfile::tempdir().unwrap();
    let mut c2 = Client::connect(&sock).unwrap();
    let scratch_id = match c2.call(Request::Create {
        dir: scratch.path().display().to_string(),
        profile: "shell".into(),
        remember: false,
    }) {
        Ok(Response::Created { id }) => Some(id),
        other => {
            println!("MANUAL_NOTE 起不了草稿会话：{other:?}");
            None
        }
    };
    if let Some(id) = scratch_id {
        println!("MANUAL_SCRATCH {id}");
    }

    println!("MANUAL_URL http://{}/#t={}", server.addr(), token);
    // 挂 15 分钟。**三分钟太短**：这是给人用眼睛验收的脚手架，而"打开浏览器、
    // 点进一个会话、翻翻历史、试一下打字"本来就不止三分钟——上一次就是看到
    // 一半服务自己没了。
    std::thread::sleep(std::time::Duration::from_secs(900));

    // 收拾干净：这个会话是脚手架自己开的，不该留在用户的看板上。
    if let Some(id) = scratch_id {
        let _ = c2.call(Request::Kill { id });
        let _ = c2.call(Request::Prune);
    }
    server.stop();
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        .expect("拿不到 home 目录")
}

/// 本机 socket 上问得到手机端的状态，而且**默认是关着的**。
///
/// 开关那一整圈（开、连得上、关、连不上、令牌不变）在 `daemon::web_tests` 里，
/// 不在这儿：那边能绑回环，而这条路径只会绑 `0.0.0.0`——在 Windows 和 macOS 上
/// 那会弹一个防火墙授权框，系统在有人点它之前把调用按住，于是测试白等五秒。
/// **这不是绕过测试**，是把「绑哪个地址」这个决定放回调用方手里（见 `WEB_BIND`）。
#[test]
fn the_lan_client_is_off_until_someone_turns_it_on() {
    let h = common::start_daemon();
    let mut c = h.client();

    match c.call(Request::WebStatus).unwrap() {
        Response::Web(info) => {
            assert!(!info.on, "默认就不该开着");
            assert!(info.url.is_none(), "关着的时候不该有地址");
            assert!(!info.address_unknown);
        }
        other => panic!("预期 Web，实际 {other:?}"),
    }
}

/// **HTTP 上没有开关那个监听口的入口。**
///
/// 手机不该能开关自己的入口，更不该能问出那条带令牌的地址——令牌就是全部的
/// 门禁。真正的拦截在 `daemon::handle` 的 `web` 参数上（HTTP 那一路传 `None`），
/// 这条只是从外面再确认一次：这些路径压根不存在。
#[test]
fn the_http_side_exposes_no_way_to_reach_the_listener() {
    let (_h, server, base, token) = up();

    for path in [
        "/api/webstatus",
        "/api/webenable",
        "/api/webdisable",
        "/api/web",
    ] {
        let (status, body) = get(&base, path, &token);
        assert_eq!(status, 404, "HTTP 上不该有 {path}：{body}");
    }

    server.stop();
}
