## Task 8: 入站落地 —— 敲进 PTY、回执、journal

**Files:**
- Modify: `src/bridge.rs`、`src/journal.rs`
- Test: `src/bridge.rs` 内

**Interfaces:**
- Consumes: Task 7 的 `Route`、`session::Sessions::send_input(id, text) -> Result<()>`
- Produces: `Bridge::deliver(&self, route: Route, text: &str) -> Delivered`、`Delivered { Typed(u32), AskedWhich(Vec<u32>), SaidGone, SaidNeedUse, Failed(String) }`

- [ ] **Step 1: 写失败测试**

用一个假的写入器（记录被写了什么、写给谁），不碰真 PTY。

```rust
/// 回执不是锦上添花：用户在外面看不见终端，没有回执他不知道这句话
/// 到底进去没有。
#[test]
fn typing_it_in_sends_a_receipt_naming_the_session() {
    let (b, spy) = Bridge::for_test_with_writer();
    let d = b.deliver(Route::To(7), "先跑完");
    assert_eq!(d, Delivered::Typed(7));
    assert_eq!(spy.written(), vec![(7, "先跑完".to_string())]);
    assert!(spy.last_reply().contains("修登录白屏"), "回执里没说敲给了谁");
}

/// `Gone` 什么都不敲。这是重启之后那条安全路径的落地，
/// 光有 `route()` 返回 `Gone` 不够，得确认真的没写出去。
#[test]
fn a_gone_route_writes_nothing_at_all() {
    let (b, spy) = Bridge::for_test_with_writer();
    assert_eq!(b.deliver(Route::Gone, "先跑完"), Delivered::SaidGone);
    assert!(spy.written().is_empty(), "旧消息被敲进了会话");
}

#[test]
fn asking_which_writes_nothing_either() {
    let (b, spy) = Bridge::for_test_with_writer();
    assert_eq!(b.deliver(Route::Ask(vec![9, 10]), "先跑完"), Delivered::AskedWhich(vec![9, 10]));
    assert!(spy.written().is_empty());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge::tests::typing -- --test-threads=1`
Expected: `cannot find method deliver`

- [ ] **Step 3: 实现**

`deliver` 按 `Route` 分派：`To(id)` 调 `send_input` 再发回执；`Ask` 发候选列表；`Gone` 发「这条消息对应的会话已经不在了」；`NeedUse` 发「先 `/ls` 看看有哪些会话」。**全部记 journal。**

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib bridge:: -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把 `Gone` 分支改成也调 `send_input` —— `a_gone_route_writes_nothing_at_all` 必须失败。把回执里的会话名换成会话号 —— `typing_it_in_sends_a_receipt_naming_the_session` 必须失败。

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/bridge.rs src/journal.rs
git commit -m "feat: type the reply in, then say where it went

You cannot see the terminal from a train, so a receipt naming the session is
the only evidence the sentence landed. Two of the three routes deliberately
write nothing, and the tests assert the absence rather than the message --
that is the half that can go wrong quietly."
```

---

