### Task 3: 接上网关大模型和命令行

**Files:**
- Create: `src/game/advisor.rs`
- Modify: `src/game/mod.rs`（`mod advisor;`）
- Modify: `src/game/cli.rs`（`--ask-model`、`--goal`、建 advisor、传给 Options/NavOptions；`USAGE` 加这两项）
- Modify: `src/cli.rs`（把 `llm_check` 里「读配置 + 建 backend」那一段抽成 `pub fn load_llm_backend() -> Result<Arc<dyn Backend>, LoadLlmError>`，`llm_check` 改用它，行为不变）
- Modify: `src/game/skill.md`（说明卡里加一小节）
- Modify: `docs/superpowers/specs/2026-10-04-dct-match3-ask-model-design.md`（按本计划开头的修正删掉「没步可走」触发）
- Test: `src/game/cli.rs` 的 `parse` 测试；`src/game/advisor.rs` 的单元测试（假 `Backend`）

**Interfaces:**
- Consumes: Task 1 的 `Advisor`/`AskInput`/`Advice`/`prompt_text`/`parse_reply`；`llm::{Backend, Prompt, complete_with_timeout, LlmError}`。
- Produces:
  - `pub struct LlmAdvisor { backend: Arc<dyn Backend>, model: String, timeout: Duration }`，`LlmAdvisor::new(backend, model)`（超时默认 20 秒），`impl Advisor for LlmAdvisor`。
  - `Args` 新增 `pub ask_model: bool`、`pub goal: Option<String>`。

- [ ] **Step 1: 先改 spec**

在 spec 的「1. 什么时候问模型（触发）」里删掉「没步可走」那条，并在末尾加一句：「规则一步都选不出时没有候选，模型无从选择，不属于触发；这类关卡靠配置数据让规则列得出候选（例如不把彩球标成不是糖），再用 `--ask-model` 选。」

- [ ] **Step 2: 写失败的测试**

`src/game/advisor.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Backend, LlmError, Prompt};
    use dct_game::ask::{Advisor, AskInput};
    use std::sync::{Arc, Mutex};

    struct Fixed(Result<String, LlmError>, Mutex<Vec<String>>);
    impl Backend for Fixed {
        fn complete(&self, p: &Prompt) -> Result<String, LlmError> {
            self.1.lock().unwrap().push(p.user.clone());
            self.0.clone()
        }
    }
    fn input() -> AskInput {
        AskInput { board: "A A\nB B".into(), goal: "清冰".into(), candidates: vec!["甲".into(), "乙".into(), "丙".into()], failed: vec![] }
    }
    fn adv(r: Result<String, LlmError>) -> (LlmAdvisor, Arc<Fixed>) {
        let b = Arc::new(Fixed(r, Mutex::new(vec![])));
        (LlmAdvisor::new(b.clone(), "m1".into()), b)
    }

    #[test]
    fn a_valid_reply_becomes_a_zero_based_choice_with_reason() {
        let (a, b) = adv(Ok("2，能清冰".into()));
        let r = a.pick(&input()).unwrap();
        assert_eq!(r.choice, Some(1));
        assert_eq!(r.reason, "能清冰");
        assert_eq!(r.raw, "2，能清冰");
        assert_eq!(r.model, "m1");
        assert!(b.1.lock().unwrap()[0].contains("1) 甲"));
    }

    #[test]
    fn garbage_reply_is_an_answer_with_no_choice() {
        let (a, _) = adv(Ok("随便".into()));
        assert_eq!(a.pick(&input()).unwrap().choice, None);
    }

    #[test]
    fn every_backend_error_means_not_asked() {
        for e in [LlmError::Unavailable, LlmError::Timeout, LlmError::Malformed] {
            let (a, _) = adv(Err(e));
            assert!(a.pick(&input()).is_none(), "{e:?}");
        }
    }
}
```

`src/game/cli.rs` 的测试里加：

```rust
#[test]
fn parse_ask_model_and_goal() {
    let a = parse(&["play".into(), "--ask-model".into(), "--goal".into(), "清掉冰块".into()]).unwrap();
    assert!(a.ask_model);
    assert_eq!(a.goal.as_deref(), Some("清掉冰块"));
    let d = parse(&["play".into()]).unwrap();
    assert!(!d.ask_model);
    assert_eq!(d.goal, None);
    assert!(parse(&["play".into(), "--goal".into()]).is_err());
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo +1.99.0 test --lib game::`
Expected: 编译失败。

- [ ] **Step 4: 写实现**

`src/game/advisor.rs`：

