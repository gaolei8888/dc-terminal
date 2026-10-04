//! 一盘游戏怎么玩：读盘 → 选步 → 划 → 等画面停下 → 下一步，以及什么时候停。
//! 跟 dco 说话（`Dco`）和计时（`Clock`）都是传进来的，所以测试里换成假的，不碰真机也不真睡觉。
use crate::ask::{board_text, describe, describe_move, Advisor, AskInput};
use crate::board::{fixed_ids, looks_like_board, Board, GridRead};
use crate::choose::{choose, Candidate, Weights};
use crate::screen::Element;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct Profile {
    pub window: Value,
    pub region: [f64; 4], // x, y, w, h
    pub rows: usize,
    pub cols: usize,
    pub extra: Value, // inset / class_de / top_div / odd_share：原样带给 read_grid
    /// 「不是糖」的那几类（洞、蜂蜜块、糖果机……）的颜色；读到的类别颜色离其中之一不超过 match_de 就当它是。
    /// 空 = 没有这种格子（第一轮的行为）。这份数据来自用户每关的配置文件，代码里不写死任何游戏的颜色。
    pub fixed_rgb: Vec<[u8; 3]>,
    pub match_de: f64,
    /// 打分权重，来自配置文件的 `[weights]`；不写就是通用的中性默认（特殊糖全 0）。
    pub weights: Weights,
    /// 关卡号在画面上的写法（一个带恰好一个捕获组的正则），来自配置文件的 `level_pattern`。
    /// 有它、画面上对得上、又读得出合格棋盘，就直接当棋盘，不过 OCR 分类。没写 = 不启用。
    pub level_pattern: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DcoError {
    pub code: String,
    pub message: String,
}

/// 一次 `see`（OCR）：画面上读到的字和它们的编号。`snapshot_id` 要原样交给 `tap`。
#[derive(Clone, Debug)]
pub struct Seen {
    pub snapshot_id: String,
    pub observation_id: Option<String>,
    pub elements: Vec<Element>,
}

fn unsupported() -> DcoError {
    DcoError { code: "unsupported".into(), message: "这个 dco 不会认画面上的字".into() }
}

pub trait Dco {
    fn read_grid(&mut self, p: &Profile) -> Result<GridRead, DcoError>;
    /// 坐标是窗口比例（0～1）。
    fn swipe(&mut self, p: &Profile, from: (f64, f64), to: (f64, f64)) -> Result<(), DcoError>;
    /// 读画面上的字。只有“自动开始下一局”要用；默认不支持，所以只玩一关的假 dco 不用实现。
    fn see_text(&mut self, _p: &Profile) -> Result<Seen, DcoError> {
        Err(unsupported())
    }
    /// 点 `see_text` 读到的某个元素。dco 自己按那个元素上的字定档，带价格的会拒绝。
    fn tap(&mut self, _snapshot_id: &str, _element_id: &str) -> Result<(), DcoError> {
        Err(unsupported())
    }
    /// 告诉 dco 屏幕上的小章鱼现在在“想”还是“看”。只改它的样子，所以故意不返回错误：
    /// 这里出什么事都不许影响玩。默认什么都不做，只玩一关的假 dco 不用实现。
    fn show_status(&mut self, _state: &str) {}
}

pub trait Clock {
    fn now_ms(&self) -> u64;
    fn sleep_ms(&mut self, ms: u64);
}

/// 最多发给模型几个候选。
pub const ASK_SHOWN: usize = 8;

pub struct Options<'a> {
    pub max_steps: usize,
    pub dry_run: bool,
    /// 规则没把握时问的那个。`None` = 不问（和以前一样）。
    pub advisor: Option<&'a dyn Advisor>,
    /// 每一步都问（`--ask-every-step`）；否则只在这盘棋上已有失败的步时才问。
    pub ask_always: bool,
    /// 这一次 `play()` 最多问几次。
    pub ask_budget: usize,
    pub goal: &'a str,
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
    // ---- 下面几个只有 `--auto-next` 才会出现（navigate.rs）----
    /// 通关了：下一关版面不一样，先停。
    Won,
    /// 一局结束了，但看不出是通关还是没过，也没有明确的“再来一次”：先停，不乱点。
    LevelEnded,
    LivesOut,
    /// 出现了要花钱的画面，又没有安全的关闭按钮。
    Money,
    Ad,
    /// 不认识的画面；带着画面上读到的字。
    UnknownScreen(Vec<String>),
    /// 点了按钮，画面连着两次没变化。
    NoEffect,
    /// 点“开始 / 再来一次”的次数到上限。
    TriesDone,
    /// 总共点了太多次还没回到棋盘。
    TapLimit,
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

/// 只比较分组。`odd`（特殊糖标记）会跟着游戏的提示光晕和掉落动画闪，
/// 不能算成「棋盘变了」，否则没效果的一步会被当成有效，停不下来。
fn same(a: &GridRead, b: &GridRead) -> bool {
    canonical(&a.cells) == canonical(&b.cells)
}

const FIRST_WAIT_MS: u64 = 150;
const POLL_MS: u64 = 100;
const NO_CHANGE_MS: u64 = 3_000;
const GIVE_UP_MS: u64 = 8_000;
/// “没步可走”“画面变了”下结论前的停顿：出口还在一个个往下补糖，特效也会让颜色数暂时跳高，
/// 马上读到的往往是中间状态。
const CONFIRM_PAUSE_MS: u64 = 1_500;
/// 连着最多确认几次“没步可走”：盘面一直在变却始终没有步，也不能无限等。
const MAX_NO_MOVE_CONFIRMS: usize = 2;

fn big_classes(g: &GridRead) -> usize {
    g.classes.iter().filter(|c| c.count >= 2).count()
}

fn has_move(p: &Profile, g: &GridRead) -> bool {
    Board::from_read_fixed(g, &fixed_ids(g, &p.fixed_rgb, p.match_de), !p.fixed_rgb.is_empty()).is_ok_and(|b| !choose(&b, &p.weights).is_empty())
}

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

pub fn play(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, o: &Options<'_>, sink: &mut dyn FnMut(Value)) -> Summary {
    let mut steps = 0;
    let mut baseline: Option<usize> = None;
    let mut failed: Vec<(crate::sim::Move, Vec<Vec<u16>>)> = Vec::new();
    let mut asked = 0usize;
    let mut streak = 0;
    let mut current: Option<GridRead> = None;
    let mut no_move_confirms = 0;
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
        // 这一轮的第一张盘：大半是同一种平色的是弹窗不是棋盘，一步都不划。后面的读数不查（后期的盘可以很偏）。
        if baseline.is_none() && !looks_like_board(&g) {
            break Stop::NoGrid("这块区域看着不像棋盘".into());
        }
        // 只数至少两格的类别：一格的（彩色炸弹、条纹糖被读成自己的颜色）是“不认识”，忽略
        let big = big_classes(&g);
        let base = *baseline.get_or_insert(big);
        if big > base + 1 {
            if o.dry_run {
                break Stop::ClassesChanged { was: base, now: big };
            }
            // 特效清完一大片，颜色数会暂时跳高：停一下再读一遍，还高才算画面真的变了。
            clock.sleep_ms(CONFIRM_PAUSE_MS);
            match dco.read_grid(p) {
                Ok(re) => {
                    let re_big = big_classes(&re);
                    if re_big > base + 1 {
                        break Stop::ClassesChanged { was: base, now: re_big };
                    }
                    current = Some(re);
                    continue;
                }
                Err(e) if e.code == "not_a_grid" => break Stop::NoGrid(e.message),
                Err(e) => break Stop::Dco(e),
            }
        }
        let Ok(board) = Board::from_read_fixed(&g, &fixed_ids(&g, &p.fixed_rgb, p.match_de), !p.fixed_rgb.is_empty()) else {
            break Stop::NoGrid("棋盘的行列数对不上".into());
        };
        // 规则选步约 1 ms，不发 think/look：章鱼没法“想”这么短，停住的状态反而拖慢动画。
        // 只在问模型（慢）的那段时间发 think/look。
        let t_choose = clock.now_ms();
        let cands: Vec<Candidate> = choose(&board, &p.weights);
        let choose_ms = clock.now_ms().saturating_sub(t_choose);
        // 同一盘棋上划了没反应的步，不再重复选。
        let pick = cands.iter().position(|c| !failed.iter().any(|(m, cells)| *m == c.mv && *cells == canonical(&g.cells)));
        if cands.is_empty() && !o.dry_run && no_move_confirms < MAX_NO_MOVE_CONFIRMS {
            // 补糖还没补完时会有一瞬间没步可走：停一下再读，盘面变了或有步了就接着玩。
            no_move_confirms += 1;
            clock.sleep_ms(CONFIRM_PAUSE_MS);
            match dco.read_grid(p) {
                Ok(re) => {
                    if same(&re, &g) && !has_move(p, &re) {
                        break Stop::NoMoves;
                    }
                    current = Some(re);
                    continue;
                }
                Err(e) if e.code == "not_a_grid" => break Stop::NoGrid(e.message),
                Err(e) => break Stop::Dco(e),
            }
        }
        let Some(pick) = pick else {
            break if cands.is_empty() { Stop::NoMoves } else { Stop::Stuck };
        };
        let mut pick = pick;
        let mut ask_rec: Option<Value> = None;
        let mut decider = "rules";
        if let Some(adv) = o.advisor {
            let board_key = canonical(&g.cells);
            // 发给模型的候选：没失败过的，最多 ASK_SHOWN 个；下标指回 cands
            let offered: Vec<usize> = {
                let is_failed = |c: &Candidate| failed.iter().any(|(m, cells)| *m == c.mv && *cells == board_key);
                (0..cands.len()).filter(|&i| !is_failed(&cands[i])).take(ASK_SHOWN).collect()
            };
            let had_failed_here = failed.iter().any(|(_, cells)| *cells == board_key);
            if !o.dry_run && adv.available() && asked < o.ask_budget && offered.len() >= 2 && (o.ask_always || had_failed_here) {
                asked += 1;
                let fixed = fixed_ids(&g, &p.fixed_rgb, p.match_de);
                let failed_now: Vec<String> = failed.iter().filter(|(_, cells)| *cells == board_key).map(|(m, _)| describe_move(m)).collect();
                let input = AskInput {
                    board: board_text(&g, &fixed),
                    goal: o.goal.to_string(),
                    candidates: offered.iter().map(|&i| describe(&cands[i])).collect(),
                    failed: failed_now.clone(),
                };
                dco.show_status("think");
                let advice = adv.pick(&input);
                dco.show_status("look");
                if let Some(a) = advice {
                    let chosen_idx = a.choice.filter(|&k| k < offered.len()).map(|k| offered[k]);
                    ask_rec = Some(json!({
                        "model": a.model, "raw": a.raw, "reason": a.reason,
                        "asked": offered, "choice": chosen_idx,
                        "failed": failed_now, "goal": o.goal,
                    }));
                    if let Some(i) = chosen_idx {
                        pick = i;
                        decider = "model";
                    }
                }
            }
        }
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
            "chosen": pick, "decider": decider, "dry_run": o.dry_run, "swiped": false,
        });
        if let Some(a) = ask_rec {
            rec["ask"] = a;
        }
        if o.dry_run {
            rec["timing_ms"] = json!({ "read": read_ms, "choose": choose_ms, "swipe": 0, "settle": 0 });
            rec["outcome"] = json!("dry_run");
            sink(rec);
            break Stop::DryRun;
        }
        // 端点和起始时刻原样记进记录，章鱼叠加层的事件日志靠它跟 dct 真做的对账（窗口 0–1 比例）。
        let (from, to) = (centre(p, chosen.mv.a), centre(p, chosen.mv.b));
        let r4 = |v: f64| (v * 10000.0).round() / 10000.0;
        let t_swipe = clock.now_ms();
        let res = dco.swipe(p, from, to);
        let swipe_ms = clock.now_ms().saturating_sub(t_swipe);
        rec["swipe"] = json!({ "from": [r4(from.0), r4(from.1)], "to": [r4(to.0), r4(to.1)], "started_ms": t_swipe, "duration_ms": swipe_ms });
        if let Err(e) = res {
            rec["timing_ms"] = json!({ "read": read_ms, "choose": choose_ms, "swipe": swipe_ms, "settle": 0 });
            rec["outcome"] = json!("stopped");
            sink(rec);
            break Stop::Dco(e);
        }
        // 划成功了才算走了一步。
        steps += 1;
        no_move_confirms = 0;
        rec["swiped"] = json!(true);
        let t_settle = clock.now_ms();
        let result = settle(dco, clock, p, &g);
        rec["timing_ms"] = json!({ "read": read_ms, "choose": choose_ms, "swipe": swipe_ms, "settle": clock.now_ms().saturating_sub(t_settle) });
        match result {
            // 游戏动了一下又弹回原样（被笼子、锁住的糖），棋盘和划之前一样：算没反应：同一盘棋上不再重复选这一步。
            Settle::Settled(next) if same(&next, &g) => {
                rec["outcome"] = json!("no_change");
                rec["after_observation_id"] = json!(next.observation_id);
                sink(rec);
                streak += 1;
                failed.push((chosen.mv, canonical(&g.cells)));
                if streak >= 2 {
                    break Stop::Stuck;
                }
                current = Some(next);
            }
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
