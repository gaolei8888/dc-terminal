use crate::play::*;
use crate::GridRead;
use serde_json::{json, Value};
use std::collections::VecDeque;

fn grid(cells: &[&[u16]]) -> GridRead {
    let rows = cells.len();
    let cols = cells[0].len();
    let mut ids: Vec<u16> = cells.iter().flat_map(|r| r.iter().copied()).collect();
    ids.sort();
    ids.dedup();
    let classes: Vec<Value> = ids.iter().map(|&i| json!({"id": i, "count": cells.iter().flat_map(|r| r.iter()).filter(|&&c| c == i).count()})).collect();
    serde_json::from_value(json!({
        "rows": rows, "cols": cols, "cells": cells, "odd": vec![vec![false; cols]; rows], "classes": classes
    }))
    .unwrap()
}

struct Fake {
    reads: VecDeque<Result<GridRead, DcoError>>,
    last: Option<Result<GridRead, DcoError>>,
    swipes: Vec<((f64, f64), (f64, f64))>,
    swipe_err: Option<DcoError>,
    /// show_status 和 swipe 按先后记下来，测 play() 不发状态。
    events: Vec<String>,
    reads_taken: usize,
    texts: VecDeque<Vec<&'static str>>,
}
impl Fake {
    fn new(reads: Vec<Result<GridRead, DcoError>>) -> Fake {
        Fake { reads: reads.into(), last: None, swipes: vec![], swipe_err: None, events: vec![], reads_taken: 0, texts: VecDeque::new() }
    }
}
impl Dco for Fake {
    fn read_grid(&mut self, _: &Profile) -> Result<GridRead, DcoError> {
        self.reads_taken += 1;
        if let Some(r) = self.reads.pop_front() {
            self.last = Some(r.clone());
            return r;
        }
        self.last.clone().expect("脚本里一次读盘都没有")
    }
    fn swipe(&mut self, _: &Profile, f: (f64, f64), t: (f64, f64)) -> Result<(), DcoError> {
        if let Some(e) = self.swipe_err.clone() {
            return Err(e);
        }
        self.swipes.push((f, t));
        self.events.push("swipe".into());
        Ok(())
    }
    fn show_status(&mut self, state: &str) {
        self.events.push(state.into());
    }
    fn see_text(&mut self, _: &Profile) -> Result<Seen, DcoError> {
        match self.texts.pop_front() {
            Some(t) => Ok(Seen {
                snapshot_id: "s".into(),
                observation_id: None,
                elements: t.iter().enumerate().map(|(i, s)| crate::screen::Element { id: format!("e{}", i + 1), text: (*s).into() }).collect(),
            }),
            None => Err(DcoError { code: "unsupported".into(), message: "x".into() }),
        }
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
/// 这些测试里的选步细节（哪一步排第一）是按 Candy Crush 的那套权重写的，所以用它，不用通用默认。
fn tuned() -> crate::choose::Weights {
    crate::choose::Weights { striped: 6.0, wrapped: 8.0, bomb: 15.0, triggered: 5.0, ..Default::default() }
}
fn profile(rows: usize, cols: usize) -> Profile {
    Profile { window: json!({"app": "x"}), region: [0.2, 0.3, 0.5, 0.4], rows, cols, extra: json!({}), fixed_rgb: vec![], match_de: 24.0, weights: tuned(), level_pattern: None }
}
fn run(d: &mut Fake, max: usize, dry: bool) -> (Summary, Vec<Value>) {
    let mut log = vec![];
    let s = play(d, &mut Clk(0), &profile(3, 4), &Options { max_steps: max, dry_run: dry, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |v| log.push(v));
    (s, log)
}
fn err(code: &str) -> DcoError {
    DcoError { code: code.into(), message: format!("m-{code}") }
}

// 每个类别至少出现两次：只有一格的类别按设计算「认不出」，不能换。
const A: &[&[u16]] = &[&[1, 1, 2, 1], &[2, 3, 1, 3], &[3, 2, 3, 2]]; // 能走：(0,2) 和 (1,2) 对换，第 0 行四连
const DEAD: &[&[u16]] = &[&[1, 2, 3, 1], &[3, 1, 2, 3], &[2, 3, 1, 2]]; // 没有任何能走的步（每类都不止一格）
const MOVED: &[&[u16]] = DEAD;

#[test]
fn plays_one_move_then_stops_when_nothing_is_left() {
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(MOVED)), Ok(grid(MOVED)), Ok(grid(DEAD))]);
    // 第 1 次读：A；划；落定读两次 MOVED；下一步直接用落定的画面（MOVED）选步……MOVED 里有连线吗？没有 → NoMoves
    let (s, log) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::NoMoves);
    assert_eq!(s.steps, 1);
    assert_eq!(d.swipes.len(), 1);
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["outcome"], "moved");
    // (0,2)↔(1,2)：竖着划。x = 0.2 + 2.5/4*0.5，y 从 0.3+0.5/3*0.4 到 0.3+1.5/3*0.4
    let (f, t) = d.swipes[0];
    assert!((f.0 - 0.5125).abs() < 1e-9 && (t.0 - 0.5125).abs() < 1e-9);
    assert!((f.1 - 0.366666666).abs() < 1e-6 && (t.1 - 0.5).abs() < 1e-9);
}

#[test]
fn two_unmoved_swipes_stop_and_the_second_try_is_a_different_move() {
    let mut d = Fake::new(vec![Ok(grid(A))]); // 一直读到 A
    let (s, log) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::Stuck);
    assert_eq!(d.swipes.len(), 2);
    assert_ne!(d.swipes[0], d.swipes[1], "第二次不该重复划同一步");
    assert_eq!(log.iter().filter(|l| l["outcome"] == "no_change").count(), 2);
}

#[test]
fn a_screen_that_never_stops_changing_gives_up() {
    let a = grid(A);
    let mut reads = vec![Ok(a.clone())];
    // 两种分组来回换：连续两次读永远不一样（只换号码不算在动）
    for i in 0..200u16 {
        reads.push(Ok(if i % 2 == 0 { grid(DEAD) } else { grid(&[&[1, 2, 1, 2], &[3, 1, 2, 3], &[2, 3, 1, 3]]) }));
    }
    let mut d = Fake::new(reads);
    let (s, _) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::StillMoving);
}

#[test]
fn not_a_grid_during_the_animation_is_waited_out() {
    let mut d = Fake::new(vec![Ok(grid(A)), Err(err("not_a_grid")), Ok(grid(MOVED)), Ok(grid(MOVED)), Ok(grid(DEAD))]);
    let (s, _) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::NoMoves);
    assert_eq!(s.steps, 1);
}

