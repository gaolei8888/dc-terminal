//! judge：给一段内容和几个选项，答出每个选项的概率（Jev 格式）。
//!
//! 第一版只有 `choice` 题、只有一个后端：HTTP 调 Jev 兼容服务（`jeff-serve`）。
//! `[judge]` 不写 = 关。`answer_confidence` 由 dct 自己从 `probabilities` 重算，
//! 不信服务端的 `confidence`。

pub mod jev;

use crate::config::JudgeConfig;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 20;
pub const MAX_STATE_CHARS: usize = 50_000;

#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceQuestion {
    pub instructions: String,
    /// 有序：key → 说明。
    pub criteria: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceAnswer {
    pub choice: String,
    pub probabilities: Vec<(String, f64)>,
    pub answer_confidence: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JudgeError {
    /// 请求本身写错了（命令退出码 2）。
    BadRequest(String),
    /// 后端连不上 / 报错（退出码 1）。
    Unavailable(String),
    /// 后端忙，重试过一次还忙。
    Busy,
    /// 后端回的东西读不懂。绝不猜。
    Malformed(String),
    Timeout,
}

impl std::fmt::Display for JudgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JudgeError::BadRequest(m) => write!(f, "请求有误：{m}"),
            JudgeError::Unavailable(m) => write!(f, "judge 后端不可用：{m}"),
            JudgeError::Busy => write!(f, "judge 后端忙（重试一次后仍忙）"),
            JudgeError::Malformed(m) => write!(f, "judge 后端的回复读不懂：{m}"),
            JudgeError::Timeout => write!(f, "judge 后端超时"),
        }
    }
}

pub trait JudgeBackend: Send + Sync {
    fn choice(&self, state: &str, q: &ChoiceQuestion) -> Result<ChoiceAnswer, JudgeError>;
}

/// 2..=20 个选项、key 不重复（也不能为空）、state ≤ 50 000 字。
pub fn validate(state: &str, q: &ChoiceQuestion) -> Result<(), JudgeError> {
    let n = q.criteria.len();
    if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&n) {
        return Err(JudgeError::BadRequest(format!(
            "选项要 {MIN_OPTIONS} 到 {MAX_OPTIONS} 个，现在是 {n} 个"
        )));
    }
    if state.chars().count() > MAX_STATE_CHARS {
        return Err(JudgeError::BadRequest(format!(
            "state 不能超过 {MAX_STATE_CHARS} 字"
        )));
    }
    let mut seen = std::collections::HashSet::new();
    for (k, _) in &q.criteria {
        if k.is_empty() {
            return Err(JudgeError::BadRequest("选项名不能为空".into()));
        }
        if !seen.insert(k.as_str()) {
            return Err(JudgeError::BadRequest(format!("选项名重复：{k}")));
        }
    }
    Ok(())
}

pub fn from_config(c: &JudgeConfig) -> Result<Arc<dyn JudgeBackend>, JudgeError> {
    if c.backend != "jev" {
        return Err(JudgeError::BadRequest(
            "judge 现在只支持 backend = \"jev\"".into(),
        ));
    }
    if crate::llm::creds::host_of(&c.base_url).is_none() {
        return Err(JudgeError::BadRequest(format!(
            "[judge] base_url 不是一个地址：{}（要写成 http://127.0.0.1:8765 这样）",
            c.base_url
        )));
    }
    let host = crate::llm::creds::host_of(&c.base_url).unwrap_or_default();
    // 密钥只从环境变量读，只随请求发往 base_url 那台主机。
    let key = c
        .api_key_env
        .as_deref()
        .and_then(|v| std::env::var(v).ok())
        .filter(|k| !k.is_empty());
    check_key_transport(&c.base_url, &host, key.is_some())?;
    Ok(Arc::new(jev::JevHttp::new(
        &c.base_url,
        c.model.clone(),
        key,
        Duration::from_millis(c.timeout_ms),
    )))
}

