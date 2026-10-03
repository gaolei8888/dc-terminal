### Task 1: 算法核心（棋盘、模拟、打分）

**Files:**
- Create: `crates/dct-game/Cargo.toml`、`src/lib.rs`、`src/board.rs`、`src/sim.rs`、`src/choose.rs`、`src/tests.rs`
- Modify: `Cargo.toml`（workspace 成员、依赖）

**Interfaces:**
- Produces: `Board::from_read(&GridRead) -> Result<Board, BoardError>`；`Board::parse(&str)`（测试用）；`sim::try_move(&Board, Move) -> Option<Outcome>`；`choose(&Board) -> Vec<Candidate>`（全部合法交换，最好的在最前）；`Candidate { mv: Move, score: f64, features: Features }`；`Move { a: Pos, b: Pos }`，`Pos = (row, col)` 从 0 数。


- [ ] **Step 1: 把新 crate 接进 workspace**

把根 `Cargo.toml` 里 `members` 末尾加上 `"crates/dct-game"`，在 `[dependencies]` 的 `dct-mesh` 那行下面加一行：

```toml
dct-game = { path = "crates/dct-game" }
```

- [ ] **创建 `crates/dct-game/Cargo.toml`**（下面的代码已在临时副本里编译、跑过测试）

```toml
[package]
name = "dct-game"
version = "0.1.0"
edition = "2021"
license = "MIT"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[dev-dependencies]
tempfile = "3"
```

- [ ] **创建 `crates/dct-game/src/lib.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 三消游戏的选步：读到的棋盘 → 合法交换 → 模拟消除 → 打分。纯算法，不碰屏幕、磁盘和网络，
//! 所以能直接拿存下来的真实棋盘测（设计：docs/superpowers/specs/2026-10-02-dct-match3-play-design.md）。
pub mod board;
pub mod choose;
pub mod sim;

pub use board::{Board, BoardError, Cell, GridRead};
pub use choose::{choose, Candidate, Features};
pub use sim::{Move, Pos};

#[cfg(test)]
mod tests;
```

- [ ] **创建 `crates/dct-game/src/board.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
use serde::{Deserialize, Serialize};

pub type Class = u16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cell {
    /// 消掉以后留下的空位（只在模拟里出现）。
    Empty,
    /// 认不出是什么：只有一格的类别（比如彩色炸弹）。不参与连线，也不能被换。
    Unknown,
    Candy { class: Class, special: bool },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Board {
    pub rows: usize,
    pub cols: usize,
    cells: Vec<Cell>,
}

/// dco `read_grid` 返回里 dct 用得到的那几项。其余字段（rgb、frame_age_ms……）serde 会忽略。
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct GridRead {
    pub rows: usize,
    pub cols: usize,
    pub cells: Vec<Vec<Class>>,
    pub odd: Vec<Vec<bool>>,
    pub classes: Vec<ClassInfo>,
    /// 下面三项 dco 有就给：这一帧的编号、拍下它的时间（Unix 毫秒）、画面流上次确认它到现在多久。
    #[serde(default)]
    pub observation_id: Option<String>,
    #[serde(default)]
    pub observed_at_ms: Option<u64>,
    #[serde(default)]
    pub frame_age_ms: Option<u64>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ClassInfo {
    pub id: Class,
    pub count: usize,
    #[serde(default)]
    pub rgb: Option<[u8; 3]>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BoardError {
    /// cells / odd 的行列数跟 rows / cols 对不上。
    Shape,
}

impl Board {
    pub fn from_read(g: &GridRead) -> Result<Board, BoardError> {
        if g.rows == 0
            || g.cols == 0
            || g.cells.len() != g.rows
            || g.odd.len() != g.rows
            || g.cells.iter().any(|r| r.len() != g.cols)
            || g.odd.iter().any(|r| r.len() != g.cols)
        {
            return Err(BoardError::Shape);
        }
        let single = |id: Class| g.classes.iter().any(|c| c.id == id && c.count == 1);
        let mut cells = Vec::with_capacity(g.rows * g.cols);
        for r in 0..g.rows {
            for c in 0..g.cols {
                let class = g.cells[r][c];
                cells.push(if single(class) {
                    Cell::Unknown
                } else {
                    Cell::Candy { class, special: g.odd[r][c] }
                });
            }
        }
        Ok(Board { rows: g.rows, cols: g.cols, cells })
    }

    pub fn get(&self, r: usize, c: usize) -> Cell {
        self.cells[r * self.cols + c]
    }

    pub fn set(&mut self, r: usize, c: usize, v: Cell) {
        self.cells[r * self.cols + c] = v;
    }

    /// 测试用：每行一个字符串，数字是类别，`*` 前缀的数字是特殊糖（`*1`），`.` 是 Unknown。用空格隔开。
    pub fn parse(text: &str) -> Board {
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let rows = lines.len();
        let mut cells = Vec::new();
        let mut cols = 0;
        for l in &lines {
            let row: Vec<Cell> = l
                .split_whitespace()
                .map(|t| match t {
                    "." => Cell::Unknown,
                    _ => match t.strip_prefix('*') {
                        Some(n) => Cell::Candy { class: n.parse().unwrap(), special: true },
                        None => Cell::Candy { class: t.parse().unwrap(), special: false },
                    },
                })
                .collect();
            cols = row.len();
            cells.extend(row);
        }
        Board { rows, cols, cells }
    }
}
```

- [ ] **创建 `crates/dct-game/src/sim.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
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
fn gravity(b: &mut Board) {
    for c in 0..b.cols {
        let kept: Vec<Cell> = (0..b.rows).rev().map(|r| b.get(r, c)).filter(|x| *x != Cell::Empty).collect();
        for (i, r) in (0..b.rows).rev().enumerate() {
            b.set(r, c, kept.get(i).copied().unwrap_or(Cell::Empty));
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
```

- [ ] **创建 `crates/dct-game/src/choose.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
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
```

- [ ] **创建 `crates/dct-game/src/tests.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
use crate::*;
use crate::sim::try_move;

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
```

- [ ] **Step 2: 跑测试**

Run: `cargo test -p dct-game`
Expected: `15 passed`。

- [ ] **Step 3: 变异检查（故意改坏，测试必须变红，改完立刻还原）**

逐条做：改一处 → `cargo test -p dct-game` → 应当至少 1 个测试 FAILED → 还原。**注意**：改 `sim.rs` 里 `if pass == 0 {` 时要改的是第二处（`out.cleared = gone.len()` 上面那处），别误改别的。

| 文件 | 把 | 改成 | 该红的测试 |
|---|---|---|---|
| `sim.rs` | `if pass == 0 {\n            out.cleared` | `if pass == 99 {\n            out.cleared` | 多个 |
| `sim.rs` | `out.wrapped += 1;` | （删掉） | `l_shape_makes_a_wrapped_candy` |
| `sim.rs` | `if run.cells.len() >= 5` | `if run.cells.len() >= 6` | `five_in_a_row_makes_a_bomb` |
| `choose.rs` | `+ 0.5 * f.cascade as f64` | `+ 0.0 * f.cascade as f64` | `a_chain_reaction_outscores_the_same_clear_without_one` |
| `board.rs` | `c.id == id && c.count == 1` | `c.id == id && c.count == 99` | `single_cell_classes_become_unknown` |

- [ ] **Step 4: 提交**

```bash
cargo clippy -p dct-game --all-targets
git add Cargo.toml Cargo.lock crates/dct-game
git commit -m "feat(game): match-3 move picking: legal swaps, cascade simulation, scoring"
```


---

