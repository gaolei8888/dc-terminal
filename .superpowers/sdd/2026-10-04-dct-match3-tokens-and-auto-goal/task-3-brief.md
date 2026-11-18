### Task 3: 自动认出目标数字

**Files:**
- Create: `crates/dct-game/src/goal.rs`
- Modify: `crates/dct-game/src/lib.rs`（`pub mod goal;`）、`crates/dct-game/src/play.rs`、`src/game/text.rs`、`src/game/skill.md`
- Test: `crates/dct-game/src/goal.rs`、`crates/dct-game/src/play_tests.rs`、`src/game/text.rs`

**Interfaces:**
- Consumes: 第四轮的 `progress_prev`/`progress_after`、`goal_dropped(before, after, gi) -> Option<bool>`、`no_progress`、`Options.goal_index`、`STALL_STEPS`。
- Produces:
  - `pub struct GoalFinder`，`GoalFinder::new()`，`observe(&mut self, before: &[u64], after: &[u64])`（只在**成功走了一步**时调用；个数不同的忽略），`index(&self) -> Option<usize>`，`pub const MIN_SAMPLES: usize = 4;`
  - `play()` 里 `fn effective_goal(o_goal: Option<usize>, finder: &GoalFinder) -> Option<usize>`（用户给的优先）
  - 记录：`progress` 对象多 `"goal_index"`（当前生效的下标，0 起，没有就是 `null`）；认出的那一步多一个顶层字段 `"goal_found": N`（1 起，只在认出的那一步出现）
  - `step_line` 在有 `goal_found` 时追加「；我认为屏幕上第 N 个数字是这一关的目标」

识别规则（`GoalFinder`，只看数字，不看意思）：对每个下标 i 累计 `samples[i]`、`all_minus_one[i]`（到目前为止每次变化都刚好是 −1）、`ever_increased[i]`。样本只来自「前后个数相同」的成功步。至少有 `MIN_SAMPLES` 个样本后：排除 `all_minus_one[i]`（步数）和 `ever_increased[i]`（分数、连击）；若**恰好剩一个**下标，就是目标，并且**一旦认出就在本次运行里保持**（`found: Option<usize>` 锁定，个数永久变化也不改，避免反复横跳）。个数不同的步只是不进样本。

- [ ] **Step 1: 写失败的测试**

`goal.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn feed(f: &mut GoalFinder, steps: &[(&[u64], &[u64])]) {
        for (b, a) in steps {
            f.observe(b, a);
        }
    }

    #[test]
    fn the_step_counter_and_a_rising_score_are_excluded_leaving_the_goal() {
        // [步数, 目标, 分数]：步数每步 -1，目标不规律降，分数涨
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122, 0], &[37, 110, 120]),
            (&[37, 110, 120], &[36, 100, 300]),
            (&[36, 100, 300], &[35, 100, 340]),
            (&[35, 100, 340], &[34, 80, 600]),
        ]);
        assert_eq!(f.index(), Some(1));
    }

    #[test]
    fn a_goal_that_never_moves_is_still_found_by_elimination() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[20, 50, 0], &[19, 50, 100]),
            (&[19, 50, 100], &[18, 50, 150]),
            (&[18, 50, 150], &[17, 50, 400]),
            (&[17, 50, 400], &[16, 50, 410]),
        ]);
        assert_eq!(f.index(), Some(1));
    }

    #[test]
    fn two_irregular_numbers_are_ambiguous() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122, 30], &[37, 110, 25]),
            (&[37, 110, 25], &[36, 100, 20]),
            (&[36, 100, 20], &[35, 100, 15]),
            (&[35, 100, 15], &[34, 80, 5]),
        ]);
        assert_eq!(f.index(), None);
    }

    #[test]
    fn fewer_than_four_samples_decide_nothing() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122], &[37, 110]),
            (&[37, 110], &[36, 100]),
            (&[36, 100], &[35, 90]),
        ]);
        assert_eq!(f.index(), None);
    }

    #[test]
    fn steps_with_different_list_lengths_are_not_samples() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122], &[37, 110]),
            (&[37, 110], &[36]),            // 漏读一个：不进样本
            (&[36, 100], &[35, 100, 9]),    // 多出一个：不进样本
            (&[35, 100], &[34, 90]),
            (&[34, 90], &[33, 80]),
        ]);
        assert_eq!(f.index(), None, "只有 3 个有效样本");
        feed(&mut f, &[(&[33, 80], &[32, 70])]);
        assert_eq!(f.index(), Some(1));
    }

    #[test]
    fn once_found_the_goal_stays_found_for_the_run() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122, 0], &[37, 110, 120]),
            (&[37, 110, 120], &[36, 100, 300]),
            (&[36, 100, 300], &[35, 100, 340]),
            (&[35, 100, 340], &[34, 80, 600]),
        ]);
        assert_eq!(f.index(), Some(1));
        // 之后目标数字涨了一次（重置）也不改
        feed(&mut f, &[(&[34, 80, 600], &[33, 90, 650])]);
        assert_eq!(f.index(), Some(1));
    }
}
```

