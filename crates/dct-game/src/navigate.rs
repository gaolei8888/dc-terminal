//! `--auto-next`：一局结束以后自己接着来——失败了点“再来一次”，弹窗里只点白名单里的字，
//! 认出生命用完、价格、广告、不认识的画面就停。打赢也停：下一关的版面不一样，现在的棋盘位置读不对。
//! 跟 dco 说话和计时都是传进来的（同 `play`），所以测试里换成假的。
use crate::play::{play, Clock, Dco, DcoError, Options, Profile, Seen, Stop, Summary};
use crate::screen::{classify, Element, Screen};
use serde_json::{json, Value};

pub struct NavOptions {
    /// 整个命令一共最多走几步棋（不是每一局）。
    pub max_steps: usize,
    /// 整个命令最多点几次“开始 / 再来一次”；每一次都会用掉一条生命。
    pub tries: usize,
    pub dry_run: bool,
}

const AFTER_TAP_FIRST_MS: u64 = 400;
const POLL_MS: u64 = 300;
const CHANGE_GIVE_UP_MS: u64 = 8_000;
/// 整个命令里点按钮的总次数上限：防止两个弹窗互相弹来弹去停不下来。
const MAX_TAPS: usize = 40;

fn texts(seen: &Seen) -> Vec<String> {
    seen.elements.iter().map(|e| e.text.clone()).collect()
}

fn sorted_texts(seen: &Seen) -> Vec<String> {
    let mut t = texts(seen);
    t.sort();
    t
}

fn nav_record(clock: &dyn Clock, seen: &Seen, screen: &str, tapped: Option<&str>, outcome: &str) -> Value {
    json!({
        "schema": 1, "kind": "nav", "time_ms": clock.now_ms(), "observation_id": seen.observation_id,
        "screen": screen, "texts": texts(seen), "tapped": tapped, "outcome": outcome,
    })
}

/// 点一个元素，再等画面变。变了返回 `Ok(true)`，8 秒不变返回 `Ok(false)`；dco 拒绝或出错就把错误交出去。
fn tap_and_wait(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, seen: &Seen, el: &Element) -> Result<bool, DcoError> {
    dco.tap(&seen.snapshot_id, &el.id)?;
    let before = sorted_texts(seen);
    let t0 = clock.now_ms();
    clock.sleep_ms(AFTER_TAP_FIRST_MS);
    loop {
        let now = dco.see_text(p)?;
        if sorted_texts(&now) != before {
            return Ok(true);
        }
        if clock.now_ms().saturating_sub(t0) >= CHANGE_GIVE_UP_MS {
            return Ok(false);
        }
        clock.sleep_ms(POLL_MS);
    }
}

