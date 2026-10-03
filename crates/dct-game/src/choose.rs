use crate::board::Board;
use crate::sim::{all_moves, try_move, Move};

pub type Features = crate::sim::Outcome;

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub mv: Move,
    pub score: f64,
    pub features: Features,
}

/// 打分的权重。代码里不写死任何游戏的数值（2026-10-03 用户定的：dct 保持通用，游戏相关的内容是数据）：
/// 默认值是中性的——只认「消得多、连锁多、靠下」，所有特殊糖的权重都是 0；
/// 具体游戏的数值来自配置文件的 `[weights]`（过渡期的本地数据，长期归 dcv）。
#[derive(Clone, Debug, PartialEq)]
pub struct Weights {
    pub cleared: f64,
    pub cascade: f64,
    pub striped: f64,
    pub wrapped: f64,
    pub bomb: f64,
    pub triggered: f64,
    pub special_swap: f64,
    pub low_row: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Weights { cleared: 1.0, cascade: 0.5, striped: 0.0, wrapped: 0.0, bomb: 0.0, triggered: 0.0, special_swap: 0.0, low_row: 1.0 }
    }
}

pub fn score(f: &Features, rows: usize, w: &Weights) -> f64 {
    f.cleared as f64 * w.cleared
        + f.cascade as f64 * w.cascade
        + f.striped as f64 * w.striped
        + f.wrapped as f64 * w.wrapped
        + f.bomb as f64 * w.bomb
        + f.triggered as f64 * w.triggered
        + if f.special_swap { w.special_swap } else { 0.0 }
        + (f.lowest_row + 1) as f64 / rows as f64 * w.low_row
}

/// 所有合法交换，最好的在最前。「越靠下越好」已经算在分数里（`lowest_row` 一项），所以同分的行一定相同；
/// 同分时再按靠左的列、横向优先、靠上的行排——同一盘每次选得一样。
///
/// 两颗特殊糖互换却连不成线，只有权重里 `special_swap > 0`（调用方明说要这种步）才算合法：
/// 特殊糖的标记会跟着提示的光晕和掉落动画闪，不能拿它当「这一步有用」的依据。
pub fn choose(b: &Board, w: &Weights) -> Vec<Candidate> {
    let allow_special_swap = w.special_swap > 0.0;
    let mut v: Vec<Candidate> = all_moves(b)
        .into_iter()
        .filter_map(|mv| try_move(b, mv, allow_special_swap).map(|features| Candidate { mv, score: score(&features, b.rows, w), features }))
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
