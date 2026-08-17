## Task 10: 智能（入站）—— 听懂回复、猜路由

**Files:**
- Modify: `src/bridge.rs`
- Test: `src/bridge.rs` 内

**Interfaces:**
- Consumes: Task 7 的 `route`/`Route`、`llm::complete_with_timeout`
- Produces: `map_answer(user: &str, options: Option<&[String]>, backend) -> String`、`narrow(candidates: &[u32], text: &str, backend) -> Option<u32>`

- [ ] **Step 1: 写失败测试**

**红线在这里，测试就是红线本身。**

```rust
/// agent 要的是自由文本时模型完全不介入。模型一旦开始润色，敲进 agent 的
/// 就不再是用户说的话，而他在手机上看不见这件事。
#[test]
fn free_text_is_typed_verbatim_and_never_reaches_the_model() {
    let spy = SpyBackend::new(); // 被调用就记一笔
    let out = map_answer("那个啥 你先把测试跑一下然后再说", None, &spy);
    assert_eq!(out, "那个啥 你先把测试跑一下然后再说");
    assert_eq!(spy.calls(), 0, "自由文本却调了模型");
}

#[test]
fn a_spoken_ordinal_becomes_the_option_the_agent_wants() {
    let b = FakeBackend::answering("2");
    let opts = vec!["先跑完".to_string(), "现在改".to_string()];
    assert_eq!(map_answer("就第二个吧", Some(&opts), &b), "2");
}

/// 映射不确定就原样发。这是红线的另一半。
#[test]
fn an_unmappable_answer_is_sent_as_typed() {
    let b = FakeBackend::answering("我不确定");
    let opts = vec!["先跑完".to_string(), "现在改".to_string()];
    assert_eq!(map_answer("等等我再想想", Some(&opts), &b), "等等我再想想");
}

#[test]
fn a_model_timeout_sends_what_the_user_typed() {
    let b = FakeBackend::timing_out();
    let opts = vec!["先跑完".to_string()];
    assert_eq!(map_answer("就第一个", Some(&opts), &b), "就第一个");
}

/// 猜路由不确定就还是反问。**永远不因为「模型有把握」跳过那一问**——
/// 敲错 agent 的代价比多问一句大得多。
#[test]
fn an_uncertain_narrow_still_asks() {
    let b = FakeBackend::answering("说不好");
    assert_eq!(narrow(&[9, 10], "先跑完", &b), None);
}

/// 模型答了一个不在候选里的会话号，一律不采信。
#[test]
fn a_narrow_outside_the_candidates_is_refused() {
    let b = FakeBackend::answering("77");
    assert_eq!(narrow(&[9, 10], "先跑完", &b), None);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge::tests::free_text -- --test-threads=1`
Expected: `cannot find function map_answer`

- [ ] **Step 3: 实现**

```rust
/// 把用户的话变成 agent 要的形式。**只转格式，不造内容。**
pub fn map_answer(user: &str, options: Option<&[String]>, b: &dyn Backend) -> String {
    // agent 要的是自由文本：模型完全不介入。**这个 early return 就是红线。**
    let Some(opts) = options else {
        return user.to_string();
    };
    if opts.is_empty() {
        return user.to_string();
    }
    match complete_with_timeout(/* … 8 秒硬超时 … */) {
        // 答案必须是候选里的序号，别的一律不采信
        Ok(a) => match a.trim().parse::<usize>() {
            Ok(n) if n >= 1 && n <= opts.len() => n.to_string(),
            _ => user.to_string(),
        },
        Err(_) => user.to_string(),
    }
}
```

`narrow` 只在 `Route::Ask` 那一条被调用（Task 7 规则 4），答案不在候选里就返回 `None`，调用方照常反问。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

**这一步在这个任务里最重要：**
- 去掉 `options` 为 `None` 时的 early return（让自由文本也过模型）—— `free_text_is_typed_verbatim_and_never_reaches_the_model` 必须失败
- 把序号范围 `n >= 1 && n <= opts.len()` 改成 `n <= opts.len()`（放进 0）—— **如果没有测试失败，补一条 `answering("0")` 的测试**
- 把 `narrow` 的越界检查去掉 —— `a_narrow_outside_the_candidates_is_refused` 必须失败

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/bridge.rs
git commit -m "feat: understand 'the second one' without rewriting what you said

The model only ever converts a spoken ordinal into the token the agent is
waiting for. When the agent wants free text the model is not called at all --
that early return is the whole guarantee, because a polished version of your
sentence is something you cannot see from a phone.

Every failure path sends what you typed: no options, no mapping, a timeout,
a number outside the list."
```

---

