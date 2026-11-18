### Task 1: 提示和解析（纯函数）

**Files:**
- Create: `crates/dct-game/src/ask.rs`
- Modify: `crates/dct-game/src/lib.rs`（加 `pub mod ask;`）
- Test: `crates/dct-game/src/ask.rs`（`#[cfg(test)] mod tests` 同文件，和 `lab.rs` 一样的风格；若此仓库的 dct-game 习惯把测试放 `*_tests.rs`，就放 `ask_tests.rs` 并在 `lib.rs` 里 `#[cfg(test)] mod ask_tests;`）

**Interfaces:**
- Consumes: `board::GridRead`、`board::Class`（`u16`）、`choose::Candidate`（`mv.a/mv.b: (usize,usize)`，`features: sim::Outcome`）。
- Produces（后面的任务都用这些名字）：
  - `pub struct AskInput { pub board: String, pub goal: String, pub candidates: Vec<String>, pub failed: Vec<String> }`
  - `pub struct Advice { pub choice: Option<usize>, pub reason: String, pub raw: String, pub model: String }`（`choice` 是候选里的 0 起下标；`None` = 模型说都不合适，或没给出可用的编号）
  - `pub trait Advisor { fn pick(&self, input: &AskInput) -> Option<Advice>; }`（`None` = 没问成：没配、连不上、超时）
  - `pub fn board_text(g: &GridRead, fixed: &[Class]) -> String`
  - `pub fn describe(c: &Candidate) -> String`
  - `pub fn describe_move(mv: &crate::sim::Move) -> String`
  - `pub fn prompt_text(i: &AskInput) -> String`
  - `pub fn parse_reply(raw: &str, n: usize) -> (Option<usize>, String)`（返回 0 起的下标和理由）

- [ ] **Step 1: 写失败的测试**

在 `ask.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::GridRead;
    use crate::choose::Candidate;
    use crate::sim::{Move, Outcome};
    use serde_json::json;

    fn grid(cells: &[&[u16]]) -> GridRead {
        let rows = cells.len();
        let cols = cells[0].len();
        serde_json::from_value(json!({
            "rows": rows, "cols": cols, "cells": cells,
            "odd": vec![vec![false; cols]; rows], "classes": []
        }))
        .unwrap()
    }

    fn cand(a: (usize, usize), b: (usize, usize), f: Outcome) -> Candidate {
        Candidate { mv: Move { a, b }, score: 0.0, features: f }
    }

    #[test]
    fn board_text_uses_letters_in_reading_order_and_hash_for_fixed() {
        // 类别号会变，所以字母按「先出现的先给 A」排；fixed 的类别画 #
        let g = grid(&[&[7, 7, 3], &[3, 9, 7]]);
        assert_eq!(board_text(&g, &[9]), "A A B\nB # A");
    }

    #[test]
    fn describe_counts_rows_and_columns_from_one_and_names_what_happens() {
        let f = Outcome { cleared: 7, bomb: 1, triggered: 2, ..Default::default() };
        assert_eq!(
            describe(&cand((0, 3), (1, 3), f)),
            "第 1 行第 4 列和第 2 行第 4 列互换，消 7 颗，做出彩色炸弹，引爆 2 颗特殊糖"
        );
    }

    #[test]
    fn prompt_lists_numbered_candidates_goal_and_failed_swaps() {
        let i = AskInput {
            board: "A A\nB B".into(),
            goal: "清掉冰块".into(),
            candidates: vec!["甲".into(), "乙".into()],
            failed: vec!["丙".into()],
        };
        let p = prompt_text(&i);
        assert!(p.contains("A A\nB B"));
        assert!(p.contains("目标：清掉冰块"));
        assert!(p.contains("1) 甲\n2) 乙"));
        assert!(p.contains("这些换不动：\n丙"));
        // 没有失败清单时不写这一段
        let p2 = prompt_text(&AskInput { failed: vec![], ..i });
        assert!(!p2.contains("换不动"));
    }

    #[test]
    fn parse_reply_takes_the_first_number_inside_range() {
        assert_eq!(parse_reply("3", 5), (Some(2), String::new()));
        assert_eq!(parse_reply("2，因为能消更多", 5), (Some(1), "因为能消更多".into()));
        assert_eq!(parse_reply("选 4 号：做出炸弹", 5).0, Some(3));
    }

    #[test]
    fn parse_reply_zero_out_of_range_empty_and_noise_are_none() {
        assert_eq!(parse_reply("0，都不合适", 5), (None, "都不合适".into()));
        assert_eq!(parse_reply("6", 5).0, None);
        assert_eq!(parse_reply("", 5).0, None);
        assert_eq!(parse_reply("我觉得都可以", 5).0, None);
        assert_eq!(parse_reply("99999999999999999999", 5).0, None);
    }
}
```

