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

