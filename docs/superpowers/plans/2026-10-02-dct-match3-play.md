# dct 玩三消（第一轮）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `dct game play` 在 Mac 上读 dco 的棋盘、按规则选一步、让 dco 划、等画面停下，一步（读+选+划）1 秒以内，每一步完整记录；并把说明卡装给 Claude Code、Codex、千问，让用户对 AI 说一句话就能开玩。

**Architecture:** 新 crate `dct-game`：纯算法（棋盘/模拟/打分）+ 注入式的玩的循环（`Dco`、`Clock` 两个 trait）+ 真 dco 客户端（unix socket 上的 MCP，只在 unix 编译）。主 crate 加 `src/game/`：棋盘配置、记录、说给用户听的话、说明卡、`dct game play` 命令；守护进程里一个后台线程负责装说明卡。

**Tech Stack:** Rust（workspace 里新加 `crates/dct-game`）、serde/serde_json、toml、sha2（主 crate 已有）、tempfile（测试）。没有新的外部依赖。

**Spec:** `docs/superpowers/specs/2026-10-02-dct-match3-play-design.md`（读它再读本计划）；dco 接口：dc-octo `docs/superpowers/specs/2026-10-02-read-grid-design.md`。

## Global Constraints

- 一律用中文给用户说话；不出现类别编号、分数公式、git/终端黑话（项目 CLAUDE.md：用户不是程序员）。行列从 1 数、从上往下。
- 没有 C 依赖（dct 老规矩：整棵依赖树一行 C 都没有，Windows 上不装 Visual Studio Build Tools 也能编）。不新增依赖 crate。
- Windows 也要能编、能过 CI：`dco` 客户端模块和它的测试都带 `#[cfg(unix)]`；`dct game play` 在非 unix 上只说「这一版只支持 Mac」。
- 提交信息用英文，**不加任何 AI 署名行**（用户的规矩，覆盖默认的 Co-Authored-By）。
- 单元测试和集成测试**绝不能写用户真实的 `~/.dct`、`~/.claude`、`~/.codex`、`~/.qwen`**：一律用临时目录当 HOME；守护进程里装说明卡的线程只在 socket 是默认路径时才起。
- 判断放在 dct、不放 dco：dco 只看、做；这一轮 dco 一行都不用改。
- 本轮不做：自动校准、`dcv games`、开关卡/道具/弹窗、问大模型、`dct mcp`、dc.ai 按钮、小模型、i18n（本轮只有中文）。
- 路径：`~/.dct/games/<游戏>.toml`（配置）、`~/.dct/games/log/<UTC 日期>.jsonl`（记录）、`~/.dco/endpoint.json` 和 `~/.dco/token`（dco 的门）。
- 打分权重是第一轮拍的常量：`cleared + 0.5·cascade + 6·striped + 8·wrapped + 15·bomb + 5·triggered + 20·special_swap + (lowest_row+1)/rows`。
- 停下的等待时间常量：首次等 150 ms、轮询 100 ms、3 秒没反应记 `no_change`、8 秒还在变就停。

## Review Focus

每一条都已经落在下面某个任务的测试里（括号里是哪个任务）：

1. 测试棋盘里「只有一格的类别」会被当成认不出、不能换——写测试棋盘时每类至少两格，否则一个本该合法的步会消失（任务 1、2 的棋盘都这样造）。
2. 动画中间 `not_a_grid` 不是错，不能当场停；急停、锁屏却必须马上停（任务 2）。
3. 划成功后「没反应」计数必须清零，否则一次早先的没反应加一次后来的会凑成两次而误停（任务 2，`a_move_that_works_resets_the_unmoved_counter`）。
4. 同一盘棋上划了没反应的步，第二次要换一步，不能重复划同一步（任务 2）。
5. 彩色炸弹是一格的类别，会让类别数加一，这不算「换了画面」；多两个以上才停（任务 2）。
6. 用户自己写的同名说明卡绝不覆盖；没装的 agent 不替它建目录；内容一样不重写（任务 5）。
7. 配置文件写错时要指出是哪个文件、哪个字段；游戏名不能带路径（`../x`）（任务 3）。
8. 试走（`--dry-run`）一次都不能划（任务 2、4）。

---

## File Structure

| 文件 | 职责 |
|---|---|
| `crates/dct-game/src/board.rs` | `Board`、`Cell`、`GridRead`（dco `read_grid` 返回里 dct 用的那几项）、`from_read` |
| `crates/dct-game/src/sim.rs` | 合法交换、模拟消除/下落/特殊糖，返回 `Outcome` |
| `crates/dct-game/src/choose.rs` | 打分、排序，返回全部候选 |
| `crates/dct-game/src/play.rs` | 玩的循环、等画面停下、停下的条件、记录字段 |
| `crates/dct-game/src/dco.rs` | 真 dco 客户端（unix） |
| `src/game/profile.rs` | 棋盘配置：内置 + 覆盖 + 指纹 |
| `src/game/log.rs` | 追加写 JSONL |
| `src/game/text.rs` | 步骤行、停下的话、退出码 |
| `src/game/skill.rs`、`skill.md` | 说明卡 |
| `src/game/cli.rs` | `dct game play` |
| `src/main.rs`、`src/lib.rs`、`src/journal.rs`、`src/daemon.rs` | 接线（各一两行） |
| `tests/game_cli.rs` | 对着假 dco 跑整条命令 |

---

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

### Task 2: 玩的循环

**Files:**
- Create: `crates/dct-game/src/play.rs`、`src/play_tests.rs`
- Modify: `crates/dct-game/src/lib.rs`

**Interfaces:**
- Consumes: 任务 1 的 `Board::from_read`、`choose`、`GridRead`。
- Produces: `trait Dco { fn read_grid(&mut self, &Profile) -> Result<GridRead, DcoError>; fn swipe(&mut self, &Profile, (f64,f64), (f64,f64)) -> Result<(), DcoError>; }`；`trait Clock { fn now_ms(&self) -> u64; fn sleep_ms(&mut self, u64); }`；`Profile { window: Value, region: [f64;4], rows, cols, extra: Value }`；`DcoError { code: String, message: String }`；`Options { max_steps, dry_run }`；`play(&mut dyn Dco, &mut dyn Clock, &Profile, &Options, &mut dyn FnMut(Value)) -> Summary { steps, stop: Stop }`；`Stop::{NoGrid(String), ClassesChanged{was,now}, NoMoves, Stuck, StillMoving, StepsDone, DryRun, Dco(DcoError)}`。每一步通过回调交出一条记录（字段见设计「记录」一节）。


- [ ] **Step 1: 在 `crates/dct-game/src/lib.rs` 里加两行**

把 `pub mod sim;` 上面加 `pub mod play;`，文件末尾加 `#[cfg(test)]\nmod play_tests;`。

