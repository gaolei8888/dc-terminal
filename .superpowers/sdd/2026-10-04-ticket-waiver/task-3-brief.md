### Task 3: `waiver_for_tap`（纯函数：这一点在不在规矩放行的区域里）

**Files:**
- Create: `crates/dct-brain/src/waiver.rs`；Modify: `crates/dct-brain/src/lib.rs`（`pub mod waiver;`）
- Test: `waiver.rs` 的 `tests`

**Interfaces:**
- Consumes: Task 1 的 `Waiver`、`Region`。
- Produces:
```rust
pub struct TapRule {          // dcv games --json 里一条已确认规矩的这一块
    pub rule: String,         // 库:名字
    pub rule_sha256: String,
    pub approved_at: Option<String>,   // None = 没确认
    pub tap_allow: Vec<TapAllow>,
}
pub struct TapAllow { pub kind: String, pub x: f64, pub y: f64, pub w: f64, pub h: f64 }  // 窗口 0–1 比例，dcv 的 region

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    NotApproved,        // approved_at 为空
    NoBlankAreaRule,    // 没有 kind == "no_text" 的区域
    OutsideRegion,      // 点不在任何区域里
    BadPoint,           // x_bp/y_bp 超过 10000
}
impl Refusal { pub fn zh(&self) -> &'static str }   // 大白话

pub fn waiver_for_tap(rule: &TapRule, x_bp: u16, y_bp: u16) -> Result<Waiver, Refusal>
```
行为：`x_bp > 10000 || y_bp > 10000` → `BadPoint`；`approved_at` 为 `None` → `NotApproved`；筛出 `kind == "no_text"` 的区域，没有 → `NoBlankAreaRule`；把区域换算成整数万分比（`round(v * 10000)`，夹在 0..=10000，保证 `x_bp + w_bp ≤ 10000`，换算后不合格的区域跳过）；点落在**任一**区域内（含边界）就返回 `Waiver { kind: "no_text", rule, rule_sha256, approved_at, region: 那块区域 }`；否则 `OutsideRegion`。

- [ ] **Step 1: 写失败的测试**
- 用真实的那条规矩（`tap_allow: no_text 0.03 0.12 0.75 0.66`）：点 (1800, 5800) 在区域内 → `Ok`，`region == {300, 1200, 7500, 6600}`，`rule`/`rule_sha256`/`approved_at` 原样带出。
- 边界：点正好在左上角 (300, 1200) 和右下角 (7800, 7800) 在内；(299, 1200)、(7801, 7800) 在外。
- 没确认（`approved_at: None`）→ `NotApproved`，即使点在区域内。
- 只有 `kind: "with_text"` 之类的区域 → `NoBlankAreaRule`；空 `tap_allow` → `NoBlankAreaRule`。
- 点超过 10000 → `BadPoint`。
- 区域换算后越界（例如 `x=0.9, w=0.2`）→ 被跳过，点在其中也 `OutsideRegion`（不返回越界的凭据）。
- 多个区域：点在第二块里 → 返回第二块的 region。
- 每个 `Refusal::zh()` 非空、无英文术语。

- [ ] **Step 2–5：** 跑测试确认失败 → 实现 → 全部测试和 clippy → 变异检查（把「含边界」改成不含 → 边界测试变红；去掉 `approved_at` 检查 → 没确认的测试变红；不过滤 `kind` → `with_text` 的测试变红）→ 提交：`feat(brain): waiver_for_tap says whether a point lies in a confirmed rule's blank-area region and returns the waiver to sign`。

---

## Self-Review

1. **覆盖：** spec 的「票多一个可选凭据（含区域）」「verify 例外」「纯函数判断点在不在区域」→ Task 1/2/3；签票、读 dcv、`list_windows`、`tap_at` 本身不在本计划（写在 spec 里的「之后」）。
2. **占位：** 没有；`Ticket {` 字面量的补全由编译器逐个指出。
3. **类型一致：** `Region`/`Waiver`（Task 1）→ Task 2 的 `verify`、Task 3 的 `waiver_for_tap`；`Refusal`（Task 3）。
4. **Review Focus：** 五条分别落在 Task 2（签名/档位/kind）、Task 1（凭据任一字段变则指纹变、JSON）、Task 3（边界、区域）。
