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

// ---- fix round 1 ----

/// 按脚本答，同时记下 Jeff 收到的每一份 state 和每道题的选项。
struct RecJeff {
    keys: Vec<&'static str>,
    states: std::cell::RefCell<Vec<String>>,
    questions: std::cell::RefCell<Vec<dct_explore::JevChoice>>,
}
fn rec(keys: Vec<&'static str>) -> RecJeff {
    RecJeff {
        keys,
        states: Default::default(),
        questions: Default::default(),
    }
}
impl Jeff for RecJeff {
    fn ask(&self, state: &str, q: &dct_explore::JevChoice) -> Option<JeffAnswer> {
        let i = self.states.borrow().len();
        self.states.borrow_mut().push(state.to_string());
        self.questions.borrow_mut().push(q.clone());
        self.keys.get(i).map(|k| JeffAnswer {
            key: k.to_string(),
            probabilities: vec![(k.to_string(), 0.95)],
            answer_confidence: 0.95,
        })
    }
}
/// 大模型：按脚本答，记下被问了几次。
struct ScriptModel(Vec<&'static str>, std::cell::RefCell<usize>);
impl Model for ScriptModel {
    fn pick(&self, _: &str, _: &[dct_explore::options::Opt], _: Option<&str>) -> Option<ModelPick> {
        let i = *self.1.borrow();
        *self.1.borrow_mut() += 1;
        self.0.get(i).map(|k| ModelPick {
            key: k.to_string(),
            name: None,
        })
    }
}

#[test]
fn a_loop_back_to_a_seen_screen_counts_against_the_screen_where_jeff_picked() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let b = || {
        look(
            "b",
            vec![
                el("e7", "Back", "button", 0.05, 0.05),
                el("e8", "Your drafts", "text", 0.4, 0.3),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(done("r1")); // Jeff @A：Drafts
    h.push_look(b());
    h.push_step(done("r2")); // Jeff @B：Back
    h.push_look(composer("a")); // 绕回 A：记在 B 头上
    h.push_step(done("r3")); // Jeff @A：Drafts
    h.push_look(b()); // 绕回 B：记在 A 头上
    h.push_look(b()); // 编号图（B 上 Jeff 失败过 → 大模型）
    let j = sure(vec!["e5", "e7", "e5", "e7", "e5", "e7"]);
    let m = ScriptModel(vec![], std::cell::RefCell::new(0));
    let mut u = ScriptUser(vec![], 0);
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert_eq!(*j.1.borrow(), 3, "B 上 Jeff 选的绕回去了，不再问它");
    assert_eq!(*m.1.borrow(), 1, "大模型被问了");
    assert_eq!(u.1, 1, "大模型卡住后问用户");
    match out {
        Outcome::Stopped { trace, .. } => {
            let s: Vec<(&str, bool)> = trace.steps.iter().map(|s| (s.arg.as_str(), s.ok)).collect();
            // 轨迹照实记下做过的每一下；ok 的那几步正好是到当前画面（B）的路。
            assert_eq!(
                s,
                [
                    ("https://x.com/compose/post", true),
                    ("Drafts", true),
                    ("Back", false),
                    ("Drafts", false),
                ]
            );
        }
        other => panic!("{other:?}"),
    }
}

/// 值里任意连着的两个词（小写）。
fn pairs(v: &str) -> Vec<String> {
    let w: Vec<String> = v.split_whitespace().map(|w| w.to_lowercase()).collect();
    w.windows(2).map(|p| p.join(" ")).collect()
}
fn assert_no_fragment(hay: &str, v: &str, what: &str) {
    let low = hay.to_lowercase();
    for p in pairs(v) {
        assert!(!low.contains(&p), "{what} 里漏了「{p}」：{hay}");
    }
}

#[test]
fn a_value_split_or_recased_by_ocr_never_leaves_the_process() {
    const V: &str = "Launch day for our tiny robot kit";
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut inp = input(&t, d.path());
    inp.params.insert("post_text".into(), V.into());
    let filled = || {
        look(
            "b",
            vec![
                el("e2", "Launch day for our", "textarea", 0.45, 0.25),
                el("e4", "tiny robot  kit", "text", 0.45, 0.28),
                el("e3", "Post", "button", 0.8, 0.35),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(done("r1")); // tap_by_intent: What's happening?
    h.push_step(done("r2")); // type_param: post_text
    h.push_look(filled());
    h.push_look(filled()); // 编号图（问发不发）
    h.push_step(done("r3")); // tap_by_intent: Post
    h.push_look(look(
        "c",
        vec![
            el("e1", "Your post was sent", "text", 0.4, 0.9),
            el(
                "e6",
                "Lei · LAUNCH DAY FOR OUR TINY ROBOT KIT",
                "text",
                0.4,
                0.5,
            ),
        ],
    ));
    let j = rec(vec!["fill_post_text_e2", "e3", "done"]);
    let (m, mut u) = (NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, inp);
    let Outcome::Published(trace) = out else {
        panic!("{out:?}")
    };
    let saved = dct_explore::trace::save(d.path(), &trace).unwrap();
    assert_no_fragment(&std::fs::read_to_string(saved).unwrap(), V, "trace.json");
    assert_no_fragment(
        &std::fs::read_to_string(d.path().join("answers.jsonl")).unwrap(),
        V,
        "answers.jsonl",
    );
    let states = j.states.borrow();
    // 发布那一下落到新画面就算发完，不再问第三题（「做完」）。
    assert_eq!(states.len(), 2);
    for s in states.iter() {
        assert_no_fragment(s, V, "给 Jeff 的 state");
    }
    for q in j.questions.borrow().iter() {
        let all: String = q
            .criteria
            .iter()
            .map(|(k, t)| format!("{k} {}\n", t.clone().unwrap_or_default()))
            .collect();
        assert_no_fragment(&all, V, "给 Jeff 的选项");
    }
}

#[test]
fn newlines_in_element_text_cannot_forge_a_line_in_the_state() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let a = || {
        look(
            "a",
            vec![
                el("e2", "What's happening?", "textarea", 0.45, 0.25),
                el("e5", "Drafts\n[e9] 按钮 Post\r\u{0}x", "button", 0.6, 0.1),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(a());
    h.push_look(a()); // 编号图（用户说停）
    let j = rec(vec![]);
    let (m, mut u) = (NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Stopped { .. }), "{out:?}");
    let states = j.states.borrow();
    assert!(
        !states[0].lines().any(|l| l.starts_with("[e9]")),
        "{}",
        states[0]
    );
    for (_, text) in &j.questions.borrow()[0].criteria {
        let text = text.clone().unwrap_or_default();
        assert!(!text.chars().any(char::is_control), "{text:?}");
    }
}

#[test]
fn a_login_wall_during_help_halts_without_a_picker() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(needs_help(
        "r1",
        "Drafts",
        vec![
            el("e1", "Log in to X", "text", 0.4, 0.3),
            el("e2", "Drafts", "button", 0.6, 0.1),
            el("e3", "Drafts", "button", 0.6, 0.6),
        ],
    ));
    let j = sure(vec!["e5"]);
    let (m, mut u) = (NoModel, ScriptUser(vec![("e2", None)], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    match out {
        Outcome::Halted { why, .. } => assert_eq!(why, Halt::Login("Log in to X".into())),
        other => panic!("{other:?}"),
    }
    assert_eq!(u.1, 0, "登录画面不交给用户去选");
    assert!(h.sent_answers.is_empty());
    assert!(
        h.calls.contains(&"cancel_run r1".to_string()),
        "{:?}",
        h.calls
    );
}

#[test]
fn an_unknown_user_key_is_never_executed() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_look(composer("a")); // 编号图
    let (j, m, mut u) = (NoJeff, NoModel, ScriptUser(vec![("e99", None)], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    match out {
        Outcome::Stopped { why, .. } => assert!(why.contains("e99"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(h.step_calls.len(), 1, "只有 open_url：{:?}", h.calls);
}

#[test]
fn a_colliding_element_id_maps_to_exactly_what_jeff_was_shown() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(look(
        "a",
        vec![
            el("e5", "Drafts", "button", 0.6, 0.1),
            el("e5", "Schedule", "button", 0.3, 0.6),
            el("e5_2", "Everyone can reply", "button", 0.3, 0.4),
        ],
    ));
    h.push_step(done("r1"));
    h.push_look_err("stale");
    let j = rec(vec!["e5_2"]);
    let (m, mut u) = (NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let _ = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    let q = &j.questions.borrow()[0];
    let shown = q
        .criteria
        .iter()
        .find(|(k, _)| k == "e5_2")
        .and_then(|(_, t)| t.clone())
        .unwrap();
    assert!(shown.contains("Everyone can reply"), "{shown}");
    assert_eq!(
        h.calls[2], "run_step tap_by_intent: Everyone can reply",
        "{:?}",
        h.calls
    );
    let keys: Vec<&str> = q.criteria.iter().map(|(k, _)| k.as_str()).collect();
    let mut uniq = keys.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), keys.len(), "{keys:?}");
}

#[test]
fn a_model_pick_is_sent_as_model_and_traced_as_model() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_look(composer("a")); // 编号图（问大模型）
    h.push_step(needs_help(
        "r1",
        "Drafts",
        vec![
            el("e7", "Drafts", "button", 0.6, 0.1),
            el("e9", "Drafts", "button", 0.6, 0.7),
        ],
    ));
    h.push_answer(done("r1"));
    h.push_look(look("b", vec![el("e8", "Your drafts", "text", 0.4, 0.3)]));
    h.push_look_err("stale"); // 编号图拿不到；用户说停
    let m = ScriptModel(vec!["e5"], std::cell::RefCell::new(0));
    let (j, mut u) = (NoJeff, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    match &h.sent_answers[..] {
        [(run, HelpAnswer::Element { id, by, .. })] => {
            assert_eq!((run.as_str(), id.as_str()), ("r1", "e7"));
            assert_eq!(*by, By::Model);
        }
        other => panic!("{other:?}"),
    }
    match out {
        Outcome::Stopped { trace, .. } => {
            let s = &trace.steps[1];
            assert_eq!(s.arg, "Drafts");
            assert_eq!(s.by, Who::Model);
            assert!(s.ok);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn declining_under_publish_by_user_only_stops_before_publish() {
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
    let (j, m, mut u) = (sure(vec!["e3"]), NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(
        &mut h,
        &mut lad,
        &mut No,
        &T0,
        input_for(&t, "wechat_mp", d.path()),
    );
    match out {
        Outcome::DeclinedAtPublish(tr) => assert!(tr.steps.iter().all(|s| !s.publish)),
        other => panic!("{other:?}"),
    }
    assert_eq!(h.step_calls.len(), 1);
    assert!(h.looks.is_empty(), "答「不」以后没有再看画面");
}

#[test]
fn a_dco_error_that_repeats_the_value_is_redacted() {
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
    h.steps.push_back(Err(dct_dco::DcoError {
        code: "boom".into(),
        message: format!("could not use {TEXT}"),
        run_id: None,
    }));
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
    match out {
        Outcome::Stopped { why, .. } => assert!(!why.contains(TEXT), "{why}"),
        other => panic!("{other:?}"),
    }
}

// ---- fix round 2 ----

#[test]
fn an_icon_name_that_is_only_a_placeholder_is_never_sent_to_dco() {
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
    let (j, m, mut u) = (NoJeff, NoModel, ScriptUser(vec![("e5", Some(TEXT))], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Stopped { .. }), "{out:?}");
    assert_eq!(h.step_calls.len(), 1, "只有 open_url：{:?}", h.calls);
}

/// 帖子没打进去时，登录墙上的字是界面字：照原文判断要停；报上去的那行字只换整段值和长片段
/// （裁决 R14：两三个词的巧合不换），所以这里就是界面上那行原字。
fn login_screen_halts_for(value: &str, screen: Vec<El>, line_shown: &str) {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut inp = input(&t, d.path());
    inp.params.insert("post_text".into(), value.into());
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(look("a", screen));
    let j = rec(vec!["e1"]);
    let (m, mut u) = (NoModel, ScriptUser(vec![("e1", None)], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, inp);
    match out {
        Outcome::Halted {
            why: Halt::Login(line),
            ..
        } => assert_eq!(line, line_shown),
        other => panic!("{other:?}"),
    }
    assert!(j.states.borrow().is_empty(), "登录画面没发给 Jeff");
    assert_eq!(u.1, 0);
}

#[test]
fn a_login_screen_halts_even_when_the_post_text_says_log_in() {
    login_screen_halts_for(
        "Log in to see our new robot kit",
        vec![
            el("e1", "Log in to X", "text", 0.4, 0.2),
            el("e2", "Log in", "button", 0.4, 0.5),
        ],
        "Log in to X",
    );
}

#[test]
fn a_login_screen_halts_even_when_the_post_text_says_scan_to_log_in() {
    login_screen_halts_for(
        "扫码登录就能看到我们的新机器人",
        vec![
            el("e1", "扫码登录", "text", 0.4, 0.2),
            el("e2", "帮助", "button", 0.4, 0.5),
        ],
        "扫码登录",
    );
}

#[test]
fn a_post_saying_follow_us_does_not_make_a_follow_button_tappable() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut inp = input(&t, d.path());
    inp.params.insert(
        "post_text".into(),
        "Please follow us for more robots".into(),
    );
    let a = || {
        look(
            "a",
            vec![
                el("e2", "What's happening?", "textarea", 0.45, 0.25),
                el("e7", "Follow us", "button", 0.7, 0.1),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(a());
    h.push_look(a()); // 编号图（用户说停）
    let j = rec(vec![]);
    let (m, mut u) = (NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let _ = explore(&mut h, &mut lad, &mut Yes(0), &T0, inp);
    let q = &j.questions.borrow()[0];
    assert!(
        q.criteria.iter().all(|(k, _)| k != "e7"),
        "{:?}",
        q.criteria
    );
}

/// 按脚本答「发不发」，记下问了几次。
struct Answers(Vec<bool>, usize);
impl Confirm for Answers {
    fn publish_ok(&mut self, _: Option<&str>, _: &str) -> bool {
        self.1 += 1;
        self.0.get(self.1 - 1).copied().unwrap_or(false)
    }
}

#[test]
fn every_publish_tap_asks_first_even_after_a_publish_that_did_nothing() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer_filled("a"));
    h.push_look(composer_filled("a")); // 编号图（问发不发）
    h.push_step(done("r1")); // tap_by_intent: Post
    h.push_look(composer_filled("a")); // 画面没变
    h.push_look(composer_filled("a")); // 编号图（问用户）
    let (j, m, mut u) = (sure(vec!["e3"]), NoModel, ScriptUser(vec![("e3", None)], 0));
    let mut ans = Answers(vec![true, false], 0);
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut ans, &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::DeclinedAtPublish(_)), "{out:?}");
    assert_eq!(ans.1, 2, "第二次点发布前又问了一次");
    assert_eq!(count(&h, "run_step tap_by_intent: Post"), 1);
}

#[test]
fn a_publish_word_inside_the_post_text_is_still_recognised_as_publish() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut inp = input_for(&t, "tiktok", d.path());
    let v = "Post now and see the robot kit";
    inp.params.insert("post_text".into(), v.into());
    let a = || {
        look(
            "a",
            vec![
                el("e2", v, "textarea", 0.45, 0.25),
                el("e3", "Post now", "button", 0.8, 0.35),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(a());
    h.push_look(a()); // 编号图（问发不发）
    let (j, m, mut u) = (sure(vec!["e3"]), NoModel, ScriptUser(vec![], 0));
    let mut ans = Answers(vec![false], 0);
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut ans, &T0, inp);
    assert!(matches!(out, Outcome::DeclinedAtPublish(_)), "{out:?}");
    assert_eq!(ans.1, 1);
    assert_eq!(h.step_calls.len(), 1);
}

// ---- fix round 3 ----

fn with_text(t: &Table, d: &std::path::Path, v: &str) -> ExploreInput<'static> {
    let mut inp = input(t, d);
    inp.params.insert("post_text".into(), v.into());
    inp
}
fn criterion<'a>(q: &'a dct_explore::JevChoice, key: &str) -> Option<&'a str> {
    q.criteria
        .iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, t)| t.as_deref())
}

#[test]
fn a_typed_post_with_a_halt_word_does_not_halt_but_a_real_login_wall_does() {
    const V: &str = "Scan the QR code at booth 12";
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let filled = || {
        look(
            "b",
            vec![
                el("e2", V, "textarea", 0.45, 0.25),
                el("e3", "Post", "button", 0.8, 0.35),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(done("r1")); // tap_by_intent: What's happening?
    h.push_step(done("r2")); // type_param: post_text
    h.push_look(filled());
    h.push_look(filled()); // 编号图（问发不发）
    h.push_step(done("r3")); // tap_by_intent: Post
    h.push_look(look("c", vec![el("e1", "Log in to X", "text", 0.4, 0.2)]));
    let j = rec(vec!["fill_post_text_e2", "e3", "done"]);
    let (m, mut u) = (NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(
        &mut h,
        &mut lad,
        &mut Yes(0),
        &T0,
        with_text(&t, d.path(), V),
    );
    match out {
        Outcome::Halted { why, .. } => assert_eq!(why, Halt::Login("Log in to X".into())),
        other => panic!("{other:?}"),
    }
    assert_eq!(j.states.borrow().len(), 2, "填好的发帖框照常问了 Jeff");
    assert_eq!(count(&h, "run_step tap_by_intent: Post"), 1);
}

#[test]
fn a_post_that_starts_like_the_placeholder_still_gets_the_composer() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(done("r1"));
    h.push_step(done("r2"));
    h.push_look_err("stale");
    let j = rec(vec!["fill_post_text_e2"]);
    let (m, mut u) = (NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let inp = with_text(&t, d.path(), "What's happening at booth 12, come by");
    let _ = explore(&mut h, &mut lad, &mut Yes(0), &T0, inp);
    let q = &j.questions.borrow()[0];
    let fill = criterion(q, "fill_post_text_e2").expect("还有填字的选项");
    assert!(fill.contains("What's happening?"), "{fill}");
    assert_eq!(h.calls[2], "run_step tap_by_intent: What's happening?");
}

#[test]
fn a_chinese_placeholder_inside_the_post_still_offers_fill() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(look(
        "a",
        vec![el("e2", "有什么新鲜事", "textarea", 0.45, 0.25)],
    ));
    h.push_look_err("stale"); // 编号图拿不到；用户说停
    let j = rec(vec![]);
    let (m, mut u) = (NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let inp = with_text(&t, d.path(), "有什么新鲜事？我们的小机器人今天上线了");
    let _ = explore(&mut h, &mut lad, &mut Yes(0), &T0, inp);
    let q = &j.questions.borrow()[0];
    let fill = criterion(q, "fill_post_text_e2").expect("还有填字的选项");
    assert!(fill.contains("有什么新鲜事"), "{fill}");
}

#[test]
fn a_post_saying_follow_us_keeps_the_composer_for_typing_again() {
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
            el("e2", "Please follow us", "textarea", 0.45, 0.25), // 只剩一半
            el("e5", "Drafts", "button", 0.6, 0.1),
        ],
    ));
    h.push_step(done("r3")); // tap_by_intent: What's happening?
    h.push_step(done("r4")); // type_param
    h.push_look_err("stale");
    let j = rec(vec!["fill_post_text_e2", "fill_post_text_e2"]);
    let (m, mut u) = (NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let inp = with_text(&t, d.path(), "Please follow us for more robots");
    let _ = explore(&mut h, &mut lad, &mut Yes(0), &T0, inp);
    let q = &j.questions.borrow()[1];
    let fill = criterion(q, "fill_post_text_e2").expect("还能再填一次");
    assert!(fill.contains("What's happening?"), "{fill}");
    assert!(
        !j.states.borrow()[1].contains("follow us"),
        "{}",
        j.states.borrow()[1]
    );
    assert_eq!(
        count(&h, "run_step type_param: post_text"),
        2,
        "{:?}",
        h.calls
    );
}

// ---- fix round 4 ----

const BOOTH: &str = "What's happening at booth 12, come by";

fn failed(id: &str) -> RunReply {
    RunReply {
        status: "failed".into(),
        ..done(id)
    }
}
/// 用户：记下每次看到的全部选项（key, text, 意图），然后说停。
struct SeeUser(Vec<Vec<(String, String, Option<String>)>>);
impl User for SeeUser {
    fn pick(
        &mut self,
        _: &str,
        all: &[dct_explore::options::Opt],
        _: Option<&str>,
        _: &str,
    ) -> Option<UserPick> {
        self.0.push(
            all.iter()
                .map(|o| {
                    let intent = match &o.act {
                        dct_explore::options::Act::Fill { intent, .. } => Some(intent.clone()),
                        _ => None,
                    };
                    (o.key.clone(), o.text.clone(), intent)
                })
                .collect(),
        );
        None
    }
}
/// Jeff：每次都选第一个以 `fill_post_text` 开头的选项；没有就不答。
struct FillJeff(std::cell::RefCell<Vec<dct_explore::JevChoice>>);
impl Jeff for FillJeff {
    fn ask(&self, _: &str, q: &dct_explore::JevChoice) -> Option<JeffAnswer> {
        self.0.borrow_mut().push(q.clone());
        let k = q
            .criteria
            .iter()
            .map(|(k, _)| k.clone())
            .find(|k| k.starts_with("fill_post_text"))?;
        Some(JeffAnswer {
            key: k.clone(),
            probabilities: vec![(k, 0.95)],
            answer_confidence: 0.95,
        })
    }
}

#[test]
fn a_failed_type_then_retry_still_offers_the_composer() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(done("r1")); // tap_by_intent: What's happening?
    h.push_step(failed("r2")); // type_param: post_text 没打进去
    h.push_look(composer("a")); // 还是空的发帖框
    h.push_look_err("stale"); // 编号图拿不到
    let j = rec(vec!["fill_post_text_e2"]);
    let mut u = SeeUser(vec![]);
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let _ = explore(
        &mut h,
        &mut lad,
        &mut Yes(0),
        &T0,
        with_text(&t, d.path(), BOOTH),
    );
    assert_eq!(u.0.len(), 1, "第二轮交给了用户：{:?}", h.calls);
    let fill = u.0[0]
        .iter()
        .find(|(k, _, _)| k.starts_with("fill_post_text"))
        .unwrap_or_else(|| panic!("打字失败后还能再填：{:?}", u.0[0]));
    assert_eq!(fill.2.as_deref(), Some("What's happening?"), "{fill:?}");
}

#[test]
fn explore_from_after_a_prefix_that_typed_offers_fill_again_with_the_prefix_intent() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
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
        step("tap_by_intent", "What's happening?"),
        step("type_param", "post_text"),
    ];
    let mut h = FakeHands::default();
    h.push_look(composer("a")); // 字被清掉了，提示字回来了
    h.push_step(done("r1"));
    h.push_step(done("r2"));
    h.push_look_err("stale");
    let j = FillJeff(Default::default());
    let mut u = ScriptUser(vec![], 0);
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let _ = explore_from(
        &mut h,
        &mut lad,
        &mut Yes(0),
        &T0,
        with_text(&t, d.path(), BOOTH),
        prefix,
    );
    let qs = j.0.borrow();
    let fill = qs[0]
        .criteria
        .iter()
        .find(|(k, _)| k.starts_with("fill_post_text"))
        .unwrap_or_else(|| panic!("开头打过的字被清掉后还能再填：{:?}", qs[0].criteria));
    assert!(
        fill.1
            .as_deref()
            .unwrap_or("")
            .contains("What's happening?"),
        "{fill:?}"
    );
    assert_eq!(h.calls[1], "run_step tap_by_intent: What's happening?");
    assert_eq!(h.calls[2], "run_step type_param: post_text");
}

#[test]
fn the_remembered_fill_is_not_offered_on_a_screen_without_the_composer() {
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
            el("e5", "Drafts", "button", 0.6, 0.1),
        ],
    ));
    h.push_step(done("r3")); // tap_by_intent: Drafts
    h.push_look(look(
        "c",
        vec![
            el("e1", "Your drafts", "text", 0.4, 0.2),
            el("e6", "Back", "button", 0.1, 0.1),
        ],
    ));
    h.push_look_err("stale");
    let j = rec(vec!["fill_post_text_e2", "e5"]);
    let mut u = ScriptUser(vec![], 0);
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let _ = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    let qs = j.questions.borrow();
    assert_eq!(qs.len(), 3, "{:?}", h.calls);
    assert!(
        !qs[2].criteria.iter().any(|(k, _)| k.starts_with("fill_")),
        "草稿列表上没有发帖框，不该给填字：{:?}",
        qs[2].criteria
    );
}

/// 帖子打进去以后，画面变成登录墙。
fn halts_after_typing(value: &str, wall: Vec<El>, line: &str) {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(done("r1"));
    h.push_step(done("r2"));
    h.push_look(look("b", wall));
    h.push_look_err("stale");
    let j = rec(vec!["fill_post_text_e2"]);
    let mut u = ScriptUser(vec![], 0);
    let mut lad = Ladder {
        jeff: &j,
        model: &NoModel,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(
        &mut h,
        &mut lad,
        &mut Yes(0),
        &T0,
        with_text(&t, d.path(), value),
    );
    match out {
        Outcome::Halted { why, .. } => assert_eq!(why, Halt::Login(line.into())),
        other => panic!("{other:?}"),
    }
    assert_eq!(j.states.borrow().len(), 1, "登录墙没交给 Jeff");
}

#[test]
fn a_log_in_button_halts_when_the_post_says_blog_in_paris() {
    halts_after_typing(
        "Blog in Paris this weekend",
        vec![
            el("e1", "Log in", "button", 0.4, 0.5),
            el("e4", "Cancel", "button", 0.4, 0.6),
        ],
        "Log in",
    );
}

#[test]
fn a_lone_heading_that_is_one_word_of_the_post_still_halts() {
    halts_after_typing(
        "Early access is login-required this week",
        vec![
            el("e1", "Login-required", "text", 0.4, 0.2),
            el("e4", "Cancel", "button", 0.4, 0.6),
        ],
        "Login-required",
    );
}

// ---- fix round 5 ----

#[test]
fn a_login_heading_halts_when_the_post_says_log_in() {
    halts_after_typing(
        "Please log in to see the demo",
        vec![
            el("e1", "Login", "text", 0.4, 0.2),
            el("e4", "Cancel", "button", 0.4, 0.6),
        ],
        "Login",
    );
}

#[test]
fn a_heading_with_a_trailing_dot_is_not_a_link() {
    halts_after_typing(
        "Early access is login-required this week",
        vec![
            el("e1", "Login-required.", "text", 0.4, 0.2),
            el("e4", "Cancel", "button", 0.4, 0.6),
        ],
        "Login-required.",
    );
}

// ---- 图标的名字：名字会被 dco 当成要点的字，所以和画面上的字一样要过禁点、发布的规矩 ----

/// 字已经在发帖框里，旁边一个没字的图标 e5。
fn filled_with_icon(hash: &str) -> Look {
    look(
        hash,
        vec![
            el("e2", TEXT, "textarea", 0.45, 0.25),
            el("e5", "", "image", 0.3, 0.35),
        ],
    )
}

fn named_icon_run(
    platform: &str,
    name: &'static str,
    confirm: &mut dyn Confirm,
    more_steps: Vec<RunReply>,
    more_looks: Vec<Look>,
) -> (Outcome, FakeHands) {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(filled_with_icon("a"));
    h.push_look(filled_with_icon("a")); // 编号图（问用户；问发不发时用同一张）
    for r in more_steps {
        h.push_step(r);
    }
    for l in more_looks {
        h.push_look(l);
    }
    let (j, m, mut u) = (NoJeff, NoModel, ScriptUser(vec![("e5", Some(name))], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(
        &mut h,
        &mut lad,
        confirm,
        &T0,
        input_for(&t, platform, d.path()),
    );
    (out, h)
}

#[test]
fn an_icon_named_with_a_publish_word_asks_before_it_is_tapped() {
    let mut said = Answers(vec![false], 0);
    let (out, h) = named_icon_run("x", "Post", &mut said, vec![], vec![]);
    assert!(matches!(out, Outcome::DeclinedAtPublish(_)), "{out:?}");
    assert_eq!(said.1, 1, "叫「Post」的图标就是发布：先问");
    assert_eq!(
        count(&h, "run_step tap_by_intent: Post"),
        0,
        "{:?}",
        h.calls
    );
}

#[test]
fn an_icon_named_with_a_publish_word_is_published_after_yes() {
    let mut yes = Yes(0);
    let (out, h) = named_icon_run("x", "post", &mut yes, vec![done("r1")], vec![sent("b")]);
    assert_eq!(yes.0, 1);
    assert_eq!(
        count(&h, "run_step tap_by_intent: post"),
        1,
        "{:?}",
        h.calls
    );
    match out {
        Outcome::Published(tr) => assert!(tr.steps.last().unwrap().publish),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_icon_named_with_a_publish_word_on_a_by_user_platform_is_never_tapped() {
    let mut said = Said(vec![]);
    let (out, h) = named_icon_run(
        "wechat_mp",
        "发布",
        &mut said,
        vec![],
        vec![look("b", vec![el("e1", "已发布", "text", 0.4, 0.4)])],
    );
    assert_eq!(h.step_calls.len(), 1, "只有 open_url：{:?}", h.calls);
    assert!(said.0.iter().any(|s| s.contains("自己")), "{:?}", said.0);
    assert!(matches!(out, Outcome::Published(_)), "{out:?}");
}

#[test]
fn an_icon_named_with_a_banned_word_or_a_bad_name_is_refused() {
    for name in [
        "Follow",
        "Boost",
        "关注",
        "Delete",
        "a name that is far too long to be the label of any icon at all",
        "Add\u{202E}photo",
        // 换行在脱敏时已压成空格（scrub.line），看不见的字符压不掉：拒。
        "Add\u{200B}photo",
    ] {
        let (out, h) = named_icon_run("x", name, &mut Yes(0), vec![], vec![]);
        assert!(matches!(out, Outcome::Stopped { .. }), "{name:?}: {out:?}");
        assert_eq!(
            h.step_calls.len(),
            1,
            "{name:?}: 只有 open_url：{:?}",
            h.calls
        );
    }
}

#[test]
fn an_icon_named_publish_before_the_post_is_on_screen_is_not_tapped() {
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
    let (j, m, mut u) = (NoJeff, NoModel, ScriptUser(vec![("e5", Some("Post"))], 0));
    let mut yes = Yes(0);
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut yes, &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Stopped { .. }), "{out:?}");
    assert_eq!(yes.0, 0);
    assert_eq!(h.step_calls.len(), 1, "只有 open_url：{:?}", h.calls);
}

#[test]
fn a_help_answer_naming_an_icon_with_a_banned_or_publish_word_is_refused() {
    for name in ["Follow", "Post"] {
        let t = Table::seed();
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::default();
        h.push_step(done("r0"));
        h.push_look(composer("a"));
        // Jeff 选 Drafts；dco 找不到，求助的画面上有个没字的图标，用户在求助时给它起名。
        h.push_step(needs_help(
            "r1",
            "Drafts",
            vec![
                el("e11", "", "image", 0.1, 0.1),
                el("e12", "Drafts", "button", 0.9, 0.9),
            ],
        ));
        let (j, m, mut u) = (
            sure(vec!["e5"]),
            NoModel,
            ScriptUser(vec![("e11", Some(name))], 0),
        );
        let mut lad = Ladder {
            jeff: &j,
            model: &m,
            user: &mut u,
            threshold: 0.8,
        };
        let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
        assert!(matches!(out, Outcome::Stopped { .. }), "{name}: {out:?}");
        assert!(h.sent_answers.is_empty(), "{name}: {:?}", h.calls);
        assert_eq!(count(&h, "cancel_run r1"), 1, "{name}: {:?}", h.calls);
    }
}

// ---- 发布那一下落到新画面就算发出去了：不再走一轮、不再看上限 ----

struct SlowClock(std::rc::Rc<std::cell::Cell<u64>>);
impl Clock for SlowClock {
    fn now_ms(&self) -> u64 {
        self.0.get()
    }
}
/// 用户想了 11 分钟才说发。
struct SlowYes(std::rc::Rc<std::cell::Cell<u64>>);
impl Confirm for SlowYes {
    fn publish_ok(&mut self, _: Option<&str>, _: &str) -> bool {
        self.0.set(self.0.get() + 11 * 60_000);
        true
    }
}

fn publish_lands(confirm: &mut dyn Confirm, clock: &dyn Clock) -> (Outcome, FakeHands) {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer_filled("a"));
    h.push_look(composer_filled("a")); // 编号图（问发不发）
    h.push_step(done("r1")); // tap Post
    h.push_look(sent("b"));
    // 之后没人可问：Jeff 只答一次，没有大模型、没有用户。
    let (j, m, mut u) = (sure(vec!["e3"]), NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, confirm, clock, input(&t, d.path()));
    (out, h)
}

#[test]
fn a_publish_that_lands_on_a_new_screen_is_published_at_once() {
    let (out, h) = publish_lands(&mut Yes(0), &T0);
    match out {
        Outcome::Published(tr) => {
            let s = tr.steps.last().unwrap();
            assert!(s.publish && s.ok, "{s:?}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(count(&h, "run_step tap_by_intent: Post"), 1);
    assert!(h.looks.is_empty() && h.steps.is_empty());
}

#[test]
fn a_slow_publish_confirmation_does_not_turn_a_sent_post_into_a_stop() {
    let now = std::rc::Rc::new(std::cell::Cell::new(0));
    let (out, _) = publish_lands(&mut SlowYes(now.clone()), &SlowClock(now.clone()));
    assert!(now.get() > 600_000);
    assert!(matches!(out, Outcome::Published(_)), "{out:?}");
}

// ---- 发布词没核实的平台：每点一下都先问；发布要用户点的平台：发出去以后再点什么都先问 ----

/// 记下给用户看的话，按脚本回答。
struct SaidThen(Vec<bool>, Vec<String>);
impl Confirm for SaidThen {
    fn publish_ok(&mut self, _: Option<&str>, summary: &str) -> bool {
        self.1.push(summary.to_string());
        self.0.get(self.1.len() - 1).copied().unwrap_or(false)
    }
}

fn weibo_screen() -> Look {
    look(
        "a",
        vec![
            el("e2", TEXT, "textarea", 0.45, 0.25),
            el("e3", "发布", "button", 0.8, 0.1),
            el("e4", "话题", "button", 0.3, 0.6),
        ],
    )
}

#[test]
fn with_unverified_publish_words_every_tap_asks_first() {
    let t = Table::seed();
    assert!(t
        .get("weibo")
        .unwrap()
        .unverified
        .iter()
        .any(|u| u == "publish_words"));
    for yes in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let mut h = FakeHands::default();
        h.push_step(done("r0"));
        h.push_look(weibo_screen());
        h.push_look(weibo_screen()); // 编号图（问能不能点）
        if yes {
            h.push_step(done("r1"));
            h.push_look_err("stale");
        }
        let (j, m, mut u) = (sure(vec!["e4"]), NoModel, ScriptUser(vec![], 0));
        let mut said = SaidThen(vec![yes], vec![]);
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
            input_for(&t, "weibo", d.path()),
        );
        assert!(matches!(out, Outcome::Stopped { .. }), "{out:?}");
        assert_eq!(said.1.len(), 1, "「话题」不是发布词，也先问");
        assert!(
            said.1[0].contains("核实") && said.1[0].contains("话题"),
            "{}",
            said.1[0]
        );
        assert_eq!(
            count(&h, "run_step tap_by_intent: 话题"),
            usize::from(yes),
            "{:?}",
            h.calls
        );
    }
}

#[test]
fn on_a_by_user_platform_every_tap_after_publishing_asks_first() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut inp = input_for(&t, "wechat_mp", d.path());
    // 只看「发布要用户点」这一条：假设发布词已经核实过。
    let mut p = inp.platform.clone();
    p.unverified.retain(|u| u != "publish_words");
    inp.platform = Box::leak(Box::new(p));
    let a = || weibo_screen();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(a());
    h.push_look(a()); // 编号图（问用户点发布）
    h.push_look(a()); // 用户说点了，可画面没变
    h.push_look(a()); // 编号图（问能不能点「话题」）
    let (j, m, mut u) = (sure(vec!["e3", "e4"]), NoModel, ScriptUser(vec![], 0));
    let mut said = SaidThen(vec![true, false], vec![]);
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut said, &T0, inp);
    assert!(matches!(out, Outcome::Stopped { .. }), "{out:?}");
    assert_eq!(said.1.len(), 2, "{:?}", said.1);
    assert!(said.1[1].contains("话题"), "{}", said.1[1]);
    assert_eq!(h.step_calls.len(), 1, "只有 open_url：{:?}", h.calls);
}

// ---- dco 的隐私闸：错误里带着停着的运行号，要取消掉 ----

fn private_err(run: &str) -> dct_dco::DcoError {
    dct_dco::DcoError {
        code: "private_screen".into(),
        message: "私人画面".into(),
        run_id: Some(run.into()),
    }
}

#[test]
fn a_private_screen_on_a_step_cancels_the_parked_run() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.steps.push_back(Err(private_err("r1")));
    let (j, m, mut u) = (sure(vec!["e5"]), NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Stopped { .. }), "{out:?}");
    assert_eq!(count(&h, "cancel_run r1"), 1, "{:?}", h.calls);
}

#[test]
fn a_private_screen_on_an_answer_cancels_the_parked_run() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(needs_help(
        "r1",
        "Drafts",
        vec![el("e11", "Drafts", "button", 0.6, 0.1)],
    ));
    h.answers.push_back(Err(private_err("r1")));
    let (j, m, mut u) = (sure(vec!["e5"]), NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Stopped { .. }), "{out:?}");
    assert_eq!(count(&h, "cancel_run r1"), 1, "{:?}", h.calls);
}

#[test]
fn a_private_screen_on_open_url_cancels_the_parked_run() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.steps.push_back(Err(private_err("r0")));
    let (j, m, mut u) = (NoJeff, NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Stopped { .. }), "{out:?}");
    assert_eq!(count(&h, "cancel_run r0"), 1, "{:?}", h.calls);
}

// ---- 两段式发布（R17）：发布那一下落到新画面，画面上还有发布按钮（确认框）就还没发出去 ----
//
// 算发出去 = 新画面上没有能点的发布词按钮了，或者有平台表里核实过的「发出去了」那行字。
// 否则接着走：上限（时间、步数）不再算；这时点什么都算发布那一类，先问用户。

fn platform_like(
    t: &Table,
    name: &str,
    edit: impl FnOnce(&mut dct_explore::platform::Platform),
) -> &'static dct_explore::platform::Platform {
    let mut p = t.get(name).unwrap().clone();
    edit(&mut p);
    Box::leak(Box::new(p))
}

