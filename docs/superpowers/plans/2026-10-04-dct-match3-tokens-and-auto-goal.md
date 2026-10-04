# dct 玩三消：记录 token、自动认目标数字 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 每次问大模型都记下用了多少 token 并在结束时说一句；`dct game play` 不给 `--goal-number` 时，靠数字的变化规律自己认出目标数字。

**Architecture:** 三块互相独立。Task 1 在 `src/llm/` 加「带用量的调用」。Task 2 把用量带进 `Advice`、记录和结束语。Task 3 在 `crates/dct-game` 新增 `goal.rs`（只看数字变化的目标识别器），`play()` 里把三处重复的停滞计数更新收成一个函数，并让它用「用户给的下标，否则识别器认出的下标」。

**Tech Stack:** Rust（workspace `dct` + `crates/dct-game`），`serde_json`。

**Spec:** `docs/superpowers/specs/2026-10-04-dct-match3-tokens-and-auto-goal-design.md`

## Global Constraints

- 提交信息用英文，**不加任何 AI 署名或 Co-Authored-By 行**（用户的长期规则，优先于默认的署名提示）。不推送，用户说「推吧」才推。
- 面向用户的文字用大白话中文，不出现 `token` 以外的英文术语（用户那句话里写「字」不好懂，这里用「token」并在括号里说明）；本轮唯一的 token 字样是结束语：「这次问了大模型 N 次，一共用了约 X 个 token（输入 A，输出 B）。」
- dct 只放通用代码：不得出现任何游戏的名字、颜色、数字含义；「步数 = 每成功一步刚好降 1」这条按数字变化判定，不看文字。
- 读不到用量绝不丢答案；认不出目标就和现在不给 `--goal-number` 时完全一样（不判停滞、不因停滞问模型）。
- 新增记录字段只追加。
- 推送前跑：`cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings` 和 `cargo +1.99.0 test --workspace --locked`（`PATH` 含 `$HOME/.cargo/bin`，`CARGO_TARGET_DIR=/private/tmp/claude-502/-Users-lei-work-dc-dc-terminal/d52f1f4f-cb49-40cc-ba88-c37403102743/scratchpad/target-199`）。已知偶发失败：`daemon::web_tests::enabling_starts_a_listener_and_disabling_stops_it`、`session::tests::recovering_from_a_failure_after_real_input_still_does_not_count`，重跑。
- 不用 `cargo fmt`（这个工具链没装）。

## Review Focus

1. 响应体里 `usage` 不是对象、字段是字符串或负数、数字超过 `u64`：`Usage` 为 `None`，答案照常返回。→ Task 1。
2. 一次命令里问了很多次、其中有的读到用量有的没读到：结束语的总数只加读到的，并说明「有 K 次没读到」。→ Task 2。
3. 数字列表个数一会儿变一会儿不变（弹窗、分数飘字）：个数不同的步不进样本，识别器不被带偏，也不 panic。→ Task 3。
4. 用户给了 `--goal-number` 却超出这一屏数字的个数：沿用第四轮行为（该步未知，计数不动），识别器不插手。→ Task 3。
5. 连续两步之间同一个数字先降后升（识别后目标数字被重置）：认定结果不反复横跳（一旦认出就在本次运行里保持，除非列表个数永久改变）。→ Task 3。

---

### Task 1: `llm` 层的「带用量的调用」

**Files:**
- Modify: `src/llm/mod.rs`、`src/llm/http.rs`
- Test: 两个文件各自的 `tests` 模块

