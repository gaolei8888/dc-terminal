### Task 2: `play()` 里按触发条件问，合规检查，记录

**Files:**
- Modify: `crates/dct-game/src/play.rs`（`Options` 加字段；选步处接入）
- Modify: `crates/dct-game/src/navigate.rs`（`NavOptions` 加同样的字段，传给 `play`）
- Modify: 所有构造 `Options`/`NavOptions` 的地方：`src/game/cli.rs:128-130`，`play_tests.rs:72,193`，`navigate.rs:151,369,517,701`
- Test: `crates/dct-game/src/play_tests.rs`

**Interfaces:**
- Consumes: Task 1 的 `Advisor`、`AskInput`、`Advice`、`board_text`、`describe`、`describe_move`、`parse_reply` 不用（`Advice.choice` 已经解析好）。
- Produces:
  - `pub struct Options<'a> { pub max_steps: usize, pub dry_run: bool, pub advisor: Option<&'a dyn Advisor>, pub ask_always: bool, pub ask_budget: usize, pub goal: &'a str }`
  - `NavOptions<'a>` 同样加这四个字段。
  - 记录新增字段：`decider`（`"rules"`/`"model"`，每条带候选的记录都有），`ask`（只有问过才有）：`{ "model": …, "raw": …, "reason": …, "asked": [候选下标…], "choice": 候选下标或 null }`。`chosen` 仍是 `candidates` 里的下标。
  - 常量 `pub const ASK_SHOWN: usize = 8;`（最多发几个候选）。

触发规则（写死在 `play()` 里）：`advisor.is_some() && !dry_run && 本次运行已问次数 < ask_budget && 可选候选数 >= 2 && (ask_always || 这盘棋上已有失败的步)`。可选候选 = `cands` 去掉 `failed` 里的步，取前 `ASK_SHOWN` 个。

- [ ] **Step 1: 写失败的测试**

在 `play_tests.rs` 末尾（`Fake`、`Clk`、`profile`、`grid`、`A`、`MOVED` 已在文件里）：

