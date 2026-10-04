//! 离线回放：拿以前真机记下的局面问模型，看它选得怎么样。不碰游戏、不碰手机。
use dct_game::ask::{board_text, describe, Advisor, AskInput};
use dct_game::board::GridRead;
use dct_game::choose::Candidate;
use dct_game::sim::{Move, Outcome};
use serde_json::Value;

#[derive(Debug, Default)]
pub struct Report {
    pub asked: usize,
    pub answered: usize,
    pub valid: usize,
    pub picked_rules_first: usize,
    pub chance_first: f64,
    pub avg_ms: u64,
    /// 带标准答案（good 或 bad 非空）的被问局面数。
    pub labelled: usize,
    pub picked_good: usize,
    pub picked_bad: usize,
}

fn candidate(v: &Value) -> Option<Candidate> {
    let p = |x: &Value| Some((x[0].as_u64()? as usize, x[1].as_u64()? as usize));
    let f = &v["features"];
    let n = |k: &str| f[k].as_u64().unwrap_or(0) as usize;
    Some(Candidate {
        mv: Move { a: p(&v["a"])?, b: p(&v["b"])? },
        score: v["score"].as_f64().unwrap_or(0.0),
        features: Outcome {
            cleared: n("cleared"), cascade: n("cascade"), striped: n("striped"), wrapped: n("wrapped"),
            bomb: n("bomb"), triggered: n("triggered"),
            special_swap: f["special_swap"].as_bool().unwrap_or(false), lowest_row: n("lowest_row"),
        },
    })
}

pub fn replay(lines: &[Value], adv: &dyn Advisor, goal_of: &dyn Fn(&str) -> String, limit: usize) -> Report {
    let mut r = Report::default();
    let mut seed: u64 = 7;
    let mut total_ms = 0u128;
    for rec in lines {
        if r.asked >= limit {
            break;
        }
        if rec["dry_run"].as_bool() == Some(true) {
            continue;
        }
        let cands: Vec<Candidate> = rec["candidates"].as_array().map(|a| a.iter().filter_map(candidate).take(5).collect()).unwrap_or_default();
        if cands.len() < 3 {
            continue;
        }
        // 记录里没存「哪些格子不是糖」：全当普通格
        let odd: Vec<Vec<bool>> = rec["cells"].as_array().map_or(vec![], |rows| rows.iter().map(|r| vec![false; r.as_array().map_or(0, Vec::len)]).collect());
        let Ok(g) = serde_json::from_value::<GridRead>(serde_json::json!({
            "rows": rec["cells"].as_array().map_or(0, Vec::len),
            "cols": rec["cells"][0].as_array().map_or(0, Vec::len),
            "cells": rec["cells"], "odd": odd, "classes": rec["classes"],
        })) else {
            continue;
        };
        // 固定种子的洗牌：同一份记录每次回放顺序一样，结果可比
        let mut order: Vec<usize> = (0..cands.len()).collect();
        for i in (1..order.len()).rev() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            order.swap(i, (seed >> 33) as usize % (i + 1));
        }
        let input = AskInput {
            board: board_text(&g, &[]),
            goal: goal_of(rec["game"].as_str().unwrap_or("")),
            candidates: order.iter().map(|&k| describe(&cands[k])).collect(),
            failed: vec![],
        };
        let idx = |key: &str| -> Vec<usize> {
            rec[key].as_array().map(|v| v.iter().filter_map(|x| x.as_u64().map(|n| n as usize)).collect()).unwrap_or_default()
        };
        // 下标指原始 candidates（洗牌前）
        let (good, bad) = (idx("good"), idx("bad"));
        let is_labelled = !good.is_empty() || !bad.is_empty();
        r.asked += 1;
        if is_labelled {
            r.labelled += 1;
        }
        r.chance_first += 1.0 / cands.len() as f64;
        let t = std::time::Instant::now();
        let a = adv.pick(&input);
        total_ms += t.elapsed().as_millis();
        if let Some(a) = a {
            r.answered += 1;
            if let Some(k) = a.choice {
                r.valid += 1;
                if let Some(&orig) = order.get(k) {
                    if orig == 0 {
                        r.picked_rules_first += 1;
                    }
                    if good.contains(&orig) {
                        r.picked_good += 1;
                    }
                    if bad.contains(&orig) {
                        r.picked_bad += 1;
                    }
                }
            }
        }
    }
    if r.asked > 0 {
        r.avg_ms = (total_ms / r.asked as u128) as u64;
    }
    r
}

