# dct 玩三消：规则没把握时问大模型 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `dct game play` 在规则没把握时（划了没反应之后，或用户要求时），把棋盘、目标、规则列出的候选步用文字发给网关里的大模型，让它从候选里选一步；选不出或出任何问题都退回规则，并把「谁选的、为什么」记进记录。

**Architecture:** `dct-game` 不依赖 LLM 层：新增模块 `ask.rs`（纯函数：棋盘转文字、候选转大白话、拼提示、解析回答）和一个 `Advisor` trait（`&self`，所以可以放进 `Options` 里复制传递）。`play()` 在选步处按触发条件问 `Advisor`，回答只能是候选里的一个编号，其余一律退回规则。主 crate 里实现 `LlmAdvisor`（包住 `llm::Backend`，带超时），`dct game play` 加 `--ask-model`、`--goal` 两个开关，另加离线回放命令 `dct game ask-bench`，用已记的真实局面测模型。

**Tech Stack:** Rust（workspace：`dct` 主 crate + `crates/dct-game`），`serde_json`，已有的 `src/llm/`（`Backend`、`Prompt`、`complete_with_timeout`、`resolve`）。

**Spec:** `docs/superpowers/specs/2026-10-04-dct-match3-ask-model-design.md`

**对 spec 的一处修正（执行前先改 spec）：** spec §1 的第二个触发「没步可走」去掉。规则一步都选不出时没有候选，模型无从选择，这个触发没有意义。第 1715 关「彩球互换」那一步靠的是配置数据（彩球不标成不是糖）让规则列得出候选，再由 `--ask-model` 的「每一步都问」去选。Task 3 的第一步改 spec。

## Global Constraints

- 提交信息用英文，**不加任何 AI 署名或 Co-Authored-By 行**（用户的长期规则，优先于默认的署名提示）。
- 不推送：用户说「推吧」才推。
- 面向用户的文字用大白话中文，不出现类别编号、分数公式、「LLM / prompt / token」这类词；行列从 1 数。
- dct 代码保持通用：游戏专属的内容（目标、不是糖的颜色、权重）只来自配置文件或命令行，代码里不写任何游戏的东西。
- 每一处用 LLM 的地方都必须有不依赖 LLM 的退路：没配、连不上、超时、回的看不懂，行为与现在完全一样。
- 凭据只发给它自己的地址：不新增任何取凭据的路径，一律走现有的 `llm::resolve::resolve`。
- 不发截图（第一版）。发出去的只有：棋盘字母图、目标一句话、候选的大白话。
- 推送前跑：`cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings`（环境变量 `CARGO_TARGET_DIR=/private/tmp/claude-502/-Users-lei-work-dc-dc-terminal/d52f1f4f-cb49-40cc-ba88-c37403102743/scratchpad/target-199`，`PATH` 要含 `$HOME/.cargo/bin`），和 `cargo +1.99.0 test --workspace --locked`。
- 同一盘棋上已被游戏拒绝的步（`failed`）不会出现在发给模型的候选里。

## Review Focus

1. 模型回「0」「7」（越界）、空字符串、一大段话、只有理由没有编号：划出去的仍然是候选里的一步（退回规则第一名），不崩。→ Task 1 解析测试 + Task 2 合规测试。
2. 候选只有 1 个：不问模型（没有可选的），直接走。→ Task 2。
3. 模型回的是 `failed` 里的步（不可能出现在候选里，但回的编号对不上）：不能因此重复划一个已知没反应的步。→ Task 2。
4. 没配 `[llm]` 时带了 `--ask-model`：打一句大白话说明「没开模型，改用规则」，继续玩，退出码和不带时一样。→ Task 3。
5. 模型调用到达上限（默认 30 次）后：不再问，继续用规则，不停。→ Task 2。

---

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

### Task 2: `play()` 里按触发条件问，合规检查，记录

**Files:**
- Modify: `crates/dct-game/src/play.rs`（`Options` 加字段；选步处接入）
- Modify: `crates/dct-game/src/navigate.rs`（`NavOptions` 加同样的字段，传给 `play`）
- Modify: 所有构造 `Options`/`NavOptions` 的地方：`src/game/cli.rs:128-130`，`play_tests.rs:72,193`，`navigate.rs:151,369,517,701`
- Test: `crates/dct-game/src/play_tests.rs`

