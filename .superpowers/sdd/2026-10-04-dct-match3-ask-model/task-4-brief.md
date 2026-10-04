### Task 4: 离线回放：用真实局面测模型（验收第 1 条）

**Files:**
- Create: `src/game/bench.rs`
- Modify: `src/game/mod.rs`、`src/game/cli.rs`（子命令 `dct game ask-bench <记录文件> [--limit N]`）
- Test: `src/game/bench.rs`

**Interfaces:**
- Consumes: Task 3 的 `load_llm_backend`、`LlmAdvisor`；Task 1 的 `describe`、`AskInput`；记录文件（`~/.dct/games/log/*.jsonl`）里每条带 `candidates`（含 `a`、`b`、`features`）、`cells`、`classes`、`game`、`chosen`、`outcome` 的行。
- Produces: `pub fn replay(lines: &[Value], adv: &dyn Advisor, goal_of: &dyn Fn(&str) -> String, limit: usize) -> Report`；`pub struct Report { pub asked: usize, pub answered: usize, pub valid: usize, pub picked_rules_first: usize, pub chance_first: f64, pub avg_ms: u64 }`。

做法：从记录里取 `candidates` 至少 3 个、`dry_run` 不是 true 的行，每行把候选前 5 个打乱顺序（固定种子，用一个简单的线性同余，别引入新依赖），拼成 `AskInput` 问一次，统计「回答成合法编号的次数」「选中规则第一名的次数」「随机期望次数 = Σ 1/候选数」「平均耗时」。棋盘字母图用记录里的 `cells` 和 `classes`（`fixed` 取空：记录里没存哪些是不是糖，回放只比较选择，不依赖它；打印报告时明说）。

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use dct_game::ask::{Advice, Advisor, AskInput};
    use serde_json::json;

    struct First;
    impl Advisor for First {
        fn pick(&self, _: &AskInput) -> Option<Advice> {
            Some(Advice { choice: Some(0), reason: String::new(), raw: "1".into(), model: "t".into() })
        }
    }

    fn rec(n: usize, dry: bool) -> Value {
        let cands: Vec<Value> = (0..n).map(|k| json!({"a":[0,k],"b":[1,k],"score":1.0,
            "features":{"cleared":3,"cascade":0,"striped":0,"wrapped":0,"bomb":0,"triggered":0,"special_swap":false,"lowest_row":1}})).collect();
        json!({"game":"g","cells":[[1,1],[2,2]],"classes":[{"id":1,"count":2},{"id":2,"count":2}],
               "candidates": cands, "chosen":0, "dry_run": dry, "outcome": "moved"})
    }

    #[test]
    fn replay_skips_dry_runs_and_tiny_candidate_lists_and_counts_the_rest() {
        let lines = vec![rec(5, false), rec(5, true), rec(2, false), rec(4, false)];
        let r = replay(&lines, &First, &|_| "尽量多消".into(), 10);
        assert_eq!(r.asked, 2);
        assert_eq!(r.answered, 2);
        assert_eq!(r.valid, 2);
        // 候选是打乱的，模型总选第一个，所以「选中规则第一名」只可能是碰巧
        assert!(r.picked_rules_first <= 2);
        // 随机期望：5 个候选 1/5，4 个候选 1/4（候选最多取前 5 个）
        assert!((r.chance_first - (0.2 + 0.25)).abs() < 1e-9);
    }

    #[test]
    fn replay_respects_the_limit() {
        let lines: Vec<Value> = (0..10).map(|_| rec(5, false)).collect();
        assert_eq!(replay(&lines, &First, &|_| String::new(), 3).asked, 3);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test --lib game::bench`
Expected: 编译失败。

- [ ] **Step 3: 写实现**

```rust
//! 离线回放：拿以前真机记下的局面问模型，看它选得怎么样。不碰游戏、不碰手机。
use dct_game::ask::{board_text, describe, Advisor, AskInput};
use dct_game::board::GridRead;
use dct_game::choose::Candidate;
use dct_game::sim::{Move, Outcome};
use serde_json::Value;

#[derive(Debug, Default)]
pub struct Report {
    pub asked: usize,
    pub answered: usize,
    pub valid: usize,
    pub picked_rules_first: usize,
    pub chance_first: f64,
    pub avg_ms: u64,
}

fn candidate(v: &Value) -> Option<Candidate> {
    let p = |x: &Value| Some((x[0].as_u64()? as usize, x[1].as_u64()? as usize));
    let f = &v["features"];
    let n = |k: &str| f[k].as_u64().unwrap_or(0) as usize;
    Some(Candidate {
        mv: Move { a: p(&v["a"])?, b: p(&v["b"])? },
        score: v["score"].as_f64().unwrap_or(0.0),
        features: Outcome {
            cleared: n("cleared"), cascade: n("cascade"), striped: n("striped"), wrapped: n("wrapped"),
            bomb: n("bomb"), triggered: n("triggered"),
            special_swap: f["special_swap"].as_bool().unwrap_or(false), lowest_row: n("lowest_row"),
        },
    })
}

pub fn replay(lines: &[Value], adv: &dyn Advisor, goal_of: &dyn Fn(&str) -> String, limit: usize) -> Report {
    let mut r = Report::default();
    let mut seed: u64 = 7;
    let mut total_ms = 0u128;
    for rec in lines {
        if r.asked >= limit {
            break;
        }
        if rec["dry_run"].as_bool() == Some(true) {
            continue;
        }
        let cands: Vec<Candidate> = rec["candidates"].as_array().map(|a| a.iter().filter_map(candidate).take(5).collect()).unwrap_or_default();
        if cands.len() < 3 {
            continue;
        }
        let Ok(g) = serde_json::from_value::<GridRead>(serde_json::json!({
            "rows": rec["cells"].as_array().map_or(0, Vec::len),
            "cols": rec["cells"][0].as_array().map_or(0, Vec::len),
            "cells": rec["cells"], "odd": rec["odd"], "classes": rec["classes"],
        })) else {
            continue;
        };
        // 固定种子的洗牌：同一份记录每次回放顺序一样，结果可比
        let mut order: Vec<usize> = (0..cands.len()).collect();
        for i in (1..order.len()).rev() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            order.swap(i, (seed >> 33) as usize % (i + 1));
        }
        let input = AskInput {
            board: board_text(&g, &[]),
            goal: goal_of(rec["game"].as_str().unwrap_or("")),
            candidates: order.iter().map(|&k| describe(&cands[k])).collect(),
            failed: vec![],
        };
        r.asked += 1;
        r.chance_first += 1.0 / cands.len() as f64;
        let t = std::time::Instant::now();
        let a = adv.pick(&input);
        total_ms += t.elapsed().as_millis();
        if let Some(a) = a {
            r.answered += 1;
            if let Some(k) = a.choice {
                r.valid += 1;
                if order[k] == 0 {
                    r.picked_rules_first += 1;
                }
            }
        }
    }
    if r.asked > 0 {
        r.avg_ms = (total_ms / r.asked as u128) as u64;
    }
    r
}
```

`cli.rs` 里 `dct game ask-bench <文件...> [--limit 40]`：读文件的每一行 JSON，调 `load_llm_backend()`（没开就打「没开大模型，没法测」退出 1），建 `LlmAdvisor`，`goal_of` 用 `|_| "尽量多消".into()`（回放不知道每关的目标；报告里写明），打印大白话报告：

```
问了 N 个局面，回答成合法编号的 V 个，选中规则第一名的 K 个（乱选大约 C 个），平均每次 M 毫秒。
说明：回放里没有「哪些格子不是糖」和「这一关的目标」，所以这个数只说明它比乱选强多少，不说明它选得对不对。
```

- [ ] **Step 4: 跑测试**

Run: `cargo +1.99.0 test --workspace --locked` 与 clippy。
Expected: 全绿、无警告。

- [ ] **Step 5: 提交；然后做验收第 1 条**

```bash
git add src
git commit -m "feat(game): ask-bench replays recorded boards against the configured model"
```

验收（需要网关密钥，已在 `~/.dct/secrets.toml` 的 `qwen`，**不要打印**；需要 `dct llm check` 先通）：

```bash
dct game ask-bench ~/.dct/games/log/2026-10-03.jsonl ~/.dct/games/log/2026-10-04.jsonl --limit 40
```

把结果和本机小模型 Qwen3-VL-4B（40 个局面里选中规则第一名 13 次，乱选约 8 次，平均约 1 秒）并排写进 spec 的「实测依据」。这是写给 spec 的数字，别写成「模型选得好」——这个回放不知道目标。

---

## Self-Review

1. **Spec 覆盖：** 触发（没反应后 / `--ask-model`）→ Task 2；提示只发文字、失败清单、目标来源 → Task 1、3（目标来源：命令行 `--goal`；dcv 那一档等 dc-vault 格式，spec 已说明先不做）；合规检查、上限、超时 → Task 2（上限）、3（超时 20 秒）；记录字段 → Task 2；dcv 学习 → spec 明确「等格式」，本计划不做；离线对比 → Task 4。「没步可走」触发已按开头的修正去掉。
2. **占位扫描：** Task 2 里「只有一个候选」那个测试，Step 1 已写明占位必须替换成真盘面再继续（实现者要在 `play_tests.rs` 里找一个 `choose()` 返回 1 个候选的盘面；若没有现成的就用 `Fake` + 一个 2×3 小盘面构造）。其余步骤都有完整代码。
3. **类型一致：** `AskInput`/`Advice`/`Advisor` 在 Task 1 定义，Task 2/3/4 的用法一致（`Advice.choice: Option<usize>` 0 起；`Options.advisor: Option<&dyn Advisor>`）。`Outcome` 需 `Default`（Task 1 Step 1 已写）。
4. **Review Focus：** 五条分别落在 Task 1（解析）、Task 2（单候选、预算、失败步）、Task 3（没配 `[llm]`）。