```rust
//! 把「问模型选哪一步」接到 dct 自己的 LLM 连接层上。
//! 凭据怎么取、发给哪台机器，全在 `llm::resolve`，这里不碰。
use crate::llm::{complete_with_timeout, Backend, Prompt};
use dct_game::ask::{parse_reply, prompt_text, Advice, Advisor, AskInput};
use std::sync::Arc;
use std::time::Duration;

pub struct LlmAdvisor {
    backend: Arc<dyn Backend>,
    model: String,
    timeout: Duration,
}

impl LlmAdvisor {
    pub fn new(backend: Arc<dyn Backend>, model: String) -> LlmAdvisor {
        LlmAdvisor { backend, model, timeout: Duration::from_secs(20) }
    }
}

const SYSTEM: &str = "你在帮一个三消游戏的自动玩家选下一步。只能从给出的编号里选，不要自己编走法。";

impl Advisor for LlmAdvisor {
    fn pick(&self, i: &AskInput) -> Option<Advice> {
        let p = Prompt { system: SYSTEM.into(), user: prompt_text(i), max_tokens: 64 };
        // 任何错误都是「没问成」：调用方退回规则
        let raw = complete_with_timeout(self.backend.clone(), p, self.timeout).ok()?;
        let (choice, reason) = parse_reply(&raw, i.candidates.len());
        Some(Advice { choice, reason, raw, model: self.model.clone() })
    }
}
```

`src/cli.rs`：新增

```rust
pub struct LoadedLlm {
    pub backend: std::sync::Arc<dyn crate::llm::Backend>,
    pub model: String,
}

pub enum LoadLlmError {
    /// 没写 `[llm]`：正常状态。
    NotEnabled,
    Problem(crate::llm::resolve::ResolveError),
}

pub fn load_llm_backend() -> Result<LoadedLlm, LoadLlmError> {
    // 原样搬 llm_check 里从 `let socket = ...` 到 resolve 成功的那一段；
    // model 取 llm.model.clone().unwrap_or_else(|| llm.provider.clone())
}
```

`llm_check` 改成调用 `load_llm_backend()`，输出文字不变（`NotEnabled` 打原来的「还没开」，`Problem(e)` 打原来的「连不上」）。**搬运时一个字符的输出都不许变**，现有 `llm check` 的测试必须继续通过。

`src/game/cli.rs`：`Args` 加 `ask_model: bool`（默认 false）、`goal: Option<String>`；`parse` 里加

```rust
            "--ask-model" => a.ask_model = true,
            "--goal" => a.goal = Some(it.next().ok_or("--goal 后面要写目标，比如 --goal \"清掉冰块\"")?.clone()),
```

`USAGE` 末尾加 ` [--ask-model] [--goal "清掉冰块"]`。`run_parsed` 里，在 `let clock = SystemClock;` 之后：

```rust
    let advisor: Option<super::advisor::LlmAdvisor> = if a.ask_model {
        match crate::cli::load_llm_backend() {
            Ok(l) => Some(super::advisor::LlmAdvisor::new(l.backend, l.model)),
            Err(crate::cli::LoadLlmError::NotEnabled) => {
                println!("没开大模型，这次只用规则玩。");
                None
            }
            Err(crate::cli::LoadLlmError::Problem(_)) => {
                println!("大模型连不上，这次只用规则玩。可以先运行 dct llm check 看原因。");
                None
            }
        }
    } else {
        None
    };
    let goal = a.goal.clone().unwrap_or_else(|| "尽量多消".into());
    let adv_ref: Option<&dyn dct_game::ask::Advisor> = advisor.as_ref().map(|x| x as &dyn dct_game::ask::Advisor);
```

并把 `Options`/`NavOptions` 构造改成 `advisor: adv_ref, ask_always: a.ask_model, ask_budget: 30, goal: &goal`。`text.rs` 的 `step_line`：当记录里 `decider == "model"` 时在句尾加「（大模型选的）」；这是 `text.rs` 里 `nav`/`step` 现有测试风格的一个小测试：`step_line` 对带 `"decider":"model"` 的记录包含「大模型选的」，对 `"rules"` 不包含。

`src/game/skill.md` 加一小节「玩不动的时候」：一两句，写 `--ask-model` 和 `--goal` 是什么、没配大模型也能玩。全中文大白话，不出现英文术语。

- [ ] **Step 5: 跑全部测试和 clippy，提交**

Run: `cargo +1.99.0 test --workspace --locked` 和 `cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings`
Expected: 全绿、无警告。

手工冒烟（不需要游戏）：`dct game play --ask-model --dry-run` 在没配 `[llm]` 的环境里，应打印「没开大模型，这次只用规则玩。」后照常（读不到棋盘就按原来的方式停），退出码和不带 `--ask-model` 时一样。

```bash
git add src crates docs
git commit -m "feat(game): --ask-model and --goal; advisor over the gateway model with rules fallback"
```

---