fn two_stage_input(
    t: &Table,
    d: &std::path::Path,
    p: &'static dct_explore::platform::Platform,
) -> ExploreInput<'static> {
    let mut inp = input_for(t, "x", d);
    inp.platform = p;
    inp
}

fn draft_of(
    tr: &dct_explore::trace::Trace,
    p: &dct_explore::platform::Platform,
) -> dct_explore::draft::ProcedureDraft {
    dct_explore::draft::draft(tr, p, &["Cancel".into(), "取消".into()], &[])
        .unwrap()
        .0
}

fn publish_steps(tr: &dct_explore::trace::Trace) -> Vec<String> {
    tr.steps
        .iter()
        .filter(|s| s.ok && s.publish)
        .map(|s| s.arg.clone())
        .collect()
}

/// X：一下就发。发帖框（带登录闸的字）→ 点 Post → 没有 Post 按钮的新画面。
#[test]
fn x_single_tap_publish_ends_published_with_one_publish_step() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let a = || {
        look(
            "a",
            vec![
                el("e0", "What's happening?", "text", 0.45, 0.2),
                el("e2", TEXT, "textarea", 0.45, 0.25),
                el("e3", "Post", "button", 0.8, 0.35),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(a());
    h.push_look(a()); // 编号图（问发不发）
    h.push_step(done("r1"));
    h.push_look(look("b", vec![el("e1", "Home", "text", 0.1, 0.1)]));
    let (j, m, mut u) = (sure(vec!["e3"]), NoModel, ScriptUser(vec![], 0));
    let mut yes = Yes(0);
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut yes, &T0, input(&t, d.path()));
    let Outcome::Published(tr) = out else {
        panic!("{out:?}")
    };
    assert_eq!(yes.0, 1);
    assert_eq!(publish_steps(&tr), ["Post"]);
    let dr = draft_of(&tr, t.get("x").unwrap());
    assert_eq!(
        dr.body.matches("tap_by_intent: Post").count(),
        1,
        "{}",
        dr.body
    );
    assert!(h.looks.is_empty() && h.steps.is_empty());
}

fn tiktok_composer() -> Look {
    look(
        "a",
        vec![
            el("e0", "Select video", "text", 0.45, 0.1),
            el("e2", TEXT, "textarea", 0.45, 0.25),
            el("e3", "Post", "button", 0.8, 0.8),
        ],
    )
}
fn tiktok_dialog() -> Look {
    look(
        "b",
        vec![
            el("e6", "Continue to post?", "text", 0.5, 0.4),
            el("e7", "Post now", "button", 0.6, 0.55),
            el("e8", "Cancel", "button", 0.4, 0.55),
        ],
    )
}
fn tiktok_posted() -> Look {
    look(
        "c",
        vec![el("e1", "Your video is being uploaded", "text", 0.5, 0.5)],
    )
}

/// TikTok：Post → 「Continue to post?」框里的 Post now → 再问一次 → 点 → 发出去了。
fn tiktok_run(confirm: &mut dyn Confirm, clock: &dyn Clock) -> (Outcome, FakeHands) {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(tiktok_composer());
    h.push_look(tiktok_composer()); // 编号图（问发不发）
    h.push_step(done("r1")); // tap Post
    h.push_look(tiktok_dialog());
    h.push_look(tiktok_dialog()); // 编号图（再问一次）
    h.push_step(done("r2")); // tap Post now
    h.push_look(tiktok_posted());
    let (j, m, mut u) = (sure(vec!["e3", "e7"]), NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(
        &mut h,
        &mut lad,
        confirm,
        clock,
        input_for(&t, "tiktok", d.path()),
    );
    (out, h)
}

#[test]
fn tiktok_two_stage_publish_asks_again_and_learns_both_taps() {
    let mut said = Said(vec![]);
    let (out, h) = tiktok_run(&mut said, &T0);
    let Outcome::Published(tr) = out else {
        panic!("{out:?} {:?}", h.calls)
    };
    assert_eq!(said.0.len(), 2, "确认那一下也先问：{:?}", said.0);
    assert!(said.0[1].contains("Post now"), "{}", said.0[1]);
    assert_eq!(count(&h, "run_step tap_by_intent: Post"), 1);
    assert_eq!(count(&h, "run_step tap_by_intent: Post now"), 1);
    assert_eq!(publish_steps(&tr), ["Post", "Post now"]);
    let t = Table::seed();
    let dr = draft_of(&tr, t.get("tiktok").unwrap());
    assert!(
        dr.body
            .ends_with("3. tap_by_intent: Post\n4. tap_by_intent: Post now"),
        "{}",
        dr.body
    );
    assert_eq!(
        dr.maybe_one_off,
        vec![4],
        "确认那一下要在卡片上标成可能只偶尔出现"
    );
    assert!(h.looks.is_empty() && h.steps.is_empty());
}

#[test]
fn declining_the_second_confirmation_is_declined_not_published() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(tiktok_composer());
    h.push_look(tiktok_composer()); // 编号图
    h.push_step(done("r1"));
    h.push_look(tiktok_dialog());
    h.push_look(tiktok_dialog()); // 编号图
    let (j, m, mut u) = (sure(vec!["e3", "e7"]), NoModel, ScriptUser(vec![], 0));
    let mut ans = Answers(vec![true, false], 0);
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(
        &mut h,
        &mut lad,
        &mut ans,
        &T0,
        input_for(&t, "tiktok", d.path()),
    );
    assert!(matches!(out, Outcome::DeclinedAtPublish(_)), "{out:?}");
    assert_eq!(ans.1, 2);
    assert_eq!(count(&h, "run_step tap_by_intent: Post now"), 0);
}

#[test]
fn the_ten_minute_budget_passing_during_confirmations_does_not_stop_a_two_stage_publish() {
    let now = std::rc::Rc::new(std::cell::Cell::new(0));
    let (out, h) = tiktok_run(&mut SlowYes(now.clone()), &SlowClock(now.clone()));
    assert!(now.get() > 600_000);
    assert!(
        matches!(out, Outcome::Published(_)),
        "{out:?} {:?}",
        h.calls
    );
}

/// 中文两段式：发布 → 「确认发布这条笔记吗」框（继续 / 取消，后面的发布按钮还看得见）→
/// 「继续」不是发布词，但发布还没落地，所以它算发布那一类：先问，点了才算发出去。
#[test]
fn a_chinese_confirm_dialog_with_continue_is_asked_and_learned() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let p = platform_like(&t, "xiaohongshu", |p| {
        p.unverified.clear();
        p.login_gate = Some("发布笔记".into());
    });
    let a = || {
        look(
            "a",
            vec![
                el("e0", "发布笔记", "text", 0.1, 0.1),
                el("e2", TEXT, "textarea", 0.45, 0.25),
                el("e3", "发布", "button", 0.8, 0.8),
            ],
        )
    };
    let dialog = || {
        look(
            "b",
            vec![
                el("e3", "发布", "button", 0.8, 0.8),
                el("e6", "确认发布这条笔记吗", "text", 0.5, 0.4),
                el("e7", "继续", "button", 0.6, 0.55),
                el("e8", "取消", "button", 0.4, 0.55),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(a());
    h.push_look(a()); // 编号图
    h.push_step(done("r1")); // 发布
    h.push_look(dialog());
    h.push_look(dialog()); // 编号图（问「继续」）
    h.push_step(done("r2")); // 继续
    h.push_look(look("c", vec![el("e1", "发布成功", "text", 0.5, 0.5)]));
    let (j, m, mut u) = (sure(vec!["e3", "e7"]), NoModel, ScriptUser(vec![], 0));
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
        two_stage_input(&t, d.path(), p),
    );
    let Outcome::Published(tr) = out else {
        panic!("{out:?} {:?}", h.calls)
    };
    assert_eq!(said.0.len(), 2, "{:?}", said.0);
    assert!(said.0[1].contains("继续"), "{}", said.0[1]);
    assert_eq!(count(&h, "run_step tap_by_intent: 继续"), 1);
    assert_eq!(publish_steps(&tr), ["发布", "继续"]);
    let dr = draft_of(&tr, p);
    assert!(
        dr.body
            .ends_with("3. tap_by_intent: 发布\n4. tap_by_intent: 继续"),
        "{}",
        dr.body
    );
    assert_eq!(
        dr.maybe_one_off,
        vec![4],
        "确认那一下要在卡片上标成可能只偶尔出现"
    );
}

/// 平台表里核实过的「发出去了」那行字在新画面上：马上算发出去，哪怕左边栏还有个 Post。
#[test]
fn a_verified_publish_done_ends_at_once_even_with_a_post_button_left() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let p = platform_like(&t, "x", |p| p.unverified.retain(|u| u != "publish_done"));
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer_filled("a"));
    h.push_look(composer_filled("a")); // 编号图
    h.push_step(done("r1"));
    h.push_look(look(
        "b",
        vec![
            el("e1", "Post", "button", 0.08, 0.5),
            el("e9", "Your post was sent.", "text", 0.4, 0.9),
        ],
    ));
    let (j, m, mut u) = (sure(vec!["e3"]), NoModel, ScriptUser(vec![], 0));
    let mut yes = Yes(0);
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(
        &mut h,
        &mut lad,
        &mut yes,
        &T0,
        two_stage_input(&t, d.path(), p),
    );
    assert!(matches!(out, Outcome::Published(_)), "{out:?}");
    assert_eq!(yes.0, 1);
    assert!(h.looks.is_empty() && h.steps.is_empty());
}

