# dct 会话自动命名 —— 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 先修掉「消息发给了另一个 session」的根因（九宫格焦点漂移），再让守护进程在每个 agent 会话第一次干完活时用 LLM 起一个钉死不变的名字，四处显示从 `3 claude` 变成 `3 修登录白屏`。

**Architecture:** 命名整体抄 `SessionManager::request_explanation` 那条已经跑通的路 —— 一个 `Arc<Mutex<Option<String>>>` 槽 + 一个后台线程 + `complete_with_timeout`，**绝不在 tick 里同步等模型**。触发点挂在 `tick()` 已有的状态机上（`Working` → `Idle`/`Asking` 的第一次跃迁），跟 `Failed` 触发解释是同一处。协议只加一个 `#[serde(default)]` 的只读字段，不升协议号。

**Tech Stack:** Rust ≥ 1.24 toolchain（见 `Cargo.toml`）、ratatui、既有 `src/llm` 层、既有 `src/i18n` 词条表。

**设计文档：** `docs/superpowers/specs/2026-08-09-dct-session-auto-name-design.md`

## Global Constraints

- **提交信息用英文，不要 AI 署名行**（不加 `Co-Authored-By`）。
- **不升 `PROTOCOL_VERSION`。** 它现在是 6（`src/proto.rs:40`），这一版结束时还必须是 6。
- **不在 `tick()` 里同步调用模型。** tick 每 200ms 一轮，一次同步调用就能卡住整个守护进程，而卡住的 dct 和死掉的 agent 长得一模一样（`src/session.rs:693` 的原话）。
- **句子由界面组，守护进程只报码。** 唯一的例外是 LLM 生成的自由文本（`explanation` 已经开了这个口子），本版的名字走同一个例外，**不新增任何面向用户的英文/中文句子在守护进程侧**。
- **名字的语言跟用户输入走**，不跟界面语言走 —— 界面语言可以随时切（`l` 键，不重启 daemon），而名字是钉死的。
- **界面文案两种语言都要放得下 80 列**，仓库里已有的宽度测试是这条的守卫。
- **显示宽度用 `widgets::truncate` / `pad_to` 算，不用字符数** —— 中文一个字占两列。
- 每个 Task 结束时都必须干净：`cargo fmt`、`cargo clippy --all-targets -- -D warnings`、`cargo test` 三条全绿。

## 术语

- **tag / 名字** —— `SessionInfo.tag`，本版新增的那个稳定名字。
- **label** —— 界面上真正画出来的那一段：`tag` 非空就是 `tag`，空就退回 `profile`。

## File Structure

| 文件 | 这一版里的职责 |
|---|---|
| `src/ui/app.rs` | `refresh_rows()` 给九宫格 `focus` 加身份锚定（Task 1） |
| `src/proto.rs` | 无改动（确认协议号不动的回归测试放这里，Task 2） |
| `src/session.rs` | `SessionInfo.tag` 字段；`Session` 的 `first_input` / `first_input_sealed` / `name_slot`；`collect_first_input`；`clean_name`；`name_prompt`；`request_name`；`tick` 的触发点 |
| `src/ui/widgets.rs` | `session_label()` —— 「画哪一段」的唯一答案处 |
| `src/ui/board.rs` | 列表行改用 label |
| `src/ui/grid.rs` | 格子标题、回复框收件人改用 label |
| `src/ui/attach.rs` | 会话视图标题加上名字 |
| `README.md` / `README.zh-CN.md` | 记这个功能，以及「没配 LLM 时它安静下线」 |

---

### Task 1: 九宫格焦点按会话 id 锚定

这是用户报的「一个 session 的消息发给了另一个 session」的根因，**跟命名无关，独立可上线**。

**Files:**
- Modify: `src/ui/app.rs:274-305`（`refresh_rows`）
- Test: `src/ui/app.rs` 的 `mod tests`（同文件内联，跟仓库其余测试一致）

**Interfaces:**
- Consumes: 无
- Produces: 无新公开接口。`refresh_rows()` 的行为契约变成「光标**和**九宫格焦点都按会话身份找回原位」

- [ ] **Step 1: 写失败测试**

加在 `src/ui/app.rs` 的 `mod tests` 里，紧挨着已有的 `refresh_rows_clamps_the_grid_focus_into_the_new_range`：

```rust
    /// 焦点是**身份**，不是位置。前面的会话没了，格子整体前移，焦点必须
    /// 还站在原来那个会话上。
    ///
    /// 不修的话：`i 回一句` 的收件人取自 `visible.get(focus)`
    /// （`grid.rs`），焦点漂到哪儿消息就发给谁 —— 而 `s`（停止）和
    /// `u`（回滚）走同一条路，两个都不可撤销。
    #[test]
    fn refresh_rows_keeps_the_grid_focus_on_the_same_session() {
        let (mut app, _dir) = App::test_app();
        app.set_sessions(vec![sess(1, "/w/a"), sess(2, "/w/a"), sess(3, "/w/a")]);
        app.view = View::grid(2); // 焦点在 3 号身上

        // 1 号跑完停了。九宫格不画已停止的会话，后面两格整体前移一位。
        let mut gone = sess(1, "/w/a");
        gone.state = crate::session::SessionState::Stopped;
        app.set_sessions(vec![gone, sess(2, "/w/a"), sess(3, "/w/a")]);

        let visible = app.grid_sessions();
        assert_eq!(visible.len(), 2, "已停止的那个不进九宫格");
        let View::Grid { focus, .. } = app.view else {
            panic!("还该在九宫格里");
        };
        assert_eq!(
            visible[focus].id,
            3,
            "焦点必须还站在 3 号身上，实际站在 {} 号上",
            visible[focus].id
        );
    }
```

