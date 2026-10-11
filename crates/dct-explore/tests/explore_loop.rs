//! 摸索循环，全部对着 FakeHands 测：不碰真屏幕、不调真模型。
//!
//! 脚本约定：每次要问大模型或用户时，循环会多看一次（`look marks=true`）拿编号图；
//! 要问用户「发不发」而编号图还没拿过时也会多看一次。脚本里这些都标了「编号图」。
use dct_explore::explore::{explore, explore_from, Clock, Confirm, ExploreInput, Outcome};
use dct_explore::gate::{Budget, Halt};
use dct_explore::hands::*;
use dct_explore::ladder::*;
use dct_explore::platform::Table;
use dct_explore::testing::FakeHands;

struct NoJeff;
impl Jeff for NoJeff {
    fn ask(&self, _: &str, _: &dct_explore::JevChoice) -> Option<JeffAnswer> {
        None
    }
}
struct SureJeff(Vec<&'static str>, std::cell::RefCell<usize>);
impl Jeff for SureJeff {
    fn ask(&self, _: &str, _: &dct_explore::JevChoice) -> Option<JeffAnswer> {
        let i = *self.1.borrow();
        *self.1.borrow_mut() += 1;
        self.0.get(i).map(|k| JeffAnswer {
            key: k.to_string(),
            probabilities: vec![(k.to_string(), 0.95)],
            answer_confidence: 0.95,
        })
    }
}
fn sure(keys: Vec<&'static str>) -> SureJeff {
    SureJeff(keys, std::cell::RefCell::new(0))
}
struct ScriptUser(Vec<(&'static str, Option<&'static str>)>, usize);
impl User for ScriptUser {
    fn pick(
        &mut self,
        _: &str,
        _: &[dct_explore::options::Opt],
        _: Option<&str>,
        _: &str,
    ) -> Option<UserPick> {
        let r = self.0.get(self.1).map(|(k, n)| UserPick {
            key: k.to_string(),
            name: n.map(String::from),
        });
        self.1 += 1;
        r
    }
}
struct Yes(u32);
impl Confirm for Yes {
    fn publish_ok(&mut self, _: Option<&str>, _: &str) -> bool {
        self.0 += 1;
        true
    }
}
struct No;
impl Confirm for No {
    fn publish_ok(&mut self, _: Option<&str>, _: &str) -> bool {
        false
    }
}
/// 记下给用户看的话，然后点头。
struct Said(Vec<String>);
impl Confirm for Said {
    fn publish_ok(&mut self, _: Option<&str>, summary: &str) -> bool {
        self.0.push(summary.to_string());
        true
    }
}
struct T0;
impl Clock for T0 {
    fn now_ms(&self) -> u64 {
        0
    }
}

fn el(id: &str, t: &str, role: &str, x: f64, y: f64) -> El {
    El {
        id: id.into(),
        text: t.into(),
        role: role.into(),
        frac: Frac {
            x,
            y,
            w: 0.05,
            h: 0.03,
        },
        source: "ocr".into(),
    }
}
fn look(hash: &str, els: Vec<El>) -> Look {
    Look {
        snapshot_id: format!("s-{hash}"),
        observation_id: Some(format!("obs-{hash}")),
        scene_hash: Some(hash.into()),
        elements: els,
        marks_png: None,
    }
}
fn done(id: &str) -> RunReply {
    RunReply {
        run_id: id.into(),
        status: "done".into(),
        at_step: 1,
        note: None,
        help: None,
        learned_keys: vec![],
        raw: serde_json::json!({}),
    }
}
fn needs_help(id: &str, intent: &str, els: Vec<El>) -> RunReply {
    RunReply {
        run_id: id.into(),
        status: "needs_help".into(),
        at_step: 1,
        note: None,
        help: Some(Help {
            snapshot_id: "s-help".into(),
            intent: intent.into(),
            question: "哪一个？".into(),
            elements: els,
            picture_png: Some("HELP-PNG".into()),
        }),
        learned_keys: vec![],
        raw: serde_json::json!({}),
    }
}
fn input_for(t: &Table, name: &str, d: &std::path::Path) -> ExploreInput<'static> {
    let p: &'static dct_explore::platform::Platform =
        Box::leak(Box::new(t.get(name).unwrap().clone()));
    ExploreInput {
        platform: p,
        goal_text: "把这段文字发到 X",
        params: [("post_text".to_string(), "hello from dct test".to_string())]
            .into_iter()
            .collect(),
        text_params: vec!["post_text".into()],
        file_params: vec![],
        window_id: 4100,
        popups: vec![],
        budget: Budget {
            max_steps: 40,
            max_ms: 600_000,
            max_unchanged: 3,
        },
        run_id: "x-20261011-0001".into(),
        data_dir: d.to_path_buf(),
    }
}
fn input(t: &Table, d: &std::path::Path) -> ExploreInput<'static> {
    input_for(t, "x", d)
}

