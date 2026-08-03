### Task 5: `p` 键与项目选择器

**Files:**
- Modify: `src/ui.rs`

**Interfaces:**
- Consumes: `expand_path`、`filter_projects`、`move_sel_n`（Task 3）；`Msg`（Task 4）；
  `proto::Request::Projects`、`proto::Response::Projects`（Task 2）
- Produces: 无（这是最后一个任务）

**说明：** 交互规则见 spec 的「界面」段。四条容易做错的：

1. `p` **只在看板视图生效**。会话视图里所有按键都转发给 agent，抢走 `p` 会让 agent 里打不出这个字母
2. 末行「手输路径…」**不参与过滤**，永远在。否则打了没匹配的字，连兜底入口都消失
3. 手输状态下**可见字符全进输入框**，不再当过滤用
4. 只校验 `is_dir()`。**是不是 git 仓库不在这里判**——那条规则留在 `SessionManager::create()`，两处各判一次迟早漂移

- [ ] **Step 1: 写失败的测试**

在 `src/ui.rs` 的 `mod tests` 里追加：

```rust
    #[test]
    fn draw_does_not_panic_for_project_picker() {
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut st = ListState::default();
        st.select(Some(0));
        let all = vec![
            "/Users/lei/work/dc/dc-terminal".to_string(),
            "/Users/lei/work/dc/dc_workbench".to_string(),
        ];

        // 列表态
        term.draw(|f| {
            draw(
                f,
                &View::PickProject {
                    all: all.clone(),
                    filter: String::new(),
                    state: st.clone(),
                    typing_path: None,
                },
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp",
            )
        })
        .unwrap();

        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(content.contains("dc-terminal"), "列表要显示项目：{content}");
        assert!(content.contains("手输路径"), "末行兜底入口必须在：{content}");

        // 过滤到无匹配：只剩兜底那一行，不能 panic
        term.draw(|f| {
            draw(
                f,
                &View::PickProject {
                    all: all.clone(),
                    filter: "没有这个".to_string(),
                    state: st.clone(),
                    typing_path: None,
                },
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp",
            )
        })
        .unwrap();
        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(content.contains("手输路径"), "无匹配时兜底入口仍要在：{content}");

        // 手输态
        term.draw(|f| {
            draw(
                f,
                &View::PickProject {
                    all: all.clone(),
                    filter: String::new(),
                    state: st.clone(),
                    typing_path: Some("~/work/x".to_string()),
                },
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp",
            )
        })
        .unwrap();
        let content: String = buffer_text(term.backend().buffer())
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(content.contains("~/work/x"), "手输态要回显已输入的路径：{content}");

        // 空列表（全新守护进程）也不能 panic
        term.draw(|f| {
            draw(
                f,
                &View::PickProject {
                    all: Vec::new(),
                    filter: String::new(),
                    state: ListState::default(),
                    typing_path: None,
                },
                &[],
                &mut st,
                &[],
                (0, 0),
                &Msg::from(""),
                true,
                "/tmp",
            )
        })
        .unwrap();
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib ui -- --test-threads=1`
Expected: 编译失败，`View::PickProject` 不存在。

- [ ] **Step 3: 实现**

**3a.** `enum View` 加一个变体：

```rust
#[derive(Clone)]
enum View {
    Board,
    Attached(u32),
    PickProfile(Vec<String>),
    PickProject {
        /// 守护进程返回的完整列表，过滤不改动它
        all: Vec<String>,
        /// 用户打的字
        filter: String,
        state: ListState,
        /// Some 表示正处在「手输路径」的输入态
        typing_path: Option<String>,
    },
}
```

**3b.** `View::Board` 的按键 match 里，在 `KeyCode::Char('n')` 分支后面加：

```rust
                KeyCode::Char('p') => {
                    // 拿不到列表就不进选择器：进去看见一片空白，用户会以为
                    // 自己从来没开过项目。
                    match client.call(Request::Projects) {
                        Ok(Response::Projects(mut all)) => {
                            // 全新守护进程列表是空的，补上启动目录，
                            // 保证第一次用也不会看到空列表。
                            let start = start_dir.display().to_string();
                            if !all.contains(&start) {
                                all.push(start);
                            }
                            let mut state = ListState::default();
                            state.select(Some(0));
                            view = View::PickProject {
                                all,
                                filter: String::new(),
                                state,
                                typing_path: None,
                            };
                        }
                        Ok(Response::Error(e)) => message = Msg::err(e),
                        _ => message = Msg::err("拿不到项目列表".into()),
                    }
                }
```

**3c.** 在 `View::PickProfile(profiles) => match key.code { ... }` 分支**之后**加整个新分支：

