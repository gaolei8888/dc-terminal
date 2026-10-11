//! 摸索循环里的两件事：问谁（编号图按需才拿）和执行一个选项（求助、拦下、失败）。
use super::screen::{look_error, Scrub};
use super::{Clock, ExploreInput};
use crate::gate::{fold, screen_halt, Halt};
use crate::hands::{By, El, Hands, Help, HelpAnswer, RunReply, StepReq};
use crate::ladder::{Decision, Ladder, Model, ModelPick, Tries, User, UserPick, Who};
use crate::options::{steps_for, Act, Opt};
use crate::trace::{append_question, QuestionRow};
use std::cell::RefCell;

/// 求助时「同一个元素」的中心点距离上限（窗口比例）。
const SAME_PLACE: f64 = 0.03;
/// 同一步最多答几次 dco 的求助。
const MAX_HELP_ROUNDS: u32 = 3;

pub(super) fn is_done(status: &str) -> bool {
    matches!(status, "done" | "already_done")
}

fn private(r: &RunReply) -> bool {
    r.note
        .as_deref()
        .is_some_and(|n| n.contains("private_screen"))
        || r.raw.to_string().contains("private_screen")
}

/// 给用户看的 dco 回复：档位拦下的要把 dco 的原话转过去（参数值换回名字）。
pub(super) fn describe(r: &RunReply, scrub: &Scrub) -> String {
    let note = scrub.text(r.note.as_deref().unwrap_or(""));
    match r.status.as_str() {
        "blocked" => format!("dco 拦下了这一下：{note}"),
        _ if private(r) => "画面是私人内容（private_screen），停下交给你".into(),
        s => format!("dco 回 {s}：{note}"),
    }
}

/// 编号图：第一次要时才 `look(marks: true)`。这一眼如果撞上停下的画面，记下来，谁都不问。
struct Pics<'h> {
    h: &'h mut dyn Hands,
    window: u64,
    scrub: &'h Scrub,
    got: Option<Option<String>>,
    halt: Option<Halt>,
}

impl Pics<'_> {
    fn get(&mut self) -> Option<String> {
        if self.got.is_none() {
            let p = match self.h.look(self.window, true) {
                Ok(l) => match screen_halt(&self.scrub.els(&l.elements)) {
                    Some(h) => {
                        self.halt = Some(h);
                        None
                    }
                    None => l.marks_png,
                },
                // 拿不到图不算错：照样问，只是没有图。窗口真没了下一次 look 会停。
                Err(_) => None,
            };
            self.got = Some(p);
        }
        self.got.clone().flatten()
    }
}

struct LazyModel<'p, 'h> {
    inner: &'p dyn Model,
    pics: &'p RefCell<Pics<'h>>,
}
impl Model for LazyModel<'_, '_> {
    fn pick(&self, state: &str, all: &[Opt], _: Option<&str>) -> Option<ModelPick> {
        let pic = self.pics.borrow_mut().get();
        if self.pics.borrow().halt.is_some() {
            return None;
        }
        self.inner.pick(state, all, pic.as_deref())
    }
}

struct LazyUser<'p, 'h> {
    inner: &'p mut dyn User,
    pics: &'p RefCell<Pics<'h>>,
}
impl User for LazyUser<'_, '_> {
    fn pick(&mut self, state: &str, all: &[Opt], _: Option<&str>, why: &str) -> Option<UserPick> {
        let pic = self.pics.borrow_mut().get();
        if self.pics.borrow().halt.is_some() {
            return None;
        }
        self.inner.pick(state, all, pic.as_deref(), why)
    }
}