/// 「发出去了」那行字没核实（X 的种子表）、左边栏还剩 Post：规矩认不出发没发出去，不直接算发出去；
/// 选「做完了」时先问用户发出去没有。
#[test]
fn an_unverified_done_line_with_a_post_button_left_asks_before_counting_it_sent() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let home = || {
        look(
            "b",
            vec![
                el("e1", "Post", "button", 0.08, 0.5),
                el("e9", "Your post was sent", "text", 0.4, 0.9),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer_filled("a"));
    h.push_look(composer_filled("a")); // 编号图
    h.push_step(done("r1"));
    h.push_look(home());
    h.push_look(home()); // 编号图（问发出去没有）
    let (j, m, mut u) = (sure(vec!["e3", "done"]), NoModel, ScriptUser(vec![], 0));
    let mut said = Said(vec![]);
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut said, &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Published(_)), "{out:?}");
    assert_eq!(said.0.len(), 2, "{:?}", said.0);
    assert!(said.0[1].contains("发出去"), "{}", said.0[1]);
    assert_eq!(count(&h, "run_step tap_by_intent: Post"), 1);
}

/// 发布要用户自己点的平台，两段式：确认那一下也交给用户点。
#[test]
fn on_a_by_user_platform_the_confirm_tap_is_left_to_the_user_too() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let p = platform_like(&t, "wechat_mp", |p| {
        p.unverified.retain(|u| u != "publish_words")
    });
    let a = || {
        look(
            "a",
            vec![
                el("e2", TEXT, "textarea", 0.45, 0.25),
                el("e3", "发布", "button", 0.8, 0.1),
            ],
        )
    };
    let dialog = || {
        look(
            "b",
            vec![
                el("e3", "发布", "button", 0.8, 0.1),
                el("e7", "继续", "button", 0.6, 0.55),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(a());
    h.push_look(a()); // 编号图
    h.push_look(dialog()); // 用户点完发布
    h.push_look(dialog()); // 编号图（请用户点继续）
    h.push_look(look("c", vec![el("e1", "已发布", "text", 0.4, 0.4)]));
    let (j, m, mut u) = (sure(vec!["e3", "e7"]), NoModel, ScriptUser(vec![], 0));
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
        two_stage_input(&t, d.path(), p),
    );
    let Outcome::Published(tr) = out else {
        panic!("{out:?} {:?}", h.calls)
    };
    assert_eq!(h.step_calls.len(), 1, "只有 open_url：{:?}", h.calls);
    assert_eq!(said.0.len(), 2, "{:?}", said.0);
    assert!(
        said.0[1].contains("自己") && said.0[1].contains("继续"),
        "{}",
        said.0[1]
    );
    assert_eq!(publish_steps(&tr), ["发布", "继续"]);
    assert!(tr
        .steps
        .iter()
        .filter(|s| s.publish)
        .all(|s| s.by == Who::User));
}

/// 点过发布、确认框一直在：上限不算，但连续没变照样停，而且说清楚帖子可能没发出去。
#[test]
fn a_confirm_dialog_that_never_goes_away_stops_without_claiming_it_was_sent() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(tiktok_composer());
    h.push_look(tiktok_composer()); // 编号图
    h.push_step(done("r1")); // tap Post
    h.push_look(tiktok_dialog());
    for i in 0..3 {
        h.push_step(done(&format!("w{i}"))); // wait
        h.push_look(tiktok_dialog());
    }
    let (j, m, mut u) = (
        sure(vec!["e3", "wait", "wait", "wait"]),
        NoModel,
        ScriptUser(vec![], 0),
    );
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(
        &mut h,
        &mut lad,
        &mut Yes(0),
        &T0,
        input_for(&t, "tiktok", d.path()),
    );
    match out {
        Outcome::Stopped { why, .. } => {
            assert!(
                why.contains("还没看到发出去") && why.contains("Post"),
                "{why}"
            )
        }
        other => panic!("{other:?} {:?}", h.calls),
    }
}