#[test]
fn halted_while_settling_stops() {
    let mut d = Fake::new(vec![Ok(grid(A)), Err(err("halted"))]);
    let (s, _) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::Dco(err("halted")));
}

#[test]
fn halted_on_swipe_stops_and_logs_it() {
    let mut d = Fake::new(vec![Ok(grid(A))]);
    d.swipe_err = Some(err("halted"));
    let (s, log) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::Dco(err("halted")));
    assert_eq!(log[0]["outcome"], "stopped");
    assert_eq!(s.steps, 0, "划被拒绝，不算走了一步");
    assert_eq!(log[0]["swiped"], false);
}

#[test]
fn a_swipe_that_happened_is_marked_swiped_and_counted() {
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(MOVED)), Ok(grid(MOVED))]);
    let (s, log) = run(&mut d, 1, false);
    assert_eq!(s.steps, 1);
    assert_eq!(log[0]["swiped"], true);
}

#[test]
fn settle_never_finishing_after_a_swipe_counts_the_step_and_is_marked_swiped() {
    let mut reads = vec![Ok(grid(A))];
    // 两种分组来回换：连续两次读永远不一样（只换号码不算在动）
    for i in 0..200u16 {
        reads.push(Ok(if i % 2 == 0 { grid(DEAD) } else { grid(&[&[1, 2, 1, 2], &[3, 1, 2, 3], &[2, 3, 1, 3]]) }));
    }
    let mut d = Fake::new(reads);
    let (s, log) = run(&mut d, 10, false);
    assert_eq!((s.steps, &s.stop), (1, &Stop::StillMoving));
    assert_eq!((log[0]["outcome"].as_str(), log[0]["swiped"].as_bool()), (Some("stopped"), Some(true)));
}

#[test]
fn nothing_readable_after_a_swipe_is_no_grid_not_still_moving() {
    let mut d = Fake::new(vec![Ok(grid(A)), Err(DcoError { code: "not_a_grid".into(), message: "结果页".into() })]);
    let (s, _) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::NoGrid("结果页".into()));
    assert_eq!(s.steps, 1);
}

#[test]
fn a_clock_that_goes_backwards_does_not_panic() {
    // 每隔一次读数就往回跳一大截。
    struct Back(std::cell::Cell<u64>, u64);
    impl Clock for Back {
        fn now_ms(&self) -> u64 {
            self.0.set(self.0.get() + 1);
            if self.0.get().is_multiple_of(2) { self.1 } else { self.1 + 1_000_000 }
        }
        fn sleep_ms(&mut self, ms: u64) {
            self.1 += ms;
        }
    }
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(MOVED)), Ok(grid(MOVED))]);
    let mut log = vec![];
    let _ = play(&mut d, &mut Back(Default::default(), 0), &profile(3, 4), &Options { max_steps: 1, dry_run: false, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |v| log.push(v));
    // 读盘到落定都过了一轮，没有 panic 就行；用一个会倒着走的钟再跑一遍无法落定的情形
    let mut d = Fake::new(vec![Ok(grid(A))]);
    let _ = play(&mut d, &mut Back(Default::default(), 0), &profile(3, 4), &Options { max_steps: 2, dry_run: false, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |_| {});
}

#[test]
fn candidate_features_are_nested_under_features() {
    let mut d = Fake::new(vec![Ok(grid(A))]);
    let (_, log) = run(&mut d, 10, true);
    let c = &log[0]["candidates"][0];
    for k in ["a", "b", "score", "features"] {
        assert!(c.get(k).is_some(), "缺 {k}");
    }
    for k in ["cleared", "cascade", "striped", "wrapped", "bomb", "triggered", "special_swap", "lowest_row"] {
        assert!(c["features"].get(k).is_some(), "features 缺 {k}");
        assert!(c.get(k).is_none(), "{k} 不该在外层");
    }
}

#[test]
fn unreadable_board_at_the_start_stops() {
    let mut d = Fake::new(vec![Err(err("not_a_grid"))]);
    let (s, _) = run(&mut d, 10, false);
    assert!(matches!(s.stop, Stop::NoGrid(_)));
    assert_eq!(s.steps, 0);
}

#[test]
fn many_new_classes_mean_something_else_is_on_screen() {
    // 开始 3 类；落定后变成 5 类，而且新来的每类都不止一格（弹窗）
    let five: &[&[u16]] = &[&[1, 4, 2, 5], &[2, 3, 1, 3], &[4, 5, 3, 1]];
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(five)), Ok(grid(five))]);
    let (s, _) = run(&mut d, 10, false);
    assert!(matches!(s.stop, Stop::ClassesChanged { was: 3, now: 5 }), "{:?}", s.stop);
}

#[test]
fn one_extra_class_is_allowed() {
    // 多出来一个两格的类别（两个同种特殊糖果）还算同一个画面
    let four: &[&[u16]] = &[&[1, 1, 2, 1], &[2, 3, 1, 3], &[4, 2, 3, 4]];
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(four)), Ok(grid(four))]);
    let (s, _) = run(&mut d, 3, false);
    assert!(!matches!(s.stop, Stop::ClassesChanged { .. }), "{:?}", s.stop);
}

#[test]
fn new_one_cell_classes_are_not_counted() {
    // 彩色炸弹、条纹糖各自成了一格的类别：不算换了画面
    let six: &[&[u16]] = &[&[1, 1, 2, 1], &[2, 3, 1, 3], &[3, 2, 4, 5]];
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(six)), Ok(grid(six))]);
    let (s, _) = run(&mut d, 3, false);
    assert!(!matches!(s.stop, Stop::ClassesChanged { .. }), "{:?}", s.stop);
}

#[test]
fn first_board_baseline_ignores_one_cell_classes() {
    // 第一张盘里就有一格的类别：基线只数大类（3 个），之后多出两个大类才停
    let first: &[&[u16]] = &[&[1, 1, 2, 1], &[2, 3, 1, 3], &[3, 2, 3, 9]];
    let five: &[&[u16]] = &[&[1, 4, 2, 5], &[2, 3, 1, 3], &[4, 5, 3, 1]];
    let mut d = Fake::new(vec![Ok(grid(first)), Ok(grid(five)), Ok(grid(five))]);
    let (s, _) = run(&mut d, 10, false);
    assert!(matches!(s.stop, Stop::ClassesChanged { was: 3, now: 5 }), "{:?}", s.stop);
}

#[test]
fn dry_run_never_swipes() {
    let mut d = Fake::new(vec![Ok(grid(A))]);
    let (s, log) = run(&mut d, 10, true);
    assert_eq!(s.stop, Stop::DryRun);
    assert!(d.swipes.is_empty());
    assert_eq!(log[0]["outcome"], "dry_run");
}

