//! 流式调用：传输可注入，SSE 解析是纯函数。
//!
//! 整段式的 `HttpBackend` 一次读完整个响应，没法在第一句出来时就开口，也没法在
//! 对面不想听了的时候把连接关掉。这里只做「发请求、拿到读取端」和「把读到的
//! 字节解成增量」；断句在 `ask::split`，取消和期限在 `ask`。
//!
//! 读取端是**阻塞**的。ureq 2.x 里响应头和响应体共用同一个读超时（`timeout_read`），
//! 没法「等头 ≥ 1 秒、读体 250 毫秒轮询」，所以不靠读超时轮询取消：读在工作线程上
//! 阻塞，调用方轮询取消标志，工作线程在每次读返回时看标志、丢掉 reader 关连接
//! （见 `ask::run_turn`）。整体期限交给 ureq 的 `.timeout()`，保证读最迟在期限
//! 到点时返回。

use super::creds::Credential;
use crate::profile::Wire;
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;

/// 连接超时的上限：连不上就快点说连不上，别把整个期限烧在 SYN 上。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// 一行 SSE 的上限。网关的一个增量只有几十字节，超过这个数就是格式坏了。
pub const MAX_SSE_LINE: usize = 64 * 1024;

/// 发请求这一步为什么没成。对应 `ask::ErrorCode` 的一部分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamError {
    /// DNS / 拒绝连接 / TLS / 连接超时：没拿到响应头。
    Unreachable,
    /// 401 / 403。
    Auth,
    /// 429 / 503。
    Busy,
    /// 连上了，但到期限也没等到响应头。
    Slow,
    /// 其他非 2xx，或读不懂。
    BadReply,
}

pub type Body = Box<dyn Read + Send>;

/// 发出流式请求，**阻塞到响应头到达**，返回响应体读取端（已解 chunked）。
/// `deadline` 是从现在起整个请求（含读体）还能用的时间。丢掉返回的 `Body` 就是关连接。
pub trait StreamOpen: Send + Sync {
    fn open(&self, body: &Value, deadline: Duration) -> Result<Body, StreamError>;
}

/// 请求体：OpenAI 兼容、`stream:true`、**没有 `tools` / `function`**（ask 不给模型任何工具）。
pub fn stream_body(model: &str, messages: &[Value], max_tokens: u32) -> Value {
    json!({"model": model, "stream": true, "max_tokens": max_tokens, "messages": messages})
}

/// 真实传输：ureq，OpenAI 线格式。
pub struct UreqOpen {
    url: String,
    cred: Credential,
}

impl UreqOpen {
    pub fn new(url: String, cred: Credential) -> UreqOpen {
        UreqOpen { url, cred }
    }
}

impl StreamOpen for UreqOpen {
    fn open(&self, body: &Value, deadline: Duration) -> Result<Body, StreamError> {
        let agent = crate::sys::tls::agent_builder()
            .timeout(deadline)
            .timeout_connect(deadline.min(CONNECT_TIMEOUT))
            .build();
        let req = super::http::authed(Wire::Openai, agent.post(&self.url), &self.cred)
            .map_err(|_| StreamError::Unreachable)?;
        match req.send_json(body.clone()) {
            Ok(r) if (200..300).contains(&r.status()) => Ok(Box::new(r.into_reader())),
            Ok(_) => Err(StreamError::BadReply),
            Err(ureq::Error::Status(code, _)) => Err(match code {
                401 | 403 => StreamError::Auth,
                429 | 503 => StreamError::Busy,
                _ => StreamError::BadReply,
            }),
            Err(ureq::Error::Transport(t)) => Err(classify_transport(&t)),
        }
    }
}

/// 「慢」和「联不上」的判据是**有没有拿到响应头**：连接阶段的失败是联不上，
/// 连上之后读超时是慢。
fn classify_transport(t: &ureq::Transport) -> StreamError {
    use std::error::Error as _;
    let timed_out = t
        .source()
        .and_then(|s| s.downcast_ref::<std::io::Error>())
        .is_some_and(|e| matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock));
    if timed_out && t.kind() == ureq::ErrorKind::Io {
        StreamError::Slow
    } else {
        StreamError::Unreachable
    }
}

/// 字节 → `data:` 行的内容。块边界可以落在行中间。注释行（`:` 开头）、空行、
/// `event:` / `id:` / `retry:` 和不认识的行都忽略。
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LineTooLong;

impl SseParser {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, LineTooLong> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(d) = line.trim_end_matches(['\r', '\n']).strip_prefix("data:") {
                out.push(d.strip_prefix(' ').unwrap_or(d).to_string());
            }
        }
        if self.buf.len() > MAX_SSE_LINE {
            return Err(LineTooLong);
        }
        Ok(out)
    }
}

