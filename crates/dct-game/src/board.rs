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
