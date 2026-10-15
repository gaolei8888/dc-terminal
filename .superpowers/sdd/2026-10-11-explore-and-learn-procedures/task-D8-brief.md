# Brief for Task D8 (extracted from /Users/lei/Documents/work/dc/dc-octo/docs/superpowers/plans/2026-10-11-explore-and-learn-procedures.md)

Worktree: /Users/lei/Documents/work/dc/dc-terminal-explore (wherever the plan says /Users/lei/Documents/work/dc/dc-terminal, use this worktree path instead).

## Plan context (shared by all tasks)
## 先说清楚：Jeff 第一版怎么接（关键决定）

**选：dct 的 judge 用 HTTP 调 Jeff 自带的 `jeff-serve`（`POST /v1/systemone`，Jev 格式），不在 dct 进程里跑 ONNX，也不走 OpenAI 兼容 + logprobs。**

理由（按分量排）：
1. **Jeff 不是普通的「生成字」模型。** 它在 Qwen3.5-0.8B 上加了自己的输出层（`readout.safetensors` + `decision_config.json`），直接给每个选项的概率。llama.cpp、mlx_lm 那种 OpenAI 兼容服务器加载不了这个输出层，judge 设计里的「让模型只答一个字母、读 logprobs」对 Jeff 用不上。
2. **`jeff-serve` 已经存在，说的就是 judge 要回的 Jev 格式**（`experiments/fast-decision/vendor/jeff/src/jeff/server.py`：`state` / `questions` / `choice` 题 / 回 `probabilities` + `choice`）。dct 这边只是一个 HTTP 客户端，几乎不用翻译。judge 设计本来就是「后端可插拔」，这就是第一个后端。
3. **ONNX 不是「最小的那一步」**：Jeff 的自定义输出层 + Qwen3.5 的混合注意力能不能导出 ONNX、导出后准不准，都没验证过；dct 现在整棵依赖树没有 C，进程内 ONNX 要另引 onnxruntime。这些留到「正式给用户」那一版。

要处理的三个差别（都写进 Task D3 的代码和测试）：
- `jeff-serve` 的请求体必须带 `model`（用 `"jeff-latest"`），图片字段叫 `images` 不叫 `attachments`；第一版不发图（Jeff 的 MLX 后端本来就只收文字，`/health` 里 `modalities: ["text"]`）。
- **回复里没有 `answer_confidence`**，它的 `confidence` 公式也和 judge 设计不同。dct 自己从 `probabilities` 重算：`answer_confidence = probabilities[choice]`（先归一化），门槛只看这个数。
- 同时只接一个请求，忙时回 529 + `Retry-After: 1`：dct 串行发，529 等 1 秒重试一次，再忙就当「Jeff 失败」往上交。

**这一版不做、留到以后的：**
- 产品规则「模型不打包进程序；在 dcv 登记，用到才下载；正式版用 ONNX Runtime，不给用户装 Python」——这一版只在开发机和试点机上跑（用户本人的 Mac），`jeff-serve` 由人手动起；dct 不自动起它、不自动下载权重。
- judge 设计里的 OpenAI 兼容 + logprobs 后端（给能出 logprobs 的大模型用）、`score` / `noul` 题型的服务端实现、图片附件。
- Jeff 微调（只把纠正题存下来）。

没配 `[judge]` 或 Jeff 没起来：跳过 Jeff，直接大模型 → 用户（「每处模型用法都要有不靠模型的退路」）。

## 其他关键决定（计划里定的，执行时不再议）