```rust
            View::PickProject {
                all,
                mut filter,
                mut state,
                typing_path,
            } => match typing_path {
                // ——手输路径态：可见字符全进输入框，不再当过滤用——
                Some(mut buf) => match key.code {
                    KeyCode::Esc => {
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: None,
                        }
                    }
                    KeyCode::Enter => {
                        let p = expand_path(&buf, &start_dir);
                        if p.is_dir() {
                            // 「当前项目」已经在底部边框标题里，这里说的是刚发生的动作
                            message =
                                format!("已切到 {}", short_path(&p.display().to_string())).into();
                            current_dir = p;
                            view = View::Board;
                        } else {
                            // 不是 git 仓库这件事不在这里判——留给 create()
                            message = Msg::err(format!("{} 不是一个目录", p.display()));
                            view = View::PickProject {
                                all,
                                filter,
                                state,
                                typing_path: Some(buf),
                            };
                        }
                    }
                    KeyCode::Backspace => {
                        buf.pop();
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: Some(buf),
                        };
                    }
                    KeyCode::Char(c) => {
                        buf.push(c);
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: Some(buf),
                        };
                    }
                    _ => {
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: Some(buf),
                        }
                    }
                },
                // ——列表态——
                None => match key.code {
                    KeyCode::Esc => view = View::Board,
                    KeyCode::Down | KeyCode::Up => {
                        let delta = if key.code == KeyCode::Down { 1 } else { -1 };
                        // +1 是末行那个「手输路径…」，它不参与过滤，永远在
                        let n = filter_projects(&all, &filter).len() + 1;
                        move_sel_n(&mut state, n, delta);
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: None,
                        };
                    }
                    KeyCode::Enter => {
                        let shown = filter_projects(&all, &filter);
                        let i = state.selected().unwrap_or(0);
                        if i >= shown.len() {
                            // 选中的是末行「手输路径…」
                            view = View::PickProject {
                                all,
                                filter,
                                state,
                                typing_path: Some(String::new()),
                            };
                        } else {
                            let p = PathBuf::from(&shown[i]);
                            if p.is_dir() {
                                message = format!("已切到 {}", short_path(&shown[i])).into();
                                current_dir = p;
                                view = View::Board;
                            } else {
                                // 列表里那条不删——可能只是外置盘没挂
                                message =
                                    Msg::err(format!("{} 现在找不到了", short_path(&shown[i])));
                                view = View::PickProject {
                                    all,
                                    filter,
                                    state,
                                    typing_path: None,
                                };
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        filter.pop();
                        state.select(Some(0));
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: None,
                        };
                    }
                    KeyCode::Char(c) => {
                        filter.push(c);
                        // 过滤变了就回到第一项，否则光标可能停在已被过滤掉的行号上
                        state.select(Some(0));
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: None,
                        };
                    }
                    _ => {
                        view = View::PickProject {
                            all,
                            filter,
                            state,
                            typing_path: None,
                        }
                    }
                },
            },
```

**3d.** `draw()` 的 `match view` 里，在 `View::PickProfile(...)` 分支之后加渲染：

```rust
        View::PickProject {
            all,
            filter,
            state,
            typing_path,
        } => {
            if let Some(buf) = typing_path {
                f.render_widget(
                    Paragraph::new(format!("{buf}▌")).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(border_style)
                            .title("输入项目路径（Enter 确认，Esc 返回列表）"),
                    ),
                    chunks[0],
                );
            } else {
                let shown = filter_projects(all, filter);
                let mut items: Vec<ListItem> = shown
                    .iter()
                    .map(|p| {
                        let short = short_path(p);
                        let name = std::path::Path::new(p)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| short.clone());
                        ListItem::new(Line::from(vec![
                            Span::raw(format!("{:<20}", truncate(&name, 20))),
                            Span::styled(
                                truncate(&short, 50),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ]))
                    })
                    .collect();
                // 兜底入口不参与过滤，永远在最后一行
                items.push(ListItem::new(Line::from(Span::styled(
                    "手输路径…",
                    Style::default().fg(Color::Cyan),
                ))));

                let title = if filter.is_empty() {
                    "选项目（↑↓ 选，Enter 确认，直接打字过滤，Esc 取消）".to_string()
                } else {
                    format!("选项目（过滤：{filter}）")
                };
                // state 是 View 里那份的副本，draw 只读不写，所以这里克隆一份给
                // render_stateful_widget 用，不去动 `st`（那是看板的光标）。
                let mut s = state.clone();
                f.render_stateful_widget(
                    List::new(items)
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(border_style)
                                .title(title),
                        )
                        .highlight_symbol("▶ "),
                    chunks[0],
                    &mut s,
                );
            }
        }
```

