//! 摸索循环：把看、规矩、选项、逐级往上、执行、核对接起来。全部经 `Hands`，对着 `FakeHands` 测。
//!
//! 循环：
//! 1. （`explore`）`run_step open_url <platform.entry_url>`；不是 `done` → `Stopped`。网址只取自平台表。
//! 2. `look`（带 scene_hash）。先在内存里算好每个文字参数在不在画面上，然后把元素字里的参数值
//!    换回 `{名字}`（整段、整行，以及文字识别拆开 / 改了大小写空格的片段，见 `screen::Scrub`），
//!    再把换行等控制字符压成空格，之后才交给任何人。`gate::screen_halt` → `Halted`（不调 Jeff、不调大模型、
//!    不问用户选项，只把命中的那行字交给上层）。第一次看到的画面记进 `trace.first_lines`。
//! 3. `Meter::over` → `Stopped`。
//! 4. 选项（`all_options` / `shortlist`）→ `Ladder::decide`；编号图只在真要问大模型或用户时才
//!    `look(marks: true)` 去拿（见 `act::decide`）。用户说停 → `Stopped`；用户给的 key 不在选项里 → `Stopped`，不执行。
//! 5. `Done` → `Published`；`Stuck` → `Stopped`。
//! 6. 选中的是发布：每一次都先 `Confirm::publish_ok`；不 → `DeclinedAtPublish`。
//!    平台 `publish_by_user_only`：dct 不点，请用户自己点，点完说「是」，轨迹记成用户点的。
//! 7. `steps_for` → 逐步 `run_step`（`act::execute`：求助、拦下、失败的处理都在那里；
//!    求助的画面也先过 `screen_halt`，命中就取消 dco 的运行 → `Halted`，不给任何人选）。
//! 8. 执行后 `look`：scene_hash 和执行前一样、或是这次运行里见过的画面 → 失败：记在**选它的那个画面**
//!    的 `Tries` 上（每个画面一份，绕回来也还在，所以 A↔B 来回绕时第二圈就交给大模型 / 用户）。
//!    变了（新画面）→ 记 `TraceStep`、更新 `Progress`、写一行训练题。
//!
//!    轨迹怎么记做了但没起作用的一下：照实记，`ok: false`。绕回见过的画面 S 时，轨迹里每步的 `ok`
//!    和 `Progress` 恢复成上一次站在 S 时的样子——从 S 出发绕一圈又回到 S 的那几步一起变成
//!    `ok: false`。所以任何时候 `ok` 的那几步正好是「从头走到当前画面」的路（草稿只抄 `ok` 的步骤），
//!    而做过的每一下都还在轨迹里。dco 回 failed 的那一下没有步骤可记，只记一笔失败。
//! 9. 填过的参数下一轮在画面上看不到了 → 从 `filled` 里拿掉（会再填一次，发布按钮也会先消失）。
//! 10. 窗口没了（`look` 回 `stale` / `not_found`）→ `Stopped`。
mod act;
pub(crate) mod raw;
mod run;
pub(crate) mod screen;

