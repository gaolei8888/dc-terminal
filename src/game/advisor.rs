//! 把「问模型选哪一步」接到 dct 自己的 LLM 连接层上。
//! 凭据怎么取、发给哪台机器，全在 `llm::resolve`，这里不碰。
use crate::llm::{complete_with_timeout, Backend, Prompt};
use dct_game::ask::{parse_reply, prompt_text, Advice, Advisor, AskInput};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct LlmAdvisor {
    backend: Arc<dyn Backend>,
    model: String,
    timeout: Duration,
    /// 问不通以后置位：后面不再发问，只用规则。
    down: AtomicBool,
}

impl LlmAdvisor {
    pub fn new(backend: Arc<dyn Backend>, model: String) -> LlmAdvisor {
        LlmAdvisor { backend, model, timeout: Duration::from_secs(20), down: AtomicBool::new(false) }
    }
}

const SYSTEM: &str = "你在帮一个三消游戏的自动玩家选下一步。只能从给出的编号里选，不要自己编走法。";

impl Advisor for LlmAdvisor {
    fn pick(&self, i: &AskInput) -> Option<Advice> {
        if self.down.load(Ordering::SeqCst) {
            return None;
        }
        let p = Prompt { system: SYSTEM.into(), user: prompt_text(i), max_tokens: 64 };
        // 任何错误都是「没问成」：调用方退回规则
        let Ok(raw) = complete_with_timeout(self.backend.clone(), p, self.timeout) else {
            // 只说一次，之后不再问，免得每一步都白等一轮超时
            if !self.down.swap(true, Ordering::SeqCst) {
                println!("大模型没回应，后面只用规则。");
            }
            return None;
        };
        let (choice, reason) = parse_reply(&raw, i.candidates.len());
        Some(Advice { choice, reason, raw, model: self.model.clone() })
    }

    fn available(&self) -> bool {
        !self.down.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Backend, LlmError, Prompt};
    use dct_game::ask::{Advisor, AskInput};
    use std::sync::{Arc, Mutex};

    struct Fixed(Result<String, LlmError>, Mutex<Vec<String>>);
    impl Backend for Fixed {
        fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
            self.1.lock().unwrap().push(p.user.clone());
            self.0.clone()
        }
    }
    fn input() -> AskInput {
        AskInput { board: "A A\nB B".into(), goal: "清冰".into(), candidates: vec!["甲".into(), "乙".into(), "丙".into()], failed: vec![] }
    }
    fn adv(r: Result<String, LlmError>) -> (LlmAdvisor, Arc<Fixed>) {
        let b = Arc::new(Fixed(r, Mutex::new(vec![])));
        (LlmAdvisor::new(b.clone(), "m1".into()), b)
    }

    #[test]
    fn a_valid_reply_becomes_a_zero_based_choice_with_reason() {
        let (a, b) = adv(Ok("2，能清冰".into()));
        let r = a.pick(&input()).unwrap();
        assert_eq!(r.choice, Some(1));
        assert_eq!(r.reason, "能清冰");
        assert_eq!(r.raw, "2，能清冰");
        assert_eq!(r.model, "m1");
        assert!(b.1.lock().unwrap()[0].contains("1) 甲"));
    }

    #[test]
    fn garbage_reply_is_an_answer_with_no_choice() {
        let (a, _) = adv(Ok("随便".into()));
        assert_eq!(a.pick(&input()).unwrap().choice, None);
    }

    #[test]
    fn after_one_backend_error_the_advisor_is_down_and_never_calls_again() {
        let (a, b) = adv(Err(LlmError::Unavailable));
        assert!(a.available());
        assert!(a.pick(&input()).is_none());
        assert!(!a.available());
        assert!(a.pick(&input()).is_none());
        assert_eq!(b.1.lock().unwrap().len(), 1);
    }

    #[test]
    fn every_backend_error_means_not_asked() {
        for e in [LlmError::Unavailable, LlmError::Timeout, LlmError::Malformed] {
            let (a, _) = adv(Err(e));
            assert!(a.pick(&input()).is_none(), "{e:?}");
        }
    }
}