const TEXT: &str = "hello from dct test";

fn composer(hash: &str) -> Look {
    look(
        hash,
        vec![
            el("e2", "What's happening?", "textarea", 0.45, 0.25),
            el("e5", "Drafts", "button", 0.6, 0.1),
        ],
    )
}
fn composer_filled(hash: &str) -> Look {
    look(
        hash,
        vec![
            el("e1", "Post", "button", 0.08, 0.5),
            el("e2", TEXT, "textarea", 0.45, 0.25),
            el("e3", "Post", "button", 0.8, 0.35),
        ],
    )
}
fn sent(hash: &str) -> Look {
    look(hash, vec![el("e1", "Your post was sent", "text", 0.4, 0.9)])
}

fn count(h: &FakeHands, call: &str) -> usize {
    h.calls.iter().filter(|c| c.as_str() == call).count()
}

#[test]
fn opens_the_fixed_entry_url_first_and_never_asks_a_model_where_to_go() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look_err("stale");
    let (j, m, mut u) = (NoJeff, NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert_eq!(h.calls[0], "run_step open_url: https://x.com/compose/post");
    assert_eq!(h.step_calls[0].window_id, 4100);
    assert!(matches!(out, Outcome::Stopped { .. }), "{out:?}");
    assert_eq!(u.1, 0, "没人被问过去哪个网站");
}

#[test]
fn two_post_buttons_are_answered_with_the_one_jeff_chose() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0")); // open_url
    h.push_look(composer_filled("a"));
    h.push_look(composer_filled("a")); // 编号图（问发不发）
    h.push_step(needs_help(
        "r1",
        "Post",
        vec![
            el("e7", "Post", "button", 0.08, 0.5),
            el("e9", "Post", "button", 0.8, 0.35),
        ],
    ));
    h.push_answer(done("r1"));
    h.push_look(sent("b"));
    let (j, m, mut u) = (sure(vec!["e3", "done"]), NoModel, ScriptUser(vec![], 0));
    let mut yes = Yes(0);
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut yes, &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Published(_)), "{out:?}");
    assert_eq!(h.sent_answers.len(), 1);
    match &h.sent_answers[0] {
        (run, HelpAnswer::Element { id, by, .. }) => {
            assert_eq!(run, "r1");
            assert_eq!(id, "e9", "Jeff 选的是右边发帖框里的 Post");
            assert_eq!(*by, By::Model, "Jeff 的回答发 model");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(yes.0, 1, "第一次最终发布先问用户");
    assert_eq!(u.1, 0);
}

#[test]
fn the_first_publish_asks_the_user_and_declining_stops_before_tapping() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer_filled("a"));
    h.push_look(composer_filled("a")); // 编号图
    let (j, m, mut u) = (sure(vec!["e3"]), NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut No, &T0, input(&t, d.path()));
    match out {
        Outcome::DeclinedAtPublish(tr) => {
            assert!(tr.steps.iter().all(|s| !s.publish));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(count(&h, "run_step tap_by_intent: Post"), 0);
}

#[test]
fn text_wiped_by_attach_is_typed_again_before_publish() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut inp = input(&t, d.path());
    inp.file_params = vec!["image".into()];
    inp.params
        .insert("image".into(), "/Users/lei/pic.png".into());
    let field = |text: &str| el("e2", text, "textarea", 0.45, 0.25);
    let post = || el("e3", "Post", "button", 0.8, 0.35);
    let thumb = || el("e8", "1 image", "text", 0.45, 0.45);
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(look("a", vec![field("What's happening?"), post()]));
    h.push_step(done("r1")); // pick_file: image
    h.push_look(look("b", vec![field("What's happening?"), post(), thumb()]));
    h.push_step(done("r2")); // tap_by_intent: What's happening?
    h.push_step(done("r3")); // type_param: post_text
    h.push_look(look("c", vec![field(TEXT), post(), thumb()]));
    h.push_step(done("r4")); // wait
    h.push_look(look("d", vec![field("What's happening?"), post(), thumb()])); // 字被清掉了
    h.push_step(done("r5")); // tap_by_intent: What's happening?
    h.push_step(done("r6")); // type_param: post_text
    h.push_look(look("e", vec![field(TEXT), post(), thumb()]));
    h.push_look(look("e", vec![field(TEXT), post(), thumb()])); // 编号图
    h.push_step(done("r7")); // tap_by_intent: Post
    h.push_look(sent("f"));
    let j = sure(vec![
        "attach_image",
        "fill_post_text_e2",
        "wait",
        "fill_post_text_e2",
        "e3",
        "done",
    ]);
    let (m, mut u) = (NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, inp);
    assert!(matches!(out, Outcome::Published(_)), "{out:?}");
    let types: Vec<usize> = h
        .calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.as_str() == "run_step type_param: post_text")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(types.len(), 2, "{:?}", h.calls);
    let publish_at = h
        .calls
        .iter()
        .position(|c| c == "run_step tap_by_intent: Post")
        .unwrap();
    assert!(types[1] < publish_at, "{:?}", h.calls);
    assert!(
        h.calls
            .iter()
            .position(|c| c == "run_step pick_file: image")
            .unwrap()
            < types[0],
        "文件先于文字"
    );
}

