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

