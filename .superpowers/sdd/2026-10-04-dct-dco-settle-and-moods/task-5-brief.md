### Task 5: 接线（`play` / `auto_next` / `identify` / 配置里的 `theme`）

**Files:**
- Modify: `crates/dct-game/src/play.rs`、`crates/dct-game/src/navigate.rs`、`src/game/identify.rs`、`src/game/profile.rs`（可选键 `theme`）、`crates/dct-game/src/play.rs` 的 `Profile`（加 `theme: Option<String>`，所有构造处补 `None`）、`src/game/skill.md`
- Test: `crates/dct-game/src/play_tests.rs`、`navigate.rs` 的测试、`src/game/identify.rs`、`src/game/profile.rs`

**Interfaces:**
- Consumes: Task 3 的 `show_status_with`；Task 4 的 `mood::{for_step, for_stop, for_genre}`、`GoalFinder::{step_index, goal_start}`。
- 行为：
  1. **开局一次**（非 dry-run）：`show_status_with("look", None, p.theme.as_deref())`（有 `theme` 才发；没有就什么都不发，和现在一样）。
  2. **每步划完、写完记录之后**：用这一步的 `chosen.features`、`stalled_now`、剩余步数（`finder.step_index()` 指的那个数字的 `progress_after`）、目标数字当前值（`gi` 指的那个数字的 `progress_after`）和 `finder.goal_start()` 算 `for_step`。结果是 `Some(m)` 且 `m.state` 和**上一次发给章鱼的状态不同** → `show_status_with(m.state, m.text, None)`；结果是 `None` 且上一次发的不是 `"look"` → 发 `look`。**同一个状态连着不重发。**
  3. **结束时**（`play` 的 `break` 之后、返回 `Summary` 之前；`auto_next` 结束时同理）：`for_stop(&stop)` 有就发（不受「同状态不重发」限制）。`Won`、`LivesOut`、`Money`、`Ad`、`UnknownScreen`、`LevelEnded` 是 `navigate.rs` 的停止原因，在 `auto_next` 的出口发。
  4. `dct game identify`：`for_genre` 有就发（经 `show_status_with`）。
  5. 配置文件可选键 `theme = "candy"`（字符串，≤ 20 个字符；超了拒绝并说人话）；`profile::load` 读进 `Profile.theme`。**`candy-crush` 内置配置和用户本地的 `level-17xx.toml` 不改**（本地文件由用户自己加这一行；说明卡里写一句怎么加）。
  6. 说明卡加一小节「章鱼」：它会随着玩的过程表现情绪；想让它有糖果主题，在配置文件里加 `theme = "candy"`；旧版 dco 看不到新的表情，不影响玩。

- [ ] **Step 1: 写失败的测试**（`Fake` 的 `events` 记录 `show_status_with` 的 state/text/theme；旧测试继续断言「规则选步不发 think/look」不变：只在**问模型**时 think/look 成对出现，其余事件只在状态变化时出现）
- 有 `theme` 的 profile：开局第一个事件是 `look`，theme 带上；没有 theme：事件序列里没有任何新增事件（和旧测试一致）。
- 一步大招（`bomb>0`）：划完发 `celebrate`；下一步普通步发 `look`；再下一步普通步**不再发**（`look` 不重发）。
- 剩余步数 ≤5（用 `texts` 队列造步数列 −1/步，目标列恒定，先让识别器认出）：发 `tense`；之后连着多步不重发。
- 目标数字降到开局的 20% 以内：发 `hopeful`（没有 tense 时）。
- 停滞：发 `stall`。
- `Stuck` 结束：最后一个事件是 `stuck`；`StepsDone`：结束时没有新增事件。
- `auto_next`：`Won` → `won`；`LivesOut` → `sad`；`Money` → `wait` 带说明条。
- `identify`：寻物 → `confused`；消除 → 无事件。
- 旧 dco（`bad_request` 的新状态）：不影响玩，不影响 think/look（沿用 Task 3 的假 dco）。
- `profile`：`theme = "candy"` 读进来；超长拒绝；没写是 `None`。
- 变异：去掉「同状态不重发」→ 「再下一步不再发」变红；`Stuck` 的发送去掉 → 对应测试变红。

- [ ] **Step 2–5：** 跑测试确认失败 → 实现 → 测试、clippy → 变异 → 提交：`feat(game): drive the octopus from play, auto-next and identify: send a mood only when it changes, a theme from the profile`。

---

