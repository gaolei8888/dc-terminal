//! `jeff-serve`（以及任何说 Jev 格式的服务）的 HTTP 后端：`POST {base}/v1/systemone`。
//!
//! 与 Jev 原格式的三处差别：请求必带 `model`；第一版不发图；回复里没有可信的
//! `answer_confidence`，dct 自己从 `probabilities` 归一化后重算。

use super::{validate, ChoiceAnswer, ChoiceQuestion, JudgeBackend, JudgeError};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// (url, Bearer 密钥, 请求体) → (状态码, 响应体)；Err 是网络层错误文字。
/// 请求体是**已经序列化好的 JSON 文本**：`criteria` 的选项顺序要原样送到服务端
/// （Jeff 按位置读选项，并列时也按顺序取），不能经过会排序的 `Map`。
pub type Sender = dyn Fn(&str, Option<&str>, &str) -> Result<(u16, String), String> + Send + Sync;

pub struct JevHttp {
    url: String,
    model: String,
    key: Option<String>,
    send: Arc<Sender>,
    retry_wait: Duration,
}

impl JevHttp {
    pub fn new(base: &str, model: String, key: Option<String>, timeout: Duration) -> JevHttp {
        // 与 llm/http.rs 一样：agent 只从 agent_builder() 出来，两个超时都设。
        let agent = crate::sys::tls::agent_builder()
            .timeout(timeout)
            .timeout_connect(Duration::from_secs(2))
            .build();
        let send: Arc<Sender> = Arc::new(move |url, key, body| {
            let mut req = agent.post(url).set("Content-Type", "application/json");
            if let Some(k) = key {
                req = req.set("Authorization", &format!("Bearer {k}"));
            }
            match req.send_string(body) {
                Ok(r) => Ok((r.status(), r.into_string().map_err(|e| e.to_string())?)),
                Err(ureq::Error::Status(code, r)) => {
                    Ok((code, r.into_string().unwrap_or_default()))
                }
                Err(e) => Err(e.to_string()),
            }
        });
        JevHttp::with_sender(base.to_string(), model, key, send)
    }

    /// `base` 可以带也可以不带 `/v1/systemone`。
    pub fn with_sender(
        base: String,
        model: String,
        key: Option<String>,
        send: Arc<Sender>,
    ) -> JevHttp {
        let trimmed = base.trim_end_matches('/');
        let url = if trimmed.ends_with("/v1/systemone") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/v1/systemone")
        };
        JevHttp {
            url,
            model,
            key,
            send,
            retry_wait: Duration::from_secs(1),
        }
    }

    pub fn with_retry_wait(mut self, d: Duration) -> JevHttp {
        self.retry_wait = d;
        self
    }
}

impl JudgeBackend for JevHttp {
    fn choice(&self, state: &str, q: &ChoiceQuestion) -> Result<ChoiceAnswer, JudgeError> {
        validate(state, q)?;
        let body = request_body(&self.model, state, q);
        for attempt in 0..2 {
            let (code, text) = (self.send)(&self.url, self.key.as_deref(), &body).map_err(|e| {
                if e.to_lowercase().contains("timed out") {
                    JudgeError::Timeout
                } else {
                    JudgeError::Unavailable(e)
                }
            })?;
            match code {
                200 => return read_choice(&text, q),
                529 if attempt == 0 => std::thread::sleep(self.retry_wait),
                529 => return Err(JudgeError::Busy),
                422 => return Err(JudgeError::BadRequest(text)),
                _ => return Err(JudgeError::Unavailable(format!("HTTP {code}"))),
            }
        }
        Err(JudgeError::Busy)
    }
}