/// 有密钥时，明文 `http://` 只能发往本机；别的主机必须 https。
fn check_key_transport(base_url: &str, host: &str, has_key: bool) -> Result<(), JudgeError> {
    if !has_key || !base_url.trim().to_ascii_lowercase().starts_with("http://") {
        return Ok(());
    }
    let h = host.trim_start_matches('[').trim_end_matches(']');
    if matches!(h, "127.0.0.1" | "localhost" | "::1") {
        return Ok(());
    }
    Err(JudgeError::BadRequest(format!(
        "[judge] 配了 api_key_env，但 base_url 是明文 http://{host}：密钥不能明文发到别的主机，请改用 https，或指向本机（127.0.0.1 / localhost）"
    )))
}

const NO_CONFIG_HINT: &str = "judge 要先在 ~/.dct/config.toml 里写 [judge] base_url = \"http://127.0.0.1:8765\"（jeff-serve 的地址）";

const USAGE: &str = "用法：dct judge [文件]    从文件或标准输入读一个 Jev 请求
      dct judge --probe  发一道固定的题，看后端通不通";

fn answer_json(a: &ChoiceAnswer) -> Value {
    let probs: Map<String, Value> = a
        .probabilities
        .iter()
        .map(|(k, p)| (k.clone(), json!(p)))
        .collect();
    json!({
        "type": "choice",
        "choice": a.choice,
        "probabilities": probs,
        "answer_confidence": a.answer_confidence,
    })
}

/// 保持文件里写的顺序的 JSON 对象。
struct Ordered<T>(Vec<(String, T)>);

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for Ordered<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(std::marker::PhantomData<T>);
        impl<'de, T: serde::Deserialize<'de>> serde::de::Visitor<'de> for V<T> {
            type Value = Ordered<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut m: A,
            ) -> Result<Ordered<T>, A::Error> {
                let mut out = Vec::new();
                while let Some(kv) = m.next_entry::<String, T>()? {
                    out.push(kv);
                }
                Ok(Ordered(out))
            }
        }
        d.deserialize_map(V(std::marker::PhantomData))
    }
}

#[derive(serde::Deserialize)]
struct OrderedQuestion {
    #[serde(default)]
    criteria: Option<Ordered<Value>>,
}

#[derive(serde::Deserialize)]
struct OrderedRequest {
    #[serde(default)]
    questions: Option<Ordered<OrderedQuestion>>,
}

fn parse_question(
    name: &str,
    v: &Value,
    order: Option<&Ordered<Value>>,
) -> Result<ChoiceQuestion, (i32, String)> {
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "choice" => {}
        "score" | "noul" => {
            return Err((2, format!("问题 {name}：这一版只做 choice，不做 {ty}")));
        }
        other => return Err((2, format!("问题 {name}：不认识的类型 {other:?}"))),
    }
    let instructions = v
        .get("instructions")
        .and_then(Value::as_str)
        .ok_or_else(|| (2, format!("问题 {name}：缺 instructions")))?
        .to_string();
    let crit_map = v
        .get("criteria")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            (
                2,
                format!("问题 {name}：choice 的 criteria 要是 {{选项名: 说明}}"),
            )
        })?;
    // 按文件里写的顺序；有序解析拿不到（不会发生）才退回 Map 的顺序。
    let crit: Vec<(&String, &Value)> = match order {
        Some(o) if o.0.len() == crit_map.len() => o.0.iter().map(|(k, v)| (k, v)).collect(),
        _ => crit_map.iter().collect(),
    };
    let mut criteria = Vec::new();
    for (k, d) in crit {
        let desc = match d {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            _ => return Err((2, format!("问题 {name}：选项 {k} 的说明要是字符串或 null"))),
        };
        criteria.push((k.clone(), desc));
    }
    Ok(ChoiceQuestion {
        instructions,
        criteria,
    })
}