**Interfaces:**
- Consumes: Task 1 的 `Advisor`、`AskInput`、`Advice`、`board_text`、`describe`、`describe_move`、`parse_reply` 不用（`Advice.choice` 已经解析好）。
- Produces:
  - `pub struct Options<'a> { pub max_steps: usize, pub dry_run: bool, pub advisor: Option<&'a dyn Advisor>, pub ask_always: bool, pub ask_budget: usize, pub goal: &'a str }`
  - `NavOptions<'a>` 同样加这四个字段。
  - 记录新增字段：`decider`（`"rules"`/`"model"`，每条带候选的记录都有），`ask`（只有问过才有）：`{ "model": …, "raw": …, "reason": …, "asked": [候选下标…], "choice": 候选下标或 null }`。`chosen` 仍是 `candidates` 里的下标。
  - 常量 `pub const ASK_SHOWN: usize = 8;`（最多发几个候选）。

触发规则（写死在 `play()` 里）：`advisor.is_some() && !dry_run && 本次运行已问次数 < ask_budget && 可选候选数 >= 2 && (ask_always || 这盘棋上已有失败的步)`。可选候选 = `cands` 去掉 `failed` 里的步，取前 `ASK_SHOWN` 个。

- [ ] **Step 1: 写失败的测试**

在 `play_tests.rs` 末尾（`Fake`、`Clk`、`profile`、`grid`、`A`、`MOVED` 已在文件里）：

```rust
use crate::ask::{Advice, Advisor, AskInput};
use std::cell::{Cell, RefCell};

struct Say {
    choice: Option<usize>,
    calls: Cell<usize>,
    seen: RefCell<Vec<AskInput>>,
}
impl Say {
    fn new(choice: Option<usize>) -> Say {
        Say { choice, calls: Cell::new(0), seen: RefCell::new(vec![]) }
    }
}
impl Advisor for Say {
    fn pick(&self, i: &AskInput) -> Option<Advice> {
        self.calls.set(self.calls.get() + 1);
        self.seen.borrow_mut().push(AskInput {
            board: i.board.clone(),
            goal: i.goal.clone(),
            candidates: i.candidates.clone(),
            failed: i.failed.clone(),
        });
        Some(Advice { choice: self.choice, reason: "测试".into(), raw: "x".into(), model: "fake".into() })
    }
}
struct Down;
impl Advisor for Down {
    fn pick(&self, _: &AskInput) -> Option<Advice> {
        None
    }
}

fn run_ask(d: &mut Fake, adv: Option<&dyn Advisor>, always: bool, budget: usize, steps: usize) -> (Summary, Vec<Value>) {
    let mut log = vec![];
    let o = Options { max_steps: steps, dry_run: false, advisor: adv, ask_always: always, ask_budget: budget, goal: "清冰" };
    let s = play(d, &mut Clk(0), &profile(3, 4), &o, &mut |v| log.push(v));
    (s, log)
}
fn settled_script() -> Vec<Result<GridRead, DcoError>> {
    vec![Ok(grid(A)), Ok(grid(MOVED)), Ok(grid(MOVED))]
}

#[test]
fn ask_always_lets_the_model_pick_among_candidates_and_records_it() {
    // 先看规则自己会划哪一步
    let mut d0 = Fake::new(settled_script());
    let _ = run_ask(&mut d0, None, false, 30, 1);
    let rules_swipe = d0.swipes[0];

    let say = Say::new(Some(1)); // 选第 2 个候选
    let mut d = Fake::new(settled_script());
    let (_, log) = run_ask(&mut d, Some(&say), true, 30, 1);
    assert_eq!(say.calls.get(), 1);
    assert_ne!(d.swipes[0], rules_swipe, "模型选了第 2 个候选，划的应该和规则第一名不同");
    assert_eq!(log[0]["decider"], "model");
    assert_eq!(log[0]["chosen"], 1);
    assert_eq!(log[0]["ask"]["choice"], 1);
    assert_eq!(log[0]["ask"]["model"], "fake");
    // 发出去的提示里有目标、字母棋盘、编号候选
    let seen = say.seen.borrow();
    assert_eq!(seen[0].goal, "清冰");
    assert!(seen[0].board.contains('A'));
    assert!(seen[0].candidates.len() >= 2);
}

#[test]
fn without_ask_always_the_model_is_not_asked_on_a_clean_board() {
    let say = Say::new(Some(1));
    let mut d = Fake::new(settled_script());
    let (_, log) = run_ask(&mut d, Some(&say), false, 30, 1);
    assert_eq!(say.calls.get(), 0);
    assert_eq!(log[0]["decider"], "rules");
    assert!(log[0].get("ask").is_none());
}

#[test]
fn a_failed_swap_triggers_the_model_and_is_listed_but_not_offered() {
    // 第一划没反应（A 一直不变），第二次选步时这盘棋上已有失败的步：问模型，失败的步在 failed 里、不在候选里
    let mut reads = vec![];
    for _ in 0..40 {
        reads.push(Ok(grid(A)));
    }
    reads.push(Ok(grid(MOVED)));
    reads.push(Ok(grid(MOVED)));
    let say = Say::new(Some(0));
    let mut d = Fake::new(reads);
    let (_, log) = run_ask(&mut d, Some(&say), false, 30, 2);
    assert!(say.calls.get() >= 1);
    let seen = say.seen.borrow();
    assert_eq!(seen[0].failed.len(), 1);
    assert!(!seen[0].candidates.iter().any(|c| *c == seen[0].failed[0]));
    assert_eq!(log[0]["decider"], "rules");
    assert_eq!(log[1]["decider"], "model");
}

#[test]
fn model_that_cannot_decide_or_is_down_falls_back_to_rules() {
    let mut d0 = Fake::new(settled_script());
    let _ = run_ask(&mut d0, None, false, 30, 1);
    let rules_swipe = d0.swipes[0];

    for adv in [&Say::new(None) as &dyn Advisor, &Down] {
        let mut d = Fake::new(settled_script());
        let (s, log) = run_ask(&mut d, Some(adv), true, 30, 1);
        assert_eq!(d.swipes[0], rules_swipe);
        assert_eq!(log[0]["decider"], "rules");
        assert_eq!(s.stop, Stop::StepsDone);
    }
}

#[test]
fn ask_budget_stops_the_questions_but_not_the_game() {
    let say = Say::new(Some(1));
    let mut reads = vec![Ok(grid(A))];
    for _ in 0..6 {
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(A)));
        reads.push(Ok(grid(A)));
    }
    let mut d = Fake::new(reads);
    let (_, log) = run_ask(&mut d, Some(&say), true, 2, 3);
    assert_eq!(say.calls.get(), 2);
    assert_eq!(log.len(), 3);
    assert_eq!(log[2]["decider"], "rules");
}

#[test]
fn a_single_candidate_is_never_worth_asking_about() {
    // 只有一步可走的棋盘：不问
    let one: &[&[u16]] = &[&[1, 1, 2, 3], &[2, 3, 1, 1], &[2, 3, 1, 3]];
    let _ = one; // 具体盘面在实现时用只有一个合法交换的真盘面替换；断言是 calls == 0
    // 见 Step 3 的说明
}
```