use crate::gate::{Budget, Halt, Over};
use crate::hands::{Hands, Params, StepReq};
use crate::ladder::{Decision, Ladder, Who};
use crate::options::{shortlist, state_text, Act, Goal};
use crate::platform::Platform;
use crate::trace::{Trace, TraceStep};
use act::write_row;
use run::{advance, new_run, plain_step, record};
pub use screen::values_on_screen;
use screen::{look_error, see, Scrub};

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
    let scrub = Scrub::new(&input.params);
    match h.run_step(input.window_id, &open, &input.params, &input.popups) {
        Ok(r) if act::is_done(&r.status) => {
            // 网址是平台表定的，不是哪个模型选的：记成用户（平台表是用户的数据）。
            first.push(plain_step(&open, Who::User, r.learned_keys));
        }
        Ok(r) => {
            let why = format!("打不开 {}：{}", open.arg, act::describe(&r, &scrub));
            return new_run(&input, clock, vec![]).stop(why);
        }
        Err(e) => return new_run(&input, clock, vec![]).stop(look_error(&e, &scrub)),
    }
    explore_from(h, lad, confirm, clock, input, first)
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
        if let Some(why) = cur.halt(&scrub) {
            return Outcome::Halted {
                trace: r.trace,
                why,
            };
        }
        if r.trace.first_lines.is_empty() {
            r.trace.first_lines = cur.lines.clone();
        }
        // 9. 字被清掉了 → 要再填
        r.prog
            .filled
            .retain(|p| cur.on_screen.get(p).copied().unwrap_or(false));
        r.arrive(&cur.key);
        // 3. 上限
        if let Some(o) = r.meter.over(&input.budget, clock.now_ms()) {
            return r.stop(over_text(o, &input.budget));
        }
        let fj = r.failed_jeff(&cur.key);
        // 4. 选项 → 逐级往上
        let on = |p: &str| cur.on_screen.get(p).copied().unwrap_or(false);
        let all = cur.options(&goal, &r.prog, &scrub, &on);
        let short = shortlist(&all, &goal);
        let state = state_text(&goal, &r.trail(), &cur.look);
        let row = |d: &Decision, fj| write_row(&answers, clock, &input, &state, &all, d, fj);
        let tries = r.tries_at(&cur.key);
        let (dec, mut pic) = match act::decide(h, lad, &input, &scrub, &state, &short, &all, tries)
        {
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
            // 名字会进轨迹、交给 dco：参数值一样换回 `{名字}`；换完只剩占位符的名字不能用。
            let given = dec.name.as_deref().map(|n| scrub.line(n));
            if given.as_deref().is_some_and(|n| scrub.has_placeholder(n)) {
                return r.stop("图标的名字里是要发的内容，不能当名字交给 dco，停下".into());
            }
            *name = given.or(name.take()).filter(|n| !n.trim().is_empty());
            if name.is_none() {
                return r.stop("选了一个没字的图标，但没给它起名字，没法交给 dco".into());
            }
        }
        // 5. 做完 / 卡住
        match opt.act {
            Act::Done if r.prog.published => {
                row(&dec, fj);
                return Outcome::Published(r.trace);
            }
            Act::Done => return r.stop("还没发布就说做完了".into()),
            Act::Stuck => return r.stop("卡住了，交给你".into()),
            _ => {}
        }
        // 6. 最终发布先问用户
        let by_user = opt.publish && input.platform.publish_by_user_only;
        // 每一次点「最终发布」都先问（不只第一次）：上一次点了没反应、又要点时也问，绝不悄悄发第二次。
        if opt.publish {
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
                row(&dec, fj);
                record(
                    &mut r,
                    &cur,
                    &post,
                    &opt,
                    &user_dec,
                    vec![(step, vec![])],
                    true,
                );
                advance(&mut r, &opt);
                cur = post;
                continue;
            }
        }
        // 7. 执行
        let done = match act::execute(h, &mut *lad.user, &input, &scrub, &state, &opt, dec.by) {
            act::Exec::Stop(why) => return r.stop(why),
            act::Exec::Halt(why) => {
                return Outcome::Halted {
                    trace: r.trace,
                    why,
                }
            }
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
            Some(steps) if !r.seen(&post.key) => {
                row(&dec, fj);
                record(&mut r, &cur, &post, &opt, &dec, steps, true);
                advance(&mut r, &opt);
            }
            Some(steps) => {
                // 做了，但画面没变或绕回了见过的画面：照实记成 ok: false，失败记在选它的画面上。
                r.back_to(&post.key);
                record(&mut r, &cur, &post, &opt, &dec, steps, false);
                r.charge(&cur.key, &dec);
            }
            None => r.charge(&cur.key, &dec),
        }
        cur = post;
    }
}
