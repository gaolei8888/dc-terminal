//! 摸索的轨迹和 Jeff 训练题记录。写盘前参数值一律换回 `{参数名}`。
use crate::hands::Params;
use crate::ladder::Who;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TraceStep {
    pub action: String, // 封闭词表
    pub arg: String,    // 意图名或参数名，永远不是参数值
    pub by: Who,
    pub obs_before: Option<String>,
    pub obs_after: Option<String>,
    pub scene_before: Option<String>,
    pub scene_after: Option<String>,
    /// dco 回 done。摸索轨迹里另有一层意思：这一步在通往当前画面的路上（做了但画面没变、
    /// 或绕回了见过的画面，那几步记成 false；见 `explore` 第 8 条）。草稿只抄 ok 的步骤。
    pub ok: bool,
    pub publish: bool, // 这一下是「最终发布」
    /// 这一下之前在、之后不在的那几行字（判断是不是关弹窗用）
    pub vanished: Vec<String>,
    pub learned_keys: Vec<String>,
    /// 修补时从已确认流程抄过来的前几步（Task D14 用）；摸索出来的步骤为 false。
    #[serde(default)]
    pub from_procedure: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Trace {
    pub run_id: String,
    pub platform: String,
    pub params: Vec<String>,
    pub steps: Vec<TraceStep>,
    pub first_lines: Vec<String>,
    /// 这次运行的结局；摸索循环返回前填上。旧轨迹没有这个字段（读出来是 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
}

/// 一次运行的结局（和 `explore::Outcome` 一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunOutcome {
    Published,
    Declined,
    Halted,
    Stopped,
}

/// 照这道题的答案做了以后，是不是确认离目标更近了。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepOutcome {
    /// dco 回 done、到了这次运行没见过的新画面、这一步在**最终**轨迹里还是 `ok: true`
    /// （没被后来的绕圈改掉），而且这次运行发出去了。`done` 那道题：运行发出去了。
    Good,
    /// 确认没起作用：画面没到新画面，或这一步后来被绕圈改成了 `ok: false`。
    Bad,
    /// 这一步一直在路上，但这次运行没发出去（停下 / 拦下 / 用户不让发）：说不清。
    Unknown,
}

fn id_char_ok(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// dc-vault 的 `learned_from: run:<编号>` 规则：字母、数字、`-`、`_`，最多 64 个。
fn valid_run_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(id_char_ok)
}

/// 由 Unix 天数算公历年月日（Howard Hinnant 的 civil_from_days）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

/// `<平台>-<YYYYMMDD UTC>-<四位小写十六进制>`，例如 `x-20261011-1a2b`。
pub fn new_run_id(platform: &str, now_ms: u64, rand4: u16) -> String {
    let (y, m, d) = civil_from_days((now_ms / 86_400_000) as i64);
    let mut p: String = platform
        .bytes()
        .filter(|b| id_char_ok(*b))
        .take(32)
        .map(|b| b as char)
        .collect();
    if p.is_empty() {
        p.push_str("run");
    }
    format!("{p}-{y:04}{m:02}{d:02}-{rand4:04x}")
}

/// 把文本里的参数值换回 `{参数名}`。一遍扫描、每个位置取最长的匹配值，
/// 所以一个值是另一个值的子串、或值恰好出现在占位符里，都不会出错；空值跳过。
pub fn redact(text: &str, values: &Params) -> String {
    let mut vs: Vec<(&str, &str)> = values
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    vs.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some((name, v)) = vs.iter().find(|(_, v)| rest.starts_with(v)) {
            out.push('{');
            out.push_str(name);
            out.push('}');
            rest = &rest[v.len()..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

fn open_private(path: &Path, append: bool) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true);
    if append {
        o.append(true);
    } else {
        o.truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let f = o.open(path)?;
    #[cfg(unix)]
    {
        // mode() 只在新建时生效；旧文件权限太松也收紧。
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(f)
}

/// 存到 `<dir>/<run_id>/trace.json`，0600。
pub fn save(dir: &Path, t: &Trace) -> std::io::Result<PathBuf> {
    if !valid_run_id(&t.run_id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid run_id",
        ));
    }
    let d = dir.join(&t.run_id);
    std::fs::create_dir_all(&d)?;
    let p = d.join("trace.json");
    let json = serde_json::to_string_pretty(t).map_err(std::io::Error::other)?;
    open_private(&p, false)?.write_all(json.as_bytes())?;
    Ok(p)
}

/// 训练题行的格式版本。没有 `v` 的旧行（v1）只有下面到 `corrects_jeff` 为止的字段。
pub const QUESTION_ROW_V: u32 = 2;

/// 一行训练题。v2 在运行**结束以后**才写（结局定了，`outcome` 不会再变），见 `grade`。
#[derive(serde::Serialize)]
pub struct QuestionRow<'a> {
    /// 格式版本，`QUESTION_ROW_V`。
    pub v: u32,
    pub ts_ms: u64,
    pub run_id: &'a str,
    pub platform: &'a str,
    pub state: String,
    pub options: Vec<(String, String)>,
    pub jeff: Option<(String, f64)>,
    pub answer: &'a str,
    pub by: Who,
    pub corrects_jeff: bool,
    /// 这一轮的候选单（`options::shortlist`，最多 20 个）的 key，按发给 Jeff 的顺序。
    /// 就是 Jeff 这一轮被问到的题；Jeff 在这个画面上失败过、这轮没问它时，也是会发给它的那份。
    pub shown: Vec<String>,
    /// 这一轮真的问了 Jeff（它答没答都算问了）；Jeff 在这个画面上失败过、这轮跳过它时为 false。
    pub jeff_asked: bool,
    /// 照这个答案做的第一步在 `runs/<run_id>/trace.json` 的 `steps` 里的序号；`done` 为空。
    pub step: Option<usize>,
    pub run_outcome: RunOutcome,
    pub outcome: StepOutcome,
    /// 能不能当标准答案（`grade`）。
    pub label: bool,
}

