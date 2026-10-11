//! 重放：把用户确认过的流程交给 dco 的 `run_procedure` 跑。
//!
//! - 只重放 `executable == true` 的流程；草稿一律不碰（`Stopped`，不调 dco）。dct 绝不自己改这个字段。
//! - `needs_help`：选项 = `help.elements` 各一项 + `not_there` + `stuck`，经同一个 `Ladder` 往上问
//!   （Jeff / 大模型的回答发 `by: model`，用户的发 `by: user`）。先按画面**原文**过 `gate::screen_halt`
//!   和禁点 / 档位（原文只在内存里，见 `explore/raw.rs`），出去的字才脱敏（参数值换回 `{名字}`、
//!   换行压成空格）：登录 / 验证码 / 警告 → 取消 dco 的运行、返回 `Stopped`（那行字已脱敏），
//!   不问任何人（选 `Stopped` 而不是另造一个 Halted：上层都是「停下交给用户」）。同一个运行最多答 5 次。
//! - `failed`：note 里有超时 / 急停 / 锁屏 / 私人画面的固定词 → `Stopped`；否则是流程本身走不通 →
//!   `NeedsRepair`，`prefix` 是流程第 1..卡住那步之前的每一步（`from_procedure`、`by: User`、`ok`）。
//! - `blocked`（档位）→ `Blocked`，转给用户。
//! - 平台 `publish_by_user_only`：永远当预览跑（`stop_before_step` = 发布那一步），结果 `Previewed`，
//!   由用户自己点发布；找不到发布那一步 → `Stopped`，绝不跑到底。预览时 dco 若没停就跑完了 → `Stopped`（如实说）。
use crate::explore::raw::Raw;
use crate::explore::screen::Scrub;
use crate::gate::{fold, tappable};
use crate::hands::{By, El, Hands, HelpAnswer, Params, RunReply};
use crate::ladder::{Ladder, Tries, Who};
use crate::options::{neighbour_hint, Act, Opt};
use crate::platform::Platform;
use crate::trace::TraceStep;
use serde_json::Value;

pub struct ReplayInput<'a> {
    /// `dcv show --json`，`executable` 必须为 true
    pub procedure: &'a Value,
    pub params: Params,
    pub popups: Vec<Value>,
    pub window_id: u64,
    /// true → `stop_before_step` = 发布那一步
    pub preview: bool,
    pub platform: &'a Platform,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReplayOutcome {
    Done {
        learned: Vec<String>,
    },
    Previewed {
        at_step: u32,
    },
    /// 档位拦下，转给用户
    Blocked {
        note: String,
    },
    /// 交给 `explore_from`
    NeedsRepair {
        at_step: u32,
        prefix: Vec<TraceStep>,
    },
    Stopped(String),
}

/// 一个运行最多答这么多次求助。
const MAX_ANSWERS: u32 = 5;
/// 给 Jeff 的选项上限（和 options 一致）。
const MAX_SHORT: usize = 20;
const NOT_THERE: &str = "not_there";
const STUCK: &str = "stuck";
/// dco 回 failed 但不是流程的错：超时 / 急停 / 锁屏 / 私人画面（dco `run/stop.rs`、HANDOFF 的固定词）。
const NOT_PROCEDURES_FAULT: &[&str] = &[
    "超时",
    "run_timeout",
    "急停",
    "halted",
    "锁屏",
    "screen_locked",
    "私人",
    "private_screen",
    // 窗口关了：dco 把步骤里的工具错误写成「{message}（{code}）」（run/mod.rs），窗口没了是 stale
    "stale",
    "窗口关了",
    "窗口已经关了",
    // dco 的 MAX_HELPS：求助太多次，交给用户，不是流程的错
    "要帮忙超过",
];