**Interfaces:**
- Consumes: 现有 `Backend::complete`、`complete_with_timeout`、`http::extract_text`、`HttpBackend`、`Wire`。
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Usage { pub input: u64, pub output: u64 }`（在 `src/llm/mod.rs`）
  - `Backend::complete_counted(&self, p: &Prompt) -> Result<(String, Option<Usage>), LlmError>`，**有默认实现**：`self.complete(p).map(|s| (s, None))`
  - `pub fn complete_counted_with_timeout(b: Arc<dyn Backend>, p: Prompt, d: Duration) -> Result<(String, Option<Usage>), LlmError>`
  - `http::extract_usage(wire: Wire, body: &str) -> Option<Usage>`

- [ ] **Step 1: 写失败的测试**

`src/llm/http.rs` 的 `tests` 模块末尾：

```rust
    #[test]
    fn usage_is_read_from_openai_and_anthropic_shapes() {
        let o = r#"{"choices":[{"message":{"content":"2"}}],"usage":{"prompt_tokens":120,"completion_tokens":5,"total_tokens":125}}"#;
        assert_eq!(extract_usage(Wire::Openai, o), Some(Usage { input: 120, output: 5 }));
        let a = r#"{"content":[{"type":"text","text":"2"}],"usage":{"input_tokens":77,"output_tokens":3}}"#;
        assert_eq!(extract_usage(Wire::Anthropic, a), Some(Usage { input: 77, output: 3 }));
    }

    #[test]
    fn usage_is_none_when_missing_or_malformed_but_the_answer_still_comes_back() {
        for body in [
            r#"{"choices":[{"message":{"content":"2"}}]}"#,
            r#"{"choices":[{"message":{"content":"2"}}],"usage":null}"#,
            r#"{"choices":[{"message":{"content":"2"}}],"usage":"many"}"#,
            r#"{"choices":[{"message":{"content":"2"}}],"usage":{"prompt_tokens":"12","completion_tokens":5}}"#,
            r#"{"choices":[{"message":{"content":"2"}}],"usage":{"prompt_tokens":-3,"completion_tokens":5}}"#,
            r#"{"choices":[{"message":{"content":"2"}}],"usage":{"prompt_tokens":12}}"#,
            r#"{"choices":[{"message":{"content":"2"}}],"usage":{"prompt_tokens":99999999999999999999999,"completion_tokens":1}}"#,
        ] {
            assert_eq!(extract_usage(Wire::Openai, body), None, "{body}");
            assert_eq!(extract_text(Wire::Openai, body).as_deref(), Some("2"), "{body}");
        }
    }

    #[test]
    fn http_backend_complete_counted_returns_text_and_usage() {
        let sender: Arc<Sender> = Arc::new(|_, _, _| {
            Ok((200, r#"{"choices":[{"message":{"content":"3"}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#.to_string()))
        });
        let b = HttpBackend::with_sender("http://x".into(), Wire::Openai, "m".into(), Credential::Bearer("k".into()), sender);
        let p = Prompt { system: "s".into(), user: "u".into(), max_tokens: 8 };
        assert_eq!(b.complete_counted(&p), Ok(("3".to_string(), Some(Usage { input: 10, output: 2 }))));
        assert_eq!(b.complete(&p), Ok("3".to_string()));
    }
```

（用到的 `Credential::Bearer(String)`、`Prompt` 字段以 `http.rs` 现有测试的写法为准；若 `Credential` 的构造不同，照现有测试里 `with_sender` 的用法改，断言不变。）

`src/llm/mod.rs` 的 `tests` 模块末尾：

```rust
    #[test]
    fn complete_counted_default_has_no_usage_and_the_same_text() {
        let b = Fixed(Ok("hi".into()));
        let p = Prompt { system: String::new(), user: String::new(), max_tokens: 1 };
        assert_eq!(b.complete_counted(&p), Ok(("hi".to_string(), None)));
    }

    #[test]
    fn complete_counted_with_timeout_times_out_like_the_plain_one() {
        let r = complete_counted_with_timeout(
            Arc::new(Slow(Duration::from_millis(300))),
            Prompt { system: String::new(), user: String::new(), max_tokens: 1 },
            Duration::from_millis(50),
        );
        assert_eq!(r, Err(LlmError::Timeout));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test --lib llm::`
Expected: 编译失败（`Usage`、`extract_usage`、`complete_counted` 未定义）。

- [ ] **Step 3: 写实现**

`src/llm/mod.rs`：

```rust
/// 一次调用用掉的 token 数（输入、输出）。读不到就是 `None`，不是 0。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
}

pub trait Backend: Send + Sync {
    fn complete(&self, p: &Prompt) -> Result<String, LlmError>;

    /// 带用量的调用。默认不带用量（命令行后端读不到）；HTTP 后端重写它。
    fn complete_counted(&self, p: &Prompt) -> Result<(String, Option<Usage>), LlmError> {
        self.complete(p).map(|s| (s, None))
    }
}

pub fn complete_counted_with_timeout(
    b: Arc<dyn Backend>,
    p: Prompt,
    d: Duration,
) -> Result<(String, Option<Usage>), LlmError> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(b.complete_counted(&p));
    });
    match rx.recv_timeout(d) {
        Ok(r) => r,
        Err(_) => Err(LlmError::Timeout),
    }
}
```

（`LlmError` 需要 `PartialEq` 才能 `assert_eq!`：它已经 `derive(PartialEq, Eq)`，见 `mod.rs` 现有定义。）

`src/llm/http.rs`：顶部 `use super::{Backend, LlmError, Prompt, Usage};`，加

```rust
/// 读用量：OpenAI 型 `usage.prompt_tokens / completion_tokens`，Anthropic 型 `usage.input_tokens / output_tokens`。
/// 两个字段都必须是非负整数（JSON 里的 u64）才算读到；缺一个、是字符串、是负数、超出范围都是 `None`——
/// 读不到用量绝不能丢掉一个好答案，所以这里只返回 `None`，从不报错。
pub fn extract_usage(wire: Wire, body: &str) -> Option<Usage> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let u = v.get("usage")?;
    let (a, b) = match wire {
        Wire::Openai => ("prompt_tokens", "completion_tokens"),
        Wire::Anthropic => ("input_tokens", "output_tokens"),
    };
    Some(Usage { input: u.get(a)?.as_u64()?, output: u.get(b)?.as_u64()? })
}
```

`impl Backend for HttpBackend` 里把 `complete` 的主体搬进 `complete_counted`，`complete` 改成调用它再丢掉用量：

```rust
    fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
        self.complete_counted(p).map(|(s, _)| s)
    }

    fn complete_counted(&self, p: &Prompt) -> Result<(String, Option<Usage>), LlmError> {
        let body = body_for(self.wire, &self.model, p);
        let (status, text) = (self.sender)(&self.url, &self.cred, &body).map_err(|e| {
            eprintln!("LLM HTTP 调用失败：{e}");
            LlmError::Unavailable
        })?;
        if !(200..300).contains(&status) {
            eprintln!("LLM HTTP 返回 {status}");
            return Err(LlmError::Unavailable);
        }
        // 读不懂 = 没把握。绝不猜一个答案出来；用量读不到不影响答案。
        let answer = extract_text(self.wire, &text).ok_or(LlmError::Malformed)?;
        Ok((answer, extract_usage(self.wire, &text)))
    }
