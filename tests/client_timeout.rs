//! 读超时之后，客户端不能再用这条连接——迟到的响应会留在 socket 里，
//! 接着发下一个请求就会读到上一次的响应，从此每次都差一格。
//!
//! 这个假守护进程对第一个请求故意拖过读超时，之后正常应答。

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use dct::client::Client;
use dct::proto::{Request, Response};
use dct::sys::ipc::Listener;

fn start_slow_first_server(sock: std::path::PathBuf) {
    // 只有整个服务端收到的第一个请求会被拖慢，不是每条连接的第一个——
    // 否则客户端重连之后又会撞上一次慢应答，测的就不是"重连能否恢复"了。
    let slow_used = Arc::new(AtomicBool::new(false));
    std::thread::spawn(move || {
        let listener = Listener::bind(&sock).unwrap();
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let slow_used = slow_used.clone();
            std::thread::spawn(move || {
                let mut out = conn.try_clone().unwrap();
                let reader = BufReader::new(conn);
                for line in reader.lines() {
                    if line.is_err() {
                        return;
                    }
                    if !slow_used.swap(true, Ordering::SeqCst) {
                        // 拖过客户端 5 秒的读超时，然后才回一个明显能认出来的响应
                        sleep(Duration::from_secs(7));
                        let stale = serde_json::to_string(&Response::Created { id: 4242 }).unwrap();
                        let _ = writeln!(out, "{stale}");
                        let _ = out.flush();
                    } else {
                        let ok = serde_json::to_string(&Response::Sessions(vec![])).unwrap();
                        let _ = writeln!(out, "{ok}");
                        let _ = out.flush();
                    }
                }
            });
        }
    });
}

#[test]
fn timeout_does_not_desync_the_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("slow.sock");
    start_slow_first_server(sock.clone());
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        sleep(Duration::from_millis(50));
    }

    let mut c = Client::connect(&sock).unwrap();

    // 第一次必然超时
    let started = Instant::now();
    let first = c.call(Request::List);
    assert!(first.is_err(), "第一个请求应当超时报错");
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "应当在读超时（5 秒）就放弃，而不是一直等到服务端回应"
    );

    // 关键：之后的调用绝不能读到那条迟到的 Created{id:4242}
    let second = c.call(Request::List).expect("超时之后应当能重连并正常工作");
    match second {
        Response::Sessions(_) => {}
        other => panic!("超时之后读到了错位的响应：{other:?}"),
    }

    // 再来一次，确认不是侥幸
    match c.call(Request::List).unwrap() {
        Response::Sessions(_) => {}
        other => panic!("协议仍然是错位的：{other:?}"),
    }
}