1. **摸索时每一步都经 dco 的 `run_step` 执行，不用 `tap(element_id)`。** 轨迹里记的就是 `tap_by_intent: Post` 这种封闭词表里的一步，dco 用和重放时一模一样的找法（缓存 → 字 → 凑近看 → 求助）。好处：10-11 实测里「两个 Post」那种情况，摸索时就会触发 `needs_help`，dct 把 Jeff 选中的那个元素交回去，dco 记成「未确认」缓存，用户确认后重放直接用——摸索和重放不会走出两条路。`open_url`、`pick_file`、`type_param` 本来就只有流程里才有，单个工具做不了，`run_step` 正好补上。`run_step` 不收票，档位上限和单个工具一样（最多「发布」档）。dct 绝不自己拼一份 `executable: true` 的流程交给 `run_procedure`（那等于 agent 自己放行）。
2. **「最后那个发布按钮」用规则认，不靠模型**：平台表里的「发布词」（X 是 `Post`），并且这次所有要填的参数（文字、文件）都已经在画面上 → 这一下才算「最终发布」。参数没齐之前，发布词的按钮根本不进选项（X 左栏那个 `Post` 因此不会被误点；10-11「先打字后加图，字被清掉」也会被这条兜住：字没了，发布按钮就消失，Jeff 只能重新填）。
3. **每次从摸索点「最终发布」都先停下问用户**（不只每个平台第一次：修补也是在摸索新路）。用户点头才点。重放（`run_procedure` 跑确认过的流程）一律自动发，不停。
4. **文件先于文字**：有文件参数时，「填文字」选项要等所有文件都附上以后才出现（10-11 X 上 `type_param` 的 ⌘A+粘贴被加图打断的教训，规则化）。生成草稿时同一个参数只留最后一次 `type_param`。
5. **弹窗分两种**：点的按钮在 dcv 的 `popup_dismiss` 词表里（`Cancel`、`Not now`…，从 `dcv vocab --json` 取）且点完后有字消失 → 写成 dcv 弹窗规则草稿（`dcv add --kind popup`），不进步骤；其余（`Got it`、`Post now`）照实记成步骤，并在确认卡片上标「可能只偶尔出现」。
6. **大模型的输出只收成同一种形式**：从选项清单里挑一个 key（不受 20 个限制）+ 没字图标的名字，严格 JSON；读不懂 = 当它说卡住了。第一版大模型走 dct 现有的 `[llm]`（现在是 `claude -p`，就是后备）；qwen3.8-27b 的地址和能不能看图未知，那个任务挂起（Task D18）。
7. **草稿由代码按轨迹直接抄**，不是模型写的。dcv 拒收时才把拒收原因交给大模型改（最多 3 轮）；没配大模型就第 1 轮拒收后直接给用户看。dct 写 dcv 一律带 `--source import:agent`（dcv `add` 默认是 `manual`，不带就成了「你自己写的」）。
8. **平台表是 dct 里的数据**：编进程序的一份 `platforms.toml` 种子 + 用户的 `~/.dct/platforms.toml` 覆盖。没有任何「每个平台一份代码」。
9. **要发的文字一个活动批一次**：`dct campaign approve <名字>` 把各平台的文字一次给用户看，用户在终端里点头才存；`dct do --campaign <名字>` 只用批过的文字。
10. 重放第一版不带执行票：发帖类步骤是「发布」档，dco 不带票也放行（2026-10-04 用户定）。

## Global Constraints