- [ ] **Step 2: 跑它，确认它红**

```bash
cargo test --lib ui::app::tests::refresh_rows_keeps_the_grid_focus_on_the_same_session
```

预期：FAIL，`焦点必须还站在 3 号身上，实际站在 2 号上`。

- [ ] **Step 3: 最小实现**

`src/ui/app.rs` 的 `refresh_rows()`：在函数开头（取列表光标锚点的**旁边**）加上焦点锚点 ——

```rust
    pub fn refresh_rows(&mut self) {
        let anchor = self
            .list_state
            .selected()
            .and_then(|i| super::view::anchor_of(&self.groups, &self.rows, i));
        // 九宫格焦点也要按身份锚定。**必须在重算之前取**，理由同上面那行：
        // 重算之后取到的是新列表里的东西，等于没锚。
        let grid_anchor = match &self.view {
            View::Grid { focus, .. } => self.grid_sessions().get(*focus).map(|s| s.id),
            _ => None,
        };
```

然后把函数末尾那段夹取整个换掉：

```rust
        // 焦点是身份，不是位置。会话增删会让格子整体平移，只夹取的话
        // 焦点会静默指到别的会话上 —— 而 `i` 的收件人、`Enter` 放大的
        // 那一格、`s`/`u` 作用的对象全都取自它，后两个不可撤销。
        // 锚点找不回来（那个会话真没了）才退回夹取。
        let visible_ids: Vec<u32> = self.grid_sessions().iter().map(|s| s.id).collect();
        let grid_last = visible_ids.len().saturating_sub(1);
        if let View::Grid { focus, .. } = &mut self.view {
            let clamped = (*focus).min(grid_last);
            *focus = grid_anchor
                .and_then(|id| visible_ids.iter().position(|x| *x == id))
                .unwrap_or(clamped);
        }
```

（`clamped` 先算出来再赋值，是为了绕开借用检查器 —— 闭包里再读 `*focus` 会跟 `&mut` 撞上。）

- [ ] **Step 4: 跑测试**

```bash
cargo test --lib ui::app
```

预期：新测试 PASS，`refresh_rows_clamps_the_grid_focus_into_the_new_range` 仍然 PASS
（它的旧断言在锚定下答案不变：焦点原本在 5 号身上，5 号在新列表里是第 1 格）。

- [ ] **Step 5: 全量跑一遍**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

预期：全绿。

- [ ] **Step 6: 提交**

```bash
git add src/ui/app.rs
git commit -m "fix: the grid focus stays on the session it was on, not the slot

A finished session drops out of grid_sessions(), every tile after it shifts
left, and the focus index silently lands on a different session. The reply
box addressed by 'i' takes its recipient from that index, so a message meant
for one agent went to another. Stop, roll back, and zoom read the same index.

The board list has anchored its cursor by identity since it was written;
the grid never did."
```

---

### Task 2: `SessionInfo` 加 `tag` 字段

守护进程侧先把字段和连线做出来，值恒为空串 —— 界面这一步完全不动，行为零变化。

**Files:**
- Modify: `src/session.rs:123-144`（`SessionInfo`）、`src/session.rs:417-432`（`list()`）
- Modify（补字段的测试 fixture）：`src/ui/app.rs`、`src/ui/grid.rs`、`src/ui/view.rs`
- Test: `src/session.rs` 的 `mod tests`

**Interfaces:**
- Produces: `SessionInfo.tag: String` —— 空串 = 还没起出来，界面退回 `profile`

- [ ] **Step 1: 写失败测试**

加在 `src/session.rs` 的 `mod tests` 里：

```rust
    /// 旧守护进程发来的 JSON 没有 `tag` 这个字段。必须补成空串而不是
    /// 反序列化失败 —— 这正是本版**不升协议号**的全部依据（同 `scroll`
    /// 字段当初的做法，见 `proto.rs` 里那条注释）。
    #[test]
    fn session_info_without_a_tag_field_still_parses() {
        let old = r#"{"id":3,"profile":"claude","dir":"/w/a",
                      "state":"Idle","activity":"","is_agent":true}"#;
        let s: SessionInfo = serde_json::from_str(old).expect("旧 JSON 必须还能读");
        assert_eq!(s.tag, "", "缺字段补空串");
        assert_eq!(s.id, 3);
    }

    /// 新建的会话还没起过名。
    #[test]
    fn a_fresh_session_has_no_tag() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();

        let tag = m.list().iter().find(|s| s.id == id).unwrap().tag.clone();
        assert_eq!(tag, "");
    }
```

- [ ] **Step 2: 跑它，确认它红**

```bash
cargo test --lib session::tests::session_info_without_a_tag_field_still_parses
```

预期：FAIL，编译错误 `no field 'tag' on type 'SessionInfo'`。

- [ ] **Step 3: 最小实现**

`src/session.rs`，`SessionInfo` 里 `is_agent` 之后加：

```rust
    /// 这个会话的稳定名字，守护进程在它第一次干完活时起一次，之后不变。
    ///
    /// 空串 = 还没起出来（刚建、没配 LLM、不是 agent 会话，或者对面是
    /// 认不得这个字段的旧守护进程）。**界面遇到空串一律退回 `profile`。**
    ///
    /// `#[serde(default)]` 是本版不升 `PROTOCOL_VERSION` 的依据：加纯读
    /// 字段时旧 JSON 补默认值，而 serde 反序列化本来就忽略不认识的字段，
    /// 所以新旧界面/守护进程怎么搭配都不会炸，只是没有名字。
    #[serde(default)]
    pub tag: String,
