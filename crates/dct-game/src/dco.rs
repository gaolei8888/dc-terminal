//! 连本机 dco：unix socket 上的 MCP（一行一条 JSON-RPC）。握手跟 dco 自己的 `dco call` 一样：
//! 先写 `{"dco_token": "<~/.dco/token>"}`，读到 `{"ok":true}`，再 initialize → notifications/initialized → tools/call。
use crate::board::GridRead;
use crate::play::{Dco, DcoError, Profile, Seen, SwipeOutcome, SwipeSettle, SETTLE_QUIET_MS, SETTLE_TIMEOUT_MS};
use crate::screen::Element;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// show_status 的说明条最多这么多个字符（不是字节）。
const STATUS_TEXT_MAX_CHARS: usize = 16;

/// dco 一次回话最多等多久。
pub const IO_TIMEOUT: Duration = Duration::from_secs(10);

pub struct DcoClient {
    r: BufReader<UnixStream>,
    w: UnixStream,
    next_id: u64,
    /// 这台 dco 没有 show_status（太旧）或者回得太慢：这条连接上不再发，免得每一步白等。
    no_show_status: bool,
    /// 这台 dco 对这些状态回了 `bad_request`（不认识这个状态或参数）：同一个状态不再发，其他状态照发。
    unsupported_states: HashSet<String>,
    /// 同理：认字（OCR）回得太慢或这台 dco 太旧，就不再读，免得每一步白等 10 秒。
    no_see_text: bool,
    /// 这台 dco 不认 swipe 的 `settle` 参数（回复里没有 `settle`）：这条连接上不再带，免得每步白发。
    /// 注意只有「回复里压根没有 settle」才置位；等待出错、报告读不懂都不算。
    no_swipe_settle: bool,
    /// 带 settle 的 swipe 等回复的时长：要比 dco 自己最多等的 `SETTLE_TIMEOUT_MS` 还长，否则会把还在等的划动当成失败。
    pub(crate) settle_call_timeout: Duration,
    /// 一次请求等回复的总时长；跳过别的回复也算在里面。
    timeout: Duration,
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
        let mut c = DcoClient { r: BufReader::new(stream), w, next_id: 1, no_show_status: false, unsupported_states: HashSet::new(), no_see_text: false, no_swipe_settle: false, settle_call_timeout: timeout.max(Duration::from_millis(SETTLE_TIMEOUT_MS as u64 + 4000)), timeout };
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
        // 前面超时的调用，回复可能晚到：按 id 认回复，别人的（和没有 id 的通知）跳过。
        let deadline = std::time::Instant::now() + self.timeout;
        let reply = loop {
            let r = self.read_line()?;
            if r.get("id") == Some(&json!(id)) {
                break r;
            }
            if std::time::Instant::now() >= deadline {
                return Err(io_err(&std::io::Error::from(std::io::ErrorKind::TimedOut)));
            }
        };
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

