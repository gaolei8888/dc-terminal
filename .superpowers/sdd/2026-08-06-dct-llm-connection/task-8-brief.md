### Task 8: 出错解释——会话失败时说人话

**Files:**
- Modify: `src/session.rs`（`Session` 加字段；`tick()` 里检测进入 `Failed` 的那一刻）
- Modify: `src/proto.rs`（`Request::Explanation { id }`、`Response::Explanation(Option<String>)`）
- Modify: `src/daemon.rs`（接线；启动时 resolve 一次后端）
- Modify: `src/ui/mod.rs`（`Failed` 会话上显示解释）
- Modify: `src/i18n.rs`（新词条）

**Interfaces:**
- Consumes: `llm::{Backend, Prompt, complete_with_timeout}`、`llm::resolve::resolve`
- Produces:
  - `session::Session` 新字段 `explanation_slot: Arc<Mutex<Option<String>>>`
    （**必须是 `Arc<Mutex<_>>`**：解释由后台线程写回，而那个线程拿不到
    `Session` 的锁——`tick()` 正持着它。裸 `Option<String>` 编不过。）
  - `session::explain_prompt(screen: &str) -> Prompt`
  - `SessionManager::set_backend(&self, b: Option<Arc<dyn Backend>>)`
  - `SessionManager::explanation(&self, id: u32) -> Option<String>`

**说明：** 触发点是**状态迁移进 `Failed` 的那一刻**，不是「只要还是 `Failed` 就一直问」——后者会每 200ms 打一次模型。

**退路：** 后端没配 / 调不通 / 超时，`explanation` 保持 `None`，界面显示今天就有的那句失败提示。**功能安静下线，不打扰用户。**

截屏文本要**截尾**再送：整屏可能几千字，只要最后 2000 字符——错误一定在末尾。

- [ ] **Step 1: 写失败的测试**

`src/session.rs` 的 `mod tests` 里追加：

```rust
#[test]
fn the_explain_prompt_carries_the_tail_of_the_screen() {
    let long = "x".repeat(5000) + "API Error: Connection closed mid-response.";
    let p = explain_prompt(&long);
    assert!(p.user.contains("API Error"), "错误在末尾，必须送到");
    assert!(p.user.chars().count() < 2500, "整屏太长，要截尾");
    assert!(p.system.contains("中文"), "用户默认中文");
}

#[test]
fn the_explain_prompt_asks_for_plain_language() {
    let p = explain_prompt("API Error: Connection closed mid-response.");
    // 目标用户零编程经验：不要栈追踪、不要术语。
    assert!(p.system.contains("不要"), "要明确禁止术语/栈追踪");
    assert!(p.max_tokens <= 200, "一句话就够，别让它写小作文");
}

#[test]
fn with_no_backend_the_explanation_stays_empty_and_nothing_breaks() {
    // 这是「非 LLM 退路」的回归点：没配后端时 dct 表现得和今天一模一样。
    let repo = init_repo();
    let m = SessionManager::new();
    m.register_profile(fake_agent());
    let id = m.create(repo.path(), "fake", empty_secrets(), &[]).unwrap();
    m.set_backend(None);
    m.tick();
    assert_eq!(m.explanation(id), None);
}

#[test]
fn entering_failed_asks_the_backend_once_not_every_tick() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Counting(Arc<AtomicUsize>);
    impl crate::llm::Backend for Counting {
        fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok("网络断了，重开一次就行。".into())
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let repo = init_repo();
    let m = SessionManager::new();
    m.register_profile(failing_agent()); // error_pattern 命中的假 agent
    let id = m.create(repo.path(), "failing", empty_secrets(), &[]).unwrap();
    m.set_backend(Some(Arc::new(Counting(calls.clone()))));

    let deadline = Instant::now() + Duration::from_secs(5);
    while m.explanation(id).is_none() && Instant::now() < deadline {
        m.tick();
        sleep(Duration::from_millis(50));
    }
    assert_eq!(m.explanation(id).as_deref(), Some("网络断了，重开一次就行。"));

    // 再 tick 若干轮：还是 Failed，但**不许**再问模型。
    for _ in 0..10 {
        m.tick();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "只在进入 Failed 那一刻问一次");
}
```