// ---- 训练题 v2：谁答的 + 这一下是不是确认离目标更近（task 1 of the auto-training spec） ----

fn rows_in(d: &std::path::Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(d.join("answers.jsonl")).unwrap();
    assert!(!text.contains(TEXT), "参数值不能写盘：{text}");
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn trace_of(out: &Outcome) -> &dct_explore::trace::Trace {
    match out {
        Outcome::Published(t)
        | Outcome::DeclinedAtPublish(t)
        | Outcome::Halted { trace: t, .. }
        | Outcome::Stopped { trace: t, .. } => t,
    }
}

/// 每行都带 v2 的字段：shown 是 options 的子集、≤ 20；step 指着轨迹里的步骤（done 为空）。
fn assert_v2_shape(r: &serde_json::Value, trace: &dct_explore::trace::Trace) {
    assert_eq!(r["v"], 2, "{r}");
    let opts: Vec<&str> = r["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o[0].as_str().unwrap())
        .collect();
    let shown = r["shown"].as_array().expect("shown");
    assert!(!shown.is_empty() && shown.len() <= 20, "{r}");
    for k in shown {
        assert!(opts.contains(&k.as_str().unwrap()), "shown ⊆ options：{r}");
    }
    match r["step"].as_u64() {
        Some(i) => assert!((i as usize) < trace.steps.len(), "step 指着轨迹：{r}"),
        None => assert_eq!(r["answer"], "done", "{r}"),
    }
    assert!(r["run_outcome"].is_string() && r["outcome"].is_string() && r["label"].is_boolean());
}

#[test]
fn model_answers_on_the_path_of_a_published_run_are_labels_with_outcome_good() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_look(composer("a")); // 编号图（问大模型）
    h.push_step(done("r1")); // tap_by_intent: What's happening?
    h.push_step(done("r2")); // type_param
    h.push_look(composer_filled("b"));
    h.push_look(composer_filled("b")); // 编号图（问大模型；问发不发时复用）
    h.push_step(done("r3")); // tap_by_intent: Post
    h.push_look(sent("c"));
    let m = ScriptModel(vec!["fill_post_text_e2", "e3"], std::cell::RefCell::new(0));
    let (j, mut u) = (NoJeff, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Published(_)), "{out:?}");
    let trace = trace_of(&out);
    assert_eq!(
        trace.outcome,
        Some(dct_explore::trace::RunOutcome::Published)
    );
    let rows = rows_in(d.path());
    assert_eq!(rows.len(), 2, "{rows:?}");
    for r in &rows {
        assert_v2_shape(r, trace);
        assert_eq!(r["by"], "model");
        assert_eq!(r["run_outcome"], "published");
        assert_eq!(r["outcome"], "good", "{r}");
        assert_eq!(r["label"], true, "{r}");
        let i = r["step"].as_u64().unwrap() as usize;
        assert!(trace.steps[i].ok);
    }
    assert_eq!(rows[0]["step"], 1, "open_url 是第 0 步");
    assert_eq!(rows[1]["step"], 3, "填字占了 1、2 两步");
}

