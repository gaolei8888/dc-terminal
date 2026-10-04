//! 把「问模型选哪一步」接到 dct 自己的 LLM 连接层上。
//! 凭据怎么取、发给哪台机器，全在 `llm::resolve`，这里不碰。
use crate::llm::{complete_counted_with_timeout, Backend, Prompt};
use dct_game::ask::{parse_reply, prompt_text, Advice, Advisor, AskInput};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct LlmAdvisor {
    backend: Arc<dyn Backend>,
    model: String,
    timeout: Duration,
    /// 问不通以后置位：后面不再发问，只用规则。
    down: AtomicBool,
    asks: AtomicU64,
    counted: AtomicU64,
    tokens_in: AtomicU64,
    tokens_out: AtomicU64,
}

impl LlmAdvisor {
    pub fn new(backend: Arc<dyn Backend>, model: String) -> LlmAdvisor {
        LlmAdvisor { backend, model, timeout: Duration::from_secs(20), down: AtomicBool::new(false),
            asks: AtomicU64::new(0),
            counted: AtomicU64::new(0),
            tokens_in: AtomicU64::new(0),
            tokens_out: AtomicU64::new(0),
        }
    }

    /// 结束语：没问过返回 `None`。用量读到几次算几次，读不到的次数明说。
    pub fn summary(&self) -> Option<String> {
        let asks = self.asks.load(Ordering::SeqCst);
        if asks == 0 {
            return None;
        }
        let counted = self.counted.load(Ordering::SeqCst);
        if counted == 0 {
            return Some(format!("这次问了大模型 {asks} 次（没有读到用量）。"));
        }
        let (i, o) = (self.tokens_in.load(Ordering::SeqCst), self.tokens_out.load(Ordering::SeqCst));
        let mut s = format!("这次问了大模型 {asks} 次，一共用了约 {} 个 token（输入 {i}，输出 {o}）", i + o);
        if counted < asks {
            s += &format!("，其中 {} 次没读到用量", asks - counted);
        }
        s += "。";
        Some(s)
    }
}

const SYSTEM: &str = "你在帮一个三消游戏的自动玩家选下一步。只能从给出的编号里选，不要自己编走法。";

impl Advisor for LlmAdvisor {
    fn pick(&self, i: &AskInput) -> Option<Advice> {
        if self.down.load(Ordering::SeqCst) {
            return None;
        }
        let p = Prompt { system: SYSTEM.into(), user: prompt_text(i), max_tokens: 64, image_png_base64: None, image_mime: None, };
        // 任何错误都是「没问成」：调用方退回规则
        let Ok((raw, usage)) = complete_counted_with_timeout(self.backend.clone(), p, self.timeout) else {
            // 只说一次，之后不再问，免得每一步都白等一轮超时
            if !self.down.swap(true, Ordering::SeqCst) {
                println!("大模型没回应，后面只用规则。");
            }
            return None;
        };
        self.asks.fetch_add(1, Ordering::SeqCst);
        let tokens = usage.map(|u| (u.input, u.output));
        if let Some((ti, to)) = tokens {
            self.counted.fetch_add(1, Ordering::SeqCst);
            self.tokens_in.fetch_add(ti, Ordering::SeqCst);
            self.tokens_out.fetch_add(to, Ordering::SeqCst);
        }
        let (choice, reason) = parse_reply(&raw, i.candidates.len());
        Some(Advice { choice, reason, raw, model: self.model.clone(), tokens })
    }

    fn available(&self) -> bool {
        !self.down.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Backend, LlmError, Prompt, Usage};
    use dct_game::ask::{Advisor, AskInput};
    use std::sync::{Arc, Mutex};

    struct Fixed(Result<String, LlmError>, Mutex<Vec<String>>, Option<Usage>);
    impl Backend for Fixed {
        fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
            self.1.lock().unwrap().push(p.user.clone());
            self.0.clone()
        }
        fn complete_counted(&self, p: &Prompt) -> Result<(String, Option<Usage>), LlmError> {
            self.complete(p).map(|s| (s, self.2))
        }
    }
    fn input() -> AskInput {
        AskInput { board: "A A\nB B".into(), goal: "清冰".into(), candidates: vec!["甲".into(), "乙".into(), "丙".into()], failed: vec![] }
    }
    fn adv(r: Result<String, LlmError>) -> (LlmAdvisor, Arc<Fixed>) {
        let b = Arc::new(Fixed(r, Mutex::new(vec![]), None));
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

    #[test]
    fn usage_is_carried_into_the_advice_and_summed_into_the_summary() {
        let b = Arc::new(Fixed(Ok("1".into()), Mutex::new(vec![]), Some(Usage { input: 100, output: 4 })));
        let a = LlmAdvisor::new(b, "m1".into());
        let r = a.pick(&input()).unwrap();
        assert_eq!(r.tokens, Some((100, 4)));
        a.pick(&input()).unwrap();
        assert_eq!(a.summary().as_deref(), Some("这次问了大模型 2 次，一共用了约 208 个 token（输入 200，输出 8）。"));
    }

    #[test]
    fn summary_says_how_many_asks_had_no_usage() {
        let b = Arc::new(Fixed(Ok("1".into()), Mutex::new(vec![]), None));
        let a = LlmAdvisor::new(b, "m1".into());
        a.pick(&input()).unwrap();
        assert_eq!(a.summary().as_deref(), Some("这次问了大模型 1 次（没有读到用量）。"));
    }

    #[test]
    fn summary_mixes_counted_and_uncounted_asks_honestly() {
        struct Alt(Mutex<u32>);
        impl Backend for Alt {
            fn complete(&self, _: &Prompt) -> Result<String, LlmError> {
                Ok("1".into())
            }
            fn complete_counted(&self, _: &Prompt) -> Result<(String, Option<Usage>), LlmError> {
                let mut n = self.0.lock().unwrap();
                *n += 1;
                Ok(("1".into(), if *n == 1 { Some(Usage { input: 50, output: 1 }) } else { None }))
            }
        }
        let a = LlmAdvisor::new(Arc::new(Alt(Mutex::new(0))), "m".into());
        a.pick(&input()).unwrap();
        a.pick(&input()).unwrap();
        assert_eq!(a.summary().as_deref(), Some("这次问了大模型 2 次，一共用了约 51 个 token（输入 50，输出 1），其中 1 次没读到用量。"));
    }

    #[test]
    fn no_asks_means_no_summary() {
        let b = Arc::new(Fixed(Ok("1".into()), Mutex::new(vec![]), None));
        assert_eq!(LlmAdvisor::new(b, "m".into()).summary(), None);
    }

    #[test]
    fn a_failed_ask_is_not_counted_in_the_summary() {
        let (a, _) = adv(Err(LlmError::Unavailable));
        assert!(a.pick(&input()).is_none());
        assert_eq!(a.summary(), None);
    }
}