fn steps_of(p: &Value) -> &[Value] {
    p["steps"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn step_n(i: usize, s: &Value) -> u32 {
    s["n"].as_u64().map(|n| n as u32).unwrap_or(i as u32 + 1)
}

/// 发布那一步：最后一个 `tap_by_intent`，且 arg 整句（fold 后）等于平台发布词之一；找不到 → None。
pub fn publish_step(procedure: &Value, p: &Platform) -> Option<u32> {
    let words: Vec<String> = p.publish_words.iter().map(|w| fold(w)).collect();
    steps_of(procedure)
        .iter()
        .enumerate()
        .rev()
        .find(|(_, s)| {
            s["action"].as_str() == Some("tap_by_intent")
                && s["arg"]
                    .as_str()
                    .is_some_and(|a| words.iter().any(|w| !w.is_empty() && *w == fold(a)))
        })
        .map(|(i, s)| step_n(i, s))
}

fn prefix_before(procedure: &Value, at_step: u32) -> Vec<TraceStep> {
    steps_of(procedure)
        .iter()
        .enumerate()
        .filter(|(i, s)| step_n(*i, s) < at_step)
        .map(|(_, s)| TraceStep {
            action: s["action"].as_str().unwrap_or("").to_string(),
            arg: s["arg"].as_str().unwrap_or("").to_string(),
            by: Who::User,
            obs_before: None,
            obs_after: None,
            scene_before: None,
            scene_after: None,
            ok: true,
            publish: false,
            vanished: vec![],
            learned_keys: vec![],
            from_procedure: true,
        })
        .collect()
}

/// 第 `at_step` 步之前已经打过字的参数（`type_param` 的步号 < `at_step`）。
fn typed_before(procedure: &Value, at_step: u32) -> Vec<String> {
    steps_of(procedure)
        .iter()
        .enumerate()
        .filter(|(i, s)| s["action"].as_str() == Some("type_param") && step_n(*i, s) < at_step)
        .filter_map(|(_, s)| s["arg"].as_str().map(str::to_owned))
        .collect()
}

fn text_of(e: &El, all: &[El]) -> (String, Act) {
    let label = e.text.trim();
    if label.is_empty() {
        (
            neighbour_hint(e, all),
            Act::TapIcon {
                el: e.clone(),
                name: None,
            },
        )
    } else {
        let kind = if e.role == "button" {
            "按钮"
        } else {
            "文字"
        };
        (
            format!("{kind} {label}"),
            Act::Tap {
                el: e.clone(),
                intent: label.to_string(),
            },
        )
    }
}

/// 选项：求助里每个能点的元素 + not_there + stuck。返回 (给 Jeff 的前 20 个, 全部)。
/// 超过 20 个时，字和 dco 要找的意图对得上的元素排在前面。
fn options(els: &[El], intent: &str, p: &Platform) -> (Vec<Opt>, Vec<Opt>) {
    let fi = fold(intent);
    let mut els: Vec<&El> = els
        .iter()
        .filter(|e| e.text.trim().is_empty() || tappable(e.text.trim(), p))
        .collect();
    els.sort_by_key(|e| {
        let f = fold(e.text.trim());
        !(!fi.is_empty() && !f.is_empty() && (f == fi || f.contains(&fi)))
    });
    let ctx: Vec<El> = els.iter().map(|e| (*e).clone()).collect();
    let mut all: Vec<Opt> = Vec::new();
    for e in &els {
        if all.iter().any(|o| o.key == e.id) {
            continue;
        }
        let (text, act) = text_of(e, &ctx);
        all.push(Opt {
            key: e.id.clone(),
            text,
            act,
            publish: false,
        });
    }
    all.push(Opt {
        key: NOT_THERE.into(),
        text: format!("画面上没有「{}」", intent.trim()),
        act: Act::Wait,
        publish: false,
    });
    all.push(Opt {
        key: STUCK.into(),
        text: "看不懂或者卡住了".into(),
        act: Act::Stuck,
        publish: false,
    });
    let fixed = 2;
    let mut short: Vec<Opt> = all.iter().take(MAX_SHORT - fixed).cloned().collect();
    short.extend(all[all.len() - fixed..].iter().cloned());
    if all.len() <= MAX_SHORT {
        short = all.clone();
    }
    (short, all)
}

fn stopped_text(r: &RunReply, scrub: &Scrub) -> String {
    format!(
        "dco 回 {}：{}",
        r.status,
        scrub.line(r.note.as_deref().unwrap_or(""))
    )
}

fn not_procedures_fault(r: &RunReply) -> bool {
    // 只看这一步的 note 和错误码；早先步骤的 note 不算。
    let n = r.note.as_deref().unwrap_or("");
    let code = r.raw["code"].as_str().unwrap_or("");
    NOT_PROCEDURES_FAULT
        .iter()
        .any(|w| n.contains(w) || code.contains(w))
}

pub fn replay(h: &mut dyn Hands, lad: &mut Ladder, input: ReplayInput) -> ReplayOutcome {
    if input.procedure["executable"].as_bool() != Some(true) {
        return ReplayOutcome::Stopped("这条流程还没确认，不能重放".into());
    }
    let name = input.procedure["name"].as_str().unwrap_or("").to_string();
    let scrub = Scrub::new(&input.params);
    // 发布交给用户的平台：一律预览。
    let preview = input.preview || input.platform.publish_by_user_only;
    let stop_before = if preview {
        match publish_step(input.procedure, input.platform) {
            Some(n) => Some(n),
            None => {
                return ReplayOutcome::Stopped(
                    "找不到流程里的发布那一步，不敢跑到底（预览 / 发布要用户自己点的平台都要先停在发布前）"
                        .into(),
                )
            }
        }
    } else {
        None
    };

    let mut rep = match h.run_procedure(
        input.window_id,
        input.procedure,
        &input.params,
        &input.popups,
        stop_before,
    ) {
        Ok(r) => r,
        Err(e) => {
            return ReplayOutcome::Stopped(format!(
                "dco 出错：{}（{}）",
                scrub.line(&e.message),
                scrub.line(&e.code)
            ))
        }
    };
    let mut learned: Vec<String> = Vec::new();
    let mut answers = 0u32;
    let mut tries = Tries::default();
    let mut last_by: Option<Who> = None;
    loop {
        for k in &rep.learned_keys {
            if !learned.contains(k) {
                learned.push(k.clone());
            }
        }
        match rep.status.as_str() {
            "already_done" if preview => {
                return ReplayOutcome::Stopped(
                    "完成标志已经在画面上了，这次什么都没有跑；不用再发".into(),
                )
            }
            "done" | "already_done" => {
                return if preview {
                    ReplayOutcome::Stopped(
                        "dco 没有停在发布前，流程已经跑到底了；请去看看发布出去没有".into(),
                    )
                } else {
                    ReplayOutcome::Done { learned }
                }
            }
            "stopped" if preview => {
                return ReplayOutcome::Previewed {
                    at_step: rep.at_step,
                }
            }
            "blocked" => {
                return ReplayOutcome::Blocked {
                    note: scrub.line(rep.note.as_deref().unwrap_or("")),
                }
            }
            "failed" if !not_procedures_fault(&rep) => {
                return ReplayOutcome::NeedsRepair {
                    at_step: rep.at_step,
                    prefix: prefix_before(input.procedure, rep.at_step),
                }
            }
            "needs_help" => {}
            _ => return ReplayOutcome::Stopped(stopped_text(&rep, &scrub)),
        }

        // needs_help
        let run_id = rep.run_id.clone();
        let Some(help) = rep.help.as_ref() else {
            let _ = h.cancel_run(&run_id);
            return ReplayOutcome::Stopped("dco 求助了，但没带画面，停下".into());
        };
        if answers >= MAX_ANSWERS {
            let _ = h.cancel_run(&run_id);
            return ReplayOutcome::Stopped(format!(
                "同一次重放已经答了 {MAX_ANSWERS} 次求助，停下"
            ));
        }
        // 流程已经走过某个参数的 `type_param`：从这儿起它的片段才可能在画面上（片段也算帖子）。
        // 还没打字时发帖框的提示字和帖子开头一样，也照样是发帖框。
        for p in typed_before(input.procedure, rep.at_step) {
            scrub.mark_typed(&p);
        }
        // 规矩看原文（停下、禁点词、档位），出去的字看脱敏后的（见 explore/raw.rs）。
        let raw = Raw::new(&help.elements, &scrub);
        if let Some(why) = raw.halt(&scrub) {
            let _ = h.cancel_run(&run_id);
            return ReplayOutcome::Stopped(format!(
                "重放时画面变成了登录 / 验证码 / 警告（{why:?}），停下交给你；没有问任何模型"
            ));
        }
        let intent = scrub.line(&help.intent);
        let mut state = format!(
            "流程：{}\n第 {} 步\ndco 在找：{}\n问题：{}\n页面上的字：\n",
            scrub.line(&name),
            rep.at_step,
            intent,
            scrub.line(&help.question)
        );
        let els = raw.shown(&scrub, input.platform);
        for e in els.iter().take(200) {
            let t = e.text.trim();
            let t = if t.is_empty() { "（没有字）" } else { t };
            state.push_str(&format!("[{}] {}\n", e.id, t));
        }
        let pickable: Vec<El> = els
            .iter()
            .filter(|e| raw.tappable(&e.id, input.platform))
            .cloned()
            .collect();
        let (short, all) = options(&pickable, &intent, input.platform);
        let Some(dec) = lad.decide(&state, &short, &all, help.picture_png.as_deref(), &tries)
        else {
            let _ = h.cancel_run(&run_id);
            return ReplayOutcome::Stopped("求助时用户说停，停下".into());
        };
        let by = if dec.by == Who::User {
            By::User
        } else {
            By::Model
        };
        let answer = match dec.key.as_str() {
            STUCK => {
                let _ = h.cancel_run(&run_id);
                return ReplayOutcome::Stopped("求助时没人认得出是哪一个，停下".into());
            }
            NOT_THERE => HelpAnswer::NotThere,
            key => match pickable.iter().find(|e| e.id == key) {
                Some(e) => HelpAnswer::Element {
                    id: e.id.clone(),
                    by,
                    name: dec
                        .name
                        .as_deref()
                        .map(|n| scrub.line(n))
                        .filter(|n| !scrub.has_placeholder(n)),
                    dismiss: false,
                },
                None => {
                    let _ = h.cancel_run(&run_id);
                    return ReplayOutcome::Stopped(format!("选的「{key}」不在求助的画面里，停下"));
                }
            },
        };
        // 同一个求助再来一次：上一个答案没起作用。
        match last_by {
            Some(Who::Jeff) => tries.jeff_failed = true,
            Some(Who::Model) => tries.model_tries += 1,
            _ => {}
        }
        last_by = Some(dec.by);
        answers += 1;
        rep = match h.answer_run(&run_id, &answer) {
            Ok(r) => r,
            Err(e) => {
                let _ = h.cancel_run(&run_id);
                return ReplayOutcome::Stopped(format!(
                    "dco 出错：{}（{}）",
                    scrub.line(&e.message),
                    scrub.line(&e.code)
                ));
            }
        };
    }
}
