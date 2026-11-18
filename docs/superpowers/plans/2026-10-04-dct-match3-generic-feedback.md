# dct 玩三消：通用反馈机制 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `dct game play` 不靠任何游戏知识，也能发现自己错了：被拒绝的格子在棋盘变化前不再碰、发给模型的候选只写看得见的事实、每一步记下预测和实际、读目标数字的前后变化、连续没进展时自动问模型；并让 `ask-bench` 认带答案的题库。

**Architecture:** 全部改在 `crates/dct-game`（`play.rs`、`ask.rs`、`dco.rs` 不动）和 `src/game/{bench,text}.rs`。`play()` 里新增三份循环内状态：`locked`（被拒绝的格子）、`progress_prev`（上一步后读到的数字）、`no_progress`（连着没下降的步数）。新字段全部是追加到记录里，旧记录读者不受影响。

**Tech Stack:** Rust（workspace `dct` + `crates/dct-game`），`serde_json`，已有的 `lab::delta_e`、`Dco::see_text`（OCR）。

**Spec:** `docs/superpowers/specs/2026-10-04-dct-match3-generic-feedback-design.md`

**对 spec 的两处收窄（执行前知悉）：**
1. spec §3 说「具体哪些字段可靠由一张小表决定」。第一版不做表：直接从发给模型的句子里去掉「引爆 N 颗特殊糖」（它靠 `odd` 标记猜，笼子会触发），并在提示里加一句「标记不一定准」。
2. spec §6 的「经验」格式这一轮不做（没有读写它的代码，只会是空头格式）；只做题库格式和 `ask-bench` 认它。

## Global Constraints

- 提交信息用英文，**不加任何 AI 署名或 Co-Authored-By 行**（用户的长期规则，优先于默认的署名提示）。不推送，用户说「推吧」才推。
- 面向用户的文字用大白话中文，不出现类别编号、分数公式、`OCR`/`LLM`/`prompt` 这类词；行列从 1 数。
- dct 代码保持通用：任何新增逻辑都要能回答「换一个别的方格游戏还成立吗」；游戏专属内容（彩球加分、笼子是什么、各关目标、权重数值）只来自配置或 dcv，不写进代码。
- 每一处新逻辑都不能改变「没配模型 / 没有 OCR / 假 dco 不支持 see_text」时的现有行为：读不到数字就是 `null`，不报错、不停。
- 新增的记录字段只追加，不改已有字段的含义。
- 推送前跑：`cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings` 和 `cargo +1.99.0 test --workspace --locked`（`PATH` 含 `$HOME/.cargo/bin`，`CARGO_TARGET_DIR=/private/tmp/claude-502/-Users-lei-work-dc-dc-terminal/d52f1f4f-cb49-40cc-ba88-c37403102743/scratchpad/target-199`）。已知偶发失败：`daemon::web_tests::enabling_starts_a_listener_and_disabling_stops_it`、`session::tests::recovering_from_a_failure_after_real_input_still_does_not_count`，重跑。

## Review Focus

1. 所有候选都被「不可选格子」排除时：停下并说「划了几次画面都没有变化」，不死循环、不崩。→ Task 1。
2. 一步成功后「不可选格子」必须全部解除，否则一次拒绝会永久封住棋盘一角。→ Task 1。
3. `see_text` 读不到、读到的数字个数前后不一样（OCR 漏了一个）：`progress` 为 `null`，停滞计数**不动**（既不加也不清）。→ Task 4、5。
4. 颜色信息缺失（`rgb` 为 `None`）：`observed_changed` 为 `null`，不当作 0。→ Task 3。
5. 带答案的题库里 `good` / `bad` 的编号越界或为空：该行只统计「问了几个」，不 panic。→ Task 6。

---

### Task 1: 被拒绝的格子在棋盘变化前不再碰；连着没反应的上限放宽到 5

**Files:**
- Modify: `crates/dct-game/src/play.rs`
- Test: `crates/dct-game/src/play_tests.rs`

**Interfaces:**
- Consumes: `play()` 现有的 `failed: Vec<(Move, Vec<Vec<u16>>)>`、`streak`、`cands`、`offered`、`pick`。
- Produces: `pub const MAX_REFUSED_IN_A_ROW: usize = 5;`；循环内状态 `locked: Vec<(usize, usize)>`。

- [ ] **Step 1: 写失败的测试**

在 `play_tests.rs` 末尾（`Fake`、`Clk`、`profile`、`grid`、`A`、`MOVED` 已在文件里）：