```

`Session` 结构体（`src/session.rs:146` 起，`explanation_slot` 旁边）加：

```rust
    /// 会话起名用的槽。跟 `explanation_slot` 平级、同一套用法。
    /// `None` = 还没触发过起名；`Some(_)` = 已经触发过（**只触发一次**）。
    name_slot: Arc<Mutex<Option<String>>>,
```

构造处（`src/session.rs:375` 附近，`explanation_slot` 那一行旁边）：

```rust
            name_slot: Arc::new(Mutex::new(None)),
```

`list()` 里（`src/session.rs:421` 的 `SessionInfo { .. }`）加一行：

```rust
                    tag: recover(s.name_slot.lock()).clone().unwrap_or_default(),
```

- [ ] **Step 4: 补齐测试 fixture**

`cargo test` 会点名所有构造 `SessionInfo` 的地方。已知三处，各加 `tag: String::new(),`：
`src/ui/app.rs` 的 `fn sess`、`src/ui/grid.rs:848` 和 `:859` 的 fixture、`src/ui/view.rs:1360`。
编译器报到哪补到哪，不要漏。

- [ ] **Step 5: 跑测试**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

预期：全绿。

- [ ] **Step 6: 确认协议号真没动**

```bash
grep -n "PROTOCOL_VERSION: u32" src/proto.rs
```

预期：`pub const PROTOCOL_VERSION: u32 = 6;`

- [ ] **Step 7: 提交**

```bash
git add -A
git commit -m "feat: carry a per-session name on the wire, empty for now

serde(default) keeps this off the protocol version: an old daemon's JSON
parses into an empty tag, and a new daemon's extra field is ignored by an
old UI. Bumping the version would have been expensive here, because ps,
stop, kill and prune never joined the version handshake and would answer a
mismatch with raw serde text."
```

---

### Task 3: 采集第一句用户输入

**Files:**
- Modify: `src/session.rs`（`Session` 加两个字段；`send_input` 开头调一次；新增自由函数 `collect_first_input`）
- Test: `src/session.rs` 的 `mod tests`

**Interfaces:**
- Produces:
  - `const FIRST_INPUT_MAX: usize = 200;`
  - `pub(crate) fn collect_first_input(buf: &mut String, sealed: &mut bool, text: &str)`
  - `Session.first_input: String` / `Session.first_input_sealed: bool`

两个客户端送输入的形状不同，都要接住：**会话视图**逐键转发，回车到达时 `text` 是 `"\r"`；
**九宫格 `i` 回一句**先发整段 body，再发一次**空 `Input`**（空 = 按回车，见 `src/ui/grid.rs:600-612`）。

- [ ] **Step 1: 写失败测试**

```rust
    /// 逐键送和整段送必须封存出同一句话 —— 会话视图是一个键一次
    /// `Input`，九宫格 `i` 是整段 + 一次空 `Input`。
    #[test]
    fn both_input_paths_seal_the_same_first_line() {
        let mut a = (String::new(), false);
        for k in ["h", "i", "\r"] {
            collect_first_input(&mut a.0, &mut a.1, k);
        }

        let mut b = (String::new(), false);
        collect_first_input(&mut b.0, &mut b.1, "hi");
        collect_first_input(&mut b.0, &mut b.1, "");

        assert_eq!(a.0, "hi");
        assert_eq!(b.0, "hi");
        assert!(a.1 && b.1, "两条路都要封存");
    }

    /// 封存之后再送字，第一句不再变 —— 它是「第一句」，不是「最近一句」。
    #[test]
    fn sealed_first_input_never_changes_again() {
        let mut buf = String::new();
        let mut sealed = false;
        collect_first_input(&mut buf, &mut sealed, "hi");
        collect_first_input(&mut buf, &mut sealed, "");
        collect_first_input(&mut buf, &mut sealed, "and more");
        assert_eq!(buf, "hi");
    }

    /// 粘一大段需求进来：只留前 200 个字符，剩下的不进内存。
    #[test]
    fn a_pasted_wall_of_text_is_capped() {
        let mut buf = String::new();
        let mut sealed = false;
        collect_first_input(&mut buf, &mut sealed, &"x".repeat(300));
        assert_eq!(buf.chars().count(), FIRST_INPUT_MAX);
        assert!(!sealed, "没按回车就不算封存");
    }

    /// 一次送进来的字里就带着回车（粘贴多行）：回车之前的算第一句，
    /// 回车本身封存。
    #[test]
    fn a_newline_inside_one_chunk_seals_at_the_newline() {
        let mut buf = String::new();
        let mut sealed = false;
        collect_first_input(&mut buf, &mut sealed, "fix login\nand also");
        assert_eq!(buf, "fix login");
        assert!(sealed);
    }
```

- [ ] **Step 2: 跑它，确认它红**

```bash
cargo test --lib session::tests::both_input_paths_seal_the_same_first_line
```

预期：FAIL，编译错误 `cannot find function 'collect_first_input'`。

- [ ] **Step 3: 最小实现**

`src/session.rs`，放在 `explain_prompt` 旁边（都是「喂模型的原料」这一类）：

```rust
/// 第一句输入最多留这么多字符。粘一大段需求时前 200 字足够喂模型，
/// 把几千字留在内存里没有意义。
const FIRST_INPUT_MAX: usize = 200;

