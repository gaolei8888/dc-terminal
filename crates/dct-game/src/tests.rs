use crate::sim::try_move;
use crate::*;

fn mv(a: Pos, b: Pos) -> Move {
    Move { a, b }
}

#[test]
fn horizontal_three_is_legal_and_counts() {
    let b = Board::parse("1 1 2 1\n3 4 5 6\n7 8 9 3");
    let o = try_move(&b, mv((0, 2), (0, 3)), false).unwrap();
    assert_eq!(o.cleared, 3);
    assert_eq!(o.lowest_row, 0);
}

#[test]
fn swap_without_a_line_is_illegal() {
    let b = Board::parse("1 2 3\n4 5 6\n7 8 9");
    assert!(try_move(&b, mv((0, 0), (0, 1)), false).is_none());
}

#[test]
fn four_in_a_row_makes_a_striped_candy_and_the_landing_cell_stays() {
    // 交换 (0,2)↔(1,2)，第 0 行变成 1 1 1 1
    let b = Board::parse("1 1 2 1\n3 4 1 5\n7 8 9 3");
    let o = try_move(&b, mv((0, 2), (1, 2)), false).unwrap();
    assert_eq!((o.cleared, o.striped, o.wrapped, o.bomb), (3, 1, 0, 0));
}

#[test]
fn five_in_a_row_makes_a_bomb() {
    let b = Board::parse("1 1 2 1 1\n3 4 1 5 6");
    let o = try_move(&b, mv((0, 2), (1, 2)), false).unwrap();
    assert_eq!((o.cleared, o.bomb, o.striped), (4, 1, 0));
}

#[test]
fn l_shape_makes_a_wrapped_candy() {
    // 交换 (2,0)↔(3,0) 后，第 0～2 行的第 0 列是竖线 1 1 1，第 2 行 1 1 1 是横线，在 (2,0) 交叉
    let b = Board::parse("1 5 6\n1 7 8\n2 1 1\n1 3 4");
    let o = try_move(&b, mv((2, 0), (3, 0)), false).unwrap();
    assert_eq!((o.cleared, o.wrapped, o.striped, o.bomb), (4, 1, 0, 0));
}

#[test]
fn gravity_cascade_counts_the_second_clear() {
    // 消掉第 2 行的 1 以后，上面的两个 2 落下来跟 (2,2) 的 2 以外... 构造：
    //   2 .  .      清掉中间那行后，列 0 的 2 2 落到第 1、2 行，和第 3 行的 2 连成竖线
    let b = Board::parse("2 5 6\n2 7 8\n1 1 3\n2 4 1\n9 8 7");
    // 交换 (2,2)↔(3,2)：第 2 行变成 1 1 1，清掉；上面的 2 2 落下接上 (3,0) 的 2
    let o = try_move(&b, mv((2, 2), (3, 2)), false).unwrap();
    assert_eq!(o.cleared, 3);
    assert_eq!(o.cascade, 3);
}

#[test]
fn two_specials_swapped_is_legal_even_without_a_line() {
    let b = Board::parse("*1 *2 3\n4 5 6\n7 8 9");
    let o = try_move(&b, mv((0, 0), (0, 1)), true).unwrap();
    assert!(o.special_swap);
    assert_eq!(o.cleared, 0);
}

#[test]
fn two_specials_swapped_without_a_line_is_illegal_unless_allowed() {
    let b = Board::parse("*1 *2 3\n4 5 6\n7 8 9");
    assert!(try_move(&b, mv((0, 0), (0, 1)), false).is_none());
    assert!(try_move(&b, mv((0, 0), (0, 1)), true).is_some());
}

#[test]
fn default_weights_never_choose_a_zero_clear_special_swap() {
    let b = Board::parse("*1 *2 3\n4 5 6\n7 8 9");
    assert!(choose(&b, &Weights::default()).is_empty());
    let w = Weights { special_swap: 20.0, ..Weights::default() };
    let c = choose(&b, &w);
    assert_eq!(c.len(), 1);
    assert!(c[0].features.special_swap && c[0].features.cleared == 0);
    assert!(c[0].score >= 20.0);
}

#[test]
fn special_candy_weights_are_data_not_constants() {
    // 同样的特征：做出一颗条纹糖，中性权重下不加分，配置给了权重才加分。
    let f = crate::sim::Outcome { cleared: 4, striped: 1, lowest_row: 0, ..Default::default() };
    let neutral = crate::choose::score(&f, 3, &Weights::default());
    let tuned = crate::choose::score(&f, 3, &Weights { striped: 6.0, ..Weights::default() });
    assert!((tuned - neutral - 6.0).abs() < 1e-9);
}