#[test]
fn stops_after_the_step_limit() {
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(MOVED)), Ok(grid(MOVED))]);
    let (s, _) = run(&mut d, 1, false);
    assert_eq!(s.stop, Stop::StepsDone);
    assert_eq!(s.steps, 1);
}

#[test]
fn every_log_line_has_the_fields_the_comparison_needs() {
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(MOVED)), Ok(grid(MOVED))]);
    let (_, log) = run(&mut d, 1, false);
    for k in ["schema", "cells", "odd", "classes", "observation_id", "after_observation_id", "candidates", "chosen", "outcome", "timing_ms", "time_ms"] {
        assert!(log[0].get(k).is_some(), "缺 {k}");
    }
    assert!(!log[0]["candidates"].as_array().unwrap().is_empty());
}

#[test]
fn a_move_that_works_resets_the_unmoved_counter() {
    // 没反应一次、落定一次、再没反应一次：每次都只是「一次」，不该停。
    // 脚本：A → 划 → 一直没变(NoChange) → 换一步再划 → 变成 B 落定 → 用 B 选步 → B 划 → 没变……
    // 为了让 B 也有能走的步，B 用 A 的镜像。
    let b: &[&[u16]] = &[&[2, 1, 1, 1], &[3, 1, 2, 3], &[2, 3, 2, 3]];
    let mut script = vec![];
    for _ in 0..40 {
        script.push(Ok(grid(A)));
    }
    script.push(Ok(grid(b)));
    script.push(Ok(grid(b)));
    let mut d = Fake::new(script);
    let (s, log) = run(&mut d, 3, false);
    // 前两次划：第一次没反应，第二次（另一步）落定到 b
    let outcomes: Vec<&str> = log.iter().map(|l| l["outcome"].as_str().unwrap()).collect();
    assert_eq!(&outcomes[..2], ["no_change", "moved"], "{outcomes:?}");
    // 落定之后第 3 步没反应只算第 1 次：到了步数上限停，而不是因为早先那次凑成两次而 Stuck
    assert_eq!(outcomes.len(), 3, "{outcomes:?}");
    assert_eq!(s.stop, Stop::StepsDone);
}

#[test]
fn dry_run_and_swipe_error_records_also_carry_timing() {
    let mut d = Fake::new(vec![Ok(grid(A))]);
    let (_, dry) = run(&mut d, 10, true);
    let mut d = Fake::new(vec![Ok(grid(A))]);
    d.swipe_err = Some(err("halted"));
    let (_, bad) = run(&mut d, 10, false);
    for rec in [&dry[0], &bad[0]] {
        for k in ["schema", "cells", "odd", "classes", "observation_id", "candidates", "chosen", "outcome", "timing_ms", "time_ms"] {
            assert!(rec.get(k).is_some(), "缺 {k}");
        }
        for k in ["read", "choose", "swipe", "settle"] {
            assert!(rec["timing_ms"].get(k).is_some(), "timing_ms 缺 {k}");
        }
        assert!(rec.get("after_observation_id").is_none());
    }
}

// dco 的类别号只在一次返回里有意义：同一张没动的画面，两次读可能分组一样、号码互换。
fn relabel(cells: &[&[u16]], f: impl Fn(u16) -> u16) -> GridRead {
    let v: Vec<Vec<u16>> = cells.iter().map(|r| r.iter().map(|&c| f(c)).collect()).collect();
    let refs: Vec<&[u16]> = v.iter().map(|r| r.as_slice()).collect();
    grid(&refs)
}
fn swap12(c: u16) -> u16 {
    match c {
        1 => 2,
        2 => 1,
        x => x,
    }
}

#[test]
fn canonical_ignores_class_ids_but_not_grouping() {
    let a = vec![vec![1u16, 1, 2], vec![2, 3, 3]];
    let permuted = vec![vec![7u16, 7, 4], vec![4, 9, 9]];
    let regrouped = vec![vec![1u16, 1, 2], vec![3, 2, 3]];
    assert_eq!(canonical(&a), canonical(&permuted));
    assert_ne!(canonical(&a), canonical(&regrouped));
    // 两盘棋的号码多重集一样（各有两个 1、两个 2、两个 3），但分组不同
    assert_ne!(canonical(&[vec![1, 2, 1], vec![2, 3, 3]]), canonical(&[vec![1, 1, 2], vec![3, 2, 3]]));
}

#[test]
fn permuted_ids_on_a_changed_screen_still_settle_as_moved() {
    let mut reads = vec![Ok(grid(A))];
    for i in 0..200 {
        reads.push(Ok(if i % 2 == 0 { relabel(DEAD, |c| c) } else { relabel(DEAD, swap12) }));
    }
    let mut d = Fake::new(reads);
    let (s, log) = run(&mut d, 1, false);
    assert_ne!(s.stop, Stop::StillMoving);
    assert_eq!(log[0]["outcome"], "moved");
}

#[test]
fn permuted_ids_on_an_unchanged_screen_settle_as_no_change() {
    let mut reads = vec![Ok(grid(A))];
    for i in 0..400 {
        reads.push(Ok(if i % 2 == 0 { relabel(A, swap12) } else { relabel(A, |c| c) }));
    }
    let mut d = Fake::new(reads);
    let (s, log) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::Stuck);
    assert_eq!(log.iter().filter(|l| l["outcome"] == "no_change").count(), 2);
}

#[test]
fn failed_move_memory_survives_permuted_ids() {
    let mut reads = vec![Ok(grid(A))];
    for _ in 0..400 {
        reads.push(Ok(relabel(A, swap12)));
    }
    let mut d = Fake::new(reads);
    let (_, _) = run(&mut d, 10, false);
    assert!(d.swipes.len() >= 2);
    assert_ne!(d.swipes[0], d.swipes[1], "同分组换了号码的盘面，失败的步也得跳过");
}

const POPUP: &[&[u16]] = &[&[1, 1, 1, 1, 1], &[0, 0, 0, 0, 0], &[2, 2, 2, 2, 2], &[2, 2, 2, 2, 2], &[2, 2, 2, 2, 2], &[2, 2, 2, 2, 2], &[2, 2, 2, 2, 2], &[2, 2, 2, 2, 2], &[2, 2, 2, 2, 2]];

#[test]
fn a_first_read_that_is_mostly_one_colour_is_not_swiped() {
    let mut d = Fake::new(vec![Ok(grid(POPUP))]);
    let (s, _) = run(&mut d, 10, false);
    assert!(matches!(s.stop, Stop::NoGrid(_)), "{:?}", s.stop);
    assert_eq!(s.steps, 0);
    assert!(d.swipes.is_empty());
}