/// 攒「用户对这个会话说的第一句话」。
///
/// 抽成自由函数是因为两个客户端送输入的形状完全不同（会话视图逐键、
/// 九宫格整段 + 一次空 `Input`），而这条规则必须对两条路给出同一个答案 ——
/// 那是能测的，`send_input` 里那一圈锁和 PTY 写入不是。
///
/// `text` 为空 = 按回车（见 `send_input` 的文档）。
pub(crate) fn collect_first_input(buf: &mut String, sealed: &mut bool, text: &str) {
    if *sealed {
        return;
    }
    if text.is_empty() {
        *sealed = true;
        return;
    }
    // `find` 给的是字节下标，而 `\r`/`\n` 都是 ASCII，切在这里一定是
    // 合法的字符边界。
    match text.find(['\r', '\n']) {
        Some(i) => {
            append_capped(buf, &text[..i]);
            *sealed = true;
        }
        None => append_capped(buf, text),
    }
}

/// 按**字符数**封顶追加。这里不按显示宽度算：这段字是喂给模型的原料，
/// 不是画在屏幕上的东西，宽度是界面那一侧的事。
fn append_capped(buf: &mut String, text: &str) {
    for ch in text.chars() {
        if buf.chars().count() >= FIRST_INPUT_MAX {
            return;
        }
        buf.push(ch);
    }
}
```

`Session` 结构体加两个字段（挨着 `name_slot`）：

```rust
    /// 用户对这个会话说的第一句话，起名用。只在 agent 会话上攒。
    first_input: String,
    /// 第一句攒完了没有。见 `collect_first_input`。
    first_input_sealed: bool,
```

构造处加：

```rust
            first_input: String::new(),
            first_input_sealed: false,
```

`send_input`（`src/session.rs:441`）开头，取到 `arc` 之后、别的都还没做之前：

```rust
        {
            // 攒第一句。**在所有分支之前**——下面空串那一支会提早 return，
            // 挂在它后面就永远收不到回车。
            let mut s = recover(arc.lock());
            if s.is_agent {
                let (buf, sealed) = (&mut s.first_input, &mut s.first_input_sealed);
                collect_first_input(buf, sealed, text);
            }
        }
```

> 借用两个字段要写成上面那样先解构，直接 `collect_first_input(&mut s.first_input, &mut s.first_input_sealed, text)` 也可以 —— 两者都能过借用检查（不相交字段），取编译器不报错的那个。

- [ ] **Step 4: 跑测试**

```bash
cargo test --lib session
```

预期：四个新测试 PASS。

- [ ] **Step 5: 全量 + 提交**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add src/session.rs
git commit -m "feat: remember the first thing a user says to an agent session

The attached view sends one Input per keystroke and the grid's reply box
sends the whole body plus an empty Input for Enter. One rule has to seal the
same sentence from both, so it lives in a free function that can be tested
without a PTY."
```

---

### Task 4: 洗模型返回值 + 起名 prompt

两个纯函数，先独立做出来 —— 它们是这个功能里唯一有「答案对不对」可言的部分。

**Files:**
- Modify: `src/session.rs`（`explain_prompt` 旁边）
- Test: `src/session.rs` 的 `mod tests`

**Interfaces:**
- Produces:
  - `const NAME_MAX_CHARS: usize = 24;`
  - `pub(crate) fn clean_name(raw: &str) -> String`
  - `pub fn name_prompt(first_input: &str, screen: &str) -> crate::llm::Prompt`

- [ ] **Step 1: 写失败测试**

```rust
    /// 模型多半会回一句带标点、带引号的话，不会老老实实只给名字。
    /// 洗不干净的话，格子标题上会出现「「修登录白屏」。」这种东西。
    #[test]
    fn clean_name_strips_quotes_punctuation_and_extra_lines() {
        assert_eq!(clean_name("「修登录白屏」。"), "修登录白屏");
        assert_eq!(clean_name("\"fix login blank\""), "fix login blank");
        assert_eq!(clean_name("修登录白屏\n（这个会话在修登录）"), "修登录白屏");
        assert_eq!(clean_name("  修登录白屏  "), "修登录白屏");
    }

    /// 洗完是空的就当模型没答上来，调用方走兜底。
    #[test]
    fn clean_name_returns_empty_when_there_is_nothing_left() {
        assert_eq!(clean_name(""), "");
        assert_eq!(clean_name("   \n  "), "");
        assert_eq!(clean_name("。。。"), "");
    }

    /// 模型不听话给了一长串：按字符数封顶，别让它撑爆标题。
    #[test]
    fn clean_name_caps_a_runaway_answer() {
        let long = "修".repeat(100);
        assert_eq!(clean_name(&long).chars().count(), NAME_MAX_CHARS);
    }

    /// prompt 必须带上第一句输入和屏幕末尾两样，缺一样模型就只能猜。
    #[test]
    fn name_prompt_carries_both_the_first_line_and_the_screen() {
        let p = name_prompt("修一下登录白屏", "…… 正在改 auth.ts ……");
        assert!(p.user.contains("修一下登录白屏"));
        assert!(p.user.contains("auth.ts"));
        assert!(p.max_tokens <= 64, "起个名字不需要长回答");
    }
```

- [ ] **Step 2: 跑它，确认它红**

```bash
cargo test --lib session::tests::clean_name_strips_quotes_punctuation_and_extra_lines
```

预期：FAIL，编译错误 `cannot find function 'clean_name'`。

- [ ] **Step 3: 最小实现**

