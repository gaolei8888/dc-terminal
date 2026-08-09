# dct 会话里能复制文字 —— 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让用户在会话里能用终端自己的拖选复制——只在 agent 真的订阅了鼠标时才抓鼠标，并给一个 `F4` 复制模式作为兜底。

**Architecture:** 「要不要抓鼠标」从「在不在会话里」这一个判据，换成三个条件相与的派生布尔值：`贴在会话里 && agent 订阅了鼠标 && 不在复制模式`。第二项走既有的 `App.scroll.agent_owns`（守护进程早就在传），第三项是新增的 `App.copy_mode`。**协议不变，守护进程一个文件都不改。**

**Tech Stack:** Rust 2021、ratatui、crossterm。

**Spec:** `docs/superpowers/specs/2026-08-08-dct-copy-from-a-session-design.md`

## Global Constraints

- 每个 Task 结束前必须跑：`cargo fmt`、`cargo clippy --all-targets -- -D warnings`、`cargo test`。三条全绿才提交。
- **提交信息用英文，不要 AI 署名行**（不加 `Co-Authored-By`）。
- 所有用户可见文案进 `src/i18n.rs` 的 `Key` 词条表，`en:` / `zh:` 都要给，并加进 `src/i18n.rs` 末尾那份穷举列表——漏一处编译不过。
- **键名那一列一律 Latin 且中英一致**（`Tab` / `Enter` / `F3` / `Space`）。有守卫盯着（`ui::keys::tests` 与 `ui::view::tests` 里的 `no_key_column_is_ever_written_in_chinese`）。
- **不用 emoji 当图标。**
- `src/proto.rs` / `src/pty.rs` / `src/session.rs` / `src/daemon.rs` **一行都不该改**。改到了就说明走偏了，停下来说明理由。
- 底栏优先级不变：错误消息 > 复制模式提示 > 滚动提示 > 按键表。
- 每个 Task 结束时工作树必须能编译、测试全绿。

---

### Task 1: 只在 agent 真要鼠标时才抓

**Files:**
- Modify: `src/ui/app.rs`（加 `copy_mode` 字段）
- Modify: `src/ui/mod.rs`（新增 `wants_mouse_capture`，换掉捕获判据）

**Interfaces:**
- Consumes: `App.scroll: ScrollState`，其 `agent_owns: bool` 由守护进程每帧带回（`src/pty.rs::view_of` → `session.rs::state_of` → `Response::Screen.scroll`）
- Produces: `fn wants_mouse_capture(attached: bool, agent_subscribed: bool, copy_mode: bool) -> bool`；`App.copy_mode: bool`

- [ ] **Step 1: 写失败的测试**

追加到 `src/ui/mod.rs` 的 `mod tests`：

```rust
/// 三个条件的真值表，八种组合全列。
///
/// 穷举而不是挑几个代表：这个函数错一格的后果是「用户在会话里复制不了」
/// 或者「agent 收不到它订阅的鼠标」，两种都不会 panic、不会报错，只会让人
/// 觉得工具坏了却说不清哪儿坏。八行断言比一句「应该没问题」便宜得多。
#[test]
fn mouse_is_captured_only_when_all_three_conditions_hold() {
    // attached, agent_subscribed, copy_mode -> want
    assert!(wants_mouse_capture(true, true, false));
    assert!(!wants_mouse_capture(true, true, true), "复制模式一票否决");
    assert!(
        !wants_mouse_capture(true, false, false),
        "agent 不要鼠标就别抓——抓了用户就白白丢了拖选复制"
    );
    assert!(!wants_mouse_capture(true, false, true));
    assert!(!wants_mouse_capture(false, true, false), "看板上永远不抓");
    assert!(!wants_mouse_capture(false, true, true));
    assert!(!wants_mouse_capture(false, false, false));
    assert!(!wants_mouse_capture(false, false, true));
}

/// 断连时 `Screen` 拉不到，`app.scroll` 保持上一帧的值，所以捕获状态不该翻转。
/// 翻转要往 stdout 写转义序列，断连时反复翻转是最吵的一种失败。
#[test]
fn a_failed_screen_call_does_not_flip_the_capture_state() {
    let (mut app, _d) = App::test_app();
    app.view = View::Attached(1);
    app.scroll = crate::session::ScrollState {
        agent_owns: true,
        ..Default::default()
    };
    let before = wants_mouse_capture(true, app.scroll.agent_owns, app.copy_mode);

    // 一次失败的 Screen 调用不会碰 app.scroll
    app.connected = false;

    assert_eq!(
        wants_mouse_capture(true, app.scroll.agent_owns, app.copy_mode),
        before
    );
}

#[test]
fn a_fresh_app_is_not_in_copy_mode() {
    let (app, _d) = App::test_app();
    assert!(!app.copy_mode);
}
```

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib ui::tests`
Expected: FAIL，`cannot find function 'wants_mouse_capture'`。

- [ ] **Step 3: 加 `App.copy_mode`**

`src/ui/app.rs`，在 `scroll` 字段附近加：

```rust
    /// 用户按 `F4` 打开的复制模式：**临时**把鼠标交还给终端，好让人用
    /// 终端自己的拖选去复制。
    ///
    /// 它是「此刻正在复制」的临时状态，不是配置——离开会话一律复位
    /// （见 `attach::handle_key`）。跨会话粘着的话，用户会在另一个会话里
    /// 发现鼠标莫名其妙不归 agent 管，而屏幕上没有任何东西解释为什么。
    pub copy_mode: bool,
