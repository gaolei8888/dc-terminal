use crate::board::{Board, Cell};

pub type Pos = (usize, usize);

/// 交换相邻两格。`a` 总是在 `b` 的左边或上边。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Move {
    pub a: Pos,
    pub b: Pos,
}

impl Move {
    pub fn horizontal(&self) -> bool {
        self.a.0 == self.b.0
    }
}

/// 一次交换能带来什么。
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Outcome {
    /// 第一轮消了几颗（不含新做出来的特殊糖）。
    pub cleared: usize,
    /// 后面连锁下落又消了几颗。
    pub cascade: usize,
    pub striped: usize,
    pub wrapped: usize,
    pub bomb: usize,
    /// 被消掉的格子里有几颗原本就是特殊糖（不知道是哪种，统一算一种）。
    pub triggered: usize,
    pub special_swap: bool,
    /// 第一轮消掉的格子里最靠下的那一行（0 起）。
    pub lowest_row: usize,
}

struct Run {
    cells: Vec<Pos>,
    horizontal: bool,
}

fn candy_class(b: &Board, r: usize, c: usize) -> Option<u16> {
    match b.get(r, c) {
        Cell::Candy { class, .. } => Some(class),
        _ => None,
    }
}

/// 所有横向、纵向、长度 ≥3 的同类连续段（取最长的，不拆开）。
fn find_runs(b: &Board) -> Vec<Run> {
    let mut runs = Vec::new();
    for r in 0..b.rows {
        let mut c = 0;
        while c < b.cols {
            let Some(k) = candy_class(b, r, c) else {
                c += 1;
                continue;
            };
            let mut e = c + 1;
            while e < b.cols && candy_class(b, r, e) == Some(k) {
                e += 1;
            }
            if e - c >= 3 {
                runs.push(Run { cells: (c..e).map(|x| (r, x)).collect(), horizontal: true });
            }
            c = e;
        }
    }
    for c in 0..b.cols {
        let mut r = 0;
        while r < b.rows {
            let Some(k) = candy_class(b, r, c) else {
                r += 1;
                continue;
            };
            let mut e = r + 1;
            while e < b.rows && candy_class(b, e, c) == Some(k) {
                e += 1;
            }
            if e - r >= 3 {
                runs.push(Run { cells: (r..e).map(|x| (x, c)).collect(), horizontal: false });
            }
            r = e;
        }
    }
    runs
}

fn is_special(b: &Board, p: Pos) -> bool {
    matches!(b.get(p.0, p.1), Cell::Candy { special: true, .. })
}

fn swapped(b: &Board, m: Move) -> Board {
    let mut n = b.clone();
    let (x, y) = (b.get(m.a.0, m.a.1), b.get(m.b.0, m.b.1));
    n.set(m.a.0, m.a.1, y);
    n.set(m.b.0, m.b.1, x);
    n
}

/// 只有两格都是糖才能换；换完要么连成线，要么两颗都是特殊糖。不合法返回 `None`。
pub fn try_move(b: &Board, m: Move) -> Option<Outcome> {
    let both_candy = matches!(b.get(m.a.0, m.a.1), Cell::Candy { .. }) && matches!(b.get(m.b.0, m.b.1), Cell::Candy { .. });
    if !both_candy {
        return None;
    }
    let special_swap = is_special(b, m.a) && is_special(b, m.b);
    let mut board = swapped(b, m);
    let first = find_runs(&board);
    if first.is_empty() {
        return special_swap.then_some(Outcome { special_swap: true, lowest_row: m.a.0.max(m.b.0), ..Outcome::default() });
    }
    let mut out = Outcome { special_swap, ..Outcome::default() };
    let mut runs = first;
    for pass in 0..20 {
        if runs.is_empty() {
            break;
        }
        let mut gone: Vec<Pos> = Vec::new();
        for run in &runs {
            for &p in &run.cells {
                if !gone.contains(&p) {
                    gone.push(p);
                }
            }
        }
        // 新做出来的特殊糖只在第一轮，留在交换落点（落点不在线上就取线的第一格）。
        let mut made: Vec<Pos> = Vec::new();
        if pass == 0 {
            let mut used = vec![false; runs.len()];
            for (i, h) in runs.iter().enumerate().filter(|(_, r)| r.horizontal) {
                for (j, v) in runs.iter().enumerate().filter(|(_, r)| !r.horizontal) {
                    if used[i] || used[j] {
                        continue;
                    }
                    if let Some(&x) = h.cells.iter().find(|p| v.cells.contains(p)) {
                        used[i] = true;
                        used[j] = true;
                        out.wrapped += 1;
                        made.push(x);
                    }
                }
            }
            for (i, run) in runs.iter().enumerate() {
                if used[i] || run.cells.len() < 4 {
                    continue;
                }
                let at = [m.a, m.b].into_iter().find(|p| run.cells.contains(p)).unwrap_or(run.cells[0]);
                if run.cells.len() >= 5 {
                    out.bomb += 1;
                } else {
                    out.striped += 1;
                }
                made.push(at);
            }
        }
        let gone: Vec<Pos> = gone.into_iter().filter(|p| !made.contains(p)).collect();
        out.triggered += gone.iter().filter(|&&p| is_special(&board, p)).count();
        if pass == 0 {
            out.cleared = gone.len();
            out.lowest_row = gone.iter().map(|p| p.0).max().unwrap_or(0);
        } else {
            out.cascade += gone.len();
        }
        for &p in &made {
            if let Cell::Candy { class, .. } = board.get(p.0, p.1) {
                board.set(p.0, p.1, Cell::Candy { class, special: true });
            }
        }
        for &(r, c) in &gone {
            board.set(r, c, Cell::Empty);
        }
        gravity(&mut board);
        runs = find_runs(&board);
    }
    Some(out)
}

/// 每一列里没消掉的格子落到底，上面补进来的是空（补进来的新糖是什么，谁也不知道）。
/// 「不是糖」的格子（缺口、蜂蜜块）不动，也挡住下落：糖只在同一列里两个这样的格子之间的一段内往下掉。
pub(crate) fn gravity(b: &mut Board) {
    for c in 0..b.cols {
        let mut bottom = b.rows; // 当前这一段的下边界（不含）
        while bottom > 0 {
            if b.get(bottom - 1, c) == Cell::Fixed {
                bottom -= 1;
                continue;
            }
            let mut top = bottom; // 这一段是 [top, bottom)
            while top > 0 && b.get(top - 1, c) != Cell::Fixed {
                top -= 1;
            }
            let kept: Vec<Cell> = (top..bottom).rev().map(|r| b.get(r, c)).filter(|x| *x != Cell::Empty).collect();
            for (i, r) in (top..bottom).rev().enumerate() {
                b.set(r, c, kept.get(i).copied().unwrap_or(Cell::Empty));
            }
            bottom = top;
        }
    }
}

/// 全部相邻对（右边、下边），先横后竖，按行、列从小到大。不管合不合法。
pub fn all_moves(b: &Board) -> Vec<Move> {
    let mut v = Vec::new();
    for r in 0..b.rows {
        for c in 0..b.cols {
            if c + 1 < b.cols {
                v.push(Move { a: (r, c), b: (r, c + 1) });
            }
            if r + 1 < b.rows {
                v.push(Move { a: (r, c), b: (r + 1, c) });
            }
        }
    }
    v
}
