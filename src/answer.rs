//! `dct answer --json`：一道选择题，按 Jeff → 大模型 → 用户（不问，只说要问）逐级答。
//!
//! 只是把现有的阶梯（`dct_explore::ladder`）、Jeff 客户端（`judge`）、大模型（`explore_model`）
//! 和 `[judge]` / `[llm]` 配置接成一条标准输入 → 标准输出的 JSON 命令；不新增任何模型调用，
//! 门槛、次数上限、「Jeff 的把握由 dct 从概率重算」都和 `dct do` 里一样。
//! 走到用户这一级时不弹任何东西，回 `{"by":"none","reason":"needs_user"}`（问人的服务是以后的事）。
use crate::explore_cli::{JudgeJeff, NoJeff, DEFAULT_THRESHOLD};
use crate::explore_model::{backend_from_config, LlmModel, DEFAULT_TIMEOUT};
use dct_explore::ladder::{Jeff, Ladder, Model, ModelPick, NoModel, User, UserPick};
use dct_explore::options::{Act, Opt};
use serde::Deserialize;
use serde_json::{json, Map, Value};

/// 标准输入最多读这么多字节（题面上限 5 万字，加上 20 个选项绰绰有余）。
pub const MAX_INPUT_BYTES: usize = 1 << 20;
pub const MAX_OPTIONS: usize = crate::judge::MAX_OPTIONS;
const MAX_KEY_CHARS: usize = 64;
const MAX_TEXT_CHARS: usize = 2_000;
const MAX_PICTURE_CHARS: usize = 4_096;
const MAX_FORBID: usize = 100;
const MAX_FORBID_CHARS: usize = 200;
/// 模型把这个 key 当「说不准」（见 `explore_model` 的提示词），不能同时是一个真选项。
const RESERVED_KEY: &str = "stuck";

