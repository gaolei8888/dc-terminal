### Task 4: 底部状态栏 —— 当前项目、错误红字、按视图给提示、逆转键改 F2

**Files:**
- Modify: `src/ui.rs`

**Interfaces:**
- Consumes: `ui::short_path`（已有）
- Produces:
  - `ui::Msg { pub text: String, pub error: bool }`，带 `Msg::err(String) -> Msg`、`impl From<&str> for Msg`、`impl From<String> for Msg`
  - `draw()` 签名新增两个参数：`message: &Msg`（原为 `&str`）、`current: &str`

**说明：** 这个任务收三件互相纠缠的界面债，都落在同一段底部栏代码上，分开做会改两遍。

**（1）错误看不出是错误。** 现在所有提示——包括守护进程返回的错误——都用同一种灰字。
Task 5 的选择器要报「这不是一个目录」，必须一眼能看出是错误。顺带把 `Response::Error`
也标红，与已有的「断连时边框变红」是同一套语言。

**（2）会话视图显示的是看板的按键表。** 底部栏现在不分视图，进了会话仍然写着
`n 新建  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动  q 退出`——而这些键在会话视图里
**全部会被转发给 agent**。用户照着按 `n`，字母 n 落进 Claude Code 的输入框。这不是提示
缺失，是提示在骗人。改成提示跟着视图走。

**（3）逆转键与标题栏不一致，且偷了 Esc。** 实测当前行为：

| 位置 | 现状 |
|---|---|
| `src/ui.rs:232` | 会话视图截走 **`Esc`** 回看板 |
| `src/ui.rs:417,419` | 标题栏写「**Ctrl+B** 返回看板」——按了没反应 |
| `src/ui.rs:569` | 测试注释写「返回看板改用 Ctrl+B」，并断言 Esc 会转发给 agent |

`ff1e37d` 改了文案和测试注释，没改按键处理。结果是 Esc 被吞（Claude Code 里按 Esc
取消不掉任何东西），而标题栏宣传的键什么也不做。

**裁定：逆转键改成 `F2`。** `Esc` 和 `Ctrl+B` 一律还给 agent——Esc 是 agent 的取消键，
`Ctrl+B` 是 Claude Code 的「转后台」。F2 没有任何 CLI agent 在用，不需要双击透传这种
隐形状态，对非程序员也更直白。

这个任务做完，界面可见变化：底部多了「当前项目：…」、会话视图的提示换成 F2 那句、
标题栏改说 F2。`p` 键在 Task 5 才有。

- [ ] **Step 1: 写失败的测试**

在 `src/ui.rs` 的 `mod tests` 里追加：

```rust
    #[test]
    fn msg_from_str_is_not_an_error() {
        let m: Msg = "完成".into();
        assert!(!m.error);
        assert_eq!(m.text, "完成");
        assert!(Msg::err("炸了".into()).error);
    }

    #[test]
    fn bottom_bar_shows_current_project() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        term.draw(|f| {
            draw(
                f,
                &View::Board,
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/Users/lei/work/dc/dc-terminal",
            )
        })
        .unwrap();

        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            content.contains("dc-terminal"),
            "底部必须显示当前项目，实际（已去空白）: {content}"
        );
    }

    #[test]
    fn error_message_is_red() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        term.draw(|f| {
            draw(
                f,
                &View::Board,
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::err("不是一个目录".into()),
                true,
                "/tmp",
            )
        })
        .unwrap();

        let buf = term.backend().buffer();
        let area = buf.area;
        let red = (0..area.height).any(|y| {
            (0..area.width).any(|x| {
                buf.cell((x, y))
                    .map(|c| c.style().fg == Some(Color::Red) && c.symbol() != " ")
                    .unwrap_or(false)
            })
        });
        assert!(red, "错误提示必须用红字，否则跟成功提示长得一样");
    }

    #[test]
    fn f2_is_not_forwarded_but_esc_is() {
        // F2 是逆转键，dct 自己吃掉；Esc 必须还给 agent——
        // Claude Code 靠 Esc 取消/清空/关弹窗。
        assert_eq!(key_to_input(&key(KeyCode::F(2))), None);
        assert_eq!(key_to_input(&key(KeyCode::Esc)).as_deref(), Some("\u{1b}"));
        // Ctrl+B 是 Claude Code 的「转后台」，也必须透传
        let ctrl_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_eq!(key_to_input(&ctrl_b).as_deref(), Some("\u{2}"));
    }

    #[test]
    fn bottom_bar_help_follows_the_view() {
        use ratatui::backend::TestBackend;

        let sessions = vec![SessionInfo {
            id: 1,
            profile: "claude".into(),
            dir: "/tmp/a".into(),
            state: SessionState::Working,
            activity: String::new(),
        }];
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut st = ListState::default();

        let text_of = |term: &Terminal<TestBackend>| -> String {
            buffer_text(term.backend().buffer())
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        };

        // 会话视图：绝不能显示看板的按键表——那些键在这里全被转给 agent
        term.draw(|f| {
            draw(
                f,
                &View::Attached(1),
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp/a",
            )
        })
        .unwrap();
        let c = text_of(&term);
        assert!(c.contains("F2回看板"), "会话视图要给出逆转键提示：{c}");
        assert!(c.contains("新建会话"), "还要说清新建会话怎么走：{c}");
        assert!(!c.contains("u回滚"), "会话视图不能显示看板按键表：{c}");

        // 看板视图：仍然显示看板的按键表
        term.draw(|f| {
            draw(
                f,
                &View::Board,
                &sessions,
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp/a",
            )
        })
        .unwrap();
        let c = text_of(&term);
        assert!(c.contains("u回滚"), "看板要显示自己的按键表：{c}");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib ui -- --test-threads=1`
