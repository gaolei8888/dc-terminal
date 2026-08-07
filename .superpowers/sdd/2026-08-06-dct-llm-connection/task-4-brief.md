### Task 4: 后端 trait、提示词、硬超时

**Files:**
- Modify: `src/llm/mod.rs`

**Interfaces:**
- Consumes: 无
- Produces:
  - `llm::Prompt { pub system: String, pub user: String, pub max_tokens: u32 }`
  - `llm::LlmError { Unavailable, Timeout, Malformed }`（`Debug, Clone, Copy, PartialEq, Eq`）
  - `llm::Backend: Send + Sync`，方法 `fn complete(&self, p: &Prompt) -> Result<String, LlmError>`
  - `llm::complete_with_timeout(b: Arc<dyn Backend>, p: Prompt, d: Duration) -> Result<String, LlmError>`

**说明：** `complete_with_timeout` 是「绝不进 TUI 重绘循环」这条硬约束的落地：调用方拿到的最坏情况是 `d` 之后的一个 `Timeout`，不是无限等待。用 `std::sync::mpsc` 的 `recv_timeout`，不引入 async。

超时后**工作线程会继续跑到自己结束**（Rust 杀不掉线程）——这是可以接受的：它只是在等一个 HTTP 响应或一个子进程，结束后往一个没人听的 channel 里送一次就退了。**关键是调用方已经不等它了。**

- [ ] **Step 1: 写失败的测试**

`src/llm/mod.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    struct Fixed(Result<String, LlmError>);
    impl Backend for Fixed {
        fn complete(&self, _p: &Prompt) -> Result<String, LlmError> {
            self.0.clone()
        }
    }

    struct Slow(Duration);
    impl Backend for Slow {
        fn complete(&self, _p: &Prompt) -> Result<String, LlmError> {
            std::thread::sleep(self.0);
            Ok("too late".into())
        }
    }

    fn p() -> Prompt {
        Prompt { system: "s".into(), user: "u".into(), max_tokens: 64 }
    }

    #[test]
    fn a_fast_backend_returns_its_answer() {
        let b: Arc<dyn Backend> = Arc::new(Fixed(Ok("hello".into())));
        assert_eq!(complete_with_timeout(b, p(), Duration::from_secs(5)), Ok("hello".into()));
    }

    #[test]
    fn a_backend_error_passes_through() {
        let b: Arc<dyn Backend> = Arc::new(Fixed(Err(LlmError::Unavailable)));
        assert_eq!(
            complete_with_timeout(b, p(), Duration::from_secs(5)),
            Err(LlmError::Unavailable)
        );
    }

    /// 这条是「绝不冻住界面」的回归点。一个冻住的 dct 和一个死掉的 agent
    /// 在屏幕上长得一模一样——这是用户最恨的失败模式。
    #[test]
    fn a_slow_backend_gives_up_instead_of_blocking_forever() {
        let b: Arc<dyn Backend> = Arc::new(Slow(Duration::from_secs(30)));
        let started = std::time::Instant::now();
        let r = complete_with_timeout(b, p(), Duration::from_millis(150));
        assert_eq!(r, Err(LlmError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(2), "调用方没有及时放手");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib llm::tests`
Expected: 编译失败，`cannot find type 'Prompt'`

- [ ] **Step 3: 写实现**

`src/llm/mod.rs` 改成：

```rust
//! dct 自己用的 LLM 连接层。
//!
//! **每一处用法都必须有不依赖 LLM 的退路。** 这一层的错误都是「算了，
//! 当没有这个功能」，不是「dct 坏了」。

pub mod creds;

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Prompt {
    pub system: String,
    pub user: String,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmError {
    /// 连不上、没配、凭据拿不到。功能安静下线。
    Unavailable,
    Timeout,
    /// 回来了但读不懂。**当作「没把握」处理**，绝不猜。
    Malformed,
}

pub trait Backend: Send + Sync {
    fn complete(&self, p: &Prompt) -> Result<String, LlmError>;
}

/// 在工作线程上跑，最多等 `d`。
///
/// 超时后那个线程会继续跑到自己结束（Rust 杀不掉线程）——可以接受：
/// 它只是在等一个 HTTP 响应或一个子进程，完事往没人听的 channel 送一次就退。
/// **关键是调用方已经不等它了**，而这正是「绝不冻住界面」要保的东西。
pub fn complete_with_timeout(
    b: Arc<dyn Backend>,
    p: Prompt,
    d: Duration,
) -> Result<String, LlmError> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(b.complete(&p));
    });
    match rx.recv_timeout(d) {
        Ok(r) => r,
        Err(_) => Err(LlmError::Timeout),
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib llm::`
Expected: 全绿（Task 3 的 6 个 + 本任务 3 个）

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && git add src/llm/mod.rs
git commit -m "feat(llm): add the Backend trait and a hard call timeout

complete_with_timeout is how the 'never block the TUI' constraint is
enforced: the worst a caller can experience is a Timeout after the budget.
A frozen dct and a dead agent look identical on screen, which makes blocking
the redraw loop the most expensive failure this tool has."
```

---

