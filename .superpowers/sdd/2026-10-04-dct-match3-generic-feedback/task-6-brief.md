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
