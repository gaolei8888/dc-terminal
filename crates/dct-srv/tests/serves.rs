//! 真的绑一个端口、真的走一遍 HTTP。
//!
//! `lib.rs` 里那些 `oneshot` 测试直接调 `Router`，**绕开了整个服务器**：
//! 监听、accept、解析请求行、写响应，一样都没经过。任务 3 的守护进程会拿
//! `ureq` 从外面连过来，那时候要是 `serve()` 这条路本身有问题，症状会出现在
//! 一个完全不同的 crate 里。这里先把它证掉。
//!
//! 客户端是手写的：为了一个测试往依赖树里塞一个 HTTP 客户端不值得，而这条
//! 请求简单到十几行就能写完。

use std::sync::Arc;
use std::time::Duration;

use dct_link::{
    AuthFrame, EndpointId, EndpointKind, Envelope, SendRequest, LINK_VERSION, PATH_POLL, PATH_SEND,
};
use dct_srv::{Config, Relay};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// 发一条 POST，把状态码和 body 读回来。
async fn post(addr: std::net::SocketAddr, path: &str, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();

    // `Connection: close` 之后服务器写完就关，读到 EOF 就是读全了——不用自己
    // 去解 Content-Length。
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();

    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or_else(|| panic!("这不像一条 HTTP 响应：{text:?}"));
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

fn id(s: &str) -> EndpointId {
    EndpointId::new(s).unwrap()
}

fn auth(who: &str) -> AuthFrame {
    AuthFrame {
        version: LINK_VERSION,
        kind: EndpointKind::Computer,
        endpoint: id(who),
        token: "t".into(),
    }
}

#[tokio::test]
async fn a_letter_crosses_a_real_socket() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let relay = Arc::new(Relay::new(Config {
        poll_timeout: Duration::from_millis(300),
        inbox: 4,
    }));
    tokio::spawn(dct_srv::serve(listener, relay));

    // b 露个面，好让它算在线。这一轮什么都等不到。
    let (status, body) = post(addr, PATH_POLL, &serde_json::to_string(&auth("b")).unwrap()).await;
    assert_eq!(status, 200);
    assert_eq!(body, r#"{"envelope":null}"#);

    let letter = SendRequest {
        auth: auth("a"),
        envelope: Envelope {
            from: id("a"),
            to: id("b"),
            seq: 9,
            // 不是 UTF-8，也不是 JSON：中转不该在乎。
            payload: vec![0, 0xff, b'{'],
            recipients: vec![],
        },
    };
    let (status, _) = post(addr, PATH_SEND, &serde_json::to_string(&letter).unwrap()).await;
    assert_eq!(status, 204);

    let (status, body) = post(addr, PATH_POLL, &serde_json::to_string(&auth("b")).unwrap()).await;
    assert_eq!(status, 200);
    let got: dct_link::PollResponse = serde_json::from_str(&body).unwrap();
    let got = got.envelope.expect("信封应该在这一轮回来");
    assert_eq!(got.from, id("a"));
    assert_eq!(got.seq, 9);
    assert_eq!(got.payload, vec![0, 0xff, b'{']);
}

/// 没人接的收件人，坏消息也得原样穿过 HTTP 那一层回来。
#[tokio::test]
async fn an_offline_peer_comes_back_as_a_code_over_http() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(dct_srv::serve(
        listener,
        Arc::new(Relay::new(Config::default())),
    ));

    let letter = SendRequest {
        auth: auth("a"),
        envelope: Envelope {
            from: id("a"),
            to: id("nobody"),
            seq: 1,
            payload: b"x".to_vec(),
            recipients: vec![],
        },
    };
    let (status, body) = post(addr, PATH_SEND, &serde_json::to_string(&letter).unwrap()).await;
    assert_eq!(status, 409);
    assert_eq!(body, r#"{"error":"Offline"}"#);
}

/// 第一期的硬性条件。这条测试存在的意义是：把它改红需要动一句明确的判断，
/// 而不是改一份没人跑的文档。
#[test]
fn the_relay_refuses_to_listen_anywhere_but_loopback() {
    for ok in ["127.0.0.1:8787", "[::1]:8787"] {
        assert!(
            dct_srv::must_be_loopback(ok.parse().unwrap()).is_ok(),
            "{ok}"
        );
    }
    for bad in ["0.0.0.0:8787", "192.168.1.19:8787", "[::]:8787"] {
        let why = dct_srv::must_be_loopback(bad.parse().unwrap())
            .expect_err(&format!("{bad} 不该被允许"));
        // 报错要说清楚下一步，不能只说"不行"。
        assert!(why.contains("端到端加密"), "{why}");
    }
}
