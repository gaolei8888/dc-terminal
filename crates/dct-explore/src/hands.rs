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
        run_id: None,
    }
}

fn str_of(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

fn opt_str(v: &Value, k: &str) -> Option<String> {
    v[k].as_str().map(str::to_string)
}

/// dco 的 `see` 给元素带 `frac`；`help.elements` 是原始元素，只有 `bounds`（窗口内的点，左上角为原点）。
/// 有 frac 用 frac；否则用 bounds 除以窗口大小；两样都没有就报错，不悄悄给 0。
fn parse_el(v: &Value, size: Option<(f64, f64)>) -> Result<El, DcoError> {
    let id = v["id"]
        .as_str()
        .ok_or_else(|| bad("dco 回的元素没有 id"))?
        .to_string();
    let frac = if v["frac"].is_object() {
        let f = &v["frac"];
        let n = |k: &str| f[k].as_f64().unwrap_or(0.0);
        Frac {
            x: n("x"),
            y: n("y"),
            w: n("w"),
            h: n("h"),
        }
    } else if v["bounds"].is_object() {
        let (ww, wh) = size
            .filter(|(w, h)| *w > 0.0 && *h > 0.0)
            .ok_or_else(|| bad(format!("元素 {id} 只有 bounds，但不知道窗口大小")))?;
        let b = &v["bounds"];
        let n = |k: &str| b[k].as_f64().unwrap_or(0.0);
        Frac {
            x: n("x") / ww,
            y: n("y") / wh,
            w: n("w") / ww,
            h: n("h") / wh,
        }
    } else {
        return Err(bad(format!("元素 {id} 既没有 frac 也没有 bounds")));
    };
    Ok(El {
        id,
        text: str_of(v, "text"),
        role: str_of(v, "role"),
        frac,
        source: str_of(v, "source"),
    })
}

fn parse_els(v: &Value, size: Option<(f64, f64)>) -> Result<Vec<El>, DcoError> {
    v.as_array()
        .ok_or_else(|| bad("dco 回复里的 elements 不是数组"))?
        .iter()
        .map(|e| parse_el(e, size))
        .collect()
}

/// `see` 回复里的窗口大小（点）：`window.w` / `window.h`。
pub fn window_size_of_look(j: &Value) -> Option<(f64, f64)> {
    Some((j["window"]["w"].as_f64()?, j["window"]["h"].as_f64()?))
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
        elements: parse_els(&j["elements"], None)?,
        marks_png: r.image_png_base64,
    })
}

