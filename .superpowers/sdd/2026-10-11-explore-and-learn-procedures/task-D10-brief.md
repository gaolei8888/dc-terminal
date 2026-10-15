# Brief for Task D10 (extracted from /Users/lei/Documents/work/dc/dc-octo/docs/superpowers/plans/2026-10-11-explore-and-learn-procedures.md)

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
### Task D10: 摸索循环

**目标：** 把 D5–D9 接起来：第 1 步固定 `open_url`；每轮看 → 规矩 → 选项 → 逐级往上 → 经 `run_step` 执行 → 核对画面有没有变 → 失败就往上交；第一次要点「最终发布」先问用户；到上限就停。全部对着 `FakeHands` 测。

**Files:**
- Create: `crates/dct-explore/src/explore.rs`
- Test: `crates/dct-explore/tests/explore_loop.rs`

**Interfaces:**
- Consumes: 前面所有任务的类型
- Produces:

```rust
pub struct ExploreInput<'a> {
    pub platform: &'a Platform,
    pub goal_text: &'a str,
    pub params: Params,                 // 本次的值（只在内存里）
    pub text_params: Vec<String>,
    pub file_params: Vec<String>,
    pub window_id: u64,
    pub popups: Vec<serde_json::Value>, // dcv popups --app <platform> --json 里已生效的
    pub budget: Budget,
    pub run_id: String,
    pub data_dir: std::path::PathBuf,   // ~/.dct/explore
}
pub trait Confirm { fn publish_ok(&mut self, picture_png: Option<&str>, summary: &str) -> bool; }
pub trait Clock { fn now_ms(&self) -> u64; }

#[derive(Debug)]
pub enum Outcome {
    Published(Trace),                      // 成功：发出去了（或流程走完）
    DeclinedAtPublish(Trace),              // 停在发布前，用户说不发
    Halted { trace: Trace, why: Halt },    // 登录 / 验证码 / 警告
    Stopped { trace: Trace, why: String }, // 上限、用户说停、dco blocked / private_screen / 窗口没了
}
pub fn explore(h: &mut dyn Hands, lad: &mut Ladder, confirm: &mut dyn Confirm, clock: &dyn Clock, input: ExploreInput) -> Outcome;
/// 修补用：从当前画面接着摸索，已有的步骤作为轨迹开头（不再 open_url）。
pub fn explore_from(h: &mut dyn Hands, lad: &mut Ladder, confirm: &mut dyn Confirm, clock: &dyn Clock, input: ExploreInput, prefix: Vec<TraceStep>) -> Outcome;
```

循环（写进代码注释）：
1. （`explore`）`run_step open_url <platform.entry_url>`；不是 `done` → `Stopped`。
2. `look`（带 scene_hash）。看完先 `redact` 元素文字里的参数值（再交给任何人）。`gate::screen_halt` → `Halted`（**此时不调 Jeff、不调大模型、不问用户选项**，只把命中的那行字交给上层说给用户）。第一次看到的画面记进 `trace.first_lines`。
3. `Meter::over` → `Stopped`。
4. 选项（`all_options` / `shortlist`）→ `Ladder::decide`（`picture`：只在要问大模型或用户时才 `look(marks: true)` 拿编号图）→ 用户说停 → `Stopped`。
5. `Done` → 已发布则 `Published`；`Stuck`（只有用户可能选到）→ `Stopped`。
6. 选中的是 `publish` 且这次还没问过 → `confirm.publish_ok(编号图, 「要发到 X 了，内容已经填好，看一眼？」)`；不 → `DeclinedAtPublish`。
7. `steps_for` → 逐步 `run_step`：
   - `needs_help`：在 `help.elements` 里找和选中元素**同字、中心点距离 < 0.03（窗口比例）**的那一个；正好一个 → `answer_run {element_id, by: Model, name(图标时)}`（Jeff 和大模型都发 `model`，用户选的发 `user`）；找不到或不止一个 → 把 `help` 的编号图交给 `User` 选。
   - `blocked` / `failed` 带 `private_screen` → `Stopped`（档位拦的要把 dco 的 note 原话转给用户）。
   - `failed`：这一下算「没起作用」。