- 每一步都先 Jeff，`answer_confidence` < 门槛（起始 **0.8**，配置 `[judge] threshold`）或照它选的做了画面没变 / 绕回了见过的画面 → 大模型；大模型说卡住、读不懂、照它选的做了还是不对、或同一画面已试 **2** 次 → 带编号图问用户。
- 每个回答记 `by: jeff | model | user`；大模型和用户纠正 Jeff 的题存进 `~/.dct/explore/answers.jsonl`（只存，不微调）。dco 的 `answer_run.by` 只有 `model | user`，Jeff 的回答发 `model`，dct 自己的记录里写 `jeff`。
- 第 1 步必须是 `open_url`，网址取自平台表，域名写死；模型不能决定去哪个网站。
- 登录页、验证码、二维码、手机验证、账号异常一类画面：**停**，交给用户；这些画面的字不发给 Jeff、不发给大模型。dco 回 `private_screen` 也一样停。
- 关注、订阅、Boost / Promote（付费推广）、平台表的禁点按钮一律不进选项；`dct_brain::tier::tier_for_label(label) >= Tier::Critical` 的也不进选项（dco 本来也会拒）。
- 摸索最多 **40** 步、最长 **10 分钟**；连续 **3** 步画面没变就停。
- 生成的流程名 `promote-<平台>-<种类>`（种类：有文件参数是 `video`，否则 `post`）；第 2 步自动插入登录闸 `verify_text`；完成标志不能含参数占位符，也不能含本次填进去的字。
- 草稿进 dcv 一律 `--source import:agent`、`--learned-from run:<编号>`；拒收最多改 **3** 轮；**agent 写的东西永远不给自己放行**：用户确认 → `dct procedure approve`（Touch ID）→ `dcv approve --sha`。
- 选择题选项 **2–20** 个（judge 设计；Jeff 自己能到 26，我们按 20）。
- dco 永远不调模型、不做判断、不存流程。
- 不写任何平台专用代码；平台差异只在平台表里。
- 写到磁盘的记录（轨迹、answers.jsonl、日志）里，参数值一律换回 `{参数名}`。
- 提交信息用英文。dco 和 dct 仓库不加任何 AI 署名行（dco 的 CLAUDE.md；dct 近来的提交也没有）；dcv 照它自己近来的提交习惯（带 `Co-Authored-By`）。
- cargo 在 `~/.cargo/bin`，不在默认 PATH：命令一律写 `~/.cargo/bin/cargo`。
- dct 仓库的工作区现在有别人没提交的改动（`src/daemon.rs`、`src/pty.rs`、`src/session.rs`、`src/ui/*`、`tests/concurrency.rs`）：执行前用 `git worktree add ../dc-terminal-explore -b explore-learn` 开一个干净的工作树，不碰那些文件。dco、dcv 同理各开一个分支。

## Review Focus

1. **同一个字的两个按钮**（X 左栏 `Post` 和发帖框 `Post`）：摸索时必须点 Jeff 选的那个，而且重放时不能点到另一个。→ Task D10 加测试 `two_post_buttons_are_answered_with_the_one_jeff_chose`；Task D14 加测试 `replay_help_on_duplicate_text_goes_through_the_ladder`。
2. **只偶尔出现的弹窗被记成固定步骤**（TikTok 的 `Got it`、`Continue to post?`）：下次没弹，重放卡在这一步。期望：拒绝类按钮变成弹窗规则草稿，不进步骤；其余的照实记，但确认卡片上标出来。→ Task D11 加测试 `a_dismiss_tap_becomes_a_popup_draft_not_a_step` 和 `a_non_dismiss_popup_step_is_flagged_for_the_user`。
3. **后一个动作把前面填的字清掉了**（X：先打字后加图）：发布前画面上必须看得到要发的字，否则发布按钮不出现。→ Task D7 加测试 `publish_is_hidden_until_every_text_param_shows`；Task D10 加测试 `text_wiped_by_attach_is_typed_again_before_publish`。
4. **半路冒出来的登录 / 验证码 / 账号异常**（不只在开头）：立即停，画面的字一个都不发给 Jeff 或大模型。→ Task D6 加测试 `captcha_mid_run_is_a_stop`；Task D10 加测试 `a_captcha_in_the_middle_stops_without_asking_any_model`。
5. **开着好几个 Chrome 窗口 / 窗口半路被关**：只按 `window_id` 认窗口；不止一个候选时要用户指定，不猜。→ Task D16 加测试 `several_browser_windows_need_an_explicit_window`；Task D10 加测试 `a_closed_window_stops_the_run`。

---

# 三、dct（dc-terminal）

> 以下所有命令在 dct 的干净工作树里跑（见 Global Constraints）。下文写 `/Users/lei/Documents/work/dc/dc-terminal`，执行者换成自己的工作树路径即可。
> 全量检查命令（每个任务最后都跑）：
> `cd /Users/lei/Documents/work/dc/dc-terminal && ~/.cargo/bin/cargo test --workspace -- --test-threads=1 && ~/.cargo/bin/cargo fmt --check && ~/.cargo/bin/cargo clippy --all-targets`


## Your task
### Task D8: 逐级往上（Jeff → 大模型 → 用户）

**目标：** 一道题按用户定的顺序问；谁答的都记下来；Jeff 的门槛、大模型的 2 次上限、读不懂当卡住，全在这里。

**Files:**
- Create: `crates/dct-explore/src/ladder.rs`
- Test: 同文件 `mod tests`

**Interfaces:**
- Consumes: `options::{Opt, Act}`、`JevChoice`
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Who { Jeff, Model, User }

