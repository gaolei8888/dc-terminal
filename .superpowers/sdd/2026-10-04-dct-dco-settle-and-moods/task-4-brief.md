### Task 4: `mood.rs`（什么时候发什么情绪）

**Files:**
- Create: `crates/dct-game/src/mood.rs`；Modify: `crates/dct-game/src/lib.rs`（`pub mod mood;`）、`crates/dct-game/src/goal.rs`（加 `step_index()` 和 `goal_start`）
- Test: `mood.rs` 内 `tests`

**Interfaces:**
- Produces（全是纯函数，不碰 dco）：
  - `pub struct Mood { pub state: &'static str, pub text: Option<&'static str> }`
  - 常量 `BIG_CLEAR: usize = 20`（预计消 ≥ 这么多颗算大招；注明「以后进 dcv 的规矩」）、`TENSE_STEPS_LEFT: u64 = 5`、`HOPEFUL_FRACTION_PERCENT: u64 = 20`、`HOPEFUL_ABS: u64 = 3`（目标数字剩余 ≤ 开局值的 20% 或 ≤ 3，且 > 0）
  - `pub fn for_step(f: &Features, stalled: bool, steps_left: Option<u64>, goal_now: Option<u64>, goal_start: Option<u64>) -> Option<Mood>`，优先级从高到低：`tense`（`steps_left ≤ TENSE_STEPS_LEFT`）> `celebrate`（大招：`cleared + cascade ≥ BIG_CLEAR` 或 `bomb > 0`；特效含 `wrapped`/`striped` 不算，只算彩球和大消）> `hopeful`（目标快完成）> `stall`（`stalled`）；都不满足 → `None`（调用方自己回 `look`，见 Task 5）。
  - `pub fn for_stop(stop: &Stop) -> Option<Mood>`：`Won`→`won`；`LivesOut`→`sad`；`Money`/`Ad`→`wait`+「要你处理」；`UnknownScreen`/`LevelEnded`/`NoGrid`→`wait`+「看不懂这个画面」；`Stuck`→`stuck`；其余（`StepsDone`、`DryRun`、`NoMoves`、`Dco`、…）→ `None`。
  - `pub fn for_genre(g: Genre) -> Option<Mood>`：`HiddenObject`→`confused`+「不会玩这类」；其余 → `None`。
  - `GoalFinder::step_index(&self) -> Option<usize>`：在 `found` 之后，被 `all_minus_one` 标记的唯一下标（至少 `MIN_SAMPLES` 个样本且恰好一个）；没有 → `None`。`GoalFinder::goal_start(&self) -> Option<u64>`：目标认出时，该下标的**第一份** `before` 值（认出前一直记着第一个样本的 `before` 向量）。

- [ ] **Step 1: 写失败的测试**
- `tense` 在 `steps_left=Some(5)` 触发，`Some(6)` 不触发；优先于 `celebrate`。
- `celebrate`：`cleared+cascade=20` 触发，`19` 不触发；`bomb>0` 触发；只有 `striped>0` 不触发。
- `hopeful`：`goal_now=Some(20), goal_start=Some(100)` 触发（20%），`Some(21)` 不触发；`goal_now=Some(3)` 总是触发，`Some(0)` 不触发（0 = 已经完成，不是期待）；`goal_start=None` 时只看 `HOPEFUL_ABS`。
- `stall`：`stalled=true` 且别的都不满足 → `stall`；有 `tense` 时让位。
- `for_stop`：每个 `Stop` 对应的状态和说明条；`text` 都 ≤16 个字符（遍历所有有文字的断言长度）。
- `for_genre`：`HiddenObject`→`confused`，`Match3`/`Unknown`→`None`。
- `goal.rs`：`step_index` 在步数列恰好 −1/步时给出；两个下标都 −1/步时 `None`；`goal_start` 给出认出时第一个样本的目标数字。
- 变异：把 `<= TENSE_STEPS_LEFT` 改成 `<`；把 `celebrate` 的阈值改成 `>`；各自对应测试变红。

- [ ] **Step 2–5：** 跑测试确认失败 → 实现 → 测试、clippy → 变异 → 提交：`feat(game): pure rules for which octopus mood fits a step, a stop or a genre`。

---

