### Task 2: `play()` 用 `swipe_settle`

**Files:**
- Modify: `crates/dct-game/src/play.rs`
- Test: `crates/dct-game/src/play_tests.rs`（`Fake` 加可选的 `settles` 队列和 `swipe_settle` 实现）

**Interfaces:**
- Consumes: Task 1 的 `SwipeOutcome`、`SwipeSettle`。
- Produces: 常量 `pub const SETTLE_QUIET_MS: u32 = 300; pub const SETTLE_TIMEOUT_MS: u32 = 8000;`（`DcoClient` 用）；记录字段 `settle: {"source": "dco"|"poll", "changed": bool|null, "settled_ms": u64|null}`；`timing_ms.settle` 仍是整段等待毫秒。

行为（把现在 `let res = dco.swipe(...)` + `let result = settle(...)` 换成）：
1. `match dco.swipe_settle(p, from, to)`：
   - `NotSwiped(e)` → 和现在 `Err(e)` 一样（`outcome: stopped`，`Stop::Dco(e)`）。
   - `Settled(s)` 且 `s.settled && !s.timed_out`：
     - `!s.changed` → 走现有「没反应」分支（`no_change`，上锁、`failed`、`streak`），**不再读棋盘轮询**；`observed_changed = 0`；`rec["settle"] = {"source":"dco","changed":false,"settled_ms":…}`。
     - `s.changed` → **读一次棋盘** `next`；`Ok` → 走现有 `Settle::Settled(next)`（含「弹回原样」的判断 `same(&next,&g)`、`moved` 分支）；`not_a_grid` → 当作 `Settle::NoGrid`；其他错误 → `Settle::Failed`。`rec["settle"] = {"source":"dco","changed":true,"settled_ms":…}`。
   - `Settled(s)` 但 `timed_out` 或 `!settled`，或 `SwipedNoSettle(_)` → **不再划**，直接调用现有轮询 `settle(dco, clock, p, &g)`；`rec["settle"] = {"source":"poll","changed":null,"settled_ms":null}`。
2. 其余逻辑（`progress`、`finder`、`locked`、问模型、停滞……）一字不改。

- [ ] **Step 1: 写失败的测试**（`Fake` 里：`settles: VecDeque<SwipeOutcome>`，为空时 `swipe_settle` 走 trait 默认实现，所以**所有旧测试不变**）

- dco 说没变（`Settled{changed:false,settled:true}`）：`outcome == "no_change"`，且**这一步没有任何额外的 `read_grid` 轮询**（`reads_taken` 只比旧流程少，断言它不超过「开局 1 次」）；记录 `settle.source == "dco"`，`changed == false`。
- dco 说变了（`changed:true`）：读一次棋盘，`outcome == "moved"`；`settle.source == "dco"`。
- dco 说变了但读回的棋盘和划前一样（弹回）：`outcome == "no_change"`（沿用 6248a5f 的规则）。
- `timed_out`：回退轮询，`settle.source == "poll"`，**`swipes.len()` 没有因此增加**（不重划）。
- `SwipedNoSettle(stale)`：回退轮询，不重划，不当成没反应。
- `NotSwiped(halted)`：`Stop::Dco`，和旧行为一致。
- 默认实现（旧 dco）：所有已有测试照常通过（不改断言）。
- 变异：把「`!s.changed` 走没反应」改成「读棋盘再判断」→ 第一个测试变红；把 timed_out 当稳定 → timed_out 测试变红。

- [ ] **Step 2–5：** 跑测试确认失败 → 实现 → 全部测试、clippy → 变异 → 提交：`feat(game): play() lets dco wait for the screen and say whether a swipe changed it; polling stays as the fallback`。

---

