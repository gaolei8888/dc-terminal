//! 只读：问本机 dco 现在在干什么（`status` 工具），归一成一个很小的封闭集合。
//! 整个「dco 状态」功能的 dct 侧就这一个文件：看板头一格和 `dct status --json` 都从这里取。
//! （seam-and-answer 分支会把 dco 客户端抽成 `dct-dco`；到时这个文件原样跟着搬过去，改个 `use` 就行。）
//!
//! 不起 dco、不起守护进程、不发任何会动手的工具；整个读取有一个总期限，超了就是 `unknown`。
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// 一次读取的总期限（连接 + 握手 + 调用）。
pub const BUDGET: Duration = Duration::from_millis(800);
/// `activity` 文字最多这么多个字符。
const ACTIVITY_MAX_CHARS: usize = 40;
/// dco 回复里 `activity.schema_version` 认这一版；别的版本字段含义可能变了，不猜。
const SCHEMA: u64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DcoState {
    Idle,
    Running,
    /// 有一条 `dco.needs_help` 的待处理项（哪怕 dco 的 `activity.state` 此刻是 running）。
    WaitingUser,
    Paused,
    Halted,
    /// 缺系统权限（`dco.permission_missing`）。
    PermissionMissing,
    /// 屏幕锁着（`dco.screen_locked`）。
    ScreenLocked,
    /// 问了，连不上。
    Down,
    /// 连上了但答不出（超时、回复读不懂、状态词不认识、版本不对、有不认识的待处理项）。**永远不当成 idle。**
    Unknown,
}

impl DcoState {
    pub fn as_str(self) -> &'static str {
        match self {
            DcoState::Idle => "idle",
            DcoState::Running => "running",
            DcoState::WaitingUser => "waiting_user",
            DcoState::Paused => "paused",
            DcoState::Halted => "halted",
            DcoState::PermissionMissing => "permission_missing",
            DcoState::ScreenLocked => "screen_locked",
            DcoState::Down => "down",
            DcoState::Unknown => "unknown",
        }
    }
}

/// 认得的 `attention[].kind`（去掉 `dco.` 前缀），按严重程度排好：输出列表就按这个顺序，和 dco 回的顺序无关。
const KNOWN_KINDS: [&str; 5] = ["halted", "permission_missing", "screen_locked", "paused", "needs_help"];
const NEEDS_HELP: usize = 4;
/// 输出的 `attention` 列表最多这么多项（认得的 kind 只有 5 种、已去重，这只是一道保险）。
const ATTENTION_MAX: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DcoStatus {
    pub state: DcoState,
    /// 第一条 `dco.needs_help` 待处理项自己写的短句（已清洗、截断）；没有就是 `None`。不含屏幕或终端内容。
    pub activity: Option<String>,
    /// 正在跑的流程个数。
    pub runs: Option<u32>,
    /// 认得的待处理项种类（去掉 `dco.` 前缀、去重、按严重程度排）。`None` = 没读到（down、读不懂、版本不对）。
    pub attention: Option<Vec<&'static str>>,
}

impl DcoStatus {
    pub fn bare(state: DcoState) -> DcoStatus {
        DcoStatus { state, activity: None, runs: None, attention: None }
    }

    /// `dct status --json` 里 `dco` 那一块。
    pub fn to_json(&self) -> Value {
        let mut v = json!({ "state": self.state.as_str() });
        if let Some(a) = &self.activity {
            v["activity"] = json!(a);
        }
        if let Some(n) = self.runs {
            v["runs"] = json!(n);
        }
        if let Some(k) = &self.attention {
            v["attention"] = json!(k);
        }
        v
    }
}

/// 去掉控制字符、压缩空白、截断；长得像密钥的（一长串字母数字）整句丢掉。
fn clean(s: &str) -> Option<String> {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ").chars().filter(|c| !c.is_control()).collect();
    let mut run = 0;
    for c in s.chars() {
        run = if c.is_ascii_alphanumeric() || c == '_' || c == '-' { run + 1 } else { 0 };
        if run >= 24 {
            return None;
        }
    }
    let s: String = s.chars().take(ACTIVITY_MAX_CHARS).collect();
    (!s.is_empty()).then_some(s)
}

