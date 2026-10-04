### Task 2: 用量带进 `Advice`、记录和结束语

**Files:**
- Modify: `crates/dct-game/src/ask.rs`（`Advice` 加 `tokens`）、`crates/dct-game/src/play.rs`（`ask` 记录加两个字段）、`src/game/advisor.rs`、`src/game/cli.rs`
- Test: `src/game/advisor.rs`、`crates/dct-game/src/play_tests.rs`、`src/game/cli.rs`（或 `text.rs`）

**Interfaces:**
- Consumes: Task 1 的 `Usage`、`complete_counted_with_timeout`；现有 `Advice`（字段 `choice, reason, raw, model`）、`Say`/`Down` 测试辅助。
- Produces:
  - `Advice` 新增 `pub tokens: Option<(u64, u64)>`（输入、输出）；**所有构造 `Advice` 的地方**（`LlmAdvisor`、`play_tests.rs` 的 `Say`、`bench.rs` 的测试）补 `tokens: None`
  - `ask` 记录新增 `"tokens_in"` / `"tokens_out"`（`Option<u64>`，读不到是 `null`）
  - `LlmAdvisor` 新增 `pub fn summary(&self) -> Option<String>`（没问过返回 `None`）和内部原子计数 `asks`、`counted`、`tokens_in`、`tokens_out`
  - 常量 `src/game/advisor.rs` 里结束语文字见下

- [ ] **Step 1: 写失败的测试**

`src/game/advisor.rs` 的 `tests`（`Fixed` 假后端要能返回用量：给它加一个字段 `usage: Option<Usage>` 并重写 `complete_counted`）：

```rust
    // Fixed 改成：struct Fixed(Result<String, LlmError>, Mutex<Vec<String>>, Option<Usage>);
    // impl Backend for Fixed { fn complete(..) 同前; fn complete_counted(&self, p) -> Result<(String, Option<Usage>), LlmError> { self.complete(p).map(|s| (s, self.2)) } }

    #[test]
    fn usage_is_carried_into_the_advice_and_summed_into_the_summary() {
        let b = Arc::new(Fixed(Ok("1".into()), Mutex::new(vec![]), Some(Usage { input: 100, output: 4 })));
        let a = LlmAdvisor::new(b, "m1".into());
        let r = a.pick(&input()).unwrap();
        assert_eq!(r.tokens, Some((100, 4)));
        a.pick(&input()).unwrap();
        assert_eq!(a.summary().as_deref(), Some("这次问了大模型 2 次，一共用了约 208 个 token（输入 200，输出 8）。"));
    }

    #[test]
    fn summary_says_how_many_asks_had_no_usage() {
        let b = Arc::new(Fixed(Ok("1".into()), Mutex::new(vec![]), None));
        let a = LlmAdvisor::new(b, "m1".into());
        a.pick(&input()).unwrap();
        assert_eq!(a.summary().as_deref(), Some("这次问了大模型 1 次（没有读到用量）。"));
    }

    #[test]
    fn summary_mixes_counted_and_uncounted_asks_honestly() {
        // 两次问：只有第一次有用量
        struct Alt(Mutex<u32>);
        impl Backend for Alt {
            fn complete(&self, _: &Prompt) -> Result<String, LlmError> { Ok("1".into()) }
            fn complete_counted(&self, _: &Prompt) -> Result<(String, Option<Usage>), LlmError> {
                let mut n = self.0.lock().unwrap();
                *n += 1;
                Ok(("1".into(), if *n == 1 { Some(Usage { input: 50, output: 1 }) } else { None }))
            }
        }
        let a = LlmAdvisor::new(Arc::new(Alt(Mutex::new(0))), "m".into());
        a.pick(&input()).unwrap();
        a.pick(&input()).unwrap();
        assert_eq!(a.summary().as_deref(), Some("这次问了大模型 2 次，一共用了约 51 个 token（输入 50，输出 1），其中 1 次没读到用量。"));
    }

    #[test]
    fn no_asks_means_no_summary() {
        let b = Arc::new(Fixed(Ok("1".into()), Mutex::new(vec![]), None));
        assert_eq!(LlmAdvisor::new(b, "m".into()).summary(), None);
    }
```

`play_tests.rs` 里（`Say` 的 `Advice` 构造补 `tokens: Some((11, 2))`；再加）：