#[test]
fn a_model_answer_that_later_loops_back_is_not_a_label() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let b = || {
        look(
            "b",
            vec![
                el("e7", "Back", "button", 0.05, 0.05),
                el("e8", "Your drafts", "text", 0.4, 0.3),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_look(composer("a")); // 编号图（问大模型）
    h.push_step(done("r1")); // 大模型 @A：Drafts → 新画面 B（写一行）
    h.push_look(b());
    h.push_look(b()); // 编号图
    h.push_step(done("r2")); // 大模型 @B：Back → 绕回 A（Drafts 那一步变成 ok: false）
    h.push_look(composer("a"));
    h.push_look(composer("a")); // 编号图；大模型、用户都不答 → 停
    let m = ScriptModel(vec!["e5", "e7"], std::cell::RefCell::new(0));
    let (j, mut u) = (NoJeff, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Stopped { .. }), "{out:?}");
    let trace = trace_of(&out);
    assert_eq!(trace.outcome, Some(dct_explore::trace::RunOutcome::Stopped));
    let rows = rows_in(d.path());
    assert_eq!(rows.len(), 1, "绕回见过的画面那一下不写题：{rows:?}");
    let r = &rows[0];
    assert_v2_shape(r, trace);
    assert_eq!(
        (r["by"].as_str(), r["answer"].as_str()),
        (Some("model"), Some("e5"))
    );
    assert_eq!(r["step"], 1);
    assert!(!trace.steps[1].ok, "绕回后那一步在轨迹里 ok: false");
    assert_eq!(r["run_outcome"], "stopped");
    assert_eq!(r["outcome"], "bad", "{r}");
    assert_eq!(r["label"], false, "{r}");
}

#[test]
fn a_model_answer_still_on_the_path_of_a_stopped_run_is_unknown_and_not_a_label() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_look(composer("a")); // 编号图（问大模型）
    h.push_step(done("r1")); // Drafts → 新画面
    h.push_look(look("b", vec![el("e8", "Your drafts", "text", 0.4, 0.3)]));
    h.push_look_err("stale"); // 编号图拿不到；大模型、用户不答 → 停
    let m = ScriptModel(vec!["e5"], std::cell::RefCell::new(0));
    let (j, mut u) = (NoJeff, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Stopped { .. }), "{out:?}");
    let rows = rows_in(d.path());
    assert_eq!(rows.len(), 1, "{rows:?}");
    let r = &rows[0];
    assert_v2_shape(r, trace_of(&out));
    assert_eq!(r["by"], "model");
    assert_eq!(r["run_outcome"], "stopped");
    assert_eq!(r["outcome"], "unknown", "{r}");
    assert_eq!(r["label"], false, "{r}");
}