最后一个测试要有真实内容：在 `play_tests.rs` 里找到（或构造）一个只有一个合法交换的小盘面（`choose()` 返回长度 1），断言 `say.calls.get() == 0` 且 `log[0]["decider"] == "rules"`。把上面那个占位测试**替换**成这个真测试后再继续；占位不许留下。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-game play_tests::`
Expected: 编译失败（`Options` 没有 `advisor` 等字段）。

- [ ] **Step 3: 写实现**

`play.rs`：

```rust
use crate::ask::{board_text, describe, describe_move, Advisor, AskInput};

/// 最多发给模型几个候选。
pub const ASK_SHOWN: usize = 8;

pub struct Options<'a> {
    pub max_steps: usize,
    pub dry_run: bool,
    /// 规则没把握时问的那个。`None` = 不问（和以前一样）。
    pub advisor: Option<&'a dyn Advisor>,
    /// 每一步都问（`--ask-model`）；否则只在这盘棋上已有失败的步时才问。
    pub ask_always: bool,
    /// 这一次 `play()` 最多问几次。
    pub ask_budget: usize,
    pub goal: &'a str,
}
```

`play()` 里：在现有 `let mut failed ...` 旁加 `let mut asked = 0usize;`。选步处（`let pick = cands.iter().position(...)` 之后、`let Some(pick) = pick else {...}` 之前不动；在 `let chosen = &cands[pick];` 之前）改成：

```rust
        let board_key = canonical(&g.cells);
        let is_failed = |c: &Candidate| failed.iter().any(|(m, cells)| *m == c.mv && *cells == board_key);
        // 发给模型的候选：没失败过的，最多 ASK_SHOWN 个；下标指回 cands
        let offered: Vec<usize> = (0..cands.len()).filter(|&i| !is_failed(&cands[i])).take(ASK_SHOWN).collect();
        let had_failed_here = failed.iter().any(|(_, cells)| *cells == board_key);
        let mut pick = pick;
        let mut ask_rec: Option<Value> = None;
        let mut decider = "rules";
        if let Some(adv) = o.advisor {
            if !o.dry_run && asked < o.ask_budget && offered.len() >= 2 && (o.ask_always || had_failed_here) && pick.is_some() {
                asked += 1;
                let fixed = fixed_ids(&g, &p.fixed_rgb, p.match_de);
                let input = AskInput {
                    board: board_text(&g, &fixed),
                    goal: o.goal.to_string(),
                    candidates: offered.iter().map(|&i| describe(&cands[i])).collect(),
                    failed: failed
                        .iter()
                        .filter(|(_, cells)| *cells == board_key)
                        .map(|(m, _)| describe_move(m))
                        .collect(),
                };
                if let Some(a) = adv.pick(&input) {
                    let chosen_idx = a.choice.filter(|&k| k < offered.len()).map(|k| offered[k]);
                    ask_rec = Some(json!({
                        "model": a.model, "raw": a.raw, "reason": a.reason,
                        "asked": offered, "choice": chosen_idx,
                    }));
                    if let Some(i) = chosen_idx {
                        pick = Some(i);
                        decider = "model";
                    }
                }
            }
        }