pub fn run(args: &[String]) -> i32 {
    let mut files: Vec<String> = vec![];
    let mut limit = 40usize;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--limit" {
            match it.next().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n >= 1) {
                Some(n) => limit = n,
                None => {
                    eprintln!("--limit 后面要写个数（至少 1）。{USAGE}");
                    return 2;
                }
            }
        } else if a.starts_with("--") {
            eprintln!("不认识 {a}。{USAGE}");
            return 2;
        } else {
            files.push(a.clone());
        }
    }
    if files.is_empty() {
        eprintln!("{USAGE}");
        return 2;
    }
    let mut lines: Vec<Value> = vec![];
    for f in &files {
        match std::fs::read_to_string(f) {
            Ok(s) => lines.extend(s.lines().filter_map(|l| serde_json::from_str(l).ok())),
            Err(e) => {
                eprintln!("读不了 {f}：{e}");
                return 1;
            }
        }
    }
    let loaded = match crate::cli::load_llm_backend() {
        Ok(l) => l,
        Err(crate::cli::LoadLlmError::NotEnabled(_)) => {
            println!("没开大模型，没法测。");
            return 1;
        }
        Err(crate::cli::LoadLlmError::Problem { .. }) => {
            println!("大模型连不上，没法测。可以先运行 dct llm check 看原因。");
            return 1;
        }
    };
    let adv = super::advisor::LlmAdvisor::new(loaded.backend, loaded.model);
    let r = replay(&lines, &adv, &|_| "尽量多消".into(), limit);
    println!("{}", report_text(&r));
    0
}

pub const USAGE: &str = "用法：dct game ask-bench <记录文件>... [--limit 40]";