`Outcome` 要有 `Default`：先看 `sim.rs` 里 `Outcome` 有没有 `#[derive(Default)]`，没有就加上（只加 derive，不改字段）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-game ask::`
Expected: 编译失败（`board_text` 等未定义）。

- [ ] **Step 3: 写实现**

```rust
//! 规则没把握时问大模型：怎么描述棋盘和候选、怎么读回答。纯函数，不碰网络。
//! 只发文字：棋盘字母图、目标、候选的大白话。不发截图。
use crate::board::{Class, GridRead};
use crate::choose::Candidate;
use crate::sim::Move;

pub struct AskInput {
    pub board: String,
    pub goal: String,
    pub candidates: Vec<String>,
    pub failed: Vec<String>,
}

pub struct Advice {
    /// 候选里的下标（0 起）。`None`：模型说都不合适，或没给出可用的编号。
    pub choice: Option<usize>,
    pub reason: String,
    pub raw: String,
    pub model: String,
}

/// `None` = 没问成（没配、连不上、超时）：调用方当没有这个功能，退回规则。
pub trait Advisor {
    fn pick(&self, input: &AskInput) -> Option<Advice>;
}

/// 字母按先出现的先给 A（类别号每次读都会变，不能直接用）；不是糖的类别画 `#`。
pub fn board_text(g: &GridRead, fixed: &[Class]) -> String {
    let mut letters: Vec<Class> = Vec::new();
    g.cells
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| {
                    if fixed.contains(c) {
                        '#'
                    } else {
                        let k = letters.iter().position(|x| x == c).unwrap_or_else(|| {
                            letters.push(*c);
                            letters.len() - 1
                        });
                        (b'A' + (k % 26) as u8) as char
                    }
                })
                .map(String::from)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn describe_move(mv: &Move) -> String {
    format!(
        "第 {} 行第 {} 列和第 {} 行第 {} 列互换",
        mv.a.0 + 1,
        mv.a.1 + 1,
        mv.b.0 + 1,
        mv.b.1 + 1
    )
}

pub fn describe(c: &Candidate) -> String {
    let f = &c.features;
    let mut s = format!("{}，消 {} 颗", describe_move(&c.mv), f.cleared);
    for (n, what) in [(f.striped, "条纹糖"), (f.wrapped, "包装糖"), (f.bomb, "彩色炸弹")] {
        if n > 0 {
            s += &format!("，做出{what}");
        }
    }
    if f.triggered > 0 {
        s += &format!("，引爆 {} 颗特殊糖", f.triggered);
    }
    s
}

pub fn prompt_text(i: &AskInput) -> String {
    let mut s = format!(
        "三消游戏，棋盘如下（字母是不同颜色的糖，# 是不能动的格子）：\n{}\n\n目标：{}。\n",
        i.board, i.goal
    );
    if !i.failed.is_empty() {
        s += &format!("这些换不动：\n{}\n", i.failed.join("\n"));
    }
    s += "下面是可以走的几步，选对目标最有帮助的一步：\n";
    for (k, c) in i.candidates.iter().enumerate() {
        s += &format!("{}) {}\n", k + 1, c);
    }
    s += "只回答一个编号，后面可以跟一句理由；都不合适就回 0。";
    s
}

/// 取回答里第一串数字当编号（1 起），在 1..=n 里才算；其余（0、越界、没数字、太长）都是 `None`。
/// 编号后面的话当理由，去掉开头的标点和空白。
pub fn parse_reply(raw: &str, n: usize) -> (Option<usize>, String) {
    let t = raw.trim();
    let start = t.find(|c: char| c.is_ascii_digit());
    let Some(start) = start else { return (None, String::new()) };
    let digits: String = t[start..].chars().take_while(|c| c.is_ascii_digit()).collect();
    let rest = t[start + digits.len()..]
        .trim_start_matches(|c: char| c.is_whitespace() || "，,。.：:、；;-—".contains(c))
        .trim()
        .to_string();
    let idx = digits.parse::<usize>().ok().filter(|k| (1..=n).contains(k)).map(|k| k - 1);
    (idx, rest)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo +1.99.0 test -p dct-game ask::`
Expected: 5 passed。再跑 `cargo +1.99.0 clippy -p dct-game --all-targets -- -D warnings`，无警告。

- [ ] **Step 5: 变异检查，然后提交**

临时把 `parse_reply` 里的 `(1..=n).contains(k)` 改成 `(0..=n)`，测试应该变红；改回来。把 `board_text` 的 `fixed.contains(c)` 改成 `false`，测试应该变红；改回来。

```bash
git add crates/dct-game/src/ask.rs crates/dct-game/src/lib.rs crates/dct-game/src/sim.rs
git commit -m "feat(game): text prompt and reply parser for asking a model to pick a move"
```

---