#[test]
fn a_lopsided_read_later_in_a_run_is_still_played() {
    // 第一张正常；落定后的盘大半是一个颜色（后期常见），照旧往下玩，不拦。
    let lopsided: &[&[u16]] = &[&[1, 1, 1, 1], &[1, 1, 1, 1], &[1, 2, 3, 1], &[2, 3, 2, 3]];
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(lopsided)), Ok(grid(lopsided))]);
    let (s, _) = run(&mut d, 3, false);
    assert!(s.steps >= 1, "{:?}", s.stop);
    assert!(!matches!(s.stop, Stop::NoGrid(_)), "{:?}", s.stop);
}

#[test]
fn a_step_sends_no_status_only_the_swipe() {
    // 规则选步约 1 ms，章鱼没法“想”这么短；think/look 留给以后慢的（模型）决策。
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(MOVED))]);
    let (_, _) = run(&mut d, 1, false);
    assert_eq!(d.events, ["swipe"]);
}

#[test]
fn a_dry_run_sends_no_status() {
    let mut d = Fake::new(vec![Ok(grid(A))]);
    let (s, _) = run(&mut d, 5, true);
    assert_eq!(s.stop, Stop::DryRun);
    assert!(d.events.is_empty(), "{:?}", d.events);
}

#[test]
fn several_steps_send_no_status() {
    let b: &[&[u16]] = &[&[3, 2, 3, 2], &[2, 3, 1, 3], &[1, 1, 2, 1]];
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(b)), Ok(grid(b)), Ok(grid(MOVED)), Ok(grid(MOVED))]);
    let (s, _) = run(&mut d, 2, false);
    assert_eq!(s.steps, 2);
    assert_eq!(d.events, ["swipe", "swipe"]);
}

#[test]
fn no_moves_sends_no_status() {
    let mut d = Fake::new(vec![Ok(grid(DEAD))]);
    let (s, _) = run(&mut d, 5, false);
    assert_eq!(s.stop, Stop::NoMoves);
    assert!(d.events.is_empty(), "{:?}", d.events);
}

#[test]
fn the_status_trait_method_still_exists_but_play_never_calls_it() {
    // 方法留着给以后的慢决策；录音假件能收到直接调用，play() 却一次都不调。
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(MOVED))]);
    Dco::show_status(&mut d, "think");
    assert_eq!(d.events, ["think"]);
    d.events.clear();
    let (_, _) = run(&mut d, 1, false);
    assert!(!d.events.iter().any(|e| e == "think" || e == "look"), "{:?}", d.events);
}

#[test]
fn a_board_that_cannot_be_read_says_nothing() {
    let mut d = Fake::new(vec![Err(err("not_a_grid"))]);
    let (_, _) = run(&mut d, 5, false);
    assert!(d.events.is_empty(), "{:?}", d.events);
}

// ---- 「不是糖」的格子：划的两端都不能是它 ----

const HOLE: [u8; 3] = [196, 148, 101];
// 9 是洞（3 格）。把 9 当普通颜色看，(0,2)↔(1,2) 能连出第 0 行四连；真正的糖还有别的走法（3 和 2）。
const WITH_HOLES: &[&[u16]] = &[&[1, 1, 9, 1], &[2, 3, 1, 3], &[3, 2, 3, 2], &[9, 3, 2, 9]];

fn grid_with_hole_rgb() -> GridRead {
    let mut g = serde_json::to_value(grid(WITH_HOLES)).unwrap();
    for c in g["classes"].as_array_mut().unwrap() {
        if c["id"] == 9 {
            c["rgb"] = json!(HOLE);
        }
    }
    serde_json::from_value(g).unwrap()
}

type Swipe = ((f64, f64), (f64, f64));
fn swipe_cells(p: &Profile, s: &[Swipe]) -> Vec<(usize, usize)> {
    let [x, y, w, h] = p.region;
    let cell = |(px, py): (f64, f64)| (((py - y) / h * p.rows as f64).floor() as usize, ((px - x) / w * p.cols as f64).floor() as usize);
    s.iter().flat_map(|&(f, t)| [cell(f), cell(t)]).collect()
}

fn play_holes(fixed_rgb: Vec<[u8; 3]>) -> (Profile, Vec<Swipe>) {
    let p = Profile { fixed_rgb, ..profile(4, 4) };
    let mut d = Fake::new(vec![Ok(grid_with_hole_rgb())]);
    play(&mut d, &mut Clk(0), &p, &Options { max_steps: 10, dry_run: false, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |_| {});
    (p, d.swipes)
}

#[test]
fn swipes_never_touch_a_cell_whose_colour_is_listed_as_fixed() {
    let (p, swipes) = play_holes(vec![HOLE]);
    assert!(!swipes.is_empty(), "除了洞还有别的走法，该划");
    let fixed = [(0, 2), (3, 0), (3, 3)];
    assert!(swipe_cells(&p, &swipes).iter().all(|c| !fixed.contains(c)), "{swipes:?}");
}

#[test]
fn without_the_fixed_list_the_same_grid_swipes_a_hole() {
    let (p, swipes) = play_holes(vec![]);
    let fixed = [(0, 2), (3, 0), (3, 3)];
    assert!(swipe_cells(&p, &swipes).iter().any(|c| fixed.contains(c)), "{swipes:?}");
}

// ---- 每步记录里的 swipe：真实端点和起始时刻，给章鱼的叠加层日志对账 ----

fn r4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

#[test]
fn the_record_carries_the_real_swipe_points_start_and_duration() {
    let p = profile(3, 4);
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(MOVED))]);
    let mut log = vec![];
    play(&mut d, &mut Clk(1000), &p, &Options { max_steps: 1, dry_run: false, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |v| log.push(v));
    let (f, t) = d.swipes[0];
    let sw = &log[0]["swipe"];
    assert_eq!(sw["from"], json!([r4(f.0), r4(f.1)]));
    assert_eq!(sw["to"], json!([r4(t.0), r4(t.1)]));
    // 端点就是选中那步两格的中心
    let c = &log[0]["candidates"][log[0]["chosen"].as_u64().unwrap() as usize];
    let (a, b) = (c["a"].as_array().unwrap(), c["b"].as_array().unwrap());
    let [x, y, w, h] = p.region;
    let ctr = |r: u64, c: u64| json!([r4(x + (c as f64 + 0.5) / 4.0 * w), r4(y + (r as f64 + 0.5) / 3.0 * h)]);
    assert_eq!(sw["from"], ctr(a[0].as_u64().unwrap(), a[1].as_u64().unwrap()));
    assert_eq!(sw["to"], ctr(b[0].as_u64().unwrap(), b[1].as_u64().unwrap()));
    // 起始时刻 = 划之前一刻的时钟（记录的 time_ms 也是那一刻）
    assert_eq!(sw["started_ms"], 1000);
    assert_eq!(sw["started_ms"], log[0]["time_ms"]);
    assert_eq!(sw["duration_ms"], log[0]["timing_ms"]["swipe"]);
}

