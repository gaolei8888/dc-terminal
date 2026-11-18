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