/// dco `status` 的回复 → 状态。纯函数。
///
/// 两个来源合起来看：`activity.state`（dco 自己取 halted > running > paused > needs_help 里最高的一个，
/// 所以待处理请求会被 running 盖住）和 `activity.attention[].kind`（独立的列表）。
/// 单个 `state` 的优先级，高到低：
/// halted > permission_missing > screen_locked > paused > waiting_user（attention 里有 `dco.needs_help`，
/// 即使 activity.state 是 running）> running > idle。
///
/// 「不认识」的处理（保守，永远不让状态看起来比已知的更健康）：不认识的 `activity.state`、
/// 不认识的 / 形状不对的 attention 项、不是数组的 `attention`，都记一笔「没把握」。
/// 没把握时：已知的告警态（halted、permission_missing、screen_locked、paused、waiting_user）照常保留；
/// 否则（本来会是 idle 或 running）一律 `unknown`。**不认识的东西永远不会变成 idle。**
/// 不认识的 kind 不进 `attention` 列表。`activity.state == "needs_help"` 但 attention 里没有
/// `dco.needs_help`：按批准的口径「等你」只认 attention，这种自相矛盾的回复算没把握（unknown）。
/// `activity` 文字只取第一条 `dco.needs_help` 项的 `text`。
///
/// 验证情况：**只亲眼见过空闲态**（2026-10-14 真实输出：state idle，attention 和 runs 都是空）。
/// 其余——running / paused / halted 的 state 词，以及 attention 的 kind 名和「优先级」——
/// 都来自 dco 会话读 dco 源码后的转述，**未验证**。
pub fn parse_status(body: &Value) -> DcoStatus {
    let a = &body["activity"];
    if !a.is_object() || a["schema_version"].as_u64() != Some(SCHEMA) {
        return DcoStatus::bare(DcoState::Unknown);
    }
    let mut unsure = false;
    let base = match a["state"].as_str() {
        Some("idle") => Some(DcoState::Idle),
        Some("running") => Some(DcoState::Running),
        Some("paused") => Some(DcoState::Paused),
        Some("halted") => Some(DcoState::Halted),
        _ => {
            unsure = true;
            None
        }
    };
    let mut seen = [false; KNOWN_KINDS.len()];
    let mut text = None;
    match a["attention"].as_array() {
        None => unsure = true,
        Some(items) => {
            for it in items {
                let known = it["kind"].as_str().and_then(|k| k.strip_prefix("dco.")).and_then(|k| KNOWN_KINDS.iter().position(|n| *n == k));
                match known {
                    Some(i) => {
                        if i == NEEDS_HELP && !seen[i] {
                            text = it["text"].as_str().and_then(clean);
                        }
                        seen[i] = true;
                    }
                    None => unsure = true,
                }
            }
        }
    }
    let state = if seen[0] || base == Some(DcoState::Halted) {
        DcoState::Halted
    } else if seen[1] {
        DcoState::PermissionMissing
    } else if seen[2] {
        DcoState::ScreenLocked
    } else if seen[3] || base == Some(DcoState::Paused) {
        DcoState::Paused
    } else if seen[NEEDS_HELP] {
        DcoState::WaitingUser
    } else {
        match (unsure, base) {
            (false, Some(s)) => s,
            _ => DcoState::Unknown,
        }
    };
    DcoStatus {
        state,
        activity: text,
        runs: a["runs"].as_array().map(|r| r.len().min(u32::MAX as usize) as u32),
        attention: Some(KNOWN_KINDS.iter().zip(seen).filter(|(_, s)| *s).map(|(k, _)| *k).take(ATTENTION_MAX).collect()),
    }
}

/// 问一次。`None` = 没装 dco（没有 `~/.dco/endpoint.json`），调用方应当什么都不显示。
/// 总耗时不超过 `budget` 多少：读取放在另一条线程里，主线程只等 `budget`；
/// 超时的线程自己最多再等几个 `budget` 就会因读超时结束（它不持有任何东西）。
#[cfg(unix)]
pub fn probe(home: &Path, budget: Duration) -> Option<DcoStatus> {
    use crate::dco::DcoClient;
    let dir = home.join(".dco");
    if !dir.join("endpoint.json").exists() {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // 第一项 = 出错发生在「连上」这一步。
        let r = match DcoClient::connect_with_timeout(&dir, budget) {
            Err(e) => Err((true, e)),
            Ok(mut c) => c.call("status", json!({})).map_err(|e| (false, e)),
        };
        let _ = tx.send(r);
    });
    Some(match rx.recv_timeout(budget) {
        Ok(Ok(body)) => parse_status(&body),
        // 没连上（没在跑、不让连、握手断了）= down；连上之后出任何错（超时、读不懂、太旧、报错）= unknown。
        Ok(Err((true, e))) if e.code == "dco_down" || e.code == "dco_blocked" => DcoStatus::bare(DcoState::Down),
        Ok(Err(_)) | Err(_) => DcoStatus::bare(DcoState::Unknown),
    })
}

