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