#[test]
fn a_later_step_starts_its_swipe_after_the_earlier_settle() {
    let b: &[&[u16]] = &[&[3, 2, 3, 2], &[2, 3, 1, 3], &[1, 1, 2, 1]];
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(b)), Ok(grid(b)), Ok(grid(MOVED)), Ok(grid(MOVED))]);
    let (_, log) = run(&mut d, 2, false);
    let s0 = log[0]["swipe"]["started_ms"].as_u64().unwrap();
    let s1 = log[1]["swipe"]["started_ms"].as_u64().unwrap();
    assert!(s1 > s0, "{s0} {s1}");
    assert_eq!(s1, log[1]["time_ms"].as_u64().unwrap());
}

#[test]
fn a_dry_run_record_has_no_swipe() {
    let mut d = Fake::new(vec![Ok(grid(A))]);
    let (_, log) = run(&mut d, 5, true);
    assert!(log[0].get("swipe").is_none(), "{}", log[0]);
}

#[test]
fn a_refused_swipe_record_still_has_swipe_points() {
    let mut d = Fake::new(vec![Ok(grid(A))]);
    d.swipe_err = Some(err("halted"));
    let (_, log) = run(&mut d, 10, false);
    let sw = &log[0]["swipe"];
    assert_eq!(sw["from"].as_array().unwrap().len(), 2);
    assert_eq!(sw["to"].as_array().unwrap().len(), 2);
    assert_eq!(sw["started_ms"], log[0]["time_ms"]);
    assert_eq!(sw["duration_ms"], log[0]["timing_ms"]["swipe"]);
    assert_eq!(log[0]["swiped"], false);
}

#[test]
fn a_changed_odd_flag_alone_is_not_a_changed_board() {
    // 分组完全一样，只有 odd 标记在闪（提示光晕）：不能算「变了」，要按没反应处理。
    let flicker = |on: bool| {
        let mut g = grid(A);
        g.odd[0][0] = on;
        g
    };
    let mut reads = vec![Ok(grid(A))];
    for i in 0..400 {
        reads.push(Ok(flicker(i % 2 == 0)));
    }
    let mut d = Fake::new(reads);
    let (s, log) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::Stuck);
    assert_eq!(log.iter().filter(|l| l["outcome"] == "no_change").count(), 2);
    assert!(log.iter().all(|l| l["outcome"] != "moved"));
}

/// 2026-10-03 真机第 1714 关的顶行：蛋 蓝 红 蓝 蓝 绿 橙 绿 蛋。红和橙全盘各只有一个，
/// 唯一的走法是红和左边的蓝对换凑三蓝。配置列了「不是糖」的颜色就该走；没列就是旧规则，一格的类别不能换。
#[test]
fn a_one_cell_candy_may_move_once_the_profile_lists_the_not_candies() {
    const ROW: &[&[u16]] = &[&[0, 1, 2, 1, 1, 3, 4, 3, 0]];
    let go = |fixed_rgb: Vec<[u8; 3]>| {
        let mut d = Fake::new(vec![Ok(grid(ROW))]);
        let p = Profile { fixed_rgb, ..profile(1, 9) };
        let s = play(&mut d, &mut Clk(0), &p, &Options { max_steps: 1, dry_run: true, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |_| {});
        s.stop
    };
    assert_ne!(go(vec![[196, 148, 101]]), Stop::NoMoves);
    assert_eq!(go(vec![]), Stop::NoMoves);
}

// ---- “没步可走”和“画面变了”要停一下再读一遍才算数 ----

const DEAD2: &[&[u16]] = &[&[3, 3, 2, 2], &[2, 1, 1, 2], &[3, 1, 1, 3]]; // 另一种分组，同样没步可走

#[test]
fn no_moves_that_a_refill_fixes_is_not_the_end() {
    // 第一次读没步（补糖补到一半），停一下再读有步了：接着划，不停
    let mut d = Fake::new(vec![Ok(grid(DEAD)), Ok(grid(A))]);
    let (s, log) = run(&mut d, 1, false);
    assert_eq!(s.stop, Stop::StepsDone);
    assert_eq!(d.swipes.len(), 1);
    assert_eq!(log.len(), 1, "确认读不算一步，不写步骤记录");
}

#[test]
fn no_moves_confirmed_by_an_identical_second_read_stops_after_one_confirmation() {
    let mut d = Fake::new(vec![Ok(grid(DEAD))]);
    let mut clk = Clk(0);
    let s = play(&mut d, &mut clk, &profile(3, 4), &Options { max_steps: 5, dry_run: false, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |_| {});
    assert_eq!(s.stop, Stop::NoMoves);
    assert_eq!(d.reads_taken, 2);
    assert_eq!(clk.0, 1_500);
}

#[test]
fn a_board_that_keeps_changing_without_a_move_stops_after_two_confirmations() {
    let mut reads = vec![];
    for i in 0..50 {
        reads.push(Ok(grid(if i % 2 == 0 { DEAD } else { DEAD2 })));
    }
    let mut d = Fake::new(reads);
    let (s, _) = run(&mut d, 5, false);
    assert_eq!(s.stop, Stop::NoMoves);
    assert_eq!(d.reads_taken, 3);
}

#[test]
fn a_brief_colour_jump_that_is_gone_on_the_reread_keeps_playing() {
    let five: &[&[u16]] = &[&[1, 4, 2, 5], &[2, 3, 1, 3], &[4, 5, 3, 1]];
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(five)), Ok(grid(five)), Ok(grid(A))]);
    let (s, _) = run(&mut d, 2, false);
    assert!(!matches!(s.stop, Stop::ClassesChanged { .. }), "{:?}", s.stop);
    assert_eq!(d.swipes.len(), 2);
}

#[test]
fn a_colour_jump_that_stays_on_the_reread_is_a_changed_screen() {
    let five: &[&[u16]] = &[&[1, 4, 2, 5], &[2, 3, 1, 3], &[4, 5, 3, 1]];
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(five)), Ok(grid(five))]);
    let (s, _) = run(&mut d, 10, false);
    assert!(matches!(s.stop, Stop::ClassesChanged { was: 3, now: 5 }), "{:?}", s.stop);
}