```rust
/// 名字最多留这么多字符。**按字符数、不按显示宽度**：守护进程存的是
/// 一段文字，画多宽是界面那一侧按各自的位置算的（见 `widgets::truncate`）。
/// 24 是 12 个汉字，跟 prompt 里要的「不超过 12 个字」对得上。
const NAME_MAX_CHARS: usize = 24;

/// 把模型回的东西洗成一个能直接画在标题上的名字。
///
/// 模型很少老老实实只给名字：会加引号、会加句号、会多写一句解释。
/// 洗不干净的话屏幕上就会出现「「修登录白屏」。」。洗完是空串表示
/// 这次没拿到可用的答案，调用方走兜底。
pub(crate) fn clean_name(raw: &str) -> String {
    const QUOTES: [char; 12] = ['"', '\'', '「', '」', '『', '』', '“', '”', '‘', '’', '《', '》'];
    const TAIL: [char; 12] = ['。', '．', '.', '，', ',', '！', '!', '？', '?', '；', ';', '、'];

    let line = raw.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line.trim_matches(|c: char| QUOTES.contains(&c) || c.is_whitespace());
    let line = line.trim_end_matches(|c: char| TAIL.contains(&c));
    let line = line.trim();
    line.chars().take(NAME_MAX_CHARS).collect()
}

/// 让模型给这个会话起个名字。
///
/// **只送屏幕末尾**，理由同 `explain_prompt`：整屏几千字，又慢又贵，
/// 还容易让模型抓错重点。
///
/// **语言写进 prompt，不做参数**：名字由守护进程生成并钉死，而界面语言
/// 用户随时能切（`l` 键，不重启 daemon）。跟着用户输入的语言走，切界面
/// 语言之后也不会留下一堆对不上的名字。
pub fn name_prompt(first_input: &str, screen: &str) -> crate::llm::Prompt {
    const TAIL: usize = 2000;
    let tail: String = {
        let chars: Vec<char> = screen.chars().collect();
        let start = chars.len().saturating_sub(TAIL);
        chars[start..].iter().collect()
    };
    crate::llm::Prompt {
        system: "给下面这个编程会话起一个名字，好让人在一屏几个会话里认出它。\
                 只回名字本身，不超过 12 个字。说的是这个会话在做的**任务**，\
                 不是它此刻的动作。不要引号、不要标点、不要「任务」「会话」\
                 这类没有信息的词。**用与用户那句话相同的语言。**"
            .into(),
        user: format!("用户说的第一句话：\n{first_input}\n\n屏幕上的最后一段内容：\n\n{tail}"),
        max_tokens: 64,
    }
}
```

- [ ] **Step 4: 跑测试**

```bash
cargo test --lib session::tests::clean_name
cargo test --lib session::tests::name_prompt
```

预期：四个都 PASS。

- [ ] **Step 5: 全量 + 提交**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add src/session.rs
git commit -m "feat: ask a model for a session name, and scrub what comes back

Models answer with quotes, a full stop, and often a second sentence of
explanation. Unscrubbed that lands in a tile title verbatim."
```

---

### Task 5: 触发起名

**Files:**
- Modify: `src/session.rs`（新增 `request_name`；`tick()` 里加触发点）
- Test: `src/session.rs` 的 `mod tests`

**Interfaces:**
- Consumes: `collect_first_input`（Task 3）、`clean_name` / `name_prompt`（Task 4）、`Session.name_slot`（Task 2）
- Produces: `fn request_name(&self, s: &mut Session)`（私有）

**触发条件**（四个全真）：这一轮状态从 `Working` 变成 `Idle` 或 `Asking`、是 agent 会话、
`name_slot` 还是 `None`。**`name_slot` 非 `None` 就是「已经触发过」**——因为触发那一刻会同步写入
兜底值（可能是空串），所以它同时兼任「只做一次」的标志位，不必另加一个 bool。

- [ ] **Step 1: 写失败测试**

装后端走既有入口：`SessionManager::set_backend(&self, b: Option<Arc<dyn crate::llm::Backend>>)`
（`src/session.rs:238`）。仓库里的既有测试（`entering_failed_asks_the_backend_once_not_every_tick`）
把假后端声明成**测试函数内部的局部 struct**，照那个风格写：

```rust
    struct FixedBackend(String);
    impl crate::llm::Backend for FixedBackend {
        fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
            Ok(self.0.clone())
        }
    }

    struct DeadBackend;
    impl crate::llm::Backend for DeadBackend {
        fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
            Err(crate::llm::LlmError::Unavailable)
        }
    }

    /// 起名的正路：干完一轮活，名字就出来了。
    #[test]
    fn a_session_gets_named_after_its_first_round_of_work() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        m.set_backend(Some(Arc::new(FixedBackend("「修登录白屏」。".into()))));
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();

        m.send_input(id, "修一下登录白屏").unwrap();
        m.send_input(id, "").unwrap(); // 空字符串 = 回车，状态进 Working

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            let tag = m.list().iter().find(|s| s.id == id).unwrap().tag.clone();
            if tag == "修登录白屏" {
                break;
            }
            assert!(Instant::now() < deadline, "一直没起出名字，最后是 {tag:?}");
            sleep(Duration::from_millis(50));
        }
    }

    /// **钉死**：再干一轮，名字不变。这是「只起一次」唯一测得到的地方。
    #[test]
    fn a_name_is_pinned_and_never_asked_for_twice() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        m.set_backend(Some(Arc::new(FixedBackend("第一个名字".into()))));
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();

        m.send_input(id, "干活").unwrap();
        m.send_input(id, "").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            if m.list().iter().find(|s| s.id == id).unwrap().tag == "第一个名字" {
                break;
            }
            assert!(Instant::now() < deadline, "第一次就没起出来");
            sleep(Duration::from_millis(50));
        }

        // 换一个会给别的答案的后端，再走一轮 Working → Idle
        m.set_backend(Some(Arc::new(FixedBackend("第二个名字".into()))));
        m.send_input(id, "再干一轮").unwrap();
        m.send_input(id, "").unwrap();
        for _ in 0..20 {
            m.tick();
            sleep(Duration::from_millis(50));
        }

        assert_eq!(
            m.list().iter().find(|s| s.id == id).unwrap().tag,
            "第一个名字",
            "名字是钉死的，第二轮不该重起"
        );
    }

    /// 模型答不上来（或者压根没配后端）时，名字停在第一句输入上，
    /// 不是空着。
    #[test]
    fn a_dead_model_leaves_the_first_line_as_the_name() {
        let repo = init_repo();
        let m = SessionManager::new();
        m.register_profile(fake_agent());
        m.set_backend(Some(Arc::new(DeadBackend)));
        let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();

        m.send_input(id, "修一下登录白屏").unwrap();
        m.send_input(id, "").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            m.tick();
            let tag = m.list().iter().find(|s| s.id == id).unwrap().tag.clone();
            if tag == "修一下登录白屏" {
                break;
            }
            assert!(Instant::now() < deadline, "兜底没生效，最后是 {tag:?}");
            sleep(Duration::from_millis(50));
        }
    }
