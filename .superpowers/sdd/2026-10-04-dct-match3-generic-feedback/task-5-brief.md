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