- [ ] **创建 `crates/dct-game/src/play.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 一盘游戏怎么玩：读盘 → 选步 → 划 → 等画面停下 → 下一步，以及什么时候停。
//! 跟 dco 说话（`Dco`）和计时（`Clock`）都是传进来的，所以测试里换成假的，不碰真机也不真睡觉。
use crate::board::{Board, GridRead};
use crate::choose::{choose, Candidate};
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct Profile {
    pub window: Value,
    pub region: [f64; 4], // x, y, w, h
    pub rows: usize,
    pub cols: usize,
    pub extra: Value, // inset / class_de / top_div / odd_share：原样带给 read_grid
}

#[derive(Debug, Clone, PartialEq)]
pub struct DcoError {
    pub code: String,
    pub message: String,
}

pub trait Dco {
    fn read_grid(&mut self, p: &Profile) -> Result<GridRead, DcoError>;
    /// 坐标是窗口比例（0～1）。
    fn swipe(&mut self, p: &Profile, from: (f64, f64), to: (f64, f64)) -> Result<(), DcoError>;
}

pub trait Clock {
    fn now_ms(&self) -> u64;
    fn sleep_ms(&mut self, ms: u64);
}

pub struct Options {
    pub max_steps: usize,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    /// 读不出棋盘（含：类别太多，多半是弹了窗）。
    NoGrid(String),
    /// 类别比开始时多出两个以上：多半这关结束了、弹了窗。
    ClassesChanged { was: usize, now: usize },
    NoMoves,
    /// 连着两次划了没反应。
    Stuck,
    /// 8 秒了画面还在变。
    StillMoving,
    StepsDone,
    DryRun,
    /// dco 急停 / 暂停 / 锁屏 / 别的错误。
    Dco(DcoError),
}

pub struct Summary {
    pub steps: usize,
    pub stop: Stop,
}

enum Settle {
    Settled(GridRead),
    NoChange(GridRead),
    StillMoving,
    Failed(DcoError),
}

fn same(a: &GridRead, b: &GridRead) -> bool {
    a.cells == b.cells && a.odd == b.odd
}

const FIRST_WAIT_MS: u64 = 150;
const POLL_MS: u64 = 100;
const NO_CHANGE_MS: u64 = 3_000;
const GIVE_UP_MS: u64 = 8_000;

fn settle(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, before: &GridRead) -> Settle {
    let t0 = clock.now_ms();
    clock.sleep_ms(FIRST_WAIT_MS);
    let mut prev: Option<GridRead> = None;
    let mut changed = false;
    loop {
        let waited = clock.now_ms() - t0;
        match dco.read_grid(p) {
            Ok(g) => {
                if !same(&g, before) {
                    changed = true;
                }
                if changed {
                    if prev.as_ref().is_some_and(|x| same(x, &g)) {
                        return Settle::Settled(g);
                    }
                } else if waited >= NO_CHANGE_MS {
                    return Settle::NoChange(g);
                }
                prev = Some(g);
            }
            // 动画中间读不出来是常事：接着等，等到时间到。别的错误（急停、锁屏）马上停。
            Err(e) if e.code == "not_a_grid" => prev = None,
            Err(e) => return Settle::Failed(e),
        }
        if waited >= GIVE_UP_MS {
            return Settle::StillMoving;
        }
        clock.sleep_ms(POLL_MS);
    }
}

fn centre(p: &Profile, (r, c): (usize, usize)) -> (f64, f64) {
    let [x, y, w, h] = p.region;
    (x + (c as f64 + 0.5) / p.cols as f64 * w, y + (r as f64 + 0.5) / p.rows as f64 * h)
}

pub fn play(dco: &mut dyn Dco, clock: &mut dyn Clock, p: &Profile, o: &Options, sink: &mut dyn FnMut(Value)) -> Summary {
    let mut steps = 0;
    let mut baseline: Option<usize> = None;
    let mut failed: Vec<(crate::sim::Move, Vec<Vec<u16>>)> = Vec::new();
    let mut streak = 0;
    let mut current: Option<GridRead> = None;
    let stop = loop {
        if steps >= o.max_steps {
            break Stop::StepsDone;
        }
        let t_read = clock.now_ms();
        let g = match current.take() {
            Some(g) => g,
            None => match dco.read_grid(p) {
                Ok(g) => g,
                Err(e) if e.code == "not_a_grid" => break Stop::NoGrid(e.message),
                Err(e) => break Stop::Dco(e),
            },
        };
        let read_ms = clock.now_ms() - t_read;
        let base = *baseline.get_or_insert(g.classes.len());
        if g.classes.len() > base + 1 {
            break Stop::ClassesChanged { was: base, now: g.classes.len() };
        }
        let Ok(board) = Board::from_read(&g) else {
            break Stop::NoGrid("棋盘的行列数对不上".into());
        };
        let t_choose = clock.now_ms();
        let cands: Vec<Candidate> = choose(&board);
        let choose_ms = clock.now_ms() - t_choose;
        // 同一盘棋上划了没反应的步，不再重复选。
        let pick = cands.iter().position(|c| !failed.iter().any(|(m, cells)| *m == c.mv && *cells == g.cells));
        let Some(pick) = pick else {
            break if cands.is_empty() { Stop::NoMoves } else { Stop::Stuck };
        };
        let chosen = &cands[pick];
        steps += 1;
        let mut rec = json!({
            "schema": 1, "time_ms": clock.now_ms(),
            "observation_id": g.observation_id, "observed_at_ms": g.observed_at_ms, "frame_age_ms": g.frame_age_ms,
            "rows": g.rows, "cols": g.cols, "cells": g.cells, "odd": g.odd, "classes": g.classes,
            "candidates": cands.iter().map(|c| json!({
                "a": [c.mv.a.0, c.mv.a.1], "b": [c.mv.b.0, c.mv.b.1], "score": c.score,
                "cleared": c.features.cleared, "cascade": c.features.cascade, "striped": c.features.striped,
                "wrapped": c.features.wrapped, "bomb": c.features.bomb, "triggered": c.features.triggered,
                "special_swap": c.features.special_swap, "lowest_row": c.features.lowest_row })).collect::<Vec<_>>(),
            "chosen": pick, "dry_run": o.dry_run,
        });
        if o.dry_run {
            rec["outcome"] = json!("dry_run");
            sink(rec);
            break Stop::DryRun;
        }
        let t_swipe = clock.now_ms();
        if let Err(e) = dco.swipe(p, centre(p, chosen.mv.a), centre(p, chosen.mv.b)) {
            rec["outcome"] = json!("stopped");
            sink(rec);
            break Stop::Dco(e);
        }
        let swipe_ms = clock.now_ms() - t_swipe;
        let t_settle = clock.now_ms();
        let result = settle(dco, clock, p, &g);
        rec["timing_ms"] = json!({ "read": read_ms, "choose": choose_ms, "swipe": swipe_ms, "settle": clock.now_ms() - t_settle });
        match result {
            Settle::Settled(next) => {
                rec["outcome"] = json!("moved");
                rec["after_observation_id"] = json!(next.observation_id);
                sink(rec);
                streak = 0;
                failed.clear();
                current = Some(next);
            }
            Settle::NoChange(same_board) => {
                rec["outcome"] = json!("no_change");
                rec["after_observation_id"] = json!(same_board.observation_id);
                sink(rec);
                streak += 1;
                failed.push((chosen.mv, g.cells.clone()));
                if streak >= 2 {
                    break Stop::Stuck;
                }
                current = Some(same_board);
            }
            Settle::StillMoving => {
                rec["outcome"] = json!("stopped");
                sink(rec);
                break Stop::StillMoving;
            }
            Settle::Failed(e) => {
                rec["outcome"] = json!("stopped");
                sink(rec);
                break Stop::Dco(e);
            }
        }
    };
    Summary { steps, stop }
}
```

