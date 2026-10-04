use crate::play::{Clock, Dco, DcoError, Profile, Region, Seen, TapAt, TapSettle, TapSettleReq};
use crate::scene::*;
use crate::screen::Element;
use serde_json::json;
use std::collections::VecDeque;

fn er(code: &str) -> DcoError {
    DcoError { code: code.into(), message: format!("msg-{code}") }
}

fn el(text: &str, frac: Option<[u16; 4]>) -> Element {
    Element { id: "e".into(), text: text.into(), frac }
}

fn seen(texts: &[&str]) -> Seen {
    Seen { snapshot_id: "s".into(), observation_id: None, elements: texts.iter().map(|t| el(t, None)).collect() }
}

struct Fake {
    /// 每次 see_text 取一个；空了就重复 `default_seen`。
    sees: VecDeque<Result<Seen, DcoError>>,
    default_seen: Seen,
    captures: usize,
    taps: Vec<(u16, u16, usize)>,
    tap_result: Result<TapAt, DcoError>,
    moods: Vec<(String, Option<String>)>,
    settle_asked: Vec<Option<TapSettleReq>>,
}
impl Fake {
    fn new() -> Fake {
        Fake { sees: VecDeque::new(), default_seen: seen(&["1", "MENU"]), captures: 0, taps: vec![], tap_result: Ok(TapAt { kind: "no_text".into(), text: String::new(), settle: None }), moods: vec![], settle_asked: vec![] }
    }
}
impl Dco for Fake {
    fn read_grid(&mut self, _: &Profile) -> Result<crate::GridRead, DcoError> {
        panic!("scene 不读棋盘")
    }
    fn swipe(&mut self, _: &Profile, _: (f64, f64), _: (f64, f64)) -> Result<(), DcoError> {
        panic!("scene 不划")
    }
    fn see_text(&mut self, _: &Profile) -> Result<Seen, DcoError> {
        self.sees.pop_front().unwrap_or_else(|| Ok(self.default_seen.clone()))
    }
    fn capture(&mut self, _: &Profile) -> Result<Vec<u8>, DcoError> {
        self.captures += 1;
        Ok(vec![1, 2, 3])
    }
    fn tap_at(&mut self, _: &Profile, x: u16, y: u16, avoid: &[Region], settle: Option<&TapSettleReq>) -> Result<TapAt, DcoError> {
        self.settle_asked.push(settle.cloned());
        self.taps.push((x, y, avoid.len()));
        self.tap_result.clone()
    }
    fn show_status(&mut self, _: &str) {}
    fn show_status_with(&mut self, state: &str, text: Option<&str>, _: Option<&str>) {
        self.moods.push((state.into(), text.map(Into::into)));
    }
}

struct Clk(u64);
impl Clock for Clk {
    fn now_ms(&self) -> u64 {
        self.0
    }
    fn sleep_ms(&mut self, ms: u64) {
        self.0 += ms;
    }
}

struct Script {
    picks: std::cell::RefCell<VecDeque<Option<(f64, f64)>>>,
    asked: std::cell::Cell<usize>,
}
impl Script {
    fn new(p: &[(f64, f64)]) -> Script {
        Script { picks: std::cell::RefCell::new(p.iter().map(|x| Some(*x)).collect()), asked: 0.into() }
    }
}
impl Vision for Script {
    fn pick(&self, _: &[u8], _: &[(u16, u16)]) -> Result<VisionAnswer, VisionFail> {
        self.asked.set(self.asked.get() + 1);
        let p = self.picks.borrow_mut().pop_front().ok_or(VisionFail::Silent)?;
        Ok(match p {
            Some((x, y)) => VisionAnswer { pick: Some(Pick { name: "木箱".into(), x, y, why: "近".into() }), raw: String::new(), model: "m".into(), tokens: Some((10, 2)), image_bytes: Some(3), image_note: None },
            None => VisionAnswer { pick: None, raw: "乱码".into(), model: "m".into(), tokens: None, image_bytes: None, image_note: None },
        })
    }
}

