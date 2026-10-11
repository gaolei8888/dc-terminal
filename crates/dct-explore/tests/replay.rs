use dct_explore::hands::{By, El, Frac, Help, HelpAnswer, Params};
use dct_explore::ladder::{
    Jeff, JeffAnswer, Ladder, Model, ModelPick, NoModel, User, UserPick, Who,
};
use dct_explore::options::Opt;
use dct_explore::platform::Table;
use dct_explore::replay::{publish_step, replay, ReplayInput, ReplayOutcome};
use dct_explore::testing::{reply, FakeHands};
use serde_json::{json, Value};
use std::cell::RefCell;

struct J(Option<(&'static str, f64)>, RefCell<Vec<String>>);
impl Jeff for J {
    fn ask(&self, state: &str, _: &dct_explore::JevChoice) -> Option<JeffAnswer> {
        self.1.borrow_mut().push(state.to_string());
        self.0.map(|(k, c)| JeffAnswer {
            key: k.into(),
            probabilities: vec![(k.into(), c)],
            answer_confidence: c,
        })
    }
}
fn jeff(k: Option<(&'static str, f64)>) -> J {
    J(k, RefCell::new(vec![]))
}
struct U(Option<&'static str>, Vec<String>);
impl User for U {
    fn pick(&mut self, state: &str, _: &[Opt], _: Option<&str>, _: &str) -> Option<UserPick> {
        self.1.push(state.to_string());
        self.0.map(|k| UserPick {
            key: k.into(),
            name: None,
        })
    }
}
struct M(&'static str, RefCell<u32>);
impl Model for M {
    fn pick(&self, _: &str, _: &[Opt], _: Option<&str>) -> Option<ModelPick> {
        *self.1.borrow_mut() += 1;
        Some(ModelPick {
            key: self.0.into(),
            name: None,
        })
    }
}

fn el(id: &str, t: &str, x: f64, y: f64) -> El {
    El {
        id: id.into(),
        text: t.into(),
        role: "button".into(),
        frac: Frac {
            x,
            y,
            w: 0.1,
            h: 0.05,
        },
        source: "ocr".into(),
    }
}

fn x_proc(executable: bool) -> Value {
    json!({
        "name": "promote-x-post", "executable": executable,
        "steps": [
            {"n": 1, "action": "open_url", "arg": "https://x.com/compose/post"},
            {"n": 2, "action": "verify_text", "arg": "「What is happening」"},
            {"n": 3, "action": "tap_by_intent", "arg": "What is happening?"},
            {"n": 4, "action": "type_param", "arg": "post_text"},
            {"n": 5, "action": "tap_by_intent", "arg": "Post"}
        ]
    })
}

fn params() -> Params {
    [("post_text".to_string(), "Launch day is here".to_string())]
        .into_iter()
        .collect()
}

fn input<'a>(
    proc_: &'a Value,
    platform: &'a dct_explore::platform::Platform,
    preview: bool,
) -> ReplayInput<'a> {
    ReplayInput {
        procedure: proc_,
        params: params(),
        popups: vec![json!({"name": "x-popup"})],
        window_id: 7,
        preview,
        platform,
    }
}

fn help_reply(run: &str, at: u32, els: Vec<El>) -> dct_explore::hands::RunReply {
    let mut r = reply(run, "needs_help", at);
    r.help = Some(Help {
        snapshot_id: "s1".into(),
        intent: "Post".into(),
        question: "which Post?".into(),
        elements: els,
        picture_png: None,
    });
    r
}

#[test]
fn replay_runs_the_approved_procedure_with_popups_and_params() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(true);
    let mut h = FakeHands::new();
    let mut done = reply("r1", "done", 5);
    done.learned_keys = vec!["k1".into()];
    h.push_procedure(done);
    let (j, mut u) = (jeff(None), U(None, vec![]));
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let out = replay(&mut h, &mut lad, input(&pr, p, false));
    assert_eq!(
        out,
        ReplayOutcome::Done {
            learned: vec!["k1".into()]
        }
    );
    let (sent, prm, pops, stop) = &h.sent_procedures[0];
    assert_eq!(sent, &pr);
    assert_eq!(prm["post_text"], "Launch day is here");
    assert_eq!(pops.len(), 1);
    assert_eq!(*stop, None);
    assert_eq!(h.procedure_windows, [7]);
}

#[test]
fn preview_stops_before_the_publish_step() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(true);
    assert_eq!(publish_step(&pr, p), Some(5));
    let mut h = FakeHands::new();
    h.push_procedure(reply("r1", "stopped", 5));
    let (j, mut u) = (jeff(None), U(None, vec![]));
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let out = replay(&mut h, &mut lad, input(&pr, p, true));
    assert_eq!(out, ReplayOutcome::Previewed { at_step: 5 });
    assert_eq!(h.sent_procedures[0].3, Some(5));
}

#[test]
fn a_user_only_publish_platform_always_previews() {
    let t = Table::seed().with_override(
        "[[platform]]\nname = \"wechat_mp\"\ndomain = \"mp.weixin.qq.com\"\nentry_url = \"https://mp.weixin.qq.com/x\"\nbrowser = \"Google Chrome\"\nvault = \"v\"\npublish_words = [\"Post\"]\nnever_tap = []\npublish_by_user_only = true\n",
    ).unwrap();
    let p = t.get("wechat_mp").unwrap();
    let pr = x_proc(true);
    let mut h = FakeHands::new();
    h.push_procedure(reply("r1", "stopped", 5));
    let (j, mut u) = (jeff(None), U(None, vec![]));
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    // preview: false is overridden
    let out = replay(&mut h, &mut lad, input(&pr, p, false));
    assert_eq!(out, ReplayOutcome::Previewed { at_step: 5 });
    assert_eq!(h.sent_procedures[0].3, Some(5));

    // no findable publish step: never run to the end
    let mut no_pub = x_proc(true);
    no_pub["steps"].as_array_mut().unwrap().pop();
    let mut h = FakeHands::new();
    let out = replay(&mut h, &mut lad, input(&no_pub, p, false));
    assert!(matches!(out, ReplayOutcome::Stopped(_)));
    assert!(h.calls.is_empty());
}

#[test]
fn replay_help_on_duplicate_text_goes_through_the_ladder() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(true);
    let mut h = FakeHands::new();
    h.push_procedure(help_reply(
        "r1",
        5,
        vec![el("e3", "Post", 0.05, 0.3), el("e9", "Post", 0.7, 0.4)],
    ));
    h.push_answer(reply("r1", "done", 5));
    let (j, mut u) = (jeff(Some(("e9", 0.9))), U(None, vec![]));
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let out = replay(&mut h, &mut lad, input(&pr, p, false));
    assert_eq!(out, ReplayOutcome::Done { learned: vec![] });
    assert_eq!(
        h.sent_answers,
        [(
            "r1".to_string(),
            HelpAnswer::Element {
                id: "e9".into(),
                by: By::Model,
                name: None,
                dismiss: false
            }
        )]
    );
    // the question never carries the param value
    let st = j.1.borrow();
    assert!(st[0].contains("promote-x-post") && st[0].contains("[e9] Post"));
}