- [ ] **创建 `crates/dct-game/src/play_tests.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
use crate::play::*;
use crate::GridRead;
use serde_json::{json, Value};
use std::collections::VecDeque;

fn grid(cells: &[&[u16]]) -> GridRead {
    let rows = cells.len();
    let cols = cells[0].len();
    let mut ids: Vec<u16> = cells.iter().flat_map(|r| r.iter().copied()).collect();
    ids.sort();
    ids.dedup();
    let classes: Vec<Value> = ids.iter().map(|&i| json!({"id": i, "count": cells.iter().flat_map(|r| r.iter()).filter(|&&c| c == i).count()})).collect();
    serde_json::from_value(json!({
        "rows": rows, "cols": cols, "cells": cells, "odd": vec![vec![false; cols]; rows], "classes": classes
    }))
    .unwrap()
}

struct Fake {
    reads: VecDeque<Result<GridRead, DcoError>>,
    last: Option<Result<GridRead, DcoError>>,
    swipes: Vec<((f64, f64), (f64, f64))>,
    swipe_err: Option<DcoError>,
}
impl Fake {
    fn new(reads: Vec<Result<GridRead, DcoError>>) -> Fake {
        Fake { reads: reads.into(), last: None, swipes: vec![], swipe_err: None }
    }
}
impl Dco for Fake {
    fn read_grid(&mut self, _: &Profile) -> Result<GridRead, DcoError> {
        if let Some(r) = self.reads.pop_front() {
            self.last = Some(r.clone());
            return r;
        }
        self.last.clone().expect("脚本里一次读盘都没有")
    }
    fn swipe(&mut self, _: &Profile, f: (f64, f64), t: (f64, f64)) -> Result<(), DcoError> {
        if let Some(e) = self.swipe_err.clone() {
            return Err(e);
        }
        self.swipes.push((f, t));
        Ok(())
    }
}
struct Clk(u64);
impl Clock for Clk {
    fn now_ms(&self) -> u64 {
        self.0
    }
    fn sleep_ms(&mut self, ms: u64) {
        self.0 += ms;
    }
}
fn profile(rows: usize, cols: usize) -> Profile {
    Profile { window: json!({"app": "x"}), region: [0.2, 0.3, 0.5, 0.4], rows, cols, extra: json!({}) }
}
fn run(d: &mut Fake, max: usize, dry: bool) -> (Summary, Vec<Value>) {
    let mut log = vec![];
    let s = play(d, &mut Clk(0), &profile(3, 4), &Options { max_steps: max, dry_run: dry }, &mut |v| log.push(v));
    (s, log)
}
fn err(code: &str) -> DcoError {
    DcoError { code: code.into(), message: format!("m-{code}") }
}

// 每个类别至少出现两次：只有一格的类别按设计算「认不出」，不能换。
const A: &[&[u16]] = &[&[1, 1, 2, 1], &[2, 3, 1, 3], &[3, 2, 3, 2]]; // 能走：(0,2) 和 (1,2) 对换，第 0 行四连
const DEAD: &[&[u16]] = &[&[1, 2, 3, 1], &[3, 1, 2, 3], &[2, 3, 1, 2]]; // 没有任何能走的步（每类都不止一格）
const MOVED: &[&[u16]] = DEAD;

#[test]
fn plays_one_move_then_stops_when_nothing_is_left() {
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(MOVED)), Ok(grid(MOVED)), Ok(grid(DEAD))]);
    // 第 1 次读：A；划；落定读两次 MOVED；下一步直接用落定的画面（MOVED）选步……MOVED 里有连线吗？没有 → NoMoves
    let (s, log) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::NoMoves);
    assert_eq!(s.steps, 1);
    assert_eq!(d.swipes.len(), 1);
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["outcome"], "moved");
    // (0,2)↔(1,2)：竖着划。x = 0.2 + 2.5/4*0.5，y 从 0.3+0.5/3*0.4 到 0.3+1.5/3*0.4
    let (f, t) = d.swipes[0];
    assert!((f.0 - 0.5125).abs() < 1e-9 && (t.0 - 0.5125).abs() < 1e-9);
    assert!((f.1 - 0.366666666).abs() < 1e-6 && (t.1 - 0.5).abs() < 1e-9);
}

#[test]
fn two_unmoved_swipes_stop_and_the_second_try_is_a_different_move() {
    let mut d = Fake::new(vec![Ok(grid(A))]); // 一直读到 A
    let (s, log) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::Stuck);
    assert_eq!(d.swipes.len(), 2);
    assert_ne!(d.swipes[0], d.swipes[1], "第二次不该重复划同一步");
    assert_eq!(log.iter().filter(|l| l["outcome"] == "no_change").count(), 2);
}

#[test]
fn a_screen_that_never_stops_changing_gives_up() {
    let a = grid(A);
    let mut reads = vec![Ok(a.clone())];
    for i in 0..200u16 {
        reads.push(Ok(grid(&[&[i, 1, 2, 1], &[2, 3, 1, 3], &[3, 2, 3, 2]])));
    }
    let mut d = Fake::new(reads);
    let (s, _) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::StillMoving);
}

#[test]
fn not_a_grid_during_the_animation_is_waited_out() {
    let mut d = Fake::new(vec![Ok(grid(A)), Err(err("not_a_grid")), Ok(grid(MOVED)), Ok(grid(MOVED)), Ok(grid(DEAD))]);
    let (s, _) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::NoMoves);
    assert_eq!(s.steps, 1);
}

#[test]
fn halted_while_settling_stops() {
    let mut d = Fake::new(vec![Ok(grid(A)), Err(err("halted"))]);
    let (s, _) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::Dco(err("halted")));
}

#[test]
fn halted_on_swipe_stops_and_logs_it() {
    let mut d = Fake::new(vec![Ok(grid(A))]);
    d.swipe_err = Some(err("halted"));
    let (s, log) = run(&mut d, 10, false);
    assert_eq!(s.stop, Stop::Dco(err("halted")));
    assert_eq!(log[0]["outcome"], "stopped");
}

#[test]
fn unreadable_board_at_the_start_stops() {
    let mut d = Fake::new(vec![Err(err("not_a_grid"))]);
    let (s, _) = run(&mut d, 10, false);
    assert!(matches!(s.stop, Stop::NoGrid(_)));
    assert_eq!(s.steps, 0);
}

#[test]
fn many_new_classes_mean_something_else_is_on_screen() {
    // 开始 3 类；落定后变成 5 类（弹窗）
    let five: &[&[u16]] = &[&[1, 4, 2, 5], &[2, 3, 1, 3], &[3, 2, 3, 2]];
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(five)), Ok(grid(five))]);
    let (s, _) = run(&mut d, 10, false);
    assert!(matches!(s.stop, Stop::ClassesChanged { was: 3, now: 5 }), "{:?}", s.stop);
}

#[test]
fn one_extra_class_is_allowed() {
    // 多出来一个一格的类别（彩色炸弹）不算换了画面
    let four: &[&[u16]] = &[&[1, 1, 2, 1], &[2, 3, 1, 3], &[3, 2, 3, 4]];
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(four)), Ok(grid(four))]);
    let (s, _) = run(&mut d, 1, false);
    assert_eq!(s.stop, Stop::StepsDone);
}

#[test]
fn dry_run_never_swipes() {
    let mut d = Fake::new(vec![Ok(grid(A))]);
    let (s, log) = run(&mut d, 10, true);
    assert_eq!(s.stop, Stop::DryRun);
    assert!(d.swipes.is_empty());
    assert_eq!(log[0]["outcome"], "dry_run");
}

#[test]
fn stops_after_the_step_limit() {
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(MOVED)), Ok(grid(MOVED))]);
    let (s, _) = run(&mut d, 1, false);
    assert_eq!(s.stop, Stop::StepsDone);
    assert_eq!(s.steps, 1);
}

#[test]
fn every_log_line_has_the_fields_the_comparison_needs() {
    let mut d = Fake::new(vec![Ok(grid(A)), Ok(grid(MOVED)), Ok(grid(MOVED))]);
    let (_, log) = run(&mut d, 1, false);
    for k in ["schema", "cells", "odd", "classes", "observation_id", "after_observation_id", "candidates", "chosen", "outcome", "timing_ms", "time_ms"] {
        assert!(log[0].get(k).is_some(), "缺 {k}");
    }
    assert!(log[0]["candidates"].as_array().unwrap().len() >= 1);
}

#[test]
fn a_move_that_works_resets_the_unmoved_counter() {
    // 没反应一次、落定一次、再没反应一次：每次都只是「一次」，不该停。
    // 脚本：A → 划 → 一直没变(NoChange) → 换一步再划 → 变成 B 落定 → 用 B 选步 → B 划 → 没变……
    // 为了让 B 也有能走的步，B 用 A 的镜像。
    let b: &[&[u16]] = &[&[2, 1, 1, 1], &[3, 1, 2, 3], &[2, 3, 2, 3]];
    let mut script = vec![];
    for _ in 0..40 {
        script.push(Ok(grid(A)));
    }
    script.push(Ok(grid(b)));
    script.push(Ok(grid(b)));
    let mut d = Fake::new(script);
    let (s, log) = run(&mut d, 3, false);
    // 前两次划：第一次没反应，第二次（另一步）落定到 b
    let outcomes: Vec<&str> = log.iter().map(|l| l["outcome"].as_str().unwrap()).collect();
    assert_eq!(&outcomes[..2], ["no_change", "moved"], "{outcomes:?}");
    // 落定之后第 3 步没反应只算第 1 次：到了步数上限停，而不是因为早先那次凑成两次而 Stuck
    assert_eq!(outcomes.len(), 3, "{outcomes:?}");
    assert_eq!(s.stop, Stop::StepsDone);
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test -p dct-game`
Expected: `28 passed`（任务 1 的 15 个 + 这里 13 个）。

- [ ] **Step 3: 变异检查**

| 文件 | 把 | 改成 | 该红的测试 |
|---|---|---|---|
| `play.rs` | `streak >= 2` | `streak >= 3` | `two_unmoved_swipes_stop_…` |
| `play.rs` | `g.classes.len() > base + 1` | `g.classes.len() > base + 2` | `many_new_classes_…` |
| `play.rs` | `streak = 0;\n                failed.clear();`（**两行一起**，别动 `let mut streak = 0;`） | `failed.clear();` | `a_move_that_works_resets_the_unmoved_counter` |
| `play.rs` | `failed.push((chosen.mv, g.cells.clone()));` | （删掉） | `two_unmoved_swipes_stop_…` |
| `play.rs` | `Err(e) if e.code == "not_a_grid" => prev = None,` | `Err(e) if e.code == "not_a_grid" => break Settle::Failed(e),` | `not_a_grid_during_the_animation_is_waited_out` |
| `play.rs` | `if o.dry_run {` | `if false {` | `dry_run_never_swipes` |

- [ ] **Step 4: 提交**

```bash
cargo clippy -p dct-game --all-targets
git add crates/dct-game
git commit -m "feat(game): the play loop: read, choose, swipe, wait for the board to settle, and when to stop"
```


---

### Task 3: 连 dco

**Files:**
- Create: `crates/dct-game/src/dco.rs`、`src/dco_tests.rs`
- Modify: `crates/dct-game/src/lib.rs`

**Interfaces:**
- Consumes: 任务 2 的 `Dco`、`Profile`、`DcoError`。
- Produces: `DcoClient::connect(dir: &Path) -> Result<DcoClient, DcoError>`（`dir` 是 `~/.dco`；失败的错误码 `dco_down` / `dco_refused`）；`impl Dco for DcoClient`；`DcoClient::call(tool, args) -> Result<Value, DcoError>`（工具报错时错误码取 dco 给的 `error.code`，比如 `halted`、`not_a_grid`、`not_found`、`screen_locked`）。

协议依据（dc-octo `src/client.rs`、`src/mcp.rs`）：先写一行 `{"dco_token": "<~/.dco/token 的 64 位十六进制>"}`，读到 `{"ok":true}`；再 `initialize` → `notifications/initialized` → `tools/call`；一行一条 JSON-RPC；工具结果在 `result.content[0].text` 里（一段 JSON 文字），`result.isError` 为真时那段 JSON 是 `{"error":{"code","message"}}`。`swipe`、`read_grid` 都不需要执行票（read、self 档）。


- [ ] **Step 1: 在 `crates/dct-game/src/lib.rs` 里加接线**

`pub mod choose;` 下面加：

```rust
#[cfg(unix)]
pub mod dco;
```

文件末尾加：

```rust
#[cfg(all(test, unix))]
mod dco_tests;
```

