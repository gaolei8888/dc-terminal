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