#[test]
fn a_dry_run_does_not_pause_or_reread() {
    let mut d = Fake::new(vec![Ok(grid(DEAD))]);
    let mut clk = Clk(0);
    let s = play(&mut d, &mut clk, &profile(3, 4), &Options { max_steps: 5, dry_run: true, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |_| {});
    assert_eq!(s.stop, Stop::NoMoves);
    assert_eq!(d.reads_taken, 1);
    assert_eq!(clk.0, 0);
}

#[test]
fn a_swap_the_game_bounces_back_is_no_change() {
    // 被笼子锁住的糖：划下去先动一下（读到 MOVED），又弹回 A 并落定。落定后和划之前一样，算没反应，连着两次就停。
    let mut reads = vec![Ok(grid(A)), Ok(grid(MOVED))];
    for _ in 0..400 {
        reads.push(Ok(grid(A)));
    }
    let mut d = Fake::new(reads);
    let (s, log) = run(&mut d, 10, false);
    assert_eq!(log[0]["outcome"], "no_change", "{log:?}");
    assert_eq!(log[1]["outcome"], "no_change", "{log:?}");
    assert_eq!(s.stop, Stop::Stuck);
}

use crate::ask::{Advice, Advisor, AskInput};
use std::cell::{Cell, RefCell};

struct Say {
    choice: Option<usize>,
    calls: Cell<usize>,
    seen: RefCell<Vec<AskInput>>,
}
impl Say {
    fn new(choice: Option<usize>) -> Say {
        Say { choice, calls: Cell::new(0), seen: RefCell::new(vec![]) }
    }
}
impl Advisor for Say {
    fn pick(&self, i: &AskInput) -> Option<Advice> {
        self.calls.set(self.calls.get() + 1);
        self.seen.borrow_mut().push(AskInput {
            board: i.board.clone(),
            goal: i.goal.clone(),
            candidates: i.candidates.clone(),
            failed: i.failed.clone(),
        });
        Some(Advice { choice: self.choice, reason: "测试".into(), raw: "x".into(), model: "fake".into() })
    }
}
struct Gone(Cell<usize>);
impl Advisor for Gone {
    fn pick(&self, _: &AskInput) -> Option<Advice> {
        self.0.set(self.0.get() + 1);
        None
    }
    fn available(&self) -> bool {
        false
    }
}
struct Down;
impl Advisor for Down {
    fn pick(&self, _: &AskInput) -> Option<Advice> {
        None
    }
}

fn run_ask(d: &mut Fake, adv: Option<&dyn Advisor>, always: bool, budget: usize, steps: usize) -> (Summary, Vec<Value>) {
    let mut log = vec![];
    let o = Options { max_steps: steps, dry_run: false, advisor: adv, ask_always: always, ask_budget: budget, goal: "清冰" };
    let s = play(d, &mut Clk(0), &profile(3, 4), &o, &mut |v| log.push(v));
    (s, log)
}
fn settled_script() -> Vec<Result<GridRead, DcoError>> {
    vec![Ok(grid(A)), Ok(grid(MOVED)), Ok(grid(MOVED))]
}

#[test]
fn ask_always_lets_the_model_pick_among_candidates_and_records_it() {
    // 先看规则自己会划哪一步
    let mut d0 = Fake::new(settled_script());
    let _ = run_ask(&mut d0, None, false, 30, 1);
    let rules_swipe = d0.swipes[0];

    let say = Say::new(Some(1)); // 选第 2 个候选
    let mut d = Fake::new(settled_script());
    let (_, log) = run_ask(&mut d, Some(&say), true, 30, 1);
    assert_eq!(say.calls.get(), 1);
    assert_ne!(d.swipes[0], rules_swipe, "模型选了第 2 个候选，划的应该和规则第一名不同");
    assert_eq!(log[0]["decider"], "model");
    assert_eq!(log[0]["chosen"], 1);
    assert_eq!(log[0]["ask"]["choice"], 1);
    assert_eq!(log[0]["ask"]["model"], "fake");
    // 发出去的提示里有目标、字母棋盘、编号候选
    let seen = say.seen.borrow();
    assert_eq!(seen[0].goal, "清冰");
    assert!(seen[0].board.contains('A'));
    assert!(seen[0].candidates.len() >= 2);
}

#[test]
fn without_ask_always_the_model_is_not_asked_on_a_clean_board() {
    let say = Say::new(Some(1));
    let mut d = Fake::new(settled_script());
    let (_, log) = run_ask(&mut d, Some(&say), false, 30, 1);
    assert_eq!(say.calls.get(), 0);
    assert_eq!(log[0]["decider"], "rules");
    assert!(log[0].get("ask").is_none());
}

#[test]
fn a_failed_swap_triggers_the_model_and_is_listed_but_not_offered() {
    // 第一划没反应（A 一直不变），第二次选步时这盘棋上已有失败的步：问模型，失败的步在 failed 里、不在候选里
    // （用 MANY：被拒绝的步锁住两格以后，棋盘上还得有至少两个不碰这两格的候选，模型才会被问）
    let mut reads = vec![];
    for _ in 0..40 {
        reads.push(Ok(grid(MANY)));
    }
    let moved: &[&[u16]] = &[&[1, 1, 1, 1], &[3, 3, 4, 3], &[1, 1, 2, 1], &[3, 3, 4, 3], &[1, 1, 2, 1], &[3, 3, 4, 3]];
    reads.push(Ok(grid(moved)));
    reads.push(Ok(grid(moved)));
    let say = Say::new(Some(0));
    let mut d = Fake::new(reads);
    let mut log = vec![];
    let o = Options { max_steps: 2, dry_run: false, advisor: Some(&say), ask_always: false, ask_budget: 30, goal: "清冰" };
    let _ = play(&mut d, &mut Clk(0), &profile(6, 4), &o, &mut |v| log.push(v));
    assert!(say.calls.get() >= 1);
    let seen = say.seen.borrow();
    assert_eq!(seen[0].failed.len(), 1);
    assert!(!seen[0].candidates.iter().any(|c| *c == seen[0].failed[0]));
    assert_eq!(log[0]["decider"], "rules");
    assert_eq!(log[1]["decider"], "model");
    // 模型回的是「候选里的第 1 个」；要靠 offered 映射回 cands，不能直接当 cands 下标，否则又选回刚失败的那步
    assert_ne!(log[1]["chosen"], log[0]["chosen"]);
    assert_ne!(log[1]["swipe"]["from"], log[0]["swipe"]["from"]);
    let refused = log[0]["chosen"].clone();
    let asked = log[1]["ask"]["asked"].as_array().unwrap();
    assert!(!asked.contains(&refused));
    assert_eq!(log[1]["ask"]["choice"], asked[0]);
    assert_eq!(log[1]["ask"]["failed"].as_array().unwrap().len(), 1);
    assert_eq!(log[1]["ask"]["goal"], "清冰");
}

#[test]
fn model_that_cannot_decide_or_is_down_falls_back_to_rules() {
    let mut d0 = Fake::new(settled_script());
    let _ = run_ask(&mut d0, None, false, 30, 1);
    let rules_swipe = d0.swipes[0];

    for adv in [&Say::new(None) as &dyn Advisor, &Down] {
        let mut d = Fake::new(settled_script());
        let (s, log) = run_ask(&mut d, Some(adv), true, 30, 1);
        assert_eq!(d.swipes[0], rules_swipe);
        assert_eq!(log[0]["decider"], "rules");
        assert_eq!(s.stop, Stop::StepsDone);
    }
}

#[test]
fn an_unavailable_advisor_is_never_asked_and_the_octopus_never_thinks() {
    let gone = Gone(Cell::new(0));
    let mut d = Fake::new(settled_script());
    let (s, log) = run_ask(&mut d, Some(&gone), true, 30, 1);
    assert_eq!(gone.0.get(), 0);
    assert_eq!(d.events, vec!["swipe"]);
    assert_eq!(log[0]["decider"], "rules");
    assert!(log[0].get("ask").is_none());
    assert_eq!(s.stop, Stop::StepsDone);
}

#[test]
fn ask_budget_stops_the_questions_but_not_the_game() {
    let say = Say::new(Some(1));
    let mut reads = vec![Ok(grid(A))];
    for _ in 0..6 {
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(A)));
        reads.push(Ok(grid(A)));
    }
    let mut d = Fake::new(reads);
    let (_, log) = run_ask(&mut d, Some(&say), true, 2, 3);
    assert_eq!(say.calls.get(), 2);
    assert_eq!(log.len(), 3);
    assert_eq!(log[2]["decider"], "rules");
}