```rust
use crate::ask::{Advice, Advisor, AskInput};
use std::cell::{Cell, RefCell};

struct Say {
    choice: Option<usize>,
    calls: Cell<usize>,
    seen: RefCell<Vec<AskInput>>,
}
impl Say {
    fn new(choice: Option<usize>) -> Say {
        Say { choice, calls: Cell::new(0), seen: RefCell::new(vec![]) }
    }
}
impl Advisor for Say {
    fn pick(&self, i: &AskInput) -> Option<Advice> {
        self.calls.set(self.calls.get() + 1);
        self.seen.borrow_mut().push(AskInput {
            board: i.board.clone(),
            goal: i.goal.clone(),
            candidates: i.candidates.clone(),
            failed: i.failed.clone(),
        });
        Some(Advice { choice: self.choice, reason: "测试".into(), raw: "x".into(), model: "fake".into() })
    }
}
struct Down;
impl Advisor for Down {
    fn pick(&self, _: &AskInput) -> Option<Advice> {
        None
    }
}

fn run_ask(d: &mut Fake, adv: Option<&dyn Advisor>, always: bool, budget: usize, steps: usize) -> (Summary, Vec<Value>) {
    let mut log = vec![];
    let o = Options { max_steps: steps, dry_run: false, advisor: adv, ask_always: always, ask_budget: budget, goal: "清冰" };
    let s = play(d, &mut Clk(0), &profile(3, 4), &o, &mut |v| log.push(v));
    (s, log)
}
fn settled_script() -> Vec<Result<GridRead, DcoError>> {
    vec![Ok(grid(A)), Ok(grid(MOVED)), Ok(grid(MOVED))]
}

#[test]
fn ask_always_lets_the_model_pick_among_candidates_and_records_it() {
    // 先看规则自己会划哪一步
    let mut d0 = Fake::new(settled_script());
    let _ = run_ask(&mut d0, None, false, 30, 1);
    let rules_swipe = d0.swipes[0];

    let say = Say::new(Some(1)); // 选第 2 个候选
    let mut d = Fake::new(settled_script());
    let (_, log) = run_ask(&mut d, Some(&say), true, 30, 1);
    assert_eq!(say.calls.get(), 1);
    assert_ne!(d.swipes[0], rules_swipe, "模型选了第 2 个候选，划的应该和规则第一名不同");
    assert_eq!(log[0]["decider"], "model");
    assert_eq!(log[0]["chosen"], 1);
    assert_eq!(log[0]["ask"]["choice"], 1);
    assert_eq!(log[0]["ask"]["model"], "fake");
    // 发出去的提示里有目标、字母棋盘、编号候选
    let seen = say.seen.borrow();
    assert_eq!(seen[0].goal, "清冰");
    assert!(seen[0].board.contains('A'));
    assert!(seen[0].candidates.len() >= 2);
}

#[test]
fn without_ask_always_the_model_is_not_asked_on_a_clean_board() {
    let say = Say::new(Some(1));
    let mut d = Fake::new(settled_script());
    let (_, log) = run_ask(&mut d, Some(&say), false, 30, 1);
    assert_eq!(say.calls.get(), 0);
    assert_eq!(log[0]["decider"], "rules");
    assert!(log[0].get("ask").is_none());
}

#[test]
fn a_failed_swap_triggers_the_model_and_is_listed_but_not_offered() {
    // 第一划没反应（A 一直不变），第二次选步时这盘棋上已有失败的步：问模型，失败的步在 failed 里、不在候选里
    let mut reads = vec![];
    for _ in 0..40 {
        reads.push(Ok(grid(A)));
    }
    reads.push(Ok(grid(MOVED)));
    reads.push(Ok(grid(MOVED)));
    let say = Say::new(Some(0));
    let mut d = Fake::new(reads);
    let (_, log) = run_ask(&mut d, Some(&say), false, 30, 2);
    assert!(say.calls.get() >= 1);
    let seen = say.seen.borrow();
    assert_eq!(seen[0].failed.len(), 1);
    assert!(!seen[0].candidates.iter().any(|c| *c == seen[0].failed[0]));
    assert_eq!(log[0]["decider"], "rules");
    assert_eq!(log[1]["decider"], "model");
}

#[test]
fn model_that_cannot_decide_or_is_down_falls_back_to_rules() {
    let mut d0 = Fake::new(settled_script());
    let _ = run_ask(&mut d0, None, false, 30, 1);
    let rules_swipe = d0.swipes[0];

    for adv in [&Say::new(None) as &dyn Advisor, &Down] {
        let mut d = Fake::new(settled_script());
        let (s, log) = run_ask(&mut d, Some(adv), true, 30, 1);
        assert_eq!(d.swipes[0], rules_swipe);
        assert_eq!(log[0]["decider"], "rules");
        assert_eq!(s.stop, Stop::StepsDone);
    }
}

#[test]
fn ask_budget_stops_the_questions_but_not_the_game() {
    let say = Say::new(Some(1));
    let mut reads = vec![Ok(grid(A))];
    for _ in 0..6 {
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(MOVED)));
        reads.push(Ok(grid(A)));
        reads.push(Ok(grid(A)));
    }
    let mut d = Fake::new(reads);
    let (_, log) = run_ask(&mut d, Some(&say), true, 2, 3);
    assert_eq!(say.calls.get(), 2);
    assert_eq!(log.len(), 3);
    assert_eq!(log[2]["decider"], "rules");
}

#[test]
fn a_single_candidate_is_never_worth_asking_about() {
    // 只有一步可走的棋盘：不问
    let one: &[&[u16]] = &[&[1, 1, 2, 3], &[2, 3, 1, 1], &[2, 3, 1, 3]];
    let _ = one; // 具体盘面在实现时用只有一个合法交换的真盘面替换；断言是 calls == 0
    // 见 Step 3 的说明
}
```

最后一个测试要有真实内容：在 `play_tests.rs` 里找到（或构造）一个只有一个合法交换的小盘面（`choose()` 返回长度 1），断言 `say.calls.get() == 0` 且 `log[0]["decider"] == "rules"`。把上面那个占位测试**替换**成这个真测试后再继续；占位不许留下。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo +1.99.0 test -p dct-game play_tests::`
Expected: 编译失败（`Options` 没有 `advisor` 等字段）。

- [ ] **Step 3: 写实现**

`play.rs`：

```rust
use crate::ask::{board_text, describe, describe_move, Advisor, AskInput};