```

- [ ] **Step 2: 跑它，确认它红**

```bash
cargo test --lib session::tests::a_session_gets_named_after_its_first_round_of_work
```

预期：FAIL —— 名字一直是空串。

- [ ] **Step 3: 最小实现**

`src/session.rs`，紧挨着 `request_explanation`：

```rust
    /// 给这个会话起个名字。**只在它第一次干完活时调用一次**（见 `tick`）。
    ///
    /// 跟 `request_explanation` 是同一条路，但**不需要 generation 计数器**：
    /// 失败会反复发生、迟到的旧解释会盖掉新解释，而名字一辈子只问一次，
    /// 全程只有一个线程可能写这个槽。
    fn request_name(&self, s: &mut Session) {
        // 先把兜底同步写进去：从这一刻起 `name_slot` 就是 `Some(_)`，
        // 它同时兼任「已经起过名了」的标志位。模型答得出就覆盖，
        // 答不出就把第一句留在这儿。
        let fallback: String = s.first_input.chars().take(NAME_MAX_CHARS).collect();
        *recover(s.name_slot.lock()) = Some(fallback);

        let Some(b) = recover(self.backend.lock()).clone() else {
            return; // 没配后端：功能安静下线，兜底那句留着
        };
        let p = name_prompt(&s.first_input, &s.pty.screen_text());
        let slot = s.name_slot.clone();
        std::thread::spawn(move || {
            // 15 秒，比 `explanation` 的 30 秒短：那个是用户正等着看解释，
            // 这个是后台起名，没人等，等太久只是白占一个线程。
            if let Ok(text) =
                crate::llm::complete_with_timeout(b, p, std::time::Duration::from_secs(15))
            {
                let name = clean_name(&text);
                if !name.is_empty() {
                    if let Ok(mut g) = slot.lock() {
                        *g = Some(name);
                    }
                }
            }
            // 失败就什么都不做——兜底那句已经在槽里了
        });
    }
```

`tick()` 里（`src/session.rs:678-687`），在 `Failed` 那个 `if` 后面加：

```rust
                    // 第一次干完活 = 起名的时机。不在第一句输入送出去时起：
                    // 那一刻信息最少，正是「继续」「帮我看看」出现的地方；
                    // 干完一轮之后屏幕上才有它到底在做什么的实证。
                    //
                    // `name_slot` 非 None 就是已经起过了（`request_name` 一进门
                    // 就同步写兜底），所以这个条件同时管住了「只起一次」。
                    if was == SessionState::Working
                        && matches!(next, SessionState::Idle | SessionState::Asking)
                        && s.is_agent
                        && recover(s.name_slot.lock()).is_none()
                    {
                        self.request_name(&mut s);
                    }
```

- [ ] **Step 4: 跑测试**

```bash
cargo test --lib session::tests::a_session_gets_named_after_its_first_round_of_work
cargo test --lib session::tests::a_name_is_pinned_and_never_asked_for_twice
cargo test --lib session::tests::a_dead_model_leaves_the_first_line_as_the_name
```

预期：三个都 PASS。

- [ ] **Step 5: 变异测试（必做）**

计划里的代码不是权威 —— 上一轮滚屏就是照着计划写的代码里带着三个真 bug。
手动改坏实现，确认测试真的会红：

1. 把触发条件里的 `&& recover(s.name_slot.lock()).is_none()` 删掉
   → `a_name_is_pinned_and_never_asked_for_twice` 必须 FAIL
2. 把 `request_name` 开头那两行兜底删掉
   → `a_dead_model_leaves_the_first_line_as_the_name` 必须 FAIL
3. 把 `was == SessionState::Working` 换成 `true`
   → 至少有一个测试 FAIL（起名会在第一次 tick 就发生，那时 `first_input` 还是空的）

三个都确认之后把改动还原。任何一个改坏了测试还是绿的，说明那条测试没测到东西，
**当场把它修好再往下走**。

- [ ] **Step 6: 全量 + 提交**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add src/session.rs
git commit -m "feat: name a session the first time it finishes a round of work

Not when the first line is sent — that is the moment with the least
information, and the moment 'continue' gets typed. One shot per session: the
fallback is written into the slot synchronously, which is also what marks the
session as already named."
```

