//! 摸索循环：把看、规矩、选项、逐级往上、执行、核对接起来。全部经 `Hands`，对着 `FakeHands` 测。
//!
//! 循环：
//! 1. （`explore`）`run_step open_url <platform.entry_url>`；不是 `done` → `Stopped`。网址只取自平台表。
//! 2. `look`（带 scene_hash）。先在内存里算好每个文字参数在不在画面上，然后把元素字里的参数值
//!    `redact` 掉，之后才交给任何人。`gate::screen_halt` → `Halted`（不调 Jeff、不调大模型、
//!    不问用户选项，只把命中的那行字交给上层）。第一次看到的画面记进 `trace.first_lines`。
//! 3. `Meter::over` → `Stopped`。
//! 4. 选项（`all_options` / `shortlist`）→ `Ladder::decide`；编号图只在真要问大模型或用户时才
//!    `look(marks: true)` 去拿（见 `act::decide`）。用户说停 → `Stopped`；用户给的 key 不在选项里 → `Stopped`，不执行。
//! 5. `Done` → `Published`；`Stuck` → `Stopped`。
//! 6. 选中的是发布：这次还没问过 → `Confirm::publish_ok`；不 → `DeclinedAtPublish`。
//!    平台 `publish_by_user_only`：dct 不点，请用户自己点，点完说「是」，轨迹记成用户点的。
//! 7. `steps_for` → 逐步 `run_step`（`act::execute`：求助、拦下、失败的处理都在那里）。
//! 8. 执行后 `look`：scene_hash 和执行前一样、或是这次运行里见过的画面 → 失败，`Tries` 记一笔，回到 4。
//!    变了 → `Tries` 清零，记 `TraceStep`、更新 `Progress`、写一行训练题。
//! 9. 填过的参数下一轮在画面上看不到了 → 从 `filled` 里拿掉（会再填一次，发布按钮也会先消失）。
//! 10. 窗口没了（`look` 回 `stale` / `not_found`）→ `Stopped`。
mod act;

use crate::gate::{screen_halt, Budget, Halt, Meter, Over};
use crate::hands::{Hands, Params, StepReq};
use crate::ladder::{Decision, Ladder, Tries, Who};
use crate::options::{all_options, shortlist, state_text, Act, Goal, Opt, Progress};
use crate::platform::Platform;
use crate::trace::{Trace, TraceStep};
pub use act::values_on_screen;
use act::{look_error, see, write_row, Screen, Scrub};

pub struct ExploreInput<'a> {
    pub platform: &'a Platform,
    pub goal_text: &'a str,
    /// 本次的值（只在内存里）
    pub params: Params,
    pub text_params: Vec<String>,
    pub file_params: Vec<String>,
    pub window_id: u64,
    /// dcv popups --app <platform> --json 里已生效的
    pub popups: Vec<serde_json::Value>,
    pub budget: Budget,
    pub run_id: String,
    /// ~/.dct/explore
    pub data_dir: std::path::PathBuf,
}

pub trait Confirm {
    fn publish_ok(&mut self, picture_png: Option<&str>, summary: &str) -> bool;
}

pub trait Clock {
    fn now_ms(&self) -> u64;
}

#[derive(Debug)]
pub enum Outcome {
    /// 成功：发出去了（或流程走完）
    Published(Trace),
    /// 停在发布前，用户说不发
    DeclinedAtPublish(Trace),
    /// 登录 / 验证码 / 警告
    Halted { trace: Trace, why: Halt },
    /// 上限、用户说停、dco blocked / private_screen / 窗口没了
    Stopped { trace: Trace, why: String },
}

fn over_text(o: Over, b: &Budget) -> String {
    match o {
        Over::Steps => format!("已经走了 {} 步，到上限了", b.max_steps),
        Over::Time => format!("摸索超过 {} 分钟，到上限了", b.max_ms / 60_000),
        Over::Unchanged => format!("连续 {} 步画面没变，停下", b.max_unchanged),
    }
}

struct Run {
    trace: Trace,
    prog: Progress,
    meter: Meter,
    /// 这次运行里看到过的画面指纹
    seen: Vec<String>,
    asked_publish: bool,
    tries: Tries,
    tries_at: Option<String>,
    /// 这个画面上失败过的 Jeff 答案（训练题里算「被纠正的」那个）
    failed_jeff: Option<(String, f64)>,
}