```rust
#[test]
fn after_a_refused_swap_no_later_candidate_touches_its_two_cells() {
    // 一直读到同一张盘（没变化）：第一步被拒绝以后，第二步选的步不碰第一步的两个格子
    let mut reads = vec![];
    for _ in 0..80 {
        reads.push(Ok(grid(A)));
    }
    let mut d = Fake::new(reads);
    let (_, log) = run(&mut d, 3, false);
    let pos = |l: &Value, k: &str| (l["candidates"][l["chosen"].as_u64().unwrap() as usize][k].clone());
    if log.len() >= 2 {
        let (a0, b0) = (pos(&log[0], "a"), pos(&log[0], "b"));
        let (a1, b1) = (pos(&log[1], "a"), pos(&log[1], "b"));
        for p in [&a1, &b1] {
            assert!(*p != a0 && *p != b0, "第二步碰了被拒绝的格子：{log:?}");
        }
    }
}

// 6 行 4 列，每行一个互不相干的三连换法（交替用 2/4、1/3，避免竖向连线）
const MANY: &[&[u16]] = &[
    &[1, 1, 2, 1],
    &[3, 3, 4, 3],
    &[1, 1, 2, 1],
    &[3, 3, 4, 3],
    &[1, 1, 2, 1],
    &[3, 3, 4, 3],
];

#[test]
fn five_refusals_in_a_row_stop_the_run_not_two() {
    let mut reads = vec![];
    for _ in 0..400 {
        reads.push(Ok(grid(MANY)));
    }
    let mut d = Fake::new(reads);
    let mut log = vec![];
    let s = play(&mut d, &mut Clk(0), &profile(6, 4), &Options { max_steps: 20, dry_run: false, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |v| log.push(v));
    assert_eq!(s.stop, Stop::Stuck);
    assert_eq!(d.swipes.len(), 5, "应该试满 5 次才停");
}

#[test]
fn a_move_that_works_unlocks_the_cells() {
    // 第一步没反应（锁住它的两格），第二步（别处）成功 → 解除；第三步又可以碰第一步的格子
    let mut reads = vec![];
    for _ in 0..40 {
        reads.push(Ok(grid(MANY)));
    }
    // 第二次划：变成另一张盘并落定
    let moved: &[&[u16]] = &[&[1, 1, 1, 1], &[3, 3, 4, 3], &[1, 1, 2, 1], &[3, 3, 4, 3], &[1, 1, 2, 1], &[3, 3, 4, 3]];
    reads.push(Ok(grid(moved)));
    reads.push(Ok(grid(moved)));
    for _ in 0..400 {
        reads.push(Ok(grid(MANY)));
    }
    let mut d = Fake::new(reads);
    let mut log = vec![];
    let _ = play(&mut d, &mut Clk(0), &profile(6, 4), &Options { max_steps: 3, dry_run: false, advisor: None, ask_always: false, ask_budget: 0, goal: "" }, &mut |v| log.push(v));
    let outcomes: Vec<&str> = log.iter().map(|l| l["outcome"].as_str().unwrap()).collect();
    assert_eq!(&outcomes[..2], ["no_change", "moved"], "{outcomes:?}");
}
```

