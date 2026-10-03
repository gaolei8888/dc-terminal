### Task 3: 连 dco

**Files:**
- Create: `crates/dct-game/src/dco.rs`、`src/dco_tests.rs`
- Modify: `crates/dct-game/src/lib.rs`

**Interfaces:**
- Consumes: 任务 2 的 `Dco`、`Profile`、`DcoError`。
- Produces: `DcoClient::connect(dir: &Path) -> Result<DcoClient, DcoError>`（`dir` 是 `~/.dco`；失败的错误码 `dco_down` / `dco_refused`）；`impl Dco for DcoClient`；`DcoClient::call(tool, args) -> Result<Value, DcoError>`（工具报错时错误码取 dco 给的 `error.code`，比如 `halted`、`not_a_grid`、`not_found`、`screen_locked`）。

协议依据（dc-octo `src/client.rs`、`src/mcp.rs`）：先写一行 `{"dco_token": "<~/.dco/token 的 64 位十六进制>"}`，读到 `{"ok":true}`；再 `initialize` → `notifications/initialized` → `tools/call`；一行一条 JSON-RPC；工具结果在 `result.content[0].text` 里（一段 JSON 文字），`result.isError` 为真时那段 JSON 是 `{"error":{"code","message"}}`。`swipe`、`read_grid` 都不需要执行票（read、self 档）。


- [ ] **Step 1: 在 `crates/dct-game/src/lib.rs` 里加接线**

`pub mod choose;` 下面加：

```rust
#[cfg(unix)]
pub mod dco;
```

文件末尾加：

```rust
#[cfg(all(test, unix))]
mod dco_tests;
```

- [ ] **创建 `crates/dct-game/src/dco.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 连本机 dco：unix socket 上的 MCP（一行一条 JSON-RPC）。握手跟 dco 自己的 `dco call` 一样：
//! 先写 `{"dco_token": "<~/.dco/token>"}`，读到 `{"ok":true}`，再 initialize → notifications/initialized → tools/call。
use crate::board::GridRead;
use crate::play::{Dco, DcoError, Profile};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub struct DcoClient {
    r: BufReader<UnixStream>,
    w: UnixStream,
    next_id: u64,
}

fn err(code: &str, message: impl Into<String>) -> DcoError {
    DcoError { code: code.into(), message: message.into() }
}

impl DcoClient {
    /// `dir` 是 `~/.dco`。
    pub fn connect(dir: &Path) -> Result<DcoClient, DcoError> {
        let endpoint: Value = std::fs::read_to_string(dir.join("endpoint.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .ok_or_else(|| err("dco_down", "dco 没在运行，先打开 dco 再试"))?;
        let socket = endpoint["socket"].as_str().ok_or_else(|| err("dco_down", "dco 没在运行，先打开 dco 再试"))?;
        let token = std::fs::read_to_string(dir.join("token")).map_err(|_| err("dco_down", "读不到 dco 的钥匙文件，dco 还没启动过？"))?;
        let stream = UnixStream::connect(socket).map_err(|_| err("dco_down", "dco 没在运行，先打开 dco 再试"))?;
        let w = stream.try_clone().map_err(|e| err("dco_down", e.to_string()))?;
        let mut c = DcoClient { r: BufReader::new(stream), w, next_id: 1 };
        c.send(&json!({ "dco_token": token.trim() }))?;
        let ack = c.read_line()?;
        if ack.get("ok") != Some(&Value::Bool(true)) {
            return Err(err("dco_refused", format!("dco 拒绝了连接：{ack}")));
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
        writeln!(self.w, "{v}").map_err(|_| err("dco_down", "dco 断开了连接"))
    }

    fn read_line(&mut self) -> Result<Value, DcoError> {
        let mut line = String::new();
        match self.r.read_line(&mut line) {
            Ok(0) | Err(_) => Err(err("dco_down", "dco 断开了连接")),
            Ok(_) => serde_json::from_str(line.trim()).map_err(|_| err("dco_down", "dco 回了看不懂的话")),
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, DcoError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let reply = self.read_line()?;
        if let Some(e) = reply.get("error") {
            return Err(err("dco_error", e["message"].as_str().unwrap_or("dco 报错")));
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
```

