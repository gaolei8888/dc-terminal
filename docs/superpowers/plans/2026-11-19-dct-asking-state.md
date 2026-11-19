# dct「等你回答」状态 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 停在弹框上等人的会话判成 `SessionState::Asking`，不再冒充「干活中」。

**Architecture:** profile 新增可选 `ask_pattern`，`classify()` 按 error > ask > busy > idle 判定；会话持有编译好的 `ask_re`，`tick()` 每轮都重新分类（去掉 Asking 的跳过）。UI 不用改：`status_style(Asking)` 已经是琥珀色（`theme.rs::asking`），`dct ps`/`status --json` 读同一份状态。

**Tech Stack:** Rust，`regex`，现有 `SessionManager::tick()` 测试套路（`toml_with_sh` + `/bin/sh` 假 agent）。

**Spec:** `docs/superpowers/specs/2026-11-19-dct-asking-state-design.md`

## Global Constraints

- 没声明 `ask_pattern` 的 profile 行为一个字不变（现有测试全绿即证明）。
- claude 系 pattern 整行锚定：`(?m)^\s*Enter to confirm · Esc to cancel\s*$`（TOML 里用单引号字面串）。
- 不自动替用户点「信任」。
- 提交信息不加 Co-Authored-By（用户要求）。

## Review Focus

1. 正文里引用这句话（agent 的回答、本文档被 cat 到屏幕上）→ 不能判 Asking。Task 1 测试覆盖。
2. 用户答完弹框（屏幕变成 `esc to interrupt`）→ 必须翻回 Working。Task 2 测试覆盖。
3. 弹框和出错同屏 → Failed 优先。Task 1 测试覆盖。
4. 刚建的会话停在信任框 → 不推送手机：现有 `should_notify` 的 `first_input.is_empty()` 门挡住；Task 2 不改它，只在测试里断言状态。
5. claude 系五个 profile（claude/deepseek/glm/kimi/qwen-api）跑的是同一个 claude CLI → 同一个信任框；Task 1 对五个一起测。

---

### Task 1: `ask_pattern` + `classify()` 判定顺序

**Files:**
- Modify: `src/profile.rs`（`Profile` 结构体 ~L120 加字段；`error_regex` 旁加 `ask_regex`）
- Modify: `profiles/claude.toml`、`profiles/deepseek.toml`、`profiles/glm.toml`、`profiles/kimi.toml`、`profiles/qwen-api.toml`（加 `ask_pattern`，注释写明 2026-11-19 本机 11 个会话实测）
- Modify: `src/session.rs:97-130`（`classify`）及所有调用点（L2035，测试里 L2318/2338/2393/2438 等）
- Test: `src/session.rs` 的 `mod tests`

**Interfaces:**
- Produces: `Profile::ask_pattern: Option<String>`；`Profile::ask_regex(&self) -> Result<Option<regex::Regex>>`（错误信息 `"ask_pattern 不是合法正则: {p}"`）；
  `fn classify(text: &str, error_re: Option<&Regex>, ask_re: Option<&Regex>, busy_re: Option<&Regex>, idle_re: Option<&Regex>) -> Option<SessionState>`

- [ ] **Step 1: 写失败测试**

在 tests 里加常量 `CLAUDE_TRUST_DIALOG`，内容取实测屏（`Accessing workspace:` … `❯ No, exit` / `Yes, I trust this folder` / ` Enter to confirm · Esc to cancel`，每行前导一个空格，照抄原样）。再加：

```rust
#[test]
fn a_claude_family_trust_dialog_is_asking() {
    for name in ["claude", "deepseek", "glm", "kimi", "qwen-api"] {
        let p = crate::profile::Profile::builtin(name).unwrap();
        let state = classify(CLAUDE_TRUST_DIALOG,
            p.error_regex().unwrap().as_ref(), p.ask_regex().unwrap().as_ref(),
            p.busy_regex().unwrap().as_ref(), p.idle_regex().unwrap().as_ref());
        assert_eq!(state, Some(SessionState::Asking), "{name}");
    }
}

#[test]
fn quoting_the_dialog_footer_inside_prose_is_not_asking() {
    // agent 的回答里引用了这句；屏幕底下是正常空闲提示符
    let text = format!("最后一行写着 `Enter to confirm · Esc to cancel`。\n{CLAUDE_WAITING_FOR_YOU}");
    let p = crate::profile::Profile::builtin("claude").unwrap();
    assert_eq!(classify(&text, p.error_regex().unwrap().as_ref(), p.ask_regex().unwrap().as_ref(),
        p.busy_regex().unwrap().as_ref(), p.idle_regex().unwrap().as_ref()), Some(SessionState::Idle));
}

#[test]
fn error_beats_ask() {
    let text = format!("API Error: 529\n{CLAUDE_TRUST_DIALOG}");
    // 同上取 claude 的四条 regex → Some(Failed)
}

#[test]
fn ask_beats_busy() {
    // ask_re = "ASK", busy_re = "BUSY", idle_re = "IDLE"，text = "BUSY\nASK" → Some(Asking)
}
```

