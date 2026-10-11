# Brief for Task D14 (extracted from /Users/lei/Documents/work/dc/dc-octo/docs/superpowers/plans/2026-10-11-explore-and-learn-procedures.md)

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
### Task D14: 重放 + 卡住时答题 + 走不通时补一段

**目标：** 有正式流程就 `run_procedure` 重放；`needs_help` 时用同一个逐级往上答题（选项 = 编号图里的元素）；流程本身走不通时从卡住那步起摸索，出一份 `draft_of` 原流程的新草稿。

**Files:**
- Create: `crates/dct-explore/src/replay.rs`
- Test: `crates/dct-explore/tests/replay.rs`

**Interfaces:**
- Produces:

```rust
pub struct ReplayInput<'a> { pub procedure: &'a serde_json::Value /* dcv show --json，executable 必须为 true */, pub params: Params, pub popups: Vec<serde_json::Value>, pub window_id: u64, pub preview: bool /* true → stop_before_step = 发布那一步 */ }
pub enum ReplayOutcome {
    Done { learned: Vec<String> },
    Previewed { at_step: u32 },
    Blocked { note: String },               // 档位拦下，转给用户
    NeedsRepair { at_step: u32, prefix: Vec<TraceStep> }, // 交给 explore_from
    Stopped(String),
}
pub fn replay(h: &mut dyn Hands, lad: &mut Ladder, input: ReplayInput) -> ReplayOutcome;
/// 发布那一步：最后一个 tap_by_intent 且 arg 在平台发布词里；找不到 → None（preview 时报错）。
pub fn publish_step(procedure: &serde_json::Value, p: &Platform) -> Option<u32>;
```

规则：
- `executable != true` → `Stopped("这条流程还没确认，不能重放")`（dct 不自己改这个字段）。
- `needs_help`：选项 = `help.elements` 各一项（key = 元素 id，text 同 D7 的写法）+ `not_there` + `stuck`；`state` = 流程名 + 第几步 + `help.intent` + `help.question` + 元素字（脱敏）。`Ladder::decide` → `answer_run`（Jeff/大模型 → `by: model`；用户 → `by: user`；`not_there` → `HelpAnswer::NotThere`）。同一个 run 最多答 5 次，超了 `Stopped`。
- `failed` 且 note 不是档位 / 急停 / 超时 / 锁屏 / 私人画面一类（按 dco 原话里的固定词判断，见 dco `run/stop.rs` 和 HANDOFF）→ `NeedsRepair{at_step, prefix}`：`prefix` = 流程第 1..at_step-1 步，每步一个 `TraceStep`，`from_procedure: true`（D9 定义的字段）、`by: Who::User`（用户确认过的流程）、`ok: true`。
- `answer_run` 返回 `NotThere` 后 dco 报 `failed` → 同样 `NeedsRepair`。

- [ ] **Step 1: 写失败的测试**（`tests/replay.rs`）

1. `replay_runs_the_approved_procedure_with_popups_and_params`：`run_procedure` 参数里有 `params.post_text`、`popups`，没有 `stop_before_step`；回 `done` → `Done`。
2. `preview_stops_before_the_publish_step`：X 的 5 步流程，`preview: true` → `run_procedure` 带 `stop_before_step: 5`；回 `stopped` → `Previewed{at_step: 5}`。
3. `replay_help_on_duplicate_text_goes_through_the_ladder`（Review Focus 1）：`needs_help` 带两个 Post；Jeff 0.9 选 `e9` → `answer_run e9 by model`。
4. `low_confidence_help_goes_to_the_user_and_is_sent_as_user`：Jeff 0.3、没大模型 → 用户选 `e7` → `by: user`。
5. `a_missing_button_turns_into_a_repair_from_that_step`：第 4 步 `failed`（note「找不到「Post」」）→ `NeedsRepair{at_step: 4}`，`prefix.len() == 3`，都 `from_procedure`。
6. `a_draft_is_never_replayed`：`executable: false` → `Stopped`，`FakeHands.calls` 为空。
7. `a_tier_block_is_reported_not_repaired`：`blocked` + `needs_authorization` → `Blocked`。

- [ ] **Step 2–4:** 跑 → FAIL → 实现 → 全量检查 PASS。
- [ ] **Step 5: 提交** — `git commit -m "dct-explore: replay with ladder answers on needs_help, preview via stop_before_step, repair hand-off"`

**完成标准：** 7 个测试全过；重放永远不碰草稿。

---