---

### Task 6: 四处显示

**Files:**
- Create（函数）: `src/ui/widgets.rs` 的 `session_label`
- Modify: `src/ui/board.rs:205-212`（列表行）、`src/ui/grid.rs:358-361`（回复框收件人）、
  `src/ui/grid.rs:469`（格子标题）、`src/ui/attach.rs:220-231`（会话视图标题）
- Test: 各自文件的 `mod tests`

**Interfaces:**
- Produces: `pub(crate) fn session_label(s: &crate::session::SessionInfo) -> &str`

- [ ] **Step 1: 写失败测试**

`src/ui/widgets.rs` 的 `mod tests`：

```rust
    /// 「画哪一段」只有这一个答案处。散在四个视图里各判一次，迟早分叉成
    /// 「列表写着名字、格子写着 profile」。
    #[test]
    fn session_label_falls_back_to_the_profile_when_there_is_no_tag() {
        let mut s = crate::session::SessionInfo {
            id: 3,
            profile: "claude".into(),
            dir: "/w/a".into(),
            state: crate::session::SessionState::Idle,
            activity: String::new(),
            is_agent: true,
            tag: String::new(),
        };
        assert_eq!(session_label(&s), "claude");
        s.tag = "修登录白屏".into();
        assert_eq!(session_label(&s), "修登录白屏");
    }
```

`src/ui/grid.rs` 的 `mod tests`（回复框收件人 —— 这是用户被咬的那一处）：

```rust
    /// 回复框写的是名字，不是 `3 claude` —— 一个项目挂三个 claude 时，
    /// 后者三处写的东西一模一样，用户没法在按 Enter 之前核对发给了谁。
    #[test]
    fn the_reply_box_is_addressed_by_name() {
        // 用本文件既有的 fixture 造一个带 tag 的会话，打开回复框，
        // 断言画出来的收件人里有 "修登录白屏"、没有 "claude"。
        // （照抄 `the_reply_box_names_who_it_is_addressed_to` 的骨架，
        // 只把 profile 换成 tag，断言换成上面这一条。）
    }
```

> 上面这一处**必须写成真代码再跑**，不能留成注释。做法：先读
> `src/ui/grid.rs` 里已有的 `the_reply_box_names_who_it_is_addressed_to`，
> 照它的骨架复制一份，给 fixture 填上 `tag`，断言换掉。

- [ ] **Step 2: 跑它，确认它红**

```bash
cargo test --lib ui::widgets::tests::session_label_falls_back_to_the_profile_when_there_is_no_tag
```

预期：FAIL，`cannot find function 'session_label'`。

- [ ] **Step 3: 实现 `session_label`**

`src/ui/widgets.rs`：

```rust
/// 界面上代表一个会话的那一段文字：有名字就是名字，没有就退回 profile。
///
/// **四个视图共用这一个答案处。** 各判各的迟早分叉成「列表写着名字、
/// 格子写着 claude」，而这个功能存在的全部理由就是让同一个会话在哪儿
/// 看都是同一个东西。
pub(crate) fn session_label(s: &crate::session::SessionInfo) -> &str {
    if s.tag.is_empty() {
        &s.profile
    } else {
        &s.tag
    }
}
```

- [ ] **Step 4: 改四处显示**

**列表行**（`src/ui/board.rs:205-212`）—— profile 那一格 10 列换成名字 16 列，
`activity` 从 76 收到 70 补回去（一行的总宽度不变）：

```rust
                super::view::Row::Session(_, si) => {
                    let s = &g.sessions[*si];
                    spans.push(Span::raw(format!("  {:>3}  ", s.id)));
                    spans.push(Span::styled(
                        pad_to(status_label(s.state, app.lang), 8),
                        status_style(s.state),
                    ));
                    // 名字比原来的 profile 那一格宽（10 → 16）：profile 名最长
                    // 8 列，名字是 12 个汉字。多出来的 6 列从 activity 那边收，
                    // 整行总宽不变。传 15 给 truncate 而不是 16 —— 它真裁了的
                    // 时候返回的是 max + 1 列（那个 `…` 是长度判断之后才追加的），
                    // 照 16 传的话省略号会把列宽顶宽一格。
                    spans.push(Span::raw(pad_to(&truncate(session_label(s), 15), 16)));
                    // 会话行不重复项目名——组头已经说了，宽度还给 activity，
                    // 它是屏幕上最先被截断的信息。
                    spans.push(Span::raw(truncate(&s.activity, 70)));
                }
```

**格子标题**（`src/ui/grid.rs:469`）：

```rust
            Span::raw(format!("{} {} ", info.id, truncate(session_label(info), 20))),
```

**回复框收件人**（`src/ui/grid.rs:358-361`）：

```rust
        let who = visible
            .iter()
            .find(|s| s.id == draft.id)
            .map(|s| format!("{} {}", s.id, session_label(s)))
            // 收件人在打字途中被停掉了。仍然照实写出 id——用户得看得见
            // 自己正在对谁说话，哪怕那个会话刚没了。
            .unwrap_or_else(|| draft.id.to_string());
```

**会话视图标题**（`src/ui/attach.rs:220-231`）：