```rust
#[test]
fn the_ask_record_carries_the_token_counts() {
    // Say 的 Advice 带 tokens: Some((11, 2))
    let say = Say::new(Some(1));
    let mut d = Fake::new(settled_script());
    let (_, log) = run_ask(&mut d, Some(&say), true, 30, 1);
    assert_eq!(log[0]["ask"]["tokens_in"], 11);
    assert_eq!(log[0]["ask"]["tokens_out"], 2);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test --workspace --locked`
Expected: 编译失败（`Advice.tokens`、`LlmAdvisor::summary` 未定义）。

- [ ] **Step 3: 写实现**

`ask.rs`：`pub struct Advice` 加 `pub tokens: Option<(u64, u64)>,`（带一行文档：输入、输出 token 数，读不到是 `None`）。`play.rs` 里写 `ask_rec` 的 `json!` 加：

```rust
"tokens_in": a.tokens.map(|t| t.0), "tokens_out": a.tokens.map(|t| t.1),
```

`src/game/advisor.rs`：

```rust
use crate::llm::{complete_counted_with_timeout, Backend, Prompt};
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};

pub struct LlmAdvisor {
    backend: Arc<dyn Backend>,
    model: String,
    timeout: Duration,
    down: AtomicBool,
    asks: AtomicU64,
    counted: AtomicU64,
    tokens_in: AtomicU64,
    tokens_out: AtomicU64,
}
```

`new` 里把四个计数初始化为 0。`pick` 里用 `complete_counted_with_timeout`，成功后：

```rust
        let Ok((raw, usage)) = complete_counted_with_timeout(self.backend.clone(), p, self.timeout) else { ...同前... };
        self.asks.fetch_add(1, Ordering::SeqCst);
        let tokens = usage.map(|u| (u.input, u.output));
        if let Some((i, o)) = tokens {
            self.counted.fetch_add(1, Ordering::SeqCst);
            self.tokens_in.fetch_add(i, Ordering::SeqCst);
            self.tokens_out.fetch_add(o, Ordering::SeqCst);
        }
        let (choice, reason) = parse_reply(&raw, i.candidates.len());
        Some(Advice { choice, reason, raw, model: self.model.clone(), tokens })
```

（注意 `pick` 的参数名是 `i: &AskInput`，上面解构用 `(ti, to)` 避免重名。）

```rust
    /// 结束语：没问过返回 `None`。用量读到几次算几次，读不到的次数明说。
    pub fn summary(&self) -> Option<String> {
        let asks = self.asks.load(Ordering::SeqCst);
        if asks == 0 {
            return None;
        }
        let counted = self.counted.load(Ordering::SeqCst);
        if counted == 0 {
            return Some(format!("这次问了大模型 {asks} 次（没有读到用量）。"));
        }
        let (i, o) = (self.tokens_in.load(Ordering::SeqCst), self.tokens_out.load(Ordering::SeqCst));
        let mut s = format!("这次问了大模型 {asks} 次，一共用了约 {} 个 token（输入 {i}，输出 {o}）", i + o);
        if counted < asks {
            s += &format!("，其中 {} 次没读到用量", asks - counted);
        }
        s += "。";
        Some(s)
    }
```

`src/game/cli.rs`：在 `play`/`auto_next` 返回、打印完停止语句之后（找到打印 `stop_line` 的位置），加：

```rust
    if let Some(a) = &advisor {
        if let Some(line) = a.summary() {
            println!("{line}");
        }
    }
```

所有其他构造 `Advice { .. }` 的地方（`play_tests.rs` 的 `Say`、`bench.rs` 测试、`advisor.rs` 旧测试）补 `tokens: None`。

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked` 与 clippy。
Expected: 全绿，无警告。

- [ ] **Step 5: 变异检查，然后提交**

把 `summary` 里 `counted < asks` 的分支删掉 → `summary_mixes_…` 变红；把 `asks.fetch_add` 挪到失败分支之前（失败也计数）→ 需要一个测试覆盖「失败的问不计入」：加断言（后端 `Err` 一次后 `summary()` 仍为 `None`）并确认该变异变红。

```bash
git add src crates
git commit -m "feat(game): record token usage on every model ask and say the total when the run ends"
```

---