```

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked` 与 clippy。
Expected: 全绿，无警告。

- [ ] **Step 5: 变异检查，然后提交**

把 `extract_usage` 里的 `.as_u64()?` 改成 `.as_f64()? as u64` → `usage_is_none_when_…` 应变红（负数、字符串）；把 Anthropic 的字段名改成 OpenAI 的 → `usage_is_read_…` 变红。改回。

```bash
git add src/llm
git commit -m "feat(llm): complete_counted returns token usage from OpenAI and Anthropic style responses; unreadable usage never loses the answer"
```

---

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

### Task 3: 自动认出目标数字

**Files:**
- Create: `crates/dct-game/src/goal.rs`
- Modify: `crates/dct-game/src/lib.rs`（`pub mod goal;`）、`crates/dct-game/src/play.rs`、`src/game/text.rs`、`src/game/skill.md`
- Test: `crates/dct-game/src/goal.rs`、`crates/dct-game/src/play_tests.rs`、`src/game/text.rs`

**Interfaces:**
- Consumes: 第四轮的 `progress_prev`/`progress_after`、`goal_dropped(before, after, gi) -> Option<bool>`、`no_progress`、`Options.goal_index`、`STALL_STEPS`。
- Produces:
  - `pub struct GoalFinder`，`GoalFinder::new()`，`observe(&mut self, before: &[u64], after: &[u64])`（只在**成功走了一步**时调用；个数不同的忽略），`index(&self) -> Option<usize>`，`pub const MIN_SAMPLES: usize = 4;`
  - `play()` 里 `fn effective_goal(o_goal: Option<usize>, finder: &GoalFinder) -> Option<usize>`（用户给的优先）
  - 记录：`progress` 对象多 `"goal_index"`（当前生效的下标，0 起，没有就是 `null`）；认出的那一步多一个顶层字段 `"goal_found": N`（1 起，只在认出的那一步出现）
  - `step_line` 在有 `goal_found` 时追加「；我认为屏幕上第 N 个数字是这一关的目标」

识别规则（`GoalFinder`，只看数字，不看意思）：对每个下标 i 累计 `samples[i]`、`all_minus_one[i]`（到目前为止每次变化都刚好是 −1）、`ever_increased[i]`。样本只来自「前后个数相同」的成功步。至少有 `MIN_SAMPLES` 个样本后：排除 `all_minus_one[i]`（步数）和 `ever_increased[i]`（分数、连击）；若**恰好剩一个**下标，就是目标，并且**一旦认出就在本次运行里保持**（`found: Option<usize>` 锁定，个数永久变化也不改，避免反复横跳）。个数不同的步只是不进样本。