```

`new_inner` 的字段初值里加 `copy_mode: false,`。

- [ ] **Step 4: 写 `wants_mouse_capture`**

`src/ui/mod.rs`，紧挨着 `mouse_capture_transition`：

```rust
/// 这一帧该不该抓鼠标。三个条件全真才抓。
///
/// 抽成纯函数的理由同 `mouse_capture_transition`：副作用（往 stdout 写转义
/// 序列）没法单测，判断能测——而且判断错了两个方向都难受：漏关，用户在会话里
/// 连拖选复制都做不了；漏开，agent 收不到它明明订阅了的鼠标事件。
///
/// `agent_subscribed` 来自 `App.scroll.agent_owns`，**不新开一条判据**。
/// 那个字段的语义就是「agent 自己攥着鼠标」，跟这里问的是同一个事实；
/// 各读各的，迟早会分叉成「dct 抓着鼠标却不肯滚」这种自相矛盾的状态。
fn wants_mouse_capture(attached: bool, agent_subscribed: bool, copy_mode: bool) -> bool {
    attached && agent_subscribed && !copy_mode
}
```

- [ ] **Step 5: 换掉主循环里的判据**

`src/ui/mod.rs` 的 `run()`，把原来那段（`let is_attached = matches!(app.view, View::Attached(_));` 起）改成：

```rust
        // 抓不抓鼠标不再只看「在不在会话里」：agent 没订阅鼠标的会话
        // （codex、shell）里抓着它，唯一的效果是把终端的拖选复制废掉，
        // 换来一个 PageUp/PageDown/End 已经能做的滚轮。
        //
        // 放在 `term.draw` 之前的理由不变（见下一段注释）；而且这一轮的
        // `Screen` 响应已经落进 `app.scroll` 了，`agent_owns` 就是这一帧的事实。
        let is_attached = matches!(app.view, View::Attached(_));
        let want = wants_mouse_capture(is_attached, app.scroll.agent_owns, app.copy_mode);
        if let Some(enable) = mouse_capture_transition(mouse_captured, want) {
            let _ = if enable {
                execute!(std::io::stdout(), EnableMouseCapture)
            } else {
                execute!(std::io::stdout(), DisableMouseCapture)
            };
            mouse_captured = enable;
        }
```

原来那段「检查一次『在不在会话里』有没有变」的注释要跟着改——它现在检查的是三个条件的**合取**，不是「在不在会话里」。保留它关于「为什么放在 draw 之前」和「为什么不在每个分支各开关一次」的两条理由，那两条依然成立。

- [ ] **Step 6: 跑测试，确认通过**

Run: `cargo test`
Expected: PASS。既有的 `mouse_capture_toggles_only_on_a_real_transition` 不受影响——`mouse_capture_transition` 的签名没变。

- [ ] **Step 7: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui/app.rs src/ui/mod.rs
git commit -m "feat: only grab the mouse when the agent actually asked for it"
```

---

### Task 2: `F4` 复制模式

**Files:**
- Modify: `src/ui/attach.rs`（`F4` 分支；离开会话时复位）