```rust
    // 标题显示用户当初指定的项目目录，不是内部的 worktree 路径——
    // 给用户看 .git/dct-worktrees/s2 只会让他不知道自己在哪。
    //
    // 有名字就把它接在项目后面。**不动 `session_title` 的签名**：那两条
    // i18n 词条已经被宽度测试盯着，往里加参数等于要同时改两种语言的
    // 句式；接在 `project` 后面是纯拼接，句式不动。
    let here = app
        .sessions
        .iter()
        .find(|s| s.id == id)
        .map(|s| {
            let project = short_path(&s.dir);
            if s.tag.is_empty() {
                project
            } else {
                format!("{project} · {}", s.tag)
            }
        })
        .unwrap_or_default();
    let title = if app.connected {
        crate::i18n::msg::session_title(app.lang, id, &here)
    } else {
        crate::i18n::msg::session_title_disconnected(app.lang, id, &here)
    };
```

**import 要动两处**（已核对过现状）：

- `src/ui/board.rs:8` 已经有 `truncate`，只需把 `session_label` 加进那一行：
  `use super::widgets::{pad_to, session_label, status_label, status_style, truncate};`
- `src/ui/grid.rs:14` **没有** `truncate`，两个都要加：
  `use super::widgets::{char_width, pad_to, screen_to_lines, session_label, status_label, status_style, truncate};`
- `src/ui/attach.rs` 不需要新 import —— 那一处直接读 `s.tag`，没用 `session_label`
  （标题那句是「项目 · 名字」，`tag` 为空时整段不出现，跟别处「退回 profile」不是同一条规则）。

- [ ] **Step 5: 跑测试**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

预期：全绿。**已有的宽度测试**（80 列 / 60 列那批）是这一步的守卫；
有测试红了说明列宽算错了，回到上一步调数字，不要改测试。

- [ ] **Step 6: 提交**

```bash
git add src/ui/
git commit -m "feat: show a session by its name everywhere it appears

Three claude sessions in one project used to read '3 claude', '5 claude',
'7 claude' — the same string with a different number, in all three places
you might check before pressing Enter. One helper answers 'what do we draw
for this session' so the four views cannot drift apart."
```

---

### Task 7: 两份 README

**Files:**
- Modify: `README.md`、`README.zh-CN.md`

- [ ] **Step 1: 找到该改的段落**

```bash
grep -n "claude\b" README.md README.zh-CN.md | head -30
```

看板/九宫格那一节里出现 `3 claude` 这类示例的地方，以及「会让你不爽的地方」那一节。

- [ ] **Step 2: 写进去**

两份都要写，内容对齐（英文那份不是中文那份的翻译，但事实必须一致）：

- 会话会自动得到一个名字，在第一次干完活时由模型起一次，**之后不再变**
- **没配 LLM 时这个功能安静下线**，名字退回你说的第一句话，再退回 agent 名 —— 会话照跑
- 名字跟着**你输入的语言**走，不跟界面语言走
- 名字不能手改（这一版没有重命名）

顺带把示例里的 `3 claude` 更新成带名字的样子。

- [ ] **Step 3: 提交**

```bash
git add README.md README.zh-CN.md
git commit -m "docs: sessions carry a name now, and it degrades quietly"
```

---

### Task 8: 收尾自检

- [ ] **Step 1: 三条全绿**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

- [ ] **Step 2: 协议号确实没动**

```bash
git diff main --stat -- src/proto.rs
grep -n "PROTOCOL_VERSION: u32" src/proto.rs
```

预期：`proto.rs` 零改动，协议号还是 6。

- [ ] **Step 3: 实机跑一遍（这一步不能省）**

```bash
cargo build --release
```

用**新二进制**起一个守护进程，开两个 claude 会话在同一个项目里，各说一句不同的话，
等它们各自干完一轮，确认：

1. 两个格子的标题是两个不同的名字，不是 `claude` / `claude`
2. 停掉其中一个，剩下那个的焦点没有漂（Task 1 的实机验证）
3. `i` 回一句时回复框写的是名字
4. 把 `[llm]` 配置临时去掉再来一次：名字退回第一句话，界面没有任何报错

第 4 条**必须真的试**：这个功能最常见的运行状态就是「用户没配 LLM」。

- [ ] **Step 4: 把发现的问题写回计划**

实机发现的任何一条，先在这份计划末尾补一节记下来，再动手改 —— 不要静默修掉。

---

## Self-Review 记录

- **Spec 覆盖**：触发时机 → Task 5；采集第一句 → Task 3；prompt 与语言 → Task 4；
  三级兜底 → Task 5（前两级）+ Task 6（第三级退回 profile）；显示四处 → Task 6；
  协议不升号 → Task 2；测试清单 → 分散在 Task 1/3/4/5/6，spec 里列的每一条都有对应断言；
  前置的焦点锚定 → Task 1。
- **占位符**：无 TBD。Task 6 Step 1 的第二个测试是唯一一处写成骨架描述的，
  已在紧跟的引用块里点明「必须写成真代码再跑」，并指名了照抄哪一个既有测试。
- **类型一致**：`session_label(&SessionInfo) -> &str` 在 Task 6 的四处调用签名一致；
  `clean_name(&str) -> String`、`name_prompt(&str, &str) -> Prompt`、
  `collect_first_input(&mut String, &mut bool, &str)` 在 Task 5 的调用与 Task 3/4 的定义一致；
  `NAME_MAX_CHARS` 在 Task 4 定义、Task 5 使用。
- **与 spec 的一处细化**：spec 写「按显示宽度截到 12 字」，计划把它拆成两半 ——
  守护进程按**字符数**封顶 24（`NAME_MAX_CHARS`，即 12 个汉字），界面各处按**显示列**
  裁到自己那一格的宽度。理由是显示宽度是界面的知识（`widgets::char_width`），
  把它塞进守护进程等于让 daemon 依赖 UI 层的布局常量。