/// dco 只有 Mac 版：别的系统上永远是「没装」。
#[cfg(not(unix))]
pub fn probe(_home: &Path, _budget: Duration) -> Option<DcoStatus> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(state: &str, extra: Value) -> Value {
        let mut a = json!({ "schema_version": 1, "state": state, "runs": [], "attention": [] });
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            a[k] = v;
        }
        json!({ "activity": a, "name": "dco" })
    }

    #[test]
    fn the_real_idle_reply_maps_to_idle() {
        let real: Value = serde_json::from_str(r#"{"activity":{"attention":[],"runs":[],"schema_version":1,"state":"idle"},"elapsed_ms":14,"name":"dco"}"#).unwrap();
        assert_eq!(parse_status(&real), DcoStatus { state: DcoState::Idle, activity: None, runs: Some(0), attention: Some(vec![]) });
    }

    fn att(kinds: &[&str]) -> Value {
        json!({ "attention": kinds.iter().map(|k| json!({"level": "yellow", "kind": k, "text": "t"})).collect::<Vec<_>>() })
    }

    fn st(raw: &str, kinds: &[&str]) -> DcoStatus {
        parse_status(&body(raw, att(kinds)))
    }

    #[test]
    fn precedence_is_halted_permission_locked_paused_waiting_running_idle() {
        use DcoState::*;
        for (raw, kinds, want) in [
            ("idle", &[][..], Idle),
            ("running", &[], Running),
            ("running", &["dco.needs_help"], WaitingUser),
            ("paused", &[], Paused),
            ("paused", &["dco.needs_help"], Paused),
            ("halted", &[], Halted),
            ("halted", &["dco.needs_help"], Halted),
            ("running", &["dco.halted"], Halted),
            ("running", &["dco.needs_help", "dco.paused", "dco.screen_locked", "dco.permission_missing"], PermissionMissing),
            ("running", &["dco.needs_help", "dco.paused", "dco.screen_locked"], ScreenLocked),
            ("running", &["dco.needs_help", "dco.paused"], Paused),
            ("idle", &["dco.permission_missing"], PermissionMissing),
            ("idle", &["dco.screen_locked"], ScreenLocked),
        ] {
            assert_eq!(st(raw, kinds).state, want, "{raw} {kinds:?}");
        }
    }

    #[test]
    fn needs_help_is_waiting_only_via_attention() {
        // activity.state 自己说 needs_help 但 attention 里没有：不算「等你」，也绝不是 idle。
        assert_eq!(st("needs_help", &[]).state, DcoState::Unknown);
        assert_eq!(st("needs_help", &["dco.needs_help"]).state, DcoState::WaitingUser);
    }

    #[test]
    fn unknown_things_never_look_healthier_than_known_ones_and_never_idle() {
        use DcoState::*;
        for raw in ["", "sleeping", "IDLE", "waiting_user"] {
            assert_eq!(st(raw, &[]).state, Unknown, "{raw:?}");
        }
        assert_eq!(st("idle", &["dco.new_thing"]).state, Unknown);
        assert_eq!(st("idle", &["needs_help"]).state, Unknown, "没有 dco. 前缀的不认");
        assert_eq!(st("running", &["dco.new_thing"]).state, Unknown);
        // 已知的告警态不被「不认识」冲淡，也不被它升级
        assert_eq!(st("running", &["dco.new_thing", "dco.needs_help"]).state, WaitingUser);
        assert_eq!(st("running", &["dco.new_thing", "dco.halted"]).state, Halted);
        assert_eq!(st("sleeping", &["dco.paused"]).state, Paused);
        assert_eq!(st("sleeping", &["dco.needs_help"]).state, WaitingUser);
        // 不认识的 kind 不进列表
        assert_eq!(st("idle", &["dco.new_thing"]).attention, Some(vec![]));
        assert_eq!(parse_status(&json!({"activity": {"schema_version": 1}})).state, Unknown);
        assert_eq!(parse_status(&json!({"activity": {"schema_version": 2, "state": "idle"}})).state, Unknown);
        assert_eq!(parse_status(&json!({"activity": "idle"})).state, Unknown);
        assert_eq!(parse_status(&json!({})).state, Unknown);
        assert_eq!(parse_status(&Value::Null).state, Unknown);
    }

    #[test]
    fn odd_attention_shapes_do_not_panic_and_are_unknown() {
        for odd in [json!(null), json!("x"), json!({"kind": "dco.needs_help"}), json!(7), json!([null]), json!([7, "dco.needs_help"]), json!([{"kind": 5}]), json!([{"kind": ["dco.paused"]}]), json!([{"text": "no kind"}]), json!([[]])] {
            let s = parse_status(&body("idle", json!({ "attention": odd.clone() })));
            assert_eq!(s.state, DcoState::Unknown, "{odd}");
            let s = parse_status(&body("running", json!({ "attention": odd.clone() })));
            assert_eq!(s.state, DcoState::Unknown, "{odd}");
        }
        // attention 键整个没有
        let a = json!({"activity": {"schema_version": 1, "state": "idle", "runs": []}});
        assert_eq!(parse_status(&a).state, DcoState::Unknown);
    }

    #[test]
    fn the_attention_list_is_stable_deduplicated_known_only_and_capped() {
        let s = st("running", &["dco.needs_help", "dco.paused", "dco.needs_help", "dco.paused", "dco.x"]);
        assert_eq!(s.attention, Some(vec!["paused", "needs_help"]), "按严重程度排，不按 dco 给的顺序");
        let many: Vec<String> = (0..30).map(|i| if i % 2 == 0 { format!("dco.junk{i}") } else { "dco.screen_locked".to_string() }).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let s = st("idle", &refs);
        assert_eq!(s.attention, Some(vec!["screen_locked"]));
        let all = st("idle", &["dco.needs_help", "dco.paused", "dco.screen_locked", "dco.permission_missing", "dco.halted"]);
        assert_eq!(all.attention.unwrap(), ["halted", "permission_missing", "screen_locked", "paused", "needs_help"]);
        assert!(ATTENTION_MAX >= KNOWN_KINDS.len());
    }

    #[test]
    fn running_with_needs_help_json_shape() {
        let b = body("running", json!({ "attention": [{"level": "yellow", "kind": "dco.needs_help", "text": "need you"}], "runs": [{"intent": "x"}] }));
        assert_eq!(parse_status(&b).to_json(), json!({"state": "waiting_user", "activity": "need you", "runs": 1, "attention": ["needs_help"]}));
    }

    #[test]
    fn activity_text_is_the_first_needs_help_line_cleaned_and_capped() {
        let b = body("running", json!({ "attention": [{"kind": "dco.paused", "text": "ignore me"}, {"kind": "dco.needs_help", "text": "  need\u{7}\n you  "}, {"kind": "dco.needs_help", "text": "second"}] }));
        assert_eq!(parse_status(&b).activity.as_deref(), Some("need you"));
        let long = "字".repeat(100);
        let b = body("running", json!({ "attention": [{"kind": "dco.needs_help", "text": long}] }));
        assert_eq!(parse_status(&b).activity.unwrap().chars().count(), ACTIVITY_MAX_CHARS);
        // 别的 kind 的文字不外泄
        let b = body("running", json!({ "attention": [{"kind": "dco.paused", "text": "x"}] }));
        assert_eq!(parse_status(&b).activity, None);
    }

    #[test]
    fn a_secret_looking_text_and_run_details_never_reach_the_output() {
        let b = body("running", json!({ "attention": [{"kind": "dco.needs_help", "text": "token 0123456789abcdef0123456789abcdef"}], "runs": [{"intent": "secret business", "procedure": "p", "app": "SomeApp"}] }));
        let s = parse_status(&b);
        assert_eq!(s.activity, None);
        assert_eq!(s.runs, Some(1));
        let j = s.to_json().to_string();
        assert!(!j.contains("business") && !j.contains("0123") && !j.contains("SomeApp"), "{j}");
    }
}