- [ ] **创建 `crates/dct-game/src/dco.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 连本机 dco：unix socket 上的 MCP（一行一条 JSON-RPC）。握手跟 dco 自己的 `dco call` 一样：
//! 先写 `{"dco_token": "<~/.dco/token>"}`，读到 `{"ok":true}`，再 initialize → notifications/initialized → tools/call。
use crate::board::GridRead;
use crate::play::{Dco, DcoError, Profile};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub struct DcoClient {
    r: BufReader<UnixStream>,
    w: UnixStream,
    next_id: u64,
}

fn err(code: &str, message: impl Into<String>) -> DcoError {
    DcoError { code: code.into(), message: message.into() }
}

impl DcoClient {
    /// `dir` 是 `~/.dco`。
    pub fn connect(dir: &Path) -> Result<DcoClient, DcoError> {
        let endpoint: Value = std::fs::read_to_string(dir.join("endpoint.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .ok_or_else(|| err("dco_down", "dco 没在运行，先打开 dco 再试"))?;
        let socket = endpoint["socket"].as_str().ok_or_else(|| err("dco_down", "dco 没在运行，先打开 dco 再试"))?;
        let token = std::fs::read_to_string(dir.join("token")).map_err(|_| err("dco_down", "读不到 dco 的钥匙文件，dco 还没启动过？"))?;
        let stream = UnixStream::connect(socket).map_err(|_| err("dco_down", "dco 没在运行，先打开 dco 再试"))?;
        let w = stream.try_clone().map_err(|e| err("dco_down", e.to_string()))?;
        let mut c = DcoClient { r: BufReader::new(stream), w, next_id: 1 };
        c.send(&json!({ "dco_token": token.trim() }))?;
        let ack = c.read_line()?;
        if ack.get("ok") != Some(&Value::Bool(true)) {
            return Err(err("dco_refused", format!("dco 拒绝了连接：{ack}")));
        }
        let init = c.request("initialize", json!({
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": { "name": "dct", "version": env!("CARGO_PKG_VERSION") }
        }))?;
        let _ = init;
        c.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))?;
        Ok(c)
    }

    fn send(&mut self, v: &Value) -> Result<(), DcoError> {
        writeln!(self.w, "{v}").map_err(|_| err("dco_down", "dco 断开了连接"))
    }

    fn read_line(&mut self) -> Result<Value, DcoError> {
        let mut line = String::new();
        match self.r.read_line(&mut line) {
            Ok(0) | Err(_) => Err(err("dco_down", "dco 断开了连接")),
            Ok(_) => serde_json::from_str(line.trim()).map_err(|_| err("dco_down", "dco 回了看不懂的话")),
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, DcoError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let reply = self.read_line()?;
        if let Some(e) = reply.get("error") {
            return Err(err("dco_error", e["message"].as_str().unwrap_or("dco 报错")));
        }
        Ok(reply["result"].clone())
    }

    /// 调一个工具，返回它文字部分里的 JSON。工具报错时错误码取 dco 给的 `error.code`。
    pub fn call(&mut self, tool: &str, args: Value) -> Result<Value, DcoError> {
        let result = self.request("tools/call", json!({ "name": tool, "arguments": args }))?;
        let text = result["content"][0]["text"].as_str().unwrap_or("{}");
        let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        if result["isError"].as_bool().unwrap_or(false) {
            let e = &body["error"];
            return Err(err(e["code"].as_str().unwrap_or("dco_error"), e["message"].as_str().unwrap_or(text)));
        }
        Ok(body)
    }
}

impl Dco for DcoClient {
    fn read_grid(&mut self, p: &Profile) -> Result<GridRead, DcoError> {
        let mut args = json!({
            "window": p.window,
            "region": { "x": p.region[0], "y": p.region[1], "w": p.region[2], "h": p.region[3] },
            "rows": p.rows, "cols": p.cols,
        });
        if let (Some(extra), Some(obj)) = (p.extra.as_object(), args.as_object_mut()) {
            for (k, v) in extra {
                obj.insert(k.clone(), v.clone());
            }
        }
        let body = self.call("read_grid", args)?;
        serde_json::from_value(body).map_err(|e| err("dco_error", format!("dco 的棋盘回复读不懂：{e}")))
    }

    fn swipe(&mut self, p: &Profile, from: (f64, f64), to: (f64, f64)) -> Result<(), DcoError> {
        self.call("swipe", json!({
            "window": p.window,
            "from": { "x": from.0, "y": from.1 }, "to": { "x": to.0, "y": to.1 },
        }))
        .map(|_| ())
    }
}
```

- [ ] **创建 `crates/dct-game/src/dco_tests.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
use crate::dco::DcoClient;
use crate::play::{Dco, Profile};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

/// 假 dco：按 dco 真实的握手和回复形状说话。`handler(tool, args)` 返回 (JSON, 是不是错误)。
fn fake(dir: &std::path::Path, token: &str, handler: impl Fn(&str, &Value) -> (Value, bool) + Send + 'static) -> std::thread::JoinHandle<Vec<String>> {
    let sock = dir.join("dco.sock");
    std::fs::write(dir.join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.join("token"), token).unwrap();
    let l = UnixListener::bind(&sock).unwrap();
    let token = token.to_string();
    std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let mut seen = vec![];
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let hello: Value = serde_json::from_str(line.trim()).unwrap();
        seen.push(hello.to_string());
        if hello["dco_token"] != token {
            writeln!(w, r#"{{"ok":false}}"#).unwrap();
            return seen;
        }
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return seen;
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            seen.push(req["method"].as_str().unwrap().to_string());
            let Some(id) = req.get("id") else { continue };
            let result = match req["method"].as_str().unwrap() {
                "initialize" => json!({"protocolVersion": "2025-06-18"}),
                "tools/call" => {
                    let (body, is_error) = handler(req["params"]["name"].as_str().unwrap(), &req["params"]["arguments"]);
                    json!({"content": [{"type": "text", "text": body.to_string()}], "isError": is_error})
                }
                _ => json!({}),
            };
            writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap();
        }
    })
}

fn profile() -> Profile {
    Profile { window: json!({"app": "iPhone Mirroring"}), region: [0.2, 0.3, 0.5, 0.4], rows: 2, cols: 2, extra: json!({"class_de": 20.0}) }
}

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn reads_a_grid_with_the_extra_parameters_and_swipes() {
    let dir = tempfile::tempdir().unwrap();
    let h = fake(dir.path(), TOKEN, |tool, args| match tool {
        "read_grid" => {
            assert_eq!(args["rows"], 2);
            assert_eq!(args["class_de"], 20.0);
            assert_eq!(args["region"]["w"], 0.5);
            assert_eq!(args["window"]["app"], "iPhone Mirroring");
            (json!({"rows":2,"cols":2,"cells":[[0,0],[1,1]],"odd":[[false,false],[false,false]],
                    "classes":[{"id":0,"rgb":[1,2,3],"count":2},{"id":1,"rgb":[4,5,6],"count":2}],"elapsed_ms":1}), false)
        }
        "swipe" => {
            assert_eq!(args["from"]["x"], 0.3);
            assert_eq!(args["to"]["y"], 0.5);
            (json!({"swiped": true}), false)
        }
        other => panic!("没想到会调 {other}"),
    });
    let mut c = DcoClient::connect(dir.path()).unwrap();
    let g = c.read_grid(&profile()).unwrap();
    assert_eq!((g.rows, g.cols, g.classes.len()), (2, 2, 2));
    c.swipe(&profile(), (0.3, 0.4), (0.4, 0.5)).unwrap();
    drop(c);
    let seen = h.join().unwrap();
    assert_eq!(&seen[1..4], ["initialize", "notifications/initialized", "tools/call"]);
}

#[test]
fn a_tool_error_keeps_dcos_error_code() {
    let dir = tempfile::tempdir().unwrap();
    let _h = fake(dir.path(), TOKEN, |_, _| (json!({"error": {"code": "halted", "message": "急停中"}}), true));
    let mut c = DcoClient::connect(dir.path()).unwrap();
    let e = c.swipe(&profile(), (0.3, 0.4), (0.4, 0.5)).unwrap_err();
    assert_eq!((e.code.as_str(), e.message.as_str()), ("halted", "急停中"));
}

#[test]
fn a_wrong_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let _h = fake(dir.path(), TOKEN, |_, _| (json!({}), false));
    std::fs::write(dir.path().join("token"), "f".repeat(64)).unwrap();
    assert_eq!(DcoClient::connect(dir.path()).err().unwrap().code, "dco_refused");
}

#[test]
fn no_endpoint_file_means_dco_is_not_running() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(DcoClient::connect(dir.path()).err().unwrap().code, "dco_down");
}

#[test]
fn a_dead_socket_means_dco_is_not_running() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("endpoint.json"), json!({"socket": dir.path().join("none.sock")}).to_string()).unwrap();
    std::fs::write(dir.path().join("token"), TOKEN).unwrap();
    assert_eq!(DcoClient::connect(dir.path()).err().unwrap().code, "dco_down");
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test -p dct-game`
Expected: `33 passed`。

- [ ] **Step 3: 提交**

```bash
cargo clippy -p dct-game --all-targets
git add crates/dct-game
git commit -m "feat(game): talk to dco over its unix socket (read_grid, swipe)"
```


---

### Task 4: 棋盘配置和记录文件

**Files:**
- Create: `src/game/mod.rs`、`src/game/profile.rs`、`src/game/log.rs`
- Modify: `src/lib.rs`（加 `pub mod game;`，放在 `pub mod gate;` 上面）、`src/journal.rs`（`fn civil_from_days` 前面加 `pub(crate)`）

**Interfaces:**
- Consumes: `dct_game::play::Profile`。
- Produces: `profile::load(home: &Path, game: &str) -> Result<Loaded, String>`，`Loaded { profile, sha256, source }`；`profile::games_dir(home) -> PathBuf`；`log::LogFile::open(home, secs_since_epoch) -> io::Result<LogFile>`，`.path()`，`.append(&Value)`。

注意：`src/game/mod.rs` 这一步先只声明 `profile`、`log` 两个模块；`cli`、`skill`、`text` 在后面的任务里加（否则编不过）。