pub struct JeffAnswer { pub key: String, pub probabilities: Vec<(String, f64)>, pub answer_confidence: f64 }
pub trait Jeff { fn ask(&self, state: &str, q: &crate::JevChoice) -> Option<JeffAnswer>; } // None = 没配 / 连不上 / 忙
pub struct ModelPick { pub key: String, pub name: Option<String> }
pub trait Model { fn pick(&self, state: &str, all: &[Opt], picture_png: Option<&str>) -> Option<ModelPick>; } // None = 卡住 / 读不懂 / 没配
pub struct UserPick { pub key: String, pub name: Option<String> }
pub trait User { fn pick(&mut self, state: &str, all: &[Opt], picture_png: Option<&str>, why: &str) -> Option<UserPick>; } // None = 用户说停

pub struct NoModel; impl Model for NoModel { fn pick(&self, _: &str, _: &[Opt], _: Option<&str>) -> Option<ModelPick> { None } }

#[derive(Debug, Clone)]
pub struct Decision { pub key: String, pub name: Option<String>, pub by: Who, pub jeff: Option<(String, f64)> }

pub struct Ladder<'a> { pub jeff: &'a dyn Jeff, pub model: &'a dyn Model, pub user: &'a mut dyn User, pub threshold: f64 }

/// 同一画面（scene_hash）上的失败次数，由调用方保管：执行后画面没变 / 绕回 → 调 fail(by)。
#[derive(Default)]
pub struct Tries { pub jeff_failed: bool, pub model_tries: u32 }