pub fn auto_next(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, o: &NavOptions, sink: &mut dyn FnMut(Value)) -> Summary {
    let (mut steps, mut tries, mut taps, mut no_effect) = (0usize, 0usize, 0usize, 0usize);
    // after_board：刚玩完一局，还没有被明确告知“再来一次”。这时看到的 Play 可能是通关后的下一关，不点。
    let (mut after_board, mut retrying) = (false, false);
    let stop = loop {
        let seen = match dco.see_text(p) {
            Ok(s) => s,
            Err(e) => break Stop::Dco(e),
        };
        let screen = classify(&seen.elements);
        // 要点哪个元素、它是什么类型；不点的情况直接在这里停。
        let (kind, el) = match screen {
            Screen::LivesOut => {
                sink(nav_record(clock, &seen, "lives_out", None, "stopped"));
                break Stop::LivesOut;
            }
            Screen::Won => {
                sink(nav_record(clock, &seen, "won", None, "stopped"));
                break Stop::Won;
            }
            Screen::Money => {
                sink(nav_record(clock, &seen, "money", None, "stopped"));
                break Stop::Money;
            }
            Screen::Ad => {
                sink(nav_record(clock, &seen, "ad", None, "stopped"));
                break Stop::Ad;
            }
            Screen::Dismiss(el) => ("dismiss", el),
            Screen::Retry(el) => {
                if tries >= o.tries {
                    break Stop::TriesDone;
                }
                ("retry", el)
            }
            Screen::PlayButton(el) => {
                if after_board && !retrying {
                    sink(nav_record(clock, &seen, "level_ended", None, "stopped"));
                    break Stop::LevelEnded;
                }
                if tries >= o.tries {
                    break Stop::TriesDone;
                }
                ("play", el)
            }
            Screen::Unknown => {
                // 不是任何已知的按钮画面：看看是不是棋盘。
                match dco.read_grid(p) {
                    Ok(g) if g.classes.iter().filter(|c| c.count >= 2).count() >= 3 => {
                        let remaining = o.max_steps.saturating_sub(steps);
                        if remaining == 0 {
                            break Stop::StepsDone;
                        }
                        let s = play(dco, clock, p, &Options { max_steps: remaining, dry_run: o.dry_run }, sink);
                        steps += s.steps;
                        match s.stop {
                            // 画面变了（结算页、弹窗）：回到上面重新看是什么。
                            Stop::NoGrid(_) | Stop::ClassesChanged { .. } => {
                                after_board = true;
                                retrying = false;
                                no_effect = 0;
                                continue;
                            }
                            other => break other,
                        }
                    }
                    Ok(_) => {
                        sink(nav_record(clock, &seen, "unknown", None, "stopped"));
                        break Stop::UnknownScreen(texts(&seen));
                    }
                    Err(e) if e.code == "not_a_grid" => {
                        sink(nav_record(clock, &seen, "unknown", None, "stopped"));
                        break Stop::UnknownScreen(texts(&seen));
                    }
                    Err(e) => break Stop::Dco(e),
                }
            }
        };
        if o.dry_run {
            sink(nav_record(clock, &seen, kind, Some(&el.text), "dry_run"));
            break Stop::DryRun;
        }
        if taps >= MAX_TAPS {
            break Stop::TapLimit;
        }
        taps += 1;
        if kind != "dismiss" {
            tries += 1;
        }
        match tap_and_wait(dco, clock, p, &seen, &el) {
            Ok(true) => {
                sink(nav_record(clock, &seen, kind, Some(&el.text), "changed"));
                no_effect = 0;
                if kind == "retry" {
                    after_board = false;
                    retrying = true;
                }
            }
            Ok(false) => {
                sink(nav_record(clock, &seen, kind, Some(&el.text), "no_effect"));
                no_effect += 1;
                if no_effect >= 2 {
                    break Stop::NoEffect;
                }
            }
            Err(e) => {
                sink(nav_record(clock, &seen, kind, Some(&el.text), "stopped"));
                break Stop::Dco(e);
            }
        }
    };
    Summary { steps, stop }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::GridRead;

    fn grid(cells: &[&[u16]]) -> GridRead {
        let (rows, cols) = (cells.len(), cells[0].len());
        let mut ids: Vec<u16> = cells.iter().flat_map(|r| r.iter().copied()).collect();
        ids.sort();
        ids.dedup();
        let classes: Vec<Value> = ids.iter().map(|&i| json!({"id": i, "count": cells.iter().flat_map(|r| r.iter()).filter(|&&c| c == i).count()})).collect();
        serde_json::from_value(json!({
            "rows": rows, "cols": cols, "cells": cells, "odd": vec![vec![false; cols]; rows], "classes": classes
        }))
        .unwrap()
    }

    /// 能走的棋盘（把 (0,2) 和 (1,2) 对换，第 0 行四连），三个大类别。
    const A: &[&[u16]] = &[&[1, 1, 2, 1], &[2, 3, 1, 3], &[3, 2, 3, 2]];
    /// 没有任何能走的步，也是三个大类别。
    const DEAD: &[&[u16]] = &[&[1, 2, 3, 1], &[3, 1, 2, 3], &[2, 3, 1, 2]];

    #[derive(Clone)]
    enum Frame {
        Text(Vec<&'static str>),
        Board(GridRead),
    }

    /// 一个按帧往前走的假世界：点了按钮、划了一下，就换到下一帧；最后一帧停在那里。
    struct World {
        frames: Vec<Frame>,
        at: usize,
        taps: Vec<String>,
        swipes: usize,
        tap_error: Option<DcoError>,
        /// 为真时，点按钮不换帧（模拟点了没反应）。
        stuck: bool,
        /// 逐次点按钮的脚本：`true` = 这一次点了没反应。用完以后看 `stuck`。
        stuck_script: Vec<bool>,
    }

    impl World {
        fn new(frames: Vec<Frame>) -> World {
            World { frames, at: 0, taps: vec![], swipes: 0, tap_error: None, stuck: false, stuck_script: vec![] }
        }
        fn advance(&mut self) {
            if self.at + 1 < self.frames.len() {
                self.at += 1;
            }
        }
        /// 两个文字帧交替，让每次点完画面都“变”，但永远停在弹窗上。
        fn cycling(a: Vec<&'static str>, b: Vec<&'static str>) -> World {
            let mut frames = vec![];
            for i in 0..200 {
                frames.push(Frame::Text(if i % 2 == 0 { a.clone() } else { b.clone() }));
            }
            World::new(frames)
        }
    }

    impl Dco for World {
        fn read_grid(&mut self, _: &Profile) -> Result<GridRead, DcoError> {
            match &self.frames[self.at] {
                Frame::Board(g) => Ok(g.clone()),
                Frame::Text(_) => Err(DcoError { code: "not_a_grid".into(), message: "这块区域分不清类别".into() }),
            }
        }
        fn swipe(&mut self, _: &Profile, _: (f64, f64), _: (f64, f64)) -> Result<(), DcoError> {
            self.swipes += 1;
            self.advance();
            Ok(())
        }
        fn see_text(&mut self, _: &Profile) -> Result<Seen, DcoError> {
            let texts: Vec<&str> = match &self.frames[self.at] {
                Frame::Text(t) => t.clone(),
                Frame::Board(_) => vec!["38", "90"],
            };
            Ok(Seen {
                snapshot_id: format!("s{}", self.at),
                observation_id: Some(format!("obs-{}", self.at)),
                elements: texts.iter().enumerate().map(|(i, t)| Element { id: format!("e{}", i + 1), text: t.to_string() }).collect(),
            })
        }
        fn tap(&mut self, snapshot_id: &str, element_id: &str) -> Result<(), DcoError> {
            if let Some(e) = self.tap_error.clone() {
                return Err(e);
            }
            // 点的必须是眼前这一帧里的元素（snapshot 对得上）。
            assert_eq!(snapshot_id, format!("s{}", self.at), "点的不是刚读到的那一屏");
            let text = match &self.frames[self.at] {
                Frame::Text(t) => t[element_id.trim_start_matches('e').parse::<usize>().unwrap() - 1].to_string(),
                Frame::Board(_) => panic!("棋盘上不该点按钮"),
            };
            self.taps.push(text);
            let stuck = if self.stuck_script.is_empty() { self.stuck } else { self.stuck_script.remove(0) };
            if !stuck {
                self.advance();
            }
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

    fn profile() -> Profile {
        Profile { window: json!({"app": "x"}), region: [0.2, 0.3, 0.5, 0.4], rows: 3, cols: 4, extra: json!({}) }
    }

    fn run(w: &mut World, tries: usize, dry: bool) -> (Summary, Vec<Value>) {
        let mut log = vec![];
        let s = auto_next(w, &mut Clk(0), &profile(), &NavOptions { max_steps: 50, tries, dry_run: dry }, &mut |v| log.push(v));
        (s, log)
    }

    fn text(t: &[&'static str]) -> Frame {
        Frame::Text(t.to_vec())
    }
    fn board(c: &[&[u16]]) -> Frame {
        Frame::Board(grid(c))
    }

    #[test]
    fn a_failed_level_is_retried_through_the_start_box_and_played_again() {
        let mut w = World::new(vec![
            board(A),
            text(&["Out of moves", "Try again"]),
            text(&["Level 1712", "Select boosters:", "Play", "B Play"]),
            board(DEAD),
        ]);
        let (s, log) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::NoMoves);
        assert_eq!(w.taps, ["Try again", "Play"]);
        assert_eq!(w.swipes, 1);
        let screens: Vec<&str> = log.iter().filter(|r| r["kind"] == "nav").map(|r| r["screen"].as_str().unwrap()).collect();
        assert_eq!(screens, ["retry", "play"]);
        // 每条 nav 记录都带着画面上的字（以后补白名单的证据）
        assert!(log.iter().filter(|r| r["kind"] == "nav").all(|r| r["texts"].as_array().unwrap().len() >= 2));
    }

    #[test]
    fn after_a_level_a_play_button_is_not_pressed_because_it_may_be_the_next_level() {
        let mut w = World::new(vec![board(A), text(&["Daily Stamps", "Play"])]);
        let (s, log) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::LevelEnded);
        assert!(w.taps.is_empty());
        assert_eq!(log.last().unwrap()["screen"], "level_ended");
    }

    #[test]
    fn the_guard_survives_dismissing_popups_after_a_level() {
        let mut w = World::new(vec![board(A), text(&["Not now"]), text(&["Daily Stamps", "Play"])]);
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::LevelEnded);
        assert_eq!(w.taps, ["Not now"]);
    }

    #[test]
    fn a_fresh_start_presses_play_then_plays_the_board() {
        let mut w = World::new(vec![text(&["Daily Stamps", "Play"]), board(DEAD)]);
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::NoMoves);
        assert_eq!(w.taps, ["Play"]);
    }

    #[test]
    fn popups_are_dismissed_one_after_another() {
        let mut w = World::new(vec![text(&["Not now"]), text(&["Got it"]), text(&["Close"]), board(DEAD)]);
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::NoMoves);
        assert_eq!(w.taps, ["Not now", "Got it", "Close"]);
    }

    #[test]
    fn money_ads_lives_won_and_unknown_screens_stop_without_a_single_tap() {
        for (frame, want) in [
            (text(&["Buy 💎 5"]), Stop::Money),
            (text(&["Watch an ad for a sweet treat", "Watch ad"]), Stop::Ad),
            (text(&["No more lives", "Ask friends"]), Stop::LivesOut),
            (text(&["Level Complete!"]), Stop::Won),
            (text(&["Collect your daily treat!", "Claim"]), Stop::UnknownScreen(vec!["Collect your daily treat!".into(), "Claim".into()])),
        ] {
            let mut w = World::new(vec![frame]);
            let (s, log) = run(&mut w, 5, false);
            assert_eq!(s.stop, want);
            assert!(w.taps.is_empty(), "{want:?} 不该点任何东西");
            assert_eq!(log.last().unwrap()["outcome"], "stopped");
        }
    }

    #[test]
    fn a_priced_failure_popup_with_no_thanks_is_declined_and_play_goes_on() {
        let mut w = World::new(vec![
            board(A),
            text(&["Out of moves", "Continue for 💎 5", "No thanks"]),
            text(&["Out of moves", "Try again"]),
            text(&["Play"]),
            board(DEAD),
        ]);
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::NoMoves);
        assert_eq!(w.taps, ["No thanks", "Try again", "Play"]);
    }

    #[test]
    fn a_button_that_does_nothing_stops_after_two_tries() {
        let mut w = World::new(vec![text(&["Not now"])]);
        w.stuck = true;
        let (s, log) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::NoEffect);
        assert_eq!(w.taps.len(), 2);
        assert_eq!(log.iter().filter(|r| r["outcome"] == "no_effect").count(), 2);
    }

    #[test]
    fn the_retry_limit_counts_play_and_try_again_but_not_dismissals() {
        let mut w = World::cycling(vec!["Play", "a"], vec!["Play", "b"]);
        let (s, _) = run(&mut w, 2, false);
        assert_eq!(s.stop, Stop::TriesDone);
        assert_eq!(w.taps.len(), 2);
    }

    #[test]
    fn popups_that_keep_coming_back_hit_the_tap_limit() {
        let mut w = World::cycling(vec!["Close", "a"], vec!["Close", "b"]);
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::TapLimit);
        assert_eq!(w.taps.len(), MAX_TAPS);
    }

    #[test]
    fn dry_run_taps_nothing_and_says_what_it_would_press() {
        let mut w = World::new(vec![text(&["Daily Stamps", "Play"]), board(DEAD)]);
        let (s, log) = run(&mut w, 5, true);
        assert_eq!(s.stop, Stop::DryRun);
        assert!(w.taps.is_empty());
        assert_eq!((log[0]["outcome"].as_str(), log[0]["tapped"].as_str()), (Some("dry_run"), Some("Play")));
    }

    #[test]
    fn a_dco_refusal_to_tap_is_passed_on() {
        let mut w = World::new(vec![text(&["Play"]), board(DEAD)]);
        w.tap_error = Some(DcoError { code: "needs_ticket".into(), message: "x".into() });
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::Dco(DcoError { code: "needs_ticket".into(), message: "x".into() }));
    }

    #[test]
    fn the_step_budget_is_shared_across_levels() {
        let mut w = World::new(vec![board(A), board(A)]);
        let mut log = vec![];
        let s = auto_next(&mut w, &mut Clk(0), &profile(), &NavOptions { max_steps: 1, tries: 5, dry_run: false }, &mut |v| log.push(v));
        assert_eq!(s.steps, 1);
        assert_eq!(s.stop, Stop::StepsDone);
    }

    #[test]
    fn dismissals_do_not_use_up_the_retry_budget() {
        let mut w = World::new(vec![text(&["Not now"]), text(&["Got it"]), text(&["Daily Stamps", "Play"]), board(DEAD)]);
        let (s, _) = run(&mut w, 1, false);
        assert_eq!(s.stop, Stop::NoMoves);
        assert_eq!(w.taps, ["Not now", "Got it", "Play"]);
    }

    #[test]
    fn a_button_that_works_resets_the_no_effect_count() {
        // 没反应、成功、没反应、成功：每次都只是“一次没反应”，不该停
        let mut w = World::new(vec![text(&["Not now", "a"]), text(&["Not now", "b"]), text(&["Not now", "c"]), board(DEAD)]);
        w.stuck_script = vec![true, false, true, false, false];
        let (s, _) = run(&mut w, 5, false);
        assert_eq!(s.stop, Stop::NoMoves);
    }
}
