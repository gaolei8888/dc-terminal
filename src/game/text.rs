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
    if rec["stalled"].as_bool() == Some(true) {
        s += "；这几步看起来没有进展";
    }
    if rec["decider"] == "model" {
        s += "（大模型选的）";
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
        Stop::Won => "won",
        Stop::LevelEnded => "level_ended",
        Stop::LivesOut => "lives_out",
        Stop::Money => "money",
        Stop::Ad => "ad",
        Stop::UnknownScreen(_) => "unknown_screen",
        Stop::NoEffect => "no_effect",
        Stop::TriesDone => "tries_done",
        Stop::TapLimit => "tap_limit",
    }
}

/// `--auto-next` 点按钮时印的一句话；停下的几种（没有点任何东西）由最后一句话说，这里返回 `None`。
pub fn nav_line(rec: &Value) -> Option<String> {
    let tapped = rec["tapped"].as_str()?;
    let what = match rec["screen"].as_str().unwrap_or("") {
        "play" => format!("点了「{tapped}」，开始新的一局"),
        "retry" => format!("这一局没过，点了「{tapped}」重来"),
        "dismiss" => format!("关掉了一个弹窗（{tapped}）"),
        _ => return None,
    };
    Some(match rec["outcome"].as_str().unwrap_or("") {
        "dry_run" => format!("画面上有「{tapped}」，会点它（试走，没有真点）"),
        "no_effect" => format!("{what}；可是画面没有变化"),
        "stopped" => format!("{what}；没点成"),
        _ => what,
    })
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
        Stop::DryRun => ("试走结束，没有真点也没有真划。".into(), 0),
        Stop::NoGrid(why) => (format!("停了：读不出棋盘（{why}）。多半是这一关结束了，或者弹出了别的画面。{tail}"), 1),
        Stop::ClassesChanged { was, now } => (
            format!("停了：棋盘上的颜色种类一下子变多了（原来 {was} 种，现在 {now} 种），多半是这一关结束了，或者弹出了窗口。{tail}"),
            1,
        ),
        Stop::Stuck => (format!("停了：划了几次画面都没有变化。请看一眼屏幕上是不是弹出了什么。{tail}"), 1),
        Stop::StillMoving => (format!("停了：等了 8 秒画面还在动。{tail}"), 1),
        Stop::Dco(e) => (format!("停了：{} {tail}", dco_error(e)), 1),
        Stop::Won => (format!("通关了。下一关的棋盘位置不一样，要先重新认棋盘，所以先停在这里。{tail}"), 0),
        Stop::LevelEnded => (
            format!("这一局结束了，我看不出是通关还是没过，为了不乱点先停在这里。要接着玩，请你自己点 Play。{tail}"),
            0,
        ),
        Stop::LivesOut => (format!("生命用完了，先停下。等生命恢复了再让我继续。{tail}"), 0),
        Stop::Money => (format!("出现了要花钱的画面，请你自己处理。这个画面上我没有点任何东西。{tail}"), 1),
        Stop::Ad => (format!("出现了广告，请你自己关掉。这个画面上我没有点任何东西。{tail}"), 1),
        Stop::UnknownScreen(texts) => {
            let shown: Vec<&str> = texts.iter().map(String::as_str).filter(|t| !t.trim().is_empty()).take(8).collect();
            let seen = if shown.is_empty() { "画面上没有读到字。".to_string() } else { format!("画面上的字：{}。", shown.join(" / ")) };
            (format!("出现了我不认识的画面，先停下。这个画面上我没有点任何东西。{seen}{tail}"), 1)
        }
        Stop::NoEffect => (format!("点了按钮，画面却一直没变化，先停下。请看一眼屏幕。{tail}"), 1),
        Stop::TriesDone => (format!("开始和重来的次数到上限了，先停下。要接着来，再运行一次。{tail}"), 0),
        Stop::TapLimit => (format!("点了很多次还没回到棋盘，先停下。请看一眼屏幕上是不是有弹窗在反复出现。{tail}"), 1),
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
    fn a_step_picked_by_the_model_says_so() {
        assert!(step_line(1, &rec(json!({"decider": "model"}))).contains("大模型选的"));
        assert!(!step_line(1, &rec(json!({"decider": "rules"}))).contains("大模型选的"));
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
            (Stop::Won, 0),
            (Stop::LevelEnded, 0),
            (Stop::LivesOut, 0),
            (Stop::TriesDone, 0),
            (Stop::Money, 1),
            (Stop::Ad, 1),
            (Stop::UnknownScreen(vec!["Claim".into()]), 1),
            (Stop::NoEffect, 1),
            (Stop::TapLimit, 1),
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
            Stop::Won,
            Stop::LevelEnded,
            Stop::LivesOut,
            Stop::Money,
            Stop::Ad,
            Stop::UnknownScreen(vec![]),
            Stop::NoEffect,
            Stop::TriesDone,
            Stop::TapLimit,
        ];
        let mut codes: Vec<&str> = all.iter().map(stop_code).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
    }

    #[test]
    fn nav_lines_say_what_was_pressed_and_never_use_jargon() {
        let r = |screen: &str, outcome: &str| json!({"kind": "nav", "screen": screen, "tapped": "Play", "outcome": outcome});
        assert_eq!(nav_line(&r("play", "changed")).unwrap(), "点了「Play」，开始新的一局");
        assert_eq!(nav_line(&r("retry", "changed")).unwrap(), "这一局没过，点了「Play」重来");
        assert_eq!(nav_line(&r("dismiss", "changed")).unwrap(), "关掉了一个弹窗（Play）");
        assert!(nav_line(&r("play", "no_effect")).unwrap().ends_with("可是画面没有变化"));
        assert!(nav_line(&r("play", "dry_run")).unwrap().contains("试走"));
        // 没点任何东西的停下，不在这里说
        assert_eq!(nav_line(&json!({"kind": "nav", "screen": "money", "tapped": null, "outcome": "stopped"})), None);
        for s in ["play", "retry", "dismiss"] {
            let line = nav_line(&r(s, "changed")).unwrap();
            for w in ["snapshot", "element", "tap", "OCR", "daemon"] {
                assert!(!line.to_lowercase().contains(&w.to_lowercase()), "{line}");
            }
        }
    }

    #[test]
    fn an_unknown_screen_shows_what_was_on_it_but_at_most_eight_pieces() {
        let texts: Vec<String> = (1..=12).map(|i| format!("t{i}")).collect();
        let (line, code) = stop_line(&Stop::UnknownScreen(texts), 0, Path::new("/x"));
        assert_eq!(code, 1);
        assert!(line.contains("t1 / t2") && line.contains("t8") && !line.contains("t9"), "{line}");
        assert!(line.contains("没有点任何东西"));
    }

    #[test]
    fn an_unknown_screen_with_no_words_says_nothing_was_read() {
        let (line, _) = stop_line(&Stop::UnknownScreen(vec!["  ".into()]), 0, Path::new("/x"));
        assert!(line.contains("画面上没有读到字。") && !line.contains("画面上的字"), "{line}");
        assert!(line.contains("这个画面上我没有点任何东西"), "{line}");
        assert!(stop_line(&Stop::DryRun, 0, Path::new("/x")).0.contains("没有真点也没有真划"));
        for s in [Stop::Money, Stop::Ad] {
            assert!(stop_line(&s, 3, Path::new("/x")).0.contains("这个画面上我没有点任何东西"));
        }
    }

    #[test]
    fn stop_sentences_for_money_and_ads_promise_nothing_was_pressed() {
        for s in [Stop::Money, Stop::Ad] {
            assert!(stop_line(&s, 3, Path::new("/x")).0.contains("没有点任何东西"));
        }
    }

    #[test]
    fn step_line_says_so_when_stalled() {
        let rec = serde_json::json!({"candidates":[{"a":[0,0],"b":[0,1],"features":{"cleared":3}}],"chosen":0,"outcome":"moved","stalled":true});
        assert!(step_line(1, &rec).contains("这几步看起来没有进展"));
        let rec2 = serde_json::json!({"candidates":[{"a":[0,0],"b":[0,1],"features":{"cleared":3}}],"chosen":0,"outcome":"moved"});
        assert!(!step_line(1, &rec2).contains("没有进展"));
    }
}