8. 执行后 `look`：scene_hash 和执行前一样、或等于这次运行里 2 步以前见过的某个 → 算失败：`Tries` 记一笔（是 Jeff 选的就 `jeff_failed = true`，是大模型选的 `model_tries += 1`），回到 4（同一画面）。变了 → `Meter::record(true)`，`Tries` 清零，`TraceStep` 记下（`vanished` = 前一画面有、这一画面没有的行），`Progress` 更新（`Fill` 成功 → `filled`；`Attach` 成功 → `attached`；`publish` 成功 → `published`），每道题写 `append_question`。
9. `Fill` 成功但下一轮看画面时这个参数的字不见了（`values_on_screen` 为假）→ 从 `filled` 里拿掉（字被清掉了，发布按钮会消失，会再填一次）。
10. 窗口没了（`look` 回 `stale` / `not_found`）→ `Stopped`。

`values_on_screen(param)`：取参数值第一行，fold 后按空白切词，≥ 80% 的词出现在本画面所有元素字的 fold 拼接里（词少于 3 个时要求全部出现）。

- [ ] **Step 1: 写失败的测试**（`tests/explore_loop.rs`，每个测试一个 `FakeHands` 脚本；下面给出公共工具和 9 个测试）

```rust
use dct_explore::explore::{explore, Confirm, Clock, ExploreInput, Outcome};
use dct_explore::gate::Budget;
use dct_explore::hands::*;
use dct_explore::ladder::*;
use dct_explore::platform::Table;
use dct_explore::testing::FakeHands;

struct NoJeff; impl Jeff for NoJeff { fn ask(&self, _: &str, _: &dct_explore::JevChoice) -> Option<JeffAnswer> { None } }
struct SureJeff(Vec<&'static str>, std::cell::RefCell<usize>);
impl Jeff for SureJeff { fn ask(&self, _: &str, _: &dct_explore::JevChoice) -> Option<JeffAnswer> { let i = *self.1.borrow(); *self.1.borrow_mut() += 1; self.0.get(i).map(|k| JeffAnswer { key: k.to_string(), probabilities: vec![(k.to_string(), 0.95)], answer_confidence: 0.95 }) } }
struct ScriptUser(Vec<(&'static str, Option<&'static str>)>, usize);
impl User for ScriptUser { fn pick(&mut self, _: &str, _: &[dct_explore::options::Opt], _: Option<&str>, _: &str) -> Option<UserPick> { let r = self.0.get(self.1).map(|(k, n)| UserPick { key: k.to_string(), name: n.map(String::from) }); self.1 += 1; r } }
struct Yes(u32); impl Confirm for Yes { fn publish_ok(&mut self, _: Option<&str>, _: &str) -> bool { self.0 += 1; true } }
struct No; impl Confirm for No { fn publish_ok(&mut self, _: Option<&str>, _: &str) -> bool { false } }
struct T0; impl Clock for T0 { fn now_ms(&self) -> u64 { 0 } }

fn el(id: &str, t: &str, role: &str, x: f64, y: f64) -> El { El { id: id.into(), text: t.into(), role: role.into(), frac: Frac { x, y, w: 0.05, h: 0.03 }, source: "ocr".into() } }
fn look(hash: &str, els: Vec<El>) -> Look { Look { snapshot_id: format!("s-{hash}"), observation_id: Some(format!("obs-{hash}")), scene_hash: Some(hash.into()), elements: els, marks_png: None } }
fn done(id: &str) -> RunReply { RunReply { run_id: id.into(), status: "done".into(), at_step: 1, note: None, help: None, learned_keys: vec![], raw: serde_json::json!({}) } }
fn input(t: &Table, d: &std::path::Path) -> ExploreInput<'static> {
    let p: &'static dct_explore::platform::Platform = Box::leak(Box::new(t.get("x").unwrap().clone()));
    ExploreInput {
        platform: p,
        goal_text: "把这段文字发到 X",
        params: [("post_text".to_string(), "hello from dct test".to_string())].into_iter().collect(),
        text_params: vec!["post_text".into()],
        file_params: vec![],
        window_id: 4100,
        popups: vec![],
        budget: Budget { max_steps: 40, max_ms: 600_000, max_unchanged: 3 },
        run_id: "x-20261011-0001".into(),
        data_dir: d.to_path_buf(),
    }
}
```