    fn swipe_settle(&mut self, p: &Profile, from: (f64, f64), to: (f64, f64)) -> SwipeOutcome {
        if self.no_swipe_settle {
            return match self.swipe(p, from, to) {
                Err(e) => SwipeOutcome::NotSwiped(e),
                Ok(()) => SwipeOutcome::SwipedNoSettle(err("unsupported", "这个 dco 不会等屏幕稳定")),
            };
        }
        let [x, y, w, h] = p.region;
        let normal = self.timeout;
        self.timeout = self.settle_call_timeout;
        let _ = self.w.set_read_timeout(Some(self.timeout));
        let res = self.call("swipe", json!({
            "window": p.window,
            "from": { "x": from.0, "y": from.1 }, "to": { "x": to.0, "y": to.1 },
            "settle": { "quiet_ms": SETTLE_QUIET_MS, "timeout_ms": SETTLE_TIMEOUT_MS, "region": { "x": x, "y": y, "w": w, "h": h } },
        }));
        self.timeout = normal;
        let _ = self.w.set_read_timeout(Some(normal));
        let body = match res {
            Ok(b) => b,
            // 请求已经写出去了，划动很可能已经发生：不能说「没划」，否则调用方会再划一次。
            // 其他传输错误（断线等）分不清是写出前还是写出后，保持 NotSwiped。
            Err(e) if e.code == "dco_timeout" => return SwipeOutcome::SwipedNoSettle(e),
            Err(e) => return SwipeOutcome::NotSwiped(e),
        };
        // 到这里划动已经发生；下面任何一种“没拿到报告”都不许再划。
        let s = &body["settle"];
        if !s.is_object() {
            // 旧 dco 不认 settle 参数，只划了。以后别再带。
            self.no_swipe_settle = true;
            return SwipeOutcome::SwipedNoSettle(err("unsupported", "这个 dco 不会等屏幕稳定"));
        }
        if s["error"].is_object() {
            let e = &s["error"];
            return SwipeOutcome::SwipedNoSettle(err(e["code"].as_str().unwrap_or("dco_error"), e["message"].as_str().unwrap_or("等屏幕稳定时出错")));
        }
        // 缺 changed / settled 的报告不能当「弹回原样」用。
        let (Some(changed), Some(settled)) = (s["changed"].as_bool(), s["settled"].as_bool()) else {
            return SwipeOutcome::SwipedNoSettle(err("bad_settle", "dco 回的稳定报告读不懂"));
        };
        SwipeOutcome::Settled(SwipeSettle {
            changed,
            change: s["change"].as_f64().unwrap_or(0.0),
            settled,
            timed_out: s["timed_out"].as_bool().unwrap_or(false),
            settled_ms: s["settled_ms"].as_u64(),
        })
    }

    fn see_text(&mut self, p: &Profile) -> Result<Seen, DcoError> {
        if self.no_see_text {
            return Err(err("unsupported", "这个 dco 不会认画面上的字"));
        }
        let body = match self.call("see", json!({ "window": p.window, "source": "ocr" })) {
            Ok(b) => b,
            Err(e) => {
                if e.code == "dco_too_old" || e.code == "dco_timeout" {
                    self.no_see_text = true;
                }
                return Err(e);
            }
        };
        let elements = body["elements"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| Some(Element { id: e["id"].as_str()?.to_string(), text: e["text"].as_str().unwrap_or("").to_string() }))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Seen {
            snapshot_id: body["snapshot_id"].as_str().unwrap_or("").to_string(),
            observation_id: body["observation_id"].as_str().map(String::from),
            elements,
        })
    }

    fn show_status(&mut self, state: &str) {
        self.show_status_with(state, None, None);
    }

    fn show_status_with(&mut self, state: &str, text: Option<&str>, theme: Option<&str>) {
        if self.no_show_status || self.unsupported_states.contains(state) {
            return;
        }
        let mut args = json!({ "state": state });
        if let Some(t) = text {
            args["text"] = json!(t.chars().take(STATUS_TEXT_MAX_CHARS).collect::<String>());
        }
        if let Some(t) = theme {
            args["theme"] = json!(t);
        }
        // 结果不重要。旧 dco 没这个工具、或者超时，都别再试：超时一次就是 10 秒，每步一次游戏就拖死了。
        // 只有这个状态 / 参数不被认（bad_request）：只记住这个状态，别的状态（think / look）照发。
        if let Err(e) = self.call("show_status", args) {
            if e.code == "dco_too_old" || e.code == "dco_timeout" {
                self.no_show_status = true;
            } else if e.code == "bad_request" {
                self.unsupported_states.insert(state.to_string());
            }
        }
    }

    fn tap(&mut self, snapshot_id: &str, element_id: &str) -> Result<(), DcoError> {
        self.call("tap", json!({ "snapshot_id": snapshot_id, "element_id": element_id })).map(|_| ())
    }
}