fn profile() -> Profile {
    Profile { window: json!({"app": "x"}), region: [0.0, 0.0, 1.0, 1.0], rows: 1, cols: 1, extra: json!({}), fixed_rgb: vec![], match_de: 24.0, weights: Default::default(), level_pattern: None, theme: None }
}

fn opts(max: usize) -> SceneOptions {
    SceneOptions { max_steps: max, dry_run: false, no_tap: vec![], game: "g".into() }
}

fn run(d: &mut Fake, v: &Script, o: &SceneOptions) -> (SceneSummary, Vec<SceneStep>) {
    let mut steps = vec![];
    let s = scene(d, &mut Clk(0), &profile(), v, o, &mut |st| steps.push(st));
    (s, steps)
}

// ---- 纯函数 ----

#[test]
fn parse_pick_reads_plain_fenced_and_thinking_answers() {
    let p = parse_pick(r#"{"name":"门","x":0.5,"y":0.25,"why":"能开"}"#).unwrap();
    assert_eq!((p.name.as_str(), p.x, p.y, p.why.as_str()), ("门", 0.5, 0.25, "能开"));
    assert!(parse_pick("```json\n{\"name\":\"a\",\"x\":0,\"y\":1,\"why\":\"\"}\n```").is_some());
    let t = parse_pick("<think>我想 {\"x\":9} 吧</think>{\"name\":\"a\",\"x\":0.1,\"y\":0.2,\"why\":\"w\"}").unwrap();
    assert_eq!((t.x, t.y), (0.1, 0.2));
}

#[test]
fn parse_pick_rejects_out_of_range_missing_and_garbage() {
    assert!(parse_pick(r#"{"name":"a","x":1.2,"y":0.5}"#).is_none());
    assert!(parse_pick(r#"{"name":"a","x":-0.1,"y":0.5}"#).is_none());
    assert!(parse_pick(r#"{"name":"a","x":0.5}"#).is_none());
    assert!(parse_pick(r#"{"x":0.5,"y":0.5}"#).is_none());
    assert!(parse_pick(r#"{"name":"a","x":"0.5","y":0.5}"#).is_none());
    assert!(parse_pick("没有 json").is_none());
    assert!(parse_pick("} {").is_none());
}

#[test]
fn to_bp_rounds_and_clamps() {
    assert_eq!((to_bp(0.0), to_bp(1.0), to_bp(0.5), to_bp(0.12345)), (0, 10000, 5000, 1235));
    assert_eq!((to_bp(-3.0), to_bp(7.0)), (0, 10000));
}

#[test]
fn in_regions_includes_the_edges() {
    let r = [Region { x_bp: 7800, y_bp: 0, w_bp: 2200, h_bp: 2000 }];
    assert!(in_regions(7800, 0, &r) && in_regions(10000, 2000, &r) && in_regions(9000, 1000, &r));
    assert!(!in_regions(7799, 1000, &r) && !in_regions(9000, 2001, &r));
    assert!(!in_regions(5, 5, &[]));
}

#[test]
fn near_forbidden_finds_words_within_the_box_only() {
    // 中心在 (5000, 5000)
    let hint = el("HINT", Some([4500, 4800, 1000, 400]));
    assert_eq!(near_forbidden(std::slice::from_ref(&hint), 5000, 5000).as_deref(), Some("HINT"));
    assert_eq!(near_forbidden(std::slice::from_ref(&hint), 5699, 5899).as_deref(), Some("HINT"));
    assert_eq!(near_forbidden(std::slice::from_ref(&hint), 5700, 5000), None, "7% 是开区间");
    assert_eq!(near_forbidden(std::slice::from_ref(&hint), 5000, 5900), None, "9% 是开区间");
    assert_eq!(near_forbidden(&[el("提示", Some([4900, 4900, 200, 200]))], 5000, 5000).as_deref(), Some("提示"));
    assert_eq!(near_forbidden(&[el("Buy more", Some([4900, 4900, 200, 200]))], 5000, 5000).as_deref(), Some("Buy more"));
    assert_eq!(near_forbidden(&[el("木箱", Some([4900, 4900, 200, 200]))], 5000, 5000), None);
    assert_eq!(near_forbidden(&[el("HINT", None)], 5000, 5000), None, "没位置的元素不判断");
}

#[test]
fn a_plus_only_counts_when_it_is_the_whole_text() {
    let near = |t: &str| near_forbidden(&[el(t, Some([4900, 4900, 200, 200]))], 5000, 5000);
    assert_eq!(near("+").as_deref(), Some("+"));
    assert_eq!(near(" + ").as_deref(), Some(" + "));
    assert_eq!(near("5+3"), None);
    assert_eq!(near("+10"), None);
}

#[test]
fn quantise_groups_by_two_percent() {
    assert_eq!(quantise(0, 0), quantise(199, 199));
    assert_ne!(quantise(199, 0), quantise(200, 0));
    assert_eq!(quantise(1800, 6000), (9, 30));
}

// ---- 循环 ----

#[test]
fn a_good_step_taps_and_writes_a_full_record() {
    let mut d = Fake::new();
    d.sees = VecDeque::from([Ok(seen(&["5", "4/6", "MENU"])), Ok(seen(&["5", "5/6", "MENU"]))]);
    let v = Script::new(&[(0.18, 0.6)]);
    let (s, steps) = run(&mut d, &v, &opts(1));
    assert_eq!((s.stop.clone(), s.steps, s.taps, s.asks, s.tokens_in, s.tokens_out), (SceneStop::StepsDone, 1, 1, 1, 10, 2));
    assert_eq!(d.taps, vec![(1800, 6000, 0)]);
    let r = &steps[0].record;
    assert_eq!((r["schema"].as_u64(), r["game"].as_str(), r["teacher"].as_str(), r["label"].as_str()), (Some(1), Some("g"), Some("qwen"), Some("effective")));
    assert_eq!(r["screen"]["png"], "png/0001.png");
    assert_eq!(r["screen"]["texts"], json!(["5", "4/6", "MENU"]));
    assert_eq!(r["screen"]["numbers"], json!([5]));
    assert_eq!(r["action"]["kind"], "tap_at");
    assert_eq!((r["action"]["x_bp"].as_u64(), r["action"]["y_bp"].as_u64()), (Some(1800), Some(6000)));
    assert_eq!((r["model"].as_str(), r["tokens_in"].as_u64(), r["tokens_out"].as_u64()), (Some("m"), Some(10), Some(2)));
    assert_eq!(r["image_bytes"].as_u64(), Some(3));
    assert!(r["image_note"].is_null());
    assert_eq!(r["outcome"]["changed"], true);
    assert_eq!(r["outcome"]["texts_after"], json!(["5", "5/6", "MENU"]));
    assert_eq!(steps[0].png.as_deref(), Some(&[1u8, 2, 3][..]));
    assert!(steps[0].say.contains("木箱") && steps[0].say.contains("变了"));
}

#[test]
fn an_unchanged_screen_is_noop_and_five_in_a_row_stop() {
    let mut d = Fake::new();
    let pts: Vec<(f64, f64)> = (0..6).map(|i| (0.1 + 0.1 * i as f64, 0.5)).collect();
    let v = Script::new(&pts);
    let (s, steps) = run(&mut d, &v, &opts(20));
    assert_eq!((s.stop, s.taps), (SceneStop::Noop5, 5));
    assert!(steps.iter().all(|x| x.record["label"] == "noop"));
}

#[test]
fn an_effective_step_resets_the_noop_count() {
    let mut d = Fake::new();
    // 4 步没变，第 5 步变了，再 4 步没变：不该停。
    let mut sees = VecDeque::new();
    for _ in 0..4 {
        sees.push_back(Ok(seen(&["1"])));
        sees.push_back(Ok(seen(&["1"])));
    }
    sees.push_back(Ok(seen(&["1"])));
    sees.push_back(Ok(seen(&["2"])));
    for _ in 0..4 {
        sees.push_back(Ok(seen(&["2"])));
        sees.push_back(Ok(seen(&["2"])));
    }
    d.sees = sees;
    let pts: Vec<(f64, f64)> = (0..9).map(|i| (0.05 + 0.1 * i as f64, 0.5)).collect();
    let (s, _) = run(&mut d, &Script::new(&pts), &opts(9));
    assert_eq!((s.stop, s.taps), (SceneStop::StepsDone, 9));
}

#[test]
fn a_pick_inside_a_no_tap_region_is_skipped_and_never_tapped() {
    let mut d = Fake::new();
    let mut o = opts(1);
    o.no_tap = vec![Region { x_bp: 7800, y_bp: 0, w_bp: 2200, h_bp: 2000 }];
    let (s, steps) = run(&mut d, &Script::new(&[(0.9, 0.1)]), &o);
    assert!(d.taps.is_empty());
    assert_eq!(s.taps, 0);
    assert_eq!(steps[0].record["label"], "skipped");
    assert!(steps[0].record["skip_reason"].as_str().unwrap().contains("不点"));
}

#[test]
fn a_pick_next_to_a_forbidden_word_is_skipped() {
    let mut d = Fake::new();
    d.default_seen = Seen { snapshot_id: "s".into(), observation_id: None, elements: vec![el("HINT", Some([4000, 4800, 1000, 400]))] };
    let (_, steps) = run(&mut d, &Script::new(&[(0.45, 0.5)]), &opts(1));
    assert!(d.taps.is_empty());
    assert_eq!(steps[0].record["label"], "skipped");
    assert!(steps[0].record["skip_reason"].as_str().unwrap().contains("HINT"));
}

#[test]
fn tapping_the_same_cell_twice_in_a_row_is_skipped() {
    let mut d = Fake::new();
    d.sees = VecDeque::from([Ok(seen(&["1"])), Ok(seen(&["2"]))]);
    let (_, steps) = run(&mut d, &Script::new(&[(0.5, 0.5), (0.505, 0.505)]), &opts(2));
    assert_eq!(d.taps.len(), 1);
    assert_eq!(steps[1].record["label"], "skipped");
}

#[test]
fn three_skips_in_a_row_stop_and_a_tap_in_between_resets() {
    let mut d = Fake::new();
    let mut o = opts(10);
    o.no_tap = vec![Region { x_bp: 0, y_bp: 0, w_bp: 10000, h_bp: 1000 }];
    let top = (0.5, 0.05);
    let (s, _) = run(&mut d, &Script::new(&[top, top, top]), &o);
    assert_eq!(s.stop, SceneStop::Skipped3);
    // skip, skip, tap, skip, skip, tap ... 不该停
    let mut d = Fake::new();
    d.sees = VecDeque::from([Ok(seen(&["1"])), Ok(seen(&["2"]))]);
    let (s, _) = run(&mut d, &Script::new(&[top, top, (0.5, 0.5), top, top]), &{
        let mut o = opts(5);
        o.no_tap = vec![Region { x_bp: 0, y_bp: 0, w_bp: 10000, h_bp: 1000 }];
        o
    });
    assert_eq!(s.stop, SceneStop::StepsDone);
}

#[test]
fn an_unusable_model_answer_counts_as_a_skip() {
    let mut d = Fake::new();
    let v = Script { picks: std::cell::RefCell::new(VecDeque::from([None, None, None])), asked: 0.into() };
    let (s, steps) = run(&mut d, &v, &opts(10));
    assert_eq!(s.stop, SceneStop::Skipped3);
    assert!(steps.iter().all(|x| x.record["label"] == "skipped"));
    assert!(d.taps.is_empty());
}

#[test]
fn an_old_dco_without_tap_at_stops_with_no_tap_and_nothing_tapped() {
    let mut d = Fake::new();
    d.tap_result = Err(er("unsupported"));
    let (s, steps) = run(&mut d, &Script::new(&[(0.5, 0.5), (0.2, 0.2)]), &opts(5));
    assert_eq!(s.stop, SceneStop::NoTapAt);
    assert_eq!(s.taps, 0);
    assert!(steps.is_empty() || steps.iter().all(|x| x.record["label"] != "effective" && x.record["label"] != "noop"));
    assert!(d.moods.iter().any(|(st, t)| st == "wait" && t.is_some()));
}

#[test]
fn a_private_screen_stops_before_any_capture_or_model_call() {
    let mut d = Fake::new();
    d.sees = VecDeque::from([Err(er("private_screen"))]);
    let v = Script::new(&[(0.5, 0.5)]);
    let (s, steps) = run(&mut d, &v, &opts(5));
    assert_eq!(s.stop, SceneStop::PrivateScreen);
    assert_eq!(d.captures, 0, "私人画面不能取截图");
    assert_eq!(v.asked.get(), 0, "私人画面不能问模型");
    assert!(d.taps.is_empty() && steps.is_empty());
    assert!(d.moods.iter().any(|(st, t)| st == "wait" && t.as_deref() == Some("私人画面，没操作")));
}

#[test]
fn any_see_text_failure_means_no_capture_and_no_model_call() {
    let mut d = Fake::new();
    d.sees = VecDeque::from([Err(er("dco_timeout"))]);
    let v = Script::new(&[(0.5, 0.5)]);
    let (s, _) = run(&mut d, &v, &opts(5));
    assert_eq!(s.stop, SceneStop::Dco(er("dco_timeout")));
    assert_eq!((d.captures, v.asked.get()), (0, 0));
}

#[test]
fn a_screen_that_turns_private_after_the_tap_stops() {
    let mut d = Fake::new();
    d.sees = VecDeque::from([Ok(seen(&["1"])), Err(er("private_screen"))]);
    let (s, steps) = run(&mut d, &Script::new(&[(0.5, 0.5), (0.2, 0.2)]), &opts(5));
    assert_eq!((s.stop, d.captures), (SceneStop::PrivateScreen, 1));
    assert_eq!(steps[0].record["label"], "error");
}

#[test]
fn dry_run_never_calls_tap_at_and_writes_a_dry_run_record() {
    let mut d = Fake::new();
    let mut o = opts(5);
    o.dry_run = true;
    let (s, steps) = run(&mut d, &Script::new(&[(0.5, 0.5)]), &o);
    assert_eq!(s.stop, SceneStop::DryRun);
    assert!(d.taps.is_empty());
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].record["label"], "dry_run");
}

#[test]
fn no_vision_stops_without_tapping() {
    let mut d = Fake::new();
    let (s, _) = run(&mut d, &Script::new(&[]), &opts(5));
    assert_eq!(s.stop, SceneStop::NoVision(VisionFail::Silent));
    assert!(d.taps.is_empty());
}

#[test]
fn the_step_limit_is_respected_and_no_tap_regions_are_sent_to_dco() {
    let mut d = Fake::new();
    let mut o = opts(2);
    o.no_tap = vec![Region { x_bp: 0, y_bp: 9000, w_bp: 10000, h_bp: 1000 }];
    d.sees = VecDeque::from([Ok(seen(&["1"])), Ok(seen(&["2"])), Ok(seen(&["2"])), Ok(seen(&["3"]))]);
    let (s, _) = run(&mut d, &Script::new(&[(0.1, 0.1), (0.5, 0.5), (0.9, 0.2)]), &o);
    assert_eq!((s.stop, s.taps), (SceneStop::StepsDone, 2));
    assert_eq!(d.taps.iter().map(|t| t.2).collect::<Vec<_>>(), vec![1, 1]);
}

#[test]
fn dco_refusing_a_tap_is_a_skip_not_a_crash() {
    let mut d = Fake::new();
    d.tap_result = Err(er("not_allowed"));
    let (s, steps) = run(&mut d, &Script::new(&[(0.1, 0.1), (0.3, 0.3), (0.6, 0.6)]), &opts(10));
    assert_eq!(s.stop, SceneStop::Skipped3);
    assert!(steps.iter().all(|x| x.record["label"] == "skipped"));
}

// ---- 有没有变：稳定报告优先，其次是不怕认字抖动的文字比较 ----

fn st(changed: bool, settled: bool, timed_out: bool) -> TapSettle {
    TapSettle { changed, change: 0.0, settled, timed_out, settled_ms: Some(500) }
}

#[test]
fn ocr_jitter_alone_is_not_an_effect() {
    let b = seen(&["5", "4/6", "HINT", "MENU"]);
    assert!(!effective(&b, &seen(&["5+", "4/6", "NIE", "MENU"]), None));
    assert!(!effective(&b, &seen(&["5", "4/6", "HINI", "MENU"]), None));
    assert!(!effective(&b, &seen(&["5", "4/6", "HIN", "MENU"]), None));
    assert!(!effective(&b, &b, None));
}

#[test]
fn a_number_change_is_an_effect_even_inside_a_fraction() {
    let b = seen(&["5", "4/6", "HINT", "MENU"]);
    assert!(effective(&b, &seen(&["5", "5/6", "HINT", "MENU"]), None));
    assert!(effective(&b, &seen(&["6", "4/6", "HINT", "MENU"]), None));
}

#[test]
fn a_new_real_word_is_an_effect_but_a_one_letter_variant_is_not() {
    let b = seen(&["5", "HINT", "MENU"]);
    assert!(effective(&b, &seen(&["5", "HINT", "MENU", "Congratulations"]), None));
    assert!(effective(&b, &seen(&["5", "HINT", "MENU", "Found it"]), None));
    assert!(!effective(&b, &seen(&["5", "HINT", "MEN"]), None));
    assert!(!effective(&b, &seen(&["5", "HINT", "MENUS"]), None));
}

#[test]
fn a_settled_report_decides_over_the_texts() {
    let b = seen(&["5", "HINT"]);
    let jitter = seen(&["5+", "NIE"]);
    let real = seen(&["9", "Congratulations"]);
    assert!(!effective(&b, &real, Some(&st(false, true, false))), "没变就是没变，文字再抖也不算");
    assert!(effective(&b, &jitter, Some(&st(true, true, false))), "变了就是变了");
}

#[test]
fn an_unsettled_or_timed_out_report_falls_back_to_the_text_rule() {
    let b = seen(&["5", "HINT"]);
    let jitter = seen(&["5+", "NIE"]);
    let real = seen(&["6", "HINT"]);
    assert!(!effective(&b, &jitter, Some(&st(true, true, true))));
    assert!(effective(&b, &real, Some(&st(false, true, true))));
    assert!(!effective(&b, &jitter, Some(&st(true, false, false))));
    assert!(effective(&b, &real, Some(&st(false, false, false))));
}

#[test]
fn the_loop_asks_dco_for_the_settle_and_labels_by_it() {
    let mut d = Fake::new();
    d.tap_result = Ok(TapAt { kind: "no_text".into(), text: String::new(), settle: Some(st(false, true, false)) });
    // 点前点后文字真的不同，但 dco 说没变：以 dco 为准。
    d.sees = VecDeque::from([Ok(seen(&["1"])), Ok(seen(&["2", "Congratulations"]))]);
    let (s, steps) = run(&mut d, &Script::new(&[(0.5, 0.5)]), &opts(1));
    assert_eq!(s.taps, 1);
    assert_eq!(steps[0].record["label"], "noop");
    assert_eq!(steps[0].record["outcome"]["by"], "dco_settle");
    assert_eq!(d.settle_asked, vec![Some(TAP_SETTLE)]);
}

#[test]
fn the_loop_uses_dco_changed_true_as_effective() {
    let mut d = Fake::new();
    d.tap_result = Ok(TapAt { kind: "no_text".into(), text: String::new(), settle: Some(st(true, true, false)) });
    let (_, steps) = run(&mut d, &Script::new(&[(0.5, 0.5)]), &opts(1));
    assert_eq!(steps[0].record["label"], "effective");
}
