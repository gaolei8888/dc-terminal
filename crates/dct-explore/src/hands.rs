//! dct 要用的 dco 调用，强类型。摸索和重放的逻辑只认 `Hands`，不直接碰 JSON。
use dct_dco::{DcoError, Reply};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Frac {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct El {
    pub id: String,
    pub text: String,
    pub role: String,
    pub frac: Frac,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct WindowRow {
    pub window_id: u64,
    pub app: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct Look {
    pub snapshot_id: String,
    pub observation_id: Option<String>,
    pub scene_hash: Option<String>,
    pub elements: Vec<El>,
    /// 只有 marks: true 时有：编号图（PNG base64）。
    pub marks_png: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepReq {
    pub action: String,
    pub arg: String,
    pub done_when: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Help {
    pub snapshot_id: String,
    pub intent: String,
    pub question: String,
    pub elements: Vec<El>,
    pub picture_png: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RunReply {
    pub run_id: String,
    /// done / already_done / needs_help / blocked / failed / stopped / cancelled / halted / paused
    pub status: String,
    pub at_step: u32,
    pub note: Option<String>,
    pub help: Option<Help>,
    /// dco 报的「未确认」学习条目的 key
    pub learned_keys: Vec<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    Model,
    User,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HelpAnswer {
    Element {
        id: String,
        by: By,
        name: Option<String>,
        dismiss: bool,
    },
    NotThere,
    GiveUp,
}

pub type Params = std::collections::BTreeMap<String, String>;

pub trait Hands {
    fn list_windows(&mut self) -> Result<Vec<WindowRow>, DcoError>;
    /// see {window_id, scene_hash: true, marks}
    fn look(&mut self, window_id: u64, marks: bool) -> Result<Look, DcoError>;
    fn run_step(
        &mut self,
        window_id: u64,
        step: &StepReq,
        params: &Params,
        popups: &[Value],
    ) -> Result<RunReply, DcoError>;
    fn run_procedure(
        &mut self,
        window_id: u64,
        procedure: &Value,
        params: &Params,
        popups: &[Value],
        stop_before_step: Option<u32>,
    ) -> Result<RunReply, DcoError>;
    fn answer_run(&mut self, run_id: &str, a: &HelpAnswer) -> Result<RunReply, DcoError>;
    fn cancel_run(&mut self, run_id: &str) -> Result<(), DcoError>;
    fn confirm_learned(&mut self, accept: &[String], reject: &[String]) -> Result<(), DcoError>;
}

fn bad(msg: impl Into<String>) -> DcoError {
    DcoError {
        code: "dco_error".into(),
        message: msg.into(),
    }
}

fn str_of(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

fn opt_str(v: &Value, k: &str) -> Option<String> {
    v[k].as_str().map(str::to_string)
}

fn parse_el(v: &Value) -> Result<El, DcoError> {
    let id = v["id"]
        .as_str()
        .ok_or_else(|| bad("dco 回的元素没有 id"))?
        .to_string();
    let f = &v["frac"];
    let n = |k: &str| f[k].as_f64().unwrap_or(0.0);
    Ok(El {
        id,
        text: str_of(v, "text"),
        role: str_of(v, "role"),
        frac: Frac {
            x: n("x"),
            y: n("y"),
            w: n("w"),
            h: n("h"),
        },
        source: str_of(v, "source"),
    })
}

fn parse_els(v: &Value) -> Result<Vec<El>, DcoError> {
    v.as_array()
        .map(|a| a.iter().map(parse_el).collect())
        .unwrap_or_else(|| Ok(Vec::new()))
}

pub fn parse_look(r: Reply) -> Result<Look, DcoError> {
    let j = &r.json;
    let snapshot_id = j["snapshot_id"]
        .as_str()
        .ok_or_else(|| bad("dco 的 see 回复里没有 snapshot_id"))?
        .to_string();
    Ok(Look {
        snapshot_id,
        observation_id: opt_str(j, "observation_id"),
        scene_hash: opt_str(j, "scene_hash"),
        elements: parse_els(&j["elements"])?,
        marks_png: r.image_png_base64,
    })
}

pub fn parse_run(r: Reply) -> Result<RunReply, DcoError> {
    let j = r.json;
    let run_id = j["run_id"]
        .as_str()
        .ok_or_else(|| bad("dco 的流程回复里没有 run_id"))?
        .to_string();
    let status = j["status"]
        .as_str()
        .ok_or_else(|| bad("dco 的流程回复里没有 status"))?
        .to_string();
    let help = match j.get("help") {
        Some(h) if h.is_object() => Some(Help {
            snapshot_id: str_of(h, "snapshot_id"),
            intent: str_of(h, "intent"),
            question: str_of(h, "question"),
            elements: parse_els(&h["elements"])?,
            picture_png: r.image_png_base64,
        }),
        _ => None,
    };
    let learned_keys = j["learned"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e["key"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(RunReply {
        run_id,
        status,
        at_step: j["at_step"].as_u64().unwrap_or(0) as u32,
        note: opt_str(&j, "note"),
        help,
        learned_keys,
        raw: j,
    })
}

#[cfg(unix)]
fn parse_windows(v: &Value) -> Result<Vec<WindowRow>, DcoError> {
    let list = v["windows"]
        .as_array()
        .ok_or_else(|| bad("dco 的 list_windows 回复里没有 windows"))?;
    list.iter()
        .map(|w| {
            Ok(WindowRow {
                window_id: w["window_id"]
                    .as_u64()
                    .ok_or_else(|| bad("窗口没有 window_id"))?,
                app: str_of(w, "app"),
                title: str_of(w, "title"),
            })
        })
        .collect()
}

#[cfg(unix)]
use serde_json::json;

#[cfg(unix)]
impl Hands for dct_dco::DcoClient {
    fn list_windows(&mut self) -> Result<Vec<WindowRow>, DcoError> {
        parse_windows(&self.call("list_windows", json!({}))?)
    }
    fn look(&mut self, window_id: u64, marks: bool) -> Result<Look, DcoError> {
        let mut args = json!({"window": {"window_id": window_id}, "scene_hash": true});
        if marks {
            args["marks"] = json!(true);
            args["image_format"] = json!("jpeg");
        }
        parse_look(self.call_full("see", args)?)
    }
    fn run_step(
        &mut self,
        window_id: u64,
        step: &StepReq,
        params: &Params,
        popups: &[Value],
    ) -> Result<RunReply, DcoError> {
        let mut s = json!({"action": step.action, "arg": step.arg});
        if let Some(d) = &step.done_when {
            s["done_when"] = json!(d);
        }
        parse_run(self.call_full(
            "run_step",
            json!({"window": {"window_id": window_id}, "step": s, "params": params, "popups": popups}),
        )?)
    }
    fn run_procedure(
        &mut self,
        window_id: u64,
        procedure: &Value,
        params: &Params,
        popups: &[Value],
        stop_before_step: Option<u32>,
    ) -> Result<RunReply, DcoError> {
        let mut a = json!({"procedure": procedure, "window": {"window_id": window_id}, "params": params, "popups": popups});
        if let Some(n) = stop_before_step {
            a["stop_before_step"] = json!(n);
        }
        parse_run(self.call_full("run_procedure", a)?)
    }
    fn answer_run(&mut self, run_id: &str, a: &HelpAnswer) -> Result<RunReply, DcoError> {
        let args = match a {
            HelpAnswer::Element {
                id,
                by,
                name,
                dismiss,
            } => {
                let mut v = json!({"run_id": run_id, "element_id": id,
                    "by": if *by == By::User { "user" } else { "model" }});
                if let Some(n) = name {
                    v["name"] = json!(n);
                }
                if *dismiss {
                    v["dismiss"] = json!(true);
                }
                v
            }
            HelpAnswer::NotThere => json!({"run_id": run_id, "none": true}),
            HelpAnswer::GiveUp => json!({"run_id": run_id, "give_up": true}),
        };
        parse_run(self.call_full("answer_run", args)?)
    }
    fn cancel_run(&mut self, run_id: &str) -> Result<(), DcoError> {
        self.call("cancel_run", json!({"run_id": run_id}))
            .map(|_| ())
    }
    fn confirm_learned(&mut self, accept: &[String], reject: &[String]) -> Result<(), DcoError> {
        self.call(
            "confirm_learned",
            json!({"accept": accept, "reject": reject}),
        )
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(j: Value, img: Option<&str>) -> Reply {
        Reply {
            json: j,
            image_png_base64: img.map(str::to_string),
        }
    }

    #[test]
    fn parse_run_needs_run_id_and_status() {
        let e = parse_run(reply(json!({"status": "done"}), None)).unwrap_err();
        assert_eq!(e.code, "dco_error");
        let e = parse_run(reply(json!({"run_id": "r"}), None)).unwrap_err();
        assert_eq!(e.code, "dco_error");
    }

    #[test]
    fn parse_look_keeps_the_marks_picture_and_needs_a_snapshot() {
        let l = parse_look(reply(
            json!({"snapshot_id": "s", "elements": []}),
            Some("PNG"),
        ))
        .unwrap();
        assert_eq!(l.marks_png.as_deref(), Some("PNG"));
        assert!(parse_look(reply(json!({"elements": []}), None)).is_err());
    }

    #[test]
    fn parse_run_without_help_has_no_picture_attached() {
        let r = parse_run(reply(
            json!({"run_id": "r", "status": "done", "learned": [{"key": "a"}, {"key": "b"}]}),
            Some("X"),
        ))
        .unwrap();
        assert!(r.help.is_none());
        assert_eq!(r.learned_keys, ["a", "b"]);
    }
}