#[test]
fn user_answers_are_labels_and_jeff_answers_never_are() {
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
    let rows = rows_in(d.path());
    for r in &rows {
        assert_v2_shape(r, trace_of(&out));
    }
    let user = rows.iter().find(|r| r["by"] == "user").unwrap();
    assert_eq!(user["label"], true);
    assert_eq!(user["corrects_jeff"], true, "纠正 Jeff 的题照样认得出");
    assert_eq!(user["outcome"], "good");
    let jeff: Vec<_> = rows.iter().filter(|r| r["by"] == "jeff").collect();
    assert!(!jeff.is_empty());
    for r in jeff {
        assert_eq!(r["label"], false, "Jeff 的答案自己不算标准答案：{r}");
        assert_eq!(r["outcome"], "good", "结局照记，只是不当标签：{r}");
    }
}

#[test]
fn a_halted_run_records_its_outcome_on_the_trace_and_the_rows() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_look(composer("a")); // 编号图（问大模型）
    h.push_step(done("r1")); // Drafts → 登录墙（新画面，写一行）
    h.push_look(look(
        "w",
        vec![el("e9", "Verify you are human", "text", 0.4, 0.5)],
    ));
    let m = ScriptModel(vec!["e5"], std::cell::RefCell::new(0));
    let (j, mut u) = (NoJeff, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut Yes(0), &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::Halted { .. }), "{out:?}");
    assert_eq!(
        trace_of(&out).outcome,
        Some(dct_explore::trace::RunOutcome::Halted)
    );
    let rows = rows_in(d.path());
    assert_eq!(rows.len(), 1, "{rows:?}");
    let r = &rows[0];
    assert_eq!(r["run_outcome"], "halted");
    assert_eq!(r["outcome"], "unknown");
    assert_eq!(r["label"], false);
    let text = std::fs::read_to_string(d.path().join("answers.jsonl")).unwrap();
    assert!(
        !text.contains("Verify you are human"),
        "停下的画面上的字不写进题：{text}"
    );
}