- [ ] **创建 `crates/dct-game/src/dco_tests.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
use crate::dco::DcoClient;
use crate::play::{Dco, Profile};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

/// 假 dco：按 dco 真实的握手和回复形状说话。`handler(tool, args)` 返回 (JSON, 是不是错误)。
fn fake(dir: &std::path::Path, token: &str, handler: impl Fn(&str, &Value) -> (Value, bool) + Send + 'static) -> std::thread::JoinHandle<Vec<String>> {
    let sock = dir.join("dco.sock");
    std::fs::write(dir.join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.join("token"), token).unwrap();
    let l = UnixListener::bind(&sock).unwrap();
    let token = token.to_string();
    std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let mut seen = vec![];
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let hello: Value = serde_json::from_str(line.trim()).unwrap();
        seen.push(hello.to_string());
        if hello["dco_token"] != token {
            writeln!(w, r#"{{"ok":false}}"#).unwrap();
            return seen;
        }
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return seen;
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            seen.push(req["method"].as_str().unwrap().to_string());
            let Some(id) = req.get("id") else { continue };
            let result = match req["method"].as_str().unwrap() {
                "initialize" => json!({"protocolVersion": "2025-06-18"}),
                "tools/call" => {
                    let (body, is_error) = handler(req["params"]["name"].as_str().unwrap(), &req["params"]["arguments"]);
                    json!({"content": [{"type": "text", "text": body.to_string()}], "isError": is_error})
                }
                _ => json!({}),
            };
            writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap();
        }
    })
}

fn profile() -> Profile {
    Profile { window: json!({"app": "iPhone Mirroring"}), region: [0.2, 0.3, 0.5, 0.4], rows: 2, cols: 2, extra: json!({"class_de": 20.0}) }
}

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn reads_a_grid_with_the_extra_parameters_and_swipes() {
    let dir = tempfile::tempdir().unwrap();
    let h = fake(dir.path(), TOKEN, |tool, args| match tool {
        "read_grid" => {
            assert_eq!(args["rows"], 2);
            assert_eq!(args["class_de"], 20.0);
            assert_eq!(args["region"]["w"], 0.5);
            assert_eq!(args["window"]["app"], "iPhone Mirroring");
            (json!({"rows":2,"cols":2,"cells":[[0,0],[1,1]],"odd":[[false,false],[false,false]],
                    "classes":[{"id":0,"rgb":[1,2,3],"count":2},{"id":1,"rgb":[4,5,6],"count":2}],"elapsed_ms":1}), false)
        }
        "swipe" => {
            assert_eq!(args["from"]["x"], 0.3);
            assert_eq!(args["to"]["y"], 0.5);
            (json!({"swiped": true}), false)
        }
        other => panic!("没想到会调 {other}"),
    });
    let mut c = DcoClient::connect(dir.path()).unwrap();
    let g = c.read_grid(&profile()).unwrap();
    assert_eq!((g.rows, g.cols, g.classes.len()), (2, 2, 2));
    c.swipe(&profile(), (0.3, 0.4), (0.4, 0.5)).unwrap();
    drop(c);
    let seen = h.join().unwrap();
    assert_eq!(&seen[1..4], ["initialize", "notifications/initialized", "tools/call"]);
}

#[test]
fn a_tool_error_keeps_dcos_error_code() {
    let dir = tempfile::tempdir().unwrap();
    let _h = fake(dir.path(), TOKEN, |_, _| (json!({"error": {"code": "halted", "message": "急停中"}}), true));
    let mut c = DcoClient::connect(dir.path()).unwrap();
    let e = c.swipe(&profile(), (0.3, 0.4), (0.4, 0.5)).unwrap_err();
    assert_eq!((e.code.as_str(), e.message.as_str()), ("halted", "急停中"));
}

#[test]
fn a_wrong_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let _h = fake(dir.path(), TOKEN, |_, _| (json!({}), false));
    std::fs::write(dir.path().join("token"), "f".repeat(64)).unwrap();
    assert_eq!(DcoClient::connect(dir.path()).err().unwrap().code, "dco_refused");
}

#[test]
fn no_endpoint_file_means_dco_is_not_running() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(DcoClient::connect(dir.path()).err().unwrap().code, "dco_down");
}

#[test]
fn a_dead_socket_means_dco_is_not_running() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("endpoint.json"), json!({"socket": dir.path().join("none.sock")}).to_string()).unwrap();
    std::fs::write(dir.path().join("token"), TOKEN).unwrap();
    assert_eq!(DcoClient::connect(dir.path()).err().unwrap().code, "dco_down");
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test -p dct-game`
Expected: `33 passed`。

- [ ] **Step 3: 提交**

```bash
cargo clippy -p dct-game --all-targets
git add crates/dct-game
git commit -m "feat(game): talk to dco over its unix socket (read_grid, swipe)"
```


---