```

并把原来的 `let pick = cands.iter().position(...)` 保留作为规则的选择（上面 `let mut pick = pick;` 接它）。把 `let is_failed` 闭包借用 `failed` 的位置安排在 `failed.push` 之前（后面是同一个循环迭代里的不同语句，Rust 借用在闭包最后一次使用后结束，不冲突；若编译器报借用冲突，把 `offered` 的计算放进一个小块 `{ ... }` 里）。

记录里加：`"decider": decider`（`rec` 的 json! 里），以及 `if let Some(a) = ask_rec { rec["ask"] = a; }`。`chosen` 字段用最终的 `pick`。

`navigate.rs`：`NavOptions` 加 `pub advisor: Option<&'a dyn Advisor>, pub ask_always: bool, pub ask_budget: usize, pub goal: &'a str`（`NavOptions<'a>`），`navigate.rs:151` 构造 `Options` 时带上这四个字段。其余构造点（测试、`cli.rs`）补 `advisor: None, ask_always: false, ask_budget: 0, goal: ""`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo +1.99.0 test --workspace --locked`
Expected: 全绿（含旧测试：旧构造点只补了默认字段，行为不变）。

- [ ] **Step 5: 变异检查，然后提交**

分别临时改：把触发条件里 `offered.len() >= 2` 改成 `>= 1`；把 `asked < o.ask_budget` 改成 `true`；把 `chosen_idx` 的 `filter(|&k| k < offered.len())` 去掉（会越界 panic 或选错）；每一个都应该让至少一个测试变红。改回。

```bash
git add crates/dct-game/src src/game/cli.rs
git commit -m "feat(game): ask an advisor to pick among rule candidates when a swap was refused or on request; every failure falls back to rules"
```

---

### Task 3: 接上网关大模型和命令行

**Files:**
- Create: `src/game/advisor.rs`
- Modify: `src/game/mod.rs`（`mod advisor;`）
- Modify: `src/game/cli.rs`（`--ask-model`、`--goal`、建 advisor、传给 Options/NavOptions；`USAGE` 加这两项）
- Modify: `src/cli.rs`（把 `llm_check` 里「读配置 + 建 backend」那一段抽成 `pub fn load_llm_backend() -> Result<Arc<dyn Backend>, LoadLlmError>`，`llm_check` 改用它，行为不变）
- Modify: `src/game/skill.md`（说明卡里加一小节）
- Modify: `docs/superpowers/specs/2026-10-04-dct-match3-ask-model-design.md`（按本计划开头的修正删掉「没步可走」触发）
- Test: `src/game/cli.rs` 的 `parse` 测试；`src/game/advisor.rs` 的单元测试（假 `Backend`）

**Interfaces:**
- Consumes: Task 1 的 `Advisor`/`AskInput`/`Advice`/`prompt_text`/`parse_reply`；`llm::{Backend, Prompt, complete_with_timeout, LlmError}`。
- Produces:
  - `pub struct LlmAdvisor { backend: Arc<dyn Backend>, model: String, timeout: Duration }`，`LlmAdvisor::new(backend, model)`（超时默认 20 秒），`impl Advisor for LlmAdvisor`。
  - `Args` 新增 `pub ask_model: bool`、`pub goal: Option<String>`。