const ONE: &[&[u16]] = &[&[1, 3, 3, 2], &[2, 2, 1, 1], &[1, 2, 1, 1]]; // 只有一个合法交换

#[test]
fn the_octopus_thinks_only_while_the_model_is_asked() {
    let say = Say::new(Some(1));
    let mut d = Fake::new(settled_script());
    let _ = run_ask(&mut d, Some(&say), true, 30, 1);
    assert_eq!(d.events, vec!["think", "look", "swipe"]);

    let mut d = Fake::new(settled_script());
    let _ = run_ask(&mut d, Some(&say), false, 30, 1);
    assert_eq!(d.events, vec!["swipe"]);

    // 模型不可用也要回到“看”
    let mut d = Fake::new(settled_script());
    let _ = run_ask(&mut d, Some(&Down), true, 30, 1);
    assert_eq!(d.events, vec!["think", "look", "swipe"]);
}

#[test]
fn a_single_candidate_is_never_worth_asking_about() {
    let say = Say::new(Some(0));
    let mut d = Fake::new(vec![Ok(grid(ONE)), Ok(grid(MOVED)), Ok(grid(MOVED))]);
    let (_, log) = run_ask(&mut d, Some(&say), true, 30, 1);
    assert_eq!(log[0]["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(say.calls.get(), 0);
    assert_eq!(log[0]["decider"], "rules");
    assert!(log[0].get("ask").is_none());
}

#[test]
fn a_choice_outside_the_offered_list_falls_back_to_rules_and_is_recorded() {
    let mut d0 = Fake::new(settled_script());
    let _ = run_ask(&mut d0, None, false, 30, 1);
    let say = Say::new(Some(99));
    let mut d = Fake::new(settled_script());
    let (_, log) = run_ask(&mut d, Some(&say), true, 30, 1);
    assert_eq!(d.swipes[0], d0.swipes[0]);
    assert_eq!(log[0]["decider"], "rules");
    assert!(log[0]["ask"]["choice"].is_null());
}

#[test]
fn after_a_refused_swap_no_later_candidate_touches_its_two_cells() {
    // 一直读到同一张盘（没变化）：第一步被拒绝以后，第二步选的步不碰第一步的两个格子
    let mut reads = vec![];
    for _ in 0..80 {
        reads.push(Ok(grid(MANY)));
    }
    let mut d = Fake::new(reads);
    let mut log = vec![];
    let _ = play(&mut d, &mut Clk(0), &profile(6, 4), &Options { max_steps: 5, dry_run: false, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |v| log.push(v));
    let pos = |l: &Value, k: &str| l["candidates"][l["chosen"].as_u64().unwrap() as usize][k].clone();
    assert!(log.len() >= 2, "{log:?}");
    for i in 1..log.len() {
        let (ai, bi) = (pos(&log[i], "a"), pos(&log[i], "b"));
        for j in 0..i {
            let (aj, bj) = (pos(&log[j], "a"), pos(&log[j], "b"));
            for p in [&ai, &bi] {
                assert!(*p != aj && *p != bj, "第 {i} 步碰了被拒绝的格子（第 {j} 步）：{log:?}");
            }
        }
    }
}

// 6 行 4 列，每行一个互不相干的三连换法（交替用 2/4、1/3，避免竖向连线）
const MANY: &[&[u16]] = &[
    &[1, 1, 2, 1],
    &[3, 3, 4, 3],
    &[1, 1, 2, 1],
    &[3, 3, 4, 3],
    &[1, 1, 2, 1],
    &[3, 3, 4, 3],
];

#[test]
fn five_refusals_in_a_row_stop_the_run_not_two() {
    let mut reads = vec![];
    for _ in 0..400 {
        reads.push(Ok(grid(MANY)));
    }
    let mut d = Fake::new(reads);
    let mut log = vec![];
    let s = play(&mut d, &mut Clk(0), &profile(6, 4), &Options { max_steps: 20, dry_run: false, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |v| log.push(v));
    assert_eq!(s.stop, Stop::Stuck);
    assert_eq!(d.swipes.len(), 5, "应该试满 5 次才停");
}

#[test]
fn a_move_that_works_unlocks_the_cells() {
    // 第一步没反应（锁住它的两格），第二步（别处）成功 → 解除；第三步又可以碰第一步的格子
    let mut reads = vec![];
    for _ in 0..40 {
        reads.push(Ok(grid(MANY)));
    }
    // 第二次划：变成另一张盘并落定
    let moved: &[&[u16]] = &[&[1, 1, 1, 1], &[3, 3, 4, 3], &[1, 1, 2, 1], &[3, 3, 4, 3], &[1, 1, 2, 1], &[3, 3, 4, 3]];
    reads.push(Ok(grid(moved)));
    reads.push(Ok(grid(moved)));
    for _ in 0..400 {
        reads.push(Ok(grid(MANY)));
    }
    let mut d = Fake::new(reads);
    let mut log = vec![];
    let _ = play(&mut d, &mut Clk(0), &profile(6, 4), &Options { max_steps: 3, dry_run: false, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |v| log.push(v));
    let outcomes: Vec<&str> = log.iter().map(|l| l["outcome"].as_str().unwrap()).collect();
    assert_eq!(&outcomes[..2], ["no_change", "moved"], "{outcomes:?}");
}

// 4x4：规则第一名之后，排在第一个「完全不碰」的候选前面，既有只和它共用 a 格的，也有只共用 b 格的候选
const OVERLAP: &[&[u16]] = &[&[2, 3, 4, 2], &[3, 3, 3, 4], &[3, 4, 3, 3], &[2, 1, 4, 4]];

#[test]
fn a_refused_swap_locks_both_of_its_cells_even_when_better_candidates_share_only_one() {
    let mut reads = vec![];
    for _ in 0..200 {
        reads.push(Ok(grid(OVERLAP)));
    }
    let mut d = Fake::new(reads);
    let mut log = vec![];
    let _ = play(&mut d, &mut Clk(0), &profile(4, 4), &Options { max_steps: 2, dry_run: false, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |v| log.push(v));
    assert!(log.len() >= 2, "{log:?}");
    let cell = |l: &Value, k: &str| l["candidates"][l["chosen"].as_u64().unwrap() as usize][k].clone();
    let (a0, b0) = (cell(&log[0], "a"), cell(&log[0], "b"));
    for k in ["a", "b"] {
        let p = cell(&log[1], k);
        assert!(p != a0 && p != b0, "第二步碰了被拒绝的格子：{log:?}");
    }
}

#[test]
fn a_move_that_works_unlocks_cells_that_the_only_remaining_move_needs() {
    // TWO 上有两个互不相干的步，规则先选下面一行的（低行优先），划了没反应 → 锁住那两格；
    // 第二步选上面的，成功，盘面变成 AFTER：那里只剩下面那一行的步，要碰被锁过的格子。解锁了才能继续划。
    const TWO: &[&[u16]] = &[&[1, 1, 2, 1], &[3, 4, 3, 4], &[4, 3, 4, 3], &[5, 5, 6, 5]];
    const AFTER: &[&[u16]] = &[&[1, 2, 1, 2], &[2, 1, 2, 1], &[3, 4, 3, 4], &[5, 5, 6, 5]];
    let mut reads = vec![];
    for _ in 0..40 {
        reads.push(Ok(grid(TWO)));
    }
    reads.push(Ok(grid(AFTER)));
    reads.push(Ok(grid(AFTER)));
    let mut d = Fake::new(reads);
    let mut log = vec![];
    let _ = play(&mut d, &mut Clk(0), &profile(4, 4), &Options { max_steps: 3, dry_run: false, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |v| log.push(v));
    let outcomes: Vec<&str> = log.iter().map(|l| l["outcome"].as_str().unwrap()).collect();
    // 第三步：规则第一名就是那个碰被锁过的格子的步（没解锁的话它会被跳过）
    let cell = |l: &Value, k: &str| l["candidates"][l["chosen"].as_u64().unwrap() as usize][k].clone();
    let first_cells = [cell(&log[0], "a"), cell(&log[0], "b")];
    assert!(outcomes.len() >= 3, "{outcomes:?}");
    assert!(first_cells.contains(&cell(&log[2], "a")) || first_cells.contains(&cell(&log[2], "b")), "第三步该碰第一步的格子：{log:?}");
    assert!(outcomes[0] == "no_change" && outcomes[1] == "moved", "{outcomes:?}");
}

fn grid_rgb(cells: &[&[u16]], rgbs: &[(u16, [u8; 3])]) -> GridRead {
    let rows = cells.len();
    let cols = cells[0].len();
    let classes: Vec<Value> = rgbs
        .iter()
        .map(|(i, c)| json!({"id": i, "count": cells.iter().flat_map(|r| r.iter()).filter(|&&x| x == *i).count(), "rgb": c}))
        .collect();
    serde_json::from_value(json!({"rows": rows, "cols": cols, "cells": cells, "odd": vec![vec![false; cols]; rows], "classes": classes})).unwrap()
}

#[test]
fn changed_cells_compares_colours_not_class_ids() {
    let a = grid_rgb(&[&[1, 2], &[2, 1]], &[(1, [200, 30, 30]), (2, [30, 30, 200])]);
    let b = grid_rgb(&[&[7, 8], &[8, 7]], &[(7, [200, 30, 30]), (8, [30, 30, 200])]);
    assert_eq!(changed_cells(&a, &b), Some(0));
    let c = grid_rgb(&[&[7, 8], &[8, 8]], &[(7, [200, 30, 30]), (8, [30, 30, 200])]);
    assert_eq!(changed_cells(&a, &c), Some(1));
}

#[test]
fn changed_cells_is_none_when_colours_are_missing() {
    let a = grid(A);
    assert_eq!(changed_cells(&a, &a), None);
}

#[test]
fn records_carry_predicted_and_observed() {
    let mut d = Fake::new(settled_script());
    let (_, log) = run(&mut d, 1, false);
    assert!(log[0]["predicted_cleared"].as_u64().unwrap() >= 3);
    assert!(log[0].get("observed_changed").is_some());
}

#[test]
fn numbers_keeps_only_whole_number_elements_in_order() {
    let seen = Seen {
        snapshot_id: "s".into(),
        observation_id: None,
        elements: ["1716/♥5", "38", "x", "122", " 7 "].iter().enumerate().map(|(i, s)| crate::screen::Element { id: format!("e{i}"), text: (*s).into() }).collect(),
    };
    assert_eq!(numbers(&seen), vec![38, 122, 7]);
}

#[test]
fn progress_is_recorded_before_and_after_each_step() {
    let mut d = Fake::new(settled_script());
    d.texts = vec![vec!["50", "30"], vec!["49", "30"]].into();
    let (_, log) = run(&mut d, 1, false);
    assert_eq!(log[0]["progress"]["before"], json!([50, 30]));
    assert_eq!(log[0]["progress"]["after"], json!([49, 30]));
}

#[test]
fn progress_is_null_when_the_dco_cannot_read_text() {
    let mut d = Fake::new(settled_script());
    let (s, log) = run(&mut d, 1, false);
    assert!(log[0]["progress"]["before"].is_null());
    assert!(log[0]["progress"]["after"].is_null());
    assert_eq!(s.stop, Stop::StepsDone);
}

#[test]
fn the_after_of_one_step_is_the_before_of_the_next() {
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(MOVED)), Ok(grid(MOVED)), Ok(grid(A)), Ok(grid(A))]);
    d.texts = vec![vec!["50"], vec!["49"], vec!["48"]].into();
    let (_, log) = run(&mut d, 2, false);
    assert!(log.len() >= 2);
    assert_eq!(log[1]["progress"]["before"], json!([49]));
    assert_eq!(log[1]["progress"]["after"], json!([48]));
}
