//! 一盘真实的第 1712 关棋盘（2026-10-03 从 iPhone 镜像里读到的，没有特殊糖；读数和屏幕截图逐格核对过）。
//! 钉住的是：合法交换一共有几种，和最好的那一步——以后改打分或模拟，这盘棋的选择不能悄悄变。
use dct_game::{choose, Board, GridRead, Weights};

#[test]
fn the_live_1712_board_has_the_moves_a_person_sees() {
    let g: GridRead = serde_json::from_str(include_str!("fixtures/candy-1712-live.json")).unwrap();
    let b = Board::from_read(&g).unwrap();
    let c = choose(&b, &Weights { striped: 6.0, wrapped: 8.0, bomb: 15.0, triggered: 5.0, ..Weights::default() });
    assert_eq!(c.len(), 17, "合法交换的总数");
    // 第 6 行第 2、3 列对换（0 起：(5,1) 和 (5,2)）：第 6 行右边三个蓝加第 3 列下面三个蓝连成 L 形。
    let best = &c[0];
    assert_eq!((best.mv.a, best.mv.b), ((5, 1), (5, 2)));
    assert_eq!((best.features.cleared, best.features.wrapped, best.features.striped, best.features.bomb), (4, 1, 0, 0));
    // 第二选择：第 6 行第 3 列和第 7 行第 3 列对换，做出条纹糖。
    assert_eq!((c[1].mv.a, c[1].mv.b), ((5, 2), (6, 2)));
    assert_eq!(c[1].features.striped, 1);
    // 每个候选都是真的能连成线的（分数都为正）。
    assert!(c.iter().all(|x| x.score > 0.0));
}
