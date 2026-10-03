//! 说给用户（和转述给用户的 agent）听的话。不出现类别编号、分数公式；行列从 1 数、从上往下。
//! 第一轮只有中文；换成 i18n 是后面的事（设计「不在这一轮」之外的收尾项）。
use dct_game::play::{DcoError, Stop};
use serde_json::Value;
use std::path::Path;

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

pub fn step_line(step: usize, rec: &Value) -> String {
    let c = &rec["candidates"][rec["chosen"].as_u64().unwrap_or(0) as usize];
    let pos = |p: &Value| format!("第 {} 行第 {} 列", n(&p[0]) + 1, n(&p[1]) + 1);
    let outcome = rec["outcome"].as_str().unwrap_or("");
    // 划被拒绝：没有走成这一步，不能说消了几颗，也不能叫它第几步。
    if outcome == "stopped" && rec["swiped"].as_bool() == Some(false) {
        return format!("想走 {} ↔ {}，这一步没划成", pos(&c["a"]), pos(&c["b"]));
    }
    // 划了，但画面一直没停下来：消了几颗是我们的预测，没有看到结果，不报。
    if outcome == "stopped" {
        return format!("第 {step} 步：{} ↔ {}，划了以后没等到画面停下", pos(&c["a"]), pos(&c["b"]));
    }
    let f = &c["features"];
    let mut s = format!("第 {step} 步：{} ↔ {}，消 {} 颗", pos(&c["a"]), pos(&c["b"]), n(&f["cleared"]));
    for (key, what) in [("striped", "条纹糖"), ("wrapped", "包装糖"), ("bomb", "彩色炸弹")] {
        if n(&f[key]) > 0 {
            s += &format!("，做出{what}");
        }
    }
    if n(&f["triggered"]) > 0 {
        s += &format!("，引爆 {} 颗特殊糖", n(&f["triggered"]));
    }
    if f["special_swap"].as_bool().unwrap_or(false) {
        s += "，两颗特殊糖互换";
    }
    match outcome {
        "dry_run" => s += "（试走，没有真划）",
        outcome => {
            let t = &rec["timing_ms"];
            if t.is_object() {
                s += &format!("（读 {} ms，选 {} ms，划 {} ms）", n(&t["read"]), n(&t["choose"]), n(&t["swipe"]));
            }
            if outcome == "no_change" {
                s += "；这一步划了没反应";
            }
        }
    }
    s
}

/// 写进记录文件的停下原因（给程序看的，不是给人看的）。
pub fn stop_code(stop: &Stop) -> &'static str {
    match stop {
        Stop::NoGrid(_) => "no_grid",
        Stop::ClassesChanged { .. } => "classes_changed",
        Stop::NoMoves => "no_moves",
        Stop::Stuck => "stuck",
        Stop::StillMoving => "still_moving",
        Stop::StepsDone => "steps_done",
        Stop::DryRun => "dry_run",
        Stop::Dco(_) => "dco",
    }
}

pub fn dco_error(e: &DcoError) -> String {
    match e.code.as_str() {
        "dco_down" | "dco_timeout" => e.message.clone(),
        "dco_refused" => "dco 不认这把钥匙（~/.dco/token），先重启 dco。".into(),
        "dco_too_old" => "dco 版本太旧，先更新 dco 再试。".into(),
        "dco_blocked" => "这里不让连 dco（可能是这个 AI 的沙盒限制）。换一个能访问本机的地方再试。".into(),
        "halted" => "dco 急停了，这次停下。要接着玩，先让 dco 恢复。".into(),
        "paused" => "dco 暂停了，这次停下。".into(),
        "screen_locked" => "屏幕锁着，解锁以后再试。".into(),
        "not_found" => "没找到 iPhone 镜像的窗口。先打开它，进到一关里再试。".into(),
        _ => format!("dco 说：{}", e.message),
    }
}