- [ ] **Step 1: 先改 spec**

在 spec 的「1. 什么时候问模型（触发）」里删掉「没步可走」那条，并在末尾加一句：「规则一步都选不出时没有候选，模型无从选择，不属于触发；这类关卡靠配置数据让规则列得出候选（例如不把彩球标成不是糖），再用 `--ask-model` 选。」

- [ ] **Step 2: 写失败的测试**

`src/game/advisor.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Backend, LlmError, Prompt};
    use dct_game::ask::{Advisor, AskInput};
    use std::sync::{Arc, Mutex};

    struct Fixed(Result<String, LlmError>, Mutex<Vec<String>>);
    impl Backend for Fixed {
        fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
            self.1.lock().unwrap().push(p.user.clone());
            self.0.clone()
        }
    }
    fn input() -> AskInput {
        AskInput { board: "A A\nB B".into(), goal: "清冰".into(), candidates: vec!["甲".into(), "乙".into(), "丙".into()], failed: vec![] }
    }
    fn adv(r: Result<String, LlmError>) -> (LlmAdvisor, Arc<Fixed>) {
        let b = Arc::new(Fixed(r, Mutex::new(vec![])));
        (LlmAdvisor::new(b.clone(), "m1".into()), b)
    }

    #[test]
    fn a_valid_reply_becomes_a_zero_based_choice_with_reason() {
        let (a, b) = adv(Ok("2，能清冰".into()));
        let r = a.pick(&input()).unwrap();
        assert_eq!(r.choice, Some(1));
        assert_eq!(r.reason, "能清冰");
        assert_eq!(r.raw, "2，能清冰");
        assert_eq!(r.model, "m1");
        assert!(b.1.lock().unwrap()[0].contains("1) 甲"));
    }

    #[test]
    fn garbage_reply_is_an_answer_with_no_choice() {
        let (a, _) = adv(Ok("随便".into()));
        assert_eq!(a.pick(&input()).unwrap().choice, None);
    }

    #[test]
    fn every_backend_error_means_not_asked() {
        for e in [LlmError::Unavailable, LlmError::Timeout, LlmError::Malformed] {
            let (a, _) = adv(Err(e));
            assert!(a.pick(&input()).is_none(), "{e:?}");
        }
    }
}
```

`src/game/cli.rs` 的测试里加：

```rust
#[test]
fn parse_ask_model_and_goal() {
    let a = parse(&["play".into(), "--ask-model".into(), "--goal".into(), "清掉冰块".into()]).unwrap();
    assert!(a.ask_model);
    assert_eq!(a.goal.as_deref(), Some("清掉冰块"));
    let d = parse(&["play".into()]).unwrap();
    assert!(!d.ask_model);
    assert_eq!(d.goal, None);
    assert!(parse(&["play".into(), "--goal".into()]).is_err());
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo +1.99.0 test --lib game::`
Expected: 编译失败。

- [ ] **Step 4: 写实现**

`src/game/advisor.rs`：

```rust
//! 把「问模型选哪一步」接到 dct 自己的 LLM 连接层上。
//! 凭据怎么取、发给哪台机器，全在 `llm::resolve`，这里不碰。
use crate::llm::{complete_with_timeout, Backend, Prompt};
use dct_game::ask::{parse_reply, prompt_text, Advice, Advisor, AskInput};
use std::sync::Arc;
use std::time::Duration;

pub struct LlmAdvisor {
    backend: Arc<dyn Backend>,
    model: String,
    timeout: Duration,
}

impl LlmAdvisor {
    pub fn new(backend: Arc<dyn Backend>, model: String) -> LlmAdvisor {
        LlmAdvisor { backend, model, timeout: Duration::from_secs(20) }
    }
}

const SYSTEM: &str = "你在帮一个三消游戏的自动玩家选下一步。只能从给出的编号里选，不要自己编走法。";

impl Advisor for LlmAdvisor {
    fn pick(&self, i: &AskInput) -> Option<Advice> {
        let p = Prompt { system: SYSTEM.into(), user: prompt_text(i), max_tokens: 64 };
        // 任何错误都是「没问成」：调用方退回规则
        let raw = complete_with_timeout(self.backend.clone(), p, self.timeout).ok()?;
        let (choice, reason) = parse_reply(&raw, i.candidates.len());
        Some(Advice { choice, reason, raw, model: self.model.clone() })
    }
}
```