impl Run {
    fn trail(&self) -> Vec<String> {
        self.trace
            .steps
            .iter()
            .map(|s| format!("{}: {}", s.action, s.arg))
            .collect()
    }
    fn stop(self, why: String) -> Outcome {
        Outcome::Stopped {
            trace: self.trace,
            why,
        }
    }
}

fn plain_step(s: &StepReq, by: Who, learned: Vec<String>) -> TraceStep {
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

pub fn explore(
    h: &mut dyn Hands,
    lad: &mut Ladder,
    confirm: &mut dyn Confirm,
    clock: &dyn Clock,
    input: ExploreInput,
) -> Outcome {
    let open = StepReq {
        action: "open_url".into(),
        arg: input.platform.entry_url.clone(),
        done_when: None,
    };
    let mut first = Vec::new();
    match h.run_step(input.window_id, &open, &input.params, &input.popups) {
        Ok(r) if act::is_done(&r.status) => {
            // 网址是平台表定的，不是哪个模型选的：记成用户（平台表是用户的数据）。
            first.push(plain_step(&open, Who::User, r.learned_keys));
        }
        Ok(r) => {
            let scrub = Scrub::new(&input.params);
            let why = format!("打不开 {}：{}", open.arg, act::describe(&r, &scrub));
            return new_run(&input, clock, vec![]).stop(why);
        }
        Err(e) => return new_run(&input, clock, vec![]).stop(look_error(&e)),
    }
    explore_from(h, lad, confirm, clock, input, first)
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

fn new_run(input: &ExploreInput, clock: &dyn Clock, prefix: Vec<TraceStep>) -> Run {
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
        seen: vec![],
        asked_publish: false,
        tries: Tries::default(),
        tries_at: None,
        failed_jeff: None,
    }
}

/// 修补用：从当前画面接着摸索，已有的步骤作为轨迹开头（不再 open_url）。
pub fn explore_from(
    h: &mut dyn Hands,
    lad: &mut Ladder,
    confirm: &mut dyn Confirm,
    clock: &dyn Clock,
    input: ExploreInput,
    prefix: Vec<TraceStep>,
) -> Outcome {
    let mut r = new_run(&input, clock, prefix);
    let scrub = Scrub::new(&input.params);
    let goal_text = scrub.text(input.goal_text);
    let goal = Goal {
        text: &goal_text,
        platform: input.platform,
        text_params: &input.text_params,
        file_params: &input.file_params,
    };
    let answers = input.data_dir.join("answers.jsonl");
    let mut cur = match see(h, &input, &scrub) {
        Ok(s) => s,
        Err(why) => return r.stop(why),
    };
    loop {
        // 2. 停下的画面：什么都不问
        if let Some(why) = screen_halt(&cur.look.elements) {
            return Outcome::Halted {
                trace: r.trace,
                why,
            };
        }
        if r.trace.first_lines.is_empty() {
            r.trace.first_lines = cur.lines.clone();
        }
        r.seen.push(cur.key.clone());
        // 9. 字被清掉了 → 要再填
        r.prog
            .filled
            .retain(|p| cur.on_screen.get(p).copied().unwrap_or(false));
        // 3. 上限
        if let Some(o) = r.meter.over(&input.budget, clock.now_ms()) {
            return r.stop(over_text(o, &input.budget));
        }
        if r.tries_at.as_deref() != Some(cur.key.as_str()) {
            r.tries = Tries::default();
            r.failed_jeff = None;
            r.tries_at = Some(cur.key.clone());
        }
        // 4. 选项 → 逐级往上
        let on = |p: &str| cur.on_screen.get(p).copied().unwrap_or(false);
        let all = all_options(&cur.look, &goal, &r.prog, &on);
        let short = shortlist(&all, &goal);
        let state = state_text(&goal, &r.trail(), &cur.look);
        let row = |d: &Decision, fj| write_row(&answers, clock, &input, &state, &all, d, fj);
        let (dec, mut pic) =
            match act::decide(h, lad, &input, &scrub, &state, &short, &all, &r.tries) {
                act::Decided::Pick(d, pic) => (d, pic),
                act::Decided::Halt(why) => {
                    return Outcome::Halted {
                        trace: r.trace,
                        why,
                    }
                }
                act::Decided::UserStopped => return r.stop("你说停下".into()),
            };
        let Some(base) = all.iter().find(|o| o.key == dec.key) else {
            return r.stop(format!("选的「{}」不在选项里，没有执行", dec.key));
        };
        let mut opt = base.clone();
        if let Act::TapIcon { name, .. } = &mut opt.act {
            *name = dec
                .name
                .clone()
                .or(name.take())
                .filter(|n| !n.trim().is_empty());
            if name.is_none() {
                return r.stop("选了一个没字的图标，但没给它起名字，没法交给 dco".into());
            }
        }
        // 5. 做完 / 卡住
        match opt.act {
            Act::Done if r.prog.published => {
                row(&dec, r.failed_jeff.clone());
                return Outcome::Published(r.trace);
            }
            Act::Done => return r.stop("还没发布就说做完了".into()),
            Act::Stuck => return r.stop("卡住了，交给你".into()),
            _ => {}
        }
        // 6. 最终发布先问用户
        let by_user = opt.publish && input.platform.publish_by_user_only;
        if opt.publish && (by_user || !r.asked_publish) {
            if pic.is_none() {
                pic = match act::marks_now(h, &input, &scrub) {
                    Ok(p) => Some(p),
                    Err(why) => {
                        return Outcome::Halted {
                            trace: r.trace,
                            why,
                        }
                    }
                };
            }
            let word = match &opt.act {
                Act::Tap { intent, .. } => intent.clone(),
                _ => opt.text.clone(),
            };
            let summary = if by_user {
                format!(
                    "要发到 {} 了，内容已经填好。这个平台的发布按钮要你自己点：请在窗口里点「{word}」，点完回答「是」。",
                    input.platform.name
                )
            } else {
                format!("要发到 {} 了，内容已经填好，看一眼？", input.platform.name)
            };
            if !confirm.publish_ok(pic.flatten().as_deref(), &summary) {
                return Outcome::DeclinedAtPublish(r.trace);
            }
            r.asked_publish = true;
            if by_user {
                // R11：不调 run_step，轨迹记成用户点的发布，再看一眼画面接着走。
                let step = StepReq {
                    action: "tap_by_intent".into(),
                    arg: word,
                    done_when: None,
                };
                let post = match see(h, &input, &scrub) {
                    Ok(s) => s,
                    Err(why) => return r.stop(why),
                };
                let user_dec = Decision {
                    by: Who::User,
                    ..dec.clone()
                };
                r.meter.record(post.key != cur.key);
                row(&dec, r.failed_jeff.clone());
                succeed(&mut r, &cur, &post, &opt, &user_dec, vec![(step, vec![])]);
                cur = post;
                continue;
            }
        }
        // 7. 执行
        let done = match act::execute(h, &mut *lad.user, &input, &scrub, &state, &opt, dec.by) {
            act::Exec::Stop(why) => return r.stop(why),
            act::Exec::Failed => None,
            act::Exec::Ok(steps) => Some(steps),
        };
        // 8. 核对画面
        let post = match see(h, &input, &scrub) {
            Ok(s) => s,
            Err(why) => return r.stop(why),
        };
        r.meter.record(post.key != cur.key);
        match done {
            Some(steps) if !r.seen.contains(&post.key) => {
                row(&dec, r.failed_jeff.clone());
                succeed(&mut r, &cur, &post, &opt, &dec, steps);
            }
            _ => match dec.by {
                Who::Jeff => {
                    r.tries.jeff_failed = true;
                    r.failed_jeff = dec.jeff.clone().or(Some((dec.key.clone(), 1.0)));
                }
                Who::Model => r.tries.model_tries += 1,
                Who::User => {}
            },
        }
        cur = post;
    }
}

/// 一个选项起作用了：记轨迹、更新进度。
fn succeed(
    r: &mut Run,
    cur: &Screen,
    post: &Screen,
    opt: &Opt,
    dec: &Decision,
    steps: Vec<(StepReq, Vec<String>)>,
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
    r.tries = Tries::default();
    r.failed_jeff = None;
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