const USAGE: &str = "用法：dct answer --json    从标准输入读一个请求（一个 JSON），向标准输出写一行 JSON 答案
请求：{\"state\": \"…\", \"options\": [{\"key\": \"a\", \"text\": \"…\"}], \"picture\": \"路径(可选)\",
      \"allow\": [\"jeff\",\"llm\",\"user\"] (默认全部), \"forbid\": [\"不许发给大模型的字\"] (可选)}
答案：{\"key\": \"a\"|null, \"by\": \"jeff\"|\"model\"|\"none\", \"confidence\": 数|null, \"reason\": \"…\"(可选)}
走到用户这一级时不弹任何东西，回 by=none、reason=needs_user。
退出码：0 有答案或要用户；1 配置读不了 / 写不出；2 请求不对";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Rung {
    Jeff,
    Llm,
    User,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOption {
    key: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequest {
    state: String,
    options: Vec<RawOption>,
    picture: Option<String>,
    allow: Option<Vec<Rung>>,
    forbid: Option<Vec<String>>,
}

struct Request {
    state: String,
    options: Vec<Opt>,
    picture: Option<String>,
    allow: Vec<Rung>,
    forbid: Vec<String>,
}

/// 同 `gate::fold` 的规整（全角、零宽、弯引号、小写），不另写一份。
fn fold(s: &str) -> String {
    dct_brain::tier::normalize(s)
}

/// 任一禁词出现在任一段文字里（规整后比，不分大小写）。
pub fn hits_forbid<'a>(forbid: &[String], texts: impl IntoIterator<Item = &'a str>) -> bool {
    let words: Vec<String> = forbid.iter().map(|w| fold(w)).collect();
    texts.into_iter().any(|t| {
        let f = fold(t);
        words.iter().any(|w| f.contains(w.as_str()))
    })
}

/// 禁词列表本身的检查，`dct answer` 和 `dct train record` 共用。
pub fn check_forbid(forbid: &[String]) -> Result<(), String> {
    if forbid.len() > MAX_FORBID {
        return Err(format!("forbid 最多 {MAX_FORBID} 条"));
    }
    for w in forbid {
        if w.trim().is_empty() {
            return Err("forbid 里不能有空字符串（会命中一切）".into());
        }
        if w.chars().count() > MAX_FORBID_CHARS {
            return Err(format!("forbid 的每条最多 {MAX_FORBID_CHARS} 个字"));
        }
    }
    Ok(())
}

fn parse(input: &str) -> Result<Request, String> {
    let raw: RawRequest =
        serde_json::from_str(input).map_err(|e| format!("请求不是合法的 JSON 或字段不对：{e}"))?;
    if raw.state.chars().count() > crate::judge::MAX_STATE_CHARS {
        return Err(format!(
            "state 不能超过 {} 字",
            crate::judge::MAX_STATE_CHARS
        ));
    }
    let n = raw.options.len();
    if !(1..=MAX_OPTIONS).contains(&n) {
        return Err(format!("options 要 1 到 {MAX_OPTIONS} 个，现在是 {n} 个"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut options = Vec::with_capacity(n);
    for o in raw.options {
        if o.key.trim().is_empty() || o.key.chars().count() > MAX_KEY_CHARS {
            return Err(format!("option 的 key 不能为空、最多 {MAX_KEY_CHARS} 个字"));
        }
        if o.key == RESERVED_KEY {
            return Err(format!(
                "option 的 key 不能叫 {RESERVED_KEY}（模型用它表示说不准）"
            ));
        }
        if !seen.insert(o.key.clone()) {
            return Err(format!("option 的 key 重复：{}", o.key));
        }
        if o.text.chars().count() > MAX_TEXT_CHARS {
            return Err(format!(
                "option {} 的 text 最多 {MAX_TEXT_CHARS} 个字",
                o.key
            ));
        }
        options.push(Opt {
            key: o.key,
            text: o.text,
            // 题只关心 key；动作一栏填一个不是「卡住」也不是图标的占位，阶梯就当它直接可用。
            act: Act::Wait,
            publish: false,
        });
    }
    if raw
        .picture
        .as_ref()
        .is_some_and(|p| p.chars().count() > MAX_PICTURE_CHARS)
    {
        return Err(format!("picture 路径最多 {MAX_PICTURE_CHARS} 个字"));
    }
    let forbid = raw.forbid.unwrap_or_default();
    check_forbid(&forbid)?;
    let mut allow = raw
        .allow
        .unwrap_or_else(|| vec![Rung::Jeff, Rung::Llm, Rung::User]);
    allow.dedup();
    Ok(Request {
        state: raw.state,
        options,
        picture: raw.picture,
        allow,
        forbid,
    })
}

/// 走到用户这一级：不问，只记下「要问人」，让阶梯收工。
struct NeedsUser;
impl User for NeedsUser {
    fn pick(&mut self, _: &str, _: &[Opt], _: Option<&str>, _: &str) -> Option<UserPick> {
        None
    }
}

/// 不管阶梯传什么图，都不交给大模型（它本来就只收文字；这里再用结构保证一次）。
struct TextOnly<'a>(&'a dyn Model);
impl Model for TextOnly<'_> {
    fn pick(&self, state: &str, all: &[Opt], _picture: Option<&str>) -> Option<ModelPick> {
        self.0.pick(state, all, None)
    }
}

fn run_ladder(req: &Request, jeff: &dyn Jeff, model: &dyn Model, threshold: f64) -> Value {
    let has = |r: Rung| req.allow.contains(&r);
    // 禁词命中：不发给大模型（它可能在别人的机器上）。Jeff 跑在本机，照问。
    let withheld = has(Rung::Llm)
        && hits_forbid(
            &req.forbid,
            std::iter::once(req.state.as_str()).chain(req.options.iter().map(|o| o.text.as_str())),
        );
    let text_only = TextOnly(model);
    let model: &dyn Model = if has(Rung::Llm) && !withheld {
        &text_only
    } else {
        &NoModel
    };
    let jeff: &dyn Jeff = if has(Rung::Jeff) { jeff } else { &NoJeff };
    let mut user = NeedsUser;
    let decided = Ladder {
        jeff,
        model,
        user: &mut user,
        threshold,
    }
    .decide(
        &req.state,
        &req.options,
        &req.options,
        req.picture.as_deref(),
        &Default::default(),
    );
    let mut out = Map::new();
    let mut reason: Option<String> = None;
    match decided {
        Some(d) => {
            use dct_explore::ladder::Who;
            let (by, conf) = match d.by {
                Who::Jeff => ("jeff", d.jeff.as_ref().map(|(_, c)| *c)),
                Who::Model => ("model", None),
                Who::User => ("user", None),
            };
            out.insert("key".into(), json!(d.key));
            if let Some(n) = d.name {
                out.insert("name".into(), json!(n));
            }
            out.insert("by".into(), json!(by));
            out.insert("confidence".into(), json!(conf));
        }
        None => {
            out.insert("key".into(), Value::Null);
            out.insert("by".into(), json!("none"));
            out.insert("confidence".into(), Value::Null);
            reason = Some(
                if has(Rung::User) {
                    "needs_user"
                } else {
                    "no_answer"
                }
                .into(),
            );
        }
    }
    if withheld {
        let note = "llm_withheld: forbidden string in the request";
        reason = Some(match reason {
            Some(r) => format!("{r}; {note}"),
            None => note.into(),
        });
    }
    if let Some(r) = reason {
        out.insert("reason".into(), json!(r));
    }
    Value::Object(out)
}

/// 处理一个请求，返回 (退出码, 要打到 stdout 的 JSON)。可注入 Jeff / 大模型，测试用。
/// 后端的失败（连不上、读不懂、超时）在 `Jeff` / `Model` 里已经变成 `None`，这里只会逐级往上，不会 panic。
pub fn handle(
    input: &str,
    jeff: &dyn Jeff,
    model: &dyn Model,
    threshold: f64,
) -> (i32, Result<Value, String>) {
    match parse(input) {
        Ok(req) => (0, Ok(run_ladder(&req, jeff, model, threshold))),
        Err(m) => (2, Err(m)),
    }
}

/// 从标准输入读至多 `MAX_INPUT_BYTES` 字节；多了算请求不对（2），读不了算 1。
pub fn read_stdin_limited() -> Result<String, (i32, String)> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| (1, format!("读标准输入失败：{e}")))?;
    if buf.len() > MAX_INPUT_BYTES {
        return Err((2, format!("请求超过 {MAX_INPUT_BYTES} 字节")));
    }
    String::from_utf8(buf).map_err(|_| (2, "请求不是 UTF-8".into()))
}

pub fn run_cli(args: &[String]) -> i32 {
    if args != ["--json"] {
        eprintln!("{USAGE}");
        return 2;
    }
    let input = match read_stdin_limited() {
        Ok(s) => s,
        Err((code, m)) => {
            eprintln!("{m}");
            return code;
        }
    };
    let socket = crate::proto::socket_path();
    // 配置坏了要说出来（和 `dct do` 一样），不能悄悄当没配：那会让「该走本机 Jeff」的人白白落到用户那一级。
    let cfg = match crate::explore_cli::load_config(&crate::config::config_path_for_socket(&socket))
    {
        Ok(c) => c,
        Err(m) => {
            eprintln!("{m}");
            return 1;
        }
    };
    // 每一级都有退路：没配 / 用不了就跳过，用户（这里是「要用户」）永远是最后一级。
    let (jeff, threshold): (Box<dyn Jeff>, f64) = match &cfg.judge {
        Some(jc) => match crate::judge::from_config(jc) {
            Ok(b) => (Box::new(JudgeJeff(b)), jc.threshold),
            Err(e) => {
                eprintln!("[judge] 用不了（{e}），这次不问 Jeff");
                (Box::new(NoJeff), jc.threshold)
            }
        },
        None => (Box::new(NoJeff), DEFAULT_THRESHOLD),
    };
    let model: Box<dyn Model> = match cfg.llm.as_ref().map(|l| backend_from_config(l, &socket)) {
        Some(Ok(b)) => Box::new(LlmModel::new(b, DEFAULT_TIMEOUT)),
        Some(Err(e)) => {
            eprintln!("[llm] 连不上（{e:?}），这次不问大模型");
            Box::new(NoModel)
        }
        None => Box::new(NoModel),
    };
    let (code, out) = handle(&input, jeff.as_ref(), model.as_ref(), threshold);
    match out {
        Ok(v) => println!("{v}"),
        Err(m) => eprintln!("{m}"),
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use dct_explore::ladder::{JeffAnswer, ModelPick};
    use dct_explore::options::Opt;
    use std::cell::RefCell;

    struct FakeJeff(Option<(&'static str, f64)>, RefCell<u32>);
    impl Jeff for FakeJeff {
        fn ask(&self, _: &str, _: &dct_explore::JevChoice) -> Option<JeffAnswer> {
            *self.1.borrow_mut() += 1;
            self.0.map(|(k, c)| JeffAnswer {
                key: k.into(),
                probabilities: vec![(k.into(), c)],
                answer_confidence: c,
            })
        }
    }
    struct FakeModel(
        Option<&'static str>,
        RefCell<u32>,
        RefCell<Vec<Option<String>>>,
    );
    impl Model for FakeModel {
        fn pick(&self, _: &str, _: &[Opt], picture: Option<&str>) -> Option<ModelPick> {
            *self.1.borrow_mut() += 1;
            self.2.borrow_mut().push(picture.map(String::from));
            self.0.map(|k| ModelPick {
                key: k.into(),
                name: None,
            })
        }
    }
    fn jeff(a: Option<(&'static str, f64)>) -> FakeJeff {
        FakeJeff(a, RefCell::new(0))
    }
    fn model(a: Option<&'static str>) -> FakeModel {
        FakeModel(a, RefCell::new(0), RefCell::new(vec![]))
    }

    fn req(extra: &str) -> String {
        format!(
            r#"{{"state":"pick one","options":[{{"key":"a","text":"Alpha"}},{{"key":"b","text":"Beta"}}]{extra}}}"#
        )
    }
    fn run(input: &str, j: &FakeJeff, m: &FakeModel) -> (i32, Result<Value, String>) {
        handle(input, j, m, 0.8)
    }
    fn ok(r: (i32, Result<Value, String>)) -> Value {
        assert_eq!(r.0, 0, "{:?}", r.1);
        r.1.unwrap()
    }

    #[test]
    fn a_confident_jeff_answers_and_the_model_is_not_asked() {
        let (j, m) = (jeff(Some(("b", 0.93))), model(Some("a")));
        let v = ok(run(&req(""), &j, &m));
        assert_eq!(v["key"], "b");
        assert_eq!(v["by"], "jeff");
        assert_eq!(v["confidence"], 0.93);
        assert!(v.get("name").is_none());
        assert_eq!(*m.1.borrow(), 0);
    }

    #[test]
    fn jeff_below_the_threshold_goes_to_the_model() {
        let (j, m) = (jeff(Some(("b", 0.5))), model(Some("a")));
        let v = ok(run(&req(""), &j, &m));
        assert_eq!(
            (v["key"].as_str(), v["by"].as_str()),
            (Some("a"), Some("model"))
        );
        assert!(v["confidence"].is_null());
    }

    #[test]
    fn nothing_answering_means_needs_user_and_exit_zero() {
        let (j, m) = (jeff(None), model(None));
        let v = ok(run(&req(""), &j, &m));
        assert!(v["key"].is_null());
        assert_eq!(v["by"], "none");
        assert!(v["confidence"].is_null());
        assert_eq!(v["reason"], "needs_user");
    }

    #[test]
    fn allow_limits_the_rungs_and_leaves_the_default_all() {
        let (j, m) = (jeff(Some(("b", 0.99))), model(Some("a")));
        let v = ok(run(&req(r#","allow":["llm"]"#), &j, &m));
        assert_eq!(
            (v["by"].as_str(), v["key"].as_str()),
            (Some("model"), Some("a"))
        );
        assert_eq!(*j.1.borrow(), 0, "没让用 Jeff 就不问");
        let (j, m) = (jeff(None), model(Some("a")));
        let v = ok(run(&req(r#","allow":["jeff"]"#), &j, &m));
        assert_eq!(v["by"], "none");
        assert_eq!(*m.1.borrow(), 0, "没让用大模型就不问");
        assert_ne!(v["reason"], "needs_user", "用户这一级也没让用");
        let (j, m) = (jeff(Some(("b", 0.99))), model(Some("a")));
        assert_eq!(ok(run(&req(r#","allow":[]"#), &j, &m))["by"], "none");
        assert_eq!(*j.1.borrow() + *m.1.borrow(), 0);
    }

    #[test]
    fn a_forbidden_string_keeps_the_request_from_the_model_but_not_from_jeff() {
        for hit in [
            r#"{"state":"Acme Corp quarterly","options":[{"key":"a","text":"Alpha"},{"key":"b","text":"Beta"}],"forbid":["acme corp"]}"#,
            r#"{"state":"pick","options":[{"key":"a","text":"Alpha"},{"key":"b","text":"Send to ACME CORP"}],"forbid":["Acme Corp"]}"#,
        ] {
            let (j, m) = (jeff(None), model(Some("a")));
            let v = ok(run(hit, &j, &m));
            assert_eq!(*m.1.borrow(), 0, "命中禁词的请求不发给大模型");
            assert_eq!(*j.1.borrow(), 1, "Jeff 在本机，照问");
            assert_eq!(v["by"], "none");
            let reason = v["reason"].as_str().unwrap();
            assert!(
                reason.contains("needs_user") && reason.contains("llm_withheld"),
                "{reason}"
            );
        }
        let (j, m) = (jeff(Some(("b", 0.9))), model(Some("a")));
        let v = ok(run(
            r#"{"state":"Acme","options":[{"key":"a","text":"x"},{"key":"b","text":"y"}],"forbid":["acme"]}"#,
            &j,
            &m,
        ));
        assert_eq!(
            (v["by"].as_str(), v["key"].as_str()),
            (Some("jeff"), Some("b"))
        );
        assert!(v["reason"].as_str().unwrap().contains("llm_withheld"));
    }

    #[test]
    fn a_forbid_string_that_is_not_present_changes_nothing() {
        let (j, m) = (jeff(None), model(Some("a")));
        let v = ok(run(&req(r#","forbid":["nowhere"]"#), &j, &m));
        assert_eq!(v["by"], "model");
        assert!(v.get("reason").is_none());
    }

    #[test]
    fn the_picture_path_is_accepted_and_never_reaches_the_model_prompt() {
        let (j, m) = (jeff(None), model(Some("a")));
        let v = ok(run(&req(r#","picture":"/tmp/x.png""#), &j, &m));
        assert_eq!(v["by"], "model");
        assert_eq!(*m.2.borrow(), vec![None::<String>], "图不给大模型");
    }

    #[test]
    fn bad_input_is_exit_two_with_a_clear_message() {
        let many: String = (0..21)
            .map(|i| format!(r#"{{"key":"k{i}","text":"t"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let long = "x".repeat(50_001);
        let cases: Vec<(String, &str)> = vec![
            ("not json".into(), "JSON"),
            (r#"{"state":"s"}"#.into(), "options"),
            (r#"{"state":"s","options":[]}"#.into(), "options"),
            (r#"{"options":[{"key":"a","text":"A"}]}"#.into(), "state"),
            (
                r#"{"state":"s","options":[{"key":"a","text":"A"},{"key":"a","text":"B"}]}"#.into(),
                "重复",
            ),
            (
                r#"{"state":"s","options":[{"key":"","text":"A"}]}"#.into(),
                "key",
            ),
            (
                r#"{"state":"s","options":[{"key":"stuck","text":"A"}]}"#.into(),
                "stuck",
            ),
            (format!(r#"{{"state":"s","options":[{many}]}}"#), "20"),
            (
                format!(r#"{{"state":"{long}","options":[{{"key":"a","text":"A"}}]}}"#),
                "state",
            ),
            (req(r#","allow":["jeff","gpt"]"#), "gpt"),
            (req(r#","forbid":[""]"#), "forbid"),
            (req(r#","surprise":1"#), "surprise"),
            (r#"{"state":"s","options":[{"key":"a"}]}"#.into(), "text"),
        ];
        for (input, word) in cases {
            let (j, m) = (jeff(Some(("a", 0.99))), model(Some("a")));
            let (code, out) = run(&input, &j, &m);
            assert_eq!(code, 2, "{word}");
            assert!(out.unwrap_err().contains(word), "{word}");
            assert_eq!(*j.1.borrow() + *m.1.borrow(), 0, "坏请求不问任何人");
        }
    }

    #[test]
    fn exactly_twenty_options_are_fine() {
        let many: String = (0..20)
            .map(|i| format!(r#"{{"key":"k{i}","text":"t{i}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let (j, m) = (jeff(Some(("k3", 0.9))), model(None));
        let v = ok(run(
            &format!(r#"{{"state":"s","options":[{many}]}}"#),
            &j,
            &m,
        ));
        assert_eq!(v["key"], "k3");
    }

    #[test]
    fn a_jeff_key_that_is_not_an_option_is_not_taken() {
        let (j, m) = (jeff(Some(("zzz", 0.99))), model(Some("a")));
        let v = ok(run(&req(""), &j, &m));
        assert_eq!(v["by"], "model");
    }
}