测试里的脚本依赖 `Fake` 的读盘次序（`settle` 要读多次才下结论）。如果 `a_move_that_works_unlocks_the_cells` 的脚本和真实读盘次数对不上，按 `a_move_that_works_resets_the_unmoved_counter`（`play_tests.rs` 里已有）的做法调整读数条数，**断言保持不变**；若调整了请在报告里说明。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-game play_tests::five_refusals play_tests::after_a_refused play_tests::a_move_that_works_unlocks`
Expected: `five_refusals…` 失败（现在 2 次就停）。

- [ ] **Step 3: 写实现**

`play.rs` 顶部常量区加：

```rust
/// 连着最多几次「划了没反应」才停。被拒绝的格子在棋盘变化前不再被选，所以多试几次不会重复同一处。
pub const MAX_REFUSED_IN_A_ROW: usize = 5;
```

`play()` 里 `let mut failed ...` 旁加 `let mut locked: Vec<(usize, usize)> = Vec::new();`。

把选步处的

```rust
let pick = cands.iter().position(|c| !failed.iter().any(|(m, cells)| *m == c.mv && *cells == canonical(&g.cells)));
```

改成（先算 `board_key`，下面问模型那块里已有的 `let board_key` 删掉复用这个）：

```rust
let board_key = canonical(&g.cells);
let blocked = |c: &Candidate| {
    failed.iter().any(|(m, cells)| *m == c.mv && *cells == board_key) || locked.contains(&c.mv.a) || locked.contains(&c.mv.b)
};
let pick = cands.iter().position(|c| !blocked(c));
```

问模型那块里 `offered` 的计算改成 `(0..cands.len()).filter(|&i| !blocked(&cands[i])).take(ASK_SHOWN).collect()`（`is_failed` 闭包删掉）。`had_failed_here` 保持。

两个「没反应」分支（`Settle::Settled(next) if same(&next, &g)` 和 `Settle::NoChange`）里，在 `failed.push(...)` 之后各加：

```rust
locked.push(chosen.mv.a);
locked.push(chosen.mv.b);
```

并把 `if streak >= 2 {` 改成 `if streak >= MAX_REFUSED_IN_A_ROW {`（两处）。`Settle::Settled(next)`（成功那支）里 `failed.clear();` 旁加 `locked.clear();`。

借用冲突时（`blocked` 闭包借着 `failed`/`locked`，后面要 `push`）：把 `blocked` 的使用都放在 `pick` 和 `offered` 计算完成之后，闭包最后一次使用后借用自然结束；若编译器仍报错，把两处计算包进一个 `{ ... }` 块。

- [ ] **Step 4: 跑全部 dct-game 测试**

Run: `cargo +1.99.0 test -p dct-game`
Expected: 全绿。已有的两个「连着两次就停」的测试（`play_tests.rs` 里断言 `Stop::Stuck` 且 `no_change` 数为 2 的那几处，例如约 107、371、578 行）会因上限改成 5 而失败：把它们的断言改成新的上限（`MAX_REFUSED_IN_A_ROW`），**不改它们要证明的事**（比如「编号被打乱的没变化画面仍判没反应」），并在报告里列出改了哪几个。

- [ ] **Step 5: 变异检查，然后提交**

临时把 `locked.contains(&c.mv.b)` 删掉 → `after_a_refused…` 或相关测试应变红；把 `locked.clear()` 删掉 → `a_move_that_works_unlocks…` 应变红；把常量改成 2 → `five_refusals…` 变红。每次改回。

```bash
git add crates/dct-game/src/play.rs crates/dct-game/src/play_tests.rs
git commit -m "feat(game): cells of a refused swap stay off-limits until the board changes; five refusals in a row before stopping"
```

---

### Task 2: 发给模型的候选只写看得见的事实

**Files:**
- Modify: `crates/dct-game/src/ask.rs`
- Test: 同文件的 `tests` 模块

**Interfaces:**
- Consumes: `describe(&Candidate)`、`prompt_text(&AskInput)`。
- Produces: `describe` 不再输出「引爆 N 颗特殊糖」；`prompt_text` 在候选之前加一句「棋盘上的特殊标记不一定准」。

- [ ] **Step 1: 改测试（先写成会失败的样子）**

把现有 `describe_counts_rows_and_columns_from_one_and_names_what_happens` 的期望改成不含「引爆」：

```rust
    #[test]
    fn describe_counts_rows_and_columns_from_one_and_names_only_what_can_be_seen() {
        let f = Outcome { cleared: 7, bomb: 1, triggered: 2, ..Default::default() };
        let s = describe(&cand((0, 3), (1, 3), f));
        assert_eq!(s, "第 1 行第 4 列和第 2 行第 4 列互换，消 7 颗，做出彩色炸弹");
        assert!(!s.contains("引爆"), "引爆靠特殊标记猜，不可靠，不能发给模型");
    }
```

并在 `prompt_lists_numbered_candidates_goal_and_failed_swaps` 里加断言：`assert!(p.contains("特殊标记不一定准"));`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-game ask::`
Expected: 这两个测试失败。

- [ ] **Step 3: 写实现**

`describe` 里删掉 `if f.triggered > 0 { ... }` 整段。`prompt_text` 里，在「下面是可以走的几步……」那行之前加：

```rust
    s += "提醒：棋盘上看起来像特殊糖的标记不一定准，不要因为它就选某一步。\n";
```

（要含「特殊标记不一定准」这几个字：用 `"提醒：棋盘上的特殊标记不一定准，不要因为它就选某一步。\n"`，以测试断言为准。）

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked`
Expected: 全绿。`src/game/bench.rs` 里用 `describe` 的测试若断言了「引爆」就同步改。

- [ ] **Step 5: 提交**

```bash
git add crates/dct-game/src/ask.rs src/game/bench.rs
git commit -m "fix(game): do not tell the model about triggered special candies (a guess from a marker that cages also set); warn that markers may be wrong"
```

---

### Task 3: 记录预测和实际

**Files:**
- Modify: `crates/dct-game/src/play.rs`
- Test: `crates/dct-game/src/play_tests.rs`

**Interfaces:**
- Consumes: `GridRead.classes[].rgb: Option<[u8; 3]>`（`board.rs` 的 `ClassInfo`），`crate::lab::delta_e([u8;3],[u8;3]) -> f64`。
- Produces: `pub(crate) fn changed_cells(a: &GridRead, b: &GridRead) -> Option<usize>`；记录字段 `predicted_cleared`（划之前，所有非 dry-run 的带候选记录都有）和 `observed_changed`（划之后，数字或 `null`）。常量 `COLOUR_SAME_DE: f64 = 12.0`。

- [ ] **Step 1: 写失败的测试**

```rust
fn grid_rgb(cells: &[&[u16]], rgbs: &[(u16, [u8; 3])]) -> GridRead {
    let rows = cells.len();
    let cols = cells[0].len();
    let classes: Vec<Value> = rgbs
        .iter()
        .map(|(i, c)| json!({"id": i, "count": cells.iter().flat_map(|r| r.iter()).filter(|&&x| x == *i).count(), "rgb": c}))
        .collect();
    serde_json::from_value(json!({"rows": rows, "cols": cols, "cells": cells, "odd": vec![vec![false; cols]; rows], "classes": classes})).unwrap()
}

#[test]
fn changed_cells_compares_colours_not_class_ids() {
    // 类别号互换但颜色没变 → 0；一格换了颜色 → 1
    let a = grid_rgb(&[&[1, 2], &[2, 1]], &[(1, [200, 30, 30]), (2, [30, 30, 200])]);
    let b = grid_rgb(&[&[7, 8], &[8, 7]], &[(7, [200, 30, 30]), (8, [30, 30, 200])]);
    assert_eq!(changed_cells(&a, &b), Some(0));
    let c = grid_rgb(&[&[7, 8], &[8, 8]], &[(7, [200, 30, 30]), (8, [30, 30, 200])]);
    assert_eq!(changed_cells(&a, &c), Some(1));
}

#[test]
fn changed_cells_is_none_when_colours_are_missing() {
    let a = grid(A); // 没有 rgb
    assert_eq!(changed_cells(&a, &a), None);
}

#[test]
fn records_carry_predicted_and_observed() {
    let mut d = Fake::new(settled_script());
    let (_, log) = run(&mut d, 1, false);
    assert!(log[0]["predicted_cleared"].as_u64().unwrap() >= 3);
    assert!(log[0].get("observed_changed").is_some()); // 假盘没有 rgb → null，但字段在
}
```

（`settled_script()` 是 `play_tests.rs` 里已有的辅助函数，返回 `[A, MOVED, MOVED]` 的读数。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-game play_tests::changed_cells play_tests::records_carry`
Expected: 编译失败（`changed_cells` 未定义）。

- [ ] **Step 3: 写实现**

`play.rs`：

```rust
/// 同一格划前划后的颜色相差不到这个就当没变（dco 的类别号每次读都会变，所以比颜色，不比编号）。
const COLOUR_SAME_DE: f64 = 12.0;

fn rgb_of(g: &GridRead, id: u16) -> Option<[u8; 3]> {
    g.classes.iter().find(|c| c.id == id).and_then(|c| c.rgb)
}

/// 划前划后有几格颜色变了。行列数不同或任何一格拿不到颜色就是 `None`（不能当 0）。
pub(crate) fn changed_cells(a: &GridRead, b: &GridRead) -> Option<usize> {
    if a.rows != b.rows || a.cols != b.cols {
        return None;
    }
    let mut n = 0;
    for (ra, rb) in a.cells.iter().zip(&b.cells) {
        for (ca, cb) in ra.iter().zip(rb) {
            let (x, y) = (rgb_of(a, *ca)?, rgb_of(b, *cb)?);
            if crate::lab::delta_e(x, y) > COLOUR_SAME_DE {
                n += 1;
            }
        }
    }
    Some(n)
}
```

记录里：构造 `rec` 时加 `"predicted_cleared": chosen.features.cleared + chosen.features.cascade,`。三处结果里写 `observed_changed`：
- `Settle::Settled(next) if same(&next, &g)` 和 `Settle::NoChange(_)`：`rec["observed_changed"] = json!(0);`
- `Settle::Settled(next)`（成功）：`rec["observed_changed"] = json!(changed_cells(&g, &next));`（`Option<usize>` 序列化成数字或 `null`）。

`ClassInfo`、`rgb` 字段若 `board.rs` 里不是 `pub`，改成 `pub`（只改可见性）。

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked`
Expected: 全绿。

- [ ] **Step 5: 变异检查，然后提交**

把 `COLOUR_SAME_DE` 改成 0.0 → `changed_cells_compares_colours…` 的第一个断言应变红；把 `?` 换成 `.unwrap_or([0,0,0])` → `…is_none_when_colours_are_missing` 变红。改回。

```bash
git add crates/dct-game/src
git commit -m "feat(game): record predicted_cleared and observed_changed for every step (compared by colour, not class id)"
```

---

### Task 4: 读进度数字

**Files:**
- Modify: `crates/dct-game/src/play.rs`
- Test: `crates/dct-game/src/play_tests.rs`

**Interfaces:**
- Consumes: `Dco::see_text(&mut self, &Profile) -> Result<Seen, DcoError>`（默认返回 unsupported）；`Seen.elements: Vec<Element{id,text}>`。
- Produces: `pub(crate) fn numbers(seen: &Seen) -> Vec<u64>`（只取**整条文字全是数字**的元素，按出现顺序）；记录字段 `progress`：`{"before": [..]|null, "after": [..]|null}`；循环状态 `progress_prev: Option<Vec<u64>>`。

- [ ] **Step 1: 写失败的测试**

`Fake` 要支持 `see_text`：给 `Fake` 加字段 `texts: VecDeque<Vec<&'static str>>`（默认空），并实现

```rust
    fn see_text(&mut self, _: &Profile) -> Result<Seen, DcoError> {
        match self.texts.pop_front() {
            Some(t) => Ok(Seen {
                snapshot_id: "s".into(),
                observation_id: None,
                elements: t.iter().enumerate().map(|(i, s)| crate::screen::Element { id: format!("e{}", i + 1), text: (*s).into() }).collect(),
            }),
            None => Err(DcoError { code: "unsupported".into(), message: "x".into() }),
        }
    }
```

（`Fake::new` 里 `texts: VecDeque::new()`；已有测试不受影响。）测试：

```rust
#[test]
fn numbers_keeps_only_whole_number_elements_in_order() {
    let seen = Seen {
        snapshot_id: "s".into(),
        observation_id: None,
        elements: ["1716/♥5", "38", "x", "122", " 7 "].iter().enumerate().map(|(i, s)| crate::screen::Element { id: format!("e{i}"), text: (*s).into() }).collect(),
    };
    assert_eq!(numbers(&seen), vec![38, 122, 7]);
}

#[test]
fn progress_is_recorded_before_and_after_each_step() {
    let mut d = Fake::new(settled_script());
    // 一次循环前读一回（起始），每步划完读一回
    d.texts = vec![vec!["50", "30"], vec!["49", "30"]].into();
    let (_, log) = run(&mut d, 1, false);
    assert_eq!(log[0]["progress"]["before"], json!([50, 30]));
    assert_eq!(log[0]["progress"]["after"], json!([49, 30]));
}

#[test]
fn progress_is_null_when_the_dco_cannot_read_text() {
    let mut d = Fake::new(settled_script()); // texts 为空 → unsupported
    let (s, log) = run(&mut d, 1, false);
    assert!(log[0]["progress"]["before"].is_null());
    assert!(log[0]["progress"]["after"].is_null());
    assert_eq!(s.stop, Stop::StepsDone);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-game play_tests::numbers play_tests::progress`
Expected: 编译失败（`numbers` 未定义）。

- [ ] **Step 3: 写实现**

```rust
/// 画面上整条文字就是一个数字的元素（顶部的目标数、步数……），按出现顺序。
/// 「1716/♥5」这种夹着别的字的不要。哪个数是目标，这里不猜；只记下来。
pub(crate) fn numbers(seen: &Seen) -> Vec<u64> {
    seen.elements.iter().filter_map(|e| e.text.trim().parse::<u64>().ok()).collect()
}
```

`play()`：循环前（`let stop = loop {` 之前）`let mut progress_prev: Option<Vec<u64>> = if o.dry_run { None } else { dco.see_text(p).ok().map(|s| numbers(&s)) };`。

两种结果分支（成功 / 没反应）写记录前各读一次：

```rust
let progress_after = dco.see_text(p).ok().map(|s| numbers(&s));
rec["progress"] = json!({ "before": progress_prev, "after": progress_after });
progress_prev = progress_after.clone();
```

（放进一个小闭包或内联；`stopped` 类的分支不读。`dry_run` 分支直接 `break`，不读。）

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked`
Expected: 全绿（其它假 dco 不支持 `see_text`，字段是 `null`，行为不变）。

- [ ] **Step 5: 变异检查，然后提交**

把 `numbers` 的 `trim()` 去掉 → `numbers_keeps_only…` 变红（`" 7 "`）；把 `progress_prev = progress_after.clone()` 删掉 → `progress_is_recorded…` 在多步时的 before 会错（若当前测试只有一步没抓到，加一个两步的断言）。改回。

```bash
git add crates/dct-game/src
git commit -m "feat(game): record the whole-number texts on screen before and after each step as progress"
```

---

### Task 5: 停滞检测

**Files:**
- Modify: `crates/dct-game/src/play.rs`、`src/game/text.rs`
- Test: `crates/dct-game/src/play_tests.rs`、`src/game/text.rs` 的测试

**Interfaces:**
- Consumes: Task 4 的 `progress_prev` / `progress_after`；Task 2/3 的记录。
- Produces: 常量 `pub const STALL_STEPS: usize = 6;`；`fn decreased(before: &[u64], after: &[u64]) -> bool`（个数相同且至少一项变小）；循环状态 `no_progress: usize`；问模型的触发条件加上「停滞」；记录字段 `stalled: true`（只在停滞时才有）；`step_line` 在 `stalled` 时加「；这几步看起来没有进展」。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn decreased_needs_same_length_and_a_smaller_number() {
    assert!(decreased(&[50, 30], &[49, 30]));
    assert!(!decreased(&[50, 30], &[50, 30]));
    assert!(!decreased(&[50, 30], &[51, 30]));
    assert!(!decreased(&[50, 30], &[49])); // 个数不一样（OCR 漏了）：不算下降
}

fn progress_texts(n: usize, flat: bool) -> VecDeque<Vec<&'static str>> {
    let mut v: Vec<Vec<&'static str>> = vec![vec!["50"]];
    for i in 0..n {
        v.push(if flat { vec!["50"] } else { vec![Box::leak(format!("{}", 49 - i).into_boxed_str())] });
    }
    v.into()
}

#[test]
fn six_steps_without_the_number_dropping_is_a_stall_and_asks_the_model() {
    let say = Say::new(Some(0));
    let mut reads = vec![Ok(grid(A))];
    for _ in 0..40 {
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(A)));
        reads.push(Ok(grid(A)));
    }
    let mut d = Fake::new(reads);
    d.texts = progress_texts(8, true);
    // 没有 --ask-model 的「每步都问」，也没有被拒绝：只有停滞会触发
    let (_, log) = run_ask(&mut d, Some(&say), false, 30, 8);
    assert_eq!(log[5].get("stalled"), None, "第 6 步之前还不算停滞");
    assert!(say.calls.get() >= 1, "停滞以后应该问模型");
    assert!(log.iter().any(|l| l["stalled"] == true));
}

#[test]
fn a_dropping_number_never_stalls() {
    let say = Say::new(Some(0));
    let mut reads = vec![Ok(grid(A))];
    for _ in 0..40 {
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(A)));
        reads.push(Ok(grid(A)));
    }
    let mut d = Fake::new(reads);
    d.texts = progress_texts(8, false);
    let (_, log) = run_ask(&mut d, Some(&say), false, 30, 8);
    assert_eq!(say.calls.get(), 0);
    assert!(log.iter().all(|l| l.get("stalled").is_none()));
}

#[test]
fn unreadable_numbers_neither_add_to_nor_clear_the_stall_count() {
    // 读不到数字（unsupported）：没有「停滞」，也不报错
    let say = Say::new(Some(0));
    let mut d = Fake::new(settled_script());
    let (s, log) = run_ask(&mut d, Some(&say), false, 30, 1);
    assert_eq!(s.stop, Stop::StepsDone);
    assert!(log[0].get("stalled").is_none());
}
```

`run_ask`、`Say` 是第三轮已有的测试辅助（在 `play_tests.rs`）；`MOVED`、`A`、`settled_script()` 也已有。第一个停滞测试的读盘脚本依赖 `settle` 读几次，若对不上按 Task 1 的说明调整读数条数，断言不变。

`text.rs` 的测试：

```rust
#[test]
fn step_line_says_so_when_stalled() {
    let rec = serde_json::json!({"candidates":[{"a":[0,0],"b":[0,1],"features":{"cleared":3}}],"chosen":0,"outcome":"moved","stalled":true});
    assert!(step_line(1, &rec).contains("这几步看起来没有进展"));
    let rec2 = serde_json::json!({"candidates":[{"a":[0,0],"b":[0,1],"features":{"cleared":3}}],"chosen":0,"outcome":"moved"});
    assert!(!step_line(1, &rec2).contains("没有进展"));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-game play_tests::decreased play_tests::six_steps && cargo +1.99.0 test --lib game::text`
Expected: 编译失败 / 断言失败。

- [ ] **Step 3: 写实现**

`play.rs`：

```rust
/// 连着这么多步目标数字都没下降，就算停滞。
pub const STALL_STEPS: usize = 6;

/// 个数相同、且至少有一个数变小才算「下降」。个数不同（OCR 漏读了一个）一律不算，也不算「没下降」。
fn decreased(before: &[u64], after: &[u64]) -> bool {
    before.len() == after.len() && before.iter().zip(after).any(|(b, a)| a < b)
}
```

循环状态加 `let mut no_progress = 0usize;`。问模型的条件由 `(o.ask_always || had_failed_here)` 改成 `(o.ask_always || had_failed_here || no_progress >= STALL_STEPS)`；构造 `rec` 之后加 `if no_progress >= STALL_STEPS { rec["stalled"] = json!(true); }`。

Task 4 里读 `progress_after` 之后更新计数（两种结果分支都做）：

```rust
if let (Some(b), Some(a)) = (&progress_prev, &progress_after) {
    if b.len() == a.len() {
        no_progress = if decreased(b, a) { 0 } else { no_progress + 1 };
    }
}
```

（要放在 `progress_prev = progress_after.clone()` 之前；个数不同或读不到时 `no_progress` 不动。）

`src/game/text.rs` 的 `step_line`：在 `match outcome {...}` 之后、返回前加：

```rust
    if rec["stalled"].as_bool() == Some(true) {
        s += "；这几步看起来没有进展";
    }
```

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked`
Expected: 全绿。

- [ ] **Step 5: 变异检查，然后提交**

把 `STALL_STEPS` 改成 1 → `…is_a_stall…` 里「第 6 步之前不算停滞」的断言变红；把 `b.len() == a.len()` 守卫去掉 → 个数不同的情形被算成「没下降」，`unreadable…`/`decreased…` 变红；把触发条件里的 `|| no_progress >= STALL_STEPS` 去掉 → `…asks_the_model` 变红。改回。

```bash
git add crates/dct-game/src src/game/text.rs
git commit -m "feat(game): six steps without the on-screen numbers dropping is a stall: recorded, said in plain words, and the model is asked"
```

---

### Task 6: `ask-bench` 认带答案的题库

**Files:**
- Modify: `src/game/bench.rs`、`src/game/cli.rs`（只改报告输出调用，若需要）
- Test: `src/game/bench.rs`

**Interfaces:**
- Consumes: 现有 `replay(lines, adv, goal_of, limit) -> Report`；题库行 = 记录行 + `"good": [候选下标…]`、`"bad": [候选下标…]`（下标指 `candidates` 里的位置，0 起）。
- Produces: `Report` 新增 `pub labelled: usize`（有 `good` 或 `bad` 的被问局面数）、`pub picked_good: usize`、`pub picked_bad: usize`；报告多一行。

- [ ] **Step 1: 写失败的测试**

```rust
    struct Pick(usize);
    impl Advisor for Pick {
        fn pick(&self, _: &AskInput) -> Option<Advice> {
            Some(Advice { choice: Some(self.0), reason: String::new(), raw: String::new(), model: "t".into() })
        }
    }

    fn labelled(good: Vec<usize>, bad: Vec<usize>) -> Value {
        let mut r = rec(5, false);
        r["good"] = json!(good);
        r["bad"] = json!(bad);
        r
    }

    #[test]
    fn replay_counts_good_and_bad_picks_on_labelled_lines() {
        // 打乱后模型总选第 1 个展示项；用只标 good=[候选0] 的行，无论洗成什么样，选中 good 的次数 ∈ {0,1}
        let lines = vec![labelled(vec![0], vec![1])];
        let r = replay(&lines, &Pick(0), &|_| String::new(), 10);
        assert_eq!(r.labelled, 1);
        assert_eq!(r.picked_good + r.picked_bad <= 1, true);
    }

    #[test]
    fn replay_good_and_bad_are_in_terms_of_original_candidate_indexes() {
        // 把每个候选都标成 bad：模型选什么都是 bad
        let lines = vec![labelled(vec![], vec![0, 1, 2, 3, 4])];
        let r = replay(&lines, &Pick(2), &|_| String::new(), 10);
        assert_eq!((r.picked_good, r.picked_bad), (0, 1));
        // 都标成 good：选什么都是 good
        let lines = vec![labelled(vec![0, 1, 2, 3, 4], vec![])];
        let r = replay(&lines, &Pick(2), &|_| String::new(), 10);
        assert_eq!((r.picked_good, r.picked_bad), (1, 0));
    }

    #[test]
    fn replay_survives_out_of_range_or_empty_labels() {
        let lines = vec![labelled(vec![99], vec![]), labelled(vec![], vec![]), rec(5, false)];
        let r = replay(&lines, &Pick(0), &|_| String::new(), 10);
        assert_eq!(r.asked, 3);
        assert_eq!(r.picked_good, 0);
    }
```

（`rec`、`First`、`json!`、`Value` 已在 `bench.rs` 的测试模块里；上面 `Pick` 是新增的辅助。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test --lib game::bench`
Expected: 编译失败（`labelled` 字段不存在）。

- [ ] **Step 3: 写实现**

`Report` 加三个字段（`Default` 已 derive）。`replay` 里，问完得到 `a` 之后，算「模型选的原始候选下标」`order[k]`，若这一行有标注：

```rust
let good: Vec<usize> = rec["good"].as_array().map(|v| v.iter().filter_map(|x| x.as_u64().map(|n| n as usize)).collect()).unwrap_or_default();
let bad: Vec<usize> = rec["bad"].as_array().map(|v| v.iter().filter_map(|x| x.as_u64().map(|n| n as usize)).collect()).unwrap_or_default();
if !good.is_empty() || !bad.is_empty() {
    r.labelled += 1;
    if let Some(k) = chosen_original_index {
        if good.contains(&k) { r.picked_good += 1; }
        if bad.contains(&k) { r.picked_bad += 1; }
    }
}
```

（`chosen_original_index` 即现有代码里 `order[k]`。注意 `good`/`bad` 里的下标可能超出 `0..cands.len()`：用 `contains` 比较，天然安全。）

报告文字（在 `cli.rs` 里打印的那段，或 `bench.rs` 里生成报告字符串的函数）末尾，当 `labelled > 0` 时多一行：

`其中有标准答案的 {labelled} 个：选中好走法 {picked_good} 个，选中坏走法 {picked_bad} 个。`

并把原来的说明句改成：有标准答案的行才能说「选得对不对」，没有的仍只能说比乱选强多少（保留原来的 caveat 句子，只在 `labelled == 0` 时出现）。

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked` 与 clippy。
Expected: 全绿、无警告。

- [ ] **Step 5: 变异检查，然后提交**

把 `good.contains(&k)` 改成 `good.contains(&order[0])` → `replay_good_and_bad_are_in_terms_of…` 的某一断言应变红（选 2 vs 选 0）；改回。

```bash
git add src/game
git commit -m "feat(game): ask-bench reads labelled question files and reports good and bad picks"
```

---

## Self-Review

1. **Spec 覆盖：** §1 预测对不对 → Task 3（预测字段）+ Task 1（被拒绝）；§2 动不了的格子 + 上限 5 → Task 1；§3 只写看得见的事实 → Task 2（收窄见开头）；§4 进度数字 → Task 4；§5 停滞 → Task 5；§6 题库格式 → Task 6（经验格式按开头说明不做）。验收第 3 条（带答案的题库放 dcv）靠 Task 6 提供格式，题目由用户/dcv 提供，不在本计划。
2. **占位扫描：** 没有 TBD；有两处写明「按已有同类测试调整读数条数，断言不变」（Task 1、5 的假读盘脚本依赖 `settle` 读几次），这是实现时必须看现有测试才能定的数，不是占位；已要求在报告里说明。
3. **类型一致：** `MAX_REFUSED_IN_A_ROW`（Task 1）、`COLOUR_SAME_DE`/`changed_cells`（Task 3）、`numbers`/`progress_prev`/`progress_after`（Task 4）、`STALL_STEPS`/`decreased`/`no_progress`（Task 5）在各任务里一致；Task 5 依赖 Task 4 的 `progress_after`；Task 1 的 `Options` 构造字面量沿用第三轮的六个字段。
4. **Review Focus：** 五条分别落在 Task 1（全排除、解锁）、Task 3（rgb 缺失）、Task 4/5（读不到数字、个数不同）、Task 6（标注越界）。