Expected: 编译失败，`Msg` 未定义、`draw` 参数个数不对；`bottom_bar_help_follows_the_view` 断言失败
（现在的底部栏不分视图，会话视图里照样显示看板按键表）；`f2_is_not_forwarded_but_esc_is` 通过
（`key_to_input` 本来就这么写的——真正的缺陷在 `View::Attached` 分支截了 `Esc`，见 Step 3g）。

- [ ] **Step 3: 实现**

**3a.** 在 `src/ui.rs` 的 `enum View` 定义**之前**加：

```rust
/// 底部状态栏要显示的一句话。`error` 决定它是灰字还是红字——
/// 出错和成功用同一种颜色，用户分不出刚才那步到底成没成。
pub struct Msg {
    pub text: String,
    pub error: bool,
}

impl Msg {
    pub fn err(text: String) -> Msg {
        Msg { text, error: true }
    }
}

impl From<&str> for Msg {
    fn from(s: &str) -> Msg {
        Msg {
            text: s.to_string(),
            error: false,
        }
    }
}

impl From<String> for Msg {
    fn from(text: String) -> Msg {
        Msg { text, error: false }
    }
}
```

**3c.** `message` 的全部赋值点共 8 处，逐处照下表改。行号是改动前的 `src/ui.rs`，
**从下往上改**，免得前面的编辑把后面的行号顶跑。

| 行 | 改前 | 改后 |
|---|---|---|
| 499 | `} else if message.is_empty() {` | 见 3e，整段替换 |
| 239 | `message = "守护进程连不上，刚才那次输入没发出去".into();` | `message = Msg::err("守护进程连不上，刚才那次输入没发出去".into());` |
| 215 | 见下方 A | |
| 195 | 见下方 B | |
| 184 / 189 | `message = act(...)` | 不改（`act` 的返回类型换了，赋值处不用动） |
| 154 | `message = "守护进程连不上，粘贴的内容没发出去".into();` | `message = Msg::err("守护进程连不上，粘贴的内容没发出去".into());` |
| 78 | `let mut message = String::new();` | `let mut message: Msg = "".into();` |

**A（215 起，`Request::Create` 的 match）：**

```rust
                        message = match client.call(Request::Create {
                            dir: current_dir.display().to_string(),
                            profile,
                        }) {
                            Ok(Response::Created { id }) => format!("已开会话 {id}").into(),
                            Ok(Response::Error(e)) => Msg::err(e),
                            _ => Msg::err("创建失败".into()),
                        };
```

**B（195 起，`Request::Diff` 的 match）：**

```rust
                        message = match client.call(Request::Diff { id: s.id }) {
                            Ok(Response::Diff(v)) if v.is_empty() => "没有改动".into(),
                            Ok(Response::Diff(v)) => v
                                .iter()
                                .map(|f| format!("{} +{} -{}", f.path, f.added, f.removed))
                                .collect::<Vec<_>>()
                                .join("  ")
                                .into(),
                            Ok(Response::Error(e)) => Msg::err(e),
                            _ => Msg::err("请求失败".into()),
                        };
```

**C（371 起，`act()`）：** 返回类型 `-> String` 改成 `-> Msg`，三个分支：

```rust
        Ok(Response::Ok) => "完成".into(),
        Ok(Response::Error(e)) => Msg::err(e),
        _ => Msg::err("请求失败".into()),
```

判断标准只有一条：**这句话是不是在报错**。是就 `Msg::err(...)`，不是就 `.into()`。

**3d.** `draw()` 签名末尾加一个参数，并把 `message` 的类型换掉：

```rust
fn draw(
    f: &mut Frame,
    view: &View,
    sessions: &[SessionInfo],
    st: &mut ListState,
    screen: &[Vec<ScreenSpan>],
    cursor: (u16, u16),
    message: &Msg,
    connected: bool,
    current: &str,
) {
```

**3e.** `draw()` 末尾那段底部栏整个替换成：

