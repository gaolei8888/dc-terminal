# dct 用 dco 的新能力：划动时等稳定并比较前后；驱动章鱼的情绪  Design + Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** (0) 认出 dco 的「私人画面」就停下不操作；(1) `play()` 划动时让 dco 自己等画面稳定并报告「变了/没变」，不再每 0.1 秒读棋盘去猜，被弹回的笼子几百毫秒就知道（旧 dco 回退到现在的轮询）；(2) `play()` / `auto_next()` / `identify` 在对应时刻给章鱼发情绪（想、没进展、卡住、大招、紧张、期待、过关、失败、要人处理、困惑），不认识的新状态一律忽略。

**Architecture:** 全在 `crates/dct-game`。`Dco` trait 加两个**有默认实现**的方法：`swipe_settle`（默认返回 `Err(unsupported)`，调用方回退轮询）和 `show_status_with`（默认转调 `show_status`）。`DcoClient` 实现它们，并各自记「这台 dco 不支持」。新增纯函数模块 `mood.rs`（什么时候发什么情绪、阈值、说明条文字），`play()` 里只负责「状态变了才发」。数据（`theme`）来自配置文件，不写进代码。

**Tech Stack:** Rust（workspace `dct` + `crates/dct-game`），`serde_json`。

**dco 的真实接口（2026-10-04，dc-octo commit 9dfe277，已在真机用 iTerm2 窗口验证）：**
- `wait_idle {window, region?, quiet_ms?(默认300, 50–5000), timeout_ms?(默认5000, 100–60000), change_max?}` → `{settled, timed_out, waited_ms, quiet_ms, last_change}`；超时不是错误。
- `swipe` 多一个可选参数 `settle:{quiet_ms?, timeout_ms?, region?, change_max?, min_change?(默认0.01)}`；不传 settle，返回和以前一样。传了 → `{swiped:true, settle:{changed, change, settled, timed_out, settled_ms|null, region}}`；划完后等待出错时仍是 `swiped:true`，`settle:{error:{code,message}}`（**绝不能因此重划**）。`region` 用窗口 0–1 比例，建议给棋盘区域。
- `see` 每个元素有 `frac:{x,y,w,h}`（0–1）；窗口大小在 `window.w/h`。（本计划不用，留给以后。）
- `show_status {state, window?, text?, ttl_ms?, theme?, quiet?}`：`state` ∈ think|look|wait|stall|stuck|celebrate|won|sad|confused|tense|hopeful|note（后面几个**dco 还没装好**，没装好前会回 `bad_request`）；`text` ≤16 字；`theme` 目前认 `"candy"`、`"study"`，保持到换或发 `""`；`ttl_ms` 500–600000。只读档，锁屏也能用。**规则选步太快，不要发 think。**

## Global Constraints

- 提交信息用英文，**不加任何 AI 署名或 Co-Authored-By 行**（用户的长期规则）。不推送，用户说「推吧」才推；用户要自动，合并和推送前不再逐项问。
- 面向用户的文字用大白话中文。章鱼的说明条（`text`）≤16 个字。
- dct 代码保持通用：不出现任何游戏名；阈值是命名常量并注明「以后进 dcv 的规矩」；`theme` 来自配置文件的 `theme = "…"`（可选）。
- 旧 dco / 新 dco 都要行为正确：**没装新版 dco 时，一切和现在完全一样**（轮询 settle、只发 think/look）。任何章鱼状态发送失败都不许影响玩（`show_status` 本来就不返回错误）。
- 不发图。
- 推送前跑：`cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings` 和 `cargo +1.99.0 test --workspace --locked`（`PATH` 含 `$HOME/.cargo/bin`，`CARGO_TARGET_DIR=/private/tmp/claude-502/-Users-lei-work-dc-dc-terminal/d52f1f4f-cb49-40cc-ba88-c37403102743/scratchpad/target-199`）。已知偶发失败（机器忙时）：`daemon::web_tests::enabling_starts_a_listener_and_disabling_stops_it`、`session::tests::recovering_from_a_failure_after_real_input_still_does_not_count`、`list_is_not_blocked_by_slow_create`、`zombie_reaping`，单独重跑即可。

## Review Focus

