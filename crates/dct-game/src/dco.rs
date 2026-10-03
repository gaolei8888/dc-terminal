//! 连本机 dco：unix socket 上的 MCP（一行一条 JSON-RPC）。握手跟 dco 自己的 `dco call` 一样：
//! 先写 `{"dco_token": "<~/.dco/token>"}`，读到 `{"ok":true}`，再 initialize → notifications/initialized → tools/call。
use crate::board::GridRead;
use crate::play::{Dco, DcoError, Profile};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// dco 一次回话最多等多久。
pub const IO_TIMEOUT: Duration = Duration::from_secs(10);

pub struct DcoClient {
    r: BufReader<UnixStream>,
    w: UnixStream,
    next_id: u64,
}

fn err(code: &str, message: impl Into<String>) -> DcoError {
    DcoError { code: code.into(), message: message.into() }
}

/// 读写出错：超时单独说，别的都当作 dco 断开了。
fn io_err(e: &std::io::Error) -> DcoError {
    match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
            err("dco_timeout", "dco 没有回应，等了 10 秒。先看看 dco 是不是卡住了。")
        }
        _ => err("dco_down", "dco 断开了连接"),
    }
}

impl DcoClient {
    /// `dir` 是 `~/.dco`。
    pub fn connect(dir: &Path) -> Result<DcoClient, DcoError> {
        Self::connect_with_timeout(dir, IO_TIMEOUT)
    }

    /// 同 `connect`，读写的等待时间可以自己定（测试用短的）。
    pub fn connect_with_timeout(dir: &Path, timeout: Duration) -> Result<DcoClient, DcoError> {
        let endpoint: Value = std::fs::read_to_string(dir.join("endpoint.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .ok_or_else(|| err("dco_down", "dco 没在运行，先打开 dco 再试"))?;
        let socket = endpoint["socket"].as_str().ok_or_else(|| err("dco_down", "dco 没在运行，先打开 dco 再试"))?;
        let token = std::fs::read_to_string(dir.join("token")).map_err(|_| err("dco_down", "读不到 dco 的钥匙文件，dco 还没启动过？"))?;
        let stream = UnixStream::connect(socket).map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => err("dco_blocked", "这里不让连 dco"),
            _ => err("dco_down", "dco 没在运行，先打开 dco 再试"),
        })?;
        stream.set_read_timeout(Some(timeout)).map_err(|e| err("dco_down", e.to_string()))?;
        stream.set_write_timeout(Some(timeout)).map_err(|e| err("dco_down", e.to_string()))?;
        let w = stream.try_clone().map_err(|e| err("dco_down", e.to_string()))?;
        let mut c = DcoClient { r: BufReader::new(stream), w, next_id: 1 };
        c.send(&json!({ "dco_token": token.trim() }))?;
        let ack = c.read_line()?;
        if ack.get("ok") != Some(&Value::Bool(true)) {
            return Err(err("dco_refused", "dco 拒绝了连接"));
        }
        let init = c.request("initialize", json!({
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": { "name": "dct", "version": env!("CARGO_PKG_VERSION") }
        }))?;
        let _ = init;
        c.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))?;
        Ok(c)
    }

    fn send(&mut self, v: &Value) -> Result<(), DcoError> {
        writeln!(self.w, "{v}").map_err(|e| io_err(&e))
    }

    fn read_line(&mut self) -> Result<Value, DcoError> {
        let mut line = String::new();
        match self.r.read_line(&mut line) {
            Ok(0) => Err(err("dco_down", "dco 断开了连接")),
            Err(e) => Err(io_err(&e)),
            Ok(_) => serde_json::from_str(line.trim()).map_err(|_| err("dco_down", "dco 回了看不懂的话")),
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, DcoError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let reply = self.read_line()?;
        if let Some(e) = reply.get("error") {
            let msg = e["message"].as_str().unwrap_or("dco 报错");
            let code = e["code"].as_i64();
            if code == Some(-32602) && msg.contains("unknown tool") {
                return Err(err("dco_too_old", msg));
            }
            return Err(err("dco_error", match code {
                Some(c) => format!("{msg}（协议错误码 {c}）"),
                None => msg.to_string(),
            }));
        }
        Ok(reply["result"].clone())
    }

    /// 调一个工具，返回它文字部分里的 JSON。工具报错时错误码取 dco 给的 `error.code`。
    pub fn call(&mut self, tool: &str, args: Value) -> Result<Value, DcoError> {
        let result = self.request("tools/call", json!({ "name": tool, "arguments": args }))?;
        let text = result["content"][0]["text"].as_str().unwrap_or("{}");
        let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        if result["isError"].as_bool().unwrap_or(false) {
            let e = &body["error"];
            return Err(err(e["code"].as_str().unwrap_or("dco_error"), e["message"].as_str().unwrap_or(text)));
        }
        Ok(body)
    }
}

impl Dco for DcoClient {
    fn read_grid(&mut self, p: &Profile) -> Result<GridRead, DcoError> {
        let mut args = json!({
            "window": p.window,
            "region": { "x": p.region[0], "y": p.region[1], "w": p.region[2], "h": p.region[3] },
            "rows": p.rows, "cols": p.cols,
        });
        if let (Some(extra), Some(obj)) = (p.extra.as_object(), args.as_object_mut()) {
            for (k, v) in extra {
                obj.insert(k.clone(), v.clone());
            }
        }
        let body = self.call("read_grid", args)?;
        serde_json::from_value(body).map_err(|e| err("dco_error", format!("dco 的棋盘回复读不懂：{e}")))
    }

    fn swipe(&mut self, p: &Profile, from: (f64, f64), to: (f64, f64)) -> Result<(), DcoError> {
        self.call("swipe", json!({
            "window": p.window,
            "from": { "x": from.0, "y": from.1 }, "to": { "x": to.0, "y": to.1 },
        }))
        .map(|_| ())
    }
}