```rust
    // 提示必须跟着视图走。底部栏原来不分视图，进了会话仍写着看板的按键表，
    // 而那些键在会话视图里全部被转发给 agent——用户照着按 n，字母 n 会落进
    // Claude Code 的输入框。显示做不到的操作比不显示更糟。
    let idle_help = match view {
        View::Attached(_) => "F2 回看板（回看板后按 n 新建会话）　其余按键都发给 agent",
        View::PickProfile(_) => "按数字选 agent，Esc 取消",
        View::Board => "n 新建  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动  q 退出",
    };

    let (help, style) = if !connected {
        (
            "守护进程连不上，界面数据可能已过期".to_string(),
            Style::default().fg(Color::Red),
        )
    } else if message.text.is_empty() {
        (idle_help.to_string(), Style::default())
    } else if message.error {
        (message.text.clone(), Style::default().fg(Color::Red))
    } else {
        (message.text.clone(), Style::default())
    };
    // 当前项目放在边框标题里，框内只留一行字。中文是双宽字符，
    // 「当前项目：~/work/dc/dc-terminal」加上看板按键表在 80 列终端里放不下同一行，
    // 挤在一起会被 Paragraph 直接截断——标题行本来就空着，正好用它。
    f.render_widget(
        Paragraph::new(help).style(style).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("当前项目：{}", short_path(current))),
        ),
        chunks[1],
    );
```

底部栏框内仍是一行文字，`Layout::vertical` 的 `Constraint::Length(3)` 不用动。

**3f.** `run()` 里调用 `draw` 的地方补上新参数。`run()` 的 `default_dir` 现在既是当前项目、
又是相对路径的解析基准，先改名并留出可变性（Task 5 会真正改它）：

```rust
pub fn run(mut client: Client, default_dir: PathBuf) -> Result<()> {
```

函数体开头（`enable_raw_mode()?;` 之前）加：

```rust
    // start_dir 是 dct 启动时的目录，只用来解析用户敲进来的相对路径，永不改变。
    // current_dir 是「新会话开在哪」，Task 5 的选择器会改它。
    let start_dir = default_dir.clone();
    let mut current_dir = default_dir;
```

`term.draw(...)` 的闭包改成：

```rust
        term.draw(|f| {
            draw(
                f,
                &view,
                &sessions,
                &mut list_state,
                &screen,
                screen_cursor,
                &message,
                connected,
                &current_dir.display().to_string(),
            )
        })?;
```

`Request::Create` 那处的 `dir` 改用 `current_dir`：

```rust
                        message = match client.call(Request::Create {
                            dir: current_dir.display().to_string(),
                            profile,
                        }) {
```

`start_dir` 此时还没有调用点，会有 `unused_variable` 警告——Task 5 接线后消失。

**3g. 逆转键改成 F2，`Esc` 还给 agent。** 找到 `View::Attached(id)` 那个分支
（`src/ui.rs:228-242`），把开头的注释与条件整个换掉：

```rust
            View::Attached(id) => {
                // F2 是唯一被 dct 吃掉的键，其余一律 key_to_input 翻译成终端字节
                // 送进去。Esc 必须还给 agent——Claude Code 靠它取消/清空/关弹窗；
                // Ctrl+B 也必须还回去，那是 Claude Code 的「转后台」。
                // 逆转键挑 F2 是因为没有 CLI agent 在用它，不必搞双击透传。
                if key.code == KeyCode::F(2) {
                    view = View::Board;
                    need_sessions = true;
                } else if let Some(text) = key_to_input(&key) {
```

后面的函数体（发送失败的错误提示那几行）保持原样，只是那句赋值按 3c 的规则改成
`Msg::err(...)`。

**3h. 标题栏改说 F2。** `draw()` 的 `View::Attached` 分支里（`src/ui.rs:417,419`）
两处字面量把 `Ctrl+B` 换成 `F2`：

```rust
            let title = if connected {
                format!("会话 {id} · {project} —— F2 返回看板")
            } else {
                format!("会话 {id} · {project}（连接已断开，画面可能过期）—— F2 返回看板")
            };
```

**3i. 修掉那条会误导人的测试注释。** `mod tests` 里 `esc_is_forwarded_to_the_agent`
的注释写着「返回看板改用 Ctrl+B」，是错的（`ff1e37d` 只改了文案没改代码）。改成：

```rust
    #[test]
    fn esc_is_forwarded_to_the_agent() {
        // agent 靠 Esc 做取消/清空/关弹窗，抢走它会让 agent 的交互失灵。
        // 返回看板用 F2。
        assert_eq!(key_to_input(&key(KeyCode::Esc)).as_deref(), Some("\u{1b}"));
    }
```

**3g.** `mod tests` 里已有的 `draw_does_not_panic_for_all_views` 和
`disconnected_state_shows_warning_in_bottom_bar` 每个 `draw(...)` 调用都要补参数：
`""` / `"完成"` 这类实参改成 `&Msg::from("")` / `&Msg::from("完成")`，末尾加 `"/tmp/proj"`。

- [ ] **Step 4: 跑测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -- --test-threads=1`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt && git add -A
git commit -m "feat: 底部显示当前项目，错误提示改红字"
```

---

