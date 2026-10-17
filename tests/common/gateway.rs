//! 本机回环上的假 OpenAI 兼容 SSE 网关：按脚本吐 chunked 的 SSE、记下每个连接的请求体、
//! 鉴权头、被对面关掉的时刻。`ask_stream` / `ask_service` / `ask_cli` 共用。
use serde_json::json;
use std::io::Read;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub enum Step {
    Headers,
    /// 一个 SSE 事件（一个 chunked 块）。
    Sse(String),
    Sleep(u64),
    /// 空的 chunked 终止块。
    EndChunks,
    /// 整个响应一次写完（非 200 用）。
    Status(u16),
}

#[derive(Default)]
pub struct Conn {
    pub body: String,
    pub authed: bool,
    /// 假网关第一次看到对面关了连接的时刻。
    pub closed_at: Option<Instant>,
    /// 每个 `Sse` 块写出去的时刻。
    pub sent_at: Vec<Instant>,
    /// 写失败了（对面已关）之后脚本就停了。
    pub write_failed: bool,
}

pub struct Gw {
    pub url: String,
    pub conns: Arc<Mutex<Vec<Arc<Mutex<Conn>>>>>,
}

impl Gw {
    pub fn conn(&self, i: usize) -> Arc<Mutex<Conn>> {
        // 先等连接出现，免得断言跑在 accept 前面。
        let t = Instant::now();
        loop {
            if let Some(c) = self.conns.lock().unwrap().get(i).cloned() {
                return c;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "第 {i} 个连接一直没来");
            sleep(Duration::from_millis(10));
        }
    }

    pub fn wait_closed(&self, i: usize, within: Duration) -> Instant {
        let t = Instant::now();
        loop {
            if let Some(at) = self.conn(i).lock().unwrap().closed_at {
                return at;
            }
            assert!(t.elapsed() < within, "假网关 {within:?} 内没看到第 {i} 个连接被关");
            sleep(Duration::from_millis(5));
        }
    }
}

pub fn spawn_gw(scripts: Vec<Vec<Step>>) -> Gw {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/chat/completions", l.local_addr().unwrap());
    let conns: Arc<Mutex<Vec<Arc<Mutex<Conn>>>>> = Arc::default();
    let seen = conns.clone();
    std::thread::spawn(move || {
        for script in scripts {
            let Ok((mut s, _)) = l.accept() else { return };
            let conn: Arc<Mutex<Conn>> = Arc::default();
            seen.lock().unwrap().push(conn.clone());
            std::thread::spawn(move || serve(&mut s, script, &conn));
        }
    });
    Gw { url, conns }
}

fn serve(s: &mut TcpStream, script: Vec<Step>, conn: &Arc<Mutex<Conn>>) {
    // 读请求：头 + Content-Length 的正文。
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head_end, len) = loop {
        let n = s.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).to_string();
        if let Some(i) = text.find("\r\n\r\n") {
            let len = text[..i]
                .lines()
                .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                .unwrap_or(0);
            if buf.len() >= i + 4 + len {
                let head = text[..i].to_lowercase();
                conn.lock().unwrap().authed = head.contains("authorization: bearer test-key");
                break (i + 4, len);
            }
        }
    };
    conn.lock().unwrap().body = String::from_utf8_lossy(&buf[head_end..head_end + len]).into();

    // 看着对面：读到 EOF / 错误 = 对面关了。
    let mut watch = s.try_clone().unwrap();
    let c2 = conn.clone();
    std::thread::spawn(move || {
        let mut b = [0u8; 64];
        while watch.read(&mut b).map(|n| n > 0).unwrap_or(false) {}
        c2.lock().unwrap().closed_at.get_or_insert_with(Instant::now);
    });

    for step in script {
        let r = match step {
            Step::Headers => s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Step::Sse(t) => {
                let r = s.write_all(format!("{:x}\r\n{t}\r\n", t.len()).as_bytes());
                conn.lock().unwrap().sent_at.push(Instant::now());
                r
            }
            Step::Sleep(ms) => {
                sleep(Duration::from_millis(ms));
                Ok(())
            }
            Step::EndChunks => s.write_all(b"0\r\n\r\n"),
            Step::Status(code) => s.write_all(
                format!("HTTP/1.1 {code} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").as_bytes(),
            ),
        };
        if r.and_then(|_| s.flush()).is_err() {
            conn.lock().unwrap().write_failed = true;
            return;
        }
    }
}

pub fn delta(text: &str) -> Step {
    let v = json!({"choices":[{"delta":{"content": text}}]});
    Step::Sse(format!("data: {v}\n\n"))
}

pub fn done() -> Step {
    Step::Sse("data: [DONE]\n\n".into())
}