`src/cli.rs`：新增

```rust
pub struct LoadedLlm {
    pub backend: std::sync::Arc<dyn crate::llm::Backend>,
    pub model: String,
}

pub enum LoadLlmError {
    /// 没写 `[llm]`：正常状态。
    NotEnabled,
    Problem(crate::llm::resolve::ResolveError),
}

pub fn load_llm_backend() -> Result<LoadedLlm, LoadLlmError> {
    // 原样搬 llm_check 里从 `let socket = ...` 到 resolve 成功的那一段；
    // model 取 llm.model.clone().unwrap_or_else(|| llm.provider.clone())
}
```

`llm_check` 改成调用 `load_llm_backend()`，输出文字不变（`NotEnabled` 打原来的「还没开」，`Problem(e)` 打原来的「连不上」）。**搬运时一个字符的输出都不许变**，现有 `llm check` 的测试必须继续通过。

`src/game/cli.rs`：`Args` 加 `ask_model: bool`（默认 false）、`goal: Option<String>`；`parse` 里加

```rust
            "--ask-model" => a.ask_model = true,
            "--goal" => a.goal = Some(it.next().ok_or("--goal 后面要写目标，比如 --goal \"清掉冰块\"")?.clone()),
```

`USAGE` 末尾加 ` [--ask-model] [--goal "清掉冰块"]`。`run_parsed` 里，在 `let clock = SystemClock;` 之后：

```rust
    let advisor: Option<super::advisor::LlmAdvisor> = if a.ask_model {
        match crate::cli::load_llm_backend() {
            Ok(l) => Some(super::advisor::LlmAdvisor::new(l.backend, l.model)),
            Err(crate::cli::LoadLlmError::NotEnabled) => {
                println!("没开大模型，这次只用规则玩。");
                None
            }
            Err(crate::cli::LoadLlmError::Problem(_)) => {
                println!("大模型连不上，这次只用规则玩。可以先运行 dct llm check 看原因。");
                None
            }
        }
    } else {
        None
    };
    let goal = a.goal.clone().unwrap_or_else(|| "尽量多消".into());
    let adv_ref: Option<&dyn dct_game::ask::Advisor> = advisor.as_ref().map(|x| x as &dyn dct_game::ask::Advisor);
```

并把 `Options`/`NavOptions` 构造改成 `advisor: adv_ref, ask_always: a.ask_model, ask_budget: 30, goal: &goal`。`text.rs` 的 `step_line`：当记录里 `decider == "model"` 时在句尾加「（大模型选的）」；这是 `text.rs` 里 `nav`/`step` 现有测试风格的一个小测试：`step_line` 对带 `"decider":"model"` 的记录包含「大模型选的」，对 `"rules"` 不包含。

`src/game/skill.md` 加一小节「玩不动的时候」：一两句，写 `--ask-model` 和 `--goal` 是什么、没配大模型也能玩。全中文大白话，不出现英文术语。

- [ ] **Step 5: 跑全部测试和 clippy，提交**

Run: `cargo +1.99.0 test --workspace --locked` 和 `cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings`
Expected: 全绿、无警告。

手工冒烟（不需要游戏）：`dct game play --ask-model --dry-run` 在没配 `[llm]` 的环境里，应打印「没开大模型，这次只用规则玩。」后照常（读不到棋盘就按原来的方式停），退出码和不带 `--ask-model` 时一样。

```bash
git add src crates docs
git commit -m "feat(game): --ask-model and --goal; advisor over the gateway model with rules fallback"
```

---

### Task 4: 离线回放：用真实局面测模型（验收第 1 条）

**Files:**
- Create: `src/game/bench.rs`
- Modify: `src/game/mod.rs`、`src/game/cli.rs`（子命令 `dct game ask-bench <记录文件> [--limit N]`）
- Test: `src/game/bench.rs`

**Interfaces:**
- Consumes: Task 3 的 `load_llm_backend`、`LlmAdvisor`；Task 1 的 `describe`、`AskInput`；记录文件（`~/.dct/games/log/*.jsonl`）里每条带 `candidates`（含 `a`、`b`、`features`）、`cells`、`classes`、`game`、`chosen`、`outcome` 的行。
- Produces: `pub fn replay(lines: &[Value], adv: &dyn Advisor, goal_of: &dyn Fn(&str) -> String, limit: usize) -> Report`；`pub struct Report { pub asked: usize, pub answered: usize, pub valid: usize, pub picked_rules_first: usize, pub chance_first: f64, pub avg_ms: u64 }`。

