### Task 2: 玩的循环

**Files:**
- Create: `crates/dct-game/src/play.rs`、`src/play_tests.rs`
- Modify: `crates/dct-game/src/lib.rs`

**Interfaces:**
- Consumes: 任务 1 的 `Board::from_read`、`choose`、`GridRead`。
- Produces: `trait Dco { fn read_grid(&mut self, &Profile) -> Result<GridRead, DcoError>; fn swipe(&mut self, &Profile, (f64,f64), (f64,f64)) -> Result<(), DcoError>; }`；`trait Clock { fn now_ms(&self) -> u64; fn sleep_ms(&mut self, u64); }`；`Profile { window: Value, region: [f64;4], rows, cols, extra: Value }`；`DcoError { code: String, message: String }`；`Options { max_steps, dry_run }`；`play(&mut dyn Dco, &mut dyn Clock, &Profile, &Options, &mut dyn FnMut(Value)) -> Summary { steps, stop: Stop }`；`Stop::{NoGrid(String), ClassesChanged{was,now}, NoMoves, Stuck, StillMoving, StepsDone, DryRun, Dco(DcoError)}`。每一步通过回调交出一条记录（字段见设计「记录」一节）。


- [ ] **Step 1: 在 `crates/dct-game/src/lib.rs` 里加两行**

把 `pub mod sim;` 上面加 `pub mod play;`，文件末尾加 `#[cfg(test)]\nmod play_tests;`。