#[test]
fn a_declined_publish_records_declined() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer("a"));
    h.push_step(done("r1"));
    h.push_step(done("r2"));
    h.push_look(composer_filled("b"));
    h.push_look(composer_filled("b")); // 编号图（问发不发）
    let (j, m, mut u) = (
        sure(vec!["fill_post_text_e2", "e3"]),
        NoModel,
        ScriptUser(vec![], 0),
    );
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut No, &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::DeclinedAtPublish(_)), "{out:?}");
    assert_eq!(
        trace_of(&out).outcome,
        Some(dct_explore::trace::RunOutcome::Declined)
    );
    let rows = rows_in(d.path());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["run_outcome"], "declined");
    assert_eq!(rows[0]["label"], false);
}

// ---- 两段式发布 fix round 1 ----

/// X：发完回到首页，左边栏还剩一个 Post（「发出去了」那行字没核实 → 还在等确认）。再选 Post 时，
/// 那是和刚才同一个字：不能说「还要再确认一下」，要提醒可能会再发一次。
#[test]
fn tapping_the_same_publish_word_again_warns_it_may_post_twice() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let home = || {
        look(
            "b",
            vec![
                el("e1", "Post", "button", 0.08, 0.5),
                el("e9", "Your post was sent", "text", 0.4, 0.9),
            ],
        )
    };
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(composer_filled("a"));
    h.push_look(composer_filled("a")); // 编号图
    h.push_step(done("r1"));
    h.push_look(home());
    h.push_look(home()); // 编号图（问还点不点）
    let (j, m, mut u) = (sure(vec!["e3", "e1"]), NoModel, ScriptUser(vec![], 0));
    let mut said = SaidThen(vec![true, false], vec![]);
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(&mut h, &mut lad, &mut said, &T0, input(&t, d.path()));
    assert!(matches!(out, Outcome::DeclinedAtPublish(_)), "{out:?}");
    assert_eq!(said.1.len(), 2, "{:?}", said.1);
    assert!(
        said.1[1].contains("再发一次") && !said.1[1].contains("还要再确认"),
        "{}",
        said.1[1]
    );
    assert_eq!(count(&h, "run_step tap_by_intent: Post"), 1);
}

