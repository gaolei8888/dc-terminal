//! `dct game identify`：看一眼屏幕，说这是哪一类游戏。只用本机能得到的东西（有没有合格棋盘、
//! 屏幕上的字），不上传、不划、不点。
use dct_game::genre::{classify, Genre};
use dct_game::looks_like_board;
use dct_game::play::{Dco, Profile};

/// 认出类别；`None` = dco 说这是私人画面：不读棋盘、不发任何表情。
pub fn identify(dco: &mut dyn Dco, p: &Profile) -> Option<Genre> {
    let texts: Vec<String> = match dco.see_text(p) {
        Ok(s) => s.elements.into_iter().map(|e| e.text).collect(),
        Err(e) if e.code == "private_screen" => return None,
        Err(_) => Vec::new(),
    };
    let board_ok = Some(dco.read_grid(p).map(|g| looks_like_board(&g)).unwrap_or(false));
    let g = classify(&texts, board_ok);
    if let Some(m) = dct_game::mood::for_genre(g) {
        dco.show_status_with(m.state, m.text, None);
    }
    Some(g)
}

const PRIVATE_LINE: &str = "这个画面看起来是私人内容，我没有读它，也不会操作。请切回游戏。";

/// `dct game identify [--game 名字]`
pub fn run(args: &[String]) -> i32 {
    let mut game = "candy-crush".to_string();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--game" => match it.next() {
                Some(v) => game = v.clone(),
                None => {
                    eprintln!("--game 后面要写游戏名");
                    return 2;
                }
            },
            other => {
                eprintln!("不认识的选项 {other}。用法：dct game identify [--game 名字]");
                return 2;
            }
        }
    }
    run_identify(&game)
}

#[cfg(not(unix))]
fn run_identify(_: &str) -> i32 {
    eprintln!("这一版只支持 Mac：识别游戏要用 dco，而 dco 目前只有 Mac 版。");
    1
}