- [ ] **创建 `src/game/mod.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! `dct game`：让 dct 自己玩三消游戏。选步是纯算法（`crates/dct-game`），这里只管棋盘配置、
//! 记录、给用户看的话、装给 agent 的说明卡，和命令本身（设计：
//! docs/superpowers/specs/2026-10-02-dct-match3-play-design.md）。
pub mod log;
pub mod profile;
```

- [ ] **创建 `src/game/profile.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 棋盘配置：窗口、棋盘在窗口里的位置、行列数、读棋盘的参数。第一轮手写：内置一份 Candy Crush，
//! `~/.dct/games/<游戏>.toml` 存在就用它。棋盘位置每关不同，换关要改——自动校准是下一轮的事。
use dct_game::play::Profile;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// 第 1712 关的版面（出处：dc-octo `tests/grid_real.rs` 的 region；odd_share 0.15 是 dco 实测：
/// 包装糖也标得出来，且不误标普通糖）。
pub const CANDY_CRUSH: &str = r#"window = { app = "iPhone Mirroring" }
region = { x = 0.2372, y = 0.3156, w = 0.5257, h = 0.4622 }
rows = 9
cols = 5
inset = 0.6
class_de = 24
top_div = 5
odd_share = 0.15
"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    window: Window,
    region: Region,
    rows: usize,
    cols: usize,
    inset: Option<f64>,
    class_de: Option<f64>,
    top_div: Option<u32>,
    odd_share: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Window {
    app: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Region {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

pub struct Loaded {
    pub profile: Profile,
    /// 配置文本的 sha256（十六进制），记进每一步，事后才知道那一步是按哪份配置走的。
    pub sha256: String,
    /// 说给用户听的来源：「内置的」或文件路径。
    pub source: String,
}

pub fn games_dir(home: &Path) -> PathBuf {
    home.join(".dct").join("games")
}

pub fn load(home: &Path, game: &str) -> Result<Loaded, String> {
    if game.is_empty() || !game.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err(format!("游戏名「{game}」只能用小写英文字母、数字和短横线"));
    }
    let path = games_dir(home).join(format!("{game}.toml"));
    let (text, source) = match std::fs::read_to_string(&path) {
        Ok(t) => (t, path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if game == "candy-crush" {
                (CANDY_CRUSH.to_string(), "内置的".to_string())
            } else {
                return Err(format!("不认识游戏「{game}」。现在只内置了 candy-crush，其它的要自己写 {}", path.display()));
            }
        }
        Err(e) => return Err(format!("读不了 {}：{e}", path.display())),
    };
    let f: File = toml::from_str(&text).map_err(|e| format!("{source} 写得不对：{e}"))?;
    check(&f).map_err(|m| format!("{source} 写得不对：{m}"))?;
    let mut extra = Map::new();
    if let Some(v) = f.inset {
        extra.insert("inset".into(), json!(v));
    }
    if let Some(v) = f.class_de {
        extra.insert("class_de".into(), json!(v));
    }
    if let Some(v) = f.top_div {
        extra.insert("top_div".into(), json!(v));
    }
    if let Some(v) = f.odd_share {
        extra.insert("odd_share".into(), json!(v));
    }
    let sha256 = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    Ok(Loaded {
        profile: Profile {
            window: json!({ "app": f.window.app }),
            region: [f.region.x, f.region.y, f.region.w, f.region.h],
            rows: f.rows,
            cols: f.cols,
            extra: Value::Object(extra),
        },
        sha256,
        source,
    })
}

fn check(f: &File) -> Result<(), String> {
    let r = &f.region;
    let inside = |v: f64| (0.0..=1.0).contains(&v);
    if ![r.x, r.y, r.w, r.h].iter().all(|&v| inside(v)) || r.w <= 0.0 || r.h <= 0.0 || r.x + r.w > 1.0 || r.y + r.h > 1.0 {
        return Err("region 要整个落在窗口里：x、y、w、h 都取 0 到 1，w 和 h 大于 0，x+w、y+h 不超过 1".into());
    }
    if !(1..=32).contains(&f.rows) || !(1..=32).contains(&f.cols) {
        return Err("rows、cols 都要在 1 到 32 之间".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(game: &str, text: &str) -> tempfile::TempDir {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(games_dir(h.path())).unwrap();
        std::fs::write(games_dir(h.path()).join(format!("{game}.toml")), text).unwrap();
        h
    }

    #[test]
    fn the_built_in_candy_crush_loads_and_passes_its_own_checks() {
        let h = tempfile::tempdir().unwrap();
        let l = load(h.path(), "candy-crush").unwrap();
        assert_eq!((l.profile.rows, l.profile.cols), (9, 5));
        assert_eq!(l.profile.window["app"], "iPhone Mirroring");
        assert_eq!(l.profile.extra["odd_share"], 0.15);
        assert_eq!(l.source, "内置的");
        assert_eq!(l.sha256.len(), 64);
    }

    #[test]
    fn a_file_in_the_games_folder_wins_and_changes_the_fingerprint() {
        let h = home_with("candy-crush", &CANDY_CRUSH.replace("rows = 9", "rows = 8"));
        let l = load(h.path(), "candy-crush").unwrap();
        assert_eq!(l.profile.rows, 8);
        let builtin = load(tempfile::tempdir().unwrap().path(), "candy-crush").unwrap();
        assert_ne!(l.sha256, builtin.sha256);
        assert!(l.source.ends_with("candy-crush.toml"));
    }

    #[test]
    fn a_typo_names_the_file_and_the_line() {
        let h = home_with("candy-crush", &CANDY_CRUSH.replace("cols = 5", "colz = 5"));
        let e = load(h.path(), "candy-crush").err().unwrap();
        assert!(e.contains("candy-crush.toml") && e.contains("colz"), "{e}");
    }

    #[test]
    fn out_of_range_values_are_refused_with_a_reason() {
        for (from, to) in [("rows = 9", "rows = 0"), ("cols = 5", "cols = 33"), ("w = 0.5257", "w = 0.9"), ("w = 0.5257", "w = 0.0")] {
            let h = home_with("candy-crush", &CANDY_CRUSH.replace(from, to));
            assert!(load(h.path(), "candy-crush").is_err(), "{to} 该被拒");
        }
    }

    #[test]
    fn unknown_games_and_unsafe_names_are_refused() {
        let h = tempfile::tempdir().unwrap();
        assert!(load(h.path(), "chess").err().unwrap().contains("不认识"));
        for bad in ["", "../x", "A", "a/b", "a.b"] {
            assert!(load(h.path(), bad).is_err(), "{bad:?}");
        }
    }
}
```

- [ ] **创建 `src/game/log.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 每一步一行 JSON，追加到 `~/.dct/games/log/<日期>.jsonl`。每行写完就落盘：玩到一半被 Ctrl-C，
//! 已经走过的那些步不会丢。以后拿这些记录和大模型的选择比。
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct LogFile {
    path: PathBuf,
}

impl LogFile {
    pub fn open(home: &Path, secs_since_epoch: u64) -> std::io::Result<LogFile> {
        let dir = super::profile::games_dir(home).join("log");
        std::fs::create_dir_all(&dir)?;
        let (y, m, d) = crate::journal::civil_from_days((secs_since_epoch / 86_400) as i64);
        Ok(LogFile { path: dir.join(format!("{y:04}-{m:02}-{d:02}.jsonl")) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, v: &Value) -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(f, "{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lines_are_appended_to_a_file_named_by_the_utc_date() {
        let h = tempfile::tempdir().unwrap();
        // 2026-10-02 12:00:00 UTC
        let l = LogFile::open(h.path(), 1_790_942_400).unwrap();
        assert!(l.path().ends_with("2026-10-02.jsonl"), "{:?}", l.path());
        l.append(&json!({"a": 1})).unwrap();
        l.append(&json!({"b": 2})).unwrap();
        let again = LogFile::open(h.path(), 1_790_942_400 + 60).unwrap();
        again.append(&json!({"c": 3})).unwrap();
        let text = std::fs::read_to_string(l.path()).unwrap();
        let lines: Vec<Value> = text.lines().map(|x| serde_json::from_str(x).unwrap()).collect();
        assert_eq!(lines, vec![json!({"a": 1}), json!({"b": 2}), json!({"c": 3})]);
    }

    #[test]
    fn a_new_day_is_a_new_file() {
        let h = tempfile::tempdir().unwrap();
        let a = LogFile::open(h.path(), 1_790_942_400).unwrap();
        let b = LogFile::open(h.path(), 1_790_942_400 + 86_400).unwrap();
        assert_ne!(a.path(), b.path());
    }
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test --lib game::`
Expected: `7 passed`（profile 5 个 + log 2 个）。`1790942400` 是 2026-10-02 12:00:00 UTC（已用 Python 核对）。

- [ ] **Step 3: 提交**

```bash
git add src/lib.rs src/journal.rs src/game
git commit -m "feat(game): board profile (built-in Candy Crush, overridable) and the per-step log file"
```


---

### Task 5: 说给用户听的话、说明卡、`dct game play` 命令

**Files:**
- Create: `src/game/text.rs`、`src/game/skill.rs`、`src/game/skill.md`、`src/game/cli.rs`、`tests/game_cli.rs`
- Modify: `src/game/mod.rs`、`src/main.rs`、`src/daemon.rs`

