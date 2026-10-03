use crate::sim::try_move;
use crate::*;

fn mv(a: Pos, b: Pos) -> Move {
    Move { a, b }
}

#[test]
fn horizontal_three_is_legal_and_counts() {
    let b = Board::parse("1 1 2 1\n3 4 5 6\n7 8 9 3");
    let o = try_move(&b, mv((0, 2), (0, 3))).unwrap();
    assert_eq!(o.cleared, 3);
    assert_eq!(o.lowest_row, 0);
}

#[test]
fn swap_without_a_line_is_illegal() {
    let b = Board::parse("1 2 3\n4 5 6\n7 8 9");
    assert!(try_move(&b, mv((0, 0), (0, 1))).is_none());
}

#[test]
fn four_in_a_row_makes_a_striped_candy_and_the_landing_cell_stays() {
    // 交换 (0,2)↔(1,2)，第 0 行变成 1 1 1 1
    let b = Board::parse("1 1 2 1\n3 4 1 5\n7 8 9 3");
    let o = try_move(&b, mv((0, 2), (1, 2))).unwrap();
    assert_eq!((o.cleared, o.striped, o.wrapped, o.bomb), (3, 1, 0, 0));
}

#[test]
fn five_in_a_row_makes_a_bomb() {
    let b = Board::parse("1 1 2 1 1\n3 4 1 5 6");
    let o = try_move(&b, mv((0, 2), (1, 2))).unwrap();
    assert_eq!((o.cleared, o.bomb, o.striped), (4, 1, 0));
}

#[test]
fn l_shape_makes_a_wrapped_candy() {
    // 交换 (2,0)↔(3,0) 后，第 0～2 行的第 0 列是竖线 1 1 1，第 2 行 1 1 1 是横线，在 (2,0) 交叉
    let b = Board::parse("1 5 6\n1 7 8\n2 1 1\n1 3 4");
    let o = try_move(&b, mv((2, 0), (3, 0))).unwrap();
    assert_eq!((o.cleared, o.wrapped, o.striped, o.bomb), (4, 1, 0, 0));
}

#[test]
fn gravity_cascade_counts_the_second_clear() {
    // 消掉第 2 行的 1 以后，上面的两个 2 落下来跟 (2,2) 的 2 以外... 构造：
    //   2 .  .      清掉中间那行后，列 0 的 2 2 落到第 1、2 行，和第 3 行的 2 连成竖线
    let b = Board::parse("2 5 6\n2 7 8\n1 1 3\n2 4 1\n9 8 7");
    // 交换 (2,2)↔(3,2)：第 2 行变成 1 1 1，清掉；上面的 2 2 落下接上 (3,0) 的 2
    let o = try_move(&b, mv((2, 2), (3, 2))).unwrap();
    assert_eq!(o.cleared, 3);
    assert_eq!(o.cascade, 3);
}

#[test]
fn two_specials_swapped_is_legal_even_without_a_line() {
    let b = Board::parse("*1 *2 3\n4 5 6\n7 8 9");
    let o = try_move(&b, mv((0, 0), (0, 1))).unwrap();
    assert!(o.special_swap);
    assert_eq!(o.cleared, 0);
}

#[test]
fn unknown_cells_do_not_take_part() {
    let b = Board::parse("1 1 .\n2 3 1\n4 5 6");
    assert!(try_move(&b, mv((0, 1), (0, 2))).is_none()); // 一格是 Unknown，不能换
    assert!(try_move(&b, mv((0, 2), (1, 2))).is_none());
}

#[test]
fn triggered_counts_specials_inside_the_cleared_line() {
    let b = Board::parse("1 *1 2 1\n3 4 5 6");
    let o = try_move(&b, mv((0, 2), (0, 3))).unwrap();
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

#[test]
fn wrong_shape_is_an_error() {
    let g = GridRead { rows: 2, cols: 2, cells: vec![vec![0, 0]], odd: vec![vec![false; 2]; 2], classes: vec![], observation_id: None, observed_at_ms: None, frame_age_ms: None };
    assert_eq!(Board::from_read(&g), Err(BoardError::Shape));
}

#[test]
fn choose_prefers_bigger_special_and_lower_rows_and_is_deterministic() {
    let b = Board::parse("1 1 2 1\n3 4 1 5\n7 8 9 3\n3 3 4 3");
    let a = choose(&b);
    let c = choose(&b);
    assert_eq!(a, c);
    assert!(a[0].score >= a.last().unwrap().score);
    assert!(a.len() > 1);
}

#[test]
fn no_legal_move_gives_an_empty_list() {
    let b = Board::parse("1 2 3\n4 5 6\n7 8 9");
    assert!(choose(&b).is_empty());
}

#[test]
fn a_chain_reaction_outscores_the_same_clear_without_one() {
    // 左右两个一模一样的三连：左边消完上面的 2 2 落下来跟下面的 2 连成竖线（连锁），右边没有。
    let with_chain = Board::parse("2 5 6\n2 7 8\n1 1 3\n2 4 1\n9 8 7");
    let without = Board::parse("5 5 6\n7 7 8\n1 1 3\n2 4 1\n9 8 7");
    let m = mv((2, 2), (3, 2));
    let f1 = try_move(&with_chain, m).unwrap();
    let f0 = try_move(&without, m).unwrap();
    assert_eq!(f1.cleared, f0.cleared);
    assert!(crate::choose::score(&f1, 5) > crate::choose::score(&f0, 5));
}

#[test]
fn equal_scores_prefer_the_lower_row() {
    // 上下各有一个一模一样的横三连，只差在第几行；同分时低的那个排前面。
    let b = Board::parse("1 1 2 1\n3 4 5 6\n3 4 5 6\n7 8 9 7\n1 1 2 1");
    let c = choose(&b);
    let (hi, lo) = (
        c.iter().position(|x| x.mv == mv((0, 2), (0, 3))).unwrap(),
        c.iter().position(|x| x.mv == mv((4, 2), (4, 3))).unwrap(),
    );
    assert_eq!(c[hi].features.cleared, c[lo].features.cleared);
    assert!(lo < hi, "低的那个应该排前面");
}