0. dco 的隐私闸回 `private_screen`（`see`/`wait_for_text`）：`play` 一步都不划、`identify` 不读棋盘，说一句人话；`read_grid` 不被拦，所以不能只靠它判断。→ Task 6。
1. dco 返回 `settle:{error:…}`（划了，等待出错）：**不能重划**，也不能当成「没反应」；回退到现有的轮询去看结果。→ Task 1/2。
2. `timed_out: true`（5–8 秒还没稳定）：不当作稳定，也不当作没反应；回退到轮询，轮询自己会得出 `StillMoving` 等结论。→ Task 2。
3. 旧 dco 忽略 `settle` 参数（返回里没有 `settle` 字段）：识别成「不支持」，本次连接后面不再传，回退轮询。→ Task 1。
4. 新情绪状态在旧 dco 上回 `bad_request`：只记住「这个状态不支持」，**不能**连带停掉 think/look。→ Task 3。
5. 情绪狂闪：同一个情绪连着发、每步都发 `look`：只在**变化时**发；规则选步每步都不发 think。→ Task 4/5。

---

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

### Task 3: `show_status_with` 和不支持的状态记忆

**Files:**
- Modify: `crates/dct-game/src/play.rs`（trait）、`crates/dct-game/src/dco.rs`
- Test: `crates/dct-game/src/dco_tests.rs`

**Interfaces:**
- Produces: `Dco::show_status_with(&mut self, state: &str, text: Option<&str>, theme: Option<&str>)`，**默认实现**转调 `self.show_status(state)`（所以旧测试的假 dco 不变）。`DcoClient` 重写：发 `show_status {state, text?, theme?}`；如果回的是 `bad_request`（旧 dco 不认识这个状态或参数）→ 把这个 `state` 记进内部的 `unsupported_states: HashSet<String>`，以后同一个 state 不再发；**不**置位 `no_show_status`（那会停掉 think/look）。其他错误（`dco_too_old`、`dco_timeout`）仍按 `show_status` 现有的方式置位 `no_show_status`。`text` 超过 16 个字符时截断到 16（按字符不按字节）。

- [ ] **Step 1: 写失败的测试**
- 新状态在旧 dco 上回 `bad_request`：只发一次；之后同一状态不再发；**紧接着发 `think` 仍然发得出去**。
- `theme`、`text` 出现在发出的参数里；`text` 为 `None` 时不带这个键。
- `text` 20 个中文字 → 发出去的是前 16 个字。
- `dco_timeout` 仍然停掉之后所有 show_status（现有行为不变）。
- 默认实现：`Fake` 里 `show_status_with("stall", None, None)` 记成 `events` 里的 `"stall"`（转调）。
- 变异：把 `bad_request` 也置位 `no_show_status` → 「紧接着 think 仍发」变红。

- [ ] **Step 2–5：** 跑测试确认失败 → 实现 → 测试、clippy → 变异 → 提交：`feat(game): show_status_with carries text and theme; a state an older dco rejects is remembered per state without silencing the rest`。

---

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

### Task 5: 接线（`play` / `auto_next` / `identify` / 配置里的 `theme`）

**Files:**
- Modify: `crates/dct-game/src/play.rs`、`crates/dct-game/src/navigate.rs`、`src/game/identify.rs`、`src/game/profile.rs`（可选键 `theme`）、`crates/dct-game/src/play.rs` 的 `Profile`（加 `theme: Option<String>`，所有构造处补 `None`）、`src/game/skill.md`
- Test: `crates/dct-game/src/play_tests.rs`、`navigate.rs` 的测试、`src/game/identify.rs`、`src/game/profile.rs`

**Interfaces:**
- Consumes: Task 3 的 `show_status_with`；Task 4 的 `mood::{for_step, for_stop, for_genre}`、`GoalFinder::{step_index, goal_start}`。
- 行为：
  1. **开局一次**（非 dry-run）：`show_status_with("look", None, p.theme.as_deref())`（有 `theme` 才发；没有就什么都不发，和现在一样）。
  2. **每步划完、写完记录之后**：用这一步的 `chosen.features`、`stalled_now`、剩余步数（`finder.step_index()` 指的那个数字的 `progress_after`）、目标数字当前值（`gi` 指的那个数字的 `progress_after`）和 `finder.goal_start()` 算 `for_step`。结果是 `Some(m)` 且 `m.state` 和**上一次发给章鱼的状态不同** → `show_status_with(m.state, m.text, None)`；结果是 `None` 且上一次发的不是 `"look"` → 发 `look`。**同一个状态连着不重发。**
  2b. **问大模型的那段时间**（play() 里已有的 `show_status("think")` … `show_status("look")` 之间）：把 `think` 改成 `show_status_with("think", Some("问大模型中"), None)`（说明条「问大模型中」5 个字，≤16），问完仍回 `look`。这样用户一眼看出章鱼是在等模型，不是卡住。模型没回应（`down`）时发一次 `show_status_with("stall", Some("大模型没回应"), None)`。测试：问模型的那一步，`events` 里 think 带 `text == "问大模型中"`；规则选步的步不发 think。
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