impl<'a> Ladder<'a> {
    /// 返回 None = 用户说停。
    pub fn decide(&mut self, state: &str, short: &[Opt], all: &[Opt], picture: Option<&str>, tries: &Tries) -> Option<Decision>;
}
```

规则：
1. `!tries.jeff_failed` 且 Jeff 答了且 `answer_confidence >= threshold` 且选的不是 `stuck` 且选的不是 `TapIcon{name: None}`（Jeff 起不了名字）→ `by: Jeff`。
2. 否则 `tries.model_tries < 2` 且大模型给了 key 且 key 在 `all` 里且不是 `stuck`，`TapIcon` 必须带 name → `by: Model`。
3. 否则问用户；`why` 说明为什么到了用户这里（「Jeff 只有 0.42 的把握，大模型说卡住了」）；`TapIcon` 没名字时 `User` 实现要追问名字（见 D16 的终端实现）。
4. `Decision.jeff` 永远带 Jeff 当时的答案（有的话），用于训练题。

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{Act, Opt};
    use std::cell::RefCell;

    struct J(Option<(&'static str, f64)>);
    impl Jeff for J { fn ask(&self, _: &str, _: &crate::JevChoice) -> Option<JeffAnswer> { self.0.map(|(k, c)| JeffAnswer { key: k.into(), probabilities: vec![(k.into(), c)], answer_confidence: c }) } }
    struct M(Option<(&'static str, Option<&'static str>)>, RefCell<u32>);
    impl Model for M { fn pick(&self, _: &str, _: &[Opt], _: Option<&str>) -> Option<ModelPick> { *self.1.borrow_mut() += 1; self.0.map(|(k, n)| ModelPick { key: k.into(), name: n.map(String::from) }) } }
    struct U(Option<(&'static str, Option<&'static str>)>, Vec<String>);
    impl User for U { fn pick(&mut self, _: &str, _: &[Opt], _: Option<&str>, why: &str) -> Option<UserPick> { self.1.push(why.into()); self.0.map(|(k, n)| UserPick { key: k.into(), name: n.map(String::from) }) } }

    fn opts() -> Vec<Opt> { vec![
        Opt { key: "e3".into(), text: "按钮 Post".into(), act: Act::Stuck, publish: true },
        Opt { key: "e5".into(), text: "没有字的图标".into(), act: Act::TapIcon { el: crate::hands::El { id: "e5".into(), text: "".into(), role: "image".into(), frac: crate::hands::Frac { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }, source: "detector".into() }, name: None }, publish: false },
        Opt { key: "stuck".into(), text: "卡住了".into(), act: Act::Stuck, publish: false },
    ] }

    #[test]
    fn a_confident_jeff_answers_alone() {
        let (j, m, mut u) = (J(Some(("e3", 0.93))), M(Some(("e5", Some("add media"))), RefCell::new(0)), U(None, vec![]));
        let d = Ladder { jeff: &j, model: &m, user: &mut u, threshold: 0.8 }.decide("s", &opts(), &opts(), None, &Tries::default()).unwrap();
        assert_eq!((d.key.as_str(), d.by), ("e3", Who::Jeff));
        assert_eq!(*m.1.borrow(), 0, "Jeff 有把握就不叫大模型");
    }

    #[test]
    fn low_confidence_goes_to_the_model_and_keeps_jeffs_answer_for_training() {
        let (j, m, mut u) = (J(Some(("stuck", 0.42))), M(Some(("e3", None)), RefCell::new(0)), U(None, vec![]));
        let d = Ladder { jeff: &j, model: &m, user: &mut u, threshold: 0.8 }.decide("s", &opts(), &opts(), None, &Tries::default()).unwrap();
        assert_eq!((d.key.as_str(), d.by), ("e3", Who::Model));
        assert_eq!(d.jeff, Some(("stuck".into(), 0.42)));
    }

    #[test]
    fn jeff_cannot_name_an_icon_so_a_confident_icon_pick_still_goes_up() {
        let (j, m, mut u) = (J(Some(("e5", 0.95))), M(Some(("e5", Some("add media"))), RefCell::new(0)), U(None, vec![]));
        let d = Ladder { jeff: &j, model: &m, user: &mut u, threshold: 0.8 }.decide("s", &opts(), &opts(), None, &Tries::default()).unwrap();
        assert_eq!((d.by, d.name.as_deref()), (Who::Model, Some("add media")));
    }

    #[test]
    fn after_a_failed_jeff_pick_jeff_is_skipped_on_the_same_screen() {
        let (j, m, mut u) = (J(Some(("e3", 0.99))), M(Some(("e3", None)), RefCell::new(0)), U(None, vec![]));
        let tries = Tries { jeff_failed: true, model_tries: 0 };
        let d = Ladder { jeff: &j, model: &m, user: &mut u, threshold: 0.8 }.decide("s", &opts(), &opts(), None, &tries).unwrap();
        assert_eq!(d.by, Who::Model);
    }

    #[test]
    fn model_stuck_malformed_or_two_tries_goes_to_the_user() {
        for (model, tries) in [(None, 0), (Some(("stuck", None)), 0), (Some(("e3", None)), 2), (Some(("nope", None)), 0), (Some(("e5", None)), 0)] {
            let (j, m, mut u) = (J(None), M(model, RefCell::new(0)), U(Some(("e3", None)), vec![]));
            let d = Ladder { jeff: &j, model: &m, user: &mut u, threshold: 0.8 }.decide("s", &opts(), &opts(), None, &Tries { jeff_failed: false, model_tries: tries }).unwrap();
            assert_eq!(d.by, Who::User, "{model:?} {tries}");
            assert!(!u.1[0].is_empty(), "要告诉用户为什么问他");
        }
    }

    #[test]
    fn the_user_saying_stop_ends_it() {
        let (j, m, mut u) = (J(None), M(None, RefCell::new(0)), U(None, vec![]));
        assert!(Ladder { jeff: &j, model: &m, user: &mut u, threshold: 0.8 }.decide("s", &opts(), &opts(), None, &Tries::default()).is_none());
    }
}
```

- [ ] **Step 2: 跑测试，确认失败** → FAIL。
- [ ] **Step 3: 实现**（按规则 1–4；`why` 用中文一句话拼：「Jeff 没有把握（0.42）」/「Jeff 选的那一下没起作用」/「大模型说卡住了」/「大模型已经试了 2 次」/「这个图标没有名字」）。
- [ ] **Step 4: 跑全量检查** → PASS。
- [ ] **Step 5: 提交** — `git commit -m "dct-explore: the ladder (Jeff above threshold, else model up to 2 tries, else the user), who answered recorded"`

**完成标准：** 用户定的升级规则逐条有测试。

---