/// 处理一个 Jev 请求，返回 (退出码, 要打到 stdout 的 JSON)。可注入后端，测试用。
pub fn handle_request(backend: &dyn JudgeBackend, input: &str) -> (i32, Result<Value, String>) {
    let req: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => return (2, Err(format!("请求不是合法 JSON：{e}"))),
    };
    let state = match req.get("state") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => return (2, Err("缺 state".into())),
        Some(other) => other.to_string(),
    };
    let Some(qs) = req.get("questions").and_then(Value::as_object) else {
        return (2, Err("缺 questions".into()));
    };
    if qs.is_empty() {
        return (2, Err("questions 是空的".into()));
    }
    let ordered: Option<OrderedRequest> = serde_json::from_str(input).ok();
    let ordered_qs = ordered.and_then(|o| o.questions);
    let mut parsed = Vec::new();
    for (name, v) in qs {
        let order = ordered_qs
            .as_ref()
            .and_then(|o| o.0.iter().find(|(n, _)| n == name))
            .and_then(|(_, q)| q.criteria.as_ref());
        match parse_question(name, v, order) {
            Ok(q) => parsed.push((name.clone(), q)),
            Err((code, m)) => return (code, Err(m)),
        }
    }
    let mut answers = Map::new();
    for (name, q) in &parsed {
        match backend.choice(&state, q) {
            Ok(a) => {
                answers.insert(name.clone(), answer_json(&a));
            }
            Err(e) => {
                let code = if matches!(e, JudgeError::BadRequest(_)) {
                    2
                } else {
                    1
                };
                return (code, Err(format!("问题 {name}：{e}")));
            }
        }
    }
    (0, Ok(json!({ "answers": answers })))
}

fn probe_input() -> String {
    json!({
        "state": "目标：发帖。画面上有一个输入框和几个按钮。",
        "questions": {"q": {"type": "choice", "instructions": "下一步点哪个？",
            "criteria": {"e1": "按钮 Post", "e2": "按钮 Settings", "scroll_down": "往下滚"}}}
    })
    .to_string()
}