/// 标签规矩里「谁的、结局如何才算」那一半，只此一处：用户的答案永远算；大模型的只有 `Good` 才算；
/// Jeff 的永远不算（拿自己的答案练自己）。`grade` 和外面直接给了结局的记录（`dct train record`）都走它。
pub fn label_for(by: Who, outcome: StepOutcome) -> bool {
    match by {
        Who::User => true,
        Who::Model => outcome == StepOutcome::Good,
        Who::Jeff => false,
    }
}

/// 标签规矩。`step_ok`：这道题的步骤在最终轨迹里的 `ok`（`done` 那道题为 None）；
/// `moved`：照做后 dco 回 done、而且到了这次运行没见过的新画面。
/// 谁的答案算标签见 `label_for`。
pub fn grade(by: Who, step_ok: Option<bool>, moved: bool, run: RunOutcome) -> (StepOutcome, bool) {
    let outcome = if !moved || step_ok == Some(false) {
        StepOutcome::Bad
    } else if run == RunOutcome::Published {
        StepOutcome::Good
    } else {
        StepOutcome::Unknown
    };
    let label = label_for(by, outcome);
    (outcome, label)
}

/// 读一行训练题（任何版本）判断是不是标准答案：v2 看 `label`（`by: jeff` 一律不算）；
/// 没有 `v` 的旧行没有结局可查，只有 `by: user` 算。
pub fn is_label(row: &serde_json::Value) -> bool {
    let by = row["by"].as_str();
    match row["v"].as_u64() {
        None => by == Some("user"),
        Some(_) => by != Some("jeff") && row["label"].as_bool() == Some(true),
    }
}