#[test]
fn unknown_cells_do_not_take_part() {
    let b = Board::parse("1 1 .\n2 3 1\n4 5 6");
    assert!(try_move(&b, mv((0, 1), (0, 2)), false).is_none()); // 一格是 Unknown，不能换
    assert!(try_move(&b, mv((0, 2), (1, 2)), false).is_none());
}

#[test]
fn triggered_counts_specials_inside_the_cleared_line() {
    let b = Board::parse("1 *1 2 1\n3 4 5 6");
    let o = try_move(&b, mv((0, 2), (0, 3)), false).unwrap();
    assert_eq!((o.cleared, o.triggered), (3, 1));
}

#[test]
fn single_cell_classes_become_unknown() {
    let g: GridRead = serde_json::from_str(
        r#"{"rows":2,"cols":2,"cells":[[0,0],[1,2]],"odd":[[false,false],[false,false]],
            "classes":[{"id":0,"count":2},{"id":1,"count":1},{"id":2,"count":1}],"elapsed_ms":1}"#,
    )
    .unwrap();
    let b = Board::from_read(&g).unwrap();
    assert_eq!(b.get(1, 0), Cell::Unknown);
    assert_eq!(b.get(0, 0), Cell::Candy { class: 0, special: false });
}

/// 配置列了「不是糖」的颜色之后，没列的都是糖，一格的类别也是（真机 1714：全盘只有一个红、一个橙）。
#[test]
fn singles_are_candies_when_the_profile_lists_the_not_candies() {
    let g: GridRead = serde_json::from_str(
        r#"{"rows":2,"cols":2,"cells":[[0,0],[1,2]],"odd":[[false,false],[false,false]],
            "classes":[{"id":0,"count":2},{"id":1,"count":1},{"id":2,"count":1}]}"#,
    )
    .unwrap();
    let b = Board::from_read_fixed(&g, &[2], true).unwrap();
    assert_eq!(b.get(1, 0), Cell::Candy { class: 1, special: false });
    // 列出来的一格类别仍然是 fixed
    assert_eq!(b.get(1, 1), Cell::Fixed);
    // 没列就保持旧规则
    assert_eq!(Board::from_read_fixed(&g, &[2], false).unwrap().get(1, 0), Cell::Unknown);
}

#[test]
fn wrong_shape_is_an_error() {
    let g = GridRead { rows: 2, cols: 2, cells: vec![vec![0, 0]], odd: vec![vec![false; 2]; 2], classes: vec![], observation_id: None, observed_at_ms: None, frame_age_ms: None };
    assert_eq!(Board::from_read(&g), Err(BoardError::Shape));
}

#[test]
fn choose_prefers_bigger_special_and_lower_rows_and_is_deterministic() {
    let b = Board::parse("1 1 2 1\n3 4 1 5\n7 8 9 3\n3 3 4 3");
    let a = choose(&b, &Weights::default());
    let c = choose(&b, &Weights::default());
    assert_eq!(a, c);
    assert!(a[0].score >= a.last().unwrap().score);
    assert!(a.len() > 1);
}

#[test]
fn no_legal_move_gives_an_empty_list() {
    let b = Board::parse("1 2 3\n4 5 6\n7 8 9");
    assert!(choose(&b, &Weights::default()).is_empty());
}

#[test]
fn a_chain_reaction_outscores_the_same_clear_without_one() {
    // 左右两个一模一样的三连：左边消完上面的 2 2 落下来跟下面的 2 连成竖线（连锁），右边没有。
    let with_chain = Board::parse("2 5 6\n2 7 8\n1 1 3\n2 4 1\n9 8 7");
    let without = Board::parse("5 5 6\n7 7 8\n1 1 3\n2 4 1\n9 8 7");
    let m = mv((2, 2), (3, 2));
    let f1 = try_move(&with_chain, m, false).unwrap();
    let f0 = try_move(&without, m, false).unwrap();
    assert_eq!(f1.cleared, f0.cleared);
    assert!(crate::choose::score(&f1, 5, &Weights::default()) > crate::choose::score(&f0, 5, &Weights::default()));
}