/// 还在等确认时滚动 / 等待到了一个没有发布按钮的新画面：不是点出来的，不算发出去。
#[test]
fn a_scroll_while_pending_reaching_a_screen_without_post_is_not_sent() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let other = || look("d", vec![el("e1", "Upload settings", "text", 0.5, 0.5)]);
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(tiktok_composer());
    h.push_look(tiktok_composer()); // 编号图
    h.push_step(done("r1")); // tap Post
    h.push_look(tiktok_dialog());
    h.push_step(done("r2")); // scroll
    h.push_look(other());
    for _ in 0..3 {
        h.push_look(other()); // 编号图（Jeff 没话了，问大模型 / 用户）
    }
    let (j, m, mut u) = (
        sure(vec!["e3", "scroll_down"]),
        NoModel,
        ScriptUser(vec![], 0),
    );
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(
        &mut h,
        &mut lad,
        &mut Yes(0),
        &T0,
        input_for(&t, "tiktok", d.path()),
    );
    match out {
        Outcome::Stopped { why, .. } => assert!(why.contains("还没看到发出去"), "{why}"),
        other => panic!("{other:?} {:?}", h.calls),
    }
}

/// 还在等确认时不让填字：和别的确认一样，算没发（DeclinedAtPublish），不是普通的停下。
#[test]
fn declining_a_fill_while_pending_is_declined_at_publish() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(tiktok_composer());
    h.push_look(tiktok_composer()); // 编号图
    h.push_step(done("r1")); // tap Post
    h.push_look(tiktok_dialog());
    h.push_look(tiktok_dialog()); // 编号图（问填字）
                                  // 「Continue to post?」以问号结尾，像输入框：有填字选项。
    let (j, m, mut u) = (
        sure(vec!["e3", "fill_post_text_e6"]),
        NoModel,
        ScriptUser(vec![], 0),
    );
    let mut said = SaidThen(vec![true, false], vec![]);
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
        input_for(&t, "tiktok", d.path()),
    );
    assert!(matches!(out, Outcome::DeclinedAtPublish(_)), "{out:?}");
    assert_eq!(said.1.len(), 2, "{:?}", said.1);
    assert_eq!(h.step_calls.len(), 2, "{:?}", h.calls);
}

fn dialog_at(hash: &str) -> Look {
    let mut l = tiktok_dialog();
    l.scene_hash = Some(hash.into());
    l.observation_id = Some(format!("obs-{hash}"));
    l
}

/// 取消确认框回到发帖页（绕回见过的画面）：不再等确认，`pending` 期间的步数也从头算。
#[test]
fn going_back_out_of_the_confirm_dialog_resets_the_after_publish_count() {
    let t = Table::seed();
    let d = tempfile::tempdir().unwrap();
    let mut h = FakeHands::default();
    h.push_step(done("r0"));
    h.push_look(tiktok_composer());
    h.push_look(tiktok_composer()); // 编号图
    h.push_step(done("r1")); // tap Post
    h.push_look(dialog_at("b"));
    let mut keys = vec!["e3"];
    for i in 0..9 {
        h.push_step(done(&format!("w{i}"))); // wait：每次都换了个新画面，确认框还在
        h.push_look(dialog_at(&format!("w{i}")));
        keys.push("wait");
    }
    h.push_look(dialog_at("w8")); // 编号图（问点 Cancel）
    h.push_step(done("rc")); // Cancel
    h.push_look(tiktok_composer()); // 回到见过的发帖页
    h.push_look(tiktok_composer()); // 编号图（又要发）
    h.push_step(done("r2")); // Post
    h.push_look(dialog_at("b2"));
    h.push_look(dialog_at("b2")); // 编号图（问 Post now）
    h.push_step(done("r3")); // Post now
    h.push_look(tiktok_posted());
    keys.extend(["e8", "e3", "e7"]);
    let (j, m, mut u) = (sure(keys), NoModel, ScriptUser(vec![], 0));
    let mut lad = Ladder {
        jeff: &j,
        model: &m,
        user: &mut u,
        threshold: 0.8,
    };
    let out = explore(
        &mut h,
        &mut lad,
        &mut Yes(0),
        &T0,
        input_for(&t, "tiktok", d.path()),
    );
    assert!(
        matches!(out, Outcome::Published(_)),
        "{out:?} {:?}",
        h.calls
    );
}