/// 追加一行 JSON，0600。调用方要先对 `state` 和选项文字做 `redact`。
pub fn append_question(file: &Path, row: &QuestionRow) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(row).map_err(std::io::Error::other)?;
    line.push('\n');
    open_private(file, true)?.write_all(line.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hands::Params;
    use crate::ladder::Who;

    fn vals() -> Params {
        [
            (
                "post_text".to_string(),
                "hello world, this is Lei".to_string(),
            ),
            ("link".to_string(), "hello".to_string()),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn redact_replaces_longest_values_first() {
        assert_eq!(
            redact("typed: hello world, this is Lei / hello", &vals()),
            "typed: {post_text} / {link}"
        );
    }

    #[test]
    fn redact_does_not_touch_placeholders_or_empty_values() {
        let v: Params = [
            ("a".to_string(), "post".to_string()),
            ("b".to_string(), "my post text".to_string()),
            ("c".to_string(), String::new()),
        ]
        .into_iter()
        .collect();
        assert_eq!(redact("my post text and post", &v), "{b} and {a}");
        assert_eq!(redact("plain", &v), "plain");
    }

    #[test]
    fn run_ids_fit_dcvs_learned_from_rule() {
        let id = new_run_id("x", 1_791_676_800_000, 0x1a2b);
        assert!(id.starts_with("x-2026"));
        assert!(
            id.len() <= 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "{id}"
        );
        assert_eq!(id, "x-20261011-1a2b");
        let long = new_run_id(&"p/../".repeat(40), 0, 0xffff);
        assert!(
            long.len() <= 64
                && long
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "{long}"
        );
    }

    #[test]
    fn question_rows_on_disk_hold_no_param_values() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("answers.jsonl");
        let state = redact(
            "目标：发帖\n页面上的字：\n[e2] hello world, this is Lei",
            &vals(),
        );
        append_question(
            &f,
            &QuestionRow {
                v: QUESTION_ROW_V,
                ts_ms: 1,
                run_id: "x-1",
                platform: "x",
                state,
                options: vec![("e3".into(), "按钮 Post".into())],
                jeff: Some(("stuck".into(), 0.4)),
                answer: "e3",
                by: Who::Model,
                corrects_jeff: true,
                shown: vec!["e3".into()],
                jeff_asked: true,
                step: Some(1),
                run_outcome: RunOutcome::Published,
                outcome: StepOutcome::Good,
                label: true,
            },
        )
        .unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(!text.contains("hello world"), "{text}");
        assert!(text.contains("\"corrects_jeff\":true"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn question_rows_append_one_line_each() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("sub").join("answers.jsonl");
        for i in 0..2 {
            append_question(
                &f,
                &QuestionRow {
                    v: QUESTION_ROW_V,
                    ts_ms: i,
                    run_id: "x-1",
                    platform: "x",
                    state: "s".into(),
                    options: vec![],
                    jeff: None,
                    answer: "e1",
                    by: Who::Jeff,
                    corrects_jeff: false,
                    shown: vec![],
                    jeff_asked: true,
                    step: None,
                    run_outcome: RunOutcome::Stopped,
                    outcome: StepOutcome::Unknown,
                    label: false,
                },
            )
            .unwrap();
        }
        assert_eq!(std::fs::read_to_string(&f).unwrap().lines().count(), 2);
    }

    #[test]
    fn the_label_rule_counts_users_and_confirmed_model_steps_only() {
        use RunOutcome::*;
        use StepOutcome::*;
        // 用户：永远算；结局照记。
        assert_eq!(grade(Who::User, Some(false), true, Stopped), (Bad, true));
        // 大模型：在发出去那条路上、到了新画面 → good，算。
        assert_eq!(grade(Who::Model, Some(true), true, Published), (Good, true));
        assert_eq!(grade(Who::Model, None, true, Published), (Good, true)); // done
                                                                            // 绕回（最终 ok: false）、画面没到新画面 → bad，不算。
        assert_eq!(
            grade(Who::Model, Some(false), true, Published),
            (Bad, false)
        );
        assert_eq!(
            grade(Who::Model, Some(true), false, Published),
            (Bad, false)
        );
        assert_eq!(grade(Who::Model, Some(false), true, Halted), (Bad, false));
        // 还在路上、可这次没发出去：说不清，不算。
        for run in [Stopped, Halted, Declined] {
            assert_eq!(grade(Who::Model, Some(true), true, run), (Unknown, false));
        }
        // Jeff：结局照记，自己永远不算。
        assert_eq!(grade(Who::Jeff, Some(true), true, Published), (Good, false));
    }

    #[test]
    fn old_rows_without_v_only_count_user_answers() {
        let old_user: serde_json::Value =
            serde_json::from_str(r#"{"ts_ms":1,"run_id":"x-1","platform":"x","state":"s","options":[["e1","a"]],"jeff":null,"answer":"e1","by":"user","corrects_jeff":false}"#).unwrap();
        let old_model: serde_json::Value =
            serde_json::from_str(r#"{"ts_ms":1,"run_id":"x-1","platform":"x","state":"s","options":[["e1","a"]],"jeff":null,"answer":"e1","by":"model","corrects_jeff":false}"#).unwrap();
        assert!(is_label(&old_user));
        assert!(!is_label(&old_model));
        let v2 = |by: &str, label: bool| serde_json::json!({"v": 2, "by": by, "label": label});
        assert!(is_label(&v2("model", true)));
        assert!(!is_label(&v2("model", false)));
        // 一行 v2 写着 jeff 的 label: true 也不算（规矩不靠写盘的那一位）。
        assert!(!is_label(&v2("jeff", true)));
    }

    #[test]
    fn old_traces_without_outcome_still_load() {
        let json = r#"{"run_id":"x-1","platform":"x","params":[],"steps":[],"first_lines":[]}"#;
        let t: Trace = serde_json::from_str(json).unwrap();
        assert_eq!(t.outcome, None);
    }

    #[test]
    fn trace_round_trips() {
        let d = tempfile::tempdir().unwrap();
        let t = Trace {
            run_id: "x-1".into(),
            platform: "x".into(),
            params: vec!["post_text".into()],
            first_lines: vec!["What's happening?".into()],
            steps: vec![],
            outcome: None,
        };
        let p = save(d.path(), &t).unwrap();
        let back: Trace = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(back.run_id, "x-1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn save_refuses_a_run_id_that_could_escape_the_directory() {
        let d = tempfile::tempdir().unwrap();
        let t = Trace {
            run_id: "../evil".into(),
            platform: "x".into(),
            params: vec![],
            first_lines: vec![],
            steps: vec![],
            outcome: None,
        };
        assert!(save(d.path(), &t).is_err());
    }

    #[test]
    fn label_for_is_the_one_place_that_decides_who_counts() {
        use StepOutcome::*;
        for o in [Good, Bad, Unknown] {
            assert!(label_for(Who::User, o), "用户的答案永远算：{o:?}");
            assert!(!label_for(Who::Jeff, o), "Jeff 的永远不算：{o:?}");
            assert_eq!(label_for(Who::Model, o), o == Good, "{o:?}");
        }
    }

    #[test]
    fn grade_agrees_with_label_for() {
        for by in [Who::User, Who::Model, Who::Jeff] {
            for (ok, moved, run) in [
                (Some(true), true, RunOutcome::Published),
                (Some(false), true, RunOutcome::Published),
                (None, false, RunOutcome::Published),
                (Some(true), true, RunOutcome::Stopped),
            ] {
                let (o, l) = grade(by, ok, moved, run);
                assert_eq!(l, label_for(by, o));
            }
        }
    }
}