#[test]
fn a_captcha_in_the_middle_stops_without_asking_any_model() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(done("r1")); // tap_by_intent: What's happening?
    h.push_step(done("r2")); // type_param
    h.push_look(look(
        "b",
        vec![
            el("e2", TEXT, "textarea", 0.45, 0.25),
            el("e6", "Everyone can reply", "button", 0.3, 0.4),
        ],
    ));
    h.push_step(done("r3")); // tap_by_intent: Everyone can reply
    h.push_look(look(
        "c",
        vec![
            el("e1", TEXT, "text", 0.4, 0.2),
            el("e9", "Verify you are human", "text", 0.4, 0.5),
        ],
    ));
    let (j, m, mut u) = (
        sure(vec!["fill_post_text_e2", "e6", "e1"]),
        NoModel,
        ScriptUser(vec![("e1", None)], 0),
    );
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    match out {
        Outcome::Halted { why, trace } => {
            assert_eq!(why, Halt::Captcha("Verify you are human".into()));
            assert_eq!(trace.steps.len(), 4, "open_url + 填字两步 + 一次点");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(*j.1.borrow(), 2, "验证码画面没发给 Jeff");
    assert_eq!(u.1, 0, "也没问用户选项");
    assert!(h.looks.is_empty());
}

#[test]
fn an_unchanged_screen_counts_as_a_failed_jeff_pick_and_goes_up() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(done("r1")); // tap_by_intent: Drafts
    h.push_look(composer("a")); // 画面没变
    h.push_look(composer("a")); // 编号图（问大模型 / 用户）
    let (j, m, mut u) = (sure(vec!["e5", "e5"]), NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Stopped { .. }), "{out:?}");
    assert_eq!(*j.1.borrow(), 1, "同一画面 Jeff 失败过就不再问它");
    assert_eq!(u.1, 1, "没配大模型，直接问用户");
    assert_eq!(count(&h, "look marks=true"), 1);
}

