//! 测试用的假 dco。
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;

/// 假 dco：按 dco 真实的握手和回复形状说话。`handler(tool, args)` 返回 (JSON, 是不是错误, 图片 base64)。
pub fn serve(
    dir: &Path,
    token: &str,
    handler: impl Fn(&str, &Value) -> (Value, bool, Option<String>) + Send + 'static,
) -> std::thread::JoinHandle<Vec<String>> {
    let sock = dir.join("dco.sock");
    std::fs::write(
        dir.join("endpoint.json"),
        json!({"socket": sock}).to_string(),
    )
    .unwrap();
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
                    let (body, is_error, image) = handler(
                        req["params"]["name"].as_str().unwrap(),
                        &req["params"]["arguments"],
                    );
                    let mut content = vec![json!({"type": "text", "text": body.to_string()})];
                    if let Some(data) = image {
                        content
                            .push(json!({"type": "image", "mimeType": "image/png", "data": data}));
                    }
                    json!({"content": content, "isError": is_error})
                }
                _ => json!({}),
            };
            writeln!(
                w,
                "{}",
                json!({"jsonrpc": "2.0", "id": id, "result": result})
            )
            .unwrap();
        }
    })
}