- [ ] **创建 `crates/dct-game/src/play.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 一盘游戏怎么玩：读盘 → 选步 → 划 → 等画面停下 → 下一步，以及什么时候停。
//! 跟 dco 说话（`Dco`）和计时（`Clock`）都是传进来的，所以测试里换成假的，不碰真机也不真睡觉。
use crate::board::{Board, GridRead};
use crate::choose::{choose, Candidate};
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct Profile {
    pub window: Value,
    pub region: [f64; 4], // x, y, w, h
    pub rows: usize,
    pub cols: usize,
    pub extra: Value, // inset / class_de / top_div / odd_share：原样带给 read_grid
}

#[derive(Debug, Clone, PartialEq)]
pub struct DcoError {
    pub code: String,
    pub message: String,
}

pub trait Dco {
    fn read_grid(&mut self, p: &Profile) -> Result<GridRead, DcoError>;
    /// 坐标是窗口比例（0～1）。
    fn swipe(&mut self, p: &Profile, from: (f64, f64), to: (f64, f64)) -> Result<(), DcoError>;
}

pub trait Clock {
    fn now_ms(&self) -> u64;
    fn sleep_ms(&mut self, ms: u64);
}

pub struct Options {
    pub max_steps: usize,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    /// 读不出棋盘（含：类别太多，多半是弹了窗）。
    NoGrid(String),
    /// 类别比开始时多出两个以上：多半这关结束了、弹了窗。
    ClassesChanged { was: usize, now: usize },
    NoMoves,
    /// 连着两次划了没反应。
    Stuck,
    /// 8 秒了画面还在变。
    StillMoving,
    StepsDone,
    DryRun,
    /// dco 急停 / 暂停 / 锁屏 / 别的错误。
    Dco(DcoError),
}

pub struct Summary {
    pub steps: usize,
    pub stop: Stop,
}

enum Settle {
    Settled(GridRead),
    NoChange(GridRead),
    StillMoving,
    Failed(DcoError),
}

fn same(a: &GridRead, b: &GridRead) -> bool {
    a.cells == b.cells && a.odd == b.odd
}

const FIRST_WAIT_MS: u64 = 150;
const POLL_MS: u64 = 100;
const NO_CHANGE_MS: u64 = 3_000;
const GIVE_UP_MS: u64 = 8_000;

fn settle(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, before: &GridRead) -> Settle {
    let t0 = clock.now_ms();
    clock.sleep_ms(FIRST_WAIT_MS);
    let mut prev: Option<GridRead> = None;
    let mut changed = false;
    loop {
        let waited = clock.now_ms() - t0;
        match dco.read_grid(p) {
            Ok(g) => {
                if !same(&g, before) {
                    changed = true;
                }
                if changed {
                    if prev.as_ref().is_some_and(|x| same(x, &g)) {
                        return Settle::Settled(g);
                    }
                } else if waited >= NO_CHANGE_MS {
                    return Settle::NoChange(g);
                }
                prev = Some(g);
            }
            // 动画中间读不出来是常事：接着等，等到时间到。别的错误（急停、锁屏）马上停。
            Err(e) if e.code == "not_a_grid" => prev = None,
            Err(e) => return Settle::Failed(e),
        }
        if waited >= GIVE_UP_MS {
            return Settle::StillMoving;
        }
        clock.sleep_ms(POLL_MS);
    }
}

fn centre(p: &Profile, (r, c): (usize, usize)) -> (f64, f64) {
    let [x, y, w, h] = p.region;
    (x + (c as f64 + 0.5) / p.cols as f64 * w, y + (r as f64 + 0.5) / p.rows as f64 * h)
}

pub fn play(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, o: &Options, sink: &mut dyn FnMut(Value)) -> Summary {
    let mut steps = 0;
    let mut baseline: Option<usize> = None;
    let mut failed: Vec<(crate::sim::Move, Vec<Vec<u16>>)> = Vec::new();
    let mut streak = 0;
    let mut current: Option<GridRead> = None;
    let stop = loop {
        if steps >= o.max_steps {
            break Stop::StepsDone;
        }
        let t_read = clock.now_ms();
        let g = match current.take() {
            Some(g) => g,
            None => match dco.read_grid(p) {
                Ok(g) => g,
                Err(e) if e.code == "not_a_grid" => break Stop::NoGrid(e.message),
                Err(e) => break Stop::Dco(e),
            },
        };
        let read_ms = clock.now_ms() - t_read;
        let base = *baseline.get_or_insert(g.classes.len());
        if g.classes.len() > base + 1 {
            break Stop::ClassesChanged { was: base, now: g.classes.len() };
        }
        let Ok(board) = Board::from_read(&g) else {
            break Stop::NoGrid("棋盘的行列数对不上".into());
        };
        let t_choose = clock.now_ms();
        let cands: Vec<Candidate> = choose(&board);
        let choose_ms = clock.now_ms() - t_choose;
        // 同一盘棋上划了没反应的步，不再重复选。
        let pick = cands.iter().position(|c| !failed.iter().any(|(m, cells)| *m == c.mv && *cells == g.cells));
        let Some(pick) = pick else {
            break if cands.is_empty() { Stop::NoMoves } else { Stop::Stuck };
        };
        let chosen = &cands[pick];
        steps += 1;
        let mut rec = json!({
            "schema": 1, "time_ms": clock.now_ms(),
            "observation_id": g.observation_id, "observed_at_ms": g.observed_at_ms, "frame_age_ms": g.frame_age_ms,
            "rows": g.rows, "cols": g.cols, "cells": g.cells, "odd": g.odd, "classes": g.classes,
            "candidates": cands.iter().map(|c| json!({
                "a": [c.mv.a.0, c.mv.a.1], "b": [c.mv.b.0, c.mv.b.1], "score": c.score,
                "cleared": c.features.cleared, "cascade": c.features.cascade, "striped": c.features.striped,
                "wrapped": c.features.wrapped, "bomb": c.features.bomb, "triggered": c.features.triggered,
                "special_swap": c.features.special_swap, "lowest_row": c.features.lowest_row })).collect::<Vec<_>>(),
            "chosen": pick, "dry_run": o.dry_run,
        });
        if o.dry_run {
            rec["outcome"] = json!("dry_run");
            sink(rec);
            break Stop::DryRun;
        }
        let t_swipe = clock.now_ms();
        if let Err(e) = dco.swipe(p, centre(p, chosen.mv.a), centre(p, chosen.mv.b)) {
            rec["outcome"] = json!("stopped");
            sink(rec);
            break Stop::Dco(e);
        }
        let swipe_ms = clock.now_ms() - t_swipe;
        let t_settle = clock.now_ms();
        let result = settle(dco, clock, p, &g);
        rec["timing_ms"] = json!({ "read": read_ms, "choose": choose_ms, "swipe": swipe_ms, "settle": clock.now_ms() - t_settle });
        match result {
            Settle::Settled(next) => {
                rec["outcome"] = json!("moved");
                rec["after_observation_id"] = json!(next.observation_id);
                sink(rec);
                streak = 0;
                failed.clear();
                current = Some(next);
            }
            Settle::NoChange(same_board) => {
                rec["outcome"] = json!("no_change");
                rec["after_observation_id"] = json!(same_board.observation_id);
                sink(rec);
                streak += 1;
                failed.push((chosen.mv, g.cells.clone()));
                if streak >= 2 {
                    break Stop::Stuck;
                }
                current = Some(same_board);
            }
            Settle::StillMoving => {
                rec["outcome"] = json!("stopped");
                sink(rec);
                break Stop::StillMoving;
            }
            Settle::Failed(e) => {
                rec["outcome"] = json!("stopped");
                sink(rec);
                break Stop::Dco(e);
            }
        }
    };
    Summary { steps, stop }
}
```

