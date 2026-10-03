use crate::board::Board;
use crate::sim::{all_moves, try_move, Move};

pub type Features = crate::sim::Outcome;

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub mv: Move,
    pub score: f64,
    pub features: Features,
}

/// 第一轮拍的权重，以后按记录调。出处：设计第「① 规则选步」节。
pub fn score(f: &Features, rows: usize) -> f64 {
    f.cleared as f64
        + 0.5 * f.cascade as f64
        + 6.0 * f.striped as f64
        + 8.0 * f.wrapped as f64
        + 15.0 * f.bomb as f64
        + 5.0 * f.triggered as f64
        + if f.special_swap { 20.0 } else { 0.0 }
        + (f.lowest_row + 1) as f64 / rows as f64
}

/// 所有合法交换，最好的在最前。「越靠下越好」已经算在分数里（`lowest_row` 一项），所以同分的行一定相同；
/// 同分时再按靠左的列、横向优先、靠上的行排——同一盘每次选得一样。
pub fn choose(b: &Board) -> Vec<Candidate> {
    let mut v: Vec<Candidate> = all_moves(b)
        .into_iter()
        .filter_map(|mv| try_move(b, mv).map(|features| Candidate { mv, score: score(&features, b.rows), features }))
        .collect();
    v.sort_by(|x, y| {
        y.score
            .partial_cmp(&x.score)
            .unwrap()
            .then(x.mv.a.1.cmp(&y.mv.a.1))
            .then(y.mv.horizontal().cmp(&x.mv.horizontal()))
            .then(x.mv.a.0.cmp(&y.mv.a.0))
    });
    v
}
