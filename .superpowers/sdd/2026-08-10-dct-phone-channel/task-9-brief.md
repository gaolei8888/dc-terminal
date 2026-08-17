## Task 9: 智能（出站）—— 合并与编号选项

**Files:**
- Modify: `src/bridge.rs`、`src/llm/mod.rs`
- Test: `src/bridge.rs` 内

**Interfaces:**
- Consumes: Task 1 的 `Event`、`llm::complete_with_timeout`、`llm::Backend`
- Produces: `merge(&[Event], Lang) -> String`、`options_prompt(screen: &str) -> Prompt`、`parse_options(&str) -> Option<Vec<String>>`

- [ ] **Step 1: 写失败测试**

```rust
/// 合并不需要模型。断网八小时不该在恢复瞬间收到五百条。
#[test]
fn several_events_become_one_message() {
    let evs = vec![
        Event { session: 1, kind: EventKind::Stopped, name: "修登录白屏".into(), project: "web".into() },
        Event { session: 2, kind: EventKind::Failed, name: "对账".into(), project: "fin".into() },
    ];
    let m = merge(&evs, Lang::Zh);
    assert!(m.contains("修登录白屏") && m.contains("对账"));
    // 一条消息，不是两条拼起来——两个会话名之间不该出现消息分隔
    assert_eq!(m.matches("\n\n\n").count(), 0);
}

#[test]
fn a_single_event_is_not_dressed_up_as_a_list() {
    let evs = vec![Event { session: 1, kind: EventKind::Stopped, name: "修登录白屏".into(), project: "web".into() }];
    let m = merge(&evs, Lang::Zh);
    assert!(!m.contains("1."), "只有一件事却排了个编号列表：{m}");
}

/// 模型答得不成形就当没有选项——**绝不猜**，退回只有元数据的消息。
#[test]
fn unparseable_options_mean_no_options() {
    assert_eq!(parse_options("我觉得他大概想问你要不要继续吧"), None);
    assert_eq!(parse_options(""), None);
}

#[test]
fn options_come_back_in_order() {
    let got = parse_options("1. 先跑完\n2. 现在改").unwrap();
    assert_eq!(got, vec!["先跑完".to_string(), "现在改".to_string()]);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge::tests::several_events -- --test-threads=1`
Expected: `cannot find function merge`

- [ ] **Step 3: 实现**

`merge` 纯函数。`options_prompt` 走 `request_explanation` 已建立的范式：**兜底（只有元数据的消息）同步先就位**，再起线程问模型，硬超时 15 秒，超时/畸形就发兜底。

**消息里绝不出现路径、diff、代码块** —— prompt 里明确要求，且 `parse_options` 把含 `/`、`\`` 的候选项丢弃。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把 `parse_options` 的失败分支改成返回 `Some(vec![原文])` —— `unparseable_options_mean_no_options` 必须失败。把 `merge` 单条那一支去掉（永远排编号列表）—— `a_single_event_is_not_dressed_up_as_a_list` 必须失败。

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/bridge.rs src/llm/mod.rs
git commit -m "feat: one message for several agents, numbered options for one

Merging needs no model at all, which is why it is the one piece of this that
still works with [llm] unset.

Options do need one, so the fallback is written synchronously before the
thread starts: a slow model makes the message plainer, never later. An answer
that does not parse yields no options rather than a guessed list."
```

---