- [ ] **创建 `crates/dct-game/src/play_tests.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
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
    // 开始 3 类；落定后变成 5 类（弹窗）
    let five: &[&[u16]] = &[&[1, 4, 2, 5], &[2, 3, 1, 3], &[3, 2, 3, 2]];
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(five)), Ok(grid(five))]);
    let (s, _) = run(&mut d, 10, false);
    assert!(matches!(s.stop, Stop::ClassesChanged { was: 3, now: 5 }), "{:?}", s.stop);
}

#[test]
fn one_extra_class_is_allowed() {
    // 多出来一个一格的类别（彩色炸弹）不算换了画面
    let four: &[&[u16]] = &[&[1, 1, 2, 1], &[2, 3, 1, 3], &[3, 2, 3, 4]];
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(four)), Ok(grid(four))]);
    let (s, _) = run(&mut d, 1, false);
    assert_eq!(s.stop, Stop::StepsDone);
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
    assert!(log[0]["candidates"].as_array().unwrap().len() >= 1);
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
```

- [ ] **Step 2: 跑测试**

Run: `cargo test -p dct-game`
Expected: `28 passed`（任务 1 的 15 个 + 这里 13 个）。

- [ ] **Step 3: 变异检查**

| 文件 | 把 | 改成 | 该红的测试 |
|---|---|---|---|
| `play.rs` | `streak >= 2` | `streak >= 3` | `two_unmoved_swipes_stop_…` |
| `play.rs` | `g.classes.len() > base + 1` | `g.classes.len() > base + 2` | `many_new_classes_…` |
| `play.rs` | `streak = 0;\n                failed.clear();`（**两行一起**，别动 `let mut streak = 0;`） | `failed.clear();` | `a_move_that_works_resets_the_unmoved_counter` |
| `play.rs` | `failed.push((chosen.mv, g.cells.clone()));` | （删掉） | `two_unmoved_swipes_stop_…` |
| `play.rs` | `Err(e) if e.code == "not_a_grid" => prev = None,` | `Err(e) if e.code == "not_a_grid" => break Settle::Failed(e),` | `not_a_grid_during_the_animation_is_waited_out` |
| `play.rs` | `if o.dry_run {` | `if false {` | `dry_run_never_swipes` |

- [ ] **Step 4: 提交**

```bash
cargo clippy -p dct-game --all-targets
git add crates/dct-game
git commit -m "feat(game): the play loop: read, choose, swipe, wait for the board to settle, and when to stop"
```


---