/// 最后一句话，和退出码：正常结束（走满步数、没有能走的步、试走）是 0，其余都是 1。
pub fn stop_line(stop: &Stop, steps: usize, log: &Path) -> (String, i32) {
    let tail = format!("这次走了 {steps} 步，记录在 {}", log.display());
    match stop {
        Stop::StepsDone => (format!("到设定的步数了。{tail}。要接着玩，再运行一次。"), 0),
        Stop::NoMoves => (format!("停了：没有能走的步了。{tail}"), 0),
        Stop::DryRun => ("试走结束，没有真划。".into(), 0),
        Stop::NoGrid(why) => (format!("停了：读不出棋盘（{why}）。多半是这一关结束了，或者弹出了别的画面。{tail}"), 1),
        Stop::ClassesChanged { was, now } => (
            format!("停了：棋盘上的颜色种类一下子变多了（原来 {was} 种，现在 {now} 种），多半是这一关结束了，或者弹出了窗口。{tail}"),
            1,
        ),
        Stop::Stuck => (format!("停了：划了几次画面都没有变化。请看一眼屏幕上是不是弹出了什么。{tail}"), 1),
        Stop::StillMoving => (format!("停了：等了 8 秒画面还在动。{tail}"), 1),
        Stop::Dco(e) => (format!("停了：{} {tail}", dco_error(e)), 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(extra: Value) -> Value {
        let mut r = json!({
            "chosen": 1,
            "candidates": [
                {"a": [0, 0], "b": [0, 1], "score": 3.0, "features": {"cleared": 3, "striped": 0, "wrapped": 0, "bomb": 0, "triggered": 0, "special_swap": false}},
                {"a": [6, 1], "b": [6, 2], "score": 9.0, "features": {"cleared": 4, "striped": 1, "wrapped": 0, "bomb": 0, "triggered": 0, "special_swap": false}}
            ],
            "outcome": "moved", "timing_ms": {"read": 2, "choose": 0, "swipe": 262, "settle": 900}
        });
        for (k, v) in extra.as_object().unwrap() {
            r[k] = v.clone();
        }
        r
    }

    #[test]
    fn a_step_reads_as_plain_chinese_counting_from_one() {
        assert_eq!(
            step_line(3, &rec(json!({}))),
            "第 3 步：第 7 行第 2 列 ↔ 第 7 行第 3 列，消 4 颗，做出条纹糖（读 2 ms，选 0 ms，划 262 ms）"
        );
    }

    #[test]
    fn an_unmoved_step_and_a_dry_run_say_so() {
        assert!(step_line(1, &rec(json!({"outcome": "no_change"}))).ends_with("；这一步划了没反应"));
        let d = step_line(1, &rec(json!({"outcome": "dry_run", "timing_ms": null})));
        assert!(d.ends_with("（试走，没有真划）") && !d.contains("ms"), "{d}");
    }

    #[test]
    fn a_refused_swipe_is_not_a_step_and_claims_no_cleared_count() {
        let s = step_line(1, &rec(json!({"outcome": "stopped", "swiped": false})));
        assert!(s.contains("这一步没划成") && !s.contains("消 ") && !s.contains("第 1 步"), "{s}");
    }

    #[test]
    fn a_swipe_whose_settle_never_finished_says_so_and_claims_no_cleared_count() {
        let s = step_line(2, &rec(json!({"outcome": "stopped", "swiped": true})));
        assert!(s.contains("划了以后没等到画面停下") && !s.contains("消 ") && !s.contains("没划成"), "{s}");
    }

    #[test]
    fn no_internal_words_leak_into_a_step_line() {
        let s = step_line(1, &rec(json!({})));
        for w in ["class", "score", "类别", "分数", "odd", "cells"] {
            assert!(!s.contains(w), "{s}");
        }
    }

    #[test]
    fn exit_codes_separate_normal_ends_from_trouble() {
        let p = Path::new("/x/log.jsonl");
        let e = |code: &str| Stop::Dco(DcoError { code: code.into(), message: "m".into() });
        for (s, want) in [
            (Stop::StepsDone, 0),
            (Stop::NoMoves, 0),
            (Stop::DryRun, 0),
            (Stop::Stuck, 1),
            (Stop::StillMoving, 1),
            (Stop::NoGrid("x".into()), 1),
            (Stop::ClassesChanged { was: 5, now: 8 }, 1),
            (e("halted"), 1),
        ] {
            assert_eq!(stop_line(&s, 4, p).1, want, "{s:?}");
        }
        assert!(stop_line(&Stop::NoMoves, 4, p).0.contains("走了 4 步") && stop_line(&Stop::NoMoves, 4, p).0.contains("/x/log.jsonl"));
    }

    #[test]
    fn dco_errors_are_translated() {
        let e = |c: &str| DcoError { code: c.into(), message: "原话".into() };
        assert!(dco_error(&e("halted")).contains("急停"));
        assert!(dco_error(&e("paused")).contains("暂停"));
        assert!(dco_error(&e("screen_locked")).contains("锁"));
        assert!(dco_error(&e("not_found")).contains("iPhone 镜像"));
        assert_eq!(dco_error(&e("dco_down")), "原话");
        assert_eq!(dco_error(&e("dco_timeout")), "原话");
        assert!(dco_error(&e("dco_too_old")).contains("版本太旧"));
        assert!(dco_error(&e("dco_blocked")).contains("不让连 dco") && !dco_error(&e("dco_blocked")).contains("原话"));
        let refused = DcoError { code: "dco_refused".into(), message: r#"{"ok":false}"#.into() };
        assert_eq!(dco_error(&refused), "dco 不认这把钥匙（~/.dco/token），先重启 dco。");
        assert_eq!(dco_error(&e("weird")), "dco 说：原话");
    }

    #[test]
    fn the_stuck_sentence_does_not_claim_two_tries() {
        let l = stop_line(&Stop::Stuck, 1, Path::new("/x")).0;
        assert!(l.contains("划了几次画面都没有变化") && !l.contains("两次"), "{l}");
    }

    #[test]
    fn every_stop_has_a_distinct_code() {
        let all = [
            Stop::NoGrid("".into()),
            Stop::ClassesChanged { was: 1, now: 3 },
            Stop::NoMoves,
            Stop::Stuck,
            Stop::StillMoving,
            Stop::StepsDone,
            Stop::DryRun,
            Stop::Dco(DcoError { code: "x".into(), message: "".into() }),
        ];
        let mut codes: Vec<&str> = all.iter().map(stop_code).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
    }
}
