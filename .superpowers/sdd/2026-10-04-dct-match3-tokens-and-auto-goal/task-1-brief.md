### Task 1: `llm` 层的「带用量的调用」

**Files:**
- Modify: `src/llm/mod.rs`、`src/llm/http.rs`
- Test: 两个文件各自的 `tests` 模块

**Interfaces:**
- Consumes: 现有 `Backend::complete`、`complete_with_timeout`、`http::extract_text`、`HttpBackend`、`Wire`。
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Usage { pub input: u64, pub output: u64 }`（在 `src/llm/mod.rs`）
  - `Backend::complete_counted(&self, p: &Prompt) -> Result<(String, Option<Usage>), LlmError>`，**有默认实现**：`self.complete(p).map(|s| (s, None))`
  - `pub fn complete_counted_with_timeout(b: Arc<dyn Backend>, p: Prompt, d: Duration) -> Result<(String, Option<Usage>), LlmError>`
  - `http::extract_usage(wire: Wire, body: &str) -> Option<Usage>`

- [ ] **Step 1: 写失败的测试**

`src/llm/http.rs` 的 `tests` 模块末尾：

```rust
    #[test]
    fn usage_is_read_from_openai_and_anthropic_shapes() {
        let o = r#"{"choices":[{"message":{"content":"2"}}],"usage":{"prompt_tokens":120,"completion_tokens":5,"total_tokens":125}}"#;
        assert_eq!(extract_usage(Wire::Openai, o), Some(Usage { input: 120, output: 5 }));
        let a = r#"{"content":[{"type":"text","text":"2"}],"usage":{"input_tokens":77,"output_tokens":3}}"#;
        assert_eq!(extract_usage(Wire::Anthropic, a), Some(Usage { input: 77, output: 3 }));
    }

    #[test]
    fn usage_is_none_when_missing_or_malformed_but_the_answer_still_comes_back() {
        for body in [
            r#"{"choices":[{"message":{"content":"2"}}]}"#,
            r#"{"choices":[{"message":{"content":"2"}}],"usage":null}"#,
            r#"{"choices":[{"message":{"content":"2"}}],"usage":"many"}"#,
            r#"{"choices":[{"message":{"content":"2"}}],"usage":{"prompt_tokens":"12","completion_tokens":5}}"#,
            r#"{"choices":[{"message":{"content":"2"}}],"usage":{"prompt_tokens":-3,"completion_tokens":5}}"#,
            r#"{"choices":[{"message":{"content":"2"}}],"usage":{"prompt_tokens":12}}"#,
            r#"{"choices":[{"message":{"content":"2"}}],"usage":{"prompt_tokens":99999999999999999999999,"completion_tokens":1}}"#,
        ] {
            assert_eq!(extract_usage(Wire::Openai, body), None, "{body}");
            assert_eq!(extract_text(Wire::Openai, body).as_deref(), Some("2"), "{body}");
        }
    }

    #[test]
    fn http_backend_complete_counted_returns_text_and_usage() {
        let sender: Arc<Sender> = Arc::new(|_, _, _| {
            Ok((200, r#"{"choices":[{"message":{"content":"3"}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#.to_string()))
        });
        let b = HttpBackend::with_sender("http://x".into(), Wire::Openai, "m".into(), Credential::Bearer("k".into()), sender);
        let p = Prompt { system: "s".into(), user: "u".into(), max_tokens: 8 };
        assert_eq!(b.complete_counted(&p), Ok(("3".to_string(), Some(Usage { input: 10, output: 2 }))));
        assert_eq!(b.complete(&p), Ok("3".to_string()));
    }
```

（用到的 `Credential::Bearer(String)`、`Prompt` 字段以 `http.rs` 现有测试的写法为准；若 `Credential` 的构造不同，照现有测试里 `with_sender` 的用法改，断言不变。）

`src/llm/mod.rs` 的 `tests` 模块末尾：

```rust
    #[test]
    fn complete_counted_default_has_no_usage_and_the_same_text() {
        let b = Fixed(Ok("hi".into()));
        let p = Prompt { system: String::new(), user: String::new(), max_tokens: 1 };
        assert_eq!(b.complete_counted(&p), Ok(("hi".to_string(), None)));
    }

    #[test]
    fn complete_counted_with_timeout_times_out_like_the_plain_one() {
        let r = complete_counted_with_timeout(
            Arc::new(Slow(Duration::from_millis(300))),
            Prompt { system: String::new(), user: String::new(), max_tokens: 1 },
            Duration::from_millis(50),
        );
        assert_eq!(r, Err(LlmError::Timeout));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test --lib llm::`
Expected: 编译失败（`Usage`、`extract_usage`、`complete_counted` 未定义）。

- [ ] **Step 3: 写实现**

`src/llm/mod.rs`：

```rust
/// 一次调用用掉的 token 数（输入、输出）。读不到就是 `None`，不是 0。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
}

pub trait Backend: Send + Sync {
    fn complete(&self, p: &Prompt) -> Result<String, LlmError>;

    /// 带用量的调用。默认不带用量（命令行后端读不到）；HTTP 后端重写它。
    fn complete_counted(&self, p: &Prompt) -> Result<(String, Option<Usage>), LlmError> {
        self.complete(p).map(|s| (s, None))
    }
}

pub fn complete_counted_with_timeout(
    b: Arc<dyn Backend>,
    p: Prompt,
    d: Duration,
) -> Result<(String, Option<Usage>), LlmError> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(b.complete_counted(&p));
    });
    match rx.recv_timeout(d) {
        Ok(r) => r,
        Err(_) => Err(LlmError::Timeout),
    }
}
```

（`LlmError` 需要 `PartialEq` 才能 `assert_eq!`：它已经 `derive(PartialEq, Eq)`，见 `mod.rs` 现有定义。）

`src/llm/http.rs`：顶部 `use super::{Backend, LlmError, Prompt, Usage};`，加

```rust
/// 读用量：OpenAI 型 `usage.prompt_tokens / completion_tokens`，Anthropic 型 `usage.input_tokens / output_tokens`。
/// 两个字段都必须是非负整数（JSON 里的 u64）才算读到；缺一个、是字符串、是负数、超出范围都是 `None`——
/// 读不到用量绝不能丢掉一个好答案，所以这里只返回 `None`，从不报错。
pub fn extract_usage(wire: Wire, body: &str) -> Option<Usage> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let u = v.get("usage")?;
    let (a, b) = match wire {
        Wire::Openai => ("prompt_tokens", "completion_tokens"),
        Wire::Anthropic => ("input_tokens", "output_tokens"),
    };
    Some(Usage { input: u.get(a)?.as_u64()?, output: u.get(b)?.as_u64()? })
}
```

`impl Backend for HttpBackend` 里把 `complete` 的主体搬进 `complete_counted`，`complete` 改成调用它再丢掉用量：

```rust
    fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
        self.complete_counted(p).map(|(s, _)| s)
    }

    fn complete_counted(&self, p: &Prompt) -> Result<(String, Option<Usage>), LlmError> {
        let body = body_for(self.wire, &self.model, p);
        let (status, text) = (self.sender)(&self.url, &self.cred, &body).map_err(|e| {
            eprintln!("LLM HTTP 调用失败：{e}");
            LlmError::Unavailable
        })?;
        if !(200..300).contains(&status) {
            eprintln!("LLM HTTP 返回 {status}");
            return Err(LlmError::Unavailable);
        }
        // 读不懂 = 没把握。绝不猜一个答案出来；用量读不到不影响答案。
        let answer = extract_text(self.wire, &text).ok_or(LlmError::Malformed)?;
        Ok((answer, extract_usage(self.wire, &text)))
    }
```

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked` 与 clippy。
Expected: 全绿，无警告。

- [ ] **Step 5: 变异检查，然后提交**

把 `extract_usage` 里的 `.as_u64()?` 改成 `.as_f64()? as u64` → `usage_is_none_when_…` 应变红（负数、字符串）；把 Anthropic 的字段名改成 OpenAI 的 → `usage_is_read_…` 变红。改回。

```bash
git add src/llm
git commit -m "feat(llm): complete_counted returns token usage from OpenAI and Anthropic style responses; unreadable usage never loses the answer"
```

---

