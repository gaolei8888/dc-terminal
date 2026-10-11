//! 一次摸索的账本：轨迹、进度、每个画面的失败次数、上一次站在某个画面时的样子。
use super::screen::Screen;
use super::{Clock, ExploreInput, Outcome};
use crate::gate::Meter;
use crate::hands::StepReq;
use crate::ladder::{Decision, Tries, Who};
use crate::options::{Act, Opt, Progress};
use crate::trace::{Trace, TraceStep};
use std::collections::HashMap;

/// 一个画面上的失败记录。
#[derive(Default)]
struct ScreenTries {
    tries: Tries,
    /// 这个画面上失败过的 Jeff 答案（训练题里算「被纠正的」那个）
    failed_jeff: Option<(String, f64)>,
}

/// 上一次站在某个画面时的样子：轨迹里每步的 `ok`，和进度。
struct Snap {
    ok: Vec<bool>,
    prog: Progress,
}

fn copy_prog(p: &Progress) -> Progress {
    Progress {
        filled: p.filled.clone(),
        attached: p.attached.clone(),
        published: p.published,
        done_lines: p.done_lines.clone(),
    }
}

pub(super) struct Run {
    pub(super) trace: Trace,
    pub(super) prog: Progress,
    pub(super) meter: Meter,
    /// 这次运行里看到过的画面指纹 → 上一次站在那儿时的样子
    snaps: HashMap<String, Snap>,
    /// 画面指纹 → 在那个画面上选的、没起作用的次数
    tries: HashMap<String, ScreenTries>,
}

impl Run {
    pub(super) fn trail(&self) -> Vec<String> {
        self.trace
            .steps
            .iter()
            .map(|s| {
                let miss = if s.ok { "" } else { "（没起作用）" };
                format!("{}: {}{miss}", s.action, s.arg)
            })
            .collect()
    }
    /// 站到这个画面上了：记下此刻的样子，以后绕回来时恢复。
    pub(super) fn arrive(&mut self, key: &str) {
        let ok = self.trace.steps.iter().map(|s| s.ok).collect();
        let prog = copy_prog(&self.prog);
        self.snaps.insert(key.to_string(), Snap { ok, prog });
    }
    /// 绕回见过的画面：轨迹的 `ok` 和进度恢复成上一次站在那儿时的样子。
    pub(super) fn back_to(&mut self, key: &str) {
        if let Some(s) = self.snaps.get(key) {
            for (i, t) in self.trace.steps.iter_mut().enumerate() {
                t.ok = s.ok.get(i).copied().unwrap_or(false);
            }
            self.prog = copy_prog(&s.prog);
        }
    }
    /// 在 `key` 这个画面上选的那一下没起作用。
    pub(super) fn charge(&mut self, key: &str, dec: &Decision) {
        let t = self.tries.entry(key.to_string()).or_default();
        match dec.by {
            Who::Jeff => {
                t.tries.jeff_failed = true;
                t.failed_jeff = dec.jeff.clone().or(Some((dec.key.clone(), 1.0)));
            }
            Who::Model => t.tries.model_tries += 1,
            Who::User => {}
        }
    }
    /// 这次运行里站到过这个画面。
    pub(super) fn seen(&self, key: &str) -> bool {
        self.snaps.contains_key(key)
    }
    /// 这个画面上的失败次数（逐级往上用）。
    pub(super) fn tries_at(&mut self, key: &str) -> &Tries {
        &self.tries.entry(key.to_string()).or_default().tries
    }
    pub(super) fn failed_jeff(&self, key: &str) -> Option<(String, f64)> {
        self.tries.get(key).and_then(|t| t.failed_jeff.clone())
    }
    pub(super) fn stop(self, why: String) -> Outcome {
        Outcome::Stopped {
            trace: self.trace,
            why,
        }
    }
}

pub(super) fn plain_step(s: &StepReq, by: Who, learned: Vec<String>) -> TraceStep {
    TraceStep {
        action: s.action.clone(),
        arg: s.arg.clone(),
        by,
        obs_before: None,
        obs_after: None,
        scene_before: None,
        scene_after: None,
        ok: true,
        publish: false,
        vanished: vec![],
        learned_keys: learned,
        from_procedure: false,
    }
}

/// 修补时轨迹开头已经做过的事算进进度：选过的文件算已附上，填过的字算已填
/// （字要是不在画面上，第一轮就会被拿掉，照样再填）。
fn progress_of(prefix: &[TraceStep], input: &ExploreInput) -> Progress {
    let did = |action: &str, list: &[String]| -> Vec<String> {
        list.iter()
            .filter(|p| prefix.iter().any(|s| s.action == action && s.arg == **p))
            .cloned()
            .collect()
    };
    Progress {
        filled: did("type_param", &input.text_params),
        attached: did("pick_file", &input.file_params),
        published: false,
        done_lines: vec![],
    }
}

pub(super) fn new_run(input: &ExploreInput, clock: &dyn Clock, prefix: Vec<TraceStep>) -> Run {
    Run {
        prog: progress_of(&prefix, input),
        trace: Trace {
            run_id: input.run_id.clone(),
            platform: input.platform.name.clone(),
            params: input
                .text_params
                .iter()
                .chain(&input.file_params)
                .cloned()
                .collect(),
            steps: prefix,
            first_lines: vec![],
        },
        meter: Meter::new(clock.now_ms()),
        snaps: HashMap::new(),
        tries: HashMap::new(),
    }
}

/// 把执行过的步骤记进轨迹；`ok` = 这几步在通往当前画面的路上。
pub(super) fn record(
    r: &mut Run,
    cur: &Screen,
    post: &Screen,
    opt: &Opt,
    dec: &Decision,
    steps: Vec<(StepReq, Vec<String>)>,
    ok: bool,
) {
    let n = steps.len();
    let vanished: Vec<String> =
        cur.lines
            .iter()
            .filter(|l| !post.lines.contains(l))
            .fold(Vec::new(), |mut v, l| {
                if !v.contains(l) {
                    v.push(l.clone());
                }
                v
            });
    for (i, (s, learned)) in steps.into_iter().enumerate() {
        let (first, last) = (i == 0, i + 1 == n);
        let mut t = plain_step(&s, dec.by, learned);
        t.ok = ok;
        if first {
            t.obs_before = cur.look.observation_id.clone();
            t.scene_before = cur.look.scene_hash.clone();
        }
        if last {
            t.obs_after = post.look.observation_id.clone();
            t.scene_after = post.look.scene_hash.clone();
            t.publish = opt.publish;
            t.vanished = vanished.clone();
        }
        r.trace.steps.push(t);
    }
}

/// 一个选项起作用了：更新进度。
pub(super) fn advance(r: &mut Run, opt: &Opt) {
    match &opt.act {
        Act::Fill { param, .. } if !r.prog.filled.contains(param) => {
            r.prog.filled.push(param.clone())
        }
        Act::Attach { param } if !r.prog.attached.contains(param) => {
            r.prog.attached.push(param.clone())
        }
        _ => {}
    }
    if opt.publish {
        r.prog.published = true;
    }
}