做法：从记录里取 `candidates` 至少 3 个、`dry_run` 不是 true 的行，每行把候选前 5 个打乱顺序（固定种子，用一个简单的线性同余，别引入新依赖），拼成 `AskInput` 问一次，统计「回答成合法编号的次数」「选中规则第一名的次数」「随机期望次数 = Σ 1/候选数」「平均耗时」。棋盘字母图用记录里的 `cells` 和 `classes`（`fixed` 取空：记录里没存哪些是不是糖，回放只比较选择，不依赖它；打印报告时明说）。

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use dct_game::ask::{Advice, Advisor, AskInput};
    use serde_json::json;

    struct First;
    impl Advisor for First {
        fn pick(&self, _: &AskInput) -> Option<Advice> {
            Some(Advice { choice: Some(0), reason: String::new(), raw: "1".into(), model: "t".into() })
        }
    }

    fn rec(n: usize, dry: bool) -> Value {
        let cands: Vec<Value> = (0..n).map(|k| json!({"a":[0,k],"b":[1,k],"score":1.0,
            "features":{"cleared":3,"cascade":0,"striped":0,"wrapped":0,"bomb":0,"triggered":0,"special_swap":false,"lowest_row":1}})).collect();
        json!({"game":"g","cells":[[1,1],[2,2]],"classes":[{"id":1,"count":2},{"id":2,"count":2}],
               "candidates": cands, "chosen":0, "dry_run": dry, "outcome": "moved"})
    }

    #[test]
    fn replay_skips_dry_runs_and_tiny_candidate_lists_and_counts_the_rest() {
        let lines = vec![rec(5, false), rec(5, true), rec(2, false), rec(4, false)];
        let r = replay(&lines, &First, &|_| "尽量多消".into(), 10);
        assert_eq!(r.asked, 2);
        assert_eq!(r.answered, 2);
        assert_eq!(r.valid, 2);
        // 候选是打乱的，模型总选第一个，所以「选中规则第一名」只可能是碰巧
        assert!(r.picked_rules_first <= 2);
        // 随机期望：5 个候选 1/5，4 个候选 1/4（候选最多取前 5 个）
        assert!((r.chance_first - (0.2 + 0.25)).abs() < 1e-9);
    }

    #[test]
    fn replay_respects_the_limit() {
        let lines: Vec<Value> = (0..10).map(|_| rec(5, false)).collect();
        assert_eq!(replay(&lines, &First, &|_| String::new(), 3).asked, 3);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test --lib game::bench`
Expected: 编译失败。

- [ ] **Step 3: 写实现**

```rust
//! 离线回放：拿以前真机记下的局面问模型，看它选得怎么样。不碰游戏、不碰手机。
use dct_game::ask::{board_text, describe, Advisor, AskInput};
use dct_game::board::GridRead;
use dct_game::choose::Candidate;
use dct_game::sim::{Move, Outcome};
use serde_json::Value;

#[derive(Debug, Default)]
pub struct Report {
    pub asked: usize,
    pub answered: usize,
    pub valid: usize,
    pub picked_rules_first: usize,
    pub chance_first: f64,
    pub avg_ms: u64,
}

fn candidate(v: &Value) -> Option<Candidate> {
    let p = |x: &Value| Some((x[0].as_u64()? as usize, x[1].as_u64()? as usize));
    let f = &v["features"];
    let n = |k: &str| f[k].as_u64().unwrap_or(0) as usize;
    Some(Candidate {
        mv: Move { a: p(&v["a"])?, b: p(&v["b"])? },
        score: v["score"].as_f64().unwrap_or(0.0),
        features: Outcome {
            cleared: n("cleared"), cascade: n("cascade"), striped: n("striped"), wrapped: n("wrapped"),
            bomb: n("bomb"), triggered: n("triggered"),
            special_swap: f["special_swap"].as_bool().unwrap_or(false), lowest_row: n("lowest_row"),
        },
    })
}

pub fn replay(lines: &[Value], adv: &dyn Advisor, goal_of: &dyn Fn(&str) -> String, limit: usize) -> Report {
    let mut r = Report::default();
    let mut seed: u64 = 7;
    let mut total_ms = 0u128;
    for rec in lines {
        if r.asked >= limit {
            break;
        }
        if rec["dry_run"].as_bool() == Some(true) {
            continue;
        }
        let cands: Vec<Candidate> = rec["candidates"].as_array().map(|a| a.iter().filter_map(candidate).take(5).collect()).unwrap_or_default();
        if cands.len() < 3 {
            continue;
        }
        let Ok(g) = serde_json::from_value::<GridRead>(serde_json::json!({
            "rows": rec["cells"].as_array().map_or(0, Vec::len),
            "cols": rec["cells"][0].as_array().map_or(0, Vec::len),
            "cells": rec["cells"], "odd": rec["odd"], "classes": rec["classes"],
        })) else {
            continue;
        };
        // 固定种子的洗牌：同一份记录每次回放顺序一样，结果可比
        let mut order: Vec<usize> = (0..cands.len()).collect();
        for i in (1..order.len()).rev() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            order.swap(i, (seed >> 33) as usize % (i + 1));
        }
        let input = AskInput {
            board: board_text(&g, &[]),
            goal: goal_of(rec["game"].as_str().unwrap_or("")),
            candidates: order.iter().map(|&k| describe(&cands[k])).collect(),
            failed: vec![],
        };
        r.asked += 1;
        r.chance_first += 1.0 / cands.len() as f64;
        let t = std::time::Instant::now();
        let a = adv.pick(&input);
        total_ms += t.elapsed().as_millis();
        if let Some(a) = a {
            r.answered += 1;
            if let Some(k) = a.choice {
                r.valid += 1;
                if order[k] == 0 {
                    r.picked_rules_first += 1;
                }
            }
        }
    }
    if r.asked > 0 {
        r.avg_ms = (total_ms / r.asked as u128) as u64;
    }
    r
}
```

`cli.rs` 里 `dct game ask-bench <文件...> [--limit 40]`：读文件的每一行 JSON，调 `load_llm_backend()`（没开就打「没开大模型，没法测」退出 1），建 `LlmAdvisor`，`goal_of` 用 `|_| "尽量多消".into()`（回放不知道每关的目标；报告里写明），打印大白话报告：

```
问了 N 个局面，回答成合法编号的 V 个，选中规则第一名的 K 个（乱选大约 C 个），平均每次 M 毫秒。
说明：回放里没有「哪些格子不是糖」和「这一关的目标」，所以这个数只说明它比乱选强多少，不说明它选得对不对。
```

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked` 与 clippy。
Expected: 全绿、无警告。

