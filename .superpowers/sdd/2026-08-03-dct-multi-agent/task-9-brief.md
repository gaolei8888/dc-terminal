## Task 9: 密钥验证

**Files:**
- Create: `src/verify.rs`
- Modify: `Cargo.toml`（加 ureq）、`src/lib.rs`、`src/proto.rs`、`src/daemon.rs`
- Test: `src/verify.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 1 的 `VerifySpec`、Task 8 的协议
- Produces:
  ```rust
  pub enum VerifyOutcome { Ok, BadKey, Unreachable }
  pub fn verify_with(url: &str, key: &str, send: &dyn Fn(&str, &str) -> Result<u16, String>) -> VerifyOutcome;
  pub fn send_probe(url: &str, key: &str) -> Result<u16, String>;
  Request::VerifySecret { profile: String, value: String }
  Response::Verify(VerifyOutcome)
  ```

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthorized_means_bad_key() {
        for code in [401, 403] {
            assert_eq!(
                verify_with("u", "k", &|_, _| Ok(code)),
                VerifyOutcome::BadKey,
                "{code} 是「这个 key 不行」"
            );
        }
    }

    #[test]
    fn network_failure_is_reported_as_unreachable() {
        assert_eq!(
            verify_with("u", "k", &|_, _| Err("connection refused".into())),
            VerifyOutcome::Unreachable
        );
    }

    #[test]
    fn anything_else_passes() {
        // 刻意放行。各家 Anthropic 兼容端点行为不一，不能因为返回码奇怪
        // 就把用户拦在门外——验证的职责是抓住「key 明显是错的」，不是当网关。
        for code in [200, 400, 404, 429, 500, 502] {
            assert_eq!(
                verify_with("u", "k", &|_, _| Ok(code)),
                VerifyOutcome::Ok,
                "{code} 不该拦人"
            );
        }
    }

    #[test]
    fn the_key_reaches_the_transport() {
        let seen = std::cell::RefCell::new(String::new());
        verify_with("https://x/v1/messages", "sk-abc", &|url, key| {
            assert_eq!(url, "https://x/v1/messages");
            *seen.borrow_mut() = key.to_string();
            Ok(200)
        });
        assert_eq!(*seen.borrow(), "sk-abc");
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `~/.cargo/bin/cargo test --lib verify`
Expected: FAIL，模块不存在

- [ ] **Step 3: 加依赖**

`Cargo.toml`：

```toml
ureq = { version = "2", default-features = false, features = ["tls", "json"] }
```

- [ ] **Step 4: 实现**

`src/verify.rs`：

```rust
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 探测请求的超时。**必须小于 `client::READ_TIMEOUT`（5 秒）**：
/// 守护进程在这里等多久，界面那条连接就等多久，超过 5 秒界面会判定
/// 连接错位并丢弃重连，用户看到的是「连不上守护进程」而不是验证结果。
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifyOutcome {
    Ok,
    BadKey,
    Unreachable,
}

/// 判定逻辑本身，传输层由调用方注入——测试才能覆盖 401 / 网络错 / 奇怪返回码，
/// 而不用真打网络。
pub fn verify_with(
    url: &str,
    key: &str,
    send: &dyn Fn(&str, &str) -> Result<u16, String>,
) -> VerifyOutcome {
    match send(url, key) {
        Err(_) => VerifyOutcome::Unreachable,
        Ok(401) | Ok(403) => VerifyOutcome::BadKey,
        // 其余一律放行，见测试 `anything_else_passes` 的注释
        Ok(_) => VerifyOutcome::Ok,
    }
}

/// 真的传输层。发一个最小的 Anthropic 风格请求，只看状态码，不读 body。
///
/// `model` 随便填一个：我们不在乎它认不认这个模型（那会返回 400，属于放行），
/// 只在乎它认不认这个 key。
pub fn send_probe(url: &str, key: &str) -> Result<u16, String> {
    let body = serde_json::json!({
        "model": "probe",
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "hi"}],
    });
    let resp = ureq::AgentBuilder::new()
        .timeout(PROBE_TIMEOUT)
        .build()
        .post(url)
        .set("content-type", "application/json")
        .set("x-api-key", key)
        .set("authorization", &format!("Bearer {key}"))
        .set("anthropic-version", "2023-06-01")
        .send_json(body);

    match resp {
        Ok(r) => Ok(r.status()),
        // ureq 把 4xx/5xx 也当 Err，得挑出来——它们是有效的状态码，不是网络故障
        Err(ureq::Error::Status(code, _)) => Ok(code),
        Err(e) => Err(format!("{e}")),
    }
}
```

两个 auth 头都发：各家兼容端点认的不一样，多发一个头的代价远小于「明明 key 是对的却报无效」。

`src/lib.rs` 加 `pub mod verify;`。

`src/proto.rs`：

```rust
    VerifySecret { profile: String, value: String },
    // Response 侧
    Verify(crate::verify::VerifyOutcome),
```

`src/daemon.rs` 的 `handle`：

```rust
        Request::VerifySecret { profile, value } => {
            let (all, _) = all_profiles(profiles_dir);
            let spec = all
                .iter()
                .find(|p| p.name == profile)
                .and_then(|p| p.secret.as_ref())
                .and_then(|s| s.verify.as_ref());
            match spec {
                // 没声明 verify 的 profile 直接放行，不是错误
                None => Ok(Response::Verify(VerifyOutcome::Ok)),
                Some(v) => Ok(Response::Verify(verify_with(&v.url, &value, &send_probe))),
            }
        }
```

- [ ] **Step 5: 跑测试确认通过**

Run: `~/.cargo/bin/cargo test`
Expected: PASS

- [ ] **Step 6: 提交**

```bash
~/.cargo/bin/cargo fmt
git add -A
git commit -m "feat: 密钥存盘前先探一下端点，401/403 当场拦住

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