另在 `src/profile.rs` 测试里加 `bad_ask_pattern_is_an_error`（`ask_pattern = "["` → `ask_regex()` 返回 Err），仿照 `bad_busy_pattern_is_an_error`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib -- trust_dialog quoting_the_dialog error_beats_ask ask_beats_busy bad_ask_pattern`
Expected: 编译失败（`ask_regex` 不存在 / `classify` 参数个数不对）

- [ ] **Step 3: 实现**

`classify` 在 error 判定之后、`match (busy_re, idle_re)` 之前：`ask_re` 命中 → `Some(Asking)`。文档注释补一段「ask 为什么压过 busy、为什么整行锚定」（引 spec 的实测）。所有现有调用点在 error 之后插一个参数：生产调用点暂传 `None`（Task 2 接上），测试调用点传 `None`，`a_claude_family_*` 两条现有测试改传 `p.ask_regex()`。五个 profile toml 加：

```toml
# 弹框等人：Claude 的确认框（实测：启动时的「信任这个文件夹」）最后一行
# 独占一行写这句。整行锚定——agent 回答里引用这句话不能把会话判成等人。
ask_pattern = '(?m)^\s*Enter to confirm · Esc to cancel\s*$'
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib`
Expected: 全部 PASS（含原有 `a_claude_family_session_waiting_at_the_prompt_is_idle`、`..._mid_turn_is_working`）

- [ ] **Step 5: Commit**

```bash
git add src/profile.rs src/session.rs profiles/
git commit -m "feat(session): ask_pattern — a dialog waiting for the user is Asking, not Working"
```

### Task 2: 会话接上 `ask_re`，Asking 能翻回去

**Files:**
- Modify: `src/session.rs`（会话结构体 ~L537 `idle_re` 旁加 `ask_re: Option<regex::Regex>`；`create()` ~L1423 编译；构造处 ~L1531；`tick()` L2019 删掉 `if s.state == SessionState::Asking { continue; }`；L2033 的门加上 `|| s.ask_re.is_some()`；L2035 传 `s.ask_re.as_ref()`）
- Modify: `README.zh-CN.md`、`README.md`（状态说明处加一句：Claude 弹确认框时显示「等你回答」）
- Test: `src/session.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 1 的 `Profile::ask_regex`、五参 `classify`

- [ ] **Step 1: 写失败测试**

仿照 `busy_pattern_wins_over_idle_pattern`（~L5300）：

```rust
#[test]
fn asking_flips_back_to_working_once_answered() {
    // profile: command = ["/bin/sh","-c","echo ASK; sleep 1; printf '\\033[2J\\033[HBUSY'; sleep 5"]
    //          ask_pattern = "ASK", busy_pattern = "BUSY", idle_pattern = "IDLE", is_agent = false
    // 循环 tick 直到 state_of == Asking（5s 超时，消息「弹框屏应判 Asking」）
    // 再循环 tick 直到 state_of == Working（5s 超时，消息「答完弹框必须翻回 Working」）
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib asking_flips_back`
Expected: FAIL，第一段超时（`ask_re` 还没接上，屏幕只有 ASK 时 classify 返回 None，停在 Working）

- [ ] **Step 3: 实现**

按 Files 列的四处改。删 `continue` 前 `grep -rn "SessionState::Asking" src` 确认生产代码里没有任何地方在设 Asking（Bridge 目前不设）；若有，停下报告。

- [ ] **Step 4: 跑全量测试**

Run: `cargo test`
Expected: 全部 PASS。若 `tests/concurrency.rs` 里有已知 flaky（见 memory: openpty 并发竞争），单独重跑确认与本改动无关，并在报告里写明。

- [ ] **Step 5: 实机验证**

`cargo install --path . --root ~/.local`，`dct restart`（会等在忙的会话停下；**先问用户再跑**，restart 会影响全部会话），然后 `dct ps`：停在信任框的会话应显示「等你回答」。

- [ ] **Step 6: Commit**

```bash
git add src/session.rs README.md README.zh-CN.md
git commit -m "feat(session): wire ask_re into tick; Asking is re-classified every tick"
```