#[test]
fn three_unchanged_screens_in_a_row_stop() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    for i in 1..=3 {
        h.push_look(composer("a")); // 编号图
        h.push_step(done(&format!("r{i}"))); // scroll: down
        h.push_look(composer("a"));
    }
    let (j, m, mut u) = (
        NoJeff,
        NoModel,
        ScriptUser(vec![("scroll_down", None); 5], 0),
    );
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    match out {
        Outcome::Stopped { why, .. } => assert!(why.contains("画面"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(count(&h, "run_step scroll: down"), 3);
    assert_eq!(u.1, 3);
    assert!(h.looks.is_empty());
}

#[test]
fn an_icon_answer_carries_its_name_to_dco() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let a = || {
        look(
            "a",
            vec![
                el("e2", "What's happening?", "textarea", 0.45, 0.25),
                el("e5", "", "image", 0.3, 0.35),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(a());
    h.push_look(a()); // 编号图
    h.push_step(needs_help(
        "r1",
        "add media",
        vec![
            el("e11", "", "image", 0.3, 0.35),
            el("e12", "Post", "button", 0.8, 0.35),
        ],
    ));
    h.push_answer(done("r1"));
    h.push_look(composer("b"));
    h.push_look(composer("b")); // 编号图（用户说停）
    let (j, m, mut u) = (
        NoJeff,
        NoModel,
        ScriptUser(vec![("e5", Some("add media"))], 0),
    );
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert_eq!(
        h.calls[3], "run_step tap_by_intent: add media",
        "{:?}",
        h.calls
    );
    match &h.sent_answers[0].1 {
        HelpAnswer::Element { id, by, name, .. } => {
            assert_eq!(id, "e11");
            assert_eq!(*by, By::User);
            assert_eq!(name.as_deref(), Some("add media"));
        }
        other => panic!("{other:?}"),
    }
    match out {
        Outcome::Stopped { trace, .. } => {
            let s = &trace.steps[1];
            assert_eq!(
                (s.action.as_str(), s.arg.as_str()),
                ("tap_by_intent", "add media")
            );
            assert_eq!(s.by, Who::User);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_closed_window_stops_the_run() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(done("r1")); // tap_by_intent: Drafts
    h.push_look_err("stale"); // 窗口半路被关
    let (j, m, mut u) = (sure(vec!["e5"]), NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    match out {
        Outcome::Stopped { why, .. } => assert!(why.contains("窗口"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(h.look_windows.iter().all(|w| *w == 4100), "只认这一个窗口");
    assert!(h.step_calls.iter().all(|s| s.window_id == 4100));
}

#[test]
fn training_rows_are_written_with_who_answered() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(done("r1")); // Jeff: tap_by_intent: Drafts
    h.push_look(composer("a")); // 没变 -> Jeff 失败
    h.push_look(composer("a")); // 编号图
    h.push_step(done("r2")); // 用户：tap_by_intent: What's happening?
    h.push_step(done("r3")); // type_param
    h.push_look(composer_filled("b"));
    h.push_look(composer_filled("b")); // 编号图（问发不发）
    h.push_step(done("r4")); // tap_by_intent: Post
    h.push_look(sent("c"));
    let (j, m, mut u) = (
        sure(vec!["e5", "e3", "done"]),
        NoModel,
        ScriptUser(vec![("fill_post_text_e2", None)], 0),
    );
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Published(_)), "{out:?}");
    let text = std::fs::read_to_string(d.path().join("answers.jsonl")).unwrap();
    assert!(!text.contains(TEXT), "{text}");
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(rows.len() >= 2, "{text}");
    for r in &rows {
        assert!(r["by"].is_string(), "{r}");
    }
    let fix = rows
        .iter()
        .find(|r| r["by"] == "user")
        .expect("用户纠正 Jeff 的那一行");
    assert_eq!(fix["corrects_jeff"], true);
    assert_eq!(fix["answer"], "fill_post_text_e2");
    assert_eq!(fix["jeff"][0], "e5");
    assert!(rows
        .iter()
        .any(|r| r["by"] == "jeff" && r["corrects_jeff"] == false));
}

#[test]
fn publish_by_user_only_leaves_the_last_tap_to_the_user() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let a = || {
        look(
            "a",
            vec![
                el("e2", TEXT, "textarea", 0.45, 0.25),
                el("e3", "发布", "button", 0.8, 0.1),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(a());
    h.push_look(a()); // 编号图
    h.push_look(look("b", vec![el("e1", "已发布", "text", 0.4, 0.4)]));
    let (j, m, mut u) = (sure(vec!["e3", "done"]), NoModel, ScriptUser(vec![], 0));
    let mut said = Said(vec![]);
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(
        &mut h,
        &mut lad,
        &mut said,
        &T0,
        input_for(&t, "wechat_mp", d.path()),
    );
    assert_eq!(h.step_calls.len(), 1, "只有 open_url：{:?}", h.calls);
    assert_eq!(count(&h, "run_step tap_by_intent: 发布"), 0);
    assert_eq!(said.0.len(), 1);
    assert!(said.0[0].contains("自己"), "{}", said.0[0]);
    match out {
        Outcome::Published(tr) => {
            let s = tr.steps.last().unwrap();
            assert_eq!(
                (s.action.as_str(), s.arg.as_str()),
                ("tap_by_intent", "发布")
            );
            assert_eq!(s.by, Who::User);
            assert!(s.ok && s.publish);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn explore_from_continues_on_the_current_screen_after_the_prefix() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut inp = input(&t, d.path());
    inp.file_params = vec!["image".into()];
    inp.params
        .insert("image".into(), "/Users/lei/pic.png".into());
    let step = |action: &str, arg: &str| dct_explore::trace::TraceStep {
        action: action.into(),
        arg: arg.into(),
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
    };
    let prefix = vec![
        step("open_url", "https://x.com/compose/post"),
        step("pick_file", "image"),
    ];
    let mut h = FakeHands::default();
    h.push_look(composer("a"));
    h.push_step(done("r1")); // tap_by_intent: What's happening?
    h.push_step(done("r2")); // type_param
    h.push_look(composer_filled("b"));
    h.push_look_err("stale"); // 编号图拿不到；用户说停
    let (j, m, mut u) = (
        sure(vec!["fill_post_text_e2"]),
        NoModel,
        ScriptUser(vec![], 0),
    );
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore_from(&mut h, &mut lad, &mut Yes(0), &T0, inp, prefix);
    assert_eq!(h.calls[0], "look marks=false", "不再 open_url");
    assert_eq!(
        count(&h, "run_step type_param: post_text"),
        1,
        "文件已在开头选过，可以直接填字"
    );
    match out {
        Outcome::Stopped { trace, .. } => {
            assert!(trace.steps[0].from_procedure && trace.steps[1].from_procedure);
            assert_eq!(trace.steps.len(), 4);
            assert!(!trace.steps[2].from_procedure);
        }
        other => panic!("{other:?}"),
    }
}