测试清单（每个都写全脚本）：

1. `opens_the_fixed_entry_url_first_and_never_asks_a_model_where_to_go`：第一个调用是 `run_step` 且 `step.arg == "https://x.com/compose/post"`（`FakeHands.calls[0]`）。
2. `two_post_buttons_are_answered_with_the_one_jeff_chose`（Review Focus 1）：画面有 `e1 Post (0.08,0.5)` 和 `e3 Post (0.8,0.35)`，字已在画面；Jeff 选 `e3`；`run_step tap_by_intent: Post` 回 `needs_help`，`help.elements` 里两个 Post（id `e7`、`e9`，`e9` 在 0.8,0.35）→ 断言 `answer_run` 发的是 `e9`、`by: model`。
3. `the_first_publish_asks_the_user_and_declining_stops_before_tapping`：`No` → `Outcome::DeclinedAtPublish`，且没有任何 `run_step tap_by_intent: Post` 调用。
4. `text_wiped_by_attach_is_typed_again_before_publish`（Review Focus 3）：带 `file_params: ["image"]`；脚本：Attach 成功 → Fill 成功（画面出现字）→ 下一轮画面字没了 → 断言再出一次 `type_param: post_text`，之后才有发布。
5. `a_captcha_in_the_middle_stops_without_asking_any_model`（Review Focus 4）：第 3 轮画面出现「Verify you are human」→ `Outcome::Halted`；`SureJeff` 的计数器停在第 2 题，`ScriptUser` 没被调用。
6. `an_unchanged_screen_counts_as_a_failed_jeff_pick_and_goes_up`：Jeff 选 `e5`，执行后 scene_hash 不变 → 同一画面第二次问时 `ScriptUser` 被调用（没配大模型）。
7. `three_unchanged_screens_in_a_row_stop`：每个动作后 hash 都不变（用户一直选 `scroll_down`）→ `Stopped`，why 里有「画面」。
8. `an_icon_answer_carries_its_name_to_dco`：用户选 `e5`（没字图标）并给名字 `add media` → `run_step tap_by_intent: add media` → `needs_help` → `answer_run` 带 `name: "add media"`、`by: user`；轨迹里这一步 `arg == "add media"`。
9. `a_closed_window_stops_the_run`（Review Focus 5）：`look` 回 `DcoError{code:"stale"}` → `Stopped`。
10. `training_rows_are_written_with_who_answered`：跑完 `data_dir/answers.jsonl` 每行都有 `by`，用户纠正 Jeff 的那行 `corrects_jeff: true`，全文不含 `hello from dct test`。

- [ ] **Step 2: 跑测试，确认失败** — `~/.cargo/bin/cargo test -p dct-explore --test explore_loop` → FAIL。
- [ ] **Step 3: 实现**（按上面 1–10 的循环写；每个分支一个小函数，`explore.rs` 控制在 400 行以内，超了把「执行一个选项」拆到 `explore/act.rs`）。
- [ ] **Step 4: 跑全量检查** → PASS。
- [ ] **Step 5: 提交** — `git commit -m "dct-explore: the exploration loop over run_step (fixed entry, stop rules, ladder, publish confirmation, limits)"`

**完成标准：** 10 个测试全过，Review Focus 1、3、4、5 各有一条落在这里。

---