`play_tests.rs` 末尾（`Fake` 的 `texts` 队列、`run_ask`、`Say`、`settled_script` 已有）：

```rust
#[test]
fn without_goal_number_the_finder_picks_the_goal_and_stall_then_works() {
    // 数字列表 [步数, 目标]：步数每步 -1，目标恒为 50（不动）。4 步样本后认出目标 = 下标 1，之后 6 步没降 → 停滞 → 问模型
    let say = Say::new(Some(0));
    let mut reads = vec![Ok(grid(A))];
    for _ in 0..60 {
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(A)));
        reads.push(Ok(grid(A)));
    }
    let mut d = Fake::new(reads);
    let mut texts: Vec<Vec<&'static str>> = vec![vec!["30", "50"]];
    for i in 0..14 {
        texts.push(vec![Box::leak(format!("{}", 29 - i).into_boxed_str()), "50"]);
    }
    d.texts = texts.into();
    // goal_index 为 None（用户没给）→ 认
    let (_, log) = run_ask(&mut d, Some(&say), false, 30, 14);
    assert!(log.iter().any(|l| l["goal_found"] == 2), "应在某一步认出第 2 个数字：{log:?}");
    assert!(log.iter().any(|l| l["stalled"] == true));
    assert!(say.calls.get() >= 1);
}

#[test]
fn a_user_given_goal_number_is_never_overridden_by_the_finder() {
    let mut reads = vec![Ok(grid(A))];
    for _ in 0..60 {
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(A)));
        reads.push(Ok(grid(A)));
    }
    let mut d = Fake::new(reads);
    let mut texts: Vec<Vec<&'static str>> = vec![vec!["30", "50"]];
    for i in 0..10 {
        texts.push(vec![Box::leak(format!("{}", 29 - i).into_boxed_str()), "50"]);
    }
    d.texts = texts.into();
    let say = Say::new(Some(0));
    let mut log = vec![];
    // 用户指定第 1 个数（下标 0 = 步数，每步都降）
    let o = Options { max_steps: 10, dry_run: false, advisor: Some(&say), ask_always: false, ask_budget: 30, goal: "", goal_index: Some(0) };
    let _ = play(&mut d, &mut Clk(0), &profile(3, 4), &o, &mut |v| log.push(v));
    assert!(log.iter().all(|l| l.get("goal_found").is_none()), "用户给了，就不自动认");
    assert!(log.iter().all(|l| l["progress"]["goal_index"] == 0 || l["progress"]["goal_index"].is_null()));
}
```

（`run_ask` 的 `Options` 里 `goal_index` 现在写死 `None`——保持。假读盘的条数按 `a_move_that_works…`/第四轮已有停滞测试的做法调整，断言不变。）