### Task 6: 认出「私人画面」就停下，不操作

背景（dc-octo commit 01e1f32，2026-10-04）：dco 加了隐私闸。认出明显私人的画面（微信、信息、电话、相册、邮件、通讯录、设置、备忘录、手机主屏幕）时，`see`（含图）和 `wait_for_text` 返回错误 `private_screen`，消息「这个画面看起来是私人内容，没有给出」；`read_grid`（只回颜色）、点击、划动、`wait_idle` **不拦**。所以 dct 如果只在读不到字时当「没文字」，手机停在私人画面时仍可能继续划。

**Files:**
- Modify: `crates/dct-game/src/play.rs`（`Stop::PrivateScreen`、开局检查）、`crates/dct-game/src/navigate.rs`（出口）、`src/game/text.rs`（停止语句、`stop_code`）、`src/game/identify.rs`、`crates/dct-game/src/mood.rs`（`for_stop` 里 `PrivateScreen` → `wait`+「私人画面，没操作」）
- Test: `play_tests.rs`、`text.rs`、`identify.rs`

**Interfaces:**
- Produces: `Stop::PrivateScreen`；`DcoClient::see_text` 把 dco 的 `private_screen` 错误原样带出（`DcoError.code == "private_screen"`，现有代码本来就原样透传错误，确认即可）。
- 行为：
  1. `play()` 开局（非 dry-run、还没划任何一步）读到的 `see_text` 是 `Err` 且 `code == "private_screen"` → `break Stop::PrivateScreen`（一步都不划）。**之后**每步划完读 `progress` 时遇到 `private_screen` → 同样 `break Stop::PrivateScreen`（画面中途变成了私人界面）；记录该步 `outcome: "stopped"`。
  2. `dct game identify`：`see_text` 返回 `private_screen` → 打印「这个画面看起来是私人内容，我没有读它，也不会操作。请切回游戏。」，退出码 0（这不是错误）。
  3. 停止语句（`text::stop_line`）：`PrivateScreen` → 「这个画面看起来是私人内容，我没有读它，也不会操作。请切回游戏再运行。」，`stop_code` 为 `private_screen`，退出码 0。
  4. `--dry-run` 不读字，行为不变。
  5. 其他 `see_text` 错误（`unsupported`、超时等）行为不变。

- [ ] **Step 1: 写失败的测试**
- 开局 `see_text` 回 `private_screen`：`Stop::PrivateScreen`，`swipes.len() == 0`，没有任何 `read_grid` 之外的操作。
- 划了一步之后 `see_text` 回 `private_screen`：`Stop::PrivateScreen`，记录里最后一条 `outcome == "stopped"`。
- `unsupported` 仍然只是 `progress: null`，照常玩。
- `stop_line(PrivateScreen)` 的文字和退出码 0、`stop_code == "private_screen"`。
- `identify` 遇到 `private_screen`：打印那句话，不调用 `read_grid`（假 dco 对 `read_grid` panic 即可证明），退出码 0。
- `mood::for_stop(PrivateScreen)` → `wait` 带说明条，≤16 字符。
- 变异：把 `private_screen` 当普通错误吞掉 → 前两个测试变红。

- [ ] **Step 2–5：** 跑测试确认失败 → 实现 → 全部测试、clippy → 变异 → 提交：`feat(game): stop and say so when dco withholds a private screen; never swipe on one`。

---

## Self-Review

1. **覆盖：** 用户的两件事——(1) 等稳定 + 前后对比 → Task 1/2；(2) 章鱼情绪 → Task 3/4/5。dco 的 `see.frac` 和 `note/quiet/study` 不在本轮（写在「不做」）。
2. **不做：** `see` 的 `frac`（现在没有用它的地方）；`note`、`quiet`、`study` 主题（上网课场景，另一个功能）；把阈值搬进 dcv（等对接 dcv 那一轮）；`wait_idle` 单独调用（`swipe` 的 settle 已够用，轮询回退保留）。
3. **类型一致：** `SwipeOutcome`/`SwipeSettle`（Task 1）→ Task 2；`show_status_with`（Task 3）→ Task 5；`Mood`、`for_step/for_stop/for_genre`、`step_index/goal_start`（Task 4）→ Task 5；`Profile.theme`（Task 5）。
4. **Review Focus：** 五条分别落在 Task 1/2（error、timed_out、旧 dco）、Task 3（bad_request 不连带）、Task 4/5（只在变化时发、规则选步不发 think）。