#[test]
fn low_confidence_help_goes_to_the_user_and_is_sent_as_user() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(true);
    let mut h = FakeHands::new();
    h.push_procedure(help_reply(
        "r1",
        5,
        vec![el("e3", "Post", 0.05, 0.3), el("e7", "Post", 0.7, 0.4)],
    ));
    h.push_answer(reply("r1", "done", 5));
    let (j, mut u) = (jeff(Some(("e3", 0.3))), U(Some("e7"), vec![]));
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let out = replay(&mut h, &mut lad, input(&pr, p, false));
    assert!(matches!(out, ReplayOutcome::Done { .. }));
    assert_eq!(
        h.sent_answers[0].1,
        HelpAnswer::Element {
            id: "e7".into(),
            by: By::User,
            name: None,
            dismiss: false
        }
    );
}

#[test]
fn the_model_answer_is_sent_as_model() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(true);
    let mut h = FakeHands::new();
    h.push_procedure(help_reply(
        "r1",
        5,
        vec![el("e3", "Post", 0.1, 0.3), el("e7", "Post", 0.7, 0.4)],
    ));
    h.push_answer(reply("r1", "done", 5));
    let (j, mut u) = (jeff(None), U(None, vec![]));
    let m = M("e7", RefCell::new(0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    replay(&mut h, &mut lad, input(&pr, p, false));
    assert_eq!(
        h.sent_answers[0].1,
        HelpAnswer::Element {
            id: "e7".into(),
            by: By::Model,
            name: None,
            dismiss: false
        }
    );
}

#[test]
fn help_on_a_login_screen_cancels_without_asking_anyone() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(true);
    let mut h = FakeHands::new();
    h.push_procedure(help_reply(
        "r1",
        5,
        vec![el("e1", "Sign in", 0.1, 0.1), el("e2", "Post", 0.5, 0.5)],
    ));
    let (j, mut u) = (jeff(Some(("e2", 0.99))), U(Some("e2"), vec![]));
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let out = replay(&mut h, &mut lad, input(&pr, p, false));
    assert!(matches!(out, ReplayOutcome::Stopped(_)));
    assert!(h.calls.contains(&"cancel_run r1".to_string()));
    assert!(j.1.borrow().is_empty() && u.1.is_empty());
    assert!(h.sent_answers.is_empty());
}

