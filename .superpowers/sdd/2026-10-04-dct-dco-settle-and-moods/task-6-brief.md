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