**Interfaces:**
- Consumes: 任务 2、3 的 `play`、`DcoClient`；任务 4 的 `profile::load`、`LogFile`。
- Produces: `dct game play [--game 名字] [--steps N] [--dry-run]`，退出码：走满步数 / 没有能走的步 / 试走 = 0，其余 = 1，参数错 = 2；`skill::install_all(home) -> Vec<(&str, io::Result<Installed>)>`。


- [ ] **Step 1: 把 `src/game/mod.rs` 改成完整的**

```rust
//! `dct game`：让 dct 自己玩三消游戏。选步是纯算法（`crates/dct-game`），这里只管棋盘配置、
//! 记录、给用户看的话、装给 agent 的说明卡，和命令本身（设计：
//! docs/superpowers/specs/2026-10-02-dct-match3-play-design.md）。
pub mod cli;
pub mod log;
pub mod profile;
pub mod skill;
pub mod text;
```

- [ ] **创建 `src/game/text.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
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
    let mut s = format!("第 {step} 步：{} ↔ {}，消 {} 颗", pos(&c["a"]), pos(&c["b"]), n(&c["cleared"]));
    for (key, what) in [("striped", "条纹糖"), ("wrapped", "包装糖"), ("bomb", "彩色炸弹")] {
        if n(&c[key]) > 0 {
            s += &format!("，做出{what}");
        }
    }
    if n(&c["triggered"]) > 0 {
        s += &format!("，引爆 {} 颗特殊糖", n(&c["triggered"]));
    }
    if c["special_swap"].as_bool().unwrap_or(false) {
        s += "，两颗特殊糖互换";
    }
    match rec["outcome"].as_str().unwrap_or("") {
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
    }
}

pub fn dco_error(e: &DcoError) -> String {
    match e.code.as_str() {
        "dco_down" | "dco_refused" => e.message.clone(),
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
        Stop::DryRun => ("试走结束，没有真划。".into(), 0),
        Stop::NoGrid(why) => (format!("停了：读不出棋盘（{why}）。多半是这一关结束了，或者弹出了别的画面。{tail}"), 1),
        Stop::ClassesChanged { was, now } => (
            format!("停了：棋盘上的颜色种类一下子变多了（原来 {was} 种，现在 {now} 种），多半是这一关结束了，或者弹出了窗口。{tail}"),
            1,
        ),
        Stop::Stuck => (format!("停了：连着两次划了都没反应。请看一眼屏幕上是不是弹出了什么。{tail}"), 1),
        Stop::StillMoving => (format!("停了：等了 8 秒画面还在动。{tail}"), 1),
        Stop::Dco(e) => (format!("停了：{} {tail}", dco_error(e)), 1),
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
                {"a": [0, 0], "b": [0, 1], "cleared": 3, "striped": 0, "wrapped": 0, "bomb": 0, "triggered": 0, "special_swap": false},
                {"a": [6, 1], "b": [6, 2], "cleared": 4, "striped": 1, "wrapped": 0, "bomb": 0, "triggered": 0, "special_swap": false}
            ],
            "outcome": "moved", "timing_ms": {"read": 2, "choose": 0, "swipe": 262, "settle": 900}
        });
        for (k, v) in extra.as_object().unwrap() {
            r[k] = v.clone();
        }
        r
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
        assert_eq!(dco_error(&e("weird")), "dco 说：原话");
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
        ];
        let mut codes: Vec<&str> = all.iter().map(stop_code).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
    }
}
```

- [ ] **创建 `src/game/skill.md`**（下面的代码已在临时副本里编译、跑过测试）

```markdown
---
name: dct-game
description: 用户想让 AI 玩三消游戏（比如 Candy Crush）时使用。运行 dct game play，按规则一步一步玩，每步不到一秒，不用截图问大模型。
---
<!-- dct-managed: dct-game-skill v1 -->

# 玩三消游戏

用户说「帮我玩一关 Candy Crush」「帮我消几步」「玩一会儿三消」之类的话，就用这个办法。

## 开始之前

- 用户要先在 iPhone 镜像里把游戏打开，进到一关里。这一步你不要代劳：关卡按钮、道具、付款都不要自己去点。
- 如果用户还没开游戏，告诉他先打开，再回来说一声。

## 怎么玩

运行：

    dct game play

它会一步一步玩，最多 20 步，每走一步就印一行，例如：

    第 3 步：第 7 行第 2 列 ↔ 第 7 行第 3 列，消 4 颗，做出条纹糖（读 2 ms，选 0 ms，划 262 ms）

- 把每一步的话转述给用户，用大白话，不用解释内部细节。
- 命令结束以后，告诉用户这次走了几步、为什么停；步数到了的话问他要不要接着玩，要就再运行一次。
- 想先看看它会怎么走、但不真的划：`dct game play --dry-run`。
- 想一次多走几步：`dct game play --steps 50`（最多 200）。

## 停

- 用户说「停」，马上中断这条命令。
- 命令自己停下时（没有能走的步、读不出棋盘、画面一直在动、连着两次没反应、dco 急停），把它印的最后一句话原样告诉用户。
- 不要自己去点屏幕补救，也不要换别的办法去操作游戏；让用户决定下一步。

## 不要做

- 不要自己调用 dco 去点关卡按钮、道具、广告、付款。
- 不要同时开两条 `dct game play`。
```

- [ ] **创建 `src/game/skill.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! 把「怎么让 AI 玩三消」的说明卡装进 Claude Code、Codex、千问各自的 skills 目录。三家都认
//! `<家目录>/.<agent>/skills/<名字>/SKILL.md` 这个格式。卡片里带一行 dct 的标记：有标记的我们可以更新，
//! 没有标记的同名文件是用户自己写的，绝不覆盖。
use std::io;
use std::path::Path;

pub const SKILL_MD: &str = include_str!("skill.md");
pub const MARKER: &str = "<!-- dct-managed: dct-game-skill v1 -->";
const AGENT_DIRS: [&str; 3] = [".claude", ".codex", ".qwen"];

#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    Wrote,
    Same,
    /// 同名文件不是我们写的，没动。
    UserOwned,
    /// 这个 agent 的目录不存在（没装过），不替它建。
    NoAgent,
}

fn install_one(home: &Path, agent_dir: &str) -> io::Result<Installed> {
    let root = home.join(agent_dir);
    if !root.is_dir() {
        return Ok(Installed::NoAgent);
    }
    let dir = root.join("skills").join("dct-game");
    let file = dir.join("SKILL.md");
    match std::fs::read_to_string(&file) {
        Ok(old) if !old.contains(MARKER) => return Ok(Installed::UserOwned),
        Ok(old) if old == SKILL_MD => return Ok(Installed::Same),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&file, SKILL_MD)?;
    Ok(Installed::Wrote)
}

/// 尽力而为：装不上不影响守护进程，所以逐个返回结果，由调用方决定要不要理。
pub fn install_all(home: &Path) -> Vec<(&'static str, io::Result<Installed>)> {
    AGENT_DIRS.iter().map(|d| (*d, install_one(home, d))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_path(home: &Path, d: &str) -> std::path::PathBuf {
        home.join(d).join("skills").join("dct-game").join("SKILL.md")
    }

    #[test]
    fn the_card_has_front_matter_on_line_one_and_the_marker() {
        assert!(SKILL_MD.starts_with("---\nname: dct-game\ndescription: "));
        assert!(SKILL_MD.contains(MARKER));
    }

    #[test]
    fn installs_into_every_agent_that_exists_and_skips_the_rest() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir(h.path().join(".claude")).unwrap();
        std::fs::create_dir(h.path().join(".qwen")).unwrap();
        let r = install_all(h.path());
        let got: Vec<_> = r.iter().map(|(d, x)| (*d, x.as_ref().unwrap())).collect();
        assert_eq!(got, vec![(".claude", &Installed::Wrote), (".codex", &Installed::NoAgent), (".qwen", &Installed::Wrote)]);
        assert_eq!(std::fs::read_to_string(skill_path(h.path(), ".claude")).unwrap(), SKILL_MD);
        assert!(!h.path().join(".codex").exists(), "不替没装的 agent 建目录");
    }

    #[test]
    fn a_second_install_writes_nothing() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir(h.path().join(".codex")).unwrap();
        install_all(h.path());
        let before = std::fs::metadata(skill_path(h.path(), ".codex")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(install_all(h.path())[1].1.as_ref().unwrap(), &Installed::Same);
        assert_eq!(std::fs::metadata(skill_path(h.path(), ".codex")).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn an_old_version_of_our_card_is_updated() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(skill_path(h.path(), ".claude").parent().unwrap()).unwrap();
        std::fs::write(skill_path(h.path(), ".claude"), format!("旧的\n{MARKER}\n")).unwrap();
        assert_eq!(install_all(h.path())[0].1.as_ref().unwrap(), &Installed::Wrote);
        assert_eq!(std::fs::read_to_string(skill_path(h.path(), ".claude")).unwrap(), SKILL_MD);
    }

    #[test]
    fn a_users_own_file_with_the_same_name_is_never_overwritten() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(skill_path(h.path(), ".claude").parent().unwrap()).unwrap();
        std::fs::write(skill_path(h.path(), ".claude"), "我自己写的").unwrap();
        assert_eq!(install_all(h.path())[0].1.as_ref().unwrap(), &Installed::UserOwned);
        assert_eq!(std::fs::read_to_string(skill_path(h.path(), ".claude")).unwrap(), "我自己写的");
    }
}
```