- [ ] **Step 1: 写失败的测试**

`goal.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn feed(f: &mut GoalFinder, steps: &[(&[u64], &[u64])]) {
        for (b, a) in steps {
            f.observe(b, a);
        }
    }

    #[test]
    fn the_step_counter_and_a_rising_score_are_excluded_leaving_the_goal() {
        // [步数, 目标, 分数]：步数每步 -1，目标不规律降，分数涨
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122, 0], &[37, 110, 120]),
            (&[37, 110, 120], &[36, 100, 300]),
            (&[36, 100, 300], &[35, 100, 340]),
            (&[35, 100, 340], &[34, 80, 600]),
        ]);
        assert_eq!(f.index(), Some(1));
    }

    #[test]
    fn a_goal_that_never_moves_is_still_found_by_elimination() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[20, 50, 0], &[19, 50, 100]),
            (&[19, 50, 100], &[18, 50, 150]),
            (&[18, 50, 150], &[17, 50, 400]),
            (&[17, 50, 400], &[16, 50, 410]),
        ]);
        assert_eq!(f.index(), Some(1));
    }

    #[test]
    fn two_irregular_numbers_are_ambiguous() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122, 30], &[37, 110, 25]),
            (&[37, 110, 25], &[36, 100, 20]),
            (&[36, 100, 20], &[35, 100, 15]),
            (&[35, 100, 15], &[34, 80, 5]),
        ]);
        assert_eq!(f.index(), None);
    }

    #[test]
    fn fewer_than_four_samples_decide_nothing() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122], &[37, 110]),
            (&[37, 110], &[36, 100]),
            (&[36, 100], &[35, 90]),
        ]);
        assert_eq!(f.index(), None);
    }

    #[test]
    fn steps_with_different_list_lengths_are_not_samples() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122], &[37, 110]),
            (&[37, 110], &[36]),            // 漏读一个：不进样本
            (&[36, 100], &[35, 100, 9]),    // 多出一个：不进样本
            (&[35, 100], &[34, 90]),
            (&[34, 90], &[33, 80]),
        ]);
        assert_eq!(f.index(), None, "只有 3 个有效样本");
        feed(&mut f, &[(&[33, 80], &[32, 70])]);
        assert_eq!(f.index(), Some(1));
    }

    #[test]
    fn once_found_the_goal_stays_found_for_the_run() {
        let mut f = GoalFinder::new();
        feed(&mut f, &[
            (&[38, 122, 0], &[37, 110, 120]),
            (&[37, 110, 120], &[36, 100, 300]),
            (&[36, 100, 300], &[35, 100, 340]),
            (&[35, 100, 340], &[34, 80, 600]),
        ]);
        assert_eq!(f.index(), Some(1));
        // 之后目标数字涨了一次（重置）也不改
        feed(&mut f, &[(&[34, 80, 600], &[33, 90, 650])]);
        assert_eq!(f.index(), Some(1));
    }
}
```

`play_tests.rs` 末尾（`Fake` 的 `texts` 队列、`run_ask`、`Say`、`settled_script` 已有）：

```rust
#[test]
fn without_goal_number_the_finder_picks_the_goal_and_stall_then_works() {
    // 数字列表 [步数, 目标]：步数每步 -1，目标恒为 50（不动）。4 步样本后认出目标 = 下标 1，之后 6 步没降 → 停滞 → 问模型
    let say = Say::new(Some(0));
    let mut reads = vec![Ok(grid(A))];
    for _ in 0..60 {
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(A)));
        reads.push(Ok(grid(A)));
    }
    let mut d = Fake::new(reads);
    let mut texts: Vec<Vec<&'static str>> = vec![vec!["30", "50"]];
    for i in 0..14 {
        texts.push(vec![Box::leak(format!("{}", 29 - i).into_boxed_str()), "50"]);
    }
    d.texts = texts.into();
    // goal_index 为 None（用户没给）→ 认
    let (_, log) = run_ask(&mut d, Some(&say), false, 30, 14);
    assert!(log.iter().any(|l| l["goal_found"] == 2), "应在某一步认出第 2 个数字：{log:?}");
    assert!(log.iter().any(|l| l["stalled"] == true));
    assert!(say.calls.get() >= 1);
}

#[test]
fn a_user_given_goal_number_is_never_overridden_by_the_finder() {
    let mut reads = vec![Ok(grid(A))];
    for _ in 0..60 {
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(A)));
        reads.push(Ok(grid(A)));
    }
    let mut d = Fake::new(reads);
    let mut texts: Vec<Vec<&'static str>> = vec![vec!["30", "50"]];
    for i in 0..10 {
        texts.push(vec![Box::leak(format!("{}", 29 - i).into_boxed_str()), "50"]);
    }
    d.texts = texts.into();
    let say = Say::new(Some(0));
    let mut log = vec![];
    // 用户指定第 1 个数（下标 0 = 步数，每步都降）
    let o = Options { max_steps: 10, dry_run: false, advisor: Some(&say), ask_always: false, ask_budget: 30, goal: "", goal_index: Some(0) };
    let _ = play(&mut d, &mut Clk(0), &profile(3, 4), &o, &mut |v| log.push(v));
    assert!(log.iter().all(|l| l.get("goal_found").is_none()), "用户给了，就不自动认");
    assert!(log.iter().all(|l| l["progress"]["goal_index"] == 0 || l["progress"]["goal_index"].is_null()));
}
```