- [ ] **Step 5: 提交；然后做验收第 1 条**

```bash
git add src
git commit -m "feat(game): ask-bench replays recorded boards against the configured model"
```

验收（需要网关密钥，已在 `~/.dct/secrets.toml` 的 `qwen`，**不要打印**；需要 `dct llm check` 先通）：

```bash
dct game ask-bench ~/.dct/games/log/2026-10-03.jsonl ~/.dct/games/log/2026-10-04.jsonl --limit 40
```

把结果和本机小模型 Qwen3-VL-4B（40 个局面里选中规则第一名 13 次，乱选约 8 次，平均约 1 秒）并排写进 spec 的「实测依据」。这是写给 spec 的数字，别写成「模型选得好」——这个回放不知道目标。

---

## Self-Review

1. **Spec 覆盖：** 触发（没反应后 / `--ask-model`）→ Task 2；提示只发文字、失败清单、目标来源 → Task 1、3（目标来源：命令行 `--goal`；dcv 那一档等 dc-vault 格式，spec 已说明先不做）；合规检查、上限、超时 → Task 2（上限）、3（超时 20 秒）；记录字段 → Task 2；dcv 学习 → spec 明确「等格式」，本计划不做；离线对比 → Task 4。「没步可走」触发已按开头的修正去掉。
2. **占位扫描：** Task 2 里「只有一个候选」那个测试，Step 1 已写明占位必须替换成真盘面再继续（实现者要在 `play_tests.rs` 里找一个 `choose()` 返回 1 个候选的盘面；若没有现成的就用 `Fake` + 一个 2×3 小盘面构造）。其余步骤都有完整代码。
3. **类型一致：** `AskInput`/`Advice`/`Advisor` 在 Task 1 定义，Task 2/3/4 的用法一致（`Advice.choice: Option<usize>` 0 起；`Options.advisor: Option<&dyn Advisor>`）。`Outcome` 需 `Default`（Task 1 Step 1 已写）。
4. **Review Focus：** 五条分别落在 Task 1（解析）、Task 2（单候选、预算、失败步）、Task 3（没配 `[llm]`）。
