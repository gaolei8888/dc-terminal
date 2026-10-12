//! `dct train record --json`：把一道别处答过的题记成一行 v2 训练题，追加进 `~/.dct/explore/answers.jsonl`。
//!
//! 标签规矩只有 `dct_explore::trace` 里那一份（`label_for`）：用户的答案永远算；大模型的只有结局是
//! good 才算；Jeff 自己的永远不算。这里不另写规矩，只把调用方给的结局交给它。
use crate::answer::{check_forbid, hits_forbid, read_stdin_limited};
use dct_explore::ladder::Who;
use dct_explore::trace::{
    append_question, label_for, QuestionRow, RunOutcome, StepOutcome, QUESTION_ROW_V,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

const MAX_ID_CHARS: usize = 128;
const MAX_KEY_CHARS: usize = 64;
const MAX_TEXT_CHARS: usize = 2_000;
const MAX_SHOWN: usize = 254;

const USAGE: &str = "用法：dct train record --json    从标准输入读一个请求（一个 JSON），记成一行训练题
请求：{\"question_id\": \"…\", \"by\": \"jeff\"|\"model\"|\"user\", \"key\": \"…\",
      \"outcome\": \"good\"|\"bad\"|\"unknown\", \"run_outcome\": \"published\"|\"declined\"|\"halted\"|\"stopped\",
      \"corrects_jeff\": 布尔, \"jeff_asked\": 布尔, \"shown\": [{\"key\",\"text\"}], \"step\": 整数,
      \"state\": \"题面(可选)\", \"forbid\": [\"不许出现的字\"] (可选)}      后面几项都可选
文字里的用户的值要调用方先换成 {名字}；命中 forbid 的记录一律拒绝。
答：{\"recorded\": true, \"label\": 布尔}
退出码：0 记下了；1 写不进文件；2 请求不对";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Shown {
    key: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    question_id: String,
    by: Who,
    key: String,
    outcome: StepOutcome,
    run_outcome: RunOutcome,
    #[serde(default)]
    corrects_jeff: bool,
    #[serde(default)]
    jeff_asked: bool,
    #[serde(default)]
    shown: Vec<Shown>,
    step: Option<u32>,
    /// 题面（可选）。没有它，这一行没法拼回当时那道题，所以训练的人要的话请带上。
    #[serde(default)]
    state: String,
    #[serde(default)]
    forbid: Vec<String>,
}

fn validate(r: &Request) -> Result<(), String> {
    if r.question_id.trim().is_empty() || r.question_id.chars().count() > MAX_ID_CHARS {
        return Err(format!("question_id 不能为空、最多 {MAX_ID_CHARS} 个字"));
    }
    if r.key.trim().is_empty() || r.key.chars().count() > MAX_KEY_CHARS {
        return Err(format!("key 不能为空、最多 {MAX_KEY_CHARS} 个字"));
    }
    if r.shown.len() > MAX_SHOWN {
        return Err(format!("shown 最多 {MAX_SHOWN} 个"));
    }
    let mut seen = std::collections::HashSet::new();
    for s in &r.shown {
        if s.key.trim().is_empty() || s.key.chars().count() > MAX_KEY_CHARS {
            return Err(format!("shown 的 key 不能为空、最多 {MAX_KEY_CHARS} 个字"));
        }
        if !seen.insert(s.key.as_str()) {
            return Err(format!("shown 的 key 重复：{}", s.key));
        }
        if s.text.chars().count() > MAX_TEXT_CHARS {
            return Err(format!(
                "shown {} 的 text 最多 {MAX_TEXT_CHARS} 个字",
                s.key
            ));
        }
    }
    if r.state.chars().count() > crate::judge::MAX_STATE_CHARS {
        return Err(format!(
            "state 不能超过 {} 字",
            crate::judge::MAX_STATE_CHARS
        ));
    }
    check_forbid(&r.forbid)?;
    let texts = std::iter::once(r.state.as_str())
        .chain(std::iter::once(r.key.as_str()))
        .chain(
            r.shown
                .iter()
                .flat_map(|s| [s.key.as_str(), s.text.as_str()]),
        );
    if hits_forbid(&r.forbid, texts) {
        return Err("记录里有 forbid 列表里的字，拒绝记录（要先把那些字换成 {名字}）".into());
    }
    Ok(())
}

/// 处理一个请求，写到 `file`（追加，0600）。返回 (退出码, 要打到 stdout 的 JSON)。
/// `question_id` 记进 `run_id` 一栏：外面答的题没有 `runs/<id>/trace.json`，`step` 也就只是调用方自己的序号。
pub fn record(file: &Path, input: &str, now_ms: u64) -> (i32, Result<Value, String>) {
    let r: Request = match serde_json::from_str(input) {
        Ok(r) => r,
        Err(e) => return (2, Err(format!("请求不是合法的 JSON 或字段不对：{e}"))),
    };
    if let Err(m) = validate(&r) {
        return (2, Err(m));
    }
    let label = label_for(r.by, r.outcome);
    let row = QuestionRow {
        v: QUESTION_ROW_V,
        ts_ms: now_ms,
        run_id: &r.question_id,
        platform: "",
        state: r.state.clone(),
        options: r
            .shown
            .iter()
            .map(|s| (s.key.clone(), s.text.clone()))
            .collect(),
        jeff: None,
        answer: &r.key,
        by: r.by,
        corrects_jeff: r.corrects_jeff,
        shown: r.shown.iter().map(|s| s.key.clone()).collect(),
        jeff_asked: r.jeff_asked,
        step: r.step.map(|n| n as usize),
        run_outcome: r.run_outcome,
        outcome: r.outcome,
        label,
    };
    match append_question(file, &row) {
        Ok(()) => (0, Ok(json!({"recorded": true, "label": label}))),
        Err(e) => (1, Err(format!("写不进 {}：{e}", file.display()))),
    }
}

/// `dct train record --json`。位置和 `dct do` 写训练题的是同一个：`<socket 同级目录>/explore/answers.jsonl`。
pub fn run_cli(args: &[String]) -> i32 {
    if args != ["record", "--json"] {
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
    let dir = socket.parent().map(Path::to_path_buf).unwrap_or_default();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let (code, out) = record(&dir.join("explore").join("answers.jsonl"), &input, now_ms);
    match out {
        Ok(v) => println!("{v}"),
        Err(m) => eprintln!("{m}"),
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(by: &str, outcome: &str, extra: &str) -> String {
        format!(
            r#"{{"question_id":"q1","by":"{by}","key":"e3","outcome":"{outcome}","run_outcome":"published"{extra}}}"#
        )
    }
    fn rec(dir: &Path, input: &str) -> (i32, Result<Value, String>) {
        record(
            &dir.join("explore").join("answers.jsonl"),
            input,
            1_700_000_000_000,
        )
    }
    fn lines(dir: &Path) -> Vec<Value> {
        std::fs::read_to_string(dir.join("explore").join("answers.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn every_by_and_outcome_gets_the_one_shared_label_rule() {
        for by in ["jeff", "model", "user"] {
            for outcome in ["good", "bad", "unknown"] {
                let d = tempfile::tempdir().unwrap();
                let (code, out) = rec(d.path(), &req(by, outcome, ""));
                assert_eq!(code, 0, "{by} {outcome}: {out:?}");
                let want = match by {
                    "user" => true,
                    "model" => outcome == "good",
                    _ => false,
                };
                let v = out.unwrap();
                assert_eq!(v["recorded"], true);
                assert_eq!(v["label"], want, "{by} {outcome}");
                let rows = lines(d.path());
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0]["label"], want);
                assert_eq!(rows[0]["by"], by);
                assert_eq!(rows[0]["outcome"], outcome);
                // 读的人用的那条规矩读出来也一样
                assert_eq!(
                    dct_explore::trace::is_label(&rows[0]),
                    want,
                    "{by} {outcome}"
                );
            }
        }
    }

    #[test]
    fn the_row_is_v2_with_the_fields_the_trainer_reads() {
        let d = tempfile::tempdir().unwrap();
        let input = req(
            "model",
            "good",
            r#","corrects_jeff":true,"jeff_asked":true,"step":4,"state":"screen text","shown":[{"key":"e3","text":"Post"},{"key":"e4","text":"Cancel"}]"#,
        );
        assert_eq!(rec(d.path(), &input).0, 0);
        let r = &lines(d.path())[0];
        assert_eq!(r["v"], 2);
        assert_eq!(r["run_id"], "q1");
        assert_eq!(r["answer"], "e3");
        assert_eq!(r["run_outcome"], "published");
        assert_eq!(r["corrects_jeff"], true);
        assert_eq!(r["jeff_asked"], true);
        assert_eq!(r["step"], 4);
        assert_eq!(r["state"], "screen text");
        assert_eq!(r["shown"], serde_json::json!(["e3", "e4"]));
        assert_eq!(
            r["options"],
            serde_json::json!([["e3", "Post"], ["e4", "Cancel"]])
        );
        assert!(r["ts_ms"].as_u64().unwrap() > 0);
    }

    #[test]
    fn optional_fields_default_and_rows_append() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(rec(d.path(), &req("user", "unknown", "")).0, 0);
        assert_eq!(rec(d.path(), &req("user", "good", "")).0, 0);
        let rows = lines(d.path());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["corrects_jeff"], false);
        assert_eq!(rows[0]["jeff_asked"], false);
        assert!(rows[0]["step"].is_null());
        assert_eq!(rows[0]["shown"], serde_json::json!([]));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_0600_even_if_it_was_looser_before() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        assert_eq!(rec(d.path(), &req("user", "good", "")).0, 0);
        let f = d.path().join("explore").join("answers.jsonl");
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(rec(d.path(), &req("user", "good", "")).0, 0);
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_forbidden_string_in_any_text_refuses_the_record_and_writes_nothing() {
        for extra in [
            r#","state":"mail to Acme Corp","forbid":["acme corp"]"#,
            r#","shown":[{"key":"e3","text":"Send to ACME"}],"forbid":["Acme"]"#,
        ] {
            let d = tempfile::tempdir().unwrap();
            let (code, out) = rec(d.path(), &req("user", "good", extra));
            assert_eq!(code, 2);
            assert!(out.unwrap_err().contains("forbid"));
            assert!(lines(d.path()).is_empty());
        }
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            rec(
                d.path(),
                &req("user", "good", r#","state":"clean","forbid":["acme"]"#)
            )
            .0,
            0
        );
    }

    #[test]
    fn bad_input_is_exit_two_and_writes_nothing() {
        let cases = [
            "nope".to_string(),
            req("robot", "good", ""),
            req("user", "great", ""),
            req("user", "good", "").replace("published", "weird"),
            req("user", "good", "").replace(r#""key":"e3""#, r#""key":"""#),
            req("user", "good", "").replace(r#""question_id":"q1""#, r#""question_id":"""#),
            req("user", "good", r#","step":-1"#),
            req("user", "good", r#","extra":1"#),
            req("user", "good", r#","forbid":[""]"#),
            req(
                "user",
                "good",
                r#","shown":[{"key":"a","text":"x"},{"key":"a","text":"y"}]"#,
            ),
            r#"{"by":"user","key":"a","outcome":"good","run_outcome":"published"}"#.to_string(),
        ];
        for c in cases {
            let d = tempfile::tempdir().unwrap();
            let (code, out) = rec(d.path(), &c);
            assert_eq!(code, 2, "{c}");
            assert!(out.is_err());
            assert!(lines(d.path()).is_empty(), "{c}");
        }
    }

    #[test]
    fn a_file_that_cannot_be_written_is_exit_one() {
        let d = tempfile::tempdir().unwrap();
        let blocker = d.path().join("explore");
        std::fs::write(&blocker, "a file where the directory should be").unwrap();
        let (code, out) = record(&blocker.join("answers.jsonl"), &req("user", "good", ""), 1);
        assert_eq!(code, 1);
        assert!(out.is_err());
    }
}