**Interfaces:**
- Consumes: Task 1 的 `App.copy_mode`
- Produces: 无新公开接口

- [ ] **Step 1: 写失败的测试**

追加到 `src/ui/attach.rs` 的 `mod tests`（若该文件的测试模块里还没有构造 `App` 的助手，用 `App::test_app()`）：

```rust
fn attached_app() -> (App, tempfile::TempDir) {
    let (mut app, d) = App::test_app();
    app.view = View::Attached(1);
    (app, d)
}

#[test]
fn f4_toggles_copy_mode_and_is_never_forwarded_to_the_agent() {
    let (mut app, _d) = attached_app();
    assert!(!app.copy_mode);

    handle_key(&mut app, key(KeyCode::F(4))).unwrap();
    assert!(app.copy_mode, "第一下打开");

    handle_key(&mut app, key(KeyCode::F(4))).unwrap();
    assert!(!app.copy_mode, "第二下关掉");

    // F4 是 dct 自己吃掉的键，一个字节都不能落进 agent 的输入
    assert_eq!(super::super::key_to_input(&key(KeyCode::F(4))), None);
}

/// 复制模式是「此刻正在复制」的临时状态，不是配置。**进会话时复位**——
/// 不管上一个会话是怎么离开的（F2、Ctrl+Q、agent 自己退出），下一个会话
/// 一定从「鼠标归 agent」开始。
///
/// 在**进入**这一侧复位，而不是在三条离开的路上各写一次：`enter_session`
/// 是所有进会话路径的唯一漏斗（看板 Enter、九宫格 Enter、F3 都走它），
/// 而离开有三条路，其中 Ctrl+Q 那条走的是 `back_one_level`——一个所有视图
/// 共用的纯函数，为这一个字段改它的签名不值。漏斗上写一次，结构上就漏不掉。
#[test]
fn entering_a_session_always_starts_outside_copy_mode() {
    let (mut app, _d) = App::test_app();
    app.copy_mode = true;

    super::super::enter_session(&mut app, 1);

    assert!(!app.copy_mode, "上一个会话的复制模式不能粘到下一个会话");
}
```

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib ui::attach::tests`
Expected: FAIL，`no field 'copy_mode'` 已经在 Task 1 解决，这里会是 `F4` 没有被处理（`copy_mode` 仍为 `false`）以及 `leave_session` 不存在。

- [ ] **Step 3: 在进会话的漏斗上复位**

`src/ui/mod.rs::enter_session`。一行，加在已有的 `app.explained_failure = None;` 旁边：

```rust
    // 上一个会话的复制模式不能粘到这一个来。**在「进入」这一侧复位**，
    // 不在三条离开的路上各写一次：`enter_session` 是所有进会话路径的唯一
    // 漏斗（看板 Enter、九宫格 Enter、F3 都走它），而离开有三条路，其中
    // Ctrl+Q 那条走的是 `back_one_level`——一个所有视图共用的纯函数，
    // 为这一个字段改它的签名不值。漏斗上写一次，结构上就漏不掉。
    //
    // 留在看板上的那个 `copy_mode` 是无害的：`wants_mouse_capture` 的第一个
    // 条件就是「贴在会话里」，不在会话里时它压根不参与判断。
    app.copy_mode = false;