/// 最多发给模型几个候选。
pub const ASK_SHOWN: usize = 8;

pub struct Options<'a> {
    pub max_steps: usize,
    pub dry_run: bool,
    /// 规则没把握时问的那个。`None` = 不问（和以前一样）。
    pub advisor: Option<&'a dyn Advisor>,
    /// 每一步都问（`--ask-model`）；否则只在这盘棋上已有失败的步时才问。
    pub ask_always: bool,
    /// 这一次 `play()` 最多问几次。
    pub ask_budget: usize,
    pub goal: &'a str,
}
```

`play()` 里：在现有 `let mut failed ...` 旁加 `let mut asked = 0usize;`。选步处（`let pick = cands.iter().position(...)` 之后、`let Some(pick) = pick else {...}` 之前不动；在 `let chosen = &cands[pick];` 之前）改成：

```rust
        let board_key = canonical(&g.cells);
        let is_failed = |c: &Candidate| failed.iter().any(|(m, cells)| *m == c.mv && *cells == board_key);
        // 发给模型的候选：没失败过的，最多 ASK_SHOWN 个；下标指回 cands
        let offered: Vec<usize> = (0..cands.len()).filter(|&i| !is_failed(&cands[i])).take(ASK_SHOWN).collect();
        let had_failed_here = failed.iter().any(|(_, cells)| *cells == board_key);
        let mut pick = pick;
        let mut ask_rec: Option<Value> = None;
        let mut decider = "rules";
        if let Some(adv) = o.advisor {
            if !o.dry_run && asked < o.ask_budget && offered.len() >= 2 && (o.ask_always || had_failed_here) && pick.is_some() {
                asked += 1;
                let fixed = fixed_ids(&g, &p.fixed_rgb, p.match_de);
                let input = AskInput {
                    board: board_text(&g, &fixed),
                    goal: o.goal.to_string(),
                    candidates: offered.iter().map(|&i| describe(&cands[i])).collect(),
                    failed: failed
                        .iter()
                        .filter(|(_, cells)| *cells == board_key)
                        .map(|(m, _)| describe_move(m))
                        .collect(),
                };
                if let Some(a) = adv.pick(&input) {
                    let chosen_idx = a.choice.filter(|&k| k < offered.len()).map(|k| offered[k]);
                    ask_rec = Some(json!({
                        "model": a.model, "raw": a.raw, "reason": a.reason,
                        "asked": offered, "choice": chosen_idx,
                    }));
                    if let Some(i) = chosen_idx {
                        pick = Some(i);
                        decider = "model";
                    }
                }
            }
        }
```

并把原来的 `let pick = cands.iter().position(...)` 保留作为规则的选择（上面 `let mut pick = pick;` 接它）。把 `let is_failed` 闭包借用 `failed` 的位置安排在 `failed.push` 之前（后面是同一个循环迭代里的不同语句，Rust 借用在闭包最后一次使用后结束，不冲突；若编译器报借用冲突，把 `offered` 的计算放进一个小块 `{ ... }` 里）。

记录里加：`"decider": decider`（`rec` 的 json! 里），以及 `if let Some(a) = ask_rec { rec["ask"] = a; }`。`chosen` 字段用最终的 `pick`。

`navigate.rs`：`NavOptions` 加 `pub advisor: Option<&'a dyn Advisor>, pub ask_always: bool, pub ask_budget: usize, pub goal: &'a str`（`NavOptions<'a>`），`navigate.rs:151` 构造 `Options` 时带上这四个字段。其余构造点（测试、`cli.rs`）补 `advisor: None, ask_always: false, ask_budget: 0, goal: ""`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo +1.99.0 test --workspace --locked`
Expected: 全绿（含旧测试：旧构造点只补了默认字段，行为不变）。

- [ ] **Step 5: 变异检查，然后提交**

分别临时改：把触发条件里 `offered.len() >= 2` 改成 `>= 1`；把 `asked < o.ask_budget` 改成 `true`；把 `chosen_idx` 的 `filter(|&k| k < offered.len())` 去掉（会越界 panic 或选错）；每一个都应该让至少一个测试变红。改回。

```bash
git add crates/dct-game/src src/game/cli.rs
git commit -m "feat(game): ask an advisor to pick among rule candidates when a swap was refused or on request; every failure falls back to rules"
```

---

