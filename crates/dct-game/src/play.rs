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
    /// 划了以后一张读得出的棋盘都没见到（多半是结果页）。
    NoGrid(String),
    Failed(DcoError),
}

/// 按行优先第一次出现的顺序重新编号。dco 的类别号只在一次返回里有意义，
/// 同一张没动的画面两次读可能分组相同、号码互换，所以跨读比较只能比分组。
pub(crate) fn canonical(cells: &[Vec<u16>]) -> Vec<Vec<u16>> {
    let mut map: std::collections::HashMap<u16, u16> = std::collections::HashMap::new();
    cells
        .iter()
        .map(|r| {
            r.iter()
                .map(|c| {
                    let n = map.len() as u16;
                    *map.entry(*c).or_insert(n)
                })
                .collect()
        })
        .collect()
}

fn same(a: &GridRead, b: &GridRead) -> bool {
    canonical(&a.cells) == canonical(&b.cells) && a.odd == b.odd
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
    let mut last_unreadable: Option<String> = None;
    let mut readable = false;
    loop {
        let waited = clock.now_ms().saturating_sub(t0);
        match dco.read_grid(p) {
            Ok(g) => {
                readable = true;
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
            Err(e) if e.code == "not_a_grid" => {
                last_unreadable = Some(e.message);
                prev = None;
            }
            Err(e) => return Settle::Failed(e),
        }
        if waited >= GIVE_UP_MS {
            return match last_unreadable {
                Some(m) if !readable => Settle::NoGrid(m),
                _ => Settle::StillMoving,
            };
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
        let read_ms = clock.now_ms().saturating_sub(t_read);
        // 只数至少两格的类别：一格的（彩色炸弹、条纹糖被读成自己的颜色）是“不认识”，忽略
        let big = g.classes.iter().filter(|c| c.count >= 2).count();
        let base = *baseline.get_or_insert(big);
        if big > base + 1 {
            break Stop::ClassesChanged { was: base, now: big };
        }
        let Ok(board) = Board::from_read(&g) else {
            break Stop::NoGrid("棋盘的行列数对不上".into());
        };
        let t_choose = clock.now_ms();
        let cands: Vec<Candidate> = choose(&board);
        let choose_ms = clock.now_ms().saturating_sub(t_choose);
        // 同一盘棋上划了没反应的步，不再重复选。
        let pick = cands.iter().position(|c| !failed.iter().any(|(m, cells)| *m == c.mv && *cells == canonical(&g.cells)));
        let Some(pick) = pick else {
            break if cands.is_empty() { Stop::NoMoves } else { Stop::Stuck };
        };
        let chosen = &cands[pick];
        let mut rec = json!({
            "schema": 1, "time_ms": clock.now_ms(),
            "observation_id": g.observation_id, "observed_at_ms": g.observed_at_ms, "frame_age_ms": g.frame_age_ms,
            "rows": g.rows, "cols": g.cols, "cells": g.cells, "odd": g.odd, "classes": g.classes,
            "candidates": cands.iter().map(|c| json!({
                "a": [c.mv.a.0, c.mv.a.1], "b": [c.mv.b.0, c.mv.b.1], "score": c.score,
                "features": {
                    "cleared": c.features.cleared, "cascade": c.features.cascade, "striped": c.features.striped,
                    "wrapped": c.features.wrapped, "bomb": c.features.bomb, "triggered": c.features.triggered,
                    "special_swap": c.features.special_swap, "lowest_row": c.features.lowest_row } })).collect::<Vec<_>>(),
            "chosen": pick, "dry_run": o.dry_run, "swiped": false,
        });
        if o.dry_run {
            rec["timing_ms"] = json!({ "read": read_ms, "choose": choose_ms, "swipe": 0, "settle": 0 });
            rec["outcome"] = json!("dry_run");
            sink(rec);
            break Stop::DryRun;
        }
        let t_swipe = clock.now_ms();
        if let Err(e) = dco.swipe(p, centre(p, chosen.mv.a), centre(p, chosen.mv.b)) {
            rec["timing_ms"] = json!({ "read": read_ms, "choose": choose_ms, "swipe": clock.now_ms().saturating_sub(t_swipe), "settle": 0 });
            rec["outcome"] = json!("stopped");
            sink(rec);
            break Stop::Dco(e);
        }
        // 划成功了才算走了一步。
        steps += 1;
        rec["swiped"] = json!(true);
        let swipe_ms = clock.now_ms().saturating_sub(t_swipe);
        let t_settle = clock.now_ms();
        let result = settle(dco, clock, p, &g);
        rec["timing_ms"] = json!({ "read": read_ms, "choose": choose_ms, "swipe": swipe_ms, "settle": clock.now_ms().saturating_sub(t_settle) });
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
                failed.push((chosen.mv, canonical(&g.cells)));
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
            Settle::NoGrid(why) => {
                rec["outcome"] = json!("stopped");
                sink(rec);
                break Stop::NoGrid(why);
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