**3e.** 底部提示：Task 4 建的 `idle_help` match 要补一个 `PickProject` 分支，
`Board` 那句加上 `p 换项目`：

```rust
    let idle_help = match view {
        View::Attached(_) => "F2 回看板（回看板后按 n 新建会话）　其余按键都发给 agent",
        View::PickProfile(_) => "按数字选 agent，Esc 取消",
        View::PickProject { typing_path: Some(_), .. } => "输入路径后 Enter 确认，Esc 返回列表",
        View::PickProject { .. } => "↑↓ 选  Enter 确认  直接打字过滤  Esc 取消",
        View::Board => "n 新建  p 换项目  ↑↓ 选择  Enter 进入  u 回滚  s 停止  d 改动  q 退出",
    };
```

注意分支顺序：`typing_path: Some(_)` 必须排在通配的 `PickProject { .. }` 之前，
否则永远命中不到。

**3f.** 让粘贴在手输路径态里可用。现在主循环顶部的 `Event::Paste` 分支**只认会话视图**
（`src/ui.rs:151-158`），在选择器里粘贴会被整段吞掉——而「能粘贴路径」正是不做目录浏览器
的理由，必须补上。把那段整个替换成：

```rust
        if let Event::Paste(text) = ev {
            match &mut view {
                View::Attached(id) => {
                    if !text.is_empty() && client.call(Request::Input { id: *id, text }).is_err() {
                        message = Msg::err("守护进程连不上，粘贴的内容没发出去".into());
                    }
                }
                // 手输路径态：粘贴直接进输入框。从别处拷一条路径粘进来一步到位，
                // 这是不做目录浏览器的底气。trim 掉换行——从终端或文件管理器
                // 拷路径经常带一个尾随换行，不去掉会拼出一个不存在的目录。
                View::PickProject {
                    typing_path: Some(buf),
                    ..
                } => buf.push_str(text.trim()),
                _ => {}
            }
            continue;
        }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -- --test-threads=1 && cargo clippy -- -D warnings && cargo fmt --check`
Expected: 全绿，且 Task 3/4 留下的 `dead_code` / `unused_variable` 警告此时应当全部消失。

- [ ] **Step 5: 手动端到端验证（需要真人，在真终端里跑）**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release
cd ~/work/dc/dc-terminal && ./target/release/dct
```

逐条确认：

1. 底部显示 `当前项目：~/work/dc/dc-terminal`
2. 按 `n` 开一个 shell 会话 → 成功。会话里底部提示应当是 `F2 回看板…` 而**不是**看板按键表；
   按 `Esc` 和 `Ctrl+B` 都应当落进 agent（在 claude 会话里最容易验：Esc 能取消、Ctrl+B 能转后台）；
   按 `F2` 回看板
3. 按 `p` → 弹出列表，至少有 `dc-terminal` 一条，末行是「手输路径…」
4. 打 `work` → 列表被过滤；`Backspace` 删掉 → 恢复
5. 打 `没有这个` → 列表只剩「手输路径…」一行，**兜底入口没消失**
6. 选中「手输路径…」按 `Enter` → 变成输入框；打 `~/work/dc/dc_workbench` → `Enter`
7. 底部变成 `当前项目：~/work/dc/dc_workbench`
8. 按 `n` 新建 → 新会话的项目列显示 `dc_workbench`，**旧会话仍在看板上**（不过滤）
9. 再按 `p` → `dc_workbench` 现在排在 `dc-terminal 前面`
10. 按 `p`，选「手输路径…」，输入 `/tmp/根本不存在` → **红字**提示「不是一个目录」，且**不切换**
11. 还在手输态，用系统剪贴板拷一条真实路径，`Cmd+V` 粘贴 → 整条路径一次性进输入框（不是一个个字符），`Enter` 能切过去
12. `Enter` 进一个会话，在里面打 `p` → **字母 p 落进 agent**，没有弹出选择器
13. `q` 退出 → 终端状态正常（有回显、有换行）
14. 重开 `dct` → 底部当前项目回到启动目录（`current_dir` 不持久化），但按 `p` 列表里两个项目都还在

第 12 条最容易做错，务必单独确认。

- [ ] **Step 6: 提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt && git add -A
git commit -m "feat: p 键切换项目，选择器支持打字过滤与手输路径"
```

---

## 完成标准

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -- --test-threads=1   # 全绿
cargo clippy -- -D warnings      # 无警告
cargo fmt --check                # 格式干净
```

加上 Task 5 Step 5 的十四条手动验证通过。

## 下一份计划

做完转 `docs/superpowers/plans/2026-08-03-dct-phone-relay.md`（ask_human + Telegram + dc_llm）。
不再往项目选择器上加东西——目录浏览器、模糊匹配、置顶、扫描根目录都已在 spec 里明确否掉。