`text.rs`：

```rust
#[test]
fn step_line_says_which_number_was_taken_as_the_goal() {
    let rec = serde_json::json!({"candidates":[{"a":[0,0],"b":[0,1],"features":{"cleared":3}}],"chosen":0,"outcome":"moved","goal_found":2});
    assert!(step_line(1, &rec).contains("我认为屏幕上第 2 个数字是这一关的目标"));
    let rec2 = serde_json::json!({"candidates":[{"a":[0,0],"b":[0,1],"features":{"cleared":3}}],"chosen":0,"outcome":"moved"});
    assert!(!step_line(1, &rec2).contains("目标"));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-game goal:: && cargo +1.99.0 test --lib game::text`
Expected: 编译失败（`GoalFinder` 未定义）。

- [ ] **Step 3: 写实现**

`goal.rs`：

```rust
//! 不看数字的意思，只看数字怎么变，认出哪个是「目标」。
//! 步数每成功一步刚好降 1；分数、连击只增不减；剩下的就是目标。
//! 恰好剩一个才认；多于一个或没有就不认（宁可不判停滞，也不乱判）。

pub const MIN_SAMPLES: usize = 4;

#[derive(Default)]
pub struct GoalFinder {
    samples: usize,
    all_minus_one: Vec<bool>,
    ever_increased: Vec<bool>,
    found: Option<usize>,
}

impl GoalFinder {
    pub fn new() -> GoalFinder {
        GoalFinder::default()
    }

    /// 只在**成功走了一步**之后调用。前后个数不同的步不进样本（OCR 漏读、弹窗飘字）。
    pub fn observe(&mut self, before: &[u64], after: &[u64]) {
        if self.found.is_some() || before.len() != after.len() || before.is_empty() {
            return;
        }
        if self.samples == 0 {
            self.all_minus_one = vec![true; before.len()];
            self.ever_increased = vec![false; before.len()];
        }
        if self.all_minus_one.len() != before.len() {
            return; // 列表个数和已有样本不一致：这一步不进样本
        }
        self.samples += 1;
        for (i, (b, a)) in before.iter().zip(after).enumerate() {
            if *a != b.saturating_sub(1) || *b == 0 {
                self.all_minus_one[i] = false;
            }
            if a > b {
                self.ever_increased[i] = true;
            }
        }
        if self.samples >= MIN_SAMPLES {
            let left: Vec<usize> = (0..self.all_minus_one.len()).filter(|&i| !self.all_minus_one[i] && !self.ever_increased[i]).collect();
            if left.len() == 1 {
                self.found = Some(left[0]);
            }
        }
    }

    pub fn index(&self) -> Option<usize> {
        self.found
    }
}
```

注意：`self.all_minus_one.len() != before.len()` 这一支让「中途个数永久变化」的步不进样本，但识别器仍用开头样本的下标；这是已知的限制（见 spec）。

`play.rs`：

1. `use crate::goal::GoalFinder;`；循环前 `let mut finder = GoalFinder::new();`。
2. 新增小函数，把三处重复的更新收成一个：

```rust
/// 停滞计数的一次更新；`gi` 是当前生效的目标下标（用户给的优先，否则识别器认出的），没有就不动。
fn update_stall(no_progress: &mut usize, gi: Option<usize>, before: &Option<Vec<u64>>, after: &Option<Vec<u64>>) {
    if let (Some(gi), Some(b), Some(a)) = (gi, before, after) {
        match goal_dropped(b, a, gi) {
            Some(true) => *no_progress = 0,
            Some(false) => *no_progress += 1,
            None => {}
        }
    }
}
```

3. 三个结果分支里原来的 `if let (Some(gi), Some(b), Some(a)) = (o.goal_index, ...) { ... }` 都换成：

```rust
let gi = o.goal_index.or(finder.index());
update_stall(&mut no_progress, gi, &progress_prev, &progress_after);
rec["progress"] = json!({ "before": progress_prev, "after": progress_after, "goal_index": gi });
```