#[test]
fn equal_scores_prefer_the_lower_row() {
    // 上下各有一个一模一样的横三连，只差在第几行；同分时低的那个排前面。
    let b = Board::parse("1 1 2 1\n3 4 5 6\n3 4 5 6\n7 8 9 7\n1 1 2 1");
    let c = choose(&b, &Weights::default());
    let (hi, lo) = (
        c.iter().position(|x| x.mv == mv((0, 2), (0, 3))).unwrap(),
        c.iter().position(|x| x.mv == mv((4, 2), (4, 3))).unwrap(),
    );
    assert_eq!(c[hi].features.cleared, c[lo].features.cleared);
    assert!(lo < hi, "低的那个应该排前面");
}

fn read_with_counts(rows: usize, cols: usize, counts: &[usize]) -> GridRead {
    GridRead {
        rows,
        cols,
        cells: vec![vec![0; cols]; rows],
        odd: vec![vec![false; cols]; rows],
        classes: counts.iter().enumerate().map(|(i, &c)| board::ClassInfo { id: i as u16, count: c, rgb: None }).collect(),
        observation_id: None,
        observed_at_ms: None,
        frame_age_ms: None,
    }
}

#[test]
fn a_mostly_flat_popup_read_is_not_a_board() {
    // 2026-10-03 真机：Daily Stamps 卡片读成 9x5「棋盘」，类别 5/5/35（78%）。
    assert!(!looks_like_board(&read_with_counts(9, 5, &[5, 5, 35])));
}

#[test]
fn the_real_1712_board_looks_like_a_board() {
    let g: GridRead = serde_json::from_str(include_str!("../tests/fixtures/candy-1712-live.json")).unwrap();
    assert!(looks_like_board(&g));
}

#[test]
fn seventy_percent_is_the_line() {
    // 20 格：14 格 = 正好 70%，可以；15 格 = 75%，不行。
    assert!(looks_like_board(&read_with_counts(4, 5, &[14, 3, 3])));
    assert!(!looks_like_board(&read_with_counts(4, 5, &[15, 3, 2])));
}

#[test]
fn fewer_than_three_big_classes_is_not_a_board() {
    assert!(!looks_like_board(&read_with_counts(3, 4, &[6, 4, 1, 1])));
}

// ---- 「不是糖」的格子（Cell::Fixed）----

#[test]
fn a_fixed_cell_is_never_swapped() {
    // (0,2) 是 #：左边两个 1、右边一个 1 隔着它，换不出线；挨着它的交换也不合法
    let b = Board::parse("1 1 # 1\n2 3 4 5\n3 2 5 4");
    assert!(try_move(&b, mv((0, 1), (0, 2)), false).is_none());
    assert!(try_move(&b, mv((0, 2), (0, 3)), false).is_none());
    assert!(try_move(&b, mv((0, 2), (1, 2)), false).is_none());
    assert!(choose(&b, &Weights::default()).iter().all(|c| c.mv.a != (0, 2) && c.mv.b != (0, 2)));
}

#[test]
fn a_fixed_cell_cuts_a_line_in_two() {
    // 不是糖的格子把一排切开：左边只有 2 个、右边只有 2 个，都连不成 3
    let b = Board::parse("1 1 # 1 1\n3 4 5 6 7\n7 6 5 4 3");
    assert_eq!(crate::sim::all_moves(&b).iter().filter(|m| try_move(&b, **m, false).is_some()).count(), 0);
    // 而同样的一排没有 # 时，换一下就是 5 连
    let b = Board::parse("1 1 2 1 1\n3 4 1 6 7\n7 6 5 4 3");
    assert!(try_move(&b, mv((0, 2), (1, 2)), false).is_some());
}

#[test]
fn gravity_drops_candies_to_the_bottom_when_nothing_blocks_them() {
    // 第 0 列：1 1 _ _ → _ _ 1 1
    let mut b = Board::parse("1 5\n1 5\n_ 5\n_ 5");
    crate::sim::gravity(&mut b);
    assert_eq!(b, Board::parse("_ 5\n_ 5\n1 5\n1 5"));
}

#[test]
fn a_fixed_cell_stops_candies_from_falling_through_it() {
    // 第 0 列：1 1 # _ _ → 1 1 # _ _（# 上面的糖落不下去，# 下面的空位也没有糖补）
    let mut b = Board::parse("1 5\n1 5\n# 5\n_ 5\n_ 5");
    crate::sim::gravity(&mut b);
    assert_eq!(b, Board::parse("1 5\n1 5\n# 5\n_ 5\n_ 5"));
}