pub fn report_text(r: &Report) -> String {
    let mut t = format!(
        "问了 {} 个局面，回答成合法编号的 {} 个，选中规则第一名的 {} 个（乱选大约 {:.0} 个），平均每次 {} 毫秒。",
        r.asked, r.valid, r.picked_rules_first, r.chance_first, r.avg_ms
    );
    if r.labelled > 0 {
        t.push_str(&format!("\n其中有标准答案的 {} 个：选中好走法 {} 个，选中坏走法 {} 个。", r.labelled, r.picked_good, r.picked_bad));
    } else {
        t.push_str("\n说明：回放里没有「哪些格子不是糖」和「这一关的目标」，所以这个数只说明它比乱选强多少，不说明它选得对不对。");
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use dct_game::ask::{Advice, Advisor, AskInput};
    use serde_json::json;

    struct First;
    impl Advisor for First {
        fn pick(&self, _: &AskInput) -> Option<Advice> {
            Some(Advice { choice: Some(0), reason: String::new(), raw: "1".into(), model: "t".into(), tokens: None })
        }
    }

    fn rec(n: usize, dry: bool) -> Value {
        let cands: Vec<Value> = (0..n).map(|k| json!({"a":[0,k],"b":[1,k],"score":1.0,
            "features":{"cleared":3,"cascade":0,"striped":0,"wrapped":0,"bomb":0,"triggered":0,"special_swap":false,"lowest_row":1}})).collect();
        json!({"game":"g","cells":[[1,1],[2,2]],"classes":[{"id":1,"count":2},{"id":2,"count":2}],
               "candidates": cands, "chosen":0, "dry_run": dry, "outcome": "moved"})
    }

    #[test]
    fn replay_skips_dry_runs_and_tiny_candidate_lists_and_counts_the_rest() {
        let lines = vec![rec(5, false), rec(5, true), rec(2, false), rec(4, false)];
        let r = replay(&lines, &First, &|_| "尽量多消".into(), 10);
        assert_eq!(r.asked, 2);
        assert_eq!(r.answered, 2);
        assert_eq!(r.valid, 2);
        // 候选是打乱的，模型总选第一个，所以「选中规则第一名」只可能是碰巧
        assert!(r.picked_rules_first <= 2);
        // 随机期望：5 个候选 1/5，4 个候选 1/4（候选最多取前 5 个）
        assert!((r.chance_first - (0.2 + 0.25)).abs() < 1e-9);
    }

    #[test]
    fn replay_respects_the_limit() {
        let lines: Vec<Value> = (0..10).map(|_| rec(5, false)).collect();
        assert_eq!(replay(&lines, &First, &|_| String::new(), 3).asked, 3);
    }

    #[test]
    fn report_text_has_the_caveat_and_numbers() {
        let t = report_text(&Report { asked: 40, answered: 40, valid: 38, picked_rules_first: 13, chance_first: 8.2, avg_ms: 900, ..Default::default() });
        assert!(t.contains("问了 40 个局面，回答成合法编号的 38 个，选中规则第一名的 13 个（乱选大约 8 个），平均每次 900 毫秒"));
        assert!(t.contains("不说明它选得对不对"));
    }

    struct Pick(usize);
    impl Advisor for Pick {
        fn pick(&self, _: &AskInput) -> Option<Advice> {
            Some(Advice { choice: Some(self.0), reason: String::new(), raw: String::new(), model: "t".into(), tokens: None })
        }
    }

    fn labelled(good: Vec<usize>, bad: Vec<usize>) -> Value {
        let mut r = rec(5, false);
        r["good"] = json!(good);
        r["bad"] = json!(bad);
        r
    }

    #[test]
    fn replay_counts_good_and_bad_picks_on_labelled_lines() {
        let lines = vec![labelled(vec![0], vec![1])];
        let r = replay(&lines, &Pick(0), &|_| String::new(), 10);
        assert_eq!(r.labelled, 1);
        assert!(r.picked_good + r.picked_bad <= 1);
    }

    #[test]
    fn replay_good_and_bad_are_in_terms_of_original_candidate_indexes() {
        let lines = vec![labelled(vec![], vec![0, 1, 2, 3, 4])];
        let r = replay(&lines, &Pick(2), &|_| String::new(), 10);
        assert_eq!((r.picked_good, r.picked_bad), (0, 1));
        let lines = vec![labelled(vec![0, 1, 2, 3, 4], vec![])];
        let r = replay(&lines, &Pick(2), &|_| String::new(), 10);
        assert_eq!((r.picked_good, r.picked_bad), (1, 0));
    }

    #[test]
    fn replay_maps_each_displayed_pick_back_to_exactly_one_original_candidate() {
        // 展示顺序是候选的一个排列：逐个展示位去选，原始候选 0 恰好被选中一次
        let lines = vec![labelled(vec![0], vec![1])];
        let (mut good, mut bad) = (0, 0);
        for k in 0..5 {
            let r = replay(&lines, &Pick(k), &|_| String::new(), 10);
            good += r.picked_good;
            bad += r.picked_bad;
        }
        assert_eq!((good, bad), (1, 1));
    }

    #[test]
    fn replay_survives_out_of_range_or_empty_labels() {
        let lines = vec![labelled(vec![99], vec![]), labelled(vec![], vec![]), rec(5, false)];
        let r = replay(&lines, &Pick(0), &|_| String::new(), 10);
        assert_eq!(r.asked, 3);
        assert_eq!(r.picked_good, 0);
        assert_eq!(r.labelled, 1);
    }

    #[test]
    fn report_text_adds_the_labelled_line_and_drops_the_caveat_only_when_labelled() {
        let t = report_text(&Report { asked: 10, valid: 10, labelled: 4, picked_good: 3, picked_bad: 1, ..Default::default() });
        assert!(t.contains("其中有标准答案的 4 个：选中好走法 3 个，选中坏走法 1 个。"));
        assert!(!t.contains("不说明它选得对不对"));
    }
}