4. 只在「成功走了一步」的 `Settle::Settled(next)`（`moved`）分支里、写记录**之前**，让识别器观察，并在刚认出时打标记：

```rust
let had = finder.index();
if let (Some(b), Some(a)) = (&progress_prev, &progress_after) {
    finder.observe(b, a);
}
if o.goal_index.is_none() && had.is_none() {
    if let Some(i) = finder.index() {
        rec["goal_found"] = json!(i + 1);
    }
}
```

（顺序：先 `observe` → 再算 `gi`（因此刚认出的这一步也用新认出的下标，计数从这一步算起）→ 再 `update_stall`。被拒绝的两个分支**不**调用 `observe`——被拒绝的步数不降，会污染「刚好 −1」的判断。）

5. `stalled_now`/触发条件里的 `no_progress >= STALL_STEPS` 不变；第四轮的 `o.goal_index.is_none()` 之类的早退如果有，改成看 `gi.is_none()`（`gi` 在循环顶部算：`let gi = o.goal_index.or(finder.index());`，供「停滞才问模型」那块用——只有 `gi.is_some()` 才可能 `stalled`）。

`text.rs` 的 `step_line`：在 `stalled` 句子之前加

```rust
    if let Some(n) = rec["goal_found"].as_u64() {
        s += &format!("；我认为屏幕上第 {n} 个数字是这一关的目标");
    }
```

`skill.md`：把 `--goal-number` 那段改成：不给时 dct 会在走几步以后自己认（认的是变化不规律、又不是每步刚好少 1 的那个数字），认出来会说一句；认错了再用 `--goal-number` 指明。大白话。

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked` 与 clippy。
Expected: 全绿，无警告。第四轮已有的停滞测试（用 `goal_index: Some(..)`）不应受影响；如有因为 `finder` 在 `Some` 时仍然 `observe` 而改变行为的，保持「用户给了就不自动认」：`observe` 只在 `o.goal_index.is_none()` 时调用。

- [ ] **Step 5: 变异检查，然后提交**

分别临时改：`MIN_SAMPLES` 改成 2 → `fewer_than_four…` 变红；去掉 `all_minus_one` 的排除 → `the_step_counter_and_a_rising_score…` 变红；`left.len() == 1` 改成 `>= 1` → `two_irregular…` 变红；去掉 `found` 的锁定（`if self.found.is_some()`）→ `once_found…` 变红；把被拒绝分支里也调用 `observe` → 需要一个测试：被拒绝的步（数字不降）夹在中间，不应让步数被排除。若现有测试抓不到这一条，**加一个**并确认它变红。

```bash
git add crates/dct-game src/game
git commit -m "feat(game): without --goal-number, pick the goal number by how the on-screen numbers change (step counter drops by exactly one, score only rises)"
```

---

## Self-Review

1. **Spec 覆盖：** 一（token）→ Task 1（用量读取）+ Task 2（Advice/记录/结束语）；二（自动认目标）→ Task 3；「不做」里的各项都没有出现在任务里。验收（真机）不在计划里，留给最后。
2. **占位扫描：** Task 1 的 `Credential` 构造、Task 3 的假读盘条数，已写明按现有测试的写法调整、断言不变。没有 TBD。
3. **类型一致：** `Usage { input, output }`（Task 1）→ `Advice.tokens: Option<(u64, u64)>`（Task 2，在 `LlmAdvisor` 里转换）；`GoalFinder::{new, observe, index}`、`MIN_SAMPLES`（Task 3）；`update_stall`、`goal_dropped` 沿用第四轮；记录字段 `tokens_in/out`、`progress.goal_index`、`goal_found` 在测试和实现里名字一致。
4. **Review Focus：** 五条分别落在 Task 1（usage 畸形）、Task 2（混合计数）、Task 3（个数变化、用户越界、锁定不反复）。