#[cfg(unix)]
fn run_identify(game: &str) -> i32 {
    use super::{profile, text};
    use dct_game::dco::DcoClient;
    use dct_game::genre::sentence;

    let Some(home) = crate::sys::home() else {
        eprintln!("找不到家目录。");
        return 1;
    };
    let loaded = match profile::load(&home, game) {
        Ok(l) => l,
        Err(m) => {
            eprintln!("{m}");
            return 1;
        }
    };
    let mut dco = match DcoClient::connect(&home.join(".dco")) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", text::dco_error(&e));
            return 1;
        }
    };
    match identify(&mut dco, &loaded.profile) {
        Some(g) => println!("{}", sentence(g)),
        None => println!("{PRIVATE_LINE}"),
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use dct_game::board::GridRead;
    use dct_game::genre::Genre;
    use dct_game::play::{Dco, DcoError, Profile, Seen};
    use dct_game::screen::Element;
    use serde_json::json;

    struct Fake {
        texts: Option<Vec<&'static str>>,
        grid: Result<GridRead, DcoError>,
        moods: Vec<String>,
    }
    impl Dco for Fake {
        fn read_grid(&mut self, _: &Profile) -> Result<GridRead, DcoError> {
            self.grid.clone()
        }
        fn swipe(&mut self, _: &Profile, _: (f64, f64), _: (f64, f64)) -> Result<(), DcoError> {
            panic!("identify must never swipe");
        }
        fn show_status_with(&mut self, state: &str, _: Option<&str>, _: Option<&str>) {
            self.moods.push(state.into());
        }
        fn see_text(&mut self, _: &Profile) -> Result<Seen, DcoError> {
            match &self.texts {
                Some(t) => Ok(Seen {
                    snapshot_id: "s".into(),
                    observation_id: None,
                    elements: t.iter().enumerate().map(|(i, s)| Element { id: format!("e{i}"), text: (*s).into() }).collect(),
                }),
                None => Err(DcoError { code: "unsupported".into(), message: "x".into() }),
            }
        }
    }

    fn profile() -> Profile {
        Profile { window: json!({"app": "x"}), region: [0.0, 0.0, 1.0, 1.0], rows: 3, cols: 4, extra: json!({}), fixed_rgb: vec![], match_de: 24.0, weights: Default::default(), level_pattern: None, theme: None }
    }
    fn no_grid() -> Result<GridRead, DcoError> {
        Err(DcoError { code: "not_a_grid".into(), message: "m".into() })
    }
    fn good_grid() -> Result<GridRead, DcoError> {
        // 3 行 4 列，三种颜色，每种至少两格，没有一种超过 70%
        Ok(serde_json::from_value(json!({
            "rows": 3, "cols": 4,
            "cells": [[1,1,2,3],[2,3,1,1],[3,2,3,2]],
            "odd": [[false,false,false,false],[false,false,false,false],[false,false,false,false]],
            "classes": [{"id":1,"count":4},{"id":2,"count":4},{"id":3,"count":4}]
        }))
        .unwrap())
    }

    /// 读得出格子，但一种颜色占了 11/12：是弹窗不是棋盘。
    fn popup_grid() -> Result<GridRead, DcoError> {
        Ok(serde_json::from_value(json!({
            "rows": 3, "cols": 4,
            "cells": [[1,1,1,1],[1,1,1,1],[1,1,1,2]],
            "odd": [[false,false,false,false],[false,false,false,false],[false,false,false,false]],
            "classes": [{"id":1,"count":11},{"id":2,"count":1}]
        }))
        .unwrap())
    }

    #[test]
    fn a_private_screen_is_never_read_further() {
        struct Private;
        impl Dco for Private {
            fn read_grid(&mut self, _: &Profile) -> Result<GridRead, DcoError> {
                panic!("a private screen must not be read");
            }
            fn swipe(&mut self, _: &Profile, _: (f64, f64), _: (f64, f64)) -> Result<(), DcoError> {
                panic!("identify must never swipe");
            }
            fn see_text(&mut self, _: &Profile) -> Result<Seen, DcoError> {
                Err(DcoError { code: "private_screen".into(), message: "x".into() })
            }
            fn show_status_with(&mut self, _: &str, _: Option<&str>, _: Option<&str>) {
                panic!("no mood on a private screen");
            }
        }
        assert_eq!(identify(&mut Private, &profile()), None);
    }

    #[test]
    fn identify_sends_a_mood_only_for_a_hidden_object_game() {
        let mut d = Fake { texts: Some(vec!["COLLECTOR'S EDITION", "PLAY"]), grid: no_grid(), moods: vec![] };
        assert_eq!(identify(&mut d, &profile()), Some(Genre::HiddenObject));
        assert_eq!(d.moods, ["confused"]);
        let mut d = Fake { texts: Some(vec!["Hint"]), grid: good_grid(), moods: vec![] };
        assert_eq!(identify(&mut d, &profile()), Some(Genre::Match3));
        assert!(d.moods.is_empty());
    }

    #[test]
    fn a_readable_grid_that_is_not_board_like_is_not_match3() {
        let mut d = Fake { texts: Some(vec!["PLAY", "OPTIONS"]), grid: popup_grid(), moods: vec![] };
        assert_eq!(identify(&mut d, &profile()), Some(Genre::Unknown));
    }

    #[test]
    fn a_strong_cue_with_a_non_board_like_grid_is_hidden_object() {
        let mut d = Fake { texts: Some(vec!["Inventory"]), grid: popup_grid(), moods: vec![] };
        assert_eq!(identify(&mut d, &profile()), Some(Genre::HiddenObject));
    }

    #[test]
    fn a_readable_board_is_match3() {
        let mut d = Fake { texts: Some(vec!["Hint"]), grid: good_grid(), moods: vec![] };
        assert_eq!(identify(&mut d, &profile()), Some(Genre::Match3));
    }

    #[test]
    fn a_strong_cue_and_no_board_is_hidden_object() {
        let mut d = Fake { texts: Some(vec!["COLLECTOR'S EDITION", "PLAY"]), grid: no_grid(), moods: vec![] };
        assert_eq!(identify(&mut d, &profile()), Some(Genre::HiddenObject));
    }

    #[test]
    fn text_unsupported_falls_back_to_the_board_only() {
        let mut d = Fake { texts: None, grid: good_grid(), moods: vec![] };
        assert_eq!(identify(&mut d, &profile()), Some(Genre::Match3));
        let mut d = Fake { texts: None, grid: no_grid(), moods: vec![] };
        assert_eq!(identify(&mut d, &profile()), Some(Genre::Unknown));
    }

    #[test]
    fn nothing_readable_is_unknown() {
        let mut d = Fake { texts: Some(vec![]), grid: no_grid(), moods: vec![] };
        assert_eq!(identify(&mut d, &profile()), Some(Genre::Unknown));
    }
}