pub fn run_cli(args: &[String]) -> i32 {
    let probe = args.iter().any(|a| a == "--probe");
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if args.iter().any(|a| a.starts_with("--") && a != "--probe") || files.len() > 1 {
        eprintln!("{USAGE}");
        return 2;
    }
    let socket = crate::proto::socket_path();
    let cfg = crate::config::Config::load(&crate::config::config_path_for_socket(&socket));
    let Some(jc) = cfg.judge else {
        eprintln!("{NO_CONFIG_HINT}");
        return 1;
    };
    let backend = match from_config(&jc) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let input = if probe {
        probe_input()
    } else if let Some(f) = files.first() {
        match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("读不了 {f}：{e}");
                return 2;
            }
        }
    } else {
        let mut s = String::new();
        if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut s) {
            eprintln!("读标准输入失败：{e}");
            return 2;
        }
        s
    };
    let t = Instant::now();
    let (code, out) = handle_request(backend.as_ref(), &input);
    match out {
        Ok(v) => {
            if probe {
                eprintln!("probe 用时 {} ms", t.elapsed().as_millis());
            }
            println!("{v}");
        }
        Err(m) => eprintln!("{m}"),
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(n: usize) -> ChoiceQuestion {
        ChoiceQuestion {
            instructions: "i".into(),
            criteria: (0..n).map(|i| (format!("o{i}"), None)).collect(),
        }
    }

    #[test]
    fn two_to_twenty_options() {
        assert!(validate("s", &q(1)).is_err());
        assert!(validate("s", &q(2)).is_ok());
        assert!(validate("s", &q(20)).is_ok());
        assert!(validate("s", &q(21)).is_err());
    }

    #[test]
    fn duplicate_keys_and_huge_state_are_refused() {
        let mut d = q(3);
        d.criteria[2].0 = "o0".into();
        assert!(validate("s", &d).is_err());
        assert!(validate(&"字".repeat(50_001), &q(3)).is_err());
    }

    #[test]
    fn unknown_backend_is_a_config_error() {
        let c = crate::config::JudgeConfig {
            backend: "onnx".into(),
            base_url: "http://127.0.0.1:1".into(),
            model: "m".into(),
            api_key_env: None,
            threshold: 0.8,
            timeout_ms: 5000,
        };
        assert!(from_config(&c).is_err());
    }

    #[test]
    fn base_url_without_a_host_is_refused() {
        let c = crate::config::JudgeConfig {
            backend: "jev".into(),
            base_url: "nonsense".into(),
            model: "m".into(),
            api_key_env: None,
            threshold: 0.8,
            timeout_ms: 5000,
        };
        assert!(from_config(&c).is_err());
    }

    #[test]
    fn key_is_refused_over_plain_http_to_a_remote_host() {
        std::env::set_var("DCT_JUDGE_TEST_KEY_D3", "secret");
        let mk = |url: &str| crate::config::JudgeConfig {
            backend: "jev".into(),
            base_url: url.into(),
            model: "m".into(),
            api_key_env: Some("DCT_JUDGE_TEST_KEY_D3".into()),
            threshold: 0.8,
            timeout_ms: 5000,
        };
        assert!(matches!(
            from_config(&mk("http://example.com:8765")),
            Err(JudgeError::BadRequest(_))
        ));
        assert!(from_config(&mk("https://example.com")).is_ok());
        assert!(from_config(&mk("http://127.0.0.1:8765")).is_ok());
        assert!(from_config(&mk("http://localhost:8765")).is_ok());
        assert!(from_config(&mk("http://[::1]:8765")).is_ok());
        // 没有密钥时明文远程不受限
        let mut c = mk("http://example.com");
        c.api_key_env = Some("DCT_JUDGE_TEST_KEY_D3_UNSET".into());
        assert!(from_config(&c).is_ok());
        std::env::remove_var("DCT_JUDGE_TEST_KEY_D3");
    }

    struct Recorder(std::sync::Mutex<Vec<Vec<String>>>);
    impl JudgeBackend for Recorder {
        fn choice(&self, _: &str, q: &ChoiceQuestion) -> Result<ChoiceAnswer, JudgeError> {
            self.0
                .lock()
                .unwrap()
                .push(q.criteria.iter().map(|(k, _)| k.clone()).collect());
            Ok(ChoiceAnswer {
                choice: q.criteria[0].0.clone(),
                probabilities: vec![],
                answer_confidence: 1.0,
            })
        }
    }

    #[test]
    fn file_option_order_is_kept() {
        let r = Recorder(Default::default());
        let input = r#"{"state":"s","questions":{"q":{"type":"choice","instructions":"x","criteria":{"e10":null,"e2":"a","b":null,"stuck":null}}}}"#;
        assert_eq!(handle_request(&r, input).0, 0);
        assert_eq!(r.0.lock().unwrap()[0], ["e10", "e2", "b", "stuck"]);
    }

    struct Fixed;
    impl JudgeBackend for Fixed {
        fn choice(&self, _: &str, q: &ChoiceQuestion) -> Result<ChoiceAnswer, JudgeError> {
            validate("s", q)?;
            Ok(ChoiceAnswer {
                choice: q.criteria[0].0.clone(),
                probabilities: vec![(q.criteria[0].0.clone(), 1.0)],
                answer_confidence: 1.0,
            })
        }
    }

    #[test]
    fn handle_request_answers_choice_and_rejects_other_types_with_2() {
        let ok = r#"{"state":"s","questions":{"a":{"type":"choice","instructions":"x","criteria":{"p":"1","q":null}}}}"#;
        let (code, out) = handle_request(&Fixed, ok);
        assert_eq!(code, 0);
        let v = out.unwrap();
        assert_eq!(v["answers"]["a"]["type"], "choice");
        assert_eq!(v["answers"]["a"]["choice"], "p");
        assert_eq!(v["answers"]["a"]["answer_confidence"], 1.0);

        let score = r#"{"state":"s","questions":{"a":{"type":"score","instructions":"x","criteria":["a","b"]}}}"#;
        let (code, out) = handle_request(&Fixed, score);
        assert_eq!(code, 2);
        assert!(out.unwrap_err().contains("只做 choice"));

        assert_eq!(handle_request(&Fixed, "not json").0, 2);
        assert_eq!(handle_request(&Fixed, r#"{"questions":{}}"#).0, 2);
        // 选项太少：后端校验出 BadRequest，退出码 2
        let few = r#"{"state":"s","questions":{"a":{"type":"choice","instructions":"x","criteria":{"p":null}}}}"#;
        assert_eq!(handle_request(&Fixed, few).0, 2);
    }
}
