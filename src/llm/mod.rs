//! dct 自己用的 LLM 连接层。
//!
//! **每一处用法都必须有不依赖 LLM 的退路。** 这一层的错误都是「算了，
//! 当没有这个功能」，不是「dct 坏了」。

pub mod cli;
pub mod creds;
pub mod http;
pub mod resolve;

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
        Prompt {
            system: "s".into(),
            user: "u".into(),
            max_tokens: 64,
        }
    }

    #[test]
    fn a_fast_backend_returns_its_answer() {
        let b: Arc<dyn Backend> = Arc::new(Fixed(Ok("hello".into())));
        assert_eq!(
            complete_with_timeout(b, p(), Duration::from_secs(5)),
            Ok("hello".into())
        );
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
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "调用方没有及时放手"
        );
    }
}