（`run_ask` 的 `Options` 里 `goal_index` 现在写死 `None`——保持。假读盘的条数按 `a_move_that_works…`/第四轮已有停滞测试的做法调整，断言不变。）

`text.rs`：

```rust
#[test]
fn step_line_says_which_number_was_taken_as_the_goal() {
    let rec = serde_json::json!({"candidates":[{"a":[0,0],"b":[0,1],"features":{"cleared":3}}],"chosen":0,"outcome":"moved","goal_found":2});
    assert!(step_line(1, &rec).contains("我认为屏幕上第 2 个数字是这一关的目标"));
    let rec2 = serde_json::json!({"candidates":[{"a":[0,0],"b":[0,1],"features":{"cleared":3}}],"chosen":0,"outcome":"moved"});
    assert!(!step_line(1, &rec2).contains("目标"));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-game goal:: && cargo +1.99.0 test --lib game::text`
Expected: 编译失败（`GoalFinder` 未定义）。

- [ ] **Step 3: 写实现**

`goal.rs`：

```rust
//! 不看数字的意思，只看数字怎么变，认出哪个是「目标」。
//! 步数每成功一步刚好降 1；分数、连击只增不减；剩下的就是目标。
//! 恰好剩一个才认；多于一个或没有就不认（宁可不判停滞，也不乱判）。

pub const MIN_SAMPLES: usize = 4;

#[derive(Default)]
pub struct GoalFinder {
    samples: usize,
    all_minus_one: Vec<bool>,
    ever_increased: Vec<bool>,
    found: Option<usize>,
}

impl GoalFinder {
    pub fn new() -> GoalFinder {
        GoalFinder::default()
    }

    /// 只在**成功走了一步**之后调用。前后个数不同的步不进样本（OCR 漏读、弹窗飘字）。
    pub fn observe(&mut self, before: &[u64], after: &[u64]) {
        if self.found.is_some() || before.len() != after.len() || before.is_empty() {
            return;
        }
        if self.samples == 0 {
            self.all_minus_one = vec![true; before.len()];
            self.ever_increased = vec![false; before.len()];
        }
        if self.all_minus_one.len() != before.len() {
            return; // 列表个数和已有样本不一致：这一步不进样本
        }
        self.samples += 1;
        for (i, (b, a)) in before.iter().zip(after).enumerate() {
            if *a != b.saturating_sub(1) || *b == 0 {
                self.all_minus_one[i] = false;
            }
            if a > b {
                self.ever_increased[i] = true;
            }
        }
        if self.samples >= MIN_SAMPLES {
            let left: Vec<usize> = (0..self.all_minus_one.len()).filter(|&i| !self.all_minus_one[i] && !self.ever_increased[i]).collect();
            if left.len() == 1 {
                self.found = Some(left[0]);
            }
        }
    }

    pub fn index(&self) -> Option<usize> {
        self.found
    }
}
```

注意：`self.all_minus_one.len() != before.len()` 这一支让「中途个数永久变化」的步不进样本，但识别器仍用开头样本的下标；这是已知的限制（见 spec）。

`play.rs`：

1. `use crate::goal::GoalFinder;`；循环前 `let mut finder = GoalFinder::new();`。
2. 新增小函数，把三处重复的更新收成一个：