在 `mod tests` 里加一个 `failing_agent()` 辅助函数，照现有 `fake_agent()` 的写法，profile 带 `error_pattern = "BOOM"`，命令是一个会打印 `BOOM` 的 shell。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib session::`
Expected: 编译失败，`cannot find function 'explain_prompt'`

- [ ] **Step 3: 写实现**

`src/session.rs`：

1. `Session` 结构加 `explanation_slot: Arc<Mutex<Option<String>>>`（`new` 里初始化为
   `Arc::new(Mutex::new(None))`）。`SessionManager::explanation(id)` 读它并 clone 出来。
2. `SessionManager` 加 `backend: Mutex<Option<Arc<dyn crate::llm::Backend>>>`
3. 加两个方法与一个纯函数：

```rust
/// 让模型把一屏失败翻译成一句人话。
///
/// **只送屏幕末尾**：整屏可能几千字，而错误一定在末尾。整屏送过去既慢又贵，
/// 还容易让模型抓错重点。
pub fn explain_prompt(screen: &str) -> crate::llm::Prompt {
    const TAIL: usize = 2000;
    let tail: String = {
        let chars: Vec<char> = screen.chars().collect();
        let start = chars.len().saturating_sub(TAIL);
        chars[start..].iter().collect()
    };
    crate::llm::Prompt {
        system: "你在帮一个完全不懂编程的人。用中文，一到两句话说清楚刚才那个\
                 命令行工具出了什么事、他现在该做什么。不要出现英文报错原文、\
                 不要栈追踪、不要术语、不要代码。"
            .into(),
        user: format!("这是屏幕上的最后一段内容：\n\n{tail}"),
        max_tokens: 200,
    }
}
```

4. `tick()` 里，在 `s.state = next;` 那一步**之前**记下 `let was = s.state;`，赋值之后加：

```rust
// 只在**进入** Failed 的那一刻问一次。条件写成「原来不是 Failed」而不是
// 「现在是 Failed」——后者会每 200ms 打一次模型，一个失败会话能把额度烧光。
if next == SessionState::Failed && was != SessionState::Failed {
    self.request_explanation(&mut s);
}
```

5. `request_explanation` 把工作丢到后台线程（**绝不在 tick 里同步等模型**），完成后写回 `explanation`：

```rust
/// **绝不在 tick 里同步等模型。** tick 每 200ms 一轮，一次同步调用就能
/// 让整个守护进程卡住，而卡住的 dct 和死掉的 agent 长得一模一样。
fn request_explanation(&self, s: &mut Session) {
    let Some(b) = self.backend.lock().ok().and_then(|g| g.clone()) else {
        return; // 没配后端：功能安静下线，会话照跑
    };
    let p = explain_prompt(&s.pty.screen_text());
    let slot = s.explanation_slot.clone(); // Arc<Mutex<Option<String>>>
    std::thread::spawn(move || {
        if let Ok(text) =
            crate::llm::complete_with_timeout(b, p, std::time::Duration::from_secs(30))
        {
            if let Ok(mut g) = slot.lock() {
                *g = Some(text);
            }
        }
        // 失败就什么都不做——界面显示今天就有的那句失败提示
    });
}
```

（把 `explanation` 实现成 `Arc<Mutex<Option<String>>>` 字段 `explanation_slot`，`explanation(id)` 读它。）

`src/proto.rs` 加 `Request::Explanation { id: u32 }` 与 `Response::Explanation(Option<String>)`（加在各自枚举**末尾**，不动既有变体的顺序）。`src/daemon.rs` 接上这条请求，并在启动时 resolve 一次后端调用 `set_backend`，resolve 失败只往 stderr 写一行、`set_backend(None)`。

`src/ui/mod.rs`：会话是 `Failed` 且拿得到解释时，把那句话显示在既有的失败提示位置；拿不到就维持现状。`src/i18n.rs` 加对应词条。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib && cargo build`
Expected: 全绿

- [ ] **Step 5: 提交**

```bash
cargo fmt && cargo test --lib && cargo build && git diff --check
git add src/session.rs src/proto.rs src/daemon.rs src/ui/mod.rs src/i18n.rs
git commit -m "feat: explain in plain language why a session failed

Fires once on the transition into Failed, not while Failed: the latter would
hit the model every 200ms and burn a quota on a single broken session.

The call runs on a worker thread — tick() must never wait on a model, since
a stalled daemon is indistinguishable from a dead agent on screen. With no
backend configured, or on any failure, the explanation stays empty and dct
behaves exactly as it does today."
```

---