/// 手工拼 JSON 文本，`criteria` 的键按 `q.criteria` 的顺序写出。
fn request_body(model: &str, state: &str, q: &ChoiceQuestion) -> String {
    let js = |s: &str| serde_json::to_string(s).expect("string always serializes");
    let criteria = q
        .criteria
        .iter()
        .map(|(k, d)| {
            let v = d.as_deref().map(js).unwrap_or_else(|| "null".to_string());
            format!("{}:{}", js(k), v)
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"model\":{},\"state\":{},\"questions\":{{\"q\":{{\"type\":\"choice\",\"instructions\":{},\"criteria\":{{{}}}}}}}}}",
        js(model),
        js(state),
        js(&q.instructions),
        criteria
    )
}

/// 只认 `answers.q.probabilities`；每个选项都要有、不能多、有限且非负、总和 > 0。
/// 归一化后取最大（并列取先出现的选项）。不信回复里的 `choice`。
fn read_choice(text: &str, q: &ChoiceQuestion) -> Result<ChoiceAnswer, JudgeError> {
    let bad = |m: &str| JudgeError::Malformed(m.to_string());
    let v: Value = serde_json::from_str(text).map_err(|e| bad(&format!("不是 JSON：{e}")))?;
    let probs = v
        .pointer("/answers/q/probabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| bad("缺 answers.q.probabilities"))?;
    if probs.len() != q.criteria.len() {
        return Err(bad("概率的选项数和题目不一致"));
    }
    let mut raw = Vec::with_capacity(q.criteria.len());
    for (k, _) in &q.criteria {
        let p = probs
            .get(k)
            .and_then(Value::as_f64)
            .ok_or_else(|| bad(&format!("缺选项 {k} 的概率")))?;
        if !p.is_finite() || p < 0.0 {
            return Err(bad(&format!("选项 {k} 的概率不合法")));
        }
        raw.push((k.clone(), p));
    }
    let sum: f64 = raw.iter().map(|(_, p)| p).sum();
    if sum <= 0.0 || !sum.is_finite() {
        return Err(bad("概率总和不是正数"));
    }
    let probabilities: Vec<(String, f64)> = raw.into_iter().map(|(k, p)| (k, p / sum)).collect();
    let mut best = 0;
    for (i, (_, p)) in probabilities.iter().enumerate() {
        if *p > probabilities[best].1 {
            best = i;
        }
    }
    Ok(ChoiceAnswer {
        choice: probabilities[best].0.clone(),
        answer_confidence: probabilities[best].1,
        probabilities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge::{ChoiceQuestion, JudgeBackend, JudgeError};
    use std::sync::{Arc, Mutex};

    fn q() -> ChoiceQuestion {
        ChoiceQuestion {
            instructions: "下一步点哪个？".into(),
            criteria: vec![
                ("e12".into(), Some("按钮 Post".into())),
                ("scroll_down".into(), Some("往下滚".into())),
                ("stuck".into(), None),
            ],
        }
    }

    fn backend(reply: (u16, String), seen: Arc<Mutex<Vec<serde_json::Value>>>) -> JevHttp {
        JevHttp::with_sender(
            "http://127.0.0.1:8765".into(),
            "jeff-latest".into(),
            None,
            Arc::new(move |url: &str, _key: Option<&str>, body: &str| {
                assert_eq!(url, "http://127.0.0.1:8765/v1/systemone");
                seen.lock()
                    .unwrap()
                    .push(serde_json::from_str(body).unwrap());
                Ok(reply.clone())
            }),
        )
    }

    #[test]
    fn sends_jev_format_with_model_and_reads_probabilities() {
        let seen = Arc::new(Mutex::new(vec![]));
        let b = backend((200, r#"{"model":"jeff-qwen3.5-0.8b","answers":{"q":{"type":"choice","choice":"e12","probabilities":{"e12":0.9,"scroll_down":0.07,"stuck":0.03},"confidence":0.85}},"usage":{"input_tokens":300,"output_tokens":0}}"#.into()), seen.clone());
        let a = b.choice("目标：发帖", &q()).unwrap();
        assert_eq!(a.choice, "e12");
        assert!(
            (a.answer_confidence - 0.9).abs() < 1e-9,
            "answer_confidence 是被选那项的概率，不是 Jeff 的 confidence"
        );
        let body = &seen.lock().unwrap()[0];
        assert_eq!(body["model"], "jeff-latest");
        assert_eq!(body["state"], "目标：发帖");
        assert_eq!(body["questions"]["q"]["type"], "choice");
        assert_eq!(body["questions"]["q"]["criteria"]["e12"], "按钮 Post");
        assert!(body["questions"]["q"]["criteria"]["stuck"].is_null());
        assert!(body.get("images").is_none(), "第一版不发图");
    }

    #[test]
    fn probabilities_are_renormalised_and_choice_recomputed() {
        let b = backend((200, r#"{"answers":{"q":{"type":"choice","choice":"stuck","probabilities":{"e12":2.0,"scroll_down":1.0,"stuck":1.0}}}}"#.into()), Arc::new(Mutex::new(vec![])));
        let a = b.choice("s", &q()).unwrap();
        assert_eq!(a.choice, "e12", "不信回复里的 choice，按概率最大的");
        assert!((a.answer_confidence - 0.5).abs() < 1e-9);
    }

    #[test]
    fn missing_option_or_unknown_key_is_malformed_never_guessed() {
        for body in [
            r#"{"answers":{"q":{"probabilities":{"e12":1.0}}}}"#,
            r#"{"answers":{"q":{"probabilities":{"e12":0.5,"scroll_down":0.3,"stuck":0.1,"zzz":0.1}}}}"#,
            r#"{}"#,
            r#"{"answers":{"q":{"probabilities":{"e12":-1.0,"scroll_down":1.0,"stuck":1.0}}}}"#,
            r#"{"answers":{"q":{"probabilities":{"e12":0.0,"scroll_down":0.0,"stuck":0.0}}}}"#,
        ] {
            let b = backend((200, body.into()), Arc::new(Mutex::new(vec![])));
            assert!(
                matches!(b.choice("s", &q()), Err(JudgeError::Malformed(_))),
                "{body}"
            );
        }
    }

    #[test]
    fn option_order_reaches_the_wire_unchanged() {
        let texts = Arc::new(Mutex::new(Vec::<String>::new()));
        let t2 = texts.clone();
        let b = JevHttp::with_sender(
            "http://127.0.0.1:8765".into(),
            "m".into(),
            None,
            Arc::new(move |_: &str, _: Option<&str>, body: &str| {
                t2.lock().unwrap().push(body.to_string());
                Ok((503, String::new()))
            }),
        );
        let q = ChoiceQuestion {
            instructions: "i".into(),
            criteria: vec![
                ("e10".into(), None),
                ("e2".into(), Some("x".into())),
                ("a".into(), None),
                ("stuck".into(), None),
            ],
        };
        let _ = b.choice("s", &q);
        let t = texts.lock().unwrap()[0].clone();
        let pos = |k: &str| t.find(&format!("\"{k}\":")).unwrap();
        assert!(
            pos("e10") < pos("e2") && pos("e2") < pos("a") && pos("a") < pos("stuck"),
            "{t}"
        );
        let v: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["questions"]["q"]["criteria"]["e2"], "x");
    }

    #[test]
    fn busy_is_retried_once_then_reported() {
        let n = Arc::new(Mutex::new(0));
        let n2 = n.clone();
        let b = JevHttp::with_sender(
            "http://127.0.0.1:8765".into(),
            "jeff-latest".into(),
            None,
            Arc::new(move |_: &str, _: Option<&str>, _: &str| {
                *n2.lock().unwrap() += 1;
                Ok((529, "busy".into()))
            }),
        )
        .with_retry_wait(std::time::Duration::from_millis(1));
        assert_eq!(b.choice("s", &q()), Err(JudgeError::Busy));
        assert_eq!(*n.lock().unwrap(), 2);
    }

    #[test]
    fn busy_then_ok_succeeds() {
        let n = Arc::new(Mutex::new(0));
        let n2 = n.clone();
        let b = JevHttp::with_sender(
            "http://127.0.0.1:8765".into(),
            "jeff-latest".into(),
            None,
            Arc::new(move |_: &str, _: Option<&str>, _: &str| {
                let mut g = n2.lock().unwrap();
                *g += 1;
                if *g == 1 {
                    Ok((529, "busy".into()))
                } else {
                    Ok((200, r#"{"answers":{"q":{"probabilities":{"e12":1,"scroll_down":0,"stuck":0}}}}"#.into()))
                }
            }),
        )
        .with_retry_wait(std::time::Duration::from_millis(1));
        assert_eq!(b.choice("s", &q()).unwrap().choice, "e12");
    }

    #[test]
    fn http_errors_are_unavailable() {
        let b = backend((503, "not ready".into()), Arc::new(Mutex::new(vec![])));
        assert!(matches!(
            b.choice("s", &q()),
            Err(JudgeError::Unavailable(_))
        ));
    }

    #[test]
    fn api_key_is_passed_to_the_sender_only_when_set() {
        let got = Arc::new(Mutex::new(None::<String>));
        let g2 = got.clone();
        let b = JevHttp::with_sender(
            "http://127.0.0.1:8765/".into(),
            "m".into(),
            Some("k123".into()),
            Arc::new(move |_: &str, key: Option<&str>, _: &str| {
                *g2.lock().unwrap() = key.map(String::from);
                Ok((503, String::new()))
            }),
        );
        let _ = b.choice("s", &q());
        assert_eq!(got.lock().unwrap().as_deref(), Some("k123"));
    }
}