/// `data:` 的内容读不懂。
#[derive(Debug, PartialEq, Eq)]
pub struct BadFrame;

#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    /// 可见文字的增量（可能为空串：角色帧、结束帧）。
    Delta(String),
    Done,
}

/// 一个 `data:` 的内容 → 增量。**只读 `choices[0].delta.content`**：`reasoning` /
/// `reasoning_content` 之类的思考字段一律不读，所以不可能外露。JSON 坏了、
/// 或流里夹了 `error` 对象 → `Err`。
pub fn parse_frame(payload: &str) -> Result<Frame, BadFrame> {
    let payload = payload.trim();
    if payload == "[DONE]" {
        return Ok(Frame::Done);
    }
    let v: Value = serde_json::from_str(payload).map_err(|_| BadFrame)?;
    if v.get("error").is_some() {
        return Err(BadFrame);
    }
    let text = v["choices"][0]["delta"]["content"].as_str().unwrap_or("");
    Ok(Frame::Delta(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(chunks: &[&[u8]]) -> Vec<String> {
        let mut p = SseParser::default();
        chunks.iter().flat_map(|c| p.push(c).unwrap()).collect()
    }

    #[test]
    fn data_lines_come_out_whole_whatever_the_chunk_boundaries() {
        let whole = feed(&[b"data: {\"a\":1}\n\ndata: [DONE]\n\n"]);
        assert_eq!(whole, ["{\"a\":1}", "[DONE]"]);
        // 每个字节一块：边界落在行中间、`data:` 中间、换行前。
        let bytes = b"data: {\"a\":1}\r\n\r\ndata:[DONE]\n";
        let one_by_one: Vec<&[u8]> = bytes.chunks(1).collect();
        assert_eq!(feed(&one_by_one), ["{\"a\":1}", "[DONE]"]);
    }

    #[test]
    fn comments_blank_lines_other_fields_and_junk_are_ignored() {
        let got = feed(&[b": keep-alive\n\nevent: message\nid: 7\nretry: 100\nnot sse at all\ndata: x\n"]);
        assert_eq!(got, ["x"]);
    }

    #[test]
    fn a_half_line_waits_for_its_newline() {
        let mut p = SseParser::default();
        assert!(p.push(b"data: par").unwrap().is_empty());
        assert_eq!(p.push(b"tial\n").unwrap(), ["partial"]);
    }

    #[test]
    fn an_enormous_line_is_refused() {
        let mut p = SseParser::default();
        assert_eq!(p.push(&vec![b'x'; MAX_SSE_LINE + 1]), Err(LineTooLong));
        // 有换行的大块不算「一行巨大」，只要每行都没超。
        let mut p = SseParser::default();
        assert!(p.push(&vec![b'x'; MAX_SSE_LINE]).is_ok());
    }

    #[test]
    fn frames_carry_only_visible_content() {
        let f = |s| parse_frame(s);
        assert_eq!(f(r#"{"choices":[{"delta":{"content":"你好"}}]}"#), Ok(Frame::Delta("你好".into())));
        assert_eq!(f(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#), Ok(Frame::Delta(String::new())));
        assert_eq!(f(r#"{"choices":[{"delta":{"content":null},"finish_reason":"stop"}]}"#), Ok(Frame::Delta(String::new())));
        assert_eq!(f("[DONE]"), Ok(Frame::Done));
        assert_eq!(f(" [DONE] "), Ok(Frame::Done));
    }

    #[test]
    fn reasoning_fields_are_never_read() {
        for field in ["reasoning", "reasoning_content"] {
            let s = format!(r#"{{"choices":[{{"delta":{{"{field}":"内心戏"}}}}]}}"#);
            assert_eq!(parse_frame(&s), Ok(Frame::Delta(String::new())));
            let both = format!(r#"{{"choices":[{{"delta":{{"{field}":"内心戏","content":"嘴上说的"}}}}]}}"#);
            assert_eq!(parse_frame(&both), Ok(Frame::Delta("嘴上说的".into())));
        }
    }

    #[test]
    fn broken_json_and_in_stream_errors_are_bad() {
        assert!(parse_frame("{not json").is_err());
        assert!(parse_frame(r#"{"error":{"message":"boom"}}"#).is_err());
        assert!(parse_frame("").is_err());
    }

    #[test]
    fn the_request_body_streams_and_offers_no_tools() {
        let b = stream_body("m", &[json!({"role":"user","content":"u"})], 200);
        assert_eq!(b["stream"], true);
        assert_eq!(b["max_tokens"], 200);
        for k in ["tools", "functions", "tool_choice", "function_call"] {
            assert!(b.get(k).is_none(), "{k} 不该出现");
        }
    }
}
