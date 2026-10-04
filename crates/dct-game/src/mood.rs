//! 什么时候让小章鱼露出什么表情：纯函数，只看一步的特征、停下的原因、游戏类别，不碰 dco。
//! 下面的阈值都是写死的常量；以后会搬进 dcv 的规矩里，由记忆来调。
use crate::choose::Features;
use crate::genre::Genre;
use crate::play::Stop;

/// 一个表情：dco 的状态名，加一条不超过 16 个字符的说明（可以没有）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mood {
    pub state: &'static str,
    pub text: Option<&'static str>,
}

const fn mood(state: &'static str) -> Option<Mood> {
    Some(Mood { state, text: None })
}

const fn mood_text(state: &'static str, text: &'static str) -> Option<Mood> {
    Some(Mood { state, text: Some(text) })
}

// 以下阈值以后会进 dcv 的规矩。
/// 预计一步消掉（含连锁）至少这么多颗，算大招。
pub const BIG_CLEAR: usize = 20;
/// 剩余步数 <= 这个数就紧张。
pub const TENSE_STEPS_LEFT: u64 = 5;
/// 目标数字剩余 <= 开局值的这个百分比，就期待。
pub const HOPEFUL_FRACTION_PERCENT: u64 = 20;
/// 目标数字剩余 <= 这个数（且 > 0）也期待。
pub const HOPEFUL_ABS: u64 = 3;

/// 这一步该露什么表情；都不合适就 `None`（调用方自己回「看」）。
/// 优先级：紧张 > 庆祝 > 期待 > 卡住。
pub fn for_step(f: &Features, stalled: bool, steps_left: Option<u64>, goal_now: Option<u64>, goal_start: Option<u64>) -> Option<Mood> {
    if steps_left.is_some_and(|n| n <= TENSE_STEPS_LEFT) {
        return mood("tense");
    }
    // 条纹、包装糖不算大招，只算彩球和大消。
    if f.cleared + f.cascade >= BIG_CLEAR || f.bomb > 0 {
        return mood("celebrate");
    }
    if let Some(now) = goal_now {
        let near_end = now > 0 && now <= HOPEFUL_ABS;
        let near_fraction = now > 0 && goal_start.is_some_and(|s| now * 100 <= s * HOPEFUL_FRACTION_PERCENT);
        if near_end || near_fraction {
            return mood("hopeful");
        }
    }
    if stalled {
        return mood("stall");
    }
    None
}

/// 停下来的时候露什么表情；没有特别的就 `None`。
pub fn for_stop(stop: &Stop) -> Option<Mood> {
    match stop {
        Stop::Won => mood("won"),
        Stop::LivesOut => mood("sad"),
        Stop::Money | Stop::Ad => mood_text("wait", "要你处理"),
        Stop::UnknownScreen(_) | Stop::LevelEnded | Stop::NoGrid(_) => mood_text("wait", "看不懂这个画面"),
        Stop::Stuck => mood("stuck"),
        _ => None,
    }
}