- [ ] **创建 `src/game/cli.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! `dct game play [--game 名字] [--steps N] [--dry-run]`。
use super::{log::LogFile, profile, text};
use dct_game::play::{play, Clock, Options};

const USAGE: &str = "用法：dct game play [--game candy-crush] [--steps 20] [--dry-run]";

pub struct Args {
    pub game: String,
    pub steps: usize,
    pub dry_run: bool,
}

pub fn parse(args: &[String]) -> Result<Args, String> {
    if args.first().map(String::as_str) != Some("play") {
        return Err(USAGE.into());
    }
    let mut a = Args { game: "candy-crush".into(), steps: 20, dry_run: false };
    let mut it = args[1..].iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--dry-run" => a.dry_run = true,
            "--game" => a.game = it.next().ok_or("--game 后面要写游戏名")?.clone(),
            "--steps" => {
                let v = it.next().ok_or("--steps 后面要写步数")?;
                a.steps = v.parse().ok().filter(|n| (1..=200).contains(n)).ok_or("--steps 要在 1 到 200 之间")?;
            }
            other => return Err(format!("不认识 {other}。{USAGE}")),
        }
    }
    Ok(a)
}

struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }
    fn sleep_ms(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

pub fn run(args: &[String]) -> i32 {
    let a = match parse(args) {
        Ok(a) => a,
        Err(m) => {
            eprintln!("{m}");
            return 2;
        }
    };
    run_parsed(&a)
}

#[cfg(not(unix))]
fn run_parsed(_: &Args) -> i32 {
    eprintln!("这一版只支持 Mac：玩游戏要用 dco，而 dco 目前只有 Mac 版。");
    1
}

#[cfg(unix)]
fn run_parsed(a: &Args) -> i32 {
    use dct_game::dco::DcoClient;
    use serde_json::json;

    let Some(home) = crate::sys::home() else {
        eprintln!("找不到家目录。");
        return 1;
    };
    let loaded = match profile::load(&home, &a.game) {
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
    let clock = SystemClock;
    let log = match LogFile::open(&home, clock.now_ms() / 1000) {
        Ok(l) => l,
        Err(e) => {
            // 记录是这件事的要点（以后拿去比），写不了就不开始。
            eprintln!("记录文件开不了（{}）：{e}", profile::games_dir(&home).join("log").display());
            return 1;
        }
    };
    let (mut n, mut warned) = (0, false);
    let mut sink = |mut rec: serde_json::Value| {
        n += 1;
        rec["game"] = json!(a.game);
        rec["profile_sha256"] = json!(loaded.sha256);
        if let Err(e) = log.append(&rec) {
            if !warned {
                eprintln!("记录文件写不进去了（{e}），后面的步不会被记下来。");
                warned = true;
            }
        }
        println!("{}", text::step_line(n, &rec));
    };
    let summary = play(&mut dco, &mut SystemClock, &loaded.profile, &Options { max_steps: a.steps, dry_run: a.dry_run }, &mut sink);
    let (line, code) = text::stop_line(&summary.stop, summary.steps, log.path());
    let _ = log.append(&json!({ "schema": 1, "game": a.game, "stop": text::stop_code(&summary.stop), "steps": summary.steps }));
    if code == 0 {
        println!("{line}");
    } else {
        eprintln!("{line}");
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[&str]) -> Result<Args, String> {
        parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn defaults() {
        let a = p(&["play"]).unwrap();
        assert_eq!((a.game.as_str(), a.steps, a.dry_run), ("candy-crush", 20, false));
    }

    #[test]
    fn flags() {
        let a = p(&["play", "--steps", "50", "--dry-run", "--game", "x-1"]).unwrap();
        assert_eq!((a.game.as_str(), a.steps, a.dry_run), ("x-1", 50, true));
    }

    #[test]
    fn bad_input_is_refused_with_a_reason() {
        for bad in [&["play", "--steps", "0"][..], &["play", "--steps", "201"], &["play", "--steps", "x"], &["play", "--steps"], &["play", "--game"], &["play", "--nope"], &[], &["stop"]] {
            assert!(p(bad).is_err(), "{bad:?}");
        }
    }
}
```

- [ ] **Step 2: 在 `src/main.rs` 里接线**

在 `Some("keys") => ...` 这一行**上面**加：

```rust
        // 玩三消游戏：连的是本机的 dco，不经守护进程。
        Some("game") => std::process::exit(dct::game::cli::run(&args[1..])),
```

在 `HELP` 里 `  dct peers        看组里有哪些电脑` 这一行**上面**加：

```text
  dct game play [--game candy-crush] [--steps 20] [--dry-run]
                   让 dct 自己玩三消游戏（要先在 iPhone 镜像里打开一关；只支持 Mac）
```

- [ ] **Step 3: 在 `src/daemon.rs` 的 `pub fn run(socket: &Path)` 开头接上说明卡的安装线程**

把 `pub fn run` 的开头（`let mgr = SessionManager::new();` 之前）改成：

```rust
pub fn run(socket: &Path) -> Result<()> {
    // 「怎么让 AI 玩三消」的说明卡：真正的守护进程里才装。单元测试和集成测试把 socket 放进临时目录，
    // 它们绝不能去写用户真实的 `~/.claude` 等。每 30 秒看一遍，是因为 agent 第一次运行才会建出自己的
    // 目录——用户装了 agent、开了会话，不用重启守护进程说明卡也会出现。
    if socket == crate::proto::socket_path().as_path() {
        let _ = std::thread::Builder::new().name("game-skill".into()).spawn(|| loop {
            if let Some(home) = crate::sys::home() {
                let _ = crate::game::skill::install_all(&home);
            }
            std::thread::sleep(Duration::from_secs(30));
        });
    }
    let mgr = SessionManager::new();
```

（`Duration` 在 `src/daemon.rs` 第 7 行已经 `use std::time::Duration;`。）

- [ ] **Step 4: 写集成测试**


- [ ] **创建 `tests/game_cli.rs`**（下面的代码已在临时副本里编译、跑过测试）

```rust
//! `dct game play` 对着一个假 dco 跑一遍：握手、读棋盘、选步、记录、退出码。假 dco 按 dco 真实的
//! 握手和回复形状说话（见 dc-octo `src/client.rs`、`src/mcp.rs`）。
#![cfg(unix)]
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::Command;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// 9 行 5 列，每个类别不止一格；(0,2)↔(1,2) 能换出第 0 行的四连。
fn board() -> Value {
    let mut cells: Vec<Vec<u64>> = (0..9).map(|r| (0..5).map(|c| ((r + c) % 3) as u64).collect()).collect();
    cells[0] = vec![0, 0, 1, 0, 2];
    cells[1] = vec![1, 2, 0, 1, 0];
    let mut counts = [0usize; 3];
    cells.iter().flatten().for_each(|&c| counts[c as usize] += 1);
    json!({
        "rows": 9, "cols": 5, "cells": cells, "odd": vec![vec![false; 5]; 9],
        "classes": (0..3).map(|i| json!({"id": i, "rgb": [i, i, i], "count": counts[i as usize]})).collect::<Vec<_>>(),
        "observation_id": "obs-0000000000000001", "observed_at_ms": 1790942400000u64, "frame_age_ms": 5, "elapsed_ms": 1
    })
}

fn fake_dco(home: &std::path::Path, swipe_error: Option<&'static str>) -> std::thread::JoinHandle<Vec<String>> {
    let dir = home.join(".dco");
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("dco.sock");
    std::fs::write(dir.join("endpoint.json"), json!({"socket": sock}).to_string()).unwrap();
    std::fs::write(dir.join("token"), TOKEN).unwrap();
    let l = UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let mut tools = vec![];
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(line.trim()).unwrap()["dco_token"], TOKEN);
        writeln!(w, r#"{{"ok":true}}"#).unwrap();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return tools;
            }
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            let Some(id) = req.get("id") else { continue };
            let result = match req["method"].as_str().unwrap() {
                "tools/call" => {
                    let name = req["params"]["name"].as_str().unwrap().to_string();
                    tools.push(name.clone());
                    let (body, is_error) = match (name.as_str(), swipe_error) {
                        ("read_grid", _) => (board(), false),
                        ("swipe", Some(code)) => (json!({"error": {"code": code, "message": "x"}}), true),
                        ("swipe", None) => (json!({"swiped": true}), false),
                        (other, _) => panic!("没想到会调 {other}"),
                    };
                    json!({"content": [{"type": "text", "text": body.to_string()}], "isError": is_error})
                }
                _ => json!({"protocolVersion": "2025-06-18"}),
            };
            writeln!(w, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap();
        }
    })
}

fn dct(home: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let o = Command::new(env!("CARGO_BIN_EXE_dct")).args(args).env("HOME", home).stdin(std::process::Stdio::null()).output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into(), o.status.code().unwrap_or(-1))
}

fn log_lines(home: &std::path::Path) -> Vec<Value> {
    let dir = home.join(".dct/games/log");
    let f = std::fs::read_dir(&dir).unwrap().next().expect("没有记录文件").unwrap().path();
    std::fs::read_to_string(f).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[test]
fn a_dry_run_prints_the_move_never_swipes_and_logs_it() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco(home.path(), None);
    let (out, err, code) = dct(home.path(), &["game", "play", "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("第 1 步：") && out.contains("试走，没有真划"), "{out}");
    assert!(out.contains("试走结束"), "{out}");
    assert_eq!(h.join().unwrap(), vec!["read_grid"], "试走不该划");
    let lines = log_lines(home.path());
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["game"], "candy-crush");
    assert_eq!(lines[0]["profile_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(lines[0]["observation_id"], "obs-0000000000000001");
    assert_eq!(lines[0]["outcome"], "dry_run");
    assert_eq!((lines[1]["stop"].as_str(), lines[1]["steps"].as_u64()), (Some("dry_run"), Some(1)));
}

#[test]
fn a_halted_dco_stops_the_game_with_a_plain_sentence_and_a_failing_exit_code() {
    let home = tempfile::tempdir().unwrap();
    let h = fake_dco(home.path(), Some("halted"));
    let (out, err, code) = dct(home.path(), &["game", "play", "--steps", "3"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("急停"), "{err}");
    assert_eq!(h.join().unwrap(), vec!["read_grid", "swipe"]);
    assert_eq!(log_lines(home.path()).last().unwrap()["stop"], "dco");
}

#[test]
fn without_dco_it_says_so_and_does_not_create_a_log() {
    let home = tempfile::tempdir().unwrap();
    let (out, err, code) = dct(home.path(), &["game", "play"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("dco 没在运行"), "{err}");
    assert!(!home.path().join(".dct/games/log").exists());
}

#[test]
fn bad_arguments_exit_2_and_show_usage() {
    let home = tempfile::tempdir().unwrap();
    for args in [&["game", "play", "--steps", "0"][..], &["game"], &["game", "play", "--nope"]] {
        let (_, err, code) = dct(home.path(), args);
        assert_eq!(code, 2, "{args:?} {err}");
        assert!(!err.is_empty());
    }
}

#[test]
fn a_broken_profile_file_names_itself() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".dct/games")).unwrap();
    std::fs::write(home.path().join(".dct/games/candy-crush.toml"), "rows = \"x\"").unwrap();
    let (_, err, code) = dct(home.path(), &["game", "play", "--dry-run"]);
    assert_eq!(code, 1);
    assert!(err.contains("candy-crush.toml"), "{err}");
}

#[test]
fn help_lists_the_game_command() {
    let home = tempfile::tempdir().unwrap();
    let (out, _, _) = dct(home.path(), &["--help"]);
    assert!(out.contains("dct game play"), "{out}");
}
```