/// `size`：这个窗口的大小（点）。help 里的元素只有 bounds 时要用它换成 frac。
pub fn parse_run(r: Reply, size: Option<(f64, f64)>) -> Result<RunReply, DcoError> {
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
            elements: match h.get("elements") {
                Some(e) if !e.is_null() => parse_els(e, size)?,
                _ => Vec::new(),
            },
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

/// 真的 dco：包一层 `DcoClient`，记住每个窗口的大小和每个 run 在哪个窗口，
/// 好把 help 里只有 bounds 的元素换成 frac。
#[cfg(unix)]
pub struct DcoHands {
    client: dct_dco::DcoClient,
    sizes: std::collections::HashMap<u64, (f64, f64)>,
    run_windows: std::collections::HashMap<String, u64>,
}

#[cfg(unix)]
impl DcoHands {
    pub fn new(client: dct_dco::DcoClient) -> Self {
        DcoHands {
            client,
            sizes: Default::default(),
            run_windows: Default::default(),
        }
    }

    pub fn client(&mut self) -> &mut dct_dco::DcoClient {
        &mut self.client
    }

    /// 窗口大小：先看记下的（来自 see），不知道就问 list_windows（`size.w/h`）。
    fn size_of(&mut self, window_id: u64) -> Result<Option<(f64, f64)>, DcoError> {
        if let Some(s) = self.sizes.get(&window_id) {
            return Ok(Some(*s));
        }
        let j = self.client.call("list_windows", json!({}))?;
        for w in j["windows"].as_array().into_iter().flatten() {
            if let (Some(id), Some(ww), Some(wh)) = (
                w["window_id"].as_u64(),
                w["size"]["w"].as_f64(),
                w["size"]["h"].as_f64(),
            ) {
                self.sizes.insert(id, (ww, wh));
            }
        }
        Ok(self.sizes.get(&window_id).copied())
    }

    /// 解析一次流程回复；help 里有元素才需要窗口大小。记下 run_id → 窗口。
    fn finish_run(&mut self, window_id: Option<u64>, r: Reply) -> Result<RunReply, DcoError> {
        let needs_size = r.json["help"]["elements"]
            .as_array()
            .is_some_and(|a| !a.is_empty());
        let size = match (needs_size, window_id) {
            (true, Some(w)) => self.size_of(w)?,
            _ => None,
        };
        let run = parse_run(r, size)?;
        if let Some(w) = window_id {
            self.run_windows.insert(run.run_id.clone(), w);
        }
        Ok(run)
    }
}

#[cfg(unix)]
impl Hands for DcoHands {
    fn list_windows(&mut self) -> Result<Vec<WindowRow>, DcoError> {
        let j = self.client.call("list_windows", json!({}))?;
        for w in j["windows"].as_array().into_iter().flatten() {
            if let (Some(id), Some(ww), Some(wh)) = (
                w["window_id"].as_u64(),
                w["size"]["w"].as_f64(),
                w["size"]["h"].as_f64(),
            ) {
                self.sizes.insert(id, (ww, wh));
            }
        }
        parse_windows(&j)
    }
    fn look(&mut self, window_id: u64, marks: bool) -> Result<Look, DcoError> {
        let mut args = json!({"window": {"window_id": window_id}, "scene_hash": true});
        if marks {
            args["marks"] = json!(true);
            args["image_format"] = json!("jpeg");
        }
        let r = self.client.call_full("see", args)?;
        if let Some(s) = window_size_of_look(&r.json) {
            self.sizes.insert(window_id, s);
        }
        parse_look(r)
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
        let r = self.client.call_full(
            "run_step",
            json!({"window": {"window_id": window_id}, "step": s, "params": params, "popups": popups}),
        )?;
        self.finish_run(Some(window_id), r)
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
        let r = self.client.call_full("run_procedure", a)?;
        self.finish_run(Some(window_id), r)
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
        let r = self.client.call_full("answer_run", args)?;
        let w = self.run_windows.get(run_id).copied();
        self.finish_run(w, r)
    }
    fn cancel_run(&mut self, run_id: &str) -> Result<(), DcoError> {
        self.run_windows.remove(run_id);
        self.client
            .call("cancel_run", json!({"run_id": run_id}))
            .map(|_| ())
    }
    fn confirm_learned(&mut self, accept: &[String], reject: &[String]) -> Result<(), DcoError> {
        self.client
            .call(
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
        let e = parse_run(reply(json!({"status": "done"}), None), None).unwrap_err();
        assert_eq!(e.code, "dco_error");
        let e = parse_run(reply(json!({"run_id": "r"}), None), None).unwrap_err();
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
        let r = parse_run(
            reply(
                json!({"run_id": "r", "status": "done", "learned": [{"key": "a"}, {"key": "b"}]}),
                Some("X"),
            ),
            None,
        )
        .unwrap();
        assert!(r.help.is_none());
        assert_eq!(r.learned_keys, ["a", "b"]);
    }

    #[test]
    fn help_elements_with_bounds_become_fractions_of_the_window() {
        let j = json!({"run_id": "r", "status": "needs_help", "help": {"snapshot_id": "s", "intent": "i", "question": "q",
            "elements": [{"id": "e1", "text": "Post", "role": "button", "source": "ocr", "confidence": 0.9,
                          "bounds": {"x": 100.0, "y": 50.0, "w": 50.0, "h": 25.0}}]}});
        let r = parse_run(reply(j.clone(), None), Some((1000.0, 500.0))).unwrap();
        let f = &r.help.unwrap().elements[0].frac;
        assert_eq!((f.x, f.y, f.w, f.h), (0.1, 0.1, 0.05, 0.05));
        // 不知道窗口大小：报错，不给 0
        assert_eq!(
            parse_run(reply(j, None), None).unwrap_err().code,
            "dco_error"
        );
    }

    #[test]
    fn an_element_with_neither_frac_nor_bounds_is_an_error() {
        let j = json!({"snapshot_id": "s", "elements": [{"id": "e1", "text": "x"}]});
        assert!(parse_look(reply(j, None)).is_err());
    }

    #[test]
    fn a_see_reply_without_an_elements_array_is_an_error() {
        assert!(parse_look(reply(json!({"snapshot_id": "s"}), None)).is_err());
        assert!(parse_look(reply(json!({"snapshot_id": "s", "elements": "x"}), None)).is_err());
    }

    #[test]
    fn window_size_comes_from_the_see_reply() {
        let j = json!({"window": {"w": 1200.0, "h": 800.0}});
        assert_eq!(window_size_of_look(&j), Some((1200.0, 800.0)));
        assert_eq!(window_size_of_look(&json!({})), None);
    }
}