```rust
/// 停滞计数的一次更新；`gi` 是当前生效的目标下标（用户给的优先，否则识别器认出的），没有就不动。
fn update_stall(no_progress: &mut usize, gi: Option<usize>, before: &Option<Vec<u64>>, after: &Option<Vec<u64>>) {
    if let (Some(gi), Some(b), Some(a)) = (gi, before, after) {
        match goal_dropped(b, a, gi) {
            Some(true) => *no_progress = 0,
            Some(false) => *no_progress += 1,
            None => {}
        }
    }
}
```

3. 三个结果分支里原来的 `if let (Some(gi), Some(b), Some(a)) = (o.goal_index, ...) { ... }` 都换成：

```rust
let gi = o.goal_index.or(finder.index());
update_stall(&mut no_progress, gi, &progress_prev, &progress_after);
rec["progress"] = json!({ "before": progress_prev, "after": progress_after, "goal_index": gi });
```

4. 只在「成功走了一步」的 `Settle::Settled(next)`（`moved`）分支里、写记录**之前**，让识别器观察，并在刚认出时打标记：

```rust
let had = finder.index();
if let (Some(b), Some(a)) = (&progress_prev, &progress_after) {
    finder.observe(b, a);
}
if o.goal_index.is_none() && had.is_none() {
    if let Some(i) = finder.index() {
        rec["goal_found"] = json!(i + 1);
    }
}
```

（顺序：先 `observe` → 再算 `gi`（因此刚认出的这一步也用新认出的下标，计数从这一步算起）→ 再 `update_stall`。被拒绝的两个分支**不**调用 `observe`——被拒绝的步数不降，会污染「刚好 −1」的判断。）

5. `stalled_now`/触发条件里的 `no_progress >= STALL_STEPS` 不变；第四轮的 `o.goal_index.is_none()` 之类的早退如果有，改成看 `gi.is_none()`（`gi` 在循环顶部算：`let gi = o.goal_index.or(finder.index());`，供「停滞才问模型」那块用——只有 `gi.is_some()` 才可能 `stalled`）。

`text.rs` 的 `step_line`：在 `stalled` 句子之前加

```rust
    if let Some(n) = rec["goal_found"].as_u64() {
        s += &format!("；我认为屏幕上第 {n} 个数字是这一关的目标");
    }
```

`skill.md`：把 `--goal-number` 那段改成：不给时 dct 会在走几步以后自己认（认的是变化不规律、又不是每步刚好少 1 的那个数字），认出来会说一句；认错了再用 `--goal-number` 指明。大白话。

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked` 与 clippy。
Expected: 全绿，无警告。第四轮已有的停滞测试（用 `goal_index: Some(..)`）不应受影响；如有因为 `finder` 在 `Some` 时仍然 `observe` 而改变行为的，保持「用户给了就不自动认」：`observe` 只在 `o.goal_index.is_none()` 时调用。

- [ ] **Step 5: 变异检查，然后提交**

分别临时改：`MIN_SAMPLES` 改成 2 → `fewer_than_four…` 变红；去掉 `all_minus_one` 的排除 → `the_step_counter_and_a_rising_score…` 变红；`left.len() == 1` 改成 `>= 1` → `two_irregular…` 变红；去掉 `found` 的锁定（`if self.found.is_some()`）→ `once_found…` 变红；把被拒绝分支里也调用 `observe` → 需要一个测试：被拒绝的步（数字不降）夹在中间，不应让步数被排除。若现有测试抓不到这一条，**加一个**并确认它变红。

```bash
git add crates/dct-game src/game
git commit -m "feat(game): without --goal-number, pick the goal number by how the on-screen numbers change (step counter drops by exactly one, score only rises)"
```

---

## Self-Review

1. **Spec 覆盖：** 一（token）→ Task 1（用量读取）+ Task 2（Advice/记录/结束语）；二（自动认目标）→ Task 3；「不做」里的各项都没有出现在任务里。验收（真机）不在计划里，留给最后。
2. **占位扫描：** Task 1 的 `Credential` 构造、Task 3 的假读盘条数，已写明按现有测试的写法调整、断言不变。没有 TBD。
3. **类型一致：** `Usage { input, output }`（Task 1）→ `Advice.tokens: Option<(u64, u64)>`（Task 2，在 `LlmAdvisor` 里转换）；`GoalFinder::{new, observe, index}`、`MIN_SAMPLES`（Task 3）；`update_stall`、`goal_dropped` 沿用第四轮；记录字段 `tokens_in/out`、`progress.goal_index`、`goal_found` 在测试和实现里名字一致。
4. **Review Focus：** 五条分别落在 Task 1（usage 畸形）、Task 2（混合计数）、Task 3（个数变化、用户越界、锁定不反复）。
