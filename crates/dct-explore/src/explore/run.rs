//! 一次摸索的账本：轨迹、进度、每个画面的失败次数、上一次站在某个画面时的样子。
use super::screen::{Screen, Scrub};
use super::{Clock, ExploreInput, Outcome};
use crate::gate::Meter;
use crate::hands::{El, Frac, StepReq};
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
    pending: Option<String>,
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
    /// 参数 → 记下的「填字」选项（字打进去以后发帖框变成值元素，不能再按它的字去点；
    /// 字被清掉要再填时，用打字前的提示字当意图名）。执行前先记（打字失败也还能再填），
    /// 起作用的那个覆盖；修补时从开头的 `tap_by_intent` → `type_param` 记。
    pub(super) fills: HashMap<String, Opt>,
    /// 点过发布、落到的新画面上还有发布按钮（平台可能还在等确认，见 `Landed::Pending`）：
    /// 第一下发布的字。这时上限（时间、步数）不算，点什么都算发布那一类，先问用户。
    pub(super) pending: Option<String>,
    /// `pending` 期间做过的动作数（防止绕个没完，见 `MAX_AFTER_PUBLISH`）。
    pub(super) after_publish: u32,
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
        let pending = self.pending.clone();
        self.snaps
            .insert(key.to_string(), Snap { ok, prog, pending });
    }
    /// 绕回见过的画面：轨迹的 `ok` 和进度恢复成上一次站在那儿时的样子。
    pub(super) fn back_to(&mut self, key: &str) {
        if let Some(s) = self.snaps.get(key) {
            for (i, t) in self.trace.steps.iter_mut().enumerate() {
                t.ok = s.ok.get(i).copied().unwrap_or(false);
            }
            self.prog = copy_prog(&s.prog);
            self.pending = s.pending.clone();
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
    /// 停下。点过发布、还没看到发出去时说清楚：帖子可能没发出去。
    pub(super) fn stop(self, why: String) -> Outcome {
        let why = match &self.pending {
            Some(w) => format!("点过「{w}」，但还没看到发出去（平台可能还在等确认）：{why}"),
            None => why,
        };
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
            outcome: None,
        },
        meter: Meter::new(clock.now_ms()),
        snaps: HashMap::new(),
        tries: HashMap::new(),
        fills: HashMap::new(),
        pending: None,
        after_publish: 0,
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

/// 要执行一个「填字」选项了：先记下（还没记过时），打字失败、发帖框变成值元素后也还能再填。
pub(super) fn remember_fill(r: &mut Run, opt: &Opt) {
    if let Act::Fill { param, .. } = &opt.act {
        r.fills.entry(param.clone()).or_insert_with(|| opt.clone());
    }
}

/// 修补：开头打过字的参数算打过（字可能还在画面上），打字前点的那个意图记成这个参数的「填字」选项。
pub(super) fn seed_from_prefix(r: &mut Run, input: &ExploreInput, scrub: &Scrub) {
    let steps = &r.trace.steps;
    for (i, t) in steps.iter().enumerate() {
        if t.action != "type_param" {
            continue;
        }
        scrub.mark_typed(&t.arg);
        if !input.text_params.contains(&t.arg) {
            continue;
        }
        let Some(tap) = i
            .checked_sub(1)
            .map(|j| &steps[j])
            .filter(|s| s.action == "tap_by_intent")
        else {
            continue;
        };
        let intent = scrub.line(&tap.arg).trim().to_string();
        if intent.is_empty() || scrub.has_placeholder(&intent) {
            continue;
        }
        let param = t.arg.clone();
        let field = El {
            id: format!("prefix_{param}"),
            text: intent.clone(),
            role: "textarea".into(),
            frac: Frac {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            source: "procedure".into(),
        };
        r.fills.insert(
            param.clone(),
            Opt {
                key: format!("fill_{param}_again"),
                text: format!("在「{intent}」里填 {{{param}}}"),
                act: Act::Fill {
                    param,
                    field,
                    intent,
                },
                publish: false,
            },
        );
    }
}

/// 一个选项起作用了：更新进度。
pub(super) fn advance(r: &mut Run, opt: &Opt) {
    if let Act::Fill { param, .. } = &opt.act {
        r.fills.insert(param.clone(), opt.clone());
    }
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
