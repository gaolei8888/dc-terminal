### Task 8: TUI「我的电脑」一栏、加入确认、留言计数

**Files:**
- Modify: `src/ui/board.rs`（`draw`）
- Modify: `src/ui/mod.rs`（按键 y/n 处理加入确认；每 5 秒请求一次 `MeshStatus`，不在绘制路径里做网络请求）
- Modify: `src/i18n.rs`

**行为：**
- 看板底部，在帮助行上面，加一段「我的电脑」：
  - 每台电脑一行：`● 家里Mac  本机` / `● 公司Windows  在线` / `○ 云服务器  离线`；
  - 没登录多电脑时只显示一行灰字：「多电脑未开启 · 运行 dct login」；
  - 最多显示 5 行，超出的显示「还有 N 台」。
- `pending` 不为空时，顶部显示一行黄字确认：「一台叫 {name} 的电脑想加入…数字是 {code} 吗？(y/n)」。y 或 n 发 `MeshApprove`。
  - 这一行出现时，y 和 n 只在看板视图里被它接管；**会话视图里不接管**，沿用「会话视图只吃 F 功能键」的规矩。
- 会话收到过留言，就在看板那一行末尾加「✉ N」：本次 dct 运行期间送进这个会话的条数，从 `MeshStatus` 带回。
- 不用 emoji 当图标的规矩：● ○ ✉ 是字符，不是彩色 emoji；按 `adaptive-color` 的设计选颜色。

- [ ] **Step 1: 失败的测试**：用 `ratatui::backend::TestBackend` 渲染，断言这些情况的输出：
  - 未登录那一行；
  - 三台电脑的行；
  - 超出 5 台时的「还有 N 台」；
  - 待确认那一行；
  - 在会话视图里按 y 不会发出 `MeshApprove`。
- [ ] **Step 2–4: 实现、通过**，外加 Windows 检查
- [ ] **Step 5: Commit** —— `git commit -m "feat(ui): my computers on the board, join prompt and message counts"`

---