/// 认出游戏类别后的表情。
pub fn for_genre(g: Genre) -> Option<Mood> {
    match g {
        Genre::HiddenObject => mood_text("confused", "不会玩这类"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::DcoError;

    fn feat(cleared: usize, cascade: usize) -> Features {
        Features { cleared, cascade, ..Default::default() }
    }
    fn state(m: Option<Mood>) -> Option<&'static str> {
        m.map(|m| m.state)
    }

    #[test]
    fn tense_fires_at_the_threshold_and_not_above_it() {
        assert_eq!(state(for_step(&feat(3, 0), false, Some(5), None, None)), Some("tense"));
        assert_eq!(state(for_step(&feat(3, 0), false, Some(6), None, None)), None);
    }

    #[test]
    fn tense_beats_celebrate() {
        assert_eq!(state(for_step(&feat(30, 0), false, Some(2), None, None)), Some("tense"));
    }

    #[test]
    fn celebrate_needs_a_big_clear_or_a_bomb_but_not_just_striped() {
        assert_eq!(state(for_step(&feat(15, 5), false, None, None, None)), Some("celebrate"));
        assert_eq!(state(for_step(&feat(15, 4), false, None, None, None)), None);
        let bomb = Features { bomb: 1, ..Default::default() };
        assert_eq!(state(for_step(&bomb, false, None, None, None)), Some("celebrate"));
        let striped = Features { striped: 2, wrapped: 1, ..Default::default() };
        assert_eq!(state(for_step(&striped, false, None, None, None)), None);
    }

    #[test]
    fn hopeful_when_the_goal_is_a_fifth_of_the_start_or_nearly_zero() {
        let f = feat(3, 0);
        assert_eq!(state(for_step(&f, false, None, Some(20), Some(100))), Some("hopeful"));
        assert_eq!(state(for_step(&f, false, None, Some(21), Some(100))), None);
        assert_eq!(state(for_step(&f, false, None, Some(3), Some(1000))), Some("hopeful"));
        assert_eq!(state(for_step(&f, false, None, Some(3), None)), Some("hopeful"));
        assert_eq!(state(for_step(&f, false, None, Some(4), None)), None);
        assert_eq!(state(for_step(&f, false, None, Some(0), Some(100))), None, "0 是已经完成，不是期待");
        assert_eq!(state(for_step(&f, false, None, Some(0), None)), None);
    }

    #[test]
    fn stall_shows_only_when_nothing_better_applies() {
        let f = feat(3, 0);
        assert_eq!(state(for_step(&f, true, None, None, None)), Some("stall"));
        assert_eq!(state(for_step(&f, true, Some(4), None, None)), Some("tense"));
        assert_eq!(state(for_step(&f, false, None, None, None)), None);
    }

    fn every_stop() -> Vec<Stop> {
        vec![
            Stop::NoGrid("x".into()),
            Stop::ClassesChanged { was: 5, now: 8 },
            Stop::NoMoves,
            Stop::Stuck,
            Stop::StillMoving,
            Stop::StepsDone,
            Stop::DryRun,
            Stop::Dco(DcoError { code: "dco_down".into(), message: "x".into() }),
            Stop::Won,
            Stop::LevelEnded,
            Stop::LivesOut,
            Stop::Money,
            Stop::Ad,
            Stop::UnknownScreen(vec![]),
            Stop::NoEffect,
            Stop::TriesDone,
            Stop::TapLimit,
        ]
    }

    #[test]
    fn each_stop_maps_to_its_mood() {
        let got = |s: Stop| for_stop(&s).map(|m| (m.state, m.text));
        assert_eq!(got(Stop::Won), Some(("won", None)));
        assert_eq!(got(Stop::LivesOut), Some(("sad", None)));
        assert_eq!(got(Stop::Money), Some(("wait", Some("要你处理"))));
        assert_eq!(got(Stop::Ad), Some(("wait", Some("要你处理"))));
        for s in [Stop::UnknownScreen(vec![]), Stop::LevelEnded, Stop::NoGrid("x".into())] {
            assert_eq!(got(s), Some(("wait", Some("看不懂这个画面"))));
        }
        assert_eq!(got(Stop::Stuck), Some(("stuck", None)));
        for s in [Stop::StepsDone, Stop::DryRun, Stop::NoMoves, Stop::StillMoving, Stop::NoEffect, Stop::TriesDone, Stop::TapLimit, Stop::ClassesChanged { was: 1, now: 4 }, Stop::Dco(DcoError { code: "c".into(), message: "m".into() })] {
            assert_eq!(for_stop(&s), None, "{s:?}");
        }
    }

    #[test]
    fn every_text_fits_the_16_character_limit() {
        let mut texts: Vec<&str> = vec![];
        for s in every_stop() {
            texts.extend(for_stop(&s).and_then(|m| m.text));
        }
        for g in [Genre::Match3, Genre::HiddenObject, Genre::Unknown] {
            texts.extend(for_genre(g).and_then(|m| m.text));
        }
        assert!(!texts.is_empty());
        for t in texts {
            assert!(t.chars().count() <= 16, "{t}");
        }
    }

    #[test]
    fn only_hidden_object_is_confused() {
        assert_eq!(for_genre(Genre::HiddenObject).map(|m| m.state), Some("confused"));
        assert_eq!(for_genre(Genre::Match3), None);
        assert_eq!(for_genre(Genre::Unknown), None);
    }
}