```

**不要**去重构 `attach::handle_key` 的 `F2` 分支、主循环里 `session_ended_notice` 之后那一段、或者 `back_one_level` 的落点。那三处各自还做着别的事（设消息、清 `explained_failure`、`sent_size = None`），为这一个 `bool` 把它们收成一个函数，风险远大于收益。

- [ ] **Step 4: 加 `F4` 分支**

`src/ui/attach.rs::handle_key`，插在 `F3` 分支之后、`key_scroll` 之前：

```rust
    } else if key.code == KeyCode::F(4) {
        // F4 = 复制模式：临时把鼠标交还给终端，用终端自己的拖选去复制。
        // 挑 F4 沿用 F2/F3 的理由：没有 CLI agent 在用 F 功能键，偷它不踩
        // 任何人，也不用搞双击透传那种隐形状态。
        //
        // 这里只翻转状态，真正开关鼠标在主循环里统一做（见
        // `mod.rs::wants_mouse_capture`）——在这儿直接 execute! 的话，
        // 就有两处在写同一个终端状态，而它们对「现在开着没有」的记忆会分叉。
        app.copy_mode = !app.copy_mode;
    } else if let Some(action) = key_scroll(
```

`key_to_input` 不用改：它的通配臂对所有 `KeyCode::F(_)` 返回 `None`，F4 天然不会被转发。上面那条断言把这件事钉住，免得以后有人给 F 键加编码时不小心让它开始转发。

- [ ] **Step 5: 跑测试，确认通过**

Run: `cargo test`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui/attach.rs src/ui/mod.rs
git commit -m "feat: F4 hands the mouse back to the terminal so you can select and copy"
```

---

### Task 3: 底栏写清楚现在是什么状态

**Files:**
- Modify: `src/ui/mod.rs`（`draw` 里底栏内容的选择）
- Modify: `src/i18n.rs`（新词条）

**Interfaces:**
- Consumes: Task 1 的 `App.copy_mode`
- Produces: `Key::CopyMode`

- [ ] **Step 1: 写失败的测试**

追加到 `src/ui/mod.rs` 的 `mod tests`：

```rust
/// 模式看不见就是下一个隐形状态，而这个仓库刚花一整轮改造消灭掉那种东西。
#[test]
fn copy_mode_says_so_in_the_bar() {
    for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
        let (mut app, _d) = app_with_one_agent_session(View::Attached(1));
        app.lang = lang;
        app.copy_mode = true;
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();

        let bar = bar_text(&term);
        let hint = crate::i18n::text(crate::i18n::Key::CopyMode, lang);
        assert!(bar.contains(&hint.replace(' ', "")), "{lang:?} 下底栏要写着复制模式：{bar}");
    }
}

/// 优先级：错误消息 > 复制模式 > 滚动提示。
///
/// 复制模式压过滚动提示，是因为在复制模式下滚轮根本不归 dct 管，
/// 那条提示这时候是错的；而错误消息压过复制模式，是因为出错是一次性的、
/// 不说就再也没机会说，复制模式则是个持续状态，下一帧还会写。
#[test]
fn an_error_beats_copy_mode_which_beats_the_scroll_hint() {
    let (mut app, _d) = app_with_one_agent_session(View::Attached(1));
    app.copy_mode = true;
    app.scroll = crate::session::ScrollState {
        offset: 5,
        max: 100,
        ..Default::default()
    };
    let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();

    term.draw(|f| draw(f, &mut app)).unwrap();
    let hint = crate::i18n::text(crate::i18n::Key::CopyMode, app.lang);
    assert!(
        bar_text(&term).contains(&hint.replace(' ', "")),
        "复制模式压过滚动提示"
    );

    app.message = Msg::err("出事了".into());
    term.draw(|f| draw(f, &mut app)).unwrap();
    assert!(bar_text(&term).contains("出事了"), "错误消息压过复制模式");
}
```

- [ ] **Step 2: 跑测试，确认它失败**

Run: `cargo test --lib ui::tests`
Expected: FAIL，`no variant named 'CopyMode'`。

- [ ] **Step 3: 加词条**

`src/i18n.rs` 的 `Key` 枚举：

```rust
    /// 复制模式下顶掉整条底栏右段的提示
    CopyMode,
```

译文表：

```rust
        CopyMode => t!(
            lang,
            en: "Copy mode · the mouse is the terminal's · F4 to exit",
            zh: "复制模式 · 鼠标已交还终端 · F4 退出"
        ),
```

并加进文件末尾那份穷举列表（漏了编译不过）。

- [ ] **Step 4: 插进底栏**

`src/ui/mod.rs::draw`，把算 `scroll_hint` 的那一段改成：

```rust
        // 复制模式压过滚动提示：这时候滚轮根本不归 dct 管，那条提示是错的。
        // 压不过错误消息——外层 if/else 链已经保证了这一点。
        let hint = match &app.view {
            View::Attached(_) if app.copy_mode => {
                Some(crate::i18n::text(crate::i18n::Key::CopyMode, app.lang).to_string())
            }
            View::Attached(_) => attach::scroll_hint(&app.scroll, app.lang),
            _ => None,
        };
        match hint {
            Some(h) => (BarContent::Text(h), Style::default()),
            None => (
                BarContent::Keys(idle_help(&app.view, app.lang, help_ctx(app))),
                Style::default(),
            ),
        }
```

- [ ] **Step 5: 跑测试，确认通过**

Run: `cargo test`
Expected: PASS。既有的 `a_scroll_hint_takes_over_the_bottom_bar_when_there_is_history` 和 `a_message_beats_the_scroll_hint` 都不该受影响——它们的 `copy_mode` 是 `false`。

- [ ] **Step 6: 提交**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui/mod.rs src/i18n.rs
git commit -m "feat: the bar says plainly when the mouse belongs to the terminal"
```

---

### Task 4: 两份 README 说实话

**Files:**
- Modify: `README.zh-CN.md`
- Modify: `README.md`

**Interfaces:**
- Consumes: 前三个 Task 的行为
- Produces: 无

⚠️ **工作树里有用户自己未提交的 README 改动**（`scripts/install.sh` 那一段）。**不要**把它们卷进你的提交——必要时 `git stash push -- README.md README.zh-CN.md`，改完再 pop，或者只 `git add -p` 你自己那几段。

- [ ] **Step 1: 重写中文那段**

`README.zh-CN.md` 现在有这么一段（在滚屏说明附近）：

> 往回滚屏能用了，但它是有代价的：进了会话之后 `dct` 会接管鼠标，终端自己那套拖动选中文字就在会话里失灵了。iTerm2 里按住 Option 能拿回来，别的终端一般也有对应的修饰键。`dct` 自己还没有复制功能。退回看板，鼠标就还给你了。

换成：

```markdown
`dct` 只在 **agent 自己要鼠标的时候**才接管它。Claude Code 会要（它自己用鼠标滚
它那一屏），codex 和普通命令行不要——那些会话里鼠标一直归终端，拖动选中文字、
复制，跟平时完全一样。代价是那些会话里滚轮不再翻 `dct` 的历史，用
`PageUp`/`PageDown`/`End`。

在 agent 要鼠标的会话里想复制，按 `F4` 进复制模式：鼠标临时还给终端，底栏会写着
现在是这个状态，复制完再按一次 `F4` 回去。也可以用终端自己的修饰键（iTerm2 是
按住 Option），不用退出会话。

`dct` 自己没有复制功能——复制用的是你终端本来那一套。
```

- [ ] **Step 2: 同步英文**

`README.md` 对应段落做等价改动。**两份是同一个文档的两种语言，不能漂移**——逐条对照，claim 对 claim。

- [ ] **Step 3: 核对文档里的每个键**

`F4` 对着 `src/ui/attach.rs::handle_key` 核一遍，`PageUp`/`PageDown`/`End` 对着 `key_scroll` 核一遍。**文档里写一个不存在的键，比漏写一个更糟。**

- [ ] **Step 4: 提交**

```bash
git add README.md README.zh-CN.md   # 只 add 你自己改的那几段，见上面的警告
git commit -m "docs: the mouse stays yours unless the agent asked for it"
```

---

## 自查

**Spec 覆盖：**

| Spec 小节 | Task |
|---|---|
| 一、一条规则（三条件相与） | 1 |
| 二、agent 订没订阅（复用 `agent_owns`，不改协议） | 1 |
| 三、`F4` 复制模式 | 2（状态）、3（底栏） |
| 四、要动的文件 | 1–4，且守护进程侧零改动 |
| 错误处理：`Screen` 拉不到不翻转 | 1 |
| 错误处理：复制模式下会话结束要复位 | 2 |
| 测试清单 | 1（真值表、断连）、2（`F4`、复位三路）、3（底栏优先级） |
| 破坏性变更：无 | —— |

**排期：** Task 1 → 2 → 3 是一条依赖链（字段 → 键 → 文案），Task 4 依赖前三个的最终行为。不能并行。

**留给执行者的两个坑：**

1. **`copy_mode` 在「进入」这一侧复位，不在「离开」那一侧。** 初稿写的是把三处「回看板」收成一个 `leave_session`，核代码之后否掉了：Ctrl+Q 那条走 `back_one_level`，是所有视图共用的纯函数，为一个 `bool` 改它的签名不值；另外两处各自还做着别的事。`enter_session` 是所有进会话路径的唯一漏斗，在那儿写一行就够，而且结构上漏不掉。
2. **别碰守护进程。** 如果发现自己想改 `proto.rs` / `pty.rs` / `session.rs` / `daemon.rs`，停下来——`agent_owns` 已经在传了，需要的事实全都在 `App.scroll` 里。