#[test]
fn param_values_on_the_help_screen_are_not_sent_to_the_ladder() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(true);
    let mut h = FakeHands::new();
    h.push_procedure(help_reply(
        "r1",
        5,
        vec![
            el("e3", "Launch day is here", 0.1, 0.3),
            el("e4", "Post", 0.7, 0.4),
        ],
    ));
    h.push_answer(reply("r1", "done", 5));
    let (j, mut u) = (jeff(Some(("e4", 0.95))), U(None, vec![]));
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    replay(&mut h, &mut lad, input(&pr, p, false));
    let st = j.1.borrow();
    assert!(!st[0].contains("Launch day"), "{}", st[0]);
    assert!(st[0].contains("{post_text}"));
}

#[test]
fn a_missing_button_turns_into_a_repair_from_that_step() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(true);
    let mut h = FakeHands::new();
    let mut f = reply("r1", "failed", 4);
    f.note = Some("找不到「Post」".into());
    h.push_procedure(f);
    let (j, mut u) = (jeff(None), U(None, vec![]));
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let out = replay(&mut h, &mut lad, input(&pr, p, false));
    let ReplayOutcome::NeedsRepair { at_step, prefix } = out else {
        panic!("{out:?}")
    };
    assert_eq!(at_step, 4);
    assert_eq!(prefix.len(), 3);
    assert!(prefix
        .iter()
        .all(|s| s.from_procedure && s.ok && s.by == Who::User));
    assert_eq!(prefix[2].action, "tap_by_intent");
    assert_eq!(prefix[2].arg, "What is happening?");
}

#[test]
fn not_there_then_failed_is_a_repair() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(true);
    let mut h = FakeHands::new();
    h.push_procedure(help_reply("r1", 5, vec![el("e3", "Post", 0.1, 0.3)]));
    let mut f = reply("r1", "failed", 5);
    f.note = Some("找不到「Post」".into());
    h.push_answer(f);
    let (j, mut u) = (jeff(Some(("not_there", 0.95))), U(None, vec![]));
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let out = replay(&mut h, &mut lad, input(&pr, p, false));
    assert_eq!(h.sent_answers[0].1, HelpAnswer::NotThere);
    assert!(matches!(out, ReplayOutcome::NeedsRepair { at_step: 5, .. }));
}

#[test]
fn a_draft_is_never_replayed() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(false);
    let mut h = FakeHands::new();
    let (j, mut u) = (jeff(None), U(None, vec![]));
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let out = replay(&mut h, &mut lad, input(&pr, p, false));
    assert!(matches!(out, ReplayOutcome::Stopped(_)));
    assert!(h.calls.is_empty());
    let mut missing = x_proc(true);
    missing.as_object_mut().unwrap().remove("executable");
    assert!(matches!(
        replay(&mut h, &mut lad, input(&missing, p, false)),
        ReplayOutcome::Stopped(_)
    ));
    assert!(h.calls.is_empty());
}

#[test]
fn a_tier_block_is_reported_not_repaired() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(true);
    let mut h = FakeHands::new();
    let mut b = reply("r1", "blocked", 5);
    b.note = Some("needs_authorization: 删除账号".into());
    b.raw = json!({"needs_authorization": {"step": 5, "tier": "critical", "button": "Delete"}});
    h.push_procedure(b);
    let (j, mut u) = (jeff(None), U(None, vec![]));
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let out = replay(&mut h, &mut lad, input(&pr, p, false));
    assert!(matches!(out, ReplayOutcome::Blocked { .. }));
}

#[test]
fn failures_that_are_not_the_procedures_fault_are_not_repaired() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(true);
    for note in [
        "超时：流程跑得比 timeout_s 还久",
        "急停了，流程停在这里",
        "screen_locked",
        "画面是私人内容 private_screen",
    ] {
        let mut h = FakeHands::new();
        let mut f = reply("r1", "failed", 3);
        f.note = Some(note.into());
        h.push_procedure(f);
        let (j, mut u) = (jeff(None), U(None, vec![]));
        let mut lad = Ladder {
            jeff: &j,
            model: &NoModel,
            user: &mut u,
            threshold: 0.8,
        };
        let out = replay(&mut h, &mut lad, input(&pr, p, false));
        assert!(matches!(out, ReplayOutcome::Stopped(_)), "{note}: {out:?}");
    }
}

#[test]
fn help_is_answered_at_most_five_times() {
    let t = Table::seed();
    let p = t.get("x").unwrap();
    let pr = x_proc(true);
    let mut h = FakeHands::new();
    h.push_procedure(help_reply("r1", 5, vec![el("e3", "Post", 0.1, 0.3)]));
    for _ in 0..5 {
        h.push_answer(help_reply("r1", 5, vec![el("e3", "Post", 0.1, 0.3)]));
    }
    let (j, mut u) = (jeff(Some(("e3", 0.95))), U(Some("e3"), vec![]));
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let out = replay(&mut h, &mut lad, input(&pr, p, false));
    assert!(matches!(out, ReplayOutcome::Stopped(_)));
    assert_eq!(h.sent_answers.len(), 5);
    assert!(h.calls.contains(&"cancel_run r1".to_string()));
}