- [ ] **Step 5: 跑测试**

Run: `cargo test --lib game:: && cargo test --test game_cli`
Expected: 库里 `21 passed`（profile 5、log 2、text 6、skill 5、cli 3），集成 `6 passed`。

再跑一遍全仓库，确认没碰坏别的（已在临时副本里跑过：库 1646 个单元测试全过）：

Run: `cargo test --workspace`
Expected: 全绿。

- [ ] **Step 6: 提交**

```bash
cargo clippy --all-targets
git add src tests/game_cli.rs
git commit -m "feat(game): dct game play, plain-Chinese output, and the skill card for Claude Code, Codex and Qwen"
```


---

### Task 6: 真机验收、真实棋盘夹具、说明文档

**Files:**
- Create: `crates/dct-game/tests/fixtures/candy-1712-live.json`、`crates/dct-game/tests/real_board.rs`
- Modify: `README.zh-CN.md`、`README.md`

这个任务要在装了 dco、能开 iPhone 镜像的 Mac 上做，**需要用户在场**（开游戏、看屏幕、在三个 agent 里各说一句话）。前面的任务不需要。

- [ ] **Step 1: 开一关，先试走**

用户在 iPhone 镜像里打开 Candy Crush 的一关（版面要跟第 1712 关一样：5 列 9 行，位置同内置配置；不一样就先照设计「棋盘配置」改 `~/.dct/games/candy-crush.toml`）。

Run: `dct game play --dry-run`
Expected: 打印一行「第 1 步：… 试走，没有真划」，退出码 0。请用户对着屏幕看，这一步是不是合理（能消、位置对得上）。**不合理就停在这里**，把当时的记录文件（`~/.dct/games/log/<今天>.jsonl`）和屏幕截图给开发者，别往下走。

- [ ] **Step 2: 存下这一盘真实的棋盘**

Run: `mkdir -p crates/dct-game/tests/fixtures && dco call read_grid '{"window":{"app":"iPhone Mirroring"},"region":{"x":0.2372,"y":0.3156,"w":0.5257,"h":0.4622},"rows":9,"cols":5,"odd_share":0.15}' > crates/dct-game/tests/fixtures/candy-1712-live.json`

`dco call` 会把工具回复的 JSON 直接打印出来，所以这个文件就是 `GridRead` 能读的那份。让用户对着屏幕核对：每格的类别分组和屏幕上一致、条纹/包装糖的位置标得对。游戏里的棋盘一动就变了，所以要在玩之前、画面不动的时候存。

- [ ] **Step 3: 把这一盘钉成测试**

先让程序告诉你它会怎么选：

Run: `cargo test -p dct-game --test real_board -- --nocapture`（第一次这个测试文件还不存在，先用下面的内容建好，断言里的两个数先写 0，看输出后再改）

```rust
use dct_game::{choose, Board, GridRead};

#[test]
fn the_live_1712_board_has_the_moves_a_person_sees() {
    let g: GridRead = serde_json::from_str(include_str!("fixtures/candy-1712-live.json")).unwrap();
    let b = Board::from_read(&g).unwrap();
    let c = choose(&b);
    for (i, x) in c.iter().enumerate().take(10) {
        println!("{i}: {:?} score={:.2} {:?}", x.mv, x.score, x.features);
    }
    // 下面两个数：先写 0 跑一遍，把打印出来、并经用户对着屏幕核对过的真实值填进来。
    assert_eq!(c.len(), 0, "合法交换的总数");
    assert_eq!((c[0].mv.a, c[0].mv.b), ((0, 0), (0, 0)), "最好的那一步");
}
```

用户对着屏幕核对打印出来的前几步（行列从 0 数，所以要加 1 再对屏幕），确认没有漏掉明显的好步、没有选出不成立的交换；然后把两个断言里的数改成真实值，再跑一次：

Run: `cargo test -p dct-game --test real_board`
Expected: PASS。

- [ ] **Step 4: 真走 20 步**

Run: `dct game play`
Expected: 每步一行，约 20 步后停，最后一句话说清为什么停；退出码 0（走满或没有能走的步）。打开 `~/.dct/games/log/<今天>.jsonl`，确认每行都有 `observation_id`、`candidates`、`timing_ms`、`outcome`。

验收数字：`timing_ms.read + choose + swipe` 每步 < 1000（用下面的命令看最慢的几步）：

Run: `python3 -c "import json,glob,sys; r=[json.loads(l) for f in glob.glob('$HOME/.dct/games/log/*.jsonl') for l in open(f)]; t=sorted(x['timing_ms']['read']+x['timing_ms']['choose']+x['timing_ms']['swipe'] for x in r if x.get('timing_ms')); print(len(t), t[-5:])"`
Expected: 最慢几步都小于 1000。

- [ ] **Step 5: 急停**

玩的时候让用户（或另一个窗口）运行 `dco call halt`；命令应在下一步前停下，最后一句话是「dco 急停了……」，退出码 1。之后 `dco call resume` 恢复。

- [ ] **Step 6: 三个 agent 都能用**

确认说明卡已经装上：`ls ~/.claude/skills/dct-game ~/.codex/skills/dct-game ~/.qwen/skills/dct-game`（守护进程在跑、三个目录都存在时 30 秒内会出现）。

在 dct 里分别开 Claude Code、Codex、千问的会话（游戏开着），各说一句「帮我玩一关 Candy Crush」。Expected：三家都去运行 `dct game play`，并把每一步转述给用户；用户说「停」能停。**哪一家没触发就如实记下来**，别改说明卡硬凑，先报告（说明卡的触发词是否够，需要看那家的实际行为）。

- [ ] **Step 7: 文档**

在 `README.zh-CN.md` 的命令清单附近（跟 `dct send` 同一块）加：

```markdown
### 让 AI 玩三消游戏（Mac）

在 iPhone 镜像里打开 Candy Crush 的一关，然后在 dct 的会话里对 AI 说「帮我玩一关 Candy Crush」。
dct 读棋盘、按规则选步、让 dco 划，每步不到一秒；每一步的棋盘、所有能走的步和选择都记在 `~/.dct/games/log/`。
也可以自己运行：`dct game play [--steps 20] [--dry-run]`。棋盘的位置写在 `~/.dct/games/candy-crush.toml`（没有这个文件就用内置的）。
目前要自己打开关卡；换一关版面不同时要改这个文件，自动调整放在下一轮。
```

`README.md`（英文）加同样内容的英文版，位置相同。

- [ ] **Step 8: 提交**

```bash
git add crates/dct-game/tests README.md README.zh-CN.md
git commit -m "test(game): pin the live 1712 board; docs for dct game play"
```

- [ ] **Step 9: 告诉 dc-octo 和 dc-vault 会话**

用 SendMessage 告诉 dc-octo 会话：第一轮做完了、验收数字是多少、三个 agent 哪些触发了；告诉 dc-vault 会话：记录文件的路径和字段（它们要读，用来做以后的对比）。
