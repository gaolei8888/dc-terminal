### Task 1: `Dco::swipe_settle` 和 `DcoClient` 的实现

**Files:**
- Modify: `crates/dct-game/src/play.rs`（trait、类型）、`crates/dct-game/src/dco.rs`（`DcoClient`）
- Test: `crates/dct-game/src/dco_tests.rs`

**Interfaces:**
- Produces:
  - `#[derive(Clone, Debug, PartialEq)] pub struct SwipeSettle { pub changed: bool, pub change: f64, pub settled: bool, pub timed_out: bool, pub settled_ms: Option<u64> }`
  - `Dco::swipe_settle(&mut self, p: &Profile, from: (f64, f64), to: (f64, f64)) -> Result<SwipeSettle, DcoError>`，**默认实现**：先调 `self.swipe(p, from, to)?` 再返回 `Err(DcoError { code: "unsupported", .. })`。**注意语义**：默认实现会真的划一次，然后告诉调用方「没有等稳定的结果」，调用方（Task 2）据此回退轮询，不再重划。
  - `DcoClient` 重写：发 `swipe` + `settle:{quiet_ms:300, timeout_ms:8000, region:<profile.region 转成 {x,y,w,h}>}`；解析返回里的 `settle`；成功返回 `Ok(SwipeSettle)`；`settle` 字段缺失 → 记 `no_swipe_settle = true` 并返回 `Err(unsupported)`（这次划动已经发生）；`settle:{error:…}` → 返回 `Err(DcoError{code: error.code, message})`（划动已发生，不是 swipe 本身失败）；`swiped` 不是 true / 顶层 `error` → `Err` 且**标明划动没发生**：用 `code` 不是 `settle_*` 的原样错误（和现有 `swipe` 的错误一致）。`no_swipe_settle` 已置位时直接走普通 `swipe`（不带 settle）并返回 `Err(unsupported)`。
  - 为区分「划动没发生」和「划动发生了但没有 settle 结果」，用一个枚举更清楚：`pub enum SwipeOutcome { NotSwiped(DcoError), SwipedNoSettle(DcoError), Settled(SwipeSettle) }`，trait 方法返回它。**用这个枚举取代上面的 `Result<SwipeSettle, DcoError>`**：`fn swipe_settle(&mut self, p, from, to) -> SwipeOutcome`，默认实现：`match self.swipe(p, from, to) { Err(e) => NotSwiped(e), Ok(()) => SwipedNoSettle(unsupported()) }`。

- [ ] **Step 1: 写失败的测试**（`dco_tests.rs`，沿用该文件里的假 dco 写法）

- 新 dco：假 dco 对带 `settle` 的 `swipe` 回 `{"swiped":true,"settle":{"changed":true,"change":0.4,"settled":true,"timed_out":false,"settled_ms":420,"region":{...}}}` → `Settled(SwipeSettle{changed:true, change:0.4, settled:true, timed_out:false, settled_ms:Some(420)})`；请求里 `settle.region` 等于 profile 的区域，`quiet_ms` 300，`timeout_ms` 8000。
- 弹回：`changed:false` → `Settled(SwipeSettle{changed:false,..})`。
- 旧 dco：回 `{"swiped":true}`（没有 `settle`）→ `SwipedNoSettle(unsupported)`；**第二次调用不再带 `settle`**（假 dco 记下收到的参数）。
- 划了、等待出错：`{"swiped":true,"settle":{"error":{"code":"stale","message":"窗口关了"}}}` → `SwipedNoSettle(DcoError{code:"stale",..})`，**不置 `no_swipe_settle`**，下一次仍带 `settle`。
- 划动本身失败：顶层 `{"error":{"code":"halted","message":"急停"}}` → `NotSwiped(DcoError{code:"halted",..})`。
- `timed_out:true`：`Settled(SwipeSettle{timed_out:true, settled:false, settled_ms:None,..})`（调用方决定怎么处理）。

- [ ] **Step 2–5：** 跑测试确认失败 → 实现 → 全部测试和 clippy → 变异检查（去掉「缺 settle 字段就置位」→ 旧 dco 的测试变红；把 `settle.error` 也置位 → 错误测试变红）→ 提交：`feat(game): swipe_settle asks dco to wait for the screen to settle and report whether it changed, falling back cleanly on older dco`。

---

