//! 假手：按脚本回放 `Look` / `RunReply`，记录每次调用。后面所有任务的测试都用它。
use crate::hands::*;
use dct_dco::DcoError;
use serde_json::Value;
use std::collections::VecDeque;

/// 一次 run_step 发出去的全部内容。
#[derive(Debug, Clone)]
pub struct SentStep {
    pub window_id: u64,
    pub step: StepReq,
    pub params: Params,
    pub popups: Vec<Value>,
}

#[derive(Default)]
pub struct FakeHands {
    pub windows: Vec<WindowRow>,
    pub looks: VecDeque<Result<Look, DcoError>>,
    pub steps: VecDeque<Result<RunReply, DcoError>>,
    pub answers: VecDeque<Result<RunReply, DcoError>>,
    /// run_procedure 的脚本回复
    pub procedures: VecDeque<Result<RunReply, DcoError>>,
    /// 每次调用一行可读的字符串。
    pub calls: Vec<String>,
    pub sent_steps: Vec<StepReq>,
    /// 同上，另带窗口、参数、弹窗规则。
    pub step_calls: Vec<SentStep>,
    /// 每次 look 的窗口。
    pub look_windows: Vec<u64>,
    /// 每次 run_procedure 的窗口。
    pub procedure_windows: Vec<u64>,
    pub sent_answers: Vec<(String, HelpAnswer)>,
    pub sent_procedures: Vec<(Value, Params, Vec<Value>, Option<u32>)>,
}

impl FakeHands {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push_look(&mut self, l: Look) -> &mut Self {
        self.looks.push_back(Ok(l));
        self
    }
    pub fn push_look_err(&mut self, code: &str) -> &mut Self {
        self.looks.push_back(Err(err(code)));
        self
    }
    pub fn push_step(&mut self, r: RunReply) -> &mut Self {
        self.steps.push_back(Ok(r));
        self
    }
    pub fn push_step_err(&mut self, code: &str) -> &mut Self {
        self.steps.push_back(Err(err(code)));
        self
    }
    pub fn push_answer(&mut self, r: RunReply) -> &mut Self {
        self.answers.push_back(Ok(r));
        self
    }
    pub fn push_procedure(&mut self, r: RunReply) -> &mut Self {
        self.procedures.push_back(Ok(r));
        self
    }
}

fn err(code: &str) -> DcoError {
    DcoError {
        code: code.into(),
        message: format!("fake {code}"),
        run_id: None,
    }
}

fn list(v: &[String]) -> String {
    format!("[{}]", v.join(","))
}

impl Hands for FakeHands {
    fn list_windows(&mut self) -> Result<Vec<WindowRow>, DcoError> {
        self.calls.push("list_windows".into());
        Ok(self.windows.clone())
    }
    fn look(&mut self, window_id: u64, marks: bool) -> Result<Look, DcoError> {
        self.look_windows.push(window_id);
        self.calls.push(format!("look marks={marks}"));
        self.looks
            .pop_front()
            .expect("FakeHands: look 被调用的次数比脚本里排的 looks 多")
    }
    fn run_step(
        &mut self,
        window_id: u64,
        step: &StepReq,
        params: &Params,
        popups: &[Value],
    ) -> Result<RunReply, DcoError> {
        self.calls
            .push(format!("run_step {}: {}", step.action, step.arg));
        self.sent_steps.push(step.clone());
        self.step_calls.push(SentStep {
            window_id,
            step: step.clone(),
            params: params.clone(),
            popups: popups.to_vec(),
        });
        self.steps
            .pop_front()
            .expect("FakeHands: run_step 被调用的次数比脚本里排的 steps 多")
    }
    fn run_procedure(
        &mut self,
        window_id: u64,
        procedure: &Value,
        params: &Params,
        popups: &[Value],
        stop_before_step: Option<u32>,
    ) -> Result<RunReply, DcoError> {
        self.calls.push(match stop_before_step {
            Some(n) => format!("run_procedure stop_before={n}"),
            None => "run_procedure".into(),
        });
        self.procedure_windows.push(window_id);
        self.sent_procedures.push((
            procedure.clone(),
            params.clone(),
            popups.to_vec(),
            stop_before_step,
        ));
        self.procedures
            .pop_front()
            .expect("FakeHands: run_procedure 被调用的次数比脚本里排的 procedures 多")
    }
    fn answer_run(&mut self, run_id: &str, a: &HelpAnswer) -> Result<RunReply, DcoError> {
        self.calls.push(match a {
            HelpAnswer::Element { id, by, .. } => format!(
                "answer_run {run_id} element {id} by={}",
                if *by == By::User { "user" } else { "model" }
            ),
            HelpAnswer::NotThere => format!("answer_run {run_id} none"),
            HelpAnswer::GiveUp => format!("answer_run {run_id} give_up"),
        });
        self.sent_answers.push((run_id.to_string(), a.clone()));
        self.answers
            .pop_front()
            .expect("FakeHands: answer_run 被调用的次数比脚本里排的 answers 多")
    }
    fn cancel_run(&mut self, run_id: &str) -> Result<(), DcoError> {
        self.calls.push(format!("cancel_run {run_id}"));
        Ok(())
    }
    fn confirm_learned(&mut self, accept: &[String], reject: &[String]) -> Result<(), DcoError> {
        let mut s = String::from("confirm_learned");
        if !accept.is_empty() {
            s += &format!(" accept={}", list(accept));
        }
        if !reject.is_empty() {
            s += &format!(" reject={}", list(reject));
        }
        self.calls.push(s);
        Ok(())
    }
}

/// 脚本里常用的两个构造。
pub fn reply(run_id: &str, status: &str, at_step: u32) -> RunReply {
    RunReply {
        run_id: run_id.into(),
        status: status.into(),
        at_step,
        note: None,
        help: None,
        learned_keys: vec![],
        raw: Value::Null,
    }
}

pub fn look(snapshot_id: &str, elements: Vec<El>) -> Look {
    Look {
        snapshot_id: snapshot_id.into(),
        observation_id: None,
        scene_hash: None,
        elements,
        marks_png: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_learned_call_strings() {
        let mut h = FakeHands::default();
        h.confirm_learned(&["k1".into()], &[]).unwrap();
        h.confirm_learned(&[], &["k1".into(), "k2".into()]).unwrap();
        h.confirm_learned(&["a".into()], &["b".into()]).unwrap();
        assert_eq!(
            h.calls,
            [
                "confirm_learned accept=[k1]",
                "confirm_learned reject=[k1,k2]",
                "confirm_learned accept=[a] reject=[b]"
            ]
        );
    }

    #[test]
    fn scripted_look_error_comes_back() {
        let mut h = FakeHands::default();
        h.push_look_err("stale");
        assert_eq!(h.look(1, false).unwrap_err().code, "stale");
        assert_eq!(h.calls, ["look marks=false"]);
    }
}
