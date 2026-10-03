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
}
impl Fake {
    fn new(reads: Vec<Result<GridRead, DcoError>>) -> Fake {
        Fake { reads: reads.into(), last: None, swipes: vec![], swipe_err: None }
    }
}
impl Dco for Fake {
    fn read_grid(&mut self, _: &Profile) -> Result<GridRead, DcoError> {
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
        Ok(())
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
fn profile(rows: usize, cols: usize) -> Profile {
    Profile { window: json!({"app": "x"}), region: [0.2, 0.3, 0.5, 0.4], rows, cols, extra: json!({}) }
}
fn run(d: &mut Fake, max: usize, dry: bool) -> (Summary, Vec<Value>) {
    let mut log = vec![];
    let s = play(d, &mut Clk(0), &profile(3, 4), &Options { max_steps: max, dry_run: dry }, &mut |v| log.push(v));
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
    for i in 0..200u16 {
        reads.push(Ok(grid(&[&[i, 1, 2, 1], &[2, 3, 1, 3], &[3, 2, 3, 2]])));
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
    for i in 0..200u16 {
        reads.push(Ok(grid(&[&[i, 1, 2, 1], &[2, 3, 1, 3], &[3, 2, 3, 2]])));
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
    let _ = play(&mut d, &mut Back(Default::default(), 0), &profile(3, 4), &Options { max_steps: 1, dry_run: false }, &mut |v| log.push(v));
    // 读盘到落定都过了一轮，没有 panic 就行；用一个会倒着走的钟再跑一遍无法落定的情形
    let mut d = Fake::new(vec![Ok(grid(A))]);
    let _ = play(&mut d, &mut Back(Default::default(), 0), &profile(3, 4), &Options { max_steps: 2, dry_run: false }, &mut |_| {});
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