pub(super) enum Decided {
    /// 第二项：拿过编号图就是 Some（里面可能没有图）。
    Pick(Decision, Option<Option<String>>),
    Halt(Halt),
    UserStopped,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn decide(
    h: &mut dyn Hands,
    lad: &mut Ladder,
    input: &ExploreInput,
    scrub: &Scrub,
    state: &str,
    short: &[Opt],
    all: &[Opt],
    tries: &Tries,
) -> Decided {
    let pics = RefCell::new(Pics {
        h,
        window: input.window_id,
        scrub,
        got: None,
        halt: None,
    });
    let model = LazyModel {
        inner: lad.model,
        pics: &pics,
    };
    let mut user = LazyUser {
        inner: &mut *lad.user,
        pics: &pics,
    };
    let d = Ladder {
        jeff: lad.jeff,
        model: &model,
        user: &mut user,
        threshold: lad.threshold,
    }
    .decide(state, short, all, None, tries);
    let p = pics.into_inner();
    match (p.halt, d) {
        (Some(why), _) => Decided::Halt(why),
        (None, Some(d)) => Decided::Pick(d, p.got),
        (None, None) => Decided::UserStopped,
    }
}

/// 问用户发不发之前要的编号图（还没拿过时）。
pub(super) fn marks_now(
    h: &mut dyn Hands,
    input: &ExploreInput,
    scrub: &Scrub,
) -> Result<Option<String>, Halt> {
    let mut p = Pics {
        h,
        window: input.window_id,
        scrub,
        got: None,
        halt: None,
    };
    let pic = p.get();
    match p.halt {
        Some(why) => Err(why),
        None => Ok(pic),
    }
}

pub(super) enum Exec {
    /// 每一步和 dco 报的学习条目
    Ok(Vec<(StepReq, Vec<String>)>),
    /// dco 回 failed：这一下没起作用
    Failed,
    Stop(String),
    /// 求助的画面是登录 / 验证码 / 警告：已取消 dco 的运行，谁都不问。
    Halt(Halt),
}

fn center(e: &El) -> (f64, f64) {
    (e.frac.x + e.frac.w / 2.0, e.frac.y + e.frac.h / 2.0)
}

/// dco 求助时怎么答：在 `help.elements` 里找和选中元素同字、中心点很近的那一个；
/// 正好一个就直接答（Jeff 和大模型发 model，用户发 user），否则把编号图交给用户选。
fn answer_for(
    help: &Help,
    opt: &Opt,
    by: Who,
    user: &mut dyn User,
    state: &str,
    scrub: &Scrub,
) -> Option<HelpAnswer> {
    let els = scrub.els(&help.elements);
    let (target, icon_name) = match &opt.act {
        Act::Tap { el, .. } => (Some(el), None),
        Act::TapIcon { el, name } => (Some(el), name.clone()),
        Act::Fill { field, .. } => (Some(field), None),
        _ => (None, None),
    };
    if let Some(t) = target {
        let (tx, ty) = center(t);
        let hits: Vec<&El> = els
            .iter()
            .filter(|e| fold(&e.text) == fold(&t.text))
            .filter(|e| {
                let (x, y) = center(e);
                ((x - tx).powi(2) + (y - ty).powi(2)).sqrt() < SAME_PLACE
            })
            .collect();
        if let [one] = hits[..] {
            return Some(HelpAnswer::Element {
                id: one.id.clone(),
                by: if by == Who::User { By::User } else { By::Model },
                name: icon_name,
                dismiss: false,
            });
        }
    }
    let opts: Vec<Opt> = els
        .iter()
        .map(|e| Opt {
            key: e.id.clone(),
            text: if e.text.trim().is_empty() {
                "（没有字）".into()
            } else {
                e.text.trim().to_string()
            },
            act: Act::Tap {
                el: e.clone(),
                intent: e.text.clone(),
            },
            publish: false,
        })
        .collect();
    let why = format!(
        "dco 找不到「{}」是哪一个，请在编号图上指出来",
        scrub.line(&help.intent)
    );
    let p = user.pick(state, &opts, help.picture_png.as_deref(), &why)?;
    let e = els.iter().find(|e| e.id == p.key)?;
    Some(HelpAnswer::Element {
        id: e.id.clone(),
        by: By::User,
        name: p.name.map(|n| scrub.line(&n)).or(icon_name),
        dismiss: false,
    })
}

/// 逐步 `run_step`。求助的画面要停下（登录、验证码…）→ 取消运行、`Halt`；
/// 求助按 `answer_for` 答；blocked / private_screen / 急停一类 → 停；
/// 其余 failed → 这一下没起作用。
pub(super) fn execute(
    h: &mut dyn Hands,
    user: &mut dyn User,
    input: &ExploreInput,
    scrub: &Scrub,
    state: &str,
    opt: &Opt,
    by: Who,
) -> Exec {
    let steps = steps_for(opt);
    if steps.is_empty() {
        return Exec::Stop(format!("「{}」没法变成 dco 的步骤", opt.key));
    }
    let mut out = Vec::with_capacity(steps.len());
    for s in steps {
        let mut rep = match h.run_step(input.window_id, &s, &input.params, &input.popups) {
            Ok(r) => r,
            Err(e) => return Exec::Stop(look_error(&e, scrub)),
        };
        let mut rounds = 0;
        loop {
            match rep.status.as_str() {
                st if is_done(st) => break,
                "needs_help" if rounds < MAX_HELP_ROUNDS => {
                    rounds += 1;
                    let help = rep.help.as_ref();
                    // 求助的画面也先过停下的规矩：命中就取消，不交给任何人去选。
                    if let Some(why) = help.and_then(|hp| screen_halt(&scrub.els(&hp.elements))) {
                        let _ = h.cancel_run(&rep.run_id);
                        return Exec::Halt(why);
                    }
                    let a = help.and_then(|help| answer_for(help, opt, by, user, state, scrub));
                    let Some(a) = a else {
                        let _ = h.cancel_run(&rep.run_id);
                        return Exec::Stop("dco 求助时没人指出是哪一个，停下".into());
                    };
                    rep = match h.answer_run(&rep.run_id, &a) {
                        Ok(r) => r,
                        Err(e) => return Exec::Stop(look_error(&e, scrub)),
                    };
                }
                "needs_help" => {
                    let _ = h.cancel_run(&rep.run_id);
                    return Exec::Failed;
                }
                "failed" if !private(&rep) => return Exec::Failed,
                _ => return Exec::Stop(describe(&rep, scrub)),
            }
        }
        out.push((s, rep.learned_keys));
    }
    Exec::Ok(out)
}

/// 一道题写一行（题面和选项字都已脱敏）。写不进去不影响这次摸索。
#[allow(clippy::too_many_arguments)]
pub(super) fn write_row(
    file: &std::path::Path,
    clock: &dyn Clock,
    input: &ExploreInput,
    state: &str,
    all: &[Opt],
    dec: &crate::ladder::Decision,
    failed_jeff: Option<(String, f64)>,
) {
    let jeff = dec.jeff.clone().or(failed_jeff);
    let corrects_jeff = dec.by != Who::Jeff && jeff.as_ref().is_some_and(|(k, _)| *k != dec.key);
    let row = QuestionRow {
        ts_ms: clock.now_ms(),
        run_id: &input.run_id,
        platform: &input.platform.name,
        state: state.to_string(),
        options: all
            .iter()
            .map(|o| (o.key.clone(), o.text.clone()))
            .collect(),
        jeff,
        answer: &dec.key,
        by: dec.by,
        corrects_jeff,
    };
    let _ = append_question(file, &row);
}