#[test]
fn candies_fall_only_inside_their_own_segment_between_fixed_cells() {
    // 第 0 列：1 _ # 2 _ 3? → 上一段 [1 _] 里 1 落到 # 上面；下一段 [2 _] 里 2 落到底
    let mut b = Board::parse("1 5\n_ 5\n# 5\n2 5\n_ 5");
    crate::sim::gravity(&mut b);
    assert_eq!(b, Board::parse("_ 5\n1 5\n# 5\n_ 5\n2 5"));
}

#[test]
fn fixed_cells_do_not_move_and_unknown_cells_do_fall() {
    let mut b = Board::parse("# 5\n. 5\n_ 5");
    crate::sim::gravity(&mut b);
    assert_eq!(b, Board::parse("# 5\n_ 5\n. 5"));
}

#[test]
fn from_read_fixed_marks_the_listed_classes_even_single_ones() {
    let g: GridRead = serde_json::from_str(
        r#"{"rows":2,"cols":3,"cells":[[0,0,1],[2,2,1]],"odd":[[false,false,false],[false,false,false]],
            "classes":[{"id":0,"count":2},{"id":1,"count":2},{"id":2,"count":2}]}"#,
    )
    .unwrap();
    let b = Board::from_read_fixed(&g, &[1], false).unwrap();
    assert_eq!((b.get(0, 2), b.get(1, 2)), (Cell::Fixed, Cell::Fixed));
    assert_eq!(b.get(0, 0), Cell::Candy { class: 0, special: false });
    // 不指定就是第一轮的行为
    assert_eq!(Board::from_read(&g).unwrap().get(0, 2), Cell::Candy { class: 1, special: false });
}

#[test]
fn fixed_classes_are_found_by_colour_not_by_id() {
    let read = |a: u16, b: u16| -> GridRead {
        serde_json::from_value(serde_json::json!({
            "rows": 1, "cols": 2, "cells": [[a, b]], "odd": [[false, false]],
            "classes": [
                {"id": a, "count": 5, "rgb": [196, 148, 101]},
                {"id": b, "count": 5, "rgb": [66, 102, 252]}
            ]
        }))
        .unwrap()
    };
    let honey = [[196u8, 148, 101]];
    // 同一张画面，类别号对调：蜂蜜块的类别号变了，找出来的还是它
    assert_eq!(crate::fixed_ids(&read(0, 1), &honey, 24.0), vec![0]);
    assert_eq!(crate::fixed_ids(&read(1, 0), &honey, 24.0), vec![1]);
    // 颜色有小偏差（RGB 差几个点）也认得出；差得远的不算
    let g = read(0, 1);
    assert_eq!(crate::fixed_ids(&g, &[[199, 150, 104]], 24.0), vec![0]);
    assert!(crate::fixed_ids(&g, &[[10, 10, 10]], 24.0).is_empty());
    assert!(crate::fixed_ids(&g, &[], 24.0).is_empty());
}

/// 2026-10-03 真机第 1713 关的读数（对着截图逐格核对过）：D 类（蜂蜜块和顶部缺口，41 格）和 E 类（糖果机，6 格）不是糖。
#[test]
fn the_real_1713_board_never_moves_a_honey_block_a_gap_or_a_gumball_machine() {
    let g: GridRead = serde_json::from_str(include_str!("../tests/fixtures/candy-1713-live.json")).unwrap();
    let fixed = crate::fixed_ids(&g, &[[196, 148, 101], [161, 180, 233]], 24.0);
    assert_eq!(fixed.len(), 2, "蜂蜜块和糖果机各是一个类别");
    let b = Board::from_read_fixed(&g, &fixed, false).unwrap();
    let c = choose(&b, &Weights::default());
    assert!(!c.is_empty());
    for x in &c {
        for p in [x.mv.a, x.mv.b] {
            assert!(!fixed.contains(&g.cells[p.0][p.1]), "选步碰到了不是糖的格子：{:?}", x.mv);
        }
    }
    // 不指定 fixed（第一轮的读法）会把蜂蜜块当成糖，多出一批假的合法交换
    let wrong = choose(&Board::from_read(&g).unwrap(), &Weights::default());
    assert!(wrong.len() > c.len(), "第一轮的读法 {} 步，认出不是糖以后 {} 步", wrong.len(), c.len());
}
