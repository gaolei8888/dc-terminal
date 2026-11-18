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

